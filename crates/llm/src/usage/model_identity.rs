use crate::LlmModel;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ModelPricing;

/// Provider, model id, and catalog pricing of the model serving a call. All
/// fields are `None` for dynamic or local models the catalog does not know.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelIdentity {
    pub provider: Option<String>,
    pub model_id: Option<String>,
    pub pricing: Option<ModelPricing>,
}

impl ModelIdentity {
    pub fn of(model: Option<&LlmModel>) -> Self {
        Self {
            provider: model.map(|model| model.provider().to_string()),
            model_id: model.map(|model| model.model_id().into_owned()),
            pricing: model.and_then(LlmModel::pricing),
        }
    }
}
