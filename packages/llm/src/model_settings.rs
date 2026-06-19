use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSettings {
    /// Sampling temperature. Lower is more deterministic (e.g. `0` for grading).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling: the probability mass to sample from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Upper bound on the number of tokens generated in the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl ModelSettings {
    /// Returns `true` when no sampling controls are set.
    pub fn is_empty(&self) -> bool {
        self.temperature.is_none() && self.top_p.is_none() && self.max_tokens.is_none()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::*;

    #[test]
    fn json_round_trips_in_camel_case_and_rejects_unknown_fields() {
        assert!(ModelSettings::default().is_empty());
        assert_eq!(serde_json::to_value(ModelSettings::default()).unwrap(), serde_json::json!({}));

        let settings = ModelSettings { temperature: Some(0.0), top_p: None, max_tokens: Some(1024) };
        let json = serde_json::to_value(&settings).unwrap();
        let parsed: ModelSettings =
            serde_json::from_str(r#"{ "temperature": 0.2, "topP": 0.9, "maxTokens": 64 }"#).unwrap();

        assert!(!settings.is_empty());
        assert_eq!(json["temperature"], 0.0);
        assert_eq!(json["maxTokens"], 1024);
        assert!(json.get("topP").is_none());
        assert_eq!(parsed, ModelSettings { temperature: Some(0.2), top_p: Some(0.9), max_tokens: Some(64) });
        assert!(from_str::<ModelSettings>(r#"{ "frequencyPenalty": 1.0 }"#).is_err());
    }
}
