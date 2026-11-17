//! 会话代理只经这个 trait 用引擎：真实实现是嵌入的 Agix（`engine` crate），测试用脚本化的
//! 假引擎。通知（`session.event` / `daemon.event`）在构造时以 channel 另行交给 `Hub`。

use async_trait::async_trait;
use serde_json::Value;

pub use wire::{Notification, RpcError};

#[async_trait]
pub trait Engine: Send + Sync + 'static {
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError>;
}

#[async_trait]
impl Engine for wire::Client {
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        wire::Client::call(self, method, params).await
    }
}
