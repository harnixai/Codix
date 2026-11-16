//! 对话区的输入与事件（交互 §4）：输入框、补全、提问框、转录滚动与折叠、提交与本地命令。

use keys::Action;
use port::{
    Answer, Command, Event, PermissionMode, PromptKind, Reply, SessionId, SlashCommand, SubmitMode,
};
use tui::layout::{self, Regions};

use crate::chat::{BlockKind, Conversation};
use crate::state::{Focus, Overlay, Screen, State, Tab};
use crate::update::{Effect, Pending};
use crate::view::chat_layout;

const ESC_ARM_MS: u64 = 800;
const DETACH_ARM_MS: u64 = 800;

/// 本地命令（交互 §4.5）：不联系后端，或只换个后端命令。
const LOCAL: &[(&str, &str)] = &[
    ("help", "帮助"),
    ("new", "新建会话"),
    ("sessions", "会话列表"),
    ("model", "切换模型"),
    ("goal", "设定 goal"),
    ("btw", "侧问"),
    ("doctor", "诊断"),
    ("quit", "脱离（实例继续运行）"),
    ("exit", "脱离（实例继续运行）"),
    ("stop", "停止实例"),
];

impl State {
    /// 补全列表的候选：以 `/` 开头、还没有空格时，按前缀过滤本地命令与后端命令。
    pub fn picker_candidates(&self) -> Vec<SlashCommand> {
        let c = &self.ui.composer;
        if c.picker_closed || c.bash || !c.text.starts_with('/') || c.text.contains([' ', '\n']) {
            return vec![];
        }
        let prefix = &c.text[1..];
        let mut out: Vec<SlashCommand> = LOCAL
            .iter()
            .map(|(n, d)| SlashCommand {
                name: n.to_string(),
                description: d.to_string(),
                source: "codix".into(),
            })
            .collect();
        if let Some(conv) = self.conv() {
            out.extend(conv.commands.iter().cloned());
        }
        out.retain(|x| x.name.starts_with(prefix));
        out.dedup_by(|a, b| a.name == b.name);
        out.truncate(8);
        out
    }

    pub fn chat_regions(&self) -> Regions {
        let (spec, labels) = chat_layout(self);
        layout::compute(self.ui.size, &spec, &labels)
    }

    fn transcript_height(&self) -> usize {
        self.chat_regions()
            .chat
            .map_or(1, |c| c.transcript.h as usize)
            .max(1)
    }
}

/// 每次 update 之后：按当前宽度重排脏块。
pub(crate) fn relayout(s: &mut State) {
    if s.ui.screen != Screen::Workbench {
        return;
    }
    let Some(chat) = s.chat_regions().chat else {
        return;
    };
    let width = chat.transcript.w as usize;
    let detail = s.ui.detail;
    let opts = crate::chat::RenderOpts {
        compact_tool_lines: s.settings.compact_tool_lines,
        syntax_theme: s.settings.syntax_theme.clone(),
    };
    if let Some(conv) = s.conv_mut() {
        let n = conv.relayout(width, detail, &opts);
        s.ui.relayouts += n;
    }
}

pub(crate) fn open_session(s: &mut State, id: SessionId) -> Vec<Effect> {
    s.ui.chat = Some(id.clone());
    s.ui.answering = None;
    s.ui.ask_active = false;
    if s.domain.convs.contains_key(&id) || s.ui.watching.contains(&id) {
        return vec![];
    }
    s.ui.watching.insert(id.clone());
    vec![
        s.call(
            Pending::Watch(id.clone()),
            Command::Watch {
                session: id.clone(),
            },
        ),
        s.call(
            Pending::Commands(id.clone()),
            Command::Commands { session: id },
        ),
    ]
}

/// 进入子 Agent 的侧链会话（交互 §4.1）：之后输入的话经"给任务发消息"送达它。
pub(crate) fn open_sidechain(s: &mut State, child: SessionId, task: port::TaskId) -> Vec<Effect> {
    let e = open_session(s, child);
    s.ui.via_task = Some(task);
    s.ui.task_view = None;
    e
}

/// 看一个任务的输出（只读，实时跟随）。对话区仍挂在它所属的会话上，归属行照常。
pub(crate) fn open_task_output(s: &mut State, owner: SessionId, task: port::TaskId) -> Vec<Effect> {
    s.ui.chat = Some(owner);
    s.ui.via_task = None;
    s.ui.task_view = Some(task.clone());
    vec![s.call(
        Pending::TaskOutput(task.clone()),
        Command::TaskOutput { task, from_byte: 0 },
    )]
}

/// 退回上一层上下文。
fn pop_context(s: &mut State) -> bool {
    let Some((id, _)) = s.ui.ctx_parents.pop() else {
        return false;
    };
    s.ui.chat = Some(id);
    s.ui.via_task = None;
    s.ui.task_view = None;
    true
}

pub(crate) fn scroll(s: &mut State, delta: isize) {
    let h = s.transcript_height();
    if let Some(c) = s.conv_mut() {
        let total = c.total_lines();
        let max = total.saturating_sub(h);
        let top = c.viewport_top(h) as isize;
        let new = (top + delta).clamp(0, max as isize) as usize;
        c.top = new;
        c.follow = new >= max;
        if c.follow {
            c.unseen = false;
        }
    }
}

fn to_bottom(s: &mut State) {
    if let Some(c) = s.conv_mut() {
        c.follow = true;
        c.unseen = false;
    }
}

fn toggle_block(s: &mut State, id: u64) {
    if let Some(c) = s.conv_mut() {
        if !c.expanded.remove(&id) {
            c.expanded.insert(id);
        }
        c.selected = Some(id);
    }
}

/// 转录里点了第 `row` 行（相对转录区顶部）：可折叠块的头一行就展开 / 收起。
pub(crate) fn transcript_click(s: &mut State, row: usize) {
    let h = s.transcript_height();
    let Some(c) = s.conv() else { return };
    let line = c.viewport_top(h) + row;
    let Some(id) = c.block_at(line) else { return };
    let is_head = c.first_line_of(id) == Some(line);
    let foldable = c.foldable().contains(&id);
    if is_head && foldable {
        toggle_block(s, id);
    }
}

pub(crate) fn transcript_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let h = s.transcript_height() as isize;
    match a {
        Action::Up => scroll(s, -1),
        Action::Down => scroll(s, 1),
        Action::PageUp => scroll(s, -h),
        Action::PageDown => scroll(s, h),
        Action::Top => {
            if let Some(c) = s.conv_mut() {
                c.follow = false;
                c.top = 0;
            }
        }
        Action::Bottom => to_bottom(s),
        Action::BlockPrev | Action::BlockNext => {
            let c = s.conv_mut()?;
            let list = c.foldable();
            let pos = c.selected.and_then(|id| list.iter().position(|x| *x == id));
            let next = match (a, pos) {
                (_, None) => list.len().checked_sub(1),
                (Action::BlockPrev, Some(p)) => Some(p.saturating_sub(1)),
                (_, Some(p)) => Some((p + 1).min(list.len().saturating_sub(1))),
            };
            c.selected = next.and_then(|i| list.get(i).copied());
            if let Some(line) = c.selected.and_then(|id| c.first_line_of(id)) {
                c.follow = false;
                c.top = line.saturating_sub(2);
            }
        }
        Action::Open => {
            let id = s.conv()?.selected?;
            toggle_block(s, id);
        }
        _ => return None,
    }
    Some(vec![])
}

pub(crate) fn text(s: &mut State, c: char) -> Vec<Effect> {
    if s.ui.task_view.is_some() {
        return vec![];
    }
    if c == '?' && s.ui.composer.is_empty() && !s.ui.composer.bash {
        s.ui.overlay = Some(Overlay::Help { scroll: 0 });
        return vec![];
    }
    s.ui.composer.insert(c);
    s.ui.esc_armed_ms = None;
    vec![]
}

pub(crate) fn composer_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let now = s.ui.now_ms;
    if a != Action::Back {
        s.ui.esc_armed_ms = None;
    }
    if a != Action::Detach {
        s.ui.detach_armed_ms = None;
    }
    let picker = s.picker_candidates();
    let c = &mut s.ui.composer;
    match a {
        Action::Up if !picker.is_empty() => c.picker = c.picker.saturating_sub(1),
        Action::Down if !picker.is_empty() => c.picker = (c.picker + 1).min(picker.len() - 1),
        Action::Complete if !picker.is_empty() => {
            let name = &picker[c.picker.min(picker.len() - 1)].name;
            c.clear();
            c.insert_str(&format!("/{name} "));
        }
        Action::Complete => {}
        Action::Submit => {
            if !picker.is_empty() {
                let name = picker[c.picker.min(picker.len() - 1)].name.clone();
                c.clear();
                c.insert_str(&format!("/{name}"));
            }
            return Some(submit(s));
        }
        Action::Newline => c.insert('\n'),
        Action::CursorLeft => c.left(),
        Action::CursorRight => c.right(),
        Action::WordLeft => c.word_left(),
        Action::WordRight => c.word_right(),
        Action::LineStart => c.line_start(),
        Action::LineEnd => c.line_end(),
        Action::ClearInput => c.clear(),
        Action::DeleteWord => c.delete_word(),
        Action::KillToEnd => c.kill_to_end(),
        Action::Backspace => c.backspace(),
        Action::DeleteForward => c.delete(),
        Action::Up => c.up(),
        Action::Down => c.down(),
        Action::PageUp => {
            let h = s.transcript_height() as isize;
            scroll(s, -h);
        }
        Action::PageDown => {
            let h = s.transcript_height() as isize;
            scroll(s, h);
        }
        Action::CycleMode => {
            s.ui.permission = match s.ui.permission {
                PermissionMode::Default => PermissionMode::AcceptEdits,
                PermissionMode::AcceptEdits => PermissionMode::Plan,
                PermissionMode::Plan | PermissionMode::BypassPermissions => PermissionMode::Default,
            };
        }
        Action::Back => {
            if s.ui.rename.take().is_some() {
                s.ui.composer.clear();
                return Some(vec![]);
            }
            // 子 Agent 或任务输出里：Esc 就是返回，不去中断上一层正在跑的那一轮
            // （要停子 Agent，用树上的 s）。
            let nested = !s.ui.ctx_parents.is_empty();
            let idle = !s.conv().is_some_and(|c| c.running);
            if s.ui.composer.is_empty() && (nested || idle) && pop_context(s) {
                return Some(vec![]);
            }
            let c = &mut s.ui.composer;
            if !picker.is_empty() {
                c.picker_closed = true;
            } else if s.conv().is_some_and(|c| c.running) {
                return Some(interrupt(s));
            } else if s.ui.answering.is_some() {
                s.ui.answering = None;
                s.ui.ask_active = true;
            } else if s.ui.esc_armed_ms.is_some_and(|t| now - t <= ESC_ARM_MS) {
                s.ui.esc_armed_ms = None;
                if s.ui.composer.is_empty() {
                    s.notice("回滚到上一条消息：阶段 3 接入 session.rewind 后可用", false);
                } else {
                    let text = s.ui.composer.text.clone();
                    s.ui.composer.history.push(text);
                    s.ui.composer.clear();
                }
            } else {
                s.ui.esc_armed_ms = Some(now);
            }
        }
        Action::Interrupt => {
            if s.conv().is_some_and(|c| c.running) {
                return Some(interrupt(s));
            }
            if !s.ui.composer.is_empty() {
                s.ui.composer.clear();
                return Some(vec![]);
            }
            return None;
        }
        Action::Detach => {
            if !s.ui.composer.is_empty() {
                s.ui.composer.delete();
            } else if s
                .ui
                .detach_armed_ms
                .is_some_and(|t| now - t <= DETACH_ARM_MS)
            {
                s.ui.detach_armed_ms = None;
                s.ui.notice = None;
                s.ui.overlay = Some(Overlay::Exit { selected: 0 });
            } else {
                s.ui.detach_armed_ms = Some(now);
                s.notice("再按一次 Ctrl+D 退出", false);
            }
        }
        Action::NextAttention if s.ask_visible() => {
            s.ui.ask_active = true;
            s.ui.answering = None;
        }
        _ => return None,
    }
    Some(vec![])
}

fn interrupt(s: &mut State) -> Vec<Effect> {
    let Some(id) = s.ui.chat.clone() else {
        return vec![];
    };
    vec![s.call(Pending::Ignore, Command::Interrupt { session: id })]
}

/// 提问框的选项（交互 §4.6，照 Claude Code）。
pub(crate) fn ask_options(kind: &PromptKind) -> Vec<String> {
    match kind {
        PromptKind::Permission { summary, .. } => vec![
            "是".into(),
            format!("是，且不再询问：{summary}"),
            "否".into(),
        ],
        PromptKind::Question { options, .. } => {
            let mut v = options.clone();
            v.push("自己写…".into());
            v
        }
    }
}

pub(crate) fn dialog_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let session = s.ui.chat.clone()?;
    let prompt = s.conv()?.prompts.first()?.clone();
    let options = ask_options(&prompt.kind);
    let pick = |s: &mut State, i: usize| -> Vec<Effect> {
        let answer = match (&prompt.kind, i) {
            (PromptKind::Permission { .. }, 0) => Answer::Permit,
            (PromptKind::Permission { .. }, 1) => Answer::PermitSession,
            (PromptKind::Permission { .. }, _) => Answer::Deny { reason: None },
            (PromptKind::Question { options, .. }, i) if i < options.len() => {
                Answer::Choice { index: i }
            }
            (PromptKind::Question { .. }, _) => {
                s.ui.answering = Some((session.clone(), prompt.id.clone()));
                s.ui.ask_active = false;
                return vec![];
            }
        };
        s.ui.ask_selected = 0;
        vec![s.call(
            Pending::Ignore,
            Command::Answer {
                session: session.clone(),
                prompt: prompt.id.clone(),
                answer,
            },
        )]
    };
    let deny = options.len() - 1;
    let effects = match a {
        Action::Up => {
            s.ui.ask_selected = s.ui.ask_selected.saturating_sub(1);
            vec![]
        }
        Action::Down => {
            s.ui.ask_selected = (s.ui.ask_selected + 1).min(options.len() - 1);
            vec![]
        }
        Action::Confirm => {
            let i = s.ui.ask_selected.min(deny);
            pick(s, i)
        }
        Action::Yes => pick(s, 0),
        Action::No | Action::Back => match prompt.kind {
            PromptKind::Permission { .. } => pick(s, 2),
            PromptKind::Question { .. } => {
                s.ui.ask_selected = 0;
                vec![s.call(
                    Pending::Ignore,
                    Command::Answer {
                        session: session.clone(),
                        prompt: prompt.id.clone(),
                        answer: Answer::Deny { reason: None },
                    },
                )]
            }
        },
        a => match a.choice_index() {
            Some(i) if i < options.len() => pick(s, i),
            _ => return None,
        },
    };
    Some(effects)
}

/// Enter：本地命令、自由回答、或者发给会话（交互 §4.4、§4.5）。
fn submit(s: &mut State) -> Vec<Effect> {
    if s.ui.composer.text.trim().is_empty() || s.ui.task_view.is_some() {
        return vec![];
    }
    if let Some(id) = s.ui.rename.take() {
        let (title, _) = s.ui.composer.take();
        return vec![s.call(
            Pending::Ignore,
            Command::Rename {
                session: id,
                title: title.trim().to_string(),
            },
        )];
    }
    if let Some((session, prompt)) = s.ui.answering.take() {
        let (text, _) = s.ui.composer.take();
        return vec![s.call(
            Pending::Ignore,
            Command::Answer {
                session,
                prompt,
                answer: Answer::Text { text },
            },
        )];
    }
    let (text, bash) = s.ui.composer.take();
    if !bash && let Some(rest) = text.strip_prefix('/') {
        let (name, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        let arg = arg.trim().to_string();
        if let Some(e) = local_command(s, name, arg) {
            return e;
        }
    }
    let text = if bash { format!("!{text}") } else { text };
    let Some(session) = s.ui.chat.clone() else {
        return vec![s.call(
            Pending::CreateThenSubmit { text },
            Command::CreateSession {
                project: None,
                scene: None,
                model: None,
            },
        )];
    };
    send(s, session, text)
}

pub(crate) fn send(s: &mut State, session: SessionId, text: String) -> Vec<Effect> {
    if let Some(task) = s.ui.via_task.clone()
        && s.ui.chat.as_ref() == Some(&session)
    {
        if let Some(c) = s.domain.convs.get_mut(&session) {
            c.follow = true;
            c.push(BlockKind::User {
                text: text.clone(),
                pending: None,
                steer: true,
            });
        }
        return vec![s.call(Pending::Ignore, Command::MessageTask { task, text })];
    }
    let running = s.domain.convs.get(&session).is_some_and(|c| c.running);
    let permission = (s.ui.permission != PermissionMode::Default).then_some(s.ui.permission);
    let block = match s.domain.convs.get_mut(&session) {
        Some(c) => {
            c.follow = true;
            c.unseen = false;
            c.push(BlockKind::User {
                text: text.clone(),
                pending: None,
                steer: running,
            })
        }
        None => 0,
    };
    vec![s.call(
        Pending::Submitted {
            session: session.clone(),
            block,
        },
        Command::Submit {
            session,
            text,
            mode: if running {
                SubmitMode::Steer
            } else {
                SubmitMode::Turn
            },
            permission,
        },
    )]
}

fn local_command(s: &mut State, name: &str, arg: String) -> Option<Vec<Effect>> {
    let session = s.ui.chat.clone();
    Some(match name {
        "help" => {
            s.ui.overlay = Some(Overlay::Help { scroll: 0 });
            vec![]
        }
        "quit" | "exit" => {
            s.ui.overlay = Some(Overlay::Exit { selected: 0 });
            vec![]
        }
        "new" => vec![s.call(
            Pending::CreateOnly,
            Command::CreateSession {
                project: None,
                scene: None,
                model: None,
            },
        )],
        "sessions" => {
            s.ui.tab = Tab::Sessions;
            s.ui.side_collapsed = false;
            s.ui.showing = tui::ShowPane::Side;
            s.ui.focus = Focus::SideUpper;
            vec![]
        }
        "stop" => {
            let name = s.ui.attached.clone()?;
            s.ui.confirm = Some(crate::state::Confirm::StopInstance(name));
            vec![]
        }
        "model" | "goal" | "btw" if session.is_none() => {
            s.notice("先打开或新建一个会话", true);
            vec![]
        }
        "model" if arg.is_empty() => {
            let m = s
                .conv()
                .map(|c| c.summary.provenance.model.clone())
                .unwrap_or_default();
            s.notice(format!("当前模型：{m}（/model <名字> 切换）"), false);
            vec![]
        }
        "model" => vec![s.call(
            Pending::Ignore,
            Command::SetModel {
                session: session?,
                model: arg,
            },
        )],
        "goal" if arg.is_empty() => {
            s.ui.tab = Tab::Sessions;
            s.ui.focus = Focus::SideUpper;
            vec![]
        }
        "goal" => vec![s.call(
            Pending::Ignore,
            Command::SetGoal {
                session: session?,
                objective: arg,
                max_rounds: None,
            },
        )],
        "btw" => vec![s.call(
            Pending::Ignore,
            Command::SideQuestion {
                session: session?,
                text: arg,
            },
        )],
        "doctor" => {
            s.notice("诊断：阶段 3 接入 daemon.doctor 后可用", false);
            vec![]
        }
        _ => return None,
    })
}

/// 发给会话的事件：交给对应的 Conversation。
pub(crate) fn chat_event(s: &mut State, ev: &Event) -> Vec<Effect> {
    let now = s.ui.now_ms;
    if let Event::SteerReturned { session, messages } = ev {
        let Some(c) = s.domain.convs.get_mut(session) else {
            return vec![];
        };
        let texts = c.take_returned(messages);
        if !texts.is_empty() {
            let mut joined = texts.join("\n");
            if !s.ui.composer.is_empty() {
                joined = format!("{joined}\n{}", s.ui.composer.text);
            }
            s.ui.composer.clear();
            s.ui.composer.insert_str(&joined);
            s.notice("插话没赶上这一轮，已放回输入框", false);
        }
        return vec![];
    }
    let Some(id) = ev.session().cloned() else {
        return vec![];
    };
    let mut effects = vec![];
    let is_prompt = matches!(ev, Event::PromptPending { .. });
    if matches!(ev, Event::CommandsChanged { .. }) && s.domain.convs.contains_key(&id) {
        effects.push(s.call(
            Pending::Commands(id.clone()),
            Command::Commands {
                session: id.clone(),
            },
        ));
    }
    let Some(c) = s.domain.convs.get_mut(&id) else {
        return effects;
    };
    if let Some(note) = c.apply(ev.clone(), now) {
        s.notice(note, false);
    }
    let showing = s.ui.chat.as_ref() == Some(&id);
    if showing && is_prompt && s.ui.composer.is_empty() && s.ui.answering.is_none() {
        s.ui.ask_active = true;
        s.ui.ask_selected = 0;
    }
    if showing && !s.ask_visible() {
        s.ui.ask_active = false;
    }
    effects
}

pub(crate) fn chat_reply(s: &mut State, what: Pending, reply: Reply) -> Vec<Effect> {
    match (what, reply) {
        (Pending::Watch(id), Reply::Watched(w)) => {
            s.ui.watching.remove(&id);
            let conv = Conversation::from_watched(*w, s.ui.now_ms);
            let prompt_waiting = !conv.prompts.is_empty();
            match s.domain.convs.get_mut(&id) {
                // 本地已经有这段对话（比如刚建的会话已经发了第一句）：它比快照新，
                // 只补快照里的状态，转录留着本地的。
                Some(existing) => existing.merge_state(conv),
                None => {
                    s.domain.convs.insert(id.clone(), conv);
                }
            }
            if s.ui.chat.as_ref() == Some(&id) && prompt_waiting && s.ui.composer.is_empty() {
                s.ui.ask_active = true;
            }
            vec![]
        }
        (Pending::Commands(id), Reply::Commands(cmds)) => {
            if let Some(c) = s.domain.convs.get_mut(&id) {
                c.commands = cmds;
            }
            vec![]
        }
        (Pending::Submitted { session, block }, Reply::Submitted { message }) => {
            if let Some(c) = s.domain.convs.get_mut(&session)
                && let Some(b) = c.block_mut(block)
                && let BlockKind::User { pending, steer, .. } = &mut b.kind
                && *steer
            {
                *pending = Some(message);
                b.cache = None;
            }
            vec![]
        }
        (Pending::CreateThenSubmit { text }, Reply::Session(summary)) => {
            let now = s.ui.now_ms;
            let id = summary.id.clone();
            let mut e = open_session(s, id.clone());
            // 会话快照回来之前先放一个空的，好让这句话马上出现在转录里。
            s.domain.convs.entry(id.clone()).or_insert_with(|| {
                Conversation::from_watched(
                    port::Watched {
                        session: summary,
                        history: vec![],
                        running: false,
                        goal: None,
                        todos: vec![],
                        tasks: vec![],
                        prompts: vec![],
                    },
                    now,
                )
            });
            e.retain(|x| match x {
                Effect::Call(req, c) if matches!(**c, Command::Watch { .. }) => {
                    s.pending.remove(req);
                    false
                }
                _ => true,
            });
            e.extend(send(s, id, text));
            e
        }
        (Pending::CreateOnly, Reply::Session(summary)) => open_session(s, summary.id),
        (Pending::TaskOutput(task), Reply::TaskOutput { text, .. }) => {
            s.domain.task_output.insert(task, text);
            vec![]
        }
        _ => vec![],
    }
}
