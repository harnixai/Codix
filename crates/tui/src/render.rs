use ratatui::Frame;
use ratatui::layout::Rect as RRect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as RLine, Span as RSpan};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::frame::*;
use crate::layout::{self, Rect, Regions, Size};
use crate::text::{display_width, truncate_to_width};

pub fn render(f: &mut Frame, state: &FrameState) {
    let area = f.area();
    let size = Size::new(area.width, area.height);
    match &state.body {
        Body::Workbench(wb) => workbench(f, size, wb),
        Body::Instances(s) => instances(f, size, s),
        Body::Detached(s) => centered_lines(f, size, &s.lines),
        Body::TooSmall { need_cols } => centered_lines(
            f,
            size,
            &[Line::toned(
                format!("终端太小（需要至少 {need_cols} 列）"),
                Tone::Warn,
            )],
        ),
    }
    footer(f, size, &state.footer);
    if let Some(o) = &state.overlay {
        overlay(f, size, o);
    }
}

fn rr(r: Rect) -> RRect {
    RRect::new(r.x, r.y, r.w, r.h)
}

fn style(t: Tone) -> Style {
    let s = Style::default();
    match t {
        Tone::Normal => s,
        Tone::Dim => s.fg(Color::DarkGray),
        Tone::Strong => s.add_modifier(Modifier::BOLD),
        Tone::Accent => s.fg(Color::Cyan),
        Tone::Ok => s.fg(Color::Green),
        Tone::Warn => s.fg(Color::Yellow),
        Tone::Error => s.fg(Color::Red),
        Tone::Add => s.fg(Color::Green),
        Tone::Del => s.fg(Color::Red),
        Tone::Code => s.fg(Color::LightBlue),
        Tone::Selected => s.add_modifier(Modifier::REVERSED),
        Tone::Attention => s.fg(Color::Yellow).add_modifier(Modifier::BOLD),
    }
}

fn rline(l: &Line) -> RLine<'static> {
    RLine::from(
        l.spans
            .iter()
            .map(|s| RSpan::styled(s.text.clone(), style(s.tone)))
            .collect::<Vec<_>>(),
    )
    .style(style(l.tone))
}

/// 一行一行放进矩形，超出的截掉；渲染端从不折行（architecture §4.3）。
fn lines(f: &mut Frame, r: Rect, ls: &[Line], selected: Option<usize>) {
    for (i, l) in ls.iter().take(r.h as usize).enumerate() {
        let mut line = rline(l);
        if selected == Some(i) {
            line = line.style(style(Tone::Selected));
        }
        let row = Rect::new(r.x, r.y + i as u16, r.w, 1);
        f.render_widget(Paragraph::new(line), rr(row));
    }
}

fn workbench(f: &mut Frame, size: Size, wb: &Workbench) {
    let labels: Vec<String> = wb
        .side
        .as_ref()
        .map(|s| s.tabs.iter().map(TabLabel::display).collect())
        .unwrap_or_default();
    let regions: Regions = layout::compute(size, &wb.layout, &labels);
    header(f, regions.header, &wb.header);
    if let (Some(c), Some(pane)) = (regions.chat, &wb.chat) {
        chat(f, &c, pane);
    }
    if let Some(d) = regions.divider {
        let bar: Vec<Line> = (0..d.h).map(|_| Line::toned("┃", Tone::Dim)).collect();
        lines(f, d, &bar, None);
    }
    if let (Some(s), Some(pane)) = (&regions.side, &wb.side) {
        side(f, s, pane);
    }
}

fn header(f: &mut Frame, r: Rect, h: &Header) {
    let rw = h.right.width() as u16;
    lines(f, r, std::slice::from_ref(&h.left), None);
    if rw < r.w {
        lines(
            f,
            Rect::new(r.right() - rw - 1, r.y, rw + 1, 1),
            std::slice::from_ref(&h.right),
            None,
        );
    }
}

fn chat(f: &mut Frame, c: &crate::layout::ChatRegions, p: &ChatPane) {
    let crumbs = if p.focused {
        p.crumbs.clone().with_tone(Tone::Strong)
    } else {
        p.crumbs.clone()
    };
    lines(f, c.crumbs, &[crumbs], None);
    lines(f, c.owner, std::slice::from_ref(&p.owner), None);
    lines(f, c.transcript, &p.transcript, None);
    if let Some(below) = &p.below
        && c.transcript.h > 0
    {
        let w = below.width() as u16;
        let r = Rect::new(
            c.transcript.right().saturating_sub(w + 1),
            c.transcript.bottom() - 1,
            w.min(c.transcript.w),
            1,
        );
        lines(f, r, std::slice::from_ref(below), None);
    }
    if let (Some(r), Some(s)) = (c.status, &p.status) {
        lines(f, r, std::slice::from_ref(s), None);
    }
    let border = if p.composer.focused {
        style(Tone::Accent)
    } else {
        style(Tone::Dim)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border);
    if let Some(t) = &p.composer.title {
        block = block.title(rline(t));
    }
    f.render_widget(block, rr(c.composer));
    lines(f, c.composer_inner, &p.composer.lines, None);
    if p.composer.focused
        && let Some((x, y)) = p.composer.cursor
        && x < c.composer_inner.w
        && y < c.composer_inner.h
    {
        f.set_cursor_position((c.composer_inner.x + x, c.composer_inner.y + y));
    }
}

fn side(f: &mut Frame, s: &crate::layout::SideRegions, p: &SidePane) {
    for (tab, hit) in p.tabs.iter().zip(&s.tab_hits) {
        let tone = if tab.active {
            Tone::Selected
        } else if tab.badge > 0 {
            Tone::Attention
        } else {
            Tone::Dim
        };
        lines(f, *hit, &[Line::toned(tab.display(), tone)], None);
    }
    let sel = |sec: &Section| if sec.focused { sec.selected } else { None };
    lines(f, s.upper, &p.upper.rows, sel(&p.upper));
    if let (Some(i), false) = (p.upper.selected, p.upper.focused) {
        dim_marker(f, s.upper, i);
    }
    let title = p.lower.title.clone().unwrap_or_default();
    let tone = if p.lower.focused || p.upper.focused {
        Tone::Accent
    } else {
        Tone::Dim
    };
    let mut rule = format!("──── {title} ");
    let fill = (s.rule.w as usize).saturating_sub(display_width(&rule));
    rule.push_str(&"─".repeat(fill));
    lines(
        f,
        s.rule,
        &[Line::toned(
            truncate_to_width(&rule, s.rule.w as usize),
            tone,
        )],
        None,
    );
    lines(f, s.lower, &p.lower.rows, sel(&p.lower));
    if let (Some(i), false) = (p.lower.selected, p.lower.focused) {
        dim_marker(f, s.lower, i);
    }
}

/// 没有焦点的一段仍标出选中项：在行首画一个暗的 `›`，不反色。
fn dim_marker(f: &mut Frame, r: Rect, i: usize) {
    if (i as u16) < r.h {
        lines(
            f,
            Rect::new(r.x, r.y + i as u16, 1, 1),
            &[Line::toned("›", Tone::Dim)],
            None,
        );
    }
}

fn footer(f: &mut Frame, size: Size, ft: &Footer) {
    let r = Rect::new(0, size.rows.saturating_sub(1), size.cols, 1.min(size.rows));
    let essential = ft
        .essential
        .iter()
        .map(|h| format!("{} {}", h.key, h.label))
        .collect::<Vec<_>>()
        .join(" · ");
    let ew = display_width(&essential) as u16;
    let left_w = r.w.saturating_sub(ew + 3);
    let left = match &ft.message {
        Some(m) => m.clone(),
        None => {
            let mut text = String::new();
            for h in &ft.hints {
                let item = format!("{} {}", h.key, h.label);
                let sep = if text.is_empty() { "" } else { " · " };
                if display_width(&text) + display_width(sep) + display_width(&item)
                    > left_w as usize
                {
                    break;
                }
                text.push_str(sep);
                text.push_str(&item);
            }
            Line::toned(text, Tone::Dim)
        }
    };
    let left = Line::toned(
        truncate_to_width(&left.text(), left_w as usize),
        left.spans.first().map(|s| s.tone).unwrap_or_default(),
    );
    lines(f, Rect::new(r.x, r.y, left_w, 1), &[left], None);
    if ew <= r.w {
        lines(
            f,
            Rect::new(r.right() - ew, r.y, ew, 1),
            &[Line::toned(essential, Tone::Strong)],
            None,
        );
    }
}

fn overlay(f: &mut Frame, size: Size, o: &Overlay) {
    let w = o.width.min(size.cols.saturating_sub(2)).max(10);
    let h = o.height.min(size.rows.saturating_sub(2)).max(3);
    let r = Rect::new((size.cols - w) / 2, (size.rows - h) / 2, w, h);
    f.render_widget(Clear, rr(r));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style(Tone::Accent))
        .title(RLine::from(format!(" {} ", o.title)));
    f.render_widget(block, rr(r));
    let inner = Rect::new(
        r.x + 2,
        r.y + 1,
        r.w.saturating_sub(4),
        r.h.saturating_sub(2),
    );
    lines(f, inner, &o.lines, None);
}

fn instances(f: &mut Frame, size: Size, s: &InstancesScreen) {
    lines(
        f,
        Rect::new(0, 0, size.cols, 1),
        std::slice::from_ref(&s.title),
        None,
    );
    let list = Rect::new(
        2,
        2,
        size.cols.saturating_sub(4),
        size.rows.saturating_sub(5),
    );
    lines(f, list, &s.rows, s.selected);
    if let Some(m) = &s.message {
        lines(
            f,
            Rect::new(
                2,
                size.rows.saturating_sub(3),
                size.cols.saturating_sub(4),
                1,
            ),
            std::slice::from_ref(m),
            None,
        );
    }
}

fn centered_lines(f: &mut Frame, size: Size, ls: &[Line]) {
    let h = ls.len() as u16;
    let top = size.rows.saturating_sub(1).saturating_sub(h) / 2;
    for (i, l) in ls.iter().enumerate() {
        let w = (l.width() as u16).min(size.cols);
        let r = Rect::new((size.cols - w) / 2, top + i as u16, w, 1);
        lines(f, r, std::slice::from_ref(l), None);
    }
}
