//! SQLite による永続化。セッション一覧・判断カード・メタ情報を保持する。

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;
use crate::types::{CardDiagram, CardOption, Decision, DecisionKind, DecisionState, Session};

pub struct Store {
    conn: Mutex<Connection>,
}

/// 新規カードの入力値。
pub struct NewDecision {
    pub session_id: String,
    pub session_title: Option<String>,
    pub session_url: String,
    pub kind: DecisionKind,
    pub title: String,
    pub body: String,
    pub summary: Option<String>,
    pub diagram: Option<CardDiagram>,
    pub options: Vec<CardOption>,
    /// 同じ未決カードを再度作らないための指紋(メッセージID等)。
    pub fingerprint: String,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS sessions (
                 session_id TEXT PRIMARY KEY,
                 title TEXT,
                 url TEXT NOT NULL,
                 status TEXT NOT NULL,
                 status_detail TEXT,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS decisions (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL,
                 session_title TEXT,
                 session_url TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 state TEXT NOT NULL,
                 title TEXT NOT NULL,
                 body TEXT NOT NULL,
                 summary TEXT,
                 diagram TEXT,
                 options TEXT NOT NULL,
                 answer TEXT,
                 fingerprint TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 answered_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn upsert_session(&self, s: &Session) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO sessions (session_id, title, url, status, status_detail, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (session_id) DO UPDATE SET
                 title = excluded.title, url = excluded.url,
                 status = excluded.status, status_detail = excluded.status_detail,
                 updated_at = excluded.updated_at",
            params![
                s.session_id,
                s.title,
                s.url,
                s.status,
                s.status_detail,
                s.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, title, url, status, status_detail, updated_at
             FROM sessions ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Session {
                session_id: r.get(0)?,
                title: r.get(1)?,
                url: r.get(2)?,
                status: r.get(3)?,
                status_detail: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// この指紋の未決カードが既にあれば true(重複作成防止)。
    pub fn has_pending_decision(&self, session_id: &str, fingerprint: &str) -> Result<bool> {
        let n: i64 = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM decisions
             WHERE session_id = ?1 AND fingerprint = ?2 AND state = 'pending'",
            params![session_id, fingerprint],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// セッションの未決カードをすべて expired にする(待ち状態を抜けたとき)。
    /// `except_prefix` があると、その接頭辞の指紋を持つカードは残す
    /// (ACP 権限要求のように sessions.db 待機推定の外側で管理するカード用)。
    /// 接頭辞に LIKE ワイルドカード(%, _)を含めないこと。
    pub fn expire_pending_for(&self, session_id: &str, except_prefix: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        match except_prefix {
            Some(p) => conn.execute(
                "UPDATE decisions SET state = 'expired'
                 WHERE session_id = ?1 AND state = 'pending'
                   AND fingerprint NOT LIKE ?2 || '%'",
                params![session_id, p],
            )?,
            None => conn.execute(
                "UPDATE decisions SET state = 'expired'
                 WHERE session_id = ?1 AND state = 'pending'",
                params![session_id],
            )?,
        };
        Ok(())
    }

    /// 指紋接頭辞が一致する未決カードをすべて expired にする。
    /// ACP 再起動時に、死んだプロセスの権限要求カードを掃除する用途。
    /// 返り値は expired にした件数。
    pub fn expire_fingerprint_prefix(&self, prefix: &str) -> Result<usize> {
        let n = self.conn.lock().unwrap().execute(
            "UPDATE decisions SET state = 'expired'
             WHERE state = 'pending' AND fingerprint LIKE ?1 || '%'",
            params![prefix],
        )?;
        Ok(n)
    }

    pub fn insert_decision(&self, d: &NewDecision, now: i64) -> Result<i64> {
        let diagram = d.diagram.as_ref().map(serde_json::to_string).transpose()?;
        let options = serde_json::to_string(&d.options)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO decisions
                 (session_id, session_title, session_url, kind, state, title,
                  body, summary, diagram, options, fingerprint, created_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                d.session_id,
                d.session_title,
                d.session_url,
                kind_str(d.kind),
                d.title,
                d.body,
                d.summary,
                diagram,
                options,
                d.fingerprint,
                now,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn decision(&self, id: i64) -> Result<Option<Decision>> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT * FROM decisions WHERE id = ?1",
                params![id],
                row_to_decision,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn decisions(&self) -> Result<Vec<Decision>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT * FROM decisions
             ORDER BY (state = 'pending') DESC, created_at DESC LIMIT 100",
        )?;
        let rows = stmt.query_map([], row_to_decision)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn answer_decision(&self, id: i64, text: &str, now: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE decisions SET state = 'answered', answer = ?2, answered_at = ?3
             WHERE id = ?1 AND state = 'pending'",
            params![id, text, now],
        )?;
        Ok(())
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

const fn kind_str(k: DecisionKind) -> &'static str {
    match k {
        DecisionKind::Choice => "choice",
        DecisionKind::Approve => "approve",
        DecisionKind::Status => "status",
    }
}

fn row_to_decision(r: &rusqlite::Row<'_>) -> rusqlite::Result<Decision> {
    let kind: String = r.get("kind")?;
    let state: String = r.get("state")?;
    let diagram: Option<String> = r.get("diagram")?;
    let options: String = r.get("options")?;
    Ok(Decision {
        id: r.get("id")?,
        session_id: r.get("session_id")?,
        session_title: r.get("session_title")?,
        session_url: r.get("session_url")?,
        kind: serde_json::from_str(&format!("\"{kind}\"")).unwrap_or(DecisionKind::Choice),
        state: serde_json::from_str(&format!("\"{state}\"")).unwrap_or(DecisionState::Pending),
        title: r.get("title")?,
        body: r.get("body")?,
        summary: r.get("summary")?,
        diagram: diagram.and_then(|d| serde_json::from_str(&d).ok()),
        options: serde_json::from_str(&options).unwrap_or_default(),
        answer: r.get("answer")?,
        fingerprint: r.get("fingerprint")?,
        created_at: r.get("created_at")?,
        answered_at: r.get("answered_at")?,
    })
}

/// UNIX 時刻(秒)。
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    fn nd(fingerprint: &str) -> NewDecision {
        NewDecision {
            session_id: "local:x".to_owned(),
            session_title: None,
            session_url: "local:x".to_owned(),
            kind: DecisionKind::Choice,
            title: "t".to_owned(),
            body: "b".to_owned(),
            summary: None,
            diagram: None,
            options: vec![],
            fingerprint: fingerprint.to_owned(),
        }
    }

    /// ACP 権限要求カードは待機推定の外で管理するため、
    /// expire から除外されなければならない(ポーラーに消されない)。
    #[test]
    fn expire_pending_for_keeps_except_prefix() {
        let s = mem();
        s.insert_decision(&nd("acp-perm:1"), now()).unwrap();
        s.insert_decision(&nd("msg:2"), now()).unwrap();
        s.expire_pending_for("local:x", Some("acp-perm:")).unwrap();
        let ds = s.decisions().unwrap();
        assert_eq!(
            ds.iter()
                .find(|d| d.fingerprint == "acp-perm:1")
                .unwrap()
                .state,
            DecisionState::Pending
        );
        assert_eq!(
            ds.iter().find(|d| d.fingerprint == "msg:2").unwrap().state,
            DecisionState::Expired
        );
    }

    #[test]
    fn expire_pending_for_without_prefix_expires_all() {
        let s = mem();
        s.insert_decision(&nd("acp-perm:1"), now()).unwrap();
        s.expire_pending_for("local:x", None).unwrap();
        assert_eq!(s.decisions().unwrap()[0].state, DecisionState::Expired);
    }

    #[test]
    fn expire_fingerprint_prefix_sweeps_only_matching() {
        let s = mem();
        s.insert_decision(&nd("acp-perm:1"), now()).unwrap();
        s.insert_decision(&nd("msg:2"), now()).unwrap();
        assert_eq!(s.expire_fingerprint_prefix("acp-perm:").unwrap(), 1);
        let ds = s.decisions().unwrap();
        assert_eq!(
            ds.iter()
                .find(|d| d.fingerprint == "acp-perm:1")
                .unwrap()
                .state,
            DecisionState::Expired
        );
        assert_eq!(
            ds.iter().find(|d| d.fingerprint == "msg:2").unwrap().state,
            DecisionState::Pending
        );
    }
}
