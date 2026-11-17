//! 作业编辑会话的宿主工具（architecture §8.5）：模型经它们读写作业，界面不另开表单。
//! 工具的说明与参数是给模型看的，不是界面文字，所以用英文写、不进语言资源。

use port::{Job, JobId, JobKind};
use serde_json::{Value, json};

pub const LIST: &str = "JobList";
pub const SAVE: &str = "JobSave";
pub const DELETE: &str = "JobDelete";

pub fn declarations() -> Value {
    json!([
        {
            "name": LIST,
            "description": "List the scheduled jobs of this Codix instance (id, name, cron, enabled, prompt, next fire time, last outcome).",
            "input_schema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": SAVE,
            "description": "Create or update a scheduled job. A job fires on a 5-field cron schedule in the local timezone (minute hour day-of-month month day-of-week); each firing starts a new unattended session in this project and submits `prompt` as its first turn. Omit `id` to create a job; pass an existing id to replace it. Confirm the schedule with the user before saving.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Existing job id to update; omit to create."},
                    "name": {"type": "string"},
                    "cron": {"type": "string", "description": "5-field cron, local time, e.g. \"0 9 * * 1-5\"."},
                    "prompt": {"type": "string", "description": "What the session should do each time."},
                    "skills": {"type": "array", "items": {"type": "string"}, "description": "Skills the session should use."},
                    "enabled": {"type": "boolean", "default": true},
                    "catch_up": {"type": "boolean", "default": false, "description": "Run once at startup if a firing was missed while the instance was down."},
                    "model": {"type": "string"}
                },
                "required": ["name", "cron", "prompt"],
                "additionalProperties": false
            }
        },
        {
            "name": DELETE,
            "description": "Delete a scheduled job and its firing history.",
            "input_schema": {
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
                "additionalProperties": false
            }
        }
    ])
}

/// 写进编辑会话系统提示词的一段：它在编辑哪个作业。
pub fn prompt(job: Option<&Job>) -> String {
    let mut s = String::from(
        "You are editing the scheduled jobs of this Codix instance. Use the JobList, JobSave and JobDelete tools to inspect and change them; never claim a change you did not make with a tool. Reply in the user's language.",
    );
    if let Some(j) = job {
        s.push_str("\n\nThe job being edited:\n");
        s.push_str(&serde_json::to_string_pretty(j).unwrap_or_default());
    }
    s
}

/// `JobSave` 的参数 → 作业；未给的字段沿用已有作业的。
pub fn job_from_input(input: &Value, existing: Option<Job>) -> Result<Job, String> {
    let str_of = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
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
    if let Some(id) = str_of("id") {
        job.id = JobId(id);
    }
    if let Some(n) = str_of("name") {
        job.name = n;
    }
    if let Some(c) = str_of("cron") {
        job.cron = c;
    }
    if let Some(e) = input.get("enabled").and_then(Value::as_bool) {
        job.enabled = e;
    }
    if let Some(c) = input.get("catch_up").and_then(Value::as_bool) {
        job.catch_up = c;
    }
    let (old_prompt, old_skills, old_model) = match &job.kind {
        JobKind::Simple {
            prompt,
            skills,
            model,
            ..
        } => (prompt.clone(), skills.clone(), model.clone()),
        JobKind::Flow { .. } => (String::new(), vec![], None),
    };
    job.kind = JobKind::Simple {
        prompt: str_of("prompt").unwrap_or(old_prompt),
        skills: input
            .get("skills")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or(old_skills),
        project: None,
        scene: None,
        model: str_of("model").or(old_model),
    };
    if job.name.trim().is_empty() {
        return Err("name is required".into());
    }
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_input_fills_a_simple_job_and_keeps_unspecified_fields() {
        let j = job_from_input(
            &json!({"name":"nightly","cron":"0 2 * * *","prompt":"run tests","skills":["ci"]}),
            None,
        )
        .unwrap();
        assert!(j.enabled && j.id.0.is_empty());
        let j2 = job_from_input(&json!({"id":"j1","enabled":false}), Some(j)).unwrap();
        assert_eq!((j2.id.0.as_str(), j2.enabled), ("j1", false));
        let JobKind::Simple { prompt, skills, .. } = &j2.kind else {
            panic!()
        };
        assert_eq!((prompt.as_str(), skills.len()), ("run tests", 1));
        assert!(job_from_input(&json!({"cron":"x"}), None).is_err());
    }
}
