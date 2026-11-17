//! 实例给每个会话的宿主工具（architecture §8.5）：会话与作业的查看、修改。经
//! `daemon.setHostTools` 挂在连接上，所以每个会话都有；能动什么由调用它的会话决定
//! （[`Scope`]），实例执行前查，不靠提示词。
//! 工具的说明与参数是给模型看的，不是界面文字，所以用英文写、不进语言资源。

use port::{Job, JobId, JobKind, Origin};
use serde_json::{Value, json};

pub const SESSION_LIST: &str = "session_list";
pub const SESSION_SHOW: &str = "session_show";
pub const SESSION_RENAME: &str = "session_rename";
pub const SESSION_CLOSE: &str = "session_close";
pub const JOB_LIST: &str = "job_list";
pub const JOB_SHOW: &str = "job_show";
pub const JOB_SAVE: &str = "job_save";
pub const JOB_DELETE: &str = "job_delete";
pub const JOB_ENABLE: &str = "job_enable";
pub const JOB_RUN: &str = "job_run";

/// 只读的几个：建会话时给它们 allow 规则，查看不用每次问人。
pub const READ_ONLY: [&str; 4] = [SESSION_LIST, SESSION_SHOW, JOB_LIST, JOB_SHOW];

/// 一次调用能动什么，由发起调用的会话（子 Agent 的调用算它的根会话）决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// 用户会话（人新建的、分叉的）：一切。
    Full,
    /// 作业触发出来的会话：只动触发它的那个作业，会话只动自己。
    Job(String),
    /// 流程运行里的会话：作业一个都不动，会话只动自己。
    Run(String),
    /// 不是这个实例管的会话：什么都不给。
    None,
}

pub fn scope_of(origin: Option<&Origin>) -> Scope {
    match origin {
        Some(Origin::User | Origin::Fork { .. }) => Scope::Full,
        Some(Origin::Job { job, .. }) => Scope::Job(job.0.clone()),
        Some(Origin::FlowStation { run, .. }) => Scope::Run(run.0.clone()),
        // 侧链会话不会是发起者（子 Agent 的调用用根会话回答）；真碰上了按最窄的算。
        Some(Origin::SubAgent { .. }) | None => Scope::None,
    }
}

const TARGET: &str = "\"#3\" (the number shown in the list right now) or the id. Numbers follow the current order and can shift when the list changes; ids never do.";

fn target(desc: &str) -> Value {
    json!({"type": "string", "description": format!("{desc}: {TARGET}")})
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let read_only = READ_ONLY.contains(&name);
    json!({
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        },
        "read_only": read_only,
        "destructive": matches!(name, SESSION_CLOSE | JOB_DELETE),
        "subagents": true
    })
}

pub fn declarations() -> Value {
    let job_fields = json!({
        "name": {"type": "string"},
        "cron": {"type": "string", "description": "5-field cron in the local timezone (minute hour day-of-month month day-of-week), e.g. \"0 9 * * 1-5\"."},
        "prompt": {"type": "string", "description": "What the session started by each firing should do."},
        "skills": {"type": "array", "items": {"type": "string"}, "description": "Skills that session should use."},
        "enabled": {"type": "boolean"},
        "catch_up": {"type": "boolean", "description": "Run once at startup if a firing was missed while the instance was down."},
        "model": {"type": "string"}
    });
    let mut save_props = job_fields.as_object().cloned().unwrap_or_default();
    save_props.insert(
        "target".into(),
        target("The job to change; omit to create a new one"),
    );
    json!([
        tool(
            SESSION_LIST,
            "List the sessions of this Codix instance's project: the user's sessions, numbered as in the sessions list (#1 is the newest). With all=true, also the sessions started by scheduled jobs and flow runs (not numbered).",
            json!({"all": {"type": "boolean", "default": false}}),
            &[]
        ),
        tool(
            SESSION_SHOW,
            "Show one session: title, state, origin, model, usage, goal, todos and background tasks.",
            json!({"target": target("The session")}),
            &["target"]
        ),
        tool(
            SESSION_RENAME,
            "Rename a session; this is the name shown in the sessions list. Omit target to rename the session you are in.",
            json!({"target": target("The session; omit for this one"), "title": {"type": "string"}}),
            &["title"]
        ),
        tool(
            SESSION_CLOSE,
            "Close a session: it leaves the sessions list (its transcript is kept). Only when the user asks for it.",
            json!({"target": target("The session")}),
            &["target"]
        ),
        tool(
            JOB_LIST,
            "List the scheduled jobs of this Codix instance, numbered as in the jobs list: name, cron, enabled, prompt, next firing, last outcome.",
            json!({}),
            &[]
        ),
        tool(
            JOB_SHOW,
            "Show one scheduled job with its recent firings.",
            json!({"target": target("The job")}),
            &["target"]
        ),
        tool(
            JOB_SAVE,
            "Create or change a scheduled job. Each firing starts a new unattended session in this project and submits `prompt` as its first turn. Without target this creates a job (name, cron and prompt required); with target only the given fields change. Confirm the schedule with the user before saving.",
            Value::Object(save_props),
            &[]
        ),
        tool(
            JOB_DELETE,
            "Delete a scheduled job and its firing history. Only when the user asks for it.",
            json!({"target": target("The job")}),
            &["target"]
        ),
        tool(
            JOB_ENABLE,
            "Enable or disable a scheduled job.",
            json!({"target": target("The job"), "enabled": {"type": "boolean"}}),
            &["target", "enabled"]
        ),
        tool(
            JOB_RUN,
            "Fire a scheduled job once now, outside its schedule.",
            json!({"target": target("The job")}),
            &["target"]
        ),
    ])
}

/// 建会话时带上的 allow 规则：只读工具不用每次问人。
pub fn read_only_rules() -> Value {
    Value::Array(
        READ_ONLY
            .iter()
            .map(|t| json!({"source": "session", "behavior": "allow", "tool_name": t}))
            .collect(),
    )
}

pub fn str_of(input: &Value, k: &str) -> Option<String> {
    input.get(k).and_then(Value::as_str).map(str::to_string)
}

/// `job_save` 的参数 → 作业；未给的字段沿用已有作业的。
pub fn job_from_input(input: &Value, existing: Option<Job>) -> Result<Job, String> {
    let creating = existing.is_none();
    let mut job = existing.unwrap_or(Job {
        id: JobId(String::new()),
        name: String::new(),
        cron: String::new(),
        kind: JobKind::Simple {
            prompt: String::new(),
            skills: vec![],
            project: None,
            scene: None,
            model: None,
        },
        enabled: true,
        catch_up: false,
        next_fire_ms: None,
        last: None,
    });
    if creating {
        for k in ["name", "cron", "prompt"] {
            if str_of(input, k).is_none_or(|v| v.trim().is_empty()) {
                return Err(format!("{k} is required to create a job"));
            }
        }
    }
    if let Some(n) = str_of(input, "name") {
        job.name = n;
    }
    if let Some(c) = str_of(input, "cron") {
        job.cron = c;
    }
    if let Some(e) = input.get("enabled").and_then(Value::as_bool) {
        job.enabled = e;
    }
    if let Some(c) = input.get("catch_up").and_then(Value::as_bool) {
        job.catch_up = c;
    }
    // 流程作业的编辑这里不管（流程还没接入）：没给提示词就原样留着它的种类。
    if matches!(job.kind, JobKind::Flow { .. }) && str_of(input, "prompt").is_none() {
        return Ok(job);
    }
    let (old_prompt, old_skills, old_project, old_scene, old_model) = match &job.kind {
        JobKind::Simple {
            prompt,
            skills,
            project,
            scene,
            model,
        } => (
            prompt.clone(),
            skills.clone(),
            project.clone(),
            scene.clone(),
            model.clone(),
        ),
        JobKind::Flow { .. } => (String::new(), vec![], None, None, None),
    };
    job.kind = JobKind::Simple {
        prompt: str_of(input, "prompt").unwrap_or(old_prompt),
        skills: input
            .get("skills")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or(old_skills),
        project: old_project,
        scene: old_scene,
        model: str_of(input, "model").or(old_model),
    };
    if job.name.trim().is_empty() {
        return Err("name must not be empty".into());
    }
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_input_creates_with_the_required_fields_and_keeps_the_rest_on_change() {
        assert!(job_from_input(&json!({"name":"x","cron":"0 2 * * *"}), None).is_err());
        let j = job_from_input(
            &json!({"name":"nightly","cron":"0 2 * * *","prompt":"run tests","skills":["ci"]}),
            None,
        )
        .unwrap();
        assert!(j.enabled && j.id.0.is_empty());
        let j2 = job_from_input(&json!({"enabled":false}), Some(j)).unwrap();
        assert!(!j2.enabled);
        let JobKind::Simple { prompt, skills, .. } = &j2.kind else {
            panic!()
        };
        assert_eq!((prompt.as_str(), skills.len()), ("run tests", 1));
    }

    #[test]
    fn every_tool_is_declared_once_and_reaches_sub_agents() {
        let d = declarations();
        let names: Vec<&str> = d
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        let mut uniq = names.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(names.len(), 10);
        assert_eq!(uniq.len(), names.len());
        assert!(d.as_array().unwrap().iter().all(|t| t["subagents"] == true));
        for r in READ_ONLY {
            assert!(names.contains(&r));
        }
    }

    #[test]
    fn scope_follows_the_calling_session_origin() {
        assert_eq!(scope_of(Some(&Origin::User)), Scope::Full);
        assert_eq!(
            scope_of(Some(&Origin::Job {
                job: JobId("j1".into()),
                name: "n".into(),
                fired_ms: 0
            })),
            Scope::Job("j1".into())
        );
        assert_eq!(scope_of(None), Scope::None);
    }
}
