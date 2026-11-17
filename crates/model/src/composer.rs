//! 输入框（交互 §4.4，照 Claude Code）：多行编辑、历史、大段粘贴的占位、`!` 命令模式、
//! 按宽度排版与光标位置。光标是字节下标，始终落在字符边界上。

use tui::{Line, Tone};
use unicode_width::UnicodeWidthChar;

const PASTE_FOLD_LINES: usize = 10;

#[derive(Debug, Clone, Default)]
pub struct ComposerState {
    pub text: String,
    pub cursor: usize,
    pub history: Vec<String>,
    /// 正在翻的历史下标；翻之前的草稿存在 `stash`。
    pub hist: Option<usize>,
    pub stash: String,
    /// `!` 命令模式（与 Claude Code 一致）。
    pub bash: bool,
    /// 大段粘贴：框里显示占位，提交时展开。
    pub pastes: Vec<String>,
    /// 补全列表的选中项；`picker_closed` 表示按过 Esc，直到文字再变。
    pub picker: usize,
    pub picker_closed: bool,
}

fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

fn next_boundary(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(i, |c| i + c.len_utf8())
}

impl ComposerState {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn edited(&mut self) {
        self.hist = None;
        self.picker = 0;
        self.picker_closed = false;
    }

    pub fn insert(&mut self, c: char) {
        if c == '!' && self.text.is_empty() && !self.bash {
            self.bash = true;
            return;
        }
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.edited();
    }

    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.edited();
    }

    pub fn paste(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        let lines = s.lines().count();
        if lines > PASTE_FOLD_LINES {
            self.pastes.push(s);
            let token = i18n::t!("composer.paste_token", lines = lines, n = self.pastes.len());
            self.insert_str(&token);
        } else {
            self.insert_str(&s);
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            if self.text.is_empty() && self.bash {
                self.bash = false;
            }
            return;
        }
        let p = prev_boundary(&self.text, self.cursor);
        self.text.replace_range(p..self.cursor, "");
        self.cursor = p;
        self.edited();
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            let n = next_boundary(&self.text, self.cursor);
            self.text.replace_range(self.cursor..n, "");
            self.edited();
        }
    }

    pub fn left(&mut self) {
        self.cursor = prev_boundary(&self.text, self.cursor);
    }

    pub fn right(&mut self) {
        self.cursor = next_boundary(&self.text, self.cursor);
    }

    pub fn word_left(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        self.cursor = trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
    }

    pub fn word_right(&mut self) {
        let after = &self.text[self.cursor..];
        let skip_ws = after.len() - after.trim_start().len();
        let rest = &after[skip_ws..];
        let word = rest.find(char::is_whitespace).unwrap_or(rest.len());
        self.cursor += skip_ws + word;
    }

    fn line_bounds(&self) -> (usize, usize) {
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
        (start, end)
    }

    pub fn line_start(&mut self) {
        self.cursor = self.line_bounds().0;
    }

    pub fn line_end(&mut self) {
        self.cursor = self.line_bounds().1;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.pastes.clear();
        self.bash = false;
        self.edited();
    }

    pub fn delete_word(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.text.replace_range(self.cursor..end, "");
        self.edited();
    }

    pub fn kill_to_end(&mut self) {
        let (_, end) = self.line_bounds();
        let end = if end == self.cursor && end < self.text.len() {
            end + 1
        } else {
            end
        };
        self.text.replace_range(self.cursor..end, "");
        self.edited();
    }

    /// ↑：多行时先在行间移动，已在第一行就翻历史（与 Claude Code 一致）。
    pub fn up(&mut self) {
        let (start, _) = self.line_bounds();
        if start > 0 {
            let col = self.text[start..self.cursor].chars().count();
            let prev_start = self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
            self.cursor = col_to_byte(&self.text, prev_start, start - 1, col);
            return;
        }
        if self.history.is_empty() {
            return;
        }
        let i = match self.hist {
            None => {
                self.stash = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.hist = Some(i);
        self.text = self.history[i].clone();
        self.cursor = self.text.len();
    }

    pub fn down(&mut self) {
        let (_, end) = self.line_bounds();
        if end < self.text.len() {
            let (start, _) = self.line_bounds();
            let col = self.text[start..self.cursor].chars().count();
            let next_end = self.text[end + 1..]
                .find('\n')
                .map_or(self.text.len(), |i| end + 1 + i);
            self.cursor = col_to_byte(&self.text, end + 1, next_end, col);
            return;
        }
        match self.hist {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.hist = Some(i + 1);
                self.text = self.history[i + 1].clone();
                self.cursor = self.text.len();
            }
            Some(_) => {
                self.hist = None;
                self.text = std::mem::take(&mut self.stash);
                self.cursor = self.text.len();
            }
        }
    }

    /// 取出要提交的文字（占位展开），记进历史，清空输入框。
    pub fn take(&mut self) -> (String, bool) {
        let mut text = self.text.clone();
        for (i, p) in self.pastes.iter().enumerate() {
            let lines = p.lines().count();
            text = text.replace(
                &i18n::t!("composer.paste_token", lines = lines, n = i + 1),
                p,
            );
        }
        let bash = self.bash;
        if !self.text.trim().is_empty() && self.history.last() != Some(&self.text) {
            self.history.push(self.text.clone());
        }
        self.clear();
        (text, bash)
    }

    /// 排版：返回各行与光标位置（列、行）。空的时候显示淡色占位。
    pub fn layout(&self, width: usize, placeholder: &str) -> (Vec<Line>, (u16, u16)) {
        let prompt = if self.bash { "! " } else { "> " };
        let prompt_tone = if self.bash { Tone::Warn } else { Tone::Strong };
        if self.text.is_empty() {
            let line = Line::toned(prompt, prompt_tone).push(placeholder, Tone::Dim);
            return (vec![line], (2, 0));
        }
        let w = width.max(4);
        let mut lines = vec![Line::toned(prompt, prompt_tone)];
        let (mut col, mut row) = (2usize, 0usize);
        let mut cursor = (2u16, 0u16);
        for (i, c) in self.text.char_indices() {
            if i == self.cursor {
                cursor = (col as u16, row as u16);
            }
            if c == '\n' {
                lines.push(Line::plain("  "));
                row += 1;
                col = 2;
                continue;
            }
            let cw = c.width().unwrap_or(0);
            if col + cw > w {
                lines.push(Line::plain("  "));
                row += 1;
                col = 2;
            }
            let last = lines.last_mut().unwrap();
            *last = std::mem::take(last).push(c.to_string(), Tone::Normal);
            col += cw;
        }
        if self.cursor >= self.text.len() {
            if col >= w {
                lines.push(Line::plain("  "));
                row += 1;
                col = 2;
            }
            cursor = (col as u16, row as u16);
        }
        (merge(lines), cursor)
    }
}

fn col_to_byte(s: &str, start: usize, end: usize, col: usize) -> usize {
    s[start..end]
        .char_indices()
        .nth(col)
        .map_or(end, |(i, _)| start + i)
}

/// 把同语气的相邻片段并起来，免得一个字一个 Span。
fn merge(lines: Vec<Line>) -> Vec<Line> {
    lines
        .into_iter()
        .map(|l| {
            let mut out = Line {
                spans: vec![],
                tone: l.tone,
            };
            for s in l.spans {
                match out.spans.last_mut() {
                    Some(last) if last.tone == s.tone => last.text.push_str(&s.text),
                    _ => out.spans.push(s),
                }
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> ComposerState {
        let mut c = ComposerState::default();
        for ch in s.chars() {
            c.insert(ch);
        }
        c
    }

    #[test]
    fn editing_respects_char_boundaries() {
        let mut c = typed("重构 auth");
        for _ in 0..5 {
            c.left();
        }
        c.backspace();
        assert_eq!(c.text, "重 auth");
        c.word_right();
        assert_eq!(c.cursor, c.text.len());
        c.delete_word();
        assert_eq!(c.text, "重 ");
    }

    #[test]
    fn history_and_stash() {
        let mut c = typed("one");
        c.take();
        c.insert_str("two");
        c.take();
        c.insert_str("draft");
        c.up();
        assert_eq!(c.text, "two");
        c.up();
        assert_eq!(c.text, "one");
        c.down();
        c.down();
        assert_eq!(c.text, "draft", "翻回来时恢复草稿");
    }

    #[test]
    fn up_moves_between_lines_before_history() {
        let mut c = typed("ab");
        c.take();
        c.insert_str("12\n34");
        c.up();
        assert_eq!(c.text, "12\n34", "多行时先在行间移动");
        c.up();
        assert_eq!(c.text, "ab");
    }

    #[test]
    fn bang_enters_bash_mode_and_backspace_leaves_it() {
        let mut c = typed("!ls");
        assert!(c.bash);
        assert_eq!(c.text, "ls");
        let (lines, _) = c.layout(20, "");
        assert!(lines[0].text().starts_with("! "));
        c.clear();
        c.insert('!');
        c.backspace();
        assert!(!c.bash);
    }

    #[test]
    fn big_paste_folds_and_expands_on_submit() {
        let mut c = ComposerState::default();
        let big = (1..=12)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        c.paste(&big);
        assert_eq!(c.text, "[粘贴了 12 行 #1]");
        let (text, _) = c.take();
        assert_eq!(text, big);
    }

    #[test]
    fn layout_wraps_and_places_cursor() {
        let c = typed("重构认证模块");
        let (lines, cur) = c.layout(10, "");
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.width() <= 10));
        assert_eq!(cur, (6, 1));
    }
}
