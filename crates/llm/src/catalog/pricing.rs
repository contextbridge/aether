use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Provider/model prices sourced from models.dev, denominated in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: Option<f64>,
    pub cache_write_per_million: Option<f64>,
}
