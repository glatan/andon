//! CLI エントリポイント。起動・サブコマンドの振り分けのみ。

use std::sync::Arc;

use clap::{Parser, Subcommand};
use tokio::sync::broadcast;

use crate::adapter::devin_cloud::{self, DevinCloud};
use crate::config::Config;
use crate::notify;
use crate::server::AppState;
use crate::store::Store;
use crate::{Result, answer};

#[derive(Parser)]
#[command(
    name = "andon",
    about = "Devin セッションの判断要求を中継するダッシュボード"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// デーモン起動。ポーリングと Web UI を開始する
    Serve,
    /// セッションを作成する(カード規約の note と管理タグを付与)
    Send {
        /// タスクのプロンプト
        prompt: String,
    },
    /// 判断カードに回答する(DB と API を直接更新。serve 不要)
    Decide {
        /// カード ID
        id: i64,
        /// 回答文(空白を含む場合はクォートする)
        text: Vec<String>,
    },
    /// ANDON_JSON 規約の knowledge note を作成して ID を保存する
    Setup,
    /// 管理対象セッションを一覧する(検証用)
    Sessions,
    /// ポーリングを 1 回だけ実行する(検証用)
    Poll,
    /// 認証の疎通確認(GET /v3/self)
    Ping,
}

pub fn run() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Serve => serve(),
        Cmd::Send { prompt } => send(&prompt),
        Cmd::Decide { id, text } => decide(id, &text.join(" ")),
        Cmd::Setup => setup(),
        Cmd::Sessions => sessions(),
        Cmd::Poll => poll(),
        Cmd::Ping => ping(),
    }
}

fn open() -> Result<(Config, Arc<Store>, Arc<DevinCloud>)> {
    let cfg = Config::from_env()?;
    let store = Arc::new(Store::open(&cfg.db_path)?);
    let devin = Arc::new(DevinCloud::new(&cfg));
    Ok((cfg, store, devin))
}

/// knowledge note ID の解決順: 環境変数 > DB メタ。
fn note_id(cfg: &Config, store: &Store) -> Result<Option<String>> {
    if cfg.knowledge_note_id.is_some() {
        return Ok(cfg.knowledge_note_id.clone());
    }
    store.meta_get("knowledge_note_id")
}

fn ntfy_target(cfg: &Config) -> Option<notify::Target> {
    cfg.ntfy_topic.clone().map(|topic| notify::Target {
        base: cfg.ntfy_base.clone(),
        topic,
    })
}

fn serve() -> Result<()> {
    let (cfg, store, devin) = open()?;
    let (tx, _) = broadcast::channel::<String>(64);
    {
        let (devin, store, tx) = (devin.clone(), store.clone(), tx.clone());
        let notify = ntfy_target(&cfg);
        let interval = cfg.poll_interval_secs;
        std::thread::spawn(move || {
            devin_cloud::run_poller(devin, store, tx, interval, notify);
        });
    }
    let state = Arc::new(AppState {
        store,
        devin,
        token: cfg.token.clone(),
        tx,
        notify: ntfy_target(&cfg),
    });
    tokio::runtime::Runtime::new()?.block_on(crate::server::serve(&cfg.bind, state))
}

fn send(prompt: &str) -> Result<()> {
    let (cfg, store, devin) = open()?;
    let note = note_id(&cfg, &store)?;
    let (id, url) = devin.create_session(prompt, note.as_deref(), cfg.bypass_approval)?;
    println!("{id}\n{url}");
    Ok(())
}

fn decide(id: i64, text: &str) -> Result<()> {
    let (_cfg, store, devin) = open()?;
    let message = answer::submit(&store, &devin, id, None, Some(text))?;
    println!("回答を送信しました: {message}");
    Ok(())
}

fn setup() -> Result<()> {
    let (_cfg, store, devin) = open()?;
    if let Some(existing) = store.meta_get("knowledge_note_id")? {
        println!(
            "既存の knowledge note: {existing}(再作成する場合は ANDON_KNOWLEDGE_NOTE_ID で上書き)"
        );
        return Ok(());
    }
    let id = devin.provision_note()?;
    store.meta_set("knowledge_note_id", &id)?;
    println!("knowledge note を作成しました: {id}");
    Ok(())
}

fn sessions() -> Result<()> {
    let (_cfg, _store, devin) = open()?;
    for s in devin.list_sessions()? {
        let detail = s.status_detail.as_deref().unwrap_or("-");
        let title = s.title.as_deref().unwrap_or("(no title)");
        println!("{}\t{}/{}\t{}", s.session_id, s.status, detail, title);
    }
    Ok(())
}

fn poll() -> Result<()> {
    let (cfg, store, devin) = open()?;
    let (tx, _) = broadcast::channel::<String>(4);
    let notify = ntfy_target(&cfg);
    devin_cloud::poll_once(&devin, &store, &tx, notify.as_ref())?;
    let pending = store
        .decisions()?
        .iter()
        .filter(|d| matches!(d.state, crate::types::DecisionState::Pending))
        .count();
    println!(
        "sessions={} pending_decisions={pending}",
        store.sessions()?.len()
    );
    Ok(())
}

fn ping() -> Result<()> {
    let (_cfg, _store, devin) = open()?;
    let v = devin.self_check()?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}
