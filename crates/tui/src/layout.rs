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
    /// 右栏上半想要的行数；最多给到右栏的一半。
    pub upper_rows: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Regions {
    pub header: Rect,
    pub footer: Rect,
    pub chat: Option<ChatRegions>,
    pub divider: Option<Rect>,
    pub side: Option<SideRegions>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatRegions {
    pub area: Rect,
    pub crumbs: Rect,
    pub owner: Rect,
    pub transcript: Rect,
    pub status: Option<Rect>,
    /// 带边框的整个输入框。
    pub composer: Rect,
    /// 输入框边框之内。
    pub composer_inner: Rect,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SideRegions {
    pub area: Rect,
    pub tabs: Rect,
    /// 每个标签头的矩形，与传入的标签一一对应。
    pub tab_hits: Vec<Rect>,
    pub upper: Rect,
    /// 上下两半之间的分隔线（带下半的标题）。
    pub rule: Rect,
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
    let body_h = size.rows.saturating_sub(2);
    let body = Rect::new(0, 1, size.cols, body_h);
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
        } => r.side = Some(side(body, spec, tab_labels)),
        PaneMode::Wide { ratio_pct, .. } => {
            let chat_w = ((u32::from(body.w) * u32::from(ratio_pct)) / 100) as u16;
            let chat_w = chat_w.clamp(20.min(body.w), body.w.saturating_sub(21));
            let divider = Rect::new(chat_w, body.y, 1, body.h);
            let side_area = Rect::new(chat_w + 1, body.y, body.w - chat_w - 1, body.h);
            r.chat = Some(chat(Rect::new(0, body.y, chat_w, body.h), spec));
            r.divider = Some(divider);
            r.side = Some(side(side_area, spec, tab_labels));
        }
    }
    r
}

fn chat(area: Rect, spec: &LayoutSpec) -> ChatRegions {
    let crumbs = Rect::new(area.x, area.y, area.w, 1.min(area.h));
    let owner = Rect::new(area.x, area.y + 1, area.w, 1.min(area.h.saturating_sub(1)));
    let rest_h = area.h.saturating_sub(2);
    let max_inner = (area.h / 3).max(1);
    let inner_h = spec.composer_rows.clamp(1, max_inner);
    let box_h = (inner_h + 2).min(rest_h);
    let composer = Rect::new(area.x, area.bottom() - box_h, area.w, box_h);
    let status_h = u16::from(spec.status && rest_h > box_h + 1);
    let status = (status_h == 1).then(|| Rect::new(area.x, composer.y - 1, area.w, 1));
    let transcript = Rect::new(
        area.x,
        area.y + 2,
        area.w,
        rest_h.saturating_sub(box_h + status_h),
    );
    let composer_inner = Rect::new(
        composer.x + 1,
        composer.y + 1,
        composer.w.saturating_sub(2),
        composer.h.saturating_sub(2),
    );
    ChatRegions {
        area,
        crumbs,
        owner,
        transcript,
        status,
        composer,
        composer_inner,
    }
}

fn side(area: Rect, spec: &LayoutSpec, tab_labels: &[String]) -> SideRegions {
    let tabs = Rect::new(area.x, area.y, area.w, 1.min(area.h));
    let mut x = area.x;
    let tab_hits = tab_labels
        .iter()
        .map(|t| {
            let w = (display_width(t) as u16).min(area.right().saturating_sub(x));
            let r = Rect::new(x, area.y, w, 1);
            x += w + 1;
            r
        })
        .collect();
    let rest = area.h.saturating_sub(1);
    let upper_h = spec
        .upper_rows
        .max(1)
        .min((rest.saturating_sub(1) / 2).max(1));
    let upper = Rect::new(area.x, area.y + 1, area.w, upper_h.min(rest));
    let rule = Rect::new(
        area.x,
        upper.bottom(),
        area.w,
        1.min(rest.saturating_sub(upper.h)),
    );
    let lower = Rect::new(
        area.x,
        rule.bottom(),
        area.w,
        area.bottom().saturating_sub(rule.bottom()),
    );
    SideRegions {
        area,
        tabs,
        tab_hits,
        upper,
        rule,
        lower,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(mode: PaneMode) -> LayoutSpec {
        LayoutSpec {
            mode,
            composer_rows: 1,
            status: true,
            upper_rows: 6,
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
        let div = r.divider.unwrap();
        assert_eq!(chat.area.w, 96);
        assert_eq!(div.x, 96);
        assert_eq!(side.area.x, 97);
        assert_eq!(chat.area.w + 1 + side.area.w, 160);
        assert_eq!(chat.composer.bottom(), 44, "输入框贴着底栏");
        assert_eq!(chat.status.unwrap().bottom(), chat.composer.y);
        assert_eq!(chat.transcript.bottom(), chat.status.unwrap().y);
        assert_eq!(side.upper.h, 6);
        assert_eq!(side.lower.bottom(), 44);
        assert_eq!(side.tab_hits.len(), 3);
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
        assert_eq!(chat.composer_inner.h, 10);
        assert!(chat.transcript.h >= 1);
    }

    #[test]
    fn upper_never_takes_more_than_half() {
        let mut s = spec(PaneMode::Single {
            showing: ShowPane::Side,
        });
        s.upper_rows = 100;
        let side = compute(Size::new(80, 24), &s, &tabs()).side.unwrap();
        assert!(side.upper.h <= side.lower.h + 1);
    }
}
