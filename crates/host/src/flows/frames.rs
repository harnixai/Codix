//! Agix 的帧（Hub 分出来的那一路，`Tapped`）→ 派工器的 `Frame`。派工器只读这几样：一轮的开始
//! （带起它的那句话）、工具调用与结果、权限问句、子 Agent 的起止、一轮怎么结束的。

use std::collections::HashSet;

use harnix_flow::Usage;
use harnix_flow::runner::{Frame, TurnEnd};
use serde_json::Value;

use crate::hub::Tapped;

#[derive(Default)]
pub(crate) struct Translator {
    /// 见过开始的轮：`run_turn` 还没开始就失败的那一轮，先补一个开始，派工器才认得它。
    started: HashSet<String>,
    agents: HashSet<String>,
}

impl Translator {
    pub(crate) fn translate(&mut self, t: Tapped) -> Vec<Frame> {
        match t {
            Tapped::Failed {
                turn,
                text,
                kind,
                retryable,
                message,
            } => {
                let mut out = vec![];
                if !self.started.remove(&turn) {
                    out.push(Frame::TurnStarted {
                        turn: turn.clone(),
                        text: text.unwrap_or_default(),
                    });
                }
                out.push(Frame::TurnSettled {
                    turn,
                    end: TurnEnd::Error {
                        retryable,
                        kind,
                        message,
                    },
                    output: None,
                    usage: None,
                });
                out
            }
            Tapped::Event { turn, event, text } => {
                self.event(turn.unwrap_or_default(), &event, text)
            }
        }
    }

    fn event(&mut self, turn: String, ev: &Value, text: Option<String>) -> Vec<Frame> {
        let kind = ev.get("kind").and_then(Value::as_str);
        let (inner, agent) = match kind {
            Some("subagent_progress") => {
                let Some(id) = ev.get("agent_session_id").and_then(Value::as_str) else {
                    return vec![];
                };
                (
                    ev.get("event").unwrap_or(&Value::Null),
                    Some(id.to_string()),
                )
            }
            _ => (ev, None),
        };
        let mut out = vec![];
        if let Some(a) = &agent
            && self.agents.insert(a.clone())
        {
            out.push(Frame::AgentStarted {
                turn: turn.clone(),
                call: str_of(ev, "parent_tool_use_id"),
                agent: a.clone(),
                agent_type: str_of(ev, "agent_type"),
            });
        }
        match inner.get("kind").and_then(Value::as_str) {
            Some("turn_start") if agent.is_none() => {
                self.started.insert(turn.clone());
                out.push(Frame::TurnStarted {
                    turn,
                    text: text.unwrap_or_default(),
                });
            }
            Some("tool_use") => {
                if let Some(call) = str_of(inner, "id") {
                    out.push(Frame::ToolCall {
                        turn,
                        call,
                        tool: str_of(inner, "name").unwrap_or_default(),
                        input: inner.get("input").cloned().unwrap_or(Value::Null),
                        agent,
                    });
                }
            }
            Some("tool_result") => {
                if let Some(call) = str_of(inner, "id").or_else(|| str_of(inner, "tool_use_id")) {
                    out.push(Frame::ToolResult { turn, call, agent });
                }
            }
            // 只有权限问句归派工器答；要话的（模型的提问、MCP 的）是人的。
            Some("prompt")
                if inner
                    .get("prompt_type")
                    .and_then(Value::as_str)
                    .is_none_or(|t| t == "permission") =>
            {
                if let Some(ask) = str_of(inner, "prompt_id") {
                    out.push(Frame::Asked {
                        turn,
                        ask,
                        call: str_of(inner, "tool_use_id"),
                        tool: str_of(inner, "tool_name").unwrap_or_default(),
                        paths: inner
                            .get("paths")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|p| p.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        agent,
                    });
                }
            }
            Some("turn_complete") => match agent {
                Some(agent) => out.push(Frame::AgentEnded {
                    turn,
                    agent,
                    ok: true,
                }),
                None => {
                    self.started.remove(&turn);
                    out.push(Frame::TurnSettled {
                        turn,
                        end: match inner.get("stop_reason").and_then(Value::as_str) {
                            Some("cancelled") => TurnEnd::Interrupted,
                            _ => TurnEnd::Done,
                        },
                        output: inner
                            .get("structured_output")
                            .filter(|v| !v.is_null())
                            .cloned(),
                        usage: inner
                            .get("usage")
                            .filter(|u| u.is_object())
                            .map(Usage::of_turn),
                    });
                }
            },
            Some("error") => match agent {
                Some(agent) => out.push(Frame::AgentEnded {
                    turn,
                    agent,
                    ok: false,
                }),
                None => {
                    self.started.remove(&turn);
                    out.push(Frame::TurnSettled {
                        turn,
                        end: TurnEnd::Error {
                            retryable: inner
                                .get("retryable")
                                .and_then(Value::as_bool)
                                .unwrap_or(true),
                            kind: str_of(inner, "code").unwrap_or_else(|| "error".into()),
                            message: str_of(inner, "message").unwrap_or_default(),
                        },
                        output: None,
                        usage: None,
                    });
                }
            },
            _ => {}
        }
        out
    }
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(turn: &str, event: Value) -> Tapped {
        Tapped::Event {
            turn: Some(turn.into()),
            event,
            text: None,
        }
    }

    #[test]
    fn agix_frames_become_the_runners_and_every_turn_settles_once() {
        let mut t = Translator::default();
        assert_eq!(
            t.translate(Tapped::Event {
                turn: Some("t1".into()),
                event: json!({"kind": "turn_start"}),
                text: Some("做这一站".into()),
            }),
            [Frame::TurnStarted {
                turn: "t1".into(),
                text: "做这一站".into()
            }]
        );
        assert_eq!(
            t.translate(ev(
                "t1",
                json!({"kind": "tool_use", "id": "c1", "name": "Write", "input": {"file_path": "a"}})
            )),
            [Frame::ToolCall {
                turn: "t1".into(),
                call: "c1".into(),
                tool: "Write".into(),
                input: json!({"file_path": "a"}),
                agent: None
            }]
        );
        assert_eq!(
            t.translate(ev(
                "t1",
                json!({"kind": "prompt", "prompt_type": "permission", "prompt_id": "p1",
                       "tool_name": "Write", "tool_use_id": "c1", "paths": ["/p/a"], "message": "m"})
            )),
            [Frame::Asked {
                turn: "t1".into(),
                ask: "p1".into(),
                call: Some("c1".into()),
                tool: "Write".into(),
                paths: vec!["/p/a".into()],
                agent: None
            }]
        );
        assert!(
            t.translate(ev(
                "t1",
                json!({"kind": "prompt", "prompt_type": "question", "prompt_id": "q", "tool_use_id": "q"})
            ))
            .is_empty(),
            "要话的问句是人的"
        );
        assert_eq!(
            t.translate(ev(
                "t1",
                json!({"kind": "tool_result", "id": "c1", "name": "Write"})
            )),
            [Frame::ToolResult {
                turn: "t1".into(),
                call: "c1".into(),
                agent: None
            }]
        );
        let sub = |e: Value| {
            json!({"kind": "subagent_progress", "agent_session_id": "a1", "agent_type": "general",
                   "parent_tool_use_id": "c2", "event": e})
        };
        let f = t.translate(ev(
            "t1",
            sub(json!({"kind": "tool_use", "id": "c9", "name": "Bash", "input": {}})),
        ));
        assert!(
            matches!(&f[0], Frame::AgentStarted { agent, call, .. } if agent == "a1" && call.as_deref() == Some("c2"))
        );
        assert!(matches!(&f[1], Frame::ToolCall { agent: Some(a), .. } if a == "a1"));
        assert_eq!(
            t.translate(ev("t1", sub(json!({"kind": "turn_complete"})))),
            [Frame::AgentEnded {
                turn: "t1".into(),
                agent: "a1".into(),
                ok: true
            }]
        );
        let f = t.translate(ev(
            "t1",
            json!({"kind": "turn_complete", "stop_reason": "end_turn",
                   "usage": {"input_tokens": 3, "output_tokens": 2}, "structured_output": {"n": 1}}),
        ));
        assert!(
            matches!(&f[..], [Frame::TurnSettled { end: TurnEnd::Done, output: Some(_), usage: Some(u), .. }] if u.output_tokens == 2)
        );
        let f = t.translate(ev(
            "t2",
            json!({"kind": "turn_complete", "stop_reason": "cancelled"}),
        ));
        assert!(matches!(
            &f[..],
            [Frame::TurnSettled {
                end: TurnEnd::Interrupted,
                ..
            }]
        ));
    }

    #[test]
    fn a_turn_that_failed_before_starting_still_starts_then_settles() {
        let mut t = Translator::default();
        let f = t.translate(Tapped::Failed {
            turn: "t9".into(),
            text: Some("做".into()),
            kind: "no_credentials".into(),
            retryable: false,
            message: "none".into(),
        });
        assert!(matches!(&f[0], Frame::TurnStarted { text, .. } if text == "做"));
        assert!(matches!(
            &f[1],
            Frame::TurnSettled {
                end: TurnEnd::Error {
                    retryable: false,
                    ..
                },
                ..
            }
        ));
        // 开始过的不再补开始。
        t.translate(ev("t3", json!({"kind": "turn_start"})));
        let f = t.translate(Tapped::Failed {
            turn: "t3".into(),
            text: None,
            kind: "x".into(),
            retryable: true,
            message: String::new(),
        });
        assert_eq!(f.len(), 1);
    }
}
