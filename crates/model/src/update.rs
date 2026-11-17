use keys::Action;
use port::{Command, ConnState, Event, PortError, Reply};
use tui::layout::{self, ShowPane, Size};

use crate::mouse::{Mouse, MouseButton, MouseKind};
use crate::state::{
    Confirm, DetachReason, Drag, Focus, HEARTBEAT_MS, IDLE_WARN_MS, Notice, Overlay, Picker,
    Screen, State, Tab,
};
use crate::view::{layout_spec, tab_labels};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReqId(pub u64);

#[derive(Debug, Clone)]
pub enum Msg {
    Action(Action),
    /// 输入框里的普通字符。
    Text(char),
    Paste(String),
    Mouse(Mouse),
    Event(Box<Event>),
    Reply(ReqId, Box<Result<Reply, PortError>>),
    Tick,
    Resize(Size),
    /// 实例目录操作的结果（`Effect::Instances` 回来的）。
    Instances(InstOp, Result<InstOk, PortError>),
}

/// 对本机实例目录的操作，由 app 交给 `port::Instances` 执行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstOp {
    List,
    Attach {
        name: String,
        force: bool,
    },
    Start {
        name: String,
    },
    /// `project` 为空时由 app 用当前目录。
    Create {
        name: Option<String>,
        project: Option<String>,
    },
    Stop {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum InstOk {
    Listed(Vec<port::InstanceInfo>),
    /// 已接上这个实例：app 已换上它的后端。
    Attached(String),
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Call(ReqId, Box<Command>),
    /// 退出 TUI；`detach` 为真表示实例继续运行（I-05）。
    Quit {
        detach: bool,
    },
    SetMouseCapture(bool),
    Redraw,
    Bell,
    Instances(InstOp),
    /// 放下当前后端：被踢、空闲脱离、实例停了之后不再收它的事件。
    Release,
}

/// 发出去还没回来的命令，回来时据此知道把结果放到哪里。
#[derive(Debug, Clone)]
pub(crate) enum Pending {
    Ignore,
    Hello,
    Sessions,
    Flows,
    Jobs,
    Watch(port::SessionId),
    Commands(port::SessionId),
    /// 插话时要拿到消息 id，等 `SteerDelivered` / `SteerReturned` 对上。
    Submitted {
        session: port::SessionId,
        block: u64,
    },
    /// 还没有会话时第一句话：先建会话，建好再发（与 Claude Code 一进来就能说话一致）。
    CreateThenSubmit {
        text: String,
    },
    CreateOnly,
    TaskOutput(port::TaskId),
    Runs(String),
    FlowDetail(String),
    RunView {
        id: port::RunId,
        follow: bool,
    },
    JobHistory(port::JobId),
    /// 流程 / 作业的编辑会话（`flow:<名字>`、`job:<id>`）。
    Authoring {
        key: String,
        follow: bool,
    },
}

const RATIO_MIN: u16 = 40;
const RATIO_MAX: u16 = 75;
const RATIO_STEP: u16 = 5;
const DOUBLE_CLICK_MS: u64 = 400;
const QUIT_ARM_MS: u64 = 2_000;
const NOTICE_MS: u64 = 3_000;
/// 状态行 `✻` 动画的帧间隔：只在有一轮在跑时唤醒。
pub(crate) const SPINNER_MS: u64 = 150;

impl State {
    /// 接入实例之后的第一批命令。
    pub fn start(&mut self) -> Vec<Effect> {
        let client = port::ClientInfo {
            user: std::env::var("USER").unwrap_or_else(|_| "me".into()),
            pid: std::process::id(),
            tty: None,
            ssh_from: None,
            attached_ms: self.ui.now_ms,
            last_input_ms: self.ui.now_ms,
        };
        vec![
            self.call(Pending::Hello, Command::Hello { client }),
            self.call(
                Pending::Sessions,
                Command::ListSessions {
                    filter: port::SessionFilter::default(),
                },
            ),
            self.call(Pending::Flows, Command::ListFlows),
            self.call(Pending::Jobs, Command::ListJobs),
        ]
    }

    /// 启动时有在跑的实例：先显示实例选择（I-01）。
    pub fn open_picker(&mut self, message: Option<String>) -> Vec<Effect> {
        self.ui.screen = Screen::Instances(Picker {
            loading: true,
            message,
            ..Picker::default()
        });
        self.ui.confirm = None;
        vec![Effect::Instances(InstOp::List)]
    }

    /// 最早的下一个定时：通知过期、Ctrl+C 失效、空闲提醒与脱离、心跳。app 据此安排 Tick，
    /// 没有定时就不唤醒（architecture §5）。
    pub fn next_deadline(&self) -> Option<u64> {
        let u = &self.ui;
        let mut d: Vec<u64> = vec![];
        if let Some(n) = &u.notice {
            d.push(n.until_ms);
        }
        if let Some(t) = u.quit_armed_ms {
            d.push(t + QUIT_ARM_MS + 1);
        }
        if u.screen == Screen::Workbench {
            d.push(u.next_heartbeat_ms);
            if self.conv().is_some_and(|c| c.running) {
                d.push((u.now_ms / SPINNER_MS + 1) * SPINNER_MS);
            }
            if u.idle_timeout_ms > 0 {
                let at = u.last_input_ms + u.idle_timeout_ms;
                d.push(if u.idle_warned {
                    at
                } else {
                    at.saturating_sub(IDLE_WARN_MS)
                });
            }
        }
        d.into_iter().min()
    }

    pub fn set_now(&mut self, now_ms: u64) {
        self.ui.now_ms = now_ms;
    }

    pub(crate) fn call(&mut self, what: Pending, cmd: Command) -> Effect {
        self.log(format!("→ {}", cmd.name()));
        let id = ReqId(self.next_req);
        self.next_req += 1;
        self.pending.insert(id, what);
        Effect::Call(id, Box::new(cmd))
    }

    pub(crate) fn log(&mut self, line: String) {
        let at = crate::panel::fmt_clock(self.ui.now_ms);
        self.ui.event_log.push_back(format!("{at}  {line}"));
        while self.ui.event_log.len() > 500 {
            self.ui.event_log.pop_front();
        }
    }

    pub(crate) fn notice(&mut self, text: impl Into<String>, error: bool) {
        self.ui.notice = Some(Notice {
            text: text.into(),
            error,
            until_ms: self.ui.now_ms + if error { 8_000 } else { NOTICE_MS },
        });
    }

    /// 把焦点放进右栏，必要时让右栏显示出来。
    fn show_side(&mut self, focus: Focus) {
        self.ui.side_collapsed = false;
        self.ui.showing = ShowPane::Side;
        self.ui.focus = focus;
    }

    pub(crate) fn show_chat_pub(&mut self) {
        self.show_chat();
    }

    fn show_chat(&mut self) {
        self.ui.showing = ShowPane::Chat;
        if self.ui.focus.in_side() {
            self.ui.focus = Focus::Composer;
        }
    }
}

pub fn update(s: &mut State, msg: Msg) -> Vec<Effect> {
    if matches!(
        msg,
        Msg::Action(_) | Msg::Text(_) | Msg::Paste(_) | Msg::Mouse(_)
    ) {
        s.ui.last_input_ms = s.ui.now_ms;
        s.ui.idle_warned = false;
    }
    let mut effects = update_confirm(s, msg);
    effects.extend(crate::follow::after_update(s));
    fix_scroll(s);
    crate::chat_update::relayout(s);
    effects
}

fn update_confirm(s: &mut State, msg: Msg) -> Vec<Effect> {
    if let Some(c) = s.ui.confirm.take() {
        return match msg {
            Msg::Action(Action::Yes | Action::Confirm) => confirmed(s, c),
            Msg::Action(_) | Msg::Text(_) => {
                s.notice("已放弃", false);
                vec![]
            }
            other => {
                s.ui.confirm = Some(c);
                update_inner(s, other)
            }
        };
    }
    update_inner(s, msg)
}

fn update_inner(s: &mut State, msg: Msg) -> Vec<Effect> {
    match (&s.ui.screen, msg) {
        (_, Msg::Instances(op, r)) => instances_done(s, op, r),
        (Screen::Instances(_), Msg::Action(a)) => picker_action(s, a),
        (Screen::Instances(_), Msg::Mouse(m)) => picker_mouse(s, m),
        (Screen::Detached(reason), Msg::Action(a)) => {
            let reason = reason.clone();
            detached_action(s, &reason, Some(a))
        }
        (Screen::Detached(reason), Msg::Text(_)) => {
            let reason = reason.clone();
            detached_action(s, &reason, None)
        }
        (Screen::Workbench, msg) => workbench(s, msg),
        (_, Msg::Resize(size)) => {
            s.ui.size = size;
            vec![]
        }
        (_, Msg::Tick) => {
            tick_common(s);
            vec![]
        }
        _ => vec![],
    }
}

fn workbench(s: &mut State, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Instances(..) => vec![],
        Msg::Action(a) => action(s, a),
        Msg::Text(c) if s.ui.filtering => {
            s.ui.filter[s.ui.tab.index()].push(c);
            s.ui.upper_mut().selected = 0;
            vec![]
        }
        Msg::Text(c) => {
            s.ui.quit_armed_ms = None;
            if s.ui.focus == Focus::Composer {
                crate::chat_update::text(s, c)
            } else {
                vec![]
            }
        }
        Msg::Paste(p) => {
            s.ui.quit_armed_ms = None;
            s.ui.focus = Focus::Composer;
            s.ui.composer.paste(&p);
            vec![]
        }
        Msg::Mouse(m) => mouse(s, m),
        Msg::Event(e) => event(s, *e),
        Msg::Reply(id, r) => reply(s, id, *r),
        Msg::Tick => {
            tick_common(s);
            idle_and_heartbeat(s)
        }
        Msg::Resize(size) => {
            s.ui.size = size;
            vec![]
        }
    }
}

fn tick_common(s: &mut State) {
    let now = s.ui.now_ms;
    if s.ui.notice.as_ref().is_some_and(|n| n.until_ms <= now) {
        s.ui.notice = None;
    }
    if s.ui
        .quit_armed_ms
        .is_some_and(|t| now.saturating_sub(t) > QUIT_ARM_MS)
    {
        s.ui.quit_armed_ms = None;
    }
}

/// 空闲脱离与心跳（需求 I-10、I-11）。
fn idle_and_heartbeat(s: &mut State) -> Vec<Effect> {
    let now = s.ui.now_ms;
    let mut out = vec![];
    let timeout = s.ui.idle_timeout_ms;
    if timeout > 0 {
        let idle = now.saturating_sub(s.ui.last_input_ms);
        if idle >= timeout {
            out.push(s.call(Pending::Ignore, Command::Detach));
            out.push(Effect::Release);
            s.ui.screen = Screen::Detached(DetachReason::Idle { after_ms: idle });
            s.ui.overlay = None;
            return out;
        }
        if idle + IDLE_WARN_MS >= timeout && !s.ui.idle_warned {
            s.ui.idle_warned = true;
            let left = (timeout - idle).div_ceil(60_000);
            s.ui.notice = Some(Notice {
                text: format!("{left} 分钟后因空闲脱离（有任何输入就不会）"),
                error: false,
                until_ms: s.ui.last_input_ms + timeout,
            });
        }
    }
    if now >= s.ui.next_heartbeat_ms {
        s.ui.next_heartbeat_ms = now + HEARTBEAT_MS;
        let last = s.ui.last_input_ms;
        out.push(s.call(
            Pending::Ignore,
            Command::Heartbeat {
                last_input_ms: last,
            },
        ));
    }
    out
}

/// 退出框的三个选项（交互 §7）：0 后台运行（脱离，实例继续）、1 直接退出（停实例再退）、2 返回。
fn exit_choice(s: &mut State, c: usize) -> Vec<Effect> {
    s.ui.overlay = None;
    match (c, s.ui.attached.clone()) {
        (0, _) | (1, None) => vec![Effect::Quit { detach: true }],
        (1, Some(name)) => {
            s.ui.quitting = true;
            s.notice(format!("正在停止实例 {name} …"), false);
            vec![Effect::Instances(InstOp::Stop { name })]
        }
        _ => vec![],
    }
}

fn confirmed(s: &mut State, c: Confirm) -> Vec<Effect> {
    match c {
        Confirm::ForceAttach(name) => {
            set_picker_message(s, format!("正在强制接入 {name} …"));
            vec![Effect::Instances(InstOp::Attach { name, force: true })]
        }
        Confirm::StopInstance(name) => {
            set_picker_message(s, format!("正在停止 {name} …"));
            if s.ui.screen == Screen::Workbench {
                s.ui.quitting = true;
            }
            vec![Effect::Instances(InstOp::Stop { name })]
        }
        Confirm::CloseSession(id, _) => {
            vec![s.call(Pending::Ignore, Command::Close { session: id })]
        }
        Confirm::StopTask(task, _) => vec![s.call(Pending::Ignore, Command::StopTask { task })],
        Confirm::CancelRun(run, _) => vec![s.call(
            Pending::Ignore,
            Command::RunOp {
                run,
                op: port::RunOp::Cancel,
            },
        )],
        Confirm::ForceStation(run, station, _) => vec![s.call(
            Pending::Ignore,
            Command::RunOp {
                run,
                op: port::RunOp::Force {
                    station,
                    reason: "在 Codix 里手动强制放行".into(),
                },
            },
        )],
        Confirm::Rework(run, to, _) => vec![s.call(
            Pending::Ignore,
            Command::RunOp {
                run,
                op: port::RunOp::Rework { to },
            },
        )],
        Confirm::ClearGoal(id) => vec![s.call(
            Pending::Ignore,
            Command::UpdateGoal {
                session: id,
                action: port::GoalAction::Clear,
            },
        )],
    }
}

fn set_picker_message(s: &mut State, m: String) {
    if let Screen::Instances(p) = &mut s.ui.screen {
        p.message = Some(m);
    }
}

fn picker_action(s: &mut State, a: Action) -> Vec<Effect> {
    let Screen::Instances(p) = &mut s.ui.screen else {
        return vec![];
    };
    let rows = p.list.len() + 1;
    let chosen = p
        .selected
        .checked_sub(1)
        .and_then(|i| p.list.get(i))
        .cloned();
    match a {
        Action::Up | Action::Down => {
            p.selected = if a == Action::Up {
                p.selected.saturating_sub(1)
            } else {
                (p.selected + 1).min(rows - 1)
            };
            let n = crate::state::PICKER_ROWS;
            if p.selected < p.scroll {
                p.scroll = p.selected;
            } else if p.selected >= p.scroll + n {
                p.scroll = p.selected + 1 - n;
            }
        }
        Action::Quit | Action::Exit => return vec![Effect::Quit { detach: true }],
        Action::Help => s.ui.overlay = Some(Overlay::Help { scroll: 0 }),
        Action::New => return create(s),
        Action::Open => match chosen {
            None => return create(s),
            Some(i) => match i.state {
                port::InstanceState::Running if i.holder.is_some() => {
                    s.ui.confirm = Some(Confirm::ForceAttach(i.name.0));
                }
                port::InstanceState::Running => {
                    p.message = Some(format!("正在接入 {} …", i.name));
                    return vec![Effect::Instances(InstOp::Attach {
                        name: i.name.0,
                        force: false,
                    })];
                }
                _ => {
                    p.message = Some(format!("正在启动 {} …", i.name));
                    return vec![Effect::Instances(InstOp::Start { name: i.name.0 })];
                }
            },
        },
        Action::Force => {
            if let Some(i) = chosen.filter(|i| i.state == port::InstanceState::Running) {
                s.ui.confirm = Some(Confirm::ForceAttach(i.name.0));
            }
        }
        Action::Stop => {
            if let Some(i) = chosen.filter(|i| i.state == port::InstanceState::Running) {
                s.ui.confirm = Some(Confirm::StopInstance(i.name.0));
            }
        }
        _ => {}
    }
    vec![]
}

/// 实例选择里的鼠标：点一行选中，双击打开，滚轮上下（需求 M-02、M-03）。
fn picker_mouse(s: &mut State, m: Mouse) -> Vec<Effect> {
    let now = s.ui.now_ms;
    let Screen::Instances(p) = &s.ui.screen else {
        return vec![];
    };
    let n = (p.list.len() + 1).min(crate::state::PICKER_ROWS) as u16;
    let scroll = p.scroll;
    let b = layout::instances_box(s.ui.size, n);
    let list = layout::Rect::new(b.x + 2, b.y + 2, b.w.saturating_sub(4), n);
    match m.kind {
        MouseKind::ScrollUp => picker_action(s, Action::Up),
        MouseKind::ScrollDown => picker_action(s, Action::Down),
        MouseKind::Down(MouseButton::Left) if list.contains(m.col, m.row) => {
            let double = s
                .ui
                .last_click
                .is_some_and(|(c, r, t)| c == m.col && r == m.row && now - t <= DOUBLE_CLICK_MS);
            s.ui.last_click = Some((m.col, m.row, now));
            if let Screen::Instances(p) = &mut s.ui.screen {
                p.selected = scroll + (m.row - list.y) as usize;
            }
            if double {
                return picker_action(s, Action::Open);
            }
            vec![]
        }
        _ => vec![],
    }
}

fn create(s: &mut State) -> Vec<Effect> {
    set_picker_message(s, "正在新建实例 …".into());
    vec![Effect::Instances(InstOp::Create {
        name: None,
        project: None,
    })]
}

fn detached_action(s: &mut State, reason: &DetachReason, a: Option<Action>) -> Vec<Effect> {
    if a == Some(Action::Quit) || a == Some(Action::Detach) {
        return vec![Effect::Quit { detach: true }];
    }
    match (reason, &s.ui.attached) {
        (DetachReason::Idle { .. }, Some(name)) => {
            vec![Effect::Instances(InstOp::Attach {
                name: name.clone(),
                force: false,
            })]
        }
        _ if a == Some(Action::Open) || a.is_none() => s.open_picker(None),
        _ => vec![],
    }
}

fn instances_done(s: &mut State, op: InstOp, r: Result<InstOk, PortError>) -> Vec<Effect> {
    match r {
        Ok(InstOk::Listed(mut list)) => {
            list.sort_by_key(|i| (i.state != port::InstanceState::Running, i.name.clone()));
            let Screen::Instances(p) = &mut s.ui.screen else {
                return vec![];
            };
            // 第一次列出来时选中第一个实例（"新建"在它上面一行）。
            if p.loading && p.list.is_empty() && !list.is_empty() {
                p.selected = 1;
            }
            p.loading = false;
            p.selected = p.selected.min(list.len());
            p.list = list;
            vec![]
        }
        Ok(InstOk::Attached(name)) => {
            s.domain = Default::default();
            s.pending.clear();
            s.ui.chat = None;
            s.ui.ask_active = false;
            s.ui.answering = None;
            s.ui.screen = Screen::Workbench;
            s.ui.attached = Some(name);
            s.ui.last_input_ms = s.ui.now_ms;
            s.ui.next_heartbeat_ms = s.ui.now_ms + HEARTBEAT_MS;
            s.ui.focus = Focus::Composer;
            s.start()
        }
        Ok(InstOk::Stopped) if s.ui.quitting => vec![Effect::Quit { detach: false }],
        Ok(InstOk::Stopped) => {
            if matches!(s.ui.screen, Screen::Instances(_)) {
                set_picker_message(s, "已停止".into());
                vec![Effect::Instances(InstOp::List)]
            } else {
                vec![]
            }
        }
        Err(e) => {
            let text = match (&op, &e) {
                (InstOp::Attach { name, .. }, PortError::Occupied { holder, idle_ms }) => format!(
                    "{name} 已被 {} 接入（空闲 {}）· f 强制接入",
                    holder.label(),
                    crate::view::fmt_duration(*idle_ms)
                ),
                _ => e.to_string(),
            };
            if matches!(s.ui.screen, Screen::Instances(_)) {
                set_picker_message(s, text);
                vec![Effect::Instances(InstOp::List)]
            } else {
                s.open_picker(Some(text))
            }
        }
    }
}

fn action(s: &mut State, a: Action) -> Vec<Effect> {
    if a != Action::Interrupt {
        s.ui.quit_armed_ms = None;
    }
    if let Some(Overlay::Exit { selected }) = &mut s.ui.overlay {
        let choice = match a {
            Action::Up => {
                *selected = selected.saturating_sub(1);
                None
            }
            Action::Down => {
                *selected = (*selected + 1).min(2);
                None
            }
            Action::Confirm | Action::Open | Action::Submit => Some(*selected),
            Action::Back | Action::No => Some(2),
            a => a.choice_index().filter(|i| *i < 3),
        };
        return match choice {
            Some(c) => exit_choice(s, c),
            None => vec![],
        };
    }
    if let Some(Overlay::Text { scroll, .. } | Overlay::EventLog { scroll }) = &mut s.ui.overlay {
        match a {
            Action::Up | Action::LineUp => *scroll = scroll.saturating_sub(1),
            Action::Down | Action::LineDown => *scroll += 1,
            Action::PageUp => *scroll = scroll.saturating_sub(10),
            Action::PageDown => *scroll += 10,
            Action::Back | Action::EventLog | Action::Confirm => s.ui.overlay = None,
            _ => {}
        }
        return vec![];
    }
    if s.ui.filtering {
        let f = &mut s.ui.filter[s.ui.tab.index()];
        match a {
            Action::Backspace => {
                f.pop();
            }
            Action::Back => {
                f.clear();
                s.ui.filtering = false;
            }
            Action::Submit | Action::Confirm | Action::Down | Action::Up => s.ui.filtering = false,
            _ => {}
        }
        s.ui.upper_mut().selected = 0;
        return vec![];
    }
    if let Some(Overlay::Help { scroll }) = &mut s.ui.overlay {
        match a {
            Action::Up | Action::LineUp => *scroll = scroll.saturating_sub(1),
            Action::Down | Action::LineDown => *scroll += 1,
            Action::PageUp => *scroll = scroll.saturating_sub(10),
            Action::PageDown => *scroll += 10,
            Action::Back | Action::Help => s.ui.overlay = None,
            Action::PaneToggle | Action::Detach | Action::Interrupt => {
                s.ui.overlay = None;
                return action(s, a);
            }
            _ => {}
        }
        return vec![];
    }
    let routed = match s.ui.focus {
        Focus::Composer if s.ask_visible() && s.ui.ask_active && s.ui.task_view.is_none() => {
            crate::chat_update::dialog_action(s, a)
        }
        Focus::Composer => crate::chat_update::composer_action(s, a),
        Focus::Transcript => crate::chat_update::transcript_action(s, a),
        _ => None,
    };
    if let Some(e) = routed {
        return e;
    }
    if s.ui.focus.in_side() {
        let routed = match s.ui.tab {
            Tab::Sessions => crate::sessions::action(s, a),
            Tab::Flows => crate::flows::action(s, a),
            Tab::Jobs => crate::jobs::action(s, a),
        };
        if let Some(e) = routed {
            return e;
        }
    }
    match a {
        Action::Help => s.ui.overlay = Some(Overlay::Help { scroll: 0 }),
        Action::ToggleDetail => s.ui.detail = !s.ui.detail,
        Action::NextAttention => return crate::follow::next_attention(s),
        Action::EventLog => s.ui.overlay = Some(Overlay::EventLog { scroll: 0 }),
        Action::Filter if s.ui.focus.in_side() => {
            s.ui.filtering = true;
            s.ui.focus = Focus::SideUpper;
        }
        Action::TogglePanel => s.toggle_panel(),
        Action::LineUp | Action::LineDown => {
            let d = if a == Action::LineUp { -1 } else { 1 };
            scroll_focused(s, d);
        }
        Action::FocusPanel => {
            if s.panel_open() {
                s.ui.focus = Focus::Panel;
            } else {
                s.notice("这里没有打开的面板 · Ctrl+T 打开", false);
            }
        }
        Action::TabSessions => switch_tab(s, Tab::Sessions),
        Action::TabFlows => switch_tab(s, Tab::Flows),
        Action::TabJobs => switch_tab(s, Tab::Jobs),
        Action::FocusToggle => {
            if s.ui.focus.in_side() {
                s.show_chat();
                s.ui.focus = Focus::Composer;
            } else {
                s.show_side(Focus::SideUpper);
            }
        }
        Action::PaneToggle => {
            if s.wide() {
                s.ui.side_collapsed = !s.ui.side_collapsed;
                if s.ui.side_collapsed && s.ui.focus.in_side() {
                    s.ui.focus = Focus::Composer;
                }
            } else if s.ui.showing == ShowPane::Chat {
                s.show_side(Focus::SideUpper);
            } else {
                s.show_chat();
            }
        }
        Action::SwitchSection => {
            s.ui.focus = match s.ui.focus {
                Focus::SideUpper => Focus::SideLower,
                _ => Focus::SideUpper,
            }
        }
        Action::PageUp | Action::PageDown if s.ui.focus == Focus::Panel => {
            let room = s
                .chat_regions()
                .chat
                .and_then(|c| c.panel)
                .map_or(1, |p| p.h.saturating_sub(2).max(1)) as isize;
            s.scroll_panel(if a == Action::PageUp { -room } else { room });
        }
        Action::Up | Action::Down | Action::PageUp | Action::PageDown if s.ui.focus.in_side() => {
            let delta: isize = match a {
                Action::Up => -1,
                Action::Down => 1,
                Action::PageUp => -10,
                _ => 10,
            };
            move_selection(s, s.ui.focus, delta);
        }
        Action::RatioDec => {
            s.ui.ratio_pct = s.ui.ratio_pct.saturating_sub(RATIO_STEP).max(RATIO_MIN)
        }
        Action::RatioInc => s.ui.ratio_pct = (s.ui.ratio_pct + RATIO_STEP).min(RATIO_MAX),
        Action::FocusTranscript => s.ui.focus = Focus::Transcript,
        Action::Back => {
            if s.ui.notice.is_some() {
                s.ui.notice = None;
            } else if s.ui.focus != Focus::Composer {
                s.show_chat();
                s.ui.focus = Focus::Composer;
            }
        }
        Action::MouseToggle => {
            s.ui.mouse_captured = !s.ui.mouse_captured;
            let text = if s.ui.mouse_captured {
                "鼠标：Codix"
            } else {
                "鼠标：终端（可以用终端自己的方式选择、复制）"
            };
            s.notice(text, false);
            return vec![Effect::SetMouseCapture(s.ui.mouse_captured)];
        }
        Action::Interrupt => {
            let now = s.ui.now_ms;
            if s.ui
                .quit_armed_ms
                .is_some_and(|t| now.saturating_sub(t) <= QUIT_ARM_MS)
            {
                s.ui.quit_armed_ms = None;
                s.ui.notice = None;
                s.ui.overlay = Some(Overlay::Exit { selected: 0 });
                return vec![];
            }
            s.ui.quit_armed_ms = Some(now);
            let key = s
                .keymap
                .key_for(keys::Context::Global, Action::Interrupt)
                .unwrap_or_default();
            s.notice(format!("再按一次 {key} 退出"), false);
        }
        Action::Detach | Action::Exit => s.ui.overlay = Some(Overlay::Exit { selected: 0 }),
        Action::Redraw => return vec![Effect::Redraw],
        _ => {}
    }
    vec![]
}

fn switch_tab(s: &mut State, tab: Tab) {
    s.ui.tab = tab;
    s.show_side(Focus::SideUpper);
}

/// 右栏两半的选中项始终在视口里。
fn fix_scroll(s: &mut State) {
    if s.ui.screen != Screen::Workbench {
        return;
    }
    let spec = layout_spec(s);
    let labels = tab_labels(s);
    let Some(side) = layout::compute(s.ui.size, &spec, &labels).side else {
        return;
    };
    let fit = |sec: &mut crate::state::SectionUi, h: usize| {
        let h = h.max(1);
        if sec.selected < sec.scroll {
            sec.scroll = sec.selected;
        } else if sec.selected >= sec.scroll + h {
            sec.scroll = sec.selected + 1 - h;
        }
    };
    fit(s.ui.upper_mut(), side.upper.h as usize);
    fit(s.ui.lower_mut(), side.lower.h as usize);
}

/// 滚动有焦点的那一块（交互 §7）：`lines` 是行数，正数向下。输入框里的字超出了框就滚它，
/// 没超出就滚转录——边打字边往回翻对话最常见。
pub(crate) fn scroll_focused(s: &mut State, lines: isize) {
    match s.ui.focus {
        Focus::Panel => s.scroll_panel(lines),
        Focus::Composer if s.composer_overflow().is_some() => s.scroll_composer(lines),
        Focus::Composer | Focus::Transcript => crate::chat_update::scroll(s, lines),
        Focus::SideUpper | Focus::SideLower => move_selection(s, s.ui.focus, lines),
    }
}

fn move_selection(s: &mut State, focus: Focus, delta: isize) {
    let len = match focus {
        Focus::SideUpper => s.upper_len(),
        _ => s.lower_len(),
    };
    let sec = match focus {
        Focus::SideLower => s.ui.lower_mut(),
        _ => s.ui.upper_mut(),
    };
    if len == 0 {
        sec.selected = 0;
        return;
    }
    sec.selected = (sec.selected as isize + delta).clamp(0, len as isize - 1) as usize;
}

fn mouse(s: &mut State, m: Mouse) -> Vec<Effect> {
    if let Some(o) = s.ui.overlay.clone() {
        return overlay_mouse(s, &o, m);
    }
    let spec = layout_spec(s);
    let labels = tab_labels(s);
    let r = layout::compute(s.ui.size, &spec, &labels);
    let (col, row) = (m.col, m.row);
    match m.kind {
        MouseKind::Down(MouseButton::Left) => {
            let double = s.ui.last_click.is_some_and(|(c, r, t)| {
                c == col && r == row && s.ui.now_ms - t <= DOUBLE_CLICK_MS
            });
            s.ui.last_click = Some((col, row, s.ui.now_ms));
            if r.divider.is_some_and(|d| d.contains(col, row)) {
                s.ui.drag = Some(Drag::Divider);
                return vec![];
            }
            if let Some(side) = &r.side {
                if let Some(i) = side.tab_hits.iter().position(|t| t.contains(col, row)) {
                    return action(
                        s,
                        [Action::TabSessions, Action::TabFlows, Action::TabJobs][i],
                    );
                }
                for (rect, focus) in [
                    (side.upper, Focus::SideUpper),
                    (side.lower, Focus::SideLower),
                ] {
                    if rect.contains(col, row) {
                        s.ui.focus = focus;
                        let offset = (row - rect.y) as isize;
                        let sec = if focus == Focus::SideUpper {
                            s.ui.upper()
                        } else {
                            s.ui.lower()
                        };
                        let target = sec.scroll as isize + offset;
                        let delta = target - sec.selected as isize;
                        move_selection(s, focus, delta);
                        if double {
                            return action(s, Action::Open);
                        }
                        return vec![];
                    }
                }
            }
            if let Some(chat) = &r.chat {
                if chat.panel.is_some_and(|p| p.contains(col, row)) && s.panel_open() {
                    s.ui.focus = Focus::Panel;
                    return vec![];
                }
                if chat.composer.contains(col, row) {
                    s.ui.focus = Focus::Composer;
                    if s.ask_visible() {
                        s.ui.ask_active = true;
                        let i = (row.saturating_sub(chat.composer_inner.y) as usize)
                            .checked_sub(crate::view::ASK_OPTIONS_OFFSET);
                        if let Some(i) = i {
                            s.ui.ask_selected = i;
                            if double {
                                return action(s, Action::Confirm);
                            }
                        }
                    }
                } else if chat.transcript.contains(col, row) {
                    s.ui.focus = Focus::Transcript;
                    crate::chat_update::transcript_click(s, (row - chat.transcript.y) as usize);
                } else if chat.frame.contains(col, row) {
                    s.ui.focus = Focus::Composer;
                }
            }
        }
        MouseKind::Drag(MouseButton::Left) if s.ui.drag == Some(Drag::Divider) => {
            if s.ui.size.cols > 0 {
                let pct = (u32::from(col) * 100 / u32::from(s.ui.size.cols)) as u16;
                s.ui.ratio_pct = pct.clamp(RATIO_MIN, RATIO_MAX);
            }
        }
        MouseKind::Up(_) => s.ui.drag = None,
        MouseKind::ScrollUp | MouseKind::ScrollDown => {
            let delta = if m.kind == MouseKind::ScrollUp { -3 } else { 3 };
            if r.chat.is_some_and(|c| c.transcript.contains(col, row)) {
                crate::chat_update::scroll(s, delta);
                return vec![];
            }
            if r.chat
                .and_then(|c| c.panel)
                .is_some_and(|p| p.contains(col, row))
            {
                s.scroll_panel(delta.signum());
                return vec![];
            }
            if let Some(side) = &r.side {
                if side.upper_frame.contains(col, row) {
                    move_selection(s, Focus::SideUpper, delta);
                } else if side.lower_frame.contains(col, row) {
                    move_selection(s, Focus::SideLower, delta);
                }
            }
        }
        _ => {}
    }
    vec![]
}

/// 浮层里的鼠标：滚轮滚动；退出框的选项可以点；点到框外关掉浮层。
fn overlay_mouse(s: &mut State, o: &Overlay, m: Mouse) -> Vec<Effect> {
    let f = crate::view::view(s);
    let Some(ov) = f.overlay else {
        return vec![];
    };
    let r = layout::overlay_rect(s.ui.size, ov.width, ov.height);
    match m.kind {
        MouseKind::ScrollUp => action(s, Action::Up),
        MouseKind::ScrollDown => action(s, Action::Down),
        MouseKind::Down(MouseButton::Left) if !r.contains(m.col, m.row) => {
            s.ui.overlay = None;
            vec![]
        }
        MouseKind::Down(MouseButton::Left) => {
            if let Overlay::Exit { .. } = o {
                // 选项在框内第 3 到 5 行（第 1 行问句、第 2 行空行）。
                let line = m.row.saturating_sub(r.y + 1) as usize;
                if (2..5).contains(&line) {
                    return exit_choice(s, line - 2);
                }
            }
            vec![]
        }
        _ => vec![],
    }
}

fn event_name(e: &Event) -> String {
    let v = serde_json::to_value(e).unwrap_or_default();
    let name = match &v {
        serde_json::Value::Object(m) => m.keys().next().cloned().unwrap_or_default(),
        serde_json::Value::String(x) => x.clone(),
        _ => String::new(),
    };
    match e.session() {
        Some(id) => format!("← {name}  {id}"),
        None => format!("← {name}"),
    }
}

fn upsert_run(s: &mut State, r: &port::RunSummary) {
    let list = s.domain.runs.entry(r.flow.clone()).or_default();
    match list.iter_mut().find(|x| x.id == r.id) {
        Some(x) => *x = r.clone(),
        None => list.insert(0, r.clone()),
    }
}

fn event(s: &mut State, e: Event) -> Vec<Effect> {
    s.log(event_name(&e));
    match &e {
        Event::RunUpserted(r) => {
            upsert_run(s, r);
            if let Some(v) = s.domain.run_views.get_mut(&r.id) {
                v.summary = r.clone();
            }
        }
        Event::RunDetail(v) => {
            upsert_run(s, &v.summary);
            s.domain
                .run_views
                .insert(v.summary.id.clone(), (**v).clone());
        }
        Event::FlowDetail(d) => {
            s.domain
                .flow_details
                .insert(d.summary.name.clone(), (**d).clone());
            match s.domain.flows.iter_mut().find(|f| f.name == d.summary.name) {
                Some(f) => *f = d.summary.clone(),
                None => s.domain.flows.push(d.summary.clone()),
            }
        }
        Event::JobFired { job, entry } => {
            s.domain
                .job_history
                .entry(job.clone())
                .or_default()
                .insert(0, entry.clone());
            if let Some(j) = s.domain.jobs.iter_mut().find(|j| &j.id == job) {
                j.last = Some(entry.outcome.clone());
            }
        }
        Event::Connection(port::ConnState::Resynced) => {
            let mut out = vec![
                s.call(
                    Pending::Sessions,
                    Command::ListSessions {
                        filter: port::SessionFilter::default(),
                    },
                ),
                s.call(Pending::Flows, Command::ListFlows),
                s.call(Pending::Jobs, Command::ListJobs),
            ];
            s.domain.runs.clear();
            s.ui.loading.clear();
            out.extend(crate::chat_update::chat_event(s, &e));
            s.domain.conn = Some(port::ConnState::Resynced);
            s.notice("已重新连上，正在刷新", false);
            return out;
        }
        _ => {}
    }
    let mut out = crate::chat_update::chat_event(s, &e);
    match &e {
        Event::TaskUpserted(t) => {
            for c in s.domain.convs.values_mut() {
                let mine = c.summary.id == t.owner
                    || c.tasks
                        .iter()
                        .any(|x| x.id == t.id || x.agent_session.as_ref() == Some(&t.owner));
                if mine {
                    c.apply(Event::TaskUpserted(t.clone()), s.ui.now_ms);
                }
            }
        }
        Event::TaskOutput { task, text, .. } => {
            s.domain
                .task_output
                .entry(task.clone())
                .or_default()
                .push_str(text);
        }
        _ => {}
    }
    match e {
        Event::Kicked { by, at_ms } => {
            s.ui.screen = Screen::Detached(DetachReason::Kicked { by, at_ms });
            s.ui.overlay = None;
            return vec![Effect::Release];
        }
        Event::InstanceStopped { reason } => {
            s.ui.screen = Screen::Detached(DetachReason::Stopped { reason });
            s.ui.overlay = None;
            return vec![Effect::Release];
        }
        _ => {}
    }
    let d = &mut s.domain;
    match e {
        Event::Connection(c) => {
            d.conn = Some(c);
            if c == ConnState::Lost {
                s.notice("与实例断开", true);
            }
        }
        Event::SessionUpserted(sum) => match d.sessions.iter_mut().find(|x| x.id == sum.id) {
            Some(x) => *x = sum,
            None => d.sessions.insert(0, sum),
        },
        Event::SessionRemoved { session } => d.sessions.retain(|x| x.id != session),
        Event::FlowUpserted(f) => match d.flows.iter_mut().find(|x| x.name == f.name) {
            Some(x) => *x = f,
            None => d.flows.push(f),
        },
        Event::JobUpserted(j) => match d.jobs.iter_mut().find(|x| x.id == j.id) {
            Some(x) => *x = j,
            None => d.jobs.push(j),
        },
        Event::JobRemoved { job } => d.jobs.retain(|x| x.id != job),
        Event::Notice { level, text } => s.notice(text, level == port::NoticeLevel::Error),
        _ => {}
    }
    std::mem::take(&mut out)
}

fn reply(s: &mut State, id: ReqId, r: Result<Reply, PortError>) -> Vec<Effect> {
    let Some(what) = s.pending.remove(&id) else {
        return vec![];
    };
    let key = match &what {
        Pending::Runs(f) => Some(format!("runs:{f}")),
        Pending::FlowDetail(f) => Some(format!("flow:{f}")),
        Pending::RunView { id, .. } => Some(format!("run:{id}")),
        Pending::JobHistory(j) => Some(format!("history:{j}")),
        Pending::Authoring { key, .. } => Some(format!("authoring:{key}")),
        _ => None,
    };
    if let Some(k) = key {
        s.ui.loading.remove(&k);
    }
    let reply = match r {
        Ok(r) => r,
        Err(e) => {
            let tab = match what {
                Pending::Sessions => Some(Tab::Sessions),
                Pending::Flows => Some(Tab::Flows),
                Pending::Jobs => Some(Tab::Jobs),
                _ => None,
            };
            match tab {
                Some(t) => s.domain.load_errors[t.index()] = Some(e.to_string()),
                None => s.notice(e.to_string(), true),
            }
            return vec![];
        }
    };
    if matches!(what, Pending::Ignore) {
        return vec![];
    }
    let d = &mut s.domain;
    match (what, reply) {
        (
            Pending::Hello,
            Reply::Hello {
                instance,
                flows,
                jobs,
            },
        ) => {
            d.instance = Some(instance);
            d.flows_available = flows;
            d.jobs_available = jobs;
            d.conn = Some(ConnState::Connected);
        }
        (Pending::Sessions, Reply::Sessions(v)) => {
            d.sessions = v;
            d.load_errors[0] = None;
        }
        (Pending::Flows, Reply::Flows(v)) => {
            d.flows = v;
            d.load_errors[1] = None;
        }
        (Pending::Jobs, Reply::Jobs(v)) => {
            d.jobs = v;
            d.load_errors[2] = None;
        }
        (Pending::Runs(f), Reply::Runs(v)) => {
            d.runs.insert(f, v);
        }
        (Pending::FlowDetail(f), Reply::Flow(v)) => {
            d.flow_details.insert(f, *v);
        }
        (Pending::RunView { id, follow }, Reply::Run(v)) => {
            let main = v.main_session.clone();
            d.run_views.insert(id, *v);
            if follow {
                return crate::follow::show(s, main, vec![], None);
            }
            s.ui.last_follow = None;
        }
        (Pending::JobHistory(j), Reply::JobHistory(v)) => {
            d.job_history.insert(j, v);
        }
        (Pending::Authoring { key, follow }, Reply::Session(sum)) => {
            d.authoring.insert(key, sum.id.clone());
            if follow {
                s.ui.last_follow = None;
                return crate::follow::show(s, sum.id, vec![], None);
            }
        }
        (what, reply) => return crate::chat_update::chat_reply(s, what, reply),
    }
    vec![]
}
