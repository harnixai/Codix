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

/// 顶栏、底栏、框上沿标题条的底色。有焦点的那一块，标题条换成强调色。
const BAR: Color = Color::Indexed(236);

fn bar(focused: bool) -> Style {
    if focused {
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(BAR).fg(Color::Gray)
    }
}

fn frame_block(focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            style(Tone::Accent)
        } else {
            style(Tone::Dim)
        })
}

/// 框上沿的标题条：左标题从第 2 列起，右标题贴右边，各自带底色。
fn titles(f: &mut Frame, frame: Rect, left: &Line, right: &Line, focused: bool) {
    let room = frame.w.saturating_sub(4) as usize;
    if room == 0 {
        return;
    }
    let paint = |l: &Line| {
        let text = format!(" {} ", l.text());
        RLine::from(RSpan::styled(text, bar(focused)))
    };
    let rw = if right.spans.is_empty() {
        0
    } else {
        (right.width() + 2).min(room / 2)
    };
    let lw = (left.width() + 2).min(room.saturating_sub(rw + 1));
    if lw > 2 {
        let l = Line::plain(truncate_to_width(&left.text(), lw - 2));
        f.render_widget(
            Paragraph::new(paint(&l)),
            rr(Rect::new(frame.x + 2, frame.y, lw as u16, 1)),
        );
    }
    if rw > 2 {
        let r = Line::plain(truncate_to_width(&right.text(), rw - 2));
        f.render_widget(
            Paragraph::new(paint(&r)),
            rr(Rect::new(
                frame.right() - 2 - rw as u16,
                frame.y,
                rw as u16,
                1,
            )),
        );
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
    if let (Some(s), Some(pane)) = (&regions.side, &wb.side) {
        side(f, s, pane);
    }
}

fn header(f: &mut Frame, r: Rect, h: &Header) {
    f.render_widget(Block::default().style(Style::default().bg(BAR)), rr(r));
    let rw = h.right.width() as u16;
    lines(
        f,
        Rect::new(r.x + 1, r.y, r.w.saturating_sub(1), 1),
        std::slice::from_ref(&h.left),
        None,
    );
    if rw + 1 < r.w {
        lines(
            f,
            Rect::new(r.right() - rw - 1, r.y, rw, 1),
            std::slice::from_ref(&h.right),
            None,
        );
    }
}

fn chat(f: &mut Frame, c: &crate::layout::ChatRegions, p: &ChatPane) {
    f.render_widget(frame_block(p.focused), rr(c.frame));
    titles(f, c.frame, &p.crumbs, &p.badge, p.focused);
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
    if let Some(r) = c.todos {
        lines(f, r, &p.todos, None);
    }
    let border = if p.composer.focused {
        style(Tone::Accent)
    } else {
        style(Tone::Dim)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border);
    f.render_widget(block, rr(c.composer));
    lines(f, c.composer_inner, &p.composer.lines, None);
    if p.composer.focused
        && let Some((x, y)) = p.composer.cursor
        && x < c.composer_inner.w
        && y < c.composer_inner.h
    {
        f.set_cursor_position((c.composer_inner.x + x, c.composer_inner.y + y));
    }
    let h = c.hints;
    lines(
        f,
        Rect::new(h.x + 2, h.y, h.w.saturating_sub(2), 1),
        std::slice::from_ref(&p.hint_left),
        None,
    );
    let rw = p.hint_right.width() as u16;
    if rw + 2 < h.w {
        lines(
            f,
            Rect::new(h.right() - rw - 1, h.y, rw, 1),
            std::slice::from_ref(&p.hint_right),
            None,
        );
    }
}

fn side(f: &mut Frame, s: &crate::layout::SideRegions, p: &SidePane) {
    let upper_focused = p.upper.focused;
    f.render_widget(frame_block(upper_focused), rr(s.upper_frame));
    for (i, (tab, hit)) in p.tabs.iter().zip(&s.tab_hits).enumerate() {
        let st = if tab.active {
            bar(true)
        } else if tab.badge > 0 {
            bar(false).fg(Color::Yellow)
        } else {
            bar(false)
        };
        f.render_widget(
            Paragraph::new(RLine::from(RSpan::styled(tab.display(), st))),
            rr(*hit),
        );
        if i + 1 < p.tabs.len() && hit.right() < s.upper_frame.right() - 1 {
            f.render_widget(
                Paragraph::new(RLine::from(RSpan::styled("│", bar(false)))),
                rr(Rect::new(hit.right(), hit.y, 1, 1)),
            );
        }
    }
    section(f, s.upper, &p.upper);
    f.render_widget(frame_block(p.lower.focused), rr(s.lower_frame));
    let title = Line::plain(p.lower.title.clone().unwrap_or_default());
    let right = p
        .lower
        .title_right
        .clone()
        .map(Line::plain)
        .unwrap_or_default();
    titles(f, s.lower_frame, &title, &right, p.lower.focused);
    section(f, s.lower, &p.lower);
}

/// 第一列留给选中标记 `›`：有焦点时亮、并反色整行；没焦点时暗，不盖住行首的状态符号。
fn section(f: &mut Frame, r: Rect, sec: &Section) {
    if r.w < 2 {
        return;
    }
    let body = Rect::new(r.x + 1, r.y, r.w - 1, r.h);
    lines(
        f,
        body,
        &sec.rows,
        if sec.focused { sec.selected } else { None },
    );
    if let Some(i) = sec.selected
        && (i as u16) < r.h
    {
        let tone = if sec.focused { Tone::Accent } else { Tone::Dim };
        lines(
            f,
            Rect::new(r.x, r.y + i as u16, 1, 1),
            &[Line::toned("›", tone)],
            None,
        );
    }
}

fn footer(f: &mut Frame, size: Size, ft: &Footer) {
    let r = Rect::new(0, size.rows.saturating_sub(1), size.cols, 1.min(size.rows));
    f.render_widget(Block::default().style(Style::default().bg(BAR)), rr(r));
    let r = Rect::new(r.x + 1, r.y, r.w.saturating_sub(2), r.h);
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
    let w = size.cols.saturating_sub(2).min(118);
    let h = size.rows.saturating_sub(2);
    let frame = Rect::new((size.cols - w) / 2, 0, w, h);
    f.render_widget(frame_block(true), rr(frame));
    titles(f, frame, &s.title, &Line::default(), true);
    let list = Rect::new(
        frame.x + 2,
        frame.y + 2,
        w.saturating_sub(4),
        h.saturating_sub(5),
    );
    lines(f, list, &s.rows, s.selected);
    if let Some(m) = &s.message {
        lines(
            f,
            Rect::new(
                frame.x + 2,
                frame.bottom().saturating_sub(2),
                w.saturating_sub(4),
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
