//! 会话代理（architecture §8.4）：端口命令 → Agix 方法，Agix 的帧 → 领域事件，再加上作业的
//! 触发。实例是引擎唯一的客户端：它订阅内存里的每个会话，所以状态不依赖 TUI 在不在。

mod history;
mod registry;
mod state;
pub(crate) mod tools;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use async_trait::async_trait;
use port::link::ServerFrame;
use port::{
    Command, Event, GoalAction, InstanceInfo, InstanceName, InstanceState, Job, JobEntry, JobId,
    JobKind, JobOutcome, JobTarget, Origin, PortError, Reply, SessionId, SlashCommand, SubmitMode,
    TaskId, Usage, Watched,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wire::frames::{DaemonEvent, HistoryMessage, SessionEvent, SessionInfo};

pub use registry::{Record, Registry};
use state::{Out, Sess, St, Todo2};

use crate::engine::{Engine, Notification, RpcError};
use crate::jobs::{Fire, JobStore, Scheduler};
use crate::service::{Service, Sink};

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// 流程那一侧（`crate::flows`）：Hub 经它碰流程，不认识流程库的类型。
#[async_trait]
pub(crate) trait FlowPort: Send + Sync {
    /// 端口的流程命令（ListFlows … RunOp）。
    async fn command(&self, cmd: Command) -> Result<Reply, PortError>;
    /// 宿主工具里归流程管的（`flow_*`，以及运行里的会话自己的那三个）；不是它的回 `None`。
    async fn tool(
        &self,
        rpc: &str,
        scope: &tools::Scope,
        name: &str,
        input: &Value,
    ) -> Option<Result<String, String>>;
    /// 流程作业的一次触发：启动一次运行，回运行 ID 与主题。
    async fn start_for_job(
        &self,
        flow: &str,
        subject: &str,
        inputs: Value,
        job: &Job,
    ) -> Result<(port::RunId, String), String>;
    /// 挂在连接上的流程工具的声明。
    fn tool_declarations(&self) -> Vec<Value>;
}

/// 一个会话的帧，原样分给看着它的流程派工器（`Hub::tap`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tapped {
    /// `text`：这一帧是 `turn_start` 时，这一轮是用哪句话起的（经 `run_turn` 发的才知道）。
    Event {
        turn: Option<String>,
        event: Value,
        text: Option<String>,
    },
    /// `run_turn` 回了错误：这一轮不会有 `turn_complete`（错误帧只发给别的订阅者）。
    /// `text`：这一轮还没开始就失败了时，它是用哪句话起的。
    Failed {
        turn: String,
        text: Option<String>,
        kind: String,
        retryable: bool,
        message: String,
    },
}

/// 协议 §9：会话正在跑一轮（或原地重建），`on_busy: "reject"` 的消息没有投递。
const SESSION_BUSY: i64 = -32015;

/// 协议 §9：没有这个会话。
const SESSION_NOT_FOUND: i64 = -32000;

/// 快照里带多少条历史：再早的翻页没有做，500 条足够接上下文。
const HISTORY_TAIL: u64 = 500;

pub struct HubConfig {
    pub instance: String,
    pub host: String,
    pub project: PathBuf,
    pub version: String,
    pub started_ms: u64,
    pub registry: Registry,
    pub jobs: Option<JobStore>,
    pub clock: Clock,
}

#[derive(Clone)]
pub struct Hub(Arc<Inner>);

struct Inner {
    engine: Arc<dyn Engine>,
    instance: String,
    host: String,
    project: String,
    version: String,
    started_ms: u64,
    clock: Clock,
    st: Mutex<St>,
    reg: Mutex<Registry>,
    link: Mutex<Link>,
    jobs: Mutex<Option<Scheduler>>,
    /// 作业触发建的会话 → (作业, 触发时刻, 开始时刻)：那一轮结束时补上结局。
    job_runs: Mutex<std::collections::HashMap<String, (String, u64, u64)>>,
    /// 侧问分叉出的会话：那一轮结束时把回答作为提示推出去。
    side: Mutex<HashSet<String>>,
    default_model: Mutex<String>,
    flows: OnceLock<Arc<dyn FlowPort>>,
    /// 流程派工器看着的会话：每帧分一份给它们。
    taps: Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Tapped>>>>,
    /// 经 `run_turn` 发出、还没开始的轮：会话 → (轮, 那句话)，按发出的先后。
    sent: Mutex<HashMap<String, VecDeque<(String, String)>>>,
}

#[derive(Default)]
struct Link {
    sink: Option<Sink>,
    watched: HashSet<String>,
    /// 接入者每换一次加一；会话每被 Unwatch 一次加一。Watch 要等好几个调用，期间换了
    /// 接入者或被 Unwatch 了，它就不该在收尾时把会话加回去。
    epoch: u64,
    unwatched: std::collections::HashMap<String, u64>,
}

impl Link {
    fn generation(&self, id: &str) -> (u64, u64) {
        (self.epoch, self.unwatched.get(id).copied().unwrap_or(0))
    }
}

fn send(link: &Link, st: &St, events: Vec<Event>) {
    if let Some(sink) = &link.sink {
        for ev in events {
            if wanted(&ev, &link.watched, st) {
                let _ = sink.send(ServerFrame::Event { event: ev });
            }
        }
    }
}

fn rpc_err(e: RpcError) -> PortError {
    match e {
        RpcError::Closed => PortError::Disconnected,
        RpcError::Remote { message, .. } => PortError::Failed(message),
    }
}

fn str_field(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis().max(0) as u64)
}

impl Hub {
    /// 连上引擎后的第一件事：订阅实例级通知、读出这个项目的会话、订阅其中在内存里的，再起作业调度。
    pub async fn start(
        engine: Arc<dyn Engine>,
        mut notes: mpsc::UnboundedReceiver<Notification>,
        cfg: HubConfig,
    ) -> Result<Hub, RpcError> {
        let project = cfg.project.to_string_lossy().into_owned();
        let st = St {
            ident: state::Ident {
                instance: cfg.instance.clone(),
                host: cfg.host.clone(),
            },
            ..St::default()
        };
        let hub = Hub(Arc::new(Inner {
            engine,
            instance: cfg.instance,
            host: cfg.host,
            project,
            version: cfg.version,
            started_ms: cfg.started_ms,
            clock: cfg.clock,
            st: Mutex::new(st),
            reg: Mutex::new(cfg.registry),
            link: Mutex::new(Link::default()),
            jobs: Mutex::new(None),
            job_runs: Mutex::default(),
            side: Mutex::default(),
            default_model: Mutex::default(),
            flows: OnceLock::new(),
            taps: Mutex::default(),
            sent: Mutex::default(),
        }));
        hub.call("daemon.subscribeEvents", json!({})).await?;
        hub.register_tools().await;
        let effective = hub
            .call(
                "config.get",
                json!({"scene": "code", "project_root": hub.0.project, "tier": "effective"}),
            )
            .await;
        if let Err(e) = &effective {
            tracing::warn!("config.get: {e}");
        }
        if let Ok(effective) = effective {
            tracing::debug!(settings = %effective["settings"], "effective settings");
            let m = &effective["settings"]["model"];
            let name = m["model_name"]
                .as_str()
                .or_else(|| m.as_str())
                .unwrap_or("");
            tracing::info!(model = name, "default model");
            *hub.0.default_model.lock().unwrap() = name.to_string();
        }
        hub.load_sessions().await?;
        // 通知在自己的任务里按到达顺序处理：同一会话的帧不能乱序。
        let weak = Arc::downgrade(&hub.0);
        tokio::spawn(async move {
            while let Some(n) = notes.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                Hub(inner).on_notification(n).await;
            }
        });
        if let Some(store) = cfg.jobs {
            let fire = Arc::new(Firer(Arc::downgrade(&hub.0)));
            let sched = Scheduler::start(Arc::new(Mutex::new(store)), hub.0.clock.clone(), fire);
            // 上一次实例退出时还没跑完的触发：那一轮已经不会有结局了。
            {
                let store = sched.store();
                let s = store.lock().unwrap();
                for j in s.list(0) {
                    for e in s.history(&j.id.0, 20) {
                        if e.outcome == JobOutcome::Running {
                            let _ = s.finish(
                                &j.id.0,
                                e.fired_ms,
                                JobOutcome::Failed {
                                    message: "interrupted: the instance stopped".into(),
                                },
                                e.elapsed_ms,
                                e.usage,
                            );
                        }
                    }
                }
            }
            *hub.0.jobs.lock().unwrap() = Some(sched);
        }
        Ok(hub)
    }

    /// 工具挂在连接上：每个会话都有，此后建起来（新建、恢复、重建）的引擎都带上。流程接上后
    /// 再挂一次，连同 `flow_*`。
    async fn register_tools(&self) {
        let mut all = tools::declarations();
        if let (Some(f), Some(a)) = (self.0.flows.get(), all.as_array_mut()) {
            a.extend(f.tool_declarations());
        }
        match self
            .call("daemon.setHostTools", json!({"host_tools": all}))
            .await
        {
            Ok(r) if r["refused"].as_array().is_some_and(|a| !a.is_empty()) => {
                tracing::warn!(refused = %r["refused"], "host tools refused")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("daemon.setHostTools: {e}"),
        }
    }

    /// 接上流程（实例起来时一次）。
    pub(crate) async fn set_flows(&self, flows: Arc<dyn FlowPort>) {
        if self.0.flows.set(flows).is_ok() {
            self.register_tools().await;
        }
    }

    fn flows(&self) -> Option<Arc<dyn FlowPort>> {
        self.0.flows.get().cloned()
    }

    pub(crate) fn downgrade(&self) -> WeakHub {
        WeakHub(Arc::downgrade(&self.0))
    }

    /// 从现在起这个会话的每一帧。会话关掉时流结束。
    pub(crate) fn tap(&self, session: &str) -> mpsc::UnboundedReceiver<Tapped> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.0
            .taps
            .lock()
            .unwrap()
            .entry(session.to_string())
            .or_default()
            .push(tx);
        rx
    }

    fn tap_send(&self, session: &str, t: Tapped) {
        let mut taps = self.0.taps.lock().unwrap();
        if let Some(v) = taps.get_mut(session) {
            v.retain(|tx| tx.send(t.clone()).is_ok());
            if v.is_empty() {
                taps.remove(session);
            }
        }
    }

    /// 这个会话此刻在不在跑一轮、在哪个项目里；不认识就是 `None`。
    pub(crate) fn session_state(&self, id: &str) -> Option<(bool, Option<String>)> {
        let st = self.0.st.lock().unwrap();
        st.sessions.get(id).map(|s| (s.running, s.project.clone()))
    }

    /// 会话在不在：内存里没有就问引擎，在盘上就带回来、订阅上。
    /// 在 Codix 里关掉的不算在：引擎的存储里还有它，但它不该被谁再带回来。
    pub(crate) async fn bring_back(&self, id: &str) -> Result<bool, RpcError> {
        if self.0.st.lock().unwrap().sessions.contains_key(id) {
            return Ok(true);
        }
        if self.0.reg.lock().unwrap().get(id).is_some_and(|r| r.closed) {
            return Ok(false);
        }
        match self.call("session.get", json!({"session_id": id})).await {
            Ok(_) => {}
            Err(RpcError::Remote { code, .. }) if code == SESSION_NOT_FOUND => return Ok(false),
            Err(e) => return Err(e),
        }
        self.adopt(id).await;
        let _ = self.subscribe(id, true).await;
        Ok(self.0.st.lock().unwrap().sessions.contains_key(id))
    }

    /// 流程派工器开的会话：`session_id` 由键算出，同一个键总是同一个会话（跨重启也是）。
    /// 回 `(会话, 原来就有)`。
    pub(crate) async fn open_flow_session(
        &self,
        session_id: &str,
        scene: &str,
        project: Option<&str>,
        options: Value,
        origin: Origin,
        title: Option<String>,
    ) -> Result<(String, bool), RpcError> {
        let mut params = json!({"session_id": session_id, "scene": scene, "options": options});
        if let Some(p) = project {
            params["project_root"] = json!(p);
        }
        let r = self.call("session.create", params).await?;
        let id = str_field(&r, "session_id");
        let existed = r["existing"].as_bool().unwrap_or(false);
        let now = self.now();
        let known = self.0.st.lock().unwrap().sessions.contains_key(&id);
        if !existed || !known {
            self.0.reg.lock().unwrap().update(&id, |rec| {
                rec.origin = origin.clone();
                if rec.title.is_none() {
                    rec.title = title.clone();
                }
                if rec.created_ms == 0 {
                    rec.created_ms = now;
                }
                rec.updated_ms = now;
                rec.closed = false;
            });
        }
        if !known {
            let created = parse_rfc3339_ms(r["created_at"].as_str().unwrap_or("")).unwrap_or(now);
            let title = self
                .0
                .reg
                .lock()
                .unwrap()
                .get(&id)
                .and_then(|r| r.title.clone())
                .or(title);
            let sess = Sess {
                id: id.clone(),
                title: title.unwrap_or_else(|| state::short(&id)),
                project: project.map(str::to_string),
                scene: r["scene"].as_str().unwrap_or(scene).to_string(),
                model: self.0.default_model.lock().unwrap().clone(),
                origin,
                live: true,
                updated_ms: now,
                created_ms: created,
                ..Sess::default()
            };
            self.0.st.lock().unwrap().sessions.insert(id.clone(), sess);
        }
        let _ = self.subscribe(&id, existed).await;
        self.upsert(&id);
        Ok((id, existed))
    }

    /// 派工器往会话里发一句：排在正在跑的那一轮后面。
    pub(crate) fn send_queued(&self, id: &str, text: &str) -> String {
        self.run_turn(id, text, "queue")
    }

    pub(crate) async fn interrupt(&self, id: &str) -> Result<(), RpcError> {
        self.call("session.interrupt", json!({"session_id": id}))
            .await
            .map(|_| ())
    }

    /// 派工器关一个会话：看着它的流结束。
    pub(crate) async fn close_session(&self, id: &str) -> Result<(), PortError> {
        self.dispatch(Command::Close {
            session: SessionId(id.to_string()),
        })
        .await?;
        self.0.taps.lock().unwrap().remove(id);
        Ok(())
    }

    /// 派工器答一个权限问句。
    pub(crate) async fn answer_permission(
        &self,
        id: &str,
        prompt: &str,
        allow: bool,
        reason: Option<String>,
    ) -> Result<(), PortError> {
        self.dispatch(Command::Answer {
            session: SessionId(id.to_string()),
            prompt: port::PromptId(prompt.to_string()),
            answer: if allow {
                port::Answer::Permit
            } else {
                port::Answer::Deny { reason }
            },
        })
        .await
        .map(|_| ())
    }

    pub(crate) async fn append_entry(
        &self,
        id: &str,
        event: &str,
        payload: Value,
    ) -> Result<(), RpcError> {
        self.call(
            "session.appendEntry",
            json!({"session_id": id, "ns": "codix.flow", "event": event, "payload": payload}),
        )
        .await
        .map(|_| ())
    }

    pub(crate) async fn project_trusted(&self, root: &str) -> Result<bool, RpcError> {
        let v = self
            .call("project.status", json!({"project_root": root}))
            .await?;
        Ok(v["trust"].as_str() == Some("trusted"))
    }

    pub(crate) fn project(&self) -> &str {
        &self.0.project
    }

    pub fn shutdown(&self) {
        if let Some(s) = self.0.jobs.lock().unwrap().as_ref() {
            s.shutdown();
        }
    }

    fn now(&self) -> u64 {
        (self.0.clock)()
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.0.engine.call(method, params).await
    }

    fn scheduler(&self) -> Result<Scheduler, PortError> {
        self.0
            .jobs
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| PortError::Unsupported("jobs".into()))
    }

    // ---- 事件出口 ----

    fn emit(&self, out: Out) {
        {
            let st = self.0.st.lock().unwrap();
            let link = self.0.link.lock().unwrap();
            send(&link, &st, out.events);
        }
        self.spawn_todo(out.todo);
    }

    /// 改状态与发出事件在同一把锁里：Watch 收尾时取的快照与之后发出的事件之间没有缝，
    /// 快照里有的不会再作为事件来一遍，快照里没有的一定作为事件来。
    fn apply(&self, f: impl FnOnce(&mut St, &mut Out)) {
        let mut out = Out::default();
        {
            let mut st = self.0.st.lock().unwrap();
            f(&mut st, &mut out);
            let link = self.0.link.lock().unwrap();
            send(&link, &st, std::mem::take(&mut out.events));
        }
        self.spawn_todo(out.todo);
    }

    fn spawn_todo(&self, todo: Vec<Todo2>) {
        for t in todo {
            let hub = self.clone();
            tokio::spawn(async move { hub.follow_up(t).await });
        }
    }

    pub(crate) fn emit_events(&self, events: Vec<Event>) {
        self.emit(Out {
            events,
            todo: vec![],
        });
    }

    fn upsert(&self, id: &str) {
        let s = self.0.st.lock().unwrap().summary(id);
        if let Some(s) = s {
            self.emit_events(vec![Event::SessionUpserted(s)]);
        }
    }

    async fn follow_up(&self, t: Todo2) {
        match t {
            Todo2::HostCall {
                rpc_session,
                call_id,
                name,
                input,
            } => {
                let (content, is_error) = match self.host_tool(&rpc_session, &name, &input).await {
                    Ok(c) => (c, false),
                    Err(e) => (e, true),
                };
                let _ = self
                    .call(
                        "session.respondToolCall",
                        json!({"session_id": rpc_session, "call_id": call_id,
                               "result": {"content": content, "is_error": is_error}}),
                    )
                    .await;
            }
            Todo2::TurnDone {
                session,
                ok,
                text,
                usage,
            } => self.turn_done(&session, ok, text, usage),
            Todo2::Resubscribe { session } => {
                let _ = self.subscribe(&session, false).await;
            }
        }
    }

    // ---- 通知 ----

    async fn on_notification(&self, n: Notification) {
        let now = self.now();
        match n {
            Notification::Session {
                session_id,
                turn_id,
                event,
            } => {
                if self.0.taps.lock().unwrap().contains_key(&session_id) {
                    let text = (event["kind"] == "turn_start")
                        .then(|| self.started(&session_id, turn_id.as_deref()))
                        .flatten();
                    self.tap_send(
                        &session_id,
                        Tapped::Event {
                            turn: turn_id.clone(),
                            event: event.clone(),
                            text,
                        },
                    );
                }
                let ev = SessionEvent::parse(&event);
                self.apply(|st, out| st.on_event(&session_id, &session_id, ev, now, out));
            }
            Notification::Daemon(v) => match DaemonEvent::parse(&v) {
                DaemonEvent::SessionCreated {
                    session_id,
                    project_root,
                }
                | DaemonEvent::SessionResumed {
                    session_id,
                    project_root,
                } => {
                    let known = self.0.st.lock().unwrap().sessions.contains_key(&session_id);
                    if known || project_root.as_deref() == Some(self.0.project.as_str()) {
                        if !known {
                            self.adopt(&session_id).await;
                        }
                        let _ = self.subscribe(&session_id, false).await;
                    }
                }
                DaemonEvent::SessionEvicted { session_id }
                | DaemonEvent::SessionClosed { session_id } => {
                    let mut st = self.0.st.lock().unwrap();
                    if let Some(s) = st.sessions.get_mut(&session_id) {
                        s.live = false;
                        s.running = false;
                    }
                    drop(st);
                    self.upsert(&session_id);
                }
                DaemonEvent::SessionDeleted { session_id } => {
                    self.0.st.lock().unwrap().sessions.remove(&session_id);
                    self.0.reg.lock().unwrap().remove(&session_id);
                    self.emit_events(vec![Event::SessionRemoved {
                        session: SessionId(session_id),
                    }]);
                }
                DaemonEvent::TurnStarted { session_id, .. }
                | DaemonEvent::TurnCompleted { session_id, .. } => {
                    let running = matches!(DaemonEvent::parse(&v), DaemonEvent::TurnStarted { .. });
                    let changed = {
                        let mut st = self.0.st.lock().unwrap();
                        match st.sessions.get_mut(&session_id) {
                            Some(s) if s.running != running => {
                                s.running = running;
                                s.updated_ms = now;
                                true
                            }
                            _ => false,
                        }
                    };
                    if changed {
                        self.upsert(&session_id);
                    }
                }
                DaemonEvent::EventsLagged {} | DaemonEvent::Unknown => {}
            },
        }
    }

    // ---- 会话 ----

    fn sess_from_info(&self, info: &SessionInfo) -> Sess {
        let reg = self.0.reg.lock().unwrap();
        let rec = reg.get(&info.session_id).cloned().unwrap_or_default();
        let model = rec
            .model
            .clone()
            .unwrap_or_else(|| self.0.default_model.lock().unwrap().clone());
        Sess {
            id: info.session_id.clone(),
            title: rec
                .title
                .clone()
                .or_else(|| info.name.clone())
                .unwrap_or_else(|| state::short(&info.session_id)),
            project: info.project_root.clone(),
            scene: info.scene.clone().unwrap_or_else(|| "code".into()),
            model,
            origin: rec.origin.clone(),
            parent: info.parent_session_id.clone(),
            live: info.status == "active",
            running: info.turn_state.as_deref() == Some("running"),
            updated_ms: parse_rfc3339_ms(&info.last_active)
                .or_else(|| parse_rfc3339_ms(&info.created_at))
                .unwrap_or(rec.updated_ms),
            created_ms: Some(rec.created_ms)
                .filter(|ms| *ms > 0)
                .or_else(|| parse_rfc3339_ms(&info.created_at))
                .unwrap_or(rec.updated_ms),
            ..Sess::default()
        }
    }

    async fn load_sessions(&self) -> Result<(), RpcError> {
        let v = self.call("session.list", json!({})).await?;
        let infos: Vec<SessionInfo> =
            serde_json::from_value(v["sessions"].clone()).unwrap_or_default();
        let mut live = vec![];
        for info in infos
            .iter()
            .filter(|i| i.project_root.as_deref() == Some(self.0.project.as_str()))
        {
            if self
                .0
                .reg
                .lock()
                .unwrap()
                .get(&info.session_id)
                .is_some_and(|r| r.closed)
            {
                continue;
            }
            let s = self.sess_from_info(info);
            if s.live {
                live.push(s.id.clone());
            }
            self.0.st.lock().unwrap().sessions.insert(s.id.clone(), s);
        }
        for id in live {
            let _ = self.subscribe(&id, false).await;
        }
        Ok(())
    }

    /// 不是这个实例建的会话出现在内存里（比如别的连接、或恢复）时，补一份摘要。
    async fn adopt(&self, id: &str) {
        let Ok(v) = self.call("session.get", json!({"session_id": id})).await else {
            return;
        };
        if let Ok(info) = serde_json::from_value::<SessionInfo>(v) {
            let s = self.sess_from_info(&info);
            // 和建会话赛跑时别把那边已有的状态（悬着的提问等）盖掉。
            self.0
                .st
                .lock()
                .unwrap()
                .sessions
                .entry(id.to_string())
                .or_insert(s);
            self.upsert(id);
        }
    }

    /// 订阅一个会话；订阅时还悬着的提问照原样补进来。
    async fn subscribe(&self, id: &str, resume: bool) -> Result<(), RpcError> {
        let v = self
            .call(
                "session.subscribe",
                json!({"session_id": id, "resume": resume}),
            )
            .await?;
        let now = self.now();
        self.apply(|st, out| {
            if let Some(s) = st.sessions.get_mut(id) {
                s.live = true;
            }
            for p in v["pending_prompts"].as_array().into_iter().flatten() {
                st.on_event(id, id, SessionEvent::parse(p), now, out);
            }
        });
        Ok(())
    }

    async fn create_session(
        &self,
        origin: Origin,
        title: Option<String>,
        scene: Option<String>,
        model: Option<String>,
        extra: Value,
    ) -> Result<String, PortError> {
        let mut options = json!({
            "prompt_types": ["permission", "question"],
            "permission_rules": tools::read_only_rules(),
        });
        if let (Some(o), Some(e)) = (options.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        if let Some(m) = &model {
            options["model"] = json!(m);
        }
        let mut params = json!({"project_root": self.0.project, "options": options});
        if let Some(sc) = &scene {
            params["scene"] = json!(sc);
        }
        let r = self.call("session.create", params).await.map_err(rpc_err)?;
        let id = str_field(&r, "session_id");
        let now = self.now();
        self.0.reg.lock().unwrap().update(&id, |rec| {
            rec.origin = origin.clone();
            rec.title = title.clone();
            rec.model = model.clone();
            rec.updated_ms = now;
            rec.created_ms = now;
        });
        let sess = Sess {
            id: id.clone(),
            title: title.unwrap_or_else(|| state::short(&id)),
            project: Some(self.0.project.clone()),
            scene: r["scene"]
                .as_str()
                .map(str::to_string)
                .or(scene)
                .unwrap_or_else(|| "code".into()),
            model: model.unwrap_or_else(|| self.0.default_model.lock().unwrap().clone()),
            origin,
            live: true,
            updated_ms: now,
            created_ms: now,
            ..Sess::default()
        };
        self.0.st.lock().unwrap().sessions.insert(id.clone(), sess);
        let _ = self.subscribe(&id, false).await;
        self.upsert(&id);
        Ok(id)
    }

    fn summary(&self, id: &str) -> Result<port::SessionSummary, PortError> {
        self.0
            .st
            .lock()
            .unwrap()
            .summary(id)
            .ok_or_else(|| PortError::Failed(format!("no session {id}")))
    }

    async fn ensure_known(&self, id: &str) -> Result<(), PortError> {
        if !self.0.st.lock().unwrap().sessions.contains_key(id) {
            self.adopt(id).await;
        }
        if self.0.st.lock().unwrap().sessions.contains_key(id) {
            Ok(())
        } else {
            Err(PortError::Failed(format!("no session {id}")))
        }
    }

    async fn history(&self, id: &str) -> Vec<HistoryMessage> {
        let first = self
            .call(
                "session.history",
                json!({"session_id": id, "offset": 0, "limit": HISTORY_TAIL}),
            )
            .await;
        let Ok(first) = first else { return vec![] };
        let total = first["total"].as_u64().unwrap_or(0);
        let page = if total > HISTORY_TAIL {
            self.call(
                "session.history",
                json!({"session_id": id, "offset": total - HISTORY_TAIL, "limit": HISTORY_TAIL}),
            )
            .await
            .unwrap_or(first)
        } else {
            first
        };
        serde_json::from_value(page["messages"].clone()).unwrap_or_default()
    }

    async fn watch(&self, id: &str) -> Result<Reply, PortError> {
        self.ensure_known(id).await?;
        let gone = || PortError::Failed(format!("no session {id}"));
        let generation = self.0.link.lock().unwrap().generation(id);
        let (parent, was_live) = {
            let st = self.0.st.lock().unwrap();
            let s = st.sessions.get(id).ok_or_else(gone)?;
            (s.parent.clone(), s.live)
        };
        let mut running_hint = None;
        match &parent {
            None => {
                self.subscribe(id, true).await.map_err(rpc_err)?;
                if let Ok(g) = self.call("session.get", json!({"session_id": id})).await {
                    running_hint = Some(g["turn_state"].as_str() == Some("running"));
                }
                if let Ok(g) = self.call("session.goal", json!({"session_id": id})).await {
                    let goal = serde_json::from_value::<wire::frames::GoalView>(g["goal"].clone())
                        .ok()
                        .map(|g| state::goal_of(&g));
                    if let Some(s) = self.0.st.lock().unwrap().sessions.get_mut(id) {
                        s.goal = goal;
                    }
                }
                if let Ok(t) = self.call("session.tasks", json!({"session_id": id})).await {
                    let infos: Vec<wire::frames::TaskInfo> =
                        serde_json::from_value(t["tasks"].clone()).unwrap_or_default();
                    let now = self.now();
                    self.apply(|st, out| {
                        for i in infos {
                            st.on_event(id, id, SessionEvent::TaskUpdated { task: i }, now, out);
                        }
                    });
                }
            }
            // 侧链的帧经父会话来：父会话订阅着就够了。
            Some(p) => {
                let _ = self.subscribe(p, false).await;
            }
        }
        let msgs = self.history(id).await;
        let conv = history::convert(&msgs);
        // 收尾在 st 与 link 两把锁里一次做完：之后的帧都作为事件发出，之前的都在快照里。
        let mut st = self.0.st.lock().unwrap();
        let s = st.sessions.get_mut(id).ok_or_else(gone)?;
        // 实例早就订阅着的会话，它自己跟着帧记的状态比刚才那次 session.get 新。
        if !was_live && let Some(r) = running_hint {
            s.running = r;
        }
        if s.todos.is_empty()
            && let Some(t) = conv.todos
        {
            s.todos = t;
        }
        let mut items = conv.items;
        // 每一步在 step_end 时连同工具结果落盘；正在写的这一步还没进历史，补上已经到了的那段。
        let pending = s.pending_step_text();
        if s.running && !pending.is_empty() {
            items.push(port::HistoryItem::Assistant { text: pending });
        }
        let s = &st.sessions[id];
        let watched = Watched {
            session: st.summary(id).ok_or_else(gone)?,
            history: items,
            running: s.running,
            goal: s.goal.clone(),
            todos: s.todos.clone(),
            tasks: s.tasks.values().cloned().collect(),
            prompts: s.prompts.clone(),
        };
        let mut link = self.0.link.lock().unwrap();
        if link.generation(id) == generation {
            link.watched.insert(id.to_string());
        }
        Ok(Reply::Watched(Box::new(watched)))
    }

    /// `run_turn` 一跑就是几分钟：另起任务等它，错误响应就是这一轮的失败（协议 §5.5）。
    fn run_turn(&self, id: &str, text: &str, on_busy: &str) -> String {
        let turn = format!("cx-{}-{}", self.now(), rand_suffix());
        if on_busy != "steer" {
            self.0
                .sent
                .lock()
                .unwrap()
                .entry(id.to_string())
                .or_default()
                .push_back((turn.clone(), text.to_string()));
        }
        let hub = self.clone();
        let (id, text, on_busy, t) = (
            id.to_string(),
            text.to_string(),
            on_busy.to_string(),
            turn.clone(),
        );
        tokio::spawn(async move {
            let r = hub
                .call(
                    "session.run_turn",
                    json!({"session_id": id, "message": text, "turn_id": t, "on_busy": on_busy}),
                )
                .await;
            match r {
                Ok(v) => {
                    if let Some(name) = v["name"].as_str() {
                        hub.retitle(&id, name, Titler::Engine);
                    }
                }
                // 会话正忙、消息没有投递（协议 §5.3）：不是这一轮失败，是这句话被退回了。
                Err(RpcError::Remote {
                    code: SESSION_BUSY, ..
                }) => {
                    hub.emit_events(vec![Event::SteerReturned {
                        session: SessionId(id.clone()),
                        messages: vec![port::MessageId(t.clone())],
                    }]);
                }
                Err(RpcError::Remote { message, data, .. }) => {
                    let text = hub.forget_sent(&id, &t);
                    hub.tap_send(
                        &id,
                        Tapped::Failed {
                            turn: t.clone(),
                            text,
                            kind: data["kind"].as_str().unwrap_or("error").to_string(),
                            retryable: data["retryable"].as_bool().unwrap_or(true),
                            message: message.clone(),
                        },
                    );
                    let now = hub.now();
                    hub.apply(|st, out| st.turn_failed(&id, message, now, out));
                }
                Err(RpcError::Closed) => {}
            }
        });
        turn
    }

    /// 一轮开始了：它是经 `run_turn` 发的哪一句。认得轮 id 就按 id 取；不认得（引擎自己
    /// 起的，或 id 没对上）就取最早发出的那一句。
    fn started(&self, session: &str, turn: Option<&str>) -> Option<String> {
        let mut sent = self.0.sent.lock().unwrap();
        let q = sent.get_mut(session)?;
        let i = turn
            .and_then(|t| q.iter().position(|(id, _)| id == t))
            .unwrap_or(0);
        let (_, text) = q.remove(i)?;
        if q.is_empty() {
            sent.remove(session);
        }
        Some(text)
    }

    fn forget_sent(&self, session: &str, turn: &str) -> Option<String> {
        let mut sent = self.0.sent.lock().unwrap();
        let q = sent.get_mut(session)?;
        let i = q.iter().position(|(id, _)| id == turn)?;
        q.remove(i).map(|(_, text)| text)
    }

    /// 起名：用户改的名字总算数；引擎首轮自动起的名字只替掉"第一句话"那种临时名字；
    /// 第一句话只在还没有名字时用（Agix 不报冷会话的名字，没有它列表里只剩 id）。
    fn retitle(&self, id: &str, title: &str, by: Titler) {
        {
            let mut reg = self.0.reg.lock().unwrap();
            let rec = reg.get(id).cloned().unwrap_or_default();
            let keep = match by {
                Titler::User => false,
                Titler::Engine => rec.title.is_some() && !rec.title_from_text,
                Titler::FirstText => rec.title.is_some(),
            };
            if keep {
                return;
            }
            reg.update(id, |r| {
                r.title = Some(title.to_string());
                r.title_from_text = by == Titler::FirstText;
            });
        }
        if let Some(s) = self.0.st.lock().unwrap().sessions.get_mut(id) {
            s.title = title.to_string();
        }
        self.upsert(id);
    }

    fn turn_done(&self, session: &str, ok: bool, text: String, usage: Usage) {
        if self.0.side.lock().unwrap().remove(session) {
            let first: String = text.chars().take(400).collect();
            self.emit_events(vec![Event::Notice {
                level: if ok {
                    port::NoticeLevel::Info
                } else {
                    port::NoticeLevel::Error
                },
                text: first,
            }]);
        }
        let run = self.0.job_runs.lock().unwrap().remove(session);
        if let (Some((job, fired, started)), Ok(sched)) = (run, self.scheduler()) {
            let outcome = if ok {
                JobOutcome::Done
            } else {
                JobOutcome::Failed { message: text }
            };
            let elapsed = self.now().saturating_sub(started);
            let store = sched.store();
            let entry = {
                let s = store.lock().unwrap();
                let _ = s.finish(&job, fired, outcome, elapsed, usage);
                s.history(&job, 50)
                    .into_iter()
                    .find(|e| e.fired_ms == fired)
            };
            let mut events = vec![];
            if let Some(entry) = entry {
                events.push(Event::JobFired {
                    job: JobId(job.clone()),
                    entry,
                });
            }
            if let Some(j) = sched.get(&job) {
                events.push(Event::JobUpserted(j));
            }
            self.emit_events(events);
        }
    }

    // ---- 作业 ----

    async fn fire_job(&self, job: Job, fired_ms: u64) {
        let Ok(sched) = self.scheduler() else { return };
        let store = sched.store();
        let (entry, run) = match &job.kind {
            JobKind::Simple {
                prompt,
                skills,
                scene,
                model,
                ..
            } => {
                let mut extra = json!({"attended": false, "auto_name": false});
                if !skills.is_empty() {
                    extra["prompt"] = json!({"append": format!(
                        "This is the scheduled job \"{}\". Use these skills: {}.",
                        job.name,
                        skills.join(", ")
                    )});
                }
                let origin = Origin::Job {
                    job: job.id.clone(),
                    name: job.name.clone(),
                    fired_ms,
                };
                match self
                    .create_session(
                        origin,
                        Some(job.name.clone()),
                        scene.clone(),
                        model.clone(),
                        extra,
                    )
                    .await
                {
                    Ok(sid) => (
                        JobEntry {
                            fired_ms,
                            outcome: JobOutcome::Running,
                            elapsed_ms: 0,
                            usage: Usage::default(),
                            target: Some(JobTarget::Session {
                                id: SessionId(sid.clone()),
                                title: job.name.clone(),
                            }),
                        },
                        Some((sid, prompt.clone())),
                    ),
                    Err(e) => (
                        JobEntry {
                            fired_ms,
                            outcome: JobOutcome::Failed {
                                message: e.to_string(),
                            },
                            elapsed_ms: 0,
                            usage: Usage::default(),
                            target: None,
                        },
                        None,
                    ),
                }
            }
            // 流程作业：启动一次运行。运行有它自己的结局，触发记录只记“启动了没有”。
            JobKind::Flow {
                flow,
                subject,
                inputs,
            } => {
                let started = match self.flows() {
                    Some(f) => f.start_for_job(flow, subject, inputs.clone(), &job).await,
                    None => Err("flows are not available in this instance".into()),
                };
                let (outcome, target) = match started {
                    Ok((id, subject)) => (JobOutcome::Done, Some(JobTarget::Run { id, subject })),
                    Err(message) => (JobOutcome::Failed { message }, None),
                };
                (
                    JobEntry {
                        fired_ms,
                        outcome,
                        elapsed_ms: 0,
                        usage: Usage::default(),
                        target,
                    },
                    None,
                )
            }
        };
        let _ = store.lock().unwrap().append(&job.id.0, &entry);
        let mut events = vec![Event::JobFired {
            job: job.id.clone(),
            entry,
        }];
        if let Some(j) = sched.get(&job.id.0) {
            events.push(Event::JobUpserted(j));
        }
        self.emit_events(events);
        if let Some((sid, prompt)) = run {
            self.0
                .job_runs
                .lock()
                .unwrap()
                .insert(sid.clone(), (job.id.0.clone(), fired_ms, self.now()));
            self.run_turn(&sid, &prompt, "queue");
        }
    }

    /// 宿主工具（tools.rs）：先按发起调用的会话定作用域，再经和端口命令同一条路执行，
    /// 事件照常推给 TUI。
    async fn host_tool(&self, rpc: &str, name: &str, input: &Value) -> Result<String, String> {
        let scope = {
            let st = self.0.st.lock().unwrap();
            tools::scope_of(st.sessions.get(rpc).map(|s| &s.origin))
        };
        if scope == tools::Scope::None {
            return Err("this session is not one of this Codix instance's sessions".into());
        }
        if let Some(f) = self.flows()
            && let Some(r) = f.tool(rpc, &scope, name, input).await
        {
            return r;
        }
        let port_err = |e: PortError| e.to_string();
        match name {
            tools::SESSION_LIST => {
                let all = input.get("all").and_then(Value::as_bool).unwrap_or(false);
                let sums = self.top_sessions();
                let mut lines: Vec<String> = port::order::sessions(&sums)
                    .iter()
                    .enumerate()
                    .map(|(i, s)| session_line(Some(i + 1), s))
                    .collect();
                if all {
                    lines.extend(
                        sums.iter()
                            .filter(|s| !port::order::is_user_session(s))
                            .map(|s| session_line(None, s)),
                    );
                }
                Ok(if lines.is_empty() {
                    "no sessions".into()
                } else {
                    lines.join("\n")
                })
            }
            tools::SESSION_SHOW => {
                let id = self.session_target(input)?;
                let st = self.0.st.lock().unwrap();
                let (Some(sum), Some(s)) = (st.summary(&id), st.sessions.get(&id)) else {
                    return Err(format!("no session {id}"));
                };
                Ok(serde_json::to_string_pretty(&json!({
                    "id": sum.id, "title": sum.title, "state": sum.state, "origin": sum.provenance.origin,
                    "scene": sum.provenance.scene, "model": sum.provenance.model, "usage": sum.usage,
                    "goal": s.goal, "todos": s.todos, "tasks": s.tasks.values().collect::<Vec<_>>(),
                }))
                .unwrap_or_default())
            }
            tools::SESSION_RENAME => {
                let title = tools::str_of(input, "title")
                    .filter(|t| !t.trim().is_empty())
                    .ok_or("title must not be empty")?;
                let id = match input.get("target") {
                    Some(_) => self.session_target(input)?,
                    None => rpc.to_string(),
                };
                self.may_touch_session(&scope, rpc, &id)?;
                self.dispatch(Command::Rename {
                    session: SessionId(id.clone()),
                    title: title.clone(),
                })
                .await
                .map_err(port_err)?;
                Ok(format!("renamed {id} to \"{title}\""))
            }
            tools::SESSION_CLOSE => {
                let id = self.session_target(input)?;
                self.may_touch_session(&scope, rpc, &id)?;
                let title = self.summary(&id).map(|s| s.title).unwrap_or_default();
                self.dispatch(Command::Close {
                    session: SessionId(id.clone()),
                })
                .await
                .map_err(port_err)?;
                Ok(format!("closed \"{title}\" ({id})"))
            }
            tools::JOB_LIST => {
                let sched = self.scheduler().map_err(port_err)?;
                let jobs = sched.list();
                let lines: Vec<String> = port::order::jobs(&jobs)
                    .iter()
                    .enumerate()
                    .map(|(i, j)| job_line(i + 1, j))
                    .collect();
                Ok(if lines.is_empty() {
                    "no jobs".into()
                } else {
                    lines.join("\n")
                })
            }
            tools::JOB_SHOW => {
                let sched = self.scheduler().map_err(port_err)?;
                let job = job_target(&sched.list(), input)?;
                Ok(serde_json::to_string_pretty(&json!({
                    "job": job,
                    "recent_firings": sched.history(&job.id.0, 10),
                }))
                .unwrap_or_default())
            }
            tools::JOB_SAVE => {
                let sched = self.scheduler().map_err(port_err)?;
                let existing = match input.get("target") {
                    Some(_) => Some(job_target(&sched.list(), input)?),
                    None => None,
                };
                match (&scope, &existing) {
                    (tools::Scope::Full, _) => {}
                    (tools::Scope::Job(mine), Some(j)) if &j.id.0 == mine => {}
                    _ => return Err(out_of_scope(&scope)),
                }
                let job = tools::job_from_input(input, existing)?;
                let Reply::Job(saved) = self
                    .dispatch(Command::SaveJob { job })
                    .await
                    .map_err(port_err)?
                else {
                    return Err("the job was not saved".into());
                };
                Ok(format!(
                    "saved \"{}\" ({})\n{}",
                    saved.name,
                    saved.id,
                    serde_json::to_string_pretty(&saved).unwrap_or_default()
                ))
            }
            tools::JOB_DELETE | tools::JOB_ENABLE | tools::JOB_RUN => {
                let sched = self.scheduler().map_err(port_err)?;
                let job = job_target(&sched.list(), input)?;
                match &scope {
                    tools::Scope::Full => {}
                    tools::Scope::Job(mine) if mine == &job.id.0 => {}
                    _ => return Err(out_of_scope(&scope)),
                }
                let (cmd, done) = match name {
                    tools::JOB_DELETE => (
                        Command::DeleteJob {
                            job: job.id.clone(),
                        },
                        "deleted",
                    ),
                    tools::JOB_ENABLE => {
                        let enabled = input
                            .get("enabled")
                            .and_then(Value::as_bool)
                            .ok_or("enabled is required")?;
                        (
                            Command::SetJobEnabled {
                                job: job.id.clone(),
                                enabled,
                            },
                            if enabled { "enabled" } else { "disabled" },
                        )
                    }
                    _ => (
                        Command::RunJobNow {
                            job: job.id.clone(),
                        },
                        "fired",
                    ),
                };
                self.dispatch(cmd).await.map_err(port_err)?;
                Ok(format!("{done} \"{}\" ({})", job.name, job.id))
            }
            other => Err(format!("unknown tool {other}")),
        }
    }

    /// 会话标签与工具看的那些：顶层的（子 Agent 的侧链不算）。
    fn top_sessions(&self) -> Vec<port::SessionSummary> {
        let st = self.0.st.lock().unwrap();
        st.sessions
            .values()
            .filter(|s| s.parent.is_none())
            .filter_map(|s| st.summary(&s.id))
            .collect()
    }

    /// `target` 指的会话：`#n` 按会话列表的顺序，否则按 id（作业、流程开出来的会话也行）。
    fn session_target(&self, input: &Value) -> Result<String, String> {
        let key = tools::str_of(input, "target").ok_or("target is required")?;
        let sums = self.top_sessions();
        if let Some(s) = port::order::resolve(&port::order::sessions(&sums), &key, |s| &s.id.0) {
            return Ok(s.id.0.clone());
        }
        let k = key.trim();
        if self.0.st.lock().unwrap().sessions.contains_key(k) {
            return Ok(k.to_string());
        }
        Err(format!("no session {key}"))
    }

    fn may_touch_session(&self, scope: &tools::Scope, rpc: &str, id: &str) -> Result<(), String> {
        if *scope == tools::Scope::Full || id == rpc {
            Ok(())
        } else {
            Err(out_of_scope(scope))
        }
    }

    // ---- 命令 ----

    async fn dispatch(&self, cmd: Command) -> Result<Reply, PortError> {
        use Command as C;
        match cmd {
            C::Hello { .. } => {
                let (instance, flows, jobs) = self.hello();
                Ok(Reply::Hello {
                    instance,
                    flows,
                    jobs,
                })
            }
            C::Heartbeat { .. } | C::Detach | C::StopInstance => Ok(Reply::Ok),
            C::ListSessions { filter } => {
                let st = self.0.st.lock().unwrap();
                let mut v: Vec<_> = st
                    .sessions
                    .values()
                    .filter(|s| s.parent.is_none())
                    .filter(|s| {
                        filter
                            .project
                            .as_ref()
                            .is_none_or(|p| s.project.as_ref() == Some(p))
                    })
                    .filter_map(|s| st.summary(&s.id))
                    .collect();
                v.sort_by_key(|s| std::cmp::Reverse(s.updated_ms));
                Ok(Reply::Sessions(v))
            }
            C::CreateSession { scene, model, .. } => {
                let id = self
                    .create_session(Origin::User, None, scene, model, json!({}))
                    .await?;
                self.summary(&id).map(Reply::Session)
            }
            C::Watch { session } => self.watch(&session.0).await,
            C::Unwatch { session } => {
                let mut link = self.0.link.lock().unwrap();
                link.watched.remove(&session.0);
                *link.unwatched.entry(session.0).or_default() += 1;
                Ok(Reply::Ok)
            }
            C::Rename { session, title } => {
                let _ = self
                    .call(
                        "session.rename",
                        json!({"session_id": session.0, "name": title}),
                    )
                    .await;
                self.retitle(&session.0, &title, Titler::User);
                Ok(Reply::Ok)
            }
            C::Close { session } => {
                if self.0.job_runs.lock().unwrap().contains_key(&session.0) {
                    self.turn_done(&session.0, false, "session closed".into(), Usage::default());
                }
                let _ = self
                    .call("session.unsubscribe", json!({"session_id": session.0}))
                    .await;
                let _ = self
                    .call("session.close", json!({"session_id": session.0}))
                    .await;
                self.0
                    .reg
                    .lock()
                    .unwrap()
                    .update(&session.0, |r| r.closed = true);
                self.0.st.lock().unwrap().sessions.remove(&session.0);
                self.0.link.lock().unwrap().watched.remove(&session.0);
                self.emit_events(vec![Event::SessionRemoved { session }]);
                Ok(Reply::Ok)
            }
            C::Submit {
                session,
                text,
                mode,
                permission,
            } => {
                // Agix 的 run_turn 只在建会话时收 permission_mode：对已有的会话照常跑会让
                // "这一轮只读"的选择悄悄失效，所以宁可拒绝（需求 C-20 等 Agix 支持）。
                if permission.is_some_and(|p| p != port::PermissionMode::Default) {
                    return Err(PortError::Unsupported("permission_mode".into()));
                }
                self.ensure_known(&session.0).await?;
                // 往哪个会话说话就是在看哪个会话：新建会话后 model 先放一个空转录、不等快照就提交。
                self.0
                    .link
                    .lock()
                    .unwrap()
                    .watched
                    .insert(session.0.clone());
                let first: String = text.lines().next().unwrap_or("").chars().take(40).collect();
                if !first.trim().is_empty() {
                    self.retitle(&session.0, first.trim(), Titler::FirstText);
                }
                let on_busy = match mode {
                    SubmitMode::Turn => "reject",
                    SubmitMode::Steer => "steer",
                    SubmitMode::Queue => "queue",
                };
                let turn = self.run_turn(&session.0, &text, on_busy);
                Ok(Reply::Submitted {
                    message: port::MessageId(turn),
                })
            }
            C::Interrupt { session } => {
                self.call("session.interrupt", json!({"session_id": session.0}))
                    .await
                    .map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::Answer { prompt, answer, .. } => {
                let meta = self.0.st.lock().unwrap().prompts.get(&prompt.0).cloned();
                // 不认识的提问多半已经被答掉或撤了：和引擎一样静默成功。
                if let Some(meta) = meta {
                    self.call(
                        "session.respondToPrompt",
                        state::answer_params(&meta, &prompt.0, &answer),
                    )
                    .await
                    .map_err(rpc_err)?;
                }
                Ok(Reply::Ok)
            }
            C::SetModel { session, model } => {
                self.call(
                    "session.setModel",
                    json!({"session_id": session.0, "model": model}),
                )
                .await
                .map_err(rpc_err)?;
                self.0
                    .reg
                    .lock()
                    .unwrap()
                    .update(&session.0, |r| r.model = Some(model.clone()));
                if let Some(s) = self.0.st.lock().unwrap().sessions.get_mut(&session.0) {
                    s.model = model;
                }
                self.upsert(&session.0);
                Ok(Reply::Ok)
            }
            C::Commands { session } => {
                let v = match self
                    .call("commands.list", json!({"session_id": session.0}))
                    .await
                {
                    Ok(v) => v,
                    Err(_) => self
                        .call("commands.list", json!({}))
                        .await
                        .map_err(rpc_err)?,
                };
                let cmds = v["commands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| SlashCommand {
                        name: str_field(c, "name"),
                        description: str_field(c, "description"),
                        source: str_field(c, "source"),
                    })
                    .collect();
                Ok(Reply::Commands(cmds))
            }
            C::SideQuestion { session, text } => {
                let r = self
                    .call("session.fork", json!({"session_id": session.0}))
                    .await
                    .map_err(rpc_err)?;
                let id = str_field(&r, "session_id");
                let (from_title, base) = {
                    let st = self.0.st.lock().unwrap();
                    let s = st.sessions.get(&session.0).cloned().unwrap_or_default();
                    (s.title.clone(), s)
                };
                let title: String = text.chars().take(40).collect();
                let origin = Origin::Fork {
                    from: session.clone(),
                    from_title,
                };
                let now = self.now();
                self.0.reg.lock().unwrap().update(&id, |r| {
                    r.origin = origin.clone();
                    r.title = Some(title.clone());
                    r.updated_ms = now;
                    r.created_ms = now;
                });
                self.0.st.lock().unwrap().sessions.insert(
                    id.clone(),
                    Sess {
                        id: id.clone(),
                        title,
                        project: base.project,
                        scene: base.scene,
                        model: base.model,
                        origin,
                        updated_ms: now,
                        created_ms: now,
                        ..Sess::default()
                    },
                );
                self.upsert(&id);
                self.0.side.lock().unwrap().insert(id.clone());
                self.run_turn(&id, &text, "queue");
                Ok(Reply::Ok)
            }
            C::SetGoal {
                session,
                objective,
                max_rounds,
            } => {
                // goal 的每一轮都是引擎自己开的，没有"第一句话"：还没名字就用目标当名字。
                let first: String = objective.chars().take(40).collect();
                self.retitle(&session.0, first.trim(), Titler::FirstText);
                let mut p = json!({"session_id": session.0, "objective": objective});
                if let Some(m) = max_rounds {
                    p["max_rounds"] = json!(m);
                }
                self.call("session.setGoal", p).await.map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::UpdateGoal { session, action } => {
                let p = match action {
                    GoalAction::Pause => json!({"session_id": session.0, "action": "pause"}),
                    GoalAction::Resume => json!({"session_id": session.0, "action": "resume"}),
                    GoalAction::Clear => json!({"session_id": session.0, "action": "clear"}),
                    GoalAction::Edit {
                        objective,
                        max_rounds,
                    } => {
                        let mut p = json!({"session_id": session.0, "action": "edit",
                                           "objective": objective});
                        if let Some(m) = max_rounds {
                            p["max_rounds"] = json!(m);
                        }
                        p
                    }
                };
                self.call("session.updateGoal", p).await.map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::StopTask { task } => {
                let owner = self.task_owner(&task)?;
                self.call(
                    "session.stopTask",
                    json!({"session_id": owner, "task_id": task.0}),
                )
                .await
                .map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::MessageTask { task, text } => {
                let owner = self.task_owner(&task)?;
                self.call(
                    "session.messageTask",
                    json!({"session_id": owner, "task_id": task.0, "message": text}),
                )
                .await
                .map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::TaskOutput { task, from_byte } => {
                let owner = self.task_owner(&task)?;
                let v = self
                    .call(
                        "session.taskOutput",
                        json!({"session_id": owner, "task_id": task.0, "from_byte": from_byte}),
                    )
                    .await
                    .map_err(rpc_err)?;
                Ok(Reply::TaskOutput {
                    text: str_field(&v, "output"),
                    next_byte: v["next_byte"].as_u64().unwrap_or(from_byte),
                    finished: v["final"].as_bool().unwrap_or(false),
                })
            }
            cmd @ (C::ListFlows
            | C::ListRuns { .. }
            | C::GetFlow { .. }
            | C::GetRun { .. }
            | C::StartRun { .. }
            | C::RunOp { .. }) => match self.flows() {
                Some(f) => f.command(cmd).await,
                None => match cmd {
                    C::ListFlows => Ok(Reply::Flows(vec![])),
                    C::ListRuns { .. } => Ok(Reply::Runs(vec![])),
                    _ => Err(PortError::Unsupported("flows".into())),
                },
            },
            C::ListJobs => Ok(Reply::Jobs(self.scheduler()?.list())),
            C::SaveJob { job } => {
                let saved = self.scheduler()?.upsert(job).map_err(PortError::Failed)?;
                self.emit_events(vec![Event::JobUpserted(saved.clone())]);
                Ok(Reply::Job(saved))
            }
            C::DeleteJob { job } => {
                self.scheduler()?
                    .delete(&job.0)
                    .map_err(PortError::Failed)?;
                self.emit_events(vec![Event::JobRemoved { job }]);
                Ok(Reply::Ok)
            }
            C::SetJobEnabled { job, enabled } => {
                let sched = self.scheduler()?;
                sched
                    .set_enabled(&job.0, enabled)
                    .map_err(PortError::Failed)?;
                if let Some(j) = sched.get(&job.0) {
                    self.emit_events(vec![Event::JobUpserted(j)]);
                }
                Ok(Reply::Ok)
            }
            C::RunJobNow { job } => {
                self.scheduler()?
                    .run_now(&job.0)
                    .map_err(PortError::Failed)?;
                Ok(Reply::Ok)
            }
            C::JobHistory { job } => Ok(Reply::JobHistory(self.scheduler()?.history(&job.0, 50))),
        }
    }

    fn task_owner(&self, task: &TaskId) -> Result<String, PortError> {
        self.0
            .st
            .lock()
            .unwrap()
            .task_owner
            .get(&task.0)
            .cloned()
            .ok_or_else(|| PortError::Failed(format!("no task {task}")))
    }
}

/// 摘要类全推；一轮的细节只推 Watch 着的会话（architecture §8.4）。
fn out_of_scope(scope: &tools::Scope) -> String {
    match scope {
        tools::Scope::Job(job) => format!(
            "not allowed: a session started by a scheduled job may change only that job ({job}) and itself"
        ),
        tools::Scope::Run(run) => format!(
            "not allowed: a session inside a flow run may change only that run ({run}) and itself"
        ),
        _ => "not allowed from this session".into(),
    }
}

fn local_time(ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn session_line(n: Option<usize>, s: &port::SessionSummary) -> String {
    let no = n.map(|n| format!("#{n}  ")).unwrap_or_default();
    let origin = match &s.provenance.origin {
        Origin::User => String::new(),
        Origin::Fork { from_title, .. } => format!("  forked from \"{from_title}\""),
        Origin::Job { name, .. } => format!("  started by job \"{name}\""),
        Origin::FlowStation {
            flow_title,
            subject,
            ..
        } => {
            format!("  in flow run \"{flow_title}\" / {subject}")
        }
        Origin::SubAgent { parent_title, .. } => format!("  sub-agent of \"{parent_title}\""),
    };
    format!("{no}{}  \"{}\"  {:?}{origin}", s.id, s.title, s.state)
}

fn job_line(n: usize, j: &Job) -> String {
    let next = j
        .next_fire_ms
        .filter(|_| j.enabled)
        .map(|ms| format!("  next {}", local_time(ms)))
        .unwrap_or_default();
    let last = j
        .last
        .as_ref()
        .map(|o| format!("  last {o:?}"))
        .unwrap_or_default();
    format!(
        "#{n}  {}  \"{}\"  {}  {}{next}{last}",
        j.id,
        j.name,
        j.cron,
        if j.enabled { "enabled" } else { "disabled" }
    )
}

/// `target` 指的作业：`#n` 按作业列表的顺序，否则按 id。
fn job_target(jobs: &[Job], input: &Value) -> Result<Job, String> {
    let key = tools::str_of(input, "target").ok_or("target is required")?;
    port::order::resolve(&port::order::jobs(jobs), &key, |j| &j.id.0)
        .cloned()
        .ok_or_else(|| format!("no job {key}"))
}

fn wanted(ev: &Event, watched: &HashSet<String>, st: &St) -> bool {
    match ev {
        Event::SessionUpserted(_) | Event::SessionRemoved { .. } | Event::TaskUpserted(_) => true,
        Event::TaskOutput { task, .. } => {
            st.task_owner
                .get(&task.0)
                .is_some_and(|o| watched.contains(o))
                || st
                    .sessions
                    .values()
                    .any(|s| s.tasks.contains_key(&task.0) && watched.contains(&s.id))
        }
        e => e.session().is_none_or(|s| watched.contains(&s.0)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Titler {
    User,
    Engine,
    FirstText,
}

fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

#[async_trait]
impl Service for Hub {
    fn attach(&self, sink: Option<Sink>) {
        let mut link = self.0.link.lock().unwrap();
        link.sink = sink;
        link.watched.clear();
        link.epoch += 1;
    }

    fn hello(&self) -> (InstanceInfo, bool, bool) {
        let st = self.0.st.lock().unwrap();
        let primary = st.sessions.values().filter(|s| s.parent.is_none());
        let (mut sessions, mut waiting) = (0, 0);
        for s in primary {
            sessions += 1;
            waiting += s.prompts.len() as u32;
        }
        let jobs = self
            .0
            .jobs
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.list().len() as u32)
            .unwrap_or(0);
        (
            InstanceInfo {
                name: InstanceName(self.0.instance.clone()),
                host: self.0.host.clone(),
                pid: Some(std::process::id()),
                project: Some(self.0.project.clone()),
                state: InstanceState::Running,
                started_ms: Some(self.0.started_ms),
                stopped_ms: None,
                version: self.0.version.clone(),
                sessions,
                waiting,
                jobs,
                holder: None,
            },
            self.0.flows.get().is_some(),
            self.0.jobs.lock().unwrap().is_some(),
        )
    }

    async fn handle(&self, cmd: Command) -> Result<Reply, PortError> {
        self.dispatch(cmd).await
    }
}

/// 流程派工器持有的：Hub 也持有流程那一侧，用弱引用免得互相拽着。
#[derive(Clone)]
pub(crate) struct WeakHub(Weak<Inner>);

impl WeakHub {
    pub(crate) fn upgrade(&self) -> Option<Hub> {
        self.0.upgrade().map(Hub)
    }
}

/// 调度器持有它；用弱引用，免得调度器与 `Hub` 互相拽着谁也放不掉。
struct Firer(Weak<Inner>);

#[async_trait]
impl Fire for Firer {
    async fn fire(&self, job: Job, fired_ms: u64) {
        if let Some(inner) = self.0.upgrade() {
            Hub(inner).fire_job(job, fired_ms).await;
        }
    }
}
