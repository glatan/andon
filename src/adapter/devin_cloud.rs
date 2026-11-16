//! Devin Cloud API v3 との境界。
//! セッション一覧の検出・メッセージ取得・回答送信・セッション作成を担う。

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::config::Config;
use crate::store::{NewDecision, Store, now};
use crate::types::{DecisionKind, Session};
use crate::{Error, Result, card, notify};

/// レスポンスを Value に正規化する。2xx 以外は Error::Api。
fn finish(
    res: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<Value> {
    let mut res = res?;
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(Error::Api {
            status,
            body: text.chars().take(500).collect(),
        });
    }
    Ok(if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    })
}

/// andon が管理対象とするセッションに付けるタグ。
pub const MANAGED_TAG: &str = "andon";

/// knowledge note の検索名(`andon setup` で作成)。
pub const NOTE_NAME: &str = "andon-card-format";

pub struct DevinCloud {
    base: String,
    org: String,
    key: String,
    agent: ureq::Agent,
}

#[derive(Debug, Deserialize)]
struct ApiSession {
    session_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    status_detail: Option<String>,
    #[serde(default)]
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct Page<T> {
    items: Vec<T>,
    #[serde(default)]
    end_cursor: Option<String>,
    #[serde(default)]
    has_next_page: bool,
}

#[derive(Debug, Deserialize)]
struct ApiMessage {
    event_id: String,
    source: String,
    message: String,
}

impl DevinCloud {
    /// `DEVIN_API_KEY`/`DEVIN_ORG_ID` が未設定なら Error::Config。
    pub fn new(cfg: &Config) -> Result<Self> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Ok(Self {
            base: cfg.api_base.trim_end_matches('/').to_owned(),
            org: cfg.org_id.clone().ok_or(Error::Config("DEVIN_ORG_ID"))?,
            key: cfg.api_key.clone().ok_or(Error::Config("DEVIN_API_KEY"))?,
            agent,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v3/organizations/{}/{}", self.base, self.org, path)
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let mut req = self
            .agent
            .get(&self.url(path))
            .header("Authorization", format!("Bearer {}", self.key));
        for (k, v) in query {
            req = req.query(k, v);
        }
        finish(req.call())
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value> {
        finish(
            self.agent
                .post(&self.url(path))
                .header("Authorization", format!("Bearer {}", self.key))
                .send_json(body),
        )
    }

    /// `GET /v3/self` — 認証疎通確認(org スコープ外のパスなので個別に組み立て)。
    pub fn self_check(&self) -> Result<Value> {
        let url = format!("{}/v3/self", self.base);
        let mut res = self
            .agent
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.key))
            .call()?;
        let status = res.status().as_u16();
        let text = res.body_mut().read_to_string().unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(Error::Api {
                status,
                body: text.chars().take(500).collect(),
            });
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// 管理対象セッション一覧。`qs` 問題(spec 上 required 扱い)に備え、
    /// flattened パラメータが 422 なら JSON 文字列パラメータで再試行する。
    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        let mut use_qs = false;
        loop {
            let page: Page<ApiSession> = if use_qs {
                let mut qs = json!({"first": 200, "is_archived": false, "tags": [MANAGED_TAG]});
                if let Some(c) = &cursor {
                    qs["after"] = json!(c);
                }
                let v = self.get("sessions", &[("qs", &qs.to_string())])?;
                serde_json::from_value(v)?
            } else {
                let mut q: Vec<(&str, &str)> = vec![
                    ("first", "200"),
                    ("is_archived", "false"),
                    ("tags", MANAGED_TAG),
                ];
                if let Some(c) = &cursor {
                    q.push(("after", c));
                }
                match self.get("sessions", &q) {
                    Ok(v) => serde_json::from_value(v)?,
                    Err(Error::Api { status: 422, .. }) => {
                        use_qs = true;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            };
            out.extend(page.items.into_iter().map(|s| Session {
                session_id: s.session_id,
                title: s.title,
                url: s.url,
                status: s.status,
                status_detail: s.status_detail,
                updated_at: s.updated_at,
            }));
            if !page.has_next_page {
                break;
            }
            cursor = page.end_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// 最新の Devin 発言を (event_id, message) で返す。
    pub fn last_devin_message(&self, session_id: &str) -> Result<Option<(String, String)>> {
        let v = self.get(
            &format!("sessions/{session_id}/messages"),
            &[("first", "200")],
        )?;
        let page: Page<ApiMessage> = serde_json::from_value(v)?;
        Ok(page
            .items
            .into_iter()
            .rfind(|m| m.source == "devin")
            .map(|m| (m.event_id, m.message)))
    }

    pub fn send_message(&self, session_id: &str, message: &str) -> Result<()> {
        self.post(
            &format!("sessions/{session_id}/messages"),
            &json!({"message": message}),
        )?;
        Ok(())
    }

    /// セッション作成。管理タグ・knowledge note・bypass_approval を付ける。
    pub fn create_session(
        &self,
        prompt: &str,
        knowledge_note_id: Option<&str>,
        bypass_approval: bool,
    ) -> Result<(String, String)> {
        let mut body = json!({
            "prompt": prompt,
            "tags": [MANAGED_TAG],
            "bypass_approval": bypass_approval,
        });
        if let Some(id) = knowledge_note_id {
            body["knowledge_ids"] = json!([id]);
        }
        let v: Value = self.post("sessions", &body)?;
        let s: ApiSession = serde_json::from_value(v)?;
        Ok((s.session_id, s.url))
    }

    /// ANDON_JSON 規約の knowledge note を作成し、note_id を返す。
    pub fn provision_note(&self) -> Result<String> {
        let v = self.post(
            "knowledge/notes",
            &json!({
                "name": NOTE_NAME,
                "trigger": "人間への確認・判断・選択を仰ぐ必要があるとき",
                "body": card::INSTRUCTION,
            }),
        )?;
        Ok(v["note_id"].as_str().unwrap_or_default().to_owned())
    }
}

/// 1 回分のポーリング。検出→カード作成→通知をまわす。
pub fn poll_once(
    devin: &DevinCloud,
    store: &Store,
    tx: &broadcast::Sender<String>,
    notify_cfg: Option<&notify::Target>,
) -> Result<()> {
    let sessions = devin.list_sessions()?;
    for s in &sessions {
        store.upsert_session(s)?;
        match s.status_detail.as_deref() {
            Some("waiting_for_user") => card_for_user_wait(devin, store, s, notify_cfg)?,
            Some("waiting_for_approval") => card_for_approval(store, s, notify_cfg)?,
            _ => store.expire_pending_for(&s.session_id)?,
        }
    }
    let _ = tx.send("update".to_owned());
    Ok(())
}

/// ポーリングループ。serve から専用スレッドで回す。
pub fn run_poller(
    devin: Arc<DevinCloud>,
    store: Arc<Store>,
    tx: broadcast::Sender<String>,
    interval_secs: u64,
    notify_cfg: Option<notify::Target>,
) {
    let mut backoff = Duration::from_secs(interval_secs.max(2));
    let interval = Duration::from_secs(interval_secs.max(2));
    loop {
        match poll_once(&devin, &store, &tx, notify_cfg.as_ref()) {
            Ok(()) => backoff = interval,
            Err(e) => {
                eprintln!("[andon] ポーリング失敗: {e}");
                backoff = (backoff * 2).min(Duration::from_secs(120));
            }
        }
        std::thread::sleep(backoff);
    }
}

fn card_for_user_wait(
    devin: &DevinCloud,
    store: &Store,
    s: &Session,
    notify_cfg: Option<&notify::Target>,
) -> Result<()> {
    let Some((event_id, message)) = devin.last_devin_message(&s.session_id)? else {
        return Ok(());
    };
    if store.has_pending_decision(&s.session_id, &event_id)? {
        return Ok(());
    }
    let (body, payload) = card::body_excerpt(&message);
    let card = card::parse(&message);
    let title = card
        .as_ref()
        .and_then(|c| c.title.clone())
        .or_else(|| s.title.clone())
        .unwrap_or_else(|| "判断待ち".to_owned());
    let options = card
        .as_ref()
        .filter(|c| !c.options.is_empty())
        .map(|c| c.options.clone())
        .unwrap_or_else(card::fallback_options);
    store.insert_decision(
        &NewDecision {
            session_id: s.session_id.clone(),
            session_title: s.title.clone(),
            session_url: s.url.clone(),
            kind: DecisionKind::Choice,
            title,
            body,
            summary: card.as_ref().and_then(|c| c.summary.clone()),
            diagram: card.and_then(|c| c.diagram.clone()),
            options,
            fingerprint: event_id,
        },
        now(),
    )?;
    if let Some(t) = notify_cfg {
        let _ = t.send(
            &format!("{}: 判断待ち", s.title.as_deref().unwrap_or("session")),
            &payload,
            &s.url,
        );
    }
    Ok(())
}

fn card_for_approval(
    store: &Store,
    s: &Session,
    notify_cfg: Option<&notify::Target>,
) -> Result<()> {
    let fingerprint = "approval".to_owned();
    if store.has_pending_decision(&s.session_id, &fingerprint)? {
        return Ok(());
    }
    store.insert_decision(
        &NewDecision {
            session_id: s.session_id.clone(),
            session_title: s.title.clone(),
            session_url: s.url.clone(),
            kind: DecisionKind::Approve,
            title: "承認待ち (safe mode)".to_owned(),
            body: "Devin がアクションの承認を待っています。アプリで承認してください。".to_owned(),
            summary: None,
            diagram: None,
            options: vec![],
            fingerprint,
        },
        now(),
    )?;
    if let Some(t) = notify_cfg {
        let _ = t.send(
            "承認待ち (safe mode)",
            "Devin アプリで承認してください",
            &s.url,
        );
    }
    Ok(())
}
