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
    /// 今天这个作业用了多少 token（派生，本机的账）。
    #[serde(default)]
    pub today_tokens: u64,
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
    /// 无人值守的这一次做了什么（简单作业）：调了哪些工具、出错几次、改了哪些文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<FiringAudit>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiringAudit {
    /// 工具名 → 调了几次。
    #[serde(default)]
    pub tools: std::collections::BTreeMap<String, u32>,
    /// 出错的工具调用（含被拒的权限：无人值守的会话里询问直接按拒绝）。
    #[serde(default)]
    pub errors: u32,
    /// 改过的文件（文件工具报的）。
    #[serde(default)]
    pub files: Vec<String>,
}

impl FiringAudit {
    pub fn calls(&self) -> u32 {
        self.tools.values().sum()
    }
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
    /// 到点时上一次还在跑、重叠策略是 `replace`：被新的一次换掉。
    Replaced,
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

    /// 算不算这个作业自己没成：连续失败自动停用按它数。人停的、额度用完、实例停了、被替换，都不是它的错。
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            JobOutcome::Failed { .. }
                | JobOutcome::Stopped {
                    reason: StopCause::Timeout | StopCause::Budget
                }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobTarget {
    Session { id: SessionId, title: String },
    Run { id: RunId, subject: String },
}
