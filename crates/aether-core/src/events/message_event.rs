use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageEvent {
    Text { message_id: String, chunk: String, is_complete: bool, model_name: String },
    Thought { message_id: String, chunk: String, is_complete: bool, model_name: String },
}
