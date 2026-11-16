use crate::keymap::Lookup;
use crate::{Action, Context, Key, Keymap};

/// 和弦超时：超时后已按下的前缀作废，当前这个键重新解析。
const CHORD_TIMEOUT_MS: u64 = 1_500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Action(Action),
    /// 输入框里的普通字符。
    Text(char),
    /// 和弦的前缀，等下一个键。
    Pending,
    Unbound,
}

#[derive(Debug, Default)]
pub struct Resolver {
    pending: Vec<Key>,
    since_ms: u64,
}

impl Resolver {
    /// `contexts` 从最具体到最宽，`Global` 总是在最后查一次（不必传）。
    pub fn resolve(
        &mut self,
        keymap: &Keymap,
        contexts: &[Context],
        key: Key,
        now_ms: u64,
    ) -> Resolution {
        if !self.pending.is_empty() && now_ms.saturating_sub(self.since_ms) > CHORD_TIMEOUT_MS {
            self.pending.clear();
        }
        let mut seq = self.pending.clone();
        seq.push(key);
        let mut any_prefix = false;
        for ctx in contexts
            .iter()
            .copied()
            .chain(std::iter::once(Context::Global))
        {
            match keymap.lookup(ctx, &seq) {
                Lookup::Exact(a) => {
                    self.pending.clear();
                    return Resolution::Action(a);
                }
                Lookup::Prefix => any_prefix = true,
                Lookup::None => {}
            }
            // 输入框里没被输入框自己绑定的可打印字符就是输入，不再让全局去抢。
            if ctx == Context::Composer
                && seq.len() == 1
                && let Some(c) = key.as_text()
            {
                return Resolution::Text(c);
            }
        }
        if any_prefix {
            if self.pending.is_empty() {
                self.since_ms = now_ms;
            }
            self.pending = seq;
            return Resolution::Pending;
        }
        let had_prefix = !self.pending.is_empty();
        self.pending.clear();
        if had_prefix {
            return self.resolve(keymap, contexts, key, now_ms);
        }
        Resolution::Unbound
    }

    pub fn pending(&self) -> &[Key] {
        &self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Binding, KeymapError};

    fn k(s: &str) -> Key {
        Key::parse(s).unwrap()
    }

    #[test]
    fn default_keymap_has_no_conflicts_and_every_global_has_help() {
        let km = Keymap::default();
        for b in km.bindings() {
            assert!(!b.action.help().is_empty());
        }
        let [toggle, help] = km.essential();
        assert_eq!(toggle.0, "Ctrl+B");
        assert_eq!(help.0, "F1");
    }

    #[test]
    fn composer_text_is_not_stolen_by_list_letters() {
        let km = Keymap::default();
        let mut r = Resolver::default();
        assert_eq!(
            r.resolve(&km, &[Context::Composer], k("n"), 0),
            Resolution::Text('n')
        );
        assert_eq!(
            r.resolve(&km, &[Context::Sessions], k("n"), 0),
            Resolution::Action(Action::New)
        );
        assert_eq!(
            r.resolve(&km, &[Context::Composer], k("Ctrl+B"), 0),
            Resolution::Action(Action::PaneToggle)
        );
    }

    #[test]
    fn specific_context_wins_over_global() {
        let km = Keymap::default();
        let mut r = Resolver::default();
        assert_eq!(
            r.resolve(&km, &[Context::Sessions], k("?"), 0),
            Resolution::Action(Action::Help)
        );
        assert_eq!(
            r.resolve(&km, &[Context::Dialog, Context::Composer], k("Enter"), 0),
            Resolution::Action(Action::Confirm)
        );
    }

    #[test]
    fn chords_wait_then_time_out() {
        let km = Keymap::new(vec![
            Binding {
                context: Context::Sessions,
                keys: vec![k("g"), k("g")],
                action: Action::Top,
            },
            Binding {
                context: Context::Sessions,
                keys: vec![k("j")],
                action: Action::Down,
            },
        ])
        .unwrap();
        let mut r = Resolver::default();
        assert_eq!(
            r.resolve(&km, &[Context::Sessions], k("g"), 0),
            Resolution::Pending
        );
        assert_eq!(
            r.resolve(&km, &[Context::Sessions], k("g"), 100),
            Resolution::Action(Action::Top)
        );
        assert_eq!(
            r.resolve(&km, &[Context::Sessions], k("g"), 200),
            Resolution::Pending
        );
        assert_eq!(
            r.resolve(&km, &[Context::Sessions], k("j"), 5_000),
            Resolution::Action(Action::Down),
            "超时后前缀作废，j 照常解析"
        );
    }

    #[test]
    fn conflicts_are_reported() {
        let err = Keymap::new(vec![
            Binding {
                context: Context::Jobs,
                keys: vec![k("d")],
                action: Action::Delete,
            },
            Binding {
                context: Context::Jobs,
                keys: vec![k("d")],
                action: Action::Edit,
            },
        ])
        .unwrap_err();
        assert!(matches!(err, KeymapError::Conflict { .. }));
    }
}
