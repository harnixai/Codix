//! TUI 一侧接入实例（architecture §8.3）：`SocketInstances` 扫描、拉起、停止实例，
//! `SocketBackend` 是经 `host.sock` 接入后的 `port::Backend`。

pub mod backend;
pub mod layout;
pub mod local;

pub use backend::SocketBackend;
pub use layout::{InstanceFile, InstanceMeta, Layout};
pub use local::{hostname, now_ms, pid_alive, this_client};
