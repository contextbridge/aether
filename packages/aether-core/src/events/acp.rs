use super::AgentMessage;
use acp_utils::AETHER_TOOL_NAME_META_KEY;
use agent_client_protocol::schema::{
    ContentBlock, ContentChunk, Meta, SessionUpdate, StopReason, ToolCall, ToolCallContent, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields,
};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use std::collections::HashMap;

pub struct AcpAgentMessageMapper {
    model_name: String,
    active_message: Option<BufferedMessage>,
    tool_calls: HashMap<String, ToolCallState>,
    message_counter: usize,
}

impl AcpAgentMessageMapper {
    pub fn new(model_name: impl Into<String>) -> Self {
        Self { model_name: model_name.into(), active_message: None, tool_calls: HashMap::new(), message_counter: 1 }
    }

    pub fn map_update(&mut self, update: SessionUpdate) -> Vec<AgentMessage> {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => self.buffer_chunk(&chunk, MessageKind::Text),
            SessionUpdate::AgentThoughtChunk(chunk) => self.buffer_chunk(&chunk, MessageKind::Thought),
            SessionUpdate::ToolCall(tool_call) => self.map_tool_call(tool_call),
            SessionUpdate::ToolCallUpdate(update) => self.map_tool_call_update(update),
            _ => Vec::new(),
        }
    }

    pub fn finish(&mut self, stop_reason: StopReason) -> Vec<AgentMessage> {
        let mut messages = self.flush_buffered();
        messages.push(map_stop_reason(stop_reason));
        messages
    }

    pub fn flush_buffered(&mut self) -> Vec<AgentMessage> {
        self.active_message.take().map_or_else(Vec::new, |buffered| vec![buffered.into_agent_message(&self.model_name)])
    }

    fn buffer_chunk(&mut self, chunk: &ContentChunk, kind: MessageKind) -> Vec<AgentMessage> {
        let Some(text) = content_block_text(&chunk.content) else { return Vec::new() };
        let message_id = self.message_id_for(chunk, kind);
        self.buffer_message(BufferedMessage { kind, message_id, text })
    }

    fn buffer_message(&mut self, incoming: BufferedMessage) -> Vec<AgentMessage> {
        if let Some(buffered) = &mut self.active_message
            && buffered.kind == incoming.kind
            && buffered.message_id == incoming.message_id
        {
            buffered.text.push_str(&incoming.text);
            return Vec::new();
        }

        let previous = self.active_message.replace(incoming);
        previous.map_or_else(Vec::new, |buffered| vec![buffered.into_agent_message(&self.model_name)])
    }

    fn message_id_for(&mut self, chunk: &ContentChunk, kind: MessageKind) -> String {
        if let Some(message_id) = &chunk.message_id {
            return message_id.0.to_string();
        }

        if let Some(buffered) = &self.active_message
            && buffered.kind == kind
        {
            return buffered.message_id.clone();
        }

        self.next_message_id()
    }

    fn next_message_id(&mut self) -> String {
        let message_id = format!("acp_msg_{}", self.message_counter);
        self.message_counter += 1;
        message_id
    }

    fn map_tool_call(&mut self, tool_call: ToolCall) -> Vec<AgentMessage> {
        let mut messages = self.flush_buffered();
        let id = tool_call.tool_call_id.0.to_string();
        let name = tool_name_from_meta(tool_call.meta.as_ref()).unwrap_or_else(|| tool_call.title.clone());
        let state = self.tool_calls.entry(id.clone()).or_default();
        state.apply_tool_call(name, tool_call.raw_input.as_ref(), tool_call.content, tool_call.raw_output);

        messages.push(AgentMessage::ToolCall {
            request: ToolCallRequest { id, name: state.name(), arguments: state.arguments.clone() },
            model_name: self.model_name.clone(),
        });
        messages
    }

    fn map_tool_call_update(&mut self, update: ToolCallUpdate) -> Vec<AgentMessage> {
        let mut messages = self.flush_buffered();
        let id = update.tool_call_id.0.to_string();
        let update_result = self.tool_calls.entry(id.clone()).or_default().apply_update(update.fields);

        if update_result.arguments_changed
            && !update_result.is_terminal()
            && let Some(state) = self.tool_calls.get(&id)
        {
            messages.push(AgentMessage::ToolCallUpdate {
                tool_call_id: id.clone(),
                chunk: state.arguments.clone(),
                model_name: self.model_name.clone(),
            });
        }

        match update_result.status {
            Some(ToolCallStatus::Completed) => {
                if let Some(state) = self.tool_calls.remove(&id) {
                    messages.push(state.into_tool_result(id, &self.model_name));
                }
            }
            Some(ToolCallStatus::Failed) => {
                if let Some(state) = self.tool_calls.remove(&id) {
                    messages.push(state.into_tool_error(id, &self.model_name));
                }
            }
            _ => {}
        }

        messages
    }
}

pub fn aether_tool_name_meta(name: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert(AETHER_TOOL_NAME_META_KEY.to_string(), name.to_string().into());
    meta
}

pub fn tool_name_from_meta(meta: Option<&Meta>) -> Option<String> {
    meta?.get(AETHER_TOOL_NAME_META_KEY).and_then(serde_json::Value::as_str).map(ToString::to_string)
}

pub fn parse_tool_call_chunk(chunk: &str) -> serde_json::Value {
    serde_json::from_str(chunk).unwrap_or_else(|_| serde_json::Value::String(chunk.to_string()))
}

pub fn humanize_tool_name(name: &str) -> String {
    let base = name.split("__").last().unwrap_or(name);
    let mut result = base.replace('_', " ");
    if let Some(first) = result.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    result
}

struct BufferedMessage {
    kind: MessageKind,
    message_id: String,
    text: String,
}

impl BufferedMessage {
    fn into_agent_message(self, model_name: &str) -> AgentMessage {
        self.kind.into_agent_message(&self.message_id, &self.text, model_name)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    Text,
    Thought,
}

impl MessageKind {
    fn into_agent_message(self, message_id: &str, text: &str, model_name: &str) -> AgentMessage {
        match self {
            MessageKind::Text => AgentMessage::text(message_id, text, true, model_name),
            MessageKind::Thought => AgentMessage::thought(message_id, text, true, model_name),
        }
    }
}

struct ToolCallState {
    name: Option<String>,
    arguments: String,
    content: Vec<ToolCallContent>,
    raw_output: Option<serde_json::Value>,
}

impl Default for ToolCallState {
    fn default() -> Self {
        Self { name: None, arguments: value_to_arguments(None), content: Vec::new(), raw_output: None }
    }
}

impl ToolCallState {
    fn apply_tool_call(
        &mut self,
        name: String,
        raw_input: Option<&serde_json::Value>,
        content: Vec<ToolCallContent>,
        raw_output: Option<serde_json::Value>,
    ) {
        self.name = Some(name);
        if let Some(raw_input) = raw_input {
            self.arguments = value_to_arguments(Some(raw_input));
        }
        if !content.is_empty() {
            self.content = content;
        }
        if let Some(raw_output) = raw_output {
            self.raw_output = Some(raw_output);
        }
    }

    fn apply_update(&mut self, fields: ToolCallUpdateFields) -> ToolCallUpdateResult {
        if self.name.is_none() {
            self.name = fields.title;
        }

        let arguments_changed = if let Some(raw_input) = &fields.raw_input {
            self.arguments = value_to_arguments(Some(raw_input));
            true
        } else {
            false
        };

        if let Some(content) = fields.content {
            self.content = content;
        }
        if let Some(raw_output) = fields.raw_output {
            self.raw_output = Some(raw_output);
        }

        ToolCallUpdateResult { status: fields.status, arguments_changed }
    }

    fn name(&self) -> String {
        self.name.clone().unwrap_or_else(|| "tool".to_string())
    }

    fn into_tool_result(self, id: String, model_name: &str) -> AgentMessage {
        AgentMessage::ToolResult {
            result: ToolCallResult {
                id,
                name: self.name(),
                arguments: self.arguments,
                result: tool_result_text(&self.content, self.raw_output.as_ref()),
            },
            result_meta: None,
            model_name: model_name.to_string(),
        }
    }

    fn into_tool_error(self, id: String, model_name: &str) -> AgentMessage {
        AgentMessage::ToolError {
            error: ToolCallError {
                id,
                name: self.name(),
                arguments: Some(self.arguments),
                error: tool_result_text(&self.content, self.raw_output.as_ref()),
            },
            model_name: model_name.to_string(),
        }
    }
}

struct ToolCallUpdateResult {
    status: Option<ToolCallStatus>,
    arguments_changed: bool,
}

impl ToolCallUpdateResult {
    fn is_terminal(&self) -> bool {
        matches!(self.status, Some(ToolCallStatus::Completed | ToolCallStatus::Failed))
    }
}

fn map_stop_reason(stop_reason: StopReason) -> AgentMessage {
    match stop_reason {
        StopReason::EndTurn => AgentMessage::Done,
        StopReason::Cancelled => AgentMessage::Cancelled { message: "ACP prompt cancelled".to_string() },
        StopReason::MaxTokens => AgentMessage::Error { message: "ACP prompt stopped: max_tokens".to_string() },
        StopReason::MaxTurnRequests => {
            AgentMessage::Error { message: "ACP prompt stopped: max_turn_requests".to_string() }
        }
        StopReason::Refusal => AgentMessage::Error { message: "ACP prompt stopped: refusal".to_string() },
        _ => AgentMessage::Error { message: format!("ACP prompt stopped: {stop_reason:?}") },
    }
}

fn content_block_text(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}

fn value_to_arguments(value: Option<&serde_json::Value>) -> String {
    value.map_or_else(|| "{}".to_string(), ToString::to_string)
}

fn tool_result_text(content: &[ToolCallContent], raw_output: Option<&serde_json::Value>) -> String {
    let mut parts = content.iter().filter_map(tool_content_text).collect::<Vec<_>>();
    if let Some(raw_output) = raw_output {
        parts.push(match raw_output {
            serde_json::Value::String(s) => s.clone(),
            value => value.to_string(),
        });
    }
    parts.join("\n")
}

fn tool_content_text(content: &ToolCallContent) -> Option<String> {
    match content {
        ToolCallContent::Content(content) => content_block_text(&content.content),
        ToolCallContent::Diff(diff) => Some(format!("diff {}\n{}", diff.path.display(), diff.new_text)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{
        ContentBlock, ContentChunk, MessageId, TextContent, ToolCallId, ToolCallUpdateFields,
    };

    #[test]
    fn buffers_text_until_finish() {
        let mut mapper = AcpAgentMessageMapper::new("model");
        assert!(mapper.map_update(SessionUpdate::AgentMessageChunk(text_chunk("hello "))).is_empty());
        assert!(mapper.map_update(SessionUpdate::AgentMessageChunk(text_chunk("world"))).is_empty());

        let messages = mapper.finish(StopReason::EndTurn);
        assert_eq!(messages, vec![AgentMessage::text("acp_msg_1", "hello world", true, "model"), AgentMessage::Done]);
    }

    #[test]
    fn preserves_acp_message_id_when_present() {
        let mut mapper = AcpAgentMessageMapper::new("model");
        let chunk = text_chunk("hello").message_id(MessageId::new("msg_42"));

        let messages = mapper.finish_after_update(SessionUpdate::AgentMessageChunk(chunk), StopReason::EndTurn);

        assert_eq!(messages[0], AgentMessage::text("msg_42", "hello", true, "model"));
    }

    #[test]
    fn flushes_text_before_tool_call_and_uses_meta_name() {
        let mut mapper = AcpAgentMessageMapper::new("model");
        assert!(mapper.map_update(SessionUpdate::AgentMessageChunk(text_chunk("hello"))).is_empty());
        let tool_call = ToolCall::new(ToolCallId::new("call_1"), "Read file")
            .raw_input(serde_json::json!({"filePath":"a.txt"}))
            .meta(aether_tool_name_meta("coding__read_file"));

        let messages = mapper.map_update(SessionUpdate::ToolCall(tool_call));
        assert_eq!(messages[0], AgentMessage::text("acp_msg_1", "hello", true, "model"));
        assert!(matches!(&messages[1], AgentMessage::ToolCall { request, .. } if request.name == "coding__read_file"));
    }

    #[test]
    fn maps_completed_tool_update_to_result() {
        let mut mapper = AcpAgentMessageMapper::new("model");
        mapper.map_update(SessionUpdate::ToolCall(ToolCall::new(ToolCallId::new("call_1"), "tool")));
        let fields = ToolCallUpdateFields::new()
            .status(ToolCallStatus::Completed)
            .content(vec![ToolCallContent::from(ContentBlock::Text(TextContent::new("ok")))]);

        let messages =
            mapper.map_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(ToolCallId::new("call_1"), fields)));
        assert!(matches!(&messages[0], AgentMessage::ToolResult { result, .. } if result.result == "ok"));
    }

    #[test]
    fn preserves_interleaved_text_and_thought_order() {
        let mut mapper = AcpAgentMessageMapper::new("model");
        assert!(mapper.map_update(SessionUpdate::AgentThoughtChunk(text_chunk("thinking"))).is_empty());

        let mut messages = mapper.map_update(SessionUpdate::AgentMessageChunk(text_chunk("answer")));
        messages.extend(mapper.finish(StopReason::EndTurn));

        assert_eq!(
            messages,
            vec![
                AgentMessage::thought("acp_msg_1", "thinking", true, "model"),
                AgentMessage::text("acp_msg_2", "answer", true, "model"),
                AgentMessage::Done,
            ]
        );
    }

    #[test]
    fn tool_call_merges_with_prior_update_state() {
        let mut mapper = AcpAgentMessageMapper::new("model");
        let fields = ToolCallUpdateFields::new()
            .raw_input(serde_json::json!({"filePath":"a.txt"}))
            .content(vec![ToolCallContent::from(ContentBlock::Text(TextContent::new("ok")))]);
        mapper.map_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(ToolCallId::new("call_1"), fields)));

        mapper.map_update(SessionUpdate::ToolCall(ToolCall::new(ToolCallId::new("call_1"), "tool")));
        let messages = mapper.map_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            ToolCallId::new("call_1"),
            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
        )));

        assert!(
            matches!(&messages[0], AgentMessage::ToolResult { result, .. } if result.arguments == r#"{"filePath":"a.txt"}"# && result.result == "ok")
        );
    }

    #[test]
    fn terminal_tool_update_removes_state() {
        let mut mapper = AcpAgentMessageMapper::new("model");
        mapper.map_update(SessionUpdate::ToolCall(
            ToolCall::new(ToolCallId::new("call_1"), "tool").raw_input(serde_json::json!({"first":true})),
        ));
        mapper.map_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            ToolCallId::new("call_1"),
            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
        )));

        let messages = mapper.map_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            ToolCallId::new("call_1"),
            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
        )));

        assert!(
            matches!(&messages[0], AgentMessage::ToolResult { result, .. } if result.name == "tool" && result.arguments == "{}")
        );
    }

    #[test]
    fn humanizes_tool_names() {
        assert_eq!(humanize_tool_name("coding__read_file"), "Read file");
        assert_eq!(humanize_tool_name("read_file"), "Read file");
        assert_eq!(humanize_tool_name("bash"), "Bash");
        assert_eq!(humanize_tool_name("plugins__coding__read_file"), "Read file");
    }

    trait MapperTestExt {
        fn finish_after_update(&mut self, update: SessionUpdate, stop_reason: StopReason) -> Vec<AgentMessage>;
    }

    impl MapperTestExt for AcpAgentMessageMapper {
        fn finish_after_update(&mut self, update: SessionUpdate, stop_reason: StopReason) -> Vec<AgentMessage> {
            let mut messages = self.map_update(update);
            messages.extend(self.finish(stop_reason));
            messages
        }
    }

    fn text_chunk(text: &str) -> ContentChunk {
        ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
    }
}
