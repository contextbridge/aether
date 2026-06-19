use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::Path;

pub const PLAN_REVIEW_UI_KIND: &str = "planReview";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanReviewDecision {
    Approve,
    Deny,
}

impl PlanReviewDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
        }
    }

    pub fn response_content(self, feedback: Option<&str>) -> Value {
        match (self, feedback) {
            (Self::Deny, Some(feedback)) => json!({ "decision": self.as_str(), "feedback": feedback }),
            _ => json!({ "decision": self.as_str() }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanReviewElicitationMeta {
    pub ui: String,
    pub plan_path: String,
    pub title: String,
    pub markdown: String,
}

impl PlanReviewElicitationMeta {
    pub fn new(plan_path: &Path, markdown: &str) -> Self {
        Self {
            ui: PLAN_REVIEW_UI_KIND.to_string(),
            plan_path: plan_path.display().to_string(),
            title: format!("Review {}", plan_path.display()),
            markdown: markdown.to_string(),
        }
    }

    pub fn to_json(&self) -> Result<Map<String, Value>, serde_json::Error> {
        serde_json::to_value(self).and_then(|value| match value {
            Value::Object(map) => Ok(map),
            _ => {
                Err(serde_json::Error::io(std::io::Error::other("plan review metadata did not serialize to an object")))
            }
        })
    }

    pub fn parse(meta: Option<&Map<String, Value>>) -> Option<Self> {
        let value = Value::Object(meta?.clone());
        let parsed = serde_json::from_value::<Self>(value).ok()?;
        (parsed.ui == PLAN_REVIEW_UI_KIND).then_some(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn new_sets_expected_defaults() {
        let path = PathBuf::from("/tmp/example-plan.md");
        let meta = PlanReviewElicitationMeta::new(&path, "# Plan");

        assert_eq!(meta.ui, PLAN_REVIEW_UI_KIND);
        assert_eq!(meta.plan_path, "/tmp/example-plan.md");
        assert_eq!(meta.title, "Review /tmp/example-plan.md");
        assert_eq!(meta.markdown, "# Plan");
    }

    #[test]
    fn serialize_and_parse_round_trip() {
        let path = PathBuf::from("/tmp/example-plan.md");
        let meta = PlanReviewElicitationMeta::new(&path, "# Plan");

        let json = meta.to_json().expect("serialize metadata");
        let parsed = PlanReviewElicitationMeta::parse(Some(&json)).expect("parse metadata");

        assert_eq!(parsed, meta);
    }

    #[test]
    fn parse_rejects_non_plan_review_ui() {
        let mut json = Map::new();
        json.insert("ui".to_string(), Value::String("form".to_string()));
        json.insert("planPath".to_string(), Value::String("/tmp/plan.md".to_string()));
        json.insert("title".to_string(), Value::String("Review /tmp/plan.md".to_string()));
        json.insert("markdown".to_string(), Value::String("# Plan".to_string()));

        assert!(PlanReviewElicitationMeta::parse(Some(&json)).is_none());
    }

    #[test]
    fn parse_returns_none_for_malformed_payload() {
        let mut json = Map::new();
        json.insert("ui".to_string(), Value::String(PLAN_REVIEW_UI_KIND.to_string()));
        json.insert("planPath".to_string(), Value::Bool(true));

        assert!(PlanReviewElicitationMeta::parse(Some(&json)).is_none());
    }

    #[test]
    fn parse_returns_none_when_missing() {
        assert!(PlanReviewElicitationMeta::parse(None).is_none());
    }
}
