//! TUI 一侧接入实例（architecture §8.3）：`SocketInstances` 扫描、拉起、停止实例，
//! `SocketBackend` 是经 `host.sock` 接入后的 `port::Backend`。

pub mod layout;

pub use layout::{InstanceFile, InstanceMeta, Layout};
