use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::catalog::LlmModel;
use crate::{LlmResponse, ToolCallError, ToolCallRequest, ToolCallResult, tool_result_parts, tool_result_request};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct AssistantReply(Vec<AssistantContent>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AssistantContent {
    Text {
        text: String,
    },
    Reasoning {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted: Option<EncryptedReasoningContent>,
    },
    ToolCall(ToolCallRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EncryptedReasoningContent {
    pub id: String,
    pub content: String,
}

impl AssistantReply {
    pub fn blocks(&self) -> &[AssistantContent] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn text(&self) -> String {
        self.0
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Text { text } => Some(text.as_str()),
                AssistantContent::Reasoning { .. } | AssistantContent::ToolCall(_) => None,
            })
            .collect()
    }

    pub fn reasoning(&self) -> String {
        self.0
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Reasoning { summary, .. } => Some(summary.as_str()),
                AssistantContent::Text { .. } | AssistantContent::ToolCall(_) => None,
            })
            .collect()
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCallRequest> {
        self.0.iter().filter_map(|block| match block {
            AssistantContent::ToolCall(call) => Some(call),
            AssistantContent::Text { .. } | AssistantContent::Reasoning { .. } => None,
        })
    }

    pub fn push(&mut self, response: &LlmResponse) {
        match response {
            LlmResponse::Text { chunk } if !chunk.is_empty() => match self.0.last_mut() {
                Some(AssistantContent::Text { text }) => text.push_str(chunk),
                _ => self.0.push(AssistantContent::text(chunk.as_str())),
            },
            LlmResponse::Reasoning { chunk } if !chunk.is_empty() => match self.0.last_mut() {
                Some(AssistantContent::Reasoning { summary, encrypted: None }) => summary.push_str(chunk),
                _ => self.0.push(AssistantContent::Reasoning { summary: chunk.clone(), encrypted: None }),
            },
            LlmResponse::EncryptedReasoning { id, content } => {
                let encrypted = EncryptedReasoningContent { id: id.clone(), content: content.clone() };
                // Encrypted reasoning closes the reasoning block streamed just before it, or
                // stands alone when the provider streamed no summary for it.
                match self.0.last_mut() {
                    Some(AssistantContent::Reasoning { encrypted: open @ None, .. }) => *open = Some(encrypted),
                    _ => {
                        self.0.push(AssistantContent::Reasoning { summary: String::new(), encrypted: Some(encrypted) });
                    }
                }
            }
            LlmResponse::ToolRequestComplete { tool_call } => {
                self.0.push(AssistantContent::ToolCall(tool_call.clone()));
            }
            _ => {}
        }
    }

    pub fn estimated_bytes(&self) -> usize {
        self.0.iter().map(AssistantContent::estimated_bytes).sum()
    }

    pub(crate) fn pair_tool_calls(&mut self, results: &[Result<ToolCallResult, ToolCallError>]) {
        let answered = |id: &str| results.iter().any(|result| tool_result_parts(result).0 == id);
        self.0.retain(|block| !matches!(block, AssistantContent::ToolCall(call) if !answered(&call.id)));
        for result in results {
            let id = tool_result_parts(result).0;
            if !self.tool_calls().any(|call| call.id == id) {
                self.0.push(AssistantContent::ToolCall(tool_result_request(result)));
            }
        }
    }
}

impl From<Vec<AssistantContent>> for AssistantReply {
    fn from(blocks: Vec<AssistantContent>) -> Self {
        Self(blocks)
    }
}

impl FromIterator<AssistantContent> for AssistantReply {
    fn from_iter<T: IntoIterator<Item = AssistantContent>>(blocks: T) -> Self {
        Self(blocks.into_iter().collect())
    }
}

impl AssistantContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    fn estimated_bytes(&self) -> usize {
        match self {
            Self::Text { text } => text.len(),
            Self::Reasoning { summary, encrypted } => {
                encrypted.as_ref().map_or(summary.len(), |encrypted| encrypted.content.len())
            }
            Self::ToolCall(call) => call.name.len() + call.arguments.len(),
        }
    }
}

pub(crate) fn replays_encrypted_reasoning(author: Option<&LlmModel>, target: Option<&LlmModel>) -> bool {
    target.is_some() && author == target
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::llm_response;

    #[test]
    fn push_keeps_each_reasoning_block_where_the_model_wrote_it() {
        let responses = llm_response()
            .reasoning(&["Pl", "an"])
            .encrypted_reasoning("r_1", "first")
            .text(&["Add", "ing"])
            .reasoning(&["Check"])
            .encrypted_reasoning("r_2", "second")
            .encrypted_reasoning("r_3", "redacted")
            .tool_call("call_1", "add", &["{}"])
            .build();

        let mut reply = AssistantReply::default();
        for response in &responses {
            reply.push(response);
        }

        let reasoning = |summary: &str, id: &str, content: &str| AssistantContent::Reasoning {
            summary: summary.to_string(),
            encrypted: Some(EncryptedReasoningContent { id: id.to_string(), content: content.to_string() }),
        };
        assert_eq!(
            reply.blocks(),
            [
                reasoning("Plan", "r_1", "first"),
                AssistantContent::text("Adding"),
                reasoning("Check", "r_2", "second"),
                reasoning("", "r_3", "redacted"),
                AssistantContent::ToolCall(ToolCallRequest {
                    id: "call_1".to_string(),
                    name: "add".to_string(),
                    arguments: "{}".to_string(),
                }),
            ]
        );
    }

    #[test]
    fn reply_roundtrips_through_json() {
        let reply = AssistantReply::from(vec![
            AssistantContent::Reasoning {
                summary: "plan".to_string(),
                encrypted: Some(EncryptedReasoningContent { id: "r_1".to_string(), content: "blob".to_string() }),
            },
            AssistantContent::text("answer"),
            AssistantContent::ToolCall(ToolCallRequest {
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: "{}".to_string(),
            }),
        ]);

        let json = serde_json::to_string(&reply).unwrap();

        assert_eq!(serde_json::from_str::<AssistantReply>(&json).unwrap(), reply);
    }
}
