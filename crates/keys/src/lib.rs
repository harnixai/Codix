//! 快捷键（docs/design/architecture.md §6）：按键、带上下文的绑定表、和弦状态机、底栏提示。
//!
//! 绑定是 `(上下文, 按键序列) → 动作`。查找先当前上下文、再 `Global`；同一上下文里同一个
//! 键只能绑一个动作，冲突在加载时报出来。`Composer` 上下文里无修饰的可打印字符一律是输入。

mod action;
mod defaults;
mod key;
mod keymap;
mod resolver;

pub use action::Action;
pub use key::{Key, KeyCode, Mods};
pub use keymap::{Binding, Context, Keymap, KeymapError};
pub use resolver::{Resolution, Resolver};
