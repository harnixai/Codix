use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{Action, Key};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Context {
    Global,
    Composer,
    Transcript,
    /// 对话区顶部面板（有焦点时滚动它）。
    Panel,
    Sessions,
    Flows,
    Jobs,
    Dialog,
    Picker,
    Instances,
    Overlay,
}

impl Context {
    pub const ALL: [Context; 11] = [
        Context::Global,
        Context::Composer,
        Context::Transcript,
        Context::Panel,
        Context::Sessions,
        Context::Flows,
        Context::Jobs,
        Context::Dialog,
        Context::Picker,
        Context::Instances,
        Context::Overlay,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Context::Global => "全局",
            Context::Composer => "输入框",
            Context::Transcript => "转录",
            Context::Panel => "顶部面板",
            Context::Sessions => "会话",
            Context::Flows => "流程",
            Context::Jobs => "作业",
            Context::Dialog => "对话框",
            Context::Picker => "补全列表",
            Context::Instances => "实例选择",
            Context::Overlay => "浮层",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub context: Context,
    pub keys: Vec<Key>,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeymapError {
    #[error("{context:?} 上下文里 {keys} 同时绑定了 {first:?} 和 {second:?}")]
    Conflict {
        context: Context,
        keys: String,
        first: Action,
        second: Action,
    },
    #[error("{context:?} 上下文里 {keys} 既是一个绑定、又是另一个和弦的前缀")]
    PrefixClash { context: Context, keys: String },
    #[error("看不懂的按键：{0}")]
    BadKey(String),
}

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
        Keymap::new(crate::defaults::bindings()).expect("内置键位表没有冲突")
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
    pub fn hints(&self, ctx: Context) -> Vec<(String, &'static str)> {
        crate::defaults::footer(ctx)
            .iter()
            .filter_map(|a| self.key_for(ctx, *a).map(|k| (k, a.help())))
            .collect()
    }

    /// 底栏最右边永远在的三项：切栏、退出、帮助（需求 L-07）。取实际绑定，用户改了键也跟着变。
    pub fn essential(&self) -> [(String, &'static str); 3] {
        let k = |a: Action| {
            self.key_for(Context::Global, a)
                .unwrap_or_else(|| "?".into())
        };
        [
            (k(Action::PaneToggle), Action::PaneToggle.help()),
            (k(Action::Exit), Action::Exit.help()),
            (k(Action::Help), Action::Help.help()),
        ]
    }

    /// 帮助浮层：某上下文与全局的全部绑定。
    pub fn listing(&self, ctx: Context) -> Vec<(Context, String, &'static str)> {
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
