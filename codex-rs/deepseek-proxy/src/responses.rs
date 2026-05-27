//! OpenAI Responses API request types. Only the fields the translator looks
//! at are typed; everything else is parsed as `serde_json::Value` so unknown
//! Codex-side additions deserialize cleanly. Built from the public Responses
//! API docs and the shape Codex actually sends on the wire.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// A `POST /v1/responses` request body.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponsesRequest {
    pub model: String,

    /// Either a plain string (single user message) or an array of input items.
    #[serde(default)]
    pub input: Option<Input>,

    /// System prompt prepended to the conversation.
    #[serde(default)]
    pub instructions: Option<String>,

    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,

    /// Codex always sets `stream: true`. B1 honours whatever the client sends;
    /// B2 will own SSE response translation.
    #[serde(default)]
    pub stream: Option<bool>,

    #[serde(default)]
    pub tools: Option<Vec<ResponsesTool>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    /// Codex's reasoning config. B4 maps `reasoning.effort` to DeepSeek's
    /// `reasoning_effort`; `summary` is dropped (DeepSeek doesn't expose it).
    #[serde(default)]
    pub reasoning: Option<ReasoningConfig>,
    //
    // Codex also sends these but the translator ignores them; serde drops
    // unknown JSON keys silently. The
    // `dropped_request_fields_do_not_break_deserialization` test pins that
    // contract: previous_response_id, store, text, service_tier, metadata,
    // include, user, prompt_cache_key, prompt_cache_retention.
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReasoningConfig {
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Input {
    /// Shorthand: a single string is treated as one user message.
    Text(String),
    /// Full form: an ordered list of typed items.
    Items(Vec<InputItem>),
}

/// One entry of the `input` array. Untyped fields are stored as `Value` so the
/// translator can defer richer handling to later phases.
///
/// Several fields (`output`, `summary`, …) are unused in B1 but must remain
/// present so `serde_json::from_*` accepts Codex's traffic without
/// `deny_unknown_fields` surprises in B3/B4 when we start reading them.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct InputItem {
    /// `"message" | "function_call" | "function_call_output" | "reasoning" |
    /// "custom_tool_call" | "local_shell_call" | ...`. Defaults to `"message"`
    /// when absent (Responses lets messages omit the type field).
    #[serde(rename = "type", default)]
    pub item_type: Option<String>,

    /// `"user" | "assistant" | "system" | "developer"` for message items.
    #[serde(default)]
    pub role: Option<String>,

    /// String or `[ContentPart]`. Parsed lazily.
    #[serde(default)]
    pub content: Option<Value>,

    // function_call / function_call_output / tool fields — surfaced in B3.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub output: Option<Value>,

    // reasoning summary — surfaced in B4.
    #[serde(default)]
    pub summary: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponsesTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Option<Value>,
    #[serde(default)]
    pub strict: Option<bool>,
}

// ============================================================================
// Output side — types we emit back to the Codex client. Mirrors what the
// official OpenAI Responses SSE stream sends. Only the subset needed for B2
// (text-only message item, single output, simple usage) is typed.
// ============================================================================

/// Snapshot of the `response` object as embedded in lifecycle events. We never
/// model the full `Response` (there are dozens of optional fields); the
/// snapshot only carries what Codex needs to recognise the in-flight response.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseSnapshot {
    pub id: String,
    pub object: &'static str,
    pub created_at: i64,
    pub status: String,
    pub model: String,
    pub output: Vec<OutputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// An entry of `response.output[]`. Serialised with an internal `type` tag
/// matching the OpenAI Responses API.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputItem {
    Message(MessageItem),
    FunctionCall(FunctionCallItem),
    Reasoning(ReasoningItem),
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageItem {
    pub id: String,
    pub status: String,
    pub role: String,
    pub content: Vec<OutputContentPart>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionCallItem {
    pub id: String,
    pub status: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReasoningItem {
    pub id: String,
    pub status: String,
    pub summary: Vec<ReasoningSummaryPart>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReasoningSummaryPart {
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputContentPart {
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens_details: Option<OutputTokensDetails>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputTokensDetails {
    pub reasoning_tokens: u64,
}
