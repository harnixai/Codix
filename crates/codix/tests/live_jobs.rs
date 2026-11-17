//! LIVE：作业（architecture §8.5）——增删改与启停、立即运行触发出的会话与结局、触发记录落盘、
//! 停实例时跑到一半的那次记为失败、崩溃后没有结局的那次在启动时记为中断、错过的触发补跑一次、作业编辑会话的宿主工具。

mod live;

use live::{Answering, Live, QUICK, TURN};
use port::{Command, Event, Job, JobId, JobKind, JobOutcome, JobTarget, Origin, Reply};

fn job(name: &str, cron: &str, prompt: &str, catch_up: bool) -> Job {
    Job {
        id: JobId(String::new()),
        name: name.into(),
        cron: cron.into(),
        kind: JobKind::Simple {
            prompt: prompt.into(),
            skills: vec![],
            project: None,
            scene: None,
            model: None,
        },
        enabled: true,
        catch_up,
        next_fire_ms: None,
        last: None,
    }
}

async fn save(c: &live::Client, j: Job) -> Job {
    match c.ok(Command::SaveJob { job: j }).await {
        Reply::Job(j) => j,
        r => panic!("save_job 回的是 {r:?}"),
    }
}

async fn list(c: &live::Client) -> Vec<Job> {
    match c.ok(Command::ListJobs).await {
        Reply::Jobs(v) => v,
        r => panic!("list_jobs 回的是 {r:?}"),
    }
}

/// 等这个作业的一条触发记录满足 `want`。
async fn fired(
    c: &mut live::Client,
    id: &JobId,
    what: &str,
    want: impl Fn(&port::JobEntry) -> bool,
) -> port::JobEntry {
    let id = id.clone();
    c.wait_event(what, TURN, |e| match e {
        Event::JobFired { job, entry } if *job == id && want(entry) => Some(entry.clone()),
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn jobs_are_saved_listed_toggled_and_deleted() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let j = save(&c, job("nightly", "0 3 * * *", "say hi", false)).await;
    assert!(!j.id.0.is_empty(), "新作业分到 id");
    assert!(j.next_fire_ms.is_some_and(|t| t > live::now_ms()));
    let id = j.id.clone();
    c.wait_event("作业出现", QUICK, |e| {
        matches!(e, Event::JobUpserted(x) if x.id == id).then_some(())
    })
    .await;
    assert_eq!(list(&c).await, vec![j.clone()]);
    let stored = lv.read("jobs.json");
    assert_eq!(stored[0]["job"]["id"], j.id.0.as_str());
    assert_eq!(stored[0]["job"]["cron"], "0 3 * * *");

    let bad = c
        .call(Command::SaveJob {
            job: job("bad", "not a cron", "x", false),
        })
        .await;
    assert!(bad.is_err(), "坏的 cron 不收：{bad:?}");

    c.ok(Command::SetJobEnabled {
        job: j.id.clone(),
        enabled: false,
    })
    .await;
    let id = j.id.clone();
    let off = c
        .wait_event("停用", QUICK, |e| match e {
            Event::JobUpserted(x) if x.id == id && !x.enabled => Some(x.clone()),
            _ => None,
        })
        .await;
    assert_eq!(off.next_fire_ms, None, "停用的作业没有下次触发");
    assert_eq!(lv.read("jobs.json")[0]["job"]["enabled"], false);

    c.ok(Command::DeleteJob { job: j.id.clone() }).await;
    let id = j.id.clone();
    c.wait_event("删掉", QUICK, |e| {
        matches!(e, Event::JobRemoved { job } if *job == id).then_some(())
    })
    .await;
    assert!(list(&c).await.is_empty());
    assert_eq!(lv.read("jobs.json"), serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn run_now_fires_a_job_session_and_records_the_outcome() {
    let lv = Live::new();
    let mut c = lv.start().await;
    // 停用的作业也能立即运行一次。
    let mut j = job("probe", "0 3 * * *", "Reply with exactly: JOB-OK", false);
    j.enabled = false;
    let j = save(&c, j).await;
    c.skip_past();
    c.ok(Command::RunJobNow { job: j.id.clone() }).await;

    let running = fired(&mut c, &j.id, "触发（进行中）", |e| {
        e.outcome == JobOutcome::Running
    })
    .await;
    let Some(JobTarget::Session { id: sid, title }) = running.target.clone() else {
        panic!("{running:?}")
    };
    assert_eq!(title, "probe");
    let sum = c
        .sessions()
        .await
        .into_iter()
        .find(|s| s.id == sid)
        .expect("触发出的会话在列表里");
    assert!(
        matches!(&sum.provenance.origin, Origin::Job { job, name, fired_ms }
            if *job == j.id && name == "probe" && *fired_ms == running.fired_ms),
        "{:?}",
        sum.provenance.origin
    );

    let done = fired(&mut c, &j.id, "触发（结局）", |e| {
        e.outcome != JobOutcome::Running
    })
    .await;
    assert_eq!(done.outcome, JobOutcome::Done);
    assert_eq!(done.fired_ms, running.fired_ms, "同一次触发补上结局");
    assert!(done.elapsed_ms > 0 && done.usage.output > 0, "{done:?}");
    let id = j.id.clone();
    c.wait_event("作业的上次结局", QUICK, |e| {
        matches!(e, Event::JobUpserted(x) if x.id == id && x.last == Some(JobOutcome::Done))
            .then_some(())
    })
    .await;

    let on_disk = lv.job_history_file(&j.id.0);
    assert_eq!(on_disk.len(), 1);
    assert_eq!(on_disk[0].outcome, JobOutcome::Done);
    let Reply::JobHistory(h) = c.ok(Command::JobHistory { job: j.id.clone() }).await else {
        panic!("job_history")
    };
    assert_eq!(h, on_disk);
    // 触发出的会话里就是那句提示词与回答。
    let w = c.watch(&sid).await;
    assert!(
        w.history
            .iter()
            .any(|h| matches!(h, port::HistoryItem::Assistant { text } if text.contains("JOB-OK"))),
        "{:?}",
        w.history
    );
}

/// 一次触发跑到一半：起一个长的作业，等它的 `Running` 记录出来。
async fn fire_long(lv: &Live) -> (live::Client, Job, port::JobEntry) {
    let mut c = lv.start().await;
    let j = save(
        &c,
        job(
            "long",
            "0 3 * * *",
            "Count from 1 to 400, writing each number on its own line, with no other text.",
            false,
        ),
    )
    .await;
    c.skip_past();
    c.ok(Command::RunJobNow { job: j.id.clone() }).await;
    let running = fired(&mut c, &j.id, "触发（进行中）", |e| {
        e.outcome == JobOutcome::Running
    })
    .await;
    (c, j, running)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_firing_cut_short_by_a_stop_is_recorded_as_failed() {
    let lv = Live::new();
    let (c, j, running) = fire_long(&lv).await;
    drop(c);
    lv.stop().await;
    let on_disk = lv.job_history_file(&j.id.0);
    let e = on_disk
        .iter()
        .find(|e| e.fired_ms == running.fired_ms)
        .expect("那次触发");
    assert!(
        matches!(&e.outcome, JobOutcome::Failed { .. }),
        "正常停实例时引擎收掉那一轮，记录随之收尾：{e:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_firing_left_running_by_a_crash_is_marked_interrupted_on_start() {
    let lv = Live::new();
    let (c, j, running) = fire_long(&lv).await;
    drop(c);
    lv.kill_host().await;
    let on_disk = lv.job_history_file(&j.id.0);
    assert_eq!(
        on_disk.last().map(|e| &e.outcome),
        Some(&JobOutcome::Running),
        "崩溃时那一轮没有结局"
    );

    let c = lv.start_again().await;
    let Reply::JobHistory(h) = c.ok(Command::JobHistory { job: j.id.clone() }).await else {
        panic!("job_history")
    };
    let e = h
        .iter()
        .find(|e| e.fired_ms == running.fired_ms)
        .expect("那次触发");
    assert!(
        matches!(&e.outcome, JobOutcome::Failed { message } if message.contains("interrupted")),
        "{e:?}"
    );
    assert!(
        lv.job_history_file(&j.id.0)
            .iter()
            .all(|e| e.outcome != JobOutcome::Running),
        "落盘的记录也改了"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_missed_firing_is_caught_up_once_only_when_asked() {
    let lv = Live::new();
    let c = lv.start().await;
    let a = save(
        &c,
        job("catch", "0 * * * *", "Reply with exactly: CAUGHT-UP", true),
    )
    .await;
    let b = save(
        &c,
        job("skip", "0 * * * *", "Reply with exactly: SKIPPED", false),
    )
    .await;
    drop(c);
    lv.stop().await;

    // 装作实例停了两个小时：把两个作业的基线挪到两小时前。
    let path = lv.instance_dir().join("jobs.json");
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let past = live::now_ms() - 2 * 3_600_000;
    for s in stored.as_array_mut().unwrap() {
        s["created_ms"] = past.into();
        s["last_fired_ms"] = past.into();
    }
    std::fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();

    let mut c = lv.start_again().await;
    let done = fired(&mut c, &a.id, "补跑的结局", |e| {
        e.outcome != JobOutcome::Running
    })
    .await;
    assert_eq!(done.outcome, JobOutcome::Done);
    assert_eq!(lv.job_history_file(&a.id.0).len(), 1, "错过两次也只补一次");
    assert!(
        lv.job_history_file(&b.id.0).is_empty(),
        "不补跑的作业不触发"
    );
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for s in stored.as_array().unwrap() {
        assert!(
            s["last_fired_ms"].as_u64().unwrap() > past,
            "两个作业的基线都前移了：{s}"
        );
    }
    let nb = list(&c).await.into_iter().find(|x| x.id == b.id).unwrap();
    assert!(
        nb.next_fire_ms.unwrap() > live::now_ms(),
        "不补跑的排到下一个整点"
    );
}

/// 第 2 批会把作业工具换成所有会话都有的 `job_*` 并按调用会话限作用域、删掉编辑会话；
/// 那时这个用例改成在普通会话里调工具。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_job_editor_session_changes_jobs_through_its_host_tools() {
    let lv = Live::new();
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    let Reply::Session(ed) = c.ok(Command::OpenJobSession { job: None }).await else {
        panic!("open_job_session")
    };
    assert!(matches!(
        ed.provenance.origin,
        Origin::Editor {
            of: port::EditorOf::Job,
            ..
        }
    ));
    let Reply::Session(again) = c.ok(Command::OpenJobSession { job: None }).await else {
        panic!("open_job_session")
    };
    assert_eq!(again.id, ed.id, "同一个键总是同一个编辑会话");
    assert_eq!(
        lv.read("sessions.json")[&ed.id.0]["editor_key"],
        "job:",
        "编辑键记在 sessions.json"
    );
    c.watch(&ed.id).await;

    let t = c
        .turn(
            &ed.id,
            "Call the JobSave tool once with name \"live-tool\", cron \"0 4 * * *\" and prompt \
             \"say hi\". Do not ask me to confirm. Then reply with only the word SAVED.",
        )
        .await;
    t.ended();
    assert!(t.tools.iter().any(|n| n == "JobSave"), "{:?}", t.tools);
    let made = c
        .events()
        .into_iter()
        .find_map(|e| match e {
            Event::JobUpserted(j) if j.name == "live-tool" => Some(j),
            _ => None,
        })
        .expect("JobSave 推了 JobUpserted");
    assert_eq!(made.cron, "0 4 * * *");
    assert!(list(&c).await.iter().any(|j| j.id == made.id));

    let t = c
        .turn(
            &ed.id,
            "Call JobList, then call JobDelete for the job named \"live-tool\". Do not ask me to \
             confirm. Then reply with only the word DELETED.",
        )
        .await;
    t.ended();
    assert!(t.tools.iter().any(|n| n == "JobDelete"), "{:?}", t.tools);
    let id = made.id.clone();
    assert!(
        c.events()
            .iter()
            .any(|e| matches!(e, Event::JobRemoved { job } if *job == id)),
        "JobDelete 推了 JobRemoved"
    );
    assert!(list(&c).await.is_empty());
}
