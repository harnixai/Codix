//! 三个标签共用的状态符号与颜色（交互 §8）。会话、任务、站、运行、作业各有自己的状态枚举，
//! 但画出来只有这几种：同一个符号、同一种颜色在哪个标签里都是同一个意思。

use port::{JobOutcome, RunStatus, SessionState, StationStatus, TaskKind, TaskStatus, TodoStatus};
use tui::Tone;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// 正在做。
    Active,
    /// 等人处理。
    Waiting,
    Done,
    Failed,
    /// 空闲 / 还没轮到。
    Idle,
    /// 暂停、停用。
    Paused,
    Stopped,
    Skipped,
    /// 要重做。
    Stale,
}

impl Mark {
    pub fn glyph(self) -> &'static str {
        match self {
            Mark::Active => "●",
            Mark::Waiting => "◐",
            Mark::Done => "✓",
            Mark::Failed => "✗",
            Mark::Idle => "○",
            Mark::Paused => "⏸",
            Mark::Stopped => "■",
            Mark::Skipped => "⊘",
            Mark::Stale => "↺",
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            Mark::Active => Tone::Accent,
            Mark::Waiting => Tone::Attention,
            Mark::Done => Tone::Ok,
            Mark::Failed => Tone::Error,
            Mark::Stale => Tone::Warn,
            Mark::Idle | Mark::Paused | Mark::Stopped | Mark::Skipped => Tone::Dim,
        }
    }

    /// 状态文字的颜色：只有要人注意的才上色，其余是淡色。
    pub fn text_tone(self) -> Tone {
        match self {
            Mark::Waiting | Mark::Failed | Mark::Stale => self.tone(),
            _ => Tone::Dim,
        }
    }

    /// 名字的颜色：流程的节点按进展上色（做完的正常、正在做的主色、没轮到的淡色）。
    pub fn name_tone(self) -> Tone {
        match self {
            Mark::Active => Tone::Accent,
            Mark::Waiting => Tone::Attention,
            Mark::Failed => Tone::Error,
            Mark::Idle | Mark::Skipped => Tone::Dim,
            _ => Tone::Normal,
        }
    }

    pub fn pair(self) -> (&'static str, Tone) {
        (self.glyph(), self.tone())
    }
}

pub(crate) fn session(s: SessionState) -> Mark {
    match s {
        SessionState::Running => Mark::Active,
        SessionState::Waiting => Mark::Waiting,
        SessionState::Idle => Mark::Idle,
    }
}

pub(crate) fn task(t: TaskStatus) -> Mark {
    match t {
        TaskStatus::Running => Mark::Active,
        TaskStatus::Waiting => Mark::Waiting,
        TaskStatus::Done => Mark::Done,
        TaskStatus::Failed => Mark::Failed,
        TaskStatus::Stopped => Mark::Stopped,
    }
}

pub(crate) fn station(s: StationStatus) -> Mark {
    match s {
        StationStatus::Pending => Mark::Idle,
        StationStatus::Held => Mark::Waiting,
        StationStatus::Working => Mark::Active,
        StationStatus::Unmet => Mark::Failed,
        StationStatus::Passed => Mark::Done,
        StationStatus::Skipped => Mark::Skipped,
        StationStatus::Stale => Mark::Stale,
    }
}

pub(crate) fn run(s: &RunStatus) -> Mark {
    match s {
        RunStatus::Running => Mark::Active,
        RunStatus::Waiting { .. } => Mark::Waiting,
        RunStatus::Pausing | RunStatus::Paused => Mark::Paused,
        RunStatus::Finished => Mark::Done,
        RunStatus::Cancelled => Mark::Stopped,
    }
}

pub(crate) fn outcome(o: &JobOutcome) -> Mark {
    match o {
        JobOutcome::Running => Mark::Active,
        JobOutcome::Done => Mark::Done,
        JobOutcome::Failed { .. } => Mark::Failed,
        JobOutcome::Skipped => Mark::Skipped,
    }
}

pub(crate) fn todo(t: TodoStatus) -> (&'static str, Tone) {
    match t {
        TodoStatus::Done => ("☒", Tone::Dim),
        TodoStatus::InProgress => ("◼", Tone::Accent),
        TodoStatus::Pending => ("☐", Tone::Normal),
    }
}

/// 拆分树里区分子 Agent 与任务的小图标：子 Agent 有会话，选中就切过去；任务只有输出。
pub(crate) fn kind_icon(k: &TaskKind) -> (&'static str, Tone) {
    match k {
        TaskKind::Agent | TaskKind::Teammate => ("◆", Tone::Accent),
        TaskKind::Shell => ("$", Tone::Code),
        TaskKind::Monitor => ("◉", Tone::Code),
        TaskKind::Code => ("λ", Tone::Code),
        TaskKind::Mcp => ("⚙", Tone::Code),
        TaskKind::Cron => ("⏰", Tone::Code),
        TaskKind::Unknown(_) => ("·", Tone::Code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_meaning_same_look_across_tabs() {
        assert_eq!(session(SessionState::Running), task(TaskStatus::Running));
        assert_eq!(task(TaskStatus::Running), station(StationStatus::Working));
        assert_eq!(station(StationStatus::Working), run(&RunStatus::Running));
        assert_eq!(run(&RunStatus::Running), outcome(&JobOutcome::Running));
        assert_eq!(task(TaskStatus::Done), station(StationStatus::Passed));
        assert_eq!(station(StationStatus::Passed), outcome(&JobOutcome::Done));
        assert_eq!(session(SessionState::Waiting), station(StationStatus::Held));
    }
}
