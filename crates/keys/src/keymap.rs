use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{Action, Key};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Context {
    Global,
    Composer,
    Transcript,
    Sessions,
    Flows,
    Jobs,
    Dialog,
    Picker,
    Instances,
    Overlay,
}

impl Context {
    pub const ALL: [Context; 10] = [
        Context::Global,
        Context::Composer,
        Context::Transcript,
        Context::Sessions,
        Context::Flows,
        Context::Jobs,
        Context::Dialog,
        Context::Picker,
        Context::Instances,
        Context::Overlay,
    ];

    pub fn label(self) -> String {
        match self {
            Context::Global => i18n::t!("keys.context.global"),
            Context::Composer => i18n::t!("keys.context.composer"),
            Context::Transcript => i18n::t!("keys.context.transcript"),
            Context::Sessions => i18n::t!("keys.context.sessions"),
            Context::Flows => i18n::t!("keys.context.flows"),
            Context::Jobs => i18n::t!("keys.context.jobs"),
            Context::Dialog => i18n::t!("keys.context.dialog"),
            Context::Picker => i18n::t!("keys.context.picker"),
            Context::Instances => i18n::t!("keys.context.instances"),
            Context::Overlay => i18n::t!("keys.context.overlay"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub context: Context,
    pub keys: Vec<Key>,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapError {
    Conflict {
        context: Context,
        keys: String,
        first: Action,
        second: Action,
    },
    PrefixClash {
        context: Context,
        keys: String,
    },
    BadKey(String),
}

// 不用 thiserror 的 `#[error("…")]`：它要字面量，而这里的文字得按当前语言取。
impl std::fmt::Display for KeymapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeymapError::Conflict {
                context,
                keys,
                first,
                second,
            } => f.write_str(&i18n::t!(
                "keys.error.conflict",
                context = format!("{context:?}"),
                keys = keys,
                first = format!("{first:?}"),
                second = format!("{second:?}")
            )),
            KeymapError::PrefixClash { context, keys } => f.write_str(&i18n::t!(
                "keys.error.prefix_clash",
                context = format!("{context:?}"),
                keys = keys
            )),
            KeymapError::BadKey(key) => f.write_str(&i18n::t!("keys.error.bad_key", key = key)),
        }
    }
}

impl std::error::Error for KeymapError {}

pub(crate) enum Lookup {
    Exact(Action),
    Prefix,
    None,
}

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<Binding>,
    index: HashMap<(Context, Vec<Key>), Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::new(crate::defaults::bindings()).expect("builtin keymap has a conflict")
    }
}

impl Keymap {
    pub fn new(bindings: Vec<Binding>) -> Result<Keymap, KeymapError> {
        let mut index = HashMap::new();
        for b in &bindings {
            let key = (b.context, b.keys.clone());
            if let Some(first) = index.insert(key, b.action)
                && first != b.action
            {
                return Err(KeymapError::Conflict {
                    context: b.context,
                    keys: chord_label(&b.keys),
                    first,
                    second: b.action,
                });
            }
        }
        for b in &bindings {
            for n in 1..b.keys.len() {
                if index.contains_key(&(b.context, b.keys[..n].to_vec())) {
                    return Err(KeymapError::PrefixClash {
                        context: b.context,
                        keys: chord_label(&b.keys[..n]),
                    });
                }
            }
        }
        Ok(Keymap { bindings, index })
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    pub(crate) fn lookup(&self, ctx: Context, keys: &[Key]) -> Lookup {
        if let Some(a) = self.index.get(&(ctx, keys.to_vec())) {
            return Lookup::Exact(*a);
        }
        let is_prefix = self
            .bindings
            .iter()
            .any(|b| b.context == ctx && b.keys.len() > keys.len() && b.keys.starts_with(keys));
        if is_prefix {
            Lookup::Prefix
        } else {
            Lookup::None
        }
    }

    /// 某个动作在某上下文（含 `Global`）里的第一个绑定键，给底栏和帮助用。
    pub fn key_for(&self, ctx: Context, action: Action) -> Option<String> {
        self.bindings
            .iter()
            .find(|b| b.action == action && (b.context == ctx || b.context == Context::Global))
            .map(|b| chord_label(&b.keys))
    }

    /// 底栏左侧的提示：当前上下文里值得提示的动作，按 `defaults::footer` 的顺序。
    pub fn hints(&self, ctx: Context) -> Vec<(String, String)> {
        crate::defaults::footer(ctx)
            .iter()
            .filter_map(|a| self.key_for(ctx, *a).map(|k| (k, a.help())))
            .collect()
    }

    /// 底栏最右边永远在的几项（需求 L-07）：切块、换标签、切栏、退出、帮助——最基本的走动与离开。
    /// 取实际绑定，用户改了键也跟着变。
    pub fn essential(&self) -> Vec<(String, String)> {
        [
            Action::FocusCycle,
            Action::NextTab,
            Action::PaneToggle,
            Action::Exit,
            Action::Help,
        ]
        .into_iter()
        .map(|a| {
            let key = self
                .key_for(Context::Global, a)
                .unwrap_or_else(|| "?".into());
            (key, a.help())
        })
        .collect()
    }

    /// 帮助浮层：某上下文与全局的全部绑定。
    pub fn listing(&self, ctx: Context) -> Vec<(Context, String, String)> {
        let mut out: Vec<_> = self
            .bindings
            .iter()
            .filter(|b| b.context == ctx)
            .chain(
                self.bindings
                    .iter()
                    .filter(|b| b.context == Context::Global),
            )
            .map(|b| (b.context, chord_label(&b.keys), b.action.help()))
            .collect();
        out.dedup();
        out
    }
}

pub fn chord_label(keys: &[Key]) -> String {
    keys.iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}
