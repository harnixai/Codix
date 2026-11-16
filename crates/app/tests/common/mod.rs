//! 无终端的驱动器（testing.md §2）：mock 后端 + tokio 暂停时钟 + 按键 / 鼠标脚本 + 快照。

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use app::Core;
use app::core::ShellEffect;
use app::snapshot::{assert_snapshot, render_text};
use model::{Mouse, MouseButton, MouseKind, Msg};
use port::{Attach, Instances};
use tui::Size;

pub const EPOCH_MS: u64 = 1_763_280_000_000;

pub struct Harness {
    pub core: Core,
    pub size: Size,
    pub instances: Arc<mock::MockInstances>,
    pub shell: Vec<ShellEffect>,
}

impl Harness {
    pub async fn new(scenario: &str, size: Size) -> Harness {
        let sc =
            mock::load_scenario(scenario).unwrap_or_else(|_| mock::Scenario::seed_only(scenario));
        let instances = Arc::new(mock::MockInstances::new(sc, mock::MockOptions::default()));
        let backend = instances.attach("dev", Attach::default()).await.unwrap();
        let start = tokio::time::Instant::now();
        let clock = Box::new(move || EPOCH_MS + start.elapsed().as_millis() as u64);
        let (core, shell) = Core::new(backend, Arc::new(keys::Keymap::default()), size, clock);
        let mut h = Harness {
            core,
            size,
            instances,
            shell,
        };
        h.settle().await;
        h
    }

    /// 让已 spawn 的后端调用跑完、结果进来。
    pub async fn settle(&mut self) {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        let s = self.core.pump();
        self.shell.extend(s);
    }

    pub async fn advance(&mut self, ms: u64) {
        tokio::time::advance(Duration::from_millis(ms)).await;
        self.settle().await;
        let s = self.core.tick();
        self.shell.extend(s);
    }

    /// 空格分隔的按键：`"Ctrl+B F2 Down"`；字面空格写 `Space`。
    pub async fn keys(&mut self, seq: &str) {
        for k in seq.split_whitespace() {
            let key = keys::Key::parse(k).unwrap_or_else(|| panic!("按键写错了：{k}"));
            let s = self.core.key(key);
            self.shell.extend(s);
        }
        self.settle().await;
    }

    pub fn mouse(&mut self, kind: MouseKind, col: u16, row: u16) {
        let s = self.core.mouse(Mouse { kind, col, row });
        self.shell.extend(s);
    }

    pub fn click(&mut self, col: u16, row: u16) {
        self.mouse(MouseKind::Down(MouseButton::Left), col, row);
        self.mouse(MouseKind::Up(MouseButton::Left), col, row);
    }

    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.mouse(MouseKind::Down(MouseButton::Left), from.0, from.1);
        self.mouse(MouseKind::Drag(MouseButton::Left), to.0, to.1);
        self.mouse(MouseKind::Up(MouseButton::Left), to.0, to.1);
    }

    pub fn resize(&mut self, size: Size) {
        self.size = size;
        let s = self.core.handle(Msg::Resize(size));
        self.shell.extend(s);
    }

    pub fn text(&mut self) -> String {
        let f = self.core.frame();
        render_text(&f, self.size)
    }

    pub fn snapshot(&mut self, name: &str) {
        let t = self.text();
        assert_snapshot(name, &t);
    }

    /// 屏幕上某段文字第一次出现的位置（列、行），按显示宽度计列。
    pub fn find(&mut self, needle: &str) -> Option<(u16, u16)> {
        let text = self.text();
        for (row, line) in text.lines().enumerate() {
            if let Some(byte) = line.find(needle) {
                let col = tui::display_width(&line[..byte]);
                return Some((col as u16, row as u16));
            }
        }
        None
    }

    pub fn last_line(&mut self) -> String {
        self.text().lines().last().unwrap_or_default().to_string()
    }
}
