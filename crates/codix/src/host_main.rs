//! `codix host`：实例进程的入口（architecture §8.7）。由 `codix new` / `start` 拉起，stdout 与
//! stderr 已接到 `host.log`。认领实例目录 → 起引擎 → 起会话代理 → 监听 socket → 写发现文件 →
//! 服务到被停 → 依次收尾。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use app::config;
use client::{InstanceFile, Layout};
use host::server::{ServeEnd, ServeOptions};
use host::{Hub, HubConfig};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;

pub async fn run(name: String, project: PathBuf) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(engine::log_filter(
            std::env::var("RUST_LOG").ok().as_deref(),
        ))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let (cfg, problems) = config::load();
    for p in problems {
        tracing::warn!("{p}");
    }
    let (settings, _) = cfg.settings();
    let layout = Layout::new(config::home());
    let project = project
        .canonicalize()
        .with_context(|| format!("project {}", project.display()))?;
    let project_str = project.to_string_lossy().into_owned();
    let claim = host::instance::claim(&layout, &name, Some(&project_str))?;
    let started_ms = host::instance::now_ms();
    let hostname = host::instance::hostname();
    let version = env!("CARGO_PKG_VERSION").to_string();

    let (embedded, client, notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: layout.home().to_path_buf(),
        project: project.clone(),
        groups_file: layout.groups(&name),
    })
    .await?;
    let clock: host::hub::Clock = Arc::new(host::instance::now_ms);
    let hub = Hub::start(
        Arc::new(client),
        notes,
        HubConfig {
            instance: name.clone(),
            host: hostname.clone(),
            project,
            version: version.clone(),
            started_ms,
            registry: host::hub::Registry::open(layout.sessions(&name)),
            jobs: Some(host::jobs::JobStore::open_in(&layout, &name)),
            clock,
        },
    )
    .await
    .context("connect to the engine")?;
    let flows = host::flows::start(
        &hub,
        host::flows::FlowsConfig {
            flows_dir: layout.flows(),
            runs_dir: layout.flow_runs(&name),
            worktrees_dir: layout.flow_worktrees(&name),
            workspaces_dir: layout.flow_workspaces(&name),
        },
    )
    .await
    .context("start the flows")?;

    let listener = claim.bind()?;
    let base = InstanceFile {
        name: name.clone(),
        pid: std::process::id(),
        host: hostname,
        project: Some(project_str),
        started_ms,
        version,
        holder: None,
    };
    claim.write_discovery(&base)?;
    tracing::info!(instance = %name, "serving");

    let (stop_tx, stop_rx) = watch::channel(None);
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    // 接住 SIGHUP 就不会被它的缺省动作结束：终端关掉不该带走实例。
    let mut hup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = term.recv() => break,
                _ = int.recv() => break,
                _ = hup.recv() => continue,
            }
        }
        let _ = stop_tx.send(Some("stopped".into()));
    });

    let mut opts = ServeOptions::new(stop_rx);
    opts.idle_timeout_ms = settings.idle_timeout_ms;
    // 只给 LIVE 测试用：不缩短的话，测心跳超时断开就得干等 35 秒。不进 codix.yaml，用户不该调它。
    if let Some(ms) = std::env::var("CODIX_TEST_HEARTBEAT_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        opts.heartbeat_timeout = std::time::Duration::from_millis(ms);
    }
    let disc_layout = layout.clone();
    let disc_name = name.clone();
    let end = host::server::serve(listener, Arc::new(hub.clone()), opts, move |holder| {
        let f = InstanceFile {
            holder: holder.cloned(),
            ..base.clone()
        };
        let _ = client::layout::write_json_atomic(&disc_layout.discovery(&disc_name), &f);
    })
    .await;
    match end {
        ServeEnd::StopRequested => tracing::info!("stop requested by the attached client"),
        ServeEnd::Shutdown(why) => tracing::info!("shutting down: {why}"),
    }
    flows.stop().await;
    hub.shutdown();
    embedded.shutdown().await;
    claim.release()?;
    Ok(())
}
