//! socket 服务与会话代理之间的接口：服务端管连接（握手、单一接入、心跳、踢人），
//! 端口命令交给 [`Service`]。测试用假的 `Service` 单独验证接入流程。

use async_trait::async_trait;
use port::link::ServerFrame;
use port::{Command, InstanceInfo, PortError, Reply};
use tokio::sync::mpsc;

/// 发给当前接入者的出口。回复与事件走同一条 channel，所以等到一个回复时，处理那条命令
/// 期间推出的事件已经排在它前面（architecture §8.3）。
pub type Sink = mpsc::UnboundedSender<ServerFrame>;

#[async_trait]
pub trait Service: Send + Sync + 'static {
    /// 接入者换了；`None` 表示没人接入。新的接入者从零开始：之前 Watch 的会话一律作废。
    fn attach(&self, sink: Option<Sink>);
    /// 实例信息（`holder` 由服务端填）与能力位 `(flows, jobs)`。
    fn hello(&self) -> (InstanceInfo, bool, bool);
    /// 握手、心跳、脱离、停实例之外的端口命令。
    async fn handle(&self, cmd: Command) -> Result<Reply, PortError>;
}
