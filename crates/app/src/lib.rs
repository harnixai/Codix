//! codix 二进制的全部逻辑。`main.rs` 只解析命令行、接上真实终端；这里的 `Core` 不碰终端，
//! 测试用 `FakeTerm` 与 mock 后端驱动它（docs/design/testing.md §2）。

pub mod config;
pub mod core;
pub mod snapshot;
pub mod term;

pub use core::Core;
