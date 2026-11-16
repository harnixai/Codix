use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use app::core::ShellEffect;
use app::term::{self, CrosstermTerm, Term};
use app::{Core, config};
use clap::{Parser, Subcommand};
use crossterm::event::{
    Event as CtEvent, EventStream, KeyEventKind, MouseButton as CtButton, MouseEventKind,
};
use futures::StreamExt;
use model::{Mouse, MouseButton, MouseKind, Msg};
use port::{Attach, Instances};

#[derive(Parser)]
#[command(name = "codix", version, about = "跑在终端里的 agent 工作台")]
struct Cli {
    /// 用 mock 后端：内置场景名（如 default）或场景文件路径。
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
        #[arg(long)]
        force: bool,
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
    let Some(scenario) = cli.mock.as_deref() else {
        eprintln!("真实实例在阶段 3 接入；现在请用 mock：codix --mock default");
        std::process::exit(2);
    };
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
    let backend = match cli.command {
        Some(Cmd::New { name, project }) => {
            instances
                .create(port::NewInstance {
                    name: name.map(Into::into),
                    project,
                })
                .await?
        }
        Some(Cmd::Attach { name, force }) => {
            instances
                .attach(
                    &name,
                    Attach {
                        force,
                        idle_timeout_ms: None,
                    },
                )
                .await?
        }
        Some(Cmd::Start { name }) => instances.start(&name).await?,
        Some(Cmd::Stop { name }) => {
            instances.stop(&name).await?;
            println!("已停止 {name}");
            return Ok(());
        }
        // 2.3 起：有在跑的实例时先弹选择列表。
        _ => instances.attach("dev", Attach::default()).await?,
    };
    run(backend).await
}

async fn run(backend: Arc<dyn port::Backend>) -> Result<()> {
    let (cfg, cfg_err) = config::load();
    let mut t = CrosstermTerm::new()?;
    let negotiated = term::negotiate(
        &mut t,
        cfg.ui.initial_size(),
        term::in_multiplexer(),
        Duration::from_millis(300),
    );
    install_panic_hook();
    t.enter(true)?;
    let result = event_loop(&mut t, backend, cfg_err).await;
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
const TICK: Duration = Duration::from_millis(250);

async fn event_loop(
    t: &mut CrosstermTerm,
    backend: Arc<dyn port::Backend>,
    cfg_err: Option<String>,
) -> Result<()> {
    let keymap = Arc::new(keys::Keymap::default());
    let (mut core, effects) = Core::new(backend, keymap, t.size(), Box::new(now_ms));
    if let Some(e) = cfg_err {
        core.handle(Msg::Event(Box::new(port::Event::Notice {
            level: port::NoticeLevel::Error,
            text: e,
        })));
    }
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    let mut last_draw = Instant::now() - FRAME_BUDGET;
    let mut pending = effects;
    loop {
        for e in pending.drain(..) {
            match e {
                ShellEffect::Quit { .. } => return Ok(()),
                ShellEffect::SetMouseCapture(on) => t.set_mouse(on)?,
                ShellEffect::Redraw => t.clear()?,
                ShellEffect::Bell => print!("\x07"),
            }
        }
        let due = last_draw + FRAME_BUDGET;
        let dirty = core.take_dirty();
        if dirty && Instant::now() >= due {
            t.draw(&core.frame())?;
            last_draw = Instant::now();
        } else if dirty {
            // 帧预算没到：记着脏，等到点再画（合帧，architecture §5）。
            core.handle(Msg::Tick);
        }
        let redraw_at = tokio::time::Instant::from_std(due);
        let need_redraw = dirty && Instant::now() < due;
        tokio::select! {
            ev = input.next() => match ev {
                Some(Ok(ev)) => pending.extend(on_terminal(&mut core, ev)),
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            },
            msg = core.next_backend_msg() => {
                if let Some(m) = msg {
                    pending.extend(core.handle(m));
                }
            }
            _ = tick.tick(), if core.wants_tick() => pending.extend(core.tick()),
            _ = tokio::time::sleep_until(redraw_at), if need_redraw => {}
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
