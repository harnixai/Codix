use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::{Attach, ClientInfo, Command, Event, InstanceInfo, Millis, NewInstance, Reply};

/// 端口的错误。这里的 Display 只给日志与调试看（英文、不翻译）；界面上显示的说法由 model
/// 按当前语言给出（端口是协议，不带界面文字）。
#[derive(Debug, Clone, thiserror::Error, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortError {
    /// 实例已被另一个 TUI 接入（需求 I-06）。
    #[error("instance attached by {}", holder.label())]
    Occupied { holder: ClientInfo, idle_ms: Millis },
    #[error("no such instance: {0}")]
    NoSuchInstance(String),
    #[error("session held by instance {0}")]
    SessionHeld(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("disconnected")]
    Disconnected,
    #[error("{0}")]
    Failed(String),
}

/// 本机实例的目录：列出、新建、接入、启动、停止（需求 §4.1）。
#[async_trait]
pub trait Instances: Send + Sync + 'static {
    async fn list(&self, all: bool) -> Result<Vec<InstanceInfo>, PortError>;
    /// 返回新实例的信息（名字可能由目录生成）与它的后端。
    async fn create(
        &self,
        opts: NewInstance,
    ) -> Result<(InstanceInfo, Arc<dyn Backend>), PortError>;
    /// 实例已被接入且 `opts.force` 为假时返回 [`PortError::Occupied`]。
    async fn attach(&self, name: &str, opts: Attach) -> Result<Arc<dyn Backend>, PortError>;
    async fn start(&self, name: &str) -> Result<Arc<dyn Backend>, PortError>;
    async fn stop(&self, name: &str) -> Result<(), PortError>;
}

/// 一个已接入的实例。
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn call(&self, cmd: Command) -> Result<Reply, PortError>;
    /// 领域事件流。只能取一次：每个 Backend 只有一个订阅者。
    fn events(&self) -> mpsc::UnboundedReceiver<Event>;
}
