//! 开场欢迎块（需求 C-18，交互 §4.2）：Codix logo、版本、实例、项目、场景、模型、常用键。

use tui::{Line, Tone, display_width, truncate_to_width};

use crate::state::State;

const LOGO: [&str; 2] = ["▄▀▀ ▄▀▄ █▀▄ █ ▀▄▀", "▀▄▄ ▀▄▀ █▄▀ █ ▄▀▄"];
const VERSION: &str = env!("CARGO_PKG_VERSION");
const BOX_MAX: usize = 60;

pub(crate) fn lines(s: &State, width: usize) -> Vec<Line> {
    let w = width.min(BOX_MAX);
    if w < 24 {
        return vec![Line::toned(format!("Codix {VERSION}"), Tone::Accent)];
    }
    let inner = w - 4;
    let mut body: Vec<Line> = vec![
        Line::toned(LOGO[0], Tone::Accent),
        Line::toned(LOGO[1], Tone::Accent).push(format!("   Codix {VERSION}"), Tone::Strong),
        Line::default(),
    ];
    if let Some(i) = &s.domain.instance {
        body.push(Line::toned(
            format!(
                "实例 {}@{} · 项目 {}",
                i.name,
                i.host,
                i.project.as_deref().unwrap_or("无")
            ),
            Tone::Dim,
        ));
    }
    body.extend([
        Line::default(),
        Line::toned("/help 帮助 · / 命令 · @ 提及文件 · ! 执行命令", Tone::Dim),
        Line::toned("F2 会话 · F3 流程 · F4 作业 · Ctrl+B 切栏", Tone::Dim),
    ]);
    let mut out = vec![Line::toned(
        format!("╭{}╮", "─".repeat(w - 2)),
        Tone::Accent,
    )];
    for l in body {
        let text_w = l.width();
        let mut row = Line::toned("│ ", Tone::Accent);
        if text_w > inner {
            row = row.push(truncate_to_width(&l.text(), inner), Tone::Dim);
        } else {
            for sp in l.spans {
                row = row.push(sp.text, sp.tone);
            }
            row = row.push(" ".repeat(inner - text_w), Tone::Normal);
        }
        out.push(row.push(" │", Tone::Accent));
    }
    out.push(Line::toned(
        format!("╰{}╯", "─".repeat(w - 2)),
        Tone::Accent,
    ));
    debug_assert!(out.iter().all(|l| display_width(&l.text()) == w));
    out
}
