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
    let t = h.panel();
    for want in [
        "收集需求",
        "代码评审 ∥ 文档评审",
        "← 等 代码评审、文档评审",
        "skill implement",
        "出口 output、gate: ci",
    ] {
        assert!(t.contains(want), "缺 {want}\n{t}");
    }
    assert!(h.text().contains("需求到上线 的各站"), "下半是定义的各站");
    assert!(
        h.text()
            .contains("─ 流程设计：需求到上线 · feature-to-ship"),
        "面板的标题线"
    );
    assert!(h.text().contains("Ctrl+T 收起"));
    assert!(!h.transcript().contains("流程设计"), "不在转录里");
    h.snapshot("flow-authoring");
    h.keys("Ctrl+T").await;
    assert!(h.panel().is_empty(), "Ctrl+T 收起");
    let text = h.text();
    assert!(
        text.contains("─ 流程设计：需求到上线") && text.contains("Ctrl+T 展开"),
        "收起后标题线还在\n{text}"
    );
    h.keys("Ctrl+T").await;
    assert!(!h.panel().is_empty());
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
        "← 等 代码评审、文档评审",
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
    assert!(!h.panel().contains("安全评审"));
    h.keys("Enter").await;
    h.type_str("加一个安全评审站").await;
    h.keys("Enter").await;
    h.advance(3_000).await;
    let t = h.panel();
    assert!(t.contains("安全评审"), "流程设计跟着变\n{t}");
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
    let t = h.panel();
    assert!(
        h.text().contains("─ 作业：每日巡检 · 启用") && t.contains("最近的触发"),
        "作业面板\n{t}"
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

fn station_row(h: &Harness, id: &str) -> usize {
    h.core
        .state
        .flow_lower_rows()
        .iter()
        .position(|r| matches!(r, FlowLower::Station(s) if s.as_str() == id))
        .unwrap_or_else(|| panic!("没有站 {id}"))
}

#[tokio::test(start_paused = true)]
async fn run_stations_are_flat_colored_and_switch_to_their_context() {
    let mut h = flows("demo").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    assert!(
        h.core
            .state
            .flow_rows()
            .iter()
            .any(|r| matches!(r, FlowRow::Run(f, _) if f == "feature-to-ship")),
        "选中流程即展开它的运行"
    );
    h.pick_upper(run_index(&h, "r1")).await;
    let p = h.panel();
    assert!(
        h.text()
            .contains("─ 运行：支付重试（需求到上线） · ● 运行中 · 站 3/7 实现")
            && p.contains("✓ 收集需求 → ✓ 设计 → ● 实现"),
        "运行里的会话，面板是运行进度\n{p}"
    );
    let text = h.text();
    assert!(
        text.contains("│   ✓ 收集需求") && text.contains("│ ▾ ● 实现"),
        "站不缩进\n{text}"
    );
    h.pick_lower(station_row(&h, "design")).await;
    assert_eq!(h.chat_title(), "设计（独立会话）", "有独立会话的站切到它");
    h.pick_lower(station_row(&h, "collect")).await;
    assert_eq!(h.chat_title(), "支付重试（主会话）", "其余的站在主会话里");
    let impl_at = station_row(&h, "impl");
    h.pick_lower(impl_at).await;
    let n = h.core.state.flow_lower_rows().len();
    h.keys("Left").await;
    assert_eq!(
        h.core.state.flow_lower_rows().len(),
        n - 2,
        "← 收起站里的子 Agent"
    );
    h.keys("Right").await;
    assert_eq!(h.core.state.flow_lower_rows().len(), n);
    h.keys("Down").await;
    assert_eq!(h.chat_title(), "后端", "选中站里的子 Agent 切到它的会话");
    h.snapshot("flow-run-demo");
}
