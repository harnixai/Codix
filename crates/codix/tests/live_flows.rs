//! LIVE：流程（architecture §8.6）——内置的示例流程、在用户会话里经 `flow_*` 写流程、一次运行
//! 从头跑到尾、取消、有运行时不许改、运行里的会话动不了别的流程。

mod live;

use std::time::Duration;

use live::{Answering, Live, QUICK};
use port::{Command, Event, FlowOrigin, Reply, RunOp, RunStatus, StationStatus};

/// 一次运行跑完的上限：每站一轮模型。
const RUN: Duration = Duration::from_secs(240);

/// 一站：主会话里做一轮，交一个词。
const ONE_STATION: &str = r#"name: live-echo
title: Live echo
stations:
  - id: say
    title: Say
    executor: {kind: session}
    skill: say-word
    output:
      type: object
      required: [word]
      properties:
        word: {type: string}
    exit: [{kind: output}]
"#;

/// 两站：第二站等人确认，运行就停在那里不结束。
const WAITS: &str = r#"name: live-wait
title: Live wait
stations:
  - id: say
    title: Say
    executor: {kind: session}
    skill: say-word
    output:
      type: object
      required: [word]
      properties:
        word: {type: string}
    exit: [{kind: output}]
  - id: approve
    title: Approve
    exit: [{kind: confirm}]
"#;

/// 全局流程包里放好这些流程，以及它们用的 skill。
fn install(lv: &Live, flows: &[(&str, &str)]) {
    let root = lv.home.path().join("flows");
    std::fs::create_dir_all(root.join("flows")).unwrap();
    std::fs::create_dir_all(root.join("skills/say-word")).unwrap();
    std::fs::write(
        root.join("skills/say-word/SKILL.md"),
        "---\nname: say-word\ndescription: Hand in the one word the subject asks for.\n---\n\n\
         Hand in `word`: the single word the run's subject asks you to say. Nothing else.\n",
    )
    .unwrap();
    for (name, yaml) in flows {
        std::fs::write(root.join(format!("flows/{name}.yaml")), yaml).unwrap();
    }
}

async fn flows(c: &live::Client) -> Vec<port::FlowSummary> {
    match c.ok(Command::ListFlows).await {
        Reply::Flows(v) => v,
        r => panic!("list_flows 回的是 {r:?}"),
    }
}

async fn start(c: &live::Client, flow: &str, subject: &str) -> port::RunSummary {
    match c
        .ok(Command::StartRun {
            flow: flow.into(),
            subject: subject.into(),
            inputs: serde_json::Value::Null,
        })
        .await
    {
        Reply::RunStarted(s) => s,
        r => panic!("start_run 回的是 {r:?}"),
    }
}

/// 在一个新的用户会话里让模型调一个工具；回这一轮。
async fn ask(c: &mut live::Client, text: &str) -> live::TurnOutcome {
    let s = c.new_session().await;
    c.watch(&s).await;
    c.turn(&s, text).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_builtin_example_flow_is_offered() {
    let lv = Live::new();
    let c = lv.start().await;
    let all = flows(&c).await;
    let ex = all
        .iter()
        .find(|f| f.name == "example-review")
        .unwrap_or_else(|| panic!("{all:?}"));
    assert_eq!(ex.origin, FlowOrigin::Builtin);
    let Reply::Flow(d) = c
        .ok(Command::GetFlow {
            name: "example-review".into(),
        })
        .await
    else {
        panic!("get_flow")
    };
    let ids: Vec<&str> = d.stations.iter().map(|s| s.id.0.as_str()).collect();
    assert_eq!(ids, ["review", "approve"]);
    assert_eq!(d.stations[1].needs[0].0, "review");
    assert!(d.yaml.contains("example-review"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_user_session_validates_and_creates_a_flow_with_flow_tools() {
    let lv = Live::new();
    install(&lv, &[]);
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    let bad = "name: broken\nstations:\n  - id: a\n    exit: [{kind: nope}]\n";
    let t = ask(
        &mut c,
        &format!(
            "Call flow_validate once with exactly this YAML:\n{bad}\nThen call flow_create once with \
             exactly this YAML:\n{ONE_STATION}\nDo not ask me to confirm. Reply with only the word DONE."
        ),
    )
    .await;
    t.ended();
    assert!(
        t.tools.iter().any(|n| n == "flow_validate"),
        "{:?}",
        t.tools
    );
    assert!(
        t.results
            .iter()
            .any(|(r, _)| r.contains("problems") || r.contains("unknown variant")),
        "校验报出了问题：{:?}",
        t.results
    );
    let made = flows(&c)
        .await
        .into_iter()
        .find(|f| f.name == "live-echo")
        .expect("建出来了");
    assert_eq!(made.origin, FlowOrigin::Global, "存在 ~/.codix/flows");
    assert!(
        lv.home.path().join("flows/flows/live-echo.yaml").exists(),
        "全局流程包的 flows/ 里"
    );
    assert!(
        c.events()
            .iter()
            .any(|e| matches!(e, Event::FlowUpserted(f) if f.name == "live-echo")),
        "流程推出来了"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_run_goes_from_start_to_finish() {
    let lv = Live::new();
    install(&lv, &[("live-echo", ONE_STATION)]);
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    let run = start(&c, "live-echo", "say the word OK").await;
    let id = run.id.clone();
    let done = c
        .wait_event("运行结束", RUN, |e| match e {
            Event::RunDetail(v)
                if v.summary.id == id && v.summary.status == RunStatus::Finished =>
            {
                Some((**v).clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(done.stations[0].status, StationStatus::Passed);
    // 主会话是这次运行的会话，归属是流程。
    let main = done.main_session.clone();
    let sum = c
        .sessions()
        .await
        .into_iter()
        .find(|s| s.id == main)
        .expect("主会话在列表里");
    assert!(
        matches!(&sum.provenance.origin, port::Origin::FlowStation { run, .. } if *run == id),
        "{:?}",
        sum.provenance.origin
    );
    let Reply::Runs(runs) = c
        .ok(Command::ListRuns {
            flow: "live-echo".into(),
        })
        .await
    else {
        panic!("list_runs")
    };
    assert!(runs.iter().any(|r| r.id == id));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_flow_with_runs_not_finished_cannot_be_changed_until_they_are_cancelled() {
    let lv = Live::new();
    install(&lv, &[("live-wait", WAITS), ("live-echo", ONE_STATION)]);
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    let run = start(&c, "live-wait", "say the word OK").await;
    let id = run.id.clone();
    let waiting = c
        .wait_event("运行停在确认上", RUN, |e| match e {
            Event::RunDetail(v)
                if v.summary.id == id && matches!(v.summary.status, RunStatus::Waiting { .. }) =>
            {
                Some((**v).clone())
            }
            _ => None,
        })
        .await;

    // 有没结束的运行：改不了。
    let t = ask(
        &mut c,
        &format!(
            "Call flow_update once with target \"live-wait\" and exactly this YAML:\n{}\nDo not ask me \
             to confirm. Report the tool's answer in one line.",
            WAITS.replace("Live wait", "Live wait 2")
        ),
    )
    .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| *err && r.contains("not finished") && r.contains(&id.0)),
        "拒绝并列出运行：{:?}",
        t.results
    );

    // 运行里的会话：动不了别的流程。
    let main = waiting.main_session.clone();
    c.watch(&main).await;
    let t = c
        .turn(
            &main,
            "Call flow_delete once with target \"live-echo\". Do not ask me to confirm. Report the \
             tool's answer in one line.",
        )
        .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| *err && r.contains("not allowed")),
        "{:?}",
        t.results
    );
    assert!(flows(&c).await.iter().any(|f| f.name == "live-echo"));

    // 取消之后就能改了。
    c.ok(Command::RunOp {
        run: id.clone(),
        op: RunOp::Cancel,
    })
    .await;
    c.wait_event("运行取消了", QUICK, |e| {
        matches!(e, Event::RunUpserted(s) if s.id == id && s.status == RunStatus::Cancelled)
            .then_some(())
    })
    .await;
    let t = ask(
        &mut c,
        &format!(
            "Call flow_update once with target \"live-wait\" and exactly this YAML:\n{}\nDo not ask me \
             to confirm. Reply with only the word DONE.",
            WAITS.replace("Live wait", "Live wait 2")
        ),
    )
    .await;
    t.ended();
    assert!(
        flows(&c)
            .await
            .iter()
            .any(|f| f.name == "live-wait" && f.title == "Live wait 2"),
        "{:?}",
        t.results
    );
}
