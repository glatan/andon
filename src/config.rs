//! 環境変数からの設定読み込み。定義と検証のみ。

use std::path::PathBuf;

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub org_id: String,
    pub api_base: String,
    pub bind: String,
    pub db_path: PathBuf,
    /// PWA / API 認証トークン。None の場合は認証なし(ローカル開発向け)。
    pub token: Option<String>,
    pub ntfy_base: String,
    pub ntfy_topic: Option<String>,
    pub poll_interval_secs: u64,
    /// セッション作成時に bypass_approval を付けるか。
    pub bypass_approval: bool,
    /// ANDON_JSON 規約の knowledge note ID。`andon setup` で発行し保存する。
    pub knowledge_note_id: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            api_key: env("DEVIN_API_KEY")?,
            org_id: env("DEVIN_ORG_ID")?,
            api_base: env_opt("DEVIN_API_BASE")
                .unwrap_or_else(|| "https://api.devin.ai".to_owned()),
            bind: env_opt("ANDON_BIND").unwrap_or_else(|| "127.0.0.1:8787".to_owned()),
            db_path: PathBuf::from(env_opt("ANDON_DB").unwrap_or_else(|| "andon.db".to_owned())),
            token: env_opt("ANDON_TOKEN"),
            ntfy_base: env_opt("ANDON_NTFY_BASE").unwrap_or_else(|| "https://ntfy.sh".to_owned()),
            ntfy_topic: env_opt("ANDON_NTFY_TOPIC"),
            poll_interval_secs: env_opt("ANDON_POLL_INTERVAL")
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            bypass_approval: env_opt("ANDON_BYPASS_APPROVAL").is_some_and(|v| v == "1"),
            knowledge_note_id: env_opt("ANDON_KNOWLEDGE_NOTE_ID"),
        })
    }
}

fn env(key: &'static str) -> Result<String> {
    std::env::var(key).map_err(|_| Error::Config(key))
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}
