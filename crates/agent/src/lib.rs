//! The background agent that shares the keyboard and mouse. It is not a
//! separate program: the Nexpingdesk app runs it in a second copy of itself
//! (`nexpingdesk --agent`) and talks to it over a private local socket.
//!
//! Entry points used by the app's `main`: [`run_agent`], [`run_selftest`],
//! [`shutdown_running_agent`].

mod agent;

/// App version (shared by the app and the agent — they are the same executable).
pub const VERSION: &str = agent::VERSION;
mod ipc_server;
mod selftest;
mod state;

use std::process::ExitCode;
use std::time::Duration;

use nexpingdesk_config::{ConfigStore, LogLevel};
use nexpingdesk_ipc::{AgentClient, AgentStatus, Endpoint, Request};
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

fn level(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
        LogLevel::Trace => "trace",
    }
}

/// Rotating log files (5 × daily) + stderr in debug builds.
fn init_logging(dir: &std::path::Path, lvl: LogLevel) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_from_env("NEXPINGDESK_LOG")
        .unwrap_or_else(|_| EnvFilter::new(format!("warn,nexpingdesk={0},nexpingdesk_agent={0}", level(lvl))));
    let _ = std::fs::create_dir_all(dir);
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("agent")
        .filename_suffix("log")
        .max_log_files(5)
        .build(dir)
        .ok();
    let (file_layer, guard) = match appender {
        Some(a) => {
            let (nb, guard) = tracing_appender::non_blocking(a);
            (Some(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(nb)), Some(guard))
        }
        None => (None, None),
    };
    let stderr = cfg!(debug_assertions).then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    let _ = tracing_subscriber::registry().with(filter).with(file_layer).with(stderr).try_init();
    guard
}

async fn shutdown_running() -> ExitCode {
    let Ok(ep) = Endpoint::default_for_user() else { return ExitCode::SUCCESS };
    let Ok((client, _events)) = AgentClient::connect(&ep).await else { return ExitCode::SUCCESS };
    let _ = client.request(Request::Quit).await;
    // Wait until the socket is gone (agent exited) — max 10 s.
    for _ in 0..100 {
        if AgentClient::connect(&ep).await.is_err() {
            return ExitCode::SUCCESS;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    ExitCode::FAILURE
}

/// Prints the self-test report as JSON (`nexpingdesk --selftest`).
pub fn run_selftest() -> ExitCode {
    nexpingdesk_input::init_process();
    let report = selftest::run(false);
    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    ExitCode::SUCCESS
}

/// Asks a running agent to quit and waits (`nexpingdesk --shutdown`; used by installers).
pub fn shutdown_running_agent() -> ExitCode {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return ExitCode::FAILURE };
    rt.block_on(shutdown_running())
}

/// Runs the agent until it is told to quit (`nexpingdesk --agent`).
pub fn run_agent() -> ExitCode {
    nexpingdesk_input::init_process();
    let Ok(rt) =
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("nd-agent").build()
    else {
        eprintln!("cannot start async runtime");
        return ExitCode::FAILURE;
    };

    let store = match ConfigStore::default_location() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let log_dir = nexpingdesk_platform::log_dir();
    let peek_level = store_peek_level(&store);
    let _guard = init_logging(&log_dir, peek_level);
    info!(version = agent::VERSION, "agent starting");

    rt.block_on(async move {
        let ep = match Endpoint::default_for_user() {
            Ok(e) => e,
            Err(e) => {
                error!(error = %e, "no IPC endpoint");
                return ExitCode::FAILURE;
            }
        };
        let listener = match nexpingdesk_ipc::transport::Listener::bind(&ep).await {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                info!("another agent is already running");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                error!(error = %e, "cannot open IPC endpoint");
                return ExitCode::FAILURE;
            }
        };
        let (status_tx, status_rx) = watch::channel(AgentStatus::default());
        let (events_tx, _) = broadcast::channel(64);
        let (calls_tx, calls_rx) = mpsc::channel(64);
        let agent = match agent::Agent::new(store, status_tx, events_tx.clone(), log_dir.display().to_string()) {
            Ok(a) => a,
            Err(e) => {
                error!(error = %e, "cannot load settings");
                return ExitCode::FAILURE;
            }
        };
        ipc_server::spawn(listener, calls_tx, status_rx, events_tx);
        agent.run(calls_rx).await;
        info!("agent stopped");
        ExitCode::SUCCESS
    })
}

/// Log level from the config file without keeping the store borrowed.
fn store_peek_level(store: &ConfigStore) -> LogLevel {
    std::fs::read_to_string(store.path())
        .ok()
        .and_then(|t| t.parse::<toml_peek::Table>().ok())
        .and_then(|t| t.get("general")?.get("log_level")?.as_str().map(str::to_owned))
        .and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok())
        .unwrap_or_default()
}

mod toml_peek {
    pub use nexpingdesk_config::__toml::Table;
}
