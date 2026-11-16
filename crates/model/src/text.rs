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

/// 简化的 markdown：标题、列表、代码块、行内代码、粗体。足以让回答读起来像 Claude Code。
pub(crate) fn markdown(text: &str, width: usize, first: &Line, cont: &Line) -> Vec<Line> {
    let mut out = vec![];
    let mut in_code = false;
    for (i, raw) in text.split('\n').enumerate() {
        let lead = if i == 0 { first } else { cont };
        let line = raw.trim_end_matches('\r');
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            if i == 0 {
                out.push(first.clone());
            }
            continue;
        }
        if in_code {
            out.extend(wrap(
                &[Span {
                    text: line.to_string(),
                    tone: Tone::Code,
                }],
                width,
                lead,
                cont,
            ));
            continue;
        }
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        let (marker, body, base) = if let Some(h) = trimmed.strip_prefix("#") {
            ("", h.trim_start_matches('#').trim_start(), Tone::Strong)
        } else if let Some(b) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ("• ", b, Tone::Normal)
        } else {
            ("", trimmed, Tone::Normal)
        };
        let mut spans = vec![];
        if !indent.is_empty() || !marker.is_empty() {
            spans.push(Span {
                text: format!("{indent}{marker}"),
                tone: Tone::Normal,
            });
        }
        spans.extend(inline(body, base));
        out.extend(wrap(&spans, width, lead, cont));
    }
    out
}

/// 行内的 `code` 与 **粗体**。
fn inline(text: &str, base: Tone) -> Vec<Span> {
    let mut out = vec![];
    let mut buf = String::new();
    let mut tone = base;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let toggle = if c == '`' {
            Some(Tone::Code)
        } else if c == '*' && chars.peek() == Some(&'*') {
            chars.next();
            Some(Tone::Strong)
        } else {
            None
        };
        match toggle {
            Some(t) => {
                if !buf.is_empty() {
                    out.push(Span {
                        text: std::mem::take(&mut buf),
                        tone,
                    });
                }
                tone = if tone == t { base } else { t };
            }
            None => buf.push(c),
        }
    }
    if !buf.is_empty() {
        out.push(Span { text: buf, tone });
    }
    out
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
    fn markdown_bullets_code_and_inline() {
        let md = "# 标题\n- 一项 `x`\n```\nfn a() {}\n```\n**粗**";
        let ls = markdown(md, 40, &Line::plain("⏺ "), &Line::plain("  "));
        let t = texts(&ls);
        assert_eq!(t[0], "⏺ 标题");
        assert_eq!(t[1], "  • 一项 x");
        assert_eq!(t[2], "  fn a() {}");
        assert_eq!(t[3], "  粗");
        assert!(ls[1].spans.iter().any(|s| s.tone == Tone::Code));
        assert!(
            ls[2]
                .spans
                .iter()
                .all(|s| s.text.trim().is_empty() || s.tone == Tone::Code)
        );
    }
}
