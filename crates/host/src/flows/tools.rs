//! `flow_*` 宿主工具（architecture §8.5）：在对话里编写流程、操作运行。名字与参数照 Harnix 的
//! `flow-author` 场景，但每个作用于流程或运行的工具都要显式给目标——Codix 没有"当前流程"，工具在
//! 每个会话里都有。运行里的会话自己的三个工具（`flow_state`、`flow_submit`、`flow_propose`）是
//! 派工器的，原样转给它。
//! 说明与参数是给模型看的，用英文写、不进语言资源。

use harnix_flow::dryrun::{Sandbox, SandboxConfig, StartIn};
use harnix_flow::{Actor, RunStatus};
use port::{RunOp, StationId};
use serde_json::{Value, json};

use super::{Flows, flow_err};
use crate::hub::tools::{FLOW_READ_ONLY, Scope};

const TARGET: &str = "The flow: \"#3\" (its number in the flow list right now) or its name.";
const RUN: &str = "The run's id (from flow_runs).";

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        },
        "read_only": FLOW_READ_ONLY.contains(&name),
        "destructive": matches!(name, "flow_delete" | "flow_cancel" | "flow_force"),
        "subagents": true
    })
}

pub(crate) fn declarations() -> Vec<Value> {
    let target = json!({"type": "string", "description": TARGET});
    let run = json!({"type": "string", "description": RUN});
    let station = json!({"type": "string", "description": "The station's id in the flow."});
    vec![
        tool(
            "flow_list",
            "List the flows on offer in this project, numbered as in the flows list: name, title, origin (builtin, global, project, override), whether it is valid, runs not finished.",
            json!({}),
            &[],
        ),
        tool(
            "flow_show",
            "Show one flow: its YAML, its stations, its problems if any, and its runs not finished.",
            json!({"target": target}),
            &["target"],
        ),
        tool(
            "flow_schema",
            "The JSON Schema of a flow definition, and the names this instance knows (scenes, tools, gates…).",
            json!({}),
            &[],
        ),
        tool(
            "flow_guide",
            "The guide to writing flows: stations, executors, exit conditions, rules, policies and permissions, gates, hooks. Read it before writing or changing a flow.",
            json!({}),
            &[],
        ),
        tool(
            "flow_validate",
            "Check a flow definition's YAML without saving it: its problems, or that it is valid.",
            json!({"yaml": {"type": "string"}}),
            &["yaml"],
        ),
        tool(
            "flow_create",
            "Save a new flow from its YAML (a definition with problems is not saved; the problems are the answer). The name must not be taken.",
            json!({"yaml": {"type": "string"}}),
            &["yaml"],
        ),
        tool(
            "flow_update",
            "Replace a flow's definition with new YAML (same name). Refused while the flow has runs not finished: cancel them or let them finish first.",
            json!({"target": target, "yaml": {"type": "string"}}),
            &["target", "yaml"],
        ),
        tool(
            "flow_delete",
            "Delete a flow (a built-in one: its saved override). Refused while the flow has runs not finished. Only when the user asks for it.",
            json!({"target": target}),
            &["target"],
        ),
        tool(
            "flow_dry_run",
            "Dry-run a flow: the real kernel in a sandbox, nothing outside touched, every task answered by its contract's example or the mocks given. Says how far it got.",
            json!({"target": target, "inputs": {"type": "object"}, "mocks": {"type": "object"}}),
            &["target"],
        ),
        tool(
            "flow_start",
            "Start a run of a flow for real, in this project.",
            json!({"target": target, "subject": {"type": "string", "description": "What this run is about, for people."}, "inputs": {"type": "object"}}),
            &["target", "subject"],
        ),
        tool(
            "flow_runs",
            "List runs, newest first: id, flow, subject, status. Omit target for every flow's.",
            json!({"target": target}),
            &[],
        ),
        tool(
            "flow_run",
            "Show one run: status, its stations and where it stands.",
            json!({"run": run}),
            &["run"],
        ),
        tool(
            "flow_pause",
            "Pause a run: work in hand finishes, nothing new starts.",
            json!({"run": run}),
            &["run"],
        ),
        tool(
            "flow_resume",
            "Resume a paused run.",
            json!({"run": run}),
            &["run"],
        ),
        tool(
            "flow_cancel",
            "Cancel a run. Only when the user asks for it.",
            json!({"run": run}),
            &["run"],
        ),
        tool(
            "flow_confirm",
            "Confirm (or reject, with pass false) a station waiting on a confirmation.",
            json!({"run": run, "station": station, "pass": {"type": "boolean", "default": true}, "reason": {"type": "string"}}),
            &["run", "station"],
        ),
        tool(
            "flow_force",
            "Let a station pass although its exit conditions did not. Only when the user asks for it, with the reason.",
            json!({"run": run, "station": station, "reason": {"type": "string"}}),
            &["run", "station", "reason"],
        ),
        tool(
            "flow_rework",
            "Send the run back to a station to do it again (and what came after it).",
            json!({"run": run, "to": station, "reason": {"type": "string"}}),
            &["run", "to"],
        ),
        tool(
            "flow_retry",
            "Retry a station's failed task.",
            json!({"run": run, "station": station}),
            &["run", "station"],
        ),
        tool(
            "flow_release",
            "Release a held station.",
            json!({"run": run, "station": station}),
            &["run", "station"],
        ),
    ]
}

fn str_of(input: &Value, k: &str) -> Option<String> {
    input.get(k).and_then(Value::as_str).map(str::to_string)
}

fn need(input: &Value, k: &str) -> Result<String, String> {
    str_of(input, k)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{k} is required"))
}

const RUN_OPS: [&str; 8] = [
    "flow_pause",
    "flow_resume",
    "flow_cancel",
    "flow_confirm",
    "flow_force",
    "flow_rework",
    "flow_retry",
    "flow_release",
];

/// 这个会话能不能用这个工具（目标是运行的，另看是不是它自己的运行）。
fn allowed(scope: &Scope, name: &str, input: &Value) -> Result<(), String> {
    if FLOW_READ_ONLY.contains(&name) {
        return Ok(());
    }
    match scope {
        Scope::Full => Ok(()),
        Scope::Run(mine) if RUN_OPS.contains(&name) => {
            if str_of(input, "run").as_deref() == Some(mine.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "not allowed: a session inside a flow run may operate only that run ({mine})"
                ))
            }
        }
        Scope::Run(mine) => Err(format!(
            "not allowed: a session inside a flow run ({mine}) may not change flow definitions or start runs"
        )),
        Scope::Job(job) => Err(format!(
            "not allowed: a session started by a scheduled job ({job}) may only look at flows"
        )),
        Scope::None => Err("not allowed from this session".into()),
    }
}

pub(crate) async fn call(
    f: &Flows,
    rpc: &str,
    scope: &Scope,
    name: &str,
    input: &Value,
) -> Option<Result<String, String>> {
    if f.runner.tools().specs().iter().any(|s| s.name == name) {
        let r = f
            .runner
            .tools()
            .call(&rpc.to_string(), name, input.clone())
            .await;
        return Some(if r.is_error { Err(r.text) } else { Ok(r.text) });
    }
    if !name.starts_with("flow_") || !declarations().iter().any(|d| d["name"] == name) {
        return None;
    }
    Some(match allowed(scope, name, input) {
        Ok(()) => run(f, rpc, name, input).await,
        Err(e) => Err(e),
    })
}

/// `#n` 或名字 → 流程的名字。
async fn flow_target(f: &Flows, input: &Value) -> Result<String, String> {
    let key = need(input, "target")?;
    let all = f.summaries().await;
    port::order::resolve(&port::order::flows(&all), &key, |s| &s.name)
        .map(|s| s.name.clone())
        .ok_or_else(|| format!("no flow {key}"))
}

/// 修改限制：这个流程还有没结束的运行时不许改、不许删。
async fn unlocked(f: &Flows, flow: &str) -> Result<(), String> {
    let active = f
        .runs(Some(flow), Some(RunStatus::Active))
        .await
        .map_err(|e| e.to_string())?;
    if active.is_empty() {
        return Ok(());
    }
    let ids: Vec<&str> = active.iter().map(|r| r.id.as_str()).collect();
    Err(format!(
        "refused: flow {flow} still has {} run(s) not finished ({}). Cancel them (flow_cancel) or let them finish first.",
        ids.len(),
        ids.join(", ")
    ))
}

fn by(rpc: &str) -> Actor {
    Actor::agent(rpc)
}

async fn run(f: &Flows, rpc: &str, name: &str, input: &Value) -> Result<String, String> {
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap_or_default();
    match name {
        "flow_list" => {
            let all = f.summaries().await;
            let rows = harnix_flow::wire::defs_list(f.packs().as_ref());
            let lines: Vec<String> = port::order::flows(&all)
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let valid = rows
                        .iter()
                        .find(|r| r.name == s.name)
                        .is_some_and(|r| r.valid);
                    format!(
                        "#{}  {}  \"{}\"  {:?}{}{}",
                        i + 1,
                        s.name,
                        s.title,
                        s.origin,
                        if valid { "" } else { "  INVALID" },
                        if s.active_runs > 0 {
                            format!("  {} run(s) not finished", s.active_runs)
                        } else {
                            String::new()
                        }
                    )
                })
                .collect();
            Ok(if lines.is_empty() {
                "no flows".into()
            } else {
                lines.join("\n")
            })
        }
        "flow_show" => {
            let name = flow_target(f, input).await?;
            let d = f.detail(&name).await.map_err(|e| e.to_string())?;
            let problems = f.engine.validate(&d.yaml).await;
            let active = f
                .runs(Some(&name), Some(RunStatus::Active))
                .await
                .map_err(|e| e.to_string())?;
            Ok(pretty(&json!({
                "name": name,
                "title": d.summary.title,
                "origin": d.summary.origin,
                "stations": d.stations.iter().map(|s| json!({"id": s.id, "title": s.title, "work": s.work, "exit": s.exit, "needs": s.needs})).collect::<Vec<_>>(),
                "problems": problems,
                "runs_not_finished": active.iter().map(|r| &r.id).collect::<Vec<_>>(),
                "yaml": d.yaml,
            })))
        }
        "flow_schema" => Ok(pretty(&json!({
            "definition": harnix_flow::definition_schema(),
            "catalog": f.packs().catalog(),
        }))),
        "flow_guide" => Ok(harnix_flow::GUIDE_EN.to_string()),
        "flow_validate" => {
            let problems = f.engine.validate(&need(input, "yaml")?).await;
            Ok(if problems.is_empty() {
                "valid".into()
            } else {
                pretty(&json!({"problems": problems}))
            })
        }
        "flow_create" => {
            let yaml = need(input, "yaml")?;
            let def = harnix_flow::FlowDef::parse(&yaml)
                .map_err(|e| format!("not a flow definition: {e}"))?;
            if f.packs().flow(&def.name).is_some() {
                return Err(format!(
                    "a flow named {} exists; use flow_update to change it",
                    def.name
                ));
            }
            let v = f
                .engine
                .save_flow(&yaml, by(rpc))
                .await
                .map_err(|e| e.to_string())?;
            f.flow_changed(&def.name).await;
            Ok(format!("created flow {}\n{}", def.name, pretty(&v)))
        }
        "flow_update" => {
            let name = flow_target(f, input).await?;
            let yaml = need(input, "yaml")?;
            let def = harnix_flow::FlowDef::parse(&yaml)
                .map_err(|e| format!("not a flow definition: {e}"))?;
            if def.name != name {
                return Err(format!(
                    "the YAML names flow {}, not {name}; a flow is not renamed",
                    def.name
                ));
            }
            unlocked(f, &name).await?;
            let v = f
                .engine
                .save_flow(&yaml, by(rpc))
                .await
                .map_err(|e| e.to_string())?;
            f.flow_changed(&name).await;
            Ok(format!("updated flow {name}\n{}", pretty(&v)))
        }
        "flow_delete" => {
            let name = flow_target(f, input).await?;
            unlocked(f, &name).await?;
            let v = f
                .engine
                .delete_flow(&name, by(rpc))
                .await
                .map_err(|e| e.to_string())?;
            f.flow_changed(&name).await;
            Ok(format!("deleted flow {name}\n{}", pretty(&v)))
        }
        "flow_dry_run" => {
            let name = flow_target(f, input).await?;
            let sandbox = Sandbox::new(
                f.packs(),
                SandboxConfig {
                    guarded: vec![],
                    submit_scenes: vec![super::SCENE.into()],
                    main_scene: Some(super::SCENE.into()),
                },
            );
            let start = StartIn {
                flow: Some(name),
                inputs: input.get("inputs").cloned().unwrap_or(Value::Null),
                mocks: input
                    .get("mocks")
                    .cloned()
                    .and_then(|m| serde_json::from_value(m).ok())
                    .unwrap_or_default(),
                ..StartIn::default()
            };
            let run = sandbox
                .start(start, by(rpc))
                .await
                .map_err(|e| e.to_string())?;
            let progress = sandbox.run(&run).await.map_err(|e| e.to_string())?;
            Ok(pretty(&serde_json::to_value(progress).unwrap_or_default()))
        }
        "flow_start" => {
            let name = flow_target(f, input).await?;
            let subject = need(input, "subject")?;
            let inputs = input.get("inputs").cloned().unwrap_or(Value::Null);
            let s = f
                .start_run(&name, &subject, inputs, None)
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "started run {} of flow {name}: \"{}\"",
                s.id, s.subject
            ))
        }
        "flow_runs" => {
            let flow = match input.get("target") {
                Some(_) => Some(flow_target(f, input).await?),
                None => None,
            };
            let mut runs = f
                .runs(flow.as_deref(), None)
                .await
                .map_err(|e| e.to_string())?;
            runs.sort_by_key(|r| std::cmp::Reverse(r.created_at));
            let lines: Vec<String> = runs
                .iter()
                .map(|r| {
                    let s = super::map::run_summary(r);
                    format!("{}  {}  \"{}\"  {:?}", s.id, s.flow, s.subject, s.status)
                })
                .collect();
            Ok(if lines.is_empty() {
                "no runs".into()
            } else {
                lines.join("\n")
            })
        }
        "flow_run" => {
            let v = f
                .view(&need(input, "run")?)
                .await
                .map_err(|e| e.to_string())?;
            Ok(pretty(&json!({
                "id": v.summary.id, "flow": v.summary.flow, "subject": v.summary.subject,
                "status": v.summary.status, "main_session": v.main_session,
                "stations": v.stations.iter().map(|s| json!({"id": s.id, "title": s.title, "status": s.status, "unmet": s.unmet})).collect::<Vec<_>>(),
            })))
        }
        _ => {
            let run = need(input, "run")?;
            let station = || need(input, "station").map(StationId);
            let op = match name {
                "flow_pause" => RunOp::Pause,
                "flow_resume" => RunOp::Resume,
                "flow_cancel" => RunOp::Cancel,
                "flow_release" => RunOp::Release {
                    station: station()?,
                },
                "flow_retry" => RunOp::Retry {
                    station: station()?,
                },
                "flow_force" => RunOp::Force {
                    station: station()?,
                    reason: need(input, "reason")?,
                },
                "flow_rework" => RunOp::Rework {
                    to: StationId(need(input, "to")?),
                },
                "flow_confirm" => {
                    let pass = input.get("pass").and_then(Value::as_bool).unwrap_or(true);
                    if !pass {
                        let st = station()?;
                        let reason = str_of(input, "reason");
                        let r = f
                            .engine
                            .confirm(&run, &st.0, false, reason.as_deref(), by(rpc))
                            .await;
                        f.runner
                            .settle(r)
                            .await
                            .map_err(flow_err)
                            .map_err(|e| e.to_string())?;
                        return Ok(format!("rejected station {} of run {run}", st.0));
                    }
                    RunOp::Confirm {
                        station: station()?,
                    }
                }
                other => return Err(format!("unknown tool {other}")),
            };
            f.op(&run, op).await.map_err(|e| e.to_string())?;
            Ok(format!("{name}: done for run {run}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_ones_are_declared_read_only_and_names_are_unique() {
        let d = declarations();
        let mut names: Vec<&str> = d.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for t in &d {
            let n = t["name"].as_str().unwrap();
            assert_eq!(t["read_only"] == true, FLOW_READ_ONLY.contains(&n), "{n}");
        }
        names.sort();
        names.dedup();
        assert_eq!(names.len(), d.len());
        for r in FLOW_READ_ONLY {
            assert!(names.contains(&r), "{r}");
        }
    }

    #[test]
    fn a_run_session_may_operate_only_its_own_run_and_change_no_definition() {
        let mine = Scope::Run("r1".into());
        assert!(allowed(&mine, "flow_list", &json!({})).is_ok());
        assert!(allowed(&mine, "flow_pause", &json!({"run": "r1"})).is_ok());
        assert!(allowed(&mine, "flow_pause", &json!({"run": "r2"})).is_err());
        assert!(allowed(&mine, "flow_update", &json!({"target": "x"})).is_err());
        assert!(allowed(&mine, "flow_start", &json!({"target": "x"})).is_err());
        let job = Scope::Job("j".into());
        assert!(allowed(&job, "flow_show", &json!({})).is_ok());
        assert!(allowed(&job, "flow_cancel", &json!({"run": "r1"})).is_err());
        assert!(allowed(&Scope::Full, "flow_delete", &json!({})).is_ok());
    }
}
