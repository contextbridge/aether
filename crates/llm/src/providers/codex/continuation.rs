use std::collections::HashSet;

use async_openai::types::responses::{CreateResponse, InputItem, InputParam, Item, OutputItem, OutputMessageContent};

use crate::providers::openai_responses::mappers::map_messages;
use crate::providers::openai_responses::streaming::ResponsesCompleted;
use crate::types::IsoString;
use crate::{
    AssistantReasoning, ChatMessage, EncryptedReasoningContent, LlmModel, LlmResponse, ReasoningEffort, ToolCallRequest,
};

pub(super) struct ContinuationCheckpoint {
    response_id: String,
    properties: CreateResponse,
    effort: Option<ReasoningEffort>,
    expected_prefix: Vec<InputItem>,
}

#[derive(Default, PartialEq)]
pub(super) struct DeliveredResponse {
    text: String,
    encrypted: Vec<(String, String)>,
    tools: Vec<ToolCallRequest>,
}

impl ContinuationCheckpoint {
    pub fn prepare(&self, mut request: CreateResponse, effort: Option<ReasoningEffort>) -> CreateResponse {
        let mut input = std::mem::replace(&mut request.input, InputParam::Items(Vec::new()));
        if request == self.properties
            && effort == self.effort
            && let InputParam::Items(items) = &mut input
            && items.starts_with(&self.expected_prefix)
            && items.len() > self.expected_prefix.len()
        {
            items.drain(..self.expected_prefix.len());
            request.previous_response_id = Some(self.response_id.clone());
        }
        request.input = input;
        request
    }

    pub fn capture(
        mut full: CreateResponse,
        effort: Option<ReasoningEffort>,
        completed: ResponsesCompleted,
        delivered: &DeliveredResponse,
        model: Option<LlmModel>,
    ) -> Option<Self> {
        let response_id = completed.id.filter(|id| !id.is_empty())?;
        let projected = project_completed_response(&completed.output?, delivered, model)?;
        let InputParam::Items(mut expected_prefix) = std::mem::replace(&mut full.input, InputParam::Items(Vec::new()))
        else {
            return None;
        };
        expected_prefix.extend(projected);
        Some(Self { response_id, properties: full, effort, expected_prefix })
    }
}

impl DeliveredResponse {
    pub fn observe(&mut self, response: &LlmResponse) {
        match response {
            LlmResponse::Text { chunk } => self.text.push_str(chunk),
            LlmResponse::EncryptedReasoning { id, content } => self.encrypted.push((id.clone(), content.clone())),
            LlmResponse::ToolRequestComplete { tool_call } => self.tools.push(tool_call.clone()),
            _ => {}
        }
    }
}

fn project_completed_response(
    output: &[OutputItem],
    delivered: &DeliveredResponse,
    model: Option<LlmModel>,
) -> Option<Vec<InputItem>> {
    let mut projected = DeliveredResponse::default();
    let mut call_ids = HashSet::new();
    let mut has_message = false;
    for item in output {
        match item {
            OutputItem::Message(message) => {
                if std::mem::replace(&mut has_message, true) {
                    return None;
                }
                for part in &message.content {
                    let OutputMessageContent::OutputText(text) = part else {
                        return None;
                    };
                    projected.text.push_str(&text.text);
                }
            }
            OutputItem::Reasoning(reasoning) => {
                if !projected.encrypted.is_empty() {
                    return None;
                }
                projected.encrypted.push((reasoning.id.clone()?, reasoning.encrypted_content.clone()?));
            }
            OutputItem::FunctionCall(call) => {
                if call.call_id.is_empty() || !call_ids.insert(call.call_id.clone()) {
                    return None;
                }
                projected.tools.push(ToolCallRequest {
                    id: call.call_id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                });
            }
            _ => return None,
        }
    }
    if &projected != delivered {
        return None;
    }
    let encrypted_content = if let Some((id, content)) = projected.encrypted.pop() {
        Some(EncryptedReasoningContent { id, content, model: model? })
    } else {
        None
    };
    let assistant = ChatMessage::Assistant {
        content: projected.text,
        reasoning: AssistantReasoning { summary_text: None, encrypted_content },
        tool_calls: projected.tools,
        timestamp: IsoString::now(),
    };
    let (_, items) = map_messages(&[assistant]).ok()?;
    let same_order = items.len() == output.len()
        && items.iter().zip(output).all(|(input, output)| {
            matches!(
                (input, output),
                (InputItem::EasyMessage(_), OutputItem::Message(_))
                    | (InputItem::Item(Item::Reasoning(_)), OutputItem::Reasoning(_))
                    | (InputItem::Item(Item::FunctionCall(_)), OutputItem::FunctionCall(_))
            )
        });
    same_order.then_some(items)
}
