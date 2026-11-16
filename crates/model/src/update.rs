use keys::Action;
use port::{Command, ConnState, Event, PortError, Reply};
use tui::layout::{self, ShowPane, Size};

use crate::mouse::{Mouse, MouseButton, MouseKind};
use crate::state::{Drag, Focus, Notice, Overlay, State, Tab};
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
}

/// 发出去还没回来的命令，回来时据此知道把结果放到哪里。
#[derive(Debug, Clone)]
pub(crate) enum Pending {
    Hello,
    Sessions,
    Flows,
    Jobs,
}

const RATIO_MIN: u16 = 40;
const RATIO_MAX: u16 = 75;
const RATIO_STEP: u16 = 5;
const DOUBLE_CLICK_MS: u64 = 400;
const QUIT_ARM_MS: u64 = 2_000;
const NOTICE_MS: u64 = 3_000;

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

    pub fn set_now(&mut self, now_ms: u64) {
        self.ui.now_ms = now_ms;
    }

    fn call(&mut self, what: Pending, cmd: Command) -> Effect {
        let id = ReqId(self.next_req);
        self.next_req += 1;
        self.pending.insert(id, what);
        Effect::Call(id, Box::new(cmd))
    }

    fn notice(&mut self, text: impl Into<String>, error: bool) {
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

    fn show_chat(&mut self) {
        self.ui.showing = ShowPane::Chat;
        if self.ui.focus.in_side() {
            self.ui.focus = Focus::Composer;
        }
    }
}

pub fn update(s: &mut State, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Action(a) => action(s, a),
        Msg::Text(_) | Msg::Paste(_) => {
            s.ui.quit_armed_ms = None;
            vec![]
        }
        Msg::Mouse(m) => mouse(s, m),
        Msg::Event(e) => {
            event(s, *e);
            vec![]
        }
        Msg::Reply(id, r) => {
            reply(s, id, *r);
            vec![]
        }
        Msg::Tick => {
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
            vec![]
        }
        Msg::Resize(size) => {
            s.ui.size = size;
            vec![]
        }
    }
}

fn action(s: &mut State, a: Action) -> Vec<Effect> {
    if a != Action::Interrupt {
        s.ui.quit_armed_ms = None;
    }
    if let Some(Overlay::Help { scroll }) = &mut s.ui.overlay {
        match a {
            Action::Up => *scroll = scroll.saturating_sub(1),
            Action::Down => *scroll += 1,
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
    match a {
        Action::Help => s.ui.overlay = Some(Overlay::Help { scroll: 0 }),
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
                return vec![Effect::Quit { detach: true }];
            }
            s.ui.quit_armed_ms = Some(now);
            let key = s
                .keymap
                .key_for(keys::Context::Global, Action::Interrupt)
                .unwrap_or_default();
            s.notice(format!("再按一次 {key} 脱离（实例继续运行）"), false);
        }
        Action::Detach => return vec![Effect::Quit { detach: true }],
        Action::Redraw => return vec![Effect::Redraw],
        _ => {}
    }
    vec![]
}

fn switch_tab(s: &mut State, tab: Tab) {
    s.ui.tab = tab;
    s.show_side(Focus::SideUpper);
}

fn move_selection(s: &mut State, focus: Focus, delta: isize) {
    let len = match focus {
        Focus::SideUpper => s.upper_len(),
        _ => 0,
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
                if chat.composer.contains(col, row) {
                    s.ui.focus = Focus::Composer;
                } else if chat.transcript.contains(col, row) {
                    s.ui.focus = Focus::Transcript;
                } else if chat.area.contains(col, row) {
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
            if let Some(side) = &r.side {
                if side.upper.contains(col, row) {
                    move_selection(s, Focus::SideUpper, delta);
                } else if side.lower.contains(col, row) {
                    move_selection(s, Focus::SideLower, delta);
                }
            }
        }
        _ => {}
    }
    vec![]
}

fn event(s: &mut State, e: Event) {
    let d = &mut s.domain;
    match e {
        Event::Connection(c) => {
            d.conn = Some(c);
            if c == ConnState::Lost {
                s.notice("与实例断开", true);
            }
        }
        Event::InstanceStopped { reason } => s.notice(format!("实例已停止：{reason}"), true),
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
}

fn reply(s: &mut State, id: ReqId, r: Result<Reply, PortError>) {
    let Some(what) = s.pending.remove(&id) else {
        return;
    };
    let reply = match r {
        Ok(r) => r,
        Err(e) => {
            s.notice(e.to_string(), true);
            return;
        }
    };
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
        (Pending::Sessions, Reply::Sessions(v)) => d.sessions = v,
        (Pending::Flows, Reply::Flows(v)) => d.flows = v,
        (Pending::Jobs, Reply::Jobs(v)) => d.jobs = v,
        _ => {}
    }
}
