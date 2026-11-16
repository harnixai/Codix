use std::collections::HashMap;
use std::sync::Arc;

use keys::{Context, Keymap};
use std::collections::HashMap as Map;

use port::{
    ClientInfo, ConnState, FlowSummary, InstanceInfo, Job, PermissionMode, PromptId, SessionId,
    SessionSummary,
};

use crate::chat::Conversation;
use crate::composer::ComposerState;
use tui::{ShowPane, Size};

use crate::update::{Pending, ReqId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Sessions,
    Flows,
    Jobs,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Sessions, Tab::Flows, Tab::Jobs];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Sessions => "会话",
            Tab::Flows => "流程",
            Tab::Jobs => "作业",
        }
    }

    pub fn context(self) -> Context {
        match self {
            Tab::Sessions => Context::Sessions,
            Tab::Flows => Context::Flows,
            Tab::Jobs => Context::Jobs,
        }
    }

    fn index(self) -> usize {
        match self {
            Tab::Sessions => 0,
            Tab::Flows => 1,
            Tab::Jobs => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Composer,
    Transcript,
    SideUpper,
    SideLower,
}

impl Focus {
    pub fn in_side(self) -> bool {
        matches!(self, Focus::SideUpper | Focus::SideLower)
    }
}

/// 整屏显示什么（交互 §2.1、§2.2、§9）。
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Instances(Picker),
    Workbench,
    /// 已脱离：实例还在跑，这个 TUI 没接着它。
    Detached(DetachReason),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picker {
    pub list: Vec<InstanceInfo>,
    /// 选中行：0 是第一行"＋ 新建实例"，`i` 是 `list[i - 1]`。
    pub selected: usize,
    /// 视口第一行（一次只显示 `PICKER_ROWS` 行，交互 §2.2）。
    pub scroll: usize,
    pub loading: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DetachReason {
    Kicked { by: ClientInfo, at_ms: u64 },
    Idle { after_ms: u64 },
    Stopped { reason: String },
}

/// 底栏上的二次确认（交互 §5.1）：`y` / Enter 确认，其他键放弃。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    ForceAttach(String),
    StopInstance(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help {
        scroll: usize,
    },
    /// 退出时的选择：后台运行 / 直接退出 / 返回（交互 §7）。
    Exit {
        selected: usize,
    },
}

/// 一个标签自己的选中与滚动；切标签再切回来保持原样。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SectionUi {
    pub selected: usize,
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drag {
    Divider,
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub error: bool,
    pub until_ms: u64,
}

/// 后端事实：只由事件与命令结果写入。
#[derive(Debug, Clone, Default)]
pub struct Domain {
    pub instance: Option<InstanceInfo>,
    pub flows_available: bool,
    pub jobs_available: bool,
    pub conn: Option<ConnState>,
    pub sessions: Vec<SessionSummary>,
    pub flows: Vec<FlowSummary>,
    pub jobs: Vec<Job>,
    /// 被 Watch 的会话。
    pub convs: Map<SessionId, Conversation>,
}

/// 界面状态：只由输入与 Tick 写入。
#[derive(Debug, Clone)]
pub struct Ui {
    pub screen: Screen,
    pub confirm: Option<Confirm>,
    /// 接着的实例名（工作台或"已脱离"时有）。
    pub attached: Option<String>,
    pub last_input_ms: u64,
    /// 0 表示不超时（需求 I-10）。
    pub idle_timeout_ms: u64,
    pub idle_warned: bool,
    pub next_heartbeat_ms: u64,
    pub size: Size,
    pub now_ms: u64,
    pub ratio_pct: u16,
    pub side_collapsed: bool,
    /// 单栏模式下显示哪一栏。
    pub showing: ShowPane,
    pub focus: Focus,
    pub tab: Tab,
    pub upper: [SectionUi; 3],
    pub lower: [SectionUi; 3],
    pub overlay: Option<Overlay>,
    pub mouse_captured: bool,
    pub drag: Option<Drag>,
    pub notice: Option<Notice>,
    /// 空闲时第一次 Ctrl+C 的时刻：两秒内再按一次才脱离。
    pub quit_armed_ms: Option<u64>,
    pub last_click: Option<(u16, u16, u64)>,
    /// 对话区当前显示的会话。
    pub chat: Option<SessionId>,
    pub composer: ComposerState,
    /// Ctrl+O：转录精简 / 详细。
    pub detail: bool,
    /// Ctrl+T：显示 todo 清单。
    pub show_todos: bool,
    /// Shift+Tab 轮换的权限模式。
    pub permission: PermissionMode,
    /// 提问框拿着键盘（交互 §3：输入框为空时提问来了直接拿，否则按 Ctrl+G / 点它）。
    pub ask_active: bool,
    pub ask_selected: usize,
    /// 正在用输入框自由回答的提问。
    pub answering: Option<(SessionId, PromptId)>,
    pub esc_armed_ms: Option<u64>,
    pub detach_armed_ms: Option<u64>,
    /// 测试用计数：重排了多少个块（testing.md §1 用计数验性能）。
    pub relayouts: u64,
    pub quitting: bool,
}

impl Ui {
    pub fn upper(&self) -> &SectionUi {
        &self.upper[self.tab.index()]
    }

    pub fn upper_mut(&mut self) -> &mut SectionUi {
        &mut self.upper[self.tab.index()]
    }

    pub fn lower_mut(&mut self) -> &mut SectionUi {
        &mut self.lower[self.tab.index()]
    }

    pub fn lower(&self) -> &SectionUi {
        &self.lower[self.tab.index()]
    }
}

pub const PICKER_ROWS: usize = 6;
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 30 * 60_000;
pub const HEARTBEAT_MS: u64 = 10_000;
pub const IDLE_WARN_MS: u64 = 60_000;

pub struct State {
    pub settings: crate::Settings,
    pub domain: Domain,
    pub ui: Ui,
    pub keymap: Arc<Keymap>,
    pub(crate) pending: HashMap<ReqId, Pending>,
    pub(crate) next_req: u64,
}

impl State {
    pub fn new(keymap: Arc<Keymap>, size: Size, now_ms: u64) -> State {
        State::with_settings(keymap, size, now_ms, crate::Settings::default())
    }

    pub fn with_settings(
        keymap: Arc<Keymap>,
        size: Size,
        now_ms: u64,
        settings: crate::Settings,
    ) -> State {
        let ratio = settings.ratio_pct.clamp(40, 75);
        let idle = settings.idle_timeout_ms;
        State {
            settings,
            domain: Domain::default(),
            ui: Ui {
                screen: Screen::Workbench,
                confirm: None,
                attached: None,
                last_input_ms: now_ms,
                idle_timeout_ms: idle,
                idle_warned: false,
                next_heartbeat_ms: now_ms + HEARTBEAT_MS,
                size,
                now_ms,
                ratio_pct: ratio,
                side_collapsed: false,
                showing: ShowPane::Chat,
                focus: Focus::Composer,
                tab: Tab::Sessions,
                upper: Default::default(),
                lower: Default::default(),
                overlay: None,
                mouse_captured: true,
                drag: None,
                notice: None,
                quit_armed_ms: None,
                last_click: None,
                chat: None,
                composer: ComposerState::default(),
                detail: false,
                show_todos: false,
                permission: PermissionMode::Default,
                ask_active: false,
                ask_selected: 0,
                answering: None,
                esc_armed_ms: None,
                detach_armed_ms: None,
                relayouts: 0,
                quitting: false,
            },
            keymap,
            pending: HashMap::new(),
            next_req: 1,
        }
    }

    /// 键位解析用的上下文，从最具体到最宽（`Global` 由解析器自己补上）。
    pub fn key_contexts(&self) -> Vec<Context> {
        if self.ui.confirm.is_some() {
            return vec![Context::Dialog];
        }
        if self.ui.overlay.is_some() {
            return vec![Context::Overlay];
        }
        if !matches!(self.ui.screen, Screen::Workbench) {
            return vec![Context::Instances];
        }
        match self.ui.focus {
            Focus::Composer if self.ask_visible() && self.ui.ask_active => vec![Context::Dialog],
            Focus::Composer if !self.picker_candidates().is_empty() => {
                vec![Context::Picker, Context::Composer]
            }
            Focus::Composer => vec![Context::Composer],
            Focus::Transcript => vec![Context::Transcript],
            Focus::SideUpper | Focus::SideLower => vec![self.ui.tab.context()],
        }
    }

    pub fn conv(&self) -> Option<&Conversation> {
        self.ui
            .chat
            .as_ref()
            .and_then(|id| self.domain.convs.get(id))
    }

    pub fn conv_mut(&mut self) -> Option<&mut Conversation> {
        let id = self.ui.chat.clone()?;
        self.domain.convs.get_mut(&id)
    }

    pub fn ask_visible(&self) -> bool {
        self.conv().is_some_and(|c| !c.prompts.is_empty())
    }

    /// 右栏会话列表只列主 Agent；子 Agent 的侧链会话挂在它的拆分树下（需求 S-01、S-04）。
    pub fn main_sessions(&self) -> impl Iterator<Item = &SessionSummary> {
        self.domain
            .sessions
            .iter()
            .filter(|s| !matches!(s.provenance.origin, port::Origin::SubAgent { .. }))
    }

    pub fn wide(&self) -> bool {
        self.ui.size.cols >= tui::layout::WIDE_MIN_COLS
    }

    /// 当前右栏上半的行数（2.5 起由各标签自己的模型给出）。
    pub(crate) fn upper_len(&self) -> usize {
        match self.ui.tab {
            Tab::Sessions => self.main_sessions().count(),
            Tab::Flows => self.domain.flows.len(),
            Tab::Jobs => self.domain.jobs.len(),
        }
    }
}
