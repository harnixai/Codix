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
        h.text().contains("─ 流程设计：需求到上线 ─ │"),
        "面板的标题线"
    );
    assert!(
        h.text().contains("─ Ctrl+T 收起 ─"),
        "收放的快捷键在下沿右边"
    );
    assert!(!h.transcript().contains("流程设计"), "不在转录里");
    h.snapshot("flow-authoring");
    h.keys("Ctrl+T").await;
    assert!(h.panel().is_empty(), "Ctrl+T 收起");
    let text = h.text();
    assert!(
        text.contains("─ 流程设计：需求到上线  Ctrl+T 展开 ─ │"),
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
    assert!(
        h.text().contains("流程 › 支付重试 › 后端"),
        "面包屑从「流程」起"
    );
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
        h.text().contains("─ 作业：每日巡检 ─ │") && t.contains("执行 3 次 · 成功 2 · 失败 1"),
        "作业面板\n{t}"
    );
    let text = h.text();
    assert!(text.contains("每日巡检 的详情"));
    assert!(text.contains("触发记录"));
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
        h.text().contains("─ 运行：支付重试 ─ │") && p.contains("✓ 收集需求 → ✓ 设计 → ● 实现"),
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

#[tokio::test(start_paused = true)]
async fn selecting_a_station_of_a_definition_shows_its_config_in_the_panel() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    let imp = h
        .core
        .state
        .flow_lower_rows()
        .iter()
        .position(|r| matches!(r, FlowLower::DefStation(2)))
        .unwrap();
    h.pick_lower(imp).await;
    assert!(
        h.chat_title().contains("编写流程：需求到上线"),
        "左边仍是编写会话"
    );
    let text = h.text();
    assert!(
        text.contains("─ 流程设计：需求到上线 › 第 3 站 实现 ─ │"),
        "{text}"
    );
    let p = h.panel();
    for want in [
        "skill",
        "implement",
        "前一站",
        "设计",
        "下一站",
        "代码评审、文档评审",
        "- id: impl",
        "tasks:",
    ] {
        assert!(p.contains(want), "缺 {want}\n{p}");
    }
    assert!(text.contains("Shift+↑↓"), "超过 10 行时下沿标出可以滚动");
    let sel = h.core.state.ui.lower().selected;
    h.keys("Shift+Down").await;
    assert_eq!(
        h.core.state.ui.lower().selected,
        sel + 1,
        "焦点在右下时 Shift+↓ 滚右下"
    );
    h.keys("Shift+Up").await;
    h.keys("Esc Ctrl+Up Ctrl+Up").await;
    assert_eq!(
        h.core.state.ui.focus,
        model::Focus::Panel,
        "转录里 Ctrl+↑ 到面板"
    );
    let before = h.text();
    h.keys("Shift+Down Down").await;
    assert_ne!(before, h.text(), "面板有焦点时滚面板");
    assert!(h.text().contains("↑2 "));
    h.snapshot("flow-station-config");
    h.keys("F3").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    assert!(
        h.text().contains("─ 流程设计：需求到上线 ─ │"),
        "回到上半看整个流程"
    );
}

#[tokio::test(start_paused = true)]
async fn global_flows_are_marked_and_flow_jobs_jump_to_their_flow() {
    let mut h = flows("default").await;
    let text = h.text();
    assert!(text.contains("◎ 需求到上线"), "全局流程标 ◎\n{text}");
    assert!(!text.contains("◎ 周报"), "项目级的不标");
    h.keys("F4").await;
    let weekly = h
        .core
        .state
        .job_rows()
        .iter()
        .position(|id| {
            h.core
                .state
                .domain
                .jobs
                .iter()
                .any(|j| &j.id == id && matches!(j.kind, port::JobKind::Flow { .. }))
        })
        .unwrap();
    h.pick_upper(weekly).await;
    let link = h
        .core
        .state
        .job_lower_rows()
        .iter()
        .position(|r| matches!(r, JobLower::Flow(_)))
        .unwrap();
    h.pick_lower(link).await;
    h.keys("Enter").await;
    h.settle().await;
    assert_eq!(h.core.state.ui.tab, model::Tab::Flows, "跳到流程标签");
    assert!(
        matches!(h.core.state.selected_flow_row(), Some(FlowRow::Flow(_))),
        "选中它用的流程"
    );
    assert!(h.chat_title().starts_with("编写流程："));
}

#[tokio::test(start_paused = true)]
async fn run_lower_shows_time_and_tokens_but_no_status_words() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.pick_upper(run_index(&h, "r1")).await;
    let tui::Body::Workbench(w) = h.core.frame().body else {
        panic!("不在工作台")
    };
    let lower: String = w
        .side
        .unwrap()
        .lower
        .rows
        .iter()
        .map(|l| l.text() + "\n")
        .collect();
    for word in ["已通过", "进行中", "待进入", "运行中", "完成"] {
        assert!(!lower.contains(word), "右下不写状态字：{word}\n{lower}");
    }
    assert!(
        lower.contains("7m00s, 3.0k/900"),
        "用时, 输入/输出\n{lower}"
    );
    assert!(!lower.contains("入") && !lower.contains("出 "), "{lower}");
    assert!(lower.contains("7m00s"), "{lower}");
}
