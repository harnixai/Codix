use ratatui::Frame;
use ratatui::layout::Rect as RRect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as RLine, Span as RSpan};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::frame::*;
use crate::layout::{self, Rect, Regions, Size};
use crate::text::{display_width, truncate_to_width};

pub fn render(f: &mut Frame, state: &FrameState) {
    THEME.with(|t| *t.borrow_mut() = state.theme.clone());
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

thread_local! {
    /// 这一帧的配色。`render` 进来时设好，整帧共用；渲染是单线程的。
    static THEME: std::cell::RefCell<crate::Theme> = std::cell::RefCell::new(crate::Theme::default());
}

fn th() -> crate::Theme {
    THEME.with(|t| t.borrow().clone())
}

fn style(t: Tone) -> Style {
    let th = th();
    let s = Style::default();
    match t {
        Tone::Normal => s,
        Tone::Dim => s.fg(th.dim),
        Tone::Strong => s.add_modifier(Modifier::BOLD),
        Tone::Accent => s.fg(th.accent),
        Tone::Ok => s.fg(th.ok),
        Tone::Warn => s.fg(th.warn),
        Tone::Error => s.fg(th.error),
        Tone::Add => s.fg(th.add),
        Tone::Del => s.fg(th.del),
        Tone::Code => s.fg(th.code),
        Tone::Selected => s.bg(th.selected_bg).fg(th.selected_fg),
        Tone::Attention => s.fg(th.warn).add_modifier(Modifier::BOLD),
        Tone::UserMsg => s.bg(th.user_bg),
        Tone::Italic => s.add_modifier(Modifier::ITALIC),
        Tone::Link => s.fg(th.link).add_modifier(Modifier::UNDERLINED),
        Tone::Rgb(r, g, b) => s.fg(Color::Rgb(r, g, b)),
    }
}

fn bar(focused: bool) -> Style {
    let th = th();
    if focused {
        Style::default()
            .bg(th.accent)
            .fg(th.accent_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(th.bar_bg).fg(th.bar_fg)
    }
}

fn border_style(focused: bool) -> Style {
    let th = th();
    Style::default().fg(if focused { th.border_focus } else { th.border })
}

fn frame_block(focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style(focused))
}

/// 框上沿的标题：左标题从第 2 列起，右标题贴右边。不带底色，和右上的标签头一个做法：
/// 有焦点时左标题换主色加粗，没焦点是淡色；右标题始终是淡色。
fn titles(f: &mut Frame, frame: Rect, left: &Line, right: &Line, focused: bool) {
    let room = frame.w.saturating_sub(4) as usize;
    if room == 0 {
        return;
    }
    let left_style = if focused {
        Style::default()
            .fg(th().accent)
            .add_modifier(Modifier::BOLD)
    } else {
        style(Tone::Dim)
    };
    let paint = |l: &Line, st: Style| {
        let text = format!(" {} ", l.text());
        RLine::from(RSpan::styled(text, st))
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
            Paragraph::new(paint(&l, left_style)),
            rr(Rect::new(frame.x + 2, frame.y, lw as u16, 1)),
        );
    }
    if rw > 2 {
        let r = Line::plain(truncate_to_width(&right.text(), rw - 2));
        f.render_widget(
            Paragraph::new(paint(&r, style(Tone::Dim))),
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
    f.render_widget(
        Block::default().style(Style::default().bg(th().bar_bg).fg(th().bar_fg)),
        rr(r),
    );
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
    if let Some(r) = c.panel {
        lines(f, r, &p.panel, None);
    }
    if !p.picker.is_empty() {
        let n = (p.picker.len() as u16).min(c.transcript.h);
        let r = Rect::new(c.transcript.x, c.composer.y - n, c.transcript.w, n);
        f.render_widget(Clear, rr(r));
        lines(f, r, &p.picker, p.picker_selected);
    }
    if let Some(ask) = &p.ask {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(style(Tone::Warn));
        f.render_widget(block, rr(c.composer));
        let inner = Rect::new(
            c.composer.x + 1,
            c.composer_inner.y,
            c.composer.w.saturating_sub(2),
            c.composer_inner.h,
        );
        lines(f, inner, ask, p.ask_selected);
    } else {
        let border = border_style(p.composer.focused);
        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_style(border);
        f.render_widget(block, rr(c.composer));
        lines(f, c.composer_inner, &p.composer.lines, None);
    }
    if p.ask.is_none()
        && p.composer.focused
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
    let up = p.upper.focused;
    let down = p.lower.focused;
    f.render_widget(frame_block(up), rr(s.upper_frame));
    f.render_widget(frame_block(down), rr(s.lower_frame));
    // 共用的那条分隔线：上半或下半有焦点都亮（交互 §2.3）。
    let mid = s.lower_frame.y;
    let rule = format!("├{}┤", "─".repeat(s.area.w.saturating_sub(2) as usize));
    f.render_widget(
        Paragraph::new(RLine::from(RSpan::styled(rule, border_style(up || down)))),
        rr(Rect::new(s.area.x, mid, s.area.w, 1)),
    );
    for (tab, hit) in p.tabs.iter().zip(&s.tab_hits) {
        // 上半的字不加底色：当前标签只换颜色。焦点在下半时也保持选中色：下半是它的延伸。
        let st = if tab.active {
            let st = Style::default().add_modifier(Modifier::BOLD);
            if up || down { st.fg(th().accent) } else { st }
        } else {
            style(Tone::Dim)
        };
        let mut spans = vec![RSpan::styled(format!(" {} ", tab.label), st)];
        if tab.badge > 0 {
            spans.push(RSpan::styled("◐ ", style(Tone::Attention)));
        }
        f.render_widget(Paragraph::new(RLine::from(spans)), rr(*hit));
    }
    section(f, s.upper, &p.upper);
    // 下半的标题：和对话区一样，有焦点时字换主色，不带底色。
    titles(
        f,
        s.lower_frame,
        &Line::plain(p.lower.title.clone().unwrap_or_default()),
        &p.lower
            .title_right
            .clone()
            .map(Line::plain)
            .unwrap_or_default(),
        down,
    );
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
    f.render_widget(
        Block::default().style(Style::default().bg(th().bar_bg).fg(th().bar_fg)),
        rr(r),
    );
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
    let r = layout::overlay_rect(size, o.width, o.height);
    f.render_widget(Clear, rr(r));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style(true))
        .title(RLine::from(RSpan::styled(
            format!(" {} ", o.title),
            bar(true),
        )));
    f.render_widget(block, rr(r));
    let inner = Rect::new(
        r.x + 2,
        r.y + 1,
        r.w.saturating_sub(4),
        r.h.saturating_sub(2),
    );
    lines(f, inner, &o.lines, None);
}

/// 实例选择：屏幕中间的小框（交互 §2.2）。
fn instances(f: &mut Frame, size: Size, s: &InstancesScreen) {
    let frame = layout::instances_box(size, s.rows.len() as u16);
    let w = frame.w;
    f.render_widget(Clear, rr(frame));
    f.render_widget(frame_block(true), rr(frame));
    titles(f, frame, &s.title, &Line::default(), true);
    let x = frame.x + 2;
    let iw = w.saturating_sub(4);
    let up = if s.more_above { "▲" } else { "" };
    lines(
        f,
        Rect::new(x, frame.y + 1, iw, 1),
        &[Line::toned(up, Tone::Dim)],
        None,
    );
    let list = Rect::new(x, frame.y + 2, iw, s.rows.len() as u16);
    lines(f, list, &s.rows, s.selected);
    let down = if s.more_below { "▼" } else { "" };
    lines(
        f,
        Rect::new(x, list.bottom(), iw, 1),
        &[Line::toned(down, Tone::Dim)],
        None,
    );
    if let Some(m) = &s.message {
        lines(
            f,
            Rect::new(x, frame.bottom().saturating_sub(2), iw, 1),
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
