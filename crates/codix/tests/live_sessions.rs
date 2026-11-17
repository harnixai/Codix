//! LIVE：会话代理里 Codix 自己做的部分（architecture §8.4）——只列本项目的会话、第一句话做
//! 名字、改名与关闭记在 `sessions.json` 里并跨重启保留、换模型、命令表、Watch 快照的历史转换
//! 与 todo、会话的权限模式（Shift+Tab：立即生效、跨重启保留）。

mod live;

use live::{Answering, Live, NAME, QUICK};
use port::{
    Command, Event, HistoryItem, Origin, PermissionMode, PortError, Reply, SessionId, TodoStatus,
};

const OK: &str = "Reply with only the word OK.";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn only_this_projects_sessions_are_listed() {
    let lv = Live::new();
    let other = live::new_project("other");
    let other_path = other.path().canonicalize().unwrap();
    let mut a = lv.start().await;
    let mut b = lv.create("lv2", &other_path.to_string_lossy()).await;
    // 两个实例共用一个数据根（Agix 的 session.list 看得到两边的会话）：说过话的会话才落盘。
    let sa = a.new_session().await;
    a.turn(&sa, OK).await.ended();
    let sb = b.new_session().await;
    b.turn(&sb, OK).await.ended();
    drop(a);

    let a = lv.restart().await;
    let ids: Vec<SessionId> = a.sessions().await.into_iter().map(|s| s.id).collect();
    assert!(ids.contains(&sa), "{ids:?}");
    assert!(!ids.contains(&sb), "别的项目的会话不该列出来：{ids:?}");
    for s in a.sessions().await {
        assert_eq!(
            s.provenance.project.as_deref(),
            Some(lv.project_path().as_str())
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_first_line_names_a_session_and_renames_and_closes_survive_a_restart() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s1 = c.new_session().await;
    c.turn(&s1, OK).await.ended();
    let titled = c.sessions().await.into_iter().find(|s| s.id == s1).unwrap();
    assert_eq!(titled.title, OK, "第一句话做名字");
    assert_eq!(titled.provenance.origin, Origin::User);
    assert_eq!(titled.provenance.instance.0, NAME);

    let s2 = c.new_session().await;
    c.turn(&s2, "Reply with only the word TWO.").await.ended();

    c.skip_past();
    c.ok(Command::Rename {
        session: s1.clone(),
        title: "renamed-live".into(),
    })
    .await;
    c.wait_upsert("改名后的摘要", |s| {
        s.id == s1 && s.title == "renamed-live"
    })
    .await;
    c.ok(Command::Close {
        session: s2.clone(),
    })
    .await;
    let s2c = s2.clone();
    c.wait_event("关闭", QUICK, |e| {
        matches!(e, Event::SessionRemoved { session } if *session == s2c).then_some(())
    })
    .await;
    // 再说一句话也不该把人起的名字换回第一句话。
    c.turn(&s1, OK).await.ended();

    let reg = lv.read("sessions.json");
    assert_eq!(reg[&s1.0]["title"], "renamed-live");
    assert_eq!(reg[&s2.0]["closed"], true);

    drop(c);
    let c = lv.restart().await;
    let list = c.sessions().await;
    let one = list.iter().find(|s| s.id == s1).expect("改过名的会话还在");
    assert_eq!(one.title, "renamed-live", "名字跨重启保留");
    assert!(list.iter().all(|s| s.id != s2), "关掉的会话重启后不再列出");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn set_model_is_recorded_and_the_command_table_is_listed() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    let creds = live::credentials();
    let other = creds
        .get("ANTHROPIC_SMALL_FAST_MODEL")
        .or(creds.get("ANTHROPIC_DEFAULT_HAIKU_MODEL"))
        .cloned()
        .unwrap_or_else(live::model);
    c.skip_past();
    c.ok(Command::SetModel {
        session: s.clone(),
        model: other.clone(),
    })
    .await;
    let sum = c
        .wait_upsert("换了模型的摘要", |x| {
            x.id == s && x.provenance.model == other
        })
        .await;
    assert_eq!(sum.provenance.model, other);
    assert_eq!(lv.read("sessions.json")[&s.0]["model"], other.as_str());
    c.turn(&s, OK).await.ended();

    let Reply::Commands(cmds) = c.ok(Command::Commands { session: s }).await else {
        panic!("commands")
    };
    assert!(!cmds.is_empty(), "命令表是空的");
    assert!(cmds.iter().all(|c| !c.name.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_watch_snapshot_rebuilds_history_and_todos() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    let t = c
        .turn(
            &s,
            "Call the TodoWrite tool exactly once with two todos: \"alpha\" with status \
             in_progress and \"beta\" with status pending. Then reply with only the word DONE.",
        )
        .await;
    t.ended();
    assert!(t.tools.iter().any(|n| n == "TodoWrite"), "{:?}", t.tools);
    assert!(
        c.events().iter().any(|e| matches!(
            e, Event::TodosChanged { session, todos } if *session == s && todos.len() == 2
        )),
        "TodoWrite 另推一份 TodosChanged"
    );

    c.ok(Command::Unwatch { session: s.clone() }).await;
    let w = c.watch(&s).await;
    assert!(!w.running);
    assert!(
        matches!(&w.history[0], HistoryItem::User { text } if text.contains("TodoWrite")),
        "{:?}",
        w.history.first()
    );
    assert!(
        w.history
            .iter()
            .any(|h| matches!(h, HistoryItem::Tool { name, .. } if name == "TodoWrite")),
        "{:?}",
        w.history
    );
    assert!(
        w.history
            .iter()
            .any(|h| matches!(h, HistoryItem::Assistant { text } if text.contains("DONE"))),
        "{:?}",
        w.history
    );
    let todos: Vec<(&str, TodoStatus)> = w
        .todos
        .iter()
        .map(|t| (t.text.as_str(), t.status))
        .collect();
    assert_eq!(
        todos,
        vec![
            ("alpha", TodoStatus::InProgress),
            ("beta", TodoStatus::Pending)
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_permission_mode_is_the_sessions_and_takes_effect_at_once() {
    let lv = Live::new();
    let mut c = lv.start().await;
    // 计划模式里写不成文件；模型若想 ExitPlanMode，问到人这里一律拒绝。
    c.answering(Answering::Deny);
    let s = c.new_session().await;
    c.watch(&s).await;
    let r = c
        .ok(Command::SetPermissionMode {
            session: s.clone(),
            mode: PermissionMode::Plan,
        })
        .await;
    assert_eq!(
        r,
        Reply::PermissionMode {
            mode: PermissionMode::Plan,
            clamped: false
        }
    );
    let me = s.clone();
    c.wait_upsert("会话的模式是 plan", |x| {
        x.id == me && x.permission == PermissionMode::Plan
    })
    .await;
    let t = c
        .turn(
            &s,
            "Use the Write tool to create a file named plan.txt containing hi. If you cannot, \
             reply with only the word BLOCKED.",
        )
        .await;
    t.ended();
    assert!(
        !std::path::Path::new(&lv.project_path())
            .join("plan.txt")
            .exists(),
        "计划模式里文件没写出来：{:?}",
        t.results
    );
    // 切回缺省；重启实例后会话仍记得（Agix 记在会话日志里）。
    let r = c
        .ok(Command::SetPermissionMode {
            session: s.clone(),
            mode: PermissionMode::AcceptEdits,
        })
        .await;
    assert!(matches!(
        r,
        Reply::PermissionMode {
            mode: PermissionMode::AcceptEdits,
            ..
        }
    ));
    drop(c);
    let c = lv.restart().await;
    let sum = c
        .sessions()
        .await
        .into_iter()
        .find(|x| x.id == s)
        .expect("会话还在");
    let watched = c.watch(&s).await;
    assert_eq!(
        watched.session.permission,
        PermissionMode::AcceptEdits,
        "重启后快照里是会话记下的模式（列表里是 {:?}）",
        sum.permission
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn flow_commands_are_wired() {
    let lv = Live::new();
    let c = lv.start().await;
    // 流程接上了：不认识的流程是普通的失败，不再是"不支持"。
    let r = c.call(Command::GetFlow { name: "x".into() }).await;
    assert!(matches!(&r, Err(PortError::Failed(_))), "{r:?}");
}
