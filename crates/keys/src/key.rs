use std::fmt;

use crossterm::event::{KeyCode as CtCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyCode {
    Char(char),
    F(u8),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
    };

    pub fn is_empty(self) -> bool {
        self == Mods::NONE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Mods,
}

impl Key {
    pub const fn new(code: KeyCode, mods: Mods) -> Key {
        Key { code, mods }
    }

    pub const fn plain(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    /// 无修饰（或只有 Shift）的可打印字符：在输入框里它就是输入。
    pub fn as_text(self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if !self.mods.ctrl && !self.mods.alt => Some(c),
            _ => None,
        }
    }

    /// `Ctrl+B`、`Alt+Enter`、`F1`、`<` 这种写法；大小写不敏感，`Shift+x` 等于 `X`。
    pub fn parse(s: &str) -> Option<Key> {
        let mut mods = Mods::NONE;
        let parts: Vec<&str> = if s == "+" {
            vec!["+"]
        } else {
            s.split('+').collect()
        };
        let (last, prefix) = parts.split_last()?;
        for p in prefix {
            match p.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "meta" | "option" => mods.alt = true,
                "shift" => mods.shift = true,
                _ => return None,
            }
        }
        let code = match last.to_ascii_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            l if l.len() > 1 && l.starts_with('f') => KeyCode::F(l[1..].parse().ok()?),
            _ => {
                let mut chars = last.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                if mods.shift && c.is_ascii_alphabetic() {
                    mods.shift = false;
                    KeyCode::Char(c.to_ascii_uppercase())
                } else if mods.ctrl || mods.alt {
                    KeyCode::Char(c.to_ascii_lowercase())
                } else {
                    KeyCode::Char(c)
                }
            }
        };
        Some(Key { code, mods })
    }

    pub fn from_crossterm(ev: KeyEvent) -> Option<Key> {
        let m = ev.modifiers;
        let mut mods = Mods {
            ctrl: m.contains(KeyModifiers::CONTROL),
            alt: m.contains(KeyModifiers::ALT),
            shift: m.contains(KeyModifiers::SHIFT),
        };
        let code = match ev.code {
            CtCode::Char(c) => {
                // 字符本身已带上 Shift 的结果（`A`、`<`），再记一次 Shift 会让绑定对不上。
                mods.shift = false;
                if mods.ctrl || mods.alt {
                    KeyCode::Char(c.to_ascii_lowercase())
                } else {
                    KeyCode::Char(c)
                }
            }
            CtCode::F(n) => KeyCode::F(n),
            CtCode::Enter => KeyCode::Enter,
            CtCode::Esc => KeyCode::Esc,
            CtCode::Tab => KeyCode::Tab,
            CtCode::BackTab => {
                mods.shift = false;
                KeyCode::BackTab
            }
            CtCode::Backspace => KeyCode::Backspace,
            CtCode::Delete => KeyCode::Delete,
            CtCode::Insert => KeyCode::Insert,
            CtCode::Up => KeyCode::Up,
            CtCode::Down => KeyCode::Down,
            CtCode::Left => KeyCode::Left,
            CtCode::Right => KeyCode::Right,
            CtCode::Home => KeyCode::Home,
            CtCode::End => KeyCode::End,
            CtCode::PageUp => KeyCode::PageUp,
            CtCode::PageDown => KeyCode::PageDown,
            _ => return None,
        };
        Some(Key { code, mods })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.mods.alt {
            f.write_str("Alt+")?;
        }
        if self.mods.shift {
            f.write_str("Shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str(&i18n::t!("keys.space")),
            KeyCode::Char(c) if self.mods.ctrl || self.mods.alt => {
                write!(f, "{}", c.to_ascii_uppercase())
            }
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::F(n) => write!(f, "F{n}"),
            KeyCode::Enter => f.write_str("Enter"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::BackTab => f.write_str("Shift+Tab"),
            KeyCode::Backspace => f.write_str("Backspace"),
            KeyCode::Delete => f.write_str("Delete"),
            KeyCode::Insert => f.write_str("Insert"),
            KeyCode::Up => f.write_str("↑"),
            KeyCode::Down => f.write_str("↓"),
            KeyCode::Left => f.write_str("←"),
            KeyCode::Right => f.write_str("→"),
            KeyCode::Home => f.write_str("Home"),
            KeyCode::End => f.write_str("End"),
            KeyCode::PageUp => f.write_str("PageUp"),
            KeyCode::PageDown => f.write_str("PageDown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_through_display() {
        for s in [
            "Ctrl+B",
            "F1",
            "Alt+1",
            "Esc",
            "<",
            "Shift+Enter",
            "R",
            "空格",
        ] {
            let s = if s == "空格" { "Space" } else { s };
            let k = Key::parse(s).unwrap_or_else(|| panic!("{s}"));
            let back = Key::parse(&k.to_string().replace("空格", "Space")).unwrap();
            assert_eq!(k, back, "{s}");
        }
        assert_eq!(Key::parse("shift+r"), Key::parse("R"));
        assert_eq!(Key::parse("ctrl+b"), Key::parse("Ctrl+B"));
    }

    #[test]
    fn crossterm_chars_drop_shift() {
        let ev = KeyEvent::new(CtCode::Char('<'), KeyModifiers::SHIFT);
        assert_eq!(Key::from_crossterm(ev), Key::parse("<"));
        let ev = KeyEvent::new(
            CtCode::Char('B'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(Key::from_crossterm(ev), Key::parse("Ctrl+b"));
    }
}
