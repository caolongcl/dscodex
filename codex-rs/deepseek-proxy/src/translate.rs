//! Pure translation of an OpenAI Responses API request into a DeepSeek
//! (OpenAI-compatible) Chat Completions request. No I/O; safe to unit-test.
//!
//! What B1+B3+B4 handle:
//! - Plain-string input → one user message
//! - Array input with `type = "message"` (or untyped) → role/content messages
//! - Array input with `type = "function_call"` → coalesced into an
//!   assistant message with one or more `tool_calls`
//! - Array input with `type = "function_call_output"` → `role: "tool"`
//!   message with `tool_call_id`
//! - Array input with `type = "reasoning"` → buffered and attached as
//!   `reasoning_content` on the next assistant-emitting message. DeepSeek
//!   thinking mode REQUIRES this passback or it 400s on the next turn.
//! - `instructions` → leading system message
//! - `reasoning.effort` → DeepSeek's `reasoning_effort` (only `high` / `max`
//!   are valid upstream; lower OpenAI levels are clamped to `high`)
//! - Scalars: model / temperature / top_p / max_output_tokens / stream
//! - `tools` (function tools only) and `tool_choice` shape conversion
//!
//! What is still deferred (logs a warning and drops the item / field):
//! - `custom_tool_call` / `local_shell_call` (OpenAI extras, never)
//! - non-function tool types (`web_search`, `code_interpreter`, …)
//! - image content parts (later)

use serde_json::Value;
use serde_json::json;

use crate::chat::ChatFunctionDef;
use crate::chat::ChatMessage;
use crate::chat::ChatRequest;
use crate::chat::ChatTool;
use crate::chat::ChatToolCall;
use crate::chat::ChatToolCallFunction;
use crate::chat::StreamOptions;
use crate::chat::ThinkingConfig;
use crate::responses::Input;
use crate::responses::InputItem;
use crate::responses::ResponsesRequest;
use crate::responses::ResponsesTool;

/// Currently infallible — unknown / not-yet-supported items are logged and
/// skipped rather than rejected. We may switch this to `Result` later if a
/// real failure mode appears (e.g. malformed required field on a function_call
/// that we cannot reasonably default).
pub fn responses_to_chat(req: ResponsesRequest) -> (ChatRequest, NamespaceMap) {
    // Compute reasoning_effort up-front so the message-building pass below
    // knows whether thinking mode is active for this request.
    let reasoning_effort_raw = req.reasoning.as_ref().and_then(|r| r.effort.as_deref());
    // Explicit "thinking off" sentinel from the GUI — distinct from reasoning
    // simply being absent (which we leave alone, so DeepSeek's default-on
    // behaviour and the existing tests stay unchanged).
    let thinking_off = matches!(reasoning_effort_raw, Some("none") | Some("off"));
    let reasoning_effort = reasoning_effort_raw.and_then(translate_reasoning_effort);
    let thinking_mode = reasoning_effort.is_some();

    let mut messages = Vec::new();
    let mut pending_tool_calls: Vec<ChatToolCall> = Vec::new();
    let mut pending_reasoning_text = String::new();

    // 1. Instructions → leading system message.
    if let Some(instr) = req.instructions
        && !instr.is_empty()
    {
        messages.push(ChatMessage::system(instr));
    }

    // 2. Input items → conversation messages, batching consecutive
    //    `function_call` items into one assistant message and attaching any
    //    immediately-preceding `reasoning` summary text as
    //    `reasoning_content` on that message.
    match req.input {
        None => {}
        Some(Input::Text(text)) if !text.is_empty() => {
            messages.push(ChatMessage::user(text));
        }
        Some(Input::Text(_)) => {}
        Some(Input::Items(items)) => {
            for item in items {
                translate_input_item(
                    item,
                    &mut messages,
                    &mut pending_tool_calls,
                    &mut pending_reasoning_text,
                    thinking_mode,
                );
            }
            // Tail flush — last items in the array may be a run of function_calls
            // (uncommon but valid; e.g. a turn that ends with the assistant
            // emitting tool calls and the user hasn't supplied outputs yet).
            flush_pending_tool_calls(
                &mut messages,
                &mut pending_tool_calls,
                &mut pending_reasoning_text,
                thinking_mode,
            );
            // Reasoning at the tail with no following assistant message is
            // orphaned — drop with a warning.
            if !pending_reasoning_text.is_empty() {
                tracing::warn!(
                    "reasoning input item at end of input with no following assistant message; dropped"
                );
                pending_reasoning_text.clear();
            }
        }
    }

    let (tools, namespace_map) = match req.tools {
        Some(tools) => translate_tools(tools),
        None => (None, NamespaceMap::new()),
    };
    let tool_choice = req.tool_choice.map(translate_tool_choice);
    let stream = req.stream.unwrap_or(false);

    // Thinking mode: DeepSeek V4's reasoning ignores / interferes with
    // sampling params, per Moon Bridge's empirical experience
    // (deepseek_v4 plugin: `req.Temperature = nil; req.TopP = nil`).
    let (temperature, top_p) = if thinking_mode {
        (None, None)
    } else {
        (req.temperature, req.top_p)
    };

    // OpenAI Chat Completions only emits a final usage chunk when the
    // client explicitly opts in via `stream_options.include_usage`.
    // Without it we'd lose `reasoning_tokens`.
    let stream_options = if stream {
        Some(StreamOptions {
            include_usage: true,
        })
    } else {
        None
    };

    (
        ChatRequest {
            model: req.model,
            messages,
            temperature,
            top_p,
            max_tokens: req.max_output_tokens,
            stream,
            tools,
            tool_choice,
            parallel_tool_calls: req.parallel_tool_calls,
            reasoning_effort,
            // Only emit the switch for the explicit off sentinel (→ disabled);
            // otherwise omit it and let DeepSeek's default-on reasoning stand, so
            // normal turns and the existing translation tests are unaffected.
            thinking: thinking_off.then(|| ThinkingConfig::new(false)),
            stream_options,
        },
        namespace_map,
    )
}

/// DeepSeek's `/chat/completions` accepts `reasoning_effort` ∈ {high, max}.
/// Codex's UI exposes {low, medium, high, xhigh}; clamp / map onto the
/// upstream set. Unknown strings → "high" with a warning so reasoning still
/// kicks in (the user asked for *some* reasoning).
fn translate_reasoning_effort(effort: &str) -> Option<String> {
    match effort {
        // Thinking-off sentinel sent by the GUI when the user disables thinking:
        // no reasoning_effort → thinking_mode false → caller emits
        // `thinking: {type: disabled}` and keeps temperature/top_p.
        "none" | "off" => None,
        "high" | "low" | "medium" => Some("high".to_string()),
        "xhigh" | "max" => Some("max".to_string()),
        other => {
            tracing::warn!(
                effort = other,
                "unknown reasoning.effort value; defaulting to \"high\""
            );
            Some("high".to_string())
        }
    }
}

fn translate_input_item(
    item: InputItem,
    messages: &mut Vec<ChatMessage>,
    pending: &mut Vec<ChatToolCall>,
    pending_reasoning: &mut String,
    thinking_mode: bool,
) {
    let item_type = item.item_type.as_deref().unwrap_or("message");
    match item_type {
        "message" => {
            flush_pending_tool_calls(messages, pending, pending_reasoning, thinking_mode);
            let role = item.role.clone().unwrap_or_else(|| "user".to_string());
            let normalized_role = normalize_role(&role);
            let text = extract_text_from_content(item.content.as_ref());
            if text.is_empty() {
                tracing::warn!(role = %role, "message input item with empty content; skipped");
                return;
            }
            // Attach buffered reasoning only when the next assistant message
            // arrives. For user/system, the reasoning got orphaned (which
            // shouldn't happen in practice from Codex; warn and drop).
            let reasoning = if normalized_role == "assistant" {
                take_pending_reasoning(pending_reasoning)
            } else {
                if !pending_reasoning.is_empty() {
                    tracing::warn!(
                        role = %normalized_role,
                        "reasoning input item preceded a non-assistant message; dropped"
                    );
                    pending_reasoning.clear();
                }
                None
            };
            messages.push(ChatMessage {
                role: normalized_role,
                content: Some(text),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: reasoning,
            });
        }
        "function_call" => {
            // Coalesce consecutive function_call items into one assistant
            // message with multiple tool_calls (OpenAI parallel-tool-call
            // convention). The flush happens when we hit any other item
            // type or the input ends.
            let id = item
                .call_id
                .clone()
                .or_else(|| item.id.clone())
                .unwrap_or_default();
            let name = item.name.clone().unwrap_or_default();
            if id.is_empty() || name.is_empty() {
                tracing::warn!(
                    has_id = !id.is_empty(),
                    has_name = !name.is_empty(),
                    "function_call item missing id/name; skipped"
                );
                return;
            }
            // Per OpenAI, arguments must be a JSON-encoded string. Codex
            // already sends them that way. If a caller sends invalid JSON
            // we still forward verbatim; DeepSeek decides what to do.
            let arguments = item.arguments.clone().unwrap_or_else(|| "{}".to_string());
            pending.push(ChatToolCall {
                id,
                call_type: "function".to_string(),
                function: ChatToolCallFunction { name, arguments },
            });
        }
        "function_call_output" => {
            flush_pending_tool_calls(messages, pending, pending_reasoning, thinking_mode);
            // tool messages don't carry reasoning_content
            pending_reasoning.clear();
            let Some(call_id) = item.call_id.clone() else {
                tracing::warn!("function_call_output missing call_id; skipped");
                return;
            };
            if call_id.is_empty() {
                tracing::warn!("function_call_output has empty call_id; skipped");
                return;
            }
            let content = stringify_tool_output(item.output.as_ref());
            messages.push(ChatMessage::tool_output(call_id, content));
        }
        "custom_tool_call" | "local_shell_call" => {
            tracing::warn!(
                item_type,
                "non-function tool variant not supported by DeepSeek; skipped"
            );
        }
        "reasoning" => {
            // Buffer the reasoning summary text; attach it to the next
            // assistant-emitting message (text or tool_calls). DeepSeek's
            // thinking mode requires this passback or it 400s.
            let text = extract_reasoning_summary_text(item.summary.as_ref());
            if !text.is_empty() {
                if !pending_reasoning.is_empty() {
                    pending_reasoning.push('\n');
                }
                pending_reasoning.push_str(&text);
            }
        }
        other => {
            tracing::warn!(item_type = other, "unknown input item type; skipped");
        }
    }
}

fn flush_pending_tool_calls(
    messages: &mut Vec<ChatMessage>,
    pending: &mut Vec<ChatToolCall>,
    pending_reasoning: &mut String,
    thinking_mode: bool,
) {
    if pending.is_empty() {
        return;
    }
    let calls = std::mem::take(pending);
    let mut msg = ChatMessage::assistant_tool_calls(calls);
    msg.reasoning_content = match take_pending_reasoning(pending_reasoning) {
        Some(text) => Some(text),
        // In thinking mode DeepSeek REQUIRES `reasoning_content` to be
        // present on every assistant message that carries `tool_calls`
        // (even if empty). Without this the upstream returns
        // `Missing required thinking blocks`. Outside thinking mode the
        // field stays omitted to avoid sending noise.
        None if thinking_mode => Some(String::new()),
        None => None,
    };
    messages.push(msg);
}

fn take_pending_reasoning(buf: &mut String) -> Option<String> {
    if buf.is_empty() {
        None
    } else {
        Some(std::mem::take(buf))
    }
}

/// Extract the joined `summary[].text` from a Responses `reasoning` input
/// item. The summary array typically contains `{type: "summary_text", text: ...}`
/// entries; we concatenate the texts in order. Anything non-text is ignored.
fn extract_reasoning_summary_text(summary: Option<&Value>) -> String {
    let Some(summary) = summary else {
        return String::new();
    };
    let Some(parts) = summary.as_array() else {
        return String::new();
    };
    let mut buf = String::new();
    for part in parts {
        let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(part_type, "summary_text" | "text")
            && let Some(text) = part.get("text").and_then(|v| v.as_str())
            && !text.is_empty()
        {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(text);
        }
    }
    buf
}

/// DeepSeek expects `tool` messages to carry a string `content`. Codex sends
/// the tool's raw output, which is most often already a string but may be a
/// structured value (object/array) for some tools. Stringify non-strings via
/// JSON for round-trip safety.
fn stringify_tool_output(output: Option<&Value>) -> String {
    match output {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// "developer" is OpenAI's newer role for tool-author instructions; collapse to
/// system for upstreams that don't understand it.
fn normalize_role(role: &str) -> String {
    match role {
        "developer" => "system".to_string(),
        other => other.to_string(),
    }
}

/// `content` may be a bare string or an array of content parts. For B1 we
/// flatten text-bearing parts (`input_text`, `output_text`, plain `text`) by
/// concatenation and drop everything else with a warning.
fn extract_text_from_content(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut buf = String::new();
            for part in parts {
                if let Some(text) = text_of_content_part(part) {
                    buf.push_str(&text);
                } else {
                    let part_type = part
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("<unknown>");
                    tracing::warn!(part_type, "non-text content part skipped");
                }
            }
            buf
        }
        Value::Null => String::new(),
        other => {
            tracing::warn!(value_kind = ?other, "unexpected content value kind; skipped");
            String::new()
        }
    }
}

fn text_of_content_part(part: &Value) -> Option<String> {
    let type_ = part.get("type").and_then(Value::as_str).unwrap_or("");
    if matches!(type_, "input_text" | "output_text" | "text") {
        part.get("text").and_then(Value::as_str).map(str::to_string)
    } else {
        None
    }
}

/// Maps a flattened function name (`<namespace>__<tool>`) we expose to DeepSeek
/// back to the `(namespace, tool)` pair codex routes MCP/namespace tool calls
/// by. Built per request in `translate_tools`, consumed in `stream.rs`.
pub type NamespaceMap = std::collections::HashMap<String, (String, String)>;

const MCP_TOOL_NAME_DELIMITER: &str = "__";

/// Translate Responses tools → Chat Completions function tools.
///
/// `function` tools pass through. `namespace` tools (codex groups each MCP
/// server's tools under one; the tool's `name` is the callable namespace, e.g.
/// `mcp__memory`) are **flattened**: each sub-tool becomes a top-level function
/// named `<namespace>__<tool>`, and the returned map remembers how to rebuild
/// the `{name, namespace}` pair on the way back — DeepSeek/ChatCompletions has
/// no namespace field, but codex routes MCP calls by it. Other tool types
/// (`web_search`, `custom`/apply_patch, …) are dropped: DeepSeek can't drive them.
fn translate_tools(tools: Vec<ResponsesTool>) -> (Option<Vec<ChatTool>>, NamespaceMap) {
    let mut chat_tools: Vec<ChatTool> = Vec::new();
    let mut namespace_map = NamespaceMap::new();
    for t in tools {
        match t.tool_type.as_str() {
            "function" => chat_tools.push(ChatTool {
                tool_type: "function".to_string(),
                function: ChatFunctionDef {
                    name: t.name.unwrap_or_default(),
                    description: t.description,
                    parameters: t.parameters,
                    strict: t.strict,
                },
            }),
            "namespace" => {
                let namespace = t.name.unwrap_or_default();
                if namespace.is_empty() {
                    continue;
                }
                for sub in t.tools.into_iter().flatten() {
                    if sub.tool_type != "function" {
                        continue;
                    }
                    let bare = sub.name.unwrap_or_default();
                    if bare.is_empty() {
                        continue;
                    }
                    let flat = format!("{namespace}{MCP_TOOL_NAME_DELIMITER}{bare}");
                    namespace_map.insert(flat.clone(), (namespace.clone(), bare));
                    chat_tools.push(ChatTool {
                        tool_type: "function".to_string(),
                        function: ChatFunctionDef {
                            name: flat,
                            description: sub.description,
                            parameters: sub.parameters,
                            strict: sub.strict,
                        },
                    });
                }
            }
            other => {
                tracing::warn!(
                    tool_type = other,
                    "tool type dropped (DeepSeek supports only function tools; namespace tools are flattened)"
                );
            }
        }
    }
    let tools = (!chat_tools.is_empty()).then_some(chat_tools);
    (tools, namespace_map)
}

/// Responses uses `{"type": "function", "name": "..."}`; ChatCompletions uses
/// `{"type": "function", "function": {"name": "..."}}`. String values
/// (`"auto"`, `"none"`, `"required"`) and unknown shapes pass through.
fn translate_tool_choice(tc: Value) -> Value {
    if let Value::Object(obj) = &tc
        && obj.get("type").and_then(Value::as_str) == Some("function")
        && let Some(name) = obj.get("name").and_then(Value::as_str)
    {
        return json!({
            "type": "function",
            "function": { "name": name },
        });
    }
    tc
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn parse(req: serde_json::Value) -> ResponsesRequest {
        serde_json::from_value(req).expect("test request deserializes")
    }

    fn translate(req: serde_json::Value) -> serde_json::Value {
        let (chat, _ns) = responses_to_chat(parse(req));
        serde_json::to_value(chat).expect("chat serializes")
    }

    /// A `namespace` tool (codex's grouping for an MCP server) flattens into
    /// one function per sub-tool, named `<namespace>__<tool>`.
    #[test]
    fn namespace_tool_flattens_into_function_tools() {
        let (chat, ns) = responses_to_chat(parse(serde_json::json!({
            "model": "deepseek-v4-pro",
            "tools": [{
                "type": "namespace",
                "name": "mcp__memory",
                "tools": [
                    { "type": "function", "name": "create_entities",
                      "parameters": { "type": "object", "properties": {} } },
                    { "type": "function", "name": "read_graph",
                      "parameters": { "type": "object", "properties": {} } }
                ]
            }]
        })));
        let names: Vec<String> = chat
            .tools
            .unwrap()
            .into_iter()
            .map(|t| t.function.name)
            .collect();
        assert_eq!(
            names,
            ["mcp__memory__create_entities", "mcp__memory__read_graph"]
        );
        // …and the reverse map rebuilds the {namespace, name} codex routes by.
        assert_eq!(
            ns.get("mcp__memory__create_entities"),
            Some(&("mcp__memory".to_string(), "create_entities".to_string()))
        );
    }

    #[test]
    fn string_input_becomes_single_user_message() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": "hello"
        }));
        assert_eq!(
            out,
            json!({
                "model": "deepseek-v4-flash",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": false,
            })
        );
    }

    #[test]
    fn instructions_become_leading_system_message() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "instructions": "you are a helpful assistant",
            "input": "hi"
        }));
        assert_eq!(
            out,
            json!({
                "model": "deepseek-v4-flash",
                "messages": [
                    {"role": "system", "content": "you are a helpful assistant"},
                    {"role": "user", "content": "hi"}
                ],
                "stream": false,
            })
        );
    }

    #[test]
    fn array_input_with_content_parts_concatenates_text() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": "hello "},
                        {"type": "input_text", "text": "world"}
                    ]
                }
            ]
        }));
        assert_eq!(
            out["messages"],
            json!([{"role": "user", "content": "hello world"}])
        );
    }

    #[test]
    fn assistant_role_preserved_developer_collapses_to_system() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "message", "role": "developer", "content": "be terse"},
                {"type": "message", "role": "assistant", "content": "ok"},
                {"type": "message", "role": "user", "content": "go"}
            ]
        }));
        assert_eq!(
            out["messages"],
            json!([
                {"role": "system", "content": "be terse"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": "go"}
            ])
        );
    }

    #[test]
    fn scalars_and_stream_pass_through() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "temperature": 0.2,
            "top_p": 0.9,
            "max_output_tokens": 1024,
            "stream": true,
            "parallel_tool_calls": false
        }));
        assert_eq!(out["temperature"], json!(0.2));
        assert_eq!(out["top_p"], json!(0.9));
        assert_eq!(out["max_tokens"], json!(1024));
        assert_eq!(out["stream"], json!(true));
        assert_eq!(out["parallel_tool_calls"], json!(false));
    }

    #[test]
    fn function_tools_translate_to_nested_form() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": "x",
            "tools": [
                {
                    "type": "function",
                    "name": "get_weather",
                    "description": "Look up the weather",
                    "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
                }
            ]
        }));
        assert_eq!(
            out["tools"],
            json!([
                {
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "description": "Look up the weather",
                        "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
                    }
                }
            ])
        );
    }

    #[test]
    fn non_function_tools_are_dropped() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": "x",
            "tools": [
                {"type": "web_search"},
                {"type": "function", "name": "f"}
            ]
        }));
        assert_eq!(out["tools"].as_array().unwrap().len(), 1);
        assert_eq!(out["tools"][0]["function"]["name"], "f");
    }

    #[test]
    fn tool_choice_specific_function_becomes_nested() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": "x",
            "tool_choice": {"type": "function", "name": "f"}
        }));
        assert_eq!(
            out["tool_choice"],
            json!({"type": "function", "function": {"name": "f"}})
        );
    }

    #[test]
    fn tool_choice_string_passes_through() {
        for s in ["auto", "none", "required"] {
            let out = translate(json!({
                "model": "deepseek-v4-flash",
                "input": "x",
                "tool_choice": s
            }));
            assert_eq!(out["tool_choice"], json!(s));
        }
    }

    #[test]
    fn empty_reasoning_before_user_message_is_a_noop() {
        // Empty reasoning summary + user message: nothing to attach to;
        // reasoning drops silently.
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "reasoning", "summary": []},
                {"type": "message", "role": "user", "content": "carry on"}
            ]
        }));
        assert_eq!(
            out["messages"],
            json!([{"role": "user", "content": "carry on"}])
        );
    }

    // ---------- Reasoning passback (DeepSeek thinking-mode requirement) ----------

    #[test]
    fn reasoning_before_assistant_message_attaches_as_reasoning_content() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "reasoning", "summary": [
                    {"type": "summary_text", "text": "Let me think..."}
                ]},
                {"type": "message", "role": "assistant", "content": "Hello!"}
            ]
        }));
        assert_eq!(
            out["messages"],
            json!([
                {"role": "user", "content": "hi"},
                {
                    "role": "assistant",
                    "content": "Hello!",
                    "reasoning_content": "Let me think..."
                }
            ])
        );
    }

    #[test]
    fn reasoning_before_function_calls_attaches_to_tool_call_message() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {"type": "message", "role": "user", "content": "weather?"},
                {"type": "reasoning", "summary": [
                    {"type": "summary_text", "text": "Need to look it up."}
                ]},
                {"type": "function_call", "call_id": "c1", "name": "get_weather", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "c1", "output": "sunny"}
            ]
        }));
        assert_eq!(
            out["messages"],
            json!([
                {"role": "user", "content": "weather?"},
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{}"}
                    }],
                    "reasoning_content": "Need to look it up."
                },
                {"role": "tool", "content": "sunny", "tool_call_id": "c1"}
            ])
        );
    }

    #[test]
    fn multiple_reasoning_summary_parts_are_joined() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {"type": "reasoning", "summary": [
                    {"type": "summary_text", "text": "step 1"},
                    {"type": "summary_text", "text": "step 2"}
                ]},
                {"type": "message", "role": "assistant", "content": "done"}
            ]
        }));
        assert_eq!(out["messages"][0]["reasoning_content"], "step 1\nstep 2");
    }

    #[test]
    fn consecutive_reasoning_items_are_concatenated() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "A"}]},
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "B"}]},
                {"type": "message", "role": "assistant", "content": "ok"}
            ]
        }));
        assert_eq!(out["messages"][0]["reasoning_content"], "A\nB");
    }

    #[test]
    fn reasoning_at_tail_with_no_assistant_is_dropped() {
        // Orphaned reasoning (no following assistant) should not break the
        // request and must not leak into the next message.
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {"type": "message", "role": "user", "content": "x"},
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "orphan"}]}
            ]
        }));
        assert_eq!(out["messages"], json!([{"role": "user", "content": "x"}]));
    }

    // ---------- Moon-Bridge-aligned tweaks ----------

    #[test]
    fn thinking_mode_clears_temperature_and_top_p() {
        // Moon Bridge's deepseek_v4 plugin clears these because they
        // interfere with V4's reasoning.
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "temperature": 0.7,
            "top_p": 0.9,
            "reasoning": {"effort": "high"}
        }));
        assert!(out.get("temperature").is_none(), "should be cleared");
        assert!(out.get("top_p").is_none(), "should be cleared");
    }

    #[test]
    fn non_thinking_mode_keeps_temperature_and_top_p() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "temperature": 0.7,
            "top_p": 0.9
        }));
        assert_eq!(out["temperature"], json!(0.7));
        assert_eq!(out["top_p"], json!(0.9));
    }

    #[test]
    fn thinking_mode_tool_call_message_gets_empty_reasoning_fallback() {
        // DeepSeek requires reasoning_content on every assistant tool_call
        // message when in thinking mode. If history lacks one (e.g. the
        // original turn's reasoning got compacted out), Moon Bridge sends
        // an explicit empty string. We do the same.
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {"type": "function_call", "call_id": "c", "name": "f", "arguments": "{}"}
            ],
            "reasoning": {"effort": "high"}
        }));
        assert_eq!(out["messages"][0]["reasoning_content"], "");
    }

    #[test]
    fn non_thinking_mode_tool_call_message_omits_reasoning_content() {
        // No reasoning_effort → no fallback; field stays absent.
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {"type": "function_call", "call_id": "c", "name": "f", "arguments": "{}"}
            ]
        }));
        assert!(
            out["messages"][0].get("reasoning_content").is_none(),
            "non-thinking mode should omit reasoning_content, got: {}",
            out["messages"][0]
        );
    }

    #[test]
    fn streaming_request_sets_stream_options_include_usage() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "stream": true
        }));
        assert_eq!(out["stream"], true);
        assert_eq!(out["stream_options"], json!({"include_usage": true}));
    }

    #[test]
    fn non_streaming_request_omits_stream_options() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "stream": false
        }));
        assert!(out.get("stream_options").is_none());
    }

    #[test]
    fn reasoning_does_not_appear_on_non_assistant_messages() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "stuck"}]},
                {"type": "message", "role": "user", "content": "follow-up"}
            ]
        }));
        // Reasoning was dropped (precedes a user, not assistant). The user
        // message has no reasoning_content.
        assert!(out["messages"][0].get("reasoning_content").is_none());
    }

    // ---------- B3: function_call / function_call_output ----------

    #[test]
    fn single_function_call_becomes_assistant_tool_call_message() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "message", "role": "user", "content": "weather please"},
                {
                    "type": "function_call",
                    "call_id": "call_abc",
                    "name": "get_weather",
                    "arguments": "{\"city\":\"Paris\"}"
                }
            ]
        }));
        assert_eq!(
            out["messages"],
            json!([
                {"role": "user", "content": "weather please"},
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Paris\"}"
                        }
                    }]
                }
            ])
        );
    }

    #[test]
    fn consecutive_function_calls_batch_into_one_assistant_message() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "message", "role": "user", "content": "x"},
                {"type": "function_call", "call_id": "c1", "name": "f1", "arguments": "{}"},
                {"type": "function_call", "call_id": "c2", "name": "f2", "arguments": "{\"k\":1}"},
                {"type": "function_call_output", "call_id": "c1", "output": "out1"},
                {"type": "function_call_output", "call_id": "c2", "output": "out2"}
            ]
        }));
        assert_eq!(
            out["messages"],
            json!([
                {"role": "user", "content": "x"},
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {"id": "c1", "type": "function", "function": {"name": "f1", "arguments": "{}"}},
                        {"id": "c2", "type": "function", "function": {"name": "f2", "arguments": "{\"k\":1}"}}
                    ]
                },
                {"role": "tool", "content": "out1", "tool_call_id": "c1"},
                {"role": "tool", "content": "out2", "tool_call_id": "c2"}
            ])
        );
    }

    #[test]
    fn function_call_falls_back_to_id_when_no_call_id() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "function_call", "id": "fc_legacy", "name": "f", "arguments": "{}"}
            ]
        }));
        assert_eq!(out["messages"][0]["tool_calls"][0]["id"], "fc_legacy");
    }

    #[test]
    fn function_call_missing_id_or_name_is_dropped() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "function_call", "name": "f"},                      // no id
                {"type": "function_call", "call_id": "c"},                   // no name
                {"type": "message", "role": "user", "content": "ok"}
            ]
        }));
        assert_eq!(out["messages"], json!([{"role": "user", "content": "ok"}]));
    }

    #[test]
    fn function_call_followed_by_message_flushes_pending() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "function_call", "call_id": "c1", "name": "f1", "arguments": "{}"},
                {"type": "message", "role": "user", "content": "interrupted"}
            ]
        }));
        assert_eq!(
            out["messages"],
            json!([
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "f1", "arguments": "{}"}
                    }]
                },
                {"role": "user", "content": "interrupted"}
            ])
        );
    }

    #[test]
    fn function_call_output_with_structured_value_is_json_stringified() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "function_call", "call_id": "c", "name": "f", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "c", "output": {"k": "v", "n": 1}}
            ]
        }));
        assert_eq!(
            out["messages"][1],
            json!({
                "role": "tool",
                "content": "{\"k\":\"v\",\"n\":1}",
                "tool_call_id": "c"
            })
        );
    }

    #[test]
    fn function_call_output_without_call_id_is_dropped() {
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "function_call_output", "output": "orphan"},
                {"type": "message", "role": "user", "content": "x"}
            ]
        }));
        assert_eq!(out["messages"], json!([{"role": "user", "content": "x"}]));
    }

    // ---------- B4: reasoning_effort ----------

    #[test]
    fn reasoning_effort_high_passes_through() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "reasoning": {"effort": "high"}
        }));
        assert_eq!(out["reasoning_effort"], "high");
    }

    #[test]
    fn reasoning_effort_max_passes_through() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "reasoning": {"effort": "max"}
        }));
        assert_eq!(out["reasoning_effort"], "max");
    }

    #[test]
    fn reasoning_effort_xhigh_maps_to_max() {
        // Codex's UI uses "xhigh" but DeepSeek only accepts "high"/"max".
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "reasoning": {"effort": "xhigh"}
        }));
        assert_eq!(out["reasoning_effort"], "max");
    }

    #[test]
    fn reasoning_effort_low_or_medium_clamps_to_high() {
        for effort in ["low", "medium"] {
            let out = translate(json!({
                "model": "deepseek-v4-pro",
                "input": "x",
                "reasoning": {"effort": effort}
            }));
            assert_eq!(
                out["reasoning_effort"], "high",
                "effort={effort} should clamp to high"
            );
        }
    }

    #[test]
    fn reasoning_field_with_no_effort_omits_reasoning_effort() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "reasoning": {"summary": "auto"}
        }));
        assert!(
            out.get("reasoning_effort").is_none(),
            "reasoning_effort should be omitted when effort missing"
        );
    }

    #[test]
    fn missing_reasoning_field_omits_reasoning_effort() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x"
        }));
        assert!(out.get("reasoning_effort").is_none());
    }

    #[test]
    fn unknown_reasoning_effort_defaults_to_high() {
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "x",
            "reasoning": {"effort": "wibble"}
        }));
        assert_eq!(out["reasoning_effort"], "high");
    }

    // ---------- thinking switch (DeepSeek V4) ----------

    #[test]
    fn thinking_off_sentinel_disables_and_drops_effort() {
        // GUI "thinking off" sends effort "none": emit thinking:{disabled}, drop
        // reasoning_effort, and keep sampling params (non-thinking mode).
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "hi",
            "temperature": 0.7,
            "top_p": 0.9,
            "reasoning": {"effort": "none"}
        }));
        assert_eq!(out["thinking"], json!({"type": "disabled"}));
        assert!(
            out.get("reasoning_effort").is_none(),
            "off sentinel must drop reasoning_effort"
        );
        assert_eq!(out["temperature"], 0.7);
        assert_eq!(out["top_p"], 0.9);
    }

    #[test]
    fn thinking_on_omits_the_switch() {
        // Reasoning present → rely on DeepSeek's default-on; no `thinking` field,
        // so normal turns stay byte-identical to before this feature.
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "hi",
            "reasoning": {"effort": "high"}
        }));
        assert_eq!(out["reasoning_effort"], "high");
        assert!(
            out.get("thinking").is_none(),
            "switch should be omitted when reasoning is on"
        );
    }

    #[test]
    fn no_reasoning_field_omits_the_switch() {
        // A plain request (no reasoning at all) is left untouched.
        let out = translate(json!({
            "model": "deepseek-v4-pro",
            "input": "hi"
        }));
        assert!(out.get("thinking").is_none());
    }

    // ---------- shared ----------

    #[test]
    fn tail_function_calls_get_flushed_at_end_of_input() {
        // No following item should still emit the assistant tool_calls message.
        let out = translate(json!({
            "model": "deepseek-v4-flash",
            "input": [
                {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"}
            ]
        }));
        assert_eq!(out["messages"][0]["tool_calls"][0]["id"], "c1");
    }

    #[test]
    fn dropped_request_fields_do_not_break_deserialization() {
        // Codex sends these; the translator should drop them silently.
        let _ = translate(json!({
            "model": "deepseek-v4-flash",
            "input": "x",
            "previous_response_id": "resp_123",
            "store": false,
            "reasoning": {"effort": "high"},
            "text": {"format": {"type": "text"}},
            "service_tier": "auto",
            "metadata": {"foo": "bar"},
            "include": ["reasoning"],
            "user": "user-1",
            "prompt_cache_key": "k",
            "prompt_cache_retention": "1h"
        }));
    }
}
