//! 環境変数からの設定読み込み。定義と検証のみ。

use std::path::PathBuf;

use crate::Result;

#[derive(Debug, Clone)]
pub struct Config {
    /// Devin Cloud 用。ローカルのみの運用では未設定でよい。
    pub api_key: Option<String>,
    pub org_id: Option<String>,
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
    /// `devin` 実行ファイル(ローカル adapter 用)。
    pub devin_bin: PathBuf,
    /// Devin CLI のデータディレクトリ(sessions.db・session_locks の親)。
    pub devin_home: PathBuf,
    /// Devin の credentials.toml。ACP 認証に windsurf_api_key を読む。
    pub devin_credentials: PathBuf,
    /// ローカルセッション一覧に含める最終活動からの日数。
    pub local_days: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            api_key: env_opt("DEVIN_API_KEY"),
            org_id: env_opt("DEVIN_ORG_ID"),
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
            devin_bin: env_opt("ANDON_DEVIN_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("devin")),
            devin_home: env_opt("ANDON_DEVIN_HOME").map_or_else(
                || {
                    std::env::home_dir()
                        .unwrap_or_default()
                        .join(".local/share/devin/cli")
                },
                PathBuf::from,
            ),
            devin_credentials: devin_credentials(),
            local_days: env_opt("ANDON_LOCAL_DAYS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
        })
    }
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// credentials.toml の場所。既定は devin_home の親/devin/credentials.toml。
fn devin_credentials() -> PathBuf {
    if let Some(p) = env_opt("ANDON_DEVIN_CREDENTIALS") {
        return PathBuf::from(p);
    }
    let home = env_opt("ANDON_DEVIN_HOME").map_or_else(
        || {
            std::env::home_dir()
                .unwrap_or_default()
                .join(".local/share/devin/cli")
        },
        PathBuf::from,
    );
    home.parent()
        .map(|p| p.join("credentials.toml"))
        .unwrap_or_else(|| home.join("credentials.toml"))
}
