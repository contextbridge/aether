use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Human-readable display metadata for a tool operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct ToolDisplayMeta {
    pub title: String,
    pub value: String,
}

impl ToolDisplayMeta {
    pub fn new(title: impl Into<String>, value: impl Into<String>) -> Self {
        Self { title: title.into(), value: value.into() }
    }
}

/// Full file contents for a diff
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct FileDiff {
    pub path: String,
    /// Original file content (`None` for new files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
}

/// A snapshot of the agent's current task plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct PlanMeta {
    pub entries: Vec<PlanMetaEntry>,
}

/// A single entry in a plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct PlanMetaEntry {
    pub content: String,
    pub status: PlanMetaStatus,
}

/// Execution status of a plan entry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanMetaStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

/// Typed wrapper for the MCP `_meta` field on tool results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct ToolResultMeta {
    pub display: ToolDisplayMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_diff: Option<FileDiff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanMeta>,
}

impl From<ToolDisplayMeta> for ToolResultMeta {
    fn from(display: ToolDisplayMeta) -> Self {
        Self::new(display)
    }
}

impl ToolResultMeta {
    pub fn new(display: ToolDisplayMeta) -> Self {
        Self { display, file_diff: None, plan: None }
    }

    pub fn with_plan(display: ToolDisplayMeta, plan: PlanMeta) -> Self {
        Self { display, file_diff: None, plan: Some(plan) }
    }

    pub fn with_file_diff(display: ToolDisplayMeta, file_diff: FileDiff) -> Self {
        Self { display, file_diff: Some(file_diff), plan: None }
    }
}

pub fn truncate(s: &str, max_length: usize) -> String {
    if s.chars().count() <= max_length {
        s.to_string()
    } else {
        let mut truncated = s.chars().take(max_length.saturating_sub(3)).collect::<String>();
        truncated.push_str("...");
        truncated
    }
}

pub fn basename(path: &str) -> String {
    let platform_basename = std::path::Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or(path);

    if platform_basename.contains('\\') {
        path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
    } else {
        platform_basename.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(title: &str, value: &str) -> ToolDisplayMeta {
        ToolDisplayMeta::new(title, value)
    }

    fn assert_serde_roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(val: &T) {
        let json = serde_json::to_string(val).unwrap();
        let parsed: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&parsed, val);
    }

    fn sample_diff(old_text: Option<&str>) -> FileDiff {
        FileDiff {
            path: "/tmp/main.rs".to_string(),
            old_text: old_text.map(str::to_string),
            new_text: Some("new content".to_string()),
        }
    }

    fn sample_plan() -> PlanMeta {
        PlanMeta {
            entries: vec![
                PlanMetaEntry { content: "Research AI agents".into(), status: PlanMetaStatus::Completed },
                PlanMetaEntry { content: "Implement tracking".into(), status: PlanMetaStatus::InProgress },
                PlanMetaEntry { content: "Write tests".into(), status: PlanMetaStatus::Pending },
            ],
        }
    }

    #[test]
    fn test_new_sets_title_and_value() {
        let meta = display("Read file", "Cargo.toml, 156 lines");
        assert_eq!(meta.title, "Read file");
        assert_eq!(meta.value, "Cargo.toml, 156 lines");
    }

    #[test]
    fn test_serde_json_shape() {
        let json = serde_json::to_value(display("Read file", "Cargo.toml")).unwrap();
        assert_eq!(json["title"], "Read file");
        assert_eq!(json["value"], "Cargo.toml");
    }

    #[test]
    fn test_serde_roundtrips() {
        assert_serde_roundtrip(&display("Grep", "'TODO' in src (42 matches)"));
        assert_serde_roundtrip(&sample_diff(Some("old content")));
        assert_serde_roundtrip(&sample_plan());

        let result_meta: ToolResultMeta = display("Read file", "Cargo.toml, 156 lines").into();
        assert_serde_roundtrip(&result_meta);
    }

    #[test]
    fn test_into_result_meta() {
        let d = display("Write file", "main.rs");
        let meta: ToolResultMeta = d.clone().into();
        assert_eq!(meta, ToolResultMeta { display: d, file_diff: None, plan: None });
    }

    #[test]
    fn test_optional_fields_omitted_when_none() {
        let diff_json = serde_json::to_value(sample_diff(None)).unwrap();
        assert!(diff_json.get("old_text").is_none());

        let meta_json = serde_json::to_value::<ToolResultMeta>(display("Read", "f.rs").into()).unwrap();
        assert!(meta_json.get("plan").is_none());
        assert!(meta_json.get("file_diff").is_none());
    }

    #[test]
    fn empty_and_deleted_file_snapshots_remain_distinct() {
        let empty: FileDiff = serde_json::from_value(serde_json::json!({
            "path": "/tmp/file", "old_text": "old", "new_text": ""
        }))
        .unwrap();
        let deleted: FileDiff = serde_json::from_value(serde_json::json!({
            "path": "/tmp/file", "old_text": "old"
        }))
        .unwrap();
        assert_eq!(empty.new_text.as_deref(), Some(""));
        assert_eq!(deleted.new_text, None);
        assert_serde_roundtrip(&empty);
        assert_serde_roundtrip(&deleted);
        assert_serde_roundtrip(&PlanMetaStatus::Cancelled);
    }

    #[test]
    fn test_file_diff_missing_old_text_defaults_to_none() {
        let parsed: FileDiff = serde_json::from_str(r#"{"path":"/tmp/f.rs","new_text":"content"}"#).unwrap();
        assert_eq!(parsed.old_text, None);
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");

        let long = truncate("cargo check --message-format=json --locked", 20);
        assert!(long.chars().count() <= 20);
        assert!(long.ends_with("..."));

        let multibyte = truncate("こんにちは世界テスト文字列", 8);
        assert_eq!(multibyte.chars().count(), 8);
        assert!(multibyte.ends_with("..."));
    }

    #[test]
    fn test_basename() {
        for (path, expected) in [
            ("/Users/josh/code/aether/Cargo.toml", "Cargo.toml"),
            (r"C:\Users\josh\code\aether\Cargo.toml", "Cargo.toml"),
            ("Cargo.toml", "Cargo.toml"),
        ] {
            assert_eq!(basename(path), expected, "path: {path}");
        }
    }

    #[test]
    fn test_plan_meta_status_serde_snake_case() {
        let json = serde_json::to_value(PlanMetaStatus::InProgress).unwrap();
        assert_eq!(json, serde_json::Value::String("in_progress".to_string()));
    }
}
