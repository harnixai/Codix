//! 鼠标只做命中测试与拖动状态，命中之后一律翻译成与键盘相同的动作（需求 N-07、
//! testing.md §5「鼠标不另走逻辑」）。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down(MouseButton),
    Up(MouseButton),
    Drag(MouseButton),
    ScrollUp,
    ScrollDown,
    Moved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    pub col: u16,
    pub row: u16,
}
