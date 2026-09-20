use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

pub const ARTIFACT_REVIEW_UI_KIND: &str = "artifactReview";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactReviewDecision {
    Approved,
    Feedback,
}

impl ArtifactReviewDecision {
    pub const ALL: [Self; 2] = [Self::Approved, Self::Feedback];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Feedback => "feedback",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "lowercase", try_from = "ReviewForm")]
pub enum ArtifactReviewSubmission {
    Approved,
    Feedback { feedback: String },
}

impl ArtifactReviewSubmission {
    pub fn fields(&self) -> impl Iterator<Item = (&'static str, &str)> {
        let (decision, feedback) = match self {
            Self::Approved => (ArtifactReviewDecision::Approved, None),
            Self::Feedback { feedback } => (ArtifactReviewDecision::Feedback, Some(feedback.as_str())),
        };
        std::iter::once(("decision", decision.as_str())).chain(feedback.map(|feedback| ("feedback", feedback)))
    }
}

impl std::fmt::Display for ArtifactReviewDecision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactReviewElicitationMeta {
    pub ui: String,
    pub path: Option<PathBuf>,
    pub title: String,
    pub markdown: String,
}

impl ArtifactReviewElicitationMeta {
    pub fn new(path: Option<PathBuf>, title: impl Into<String>, markdown: impl Into<String>) -> Self {
        Self { ui: ARTIFACT_REVIEW_UI_KIND.to_string(), path, title: title.into(), markdown: markdown.into() }
    }

    pub fn to_json(&self) -> Result<Map<String, Value>, serde_json::Error> {
        serde_json::to_value(self).and_then(|value| match value {
            Value::Object(map) => Ok(map),
            _ => Err(serde_json::Error::io(std::io::Error::other(
                "artifact review metadata did not serialize to an object",
            ))),
        })
    }

    pub fn parse(meta: Option<&Map<String, Value>>) -> Option<Self> {
        let parsed = serde_json::from_value::<Self>(Value::Object(meta?.clone())).ok()?;
        (parsed.ui == ARTIFACT_REVIEW_UI_KIND).then_some(parsed)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewForm {
    decision: ArtifactReviewDecision,
    #[serde(default)]
    feedback: String,
}

impl TryFrom<ReviewForm> for ArtifactReviewSubmission {
    type Error = &'static str;

    fn try_from(form: ReviewForm) -> Result<Self, Self::Error> {
        match form.decision {
            ArtifactReviewDecision::Approved if form.feedback.is_empty() => Ok(Self::Approved),
            ArtifactReviewDecision::Approved => Err("approval cannot contain feedback"),
            ArtifactReviewDecision::Feedback => Ok(Self::Feedback { feedback: form.feedback }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn submission_fields_match_the_serialized_contract() {
        for submission in [
            ArtifactReviewSubmission::Approved,
            ArtifactReviewSubmission::Feedback { feedback: "line one\nline two\n".into() },
        ] {
            let fields = submission
                .fields()
                .map(|(name, value)| (name.to_string(), Value::String(value.to_string())))
                .collect::<Map<_, _>>();
            assert_eq!(serde_json::to_value(&submission).unwrap(), Value::Object(fields.clone()));
            assert_eq!(serde_json::from_value::<ArtifactReviewSubmission>(Value::Object(fields)).unwrap(), submission);
        }
    }

    #[test]
    fn metadata_round_trips() {
        let meta = ArtifactReviewElicitationMeta::new(Some(PathBuf::from("docs/question.md")), "Review", "# Question");
        let parsed = ArtifactReviewElicitationMeta::parse(Some(&meta.to_json().expect("serialize"))).expect("parse");
        assert_eq!(parsed, meta);
        assert_eq!(parsed.ui, "artifactReview");
        assert_eq!(parsed.path.as_deref(), Some(Path::new("docs/question.md")));
    }
}
