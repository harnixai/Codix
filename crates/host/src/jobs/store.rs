//! `jobs.json` 与 `jobs/<id>.jsonl`。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use client::Layout;
use port::{Job, JobEntry, JobId, JobOutcome, Usage};
use serde::{Deserialize, Serialize};

use super::cron;

/// 落盘记录。`last_fired_ms` / `created_ms` 是补跑要用的基线；`Job` 里的
/// `next_fire_ms` 与 `last` 是派生值，所以写盘时清掉、读盘后也不用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredJob {
    pub job: Job,
    #[serde(default)]
    pub last_fired_ms: Option<u64>,
    #[serde(default)]
    pub created_ms: u64,
}

impl StoredJob {
    /// 调度基线：上次触发，没触发过就是创建时间。
    pub fn base_ms(&self) -> u64 {
        self.last_fired_ms.unwrap_or(self.created_ms)
    }

    /// 按基线算的下一次触发；已经到期（待触发或待补跑）时照实返回过去的时刻。
    pub fn due_ms(&self) -> Option<u64> {
        if !self.job.enabled {
            return None;
        }
        cron::next_after(&self.job.cron, self.base_ms())
    }
}

pub struct JobStore {
    path: PathBuf,
    history_dir: PathBuf,
    jobs: Vec<StoredJob>,
}

static SEQ: AtomicU64 = AtomicU64::new(0);

impl JobStore {
    pub fn open(path: impl Into<PathBuf>, history_dir: impl Into<PathBuf>) -> JobStore {
        let path = path.into();
        let jobs = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<StoredJob>>(&b).ok())
            .unwrap_or_default();
        JobStore {
            path,
            history_dir: history_dir.into(),
            jobs,
        }
    }

    pub fn open_in(layout: &Layout, instance: &str) -> JobStore {
        let history = layout.job_history(instance, "x");
        let dir = history.parent().map(Path::to_path_buf).unwrap_or_default();
        JobStore::open(layout.jobs(instance), dir)
    }

    fn history_path(&self, id: &str) -> PathBuf {
        self.history_dir.join(format!("{id}.jsonl"))
    }

    fn save(&self) -> Result<(), String> {
        let mut stored = self.jobs.clone();
        for s in &mut stored {
            s.job.next_fire_ms = None;
            s.job.last = None;
        }
        let bytes = serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // 先写临时文件再改名：写到一半崩了也不会留下截断的 jobs.json。
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    fn decorate(&self, s: &StoredJob, now_ms: u64) -> Job {
        let mut job = s.job.clone();
        job.next_fire_ms = if job.enabled {
            cron::next_after(&job.cron, now_ms)
        } else {
            None
        };
        job.last = self
            .history(job.id.as_str(), 1)
            .into_iter()
            .next()
            .map(|e| e.outcome);
        job
    }

    pub fn list(&self, now_ms: u64) -> Vec<Job> {
        self.jobs.iter().map(|s| self.decorate(s, now_ms)).collect()
    }

    pub fn get(&self, id: &str, now_ms: u64) -> Option<Job> {
        self.jobs
            .iter()
            .find(|s| s.job.id.as_str() == id)
            .map(|s| self.decorate(s, now_ms))
    }

    /// 调度器用：不带派生字段的原始记录。
    pub fn stored(&self) -> &[StoredJob] {
        &self.jobs
    }

    /// 新建或覆盖。`now_ms` 由调用方给：只在新建或改了 cron 时用作补跑基线，
    /// 存储层自己读时钟的话测试没法控制时间。
    pub fn upsert(&mut self, mut job: Job, now_ms: u64) -> Result<Job, String> {
        cron::validate(&job.cron)?;
        if job.id.as_str().is_empty() {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            job.id = JobId(format!("job-{now_ms}-{n}"));
        } else if job.id.as_str().contains(['/', '\\']) || job.id.as_str().contains("..") {
            return Err(format!("非法作业 id：{}", job.id));
        }
        job.next_fire_ms = None;
        job.last = None;
        match self.jobs.iter_mut().find(|s| s.job.id == job.id) {
            Some(s) => {
                // cron 变了，旧基线下"错过"的触发已经没有意义。
                // 从停用恢复同理：停用期间的触发不算错过。
                if s.job.cron != job.cron || (!s.job.enabled && job.enabled) {
                    s.last_fired_ms = None;
                    s.created_ms = now_ms;
                }
                s.job = job.clone();
            }
            None => self.jobs.push(StoredJob {
                job: job.clone(),
                last_fired_ms: None,
                created_ms: now_ms,
            }),
        }
        self.save()?;
        let s = self.jobs.iter().find(|s| s.job.id == job.id).unwrap();
        Ok(self.decorate(s, now_ms))
    }

    pub fn delete(&mut self, id: &str) -> Result<bool, String> {
        let before = self.jobs.len();
        self.jobs.retain(|s| s.job.id.as_str() != id);
        let removed = self.jobs.len() != before;
        if removed {
            self.save()?;
            let _ = fs::remove_file(self.history_path(id));
        }
        Ok(removed)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool, now_ms: u64) -> Result<bool, String> {
        let Some(s) = self.jobs.iter_mut().find(|s| s.job.id.as_str() == id) else {
            return Ok(false);
        };
        if enabled && !s.job.enabled {
            s.last_fired_ms = None;
            s.created_ms = now_ms;
        }
        s.job.enabled = enabled;
        self.save()?;
        Ok(true)
    }

    pub fn mark_fired(&mut self, id: &str, fired_ms: u64) -> Result<(), String> {
        if let Some(s) = self.jobs.iter_mut().find(|s| s.job.id.as_str() == id) {
            s.last_fired_ms = Some(fired_ms);
            self.save()?;
        }
        Ok(())
    }

    pub fn append(&self, id: &str, entry: &JobEntry) -> Result<(), String> {
        fs::create_dir_all(&self.history_dir).map_err(|e| e.to_string())?;
        let mut line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        line.push('\n');
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_path(id))
            .map_err(|e| e.to_string())?;
        f.write_all(line.as_bytes()).map_err(|e| e.to_string())
    }

    /// 结束一次触发：追加同 `fired_ms` 的新行，读的时候后者覆盖前者。
    /// 不改写旧行，追加在崩溃时才安全；`target` 沿用 `Running` 行里记的。
    pub fn finish(
        &self,
        id: &str,
        fired_ms: u64,
        outcome: JobOutcome,
        elapsed_ms: u64,
        usage: Usage,
    ) -> Result<(), String> {
        let target = self
            .history(id, usize::MAX)
            .into_iter()
            .find(|e| e.fired_ms == fired_ms)
            .and_then(|e| e.target);
        self.append(
            id,
            &JobEntry {
                fired_ms,
                outcome,
                elapsed_ms,
                usage,
                target,
            },
        )
    }

    /// 新的在前；同一个 `fired_ms` 取最后一行。坏行跳过。
    pub fn history(&self, id: &str, limit: usize) -> Vec<JobEntry> {
        let Ok(text) = fs::read_to_string(self.history_path(id)) else {
            return Vec::new();
        };
        let mut by_fired: HashMap<u64, JobEntry> = HashMap::new();
        for line in text.lines() {
            if let Ok(e) = serde_json::from_str::<JobEntry>(line) {
                by_fired.insert(e.fired_ms, e);
            }
        }
        let mut out: Vec<JobEntry> = by_fired.into_values().collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.fired_ms));
        out.truncate(limit);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use port::JobKind;

    fn job(id: &str, cron: &str) -> Job {
        Job {
            id: JobId(id.into()),
            name: id.into(),
            cron: cron.into(),
            kind: JobKind::Simple {
                prompt: "hi".into(),
                skills: vec![],
                project: None,
                scene: None,
                model: None,
            },
            enabled: true,
            catch_up: false,
            next_fire_ms: None,
            last: None,
        }
    }

    fn entry(fired_ms: u64, outcome: JobOutcome) -> JobEntry {
        JobEntry {
            fired_ms,
            outcome,
            elapsed_ms: 0,
            usage: Usage::default(),
            target: None,
        }
    }

    fn open(dir: &Path) -> JobStore {
        JobStore::open(dir.join("jobs.json"), dir.join("jobs"))
    }

    #[test]
    fn round_trip_and_derived_fields() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        let saved = s.upsert(job("a", "* * * * *"), 1_000).unwrap();
        assert_eq!(saved.next_fire_ms, Some(60_000));
        s.mark_fired("a", 60_000).unwrap();
        let s2 = open(d.path());
        assert_eq!(s2.stored()[0].last_fired_ms, Some(60_000));
        assert_eq!(s2.stored()[0].created_ms, 1_000);
        assert_eq!(s2.stored()[0].job.next_fire_ms, None);
        assert_eq!(s2.list(61_000)[0].next_fire_ms, Some(120_000));
        assert!(s2.get("a", 0).is_some());
        assert!(s2.get("zz", 0).is_none());
    }

    #[test]
    fn upsert_validates_and_generates_id() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        assert!(s.upsert(job("a", "bad"), 0).is_err());
        assert!(s.upsert(job("../x", "* * * * *"), 0).is_err());
        let j1 = s.upsert(job("", "* * * * *"), 5).unwrap();
        let j2 = s.upsert(job("", "* * * * *"), 5).unwrap();
        assert!(j1.id.as_str().starts_with("job-5-"));
        assert_ne!(j1.id, j2.id);
    }

    #[test]
    fn disabled_has_no_next() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        s.upsert(job("a", "* * * * *"), 0).unwrap();
        assert!(s.set_enabled("a", false, 0).unwrap());
        assert_eq!(s.list(0)[0].next_fire_ms, None);
        assert!(!s.set_enabled("nope", true, 0).unwrap());
    }

    #[test]
    fn history_last_line_wins_and_corrupt_skipped() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        s.upsert(job("a", "* * * * *"), 0).unwrap();
        s.append("a", &entry(100, JobOutcome::Running)).unwrap();
        s.append("a", &entry(200, JobOutcome::Running)).unwrap();
        s.finish("a", 100, JobOutcome::Done, 7, Usage::default())
            .unwrap();
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(d.path().join("jobs/a.jsonl"))
            .unwrap();
        f.write_all(b"{garbage\n").unwrap();
        let h = s.history("a", 10);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].fired_ms, 200);
        assert_eq!(h[0].outcome, JobOutcome::Running);
        assert_eq!(h[1].outcome, JobOutcome::Done);
        assert_eq!(h[1].elapsed_ms, 7);
        assert_eq!(s.history("a", 1).len(), 1);
        assert_eq!(s.list(0)[0].last, Some(JobOutcome::Running));
    }

    #[test]
    fn delete_removes_history() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        s.upsert(job("a", "* * * * *"), 0).unwrap();
        s.append("a", &entry(1, JobOutcome::Done)).unwrap();
        assert!(d.path().join("jobs/a.jsonl").exists());
        assert!(s.delete("a").unwrap());
        assert!(!d.path().join("jobs/a.jsonl").exists());
        assert!(!s.delete("a").unwrap());
        assert!(open(d.path()).stored().is_empty());
    }
}
