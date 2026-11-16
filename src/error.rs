//! エラー型の集約。

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("設定が不足しています: {0} を設定してください")]
    Config(&'static str),
    #[error("HTTP エラー: {0}")]
    Http(#[from] ureq::Error),
    #[error("JSON エラー: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ストアエラー: {0}")]
    Store(#[from] rusqlite::Error),
    #[error("I/O エラー: {0}")]
    Io(#[from] std::io::Error),
    #[error("Devin API エラー (status {status}): {body}")]
    Api { status: u16, body: String },
    #[error("指定された decision が見つかりません: {0}")]
    NotFound(i64),
    #[error("decision は回答済みです: {0}")]
    AlreadyAnswered(i64),
    #[error("ローカル Devin: {0}")]
    Local(String),
}

pub type Result<T> = std::result::Result<T, Error>;
