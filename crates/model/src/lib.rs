//! Codix 的 view-model（docs/design/architecture.md §4）。
//!
//! `update` 与 `view` 都是同步纯函数：不做 IO、不起任务，所以整套交互不用终端也能测。
//! 后端调用以 `Effect::Call` 交给 app 去 spawn，结果以 `Msg::Reply` 回来。

mod chat;
mod chat_update;
mod composer;
mod highlight;
mod mouse;
mod sessions;
mod settings;
mod state;
mod text;
mod update;
mod view;
mod welcome;

pub use chat::{Block, BlockKind, Conversation, Phase};
pub use composer::ComposerState;
pub use highlight::theme_names;
pub use mouse::{Mouse, MouseButton, MouseKind};
pub use sessions::{SessRow, TreeRow};
pub use settings::Settings;
pub use state::{
    Confirm, DEFAULT_IDLE_TIMEOUT_MS, DetachReason, Domain, Focus, Overlay, Picker, Screen, State,
    Tab, Ui,
};
pub use update::{Effect, InstOk, InstOp, Msg, ReqId, update};
pub use view::view;
