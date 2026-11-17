//! 场景 m09、m10、m25：会话标签（需求 S-01 至 S-09、C-12、C-16，交互 §4.1、§5.2）。

mod common;

use common::Harness;
use port::Command;
use tui::Size;

const SIZE: Size = Size::new(160, 45);

async fn wb(scenario: &str) -> Harness {
    Harness::new(scenario, SIZE).await
}

#[tokio::test(start_paused = true)]
async fn upper_lists_main_agents_with_origin_and_expands_goal_and_todo() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    let rows = h.core.state.session_rows();
    assert!(
        rows.iter()
            .all(|r| !r.session().as_str().starts_with("s1-")),
        "子 Agent 的侧链不在主列表里"
    );
    let text = h.text();
    assert!(text.contains("⏰"), "作业来源图标");
    assert!(text.contains("⇶") && text.contains("⑂"));
    h.keys("Right").await;
    let text = h.text();
    assert!(
        text.contains("goal") && text.contains("迁到 OAuth"),
        "{text}"
    );
    assert!(text.contains("第 2/256 轮"));
    assert!(text.contains("todo ▸ 2/7"));
    h.keys("Down Down Enter").await;
    assert!(h.text().contains("todo ▾"), "todo 展开明细");
    h.snapshot("sessions-upper-expanded");
    h.keys("Left").await;
    assert!(
        !h.core
            .state
            .session_rows()
            .iter()
            .any(|r| matches!(r, model::SessRow::Goal(_))),
        "收起"
    );
}

#[tokio::test(start_paused = true)]
async fn lower_tree_three_levels_and_finished_group() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    let text = h.text();
    for want in [
        "主 Agent",
        "审查认证改动",
        "cargo test",
        "检查依赖许可",
        "cargo deny check licenses",
        "已结束 (",
        "├ ",
        "└ ",
    ] {
        assert!(text.contains(want), "缺 {want}\n{text}");
    }
    assert!(text.contains("等你批准"), "在等人的子 Agent 高亮");
    let in_tree = |h: &Harness, label: &str| {
        h.core.state.tree_rows().iter().any(|r| match r {
            model::TreeRow::Task { id, .. } => h
                .core
                .state
                .conv_tasks()
                .iter()
                .any(|t| &t.id == id && t.label == label),
            _ => false,
        })
    };
    assert!(!in_tree(&h, "调研 OAuth 库"), "已结束的缺省收起");
    assert!(text.contains("◆ 审查认证改动"), "子 Agent 有图标");
    assert!(text.contains("$ cargo test"), "shell 任务有自己的图标");
    h.keys("Tab").await;
    let finished = h
        .core
        .state
        .tree_rows()
        .iter()
        .position(|r| matches!(r, model::TreeRow::Finished { .. }))
        .unwrap();
    for _ in 0..finished {
        h.keys("Down").await;
    }
    h.keys("Enter").await;
    assert!(in_tree(&h, "调研 OAuth 库"), "展开已结束");
    h.snapshot("sessions-tree");
}

#[tokio::test(start_paused = true)]
async fn drill_into_sub_agent_message_it_and_come_back() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    h.select_task("审查认证改动").await;
    h.keys("Enter").await;
    h.settle().await;
    let text = h.text();
    assert!(
        text.contains("会话 › 重构认证模块 › 审查认证改动"),
        "面包屑\n{text}"
    );
    assert!(text.contains("子 Agent ← 重构认证模块"), "归属行");
    h.type_str("先只看 auth 目录").await;
    h.keys("Enter").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::MessageTask { task, text } if task.as_str() == "t1" && text == "先只看 auth 目录"
    )));
    h.keys("Esc").await;
    assert!(h.text().contains("会话 › 重构认证模块"));
    assert!(!h.text().contains("› 审查认证改动 ─"));
}

#[tokio::test(start_paused = true)]
async fn task_output_is_read_only_and_follows() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    h.select_task("tail -f app.log").await;
    h.keys("Enter").await;
    h.settle().await;
    let text = h.text();
    assert!(text.contains("› tail -f app.log"), "{text}");
    h.type_str("abc").await;
    assert!(h.core.state.ui.composer.text.is_empty(), "只读");
    h.keys("Esc").await;
    assert!(h.core.state.ui.task_view.is_none());
}

#[tokio::test(start_paused = true)]
async fn stop_rename_close_with_confirmation() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    h.select_task("tail -f app.log").await;
    h.keys("s").await;
    assert!(h.last_line().contains("停止「tail -f app.log」"));
    h.keys("y").await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::StopTask { task } if task.as_str() == "t4"))
    );
    h.select_session("昨天的调研").await;
    h.keys("r").await;
    assert_eq!(h.core.state.ui.composer.text, "昨天的调研");
    h.type_str("（已归档）").await;
    h.keys("Enter").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::Rename { title, .. } if title == "昨天的调研（已归档）"
    )));
    h.select_session("翻译文档").await;
    h.keys("x y").await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::Close { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn goal_pause_and_clear() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    h.keys("Right Down p").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::UpdateGoal {
            action: port::GoalAction::Pause,
            ..
        }
    )));
    h.keys("x y").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::UpdateGoal {
            action: port::GoalAction::Clear,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn m09_tree_changes_live() {
    let mut h = wb("m09-session-tree").await;
    h.select_session("重构认证模块").await;
    h.advance(21_000).await;
    let tasks = h.core.state.conv_tasks();
    assert!(
        tasks.iter().any(|t| t.label == "补测试"),
        "新出现的子 Agent"
    );
    let t9 = tasks.iter().find(|t| t.id.as_str() == "t9").unwrap();
    assert_eq!(t9.status, port::TaskStatus::Done);
    assert!(tasks.iter().any(|t| t.id.as_str() == "t14"));
}

#[tokio::test(start_paused = true)]
async fn m10_goal_rounds_and_blocked() {
    let mut h = wb("m10-goal-todo").await;
    h.select_session("重构认证模块").await;
    h.keys("Right").await;
    h.advance(10_000).await;
    assert!(
        h.text().contains("第 4/256 轮") || h.text().contains("第 5/256 轮"),
        "{}",
        h.text()
    );
    h.advance(8_000).await;
    assert!(h.text().contains("受阻：需要你确认数据库迁移方案"));
}

#[tokio::test(start_paused = true)]
async fn m25_ownership_line_for_every_origin() {
    for (title, want) in [
        ("支付重试（主会话）", "流程「需求到上线」› 支付重试 › 实现"),
        ("日志巡检", "作业「"),
        ("昨天的调研", "分叉自 重构认证模块"),
        ("重构认证模块", "用户新建"),
        ("老项目会话", "⚠ 被实例 research 占用"),
    ] {
        let mut h = wb("default").await;
        h.open_session(title).await;
        let text = h.text();
        assert!(text.contains(want), "{title}：缺 {want}\n{text}");
    }
}
