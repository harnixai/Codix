//! 作业标签（需求 J，交互 §5.4）：上半是作业列表，下半是选中作业的定义、执行统计与触发记录。
//! 作业本身没有会话，选中它左边不动；新建、修改、删除回到用户会话里说（`n` `e` `d` 放话头）。

use keys::Action;
use port::{Command, JobId, JobKind, JobOutcome, JobTarget};
use tui::{Line, Tone};

use crate::panel::{JobDefRow, fmt_clock, job_def, job_stats, outcome};
use crate::state::{Focus, State, Tab};
use crate::update::{Effect, Pending};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobLower {
    Info(usize),
    /// 流程作业的流程：Enter 跳到流程标签里它的位置。
    Flow(String),
    /// 执行统计的一行（只看）。
    Stat(usize),
    Heading,
    Entry(usize),
}

impl State {
    pub fn job_rows(&self) -> Vec<JobId> {
        let filter = &self.ui.filter[Tab::Jobs.index()];
        port::order::jobs(&self.domain.jobs)
            .into_iter()
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
        let history = self.domain.job_history.get(&id);
        let stats = job_stats(history.map_or(&[][..], Vec::as_slice)).len();
        out.extend((0..stats).map(JobLower::Stat));
        out.push(JobLower::Heading);
        let n = history.map_or(0, Vec::len);
        out.extend((0..n).map(JobLower::Entry));
        out
    }

    /// 作业在规范顺序里的序号（`#n`，从 1 起）。
    pub(crate) fn job_number(&self, id: &JobId) -> usize {
        port::order::jobs(&self.domain.jobs)
            .iter()
            .position(|j| &j.id == id)
            .map_or(0, |i| i + 1)
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
    let order: Vec<JobId> = port::order::jobs(&s.domain.jobs)
        .iter()
        .map(|j| j.id.clone())
        .collect();
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
                    .map(|t| i18n::t!("jobs.next_fire", time = fmt_clock(t)))
                    .unwrap_or_default()
            } else {
                i18n::t!("jobs.disabled")
            };
            let (last, ltone) = match &j.last {
                Some(JobOutcome::Failed { .. }) => (i18n::t!("jobs.last_failed"), Tone::Error),
                Some(JobOutcome::Done) => (i18n::t!("jobs.last_done"), Tone::Ok),
                Some(JobOutcome::Running) => (i18n::t!("jobs.running"), Tone::Accent),
                _ => (String::new(), Tone::Dim),
            };
            let n = order.iter().position(|x| x == &j.id).map_or(0, |i| i + 1);
            crate::sessions::split_line(
                Line::toned(crate::sessions::number(n, order.len()), Tone::Dim)
                    .push(format!("{g} "), tone)
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
    let stats = job_stats(&history);
    s.job_lower_rows()
        .iter()
        .map(|r| match r {
            JobLower::Info(i) => line_at(*i),
            JobLower::Flow(_) => line_at(flow_at),
            JobLower::Stat(i) => stats.get(*i).cloned().unwrap_or_default(),
            JobLower::Heading => Line::toned(
                if history.is_empty() {
                    i18n::t!("jobs.history_empty")
                } else {
                    i18n::t!("jobs.history_open")
                },
                Tone::Dim,
            ),
            JobLower::Entry(i) => entry_line(&history[*i], width),
        })
        .collect()
}

/// 触发记录的一行：时刻、结局，右边是耗时、用量与去向。
fn entry_line(e: &port::JobEntry, width: usize) -> Line {
    let (o, tone) = outcome(&e.outcome);
    let target = match &e.target {
        Some(JobTarget::Session { title, .. }) => i18n::t!("jobs.target_session", title = title),
        Some(JobTarget::Run { subject, .. }) => i18n::t!("jobs.target_run", subject = subject),
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

pub(crate) fn action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let id = s.selected_job();
    let job = id
        .as_ref()
        .and_then(|i| s.domain.jobs.iter().find(|j| &j.id == i))
        .cloned();
    let n = job.as_ref().map_or(0, |j| s.job_number(&j.id));
    Some(match (a, job) {
        (Action::ToggleEnabled, Some(j)) => vec![s.call(
            Pending::Ignore,
            Command::SetJobEnabled {
                job: j.id,
                enabled: !j.enabled,
            },
        )],
        (Action::RunNow, Some(j)) => {
            s.notice(i18n::t!("jobs.run_now", name = j.name), false);
            vec![s.call(Pending::Ignore, Command::RunJobNow { job: j.id })]
        }
        (Action::New, _) => prefill(s, &i18n::t!("jobs.prefill_new"), "jobs.notice_tool"),
        (Action::Edit, Some(j)) => prefill(
            s,
            &i18n::t!("jobs.prefill_edit", n = n, name = j.name),
            "jobs.notice_tool",
        ),
        (Action::Delete, Some(j)) => prefill(
            s,
            &i18n::t!("jobs.prefill_delete", n = n, name = j.name),
            "jobs.notice_tool",
        ),
        (Action::View, Some(j)) => {
            let history = s.domain.job_history.get(&j.id).cloned().unwrap_or_default();
            let mut lines: Vec<String> =
                job_def(&j, None).iter().map(|r| r.line().text()).collect();
            lines.push(String::new());
            lines.extend(job_stats(&history).iter().map(Line::text));
            lines.push(String::new());
            lines.push(i18n::t!("jobs.history_all", n = history.len()));
            lines.extend(history.iter().map(|e| entry_line(e, 200).text()));
            s.ui.overlay = Some(crate::state::Overlay::Text {
                title: i18n::t!("jobs.view_title", n = n, name = j.name),
                lines,
                scroll: 0,
            });
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

/// 从流程作业跳到流程标签里它用的那个流程（选中即展开它的运行）。
fn jump_to_flow(s: &mut State, name: &str) {
    s.ui.tab = Tab::Flows;
    s.ui.focus = Focus::SideUpper;
    s.ui.filter[Tab::Flows.index()].clear();
    match s
        .flow_rows()
        .iter()
        .position(|r| r == &crate::flows::FlowRow::Flow(name.to_string()))
    {
        Some(i) => crate::follow::select(s, Tab::Flows, true, i),
        None => s.notice(i18n::t!("jobs.no_flow", name = name), true),
    }
}

/// 回到用户会话，把话头放进输入框（交互 §5.4）：作业、流程本身没有会话，改它们在对话里说。
pub(crate) fn prefill(s: &mut State, text: &str, notice: &str) -> Vec<Effect> {
    let e = crate::follow::back_to_user_session(s);
    s.show_chat_pub();
    s.ui.focus = Focus::Composer;
    s.ui.composer.clear();
    s.ui.composer.insert_str(text);
    s.notice(i18n::tr(notice), false);
    e
}
