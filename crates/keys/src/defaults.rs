//! 内置键位表（docs/design/interaction.md §5–§7）。

use crate::{Action, Binding, Context, Key};

fn b(context: Context, keys: &str, action: Action) -> Binding {
    Binding {
        context,
        keys: keys
            .split(' ')
            .map(|k| Key::parse(k).unwrap_or_else(|| panic!("内置键位写错了：{k}")))
            .collect(),
        action,
    }
}

pub(crate) fn bindings() -> Vec<Binding> {
    use Action as A;
    use Context as C;
    let mut v = vec![
        // 全局（交互 §7）
        b(C::Global, "F1", A::Help),
        b(C::Global, "F2", A::TabSessions),
        b(C::Global, "Alt+1", A::TabSessions),
        b(C::Global, "F3", A::TabFlows),
        b(C::Global, "Alt+2", A::TabFlows),
        b(C::Global, "F4", A::TabJobs),
        b(C::Global, "Alt+3", A::TabJobs),
        b(C::Global, "F6", A::FocusToggle),
        b(C::Global, "Alt+O", A::FocusToggle),
        b(C::Global, "Ctrl+O", A::ToggleDetail),
        b(C::Global, "Ctrl+T", A::ToggleTodos),
        b(C::Global, "Ctrl+B", A::PaneToggle),
        b(C::Global, "Ctrl+G", A::NextAttention),
        b(C::Global, "F8", A::MouseToggle),
        b(C::Global, "Alt+M", A::MouseToggle),
        b(C::Global, "Ctrl+C", A::Interrupt),
        b(C::Global, "Ctrl+D", A::Detach),
        b(C::Global, "Ctrl+L", A::Redraw),
        b(C::Global, "Esc", A::Back),
        b(C::Global, "F12", A::EventLog),
        // 输入框
        b(C::Composer, "Enter", A::Submit),
        b(C::Composer, "Shift+Enter", A::Newline),
        b(C::Composer, "Alt+Enter", A::Newline),
        b(C::Composer, "Ctrl+J", A::Newline),
        b(C::Composer, "Tab", A::Complete),
        b(C::Composer, "BackTab", A::CycleMode),
        b(C::Composer, "Left", A::CursorLeft),
        b(C::Composer, "Right", A::CursorRight),
        b(C::Composer, "Alt+Left", A::WordLeft),
        b(C::Composer, "Alt+Right", A::WordRight),
        b(C::Composer, "Home", A::LineStart),
        b(C::Composer, "Ctrl+A", A::LineStart),
        b(C::Composer, "End", A::LineEnd),
        b(C::Composer, "Ctrl+E", A::LineEnd),
        b(C::Composer, "Ctrl+U", A::ClearInput),
        b(C::Composer, "Ctrl+W", A::DeleteWord),
        b(C::Composer, "Ctrl+K", A::KillToEnd),
        b(C::Composer, "Backspace", A::Backspace),
        b(C::Composer, "Delete", A::DeleteForward),
        b(C::Composer, "Ctrl+Left", A::WordLeft),
        b(C::Composer, "Ctrl+Right", A::WordRight),
        b(C::Composer, "Up", A::Up),
        b(C::Composer, "Down", A::Down),
        b(C::Composer, "PageUp", A::PageUp),
        b(C::Composer, "PageDown", A::PageDown),
        b(C::Composer, "Ctrl+Up", A::FocusTranscript),
        // 转录
        b(C::Transcript, "Up", A::Up),
        b(C::Transcript, "k", A::Up),
        b(C::Transcript, "Down", A::Down),
        b(C::Transcript, "j", A::Down),
        b(C::Transcript, "PageUp", A::PageUp),
        b(C::Transcript, "PageDown", A::PageDown),
        b(C::Transcript, "Home", A::Top),
        b(C::Transcript, "End", A::Bottom),
        b(C::Transcript, "Alt+Up", A::BlockPrev),
        b(C::Transcript, "Alt+Down", A::BlockNext),
        b(C::Transcript, "Enter", A::Open),
        // 补全列表
        b(C::Picker, "Up", A::Up),
        b(C::Picker, "Down", A::Down),
        b(C::Picker, "Tab", A::Complete),
        b(C::Picker, "Enter", A::Submit),
        // 对话框（提问框、二次确认）
        b(C::Dialog, "Up", A::Up),
        b(C::Dialog, "Down", A::Down),
        b(C::Dialog, "Enter", A::Confirm),
        b(C::Dialog, "y", A::Yes),
        b(C::Dialog, "n", A::No),
        // 实例选择
        b(C::Instances, "Up", A::Up),
        b(C::Instances, "k", A::Up),
        b(C::Instances, "Down", A::Down),
        b(C::Instances, "j", A::Down),
        b(C::Instances, "Enter", A::Open),
        b(C::Instances, "n", A::New),
        b(C::Instances, "s", A::Stop),
        b(C::Instances, "f", A::Force),
        b(C::Instances, "q", A::Quit),
        // 浮层
        b(C::Overlay, "Up", A::Up),
        b(C::Overlay, "Down", A::Down),
        b(C::Overlay, "PageUp", A::PageUp),
        b(C::Overlay, "PageDown", A::PageDown),
        b(C::Overlay, "q", A::Back),
    ];
    for (i, a) in [
        A::Choose1,
        A::Choose2,
        A::Choose3,
        A::Choose4,
        A::Choose5,
        A::Choose6,
        A::Choose7,
        A::Choose8,
        A::Choose9,
    ]
    .into_iter()
    .enumerate()
    {
        v.push(b(C::Dialog, &(i + 1).to_string(), a));
    }
    // 右栏三个标签的公共键位（交互 §5.1）
    for ctx in [C::Sessions, C::Flows, C::Jobs] {
        v.extend([
            b(ctx, "Up", A::Up),
            b(ctx, "k", A::Up),
            b(ctx, "Down", A::Down),
            b(ctx, "j", A::Down),
            b(ctx, "Left", A::Collapse),
            b(ctx, "h", A::Collapse),
            b(ctx, "Right", A::Expand),
            b(ctx, "l", A::Expand),
            b(ctx, "Enter", A::Open),
            b(ctx, "Tab", A::SwitchSection),
            b(ctx, "/", A::Filter),
            b(ctx, "<", A::RatioDec),
            b(ctx, ">", A::RatioInc),
            b(ctx, "?", A::Help),
            b(ctx, "PageUp", A::PageUp),
            b(ctx, "PageDown", A::PageDown),
            b(ctx, "n", A::New),
        ]);
    }
    v.extend([
        b(C::Sessions, "r", A::Rename),
        b(C::Sessions, "x", A::Close),
        b(C::Sessions, "p", A::PauseResume),
        b(C::Sessions, "m", A::Message),
        b(C::Sessions, "s", A::Stop),
        b(C::Flows, "v", A::View),
        b(C::Flows, "p", A::PauseResume),
        b(C::Flows, "x", A::Close),
        b(C::Flows, "c", A::Confirm),
        b(C::Flows, "a", A::Release),
        b(C::Flows, "f", A::Force),
        b(C::Flows, "r", A::Rework),
        b(C::Flows, "t", A::Retry),
        b(C::Jobs, "e", A::Edit),
        b(C::Jobs, "Space", A::ToggleEnabled),
        b(C::Jobs, "R", A::RunNow),
        b(C::Jobs, "d", A::Delete),
    ]);
    v
}

/// 底栏左侧按上下文提示哪些动作（交互 §2.3 示意图）。切栏与帮助另由 `Keymap::essential` 给出。
pub(crate) fn footer(ctx: Context) -> &'static [Action] {
    use Action as A;
    match ctx {
        Context::Global | Context::Composer => &[
            A::TabSessions,
            A::FocusToggle,
            A::NextAttention,
            A::ToggleDetail,
            A::MouseToggle,
        ],
        Context::Transcript => &[A::BlockPrev, A::Open, A::Bottom, A::Back],
        Context::Sessions => &[A::Open, A::New, A::Rename, A::Stop, A::SwitchSection],
        Context::Flows => &[A::Open, A::New, A::Confirm, A::Rework, A::View],
        Context::Jobs => &[A::New, A::Edit, A::ToggleEnabled, A::RunNow],
        Context::Dialog => &[A::Confirm, A::Yes, A::No, A::Back],
        Context::Picker => &[A::Complete, A::Submit, A::Back],
        Context::Instances => &[A::Open, A::New, A::Force, A::Stop, A::Quit],
        Context::Overlay => &[A::Back],
    }
}
