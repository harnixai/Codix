//! 作业的一次触发：简单作业建会话、流程作业起运行（architecture §8.5）。

use super::*;

impl Hub {
    pub(super) async fn fire_job(&self, job: Job, fired_ms: u64) {
        let Ok(sched) = self.scheduler() else { return };
        let store = sched.store();
        let (entry, run) = match &job.kind {
            JobKind::Simple {
                prompt,
                skills,
                scene,
                model,
                ..
            } => {
                let mut extra = json!({"attended": false, "auto_name": false});
                if !skills.is_empty() {
                    extra["prompt"] = json!({"append": format!(
                        "This is the scheduled job \"{}\". Use these skills: {}.",
                        job.name,
                        skills.join(", ")
                    )});
                }
                let origin = Origin::Job {
                    job: job.id.clone(),
                    name: job.name.clone(),
                    fired_ms,
                };
                match self
                    .create_session(
                        origin,
                        Some(job.name.clone()),
                        scene.clone(),
                        model.clone(),
                        extra,
                    )
                    .await
                {
                    Ok(sid) => (
                        JobEntry {
                            fired_ms,
                            outcome: JobOutcome::Running,
                            elapsed_ms: 0,
                            usage: Usage::default(),
                            target: Some(JobTarget::Session {
                                id: SessionId(sid.clone()),
                                title: job.name.clone(),
                            }),
                        },
                        Some((sid, prompt.clone())),
                    ),
                    Err(e) => (
                        JobEntry {
                            fired_ms,
                            outcome: JobOutcome::Failed {
                                message: e.to_string(),
                            },
                            elapsed_ms: 0,
                            usage: Usage::default(),
                            target: None,
                        },
                        None,
                    ),
                }
            }
            // 流程作业：启动一次运行。运行有它自己的结局，触发记录只记“启动了没有”。
            JobKind::Flow {
                flow,
                subject,
                inputs,
            } => {
                let started = match self.flows() {
                    Some(f) => f.start_for_job(flow, subject, inputs.clone(), &job).await,
                    None => Err("flows are not available in this instance".into()),
                };
                let (outcome, target) = match started {
                    Ok((id, subject)) => (JobOutcome::Done, Some(JobTarget::Run { id, subject })),
                    Err(message) => (JobOutcome::Failed { message }, None),
                };
                (
                    JobEntry {
                        fired_ms,
                        outcome,
                        elapsed_ms: 0,
                        usage: Usage::default(),
                        target,
                    },
                    None,
                )
            }
        };
        let _ = store.locked().append(&job.id.0, &entry);
        let mut events = vec![Event::JobFired {
            job: job.id.clone(),
            entry,
        }];
        if let Some(j) = sched.get(&job.id.0) {
            events.push(Event::JobUpserted(j));
        }
        self.emit_events(events);
        if let Some((sid, prompt)) = run {
            self.0
                .job_runs
                .locked()
                .insert(sid.clone(), (job.id.0.clone(), fired_ms, self.now()));
            self.run_turn(&sid, &prompt, "queue");
        }
    }
}
