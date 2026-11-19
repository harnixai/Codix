//! 全局目录与项目里的布局、发现文件（architecture §8.1）。实例进程写，TUI 读，两边共用这一份定义。

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
    /// 全局流程包（可写层），所有实例共用。
    pub fn flows(&self) -> PathBuf {
        self.home.join("flows")
    }

    pub fn meta(&self, name: &str) -> PathBuf {
        self.instance(name).join("meta.json")
    }

    /// 认领过的项目：凭证 → 项目路径（architecture §8.9）。
    pub fn known_projects(&self) -> PathBuf {
        self.home.join("known-projects.json")
    }

    pub fn known_projects_lock(&self) -> PathBuf {
        self.home.join("known-projects.lock")
    }
}

/// 项目层目录，与引擎的同一个（`EmbedConfig.project_dir`）。
pub const PROJECT_DIR: &str = ".codix";

/// 一个项目里 Codix 的东西：定义在项目层目录下随项目提交，本机状态在 `state/codix/`——Agix 给嵌入方的
/// `state/<产品名小写>/`（`Layout::host_state_dir`），引擎不写它、按配置目录保护它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectState {
    project: PathBuf,
}

impl ProjectState {
    pub fn new(project: impl Into<PathBuf>) -> ProjectState {
        ProjectState {
            project: project.into(),
        }
    }

    pub fn project(&self) -> &Path {
        &self.project
    }

    pub fn data_dir(&self) -> PathBuf {
        self.project.join(PROJECT_DIR)
    }

    /// `state/codix/`。
    pub fn dir(&self) -> PathBuf {
        self.data_dir().join("state").join("codix")
    }

    pub fn owner(&self) -> PathBuf {
        self.dir().join("owner.json")
    }

    pub fn lock(&self) -> PathBuf {
        self.dir().join("host.lock")
    }

    pub fn quarantine(&self) -> PathBuf {
        self.dir().join("quarantine")
    }

    pub fn sessions(&self) -> PathBuf {
        self.dir().join("sessions.json")
    }

    /// 作业定义：随项目提交。
    pub fn jobs(&self) -> PathBuf {
        self.data_dir().join("jobs.json")
    }

    /// 作业在这台机器上的启用状态与调度基线。
    pub fn jobs_state(&self) -> PathBuf {
        self.dir().join("jobs-state.json")
    }

    pub fn job_history_dir(&self) -> PathBuf {
        self.dir().join("jobs")
    }

    pub fn job_history(&self, job: &str) -> PathBuf {
        self.job_history_dir().join(format!("{job}.jsonl"))
    }

    pub fn flow_runs(&self) -> PathBuf {
        self.dir().join("flow-runs")
    }

    pub fn flow_workspaces(&self) -> PathBuf {
        self.dir().join("workspaces")
    }
}

/// 先写同目录的临时文件再改名：读的一方（另一个进程的 `list`）不会读到写了一半的 JSON。
pub fn write_json_atomic(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)
}

/// 文件不在或解析不了都当没有：发现文件随时可能被实例进程删掉。
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
