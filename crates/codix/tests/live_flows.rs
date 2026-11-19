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
async fn a_user_session_validates_and_creates_a_flow_once_the_project_is_trusted() {
    let lv = Live::new();
    install(&lv, &[]);
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    // 没信任的项目：流程存不进去，工具照实说。
    let t = ask(
        &mut c,
        &format!(
            "Call flow_create once with exactly this YAML:\n{ONE_STATION}\nDo not ask me to confirm. \
             Report the tool's answer in one line."
        ),
    )
    .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| *err && r.contains("not trusted")),
        "{:?}",
        t.results
    );
    assert!(!flows(&c).await.iter().any(|f| f.name == "live-echo"));

    // /trust 之后：校验报错的不存，对的存进项目自己的流程包。
    c.ok(Command::TrustProject { trusted: true }).await;
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
    assert_eq!(made.origin, FlowOrigin::Project, "存进项目自己的流程包");
    assert!(
        std::path::Path::new(&lv.project_path())
            .join(".codix/flows/live-echo.yaml")
            .exists(),
        "项目的 .codix/flows/ 里"
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
    let all = c.sessions().await;
    let sum = all.iter().find(|s| s.id == main).unwrap_or_else(|| {
        panic!(
            "主会话 {main:?} 不在列表里：{:?}",
            all.iter()
                .map(|s| (&s.id, &s.provenance.origin))
                .collect::<Vec<_>>()
        )
    });
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
    let listed = runs.iter().find(|r| r.id == id).expect("运行在列表里");
    assert_eq!(
        (listed.stations_done, listed.stations_total),
        (1, 1),
        "列表里的站数从运行记录算"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_flow_with_runs_not_finished_cannot_be_changed_until_they_are_cancelled() {
    let lv = Live::new();
    install(&lv, &[("live-wait", WAITS), ("live-echo", ONE_STATION)]);
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    c.ok(Command::TrustProject { trusted: true }).await;
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
            "Call flow_update once with flow \"live-wait\" and exactly this YAML:\n{}\nDo not ask me \
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
            "Call flow_delete once with flow \"live-echo\". Do not ask me to confirm. Report the \
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
            "Call flow_update once with flow \"live-wait\" and exactly this YAML:\n{}\nDo not ask me \
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

/// 库的一致性套件对着我们的 `SessionHost`：一个键只开一次、一次 send 恰起一轮并且结束、问句
/// 送来能答、关掉的会话不在了……套件要在进程里拿到宿主，引擎就得嵌在测试进程里，凭据得在
/// 进程的环境里——写环境要 `unsafe`，所以带着凭据把自己再起一遍，只跑里面那个用例。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn our_session_host_passes_the_conformance_suite() {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "conformance_inside",
            "--nocapture",
            "--test-threads=1",
        ])
        .envs(live::credentials())
        .env("CODIX_CONFORMANCE", "1")
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .status()
        .unwrap();
    assert!(status.success(), "一致性套件没过（输出在上面）");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn conformance_inside() {
    if std::env::var_os("CODIX_CONFORMANCE").is_none() {
        return;
    }
    use harnix_flow::conformance::{Subject, run_all};
    let lv = Live::new();
    let project = std::path::PathBuf::from(lv.project_path());
    let (embedded, client, notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: lv.home.path().to_path_buf(),
        project: project.clone(),
        groups_file: lv.home.path().join("groups.json"),
    })
    .await
    .expect("嵌入引擎");
    let hub = host::Hub::start(
        std::sync::Arc::new(client),
        notes,
        host::HubConfig {
            instance: "conformance".into(),
            host: "local".into(),
            project: project.clone(),
            version: "0".into(),
            started_ms: live::now_ms(),
            registry: host::hub::Registry::in_memory(),
            jobs: None,
            clock: std::sync::Arc::new(live::now_ms),
        },
    )
    .await
    .expect("Hub");
    let subject = Subject::new(host::flows::session_host(&hub), "code")
        .project(project)
        .idle_turn("Reply with only the word OK and stop.")
        .asking_turn(
            "Use the Write tool to create a file named probe.txt containing hi. Do nothing else.",
            "Write",
        )
        .turn_timeout(Duration::from_secs(120));
    let failures = run_all(&subject).await;
    hub.shutdown();
    embedded.shutdown().await;
    assert!(failures.is_empty(), "{failures:#?}");
}

/// 流程作业：触发一次就启动一次运行，触发记录指着那次运行。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_flow_job_starts_a_run_when_it_fires() {
    let lv = Live::new();
    install(&lv, &[("live-echo", ONE_STATION)]);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let job = port::Job {
        id: port::JobId(String::new()),
        name: "flow-job".into(),
        cron: "0 4 * * *".into(),
        kind: port::JobKind::Flow {
            flow: "live-echo".into(),
            subject: "say the word OK".into(),
            inputs: serde_json::Value::Null,
        },
        enabled: true,
        catch_up: false,
        created_ms: 0,
        next_fire_ms: None,
        last: None,
    };
    let Reply::Job(job) = c.ok(Command::SaveJob { job }).await else {
        panic!("save_job")
    };
    c.ok(Command::RunJobNow {
        job: job.id.clone(),
    })
    .await;
    let jid = job.id.clone();
    let entry = c
        .wait_event("触发记录", QUICK, |e| match e {
            Event::JobFired { job, entry } if *job == jid => Some(entry.clone()),
            _ => None,
        })
        .await;
    let Some(port::JobTarget::Run { id, subject }) = entry.target.clone() else {
        panic!("{entry:?}")
    };
    assert_eq!(subject, "say the word OK");
    assert_eq!(entry.outcome, port::JobOutcome::Done);
    c.wait_event("那次运行跑完", RUN, |e| {
        matches!(e, Event::RunUpserted(s) if s.id == id && s.status == RunStatus::Finished)
            .then_some(())
    })
    .await;
}
