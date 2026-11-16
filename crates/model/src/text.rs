//! 排版：按显示宽度折行（CJK 两列、emoji 两列、控制字符不占宽）、简化的 markdown。
//! 转录里"一条 entry 就是屏幕上的一行"由这里保证：输出的每个 `Line` 都不超过给定宽度，
//! 也不含 `\n`（architecture §4.3）。

use tui::{Line, Span, Tone};
use unicode_width::UnicodeWidthChar;

fn char_w(c: char) -> usize {
    if c == '\t' {
        return 4;
    }
    c.width().unwrap_or(0)
}

/// 把一行带语气的文字折成若干行。首行前缀 `first`，续行前缀 `cont`；ASCII 单词尽量不拆。
pub(crate) fn wrap(spans: &[Span], width: usize, first: &Line, cont: &Line) -> Vec<Line> {
    let mut out = vec![];
    let mut cur = first.clone();
    let mut cur_w = first.width();
    let mut avail = width.max(cur_w + 1);
    // 上一个空格之后、当前行里的字（用于不拆单词）。
    let mut word: Vec<(char, Tone)> = vec![];
    let mut word_w = 0;
    let push_char = |line: &mut Line, c: char, tone: Tone| match line.spans.last_mut() {
        Some(s) if s.tone == tone => s.text.push(if c == '\t' { ' ' } else { c }),
        _ => line.spans.push(Span {
            text: if c == '\t' {
                "    ".into()
            } else {
                c.to_string()
            },
            tone,
        }),
    };
    let flush_word = |line: &mut Line, word: &mut Vec<(char, Tone)>| {
        for (c, t) in word.drain(..) {
            push_char(line, c, t);
        }
    };
    for s in spans {
        for c in s.text.chars() {
            if c == '\n' || c == '\r' {
                continue;
            }
            let w = char_w(c);
            let wordish = c.is_ascii_alphanumeric() || "_-./:@#%&=?~".contains(c);
            if cur_w + word_w + w > avail {
                if wordish && !word.is_empty() && word_w + w <= avail.saturating_sub(cont.width()) {
                    // 单词整体挪到下一行。
                    out.push(std::mem::replace(&mut cur, cont.clone()));
                    cur_w = cont.width();
                    avail = width.max(cur_w + 1);
                } else {
                    flush_word(&mut cur, &mut word);
                    out.push(std::mem::replace(&mut cur, cont.clone()));
                    cur_w = cont.width();
                    word_w = 0;
                    avail = width.max(cur_w + 1);
                    if c == ' ' {
                        continue;
                    }
                }
            }
            if wordish {
                word.push((c, s.tone));
                word_w += w;
            } else {
                flush_word(&mut cur, &mut word);
                cur_w += word_w;
                word_w = 0;
                push_char(&mut cur, c, s.tone);
                cur_w += w;
            }
        }
    }
    flush_word(&mut cur, &mut word);
    out.push(cur);
    out
}

/// 一段纯文字（可能多行）按 `\n` 拆开再各自折行。
pub(crate) fn wrap_text(
    text: &str,
    tone: Tone,
    width: usize,
    first: &Line,
    cont: &Line,
) -> Vec<Line> {
    let mut out = vec![];
    for (i, raw) in text.split('\n').enumerate() {
        let lead = if i == 0 { first } else { cont };
        out.extend(wrap(
            &[Span {
                text: raw.trim_end_matches('\r').to_string(),
                tone,
            }],
            width,
            lead,
            cont,
        ));
    }
    out
}

/// markdown：标题、列表（含嵌套、编号、任务）、引用、分隔线、代码块（带语言）、表格、
/// 行内代码、粗体、斜体、链接。够让回答读起来像 Claude Code。
pub(crate) fn markdown(text: &str, width: usize, first: &Line, cont: &Line) -> Vec<Line> {
    let mut out = vec![];
    let raw: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut i = 0;
    let mut code: Option<String> = None;
    let lead = |out: &Vec<Line>| {
        if out.is_empty() {
            first.clone()
        } else {
            cont.clone()
        }
    };
    while i < raw.len() {
        let line = raw[i];
        let trimmed = line.trim_start();
        if let Some(lang) = trimmed.strip_prefix("```") {
            match code.take() {
                None => {
                    let label = if lang.trim().is_empty() {
                        "代码".to_string()
                    } else {
                        lang.trim().to_string()
                    };
                    out.push(lead(&out).push(format!("┌─ {label}"), Tone::Dim));
                    code = Some(label);
                }
                Some(_) => out.push(lead(&out).push("└─", Tone::Dim)),
            }
            i += 1;
            continue;
        }
        if code.is_some() {
            let bar = lead(&out).push("│ ", Tone::Dim);
            let mut bar_cont = cont.clone();
            bar_cont = bar_cont.push("│ ", Tone::Dim);
            out.extend(wrap(
                &[Span {
                    text: line.to_string(),
                    tone: Tone::Code,
                }],
                width,
                &bar,
                &bar_cont,
            ));
            i += 1;
            continue;
        }
        // 表格：连续的 `|` 行。
        if trimmed.starts_with('|') {
            let mut rows = vec![];
            while i < raw.len() && raw[i].trim_start().starts_with('|') {
                rows.push(raw[i].trim());
                i += 1;
            }
            out.extend(table(&rows, width, &lead(&out), cont));
            continue;
        }
        i += 1;
        if trimmed.is_empty() {
            out.push(lead(&out));
            continue;
        }
        if trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') && trimmed.len() >= 3 {
            let w = width.saturating_sub(cont.width()).min(40);
            out.push(lead(&out).push("─".repeat(w), Tone::Dim));
            continue;
        }
        let indent = line.len() - trimmed.len();
        let pad = " ".repeat(indent);
        let (marker, body, base) = if let Some(h) = trimmed.strip_prefix('#') {
            let level = 1 + h.chars().take_while(|c| *c == '#').count();
            let tone = if level == 1 {
                Tone::Accent
            } else {
                Tone::Strong
            };
            (String::new(), h.trim_start_matches('#').trim_start(), tone)
        } else if let Some(q) = trimmed.strip_prefix('>') {
            ("▎ ".to_string(), q.trim_start(), Tone::Italic)
        } else if let Some(t) = trimmed
            .strip_prefix("- [x] ")
            .or_else(|| trimmed.strip_prefix("- [X] "))
        {
            (format!("{pad}☒ "), t, Tone::Dim)
        } else if let Some(t) = trimmed.strip_prefix("- [ ] ") {
            (format!("{pad}☐ "), t, Tone::Normal)
        } else if let Some(b) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let dot = if indent >= 2 { "◦ " } else { "• " };
            (format!("{pad}{dot}"), b, Tone::Normal)
        } else if let Some((n, rest)) = numbered(trimmed) {
            (format!("{pad}{n}. "), rest, Tone::Normal)
        } else {
            (pad.clone(), trimmed, Tone::Normal)
        };
        let marker_tone = if marker.starts_with('▎') {
            Tone::Dim
        } else {
            Tone::Normal
        };
        let first_line = lead(&out).push(marker.clone(), marker_tone);
        let hang = cont
            .clone()
            .push(" ".repeat(tui::display_width(&marker)), Tone::Normal);
        let hang = if marker.starts_with('▎') {
            cont.clone().push("▎ ", Tone::Dim)
        } else {
            hang
        };
        out.extend(wrap(&inline(body, base), width, &first_line, &hang));
    }
    out
}

fn numbered(s: &str) -> Option<(&str, &str)> {
    let dot = s.find(". ")?;
    let n = &s[..dot];
    (!n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit()))
        .then(|| (n, &s[dot + 2..]))
}

/// GitHub 表格：按显示宽度对齐列；放不下就原样折行。
fn table(rows: &[&str], width: usize, first: &Line, cont: &Line) -> Vec<Line> {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .filter(|r| {
            !r.trim_matches(|c| c == '|' || c == '-' || c == ':' || c == ' ')
                .is_empty()
        })
        .map(|r| {
            r.trim()
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect();
    let cols = cells.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for r in &cells {
        for (k, c) in r.iter().enumerate() {
            widths[k] = widths[k].max(tui::display_width(&strip_inline(c)));
        }
    }
    let total: usize = widths.iter().sum::<usize>() + 3 * cols.saturating_sub(1) + cont.width();
    if total > width || cols == 0 {
        return rows
            .iter()
            .enumerate()
            .flat_map(|(k, r)| {
                let lead = if k == 0 { first } else { cont };
                wrap(
                    &[Span {
                        text: r.to_string(),
                        tone: Tone::Normal,
                    }],
                    width,
                    lead,
                    cont,
                )
            })
            .collect();
    }
    let mut out = vec![];
    for (k, r) in cells.iter().enumerate() {
        let mut line = if k == 0 { first.clone() } else { cont.clone() };
        for (c, w) in widths.iter().enumerate() {
            let text = strip_inline(r.get(c).map(String::as_str).unwrap_or(""));
            let pad = w - tui::display_width(&text);
            let tone = if k == 0 { Tone::Strong } else { Tone::Normal };
            line = line.push(format!("{text}{}", " ".repeat(pad)), tone);
            if c + 1 < cols {
                line = line.push(" │ ", Tone::Dim);
            }
        }
        out.push(line);
        if k == 0 {
            let sep: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            out.push(cont.clone().push(sep.join("─┼─"), Tone::Dim));
        }
    }
    out
}

fn strip_inline(s: &str) -> String {
    s.replace("**", "").replace('`', "")
}

/// 行内的 `code`、**粗体**、*斜体*、[链接](地址)。
fn inline(text: &str, base: Tone) -> Vec<Span> {
    let mut out: Vec<Span> = vec![];
    let mut buf = String::new();
    let mut tone = base;
    let chars: Vec<char> = text.chars().collect();
    let flush = |out: &mut Vec<Span>, buf: &mut String, tone: Tone| {
        if !buf.is_empty() {
            out.push(Span {
                text: std::mem::take(buf),
                tone,
            });
        }
    };
    let mut k = 0;
    while k < chars.len() {
        let c = chars[k];
        if c == '['
            && let Some((label, url, used)) = link(&chars[k..])
        {
            flush(&mut out, &mut buf, tone);
            out.push(Span {
                text: label,
                tone: Tone::Link,
            });
            out.push(Span {
                text: format!(" ({url})"),
                tone: Tone::Dim,
            });
            k += used;
            continue;
        }
        let toggle = match c {
            '`' => Some(Tone::Code),
            '*' if chars.get(k + 1) == Some(&'*') => {
                k += 1;
                Some(Tone::Strong)
            }
            '*' | '_'
                if tone == Tone::Italic || chars.get(k + 1).is_some_and(|n| !n.is_whitespace()) =>
            {
                Some(Tone::Italic)
            }
            _ => None,
        };
        match toggle {
            Some(t) => {
                flush(&mut out, &mut buf, tone);
                tone = if tone == t { base } else { t };
            }
            None => buf.push(c),
        }
        k += 1;
    }
    flush(&mut out, &mut buf, tone);
    out
}

/// `[文字](地址)` → (文字, 地址, 用掉的字符数)。
fn link(chars: &[char]) -> Option<(String, String, usize)> {
    let close = chars.iter().position(|c| *c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = close + 2 + chars[close + 2..].iter().position(|c| *c == ')')?;
    let label: String = chars[1..close].iter().collect();
    let url: String = chars[close + 2..end].iter().collect();
    Some((label, url, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(ls: &[Line]) -> Vec<String> {
        ls.iter().map(Line::text).collect()
    }

    #[test]
    fn never_exceeds_width_and_never_contains_newlines() {
        let s = "重构认证模块需要先读现有的入口，然后 design the new OAuth provider interface carefully。\n第二行";
        for w in [10, 17, 33, 80] {
            let ls = wrap_text(s, Tone::Normal, w, &Line::plain("⏺ "), &Line::plain("  "));
            for l in &ls {
                assert!(l.width() <= w, "{w}: {:?}", l.text());
                assert!(!l.text().contains('\n'));
            }
            assert!(ls.iter().any(|l| l.text().contains("第二行")));
        }
    }

    #[test]
    fn ascii_words_move_whole() {
        let ls = wrap_text(
            "aaa bbbbbb cc",
            Tone::Normal,
            8,
            &Line::default(),
            &Line::default(),
        );
        assert_eq!(texts(&ls), ["aaa ", "bbbbbb ", "cc"]);
    }

    #[test]
    fn markdown_blocks() {
        let md = "# 标题\n- 一项 `x`\n  - 嵌套\n1. 第一\n- [x] 完成\n- [ ] 待做\n> 引用\n---\n```rust\nfn a() {}\n```\n**粗** *斜* [文档](https://e.x)";
        let ls = markdown(md, 60, &Line::plain("⏺ "), &Line::plain("  "));
        let t = texts(&ls);
        assert_eq!(t[0], "⏺ 标题");
        assert_eq!(t[1], "  • 一项 x");
        assert_eq!(t[2], "    ◦ 嵌套");
        assert_eq!(t[3], "  1. 第一");
        assert_eq!(t[4], "  ☒ 完成");
        assert_eq!(t[5], "  ☐ 待做");
        assert_eq!(t[6], "  ▎ 引用");
        assert!(t[7].contains("────"));
        assert_eq!(t[8], "  ┌─ rust");
        assert_eq!(t[9], "  │ fn a() {}");
        assert_eq!(t[10], "  └─");
        assert_eq!(t[11], "  粗 斜 文档 (https://e.x)");
        assert!(ls[11].spans.iter().any(|s| s.tone == Tone::Link));
        assert!(ls[11].spans.iter().any(|s| s.tone == Tone::Italic));
    }

    #[test]
    fn tables_align_by_display_width() {
        let md = "| 方案 | 风险 |\n|---|---|\n| 直接替换 | 高 |\n| 双写 | 低 |";
        let ls = markdown(md, 60, &Line::plain("⏺ "), &Line::plain("  "));
        let t = texts(&ls);
        assert_eq!(t[0], "⏺ 方案     │ 风险");
        assert!(t[1].contains("─┼─"));
        assert_eq!(t[2], "  直接替换 │ 高  ");
        assert_eq!(t.len(), 4);
    }
}
