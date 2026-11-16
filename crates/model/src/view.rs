use port::{ConnState, JobOutcome, SessionState};
use tui::layout::{self, LayoutSpec, MIN_COLS, MIN_ROWS};
use tui::{
    Body, ChatPane, Composer, FrameState, Header, Hint, Line, Section, SidePane, TabLabel, Tone,
    Workbench, truncate_to_width,
};

use crate::state::{Confirm, DetachReason, Focus, Overlay, Picker, Screen, State, Tab};

/// 提问框里第一个选项所在的行（鼠标点选项用，交互 §4.6）。
pub(crate) const ASK_OPTIONS_OFFSET: usize = 4;
const PLACEHOLDER: &str = "输入消息 · / 命令 · @ 文件 · ! 执行命令";
const SPINNER: [&str; 4] = ["✻", "✽", "✶", "✳"];

pub(crate) fn layout_spec(s: &State) -> LayoutSpec {
    chat_layout(s).0
}

/// 两遍：先按一行输入框算出宽度，再按这个宽度排输入框（或提问框）定高度。
pub(crate) fn chat_layout(s: &State) -> (LayoutSpec, Vec<String>) {
    let labels = tab_labels(s);
    let conv = s.conv();
    let todo_rows = if s.ui.show_todos {
        conv.map_or(0, |c| c.todos.len().min(5)) as u16
    } else {
        0
    };
    let mut spec = LayoutSpec {
        mode: layout::pane_mode_for(
            s.ui.size.cols,
            s.ui.ratio_pct,
            s.ui.side_collapsed,
            s.ui.showing,
        ),
        composer_rows: 1,
        status: conv.is_some_and(|c| c.running || !c.prompts.is_empty()),
        todo_rows,
    };
    if let Some(chat) = layout::compute(s.ui.size, &spec, &labels).chat {
        let w = chat.composer_inner.w as usize;
        spec.composer_rows = match ask_lines(s, w.saturating_sub(2)) {
            Some((lines, _)) => lines.len() as u16,
            None => s.ui.composer.layout(w, PLACEHOLDER).0.len() as u16,
        };
    }
    (spec, labels)
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
            theme: s.settings.theme.clone(),
            body: Body::TooSmall {
                need_cols: MIN_COLS,
            },
            footer,
            overlay: None,
        };
    }
    match &s.ui.screen {
        Screen::Instances(p) => {
            return FrameState {
                theme: s.settings.theme.clone(),
                body: Body::Instances(picker(s, p)),
                footer,
                overlay: s.ui.overlay.as_ref().map(|o| overlay(s, o)),
            };
        }
        Screen::Detached(r) => {
            return FrameState {
                theme: s.settings.theme.clone(),
                body: Body::Detached(tui::DetachedScreen {
                    lines: detached(s, r),
                }),
                footer,
                overlay: None,
            };
        }
        Screen::Workbench => {}
    }
    let spec = layout_spec(s);
    let tabs = tabs(s);
    let labels: Vec<String> = tabs.iter().map(TabLabel::display).collect();
    let regions = layout::compute(size, &spec, &labels);
    let chat = regions.chat.as_ref().map(|r| chat_pane(s, r));
    let side = regions.side.as_ref().map(|r| side_pane(s, tabs, r));
    FrameState {
        theme: s.settings.theme.clone(),
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

fn chat_pane(s: &State, r: &tui::layout::ChatRegions) -> ChatPane {
    let width = r.transcript.w as usize;
    let focused = matches!(s.ui.focus, Focus::Composer | Focus::Transcript);
    let conv = s.conv();
    let (crumbs, badge, owner) = match conv {
        Some(c) => (
            format!("会话 › {}", c.summary.title),
            match c.summary.state {
                port::SessionState::Running => "● 运行中",
                port::SessionState::Waiting => "◐ 等你",
                port::SessionState::Idle => "○ 空闲",
            },
            owner_line(&c.summary),
        ),
        None => (
            "会话 › 新会话（输入即开始）".to_string(),
            "",
            s.domain
                .instance
                .as_ref()
                .map(|i| {
                    format!(
                        "{}@{} · {}",
                        i.name,
                        i.host,
                        i.project.as_deref().unwrap_or("无项目")
                    )
                })
                .unwrap_or_default(),
        ),
    };
    let h = r.transcript.h as usize;
    let mut below = None;
    let transcript = match (conv, &s.ui.chat) {
        (Some(c), _) => {
            let top = c.viewport_top(h);
            let sel = (s.ui.focus == Focus::Transcript)
                .then_some(c.selected)
                .flatten()
                .and_then(|id| c.first_line_of(id));
            if c.unseen && !c.follow {
                below = Some(Line::toned("↓ 有新内容 (End)", Tone::Attention));
            }
            c.blocks
                .iter()
                .flat_map(|b| b.cache.iter().flat_map(|x| x.lines.iter()))
                .enumerate()
                .skip(top)
                .take(h)
                .map(|(i, l)| {
                    if Some(i) == sel {
                        l.clone().with_tone(Tone::Selected)
                    } else {
                        l.clone()
                    }
                })
                .collect()
        }
        (None, Some(_)) => vec![Line::toned("加载中 …", Tone::Dim)],
        (None, None) => crate::welcome::idle(s.domain.instance.as_ref(), width),
    };
    let composer_w = r.composer_inner.w as usize;
    let placeholder = match (&s.ui.answering, conv) {
        (Some(_), Some(c)) => c
            .prompts
            .first()
            .map(|p| match &p.kind {
                port::PromptKind::Question { question, .. } => format!("回答：{question}"),
                _ => "写下回答".into(),
            })
            .unwrap_or_default(),
        _ => PLACEHOLDER.to_string(),
    };
    let (lines, cursor) = s.ui.composer.layout(composer_w, &placeholder);
    let picker = s.picker_candidates();
    let ask = ask_lines(s, composer_w.saturating_sub(2));
    let running = conv.is_some_and(|c| c.running);
    let hint_left = if s.ui.esc_armed_ms.is_some() && !s.ui.composer.is_empty() {
        Line::toned("再按一次 Esc 清空", Tone::Warn)
    } else if running {
        Line::toned("esc 中断", Tone::Dim)
    } else {
        Line::toned("? 快捷键", Tone::Dim)
    };
    let hint_right = match s.ui.permission {
        port::PermissionMode::Default => Line::default(),
        port::PermissionMode::AcceptEdits => {
            Line::toned("⏵⏵ 自动接受编辑 (shift+tab 切换)", Tone::Accent)
        }
        port::PermissionMode::Plan => Line::toned("⏸ 计划模式 (shift+tab 切换)", Tone::Accent),
        port::PermissionMode::BypassPermissions => {
            Line::toned("⏵⏵ 绕过权限 (shift+tab 切换)", Tone::Error)
        }
    };
    ChatPane {
        focused,
        crumbs: Line::plain(crumbs),
        badge: Line::plain(badge),
        owner: Line::toned(truncate_to_width(&owner, width), Tone::Dim),
        transcript,
        transcript_focused: s.ui.focus == Focus::Transcript,
        below,
        status: conv.and_then(|c| status_line(s, c, width)),
        todos: if s.ui.show_todos {
            conv.map(|c| todo_lines(&c.todos, width))
                .unwrap_or_default()
        } else {
            vec![]
        },
        composer: Composer {
            focused: s.ui.focus == Focus::Composer && s.ui.overlay.is_none() && ask.is_none(),
            lines,
            cursor: Some(cursor),
        },
        picker: picker
            .iter()
            .map(|c| {
                Line::toned(format!("  /{:<14}", c.name), Tone::Accent)
                    .push(
                        truncate_to_width(&c.description, width.saturating_sub(30)),
                        Tone::Normal,
                    )
                    .push(format!("  {}", c.source), Tone::Dim)
            })
            .collect(),
        picker_selected: (!picker.is_empty()).then(|| s.ui.composer.picker.min(picker.len() - 1)),
        ask_selected: ask.as_ref().and_then(|(_, sel)| *sel),
        ask: ask.map(|(l, _)| l),
        hint_left,
        hint_right,
    }
}

fn owner_line(sum: &port::SessionSummary) -> String {
    let p = &sum.provenance;
    let origin = match &p.origin {
        port::Origin::User => "用户新建".to_string(),
        port::Origin::Job { name, .. } => format!("作业「{name}」"),
        port::Origin::FlowStation {
            flow_title,
            subject,
            station_title,
            ..
        } => format!("流程「{flow_title}」› {subject} › {station_title}"),
        port::Origin::SubAgent { parent_title, .. } => format!("子 Agent ← {parent_title}"),
        port::Origin::Fork { from_title, .. } => format!("分叉自 {from_title}"),
    };
    let mut s = format!(
        "{}@{} · {} · {} · {} · {}",
        p.instance,
        p.host,
        p.project.as_deref().unwrap_or("无项目"),
        p.scene,
        p.model,
        origin
    );
    if let Some(h) = &sum.held_by {
        s.push_str(&format!(" · ⚠ 被实例 {h} 占用"));
    }
    s
}

fn status_line(s: &State, c: &crate::chat::Conversation, width: usize) -> Option<Line> {
    if let Some(p) = c.prompts.first() {
        let what = match &p.kind {
            port::PromptKind::Permission { tool, summary, .. } => {
                format!("◐ 等你批准：{tool}({summary})")
            }
            port::PromptKind::Question { question, .. } => format!("◐ 等你回答：{question}"),
        };
        return Some(Line::toned(
            truncate_to_width(&what, width),
            Tone::Attention,
        ));
    }
    if !c.running {
        return None;
    }
    let frame = SPINNER[(s.ui.now_ms / crate::update::SPINNER_MS) as usize % SPINNER.len()];
    let verb = match &c.phase {
        crate::chat::Phase::Thinking => "思考中…".to_string(),
        crate::chat::Phase::Answering => "回答中…".to_string(),
        crate::chat::Phase::Tool(name) => format!("执行 {name}…"),
        crate::chat::Phase::Compacting => "压缩上下文…".to_string(),
    };
    let secs = s.ui.now_ms.saturating_sub(c.turn_started_ms) / 1000;
    let mut tail = format!("{secs}s");
    if c.out_chars > 0 {
        tail.push_str(&format!(
            " · ↓ {} tokens",
            crate::chat::fmt_k((c.out_chars / 3).max(1) as u64)
        ));
    }
    Some(
        Line::toned(format!("{frame} "), Tone::Accent)
            .push(verb, Tone::Accent)
            .push(format!(" ({tail})"), Tone::Dim),
    )
}

fn todo_lines(todos: &[port::Todo], width: usize) -> Vec<Line> {
    todos
        .iter()
        .take(5)
        .map(|t| {
            let (g, tone) = match t.status {
                port::TodoStatus::Done => ("☒", Tone::Dim),
                port::TodoStatus::InProgress => ("◼", Tone::Strong),
                port::TodoStatus::Pending => ("☐", Tone::Normal),
            };
            Line::toned(truncate_to_width(&format!("  {g} {}", t.text), width), tone)
        })
        .collect()
}

/// 提问框的行与选中行（交互 §4.6，照 Claude Code）。固定结构：标题、摘要、来源、问句，
/// 然后从第 `ASK_OPTIONS_OFFSET` 行起是选项，最后一行是键位提示。
fn ask_lines(s: &State, width: usize) -> Option<(Vec<Line>, Option<usize>)> {
    let c = s.conv()?;
    let p = c.prompts.first()?;
    if s.ui.answering.is_some() {
        return None;
    }
    let count = if c.prompts.len() > 1 {
        format!("({}/{})", 1, c.prompts.len())
    } else {
        String::new()
    };
    let (title, summary, question) = match &p.kind {
        port::PromptKind::Permission { tool, summary, .. } => (
            format!("{tool} 命令"),
            summary.clone(),
            "要执行吗？".to_string(),
        ),
        port::PromptKind::Question {
            question, header, ..
        } => (
            header.clone().unwrap_or_else(|| "模型提问".into()),
            String::new(),
            question.clone(),
        ),
    };
    let fit = |t: String| truncate_to_width(&t, width);
    let pad = width.saturating_sub(tui::display_width(&title) + tui::display_width(&count));
    let mut lines = vec![
        Line::toned(fit(title), Tone::Strong)
            .push(format!("{}{count}", " ".repeat(pad)), Tone::Dim),
        Line::toned(fit(format!("  {summary}")), Tone::Normal),
        match &p.from {
            Some(f) => Line::toned(fit(format!("  来自 {}", f.label)), Tone::Dim),
            None => Line::default(),
        },
        Line::toned(fit(question), Tone::Normal),
    ];
    let options = crate::chat_update::ask_options(&p.kind);
    let sel = s.ui.ask_selected.min(options.len() - 1);
    for (i, o) in options.iter().enumerate() {
        let mark = if i == sel && s.ui.ask_active {
            "❯ "
        } else {
            "  "
        };
        lines.push(Line::plain(fit(format!("{mark}{}. {o}", i + 1))));
    }
    let keys = if s.ui.ask_active {
        "Esc 拒绝 · ↑↓ / 数字选择 · Enter 确认"
    } else {
        "Ctrl+G 或点这里作答"
    };
    lines.push(Line::toned(fit(keys.into()), Tone::Dim));
    let selected = s.ui.ask_active.then_some(ASK_OPTIONS_OFFSET + sel);
    Some((lines, selected))
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
            title_right: None,
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
            title_right: None,
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
    let confirm = s.ui.confirm.as_ref().map(|c| {
        let text = match c {
            Confirm::ForceAttach(n) => format!("强制接入 {n}？对方会被踢掉 · y 确认 / 其他键放弃"),
            Confirm::StopInstance(n) => {
                format!("停止实例 {n}？里面的会话、流程、作业都会停下 · y 确认 / 其他键放弃")
            }
        };
        Line::toned(text, Tone::Attention)
    });
    let message = confirm.or_else(|| {
        s.ui.notice.as_ref().map(|n| {
            Line::toned(
                n.text.clone(),
                if n.error { Tone::Error } else { Tone::Warn },
            )
        })
    });
    tui::Footer {
        hints,
        essential,
        message,
    }
}

fn overlay(s: &State, o: &Overlay) -> tui::Overlay {
    match o {
        Overlay::Exit { selected } => {
            let name = s.ui.attached.clone().unwrap_or_else(|| "当前实例".into());
            let options = [
                (
                    "后台运行".to_string(),
                    format!("实例 {name} 继续运行，之后用 codix attach {name} 接回"),
                ),
                (
                    "直接退出".to_string(),
                    format!("停止实例 {name}，里面的会话、流程、作业都会停下"),
                ),
                ("返回".to_string(), String::new()),
            ];
            let mut lines = vec![Line::plain("要怎么退出？"), Line::default()];
            for (i, (label, desc)) in options.iter().enumerate() {
                let mark = if i == *selected { "❯ " } else { "  " };
                let mut l = Line::plain(format!("{mark}{}. ", i + 1))
                    .push(format!("{label}  "), Tone::Strong)
                    .push(desc.clone(), Tone::Dim);
                if i == *selected {
                    l = l.with_tone(Tone::Selected);
                }
                lines.push(l);
            }
            lines.push(Line::default());
            lines.push(Line::toned(
                "↑↓ / 数字选择 · Enter 确认 · Esc 返回",
                Tone::Dim,
            ));
            tui::Overlay {
                title: "退出".into(),
                height: lines.len() as u16 + 2,
                lines,
                width: 72,
            }
        }
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

/// `3h12m`、`6d`、`41m`、`12s` 这种给人看的时长。
pub(crate) fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h{}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d", s / 86_400),
    }
}

fn pad(text: &str, width: usize) -> String {
    let t = truncate_to_width(text, width);
    let w = tui::display_width(&t);
    format!("{t}{}", " ".repeat(width.saturating_sub(w)))
}

fn picker(s: &State, p: &Picker) -> tui::InstancesScreen {
    let host = p.list.first().map(|i| i.host.as_str()).unwrap_or("本机");
    let now = s.ui.now_ms;
    let mut all: Vec<Line> = vec![Line::toned("＋ 新建实例", Tone::Accent)];
    all.extend(p.list.iter().map(|i| {
        let (glyph, tone) = match (i.state, i.holder.is_some()) {
            (port::InstanceState::Running, true) => ("🔒", Tone::Warn),
            (port::InstanceState::Running, false) => ("● ", Tone::Ok),
            (port::InstanceState::Stopped, _) => ("○ ", Tone::Dim),
            (port::InstanceState::Stale, _) => ("✗ ", Tone::Error),
        };
        let project = i.project.as_deref().unwrap_or("");
        let mut line = Line::toned(format!("{glyph} "), tone)
            .push(pad(i.name.as_str(), 10), Tone::Strong)
            .push(pad(project, 14), Tone::Dim);
        match i.state {
            port::InstanceState::Running => {
                let up = i.started_ms.map(|t| fmt_duration(now.saturating_sub(t)));
                line = line
                    .push(
                        pad(&format!("运行 {}", up.unwrap_or_default()), 11),
                        Tone::Normal,
                    )
                    .push(pad(&format!("会话 {}", i.sessions), 9), Tone::Normal);
                if i.waiting > 0 {
                    line = line.push(pad(&format!("◐ 等你 {}", i.waiting), 10), Tone::Attention);
                }
                if let Some(h) = &i.holder {
                    line = line.push(
                        format!(
                            "被 {} 接入，空闲 {}",
                            h.label(),
                            fmt_duration(now.saturating_sub(h.last_input_ms))
                        ),
                        Tone::Warn,
                    );
                }
            }
            port::InstanceState::Stopped => line = line.push("已停止 · Enter 启动", Tone::Dim),
            port::InstanceState::Stale => {
                line = line.push("残留（进程已不在）· Enter 重新启动", Tone::Error)
            }
        }
        line
    }));
    let n = crate::state::PICKER_ROWS;
    let scroll = p.scroll.min(all.len().saturating_sub(n));
    let message = if p.loading && p.list.is_empty() {
        Some(Line::toned("正在列出本机的实例 …", Tone::Dim))
    } else {
        p.message
            .as_ref()
            .map(|m| Line::toned(m.clone(), Tone::Warn))
    };
    tui::InstancesScreen {
        title: Line::plain(format!("Codix · 选择实例 · 主机 {host}")),
        more_above: scroll > 0,
        more_below: scroll + n < all.len(),
        selected: p.selected.checked_sub(scroll).filter(|i| *i < n),
        rows: all.into_iter().skip(scroll).take(n).collect(),
        message,
    }
}

fn detached(s: &State, r: &DetachReason) -> Vec<Line> {
    let name = s.ui.attached.clone().unwrap_or_default();
    match r {
        DetachReason::Kicked { by, at_ms } => vec![
            Line::toned(format!("已脱离实例 {name}"), Tone::Strong),
            Line::default(),
            Line::toned(
                format!(
                    "被 {} 强制接入（{} 之前）",
                    by.label(),
                    fmt_duration(s.ui.now_ms.saturating_sub(*at_ms))
                ),
                Tone::Warn,
            ),
            Line::toned("实例仍在运行", Tone::Dim),
            Line::default(),
            Line::toned("Enter 回到实例选择 · q 退出", Tone::Dim),
        ],
        DetachReason::Idle { after_ms } => vec![
            Line::toned(format!("已脱离实例 {name}"), Tone::Strong),
            Line::default(),
            Line::toned(
                format!("空闲 {}，已自动脱离，实例仍在运行", fmt_duration(*after_ms)),
                Tone::Warn,
            ),
            Line::default(),
            Line::toned("按任意键重新接入 · q 退出", Tone::Dim),
        ],
        DetachReason::Stopped { reason } => vec![
            Line::toned(format!("实例 {name} 已停止"), Tone::Strong),
            Line::default(),
            Line::toned(reason.clone(), Tone::Warn),
            Line::default(),
            Line::toned("Enter 回到实例选择 · q 退出", Tone::Dim),
        ],
    }
}
