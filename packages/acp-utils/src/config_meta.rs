use serde::{Deserialize, Serialize};
use utils::{ReasoningEffort, is_false};

type Meta = serde_json::Map<String, serde_json::Value>;

/// Meta for a top-level `SessionConfigOption` (e.g. the "model" config).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigOptionMeta {
    #[serde(default, skip_serializing_if = "is_false")]
    pub multi_select: bool,
}

/// Meta for an individual `SessionConfigSelectOption` (e.g. one model choice).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOptionMeta {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_levels: Vec<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_image: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_audio: bool,
}

impl SelectOptionMeta {
    pub fn supports_reasoning(&self) -> bool {
        !self.reasoning_levels.is_empty()
    }
}

impl ConfigOptionMeta {
    pub fn into_meta(self) -> Option<Meta> {
        if self == Self::default() {
            return None;
        }
        match serde_json::to_value(self).expect("ConfigOptionMeta should serialize") {
            serde_json::Value::Object(map) => Some(map),
            _ => unreachable!(),
        }
    }

    pub fn from_meta(meta: Option<&Meta>) -> Self {
        meta.and_then(|m| serde_json::from_value(serde_json::Value::Object(m.clone())).ok()).unwrap_or_default()
    }
}

impl SelectOptionMeta {
    pub fn into_meta(self) -> Option<Meta> {
        if self == Self::default() {
            return None;
        }
        match serde_json::to_value(self).expect("SelectOptionMeta should serialize") {
            serde_json::Value::Object(map) => Some(map),
            _ => unreachable!(),
        }
    }

    pub fn from_meta(meta: Option<&Meta>) -> Self {
        meta.and_then(|m| serde_json::from_value(serde_json::Value::Object(m.clone())).ok()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_option_meta_roundtrip() {
        let original = ConfigOptionMeta { multi_select: true };
        let meta = original.clone().into_meta();
        assert!(meta.is_some());
        let restored = ConfigOptionMeta::from_meta(meta.as_ref());
        assert_eq!(restored, original);
    }

    #[test]
    fn select_option_meta_roundtrip() {
        let original = SelectOptionMeta {
            reasoning_levels: vec![ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High],
            supports_image: true,
            supports_audio: false,
        };
        let meta = original.clone().into_meta();
        assert!(meta.is_some());
        let restored = SelectOptionMeta::from_meta(meta.as_ref());
        assert_eq!(restored, original);
    }

    #[test]
    fn default_produces_none() {
        assert!(ConfigOptionMeta::default().into_meta().is_none());
        assert!(SelectOptionMeta::default().into_meta().is_none());
    }

    #[test]
    fn from_meta_none_returns_default() {
        assert_eq!(ConfigOptionMeta::from_meta(None), ConfigOptionMeta::default());
        assert_eq!(SelectOptionMeta::from_meta(None), SelectOptionMeta::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let mut map = serde_json::Map::new();
        map.insert("multi_select".to_string(), serde_json::Value::Bool(true));
        map.insert("unknown_field".to_string(), serde_json::Value::String("hello".to_string()));
        let parsed = ConfigOptionMeta::from_meta(Some(&map));
        assert_eq!(parsed, ConfigOptionMeta { multi_select: true });
    }

    #[test]
    fn false_fields_omitted_from_serialized_output() {
        let meta = ConfigOptionMeta { multi_select: false };
        let value = serde_json::to_value(&meta).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("multi_select"));

        let meta = SelectOptionMeta::default();
        let value = serde_json::to_value(&meta).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("reasoning_levels"));
        assert!(!obj.contains_key("supports_image"));
        assert!(!obj.contains_key("supports_audio"));
    }

    #[test]
    fn supports_reasoning_convenience() {
        assert!(!SelectOptionMeta::default().supports_reasoning());
        let meta = SelectOptionMeta {
            reasoning_levels: vec![ReasoningEffort::Low, ReasoningEffort::High],
            supports_image: false,
            supports_audio: false,
        };
        assert!(meta.supports_reasoning());
    }
}
