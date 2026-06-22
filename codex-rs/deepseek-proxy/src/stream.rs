//! B2+B3+B4 — translate DeepSeek's `/v1/chat/completions` SSE stream into the
//! OpenAI Responses API SSE event stream Codex expects.
//!
//! Covered:
//! - text content stream → `response.output_text.delta` (B2)
//! - parallel tool_calls stream → `response.function_call_arguments.delta`
//!   (B3); each tool call surfaces as its own `function_call` output item
//! - thinking-mode reasoning_content stream → `response.reasoning_summary_*`
//!   on a dedicated `reasoning` output item (B4)
//! - usage.completion_tokens_details.reasoning_tokens →
//!   usage.output_tokens_details.reasoning_tokens (B4)
//!
//! Assumptions:
//! - `n = 1`: Codex never asks for parallel choices; we always read choice 0.
//!
//! Failure modes surfaced explicitly:
//! - Upstream returns non-2xx → propagated unchanged (server.rs handles that
//!   before this module runs).
//! - SSE transport error mid-stream → terminal `response.failed` event so
//!   Codex always sees a clean close.

use std::collections::BTreeMap;
use std::pin::Pin;

use async_stream::try_stream;
use axum::response::sse::Event as SseEvent;
use eventsource_stream::Eventsource;
use futures::Stream;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use tracing::warn;

use crate::chat::ChatCompletionChunk;
use crate::chat::ChatUsage;
use crate::chat::ChunkToolCallDelta;
use crate::responses::FunctionCallItem;
use crate::responses::MessageItem;
use crate::responses::OutputContentPart;
use crate::responses::OutputItem;
use crate::responses::OutputTokensDetails;
use crate::responses::ReasoningItem;
use crate::responses::ReasoningSummaryPart;
use crate::responses::ResponseSnapshot;
use crate::responses::Usage;
use crate::translate::NamespaceMap;

const ASSISTANT_ROLE: &str = "assistant";
const OUTPUT_TEXT_TYPE: &str = "output_text";
const SUMMARY_TEXT_TYPE: &str = "summary_text";
const RESPONSE_OBJECT: &str = "response";

const SSE_DONE: &str = "[DONE]";

/// State the translator carries across upstream chunks. One instance per
/// inbound `/v1/responses` request.
pub struct StreamState {
    response_id: String,
    text_item_id: String,
    reasoning_item_id: String,
    model: String,
    created_at: i64,

    seq_num: u64,
    /// Next free `output_index` to hand out. Items get indexes in arrival
    /// order: typically reasoning → text → tool calls.
    next_output_index: u32,
    /// `Some(idx)` once the reasoning item has been opened.
    reasoning_output_index: Option<u32>,
    /// Accumulated reasoning summary text.
    reasoning_text: String,
    /// True once the reasoning item's terminal events were emitted (either
    /// because non-reasoning content arrived and we flushed it, or because
    /// `lifecycle_close` ran).
    reasoning_closed: bool,
    /// `Some(idx)` once the text/message item has been opened.
    text_output_index: Option<u32>,
    /// Accumulated assistant text; replayed in `.done` events.
    accumulated_text: String,
    /// Tool-call state keyed by the upstream `delta.tool_calls[].index`. We
    /// use BTreeMap so close-out iterates in stable order.
    tool_calls: BTreeMap<u32, ToolCallState>,
    /// Final usage block from the last chunk that included it.
    final_usage: Option<ChatUsage>,
    /// Flattened-MCP-tool name (`<ns>__<tool>`) → (namespace, tool), for
    /// rebuilding the {name, namespace} pair codex routes namespace tools by.
    namespace_map: NamespaceMap,
}

struct ToolCallState {
    output_index: u32,
    item_id: String,
    call_id: String,
    name: String,
    /// `Some` when this call resolved to a flattened MCP namespace tool.
    namespace: Option<String>,
    arguments: String,
}

impl StreamState {
    pub fn new(model: String, created_at: i64, namespace_map: NamespaceMap) -> Self {
        Self {
            namespace_map,
            response_id: format!("resp_{}", uuid::Uuid::new_v4().simple()),
            text_item_id: format!("msg_{}", uuid::Uuid::new_v4().simple()),
            reasoning_item_id: format!("rs_{}", uuid::Uuid::new_v4().simple()),
            model,
            created_at,
            seq_num: 0,
            next_output_index: 0,
            reasoning_output_index: None,
            reasoning_text: String::new(),
            reasoning_closed: false,
            text_output_index: None,
            accumulated_text: String::new(),
            tool_calls: BTreeMap::new(),
            final_usage: None,
        }
    }

    fn next_seq(&mut self) -> u64 {
        let n = self.seq_num;
        self.seq_num += 1;
        n
    }

    fn next_output_index(&mut self) -> u32 {
        let n = self.next_output_index;
        self.next_output_index += 1;
        n
    }

    fn snapshot(&self, status: &str, include_output: bool) -> ResponseSnapshot {
        let mut output = Vec::new();
        if include_output {
            // Reconstruct output items in their output_index order. We may
            // have a reasoning item, a message item, and N function_call
            // items, all carrying their arrival-order indexes.
            let mut all: Vec<(u32, OutputItem)> = Vec::new();
            if let Some(idx) = self.reasoning_output_index {
                all.push((idx, OutputItem::Reasoning(self.completed_reasoning_item())));
            }
            if let Some(idx) = self.text_output_index {
                all.push((idx, OutputItem::Message(self.completed_message_item())));
            }
            for state in self.tool_calls.values() {
                all.push((
                    state.output_index,
                    OutputItem::FunctionCall(FunctionCallItem {
                        id: state.item_id.clone(),
                        status: "completed".to_string(),
                        call_id: state.call_id.clone(),
                        name: state.name.clone(),
                        namespace: state.namespace.clone(),
                        arguments: state.arguments.clone(),
                    }),
                ));
            }
            all.sort_by_key(|(i, _)| *i);
            output = all.into_iter().map(|(_, item)| item).collect();
        }
        ResponseSnapshot {
            id: self.response_id.clone(),
            object: RESPONSE_OBJECT,
            created_at: self.created_at,
            status: status.to_string(),
            model: self.model.clone(),
            output,
            usage: self.final_usage.as_ref().map(usage_from_chat),
        }
    }

    fn empty_message_item(&self) -> MessageItem {
        MessageItem {
            id: self.text_item_id.clone(),
            status: "in_progress".to_string(),
            role: ASSISTANT_ROLE.to_string(),
            content: Vec::new(),
        }
    }

    fn completed_message_item(&self) -> MessageItem {
        MessageItem {
            id: self.text_item_id.clone(),
            status: "completed".to_string(),
            role: ASSISTANT_ROLE.to_string(),
            content: vec![OutputContentPart {
                part_type: OUTPUT_TEXT_TYPE.to_string(),
                text: self.accumulated_text.clone(),
            }],
        }
    }

    fn empty_reasoning_item(&self) -> ReasoningItem {
        ReasoningItem {
            id: self.reasoning_item_id.clone(),
            status: "in_progress".to_string(),
            summary: Vec::new(),
        }
    }

    fn completed_reasoning_item(&self) -> ReasoningItem {
        ReasoningItem {
            id: self.reasoning_item_id.clone(),
            status: "completed".to_string(),
            summary: vec![ReasoningSummaryPart {
                part_type: SUMMARY_TEXT_TYPE.to_string(),
                text: self.reasoning_text.clone(),
            }],
        }
    }

    /// The two events every response stream starts with: created + in_progress.
    pub fn lifecycle_open(&mut self) -> Vec<TranslatedEvent> {
        let snap = self.snapshot("in_progress", false);
        let created = TranslatedEvent::new(
            "response.created",
            json!({
                "type": "response.created",
                "sequence_number": self.next_seq(),
                "response": snap,
            }),
        );
        let snap2 = self.snapshot("in_progress", false);
        let in_progress = TranslatedEvent::new(
            "response.in_progress",
            json!({
                "type": "response.in_progress",
                "sequence_number": self.next_seq(),
                "response": snap2,
            }),
        );
        vec![created, in_progress]
    }

    /// Consume one upstream chunk and emit any Responses events it implies.
    pub fn on_chunk(&mut self, chunk: ChatCompletionChunk) -> Vec<TranslatedEvent> {
        let mut events = Vec::new();

        if let Some(model) = chunk.model
            && !model.is_empty()
        {
            self.model = model;
        }
        if let Some(usage) = chunk.usage {
            self.final_usage = Some(usage);
        }

        // B2/B3/B4 read choice 0 only. Codex never asks for n > 1.
        let Some(choice) = chunk.choices.into_iter().find(|c| c.index == 0) else {
            return events;
        };

        // Reasoning content delta. DeepSeek streams `reasoning_content` first,
        // then the model emits regular `content` / `tool_calls` once thinking
        // finishes; we open the reasoning item lazily on first arrival.
        if let Some(reasoning_delta) = choice.delta.reasoning_content
            && !reasoning_delta.is_empty()
        {
            let output_index = match self.reasoning_output_index {
                Some(idx) => idx,
                None => {
                    let (idx, opened) = self.open_reasoning_item();
                    events.extend(opened);
                    idx
                }
            };
            self.reasoning_text.push_str(&reasoning_delta);
            let item_id = self.reasoning_item_id.clone();
            let seq = self.next_seq();
            events.push(TranslatedEvent::new(
                "response.reasoning_summary_text.delta",
                json!({
                    "type": "response.reasoning_summary_text.delta",
                    "sequence_number": seq,
                    "item_id": item_id,
                    "output_index": output_index,
                    "summary_index": 0,
                    "delta": reasoning_delta,
                }),
            ));
        }

        // First non-reasoning delta (text or tool_call) implicitly ends the
        // reasoning block — close it here so Codex can render the reasoning
        // as completed while the answer streams.
        let has_non_reasoning = choice.delta.content.is_some() || choice.delta.tool_calls.is_some();
        if has_non_reasoning && self.reasoning_output_index.is_some() && !self.reasoning_closed {
            events.extend(self.close_reasoning_item());
        }

        // Text content delta.
        if let Some(delta_text) = choice.delta.content
            && !delta_text.is_empty()
        {
            let output_index = match self.text_output_index {
                Some(idx) => idx,
                None => {
                    let (idx, opened) = self.open_message_item();
                    events.extend(opened);
                    idx
                }
            };
            self.accumulated_text.push_str(&delta_text);
            let item_id = self.text_item_id.clone();
            events.push(TranslatedEvent::new(
                "response.output_text.delta",
                json!({
                    "type": "response.output_text.delta",
                    "sequence_number": self.next_seq(),
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "delta": delta_text,
                }),
            ));
        }

        // Tool call deltas — one upstream chunk can carry deltas for multiple
        // parallel tool calls (distinct `index`es).
        if let Some(tool_call_deltas) = choice.delta.tool_calls {
            for tc in tool_call_deltas {
                events.extend(self.on_tool_call_delta(tc));
            }
        }

        // `finish_reason` is recorded implicitly by virtue of [DONE] arriving
        // next; B3 doesn't need to branch on it here. lifecycle_close emits
        // the terminal events regardless.

        events
    }

    /// Open the assistant message item, assign it the next output_index, and
    /// emit the two events Codex expects (`output_item.added` and
    /// `content_part.added`). Returns the assigned index so the caller can use
    /// it for the follow-up `output_text.delta` without re-reading state.
    fn open_message_item(&mut self) -> (u32, Vec<TranslatedEvent>) {
        let oi = self.next_output_index();
        self.text_output_index = Some(oi);
        let item_id = self.text_item_id.clone();
        let added = TranslatedEvent::new(
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": self.next_seq(),
                "output_index": oi,
                "item": OutputItem::Message(self.empty_message_item()),
            }),
        );
        let part_added = TranslatedEvent::new(
            "response.content_part.added",
            json!({
                "type": "response.content_part.added",
                "sequence_number": self.next_seq(),
                "item_id": item_id,
                "output_index": oi,
                "content_index": 0,
                "part": OutputContentPart {
                    part_type: OUTPUT_TEXT_TYPE.to_string(),
                    text: String::new(),
                },
            }),
        );
        (oi, vec![added, part_added])
    }

    /// Open the reasoning item with summary[0] empty. Mirrors `open_message_item`
    /// but for the `response.reasoning_summary_*` event family.
    fn open_reasoning_item(&mut self) -> (u32, Vec<TranslatedEvent>) {
        let oi = self.next_output_index();
        self.reasoning_output_index = Some(oi);
        let item_id = self.reasoning_item_id.clone();
        let added = TranslatedEvent::new(
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": self.next_seq(),
                "output_index": oi,
                "item": OutputItem::Reasoning(self.empty_reasoning_item()),
            }),
        );
        let part_added = TranslatedEvent::new(
            "response.reasoning_summary_part.added",
            json!({
                "type": "response.reasoning_summary_part.added",
                "sequence_number": self.next_seq(),
                "item_id": item_id,
                "output_index": oi,
                "summary_index": 0,
            }),
        );
        (oi, vec![added, part_added])
    }

    /// Emit the terminal events for a reasoning item that has been opened:
    /// `reasoning_summary_text.done` → `reasoning_summary_part.done` →
    /// `output_item.done`. Idempotent: returns empty if already closed or
    /// never opened.
    fn close_reasoning_item(&mut self) -> Vec<TranslatedEvent> {
        let mut events = Vec::new();
        let Some(oi) = self.reasoning_output_index else {
            return events;
        };
        if self.reasoning_closed {
            return events;
        }
        self.reasoning_closed = true;
        let item_id = self.reasoning_item_id.clone();
        let text = self.reasoning_text.clone();
        events.push(TranslatedEvent::new(
            "response.reasoning_summary_text.done",
            json!({
                "type": "response.reasoning_summary_text.done",
                "sequence_number": self.next_seq(),
                "item_id": item_id,
                "output_index": oi,
                "summary_index": 0,
                "text": text,
            }),
        ));
        events.push(TranslatedEvent::new(
            "response.reasoning_summary_part.done",
            json!({
                "type": "response.reasoning_summary_part.done",
                "sequence_number": self.next_seq(),
                "item_id": self.reasoning_item_id,
                "output_index": oi,
                "summary_index": 0,
            }),
        ));
        let item = self.completed_reasoning_item();
        events.push(TranslatedEvent::new(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": self.next_seq(),
                "output_index": oi,
                "item": OutputItem::Reasoning(item),
            }),
        ));
        events
    }

    fn on_tool_call_delta(&mut self, tc: ChunkToolCallDelta) -> Vec<TranslatedEvent> {
        let chunk_index = tc.index;
        let mut events = Vec::new();

        // Bootstrap the tool call on first sight. DeepSeek puts `id` and
        // `function.name` on the very first delta for each index.
        let was_new = !self.tool_calls.contains_key(&chunk_index);
        if was_new {
            let raw_name = tc
                .function
                .as_ref()
                .and_then(|f| f.name.clone())
                .unwrap_or_default();
            // A flattened MCP namespace tool (`<namespace>__<tool>`)? Rebuild the
            // {name, namespace} pair codex routes by; otherwise pass through.
            let (name, namespace) = match self.namespace_map.get(&raw_name) {
                Some((ns, bare)) => (bare.clone(), Some(ns.clone())),
                None => (raw_name, None),
            };
            let oi = self.next_output_index();
            let item_id = format!("fc_{}", uuid::Uuid::new_v4().simple());
            let call_id = tc
                .id
                .clone()
                .unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4().simple()));
            if name.is_empty() {
                warn!(
                    chunk_index,
                    "first delta for tool call missing function.name; defaulting to empty"
                );
            }
            // Emit output_item.added for the function_call item.
            events.push(TranslatedEvent::new(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "sequence_number": self.next_seq(),
                    "output_index": oi,
                    "item": OutputItem::FunctionCall(FunctionCallItem {
                        id: item_id.clone(),
                        status: "in_progress".to_string(),
                        call_id: call_id.clone(),
                        name: name.clone(),
                        namespace: namespace.clone(),
                        arguments: String::new(),
                    }),
                }),
            ));
            self.tool_calls.insert(
                chunk_index,
                ToolCallState {
                    output_index: oi,
                    item_id,
                    call_id,
                    name,
                    namespace,
                    arguments: String::new(),
                },
            );
        }

        // Mutate the (now definitely-present) entry. We use `if let Some` to
        // satisfy clippy::unwrap_used; the None branch is logically dead.
        let Some(state) = self.tool_calls.get_mut(&chunk_index) else {
            tracing::error!(chunk_index, "tool call state vanished mid-update; dropping");
            return events;
        };

        if let Some(func) = tc.function {
            // Late-arriving name fragments — uncommon but harmless to append
            // (only after the initial open, where the first fragment was
            // already absorbed into `state.name`).
            if !was_new
                && state.namespace.is_none()
                && let Some(name_frag) = func.name
                && !name_frag.is_empty()
            {
                state.name.push_str(&name_frag);
            }
            if let Some(args_delta) = func.arguments
                && !args_delta.is_empty()
            {
                state.arguments.push_str(&args_delta);
                let item_id = state.item_id.clone();
                let output_index = state.output_index;
                let seq = self.next_seq();
                events.push(TranslatedEvent::new(
                    "response.function_call_arguments.delta",
                    json!({
                        "type": "response.function_call_arguments.delta",
                        "sequence_number": seq,
                        "item_id": item_id,
                        "output_index": output_index,
                        "delta": args_delta,
                    }),
                ));
            }
        }

        events
    }

    /// Emit the closing event sequence:
    /// - For an open-but-not-closed reasoning item: summary_text.done →
    ///   summary_part.done → output_item.done
    /// - For the text item (if any): text.done → content_part.done → output_item.done
    /// - For each function_call item: arguments.done → output_item.done
    /// - Finally: response.completed
    pub fn lifecycle_close(&mut self) -> Vec<TranslatedEvent> {
        let mut events = Vec::new();

        events.extend(self.close_reasoning_item());

        if let Some(oi) = self.text_output_index {
            let item_id = self.text_item_id.clone();
            let text = self.accumulated_text.clone();
            events.push(TranslatedEvent::new(
                "response.output_text.done",
                json!({
                    "type": "response.output_text.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item_id,
                    "output_index": oi,
                    "content_index": 0,
                    "text": text,
                }),
            ));
            events.push(TranslatedEvent::new(
                "response.content_part.done",
                json!({
                    "type": "response.content_part.done",
                    "sequence_number": self.next_seq(),
                    "item_id": self.text_item_id,
                    "output_index": oi,
                    "content_index": 0,
                    "part": OutputContentPart {
                        part_type: OUTPUT_TEXT_TYPE.to_string(),
                        text: self.accumulated_text.clone(),
                    },
                }),
            ));
            let completed = self.completed_message_item();
            events.push(TranslatedEvent::new(
                "response.output_item.done",
                json!({
                    "type": "response.output_item.done",
                    "sequence_number": self.next_seq(),
                    "output_index": oi,
                    "item": OutputItem::Message(completed),
                }),
            ));
        }

        // Close out each function_call item in output_index order. BTreeMap
        // iteration is by upstream chunk index which preserves arrival order;
        // output_index follows arrival, so iteration is monotonic.
        let tool_call_items: Vec<(u32, String, FunctionCallItem)> = self
            .tool_calls
            .values()
            .map(|s| {
                (
                    s.output_index,
                    s.item_id.clone(),
                    FunctionCallItem {
                        id: s.item_id.clone(),
                        status: "completed".to_string(),
                        call_id: s.call_id.clone(),
                        name: s.name.clone(),
                        namespace: s.namespace.clone(),
                        arguments: s.arguments.clone(),
                    },
                )
            })
            .collect();
        for (output_index, item_id, item) in tool_call_items {
            events.push(TranslatedEvent::new(
                "response.function_call_arguments.done",
                json!({
                    "type": "response.function_call_arguments.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item_id,
                    "output_index": output_index,
                    "arguments": item.arguments,
                }),
            ));
            events.push(TranslatedEvent::new(
                "response.output_item.done",
                json!({
                    "type": "response.output_item.done",
                    "sequence_number": self.next_seq(),
                    "output_index": output_index,
                    "item": OutputItem::FunctionCall(item),
                }),
            ));
        }

        let include_output = self.text_output_index.is_some()
            || !self.tool_calls.is_empty()
            || self.reasoning_output_index.is_some();
        let snap = self.snapshot("completed", include_output);
        events.push(TranslatedEvent::new(
            "response.completed",
            json!({
                "type": "response.completed",
                "sequence_number": self.next_seq(),
                "response": snap,
            }),
        ));

        events
    }

    /// Emit a terminal `response.failed` carrying the upstream's error blob.
    pub fn lifecycle_fail(&mut self, error: &Value) -> TranslatedEvent {
        let snap = self.snapshot("failed", false);
        let payload = json!({
            "type": "response.failed",
            "sequence_number": self.next_seq(),
            "response": snap,
            "error": error,
        });
        TranslatedEvent::new("response.failed", payload)
    }
}

fn usage_from_chat(u: &ChatUsage) -> Usage {
    Usage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
        output_tokens_details: u
            .completion_tokens_details
            .as_ref()
            .map(|d| OutputTokensDetails {
                reasoning_tokens: d.reasoning_tokens,
            }),
    }
}

/// A typed SSE event the bridge emits. `event` is the SSE `event:` field,
/// `data` is JSON-serialised in the SSE `data:` field.
#[derive(Debug, Clone, Serialize)]
pub struct TranslatedEvent {
    pub event: &'static str,
    pub data: Value,
}

impl TranslatedEvent {
    fn new(event: &'static str, data: Value) -> Self {
        Self { event, data }
    }

    pub fn into_axum(self) -> SseEvent {
        SseEvent::default()
            .event(self.event)
            .data(self.data.to_string())
    }
}

/// Turn an upstream `reqwest::Response` whose body is a chat-completions SSE
/// stream into an async stream of Responses SSE events ready for axum's
/// `Sse::new(stream)`. The stream never yields an error; transport errors are
/// folded into a terminal `response.failed` event so Codex always sees a
/// clean SSE close.
pub fn translate_response_stream(
    upstream: reqwest::Response,
    model: String,
    created_at: i64,
    namespace_map: NamespaceMap,
) -> Pin<Box<dyn Stream<Item = Result<SseEvent, std::convert::Infallible>> + Send>> {
    Box::pin(try_stream! {
        let mut state = StreamState::new(model, created_at, namespace_map);
        for ev in state.lifecycle_open() {
            yield ev.into_axum();
        }

        let mut sse = upstream.bytes_stream().eventsource();
        let mut transport_error: Option<String> = None;

        while let Some(item) = sse.next().await {
            match item {
                Ok(ev) => {
                    let data = ev.data;
                    if data == SSE_DONE {
                        break;
                    }
                    match serde_json::from_str::<ChatCompletionChunk>(&data) {
                        Ok(chunk) => {
                            for translated in state.on_chunk(chunk) {
                                yield translated.into_axum();
                            }
                        }
                        Err(err) => {
                            warn!(error = %err, raw = %data, "failed to parse upstream chunk");
                        }
                    }
                }
                Err(err) => {
                    transport_error = Some(err.to_string());
                    break;
                }
            }
        }

        if let Some(msg) = transport_error {
            let error = json!({
                "message": msg,
                "type": "upstream_error",
            });
            yield state.lifecycle_fail(&error).into_axum();
        } else {
            for ev in state.lifecycle_close() {
                yield ev.into_axum();
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChunkChoice;
    use crate::chat::ChunkDelta;
    use crate::chat::ChunkToolCallFunctionDelta;
    use pretty_assertions::assert_eq;

    fn make_state() -> StreamState {
        let mut s = StreamState::new("deepseek-v4-flash".to_string(), 1_700_000_000, NamespaceMap::new());
        // Pin the ids so snapshots are stable.
        s.response_id = "resp_test".to_string();
        s.text_item_id = "msg_test".to_string();
        s.reasoning_item_id = "rs_test".to_string();
        s
    }

    fn delta_chunk(
        text: &str,
        finish: Option<&str>,
        usage: Option<ChatUsage>,
    ) -> ChatCompletionChunk {
        ChatCompletionChunk {
            model: Some("deepseek-v4-flash".to_string()),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: Some(text.to_string()),
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: finish.map(str::to_string),
            }],
            usage,
        }
    }

    fn reasoning_chunk(text: &str) -> ChatCompletionChunk {
        ChatCompletionChunk {
            model: Some("deepseek-v4-pro".to_string()),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    tool_calls: None,
                    reasoning_content: Some(text.to_string()),
                },
                finish_reason: None,
            }],
            usage: None,
        }
    }

    fn role_chunk() -> ChatCompletionChunk {
        ChatCompletionChunk {
            model: Some("deepseek-v4-flash".to_string()),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: Some("assistant".to_string()),
                    content: None,
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: None,
            }],
            usage: None,
        }
    }

    fn tool_open_chunk(index: u32, id: &str, name: &str) -> ChatCompletionChunk {
        ChatCompletionChunk {
            model: Some("deepseek-v4-flash".to_string()),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: Some("assistant".to_string()),
                    content: None,
                    tool_calls: Some(vec![ChunkToolCallDelta {
                        index,
                        id: Some(id.to_string()),
                        call_type: Some("function".to_string()),
                        function: Some(ChunkToolCallFunctionDelta {
                            name: Some(name.to_string()),
                            arguments: Some(String::new()),
                        }),
                    }]),
                    reasoning_content: None,
                },
                finish_reason: None,
            }],
            usage: None,
        }
    }

    fn tool_args_chunk(index: u32, args: &str) -> ChatCompletionChunk {
        ChatCompletionChunk {
            model: Some("deepseek-v4-flash".to_string()),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![ChunkToolCallDelta {
                        index,
                        id: None,
                        call_type: None,
                        function: Some(ChunkToolCallFunctionDelta {
                            name: None,
                            arguments: Some(args.to_string()),
                        }),
                    }]),
                    reasoning_content: None,
                },
                finish_reason: None,
            }],
            usage: None,
        }
    }

    fn names(events: &[TranslatedEvent]) -> Vec<&'static str> {
        events.iter().map(|e| e.event).collect()
    }

    // -------- B2 retained ----------

    #[test]
    fn opens_with_created_and_in_progress() {
        let mut s = make_state();
        let evs = s.lifecycle_open();
        assert_eq!(names(&evs), ["response.created", "response.in_progress"]);
        assert_eq!(evs[0].data["response"]["status"], "in_progress");
        assert_eq!(evs[0].data["response"]["id"], "resp_test");
        assert_eq!(evs[0].data["sequence_number"], 0);
        assert_eq!(evs[1].data["sequence_number"], 1);
    }

    #[test]
    fn first_content_delta_opens_message_item_then_emits_delta() {
        let mut s = make_state();
        s.lifecycle_open();
        let evs = s.on_chunk(delta_chunk("Hello", None, None));
        assert_eq!(
            names(&evs),
            [
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
            ]
        );
        assert_eq!(evs[2].data["delta"], "Hello");
        assert_eq!(evs[2].data["item_id"], "msg_test");
        assert_eq!(evs[2].data["output_index"], 0);
    }

    #[test]
    fn subsequent_content_deltas_only_emit_delta() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(delta_chunk("Hello", None, None));
        let evs = s.on_chunk(delta_chunk(" world", None, None));
        assert_eq!(names(&evs), ["response.output_text.delta"]);
        assert_eq!(evs[0].data["delta"], " world");
        assert_eq!(s.accumulated_text, "Hello world");
    }

    #[test]
    fn role_only_chunk_emits_nothing() {
        let mut s = make_state();
        s.lifecycle_open();
        let evs = s.on_chunk(role_chunk());
        assert!(evs.is_empty(), "role-only chunk should not emit events");
    }

    #[test]
    fn close_with_content_emits_full_terminal_sequence() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(delta_chunk("Hello", None, None));
        s.on_chunk(delta_chunk(
            "",
            Some("stop"),
            Some(ChatUsage {
                prompt_tokens: 5,
                completion_tokens: 1,
                total_tokens: 6,
                completion_tokens_details: None,
            }),
        ));
        let evs = s.lifecycle_close();
        assert_eq!(
            names(&evs),
            [
                "response.output_text.done",
                "response.content_part.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        assert_eq!(evs[0].data["text"], "Hello");
        assert_eq!(evs[3].data["response"]["status"], "completed");
        assert_eq!(evs[3].data["response"]["usage"]["input_tokens"], 5);
        assert_eq!(
            evs[3].data["response"]["output"][0]["content"][0]["text"],
            "Hello"
        );
    }

    #[test]
    fn close_without_any_content_still_emits_completed() {
        let mut s = make_state();
        s.lifecycle_open();
        let evs = s.lifecycle_close();
        assert_eq!(names(&evs), ["response.completed"]);
        assert!(
            evs[0].data["response"]["output"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn fail_emits_response_failed_with_upstream_error() {
        let mut s = make_state();
        s.lifecycle_open();
        let err = json!({"message": "boom", "type": "upstream_error"});
        let ev = s.lifecycle_fail(&err);
        assert_eq!(ev.event, "response.failed");
        assert_eq!(ev.data["type"], "response.failed");
        assert_eq!(ev.data["response"]["status"], "failed");
        assert_eq!(ev.data["error"], err);
    }

    #[test]
    fn sequence_numbers_are_monotonic_across_lifecycle() {
        let mut s = make_state();
        let mut all = Vec::new();
        all.extend(s.lifecycle_open());
        all.extend(s.on_chunk(delta_chunk("Hi", None, None)));
        all.extend(s.on_chunk(delta_chunk(".", Some("stop"), None)));
        all.extend(s.lifecycle_close());
        let seqs: Vec<u64> = all
            .iter()
            .map(|e| e.data["sequence_number"].as_u64().unwrap())
            .collect();
        for window in seqs.windows(2) {
            assert!(window[0] < window[1], "seqs must be monotonic: {seqs:?}");
        }
        assert_eq!(seqs[0], 0);
    }

    // -------- B3: tool calling ----------

    #[test]
    fn single_tool_call_full_lifecycle() {
        let mut s = make_state();
        s.lifecycle_open();
        let evs_open = s.on_chunk(tool_open_chunk(0, "call_xyz", "get_weather"));
        // Opening a tool call only emits output_item.added; the empty-args
        // delta is not forwarded.
        assert_eq!(names(&evs_open), ["response.output_item.added"]);
        let item = &evs_open[0].data["item"];
        assert_eq!(item["type"], "function_call");
        assert_eq!(item["call_id"], "call_xyz");
        assert_eq!(item["name"], "get_weather");
        assert_eq!(item["status"], "in_progress");
        assert_eq!(evs_open[0].data["output_index"], 0);

        let evs_args1 = s.on_chunk(tool_args_chunk(0, "{\"city\":"));
        assert_eq!(
            names(&evs_args1),
            ["response.function_call_arguments.delta"]
        );
        assert_eq!(evs_args1[0].data["delta"], "{\"city\":");

        let evs_args2 = s.on_chunk(tool_args_chunk(0, "\"Paris\"}"));
        assert_eq!(evs_args2[0].data["delta"], "\"Paris\"}");

        let evs_close = s.lifecycle_close();
        assert_eq!(
            names(&evs_close),
            [
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        assert_eq!(evs_close[0].data["arguments"], "{\"city\":\"Paris\"}");
        assert_eq!(
            evs_close[1].data["item"]["arguments"],
            "{\"city\":\"Paris\"}"
        );
        assert_eq!(evs_close[1].data["item"]["status"], "completed");

        // response.completed should include the function_call in output[].
        let output = &evs_close[2].data["response"]["output"];
        assert_eq!(output[0]["type"], "function_call");
        assert_eq!(output[0]["call_id"], "call_xyz");
        assert_eq!(output[0]["arguments"], "{\"city\":\"Paris\"}");
    }

    #[test]
    fn text_then_tool_call_indices_assigned_in_arrival_order() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(delta_chunk("thinking ", None, None));
        s.on_chunk(tool_open_chunk(0, "call_a", "f"));
        s.on_chunk(tool_args_chunk(0, "{}"));
        let close = s.lifecycle_close();
        // response.completed.output should have message at index 0 and
        // function_call at index 1, in that order.
        let output = &close[close.len() - 1].data["response"]["output"];
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[1]["type"], "function_call");
    }

    #[test]
    fn parallel_tool_calls_each_get_own_output_index() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(tool_open_chunk(0, "call_a", "f1"));
        s.on_chunk(tool_open_chunk(1, "call_b", "f2"));
        s.on_chunk(tool_args_chunk(0, "{\"x\":1}"));
        s.on_chunk(tool_args_chunk(1, "{\"y\":2}"));
        let close = s.lifecycle_close();
        // Two arguments.done + two output_item.done + completed = 5
        assert_eq!(
            names(&close),
            [
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        assert_eq!(close[0].data["arguments"], "{\"x\":1}");
        assert_eq!(close[2].data["arguments"], "{\"y\":2}");
        let output = &close[close.len() - 1].data["response"]["output"];
        assert_eq!(output[0]["call_id"], "call_a");
        assert_eq!(output[1]["call_id"], "call_b");
    }

    /// Sanity check against the EXACT JSON shape we observed from DeepSeek.
    /// Parses the chunk via serde and feeds it through `on_chunk`. Caught a
    /// real bug during B3 smoke testing.
    #[test]
    fn real_world_json_chunk_opens_tool_call_item() {
        let raw = r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_xyz","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk =
            serde_json::from_str(raw).expect("real-world chunk should parse");
        let mut s = make_state();
        s.lifecycle_open();
        let evs = s.on_chunk(chunk);
        assert!(
            !evs.is_empty(),
            "expected tool call open events; got nothing — likely a deserialise regression"
        );
        assert_eq!(evs[0].event, "response.output_item.added");
        assert_eq!(evs[0].data["item"]["call_id"], "call_xyz");
        assert_eq!(evs[0].data["item"]["name"], "get_weather");
    }

    #[test]
    fn tool_call_without_id_gets_synthesized_call_id() {
        let mut s = make_state();
        s.lifecycle_open();
        let evs = s.on_chunk(ChatCompletionChunk {
            model: None,
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![ChunkToolCallDelta {
                        index: 0,
                        id: None,
                        call_type: None,
                        function: Some(ChunkToolCallFunctionDelta {
                            name: Some("f".to_string()),
                            arguments: None,
                        }),
                    }]),
                    reasoning_content: None,
                },
                finish_reason: None,
            }],
            usage: None,
        });
        let synthesized = evs[0].data["item"]["call_id"].as_str().unwrap();
        assert!(
            synthesized.starts_with("call_"),
            "expected synthesised call_id, got {synthesized}"
        );
    }

    // -------- B4: reasoning ----------

    #[test]
    fn first_reasoning_delta_opens_reasoning_item() {
        let mut s = make_state();
        s.lifecycle_open();
        let evs = s.on_chunk(reasoning_chunk("Thinking about "));
        assert_eq!(
            names(&evs),
            [
                "response.output_item.added",
                "response.reasoning_summary_part.added",
                "response.reasoning_summary_text.delta",
            ]
        );
        let item = &evs[0].data["item"];
        assert_eq!(item["type"], "reasoning");
        assert_eq!(item["status"], "in_progress");
        assert!(item["summary"].as_array().unwrap().is_empty());
        assert_eq!(evs[0].data["output_index"], 0);
        assert_eq!(evs[2].data["summary_index"], 0);
        assert_eq!(evs[2].data["delta"], "Thinking about ");
    }

    #[test]
    fn subsequent_reasoning_deltas_only_emit_delta() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(reasoning_chunk("part one "));
        let evs = s.on_chunk(reasoning_chunk("and part two"));
        assert_eq!(names(&evs), ["response.reasoning_summary_text.delta"]);
        assert_eq!(s.reasoning_text, "part one and part two");
    }

    #[test]
    fn content_after_reasoning_closes_reasoning_first() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(reasoning_chunk("hmm"));
        let evs = s.on_chunk(delta_chunk("Final answer.", None, None));
        assert_eq!(
            names(&evs),
            [
                // close reasoning
                "response.reasoning_summary_text.done",
                "response.reasoning_summary_part.done",
                "response.output_item.done",
                // open message item
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
            ]
        );
        assert_eq!(evs[0].data["text"], "hmm");
        assert_eq!(evs[2].data["item"]["status"], "completed");
        assert_eq!(evs[3].data["output_index"], 1);
    }

    #[test]
    fn tool_call_after_reasoning_closes_reasoning_first() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(reasoning_chunk("plan"));
        let evs = s.on_chunk(tool_open_chunk(0, "call_a", "f"));
        // first 3 are reasoning close, then function_call open
        assert_eq!(
            names(&evs),
            [
                "response.reasoning_summary_text.done",
                "response.reasoning_summary_part.done",
                "response.output_item.done",
                "response.output_item.added",
            ]
        );
        assert_eq!(evs[3].data["item"]["type"], "function_call");
        assert_eq!(evs[3].data["output_index"], 1);
    }

    #[test]
    fn reasoning_only_response_emits_close_on_lifecycle() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(reasoning_chunk("just thinking, no answer"));
        let close = s.lifecycle_close();
        assert_eq!(
            names(&close),
            [
                "response.reasoning_summary_text.done",
                "response.reasoning_summary_part.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        let output = &close[3].data["response"]["output"];
        assert_eq!(output[0]["type"], "reasoning");
        assert_eq!(output[0]["status"], "completed");
        assert_eq!(output[0]["summary"][0]["type"], "summary_text");
        assert_eq!(output[0]["summary"][0]["text"], "just thinking, no answer");
    }

    #[test]
    fn reasoning_then_message_response_completed_lists_both_in_order() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(reasoning_chunk("think"));
        s.on_chunk(delta_chunk("say", None, None));
        s.on_chunk(delta_chunk(
            "",
            Some("stop"),
            Some(ChatUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
                completion_tokens_details: Some(crate::chat::CompletionTokensDetails {
                    reasoning_tokens: 1,
                }),
            }),
        ));
        let close = s.lifecycle_close();
        let last = &close[close.len() - 1].data;
        assert_eq!(last["response"]["output"][0]["type"], "reasoning");
        assert_eq!(last["response"]["output"][1]["type"], "message");
        assert_eq!(
            last["response"]["usage"]["output_tokens_details"]["reasoning_tokens"],
            1
        );
    }

    #[test]
    fn reasoning_tokens_flow_through_usage() {
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(delta_chunk(
            "x",
            Some("stop"),
            Some(ChatUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                completion_tokens_details: Some(crate::chat::CompletionTokensDetails {
                    reasoning_tokens: 42,
                }),
            }),
        ));
        let close = s.lifecycle_close();
        let last = &close[close.len() - 1].data;
        assert_eq!(
            last["response"]["usage"]["output_tokens_details"]["reasoning_tokens"],
            42
        );
    }

    /// Sanity-check: parse a chunk shaped like what DeepSeek's docs describe
    /// (`choices[].delta.reasoning_content` next to `content`). Caught
    /// deserialise regressions during B4 development.
    #[test]
    fn real_world_reasoning_chunk_parses_and_emits_delta() {
        let raw = r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":null,"reasoning_content":"Let me think..."}}]}"#;
        let chunk: ChatCompletionChunk =
            serde_json::from_str(raw).expect("real-world reasoning chunk should parse");
        let mut s = make_state();
        s.lifecycle_open();
        let evs = s.on_chunk(chunk);
        assert!(!evs.is_empty());
        // First two events open the reasoning item; third carries the delta.
        let delta = evs
            .iter()
            .find(|e| e.event == "response.reasoning_summary_text.delta")
            .expect("delta event present");
        assert_eq!(delta.data["delta"], "Let me think...");
    }

    // -------- B5: edge cases / error paths ----------

    #[test]
    fn empty_response_close_includes_no_output_and_no_usage() {
        // Upstream sent [DONE] immediately with zero chunks. We still owe Codex
        // a clean response.completed so the client unblocks.
        let mut s = make_state();
        s.lifecycle_open();
        let close = s.lifecycle_close();
        assert_eq!(names(&close), ["response.completed"]);
        let resp = &close[0].data["response"];
        assert!(resp["output"].as_array().unwrap().is_empty());
        assert_eq!(resp["status"], "completed");
        assert!(
            resp["usage"].is_null(),
            "no chunk delivered usage; field should be null"
        );
    }

    #[test]
    fn fail_after_partial_text_keeps_partial_in_state() {
        // If transport dies mid-response we emit response.failed and don't
        // synthesise close events for the open message item. The accumulated
        // text stays in StreamState for diagnostics / replay tools.
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(delta_chunk("partial answer", None, None));
        let err = json!({"message": "connection reset", "type": "upstream_error"});
        let ev = s.lifecycle_fail(&err);
        assert_eq!(ev.event, "response.failed");
        assert_eq!(ev.data["response"]["status"], "failed");
        // Partial text is preserved so a future B5+/B6 lifecycle could be
        // wired to replay it; lifecycle_fail intentionally skips message
        // close events.
        assert_eq!(s.accumulated_text, "partial answer");
    }

    #[test]
    fn reasoning_only_then_fail_emits_failed_without_closing_reasoning() {
        // If we error out while still inside the reasoning block, send the
        // failed signal — don't try to close reasoning since the summary is
        // incomplete.
        let mut s = make_state();
        s.lifecycle_open();
        s.on_chunk(reasoning_chunk("thinking..."));
        let err = json!({"message": "upstream gone", "type": "upstream_error"});
        let ev = s.lifecycle_fail(&err);
        assert_eq!(ev.event, "response.failed");
        // Reasoning was never explicitly closed.
        assert!(!s.reasoning_closed);
    }

    #[test]
    fn chunk_for_unrequested_choice_index_is_ignored() {
        // Defense in depth: if a buggy upstream sends choice index != 0,
        // skip the chunk entirely.
        let mut s = make_state();
        s.lifecycle_open();
        let chunk = ChatCompletionChunk {
            model: None,
            choices: vec![ChunkChoice {
                index: 7,
                delta: ChunkDelta {
                    role: None,
                    content: Some("ghost".to_string()),
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };
        let evs = s.on_chunk(chunk);
        assert!(evs.is_empty(), "off-index choice should not produce events");
        assert!(s.accumulated_text.is_empty());
    }
}
