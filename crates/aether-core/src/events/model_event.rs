use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Model configuration events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    /// The model was successfully switched.
    Switched { previous: String, new: String },
}
