//! 无人值守的护栏的执行（architecture §8.5）：到点触发前过一遍（每日额度、重叠、并发），简单作业的会话
//! 按帧记账、看超时，作业起的运行按运行的用量记账、超了暂停，连续失败自动停用。

use super::*;
use crate::jobs::ledger::day_of;
use port::{Overlap, SkipCause, StopCause};

/// 一次简单作业的触发在跑：它的会话用了多少。
pub(super) struct Meter {
    job: String,
    fired: u64,
    started: u64,
    /// 主会话每次调用的用量之和。
    steps: u64,
    /// 子 Agent 各自到此刻的累计（任务帧带的是累计，不是增量）。
    tasks: HashMap<String, u64>,
    /// 已经记进每日账的。
    counted: u64,
    /// 已经在停了：别再停一次。
    stopping: bool,
}

/// 没能马上跑的一次到点触发：重叠策略是 `queue`，或同时在跑的满了。过了 `deadline`（下一次到点）还没轮到就跳过。
pub(super) struct Waiting {
    job: Job,
    fired: u64,
    deadline: u64,
    why: SkipCause,
}

/// 多久看一次超时、运行的用量、排队的触发。
pub(super) const TICK_MS: u64 = 10_000;

impl Hub {
    /// 到点触发之前：每日额度、重叠、并发。回 `false` 就是这一次不跑（已经记下跳过，或排进了队）。
    pub(super) async fn admit(&self, job: &Job, fired: u64) -> bool {
        let Ok(sched) = self.scheduler() else {
            return false;
        };
        let cfg = self.guard_config();
        if self.daily_exhausted(&cfg) {
            self.record_skip(job, fired, SkipCause::DailyBudget);
            return false;
        }
        let (guard, _) = cfg.effective(&job.guard);
        if self.job_running(&job.id.0).await {
            match guard.overlap.unwrap_or(Overlap::Skip) {
                Overlap::Skip => {
                    self.record_skip(job, fired, SkipCause::Overlap);
                    return false;
                }
                Overlap::Queue => {
                    let mut q = self.0.waiting.locked();
                    if q.iter().any(|w| w.job.id == job.id) {
                        drop(q);
                        self.record_skip(job, fired, SkipCause::Overlap);
                    } else {
                        q.push(Waiting {
                            job: job.clone(),
                            fired,
                            deadline: next_due(job, fired),
                            why: SkipCause::Overlap,
                        });
                    }
                    return false;
                }
                Overlap::Replace => {
                    if let Err(e) = self.stop_firing(&job.id.0, None, StopCause::Replaced).await {
                        tracing::warn!(job = %job.id, "replacing the running firing: {e}");
                    }
                    return true;
                }
            }
        }
        if self.running_count().await >= cfg.max_concurrent as usize {
            self.0.waiting.locked().push(Waiting {
                job: job.clone(),
                fired,
                deadline: next_due(job, fired),
                why: SkipCause::Busy,
            });
            return false;
        }
        drop(sched);
        true
    }

    async fn job_running(&self, job: &str) -> bool {
        if self.0.meters.locked().values().any(|m| m.job == job) {
            return true;
        }
        match self.flows() {
            Some(f) => f
                .scheduled_runs()
                .await
                .iter()
                .any(|r| r.job == job && !r.paused),
            None => false,
        }
    }

    async fn running_count(&self) -> usize {
        let sessions = self.0.meters.locked().len();
        let runs = match self.flows() {
            Some(f) => f
                .scheduled_runs()
                .await
                .iter()
                .filter(|r| !r.paused)
                .count(),
            None => 0,
        };
        sessions + runs
    }

    fn daily_exhausted(&self, cfg: &crate::jobs::guard::GuardConfig) -> bool {
        if cfg.daily_tokens == 0 {
            return false;
        }
        let Ok(sched) = self.scheduler() else {
            return false;
        };
        let today = day_of(self.now());
        sched.store().locked().ledger().total(&today) >= cfg.daily_tokens
    }

    /// 记一条跳过的触发并推出去。
    fn record_skip(&self, job: &Job, fired: u64, why: SkipCause) {
        let Ok(sched) = self.scheduler() else { return };
        let entry = JobEntry {
            fired_ms: fired,
            outcome: JobOutcome::Skipped { reason: why },
            elapsed_ms: 0,
            usage: Usage::default(),
            target: None,
        };
        let _ = sched.store().locked().append(&job.id.0, &entry);
        let mut events = vec![Event::JobFired {
            job: job.id.clone(),
            entry,
        }];
        events.extend(sched.get(&job.id.0).map(Event::JobUpserted));
        self.emit_events(events);
    }

    /// 简单作业的会话开始跑了：从这一刻起记账、看超时。
    pub(super) fn meter_start(&self, session: &str, job: &str, fired: u64) {
        self.0.meters.locked().insert(
            session.to_string(),
            Meter {
                job: job.to_string(),
                fired,
                started: self.now(),
                steps: 0,
                tasks: HashMap::new(),
                counted: 0,
                stopping: false,
            },
        );
    }

    pub(super) fn meter_end(&self, session: &str) {
        self.0.meters.locked().remove(session);
    }

    /// 作业的会话来了一帧：每次调用、子 Agent 的累计用量记进账，超了单次上限或每日额度就停。
    pub(super) fn meter_frame(&self, session: &str, ev: &SessionEvent) {
        let tokens = |u: &wire::frames::Usage| u.input_tokens + u.output_tokens;
        let (job, fired, delta, total) = {
            let mut meters = self.0.meters.locked();
            let Some(m) = meters.get_mut(session) else {
                return;
            };
            match ev {
                SessionEvent::StepEnd { usage } => m.steps += tokens(usage),
                SessionEvent::TaskStarted { task }
                | SessionEvent::TaskUpdated { task }
                | SessionEvent::TaskFinished { task } => {
                    if let Some(u) = &task.usage {
                        m.tasks.insert(task.task_id.clone(), tokens(u));
                    }
                }
                _ => return,
            }
            let total = m.steps + m.tasks.values().sum::<u64>();
            let delta = total.saturating_sub(m.counted);
            m.counted = total;
            (m.job.clone(), m.fired, delta, total)
        };
        let Ok(sched) = self.scheduler() else { return };
        let cfg = self.guard_config();
        if delta > 0 {
            let today = day_of(self.now());
            let _ = sched.store().locked().ledger().add(&today, &job, delta);
        }
        let limit = sched
            .get(&job)
            .map(|j| cfg.effective(&j.guard).0.max_tokens.unwrap_or(u64::MAX))
            .unwrap_or(u64::MAX);
        if total > limit {
            self.stop_meter(session, &job, fired, StopCause::Budget);
        } else if self.daily_exhausted(&cfg) {
            self.exhaust();
        }
    }

    /// 停一个在跑的简单作业触发（只停一次）。
    fn stop_meter(&self, session: &str, job: &str, fired: u64, why: StopCause) {
        {
            let mut meters = self.0.meters.locked();
            match meters.get_mut(session) {
                Some(m) if !m.stopping => m.stopping = true,
                _ => return,
            }
        }
        let hub = self.clone();
        let job = job.to_string();
        tokio::spawn(async move {
            if let Err(e) = hub.stop_firing(&job, Some(fired), why).await {
                tracing::warn!(job, "stopping a firing ({why:?}): {e}");
            }
        });
    }

    /// 今天的额度用完了：在跑的都停、作业起的运行都暂停，说一次。
    fn exhaust(&self) {
        let today = day_of(self.now());
        {
            let mut ex = self.0.exhausted.locked();
            if ex.as_deref() == Some(today.as_str()) {
                return;
            }
            *ex = Some(today);
        }
        let running: Vec<(String, String, u64)> = self
            .0
            .meters
            .locked()
            .iter()
            .map(|(s, m)| (s.clone(), m.job.clone(), m.fired))
            .collect();
        for (s, job, fired) in running {
            self.stop_meter(&s, &job, fired, StopCause::DailyBudget);
        }
        let hub = self.clone();
        tokio::spawn(async move {
            if let Some(f) = hub.flows() {
                for r in f.scheduled_runs().await.into_iter().filter(|r| !r.paused) {
                    hub.pause_run(&f, &r, "the daily budget is used up").await;
                }
            }
        });
        let cfg = self.guard_config();
        let text = format!(
            "unattended work used up today's budget ({} tokens): running jobs stopped, the rest of today's firings are skipped",
            crate::jobs::guard::fmt_tokens(cfg.daily_tokens)
        );
        self.alert(
            port::AlertKind::DailyBudget,
            port::NoticeLevel::Warn,
            format!("daily:{}", day_of(self.now())),
            "today's unattended budget is used up".into(),
            text.clone(),
        );
        self.emit_events(vec![Event::Notice {
            level: port::NoticeLevel::Warn,
            text,
        }]);
    }

    async fn pause_run(&self, f: &Arc<dyn FlowPort>, r: &ScheduledRun, why: &str) {
        if !self.0.paused_runs.locked().insert(r.run.0.clone()) {
            return;
        }
        let res = f
            .command(Command::RunOp {
                run: r.run.clone(),
                op: port::RunOp::Pause,
            })
            .await;
        let text = match res {
            Ok(_) => format!("paused the run \"{}\" of job {}: {why}", r.subject, r.job),
            Err(e) => format!(
                "could not pause the run \"{}\" of job {}: {e}",
                r.subject, r.job
            ),
        };
        self.emit_events(vec![Event::Notice {
            level: port::NoticeLevel::Warn,
            text,
        }]);
    }

    /// 定时看一遍：简单作业的超时、作业起的运行的用量与超时、排队的触发。
    pub(super) async fn guard_tick(&self) {
        let Ok(sched) = self.scheduler() else { return };
        let cfg = self.guard_config();
        let now = self.now();
        let effective = |job: &str| {
            sched
                .get(job)
                .map(|j| cfg.effective(&j.guard).0)
                .unwrap_or_else(|| cfg.defaults.clone())
        };
        let overdue: Vec<(String, String, u64)> = self
            .0
            .meters
            .locked()
            .iter()
            .filter(|(_, m)| !m.stopping)
            .filter(|(_, m)| {
                now.saturating_sub(m.started) > effective(&m.job).timeout_ms.unwrap_or(u64::MAX)
            })
            .map(|(s, m)| (s.clone(), m.job.clone(), m.fired))
            .collect();
        for (s, job, fired) in overdue {
            self.stop_meter(&s, &job, fired, StopCause::Timeout);
        }

        if let Some(f) = self.flows() {
            let runs = f.scheduled_runs().await;
            let today = day_of(now);
            for r in &runs {
                let delta = {
                    let mut seen = self.0.run_seen.locked();
                    let last = seen.insert(r.run.0.clone(), r.tokens).unwrap_or(0);
                    r.tokens.saturating_sub(last)
                };
                let _ = sched.store().locked().ledger().add(&today, &r.job, delta);
                if r.paused {
                    continue;
                }
                let g = effective(&r.job);
                if now.saturating_sub(r.started_ms) > g.timeout_ms.unwrap_or(u64::MAX) {
                    self.pause_run(&f, r, "it ran past the job's timeout").await;
                } else if r.tokens > g.max_tokens.unwrap_or(u64::MAX) {
                    self.pause_run(&f, r, "it used more tokens than the job allows")
                        .await;
                }
            }
            // 结束了的运行不再记。
            self.0
                .run_seen
                .locked()
                .retain(|id, _| runs.iter().any(|r| &r.run.0 == id));
        }
        if self.daily_exhausted(&cfg) {
            self.exhaust();
        }

        let waiting: Vec<Waiting> = std::mem::take(&mut *self.0.waiting.locked());
        let mut keep = vec![];
        for w in waiting {
            if now >= w.deadline {
                self.record_skip(&w.job, w.fired, w.why);
                continue;
            }
            let free = match w.why {
                SkipCause::Overlap => !self.job_running(&w.job.id.0).await,
                _ => true,
            } && self.running_count().await < cfg.max_concurrent as usize
                && !self.daily_exhausted(&cfg);
            if free {
                let hub = self.clone();
                tokio::spawn(async move { hub.launch(w.job, now).await });
            } else {
                keep.push(w);
            }
        }
        self.0.waiting.locked().extend(keep);
    }

    /// 一次触发收尾之后：连续失败到了上限就在这台机器上停用这个作业。
    pub(super) fn after_finish(&self, job: &str) {
        let Ok(sched) = self.scheduler() else { return };
        let cfg = self.guard_config();
        let Some(j) = sched.get(job) else { return };
        let after = cfg.effective(&j.guard).0.disable_after.unwrap_or(0);
        if after == 0 || !j.enabled {
            return;
        }
        let streak = sched.store().locked().failure_streak(job);
        if streak < after {
            return;
        }
        let disabled = sched
            .store()
            .locked()
            .auto_disable(job, streak, self.now())
            .unwrap_or(false);
        if !disabled {
            return;
        }
        let mut events = vec![Event::Notice {
            level: port::NoticeLevel::Warn,
            text: format!(
                "job \"{}\" failed {streak} times in a row and is disabled on this machine",
                j.name
            ),
        }];
        events.extend(sched.get(job).map(Event::JobUpserted));
        self.emit_events(events);
    }
}

/// 下一次到点的时刻：排着的触发等到这时还没轮到就跳过。
fn next_due(job: &Job, fired: u64) -> u64 {
    crate::jobs::cron::next_after(&job.cron, fired).unwrap_or(fired + 3_600_000)
}
