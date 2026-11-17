use serde::{Deserialize, Serialize};

use crate::{Millis, RunId, SessionId, StationId, TaskId, Usage};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowSummary {
    pub name: String,
    pub title: String,
    pub origin: FlowOrigin,
    #[serde(default)]
    pub active_runs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowOrigin {
    Builtin,
    Override,
    User,
}

/// 查看定义（需求 F-08）：站的概要与原始 YAML。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowDetail {
    pub summary: FlowSummary,
    pub stations: Vec<StationView>,
    pub yaml: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: RunId,
    pub flow: String,
    pub subject: String,
    pub status: RunStatus,
    /// 当前所在的站（标题）。
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub stations_done: u32,
    #[serde(default)]
    pub stations_total: u32,
    #[serde(default)]
    pub started_ms: Millis,
    #[serde(default)]
    pub ended_ms: Option<Millis>,
    #[serde(default)]
    pub usage: Usage,
}

/// 与 Harnix flow 的运行状态一致，"等待 / 暂停中 / 已暂停"是它派生出来的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    /// 卡在人或外部上：待审批、待确认、搁置、未达标、任务失败……
    Waiting {
        reason: String,
    },
    Pausing,
    Paused,
    Finished,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    pub summary: RunSummary,
    pub main_session: SessionId,
    pub stations: Vec<StationView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationView {
    pub id: StationId,
    pub title: String,
    #[serde(default)]
    pub needs: Vec<StationId>,
    /// `session`、`agent × 2`、`confirm`、`gate: ci` 这种给人看的执行方式。
    #[serde(default)]
    pub work: String,
    /// 这一站照哪个 skill 做（流程设计卡里显示）。
    #[serde(default)]
    pub skill: Option<String>,
    /// 出口条件，给人看的写法：`output`、`gate: ci`、`confirm`。
    #[serde(default)]
    pub exit: Vec<String>,
    pub status: StationStatus,
    #[serde(default)]
    pub tasks: Vec<StationTask>,
    #[serde(default)]
    pub elapsed_ms: Millis,
    #[serde(default)]
    pub usage: Usage,
    /// 未达标时的说明。
    #[serde(default)]
    pub unmet: Option<String>,
    /// 这一站有自己的会话（独立会话的站）；没有就在运行的主会话里做。
    #[serde(default)]
    pub session: Option<crate::SessionId>,
}

/// 与 Harnix flow 的 `StationStatus` 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationStatus {
    Pending,
    Held,
    Working,
    Unmet,
    Passed,
    Skipped,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationTask {
    pub id: TaskId,
    pub label: String,
    pub status: crate::TaskStatus,
    #[serde(default)]
    pub session: Option<SessionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOp {
    Confirm { station: StationId },
    Release { station: StationId },
    Force { station: StationId, reason: String },
    Rework { to: StationId },
    Retry { station: StationId },
    Pause,
    Resume,
    Cancel,
}
