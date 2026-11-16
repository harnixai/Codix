//! 不碰终端的事件循环核心（architecture §5）：持有 State、键位解析器和后端，
//! 把输入翻译成 Msg、跑 update、执行 Effect。真实终端的读写在 `main.rs`，测试直接驱动它。

use std::sync::Arc;

use keys::{Key, Keymap, Resolution, Resolver};
use model::{Effect, Mouse, Msg, ReqId, State};
use port::{Backend, Event, PortError, Reply};
use tokio::sync::mpsc;
use tui::{FrameState, Size};

/// `Core` 自己处理不了、要交给终端外壳的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEffect {
    Quit { detach: bool },
    SetMouseCapture(bool),
    Redraw,
    Bell,
}

type ReplyMsg = (ReqId, Result<Reply, PortError>);

pub struct Core {
    pub state: State,
    resolver: Resolver,
    backend: Arc<dyn Backend>,
    events: mpsc::UnboundedReceiver<Event>,
    replies_tx: mpsc::UnboundedSender<ReplyMsg>,
    replies: mpsc::UnboundedReceiver<ReplyMsg>,
    clock: Box<dyn Fn() -> u64 + Send>,
    dirty: bool,
    /// 测试用计数：画了几帧（testing.md §1「一次排空最多画一帧」）。
    pub frames: u64,
}

impl Core {
    pub fn new(
        backend: Arc<dyn Backend>,
        keymap: Arc<Keymap>,
        size: Size,
        clock: Box<dyn Fn() -> u64 + Send>,
    ) -> (Core, Vec<ShellEffect>) {
        let now = clock();
        let (replies_tx, replies) = mpsc::unbounded_channel();
        let events = backend.events();
        let mut core = Core {
            state: State::new(keymap, size, now),
            resolver: Resolver::default(),
            backend,
            events,
            replies_tx,
            replies,
            clock,
            dirty: true,
            frames: 0,
        };
        let effects = core.state.start();
        let shell = core.run(effects);
        (core, shell)
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    pub fn handle(&mut self, msg: Msg) -> Vec<ShellEffect> {
        self.state.set_now(self.now());
        let effects = model::update(&mut self.state, msg);
        self.dirty = true;
        self.run(effects)
    }

    pub fn key(&mut self, key: Key) -> Vec<ShellEffect> {
        let now = self.now();
        let contexts = self.state.key_contexts();
        let keymap = self.state.keymap.clone();
        match self.resolver.resolve(&keymap, &contexts, key, now) {
            Resolution::Action(a) => self.handle(Msg::Action(a)),
            Resolution::Text(c) => self.handle(Msg::Text(c)),
            Resolution::Pending | Resolution::Unbound => vec![],
        }
    }

    pub fn mouse(&mut self, m: Mouse) -> Vec<ShellEffect> {
        self.handle(Msg::Mouse(m))
    }

    pub fn resize(&mut self, size: Size) -> Vec<ShellEffect> {
        self.handle(Msg::Resize(size))
    }

    pub fn tick(&mut self) -> Vec<ShellEffect> {
        self.handle(Msg::Tick)
    }

    /// 把这一刻已经到达的后端事件与命令结果全部处理掉（不等待）。
    pub fn pump(&mut self) -> Vec<ShellEffect> {
        let mut out = vec![];
        while let Ok(ev) = self.events.try_recv() {
            out.extend(self.handle(Msg::Event(Box::new(ev))));
        }
        while let Ok((id, r)) = self.replies.try_recv() {
            out.extend(self.handle(Msg::Reply(id, Box::new(r))));
        }
        out
    }

    /// 等下一件事：后端事件或命令结果。给真实循环的 `select!` 用。
    pub async fn next_backend_msg(&mut self) -> Option<Msg> {
        tokio::select! {
            ev = self.events.recv() => ev.map(|e| Msg::Event(Box::new(e))),
            r = self.replies.recv() => r.map(|(id, r)| Msg::Reply(id, Box::new(r))),
        }
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn frame(&mut self) -> FrameState {
        self.frames += 1;
        model::view(&self.state)
    }

    pub fn wants_tick(&self) -> bool {
        self.state.ui.notice.is_some() || self.state.ui.quit_armed_ms.is_some()
    }

    fn run(&mut self, effects: Vec<Effect>) -> Vec<ShellEffect> {
        let mut shell = vec![];
        for e in effects {
            match e {
                Effect::Call(id, cmd) => {
                    let backend = self.backend.clone();
                    let tx = self.replies_tx.clone();
                    tokio::spawn(async move {
                        let r = backend.call(*cmd).await;
                        let _ = tx.send((id, r));
                    });
                }
                Effect::Quit { detach } => shell.push(ShellEffect::Quit { detach }),
                Effect::SetMouseCapture(on) => shell.push(ShellEffect::SetMouseCapture(on)),
                Effect::Redraw => shell.push(ShellEffect::Redraw),
                Effect::Bell => shell.push(ShellEffect::Bell),
            }
        }
        shell
    }
}
