//! 区域矩形的唯一来源（交互 §2）。model 用它决定每块该给多少行、做鼠标命中测试；渲染端
//! 用它放置内容。纯函数：同样的尺寸与 `LayoutSpec` 永远得到同样的矩形。

use crate::text::display_width;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    pub const fn new(cols: u16, rows: u16) -> Size {
        Size { cols, rows }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, col: u16, row: u16) -> bool {
        col >= self.x && col < self.x + self.w && row >= self.y && row < self.y + self.h
    }

    pub fn bottom(&self) -> u16 {
        self.y + self.h
    }

    pub fn right(&self) -> u16 {
        self.x + self.w
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// 并排所需的最小宽度；低于它进单栏（交互 §2.4）。
pub const WIDE_MIN_COLS: u16 = 120;
/// 低于它只显示"终端太小"（交互 §2.5）。
pub const MIN_COLS: u16 = 60;
pub const MIN_ROWS: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowPane {
    Chat,
    Side,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMode {
    Wide {
        ratio_pct: u16,
        side_collapsed: bool,
    },
    Single {
        showing: ShowPane,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutSpec {
    pub mode: PaneMode,
    /// 输入框内容的行数（不含边框）；会被限制在对话区高度的 1/3 以内。
    pub composer_rows: u16,
    pub status: bool,
    /// 对话区顶部面板的行数（含标题线与下沿；收起时只有标题线这 1 行；Ctrl+T，交互 §4.3）；
    /// 会被限制在对话区高度的 1/3 以内。
    pub panel_rows: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Regions {
    pub header: Rect,
    pub footer: Rect,
    pub chat: Option<ChatRegions>,
    /// 两栏相接的两道边框，拖动它调比例。
    pub divider: Option<Rect>,
    pub side: Option<SideRegions>,
}

/// 对话区是一个框：标题写在上沿，里面自上而下是归属行、转录、状态行、todo、输入框、
/// 输入框下的提示行（交互 §2.3、§4.4）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatRegions {
    pub frame: Rect,
    pub owner: Rect,
    pub transcript: Rect,
    pub status: Option<Rect>,
    /// 归属行下面的面板：goal / todo、流程设计、运行进度、作业。
    pub panel: Option<Rect>,
    /// 带边框的整个输入框。
    pub composer: Rect,
    pub composer_inner: Rect,
    pub hints: Rect,
}

/// 右栏是上下两个框，平分高度，中间共用一条分隔线；标签头写在上框的上沿。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SideRegions {
    pub area: Rect,
    pub upper_frame: Rect,
    pub upper: Rect,
    /// 每个标签头的矩形（在上框的上沿），与传入的标签一一对应。
    pub tab_hits: Vec<Rect>,
    pub lower_frame: Rect,
    pub lower: Rect,
}

/// 并排还是单栏由宽度决定；比例与收起状态由界面状态决定。
pub fn pane_mode_for(
    cols: u16,
    ratio_pct: u16,
    side_collapsed: bool,
    showing: ShowPane,
) -> PaneMode {
    if cols >= WIDE_MIN_COLS {
        PaneMode::Wide {
            ratio_pct,
            side_collapsed,
        }
    } else {
        PaneMode::Single { showing }
    }
}

/// `tab_labels` 是标签头上显示的文字（`TabLabel::display`）。
pub fn compute(size: Size, spec: &LayoutSpec, tab_labels: &[String]) -> Regions {
    let header = Rect::new(0, 0, size.cols, 1.min(size.rows));
    let footer = Rect::new(0, size.rows.saturating_sub(1), size.cols, 1.min(size.rows));
    let body = Rect::new(0, 1, size.cols, size.rows.saturating_sub(2));
    let mut r = Regions {
        header,
        footer,
        ..Regions::default()
    };
    match spec.mode {
        PaneMode::Wide {
            side_collapsed: true,
            ..
        }
        | PaneMode::Single {
            showing: ShowPane::Chat,
        } => r.chat = Some(chat(body, spec)),
        PaneMode::Single {
            showing: ShowPane::Side,
        } => r.side = Some(side(body, tab_labels)),
        PaneMode::Wide { ratio_pct, .. } => {
            let chat_w = ((u32::from(body.w) * u32::from(ratio_pct)) / 100) as u16;
            let chat_w = chat_w.clamp(20.min(body.w), body.w.saturating_sub(24));
            r.chat = Some(chat(Rect::new(0, body.y, chat_w, body.h), spec));
            r.divider = Some(Rect::new(chat_w.saturating_sub(1), body.y, 2, body.h));
            r.side = Some(side(
                Rect::new(chat_w, body.y, body.w - chat_w, body.h),
                tab_labels,
            ));
        }
    }
    r
}

fn inner(r: Rect) -> Rect {
    Rect::new(
        r.x + 1.min(r.w),
        r.y + 1.min(r.h),
        r.w.saturating_sub(2),
        r.h.saturating_sub(2),
    )
}

fn chat(frame: Rect, spec: &LayoutSpec) -> ChatRegions {
    let b = inner(frame);
    // 左右各留一列，内容不贴框线。
    let a = Rect::new(b.x + 1.min(b.w), b.y, b.w.saturating_sub(2), b.h);
    let owner = Rect::new(a.x, a.y, a.w, 1.min(a.h));
    let hints = Rect::new(a.x, a.bottom().saturating_sub(1), a.w, 1.min(a.h));
    let rest_h = a.h.saturating_sub(2);
    let max_inner = (a.h / 3).max(1);
    let inner_h = spec.composer_rows.clamp(1, max_inner);
    let box_h = (inner_h + 2).min(rest_h);
    let composer = Rect::new(a.x, hints.y.saturating_sub(box_h), a.w, box_h);
    let mut above = composer.y;
    let room = |above: u16| above.saturating_sub(a.y + 1);
    let status = (spec.status && room(above) > 2).then(|| {
        above -= 1;
        Rect::new(a.x, above, a.w, 1)
    });
    let mut top = a.y + 1;
    let panel_h = spec.panel_rows.min(a.h / 3);
    let panel = (panel_h >= 1 && room(above) > panel_h + 2).then(|| {
        top += panel_h;
        Rect::new(a.x, a.y + 1, a.w, panel_h)
    });
    let transcript = Rect::new(a.x, top, a.w, above.saturating_sub(top));
    // 输入框照 Claude Code 只有上下两条横线，内容区占满宽度。
    let composer_inner = Rect::new(
        composer.x,
        composer.y + 1.min(composer.h),
        composer.w,
        composer.h.saturating_sub(2),
    );
    ChatRegions {
        frame,
        owner,
        transcript,
        status,
        panel,
        composer,
        composer_inner,
        hints,
    }
}

fn side(area: Rect, tab_labels: &[String]) -> SideRegions {
    let upper_h = area.h / 2;
    // 两框共用中间那一行：上框的下沿就是下框的上沿。
    let upper_frame = Rect::new(area.x, area.y, area.w, (upper_h + 1).min(area.h));
    let lower_frame = Rect::new(area.x, area.y + upper_h, area.w, area.h - upper_h);
    let mut x = area.x + 2;
    let tab_hits = tab_labels
        .iter()
        .map(|t| {
            let w = (display_width(t) as u16).min(area.right().saturating_sub(x + 1));
            let r = Rect::new(x, area.y, w, 1);
            x += w + 1;
            r
        })
        .collect();
    SideRegions {
        area,
        upper_frame,
        upper: inner(upper_frame),
        tab_hits,
        lower_frame,
        lower: inner(lower_frame),
    }
}

/// 实例选择的小框（交互 §2.2）：`rows` 是视口里的行数。框内第 2 行起是列表，左边距 2。
pub fn instances_box(size: Size, rows: u16) -> Rect {
    let w = size.cols.saturating_sub(4).min(88);
    let h = (rows + 6).min(size.rows.saturating_sub(1));
    Rect::new(
        (size.cols - w) / 2,
        size.rows.saturating_sub(1).saturating_sub(h) / 2,
        w,
        h,
    )
}

/// 浮层的框；框内第 1 行起是内容，左边距 2。
pub fn overlay_rect(size: Size, w: u16, h: u16) -> Rect {
    let w = w.min(size.cols.saturating_sub(2)).max(10);
    let h = h.min(size.rows.saturating_sub(2)).max(3);
    Rect::new((size.cols - w) / 2, (size.rows - h) / 2, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(mode: PaneMode) -> LayoutSpec {
        LayoutSpec {
            mode,
            composer_rows: 1,
            status: true,
            panel_rows: 0,
        }
    }

    fn tabs() -> Vec<String> {
        vec![" 会话 ② ".into(), " 流程 ".into(), " 作业 ① ".into()]
    }

    #[test]
    fn wide_splits_by_ratio_and_tiles_the_body() {
        let r = compute(
            Size::new(160, 45),
            &spec(PaneMode::Wide {
                ratio_pct: 60,
                side_collapsed: false,
            }),
            &tabs(),
        );
        let chat = r.chat.unwrap();
        let side = r.side.unwrap();
        assert_eq!(chat.frame.w, 96);
        assert_eq!(side.area.x, 96);
        assert_eq!(chat.frame.w + side.area.w, 160);
        assert_eq!(r.divider.unwrap(), Rect::new(95, 1, 2, 43));
        assert_eq!(chat.frame.bottom(), 44, "框贴着底栏");
        assert_eq!(chat.hints.bottom(), 43, "提示行在框内最后一行");
        assert_eq!(chat.composer.bottom(), chat.hints.y);
        assert_eq!(chat.status.unwrap().bottom(), chat.composer.y);
        assert_eq!(chat.transcript.bottom(), chat.status.unwrap().y);
        assert_eq!(chat.transcript.y, chat.owner.bottom());
        assert_eq!(
            side.upper_frame.bottom() - 1,
            side.lower_frame.y,
            "共用一条分隔线"
        );
        assert_eq!(side.upper.h, 20);
        assert_eq!(side.lower.h, 20, "上下两半内容区一样高");
        assert_eq!(side.lower_frame.bottom(), 44);
        assert_eq!(side.tab_hits[0].y, side.upper_frame.y, "标签在上框的上沿");
        assert_eq!(side.tab_hits[1].x, side.tab_hits[0].right() + 1);
    }

    #[test]
    fn single_shows_one_pane_full_width() {
        let r = compute(
            Size::new(100, 30),
            &spec(PaneMode::Single {
                showing: ShowPane::Side,
            }),
            &tabs(),
        );
        assert!(r.chat.is_none());
        assert_eq!(r.side.unwrap().area.w, 100);
        assert!(r.divider.is_none());
    }

    #[test]
    fn composer_is_capped_at_a_third() {
        let mut s = spec(PaneMode::Single {
            showing: ShowPane::Chat,
        });
        s.composer_rows = 50;
        let chat = compute(Size::new(80, 32), &s, &[]).chat.unwrap();
        assert_eq!(chat.composer_inner.h, 9);
        assert_eq!(chat.composer_inner.w, chat.composer.w, "没有左右边框");
        assert!(chat.transcript.h >= 1);
    }

    #[test]
    fn panel_sits_under_owner_and_is_capped_at_a_third() {
        let mut s = spec(PaneMode::Single {
            showing: ShowPane::Chat,
        });
        s.panel_rows = 6;
        let chat = compute(Size::new(100, 40), &s, &[]).chat.unwrap();
        let panel = chat.panel.unwrap();
        assert_eq!(panel.y, chat.owner.bottom());
        assert_eq!(panel.h, 6);
        assert_eq!(chat.transcript.y, panel.bottom());
        s.panel_rows = 30;
        let chat = compute(Size::new(100, 40), &s, &[]).chat.unwrap();
        assert_eq!(chat.panel.unwrap().h, 37 / 3, "不超过对话区的 1/3");
    }
}
