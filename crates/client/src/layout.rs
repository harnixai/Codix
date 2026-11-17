//! 实例目录的布局与发现文件（architecture §8.1）。实例进程写，TUI 读，两边共用这一份定义。

use std::path::{Path, PathBuf};

use port::ClientInfo;
use serde::{Deserialize, Serialize};

/// `~/.codix`（或 `CODIX_HOME`）下的路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    home: PathBuf,
}

impl Layout {
    pub fn new(home: impl Into<PathBuf>) -> Layout {
        Layout { home: home.into() }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn instances(&self) -> PathBuf {
        self.home.join("instances")
    }

    pub fn instance(&self, name: &str) -> PathBuf {
        self.instances().join(name)
    }

    pub fn discovery(&self, name: &str) -> PathBuf {
        self.instance(name).join("instance.json")
    }

    pub fn lock(&self, name: &str) -> PathBuf {
        self.instance(name).join("instance.lock")
    }

    pub fn socket(&self, name: &str) -> PathBuf {
        self.instance(name).join("host.sock")
    }

    pub fn groups(&self, name: &str) -> PathBuf {
        self.instance(name).join("groups.json")
    }

    pub fn sessions(&self, name: &str) -> PathBuf {
        self.instance(name).join("sessions.json")
    }

    pub fn jobs(&self, name: &str) -> PathBuf {
        self.instance(name).join("jobs.json")
    }

    pub fn job_history(&self, name: &str, job: &str) -> PathBuf {
        self.instance(name)
            .join("jobs")
            .join(format!("{job}.jsonl"))
    }

    pub fn log(&self, name: &str) -> PathBuf {
        self.instance(name).join("host.log")
    }
}

/// `instance.json`：实例进程起来、socket 能连之后写，接入者变了就重写，退出时删掉。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceFile {
    pub name: String,
    pub pid: u32,
    pub host: String,
    #[serde(default)]
    pub project: Option<String>,
    pub started_ms: u64,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub holder: Option<ClientInfo>,
}

/// 实例自己的设定（`instance.json` 删掉之后也留着），`codix start` 照它重新拉起。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceMeta {
    pub name: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub stopped_ms: Option<u64>,
}

impl Layout {
    pub fn meta(&self, name: &str) -> PathBuf {
        self.instance(name).join("meta.json")
    }
}
