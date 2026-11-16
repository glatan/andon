//! ドメイン型の定義。serde でそのまま API・フロントとやり取りする。

use serde::{Deserialize, Serialize};

/// Devin セッションの一覧表示用スナップショット。
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub session_id: String,
    pub title: Option<String>,
    pub url: String,
    pub status: String,
    pub status_detail: Option<String>,
    pub updated_at: i64,
}

/// 判断要求カードの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// 選択肢または自由入力への回答待ち (waiting_for_user)。
    Choice,
    /// Devin アプリ側での承認待ち (waiting_for_approval)。リンクのみ。
    Approve,
    /// 状態表示のみ。
    Status,
}

/// カードの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionState {
    Pending,
    Answered,
    /// セッションが待ち状態を抜けて自動的に無効化された。
    Expired,
}

/// 選択肢。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardOption {
    pub id: String,
    pub label: String,
}

/// 図解の指定。現状は mermaid のみ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardDiagram {
    #[serde(rename = "type")]
    pub kind: String,
    pub source: String,
}

/// 人間に提示する判断要求カード。
#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub id: i64,
    pub session_id: String,
    pub session_title: Option<String>,
    pub session_url: String,
    pub kind: DecisionKind,
    pub state: DecisionState,
    pub title: String,
    /// 最終 Devin メッセージ(抜粋)。
    pub body: String,
    pub summary: Option<String>,
    pub diagram: Option<CardDiagram>,
    pub options: Vec<CardOption>,
    pub answer: Option<String>,
    /// 重複防止指紋。`acp-perm:` 始まりは ACP 権限要求に対応する。
    pub fingerprint: String,
    pub created_at: i64,
    pub answered_at: Option<i64>,
}
