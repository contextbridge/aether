use aether_project::{PromptCatalog, PromptFile};
use std::collections::HashSet;
use std::path::Path;

#[doc = include_str!("../docs/prompt_rule_matcher.md")]
#[derive(Debug)]
pub struct PromptRuleMatcher {
    catalog: PromptCatalog,
    activated: HashSet<String>,
}

impl PromptRuleMatcher {
    pub fn new(catalog: PromptCatalog) -> Self {
        Self { catalog, activated: HashSet::new() }
    }

    /// Returns newly-matched rules for `file_path` and marks them as activated.
    /// Subsequent calls for the same rules return an empty `Vec`.
    pub fn get_matched_rules(&mut self, root_dir: &Path, file_path: &str) -> Vec<PromptFile> {
        let relative = make_relative(root_dir, file_path);
        let relative_path = relative.as_deref().unwrap_or(file_path);
        let matches = self.catalog.matching_rules(relative_path);

        let mut result = Vec::new();
        for spec in matches {
            if self.activated.insert(spec.name.clone()) {
                tracing::info!("Activating read rule '{}' triggered by read of '{}'", spec.name, file_path);
                result.push(spec.clone());
            }
        }

        result
    }

    /// Allow previously activated rules to fire again after a context reset.
    pub fn clear(&mut self) {
        self.activated.clear();
    }
}

impl Default for PromptRuleMatcher {
    fn default() -> Self {
        Self::new(PromptCatalog::empty())
    }
}

fn make_relative(root_dir: &Path, file_path: &str) -> Option<String> {
    let path = Path::new(file_path);
    path.strip_prefix(root_dir).ok().map(|rel| rel.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestWorkspace;
    use aether_project::PromptCatalog;

    const RUST_RULES_SKILL: &str = "---\ndescription: Rust conventions\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\n";

    fn workspace_with_rust_rules(body: &str) -> TestWorkspace {
        TestWorkspace::new().file("rust-rules/SKILL.md", format!("{RUST_RULES_SKILL}{body}\n"))
    }

    #[test]
    fn returns_matched_rules_and_deduplicates() {
        let workspace = workspace_with_rust_rules("Rust best practices.");
        let catalog = PromptCatalog::from_dir(workspace.root()).unwrap();
        let mut state = PromptRuleMatcher::new(catalog);
        let root_dir = Path::new("/project");

        let matched = state.get_matched_rules(root_dir, "/project/src/main.rs");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].body, "Rust best practices.");

        let matched2 = state.get_matched_rules(root_dir, "/project/src/lib.rs");
        assert!(matched2.is_empty());
    }

    #[test]
    fn returns_empty_for_non_matching_files() {
        let workspace = workspace_with_rust_rules("Rust rules.");
        let catalog = PromptCatalog::from_dir(workspace.root()).unwrap();
        let mut state = PromptRuleMatcher::new(catalog);
        let root_dir = Path::new("/project");

        let matched = state.get_matched_rules(root_dir, "/project/README.md");
        assert!(matched.is_empty());
    }

    #[test]
    fn clear_activated_allows_rematching() {
        let workspace = workspace_with_rust_rules("Rust rules.");
        let catalog = PromptCatalog::from_dir(workspace.root()).unwrap();
        let mut state = PromptRuleMatcher::new(catalog);
        let root_dir = Path::new("/project");

        let matched = state.get_matched_rules(root_dir, "/project/src/main.rs");
        assert_eq!(matched.len(), 1);

        state.clear();

        let matched2 = state.get_matched_rules(root_dir, "/project/src/main.rs");
        assert_eq!(matched2.len(), 1);
    }

    #[test]
    fn from_catalog_builds_rules() {
        let workspace = workspace_with_rust_rules("Follow Rust best practices.")
            .file("commit/SKILL.md", "---\ndescription: Commit\nuser-invocable: true\n---\nCommit message.\n");

        let catalog = PromptCatalog::from_dir(workspace.root()).unwrap();
        let mut state = PromptRuleMatcher::new(catalog);
        let root_dir = Path::new("/project");
        let matched = state.get_matched_rules(root_dir, "/project/src/main.rs");
        assert_eq!(matched.len(), 1);
        assert!(matched[0].body.contains("Follow Rust best practices"));
    }
}
