//! `session.history` 的消息 → 转录的 `HistoryItem`；工具参数的摘要；`TodoWrite` 的参数 → todo。
//! 实时帧与历史走同一套摘要，这样重新打开一个会话，工具行和当时看到的一样。

use port::{FileChange, FileChangeKind, HistoryItem, Todo, TodoStatus, ToolId};
use serde_json::Value;
use wire::frames::HistoryMessage;

/// 给人看的工具参数：命令、路径、查询……认不出的工具退回紧凑的 JSON。
pub fn tool_summary(name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    let picked = match name {
        "Bash" | "Monitor" => field("command"),
        "Read" | "Write" | "Edit" | "NotebookEdit" => field("file_path").or(field("path")),
        "Grep" | "Glob" => field("pattern"),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Agent" | "Task" => field("description").or(field("prompt")),
        "Skill" => field("skill").or(field("name")),
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|t| format!("{} todos", t.len())),
        _ => None,
    };
    let s = picked.unwrap_or_else(|| match input {
        Value::Object(m) if m.is_empty() => String::new(),
        Value::Null => String::new(),
        v => v.to_string(),
    });
    clip(&s, 200)
}

fn clip(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() <= max && line.len() == s.len() {
        return s.to_string();
    }
    let mut out: String = line.chars().take(max).collect();
    out.push('…');
    out
}

/// `TodoWrite` 的参数是整张清单（不是增量），所以最后一次调用就是当前的 todo。
pub fn todos_from_input(input: &Value) -> Option<Vec<Todo>> {
    let items = input.get("todos")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|t| {
                let text = t.get("content")?.as_str()?.to_string();
                let status = match t.get("status").and_then(Value::as_str) {
                    Some("in_progress") => TodoStatus::InProgress,
                    Some("completed") => TodoStatus::Done,
                    _ => TodoStatus::Pending,
                };
                Some(Todo { text, status })
            })
            .collect(),
    )
}

pub fn file_changes(v: &[wire::frames::FileChange]) -> Vec<FileChange> {
    v.iter()
        .map(|c| FileChange {
            path: c.path.clone(),
            kind: if c.op == "create" {
                FileChangeKind::Created
            } else {
                FileChangeKind::Modified
            },
            diff: c.unified_diff.clone(),
        })
        .collect()
}

/// 工具结果的内容：字符串，或内容块数组里的文字拼起来。
pub fn result_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

pub struct Converted {
    pub items: Vec<HistoryItem>,
    pub todos: Option<Vec<Todo>>,
}

pub fn convert(messages: &[HistoryMessage]) -> Converted {
    let mut items: Vec<HistoryItem> = vec![];
    let mut todos = None;
    for m in messages {
        let blocks: Vec<Value> = match &m.content {
            Value::String(s) => vec![serde_json::json!({"type": "text", "text": s})],
            Value::Array(a) => a.clone(),
            _ => vec![],
        };
        match m.role.as_str() {
            "user" => {
                // 只有人说的话画成用户消息；引擎加的提醒、压缩替换进来的不画（协议 §6.3 history）。
                let human = matches!(
                    m.origin.as_deref(),
                    None | Some("user") | Some("steer") | Some("command")
                );
                for b in &blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") if human => {
                            let text = b["text"].as_str().unwrap_or("").to_string();
                            if !text.trim().is_empty() {
                                items.push(HistoryItem::User { text });
                            }
                        }
                        Some("tool_result") => {
                            let id = b["tool_use_id"].as_str().unwrap_or("");
                            let content = result_text(&b["content"]);
                            let is_err = b["is_error"].as_bool().unwrap_or(false);
                            if let Some(HistoryItem::Tool {
                                content: c,
                                is_error,
                                ..
                            }) = items.iter_mut().rev().find(
                                |i| matches!(i, HistoryItem::Tool { id: t, .. } if t.as_str() == id),
                            ) {
                                *c = content;
                                *is_error = is_err;
                            }
                        }
                        _ => {}
                    }
                }
            }
            "assistant" => {
                for b in &blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let text = b["text"].as_str().unwrap_or("").to_string();
                            if !text.is_empty() {
                                items.push(HistoryItem::Assistant { text });
                            }
                        }
                        Some("thinking") => {
                            let text = b["thinking"].as_str().unwrap_or("").to_string();
                            if !text.is_empty() {
                                items.push(HistoryItem::Thinking { text });
                            }
                        }
                        Some("tool_use") => {
                            let name = b["name"].as_str().unwrap_or("").to_string();
                            let input = &b["input"];
                            if name == "TodoWrite"
                                && let Some(t) = todos_from_input(input)
                            {
                                todos = Some(t);
                            }
                            items.push(HistoryItem::Tool {
                                id: ToolId(b["id"].as_str().unwrap_or("").to_string()),
                                input: tool_summary(&name, input),
                                name,
                                content: String::new(),
                                is_error: false,
                                file_changes: vec![],
                            });
                        }
                        _ => {}
                    }
                }
            }
            "system" => {
                if let Value::String(s) = &m.content
                    && !s.trim().is_empty()
                {
                    items.push(HistoryItem::Notice { text: s.clone() });
                }
            }
            _ => {}
        }
    }
    Converted { items, todos }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msgs(v: Value) -> Vec<HistoryMessage> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn tools_pair_with_results_by_id_and_injected_text_is_hidden() {
        let c = convert(&msgs(json!([
            {"role":"user","origin":"user","content":[{"type":"text","text":"跑一下测试"}]},
            {"role":"user","origin":"injected","content":[{"type":"text","text":"<reminder>"}]},
            {"role":"assistant","content":[
                {"type":"thinking","thinking":"先看看","signature":"x"},
                {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}},
                {"type":"tool_use","id":"t2","name":"TodoWrite","input":{"todos":[
                    {"content":"a","status":"completed"},{"content":"b","status":"in_progress"}]}}]},
            {"role":"user","origin":"tool_result","content":[
                {"type":"tool_result","tool_use_id":"t2","content":"ok"},
                {"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"2 passed"}],"is_error":true}]},
            {"role":"assistant","content":[{"type":"text","text":"都过了"}]}
        ])));
        assert_eq!(c.items.len(), 5, "{:?}", c.items);
        assert_eq!(
            c.items[0],
            HistoryItem::User {
                text: "跑一下测试".into()
            }
        );
        let HistoryItem::Tool {
            input,
            content,
            is_error,
            ..
        } = &c.items[2]
        else {
            panic!()
        };
        assert_eq!(
            (input.as_str(), content.as_str(), *is_error),
            ("cargo test", "2 passed", true)
        );
        let todos = c.todos.unwrap();
        assert_eq!(todos[0].status, TodoStatus::Done);
        assert_eq!(todos[1].status, TodoStatus::InProgress);
    }

    #[test]
    fn summaries_are_one_short_line() {
        assert_eq!(
            tool_summary("Read", &json!({"file_path":"src/a.rs"})),
            "src/a.rs"
        );
        assert_eq!(tool_summary("Bash", &json!({"command":"a\nb"})), "a…");
        assert_eq!(tool_summary("Custom", &json!({"k":1})), "{\"k\":1}");
        assert_eq!(tool_summary("Custom", &json!({})), "");
    }
}
