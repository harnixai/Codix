//! 流程（architecture §8.6）：拷进 `3rds/harnix-flow` 的流程库，装在实例里。内核与存储、
//! 流程包、派工器都用库的；会话由 [`session_host`] 经 Hub 开在嵌入的 Agix 上。端口的流程命令、
//! `flow_*` 宿主工具、流程作业的触发都从这里进。

mod frames;
mod map;
pub(crate) mod session_host;
#[cfg(test)]
mod tests;
pub(crate) mod tools;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use harnix_flow::author::AuthorTools;
use harnix_flow::dryrun::{Sandbox, SandboxConfig};
use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig};
use harnix_flow::{
    Actor, CreateIn, DirPacks, Engine, FileStore, MemPacks, Packs, ProjectLayout, RunQuery,
    RunStatus, Trigger,
};
use port::{Command, Event, Job, PortError, Reply, RunId, RunOp};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::hub::{FlowPort, Hub, tools::Scope};
use session_host::CodixSessionHost;

/// 运行的 worktree 分支：`codix/flow/<运行>`，与引擎自己的 worktree 分开。
const BRANCH_PREFIX: &str = "codix/flow";

/// 项目层目录（`.codix`）里引擎的沙箱让任务会话写的子目录（Agix 0.3.18 起引擎的 worktree 在仓库根的
/// `.wkts/`，不在这里）；其余归应用。Harnix 对着同一个引擎用的也是这两个。
const AGENT_DIRS: [&str; 2] = ["scratchpad", "memory"];

/// 定义里写的场景就是 Agix 的场景名；Codix 只开 `code`。
const SCENE: &str = "code";

pub struct FlowsConfig {
    /// 全局流程包（可写层）：`~/.codix/flows/`。
    pub flows_dir: PathBuf,
    /// 运行记录：`instances/<名>/flow-runs/`。
    pub runs_dir: PathBuf,
    /// 运行的 worktree：`instances/<名>/worktrees/`。
    pub worktrees_dir: PathBuf,
    /// 没有项目的运行在这里各有一个目录。
    pub workspaces_dir: PathBuf,
}

pub struct Flows {
    hub: crate::hub::WeakHub,
    engine: Arc<Engine>,
    runner: Arc<Runner>,
    project: String,
    /// 流程库的编写与运行工具；派工器起来之后才有。
    author: std::sync::OnceLock<AuthorTools>,
    author_host: Arc<tools::CodixAuthorHost>,
    /// 推给过界面的流程名：流程包变了时据此推 `FlowRemoved`。
    published: std::sync::Mutex<BTreeSet<String>>,
}

/// 派工器看到的会话宿主，单独拿出来给一致性套件查（`live_flows.rs`）。
pub fn session_host(hub: &Hub) -> Arc<dyn harnix_flow::runner::SessionHost> {
    CodixSessionHost::new(hub.downgrade())
}

/// 装好流程、接到 Hub 上、起派工器（它把上次没做完的活接着做）。
pub async fn start(hub: &Hub, cfg: FlowsConfig) -> anyhow::Result<Arc<Flows>> {
    let rcfg = RunnerConfig {
        profiles: vec![SCENE.into()],
        submit_profiles: BTreeSet::from([SCENE.to_string()]),
        main_profile: Some(SCENE.into()),
        project_profiles: BTreeSet::from([SCENE.to_string()]),
        hook_headers: "X-Codix".into(),
        ..RunnerConfig::default()
    };
    let layout =
        ProjectLayout::new(".codix", cfg.worktrees_dir, BRANCH_PREFIX).with_agent_dirs(AGENT_DIRS);
    // 随 Codix 发布的流程包：库自带的示例包。
    let dir = DirPacks::writable(cfg.flows_dir).with_builtin(MemPacks::examples());
    let packs = Arc::new(CatalogPacks::over(&rcfg, vec![], Arc::new(dir)));
    packs.set_layout(layout.clone());
    let packs: Arc<dyn Packs> = packs;
    let engine = Arc::new(
        Engine::new(Arc::new(FileStore::new(cfg.runs_dir)), packs)
            .with_lease_ms(rcfg.lease_ms)
            .with_guarded(rcfg.guarded.clone())
            .with_submit_scenes(rcfg.profiles.clone())
            .with_main_scene(SCENE)
            .with_workspaces(cfg.workspaces_dir)
            .with_layout(layout),
    );
    let project = hub.project().to_string();
    match hub.project_trusted(&project).await {
        Ok(t) => rcfg.trust.set(&project, t),
        Err(e) => tracing::warn!("project.status: {e}"),
    }
    let host = CodixSessionHost::new(hub.downgrade());
    let _ = host.engine.set(engine.clone());
    let sandbox = Arc::new(Sandbox::new(
        engine.packs().clone(),
        SandboxConfig {
            guarded: rcfg.guarded.clone(),
            submit_scenes: rcfg.profiles.clone(),
            main_scene: Some(SCENE.into()),
        },
    ));
    let runner = Runner::new(engine.clone(), host, rcfg);
    let flows = Arc::new_cyclic(|me| Flows {
        hub: hub.downgrade(),
        engine,
        runner: runner.clone(),
        project,
        author: std::sync::OnceLock::new(),
        published: Default::default(),
        author_host: Arc::new(tools::CodixAuthorHost::new(me.clone())),
    });
    let _ = flows.author.set(AuthorTools::new(
        &runner,
        sandbox,
        flows.author_host.clone() as Arc<dyn harnix_flow::author::AuthorHost>,
    ));
    tokio::spawn(follow(flows.clone(), runner.changes()));
    hub.set_flows(flows.clone()).await;
    runner.start().await;
    Ok(flows)
}

impl Flows {
    /// 实例收尾：派工器的任务不再等（运行不取消，下次启动接着做）。
    pub async fn stop(&self) {
        if !self.runner.stop(std::time::Duration::from_secs(5)).await {
            tracing::warn!(
                left = self.runner.tasks_left(),
                "flow tasks still running after the stop's grace"
            );
        }
    }

    fn packs(&self) -> Arc<dyn Packs> {
        self.engine.packs_for(Some(&self.project))
    }

    fn by() -> Actor {
        Actor::device("codix")
    }

    pub(crate) async fn summaries(&self) -> Vec<port::FlowSummary> {
        let active = self.active_counts().await;
        harnix_flow::wire::defs_list(self.packs().as_ref())
            .into_iter()
            .map(|d| map::flow_summary(&d, active.get(&d.name).copied().unwrap_or(0)))
            .collect()
    }

    /// 每个流程还没结束的运行有几个。
    async fn active_counts(&self) -> std::collections::HashMap<String, u32> {
        let mut out = std::collections::HashMap::new();
        if let Ok(runs) = self.runs(None, Some(RunStatus::Active)).await {
            for r in runs {
                *out.entry(r.flow).or_insert(0) += 1;
            }
        }
        out
    }

    pub(crate) async fn runs(
        &self,
        flow: Option<&str>,
        status: Option<RunStatus>,
    ) -> harnix_flow::Result<Vec<harnix_flow::RunSummary>> {
        self.engine
            .list(RunQuery {
                flow: flow.map(str::to_string),
                status,
                project: Some(self.project.clone()),
                ..RunQuery::default()
            })
            .await
    }

    async fn summary_of(&self, name: &str) -> Option<port::FlowSummary> {
        self.summaries().await.into_iter().find(|f| f.name == name)
    }

    pub(crate) async fn detail(&self, name: &str) -> Result<port::FlowDetail, PortError> {
        let packs = self.packs();
        let (def, _) = packs
            .flow(name)
            .ok_or_else(|| PortError::Failed(format!("no flow {name}")))?;
        let summary = self
            .summary_of(name)
            .await
            .ok_or_else(|| PortError::Failed(format!("no flow {name}")))?;
        Ok(port::FlowDetail {
            summary,
            stations: map::def_stations(&def),
            yaml: packs.flow_text(name).unwrap_or_default(),
        })
    }

    pub(crate) async fn view(&self, run: &str) -> Result<port::RunView, PortError> {
        let v = self.engine.get(run).await.map_err(flow_err)?;
        Ok(map::run_view(&v))
    }

    pub(crate) async fn start_run(
        &self,
        flow: &str,
        subject: &str,
        inputs: Value,
        trigger: Option<Trigger>,
    ) -> Result<port::RunSummary, PortError> {
        let input = CreateIn {
            flow: flow.to_string(),
            subject: subject.to_string(),
            inputs,
            project_root: Some(self.project.clone()),
            trigger,
            ..CreateIn::default()
        };
        let out = self
            .runner
            .settle(self.runner.start_run(input, Self::by()).await)
            .await
            .map_err(flow_err)?;
        let id = out
            .get("run")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| PortError::Failed(format!("the run started without an id: {out}")))?;
        Ok(self.view(&id).await?.summary)
    }

    pub(crate) async fn op(&self, run: &str, op: RunOp) -> Result<(), PortError> {
        let by = Self::by();
        let e = &self.engine;
        let r = match &op {
            RunOp::Pause => e.pause(run, by).await,
            RunOp::Resume => e.resume(run, by).await,
            RunOp::Cancel => e.cancel(run, "cancelled from Codix", by).await,
            RunOp::Confirm { station } => e.confirm(run, &station.0, true, None, by).await,
            RunOp::Release { station } => e.release(run, &station.0, by).await,
            RunOp::Force { station, reason } => e.force(run, &station.0, reason, by).await,
            RunOp::Rework { to } => e.rework(run, &to.0, None, "rework from Codix", by).await,
            RunOp::Retry { station } => {
                let rec = e.record(run).await.map_err(flow_err)?;
                let task = rec
                    .stations
                    .get(&station.0)
                    .and_then(|s| {
                        s.tasks
                            .iter()
                            .find(|t| t.status == harnix_flow::TaskStatus::Failed)
                    })
                    .map(|t| t.id.clone())
                    .ok_or_else(|| {
                        PortError::Failed(format!("station {} has no failed task", station.0))
                    })?;
                e.retry(run, &station.0, &task, by).await
            }
        };
        self.runner.settle(r).await.map(|_| ()).map_err(flow_err)
    }

    /// 流程包变了（存、删了流程或件）：每个流程的摘要重推一遍，没了的推 `FlowRemoved`。
    pub(crate) async fn flows_changed(&self) {
        let Some(hub) = self.hub.upgrade() else {
            return;
        };
        let now = self.summaries().await;
        let mut events: Vec<Event> = now.iter().cloned().map(Event::FlowUpserted).collect();
        let mut known = self.published.lock().unwrap();
        for gone in known.iter().filter(|n| !now.iter().any(|f| &f.name == *n)) {
            events.push(Event::FlowRemoved { name: gone.clone() });
        }
        *known = now.iter().map(|f| f.name.clone()).collect();
        drop(known);
        hub.emit_events(events);
    }

    /// 定义变了（存、删）：列表与设计卡跟着刷新。
    pub(crate) async fn flow_changed(&self, name: &str) {
        let Some(hub) = self.hub.upgrade() else {
            return;
        };
        match self.detail(name).await {
            Ok(d) => hub.emit_events(vec![
                Event::FlowUpserted(d.summary.clone()),
                Event::FlowDetail(Box::new(d)),
            ]),
            Err(_) => hub.emit_events(vec![Event::FlowRemoved {
                name: name.to_string(),
            }]),
        }
    }
}

pub(crate) fn flow_err(e: harnix_flow::FlowError) -> PortError {
    PortError::Failed(e.to_string())
}

/// 派工器每改了运行就报一次：推运行的摘要与详情，以及它所属流程的摘要（在跑的个数）。
async fn follow(flows: Arc<Flows>, mut rx: broadcast::Receiver<harnix_flow::runner::Changed>) {
    loop {
        let changed = match rx.recv().await {
            Ok(c) => c,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        if !changed.substantive {
            continue;
        }
        let Some(hub) = flows.hub.upgrade() else {
            return;
        };
        let mut seen_flows = BTreeSet::new();
        for run in changed.runs {
            let Ok(v) = flows.view(&run).await else {
                continue;
            };
            seen_flows.insert(v.summary.flow.clone());
            hub.emit_events(vec![
                Event::RunUpserted(v.summary.clone()),
                Event::RunDetail(Box::new(v)),
            ]);
        }
        for f in seen_flows {
            if let Some(s) = flows.summary_of(&f).await {
                hub.emit_events(vec![Event::FlowUpserted(s)]);
            }
        }
    }
}

#[async_trait]
impl FlowPort for Flows {
    async fn command(&self, cmd: Command) -> Result<Reply, PortError> {
        match cmd {
            Command::ListFlows => Ok(Reply::Flows(self.summaries().await)),
            Command::GetFlow { name } => Ok(Reply::Flow(Box::new(self.detail(&name).await?))),
            Command::ListRuns { flow } => {
                // 库的列表摘要不带各站，站数要从运行记录算；读不到记录的退回列表摘要。
                let mut out = vec![];
                for s in self.runs(Some(&flow), None).await.map_err(flow_err)? {
                    out.push(match self.view(&s.id).await {
                        Ok(v) => v.summary,
                        Err(_) => map::run_summary(&s),
                    });
                }
                Ok(Reply::Runs(out))
            }
            Command::GetRun { run } => Ok(Reply::Run(Box::new(self.view(&run.0).await?))),
            Command::StartRun {
                flow,
                subject,
                inputs,
            } => Ok(Reply::RunStarted(
                self.start_run(&flow, &subject, inputs, None).await?,
            )),
            Command::RunOp { run, op } => {
                self.op(&run.0, op).await?;
                Ok(Reply::Ok)
            }
            other => Err(PortError::Unsupported(other.name().into())),
        }
    }

    async fn tool(
        &self,
        rpc: &str,
        scope: &Scope,
        name: &str,
        input: &Value,
    ) -> Option<Result<String, String>> {
        tools::call(self, rpc, scope, name, input).await
    }

    async fn start_for_job(
        &self,
        flow: &str,
        subject: &str,
        inputs: Value,
        job: &Job,
    ) -> Result<(RunId, String), String> {
        let trigger = Trigger::Schedule {
            schedule: job.id.0.clone(),
            fire_at: crate::instance::now_ms(),
        };
        let s = self
            .start_run(flow, subject, inputs, Some(trigger))
            .await
            .map_err(|e| e.to_string())?;
        Ok((s.id, s.subject))
    }

    fn tool_declarations(&self) -> Vec<Value> {
        tools::declarations()
    }

    async fn trust_changed(&self) {
        // hear 问的是引擎此刻的答案，并记进派工器与流程包共用的信任表。
        self.runner.hear(&self.project).await;
        self.flows_changed().await;
    }
}
