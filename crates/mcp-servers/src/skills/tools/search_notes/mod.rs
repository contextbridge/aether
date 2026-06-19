use super::save_note::{NoteError, NoteFrontmatter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::debug;
use utils::markdown_file::split_frontmatter;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchNotesInput {
    /// Search term matched against topic names (substring) and tags (exact match)
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NoteResult {
    pub topic: String,
    pub tags: Vec<String>,
    pub updated: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchNotesOutput {
    pub results: Vec<NoteResult>,
}

pub fn search_notes(input: &SearchNotesInput, notes_dir: &Path) -> Result<SearchNotesOutput, NoteError> {
    if !notes_dir.is_dir() {
        return Ok(SearchNotesOutput { results: Vec::new() });
    }

    let query = input.query.trim().to_lowercase();
    let mut results = Vec::new();

    let entries = std::fs::read_dir(notes_dir).map_err(NoteError::Io)?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                debug!("skipping unreadable note {}: {e}", path.display());
                continue;
            }
        };

        let Some((yaml, body)) = split_frontmatter(&content) else {
            continue;
        };

        let fm: NoteFrontmatter = match serde_yml::from_str(yaml) {
            Ok(fm) => fm,
            Err(e) => {
                debug!("skipping note with malformed YAML {}: {e}", path.display());
                continue;
            }
        };

        let topic_lower = fm.topic.to_lowercase();
        let topic_matches = topic_lower.contains(&query);
        let tag_matches = fm.tags.iter().any(|t| t.to_lowercase() == query);

        if topic_matches || tag_matches {
            results.push(NoteResult { topic: fm.topic, tags: fm.tags, updated: fm.updated, content: body.to_string() });
        }
    }

    results.sort_by(|a, b| a.topic.cmp(&b.topic));

    Ok(SearchNotesOutput { results })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_note(notes_dir: &Path, filename: &str, content: &str) {
        std::fs::create_dir_all(notes_dir).unwrap();
        std::fs::write(notes_dir.join(filename), content).unwrap();
    }

    #[test]
    fn test_search_by_topic_substring() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        write_note(
            &notes_dir,
            "agent-spec.md",
            "---\ntopic: agent-spec\ntags:\n- aether\nupdated: \"2026-02-15\"\n---\nSome content.",
        );
        write_note(
            &notes_dir,
            "testing.md",
            "---\ntopic: testing\ntags:\n- rust\nupdated: \"2026-02-14\"\n---\nTest content.",
        );

        let input = SearchNotesInput { query: "agent".to_string() };
        let output = search_notes(&input, &notes_dir).unwrap();
        assert_eq!(output.results.len(), 1);
        assert_eq!(output.results[0].topic, "agent-spec");
        assert_eq!(output.results[0].content, "Some content.");
    }

    #[test]
    fn test_search_by_tag() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        write_note(
            &notes_dir,
            "agent-spec.md",
            "---\ntopic: agent-spec\ntags:\n- aether\n- architecture\nupdated: \"2026-02-15\"\n---\nContent A.",
        );
        write_note(
            &notes_dir,
            "mcp-setup.md",
            "---\ntopic: mcp-setup\ntags:\n- aether\nupdated: \"2026-02-14\"\n---\nContent B.",
        );

        let input = SearchNotesInput { query: "aether".to_string() };
        let output = search_notes(&input, &notes_dir).unwrap();
        assert_eq!(output.results.len(), 2);
    }

    #[test]
    fn test_search_case_insensitive() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        write_note(
            &notes_dir,
            "agent-spec.md",
            "---\ntopic: Agent-Spec\ntags:\n- Aether\nupdated: \"2026-02-15\"\n---\nContent.",
        );

        let input = SearchNotesInput { query: "AGENT".to_string() };
        let output = search_notes(&input, &notes_dir).unwrap();
        assert_eq!(output.results.len(), 1);

        let input = SearchNotesInput { query: "aether".to_string() };
        let output = search_notes(&input, &notes_dir).unwrap();
        assert_eq!(output.results.len(), 1);
    }

    #[test]
    fn test_search_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");
        // Don't create the directory

        let input = SearchNotesInput { query: "anything".to_string() };
        let output = search_notes(&input, &notes_dir).unwrap();
        assert!(output.results.is_empty());
    }

    #[test]
    fn test_search_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        write_note(
            &notes_dir,
            "agent-spec.md",
            "---\ntopic: agent-spec\ntags:\n- aether\nupdated: \"2026-02-15\"\n---\nContent.",
        );

        let input = SearchNotesInput { query: "nonexistent".to_string() };
        let output = search_notes(&input, &notes_dir).unwrap();
        assert!(output.results.is_empty());
    }

    #[test]
    fn test_search_skips_non_md_files() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        write_note(&notes_dir, "readme.txt", "not a note");
        write_note(
            &notes_dir,
            "agent-spec.md",
            "---\ntopic: agent-spec\ntags: []\nupdated: \"2026-02-15\"\n---\nContent.",
        );

        let input = SearchNotesInput { query: "agent".to_string() };
        let output = search_notes(&input, &notes_dir).unwrap();
        assert_eq!(output.results.len(), 1);
    }

    #[test]
    fn test_search_skips_malformed_files() {
        let temp_dir = TempDir::new().unwrap();
        let notes_dir = temp_dir.path().join("notes");

        write_note(&notes_dir, "bad.md", "no frontmatter here");
        write_note(
            &notes_dir,
            "good.md",
            "---\ntopic: good-note\ntags: []\nupdated: \"2026-02-15\"\n---\nGood content.",
        );

        let input = SearchNotesInput { query: "good".to_string() };
        let output = search_notes(&input, &notes_dir).unwrap();
        assert_eq!(output.results.len(), 1);
        assert_eq!(output.results[0].topic, "good-note");
    }
}
