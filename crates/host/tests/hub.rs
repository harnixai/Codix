//! `Hub` 对着脚本化的假引擎：端口命令变成哪些 Agix 调用，Agix 的帧变成哪些领域事件、推给谁。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use host::hub::Registry;
use host::{Engine, Hub, HubConfig, Notification, RpcError, Service};
use port::link::ServerFrame;
use port::{Answer, Command, Event, PromptId, Reply, SessionId, SubmitMode};
use serde_json::{Value, json};
use tokio::sync::mpsc;

type Handler = Box<dyn Fn(&str, &Value) -> Result<Value, RpcError> + Send + Sync>;

struct Fake {
    calls: Mutex<Vec<(String, Value)>>,
    handler: Handler,
}

#[async_trait]
impl Engine for Fake {
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), params.clone()));
        (self.handler)(method, &params)
    }
}

const PROJECT: &str = "/work/app";

fn default_handler(method: &str, p: &Value) -> Result<Value, RpcError> {
    Ok(match method {
        "session.list" => json!({"sessions": [
            {"session_id":"s1","name":"修登录","status":"active","session_kind":"primary",
             "scene":"code","project_root":PROJECT,"last_active":"2026-11-16T10:00:00Z","created_at":""},
            {"session_id":"s2","name":null,"status":"inactive","session_kind":"primary",
             "scene":"code","project_root":PROJECT,"last_active":"","created_at":""},
            {"session_id":"x9","name":"别的项目","status":"active","session_kind":"primary",
             "scene":"code","project_root":"/elsewhere","last_active":"","created_at":""}]}),
        "session.subscribe" => json!({"session_id": p["session_id"], "last_seq": 0,
            "pending_prompts": [], "pending_host_calls": [], "resumed": false}),
        "session.create" => json!({"session_id":"new1","scene":"code"}),
        "session.history" => json!({"messages": [
            {"role":"user","origin":"user","content":[{"type":"text","text":"你好"}]},
            {"role":"assistant","content":[{"type":"text","text":"在"}]}], "total": 2}),
        "session.get" => json!({"session_id": p["session_id"], "turn_state":"idle"}),
        "session.goal" => json!({"goal": null}),
        "session.tasks" => json!({"tasks": []}),
        "session.run_turn" => {
            json!({"session_id": p["session_id"], "turn_id": p["turn_id"], "delivery":"turn","name":null})
        }
        _ => json!({}),
    })
}

struct Rig {
    hub: Hub,
    fake: Arc<Fake>,
    notes: mpsc::UnboundedSender<Notification>,
    out: mpsc::UnboundedReceiver<ServerFrame>,
    _dir: tempfile::TempDir,
}

async fn rig_with(handler: Handler) -> Rig {
    let fake = Arc::new(Fake {
        calls: Mutex::default(),
        handler,
    });
    let (notes, rx) = mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let store = host::jobs::JobStore::open(dir.path().join("jobs.json"), dir.path().join("jobs"));
    let hub = Hub::start(
        fake.clone(),
        rx,
        HubConfig {
            instance: "app".into(),
            host: "box".into(),
            project: PROJECT.into(),
            version: "0".into(),
            started_ms: 1,
            registry: Registry::open(dir.path().join("sessions.json")),
            jobs: Some(store),
            clock: Arc::new(|| 1_000_000),
        },
    )
    .await
    .unwrap();
    let (tx, out) = mpsc::unbounded_channel();
    hub.attach(Some(tx));
    Rig {
        hub,
        fake,
        notes,
        out,
        _dir: dir,
    }
}

async fn rig() -> Rig {
    rig_with(Box::new(default_handler)).await
}

impl Rig {
    fn frame(&self, sid: &str, ev: Value) {
        self.notes
            .send(Notification::Session {
                session_id: sid.into(),
                turn_id: None,
                event: ev,
            })
            .unwrap();
    }

    /// 等通知任务处理完，把这期间推出的事件都取出来。
    async fn events(&mut self) -> Vec<Event> {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        let mut v = vec![];
        while let Ok(f) = self.out.try_recv() {
            if let ServerFrame::Event { event } = f {
                v.push(event);
            }
        }
        v
    }

    fn calls(&self, method: &str) -> Vec<Value> {
        self.fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

#[tokio::test]
async fn lists_only_this_projects_sessions_and_subscribes_the_live_ones() {
    let r = rig().await;
    let Reply::Sessions(v) = r
        .hub
        .handle(Command::ListSessions {
            filter: Default::default(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let ids: Vec<_> = v.iter().map(|s| s.id.0.as_str()).collect();
    assert_eq!(ids, ["s1", "s2"], "按最近活动排序，别的项目的不列");
    assert_eq!(v[0].title, "修登录");
    let subs = r.calls("session.subscribe");
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0]["session_id"], "s1");
    let (info, flows, jobs) = r.hub.hello();
    assert_eq!((info.sessions, flows, jobs), (2, false, true));
}

#[tokio::test]
async fn turn_details_reach_only_the_watched_session() {
    let mut r = rig().await;
    r.frame("s2", json!({"kind":"turn_start"}));
    r.frame("s2", json!({"kind":"text_delta","text":"不该推"}));
    let ev = r.events().await;
    assert!(
        ev.iter().all(|e| matches!(e, Event::SessionUpserted(_))),
        "没 Watch 的会话只推摘要：{ev:?}"
    );

    let Reply::Watched(w) = r
        .hub
        .handle(Command::Watch {
            session: SessionId("s1".into()),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(w.history.len(), 2);
    r.frame("s1", json!({"kind":"turn_start"}));
    r.frame("s1", json!({"kind":"step_start","step":0}));
    r.frame("s1", json!({"kind":"text_delta","text":"先看"}));
    r.frame("s1", json!({"kind":"step_start","step":0}));
    r.frame(
        "s1",
        json!({"kind":"tool_use","id":"t1","name":"TodoWrite",
                   "input":{"todos":[{"content":"读代码","status":"in_progress"}]}}),
    );
    r.frame("s1", json!({"kind":"brand_new_kind"}));
    r.frame(
        "s1",
        json!({"kind":"turn_complete","stop_reason":"end_turn",
                   "usage":{"input_tokens":10,"output_tokens":5}}),
    );
    let ev = r.events().await;
    let kinds: Vec<_> = ev
        .iter()
        .filter(|e| !matches!(e, Event::SessionUpserted(_)))
        .map(|e| {
            format!("{e:?}")
                .split([' ', '{', '('])
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "TurnStarted",
            "TextDelta",
            "StepRestarted",
            "ToolStarted",
            "ToolInput",
            "TodosChanged",
            "TurnEnded"
        ]
    );
    let last = ev
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionUpserted(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(last.usage.input, 10);
    assert_eq!(last.state, port::SessionState::Idle);
}

#[tokio::test]
async fn question_answers_use_option_keys_and_the_parent_session() {
    let mut r = rig().await;
    r.hub
        .handle(Command::Watch {
            session: SessionId("s1".into()),
        })
        .await
        .unwrap();
    r.frame(
        "s1",
        json!({"kind":"subagent_progress","agent_label":"探索","agent_session_id":"c1",
            "agent_type":"explore","parent_turn":1,"task_id":"tk1",
            "event":{"kind":"prompt","prompt_type":"question","prompt_id":"p1","tool_use_id":"x",
                     "question":"哪个分支？","header":null,
                     "options":[{"key":"a","label":"main"},{"key":"b","label":"dev"}]}}),
    );
    let ev = r.events().await;
    let pending = ev
        .iter()
        .find_map(|e| match e {
            Event::PromptPending { session, prompt } => Some((session.clone(), prompt.clone())),
            _ => None,
        })
        .expect("提问推出来了");
    assert_eq!(pending.0.0, "s1", "子 Agent 的提问挂在父会话上");
    assert_eq!(pending.1.from.unwrap().label, "探索");
    r.hub
        .handle(Command::Answer {
            session: SessionId("s1".into()),
            prompt: PromptId("p1".into()),
            answer: Answer::Choice { index: 1 },
        })
        .await
        .unwrap();
    let a = &r.calls("session.respondToPrompt")[0];
    assert_eq!(a["session_id"], "s1");
    assert_eq!(a["answer"], json!({"type":"answer","text":"b"}));
}

#[tokio::test]
async fn a_failed_run_turn_is_a_failed_turn() {
    let mut r = rig_with(Box::new(|m, p| {
        if m == "session.run_turn" {
            Err(RpcError::Remote {
                code: -32002,
                message: "no_credentials: no API key".into(),
                data: Value::Null,
            })
        } else {
            default_handler(m, p)
        }
    }))
    .await;
    r.hub
        .handle(Command::Watch {
            session: SessionId("s1".into()),
        })
        .await
        .unwrap();
    let reply = r
        .hub
        .handle(Command::Submit {
            session: SessionId("s1".into()),
            text: "hi".into(),
            mode: SubmitMode::Turn,
        })
        .await
        .unwrap();
    assert!(matches!(reply, Reply::Submitted { .. }));
    let ev = r.events().await;
    assert!(
        ev.iter().any(
            |e| matches!(e, Event::TurnFailed { message, .. } if message.contains("no_credentials"))
        ),
        "{ev:?}"
    );
    assert_eq!(r.calls("session.run_turn")[0]["on_busy"], "reject");
}

fn tool_call(sid: &str, call_id: &str, name: &str, input: Value) -> Value {
    json!({"kind":"host_tool_call","call_id":call_id,"tool_use_id":"t","name":name,
        "input":input,"timeout_ms":1000,"task_id":null,"agent_session_id":null,"cwd":PROJECT,"depth":0,
        "session_id":sid})
}

impl Rig {
    /// 工具的回答：`(是否出错, 文本)`。
    fn answer(&self, call_id: &str) -> (bool, String) {
        let r = self
            .calls("session.respondToolCall")
            .into_iter()
            .find(|c| c["call_id"] == call_id)
            .unwrap_or_else(|| panic!("{call_id} 没有回答"));
        (
            r["result"]["is_error"].as_bool().unwrap(),
            r["result"]["content"].as_str().unwrap().to_string(),
        )
    }
}

#[tokio::test]
async fn host_tools_hang_on_the_connection_and_a_user_session_saves_jobs() {
    let mut r = rig().await;
    let set = &r.calls("daemon.setHostTools")[0];
    let names: Vec<&str> = set["host_tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"job_save") && names.contains(&"session_rename"));
    r.hub
        .handle(Command::CreateSession {
            project: None,
            scene: None,
            model: None,
            permission: None,
        })
        .await
        .unwrap();
    let create = &r.calls("session.create")[0];
    assert!(
        create["options"]["permission_rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["tool_name"] == "job_list" && x["behavior"] == "allow"),
        "查看类工具不用每次问"
    );
    assert!(
        create["options"]["host_tools"].is_null(),
        "不再按会话挂工具"
    );

    r.frame(
        "s1",
        tool_call(
            "s1",
            "hc1",
            "job_save",
            json!({"name":"nightly","cron":"0 2 * * *","prompt":"跑测试"}),
        ),
    );
    let ev = r.events().await;
    let job = ev
        .iter()
        .find_map(|e| match e {
            Event::JobUpserted(j) => Some(j.clone()),
            _ => None,
        })
        .expect("保存的作业推出来了");
    let (err, text) = r.answer("hc1");
    assert!(!err, "{text}");
    assert!(text.contains(&job.id.0), "回显名字与 ID：{text}");

    r.hub
        .handle(Command::RunJobNow {
            job: job.id.clone(),
        })
        .await
        .unwrap();
    let ev = r.events().await;
    assert!(ev.iter().any(|e| matches!(e, Event::JobFired { entry, .. }
        if entry.outcome == port::JobOutcome::Running)));
    let fired = r.calls("session.create").last().cloned().unwrap();
    assert_eq!(fired["options"]["attended"], false, "作业无人值守");
    let turn = r.calls("session.run_turn").last().cloned().unwrap();
    assert_eq!(turn["message"], "跑测试");

    r.frame("new1", json!({"kind":"turn_start"}));
    r.frame(
        "new1",
        json!({"kind":"turn_complete","stop_reason":"end_turn","usage":{"output_tokens":7}}),
    );
    let ev = r.events().await;
    let done = ev
        .iter()
        .find_map(|e| match e {
            Event::JobFired { entry, .. } => Some(entry.clone()),
            _ => None,
        })
        .expect("结局补上了");
    assert_eq!(done.outcome, port::JobOutcome::Done);
    assert_eq!(done.usage.output, 7);
    let Reply::JobHistory(h) = r
        .hub
        .handle(Command::JobHistory { job: job.id })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(h.len(), 1, "开始与结局是同一条记录");
}

#[tokio::test]
async fn a_busy_session_returns_the_message_instead_of_failing_the_turn() {
    let mut r = rig_with(Box::new(|m, p| {
        if m == "session.run_turn" {
            Err(RpcError::Remote {
                code: -32015,
                message: "session is busy".into(),
                data: Value::Null,
            })
        } else {
            default_handler(m, p)
        }
    }))
    .await;
    let Reply::Submitted { message } = r
        .hub
        .handle(Command::Submit {
            session: SessionId("s1".into()),
            text: "再来".into(),
            mode: SubmitMode::Turn,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let ev = r.events().await;
    assert!(
        ev.iter().any(
            |e| matches!(e, Event::SteerReturned { messages, .. } if messages == std::slice::from_ref(&message))
        ),
        "{ev:?}"
    );
    assert!(!ev.iter().any(|e| matches!(e, Event::TurnFailed { .. })));
}

#[tokio::test]
async fn a_mid_turn_snapshot_adds_only_the_unpersisted_step() {
    let mut r = rig().await;
    r.frame("s1", json!({"kind":"turn_start"}));
    r.frame("s1", json!({"kind":"step_start","step":0}));
    r.frame("s1", json!({"kind":"text_delta","text":"先看看"}));
    r.frame(
        "s1",
        json!({"kind":"step_end","step":0,"stop_reason":"tool_use","tool_calls":1,"seq":3,"usage":{}}),
    );
    r.frame("s1", json!({"kind":"step_start","step":1}));
    r.frame("s1", json!({"kind":"text_delta","text":"找到了"}));
    r.events().await;
    let Reply::Watched(w) = r
        .hub
        .handle(Command::Watch {
            session: SessionId("s1".into()),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(w.running);
    assert_eq!(
        w.history.last(),
        Some(&port::HistoryItem::Assistant {
            text: "找到了".into()
        }),
        "落了盘的那一步在历史里，不再补一遍"
    );
}

#[tokio::test]
async fn permission_modes_are_set_on_the_session_and_its_frames_keep_them() {
    let mut r = rig_with(Box::new(|m, p| match m {
        "session.setPermissionMode" => {
            // 设置不许 bypass：夹到 default。
            let (mode, clamped) = match p["mode"].as_str() {
                Some("bypassPermissions") => ("default", true),
                Some(x) => (x, false),
                None => ("default", false),
            };
            Ok(json!({"session_id": p["session_id"], "mode": mode,
                      "requested": p["mode"], "clamped": clamped}))
        }
        _ => default_handler(m, p),
    }))
    .await;
    let reply = r
        .hub
        .handle(Command::SetPermissionMode {
            session: SessionId("s1".into()),
            mode: port::PermissionMode::Plan,
        })
        .await
        .unwrap();
    assert_eq!(
        reply,
        Reply::PermissionMode {
            mode: port::PermissionMode::Plan,
            clamped: false
        }
    );
    assert_eq!(r.calls("session.setPermissionMode")[0]["mode"], "plan");
    let ev = r.events().await;
    assert!(
        ev.iter().any(|e| matches!(e, Event::SessionUpserted(s)
            if s.id.0 == "s1" && s.permission == port::PermissionMode::Plan)),
        "{ev:?}"
    );

    // 模型自己 ExitPlanMode / EnterPlanMode 也推帧：摘要跟着变。
    r.frame(
        "s1",
        json!({"kind":"permission_mode_changed","mode":"acceptEdits","source":"model"}),
    );
    let ev = r.events().await;
    assert!(
        ev.iter().any(|e| matches!(e, Event::SessionUpserted(s)
            if s.id.0 == "s1" && s.permission == port::PermissionMode::AcceptEdits)),
        "{ev:?}"
    );

    let Reply::PermissionMode { mode, clamped } = r
        .hub
        .handle(Command::SetPermissionMode {
            session: SessionId("s1".into()),
            mode: port::PermissionMode::BypassPermissions,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        (mode, clamped),
        (port::PermissionMode::Default, true),
        "以实际生效的为准"
    );

    // 草稿里选的模式随建会话带上；Submit 不再带模式。
    r.hub
        .handle(Command::CreateSession {
            project: None,
            scene: None,
            model: None,
            permission: Some(port::PermissionMode::AcceptEdits),
        })
        .await
        .unwrap();
    let create = r.calls("session.create").last().cloned().unwrap();
    assert_eq!(create["options"]["permission_mode"], "acceptEdits");
}

#[tokio::test]
async fn late_frames_do_not_resurrect_a_closed_session() {
    let mut r = rig().await;
    r.hub
        .handle(Command::Close {
            session: SessionId("s2".into()),
        })
        .await
        .unwrap();
    r.frame(
        "s2",
        json!({"kind":"task_finished","task":{"task_id":"t1","kind":"shell","status":"stopped",
            "started_ms":0,"detail":{"command":"sleep 9"}}}),
    );
    r.events().await;
    let Reply::Sessions(v) = r
        .hub
        .handle(Command::ListSessions {
            filter: Default::default(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(v.iter().all(|s| s.id.0 != "s2"), "{v:?}");
}

#[tokio::test]
async fn a_job_fired_session_may_change_only_its_own_job() {
    let mut r = rig().await;
    let save = |name: &str| port::Job {
        id: port::JobId(String::new()),
        name: name.into(),
        cron: "0 2 * * *".into(),
        kind: port::JobKind::Simple {
            prompt: "跑测试".into(),
            skills: vec![],
            project: None,
            scene: None,
            model: None,
        },
        enabled: true,
        catch_up: false,
        next_fire_ms: None,
        last: None,
    };
    let Reply::Job(mine) = r
        .hub
        .handle(Command::SaveJob { job: save("mine") })
        .await
        .unwrap()
    else {
        panic!()
    };
    let Reply::Job(other) = r
        .hub
        .handle(Command::SaveJob { job: save("other") })
        .await
        .unwrap()
    else {
        panic!()
    };
    r.hub
        .handle(Command::RunJobNow {
            job: mine.id.clone(),
        })
        .await
        .unwrap();
    r.events().await;
    // 触发出来的会话是 new1（假引擎建的会话都叫这个）。
    r.frame(
        "new1",
        tool_call(
            "new1",
            "a",
            "job_enable",
            json!({"target": other.id.0, "enabled": false}),
        ),
    );
    r.frame(
        "new1",
        tool_call(
            "new1",
            "b",
            "job_enable",
            json!({"target": mine.id.0, "enabled": false}),
        ),
    );
    r.frame(
        "new1",
        tool_call(
            "new1",
            "c",
            "job_save",
            json!({"name":"x","cron":"* * * * *","prompt":"p"}),
        ),
    );
    r.frame(
        "new1",
        tool_call(
            "new1",
            "d",
            "session_rename",
            json!({"target": "s1", "title": "改别人"}),
        ),
    );
    r.frame(
        "new1",
        tool_call("new1", "e", "session_rename", json!({"title": "改自己"})),
    );
    // 子 Agent 的调用经根会话来，作用域也是根会话的。
    r.frame(
        "new1",
        json!({"kind":"subagent_progress","agent_label":"子","agent_session_id":"c9","task_id":"t9",
            "event": tool_call("new1", "f", "job_delete", json!({"target": other.id.0}))}),
    );
    r.frame("zz", tool_call("zz", "g", "job_list", json!({})));
    r.frame("s1", tool_call("s1", "h", "job_list", json!({})));
    r.events().await;
    for (id, ok) in [
        ("a", false),
        ("b", true),
        ("c", false),
        ("d", false),
        ("e", true),
        ("f", false),
        ("g", false),
        ("h", true),
    ] {
        let (err, text) = r.answer(id);
        assert_eq!(!err, ok, "{id}: {text}");
    }
    let (_, list) = r.answer("h");
    assert!(
        list.starts_with("#1  ") && list.contains("#2  "),
        "按 #n 列：{list}"
    );
    assert!(list.contains("mine") && list.contains("other"));
}
