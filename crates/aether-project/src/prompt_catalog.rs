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
use std::fs::{DirEntry, ReadDir, read_dir};
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
        let entries = read_dir(skills_dir).map_err(|e| SettingsError::IoError(e.to_string()))?;
        let prompts: Vec<PromptFile> = parse_skills(entries).collect();
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
            seen.extend(parse_skills(entries).map(|spec| (spec.name.clone(), spec)));
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

fn parse_skills(entries: ReadDir) -> impl Iterator<Item = PromptFile> {
    entries.filter_map(Result::ok).filter_map(|entry| get_path(&entry)).filter_map(|path| {
        PromptFile::parse(&path)
            .inspect_err(|err| tracing::warn!("Skipping invalid skill at {}: {err}", path.display()))
            .ok()
    })
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
    use crate::testing::project;

    #[test]
    fn discover_empty_project() {
        let dir = project();
        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert!(catalog.all().is_empty());
    }

    #[test]
    fn discover_user_only_prompt() {
        let dir = project().file(
            "commit/SKILL.md",
            "---\ndescription: Generate commit messages\nuser-invocable: true\nagent-invocable: false\n---\nGenerate a commit message.",
        );

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert_eq!(spec.name, "commit");
        assert!(spec.user_invocable);
        assert!(!spec.agent_invocable);
        assert!(spec.triggers.is_empty());
    }

    #[test]
    fn discover_agent_only_prompt() {
        let dir = project().file(
            "explain-code/SKILL.md",
            "---\ndescription: Explain code\nagent-invocable: true\n---\nExplain the code.",
        );

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert!(spec.agent_invocable);
        assert!(spec.user_invocable);
    }

    #[test]
    fn discover_rule_only_prompt() {
        let dir = project().file(
            "rust-rules/SKILL.md",
            "---\ndescription: Rust conventions\nagent-invocable: false\ntriggers:\n  read:\n    - \"crates/**/*.rs\"\n---\nFollow Rust conventions.",
        );

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
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
        let dir = project().file(
            "explain/SKILL.md",
            "---\ndescription: Explain code\nuser-invocable: true\nagent-invocable: true\nargument-hint: \"[path]\"\n---\nExplain with diagrams.",
        );

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
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
        let dir = project()
            // Second skill has an explicit name override so it collides with "foo"
            .file("foo/SKILL.md", "---\ndescription: First\nuser-invocable: true\n---\nContent.")
            .file("bar/SKILL.md", "---\nname: foo\ndescription: Second\nuser-invocable: true\n---\nContent.");

        let result = PromptCatalog::from_dir(dir.root());
        assert!(matches!(result, Err(SettingsError::DuplicatePromptName { .. })));
    }

    #[test]
    fn empty_description_defaults_to_name() {
        let dir = project().file("bad/SKILL.md", "---\ndescription: \"\"\nuser-invocable: true\n---\nContent.");

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].description, "bad");
    }

    #[test]
    fn flat_md_without_activation_surface_is_skipped() {
        let dir = project().file("noop.md", "---\ndescription: Does nothing\nagent-invocable: false\n---\nContent.");

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert!(catalog.all().is_empty());
    }

    #[test]
    fn name_from_frontmatter_overrides_directory() {
        let dir = project().file(
            "dir-name/SKILL.md",
            "---\nname: custom-name\ndescription: Custom\nuser-invocable: true\n---\nContent.",
        );

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert_eq!(catalog.all()[0].name, "custom-name");
    }

    #[test]
    fn matching_read_rules_finds_matches() {
        let dir = project()
            .file(
                "rust-rules/SKILL.md",
                "---\ndescription: Rust rules\ntriggers:\n  read:\n    - \"src/**/*.rs\"\n---\nRust rules.",
            )
            .file(
                "ts-rules/SKILL.md",
                "---\ndescription: TS rules\ntriggers:\n  read:\n    - \"src/**/*.ts\"\n---\nTS rules.",
            )
            .file("commit/SKILL.md", "---\ndescription: Commit\nuser-invocable: true\n---\nCommit.");

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
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
        let dir = project().file(
            "rule.md",
            "---\ndescription: A rule\nagent-invocable: false\ntriggers:\n  read:\n    - \"*.rs\"\n---\nRule content.",
        );

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.slash_commands().count(), 0);
        assert_eq!(catalog.skills().count(), 0);
    }

    #[test]
    fn skips_hidden_directories() {
        let dir = project()
            .file(".archived/SKILL.md", "---\ndescription: Archived\nuser-invocable: true\n---\nOld.")
            .file("visible/SKILL.md", "---\ndescription: Visible\nuser-invocable: true\n---\nNew.");

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].name, "visible");
    }

    #[test]
    fn from_dirs_last_wins() {
        let dir_a =
            project().file("rust/SKILL.md", "---\ndescription: Rust A\nagent-invocable: true\n---\nFrom dir A.");
        let dir_b =
            project().file("rust/SKILL.md", "---\ndescription: Rust B\nagent-invocable: true\n---\nFrom dir B.");

        let catalog = PromptCatalog::from_dirs(&[dir_a.root().to_path_buf(), dir_b.root().to_path_buf()]);
        assert_eq!(catalog.all().len(), 1);

        let spec = &catalog.all()[0];
        assert_eq!(spec.name, "rust");
        assert_eq!(spec.description, "Rust B");
        assert!(spec.body.contains("From dir B."));
    }

    #[test]
    fn from_dirs_union() {
        let dir_a =
            project().file("rust/SKILL.md", "---\ndescription: Rust\nagent-invocable: true\n---\nRust content.");
        let dir_b =
            project().file("python/SKILL.md", "---\ndescription: Python\nagent-invocable: true\n---\nPython content.");

        let catalog = PromptCatalog::from_dirs(&[dir_a.root().to_path_buf(), dir_b.root().to_path_buf()]);
        assert_eq!(catalog.all().len(), 2);

        let names: Vec<&str> = catalog.all().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"rust"));
        assert!(names.contains(&"python"));
    }

    #[test]
    fn from_dirs_skips_missing() {
        let dir_a =
            project().file("rust/SKILL.md", "---\ndescription: Rust\nagent-invocable: true\n---\nRust content.");
        let missing = PathBuf::from("/tmp/nonexistent-skills-dir-12345");

        let catalog = PromptCatalog::from_dirs(&[missing, dir_a.root().to_path_buf()]);
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].name, "rust");
    }

    #[test]
    fn discover_mixed_skill_md_and_flat_rules() {
        let dir = project()
            .file("commit/SKILL.md", "---\ndescription: Commit\nuser-invocable: true\n---\nCommit message.")
            .file("rust-rules.md", "---\ndescription: Rust rules\nglobs:\n  - \"**/*.rs\"\n---\nRust conventions.");

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert_eq!(catalog.all().len(), 2);

        let names: Vec<&str> = catalog.all().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"commit"));
        assert!(names.contains(&"rust-rules"));
    }

    #[test]
    fn from_dirs_merges_flat_rules() {
        let dir_a = project().file("commit/SKILL.md", "---\ndescription: Commit\nuser-invocable: true\n---\nCommit.");
        let dir_b = project()
            .file("rust-rules.md", "---\ndescription: Rust rules\nglobs:\n  - \"**/*.rs\"\n---\nRust conventions.");

        let catalog = PromptCatalog::from_dirs(&[dir_a.root().to_path_buf(), dir_b.root().to_path_buf()]);
        assert_eq!(catalog.all().len(), 2);

        let names: Vec<&str> = catalog.all().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"commit"));
        assert!(names.contains(&"rust-rules"));
    }

    #[test]
    fn skips_hidden_flat_md_files() {
        let dir = project()
            .file(".hidden-rule.md", "---\ndescription: Hidden\nglobs:\n  - \"**/*.rs\"\n---\nHidden.")
            .file("visible-rule.md", "---\ndescription: Visible\nglobs:\n  - \"**/*.ts\"\n---\nVisible.");

        let catalog = PromptCatalog::from_dir(dir.root()).unwrap();
        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.all()[0].name, "visible-rule");
    }
}
