//! 流程标签（需求 F，交互 §5.3）：上半是流程定义（展开是它的运行），下半是选中运行的
//! 主 Agent、各站与站里的子 Agent；选中流程定义时下半列出定义的各站。详情与修改在左边：
//! 选中流程 → 它的编写会话（最上面钉着设计卡），选中运行 → 运行的主会话。

use keys::Action;
use port::{Command, RunId, RunOp, RunStatus, RunSummary, RunView, StationId, StationStatus};
use tui::{Line, Tone, truncate_to_width};

use crate::chat::fmt_k;
use crate::sessions::{fmt_elapsed, task_glyph};
use crate::state::{Confirm, Focus, State, Tab};
use crate::update::{Effect, Pending};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowRow {
    Flow(String),
    Run(String, RunId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowLower {
    /// 运行的主 Agent。
    Main,
    Station(StationId),
    /// 站里的一件任务（子 Agent 有会话）。
    StationTask(StationId, usize),
    /// 选中流程定义时：定义里的一站（只看）。
    DefStation(usize),
}

impl State {
    pub fn flow_rows(&self) -> Vec<FlowRow> {
        let filter = &self.ui.filter[Tab::Flows.index()];
        let mut out = vec![];
        for f in &self.domain.flows {
            if !filter.is_empty()
                && !f.title.contains(filter.as_str())
                && !f.name.contains(filter.as_str())
            {
                continue;
            }
            out.push(FlowRow::Flow(f.name.clone()));
            if self.ui.flow_expanded.contains(&f.name)
                && let Some(runs) = self.domain.runs.get(&f.name)
            {
                out.extend(
                    runs.iter()
                        .map(|r| FlowRow::Run(f.name.clone(), r.id.clone())),
                );
            }
        }
        out
    }

    pub fn selected_flow_row(&self) -> Option<FlowRow> {
        self.flow_rows()
            .get(self.ui.upper[Tab::Flows.index()].selected)
            .cloned()
    }

    pub fn selected_run(&self) -> Option<&RunView> {
        match self.selected_flow_row()? {
            FlowRow::Run(_, id) => self.domain.run_views.get(&id),
            _ => None,
        }
    }

    pub fn flow_lower_rows(&self) -> Vec<FlowLower> {
        match self.selected_flow_row() {
            Some(FlowRow::Run(..)) => {
                let Some(v) = self.selected_run() else {
                    return vec![];
                };
                let mut out = vec![FlowLower::Main];
                for st in &v.stations {
                    out.push(FlowLower::Station(st.id.clone()));
                    out.extend(
                        (0..st.tasks.len()).map(|i| FlowLower::StationTask(st.id.clone(), i)),
                    );
                }
                out
            }
            Some(FlowRow::Flow(name)) => self
                .domain
                .flow_details
                .get(&name)
                .map(|d| (0..d.stations.len()).map(FlowLower::DefStation).collect())
                .unwrap_or_default(),
            None => vec![],
        }
    }

    fn run_summary(&self, flow: &str, id: &RunId) -> Option<&RunSummary> {
        self.domain.runs.get(flow)?.iter().find(|r| &r.id == id)
    }
}

pub(crate) fn run_status(s: &RunStatus) -> (&'static str, String, Tone) {
    match s {
        RunStatus::Running => ("●", "运行中".into(), Tone::Ok),
        RunStatus::Waiting { reason } => ("◐", format!("等待：{reason}"), Tone::Attention),
        RunStatus::Pausing => ("⏸", "暂停中".into(), Tone::Dim),
        RunStatus::Paused => ("⏸", "已暂停".into(), Tone::Dim),
        RunStatus::Finished => ("✓", "已完成".into(), Tone::Dim),
        RunStatus::Cancelled => ("■", "已取消".into(), Tone::Dim),
    }
}

pub(crate) fn station_glyph(s: StationStatus) -> (&'static str, &'static str, Tone) {
    match s {
        StationStatus::Pending => ("○", "待进入", Tone::Dim),
        StationStatus::Held => ("⏸", "搁置 · a 放行", Tone::Attention),
        StationStatus::Working => ("●", "进行中", Tone::Ok),
        StationStatus::Unmet => ("✗", "未达标", Tone::Error),
        StationStatus::Passed => ("✓", "已通过", Tone::Dim),
        StationStatus::Skipped => ("⊘", "已跳过", Tone::Dim),
        StationStatus::Stale => ("↺", "待重做", Tone::Warn),
    }
}

pub(crate) fn upper_lines(s: &State, width: usize) -> Vec<Line> {
    let now = s.ui.now_ms;
    s.flow_rows()
        .iter()
        .map(|r| match r {
            FlowRow::Flow(name) => {
                let Some(f) = s.domain.flows.iter().find(|f| &f.name == name) else {
                    return Line::default();
                };
                let mark = if s.ui.flow_expanded.contains(name) {
                    "▾"
                } else {
                    "▸"
                };
                let origin = match f.origin {
                    port::FlowOrigin::Builtin => "内置",
                    port::FlowOrigin::Override => "覆盖",
                    port::FlowOrigin::User => "用户",
                };
                let running = if f.active_runs > 0 {
                    format!("{} 个在跑", f.active_runs)
                } else {
                    String::new()
                };
                crate::sessions::split_line(
                    Line::toned(format!("{mark} "), Tone::Dim).push(f.title.clone(), Tone::Strong),
                    Line::toned(format!("{}  {origin}  {running}", f.name), Tone::Dim),
                    width,
                )
            }
            FlowRow::Run(flow, id) => {
                let Some(r) = s.run_summary(flow, id) else {
                    return Line::default();
                };
                let (g, text, tone) = run_status(&r.status);
                let end = r.ended_ms.unwrap_or(now);
                let station = r
                    .current
                    .as_ref()
                    .map(|c| format!("站 {}/{} {c}", r.stations_done + 1, r.stations_total))
                    .unwrap_or_default();
                crate::sessions::split_line(
                    Line::toned("    ", Tone::Normal)
                        .push(format!("{g} "), tone)
                        .push(r.subject.clone(), Tone::Normal)
                        .push(format!("  {text}"), tone),
                    Line::toned(
                        format!(
                            "{station}  {}  {}",
                            fmt_elapsed(end.saturating_sub(r.started_ms)),
                            fmt_k(r.usage.total())
                        ),
                        Tone::Dim,
                    ),
                    width,
                )
            }
        })
        .collect()
}

pub(crate) fn lower_lines(s: &State, width: usize) -> Vec<Line> {
    let rows = s.flow_lower_rows();
    if let Some(FlowRow::Flow(name)) = s.selected_flow_row() {
        let Some(d) = s.domain.flow_details.get(&name) else {
            return vec![];
        };
        let depth = crate::cards::layers(&d.stations);
        return d
            .stations
            .iter()
            .zip(depth)
            .map(|(st, l)| {
                Line::plain(truncate_to_width(
                    &format!(
                        "{}{}. {}  {}",
                        "  ".repeat(l.min(4)),
                        l + 1,
                        st.title,
                        st.work
                    ),
                    width,
                ))
            })
            .collect();
    }
    let Some(v) = s.selected_run() else {
        return vec![];
    };
    let depth = crate::cards::layers(&v.stations);
    rows.iter()
        .map(|r| match r {
            FlowLower::Main => {
                let sum = s.domain.sessions.iter().find(|x| x.id == v.main_session);
                let (g, tone) = sum
                    .map(|x| crate::sessions::state_glyph(x.state))
                    .unwrap_or(("○", Tone::Dim));
                crate::sessions::split_line(
                    Line::toned(format!("{g} "), tone).push("主 Agent", Tone::Strong),
                    Line::toned(fmt_k(sum.map_or(0, |x| x.usage.total())), Tone::Dim),
                    width,
                )
            }
            FlowLower::Station(id) => {
                let i = v.stations.iter().position(|x| &x.id == id).unwrap_or(0);
                let st = &v.stations[i];
                let (g, text, tone) = station_glyph(st.status);
                let indent = "  ".repeat(depth[i].min(4));
                let mut left = Line::toned(indent, Tone::Dim)
                    .push(format!("{g} "), tone)
                    .push(st.title.clone(), Tone::Normal)
                    .push(format!("  {}", st.work), Tone::Dim);
                if st.needs.len() > 1 {
                    let names: Vec<&str> = st
                        .needs
                        .iter()
                        .filter_map(|n| {
                            v.stations
                                .iter()
                                .find(|x| &x.id == n)
                                .map(|x| x.title.as_str())
                        })
                        .collect();
                    left = left.push(format!("  ← 等 {}", names.join(", ")), Tone::Dim);
                }
                let mut right = Line::toned(text, tone);
                if let Some(u) = &st.unmet {
                    right = Line::toned(format!("未达标：{u}"), Tone::Error);
                }
                if st.elapsed_ms > 0 {
                    right = right.push(
                        format!(
                            "  {}  {}",
                            fmt_elapsed(st.elapsed_ms),
                            fmt_k(st.usage.total())
                        ),
                        Tone::Dim,
                    );
                }
                crate::sessions::split_line(left, right, width)
            }
            FlowLower::StationTask(id, k) => {
                let i = v.stations.iter().position(|x| &x.id == id).unwrap_or(0);
                let st = &v.stations[i];
                let t = &st.tasks[*k];
                let (g, tone) = task_glyph(t.status);
                let last = *k + 1 == st.tasks.len();
                let indent = "  ".repeat(depth[i].min(4));
                crate::sessions::split_line(
                    Line::toned(
                        format!("{indent}  {}", if last { "└ " } else { "├ " }),
                        Tone::Dim,
                    )
                    .push(format!("{g} "), tone)
                    .push(t.label.clone(), Tone::Normal)
                    .push(
                        if t.session.is_some() {
                            "  子 Agent"
                        } else {
                            ""
                        },
                        Tone::Dim,
                    ),
                    Line::toned(crate::sessions::status_text(t.status), Tone::Dim),
                    width,
                )
            }
            FlowLower::DefStation(_) => Line::default(),
        })
        .collect()
}

pub(crate) fn lower_title(s: &State) -> (Option<String>, Option<String>) {
    match s.selected_flow_row() {
        Some(FlowRow::Run(flow, id)) => {
            let r = s.run_summary(&flow, &id);
            (
                r.map(|r| r.subject.clone()),
                r.map(|r| format!("站 {}/{}", r.stations_done, r.stations_total)),
            )
        }
        Some(FlowRow::Flow(name)) => (
            s.domain
                .flows
                .iter()
                .find(|f| f.name == name)
                .map(|f| format!("{} 的各站", f.title)),
            None,
        ),
        None => (None, None),
    }
}

pub(crate) fn action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    match s.ui.focus {
        Focus::SideUpper => upper_action(s, a),
        Focus::SideLower => lower_action(s, a),
        _ => None,
    }
}

fn run_op(s: &mut State, run: RunId, op: RunOp) -> Vec<Effect> {
    vec![s.call(Pending::Ignore, Command::RunOp { run, op })]
}

fn upper_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let row = s.selected_flow_row();
    Some(match (a, row) {
        (Action::Expand, Some(FlowRow::Flow(name))) => {
            s.ui.flow_expanded.insert(name);
            vec![]
        }
        (Action::Collapse, Some(FlowRow::Flow(name) | FlowRow::Run(name, _))) => {
            s.ui.flow_expanded.remove(&name);
            s.ui.upper_mut().selected = s
                .flow_rows()
                .iter()
                .position(|r| r == &FlowRow::Flow(name.clone()))
                .unwrap_or(0);
            vec![]
        }
        (Action::Open, Some(_)) => {
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            vec![]
        }
        (Action::New, Some(FlowRow::Flow(_) | FlowRow::Run(..))) | (Action::New, None) => {
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            s.ui.composer.clear();
            let text = if matches!(s.selected_flow_row(), Some(FlowRow::Flow(_))) {
                "启动一次运行"
            } else {
                "新建一个流程："
            };
            s.ui.composer.insert_str(text);
            s.notice("在对话里说清楚就行；阶段 3 由引擎侧的流程工具执行", false);
            vec![]
        }
        (Action::View, Some(FlowRow::Flow(name))) => {
            let yaml = s
                .domain
                .flow_details
                .get(&name)
                .map(|d| d.yaml.clone())
                .unwrap_or_default();
            s.ui.overlay = Some(crate::state::Overlay::Text {
                title: format!("{name} 的定义"),
                lines: yaml.lines().map(str::to_string).collect(),
                scroll: 0,
            });
            vec![]
        }
        (Action::PauseResume, Some(FlowRow::Run(flow, id))) => {
            let op = match s.run_summary(&flow, &id).map(|r| &r.status) {
                Some(RunStatus::Paused | RunStatus::Pausing) => RunOp::Resume,
                _ => RunOp::Pause,
            };
            run_op(s, id, op)
        }
        (Action::Close, Some(FlowRow::Run(flow, id))) => {
            let subject = s
                .run_summary(&flow, &id)
                .map(|r| r.subject.clone())
                .unwrap_or_default();
            s.ui.confirm = Some(Confirm::CancelRun(id, subject));
            vec![]
        }
        (Action::Rework, _) if s.domain.load_errors[Tab::Flows.index()].is_some() => {
            s.domain.load_errors[Tab::Flows.index()] = None;
            vec![s.call(Pending::Flows, Command::ListFlows)]
        }
        _ => return None,
    })
}

fn lower_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let run = s.selected_run()?.clone();
    let rows = s.flow_lower_rows();
    let row = rows.get(s.ui.lower[Tab::Flows.index()].selected)?.clone();
    let station = match &row {
        FlowLower::Station(id) | FlowLower::StationTask(id, _) => {
            run.stations.iter().find(|x| &x.id == id).cloned()
        }
        _ => None,
    };
    let id = run.summary.id.clone();
    Some(match (a, station) {
        (Action::Open, _) => {
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            vec![]
        }
        (Action::Confirm, Some(st)) => run_op(s, id, RunOp::Confirm { station: st.id }),
        (Action::Release, Some(st)) => run_op(s, id, RunOp::Release { station: st.id }),
        (Action::Retry, Some(st)) => run_op(s, id, RunOp::Retry { station: st.id }),
        (Action::Force, Some(st)) => {
            s.ui.confirm = Some(Confirm::ForceStation(id, st.id, st.title));
            vec![]
        }
        (Action::Rework, Some(st)) => {
            s.ui.confirm = Some(Confirm::Rework(id, st.id, st.title));
            vec![]
        }
        _ => return None,
    })
}
