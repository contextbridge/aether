use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadSkillsInput {
    pub requests: Vec<SkillRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillFile {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadSkillsOutput {
    pub files: Vec<SkillFile>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_request_serialization() {
        let req = SkillRequest { name: "rust".to_string(), path: Some("traits.md".to_string()) };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"name\":\"rust\""));
        assert!(json.contains("\"path\":\"traits.md\""));

        let req_no_path = SkillRequest { name: "rust".to_string(), path: None };
        let json = serde_json::to_string(&req_no_path).unwrap();
        assert!(json.contains("\"name\":\"rust\""));
        assert!(!json.contains("\"path\""));
    }

    #[test]
    fn test_skill_file_serialization() {
        let file = SkillFile {
            name: "rust".to_string(),
            path: "SKILL.md".to_string(),
            content: Some("content".to_string()),
            error: None,
            available_files: vec!["traits.md".to_string()],
        };
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("\"name\":\"rust\""));
        assert!(json.contains("\"path\":\"SKILL.md\""));
        assert!(json.contains("\"availableFiles\":[\"traits.md\"]"));
    }
}
