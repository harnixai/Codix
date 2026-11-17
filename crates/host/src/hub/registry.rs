//! Codix 自己记的会话归属（architecture §8.4，`sessions.json`）。Agix 不知道一个会话是作业
//! 触发的还是编辑作业用的，冷会话的名字它也不报，所以这些由实例记下来。

use std::collections::BTreeMap;
use std::path::PathBuf;

use port::Origin;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub origin: Origin,
    #[serde(default)]
    pub title: Option<String>,
    /// 名字只是第一句话的摘头：引擎自动起的名字来了可以替掉它。
    #[serde(default)]
    pub title_from_text: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub updated_ms: u64,
    /// 编辑会话的键（`job:<id>`），同一个作业总是同一个会话。
    #[serde(default)]
    pub editor_key: Option<String>,
    /// 在 Codix 里关掉的：Agix 的列表里还有它（transcript 在），会话标签里不再列。
    #[serde(default)]
    pub closed: bool,
}

#[derive(Debug, Default)]
pub struct Registry {
    path: Option<PathBuf>,
    records: BTreeMap<String, Record>,
}

impl Registry {
    /// 读不出来就从空的开始：归属丢了只是退回 `User`，不该让实例起不来。
    pub fn open(path: PathBuf) -> Registry {
        let records = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Registry {
            path: Some(path),
            records,
        }
    }

    pub fn in_memory() -> Registry {
        Registry::default()
    }

    pub fn get(&self, id: &str) -> Option<&Record> {
        self.records.get(id)
    }

    pub fn by_editor_key(&self, key: &str) -> Option<String> {
        self.records
            .iter()
            .find(|(_, r)| r.editor_key.as_deref() == Some(key))
            .map(|(id, _)| id.clone())
    }

    pub fn update(&mut self, id: &str, f: impl FnOnce(&mut Record)) {
        f(self.records.entry(id.to_string()).or_default());
        self.save();
    }

    pub fn remove(&mut self, id: &str) {
        if self.records.remove(id).is_some() {
            self.save();
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let tmp = path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string_pretty(&self.records)
            && std::fs::write(&tmp, text).is_ok()
        {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_reopen_and_editor_keys_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let mut r = Registry::open(path.clone());
        r.update("s1", |x| {
            x.origin = Origin::Editor {
                of: port::EditorOf::Job,
                name: "nightly".into(),
            };
            x.editor_key = Some("job:j1".into());
        });
        let r = Registry::open(path);
        assert_eq!(r.by_editor_key("job:j1").as_deref(), Some("s1"));
        assert!(matches!(r.get("s1").unwrap().origin, Origin::Editor { .. }));
    }
}
