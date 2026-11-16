//! axum による HTTP サーバ。PWA 配信・REST・SSE を担う。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

use crate::adapter::devin_cloud::{self, DevinCloud};
use crate::adapter::devin_local::{self, DevinLocal};
use crate::notify;
use crate::store::Store;
use crate::{Error, Result, answer};

const INDEX_HTML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/static/index.html"));
const APP_JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/static/app.js"));

pub struct AppState {
    pub store: Arc<Store>,
    /// 起動したソースに応じて Some。cloud 未使用なら None。
    pub devin: Option<Arc<DevinCloud>>,
    pub local: Option<Arc<DevinLocal>>,
    pub token: Option<String>,
    pub tx: broadcast::Sender<String>,
    pub notify: Option<notify::Target>,
}

pub async fn serve(bind: &str, state: Arc<AppState>) -> Result<()> {
    let api = Router::new()
        .route("/state", get(state_handler))
        .route("/decisions/{id}/answer", post(answer_handler))
        .route("/poll", post(poll_handler))
        .route("/events", get(events_handler))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    let app = Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route("/app.js", get(app_js))
        .nest("/api", api)
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!("[andon] listening on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Bearer ヘッダまたは `?token=` クエリを照合する。トークン未設定なら全通過。
async fn auth(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(expected) = &state.token else {
        return next.run(request).await;
    };
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let query_token = uri
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
    if bearer == Some(expected.as_str()) || query_token == Some(expected.as_str()) {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

async fn app_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript")], APP_JS)
}

async fn state_handler(
    State(st): State<Arc<AppState>>,
) -> std::result::Result<Json<Value>, ApiError> {
    let store = st.store.clone();
    let (sessions, decisions) = tokio::task::spawn_blocking(move || {
        Ok::<_, Error>((store.sessions()?, store.decisions()?))
    })
    .await
    .map_err(|e| ApiError(Error::Io(std::io::Error::other(e.to_string()))))??;
    Ok(Json(json!({"sessions": sessions, "decisions": decisions})))
}

#[derive(Deserialize)]
struct AnswerReq {
    option_id: Option<String>,
    text: Option<String>,
}

async fn answer_handler(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<AnswerReq>,
) -> std::result::Result<Json<Value>, ApiError> {
    let store = st.store.clone();
    let devin = st.devin.clone();
    let local = st.local.clone();
    let message = tokio::task::spawn_blocking(move || {
        answer::submit(
            &store,
            devin.as_deref(),
            local.as_deref(),
            id,
            req.option_id.as_deref(),
            req.text.as_deref(),
        )
    })
    .await
    .map_err(|e| ApiError(Error::Io(std::io::Error::other(e.to_string()))))??;
    let _ = st.tx.send("update".to_owned());
    Ok(Json(json!({"message": message})))
}

/// 手動ポーリング(検証・デバッグ用)。起動中の全ソースを 1 回ずつ回す。
async fn poll_handler(
    State(st): State<Arc<AppState>>,
) -> std::result::Result<StatusCode, ApiError> {
    let store = st.store.clone();
    let devin = st.devin.clone();
    let local = st.local.clone();
    let tx = st.tx.clone();
    let notify = st.notify.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(d) = &devin {
            devin_cloud::poll_once(d, &store, &tx, notify.as_ref())?;
        }
        if let Some(l) = &local {
            devin_local::poll_once(l, &store, &tx, notify.as_ref())?;
        }
        Ok::<(), Error>(())
    })
    .await
    .map_err(|e| ApiError(Error::Io(std::io::Error::other(e.to_string()))))??;
    Ok(StatusCode::NO_CONTENT)
}

async fn events_handler(
    State(st): State<Arc<AppState>>,
) -> Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(st.tx.subscribe())
        .filter_map(|r| r.ok().map(|s| Ok(Event::default().data(s))));
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

/// ドメインエラーを HTTP レスポンスに変換する。
struct ApiError(Error);

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::AlreadyAnswered(_) => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error": self.0.to_string()}))).into_response()
    }
}
