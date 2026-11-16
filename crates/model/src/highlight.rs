//! 代码块语法高亮（照 Claude Code）：syntect 的语法与主题按需加载一次，之后复用。
//! 只在块重排时调用，所以流式输出时的开销仍只跟着那一个块走。

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use tui::{Span, Tone};

pub const DEFAULT_THEME: &str = "base16-ocean.dark";

fn sets() -> &'static (SyntaxSet, ThemeSet) {
    static SETS: OnceLock<(SyntaxSet, ThemeSet)> = OnceLock::new();
    SETS.get_or_init(|| {
        (
            SyntaxSet::load_defaults_newlines(),
            ThemeSet::load_defaults(),
        )
    })
}

fn theme(name: &str) -> &'static Theme {
    let ts = &sets().1;
    ts.themes
        .get(name)
        .or_else(|| ts.themes.get(DEFAULT_THEME))
        .expect("syntect 自带 base16-ocean.dark")
}

/// 可用的高亮主题名（给配置说明与 /doctor 用）。
pub fn theme_names() -> Vec<String> {
    sets().1.themes.keys().cloned().collect()
}

/// 把一个代码块的每一行切成带颜色的片段。语言不认识时返回 `None`，调用方用统一的代码色。
pub(crate) fn highlight(lang: &str, lines: &[&str], theme_name: &str) -> Option<Vec<Vec<Span>>> {
    let (ss, _) = sets();
    let syntax = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))?;
    let mut h = HighlightLines::new(syntax, theme(theme_name));
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let with_nl = format!("{line}\n");
        let ranges = h.highlight_line(&with_nl, ss).ok()?;
        out.push(
            ranges
                .into_iter()
                .map(|(style, text)| Span {
                    text: text.trim_end_matches('\n').to_string(),
                    tone: Tone::Rgb(style.foreground.r, style.foreground.g, style.foreground.b),
                })
                .filter(|s| !s.text.is_empty())
                .collect(),
        );
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_gets_colours_and_unknown_language_does_not() {
        let out = highlight("rust", &["fn main() { let x = 1; }"], DEFAULT_THEME).unwrap();
        let tones: std::collections::HashSet<_> = out[0].iter().map(|s| s.tone).collect();
        assert!(tones.len() > 1, "关键字与标识符颜色不同");
        let text: String = out[0].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "fn main() { let x = 1; }");
        assert!(highlight("no-such-lang", &["x"], DEFAULT_THEME).is_none());
    }
}
