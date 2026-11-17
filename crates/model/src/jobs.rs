//! 作业标签（需求 J，交互 §5.4）：上半是作业列表，下半是选中作业的详情与触发历史。
//! 左边是这个作业的编辑会话（顶部面板是作业详情）；新建、修改、删除都在对话里说。

use keys::Action;
use port::{Command, JobId, JobKind, JobOutcome, JobTarget};
use tui::{Line, Tone};

use crate::panel::{JobDefRow, fmt_clock, job_def, outcome};
use crate::state::{Focus, State, Tab};
use crate::update::{Effect, Pending};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobLower {
    Info(usize),
    /// 流程作业的流程：Enter 跳到流程标签里它的位置。
    Flow(String),
    Heading,
    Entry(usize),
}

impl State {
    pub fn job_rows(&self) -> Vec<JobId> {
        let filter = &self.ui.filter[Tab::Jobs.index()];
        self.domain
            .jobs
            .iter()
            .filter(|j| filter.is_empty() || j.name.contains(filter.as_str()))
            .map(|j| j.id.clone())
            .collect()
    }

    pub fn selected_job(&self) -> Option<JobId> {
        self.job_rows()
            .get(self.ui.upper[Tab::Jobs.index()].selected)
            .cloned()
    }

    pub fn job_lower_rows(&self) -> Vec<JobLower> {
        let Some(id) = self.selected_job() else {
            return vec![];
        };
        let Some(job) = self.domain.jobs.iter().find(|j| j.id == id) else {
            return vec![];
        };
        let mut out: Vec<JobLower> = job_def(job, None)
            .iter()
            .enumerate()
            .map(|(i, r)| match r {
                JobDefRow::Flow(name, _) => JobLower::Flow(name.clone()),
                JobDefRow::Plain(_) => JobLower::Info(i),
            })
            .collect();
        out.push(JobLower::Heading);
        let n = self.domain.job_history.get(&id).map_or(0, Vec::len);
        out.extend((0..n).map(JobLower::Entry));
        out
    }

    /// 下半选中的那次触发去向哪里。
    pub(crate) fn selected_job_target(&self) -> Option<JobTarget> {
        let id = self.selected_job()?;
        let rows = self.job_lower_rows();
        match rows.get(self.ui.lower[Tab::Jobs.index()].selected)? {
            JobLower::Entry(i) => self.domain.job_history.get(&id)?.get(*i)?.target.clone(),
            _ => None,
        }
    }
}

pub(crate) fn upper_lines(s: &State, width: usize) -> Vec<Line> {
    s.job_rows()
        .iter()
        .filter_map(|id| s.domain.jobs.iter().find(|j| &j.id == id))
        .map(|j| {
            let m = match (&j.last, j.enabled) {
                (_, false) => crate::marks::Mark::Paused,
                (Some(o @ (JobOutcome::Running | JobOutcome::Failed { .. })), _) => {
                    crate::marks::outcome(o)
                }
                _ => crate::marks::Mark::Idle,
            };
            let (g, tone) = m.pair();

            let next = if j.enabled {
                j.next_fire_ms
                    .map(|t| format!("下次 {}", fmt_clock(t)))
                    .unwrap_or_default()
            } else {
                "已停用".into()
            };
            let (last, ltone) = match &j.last {
                Some(JobOutcome::Failed { .. }) => ("上次 ✗", Tone::Error),
                Some(JobOutcome::Done) => ("上次 ✓", Tone::Ok),
                Some(JobOutcome::Running) => ("运行中", Tone::Accent),
                _ => ("", Tone::Dim),
            };
            crate::sessions::split_line(
                Line::toned(format!("{g} "), tone)
                    .push(j.name.clone(), Tone::Normal)
                    .push(format!("  {}", j.cron), Tone::Dim),
                Line::toned(format!("{next}  "), Tone::Dim).push(last, ltone),
                width,
            )
        })
        .collect()
}

pub(crate) fn lower_lines(s: &State, width: usize) -> Vec<Line> {
    let Some(id) = s.selected_job() else {
        return vec![];
    };
    let Some(job) = s.domain.jobs.iter().find(|j| j.id == id) else {
        return vec![];
    };
    let flow_title = match &job.kind {
        JobKind::Flow { flow, .. } => s
            .domain
            .flows
            .iter()
            .find(|f| &f.name == flow)
            .map(|f| f.title.as_str()),
        _ => None,
    };
    let def = job_def(job, flow_title);
    let line_at = |i: usize| def.get(i).map(|r| r.line().clone()).unwrap_or_default();
    let flow_at = def
        .iter()
        .position(|r| matches!(r, JobDefRow::Flow(..)))
        .unwrap_or(0);
    let history = s.domain.job_history.get(&id).cloned().unwrap_or_default();
    s.job_lower_rows()
        .iter()
        .map(|r| match r {
            JobLower::Info(i) => line_at(*i),
            JobLower::Flow(_) => line_at(flow_at),
            JobLower::Heading => Line::toned(
                if history.is_empty() {
                    "触发记录：还没有"
                } else {
                    "触发记录 · Enter 打开那一次"
                },
                Tone::Dim,
            ),
            JobLower::Entry(i) => {
                let e = &history[*i];
                let (o, tone) = outcome(&e.outcome);
                let target = match &e.target {
                    Some(JobTarget::Session { title, .. }) => format!("→ 会话「{title}」"),
                    Some(JobTarget::Run { subject, .. }) => format!("→ 运行「{subject}」"),
                    None => String::new(),
                };
                crate::sessions::split_line(
                    Line::toned(format!("{}  ", fmt_clock(e.fired_ms)), Tone::Dim).push(o, tone),
                    Line::toned(
                        format!(
                            "{}  {}  {target}",
                            crate::sessions::fmt_elapsed(e.elapsed_ms),
                            crate::chat::fmt_k(e.usage.total())
                        ),
                        Tone::Dim,
                    ),
                    width,
                )
            }
        })
        .collect()
}

pub(crate) fn action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let id = s.selected_job();
    let job = id
        .as_ref()
        .and_then(|i| s.domain.jobs.iter().find(|j| &j.id == i))
        .cloned();
    let prefill = |s: &mut State, text: &str| {
        s.show_chat_pub();
        s.ui.focus = Focus::Composer;
        s.ui.composer.clear();
        s.ui.composer.insert_str(text);
        s.notice("在对话里说清楚就行；阶段 3 由引擎侧的作业工具执行", false);
    };
    Some(match (a, job) {
        (Action::ToggleEnabled, Some(j)) => vec![s.call(
            Pending::Ignore,
            Command::SetJobEnabled {
                job: j.id,
                enabled: !j.enabled,
            },
        )],
        (Action::RunNow, Some(j)) => {
            s.notice(format!("「{}」立即运行一次", j.name), false);
            vec![s.call(Pending::Ignore, Command::RunJobNow { job: j.id })]
        }
        (Action::New, _) => {
            s.domain.authoring.remove("job:");
            s.ui.last_follow = None;
            let e = vec![s.call(
                Pending::Authoring {
                    key: "job:".into(),
                    follow: true,
                },
                Command::OpenJobSession { job: None },
            )];
            prefill(s, "新建一个作业：");
            e
        }
        (Action::Edit, Some(_)) => {
            prefill(s, "把这个作业改成：");
            vec![]
        }
        (Action::Delete, Some(_)) => {
            prefill(s, "删除这个作业");
            vec![]
        }
        (Action::Open, _) if s.ui.focus == Focus::SideLower && selected_flow_link(s).is_some() => {
            let name = selected_flow_link(s)?;
            jump_to_flow(s, &name);
            vec![]
        }
        (Action::Open, _) => {
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            vec![]
        }
        (Action::Retry, _) if s.domain.load_errors[Tab::Jobs.index()].is_some() => {
            s.domain.load_errors[Tab::Jobs.index()] = None;
            vec![s.call(Pending::Jobs, Command::ListJobs)]
        }
        _ => return None,
    })
}

fn selected_flow_link(s: &State) -> Option<String> {
    match s
        .job_lower_rows()
        .get(s.ui.lower[Tab::Jobs.index()].selected)?
    {
        JobLower::Flow(name) => Some(name.clone()),
        _ => None,
    }
}

/// 从流程作业跳到流程标签里它用的那个流程（选中即展开它的运行、左边切到它的编写会话）。
fn jump_to_flow(s: &mut State, name: &str) {
    s.ui.tab = Tab::Flows;
    s.ui.focus = Focus::SideUpper;
    s.ui.filter[Tab::Flows.index()].clear();
    match s
        .flow_rows()
        .iter()
        .position(|r| r == &crate::flows::FlowRow::Flow(name.to_string()))
    {
        Some(i) => s.ui.upper_mut().selected = i,
        None => s.notice(format!("这个项目里没有流程 {name}"), true),
    }
}
