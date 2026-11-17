//! 会话代理（architecture §8.4）：端口命令 → Agix 方法，Agix 的帧 → 领域事件，再加上作业的
//! 触发。实例是引擎唯一的客户端：它订阅内存里的每个会话，所以状态不依赖 TUI 在不在。

mod history;
mod registry;
mod state;
mod tools;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

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
}

#[derive(Default)]
struct Link {
    sink: Option<Sink>,
    watched: HashSet<String>,
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
        }));
        hub.call("daemon.subscribeEvents", json!({})).await?;
        if let Ok(cfg) = hub
            .call(
                "config.get",
                json!({"project_root": hub.0.project, "tier": "effective"}),
            )
            .await
        {
            let m = &cfg["settings"]["model"];
            let name = m
                .as_str()
                .or_else(|| m["name"].as_str())
                .or_else(|| m["default"].as_str())
                .unwrap_or("");
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
            *hub.0.jobs.lock().unwrap() = Some(sched);
        }
        Ok(hub)
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
            if let Some(sink) = &link.sink {
                for ev in out.events {
                    if wanted(&ev, &link.watched, &st) {
                        let _ = sink.send(ServerFrame::Event { event: ev });
                    }
                }
            }
        }
        for t in out.todo {
            let hub = self.clone();
            tokio::spawn(async move { hub.follow_up(t).await });
        }
    }

    fn emit_events(&self, events: Vec<Event>) {
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
                let (content, is_error) = match self.host_tool(&name, &input) {
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
                session_id, event, ..
            } => {
                let ev = SessionEvent::parse(&event);
                let mut out = Out::default();
                self.0
                    .st
                    .lock()
                    .unwrap()
                    .on_event(&session_id, &session_id, ev, now, &mut out);
                self.emit(out);
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
            self.0.st.lock().unwrap().sessions.insert(id.to_string(), s);
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
        let mut out = Out::default();
        let now = self.now();
        {
            let mut st = self.0.st.lock().unwrap();
            if let Some(s) = st.sessions.get_mut(id) {
                s.live = true;
            }
            for p in v["pending_prompts"].as_array().into_iter().flatten() {
                st.on_event(id, id, SessionEvent::parse(p), now, &mut out);
            }
        }
        self.emit(out);
        Ok(())
    }

    async fn create_session(
        &self,
        origin: Origin,
        title: Option<String>,
        scene: Option<String>,
        model: Option<String>,
        extra: Value,
        editor_key: Option<String>,
    ) -> Result<String, PortError> {
        let mut options = json!({"prompt_types": ["permission", "question"]});
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
            rec.editor_key = editor_key.clone();
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
        let parent = self.0.st.lock().unwrap().sessions[id].parent.clone();
        let mut running = false;
        match &parent {
            None => {
                self.subscribe(id, true).await.map_err(rpc_err)?;
                if let Ok(g) = self.call("session.get", json!({"session_id": id})).await {
                    running = g["turn_state"].as_str() == Some("running");
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
                    let mut out = Out::default();
                    let now = self.now();
                    let mut st = self.0.st.lock().unwrap();
                    for i in infos {
                        st.on_event(id, id, SessionEvent::TaskUpdated { task: i }, now, &mut out);
                    }
                }
            }
            // 侧链的帧经父会话来：父会话订阅着就够了。
            Some(p) => {
                let _ = self.subscribe(p, false).await;
                running = self.0.st.lock().unwrap().sessions[id].running;
            }
        }
        let msgs = self.history(id).await;
        let conv = history::convert(&msgs);
        let mut st = self.0.st.lock().unwrap();
        let s = st.sessions.get_mut(id).expect("known above");
        s.running = running;
        if s.todos.is_empty()
            && let Some(t) = conv.todos
        {
            s.todos = t;
        }
        let mut items = conv.items;
        // 正在写的这一轮还没落盘：快照里补上已经到了的那段文字，之后的增量接着它。
        if s.running && !s.turn_text.is_empty() {
            items.push(port::HistoryItem::Assistant {
                text: s.turn_text.clone(),
            });
        }
        let watched = Watched {
            session: st.summary(id).expect("known above"),
            history: items,
            running,
            goal: st.sessions[id].goal.clone(),
            todos: st.sessions[id].todos.clone(),
            tasks: st.sessions[id].tasks.values().cloned().collect(),
            prompts: st.sessions[id].prompts.clone(),
        };
        self.0.link.lock().unwrap().watched.insert(id.to_string());
        Ok(Reply::Watched(Box::new(watched)))
    }

    /// `run_turn` 一跑就是几分钟：另起任务等它，错误响应就是这一轮的失败（协议 §5.5）。
    fn run_turn(&self, id: &str, text: &str, on_busy: &str) -> String {
        let turn = format!("cx-{}-{}", self.now(), rand_suffix());
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
                Err(RpcError::Remote { message, .. }) => {
                    let mut out = Out::default();
                    let now = hub.now();
                    hub.0
                        .st
                        .lock()
                        .unwrap()
                        .turn_failed(&id, message, now, &mut out);
                    hub.emit(out);
                }
                Err(RpcError::Closed) => {}
            }
        });
        turn
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
                        None,
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
            // 流程接入之前（architecture §8.6）流程作业只记一笔跳过。
            JobKind::Flow { .. } => (
                JobEntry {
                    fired_ms,
                    outcome: JobOutcome::Skipped,
                    elapsed_ms: 0,
                    usage: Usage::default(),
                    target: None,
                },
                None,
            ),
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

    fn host_tool(&self, name: &str, input: &Value) -> Result<String, String> {
        let sched = self.scheduler().map_err(|e| e.to_string())?;
        match name {
            tools::LIST => Ok(serde_json::to_string_pretty(&sched.list()).unwrap_or_default()),
            tools::SAVE => {
                let existing = input
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|id| sched.get(id));
                let job = tools::job_from_input(input, existing)?;
                let saved = sched.upsert(job)?;
                self.emit_events(vec![Event::JobUpserted(saved.clone())]);
                Ok(serde_json::to_string_pretty(&saved).unwrap_or_default())
            }
            tools::DELETE => {
                let id = input.get("id").and_then(Value::as_str).unwrap_or("");
                if sched.delete(id)? {
                    self.emit_events(vec![Event::JobRemoved {
                        job: JobId(id.to_string()),
                    }]);
                    Ok(format!("deleted {id}"))
                } else {
                    Err(format!("no job {id}"))
                }
            }
            other => Err(format!("unknown tool {other}")),
        }
    }

    async fn open_job_session(&self, job: Option<JobId>) -> Result<Reply, PortError> {
        let key = format!("job:{}", job.as_ref().map(|j| j.0.as_str()).unwrap_or(""));
        let existing = self.0.reg.lock().unwrap().by_editor_key(&key);
        if let Some(id) = existing
            && self.ensure_known(&id).await.is_ok()
        {
            return self.summary(&id).map(Reply::Session);
        }
        let current = job.as_ref().and_then(|j| self.scheduler().ok()?.get(&j.0));
        let name = current.as_ref().map(|j| j.name.clone()).unwrap_or_default();
        let extra = json!({
            "host_tools": tools::declarations(),
            "prompt": {"append": tools::prompt(current.as_ref())},
        });
        let origin = Origin::Editor {
            of: port::EditorOf::Job,
            name: name.clone(),
        };
        let id = self
            .create_session(origin, Some(name), None, None, extra, Some(key))
            .await?;
        self.summary(&id).map(Reply::Session)
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
                    .create_session(Origin::User, None, scene, model, json!({}), None)
                    .await?;
                self.summary(&id).map(Reply::Session)
            }
            C::Watch { session } => self.watch(&session.0).await,
            C::Unwatch { session } => {
                self.0.link.lock().unwrap().watched.remove(&session.0);
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
                ..
            } => {
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
            C::ListFlows => Ok(Reply::Flows(vec![])),
            C::ListRuns { .. } => Ok(Reply::Runs(vec![])),
            C::GetFlow { .. }
            | C::GetRun { .. }
            | C::StartRun { .. }
            | C::RunOp { .. }
            | C::OpenAuthoring { .. } => Err(PortError::Unsupported("flows".into())),
            C::OpenJobSession { job } => self.open_job_session(job).await,
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
            false,
            self.0.jobs.lock().unwrap().is_some(),
        )
    }

    async fn handle(&self, cmd: Command) -> Result<Reply, PortError> {
        self.dispatch(cmd).await
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
