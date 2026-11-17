//! 终端外壳的抽象。真实实现走 crossterm；测试用 `FakeTerm`（TestBackend），尺寸协商、
//! 鼠标捕获都可以断言（testing.md §2）。

use std::io::{self, Stdout, Write};
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetSize, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use tui::{FrameState, Size};

pub trait Term {
    fn size(&self) -> Size;
    /// 请求终端把窗口调到这个尺寸。终端照不照办由它决定，结果看之后的 `size()`。
    fn request_size(&mut self, size: Size);
    fn enter(&mut self, mouse: bool) -> io::Result<()>;
    fn leave(&mut self) -> io::Result<()>;
    fn set_mouse(&mut self, on: bool) -> io::Result<()>;
    fn draw(&mut self, frame: &FrameState) -> io::Result<()>;
    fn clear(&mut self) -> io::Result<()>;
}

pub struct CrosstermTerm {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    mouse: bool,
    entered: bool,
}

impl CrosstermTerm {
    /// 清屏并让下一帧整屏重画。不用 `Terminal::clear`：它要先向终端查询光标位置，
    /// 不回应查询的终端（串口控制台之类）上会等两秒然后报错退出；`resize` 不查询。
    fn clear_screen(&mut self) -> io::Result<()> {
        let (c, r) = crossterm::terminal::size().unwrap_or((80, 24));
        self.terminal.resize(ratatui::layout::Rect::new(0, 0, c, r))
    }

    pub fn new() -> io::Result<Self> {
        Ok(CrosstermTerm {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?,
            mouse: false,
            entered: false,
        })
    }
}

impl Term for CrosstermTerm {
    fn size(&self) -> Size {
        let (c, r) = crossterm::terminal::size().unwrap_or((80, 24));
        Size::new(c, r)
    }

    fn request_size(&mut self, size: Size) {
        let mut out = io::stdout();
        let _ = queue!(out, SetSize(size.cols, size.rows));
        let _ = out.flush();
    }

    fn enter(&mut self, mouse: bool) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        self.entered = true;
        self.set_mouse(mouse)?;
        self.clear_screen()
    }

    fn leave(&mut self) -> io::Result<()> {
        if !self.entered {
            return Ok(());
        }
        self.entered = false;
        if self.mouse {
            execute!(io::stdout(), DisableMouseCapture)?;
            self.mouse = false;
        }
        execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen)?;
        disable_raw_mode()?;
        self.terminal.show_cursor()
    }

    fn set_mouse(&mut self, on: bool) -> io::Result<()> {
        if on != self.mouse {
            if on {
                execute!(io::stdout(), EnableMouseCapture)?;
            } else {
                execute!(io::stdout(), DisableMouseCapture)?;
            }
            self.mouse = on;
        }
        Ok(())
    }

    fn draw(&mut self, frame: &FrameState) -> io::Result<()> {
        self.terminal.draw(|f| tui::render(f, frame)).map(|_| ())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_screen()
    }
}

impl Drop for CrosstermTerm {
    fn drop(&mut self) {
        let _ = self.leave();
    }
}

/// 测试用终端：`obeys` 决定它是否照办尺寸请求。
pub struct FakeTerm {
    pub terminal: Terminal<TestBackend>,
    pub obeys: bool,
    pub requested: Vec<Size>,
    pub mouse: bool,
    pub entered: bool,
}

impl FakeTerm {
    pub fn new(size: Size, obeys: bool) -> Self {
        FakeTerm {
            terminal: Terminal::new(TestBackend::new(size.cols, size.rows)).unwrap(),
            obeys,
            requested: vec![],
            mouse: false,
            entered: false,
        }
    }
}

impl Term for FakeTerm {
    fn size(&self) -> Size {
        let a = self.terminal.backend().buffer().area;
        Size::new(a.width, a.height)
    }

    fn request_size(&mut self, size: Size) {
        self.requested.push(size);
        if self.obeys {
            self.terminal.backend_mut().resize(size.cols, size.rows);
        }
    }

    fn enter(&mut self, mouse: bool) -> io::Result<()> {
        self.entered = true;
        self.mouse = mouse;
        Ok(())
    }

    fn leave(&mut self) -> io::Result<()> {
        self.entered = false;
        self.mouse = false;
        Ok(())
    }

    fn set_mouse(&mut self, on: bool) -> io::Result<()> {
        self.mouse = on;
        Ok(())
    }

    fn draw(&mut self, frame: &FrameState) -> io::Result<()> {
        let s = self.size();
        let Ok(()) = self
            .terminal
            .resize(ratatui::layout::Rect::new(0, 0, s.cols, s.rows));
        let Ok(_) = self.terminal.draw(|f| tui::render(f, frame));
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        let Ok(()) = self.terminal.clear();
        Ok(())
    }
}

/// 启动时的尺寸协商（需求 L-06，交互 §2.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    pub original: Size,
    /// 终端照办了：退出时要请求恢复成 `original`。
    pub changed: bool,
}

/// 在 tmux / screen 里不发请求：它会作用到外层窗口，或者被忽略。
pub fn in_multiplexer() -> bool {
    std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some()
}

pub fn negotiate(
    term: &mut dyn Term,
    want: Option<Size>,
    in_mux: bool,
    wait: Duration,
) -> Negotiated {
    let original = term.size();
    let Some(want) = want else {
        return Negotiated {
            original,
            changed: false,
        };
    };
    if in_mux || want == original {
        return Negotiated {
            original,
            changed: false,
        };
    }
    term.request_size(want);
    let deadline = Instant::now() + wait;
    loop {
        let now = term.size();
        if now != original {
            return Negotiated {
                original,
                changed: true,
            };
        }
        if Instant::now() >= deadline {
            return Negotiated {
                original,
                changed: false,
            };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn restore(term: &mut dyn Term, n: Negotiated) {
    if n.changed {
        term.request_size(n.original);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_millis(60);

    #[test]
    fn obeying_terminal_is_resized_and_restored() {
        let mut t = FakeTerm::new(Size::new(100, 30), true);
        let n = negotiate(&mut t, Some(Size::new(160, 45)), false, WAIT);
        assert!(n.changed);
        assert_eq!(t.size(), Size::new(160, 45));
        restore(&mut t, n);
        assert_eq!(t.size(), Size::new(100, 30));
    }

    #[test]
    fn ignoring_terminal_keeps_its_size() {
        let mut t = FakeTerm::new(Size::new(100, 30), false);
        let n = negotiate(&mut t, Some(Size::new(160, 45)), false, WAIT);
        assert!(!n.changed);
        assert_eq!(t.requested, vec![Size::new(160, 45)]);
        restore(&mut t, n);
        assert_eq!(t.requested.len(), 1, "没改过就不请求恢复");
    }

    #[test]
    fn multiplexer_and_off_send_nothing() {
        let mut t = FakeTerm::new(Size::new(100, 30), true);
        negotiate(&mut t, Some(Size::new(160, 45)), true, WAIT);
        negotiate(&mut t, None, false, WAIT);
        assert!(t.requested.is_empty());
    }
}
