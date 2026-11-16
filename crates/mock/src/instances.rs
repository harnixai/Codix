use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use port::{
    Attach, Backend, InstanceInfo, InstanceName, InstanceState, Instances, NewInstance, PortError,
};

use crate::backend::{MockOptions, ScriptedBackend};
use crate::scenario::Scenario;
use crate::state::MockState;

#[derive(Debug, Clone)]
pub struct MockInstance {
    pub info: InstanceInfo,
    pub seed: String,
}

/// mock 的实例目录。第一个运行中的实例用场景的种子与时间线，其余各用自己的种子、没有时间线。
/// 后端按实例缓存：脱离再接入，状态还在——实例本来就是常驻的。
pub struct MockInstances {
    list: Mutex<Vec<MockInstance>>,
    scenario: Scenario,
    opts: MockOptions,
    backends: Mutex<HashMap<InstanceName, Arc<ScriptedBackend>>>,
}

impl MockInstances {
    pub fn new(scenario: Scenario, opts: MockOptions) -> Self {
        let mut list: Vec<MockInstance> = crate::seed::default_instances()
            .into_iter()
            .map(|(info, seed)| MockInstance {
                info,
                seed: seed.to_string(),
            })
            .collect();
        if let Some(first) = list.first_mut() {
            first.seed = scenario.seed.clone();
        }
        Self::with_list(list, scenario, opts)
    }

    pub fn with_list(list: Vec<MockInstance>, scenario: Scenario, opts: MockOptions) -> Self {
        MockInstances {
            list: Mutex::new(list),
            scenario,
            opts,
            backends: Mutex::new(HashMap::new()),
        }
    }

    /// 测试用：取某个实例已经建好的后端。
    pub fn backend(&self, name: &str) -> Option<Arc<ScriptedBackend>> {
        self.backends
            .lock()
            .unwrap()
            .get(&InstanceName::from(name))
            .cloned()
    }

    fn open(&self, name: &str) -> Result<Arc<ScriptedBackend>, PortError> {
        let key = InstanceName::from(name);
        if let Some(b) = self.backends.lock().unwrap().get(&key) {
            return Ok(b.clone());
        }
        let (info, seed_name, is_first) = {
            let list = self.list.lock().unwrap();
            let pos = list
                .iter()
                .position(|i| i.info.name == key)
                .ok_or_else(|| PortError::NoSuchInstance(name.to_string()))?;
            (list[pos].info.clone(), list[pos].seed.clone(), pos == 0)
        };
        let seed = crate::load_seed(&seed_name).map_err(|e| PortError::Failed(e.to_string()))?;
        let scenario = if is_first {
            self.scenario.clone()
        } else {
            Scenario::seed_only(&seed_name)
        };
        let backend = ScriptedBackend::new(MockState::new(info, seed), scenario, self.opts);
        self.backends.lock().unwrap().insert(key, backend.clone());
        Ok(backend)
    }

    fn set(&self, name: &str, f: impl FnOnce(&mut InstanceInfo)) -> Result<(), PortError> {
        let mut list = self.list.lock().unwrap();
        let inst = list
            .iter_mut()
            .find(|i| i.info.name.as_str() == name)
            .ok_or_else(|| PortError::NoSuchInstance(name.to_string()))?;
        f(&mut inst.info);
        Ok(())
    }
}

#[async_trait]
impl Instances for MockInstances {
    async fn list(&self, all: bool) -> Result<Vec<InstanceInfo>, PortError> {
        Ok(self
            .list
            .lock()
            .unwrap()
            .iter()
            .filter(|i| all || i.info.state == InstanceState::Running)
            .map(|i| i.info.clone())
            .collect())
    }

    async fn create(&self, opts: NewInstance) -> Result<Arc<dyn Backend>, PortError> {
        let name = {
            let mut list = self.list.lock().unwrap();
            let name = opts.name.unwrap_or_else(|| {
                let n = (1..)
                    .map(|i| format!("new-{i}"))
                    .find(|n| !list.iter().any(|i| i.info.name.as_str() == n))
                    .unwrap();
                InstanceName(n)
            });
            if list.iter().any(|i| i.info.name == name) {
                return Err(PortError::Failed(format!("实例 {name} 已存在")));
            }
            let host = list
                .first()
                .map(|i| i.info.host.clone())
                .unwrap_or_default();
            let pid = 50_000 + list.len() as u32;
            list.push(MockInstance {
                info: InstanceInfo {
                    name: name.clone(),
                    host,
                    pid: Some(pid),
                    project: opts.project,
                    state: InstanceState::Running,
                    started_ms: Some(self.opts.epoch_ms),
                    stopped_ms: None,
                    version: "0.1.0".into(),
                    sessions: 0,
                    waiting: 0,
                    jobs: 0,
                    holder: None,
                },
                seed: "empty".into(),
            });
            name
        };
        Ok(self.open(name.as_str())?)
    }

    async fn attach(&self, name: &str, opts: Attach) -> Result<Arc<dyn Backend>, PortError> {
        let info = self
            .list
            .lock()
            .unwrap()
            .iter()
            .find(|i| i.info.name.as_str() == name)
            .map(|i| i.info.clone())
            .ok_or_else(|| PortError::NoSuchInstance(name.to_string()))?;
        if info.state != InstanceState::Running {
            return Err(PortError::Failed(format!("实例 {name} 没在运行")));
        }
        if let Some(holder) = &info.holder
            && !opts.force
        {
            return Err(PortError::Occupied {
                holder: holder.clone(),
                idle_ms: self.opts.epoch_ms.saturating_sub(holder.last_input_ms),
            });
        }
        self.set(name, |i| i.holder = None)?;
        Ok(self.open(name)?)
    }

    async fn start(&self, name: &str) -> Result<Arc<dyn Backend>, PortError> {
        let epoch = self.opts.epoch_ms;
        self.set(name, |i| {
            i.state = InstanceState::Running;
            i.started_ms = Some(epoch);
            i.stopped_ms = None;
            i.pid = Some(60_000);
        })?;
        Ok(self.open(name)?)
    }

    async fn stop(&self, name: &str) -> Result<(), PortError> {
        let epoch = self.opts.epoch_ms;
        self.set(name, |i| {
            i.state = InstanceState::Stopped;
            i.stopped_ms = Some(epoch);
            i.pid = None;
            i.holder = None;
        })?;
        self.backends
            .lock()
            .unwrap()
            .remove(&InstanceName::from(name));
        Ok(())
    }
}
