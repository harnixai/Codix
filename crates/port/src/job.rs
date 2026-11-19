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
    #[serde(default)]
    pub next_fire_ms: Option<Millis>,
    #[serde(default)]
    pub last: Option<JobOutcome>,
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
    Failed { message: String },
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobTarget {
    Session { id: SessionId, title: String },
    Run { id: RunId, subject: String },
}
