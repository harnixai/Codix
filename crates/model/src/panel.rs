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
            None => self.notice(i18n::t!("panel.no_goal_todo"), false),
        }
    }

    /// 面板的标题线左边：它是什么，加一句摘要（收起时也看得到）。
    /// 面板的标题：只是它是什么（状态在别处都有，这里不重复）。
    fn panel_title(&self, kind: &PanelKind) -> Line {
        let text = match kind {
            PanelKind::Flow(name) => {
                let d = self.domain.flow_details.get(name);
                let title = d
                    .map(|d| d.summary.title.clone())
                    .or_else(|| {
                        self.domain
                            .flows
                            .iter()
                            .find(|f| &f.name == name)
                            .map(|f| f.title.clone())
                    })
                    .unwrap_or_else(|| name.clone());
                let mut t = i18n::t!("panel.flow_title", title = title);
                if let (Some(d), Some(i)) = (d, self.station_in_view(name)) {
                    t.push_str(&i18n::t!(
                        "panel.flow_in_station",
                        n = i + 1,
                        title = d.stations[i].title
                    ));
                }
                t
            }
            PanelKind::Job(job) => match self.domain.jobs.iter().find(|j| &j.id == job) {
                Some(j) => i18n::t!("panel.job_title", name = j.name),
                None => i18n::t!("panel.job_new"),
            },
            PanelKind::Run(run) => match self.domain.run_views.get(run) {
                Some(v) => i18n::t!("panel.run_title", subject = v.summary.subject),
                None => i18n::t!("panel.run"),
            },
            PanelKind::Goal => i18n::t!("panel.goal_todo"),
        };
        Line::toned(text, Tone::Strong)
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
                None => vec![Line::toned(i18n::t!("panel.flow_loading"), Tone::Dim)],
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
                None => vec![Line::toned(i18n::t!("panel.job_hint"), Tone::Dim)],
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
        let Some((_, kind)) = self.panel_here() else {
            return vec![];
        };
        if height == 0 {
            return vec![];
        }
        let open = self.panel_open();
        let title = self.panel_title(&kind);
        let focused = self.ui.focus == crate::state::Focus::Panel;
        // 标题靠右；收放的快捷键在下沿右边。收起时只剩一条线，快捷键（灰）就跟在标题右边。
        if !open || height < 3 {
            let right = title.push(format!("  {}", i18n::t!("panel.expand").trim()), Tone::Dim);
            return vec![rule(None, Some(right), width, focused)];
        }
        let mut out = vec![rule(None, Some(title), width, focused)];
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
        let mut hint = i18n::t!("panel.collapse").trim().to_string();
        if off > 0 || below > 0 {
            hint = format!("↑{off} ↓{below} · Shift+↑↓ · {hint}");
        }
        out.push(rule(
            None,
            Some(Line::toned(hint, Tone::Dim)),
            width,
            focused,
        ));
        out
    }
}

/// 面板内容最多几行；再多就滚动。
const PANEL_MAX: usize = 10;

/// `─ 左边 ─────── 右边 ─`：两头各自可空；放不下先截左边，右边总在。面板有焦点时线是亮的。
fn rule(left: Option<Line>, right: Option<Line>, width: usize, focused: bool) -> Line {
    let line_tone = if focused { Tone::Strong } else { Tone::Dim };
    let fit = |t: Line, room: usize| {
        if t.width() > room {
            let tone = t.spans.first().map_or(Tone::Normal, |s| s.tone);
            Line::toned(truncate_to_width(&t.text(), room), tone)
        } else {
            t
        }
    };
    let right = right.map(|r| fit(r, width.saturating_sub(4)));
    let rw = right.as_ref().map_or(0, |r| r.width() + 2);
    let left = left.map(|l| fit(l, width.saturating_sub(rw + 6)));
    let lw = left.as_ref().map_or(0, |l| l.width() + 3);
    let mut l = Line::default();
    if let Some(t) = left {
        l = l.push("─ ", line_tone);
        for sp in t.spans {
            l = l.push(sp.text, sp.tone);
        }
        l = l.push(" ", line_tone);
    }
    l = l.push("─".repeat(width.saturating_sub(lw + rw + 1)), line_tone);
    if let Some(t) = right {
        l = l.push(" ", line_tone);
        for sp in t.spans {
            l = l.push(sp.text, sp.tone);
        }
        l = l.push(" ", line_tone);
    }
    l.push("─", line_tone)
}

fn goal_lines(c: &Conversation, width: usize) -> Vec<Line> {
    let mut out = vec![];
    if let Some(g) = &c.goal {
        let (m, tail) = match g.phase {
            GoalPhase::Active => (
                marks::Mark::Active,
                i18n::t!("panel.goal_round", n = g.rounds_started, max = g.max_rounds),
            ),
            GoalPhase::Paused => (marks::Mark::Paused, i18n::t!("panel.phase.paused")),
            GoalPhase::Blocked => (
                marks::Mark::Waiting,
                i18n::t!(
                    "panel.goal_blocked",
                    reason = g.blocked_reason.as_deref().unwrap_or("")
                ),
            ),
            GoalPhase::Complete => (marks::Mark::Done, i18n::t!("panel.phase.complete")),
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
            l = l.push(i18n::t!("panel.unmet", reason = u), Tone::Error);
        }
        if !st.tasks.is_empty() {
            let done = st.tasks.iter().filter(|t| t.status.finished()).count();
            l = l.push(
                i18n::t!("panel.sub_done", done = done, total = st.tasks.len()),
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
            line = line.push(
                i18n::t!(
                    "panel.exit_list",
                    list = s.exit.join(&i18n::t!("panel.sep"))
                ),
                Tone::Dim,
            );
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
            line = line.push(
                i18n::t!(
                    "panel.waits_for",
                    names = names.join(&i18n::t!("panel.sep"))
                ),
                Tone::Warn,
            );
        }
        out.push(line);
    }
    out.push(Line::toned(i18n::t!("panel.flow_hint"), Tone::Dim));
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
            .join(&i18n::t!("panel.sep"))
    };
    let before = if st.needs.is_empty() {
        i.checked_sub(1)
            .map(|j| d.stations[j].title.clone())
            .unwrap_or_else(|| i18n::t!("panel.start"))
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
    let (l_id, l_work, l_skill, l_exit, l_prev, l_next, l_def) = (
        i18n::t!("panel.field.id"),
        i18n::t!("panel.field.work"),
        i18n::t!("panel.field.skill"),
        i18n::t!("panel.field.exit"),
        i18n::t!("panel.field.prev"),
        i18n::t!("panel.field.next"),
        i18n::t!("panel.field.def"),
    );
    let label_w = label_width(&[&l_id, &l_work, &l_skill, &l_exit, &l_prev, &l_next, &l_def]);
    let field =
        |k: &str, v: String, tone: Tone| Line::toned(pad(k, label_w), Tone::Dim).push(v, tone);
    let mut out = vec![
        field(&l_id, st.id.to_string(), Tone::Code),
        field(&l_work, st.work.clone(), Tone::Normal),
        field(
            &l_skill,
            st.skill.clone().unwrap_or_else(|| "—".into()),
            Tone::Code,
        ),
        field(
            &l_exit,
            if st.exit.is_empty() {
                "—".into()
            } else {
                st.exit.join(&i18n::t!("panel.sep"))
            },
            Tone::Normal,
        ),
        field(&l_prev, before, Tone::Normal),
        field(
            &l_next,
            if after.is_empty() {
                i18n::t!("panel.end")
            } else {
                after.join(&i18n::t!("panel.sep"))
            },
            Tone::Normal,
        ),
    ];
    let yaml = station_yaml(&d.yaml, st.id.as_str());
    // "定义"也是一个字段：YAML 从值那一列起，续行同样缩到那一列，中线和上面一致。
    for (k, l) in yaml.into_iter().enumerate() {
        let name = if k == 0 { l_def.as_str() } else { "" };
        out.push(Line::toned(pad(name, label_w), Tone::Dim).push(l, Tone::Code));
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

/// 字段名一列的宽度：按最长的那个名字。
fn label_width(labels: &[&str]) -> usize {
    labels.iter().map(|k| display_width(k)).max().unwrap_or(0)
}

/// 字段名右对齐、后面跟 `: `，值从同一列起：` skill: implement` / `前一站: 设计`。名字为空
/// （多行值的续行）时只补空格。按显示宽度算（中文字占两列，`format!("{:>n}")` 按字符数补会错位）。
fn pad(k: &str, w: usize) -> String {
    let sep = if k.is_empty() { "  " } else { ": " };
    format!("{}{k}{sep}", " ".repeat(w.saturating_sub(display_width(k))))
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
        JobOutcome::Running => i18n::t!("panel.outcome.running"),
        JobOutcome::Done => i18n::t!("panel.outcome.done"),
        JobOutcome::Failed { message } => i18n::t!("panel.outcome.failed", message = message),
        JobOutcome::Skipped => i18n::t!("panel.outcome.skipped"),
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
    let (l_trigger, l_status) = (
        i18n::t!("panel.job_field.trigger"),
        i18n::t!("panel.job_field.status"),
    );
    let (l_a, l_b, l_c) = match &j.kind {
        JobKind::Simple { .. } => (
            i18n::t!("panel.job_field.prompt"),
            i18n::t!("panel.job_field.skills"),
            i18n::t!("panel.job_field.scene"),
        ),
        JobKind::Flow { .. } => (
            i18n::t!("panel.job_field.flow"),
            i18n::t!("panel.job_field.subject"),
            i18n::t!("panel.job_field.input"),
        ),
    };
    let label_w = label_width(&[&l_trigger, &l_status, &l_a, &l_b, &l_c]);
    let field = |k: &str| Line::toned(pad(k, label_w), Tone::Dim);
    let next = j
        .next_fire_ms
        .map(|t| i18n::t!("panel.next_fire", time = fmt_clock(t)))
        .unwrap_or_else(|| i18n::t!("panel.no_more"));
    let mut out = vec![
        JobDefRow::Plain(field(&l_trigger).push(j.cron.clone(), Tone::Code).push(
            i18n::t!(
                "panel.job_trigger",
                next = next,
                catch_up = if j.catch_up {
                    i18n::t!("panel.yes")
                } else {
                    i18n::t!("panel.no")
                }
            ),
            Tone::Dim,
        )),
        JobDefRow::Plain(field(&l_status).push(
            if j.enabled {
                i18n::t!("panel.job_on_hint")
            } else {
                i18n::t!("panel.job_off_hint")
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
                field(&l_a).push(prompt.clone(), Tone::Normal),
            ));
            out.push(JobDefRow::Plain(field(&l_b).push(
                if skills.is_empty() {
                    "—".into()
                } else {
                    skills.join(&i18n::t!("panel.sep"))
                },
                Tone::Code,
            )));
            let default = i18n::t!("panel.default");
            out.push(JobDefRow::Plain(field(&l_c).push(
                i18n::t!(
                    "panel.job_scene",
                    scene = scene.as_deref().unwrap_or(&default),
                    model = model.as_deref().unwrap_or(&default)
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
                field(&l_a)
                    .push(format!("▸ {}", flow_title.unwrap_or(flow)), Tone::Accent)
                    .push(i18n::t!("panel.job_flow_jump"), Tone::Dim),
            ));
            out.push(JobDefRow::Plain(
                field(&l_b).push(subject.clone(), Tone::Normal),
            ));
            if !inputs.is_null() && inputs.as_object().is_none_or(|o| !o.is_empty()) {
                out.push(JobDefRow::Plain(
                    field(&l_c).push(inputs.to_string(), Tone::Code),
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
        out.push(Line::toned(i18n::t!("panel.no_runs"), Tone::Dim));
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
            Line::toned(i18n::t!("panel.run_count", n = n), Tone::Normal)
                .push(i18n::t!("panel.run_ok", n = done), Tone::Ok)
                .push(" · ", Tone::Dim)
                .push(
                    i18n::t!("panel.run_failed", n = failed),
                    if failed > 0 { Tone::Error } else { Tone::Dim },
                )
                .push(
                    i18n::t!("panel.run_tail", skipped = skipped, rate = rate),
                    Tone::Dim,
                ),
        );
        out.push(Line::toned(
            i18n::t!(
                "panel.run_avg",
                elapsed = crate::sessions::fmt_elapsed(avg(|e| e.elapsed_ms)),
                avg = crate::chat::fmt_k(avg(|e| e.usage.total())),
                total = crate::chat::fmt_k(total)
            ),
            Tone::Dim,
        ));
    }
    out.push(Line::toned(
        match j.next_fire_ms {
            Some(t) if j.enabled => i18n::t!("panel.next_fire", time = fmt_clock(t)),
            _ if !j.enabled => i18n::t!("panel.disabled_no_fire"),
            _ => i18n::t!("panel.no_more"),
        },
        Tone::Dim,
    ));
    for e in history.iter().take(8) {
        let (o, tone) = outcome(&e.outcome);
        let target = match &e.target {
            Some(port::JobTarget::Session { title, .. }) => {
                i18n::t!("panel.target_session", title = title)
            }
            Some(port::JobTarget::Run { subject, .. }) => {
                i18n::t!("panel.target_run", subject = subject)
            }
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
