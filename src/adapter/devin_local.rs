//! ローカル Devin(CLI/Desktop)との境界。
//! `sessions.db` を読み取って待機状態を検出し、`devin -r -p` で回答を注入する。
//! Desktop/TUI がロックを握っているセッションは一覧のみで、注入は行わない。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::config::Config;
use crate::store::{NewDecision, Store, now};
use crate::types::{CardOption, DecisionKind, Session};
use crate::{Error, Result, card, notify};

/// store 上でローカルセッションを示す session_id 接頭辞。
pub const LOCAL_PREFIX: &str = "local:";

/// 未解決なら「入力待ち」と判定するツール名。
const WAIT_TOOLS: &[&str] = &["ask_user_question"];

pub struct DevinLocal {
    bin: PathBuf,
    db_path: PathBuf,
    locks_dir: PathBuf,
    /// 一覧対象とする最終活動からの秒数。
    window_secs: i64,
    /// andon 自身の子プロセス PID。ロック保持判定から除外する。
    own_pids: Mutex<std::collections::HashSet<u32>>,
}

#[derive(Debug)]
struct DbSession {
    id: String,
    title: Option<String>,
    last_activity_at: i64,
}

/// 直近の非 system ノード。待機検出とカード本文の材料。
#[derive(Debug)]
struct LastMsg {
    node_id: i64,
    role: String,
    content: String,
    tool_calls: Vec<ToolCall>,
}

#[derive(Debug)]
struct ToolCall {
    id: String,
    name: String,
    arguments: Option<Value>,
}

/// `ask_user_question` の arguments。
#[derive(Debug, Deserialize)]
struct AskArgs {
    questions: Vec<AskQuestion>,
}

#[derive(Debug, Deserialize)]
struct AskQuestion {
    question: String,
    #[serde(default)]
    options: Vec<AskOption>,
}

#[derive(Debug, Deserialize)]
struct AskOption {
    label: String,
    #[serde(default)]
    description: String,
}

impl DevinLocal {
    pub fn new(cfg: &Config) -> Self {
        Self {
            bin: cfg.devin_bin.clone(),
            db_path: cfg.devin_home.join("sessions.db"),
            locks_dir: cfg.devin_home.join("session_locks"),
            window_secs: cfg.local_days.max(1) as i64 * 86_400,
            own_pids: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// andon が起動した子プロセス(devin acp 等)の PID を登録する。
    /// これらが握るロックは「端末保持」とみなさずカード化を妨げない。
    pub fn allow_pid(&self, pid: u32) {
        self.own_pids.lock().unwrap().insert(pid);
    }

    /// sessions.db からセッションの working_directory を返す。
    pub fn session_cwd(&self, id: &str) -> Result<Option<String>> {
        if !valid_id(id) {
            return Ok(None);
        }
        let conn = self.connect()?;
        conn.query_row(
            "SELECT working_directory FROM sessions WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    /// `devin -r <id> -p <msg>` で既存セッションへメッセージを注入する。
    /// 起動直後の失敗(auth 未ログイン・ロック衝突)だけ検査し、
    /// ターン完了は待たない。
    pub fn resume(&self, session_id: &str, message: &str) -> Result<()> {
        let id = session_id.strip_prefix(LOCAL_PREFIX).unwrap_or(session_id);
        if !valid_id(id) {
            return Err(Error::Local(format!("不正なセッション ID: {id}")));
        }
        let mut child = Command::new(&self.bin)
            .args(["-r", id, "-p", message])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        std::thread::sleep(Duration::from_secs(5));
        match child.try_wait()? {
            Some(status) => {
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        let _ = std::io::Read::read_to_string(&mut s, &mut buf);
                        buf
                    })
                    .unwrap_or_default();
                if status.success() {
                    Ok(())
                } else {
                    Err(Error::Local(format!(
                        "devin -r が即時失敗しました。`devin auth login` 済みか確認してください: {}",
                        stderr.trim()
                    )))
                }
            }
            None => {
                std::thread::spawn(move || drop(child.wait()));
                Ok(())
            }
        }
    }

    /// `devin -p <prompt>` を指定 cwd で起動する(detached)。
    /// 新しい session id は起動後に sessions.db から拾う。
    pub fn spawn_session(&self, cwd: &Path, prompt: &str) -> Result<()> {
        Command::new(&self.bin)
            .arg("-p")
            .arg(prompt)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .stdout(Stdio::null())
            .spawn()?;
        Ok(())
    }

    /// `since` 以降に作成されたセッションを最大 `timeout` 待って返す。
    pub fn wait_new_session(&self, since: i64, timeout: Duration) -> Result<Option<String>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let conn = self.connect()?;
            let id: Option<String> = conn
                .query_row(
                    "SELECT id FROM sessions WHERE created_at >= ?1
                     ORDER BY created_at DESC LIMIT 1",
                    params![since],
                    |r| r.get(0),
                )
                .optional()?;
            if id.is_some() || std::time::Instant::now() >= deadline {
                return Ok(id);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// 直近 `window_secs` のローカルセッションを状態付きで一覧する。
    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.connect()?;
        self.rows(&conn)?
            .iter()
            .map(|row| self.inspect(&conn, row).map(|s| s.session))
            .collect::<Result<_>>()
    }

    /// 読み取り専用で開く(WAL の shm が必要な環境では通常接続に落とす)。
    /// DB が存在しない場合は作成せずエラーにする。
    fn connect(&self) -> Result<Connection> {
        if !self.db_path.exists() {
            return Err(Error::Local(format!(
                "sessions.db が見つかりません: {}",
                self.db_path.display()
            )));
        }
        let uri = format!("file:{}?mode=ro", self.db_path.display());
        Ok(Connection::open(&uri).or_else(|_| Connection::open(&self.db_path))?)
    }

    fn rows(&self, conn: &Connection) -> Result<Vec<DbSession>> {
        let cutoff = now() - self.window_secs;
        let mut stmt = conn.prepare(
            "SELECT id, title, last_activity_at FROM sessions
             WHERE hidden = 0 AND last_activity_at >= ?1
             ORDER BY last_activity_at DESC LIMIT 100",
        )?;
        let rows = stmt
            .query_map(params![cutoff], |r| {
                Ok(DbSession {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    last_activity_at: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// ロックファイルの PID が生存なら true(Desktop/TUI が掴んでいる)。
    /// パス結合前に ID を検証して traversal を防ぐ。
    fn lock_held(&self, id: &str) -> bool {
        if !valid_id(id) {
            return false;
        }
        let Ok(body) = std::fs::read_to_string(self.locks_dir.join(format!("{id}.lock"))) else {
            return false;
        };
        let Ok(pid) = body.trim().parse::<u32>() else {
            return false;
        };
        if self.own_pids.lock().unwrap().contains(&pid) {
            return false;
        }
        pid_alive(pid)
    }

    /// 1 セッション分の状態を推定する。
    fn inspect(&self, conn: &Connection, row: &DbSession) -> Result<Snap> {
        let held = self.lock_held(&row.id);
        let last = last_message(conn, &row.id)?;
        let mut pending_asks = Vec::new();
        if let Some(m) = &last
            && m.role == "assistant"
        {
            for tc in &m.tool_calls {
                if tool_resolved(conn, &row.id, &tc.id)? {
                    continue;
                }
                if WAIT_TOOLS.contains(&tc.name.as_str())
                    && let Some(args) = &tc.arguments
                {
                    pending_asks.push(args.clone());
                }
            }
        }
        // 「入力待ち」= ターン終了(assistant 発言で tool_calls なし)
        //        または 未解決の ask_user_question がある。
        let last_waiting = last.as_ref().is_some_and(|m| {
            m.role == "assistant" && (m.tool_calls.is_empty() || !pending_asks.is_empty())
        });
        let (status, detail, waiting) = match &last {
            None => ("suspended", "empty", false),
            Some(_) if last_waiting => {
                if held {
                    ("running", "in_terminal", false)
                } else {
                    ("suspended", "waiting_for_user", true)
                }
            }
            Some(_) => {
                if held {
                    ("running", "working", false)
                } else {
                    ("suspended", "interrupted", false)
                }
            }
        };
        Ok(Snap {
            session: Session {
                session_id: format!("{LOCAL_PREFIX}{}", row.id),
                title: row.title.clone(),
                url: format!("{LOCAL_PREFIX}{}", row.id),
                status: status.to_owned(),
                status_detail: Some(detail.to_owned()),
                updated_at: row.last_activity_at,
            },
            waiting,
            last,
            pending_asks,
        })
    }
}

/// セッションの推定結果。poller がカード化判断に使う。
struct Snap {
    session: Session,
    /// 入力待ちかつ注入可能(ロック未保持)。
    waiting: bool,
    last: Option<LastMsg>,
    /// 未解決 ask_user_question の arguments(先頭のみ使う)。
    pending_asks: Vec<Value>,
}

/// 最新の非 system ノードを返す。
fn last_message(conn: &Connection, session_id: &str) -> Result<Option<LastMsg>> {
    conn.query_row(
        "SELECT node_id, chat_message FROM message_nodes
         WHERE session_id = ?1
           AND json_extract(chat_message, '$.role') IN ('assistant', 'user', 'tool')
         ORDER BY node_id DESC LIMIT 1",
        params![session_id],
        |r| {
            let raw: String = r.get(1)?;
            let v: Value = serde_json::from_str(&raw).unwrap_or_default();
            let tool_calls = v
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| {
                            Some(ToolCall {
                                id: t.get("id")?.as_str()?.to_owned(),
                                name: t.get("name")?.as_str()?.to_owned(),
                                arguments: t.get("arguments").cloned(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(LastMsg {
                node_id: r.get(0)?,
                role: v["role"].as_str().unwrap_or_default().to_owned(),
                content: v["content"].as_str().unwrap_or_default().to_owned(),
                tool_calls,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// tool_call_id に対応する tool 結果ノードが既にあるか。
fn tool_resolved(conn: &Connection, session_id: &str, call_id: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM message_nodes
         WHERE session_id = ?1
           AND json_extract(chat_message, '$.role') = 'tool'
           AND json_extract(chat_message, '$.tool_call_id') = ?2",
        params![session_id, call_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// セッション ID の妥当性。CLI 引数注入とパス結合 traversal の双方を防ぐ。
/// 先頭は英数字のみ許可し `-` 始まり(フラグ注入)を排除する。
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// PID の生存確認。/proc を優先し、なければ kill -0 にフォールバック。
fn pid_alive(pid: u32) -> bool {
    if Path::new(&format!("/proc/{pid}")).exists() {
        return true;
    }
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 1 回分のポーリング。sessions.db を走査してカード化・失効を行う。
///
/// カード化するのは「入力待ち + ロック未保持 + 根拠あり」のときだけ。
/// 根拠は次のいずれか: andon 所有セッション、未解決の ask_user_question、
/// ANDON_JSON 指令付きメッセージ。根拠がない完了セッションは一覧のみ。
pub fn poll_once(
    local: &DevinLocal,
    store: &Store,
    tx: &broadcast::Sender<String>,
    notify_cfg: Option<&notify::Target>,
) -> Result<()> {
    let conn = local.connect()?;
    for row in local.rows(&conn)? {
        let snap = local.inspect(&conn, &row)?;
        store.upsert_session(&snap.session)?;
        let owned = store
            .meta_get(&format!("local_owned:{}", row.id))?
            .is_some();
        let cardable = snap.waiting
            && snap.last.as_ref().is_some_and(|m| {
                owned || !snap.pending_asks.is_empty() || card::parse(&m.content).is_some()
            });
        if cardable {
            card_for_wait(store, &snap, &row, owned, notify_cfg)?;
        } else {
            store.expire_pending_for(&snap.session.session_id)?;
        }
    }
    let _ = tx.send("update".to_owned());
    Ok(())
}

/// ポーリングループ。serve から専用スレッドで回す。ローカル DB は軽いので
/// バックオフは設けず固定間隔で回し、一時的な失敗はログだけ出して続行する。
pub fn run_poller(
    local: Arc<DevinLocal>,
    store: Arc<Store>,
    tx: broadcast::Sender<String>,
    interval_secs: u64,
    notify_cfg: Option<notify::Target>,
) {
    let interval = Duration::from_secs(interval_secs.max(2));
    loop {
        if let Err(e) = poll_once(&local, &store, &tx, notify_cfg.as_ref()) {
            eprintln!("[andon] ローカルポーリング失敗: {e}");
        }
        std::thread::sleep(interval);
    }
}

/// 待機セッションをカード化する。ask_user_question > ANDON_JSON > 汎用の順。
fn card_for_wait(
    store: &Store,
    snap: &Snap,
    row: &DbSession,
    owned: bool,
    notify_cfg: Option<&notify::Target>,
) -> Result<()> {
    let Some(m) = &snap.last else { return Ok(()) };
    let sid = &snap.session.session_id;
    if let Some(args) = snap.pending_asks.first() {
        let fp = format!("local-ask:{}", m.node_id);
        if store.has_pending_decision(sid, &fp)? {
            return Ok(());
        }
        let parsed: AskArgs =
            serde_json::from_value(args.clone()).unwrap_or(AskArgs { questions: vec![] });
        let first = parsed.questions.into_iter().next();
        let title = first
            .as_ref()
            .map(|q| q.question.clone())
            .unwrap_or_else(|| "判断待ち".to_owned());
        let options: Vec<CardOption> = first
            .map(|q| {
                q.options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| CardOption {
                        id: i.to_string(),
                        label: if o.description.is_empty() {
                            o.label.clone()
                        } else {
                            format!("{} — {}", o.label, o.description)
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();
        store.insert_decision(
            &NewDecision {
                session_id: sid.clone(),
                session_title: row.title.clone(),
                session_url: snap.session.url.clone(),
                kind: DecisionKind::Choice,
                title,
                body: m.content.clone(),
                summary: None,
                diagram: None,
                options,
                fingerprint: fp,
            },
            now(),
        )?;
        notify_wait(notify_cfg, row, "Devin が質問しています");
        return Ok(());
    }
    let fp = format!("local-msg:{}", m.node_id);
    if store.has_pending_decision(sid, &fp)? {
        return Ok(());
    }
    let (body, excerpt) = card::body_excerpt(&m.content);
    let payload = card::parse(&m.content);
    let title = payload
        .as_ref()
        .and_then(|c| c.title.clone())
        .or_else(|| row.title.clone())
        .unwrap_or_else(|| "判断待ち".to_owned());
    let options = payload
        .as_ref()
        .filter(|c| !c.options.is_empty())
        .map(|c| c.options.clone())
        .unwrap_or_else(|| {
            if owned {
                card::fallback_options()
            } else {
                vec![]
            }
        });
    store.insert_decision(
        &NewDecision {
            session_id: sid.clone(),
            session_title: row.title.clone(),
            session_url: snap.session.url.clone(),
            kind: DecisionKind::Choice,
            title,
            body,
            summary: payload.as_ref().and_then(|c| c.summary.clone()),
            diagram: payload.and_then(|c| c.diagram.clone()),
            options,
            fingerprint: fp,
        },
        now(),
    )?;
    notify_wait(notify_cfg, row, &excerpt);
    Ok(())
}

fn notify_wait(notify_cfg: Option<&notify::Target>, row: &DbSession, body: &str) {
    if let Some(t) = notify_cfg {
        let _ = t.send(
            &format!(
                "{}: 判断待ち",
                row.title.as_deref().unwrap_or("local session")
            ),
            body,
            "",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY, working_directory TEXT NOT NULL,
                 backend_type TEXT NOT NULL, model TEXT NOT NULL,
                 agent_mode TEXT NOT NULL, created_at INTEGER NOT NULL,
                 last_activity_at INTEGER NOT NULL, title TEXT,
                 hidden INTEGER NOT NULL DEFAULT 0, metadata TEXT);
             CREATE TABLE message_nodes (
                 row_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL, node_id INTEGER NOT NULL,
                 parent_node_id INTEGER, chat_message TEXT NOT NULL,
                 created_at INTEGER NOT NULL, metadata TEXT);",
        )
        .unwrap();
        conn
    }

    fn add_msg(conn: &Connection, sid: &str, node: i64, msg: &str) {
        conn.execute(
            "INSERT INTO message_nodes (session_id, node_id, chat_message, created_at)
             VALUES (?1, ?2, ?3, 0)",
            params![sid, node, msg],
        )
        .unwrap();
    }

    #[test]
    fn last_message_skips_system_nodes() {
        let conn = fixture();
        add_msg(&conn, "s", 1, r#"{"role":"assistant","content":"done"}"#);
        add_msg(&conn, "s", 2, r#"{"role":"system","content":"compact"}"#);
        let m = last_message(&conn, "s").unwrap().unwrap();
        assert_eq!(m.node_id, 1);
        assert_eq!(m.content, "done");
    }

    #[test]
    fn parses_tool_calls_from_last_message() {
        let conn = fixture();
        add_msg(
            &conn,
            "s",
            1,
            r#"{"role":"assistant","content":"","tool_calls":[{"id":"ask_user_question:0#x","name":"ask_user_question","arguments":{"questions":[{"question":"Q?"}]}}]}"#,
        );
        let m = last_message(&conn, "s").unwrap().unwrap();
        assert_eq!(m.tool_calls.len(), 1);
        assert_eq!(m.tool_calls[0].name, "ask_user_question");
    }

    #[test]
    fn tool_resolved_detects_matching_tool_message() {
        let conn = fixture();
        add_msg(
            &conn,
            "s",
            1,
            r#"{"role":"assistant","content":"","tool_calls":[{"id":"exec:0#a","name":"exec"}]}"#,
        );
        assert!(!tool_resolved(&conn, "s", "exec:0#a").unwrap());
        add_msg(
            &conn,
            "s",
            2,
            r#"{"role":"tool","content":"out","tool_call_id":"exec:0#a"}"#,
        );
        assert!(tool_resolved(&conn, "s", "exec:0#a").unwrap());
    }

    #[test]
    fn valid_id_rejects_flag_and_traversal() {
        assert!(valid_id("sandy-krill"));
        assert!(valid_id("276dbb78bfd2453f9852d5aae5fe0fd1"));
        assert!(!valid_id("--resume"));
        assert!(!valid_id("../etc/passwd"));
        assert!(!valid_id("a b"));
        assert!(!valid_id(""));
    }

    #[test]
    fn pid_alive_accepts_running_process() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(u32::MAX));
    }
}
