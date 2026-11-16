//! CLI エントリポイント。起動・サブコマンドの振り分けのみ。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use tokio::sync::broadcast;

use crate::adapter::devin_cloud::{self, DevinCloud};
use crate::adapter::devin_local::{self, DevinLocal};
use crate::config::Config;
use crate::notify;
use crate::server::AppState;
use crate::store::{Store, now};
use crate::{Result, answer, card};

#[derive(Parser)]
#[command(
    name = "andon",
    about = "Devin セッションの判断要求を中継するダッシュボード"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// 監視対象のソース。serve で選択する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Source {
    /// Devin Cloud API(要 DEVIN_API_KEY / DEVIN_ORG_ID)
    Cloud,
    /// ローカル Devin CLI/Desktop(回答には devin auth login が必要)
    Local,
    /// Cloud とローカルの両方を監視する
    Both,
}

#[derive(Subcommand)]
enum Cmd {
    /// デーモン起動。ポーリングと Web UI を開始する
    Serve {
        /// 監視対象: cloud / local / both
        #[arg(long, value_enum, default_value_t = Source::Cloud)]
        source: Source,
    },
    /// セッションを作成する(カード規約と管理タグを付与)
    Send {
        /// タスクのプロンプト
        prompt: String,
        /// ローカル Devin セッションとして起動する(devin -p)
        #[arg(long)]
        local: bool,
        /// --local 時の作業ディレクトリ。省略時はカレント
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// 判断カードに回答する(DB と送信先を直接更新。serve 不要)
    Decide {
        /// カード ID
        id: i64,
        /// 回答文(空白を含む場合はクォートする)
        text: Vec<String>,
    },
    /// ANDON_JSON 規約の knowledge note を作成して ID を保存する
    Setup,
    /// 管理対象セッションを一覧する(検証用)
    Sessions {
        /// ローカルセッションを一覧する
        #[arg(long)]
        local: bool,
    },
    /// ポーリングを 1 回だけ実行する(検証用)
    Poll {
        /// ローカル sessions.db を走査する
        #[arg(long)]
        local: bool,
    },
    /// 認証の疎通確認(GET /v3/self)
    Ping,
}

pub fn run() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Serve { source } => serve(source),
        Cmd::Send { prompt, local, cwd } => send(&prompt, local, cwd.as_deref()),
        Cmd::Decide { id, text } => decide(id, &text.join(" ")),
        Cmd::Setup => setup(),
        Cmd::Sessions { local } => sessions(local),
        Cmd::Poll { local } => poll(local),
        Cmd::Ping => ping(),
    }
}

fn open_store() -> Result<(Config, Arc<Store>)> {
    let cfg = Config::from_env()?;
    let store = Arc::new(Store::open(&cfg.db_path)?);
    Ok((cfg, store))
}

fn cloud(cfg: &Config) -> Result<Arc<DevinCloud>> {
    Ok(Arc::new(DevinCloud::new(cfg)?))
}

fn local(cfg: &Config) -> Arc<DevinLocal> {
    Arc::new(DevinLocal::new(cfg))
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

fn serve(source: Source) -> Result<()> {
    let (cfg, store) = open_store()?;
    let devin = match source {
        Source::Cloud | Source::Both => Some(cloud(&cfg)?),
        Source::Local => None,
    };
    let dl = match source {
        Source::Local | Source::Both => Some(local(&cfg)),
        Source::Cloud => None,
    };
    let (tx, _) = broadcast::channel::<String>(64);
    let notify = ntfy_target(&cfg);
    let interval = cfg.poll_interval_secs;
    if let Some(d) = &devin {
        let (d, store, tx, notify) = (d.clone(), store.clone(), tx.clone(), notify.clone());
        std::thread::spawn(move || {
            devin_cloud::run_poller(d, store, tx, interval, notify);
        });
    }
    if let Some(l) = &dl {
        let (l, store, tx, notify) = (l.clone(), store.clone(), tx.clone(), notify.clone());
        std::thread::spawn(move || {
            devin_local::run_poller(l, store, tx, interval, notify);
        });
    }
    let state = Arc::new(AppState {
        store,
        devin,
        local: dl,
        token: cfg.token.clone(),
        tx,
        notify,
    });
    tokio::runtime::Runtime::new()?.block_on(crate::server::serve(&cfg.bind, state))
}

fn send(prompt: &str, as_local: bool, cwd: Option<&std::path::Path>) -> Result<()> {
    let (cfg, store) = open_store()?;
    if as_local {
        let dl = local(&cfg);
        let cwd = cwd
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?)
            .canonicalize()?;
        let since = now();
        dl.spawn_session(&cwd, &format!("{prompt}\n\n{}", card::INSTRUCTION))?;
        match dl.wait_new_session(since, Duration::from_secs(15))? {
            Some(id) => {
                store.meta_set(&format!("local_owned:{id}"), "1")?;
                println!("local:{id}");
            }
            None => {
                println!("起動しました(session id は `andon sessions --local` で確認)");
            }
        }
        return Ok(());
    }
    let devin = cloud(&cfg)?;
    let note = note_id(&cfg, &store)?;
    let (id, url) = devin.create_session(prompt, note.as_deref(), cfg.bypass_approval)?;
    println!("{id}\n{url}");
    Ok(())
}

fn decide(id: i64, text: &str) -> Result<()> {
    let (cfg, store) = open_store()?;
    let devin = if cfg.api_key.is_some() {
        Some(cloud(&cfg)?)
    } else {
        None
    };
    let dl = local(&cfg);
    let message = answer::submit(&store, devin.as_deref(), Some(&dl), id, None, Some(text))?;
    println!("回答を送信しました: {message}");
    Ok(())
}

fn setup() -> Result<()> {
    let (cfg, store) = open_store()?;
    if let Some(existing) = store.meta_get("knowledge_note_id")? {
        println!(
            "既存の knowledge note: {existing}(再作成する場合は ANDON_KNOWLEDGE_NOTE_ID で上書き)"
        );
        return Ok(());
    }
    let devin = cloud(&cfg)?;
    let id = devin.provision_note()?;
    store.meta_set("knowledge_note_id", &id)?;
    println!("knowledge note を作成しました: {id}");
    Ok(())
}

fn sessions(as_local: bool) -> Result<()> {
    let (cfg, _store) = open_store()?;
    if as_local {
        for s in local(&cfg).list_sessions()? {
            let detail = s.status_detail.as_deref().unwrap_or("-");
            let title = s.title.as_deref().unwrap_or("(no title)");
            println!("{}\t{}/{}\t{}", s.session_id, s.status, detail, title);
        }
        return Ok(());
    }
    let devin = cloud(&cfg)?;
    for s in devin.list_sessions()? {
        let detail = s.status_detail.as_deref().unwrap_or("-");
        let title = s.title.as_deref().unwrap_or("(no title)");
        println!("{}\t{}/{}\t{}", s.session_id, s.status, detail, title);
    }
    Ok(())
}

fn poll(as_local: bool) -> Result<()> {
    let (cfg, store) = open_store()?;
    let (tx, _) = broadcast::channel::<String>(4);
    let notify = ntfy_target(&cfg);
    if as_local {
        devin_local::poll_once(&local(&cfg), &store, &tx, notify.as_ref())?;
    } else {
        devin_cloud::poll_once(&DevinCloud::new(&cfg)?, &store, &tx, notify.as_ref())?;
    }
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
    let (cfg, _store) = open_store()?;
    let v = cloud(&cfg)?.self_check()?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}
