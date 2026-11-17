use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use app::core::{ShellEffect, Start};
use app::term::{self, CrosstermTerm, Term};
use app::{Core, config};
use clap::{Parser, Subcommand};
use crossterm::event::{
    Event as CtEvent, EventStream, KeyEventKind, MouseButton as CtButton, MouseEventKind,
};
use futures::StreamExt;
use model::{InstOp, Mouse, MouseButton, MouseKind, Msg};
use port::Instances;

#[derive(Parser)]
#[command(name = "codix", version, about = "跑在终端里的 agent 工作台")]
struct Cli {
    /// 用 mock 后端：内置场景名或场景文件路径。阶段 2 不带它也是 mock，打开演示 `demo`。
    #[arg(long, global = true)]
    mock: Option<String>,
    /// mock 时间线的加速倍数。
    #[arg(long, default_value_t = 1.0, global = true)]
    speed: f64,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 新建实例并接入。
    New {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// 列出本机的实例。
    List {
        #[arg(long)]
        all: bool,
    },
    /// 接入指定实例。
    Attach {
        name: String,
        /// 踢掉当前接入者。
        #[arg(long)]
        force: bool,
        /// 空闲多久自动脱离，如 30m、2h、0（不超时）。
        #[arg(long)]
        idle_timeout: Option<String>,
    },
    /// 启动一个已停止的实例。
    Start { name: String },
    /// 停止实例。
    Stop { name: String },
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // 真实实例在阶段 3 接入；在那之前直接打开 mock 的演示。
    let scenario = cli.mock.as_deref().unwrap_or("demo");
    let scenario = mock::load_scenario(scenario)
        .or_else(|_| mock::load_seed(scenario).map(|_| mock::Scenario::seed_only(scenario)))
        .with_context(|| format!("没有场景或种子 {scenario}"))?;
    let instances = mock::MockInstances::new(
        scenario,
        mock::MockOptions {
            speed: cli.speed,
            ..mock::MockOptions::default()
        },
    );
    let idle_override = match &cli.command {
        Some(Cmd::Attach {
            idle_timeout: Some(t),
            ..
        }) => Some(config::parse_duration_ms(t).with_context(|| format!("看不懂的时长 {t}"))?),
        _ => None,
    };
    if let Some(Cmd::List { all }) = cli.command {
        for i in instances.list(all).await? {
            println!(
                "{:<10} {:<8?} {:<16} 会话 {:<4} 等你 {:<3} {}",
                i.name,
                i.state,
                i.project.unwrap_or_default(),
                i.sessions,
                i.waiting,
                i.holder
                    .map(|h| format!("已被 {} 接入", h.label()))
                    .unwrap_or_default()
            );
        }
        return Ok(());
    }
    let start = match cli.command {
        Some(Cmd::New { name, project }) => Start::Op(InstOp::Create { name, project }),
        Some(Cmd::Attach { name, force, .. }) => Start::Op(InstOp::Attach { name, force }),
        Some(Cmd::Start { name }) => Start::Op(InstOp::Start { name }),
        Some(Cmd::Stop { name }) => {
            instances.stop(&name).await?;
            println!("已停止 {name}");
            return Ok(());
        }
        Some(Cmd::List { .. }) => unreachable!(),
        None => {
            let running = instances.list(false).await?;
            if running.is_empty() {
                Start::Op(InstOp::Create {
                    name: None,
                    project: None,
                })
            } else {
                Start::Picker
            }
        }
    };
    let (cfg, mut problems) = config::load();
    problems.extend(i18n::init(
        &config::language(&cfg),
        Some(&config::home().join("locales")),
    ));
    let (mut settings, more) = cfg.settings();
    problems.extend(more);
    if let Some(idle) = idle_override {
        settings.idle_timeout_ms = idle;
    }
    let cfg_err = (!problems.is_empty()).then(|| problems.join("；"));
    run(Arc::new(instances), start, cfg, cfg_err, settings).await
}

async fn run(
    instances: Arc<dyn Instances>,
    start: Start,
    cfg: config::Config,
    cfg_err: Option<String>,
    settings: model::Settings,
) -> Result<()> {
    let mut t = CrosstermTerm::new()?;
    let negotiated = term::negotiate(
        &mut t,
        cfg.ui.initial_size(),
        term::in_multiplexer(),
        Duration::from_millis(300),
    );
    install_panic_hook();
    t.enter(true)?;
    let result = event_loop(&mut t, instances, start, cfg_err, settings, true).await;
    t.leave()?;
    term::restore(&mut t, negotiated);
    result
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen
        );
        let _ = crossterm::terminal::disable_raw_mode();
        default(info);
    }));
}

const FRAME_BUDGET: Duration = Duration::from_millis(16);

async fn event_loop(
    t: &mut CrosstermTerm,
    instances: Arc<dyn Instances>,
    start: Start,
    cfg_err: Option<String>,
    settings: model::Settings,
    mock_mode: bool,
) -> Result<()> {
    let keymap = Arc::new(keys::Keymap::default());
    // mock 的数据以一个固定时刻为"现在"：交互运行时让时钟从那一刻起走，耗时、"下次触发"
    // 这些才对得上。真实实例用墙上时间。
    let begin = std::time::Instant::now();
    let epoch = mock::MockOptions::default().epoch_ms;
    let clock: Box<dyn Fn() -> u64 + Send> = if mock_mode {
        Box::new(move || epoch + begin.elapsed().as_millis() as u64)
    } else {
        Box::new(now_ms)
    };
    let (mut core, effects) = Core::new(instances, keymap, t.size(), clock, start, settings);
    if let Some(e) = cfg_err {
        core.handle(Msg::Event(Box::new(port::Event::Notice {
            level: port::NoticeLevel::Error,
            text: e,
        })));
    }
    let mut input = EventStream::new();
    let mut last_draw = Instant::now() - FRAME_BUDGET;
    let mut pending = effects;
    let mut dirty = false;
    loop {
        for e in pending.drain(..) {
            match e {
                ShellEffect::Quit { .. } => {
                    core.detach().await;
                    return Ok(());
                }
                ShellEffect::SetMouseCapture(on) => t.set_mouse(on)?,
                ShellEffect::Redraw => t.clear()?,
                ShellEffect::Bell => print!("\x07"),
            }
        }
        dirty |= core.take_dirty();
        let due = last_draw + FRAME_BUDGET;
        if dirty && Instant::now() >= due {
            t.draw(&core.frame())?;
            last_draw = Instant::now();
            dirty = false;
        }
        // 帧预算没到就记着脏、到点再画（合帧，architecture §5）；定时只按 model 给的最早时刻。
        let redraw_at = tokio::time::Instant::from_std(due);
        let tick_at = core.deadline().map(|d| {
            let wait = d.saturating_sub(core.now_ms());
            tokio::time::Instant::now() + Duration::from_millis(wait)
        });
        tokio::select! {
            ev = input.next() => match ev {
                Some(Ok(ev)) => pending.extend(on_terminal(&mut core, ev)),
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            },
            effects = core.next_msg() => pending.extend(effects),
            _ = tokio::time::sleep_until(tick_at.unwrap_or(redraw_at)), if tick_at.is_some() => {
                pending.extend(core.tick());
            }
            _ = tokio::time::sleep_until(redraw_at), if dirty => {}
        }
        pending.extend(core.pump());
    }
}

fn on_terminal(core: &mut Core, ev: CtEvent) -> Vec<ShellEffect> {
    match ev {
        CtEvent::Key(k) if k.kind != KeyEventKind::Release => match keys::Key::from_crossterm(k) {
            Some(key) => core.key(key),
            None => vec![],
        },
        CtEvent::Mouse(m) => {
            let button = |b: CtButton| match b {
                CtButton::Left => MouseButton::Left,
                CtButton::Right => MouseButton::Right,
                CtButton::Middle => MouseButton::Middle,
            };
            let kind = match m.kind {
                MouseEventKind::Down(b) => MouseKind::Down(button(b)),
                MouseEventKind::Up(b) => MouseKind::Up(button(b)),
                MouseEventKind::Drag(b) => MouseKind::Drag(button(b)),
                MouseEventKind::ScrollUp => MouseKind::ScrollUp,
                MouseEventKind::ScrollDown => MouseKind::ScrollDown,
                _ => return vec![],
            };
            core.mouse(Mouse {
                kind,
                col: m.column,
                row: m.row,
            })
        }
        CtEvent::Resize(c, r) => core.resize(tui::Size::new(c, r)),
        CtEvent::Paste(s) => core.handle(Msg::Paste(s)),
        _ => vec![],
    }
}
