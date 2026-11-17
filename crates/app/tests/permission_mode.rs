//! 权限模式是会话的（需求 C-20）：Shift+Tab 在打开的会话上发 `SetPermissionMode`、立即生效；
//! 换会话显示跟着换；草稿里选的随建会话带上。

mod common;

use common::Harness;
use port::{Command, PermissionMode, SessionId};
use tui::Size;

const SIZE: Size = Size::new(160, 45);

fn session_id(h: &Harness, title: &str) -> SessionId {
    h.core
        .state
        .domain
        .sessions
        .iter()
        .find(|x| x.title == title)
        .unwrap_or_else(|| panic!("没有会话 {title}"))
        .id
        .clone()
}

fn sets(h: &Harness) -> Vec<(SessionId, PermissionMode)> {
    h.calls("dev")
        .into_iter()
        .filter_map(|c| match c {
            Command::SetPermissionMode { session, mode } => Some((session, mode)),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn shift_tab_sets_the_open_sessions_mode_and_the_display_follows_the_session() {
    let mut h = Harness::new("default", SIZE).await;
    h.open_session("昨天的调研").await;
    let a = session_id(&h, "昨天的调研");
    h.keys("Esc").await;
    h.keys("BackTab").await;
    h.settle().await;
    assert_eq!(sets(&h), vec![(a.clone(), PermissionMode::AcceptEdits)]);
    assert_eq!(h.core.state.permission_shown(), PermissionMode::AcceptEdits);
    assert!(h.text().contains("自动接受编辑"));
    assert_eq!(
        h.core.state.ui.permission,
        PermissionMode::Default,
        "草稿的模式没被动"
    );

    // 另一个会话是它自己的模式；回来还是切过的那个。
    h.open_session("排查内存泄漏").await;
    h.keys("Esc").await;
    assert_eq!(h.core.state.permission_shown(), PermissionMode::Default);
    assert!(!h.text().contains("自动接受编辑"));
    h.open_session("昨天的调研").await;
    h.keys("Esc").await;
    assert_eq!(h.core.state.permission_shown(), PermissionMode::AcceptEdits);
}

#[tokio::test(start_paused = true)]
async fn a_drafts_mode_goes_with_the_session_it_creates() {
    let mut h = Harness::new("default", SIZE).await;
    assert!(h.core.state.ui.chat.is_none(), "启动即草稿");
    h.keys("BackTab BackTab").await;
    assert_eq!(h.core.state.permission_shown(), PermissionMode::Plan);
    assert!(sets(&h).is_empty(), "草稿没有会话，不发命令");
    h.type_str("看看就好").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(
        h.calls("dev").iter().any(|c| matches!(
            c,
            Command::CreateSession {
                permission: Some(PermissionMode::Plan),
                ..
            }
        )),
        "建会话带上草稿的模式"
    );
    assert!(h.core.state.ui.chat.is_some());
    assert_eq!(h.core.state.permission_shown(), PermissionMode::Plan);
    assert_eq!(
        h.core.state.ui.permission,
        PermissionMode::Default,
        "下一个草稿从缺省起"
    );
}
