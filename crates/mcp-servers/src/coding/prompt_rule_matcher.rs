use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use aether_project::{PromptCatalog, PromptFile};

#[doc = include_str!("../docs/prompt_rule_matcher.md")]
#[derive(Debug)]
pub struct PromptRuleMatcher {
    catalog: PromptCatalog,
    activated: Mutex<HashSet<String>>,
}

impl PromptRuleMatcher {
    pub fn new(catalog: PromptCatalog) -> Self {
        Self { catalog, activated: Mutex::new(HashSet::new()) }
    }

    /// Returns newly-matched rules for `file_path` and marks them as activated.
    /// Subsequent calls for the same rules return an empty `Vec`.
    pub fn get_matched_rules(&self, root_dir: &Path, file_path: &str) -> Vec<PromptFile> {
        let relative = make_relative(root_dir, file_path);
        let relative_path = relative.as_deref().unwrap_or(file_path);
        let matches = self.catalog.matching_rules(relative_path);

        if matches.is_empty() {
            return Vec::new();
        }

        let mut result = Vec::new();
        let mut activated = self.activated.lock().expect("lock poisoned");
        for spec in matches {
            if activated.insert(spec.name.clone()) {
                tracing::info!("Activating read rule '{}' triggered by read of '{}'", spec.name, file_path);
                result.push(spec.clone());
            }
        }

        result
    }

    /// Clear all activated rules (e.g. on context clear).
    pub fn clear(&self) {
        self.activated.lock().expect("lock poisoned").clear();
    }
}

/// Make an absolute file path relative to the root directory.
fn make_relative(root_dir: &Path, file_path: &str) -> Option<String> {
    let path = Path::new(file_path);
    path.strip_prefix(root_dir).ok().map(|rel| rel.to_string_lossy().to_string())
}

impl Default for PromptRuleMatcher {
    fn default() -> Self {
        Self::new(PromptCatalog::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_project::PromptCatalog;

    #[test]
    fn returns_matched_rules_and_deduplicates() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let skills_dir = temp.path();

        let rust_dir = skills_dir.join("rust-rules");
        fs::create_dir_all(&rust_dir).unwrap();
        fs::write(
            rust_dir.join("SKILL.md"),
            "---\ndescription: Rust conventions\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nRust best practices.\n",
        )
        .unwrap();

        let catalog = PromptCatalog::from_dir(skills_dir).unwrap();
        let state = PromptRuleMatcher::new(catalog);
        let root_dir = Path::new("/project");

        let matched = state.get_matched_rules(root_dir, "/project/src/main.rs");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].body, "Rust best practices.");

        let matched2 = state.get_matched_rules(root_dir, "/project/src/lib.rs");
        assert!(matched2.is_empty());
    }

    #[test]
    fn returns_empty_for_non_matching_files() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let skills_dir = temp.path();

        let rust_dir = skills_dir.join("rust-rules");
        fs::create_dir_all(&rust_dir).unwrap();
        fs::write(
            rust_dir.join("SKILL.md"),
            "---\ndescription: Rust conventions\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nRust rules.\n",
        )
        .unwrap();

        let catalog = PromptCatalog::from_dir(skills_dir).unwrap();
        let state = PromptRuleMatcher::new(catalog);
        let root_dir = Path::new("/project");

        let matched = state.get_matched_rules(root_dir, "/project/README.md");
        assert!(matched.is_empty());
    }

    #[test]
    fn clear_activated_allows_rematching() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let skills_dir = temp.path();

        let rust_dir = skills_dir.join("rust-rules");
        fs::create_dir_all(&rust_dir).unwrap();
        fs::write(
            rust_dir.join("SKILL.md"),
            "---\ndescription: Rust conventions\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nRust rules.\n",
        )
        .unwrap();

        let catalog = PromptCatalog::from_dir(skills_dir).unwrap();
        let state = PromptRuleMatcher::new(catalog);
        let root_dir = Path::new("/project");

        let matched = state.get_matched_rules(root_dir, "/project/src/main.rs");
        assert_eq!(matched.len(), 1);

        state.clear();

        let matched2 = state.get_matched_rules(root_dir, "/project/src/main.rs");
        assert_eq!(matched2.len(), 1);
    }

    #[test]
    fn from_catalog_builds_rules() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let skills_dir = temp.path();

        let rust_dir = skills_dir.join("rust-rules");
        fs::create_dir_all(&rust_dir).unwrap();
        fs::write(
            rust_dir.join("SKILL.md"),
            "---\ndescription: Rust conventions\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nFollow Rust best practices.\n",
        )
        .unwrap();

        let commit_dir = skills_dir.join("commit");
        fs::create_dir_all(&commit_dir).unwrap();
        fs::write(
            commit_dir.join("SKILL.md"),
            "---\ndescription: Commit\nuser-invocable: true\n---\nCommit message.\n",
        )
        .unwrap();

        let catalog = PromptCatalog::from_dir(skills_dir).unwrap();
        let state = PromptRuleMatcher::new(catalog);
        let root_dir = Path::new("/project");
        let matched = state.get_matched_rules(root_dir, "/project/src/main.rs");
        assert_eq!(matched.len(), 1);
        assert!(matched[0].body.contains("Follow Rust best practices"));
    }
}
