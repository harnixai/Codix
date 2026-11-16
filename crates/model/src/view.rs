use port::{ConnState, JobOutcome, SessionState};
use tui::layout::{self, LayoutSpec, MIN_COLS, MIN_ROWS};
use tui::{
    Body, ChatPane, Composer, FrameState, Header, Hint, Line, Section, SidePane, TabLabel, Tone,
    Workbench, truncate_to_width,
};

use crate::state::{Focus, Overlay, State, Tab};

pub(crate) fn layout_spec(s: &State) -> LayoutSpec {
    LayoutSpec {
        mode: layout::pane_mode_for(
            s.ui.size.cols,
            s.ui.ratio_pct,
            s.ui.side_collapsed,
            s.ui.showing,
        ),
        composer_rows: 1,
        status: false,
        upper_rows: s.upper_len().max(3) as u16,
    }
}

fn tabs(s: &State) -> Vec<TabLabel> {
    Tab::ALL
        .iter()
        .map(|t| TabLabel {
            label: t.label().to_string(),
            badge: badge(s, *t),
            active: *t == s.ui.tab,
        })
        .collect()
}

pub(crate) fn tab_labels(s: &State) -> Vec<String> {
    tabs(s).iter().map(TabLabel::display).collect()
}

/// "等你处理"的数量（需求 L-04）。2.8 起由全局待处理列表给出。
fn badge(s: &State, t: Tab) -> u32 {
    match t {
        Tab::Sessions => s.main_sessions().map(|x| x.waiting).sum(),
        Tab::Flows => 0,
        Tab::Jobs => s
            .domain
            .jobs
            .iter()
            .filter(|j| matches!(j.last, Some(JobOutcome::Failed { .. })))
            .count() as u32,
    }
}

pub fn view(s: &State) -> FrameState {
    let size = s.ui.size;
    let footer = footer(s);
    if size.cols < MIN_COLS || size.rows < MIN_ROWS {
        return FrameState {
            body: Body::TooSmall {
                need_cols: MIN_COLS,
            },
            footer,
            overlay: None,
        };
    }
    let spec = layout_spec(s);
    let tabs = tabs(s);
    let labels: Vec<String> = tabs.iter().map(TabLabel::display).collect();
    let regions = layout::compute(size, &spec, &labels);
    let chat = regions.chat.map(|r| chat_pane(s, r.transcript.w as usize));
    let side = regions.side.as_ref().map(|r| side_pane(s, tabs, r));
    FrameState {
        body: Body::Workbench(Box::new(Workbench {
            layout: spec,
            header: header(s),
            chat,
            side,
        })),
        footer,
        overlay: s.ui.overlay.as_ref().map(|o| overlay(s, o)),
    }
}

fn header(s: &State) -> Header {
    let mut left = Line::toned("Codix", Tone::Strong);
    if let Some(i) = &s.domain.instance {
        left = left.push(format!("  实例 {}@{}", i.name, i.host), Tone::Dim);
    }
    let right = match s.domain.conn {
        Some(ConnState::Connected) | Some(ConnState::Resynced) => Line::toned("● 已连接", Tone::Ok),
        Some(ConnState::Reconnecting { attempt }) => {
            Line::toned(format!("⚠ 重连中 (第 {attempt} 次)"), Tone::Warn)
        }
        Some(ConnState::Lost) => Line::toned("⚠ 已断开", Tone::Error),
        None => Line::toned("○ 连接中…", Tone::Dim),
    };
    Header { left, right }
}

fn chat_pane(s: &State, width: usize) -> ChatPane {
    let focused = matches!(s.ui.focus, Focus::Composer | Focus::Transcript);
    let owner = match &s.domain.instance {
        Some(i) => format!(
            "{}@{} · {}",
            i.name,
            i.host,
            i.project.as_deref().unwrap_or("无项目")
        ),
        None => String::new(),
    };
    let welcome = [
        "还没有打开会话。",
        "",
        "F2 会话 · F3 流程 · F4 作业 · Ctrl+O 切换焦点 · Ctrl+B 切栏 · F1 帮助",
    ];
    ChatPane {
        focused,
        crumbs: Line::toned(truncate_to_width("会话 › （未打开）", width), Tone::Normal),
        owner: Line::toned(truncate_to_width(&owner, width), Tone::Dim),
        transcript: welcome
            .iter()
            .map(|t| Line::toned(truncate_to_width(t, width), Tone::Dim))
            .collect(),
        transcript_focused: s.ui.focus == Focus::Transcript,
        below: None,
        status: None,
        composer: Composer {
            focused: s.ui.focus == Focus::Composer && s.ui.overlay.is_none(),
            lines: vec![Line::toned("›", Tone::Dim)],
            cursor: Some((2, 0)),
            title: None,
        },
    }
}

fn side_pane(s: &State, tabs: Vec<TabLabel>, r: &tui::layout::SideRegions) -> SidePane {
    let w = r.upper.w as usize;
    let all = upper_rows(s, w);
    let up = s.ui.upper();
    let visible: Vec<Line> = all
        .iter()
        .skip(up.scroll)
        .take(r.upper.h as usize)
        .cloned()
        .collect();
    let selected = (!all.is_empty() && up.selected >= up.scroll)
        .then(|| up.selected - up.scroll)
        .filter(|i| *i < visible.len());
    let lower_title = upper_title(s);
    SidePane {
        tabs,
        upper: Section {
            focused: s.ui.focus == Focus::SideUpper && s.ui.overlay.is_none(),
            title: None,
            rows: if all.is_empty() {
                vec![Line::toned(empty_hint(s.ui.tab), Tone::Dim)]
            } else {
                visible
            },
            selected,
        },
        lower: Section {
            focused: s.ui.focus == Focus::SideLower && s.ui.overlay.is_none(),
            title: lower_title,
            rows: vec![Line::toned(
                truncate_to_width("（下半在 2.5–2.7 实现）", r.lower.w as usize),
                Tone::Dim,
            )],
            selected: None,
        },
    }
}

fn empty_hint(t: Tab) -> &'static str {
    match t {
        Tab::Sessions => "还没有会话 · n 新建",
        Tab::Flows => "还没有流程 · Enter 在对话里设计一个",
        Tab::Jobs => "还没有作业 · n 新建",
    }
}

fn upper_title(s: &State) -> Option<String> {
    let i = s.ui.upper().selected;
    match s.ui.tab {
        Tab::Sessions => s.main_sessions().nth(i).map(|x| x.title.clone()),
        Tab::Flows => s.domain.flows.get(i).map(|x| x.title.clone()),
        Tab::Jobs => s.domain.jobs.get(i).map(|x| x.name.clone()),
    }
}

fn upper_rows(s: &State, w: usize) -> Vec<Line> {
    let fit = |t: String| truncate_to_width(&t, w);
    match s.ui.tab {
        Tab::Sessions => s
            .main_sessions()
            .map(|x| {
                let (g, tone) = match x.state {
                    SessionState::Running => ("●", Tone::Ok),
                    SessionState::Waiting => ("◐", Tone::Attention),
                    SessionState::Idle => ("○", Tone::Dim),
                };
                Line::toned(format!("{g} "), tone).push(fit(x.title.clone()), Tone::Normal)
            })
            .collect(),
        Tab::Flows => s
            .domain
            .flows
            .iter()
            .map(|f| Line::plain(fit(format!("▸ {}  {}", f.title, f.name))))
            .collect(),
        Tab::Jobs => s
            .domain
            .jobs
            .iter()
            .map(|j| {
                let g = if j.enabled { "●" } else { "○" };
                Line::plain(fit(format!("{g} {}  {}", j.name, j.cron)))
            })
            .collect(),
    }
}

fn footer(s: &State) -> tui::Footer {
    let ctx = s.key_contexts()[0];
    let hints = s
        .keymap
        .hints(ctx)
        .into_iter()
        .map(|(key, label)| Hint {
            key,
            label: label.to_string(),
        })
        .collect();
    let essential = s.keymap.essential().map(|(key, label)| Hint {
        key,
        label: label.to_string(),
    });
    let message = s.ui.notice.as_ref().map(|n| {
        Line::toned(
            n.text.clone(),
            if n.error { Tone::Error } else { Tone::Warn },
        )
    });
    tui::Footer {
        hints,
        essential,
        message,
    }
}

fn overlay(s: &State, o: &Overlay) -> tui::Overlay {
    match o {
        Overlay::Help { scroll } => {
            let mut ctxs = s.key_contexts_without_overlay();
            ctxs.push(keys::Context::Global);
            let mut lines = vec![];
            for ctx in ctxs {
                lines.push(Line::toned(ctx.label(), Tone::Strong));
                for b in s.keymap.bindings().iter().filter(|b| b.context == ctx) {
                    let key = b
                        .keys
                        .iter()
                        .map(|k| k.to_string())
                        .collect::<Vec<_>>()
                        .join(" ");
                    let pad = 14usize.saturating_sub(tui::display_width(&key));
                    lines.push(
                        Line::toned(format!("  {key}{}", " ".repeat(pad)), Tone::Accent)
                            .push(b.action.help(), Tone::Normal),
                    );
                }
                lines.push(Line::default());
            }
            lines.push(Line::toned(
                "鼠标：单击选中 · 双击打开 · 拖动分隔线调比例 · 滚轮滚动所在区域 · F8 交还终端",
                Tone::Dim,
            ));
            let height = (lines.len() as u16 + 2).min(s.ui.size.rows.saturating_sub(4));
            let body = height.saturating_sub(2) as usize;
            let max_scroll = lines.len().saturating_sub(body);
            tui::Overlay {
                title: "帮助".into(),
                lines: lines.into_iter().skip((*scroll).min(max_scroll)).collect(),
                width: 72,
                height,
            }
        }
    }
}

impl State {
    /// 帮助浮层要列的是打开浮层之前那个焦点的键位。
    fn key_contexts_without_overlay(&self) -> Vec<keys::Context> {
        match self.ui.focus {
            Focus::Composer => vec![keys::Context::Composer],
            Focus::Transcript => vec![keys::Context::Transcript],
            _ => vec![self.ui.tab.context()],
        }
    }
}
