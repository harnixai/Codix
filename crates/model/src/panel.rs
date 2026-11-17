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
                if let Some(d) = d {
                    let origin = match d.summary.origin {
                        port::FlowOrigin::Builtin => "内置",
                        port::FlowOrigin::Override => "覆盖",
                        port::FlowOrigin::User => "用户",
                    };
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
                Some(d) => flow_body(d, width),
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

    /// 面板占几行：开着是标题线 + 内容 + 下沿，收起只剩标题线，没有面板是 0。
    pub(crate) fn panel_rows(&self, width: usize) -> u16 {
        if self.panel_here().is_none() {
            0
        } else if self.panel_open() {
            self.panel_content(width).len() as u16 + 2
        } else {
            1
        }
    }

    /// 排进 `height` 行的面板：标题线（右边是 Ctrl+T）、放得下的内容、下沿。
    pub(crate) fn panel_lines(&self, width: usize, height: usize) -> Vec<Line> {
        let Some((id, kind)) = self.panel_here() else {
            return vec![];
        };
        if height == 0 {
            return vec![];
        }
        let open = self.panel_open();
        let title = self.panel_title(&kind, self.domain.convs.get(&id));
        let mut out = vec![rule(
            Some(title),
            if open {
                " Ctrl+T 收起 "
            } else {
                " Ctrl+T 展开 "
            },
            width,
        )];
        if !open || height < 3 {
            return out;
        }
        let room = height - 2;
        let mut body: Vec<Line> = self
            .panel_content(width)
            .iter()
            .map(|l| {
                if l.width() > width {
                    Line::toned(truncate_to_width(&l.text(), width), Tone::Normal)
                } else {
                    l.clone()
                }
            })
            .collect();
        if body.len() > room {
            let hidden = body.len() - (room - 1);
            body.truncate(room - 1);
            body.push(Line::toned(
                format!("… 还有 {hidden} 行（在对话里问详情）"),
                Tone::Dim,
            ));
        }
        out.extend(body);
        out.push(rule(None, "", width));
        out
    }
}

/// `─ 标题 ─────── 右边 ─`：标题太长先截标题，右边的快捷键总在。
fn rule(title: Option<Line>, right: &str, width: usize) -> Line {
    let rw = display_width(right);
    let mut l = Line::default();
    let mut used = 0;
    if let Some(t) = title {
        l = l.push("─ ", Tone::Dim);
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
        l = l.push(" ", Tone::Dim);
    }
    l.push("─".repeat(width.saturating_sub(used + rw + 1)), Tone::Dim)
        .push(right, Tone::Dim)
        .push("─", Tone::Dim)
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
        for t in &c.todos {
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
                        truncate_to_width(&t.text, width.saturating_sub(4)),
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

pub(crate) fn job_info(j: &Job) -> Vec<Line> {
    let next = j
        .next_fire_ms
        .map(|t| format!("下次 {}", fmt_clock(t)))
        .unwrap_or_else(|| "不会再触发".into());
    let mut out = vec![Line::toned(
        format!(
            "{}（{next}）· {} · 错过补跑：{}",
            j.cron,
            if j.enabled { "启用" } else { "已停用" },
            if j.catch_up { "是" } else { "否" }
        ),
        Tone::Normal,
    )];
    match &j.kind {
        JobKind::Simple {
            prompt,
            skills,
            project,
            scene,
            model,
        } => {
            out.push(Line::toned("简单作业：提示词 + skills", Tone::Dim));
            out.push(Line::toned("提示词 ", Tone::Dim).push(prompt.clone(), Tone::Normal));
            out.push(Line::toned(
                format!(
                    "skills {} · 项目 {} · 场景 {} · 模型 {}",
                    if skills.is_empty() {
                        "—".into()
                    } else {
                        skills.join("、")
                    },
                    project.as_deref().unwrap_or("—"),
                    scene.as_deref().unwrap_or("—"),
                    model.as_deref().unwrap_or("缺省")
                ),
                Tone::Dim,
            ));
        }
        JobKind::Flow { flow, subject, .. } => {
            out.push(Line::toned("流程作业", Tone::Dim));
            out.push(Line::toned("流程 ", Tone::Dim).push(flow.clone(), Tone::Normal));
            out.push(Line::toned("主题 ", Tone::Dim).push(subject.clone(), Tone::Normal));
        }
    }
    out
}

fn job_body(j: &Job, history: &[JobEntry], inner: usize) -> Vec<Line> {
    let mut out = job_info(j);
    out.push(Line::toned("最近的触发", Tone::Dim));
    if history.is_empty() {
        out.push(Line::toned("还没有触发过", Tone::Dim));
    }
    for e in history.iter().take(5) {
        let (o, tone) = outcome(&e.outcome);
        out.push(
            Line::toned(format!("{}  ", fmt_clock(e.fired_ms)), Tone::Dim)
                .push(truncate_to_width(&o, inner.saturating_sub(16)), tone),
        );
    }
    out.push(Line::toned(
        "在下面的对话里修改或删除这个作业，或者让它立即跑一次",
        Tone::Dim,
    ));
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
