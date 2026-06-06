//! The forgiving tool-call parser must handle native tool_calls, text blocks,
//! malformed/unterminated input, and plain prose.

use localcode::engine::{ChatMessage, FunctionCall, Role, ToolCall};
use localcode::toolcall;

fn assistant(content: Option<&str>, calls: Option<Vec<ToolCall>>) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: content.map(|s| s.to_string()),
        tool_calls: calls,
        tool_call_id: None,
        name: None,
    }
}

fn call(name: &str, args: &str) -> ToolCall {
    ToolCall {
        id: String::new(),
        call_type: "function".to_string(),
        function: FunctionCall { name: name.to_string(), arguments: args.to_string() },
    }
}

#[test]
fn native_tool_calls_are_used_and_reindexed() {
    let msg = assistant(None, Some(vec![call("read_file", r#"{"path":"a.rs"}"#)]));
    let ex = toolcall::extract(&msg);
    assert_eq!(ex.calls.len(), 1);
    assert_eq!(ex.calls[0].function.name, "read_file");
    assert!(!ex.calls[0].id.is_empty(), "id should be assigned");
}

#[test]
fn text_tool_call_block_is_parsed() {
    let msg = assistant(
        Some("Sure.\n<tool_call>\n{\"name\": \"bash\", \"arguments\": {\"command\": \"ls\"}}\n</tool_call>"),
        None,
    );
    let ex = toolcall::extract(&msg);
    assert_eq!(ex.calls.len(), 1);
    assert_eq!(ex.calls[0].function.name, "bash");
    assert!(ex.calls[0].function.arguments.contains("ls"));
}

#[test]
fn bare_json_with_parameters_key_is_parsed() {
    let calls = toolcall::parse_text_tool_calls(r#"{"name":"list_dir","parameters":{"path":"."}}"#);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "list_dir");
    assert!(calls[0].function.arguments.contains("path"));
}

#[test]
fn unterminated_block_is_auto_repaired() {
    let calls = toolcall::parse_text_tool_calls(r#"<tool_call>{"name":"read_file","arguments":{"path":"x"}}"#);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "read_file");
}

#[test]
fn plain_prose_yields_no_calls_and_keeps_text() {
    let ex = toolcall::extract(&assistant(Some("Here is an explanation, no tool call."), None));
    assert!(ex.calls.is_empty());
    assert!(ex.text.is_some());
}

#[test]
fn nested_function_wrapper_is_unwrapped() {
    let calls =
        toolcall::parse_text_tool_calls(r#"{"function":{"name":"grep","arguments":{"pattern":"fn"}}}"#);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "grep");
}
