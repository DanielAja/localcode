//! Reliable tool-call extraction, independent of model family.
//!
//! Primary path: `llama-server --jinja` returns structured `tool_calls` on the
//! assistant message — we use those directly. Fallback path: weaker models emit a
//! tool call as *text* in `content` (Hermes/Qwen `<tool_call>{json}</tool_call>`,
//! or a bare JSON object). We forgivingly scan for the first balanced block and
//! auto-repair. If nothing parses, the agent re-prompts with the schema error.

use crate::engine::{ChatMessage, FunctionCall, ToolCall};

#[derive(Debug, Default)]
pub struct Extracted {
    pub calls: Vec<ToolCall>,
    /// Assistant prose that accompanied the message (if any).
    pub text: Option<String>,
}

/// Extract tool calls from an assistant message.
pub fn extract(msg: &ChatMessage) -> Extracted {
    // 1. Native structured tool_calls (the --jinja happy path).
    if let Some(tc) = &msg.tool_calls {
        if !tc.is_empty() {
            return Extracted {
                calls: reindex(tc.clone()),
                text: msg.content.clone().filter(|s| !s.trim().is_empty()),
            };
        }
    }
    // 2. Forgiving text parse from content.
    let content = msg.content.clone().unwrap_or_default();
    let calls = parse_text_tool_calls(&content);
    if calls.is_empty() {
        Extracted {
            calls,
            text: if content.trim().is_empty() { None } else { Some(content) },
        }
    } else {
        Extracted { calls, text: None }
    }
}

/// Parse tool calls embedded as text. Handles `<tool_call>…</tool_call>` blocks and
/// a bare/first-balanced JSON object `{"name":…,"arguments"|"parameters":…}`.
pub fn parse_text_tool_calls(content: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    const OPEN: &str = "<tool_call>";
    const CLOSE: &str = "</tool_call>";
    let mut rest = content;
    while let Some(s) = rest.find(OPEN) {
        let after = &rest[s + OPEN.len()..];
        match after.find(CLOSE) {
            Some(e) => {
                if let Some(c) = parse_one_json_call(after[..e].trim()) {
                    calls.push(c);
                }
                rest = &after[e + CLOSE.len()..];
            }
            None => {
                // Unterminated block (model dropped the closing tag) — auto-repair by
                // parsing the first balanced object in the remainder.
                if let Some(block) = first_balanced_json(after) {
                    if let Some(c) = parse_one_json_call(&block) {
                        calls.push(c);
                    }
                }
                break;
            }
        }
    }
    if !calls.is_empty() {
        return reindex(calls);
    }

    // No tagged blocks — try the first balanced JSON object in the whole message.
    if let Some(block) = first_balanced_json(content) {
        if let Some(c) = parse_one_json_call(&block) {
            calls.push(c);
        }
    }
    reindex(calls)
}

/// Parse one `{"name":…, "arguments"|"parameters": {…}}` object into a ToolCall.
fn parse_one_json_call(s: &str) -> Option<ToolCall> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let obj = v.as_object()?;
    // Some templates nest under "function".
    let obj = obj
        .get("function")
        .and_then(|f| f.as_object())
        .unwrap_or(obj);
    let name = obj.get("name")?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    let args_val = obj
        .get("arguments")
        .or_else(|| obj.get("parameters"))
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    // arguments may itself be a JSON-encoded string or an object.
    let arguments = match args_val {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    };
    Some(ToolCall {
        id: String::new(),
        call_type: "function".to_string(),
        function: FunctionCall { name, arguments },
    })
}

/// Return the first balanced `{…}` substring, respecting strings/escapes.
fn first_balanced_json(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s.char_indices().filter(|(i, _)| *i >= start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..i + c.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn reindex(mut calls: Vec<ToolCall>) -> Vec<ToolCall> {
    for (i, c) in calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{i}");
        }
        if c.call_type.is_empty() {
            c.call_type = "function".to_string();
        }
    }
    calls
}
