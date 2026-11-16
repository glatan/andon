//! Devin ACP(stdio JSON-RPC)との境界。
//! `devin acp` プロセスを保持し、session/new・load・prompt の発行と
//! session/request_permission(エージェント→クライアント要求)の中継を担う。
//!
//! 認証は credentials.toml の windsurf_api_key を `_meta.api_key` で渡す
//! 実機確認済みの経路を使い、`devin auth login` を必要としない。

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::notify;
use crate::store::{NewDecision, Store, now};
use crate::types::{CardOption, DecisionKind};
use crate::{Error, Result};

/// 権限要求カードの fingerprint 接頭辞(`acp-perm:<rpc_id>`)。
pub const FINGERPRINT_PERM: &str = "acp-perm:";

type Resp = mpsc::Sender<Value>;

struct PermWait {
    rpc_id: u64,
}

/// `devin acp` への接続。reader スレッドが応答・通知・権限要求を振り分ける。
pub struct DevinAcp {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Option<Child>>,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, Resp>>,
    /// fingerprint → 回答待ちの権限要求。
    perms: Mutex<HashMap<String, PermWait>>,
    /// この接続で load 済みのセッション。
    loaded: Mutex<HashSet<String>>,
    pid: u32,
}

impl DevinAcp {
    /// `devin acp` を起動し initialize + authenticate まで済ませる。
    /// `api_key` は credentials.toml の windsurf_api_key。
    /// 失敗時はプロセスごと畳んで Err。
    pub fn connect(
        bin: &Path,
        api_key: &str,
        store: Arc<Store>,
        tx: broadcast::Sender<String>,
        notify: Option<notify::Target>,
    ) -> Result<Arc<Self>> {
        let mut child = Command::new(bin)
            .arg("acp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Local("acp stdin の取得に失敗".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Local("acp stdout の取得に失敗".to_owned()))?;
        let pid = child.id();
        let acp = Arc::new(Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(Some(child)),
            next: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            perms: Mutex::new(HashMap::new()),
            loaded: Mutex::new(HashSet::new()),
            pid,
        });
        {
            let acp = acp.clone();
            std::thread::spawn(move || {
                reader(acp, BufReader::new(stdout), store, tx, notify);
            });
        }
        acp.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {"readTextFile": true, "writeTextFile": false},
                    "terminal": false
                },
                "clientInfo": {"name": "andon", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        acp.request(
            "authenticate",
            json!({"methodId": "devin-browser", "_meta": {"api_key": api_key}}),
        )?;
        Ok(acp)
    }

    /// acp 子プロセスの PID(ロック判定の除外用)。
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    fn send(&self, v: &Value) -> Result<()> {
        let mut w = self.stdin.lock().unwrap();
        writeln!(w, "{v}")?;
        w.flush()?;
        Ok(())
    }

    /// 応答を待つ JSON-RPC 呼び出し(60秒)。
    fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let sent = self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let v = rx
            .recv_timeout(Duration::from_secs(60))
            .map_err(|_| Error::Local(format!("acp {method} がタイムアウト")))?;
        if let Some(err) = v.get("error") {
            return Err(Error::Local(format!("acp {method}: {}", err["message"])));
        }
        Ok(v["result"].clone())
    }

    /// `session/new` でセッションを作り sessionId を返す。
    /// 作成済みセッションはこの接続に紐づくため loaded にも登録する。
    pub fn new_session(&self, cwd: &str) -> Result<String> {
        let v = self.request("session/new", json!({"cwd": cwd, "mcpServers": []}))?;
        let id = v["sessionId"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| Error::Local("session/new の応答に sessionId がない".to_owned()))?;
        self.loaded.lock().unwrap().insert(id.clone());
        Ok(id)
    }

    /// `session/load` で既存セッションに接続する。ロック中はエラーになる。
    fn load_session(&self, session_id: &str, cwd: &str) -> Result<()> {
        if self.loaded.lock().unwrap().contains(session_id) {
            return Ok(());
        }
        self.request(
            "session/load",
            json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []}),
        )?;
        self.loaded.lock().unwrap().insert(session_id.to_owned());
        Ok(())
    }

    /// `session/prompt` を送信する。応答はターン終了時に届くため
    /// ここでは発行だけ行い、完了は sessions.db 監視に任せる。
    pub fn prompt(&self, session_id: &str, cwd: Option<&str>, text: &str) -> Result<()> {
        if let Some(cwd) = cwd {
            self.load_session(session_id, cwd)?;
        }
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": self.next.fetch_add(1, Ordering::Relaxed),
            "method": "session/prompt",
            "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}
        }))
    }

    /// 保留中の権限要求へ回答を返す。
    /// `option_id` は要求が提示した optionId、`None` ならキャンセル扱い。
    pub fn respond_permission(&self, fingerprint: &str, option_id: Option<&str>) -> Result<()> {
        let Some(w) = self.perms.lock().unwrap().remove(fingerprint) else {
            return Err(Error::Local(format!(
                "権限要求が見つかりません: {fingerprint}"
            )));
        };
        let outcome = match option_id {
            Some(oid) => json!({"outcome": "selected", "optionId": oid}),
            None => json!({"outcome": "cancelled"}),
        };
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": w.rpc_id,
            "result": {"outcome": outcome}
        }))
        .inspect_err(|_| {
            self.perms.lock().unwrap().insert(fingerprint.to_owned(), w);
        })
    }
}

impl Drop for DevinAcp {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// stdout を読み続け、応答は pending へ、agent→client 要求はハンドラへ振り分ける。
fn reader(
    acp: Arc<DevinAcp>,
    mut r: impl BufRead,
    store: Arc<Store>,
    tx: broadcast::Sender<String>,
    notify: Option<notify::Target>,
) {
    let mut line = String::new();
    loop {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(m) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match classify(&m) {
            Msg::Response(id) => {
                if let Some(s) = acp.pending.lock().unwrap().remove(&id) {
                    let _ = s.send(m);
                }
            }
            Msg::AgentRequest => agent_request(&acp, &m, &store, &tx, notify.as_ref()),
            // 通知(method のみ)は現時点では捨てる。状態変化は sessions.db で追う。
            Msg::Notification => {}
        }
    }
    let _ = tx.send("update".to_owned());
}

/// 受信メッセージの種別。JSON-RPC 2.0 の id/method 有無で振り分ける。
enum Msg {
    /// 自前の request への応答。id で pending を引く。
    Response(u64),
    /// agent→client の要求(応答が必要)。
    AgentRequest,
    /// 通知(method のみ)。現状は監視しない。
    Notification,
}

fn classify(m: &Value) -> Msg {
    match (m.get("method").is_some(), m.get("id").is_some()) {
        (false, true) => Msg::Response(m["id"].as_u64().unwrap_or(0)),
        (true, true) => Msg::AgentRequest,
        _ => Msg::Notification,
    }
}

/// `session/request_permission` の params を判断カード入力に変換する。I/O なし。
fn permission_card(params: &Value, fingerprint: &str) -> Option<NewDecision> {
    let raw_sid = params["sessionId"].as_str()?;
    let tool = &params["toolCall"];
    let title = tool["title"].as_str().unwrap_or("権限要求").to_owned();
    let body = tool["rawInput"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| serde_json::to_string_pretty(tool).unwrap_or_default());
    let options: Vec<CardOption> = params["options"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|o| {
                    Some(CardOption {
                        id: o["optionId"].as_str()?.to_owned(),
                        label: o["name"].as_str()?.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(NewDecision {
        session_id: format!("local:{raw_sid}"),
        session_title: None,
        session_url: format!("local:{raw_sid}"),
        kind: DecisionKind::Choice,
        title: format!("権限要求: {title}"),
        body,
        summary: None,
        diagram: None,
        options,
        fingerprint: fingerprint.to_owned(),
    })
}

/// agent→client の要求。`session/request_permission` を判断カードに変換する。
fn agent_request(
    acp: &DevinAcp,
    m: &Value,
    store: &Store,
    tx: &broadcast::Sender<String>,
    notify: Option<&notify::Target>,
) {
    if m["method"].as_str() != Some("session/request_permission") {
        // 未知の agent→client 要求は応答不能なので放置(タイムアウトに委ねる)。
        return;
    }
    let Some(rpc_id) = m["id"].as_u64() else {
        return;
    };
    let fp = format!("{FINGERPRINT_PERM}{rpc_id}");
    let Some(card) = permission_card(&m["params"], &fp) else {
        return;
    };
    if store
        .has_pending_decision(&card.session_id, &fp)
        .unwrap_or(true)
    {
        return;
    }
    let title = card.title.clone();
    let Ok(id) = store.insert_decision(&card, now()) else {
        return;
    };
    acp.perms.lock().unwrap().insert(fp, PermWait { rpc_id });
    let _ = tx.send("update".to_owned());
    if let Some(n) = notify {
        let _ = n.send(&title, &format!("カード #{id}"), "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_prefix_roundtrip() {
        let fp = format!("{FINGERPRINT_PERM}42");
        assert!(fp.starts_with(FINGERPRINT_PERM));
    }

    #[test]
    fn classify_splits_response_request_notification() {
        assert!(matches!(
            classify(&json!({"jsonrpc":"2.0","id":7,"result":{}})),
            Msg::Response(7)
        ));
        assert!(matches!(
            classify(&json!({"jsonrpc":"2.0","id":3,"method":"session/request_permission"})),
            Msg::AgentRequest
        ));
        assert!(matches!(
            classify(&json!({"jsonrpc":"2.0","method":"session/update","params":{}})),
            Msg::Notification
        ));
    }

    #[test]
    fn permission_card_maps_options_and_body() {
        let params = json!({
            "sessionId": "sandy-krill",
            "toolCall": {"title": "exec ls", "rawInput": "ls -la /tmp"},
            "options": [
                {"optionId": "allow_once", "name": "許可(今回のみ)"},
                {"optionId": "reject", "name": "拒否"},
                {"name": "optionId なしは捨てる"}
            ]
        });
        let card = permission_card(&params, "acp-perm:9").unwrap();
        assert_eq!(card.session_id, "local:sandy-krill");
        assert_eq!(card.title, "権限要求: exec ls");
        assert_eq!(card.body, "ls -la /tmp");
        assert_eq!(card.options.len(), 2);
        assert_eq!(card.options[0].id, "allow_once");
        assert_eq!(card.fingerprint, "acp-perm:9");
    }

    #[test]
    fn permission_card_rejects_missing_session() {
        assert!(permission_card(&json!({"options": []}), "acp-perm:1").is_none());
    }
}
