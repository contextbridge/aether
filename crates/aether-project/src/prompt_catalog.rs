//! Unified prompt catalog for discovering and validating `.aether/skills/*/SKILL.md` artifacts.
//!
//! A single `SKILL.md` artifact can serve as:
//! - A **slash command** (`user-invocable: true`)
//! - A **skill** (`agent-invocable: true`)
//! - A **rule** (`triggers.read` globs)
//! - Any combination of the above

use crate::error::SettingsError;
use crate::prompt_file::{PromptFile, SKILL_FILENAME};
use std::collections::{HashMap, HashSet};
use std::fs::{DirEntry, read_dir};
use std::path::{Path, PathBuf};

/// A catalog of prompt artifacts discovered from `.aether/skills/`.
#[derive(Debug, Clone)]
pub struct PromptCatalog {
    specs: Vec<PromptFile>,
}

impl PromptCatalog {
    /// Discover and validate all prompt artifacts under `skills_dir/*/SKILL.md`.
    ///
    /// `skills_dir` is typically `<project_root>/.aether/skills` or `<base_dir>/skills`.
    pub fn from_dir(skills_dir: &Path) -> Result<Self, SettingsError> {
        let mut prompts = Vec::new();

        for entry in read_dir(skills_dir).map_err(|e| SettingsError::IoError(e.to_string()))?.filter_map(Result::ok) {
            if let Some(p) = get_path(&entry) {
                match PromptFile::parse(&p) {
                    Ok(spec) => prompts.push(spec),
                    Err(err) => tracing::warn!("Skipping invalid skill at {}: {err}", p.display()),
                }
            }
        }

        validate_catalog(&prompts)?;

        Ok(Self { specs: prompts })
    }

    /// Discover and merge prompt artifacts from multiple skill directories.
    ///
    /// On name collision, the last directory wins. Directories that don't exist are skipped.
    pub fn from_dirs(skills_dirs: &[PathBuf]) -> Self {
        let mut seen: HashMap<String, PromptFile> = HashMap::new();

        for dir in skills_dirs {
            let Ok(entries) = read_dir(dir) else {
                tracing::warn!("Skills directory does not exist, skipping: {}", dir.display());
                continue;
            };

            for entry in entries.filter_map(Result::ok) {
                if let Some(p) = get_path(&entry) {
                    match PromptFile::parse(&p) {
                        Ok(spec) => {
                            seen.insert(spec.name.clone(), spec);
                        }
                        Err(err) => {
                            tracing::warn!("Skipping invalid skill at {}: {err}", p.display());
                        }
                    }
                }
            }
        }

        Self { specs: seen.into_values().collect() }
    }

    /// Create an empty catalog.
    pub fn empty() -> Self {
        Self { specs: Vec::new() }
    }

    /// All prompt specs in catalog order.
    pub fn all(&self) -> &[PromptFile] {
        &self.specs
    }

    /// Find a prompt by its resolved prompt name.
    pub fn find(&self, name: &str) -> Option<&PromptFile> {
        self.specs.iter().find(|spec| spec.name == name)
    }

    /// Iterate over user-invocable prompts (slash commands).
    pub fn slash_commands(&self) -> impl Iterator<Item = &PromptFile> {
        self.specs.iter().filter(|s| s.user_invocable)
    }

    /// Iterate over agent-invocable prompts (skills).
    pub fn skills(&self) -> impl Iterator<Item = &PromptFile> {
        self.specs.iter().filter(|s| s.agent_invocable)
    }

    /// Find all prompt specs whose read triggers match the given project-relative path.
    pub fn matching_rules(&self, relative_path: &str) -> Vec<&PromptFile> {
        self.specs.iter().filter(|s| s.triggers.matches_read(relative_path)).collect()
    }
}

fn get_path(entry: &DirEntry) -> Option<PathBuf> {
    let path = entry.path();
    if entry.file_name().to_string_lossy().starts_with('.') {
        return None;
    }
    if path.is_dir() && path.join(SKILL_FILENAME).is_file() {
        Some(path.join(SKILL_FILENAME))
    } else if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
        Some(path)
    } else {
        None
    }
}

fn validate_catalog(specs: &[PromptFile]) -> Result<(), SettingsError> {
    let mut seen_names = HashSet::new();
    for spec in specs {
        if !seen_names.insert(&spec.name) {
            return Err(SettingsError::DuplicatePromptName { name: spec.name.clone() });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_project() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_skill(dir: &Path, name: &str, content: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join(SKILL_FILENAME), content).unwrap();
    }

    #[test]
    fn discover_empty_project() {
        let dir = create_temp_project();
        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert!(catalog.all().is_empty());
    }

    #[test]
    fn discover_user_only_prompt() {
        let dir = create_temp_project();
        write_skill(
            dir.path(),
            "commit",
            "---\ndescription: Generate commit messages\nuser-invocable: true\nagent-invocable: false\n---\nGenerate a commit message.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert_eq!(spec.name, "commit");
        assert!(spec.user_invocable);
        assert!(!spec.agent_invocable);
        assert!(spec.triggers.is_empty());
    }

    #[test]
    fn discover_agent_only_prompt() {
        let dir = create_temp_project();
        write_skill(
            dir.path(),
            "explain-code",
            "---\ndescription: Explain code\nagent-invocable: true\n---\nExplain the code.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert!(spec.agent_invocable);
        assert!(spec.user_invocable);
    }

    #[test]
    fn discover_rule_only_prompt() {
        let dir = create_temp_project();
        write_skill(
            dir.path(),
            "rust-rules",
            "---\ndescription: Rust conventions\nagent-invocable: false\ntriggers:\n  read:\n    - \"crates/**/*.rs\"\n---\nFollow Rust conventions.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert!(spec.user_invocable);
        assert!(!spec.agent_invocable);
        assert!(!spec.triggers.is_empty());
        assert!(spec.triggers.matches_read("crates/foo/bar.rs"));
        assert!(!spec.triggers.matches_read("other/file.py"));
    }

    #[test]
    fn discover_dual_use_prompt() {
        let dir = create_temp_project();
        write_skill(
            dir.path(),
            "explain",
            "---\ndescription: Explain code\nuser-invocable: true\nagent-invocable: true\nargument-hint: \"[path]\"\n---\nExplain with diagrams.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        let spec = &catalog.all()[0];
        assert!(spec.user_invocable);
        assert!(spec.agent_invocable);
        assert_eq!(spec.argument_hint.as_deref(), Some("[path]"));

        let user: Vec<_> = catalog.slash_commands().collect();
        assert_eq!(user.len(), 1);
        let agent: Vec<_> = catalog.skills().collect();
        assert_eq!(agent.len(), 1);
    }

    #[test]
    fn reject_duplicate_names() {
        let dir = create_temp_project();
        write_skill(dir.path(), "foo", "---\ndescription: First\nuser-invocable: true\n---\nContent.");
        // Second skill with explicit name override to "foo"
        write_skill(dir.path(), "bar", "---\nname: foo\ndescription: Second\nuser-invocable: true\n---\nContent.");

        let result = PromptCatalog::from_dir(dir.path());
        assert!(matches!(result, Err(SettingsError::DuplicatePromptName { .. })));
    }

    #[test]
    fn empty_description_defaults_to_name() {
        let dir = create_temp_project();
        write_skill(dir.path(), "bad", "---\ndescription: \"\"\nuser-invocable: true\n---\nContent.");

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].description, "bad");
    }

    #[test]
    fn skill_without_activation_surface_defaults_to_user_invocable() {
        let dir = create_temp_project();
        write_skill(dir.path(), "noop", "---\ndescription: Does nothing\n---\nContent.");

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert!(catalog.all()[0].user_invocable);
    }

    #[test]
    fn flat_md_without_activation_surface_is_skipped() {
        let dir = create_temp_project();
        write_flat_rule(dir.path(), "noop.md", "---\ndescription: Does nothing\nagent-invocable: false\n---\nContent.");

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert!(catalog.all().is_empty());
    }

    #[test]
    fn name_defaults_to_directory_name() {
        let dir = create_temp_project();
        write_skill(dir.path(), "my-skill", "---\ndescription: My skill\nagent-invocable: true\n---\nContent.");

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all()[0].name, "my-skill");
    }

    #[test]
    fn name_from_frontmatter_overrides_directory() {
        let dir = create_temp_project();
        write_skill(
            dir.path(),
            "dir-name",
            "---\nname: custom-name\ndescription: Custom\nuser-invocable: true\n---\nContent.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all()[0].name, "custom-name");
    }

    #[test]
    fn matching_read_rules_finds_matches() {
        let dir = create_temp_project();
        write_skill(
            dir.path(),
            "rust-rules",
            "---\ndescription: Rust rules\ntriggers:\n  read:\n    - \"src/**/*.rs\"\n---\nRust rules.",
        );
        write_skill(
            dir.path(),
            "ts-rules",
            "---\ndescription: TS rules\ntriggers:\n  read:\n    - \"src/**/*.ts\"\n---\nTS rules.",
        );
        write_skill(dir.path(), "commit", "---\ndescription: Commit\nuser-invocable: true\n---\nCommit.");

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        let matches = catalog.matching_rules("src/main.rs");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "rust-rules");

        let matches = catalog.matching_rules("src/app.ts");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "ts-rules");

        let matches = catalog.matching_rules("README.md");
        assert!(matches.is_empty());
    }

    #[test]
    fn pure_flat_rule_not_in_user_or_agent_invocable() {
        let dir = create_temp_project();
        write_flat_rule(
            dir.path(),
            "rule.md",
            "---\ndescription: A rule\nagent-invocable: false\ntriggers:\n  read:\n    - \"*.rs\"\n---\nRule content.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.slash_commands().count(), 0);
        assert_eq!(catalog.skills().count(), 0);
    }

    #[test]
    fn skips_hidden_directories() {
        let dir = create_temp_project();
        write_skill(dir.path(), ".archived", "---\ndescription: Archived\nuser-invocable: true\n---\nOld.");
        write_skill(dir.path(), "visible", "---\ndescription: Visible\nuser-invocable: true\n---\nNew.");

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].name, "visible");
    }

    #[test]
    fn preserves_tags_and_metadata() {
        let dir = create_temp_project();
        write_skill(
            dir.path(),
            "tagged",
            "---\ndescription: Tagged skill\nagent-invocable: true\ntags:\n  - rust\n  - testing\nagent_authored: true\nhelpful: 5\nharmful: 1\n---\nContent.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        let spec = &catalog.all()[0];
        assert_eq!(spec.tags, vec!["rust", "testing"]);
        assert!(spec.agent_authored);
        assert_eq!(spec.helpful, 5);
        assert_eq!(spec.harmful, 1);
    }

    #[test]
    fn from_dirs_last_wins() {
        let dir_a = create_temp_project();
        let dir_b = create_temp_project();
        write_skill(dir_a.path(), "rust", "---\ndescription: Rust A\nagent-invocable: true\n---\nFrom dir A.");
        write_skill(dir_b.path(), "rust", "---\ndescription: Rust B\nagent-invocable: true\n---\nFrom dir B.");

        let catalog = PromptCatalog::from_dirs(&[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()]);
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert_eq!(spec.name, "rust");
        assert_eq!(spec.description, "Rust B");
        assert!(spec.body.contains("From dir B."));
    }

    #[test]
    fn from_dirs_union() {
        let dir_a = create_temp_project();
        let dir_b = create_temp_project();
        write_skill(dir_a.path(), "rust", "---\ndescription: Rust\nagent-invocable: true\n---\nRust content.");
        write_skill(dir_b.path(), "python", "---\ndescription: Python\nagent-invocable: true\n---\nPython content.");

        let catalog = PromptCatalog::from_dirs(&[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()]);
        assert_eq!(catalog.all().len(), 2);

        let names: Vec<&str> = catalog.all().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"rust"));
        assert!(names.contains(&"python"));
    }

    #[test]
    fn from_dirs_skips_missing() {
        let dir_a = create_temp_project();
        let missing = PathBuf::from("/tmp/nonexistent-skills-dir-12345");
        write_skill(dir_a.path(), "rust", "---\ndescription: Rust\nagent-invocable: true\n---\nRust content.");

        let catalog = PromptCatalog::from_dirs(&[missing, dir_a.path().to_path_buf()]);
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].name, "rust");
    }

    fn write_flat_rule(dir: &Path, filename: &str, content: &str) {
        fs::write(dir.join(filename), content).unwrap();
    }

    #[test]
    fn discover_flat_md_rule_with_globs() {
        let dir = create_temp_project();
        write_flat_rule(
            dir.path(),
            "rust-conventions.md",
            "---\ndescription: Rust conventions\nglobs:\n  - \"**/*.rs\"\n---\nFollow Rust conventions.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert_eq!(spec.name, "rust-conventions");
        assert_eq!(spec.description, "Rust conventions");
        assert!(spec.triggers.matches_read("src/main.rs"));
        assert!(!spec.triggers.matches_read("README.md"));
    }

    #[test]
    fn discover_flat_md_rule_with_paths() {
        let dir = create_temp_project();
        write_flat_rule(
            dir.path(),
            "ts-rules.md",
            "---\ndescription: TS rules\npaths:\n  - \"**/*.ts\"\n---\nTypeScript rules.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert_eq!(spec.name, "ts-rules");
        assert!(spec.triggers.matches_read("src/index.ts"));
    }

    #[test]
    fn discover_mixed_skill_md_and_flat_rules() {
        let dir = create_temp_project();
        write_skill(dir.path(), "commit", "---\ndescription: Commit\nuser-invocable: true\n---\nCommit message.");
        write_flat_rule(
            dir.path(),
            "rust-rules.md",
            "---\ndescription: Rust rules\nglobs:\n  - \"**/*.rs\"\n---\nRust conventions.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 2);

        let names: Vec<&str> = catalog.all().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"commit"));
        assert!(names.contains(&"rust-rules"));
    }

    #[test]
    fn from_dirs_merges_flat_rules() {
        let dir_a = create_temp_project();
        let dir_b = create_temp_project();
        write_skill(dir_a.path(), "commit", "---\ndescription: Commit\nuser-invocable: true\n---\nCommit.");
        write_flat_rule(
            dir_b.path(),
            "rust-rules.md",
            "---\ndescription: Rust rules\nglobs:\n  - \"**/*.rs\"\n---\nRust conventions.",
        );

        let catalog = PromptCatalog::from_dirs(&[dir_a.path().to_path_buf(), dir_b.path().to_path_buf()]);
        assert_eq!(catalog.all().len(), 2);

        let names: Vec<&str> = catalog.all().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"commit"));
        assert!(names.contains(&"rust-rules"));
    }

    #[test]
    fn flat_rule_without_description_uses_name() {
        let dir = create_temp_project();
        write_flat_rule(dir.path(), "my-rule.md", "---\nglobs:\n  - \"**/*.rs\"\n---\nRule body.");

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert_eq!(spec.name, "my-rule");
        assert_eq!(spec.description, "my-rule");
    }

    #[test]
    fn skips_hidden_flat_md_files() {
        let dir = create_temp_project();
        write_flat_rule(
            dir.path(),
            ".hidden-rule.md",
            "---\ndescription: Hidden\nglobs:\n  - \"**/*.rs\"\n---\nHidden.",
        );
        write_flat_rule(
            dir.path(),
            "visible-rule.md",
            "---\ndescription: Visible\nglobs:\n  - \"**/*.ts\"\n---\nVisible.",
        );

        let catalog = PromptCatalog::from_dir(dir.path()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].name, "visible-rule");
    }
}
