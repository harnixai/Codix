use std::collections::HashMap;
use std::sync::Arc;

use keys::{Context, Keymap};
use port::{ConnState, FlowSummary, InstanceInfo, Job, SessionSummary};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help { scroll: usize },
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
}

/// 界面状态：只由输入与 Tick 写入。
#[derive(Debug, Clone)]
pub struct Ui {
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

pub struct State {
    pub domain: Domain,
    pub ui: Ui,
    pub keymap: Arc<Keymap>,
    pub(crate) pending: HashMap<ReqId, Pending>,
    pub(crate) next_req: u64,
}

impl State {
    pub fn new(keymap: Arc<Keymap>, size: Size, now_ms: u64) -> State {
        State {
            domain: Domain::default(),
            ui: Ui {
                size,
                now_ms,
                ratio_pct: 60,
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
            },
            keymap,
            pending: HashMap::new(),
            next_req: 1,
        }
    }

    /// 键位解析用的上下文，从最具体到最宽（`Global` 由解析器自己补上）。
    pub fn key_contexts(&self) -> Vec<Context> {
        if self.ui.overlay.is_some() {
            return vec![Context::Overlay];
        }
        match self.ui.focus {
            Focus::Composer => vec![Context::Composer],
            Focus::Transcript => vec![Context::Transcript],
            Focus::SideUpper | Focus::SideLower => vec![self.ui.tab.context()],
        }
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
