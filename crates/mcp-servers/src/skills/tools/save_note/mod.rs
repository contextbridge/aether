use std::fmt::{self, Write};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utils::markdown_file::split_frontmatter;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveNoteInput {
    /// Topic name (normalized to kebab-case for the filename)
    pub topic: String,
    /// The learning to append
    pub content: String,
    /// Tags for search/categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SaveNoteStatus {
    Created,
    Appended,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveNoteOutput {
    pub topic: String,
    pub status: SaveNoteStatus,
    pub content: String,
}

#[derive(Debug)]
pub enum NoteError {
    Io(std::io::Error),
    InvalidContent(String),
}

impl fmt::Display for NoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NoteError::Io(e) => write!(f, "IO error: {e}"),
            NoteError::InvalidContent(msg) => write!(f, "Invalid content: {msg}"),
        }
    }
}

impl From<std::io::Error> for NoteError {
    fn from(e: std::io::Error) -> Self {
        NoteError::Io(e)
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct NoteFrontmatter {
    pub topic: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub updated: String,
}

pub fn save_note(input: &SaveNoteInput, notes_dir: &Path, today: &str) -> Result<SaveNoteOutput, NoteError> {
    let content = input.content.trim();
    if content.is_empty() {
        return Err(NoteError::InvalidContent("content must not be empty".to_string()));
    }

    std::fs::create_dir_all(notes_dir)?;

    let filename = normalize_topic(&input.topic);
    if filename.is_empty() {
        return Err(NoteError::InvalidContent("topic must contain at least one alphanumeric character".to_string()));
    }

    let note_path = notes_dir.join(format!("{filename}.md"));
    let (status, final_content) = if note_path.is_file() {
        let existing = std::fs::read_to_string(&note_path)?;
        let (frontmatter, body) = parse_note(&existing)?;
        let merged_tags = merge_tags(&frontmatter.tags, &input.tags);
        let new_body = format!("{body}\n\n{content}");

        let fm = NoteFrontmatter { topic: frontmatter.topic, tags: merged_tags, updated: today.to_string() };

        let file_content = render_note(&fm, &new_body);
        std::fs::write(&note_path, &file_content)?;

        (SaveNoteStatus::Appended, new_body)
    } else {
        let fm = NoteFrontmatter {
            topic: input.topic.trim().to_string(),
            tags: input.tags.clone(),
            updated: today.to_string(),
        };

        let file_content = render_note(&fm, content);
        std::fs::write(&note_path, &file_content)?;

        (SaveNoteStatus::Created, content.to_string())
    };

    Ok(SaveNoteOutput { topic: filename, status, content: final_content })
}

fn normalize_topic(topic: &str) -> String {
    topic
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn parse_note(content: &str) -> Result<(NoteFrontmatter, String), NoteError> {
    let (yaml, body) = split_frontmatter(content)
        .ok_or_else(|| NoteError::InvalidContent("note file missing frontmatter".to_string()))?;

    let fm: NoteFrontmatter =
        serde_yml::from_str(yaml).map_err(|e| NoteError::InvalidContent(format!("invalid YAML frontmatter: {e}")))?;

    Ok((fm, body.to_string()))
}

fn merge_tags(existing: &[String], new: &[String]) -> Vec<String> {
    let mut tags = existing.to_vec();
    for tag in new {
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            tags.push(tag.clone());
        }
    }
    tags
}

fn render_note(fm: &NoteFrontmatter, body: &str) -> String {
    let mut out = String::from("---\n");

    let _ = writeln!(out, "topic: {}", fm.topic);

    if !fm.tags.is_empty() {
        out.push_str("tags:\n");
        for tag in &fm.tags {
            let _ = writeln!(out, "- {tag}");
        }
    }

    let _ = writeln!(out, "updated: \"{}\"", fm.updated);
    out.push_str("---\n");
    out.push_str(body);
    out.push('\n');

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_normalize_topic() {
        assert_eq!(normalize_topic("Agent Spec"), "agent-spec");
        assert_eq!(normalize_topic("my_topic"), "my-topic");
        assert_eq!(normalize_topic("Hello World!"), "hello-world");
        assert_eq!(normalize_topic("  spaces  "), "spaces");
        assert_eq!(normalize_topic("a--b"), "a-b");
        assert_eq!(normalize_topic("UPPER"), "upper");
    }

    #[test]
    fn test_normalize_topic_empty() {
        assert_eq!(normalize_topic("!!!"), "");
    }

    #[test]
    fn test_create_new_note() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        let input = SaveNoteInput {
            topic: "agent-spec".to_string(),
            content: "Core owns AgentSpec type.".to_string(),
            tags: vec!["aether".to_string(), "architecture".to_string()],
        };

        let output = save_note(&input, &notes_dir, "2026-02-15").unwrap();
        assert_eq!(output.topic, "agent-spec");
        assert_eq!(output.status, SaveNoteStatus::Created);
        assert_eq!(output.content, "Core owns AgentSpec type.");

        let file = std::fs::read_to_string(notes_dir.join("agent-spec.md")).unwrap();
        assert!(file.contains("topic: agent-spec"));
        assert!(file.contains("- aether"));
        assert!(file.contains("- architecture"));
        assert!(file.contains("updated: \"2026-02-15\""));
        assert!(file.contains("Core owns AgentSpec type."));
    }

    #[test]
    fn test_append_to_existing_note() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        let input1 = SaveNoteInput {
            topic: "agent-spec".to_string(),
            content: "First learning.".to_string(),
            tags: vec!["aether".to_string()],
        };
        save_note(&input1, &notes_dir, "2026-02-14").unwrap();

        let input2 = SaveNoteInput {
            topic: "agent-spec".to_string(),
            content: "Second learning.".to_string(),
            tags: vec!["architecture".to_string()],
        };
        let output = save_note(&input2, &notes_dir, "2026-02-15").unwrap();

        assert_eq!(output.status, SaveNoteStatus::Appended);
        assert!(output.content.contains("First learning."));
        assert!(output.content.contains("Second learning."));

        let file = std::fs::read_to_string(notes_dir.join("agent-spec.md")).unwrap();
        assert!(file.contains("- aether"));
        assert!(file.contains("- architecture"));
        assert!(file.contains("updated: \"2026-02-15\""));
    }

    #[test]
    fn test_merge_tags_deduplicates() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        let input1 = SaveNoteInput {
            topic: "test".to_string(),
            content: "First.".to_string(),
            tags: vec!["rust".to_string(), "testing".to_string()],
        };
        save_note(&input1, &notes_dir, "2026-01-01").unwrap();

        let input2 = SaveNoteInput {
            topic: "test".to_string(),
            content: "Second.".to_string(),
            tags: vec!["rust".to_string(), "new-tag".to_string()],
        };
        save_note(&input2, &notes_dir, "2026-01-02").unwrap();

        let file = std::fs::read_to_string(notes_dir.join("test.md")).unwrap();
        assert_eq!(file.matches("- rust").count(), 1);
        assert!(file.contains("- testing"));
        assert!(file.contains("- new-tag"));
    }

    #[test]
    fn test_reject_empty_content() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        let input = SaveNoteInput { topic: "test".to_string(), content: "   ".to_string(), tags: vec![] };
        let result = save_note(&input, &notes_dir, "2026-01-01");
        assert!(result.is_err());
    }

    #[test]
    fn test_reject_empty_topic() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        let input = SaveNoteInput { topic: "!!!".to_string(), content: "Some content.".to_string(), tags: vec![] };
        let result = save_note(&input, &notes_dir, "2026-01-01");
        assert!(result.is_err());
    }

    #[test]
    fn test_topic_normalization_in_filename() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        let input = SaveNoteInput { topic: "Agent Spec".to_string(), content: "Learning.".to_string(), tags: vec![] };
        let output = save_note(&input, &notes_dir, "2026-01-01").unwrap();
        assert_eq!(output.topic, "agent-spec");
        assert!(notes_dir.join("agent-spec.md").is_file());
    }

    #[test]
    fn test_creates_notes_dir() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("deeply").join("nested").join("notes");

        let input = SaveNoteInput { topic: "test".to_string(), content: "Works.".to_string(), tags: vec![] };
        save_note(&input, &notes_dir, "2026-01-01").unwrap();
        assert!(notes_dir.join("test.md").is_file());
    }

    #[test]
    fn test_render_note_no_tags() {
        let fm = NoteFrontmatter { topic: "test".to_string(), tags: vec![], updated: "2026-01-01".to_string() };
        let rendered = render_note(&fm, "Body content.");
        assert!(!rendered.contains("tags:"));
        assert!(rendered.contains("topic: test"));
        assert!(rendered.contains("updated: \"2026-01-01\""));
        assert!(rendered.contains("Body content."));
    }
}
