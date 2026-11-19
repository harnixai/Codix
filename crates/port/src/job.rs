use serde::{Deserialize, Serialize};

use crate::{JobId, Millis, RunId, SessionId, Usage};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub name: String,
    pub cron: String,
    pub kind: JobKind,
    pub enabled: bool,
    /// 实例没在跑时错过的触发要不要补跑。
    #[serde(default)]
    pub catch_up: bool,
    /// 创建时刻：列表按它排（ID 是随机的）。
    #[serde(default)]
    pub created_ms: Millis,
    /// 这个作业自己写的护栏（随定义提交）；没写的项用全局缺省，都不超过全局上限（architecture §8.5）。
    #[serde(default, skip_serializing_if = "JobGuard::is_empty")]
    pub guard: JobGuard,
    /// 这台机器上因为连续失败被自动停用了。
    #[serde(default)]
    pub auto_disabled: Option<AutoDisabled>,
    /// 在这台机器上实际生效的护栏（派生：作业写的与全局缺省、天花板合起来）。
    #[serde(default)]
    pub effective_guard: Option<JobGuard>,
    #[serde(default)]
    pub next_fire_ms: Option<Millis>,
    #[serde(default)]
    pub last: Option<JobOutcome>,
    /// 正在跑的那一次触发的时刻（派生）。
    #[serde(default)]
    pub running_ms: Option<Millis>,
}

/// 护栏的各项；`None` 是"用全局缺省"。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobGuard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<Millis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap: Option<Overlap>,
    /// 连续失败几次自动停用；0 是不停。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_after: Option<u32>,
}

impl JobGuard {
    pub fn is_empty(&self) -> bool {
        *self == JobGuard::default()
    }
}

/// 到点时上一次还没跑完怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overlap {
    /// 这一次跳过。
    Skip,
    /// 至多排一个，上一次结束再跑。
    Queue,
    /// 停下上一次，跑这一次。
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoDisabled {
    pub at_ms: Millis,
    /// 连着失败了几次。
    pub after: u32,
}

/// `ValidateJob` 的回答：不落盘。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobCheck {
    /// 接下来几次触发的时刻。
    pub next_fire_ms: Vec<Millis>,
    /// 实际生效的护栏：每项都有值，已按全局上限压过。
    pub guard: JobGuard,
    /// 被全局上限压过的项（`timeout`、`max_tokens`）。
    #[serde(default)]
    pub capped: Vec<String>,
    /// 存不进去的原因；空就是可以存。
    #[serde(default)]
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Simple {
        prompt: String,
        #[serde(default)]
        skills: Vec<String>,
        #[serde(default)]
        project: Option<String>,
        #[serde(default)]
        scene: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },
    Flow {
        flow: String,
        subject: String,
        #[serde(default)]
        inputs: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobEntry {
    pub fired_ms: Millis,
    pub outcome: JobOutcome,
    #[serde(default)]
    pub elapsed_ms: Millis,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub target: Option<JobTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    Running,
    Done,
    Failed {
        message: String,
    },
    /// 被停下：护栏、手动、实例停了。
    Stopped {
        reason: StopCause,
    },
    Skipped {
        reason: SkipCause,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopCause {
    Timeout,
    Budget,
    DailyBudget,
    Manual,
    InstanceStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipCause {
    /// 上一次还在跑。
    Overlap,
    DailyBudget,
    /// 同时在跑的触发满了，排了一个周期也没轮到。
    Busy,
}

/// `JobHistory` 按结局筛。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Running,
    Done,
    Failed,
    Stopped,
    Skipped,
}

impl JobOutcome {
    pub fn kind(&self) -> OutcomeKind {
        match self {
            JobOutcome::Running => OutcomeKind::Running,
            JobOutcome::Done => OutcomeKind::Done,
            JobOutcome::Failed { .. } => OutcomeKind::Failed,
            JobOutcome::Stopped { .. } => OutcomeKind::Stopped,
            JobOutcome::Skipped { .. } => OutcomeKind::Skipped,
        }
    }

    /// 算不算"这一次没成"：连续失败自动停用按它数。
    pub fn is_failure(&self) -> bool {
        matches!(self, JobOutcome::Failed { .. } | JobOutcome::Stopped { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobTarget {
    Session { id: SessionId, title: String },
    Run { id: RunId, subject: String },
}
