//! 对话区顶部的面板（交互 §4.3）：右栏只放摘要，这里放当前上下文的详情——普通会话是
//! goal 与 todo，流程的编写会话是流程设计，运行里的会话是运行进度，作业会话是作业详情。
//! 它不在转录里，所以不随对话滚走。顶上一条标题线，右边是 Ctrl+T；收起时只剩这条线，
//! 好让人知道这里有东西、随手打开。有面板的缺省打开。

use port::{
    FlowDetail, GoalPhase, Job, JobEntry, JobId, JobKind, JobOutcome, RunId, RunView, SessionId,
    StationView,
};
use tui::{Line, Tone, display_width, truncate_to_width};

use crate::chat::Conversation;
use crate::marks;
use crate::state::State;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PanelKind {
    Flow(String),
    Job(JobId),
    Run(RunId),
    /// 普通会话：goal 与 todo。
    Goal,
}

impl State {
    fn panel_target(&self) -> Option<(SessionId, PanelKind)> {
        if self.ui.task_view.is_some() {
            return None;
        }
        let id = self.ui.chat.clone()?;
        let kind = if let Some(key) = self
            .domain
            .authoring
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.clone())
        {
            if let Some(name) = key.strip_prefix("flow:") {
                PanelKind::Flow(name.to_string())
            } else {
                PanelKind::Job(JobId::from(key.trim_start_matches("job:")))
            }
        } else if let Some(v) = self.domain.run_views.values().find(|v| {
            v.main_session == id
                || v.stations.iter().any(|s| {
                    s.session.as_ref() == Some(&id)
                        || s.tasks.iter().any(|t| t.session.as_ref() == Some(&id))
                })
        }) {
            PanelKind::Run(v.summary.id.clone())
        } else {
            PanelKind::Goal
        };
        Some((id, kind))
    }

    /// 这个上下文有没有面板：流程、运行、作业总有；普通会话要有 goal 或 todo。
    fn panel_here(&self) -> Option<(SessionId, PanelKind)> {
        let (id, kind) = self.panel_target()?;
        let has = match kind {
            PanelKind::Goal => self
                .domain
                .convs
                .get(&id)
                .is_some_and(|c| c.goal.is_some() || !c.todos.is_empty()),
            _ => true,
        };
        has.then_some((id, kind))
    }

    /// 面板开着没有：人按过 Ctrl+T 就照人的，否则有面板就开。
    pub fn panel_open(&self) -> bool {
        let Some((id, _)) = self.panel_here() else {
            return false;
        };
        self.ui.panel.get(&id).copied().unwrap_or(true)
    }

    pub(crate) fn toggle_panel(&mut self) {
        match self.panel_here() {
            Some((id, _)) => {
                let open = self.panel_open();
                self.ui.panel.insert(id, !open);
            }
            None => self.notice("这个会话没有 goal 和 todo · 用 /goal 设定一个目标", false),
        }
    }

    /// 面板的标题线左边：它是什么，加一句摘要（收起时也看得到）。
    fn panel_title(&self, kind: &PanelKind, conv: Option<&Conversation>) -> Line {
        match kind {
            PanelKind::Flow(name) => {
                let d = self.domain.flow_details.get(name);
                let f = self.domain.flows.iter().find(|f| &f.name == name);
                let title = d
                    .map(|d| d.summary.title.clone())
                    .or_else(|| f.map(|f| f.title.clone()))
                    .unwrap_or_else(|| name.clone());
                let mut l = Line::toned(format!("流程设计：{title}"), Tone::Strong);
                if let (Some(d), Some(i)) = (d, self.station_in_view(name)) {
                    return l
                        .push(
                            format!(" › 第 {} 站 {}", i + 1, d.stations[i].title),
                            Tone::Strong,
                        )
                        .push(" · 回到上半看整个流程", Tone::Dim);
                }
                if let Some(d) = d {
                    let origin = d.summary.origin.label();
                    l = l.push(
                        format!(
                            " · {} · {origin} · {} 站 · {} 个在跑",
                            d.summary.name,
                            d.stations.len(),
                            d.summary.active_runs
                        ),
                        Tone::Dim,
                    );
                }
                l
            }
            PanelKind::Job(job) => match self.domain.jobs.iter().find(|j| &j.id == job) {
                Some(j) => Line::toned(format!("作业：{}", j.name), Tone::Strong).push(
                    format!(" · {}", if j.enabled { "启用" } else { "已停用" }),
                    Tone::Dim,
                ),
                None => Line::toned("新建作业", Tone::Strong),
            },
            PanelKind::Run(run) => match self.domain.run_views.get(run) {
                Some(v) => {
                    let (g, text, tone) = crate::flows::run_status(&v.summary.status);
                    let flow = self
                        .domain
                        .flows
                        .iter()
                        .find(|f| f.name == v.summary.flow)
                        .map(|f| f.title.clone())
                        .unwrap_or_else(|| v.summary.flow.clone());
                    Line::toned(format!("运行：{}", v.summary.subject), Tone::Strong)
                        .push(format!("（{flow}） · "), Tone::Dim)
                        .push(format!("{g} {text}"), tone)
                        .push(
                            format!(" · {}", crate::flows::progress(&v.summary)),
                            Tone::Dim,
                        )
                }
                None => Line::toned("运行", Tone::Strong),
            },
            PanelKind::Goal => {
                let mut l = Line::toned("goal 与 todo", Tone::Strong);
                if let Some(c) = conv {
                    if let Some(g) = &c.goal {
                        let m = goal_mark(g.phase);
                        let word = match g.phase {
                            GoalPhase::Active => "进行中",
                            GoalPhase::Paused => "已暂停",
                            GoalPhase::Blocked => "受阻",
                            GoalPhase::Complete => "已完成",
                        };
                        l = l
                            .push(" · goal ", Tone::Dim)
                            .push(format!("{} {word}", m.glyph()), m.tone());
                    }
                    if !c.todos.is_empty() {
                        let done = c
                            .todos
                            .iter()
                            .filter(|t| t.status == port::TodoStatus::Done)
                            .count();
                        l = l.push(format!(" · todo {done}/{}", c.todos.len()), Tone::Dim);
                    }
                }
                l
            }
        }
    }

    /// 面板的内容（不含标题线与下沿）；收起时为空。
    pub fn panel_content(&self, width: usize) -> Vec<Line> {
        if !self.panel_open() {
            return vec![];
        }
        let Some((id, kind)) = self.panel_here() else {
            return vec![];
        };
        let conv = self.domain.convs.get(&id);
        let mut out = match &kind {
            PanelKind::Flow(name) => match self.domain.flow_details.get(name) {
                Some(d) => match self.station_in_view(name) {
                    Some(i) => station_body(d, i),
                    None => flow_body(d, width),
                },
                None => vec![Line::toned("流程设计加载中 …", Tone::Dim)],
            },
            PanelKind::Job(job) => match self.domain.jobs.iter().find(|j| &j.id == job) {
                Some(j) => {
                    let h = self
                        .domain
                        .job_history
                        .get(job)
                        .cloned()
                        .unwrap_or_default();
                    job_body(j, &h, width)
                }
                None => vec![Line::toned(
                    "在下面的对话里说要做什么、什么时候做",
                    Tone::Dim,
                )],
            },
            PanelKind::Run(run) => match self.domain.run_views.get(run) {
                Some(v) => run_body(v, width),
                None => vec![],
            },
            PanelKind::Goal => vec![],
        };
        if let Some(c) = conv {
            if !out.is_empty() && (c.goal.is_some() || !c.todos.is_empty()) {
                out.push(Line::default());
            }
            out.extend(goal_lines(c, width));
        }
        out
    }

    /// 面板占几行：开着是标题线 + 内容（最多 `PANEL_MAX` 行）+ 下沿，收起只剩标题线，没有面板是 0。
    pub(crate) fn panel_rows(&self, width: usize) -> u16 {
        if self.panel_here().is_none() {
            0
        } else if self.panel_open() {
            self.panel_content(width).len().min(PANEL_MAX) as u16 + 2
        } else {
            1
        }
    }

    /// 面板上下文的名字：换了上下文，滚动回到顶上。
    fn panel_key(&self) -> String {
        format!(
            "{:?}:{:?}:{:?}",
            self.ui.chat,
            self.panel_target().map(|(_, k)| k),
            self.ui.flow_station
        )
    }

    /// 编写会话的面板正在看流程的哪一站。
    fn station_in_view(&self, flow: &str) -> Option<usize> {
        let (f, i) = self.ui.flow_station.as_ref()?;
        let n = self.domain.flow_details.get(flow)?.stations.len();
        (f == flow && *i < n).then_some(*i)
    }

    fn panel_offset(&self) -> usize {
        if self.ui.panel_scroll.0 == self.panel_key() {
            self.ui.panel_scroll.1
        } else {
            0
        }
    }

    /// Shift+↑ / Shift+↓ 或在面板上滚轮。
    pub(crate) fn scroll_panel(&mut self, delta: isize) {
        let len = self.panel_content(self.panel_width()).len();
        let room = self
            .chat_regions()
            .chat
            .and_then(|c| c.panel)
            .map_or(0, |p| (p.h as usize).saturating_sub(2));
        let max = len.saturating_sub(room);
        let off = (self.panel_offset() as isize + delta).clamp(0, max as isize) as usize;
        self.ui.panel_scroll = (self.panel_key(), off);
    }

    fn panel_width(&self) -> usize {
        self.chat_regions()
            .chat
            .map_or(80, |c| c.transcript.w as usize)
    }

    /// 排进 `height` 行的面板：标题线（右边是 Ctrl+T）、看得见的那段内容、下沿（放不下时
    /// 右边标出上下各还有几行）。
    pub(crate) fn panel_lines(&self, width: usize, height: usize) -> Vec<Line> {
        let Some((id, kind)) = self.panel_here() else {
            return vec![];
        };
        if height == 0 {
            return vec![];
        }
        let open = self.panel_open();
        let title = self.panel_title(&kind, self.domain.convs.get(&id));
        let focused = self.ui.focus == crate::state::Focus::Panel;
        let mut out = vec![rule(
            Some(title),
            if open {
                " Ctrl+T 收起 "
            } else {
                " Ctrl+T 展开 "
            },
            width,
            focused,
        )];
        if !open || height < 3 {
            return out;
        }
        let room = height - 2;
        let content = self.panel_content(width);
        let off = self.panel_offset().min(content.len().saturating_sub(room));
        out.extend(content.iter().skip(off).take(room).map(|l| {
            if l.width() > width {
                Line::toned(truncate_to_width(&l.text(), width), Tone::Normal)
            } else {
                l.clone()
            }
        }));
        let below = content.len().saturating_sub(off + room);
        let more = if off > 0 || below > 0 {
            format!(" ↑{off} ↓{below} · Shift+↑↓ ")
        } else {
            String::new()
        };
        out.push(rule(None, &more, width, focused));
        out
    }
}

/// 面板内容最多几行；再多就滚动。
const PANEL_MAX: usize = 10;

/// `─ 标题 ─────── 右边 ─`：标题太长先截标题，右边的快捷键总在。面板有焦点时线是亮的。
fn rule(title: Option<Line>, right: &str, width: usize, focused: bool) -> Line {
    let line_tone = if focused { Tone::Strong } else { Tone::Dim };
    let rw = display_width(right);
    let mut l = Line::default();
    let mut used = 0;
    if let Some(t) = title {
        l = l.push("─ ", line_tone);
        used = 2;
        let room = width.saturating_sub(used + rw + 4);
        let t = if t.width() > room {
            Line::toned(truncate_to_width(&t.text(), room), Tone::Strong)
        } else {
            t
        };
        used += t.width() + 1;
        for sp in t.spans {
            l = l.push(sp.text, sp.tone);
        }
        l = l.push(" ", line_tone);
    }
    l.push("─".repeat(width.saturating_sub(used + rw + 1)), line_tone)
        .push(right, Tone::Dim)
        .push("─", line_tone)
}

fn goal_mark(p: GoalPhase) -> marks::Mark {
    match p {
        GoalPhase::Active => marks::Mark::Active,
        GoalPhase::Paused => marks::Mark::Paused,
        GoalPhase::Blocked => marks::Mark::Waiting,
        GoalPhase::Complete => marks::Mark::Done,
    }
}

fn goal_lines(c: &Conversation, width: usize) -> Vec<Line> {
    let mut out = vec![];
    if let Some(g) = &c.goal {
        let (m, tail) = match g.phase {
            GoalPhase::Active => (
                marks::Mark::Active,
                format!("第 {}/{} 轮", g.rounds_started, g.max_rounds),
            ),
            GoalPhase::Paused => (marks::Mark::Paused, "已暂停".into()),
            GoalPhase::Blocked => (
                marks::Mark::Waiting,
                format!("受阻：{}", g.blocked_reason.as_deref().unwrap_or("")),
            ),
            GoalPhase::Complete => (marks::Mark::Done, "已完成".into()),
        };
        out.push(
            Line::toned("goal ", Tone::Dim)
                .push(format!("{} ", m.glyph()), m.tone())
                .push(g.objective.clone(), Tone::Strong)
                .push(format!(" · {tail}"), m.text_tone()),
        );
    }
    if !c.todos.is_empty() {
        let done = c
            .todos
            .iter()
            .filter(|t| t.status == port::TodoStatus::Done)
            .count();
        out.push(Line::toned(
            format!("todo {done}/{}", c.todos.len()),
            Tone::Dim,
        ));
        for i in marks::todo_order(&c.todos) {
            let t = &c.todos[i];
            let (g, tone) = marks::todo(t.status);
            let text_tone = if t.status == port::TodoStatus::Done {
                Tone::Dim
            } else {
                Tone::Normal
            };
            out.push(
                Line::toned("  ", Tone::Normal)
                    .push(format!("{g} "), tone)
                    .push(
                        truncate_to_width(
                            &format!("{}. {}", i + 1, t.text),
                            width.saturating_sub(4),
                        ),
                        text_tone,
                    ),
            );
        }
    }
    out
}

/// 一层一段，段与段用 → 连，同层并行用 ∥；放不下就换行。
fn chain(stations: &[StationView], width: usize, status: bool) -> Vec<Line> {
    let depth = layers(stations);
    let max = depth.iter().copied().max().unwrap_or(0);
    let mut segs: Vec<Line> = vec![];
    for l in 0..=max {
        let mut seg = Line::default();
        for (k, (st, _)) in stations
            .iter()
            .zip(&depth)
            .filter(|(_, d)| **d == l)
            .enumerate()
        {
            if k > 0 {
                seg = seg.push(" ∥ ", Tone::Dim);
            }
            if status {
                let m = marks::station(st.status);
                seg = seg
                    .push(format!("{} ", m.glyph()), m.tone())
                    .push(st.title.clone(), m.name_tone());
            } else {
                seg = seg.push(st.title.clone(), Tone::Accent);
            }
        }
        segs.push(seg);
    }
    let mut out = vec![];
    let mut cur = Line::default();
    for (i, seg) in segs.into_iter().enumerate() {
        let sep = if i == 0 { "" } else { " → " };
        if cur.width() > 0 && cur.width() + display_width(sep) + seg.width() > width {
            out.push(cur.push(" →", Tone::Dim));
            cur = Line::toned("  ", Tone::Normal);
        } else if i > 0 {
            cur = cur.push(sep, Tone::Dim);
        }
        for sp in seg.spans {
            cur = cur.push(sp.text, sp.tone);
        }
    }
    out.push(cur);
    out
}

fn run_body(v: &RunView, width: usize) -> Vec<Line> {
    let mut out = vec![];
    out.extend(chain(&v.stations, width, true));
    for st in &v.stations {
        let m = marks::station(st.status);
        if !matches!(
            m,
            marks::Mark::Active | marks::Mark::Waiting | marks::Mark::Failed | marks::Mark::Stale
        ) {
            continue;
        }
        let mut l = Line::toned(format!("{} ", m.glyph()), m.tone())
            .push(st.title.clone(), m.name_tone())
            .push(format!("  {}", st.work), Tone::Dim)
            .push(
                format!("  {}", crate::flows::station_text(st.status)),
                m.text_tone(),
            );
        if let Some(u) = &st.unmet {
            l = l.push(format!("：{u}"), Tone::Error);
        }
        if !st.tasks.is_empty() {
            let done = st.tasks.iter().filter(|t| t.status.finished()).count();
            l = l.push(
                format!("  子 Agent {done}/{} 完成", st.tasks.len()),
                Tone::Dim,
            );
        }
        out.push(l);
    }
    out
}

/// 站按依赖分层：同一层的可以并行，层号就是图上的列。
pub(crate) fn layers(stations: &[StationView]) -> Vec<usize> {
    let mut depth = vec![0usize; stations.len()];
    for i in 0..stations.len() {
        let needs: Vec<usize> = if stations[i].needs.is_empty() {
            i.checked_sub(1).into_iter().collect()
        } else {
            stations[i]
                .needs
                .iter()
                .filter_map(|n| stations.iter().position(|s| &s.id == n))
                .collect()
        };
        depth[i] = needs.iter().map(|j| depth[*j] + 1).max().unwrap_or(0);
    }
    depth
}

fn flow_body(d: &FlowDetail, inner: usize) -> Vec<Line> {
    let mut out = vec![];
    let depth = layers(&d.stations);
    out.extend(chain(&d.stations, inner, false));
    let title_w = d
        .stations
        .iter()
        .map(|s| display_width(&s.title))
        .max()
        .unwrap_or(4)
        .min(14);
    for (s, l) in d.stations.iter().zip(&depth) {
        let pad = title_w.saturating_sub(display_width(&s.title));
        let mut line = Line::toned(format!("{:>2}. ", l + 1), Tone::Dim)
            .push(format!("{}{}", s.title, " ".repeat(pad)), Tone::Strong)
            .push(format!("  {:<10}", s.work), Tone::Normal);
        if let Some(sk) = &s.skill {
            line = line.push(format!("  skill {sk}"), Tone::Code);
        }
        if !s.exit.is_empty() {
            line = line.push(format!("  出口 {}", s.exit.join("、")), Tone::Dim);
        }
        if s.needs.len() > 1 {
            let names: Vec<&str> = s
                .needs
                .iter()
                .filter_map(|n| {
                    d.stations
                        .iter()
                        .find(|x| &x.id == n)
                        .map(|x| x.title.as_str())
                })
                .collect();
            line = line.push(format!("  ← 等 {}", names.join("、")), Tone::Warn);
        }
        out.push(line);
    }
    out.push(Line::toned(
        "在下面的对话里修改：加一站、改出口条件、调整顺序、启动一次运行…",
        Tone::Dim,
    ));
    out
}

/// 流程里一站的配置：执行方式、skill、出口、前后关系，再加上定义里它那一段 YAML。
fn station_body(d: &FlowDetail, i: usize) -> Vec<Line> {
    let st = &d.stations[i];
    let names = |ids: &[port::StationId]| -> String {
        ids.iter()
            .filter_map(|n| d.stations.iter().find(|x| &x.id == n))
            .map(|x| x.title.as_str())
            .collect::<Vec<_>>()
            .join("、")
    };
    let before = if st.needs.is_empty() {
        i.checked_sub(1)
            .map(|j| d.stations[j].title.clone())
            .unwrap_or_else(|| "（起点）".into())
    } else {
        names(&st.needs)
    };
    let after: Vec<&str> = d
        .stations
        .iter()
        .enumerate()
        .filter(|(j, x)| x.needs.contains(&st.id) || (x.needs.is_empty() && *j == i + 1))
        .map(|(_, x)| x.title.as_str())
        .collect();
    let field = |k: &str, v: String, tone: Tone| Line::toned(pad(k, 8), Tone::Dim).push(v, tone);
    let mut out = vec![
        field("id", st.id.to_string(), Tone::Code),
        field("执行", st.work.clone(), Tone::Normal),
        field(
            "skill",
            st.skill.clone().unwrap_or_else(|| "—".into()),
            Tone::Code,
        ),
        field(
            "出口",
            if st.exit.is_empty() {
                "—".into()
            } else {
                st.exit.join("、")
            },
            Tone::Normal,
        ),
        field("前一站", before, Tone::Normal),
        field(
            "下一站",
            if after.is_empty() {
                "（终点）".into()
            } else {
                after.join("、")
            },
            Tone::Normal,
        ),
    ];
    let yaml = station_yaml(&d.yaml, st.id.as_str());
    if !yaml.is_empty() {
        out.push(Line::toned("定义", Tone::Dim));
        out.extend(
            yaml.into_iter()
                .map(|l| Line::toned(format!("  {l}"), Tone::Code)),
        );
    }
    out
}

/// 从流程 YAML 里摘出某一站那一段（`- id: <id>` 起，到下一个同级条目止）。只为给人看，
/// 不解析：阶段 3 的引擎给结构化的站配置时换掉。
fn station_yaml(yaml: &str, id: &str) -> Vec<String> {
    let lines: Vec<&str> = yaml.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim() == format!("- id: {id}")) else {
        return vec![];
    };
    let indent = lines[start].len() - lines[start].trim_start().len();
    let mut out = vec![lines[start][indent..].to_string()];
    for l in &lines[start + 1..] {
        let ind = l.len() - l.trim_start().len();
        if !l.trim().is_empty() && ind <= indent {
            break;
        }
        out.push(l.get(indent..).unwrap_or(l.trim_start()).to_string());
    }
    out
}

/// 按显示宽度补齐（中文字占两列，`format!("{:<n}")` 按字符数补会错位）。
fn pad(k: &str, w: usize) -> String {
    format!("{k}{}", " ".repeat(w.saturating_sub(display_width(k))))
}

/// `11-16 09:00`（UTC）。
pub(crate) fn fmt_clock(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let (h, m) = (secs % 86_400 / 3600, secs % 3600 / 60);
    // Howard Hinnant 的 civil_from_days。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{mo:02}-{d:02} {h:02}:{m:02}")
}

pub(crate) fn outcome(o: &JobOutcome) -> (String, Tone) {
    let m = marks::outcome(o);
    let text = match o {
        JobOutcome::Running => "运行中".into(),
        JobOutcome::Done => "完成".into(),
        JobOutcome::Failed { message } => format!("失败：{message}"),
        JobOutcome::Skipped => "跳过".into(),
    };
    (format!("{} {text}", m.glyph()), m.tone())
}

/// 作业定义的一行；流程作业的"流程"那一行可以跳到流程标签里那个流程。
pub(crate) enum JobDefRow {
    Plain(Line),
    Flow(String, Line),
}

impl JobDefRow {
    pub fn line(&self) -> &Line {
        match self {
            JobDefRow::Plain(l) | JobDefRow::Flow(_, l) => l,
        }
    }
}

/// 作业的定义（右下）：触发规则、状态，简单作业的提示词与 skills，流程作业的流程与主题。
/// 会话、流程、作业都是项目级的，所以这里不再写项目。
pub(crate) fn job_def(j: &Job, flow_title: Option<&str>) -> Vec<JobDefRow> {
    let field = |k: &str| Line::toned(pad(k, 8), Tone::Dim);
    let next = j
        .next_fire_ms
        .map(|t| format!("下次 {}", fmt_clock(t)))
        .unwrap_or_else(|| "不会再触发".into());
    let mut out = vec![
        JobDefRow::Plain(field("触发").push(j.cron.clone(), Tone::Code).push(
            format!(
                "（{next}）· 错过补跑：{}",
                if j.catch_up { "是" } else { "否" }
            ),
            Tone::Dim,
        )),
        JobDefRow::Plain(field("状态").push(
            if j.enabled {
                "启用 · 空格停用"
            } else {
                "已停用 · 空格启用"
            },
            if j.enabled { Tone::Normal } else { Tone::Dim },
        )),
    ];
    match &j.kind {
        JobKind::Simple {
            prompt,
            skills,
            scene,
            model,
            ..
        } => {
            out.push(JobDefRow::Plain(
                field("提示词").push(prompt.clone(), Tone::Normal),
            ));
            out.push(JobDefRow::Plain(field("skills").push(
                if skills.is_empty() {
                    "—".into()
                } else {
                    skills.join("、")
                },
                Tone::Code,
            )));
            out.push(JobDefRow::Plain(field("场景").push(
                format!(
                    "{} · 模型 {}",
                    scene.as_deref().unwrap_or("缺省"),
                    model.as_deref().unwrap_or("缺省")
                ),
                Tone::Normal,
            )));
        }
        JobKind::Flow {
            flow,
            subject,
            inputs,
        } => {
            out.push(JobDefRow::Flow(
                flow.clone(),
                field("流程")
                    .push(format!("▸ {}", flow_title.unwrap_or(flow)), Tone::Accent)
                    .push("  Enter 跳到这个流程", Tone::Dim),
            ));
            out.push(JobDefRow::Plain(
                field("主题").push(subject.clone(), Tone::Normal),
            ));
            if !inputs.is_null() && inputs.as_object().is_none_or(|o| !o.is_empty()) {
                out.push(JobDefRow::Plain(
                    field("输入").push(inputs.to_string(), Tone::Code),
                ));
            }
        }
    }
    out
}

/// 作业的执行情况（左上面板）：次数、成败、耗时与 token 的统计，再列最近几次。
fn job_body(j: &Job, history: &[JobEntry], inner: usize) -> Vec<Line> {
    let mut out = vec![];
    if history.is_empty() {
        out.push(Line::toned("还没有执行过 · R 立即运行一次", Tone::Dim));
    } else {
        let n = history.len();
        let count = |f: fn(&JobOutcome) -> bool| history.iter().filter(|e| f(&e.outcome)).count();
        let done = count(|o| matches!(o, JobOutcome::Done));
        let failed = count(|o| matches!(o, JobOutcome::Failed { .. }));
        let skipped = count(|o| matches!(o, JobOutcome::Skipped));
        let finished = done + failed;
        let rate = (done * 100).checked_div(finished).unwrap_or(0);
        let ended: Vec<&JobEntry> = history
            .iter()
            .filter(|e| matches!(e.outcome, JobOutcome::Done))
            .collect();
        let avg = |f: fn(&JobEntry) -> u64| {
            if ended.is_empty() {
                0
            } else {
                ended.iter().map(|e| f(e)).sum::<u64>() / ended.len() as u64
            }
        };
        let total: u64 = history.iter().map(|e| e.usage.total()).sum();
        out.push(
            Line::toned(format!("执行 {n} 次 · "), Tone::Normal)
                .push(format!("成功 {done}"), Tone::Ok)
                .push(" · ", Tone::Dim)
                .push(
                    format!("失败 {failed}"),
                    if failed > 0 { Tone::Error } else { Tone::Dim },
                )
                .push(format!(" · 跳过 {skipped} · 成功率 {rate}%"), Tone::Dim),
        );
        out.push(Line::toned(
            format!(
                "成功的平均耗时 {} · 平均 {} tokens · 合计 {}",
                crate::sessions::fmt_elapsed(avg(|e| e.elapsed_ms)),
                crate::chat::fmt_k(avg(|e| e.usage.total())),
                crate::chat::fmt_k(total)
            ),
            Tone::Dim,
        ));
    }
    out.push(Line::toned(
        match j.next_fire_ms {
            Some(t) if j.enabled => format!("下次 {}", fmt_clock(t)),
            _ if !j.enabled => "已停用，不会触发".into(),
            _ => "不会再触发".into(),
        },
        Tone::Dim,
    ));
    for e in history.iter().take(8) {
        let (o, tone) = outcome(&e.outcome);
        let target = match &e.target {
            Some(port::JobTarget::Session { title, .. }) => format!("  → 「{title}」"),
            Some(port::JobTarget::Run { subject, .. }) => format!("  → 运行「{subject}」"),
            None => String::new(),
        };
        out.push(
            Line::toned(format!("{}  ", fmt_clock(e.fired_ms)), Tone::Dim)
                .push(truncate_to_width(&o, inner.saturating_sub(40)), tone)
                .push(
                    format!(
                        "  {}  {}{target}",
                        crate::sessions::fmt_elapsed(e.elapsed_ms),
                        crate::chat::fmt_k(e.usage.total())
                    ),
                    Tone::Dim,
                ),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_utc_month_day() {
        assert_eq!(fmt_clock(1_763_280_000_000), "11-16 08:00");
    }

    fn st(id: &str, needs: &[&str]) -> StationView {
        StationView {
            id: id.into(),
            title: id.into(),
            needs: needs.iter().map(|n| (*n).into()).collect(),
            work: String::new(),
            skill: None,
            exit: vec![],
            status: port::StationStatus::Pending,
            tasks: vec![],
            elapsed_ms: 0,
            usage: Default::default(),
            unmet: None,
            session: None,
        }
    }

    #[test]
    fn layers_follow_needs_and_default_to_previous() {
        let s = vec![
            st("a", &[]),
            st("b", &[]),
            st("c", &["b"]),
            st("d", &["b"]),
            st("e", &["c", "d"]),
        ];
        assert_eq!(layers(&s), vec![0, 1, 2, 2, 3]);
    }
}
