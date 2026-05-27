//! Outbound DeepSeek / OpenAI Chat Completions request shape. Only the
//! fields B1 emits are typed; richer pieces (tool_calls, function_call
//! message variants) join in B3.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,

    /// Codex always sets `true`; left explicit so the wire is unambiguous.
    pub stream: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ChatTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,

    /// DeepSeek-specific: `"high"` or `"max"`. Only set when Codex asks for
    /// reasoning. See translate::translate_reasoning_effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,

    /// Required to get a final `usage` chunk in the SSE stream per the
    /// OpenAI Chat Completions spec. DeepSeek follows the same convention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    /// Serialised as `null` for assistant messages that carry only
    /// `tool_calls`; OpenAI/DeepSeek both accept that shape.
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatToolCall>>,
    /// Set on `role: "tool"` messages to reference which call this output
    /// satisfies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// DeepSeek thinking-mode requires the assistant's prior `reasoning_content`
    /// to be carried back on the corresponding historical assistant message.
    /// Set only on `role: "assistant"` messages that came from a thinking
    /// turn; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// Assistant message that batches one-or-more parallel tool calls.
    /// `content` is `null` per OpenAI/DeepSeek convention.
    pub fn assistant_tool_calls(calls: Vec<ChatToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(calls),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// Tool-output message that satisfies a prior `tool_calls[i]`.
    pub fn tool_output(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(call_id.into()),
            reasoning_content: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ChatToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatToolCallFunction {
    pub name: String,
    /// Stringified JSON. DeepSeek/OpenAI require a string here.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ChatFunctionDef,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatFunctionDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

// ============================================================================
// Streaming chunk shape — `data: {json}` payload of one SSE event from
// DeepSeek's /v1/chat/completions stream. B2 only consumes a subset; B3 will
// surface tool_calls and B4 will surface reasoning_content.
// ============================================================================

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// DeepSeek attaches usage on the final chunk when the request opts in
    /// (or unconditionally for chat completions). Always wrapped in `Option`
    /// because intermediate chunks have it null.
    #[serde(default)]
    pub usage: Option<ChatUsage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkChoice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub delta: ChunkDelta,
    /// `"stop" | "tool_calls" | "length" | "content_filter" | null`.
    /// B3 doesn't branch on this (we rely on the upstream sending `[DONE]`
    /// to terminate the stream). B5 may use `length`/`content_filter` to
    /// emit `response.incomplete` instead of `response.completed`.
    #[serde(default)]
    #[allow(dead_code)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChunkDelta {
    /// DeepSeek sets `role = "assistant"` on the very first chunk and never
    /// repeats it. Not currently consumed by the translator.
    #[serde(default)]
    #[allow(dead_code)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChunkToolCallDelta>>,
    /// DeepSeek thinking-mode only: the model's internal reasoning before
    /// the final answer. Streamed in fragments just like `content`.
    #[serde(default)]
    pub reasoning_content: Option<String>,
}

/// One slot of the `delta.tool_calls` array. DeepSeek streams parallel tool
/// calls as multiple deltas distinguished by `index`. `id`, `type`, and
/// `function.name` typically arrive only on the first delta for each index;
/// `function.arguments` is streamed incrementally.
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkToolCallDelta {
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    #[allow(dead_code)]
    pub call_type: Option<String>,
    #[serde(default)]
    pub function: Option<ChunkToolCallFunctionDelta>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChunkToolCallFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChatUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u64,
}
