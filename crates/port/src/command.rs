use serde::{Deserialize, Serialize};

use crate::{
    Answer, ClientInfo, FlowDetail, FlowSummary, Goal, HistoryItem, InstanceInfo, Job, JobEntry,
    JobId, MessageId, Prompt, PromptId, RunId, RunOp, RunSummary, RunView, SessionId,
    SessionSummary, Task, TaskId, Todo,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    // 连接
    Hello {
        client: ClientInfo,
    },
    Heartbeat {
        last_input_ms: u64,
    },
    Detach,
    StopInstance,

    // 会话
    ListSessions {
        filter: SessionFilter,
    },
    CreateSession {
        project: Option<String>,
        scene: Option<String>,
        model: Option<String>,
    },
    /// 订阅一个会话的细节事件，回 `Watched` 快照；会话不在内存时先恢复。
    Watch {
        session: SessionId,
    },
    Unwatch {
        session: SessionId,
    },
    Rename {
        session: SessionId,
        title: String,
    },
    Close {
        session: SessionId,
    },

    // 对话
    Submit {
        session: SessionId,
        text: String,
        mode: SubmitMode,
        /// Shift+Tab 选的权限模式，逐轮生效（需求 C-20）；`None` 用会话自己的。
        #[serde(default)]
        permission: Option<PermissionMode>,
    },
    Interrupt {
        session: SessionId,
    },
    Answer {
        session: SessionId,
        prompt: PromptId,
        answer: Answer,
    },
    SetModel {
        session: SessionId,
        model: String,
    },
    Commands {
        session: SessionId,
    },
    SideQuestion {
        session: SessionId,
        text: String,
    },

    // goal
    SetGoal {
        session: SessionId,
        objective: String,
        max_rounds: Option<u32>,
    },
    UpdateGoal {
        session: SessionId,
        action: GoalAction,
    },

    // 任务
    StopTask {
        task: TaskId,
    },
    MessageTask {
        task: TaskId,
        text: String,
    },
    TaskOutput {
        task: TaskId,
        from_byte: u64,
    },

    // 流程
    ListFlows,
    GetFlow {
        name: String,
    },
    ListRuns {
        flow: String,
    },
    GetRun {
        run: RunId,
    },
    StartRun {
        flow: String,
        subject: String,
        inputs: serde_json::Value,
    },
    RunOp {
        run: RunId,
        op: RunOp,
    },
    /// 打开流程的编写会话；`flow` 为空表示新建一个流程。
    OpenAuthoring {
        flow: Option<String>,
    },

    // 作业
    /// 打开作业的编辑会话：在对话里看详情、修改、清理（阶段 3 由引擎侧工具执行）。
    OpenJobSession {
        job: Option<JobId>,
    },
    ListJobs,
    SaveJob {
        job: Job,
    },
    DeleteJob {
        job: JobId,
    },
    SetJobEnabled {
        job: JobId,
        enabled: bool,
    },
    RunJobNow {
        job: JobId,
    },
    JobHistory {
        job: JobId,
    },
}

impl Command {
    /// 用于 mock 规则匹配与事件日志的命令名。
    pub fn name(&self) -> &'static str {
        match self {
            Command::Hello { .. } => "hello",
            Command::Heartbeat { .. } => "heartbeat",
            Command::Detach => "detach",
            Command::StopInstance => "stop_instance",
            Command::ListSessions { .. } => "list_sessions",
            Command::CreateSession { .. } => "create_session",
            Command::Watch { .. } => "watch",
            Command::Unwatch { .. } => "unwatch",
            Command::Rename { .. } => "rename",
            Command::Close { .. } => "close",
            Command::Submit { .. } => "submit",
            Command::Interrupt { .. } => "interrupt",
            Command::Answer { .. } => "answer",
            Command::SetModel { .. } => "set_model",
            Command::Commands { .. } => "commands",
            Command::SideQuestion { .. } => "side_question",
            Command::SetGoal { .. } => "set_goal",
            Command::UpdateGoal { .. } => "update_goal",
            Command::StopTask { .. } => "stop_task",
            Command::MessageTask { .. } => "message_task",
            Command::TaskOutput { .. } => "task_output",
            Command::ListFlows => "list_flows",
            Command::GetFlow { .. } => "get_flow",
            Command::ListRuns { .. } => "list_runs",
            Command::GetRun { .. } => "get_run",
            Command::StartRun { .. } => "start_run",
            Command::RunOp { .. } => "run_op",
            Command::OpenAuthoring { .. } => "open_authoring",
            Command::OpenJobSession { .. } => "open_job_session",
            Command::ListJobs => "list_jobs",
            Command::SaveJob { .. } => "save_job",
            Command::DeleteJob { .. } => "delete_job",
            Command::SetJobEnabled { .. } => "set_job_enabled",
            Command::RunJobNow { .. } => "run_job_now",
            Command::JobHistory { .. } => "job_history",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFilter {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub include_closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitMode {
    Turn,
    Steer,
    Queue,
}

/// 与 Agix 的 `permission_mode` 取值一致（协议 §4.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalAction {
    Pause,
    Resume,
    Edit {
        objective: String,
        max_rounds: Option<u32>,
    },
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `builtin` / `user` / `project` / `plugin` / `mcp`……
    #[serde(default)]
    pub source: String,
}

/// `Watch` 的快照：之后的增量都接在它后面。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watched {
    pub session: SessionSummary,
    #[serde(default)]
    pub history: Vec<HistoryItem>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub goal: Option<Goal>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub prompts: Vec<Prompt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Hello {
        instance: InstanceInfo,
        flows: bool,
        jobs: bool,
    },
    Sessions(Vec<SessionSummary>),
    Session(SessionSummary),
    Watched(Box<Watched>),
    Submitted {
        message: MessageId,
    },
    Commands(Vec<SlashCommand>),
    TaskOutput {
        text: String,
        next_byte: u64,
        finished: bool,
    },
    Flows(Vec<FlowSummary>),
    Flow(Box<FlowDetail>),
    Runs(Vec<RunSummary>),
    Run(Box<RunView>),
    RunStarted(RunSummary),
    Jobs(Vec<Job>),
    Job(Job),
    JobHistory(Vec<JobEntry>),
}
