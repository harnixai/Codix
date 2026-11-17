//! 场景 m11、m12、m13：流程标签与作业标签；右栏选中即切换左边的对话区（需求 F、J）。

mod common;

use common::Harness;
use model::{FlowLower, FlowRow, JobLower};
use port::{Command, RunOp};
use tui::Size;

const SIZE: Size = Size::new(160, 45);

async fn flows(scenario: &str) -> Harness {
    let mut h = Harness::new(scenario, SIZE).await;
    h.keys("F3").await;
    h.settle().await;
    h
}

fn flow_index(h: &Harness, name: &str) -> usize {
    h.core
        .state
        .flow_rows()
        .iter()
        .position(|r| r == &FlowRow::Flow(name.into()))
        .unwrap()
}

fn run_index(h: &Harness, run: &str) -> usize {
    h.core
        .state
        .flow_rows()
        .iter()
        .position(|r| matches!(r, FlowRow::Run(_, id) if id.as_str() == run))
        .unwrap_or_else(|| panic!("没有运行 {run}"))
}

#[tokio::test(start_paused = true)]
async fn selecting_a_flow_opens_its_authoring_session_with_a_design_card() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.settle().await;
    assert!(
        h.chat_title().contains("编写流程：需求到上线"),
        "{}",
        h.chat_title()
    );
    let t = h.transcript();
    for want in [
        "流程设计：需求到上线",
        "收集需求",
        "代码评审 ∥ 文档评审",
        "← 等 代码评审、文档评审",
        "skill implement",
        "出口 output、gate: ci",
    ] {
        assert!(t.contains(want), "缺 {want}\n{t}");
    }
    assert!(h.text().contains("需求到上线 的各站"), "下半是定义的各站");
    h.snapshot("flow-authoring-card");
    h.keys("v").await;
    assert!(h.text().contains("feature-to-ship 的定义"), "查看 YAML");
}

#[tokio::test(start_paused = true)]
async fn selecting_a_run_follows_its_main_agent_and_shows_stations() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "r1")).await;
    h.settle().await;
    assert_eq!(
        h.chat_title(),
        "支付重试（主会话）",
        "左边跟着切到运行的主 Agent"
    );
    let text = h.text();
    for want in [
        "主 Agent",
        "收集需求",
        "实现",
        "后端",
        "前端",
        "← 等 代码评审, 文档评审",
    ] {
        assert!(text.contains(want), "缺 {want}\n{text}");
    }
    h.snapshot("flow-run");
    let rows = h.core.state.flow_lower_rows();
    let backend = rows
        .iter()
        .position(|r| matches!(r, FlowLower::StationTask(st, 0) if st.as_str() == "impl"))
        .unwrap();
    h.pick_lower(backend).await;
    h.settle().await;
    assert_eq!(h.chat_title(), "后端", "站里的子 Agent 有会话：左边跟着切");
    assert!(h.text().contains("会话 › 支付重试 › 后端"));
}

#[tokio::test(start_paused = true)]
async fn station_ops_and_run_ops() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "r2")).await;
    h.settle().await;
    let rows = h.core.state.flow_lower_rows();
    let sign = rows
        .iter()
        .position(|r| matches!(r, FlowLower::Station(id) if id.as_str() == "sign"))
        .unwrap();
    h.pick_lower(sign).await;
    h.keys("a").await;
    h.settle().await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::RunOp {
            op: RunOp::Release { .. },
            ..
        }
    )));
    let docs = rows
        .iter()
        .position(|r| matches!(r, FlowLower::Station(id) if id.as_str() == "review-docs"))
        .unwrap();
    h.pick_lower(docs).await;
    h.keys("r").await;
    assert!(h.last_line().contains("退回到「文档评审」"));
    h.keys("y").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::RunOp {
            op: RunOp::Rework { .. },
            ..
        }
    )));
    h.keys("f y").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::RunOp {
            op: RunOp::Force { .. },
            ..
        }
    )));
    h.keys("Tab p").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::RunOp {
            op: RunOp::Pause,
            ..
        }
    )));
    h.keys("x y").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::RunOp {
            op: RunOp::Cancel,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn m11_run_progresses_live() {
    let mut h = flows("m11-flow-run").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "r1")).await;
    h.advance(14_000).await;
    let v = h.core.state.selected_run().unwrap();
    let sign = v.stations.iter().find(|s| s.id.as_str() == "sign").unwrap();
    assert_eq!(sign.status, port::StationStatus::Held, "走到评审汇总并搁置");
    assert!(h.text().contains("搁置"));
    assert!(
        h.core
            .state
            .attention()
            .iter()
            .any(|a| matches!(a, model::Attention::Run(..)))
    );
}

#[tokio::test(start_paused = true)]
async fn m12_change_the_flow_by_talking_and_the_card_updates() {
    let mut h = flows("m12-flow-author").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.settle().await;
    assert!(!h.transcript().contains("安全评审"));
    h.keys("Enter").await;
    h.type_str("加一个安全评审站").await;
    h.keys("Enter").await;
    h.advance(3_000).await;
    let t = h.transcript();
    assert!(t.contains("安全评审"), "设计卡跟着变\n{t}");
    assert!(t.contains("← 等 代码评审、文档评审、安全评审"));
    h.keys("F3 n").await;
    assert_eq!(
        h.core.state.ui.composer.text, "启动一次运行",
        "n 在对话里启动"
    );
}

#[tokio::test(start_paused = true)]
async fn jobs_list_details_history_and_follow() {
    let mut h = Harness::new("default", SIZE).await;
    h.keys("F4").await;
    h.settle().await;
    h.pick_upper(0).await;
    h.settle().await;
    assert!(h.chat_title().starts_with("作业："), "{}", h.chat_title());
    let t = h.transcript();
    assert!(
        t.contains("作业：每日巡检") && t.contains("最近的触发"),
        "作业卡\n{t}"
    );
    let text = h.text();
    assert!(text.contains("每日巡检 的详情"));
    assert!(text.contains("触发历史"));
    assert!(text.contains("0 9 * * *"));
    h.snapshot("jobs");
    let rows = h.core.state.job_lower_rows();
    let first = rows
        .iter()
        .position(|r| matches!(r, JobLower::Entry(0)))
        .unwrap();
    h.pick_lower(first).await;
    h.settle().await;
    assert!(
        !h.chat_title().starts_with("作业："),
        "跳到那一次的会话：{}",
        h.chat_title()
    );
    h.keys("Tab Space").await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::SetJobEnabled { enabled: false, .. }))
    );
    h.keys("R").await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::RunJobNow { .. }))
    );
    h.keys("F4 e").await;
    assert_eq!(
        h.core.state.ui.composer.text, "把这个作业改成：",
        "修改在对话里说"
    );
}

#[tokio::test(start_paused = true)]
async fn m13_jobs_fire_and_fail() {
    let mut h = Harness::new("m13-jobs", SIZE).await;
    h.keys("F4").await;
    h.advance(7_000).await;
    let j4 = h
        .core
        .state
        .domain
        .jobs
        .iter()
        .find(|j| j.id.as_str() == "j4")
        .unwrap();
    assert!(matches!(j4.last, Some(port::JobOutcome::Failed { .. })));
    assert!(h.core.state.attention_count(model::Tab::Jobs) >= 1);
}
