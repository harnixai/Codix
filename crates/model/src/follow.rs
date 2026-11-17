//! 右栏选中即切换（需求：选中带会话的东西，左边对话区跟着切过去）、按需加载、
//! 全局待处理与 Ctrl+G（需求 L-04、L-05）。每次 update 之后跑一遍。

use port::{Command, JobOutcome, RunStatus, SessionId, SessionState, StationStatus};

use crate::flows::{FlowLower, FlowRow};
use crate::sessions::{SessRow, TreeRow};
use crate::state::{Focus, Screen, State, Tab};
use crate::update::{Effect, Pending};

/// 让对话区显示某个会话；`parents` 是面包屑里它上面的那些，`via` 是它对应的子 Agent 任务。
pub(crate) fn show(
    s: &mut State,
    id: SessionId,
    parents: Vec<(SessionId, String)>,
    via: Option<port::TaskId>,
) -> Vec<Effect> {
    if s.ui.chat.as_ref() == Some(&id) && s.ui.ctx_parents == parents && s.ui.task_view.is_none() {
        return vec![];
    }
    s.ui.ctx_parents = parents;
    s.ui.task_view = None;
    let e = crate::chat_update::open_session(s, id);
    s.ui.via_task = via;
    e
}

fn title_of(s: &State, id: &SessionId) -> String {
    s.domain
        .sessions
        .iter()
        .find(|x| &x.id == id)
        .map(|x| x.title.clone())
        .unwrap_or_default()
}

/// 只发一次的加载。
fn load(s: &mut State, key: String, what: Pending, cmd: Command) -> Vec<Effect> {
    if !s.ui.loading.insert(key) {
        return vec![];
    }
    vec![s.call(what, cmd)]
}

pub(crate) fn after_update(s: &mut State) -> Vec<Effect> {
    if s.ui.screen != Screen::Workbench {
        return vec![];
    }
    // 选中流程即展开它的运行；人收起过的同一个流程不再自动展开。先于取数据，好让运行列表
    // 在同一轮里就去取。
    if (s.ui.tab, s.ui.focus) == (Tab::Flows, Focus::SideUpper)
        && let Some(FlowRow::Flow(name)) = s.selected_flow_row()
        && s.ui.flow_auto.as_deref() != Some(name.as_str())
    {
        s.ui.flow_expanded.insert(name.clone());
        s.ui.flow_auto = Some(name);
    }
    if s.ui.tab == Tab::Flows {
        match (s.ui.focus, s.selected_flow_row()) {
            (Focus::SideUpper, _) => s.ui.flow_station = None,
            (Focus::SideLower, Some(FlowRow::Flow(name))) => {
                if let Some(FlowLower::DefStation(i)) =
                    s.flow_lower_rows().get(s.ui.lower().selected).cloned()
                {
                    s.ui.flow_station = Some((name, i));
                }
            }
            _ => {}
        }
    }
    let mut out = data(s);
    out.extend(follow(s));
    out
}

/// 右栏当前看得见的东西缺数据就去取。
fn data(s: &mut State) -> Vec<Effect> {
    let mut out = vec![];
    match s.ui.tab {
        Tab::Flows => {
            let expanded: Vec<String> = s.ui.flow_expanded.iter().cloned().collect();
            for f in expanded {
                if !s.domain.runs.contains_key(&f) {
                    out.extend(load(
                        s,
                        format!("runs:{f}"),
                        Pending::Runs(f.clone()),
                        Command::ListRuns { flow: f },
                    ));
                }
            }
            match s.selected_flow_row() {
                Some(FlowRow::Flow(name)) if !s.domain.flow_details.contains_key(&name) => {
                    out.extend(load(
                        s,
                        format!("flow:{name}"),
                        Pending::FlowDetail(name.clone()),
                        Command::GetFlow { name },
                    ));
                }
                Some(FlowRow::Run(_, id)) if !s.domain.run_views.contains_key(&id) => {
                    out.extend(load(
                        s,
                        format!("run:{id}"),
                        Pending::RunView {
                            id: id.clone(),
                            follow: false,
                        },
                        Command::GetRun { run: id },
                    ));
                }
                _ => {}
            }
        }
        Tab::Jobs => {
            if let Some(id) = s.selected_job()
                && !s.domain.job_history.contains_key(&id)
            {
                out.extend(load(
                    s,
                    format!("history:{id}"),
                    Pending::JobHistory(id.clone()),
                    Command::JobHistory { job: id },
                ));
            }
        }
        Tab::Sessions => out.extend(crate::sessions::ensure_watch(s)),
    }
    out
}

/// 选中变了，对话区跟着切（只在焦点在右栏、是人在选的时候）。
fn follow(s: &mut State) -> Vec<Effect> {
    if !s.ui.focus.in_side() {
        return vec![];
    }
    let key = format!(
        "{:?}:{}:{}:{:?}",
        s.ui.tab,
        s.ui.upper().selected,
        s.ui.lower().selected,
        s.ui.focus
    );
    if s.ui.last_follow.as_deref() == Some(key.as_str()) {
        return vec![];
    }

    let target = target(s);
    // 目标的数据还没到（运行详情、编辑会话）时先不记下，到了再切。
    if target.is_some() {
        s.ui.last_follow = Some(key);
    }
    match target {
        Some(Target::Session(id, parents, via)) => show(s, id, parents, via),
        Some(Target::Output(owner, task, parents)) => {
            if s.ui.task_view.as_ref() == Some(&task) {
                return vec![];
            }
            s.ui.ctx_parents = parents;
            crate::chat_update::open_task_output(s, owner, task)
        }
        Some(Target::Authoring(key, cmd)) => match s.domain.authoring.get(&key).cloned() {
            Some(id) => show(s, id, vec![], None),
            None => load(
                s,
                format!("authoring:{key}"),
                Pending::Authoring { key, follow: true },
                cmd,
            ),
        },
        None => vec![],
    }
}

enum Target {
    Session(SessionId, Vec<(SessionId, String)>, Option<port::TaskId>),
    /// 没有会话的任务：看它的输出（只读）。
    Output(SessionId, port::TaskId, Vec<(SessionId, String)>),
    Authoring(String, Command),
}

fn target(s: &State) -> Option<Target> {
    match (s.ui.tab, s.ui.focus) {
        (Tab::Sessions, Focus::SideUpper) => {
            let row = s.session_rows().get(s.ui.upper().selected).cloned()?;
            Some(Target::Session(row.session()?.clone(), vec![], None))
        }
        (Tab::Sessions, Focus::SideLower) => {
            let root = s.selected_session()?;
            match s.tree_rows().get(s.ui.lower().selected)? {
                TreeRow::Root(id) => Some(Target::Session(id.clone(), vec![], None)),
                TreeRow::Task { id, .. } => {
                    let t = s.task(id)?;
                    let parents = vec![(root.clone(), title_of(s, &root))];
                    Some(match t.agent_session.clone() {
                        Some(child) => Target::Session(child, parents, Some(t.id.clone())),
                        None => Target::Output(root, t.id.clone(), parents),
                    })
                }
                TreeRow::Finished { .. } => None,
            }
        }
        (Tab::Flows, Focus::SideUpper) => match s.selected_flow_row()? {
            FlowRow::Flow(name) => Some(Target::Authoring(
                format!("flow:{name}"),
                Command::OpenAuthoring { flow: Some(name) },
            )),
            FlowRow::Run(_, id) => {
                let v = s.domain.run_views.get(&id)?;
                Some(Target::Session(v.main_session.clone(), vec![], None))
            }
        },
        (Tab::Flows, Focus::SideLower) => {
            let v = s.selected_run()?;
            match s.flow_lower_rows().get(s.ui.lower().selected)? {
                FlowLower::Main => Some(Target::Session(v.main_session.clone(), vec![], None)),
                // 有独立会话的站切到它；其余的站都在运行的主会话里做。
                FlowLower::Station(st) => {
                    let st = v.stations.iter().find(|x| &x.id == st)?;
                    Some(match &st.session {
                        Some(id) => Target::Session(
                            id.clone(),
                            vec![(v.main_session.clone(), v.summary.subject.clone())],
                            None,
                        ),
                        None => Target::Session(v.main_session.clone(), vec![], None),
                    })
                }
                FlowLower::StationTask(st, k) => {
                    let t = v.stations.iter().find(|x| &x.id == st)?.tasks.get(*k)?;
                    let child = t.session.clone()?;
                    Some(Target::Session(
                        child,
                        vec![(v.main_session.clone(), v.summary.subject.clone())],
                        Some(t.id.clone()),
                    ))
                }
                FlowLower::DefStation(_) => None,
            }
        }
        (Tab::Jobs, Focus::SideUpper) => {
            let id = s.selected_job()?;
            Some(Target::Authoring(
                format!("job:{id}"),
                Command::OpenJobSession { job: Some(id) },
            ))
        }
        (Tab::Jobs, Focus::SideLower) => match s.selected_job_target()? {
            port::JobTarget::Session { id, .. } => Some(Target::Session(id, vec![], None)),
            port::JobTarget::Run { id, .. } => {
                let v = s.domain.run_views.get(&id)?;
                Some(Target::Session(v.main_session.clone(), vec![], None))
            }
        },
        _ => None,
    }
}

/// 等你处理的地方（需求 L-04）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attention {
    Session(SessionId),
    Run(String, port::RunId),
    Job(port::JobId),
}

impl State {
    pub fn attention(&self) -> Vec<Attention> {
        let mut out = vec![];
        for x in self.user_sessions() {
            let blocked = self
                .domain
                .convs
                .get(&x.id)
                .and_then(|c| c.goal.as_ref())
                .is_some_and(|g| g.phase == port::GoalPhase::Blocked);
            if x.waiting > 0 || x.state == SessionState::Waiting || blocked {
                out.push(Attention::Session(x.id.clone()));
            }
        }
        for (flow, runs) in &self.domain.runs {
            for r in runs {
                if matches!(r.status, RunStatus::Waiting { .. }) {
                    out.push(Attention::Run(flow.clone(), r.id.clone()));
                }
            }
        }
        for v in self.domain.run_views.values() {
            let stuck = v
                .stations
                .iter()
                .any(|st| matches!(st.status, StationStatus::Held | StationStatus::Unmet));
            let item = Attention::Run(v.summary.flow.clone(), v.summary.id.clone());
            if stuck && !out.contains(&item) {
                out.push(item);
            }
        }
        for j in &self.domain.jobs {
            if matches!(j.last, Some(JobOutcome::Failed { .. })) {
                out.push(Attention::Job(j.id.clone()));
            }
        }
        out
    }

    pub fn attention_count(&self, tab: Tab) -> u32 {
        self.attention()
            .iter()
            .filter(|a| {
                matches!(
                    (a, tab),
                    (Attention::Session(_), Tab::Sessions)
                        | (Attention::Run(..), Tab::Flows)
                        | (Attention::Job(_), Tab::Jobs)
                )
            })
            .count() as u32
    }
}

/// Ctrl+G：跳到下一处待处理（需求 L-05）。
pub(crate) fn next_attention(s: &mut State) -> Vec<Effect> {
    let items = s.attention();
    if items.is_empty() {
        s.notice("没有等你处理的", false);
        return vec![];
    }
    let i = s.ui.attention_cursor % items.len();
    s.ui.attention_cursor = i + 1;
    s.ui.side_collapsed = false;
    s.ui.showing = tui::ShowPane::Side;
    s.ui.focus = Focus::SideUpper;
    s.ui.filter = Default::default();
    match &items[i] {
        Attention::Session(id) => {
            s.ui.tab = Tab::Sessions;
            if let Some(k) = s
                .session_rows()
                .iter()
                .position(|r| r == &SessRow::Session(id.clone()))
            {
                s.ui.upper_mut().selected = k;
            }
        }
        Attention::Run(flow, id) => {
            s.ui.tab = Tab::Flows;
            s.ui.flow_expanded.insert(flow.clone());
            if let Some(k) = s
                .flow_rows()
                .iter()
                .position(|r| r == &FlowRow::Run(flow.clone(), id.clone()))
            {
                s.ui.upper_mut().selected = k;
            }
        }
        Attention::Job(id) => {
            s.ui.tab = Tab::Jobs;
            if let Some(k) = s.job_rows().iter().position(|x| x == id) {
                s.ui.upper_mut().selected = k;
            }
        }
    }
    s.notice(format!("待处理 {}/{}", i + 1, items.len()), false);
    vec![]
}
