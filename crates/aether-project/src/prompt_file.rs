use std::fs;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

pub const SKILL_FILENAME: &str = "SKILL.md";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct PromptFrontmatter {
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, rename = "user-invocable", skip_serializing_if = "Option::is_none")]
    pub user_invocable: Option<bool>,
    #[serde(default, rename = "agent-invocable", skip_serializing_if = "Option::is_none")]
    pub agent_invocable: Option<bool>,
    #[serde(default, rename = "argument-hint", skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggers: Option<Triggers>,
    /// Claude Code compatibility: top-level glob patterns (alias for `triggers.read`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub globs: Vec<String>,
    /// Cursor compatibility: top-level path patterns (alias for `triggers.read`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "not")]
    pub agent_authored: bool,
    #[serde(default, skip_serializing_if = "zero")]
    pub helpful: u32,
    #[serde(default, skip_serializing_if = "zero")]
    pub harmful: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Triggers {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
}

/// A resolved skill artifact discovered from a `SKILL.md` file.
#[derive(Debug, Clone)]
pub struct PromptFile {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: PathBuf,
    pub user_invocable: bool,
    pub agent_invocable: bool,
    pub argument_hint: Option<String>,
    pub tags: Vec<String>,
    pub triggers: PromptTriggers,
    pub agent_authored: bool,
    pub helpful: u32,
    pub harmful: u32,
}

impl PromptFile {
    /// Parse a prompt file at the given path into a `PromptFile`.
    ///
    /// The name defaults to the parent directory name unless overridden in frontmatter.
    pub fn parse(path: &Path) -> Result<Self, PromptFileError> {
        let raw = fs::read_to_string(path)?;
        let is_skill_file = path.file_name().is_some_and(|n| n == SKILL_FILENAME);

        let (frontmatter, body) = Self::parse_frontmatter(raw.trim())?;

        let default_name = if is_skill_file {
            path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        };

        let name = frontmatter.name.unwrap_or(default_name);
        let description = frontmatter.description.trim().to_string();
        let description = if description.is_empty() { name.clone() } else { description };
        let user_invocable = frontmatter.user_invocable.unwrap_or(is_skill_file);
        let agent_invocable = frontmatter.agent_invocable.unwrap_or(true);

        let mut read_globs = frontmatter.triggers.map(|t| t.read).unwrap_or_default();
        read_globs.extend(frontmatter.globs);
        read_globs.extend(frontmatter.paths);

        if !user_invocable && !agent_invocable && read_globs.is_empty() {
            return Err(PromptFileError::NoActivationSurface { name });
        }

        let triggers = PromptTriggers::new(read_globs)?;

        Ok(Self {
            name,
            description,
            body,
            path: path.to_path_buf(),
            user_invocable,
            agent_invocable,
            argument_hint: frontmatter.argument_hint,
            tags: frontmatter.tags,
            triggers,
            agent_authored: frontmatter.agent_authored,
            helpful: frontmatter.helpful,
            harmful: frontmatter.harmful,
        })
    }

    /// Validate this prompt file has a non-empty description and at least one activation surface.
    pub fn validate(&self) -> Result<(), PromptFileError> {
        if self.description.trim().is_empty() {
            return Err(PromptFileError::MissingDescription { name: self.name.clone() });
        }

        let has_read_triggers = !self.triggers.is_empty();
        if !self.user_invocable && !self.agent_invocable && !has_read_triggers {
            return Err(PromptFileError::NoActivationSurface { name: self.name.clone() });
        }

        Ok(())
    }

    /// Write this prompt file to the given path, creating parent directories as needed.
    pub fn write(&self, path: &Path) -> Result<(), PromptFileError> {
        self.validate()?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let triggers =
            if self.triggers.is_empty() { None } else { Some(Triggers { read: self.triggers.patterns().to_vec() }) };

        let frontmatter = PromptFrontmatter {
            description: self.description.clone(),
            name: Some(self.name.clone()),
            user_invocable: self.user_invocable.then_some(true),
            agent_invocable: (!self.agent_invocable).then_some(false),
            argument_hint: self.argument_hint.clone(),
            tags: self.tags.clone(),
            triggers,
            globs: vec![],
            paths: vec![],
            agent_authored: self.agent_authored,
            helpful: self.helpful,
            harmful: self.harmful,
        };

        let yaml = noyalib::to_string(&frontmatter)?;
        let yaml = normalize_frontmatter_yaml(&yaml);

        let file_content = if self.body.is_empty() {
            format!("---\n{yaml}\n---\n")
        } else {
            format!("---\n{yaml}\n---\n{}\n", self.body)
        };
        fs::write(path, file_content)?;
        Ok(())
    }

    /// Confidence score based on helpful/harmful ratings.
    pub fn confidence(&self) -> f64 {
        f64::from(self.helpful) / (f64::from(self.helpful) + f64::from(self.harmful) + 1.0)
    }

    /// Parse YAML frontmatter and body from a SKILL.md content string (no I/O).
    fn parse_frontmatter(content: &str) -> Result<(PromptFrontmatter, String), PromptFileError> {
        let (yaml_str, body) =
            utils::markdown_file::split_frontmatter(content).ok_or(PromptFileError::MissingFrontmatter)?;

        let frontmatter: PromptFrontmatter = noyalib::from_str(yaml_str)?;

        Ok((frontmatter, body.to_string()))
    }
}

/// Trigger configuration for automatic prompt activation.
#[derive(Debug, Clone, Default)]
pub struct PromptTriggers {
    patterns: Vec<String>,
    globs: Option<GlobSet>,
}

impl PromptTriggers {
    pub(crate) fn new(glob_patterns: Vec<String>) -> Result<Self, PromptFileError> {
        if glob_patterns.is_empty() {
            return Ok(Self { patterns: Vec::new(), globs: None });
        }

        let mut builder = GlobSetBuilder::new();
        for pattern in &glob_patterns {
            let glob = Glob::new(pattern)
                .map_err(|e| PromptFileError::InvalidTriggerGlob { pattern: pattern.clone(), error: e.to_string() })?;
            builder.add(glob);
        }

        let globs = builder.build().map_err(|e| PromptFileError::InvalidTriggerGlob {
            pattern: glob_patterns.join(", "),
            error: e.to_string(),
        })?;

        Ok(Self { patterns: glob_patterns, globs: Some(globs) })
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    pub fn is_empty(&self) -> bool {
        self.globs.is_none()
    }

    /// Check if a project-relative path matches any read trigger glob.
    pub fn matches_read(&self, relative_path: &str) -> bool {
        self.globs.as_ref().is_some_and(|gs| gs.is_match(relative_path))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PromptFileError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML error: {0}")]
    Yaml(#[from] noyalib::Error),
    #[error("missing YAML frontmatter")]
    MissingFrontmatter,
    #[error("skill '{name}' has an empty description")]
    MissingDescription { name: String },
    #[error("skill '{name}' must have at least one of: user-invocable, agent-invocable, triggers, globs, or paths")]
    NoActivationSurface { name: String },
    #[error("invalid trigger glob '{pattern}': {error}")]
    InvalidTriggerGlob { pattern: String, error: String },
    #[error("skill not found: {0}")]
    NotFound(String),
    #[error("skill '{0}' is not agent-authored and cannot be modified")]
    NotAgentAuthored(String),
}

fn normalize_frontmatter_yaml(yaml: &str) -> &str {
    let yaml = yaml.trim();
    let yaml = yaml.strip_prefix("---\n").unwrap_or(yaml);
    yaml.strip_suffix("\n...").unwrap_or(yaml).trim()
}

#[expect(clippy::trivially_copy_pass_by_ref)]
fn not(b: &bool) -> bool {
    !b
}

#[expect(clippy::trivially_copy_pass_by_ref)]
fn zero(n: &u32) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{project, prompt_file};

    #[test]
    fn parse_skill_frontmatter_and_body() {
        let dir = project().skill(
            "my-skill",
            "---\ndescription: Test skill\ntags:\n  - rust\nagent_authored: true\nhelpful: 3\nharmful: 1\n---\n# My Skill\n\nSome content here.",
        );

        let parsed = PromptFile::parse(&dir.root().join("my-skill").join(SKILL_FILENAME)).unwrap();
        assert_eq!(parsed.name, "my-skill");
        assert_eq!(parsed.description, "Test skill");
        assert_eq!(parsed.tags, vec!["rust"]);
        assert!(parsed.agent_authored);
        assert_eq!(parsed.helpful, 3);
        assert_eq!(parsed.harmful, 1);
        assert!(parsed.body.contains("# My Skill"));
        assert!(parsed.body.contains("Some content here."));
    }

    #[test]
    fn backward_compat_old_frontmatter() {
        let dir = project().skill("old-skill", "---\ndescription: An old skill\n---\nBody.");

        let parsed = PromptFile::parse(&dir.root().join("old-skill").join(SKILL_FILENAME)).unwrap();
        assert_eq!(parsed.description, "An old skill");
        assert!(parsed.tags.is_empty());
        assert!(!parsed.agent_authored);
        assert_eq!(parsed.helpful, 0);
        assert_eq!(parsed.harmful, 0);
    }

    #[test]
    fn confidence() {
        let prompt = |helpful, harmful| prompt_file("test").ratings(helpful, harmful).build();

        assert!((prompt(0, 0).confidence() - 0.0).abs() < f64::EPSILON);
        assert!((prompt(7, 1).confidence() - 7.0 / 9.0).abs() < f64::EPSILON);
        assert!((prompt(0, 5).confidence() - 0.0).abs() < f64::EPSILON);
        assert!((prompt(3, 0).confidence() - 3.0 / 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn write_and_parse_roundtrip() {
        let project = project();
        let skill_path = project.root().join("my-skill").join(SKILL_FILENAME);

        prompt_file("my-skill")
            .description("Test skill")
            .body("# My Skill\n\nSome content here.")
            .tags(&["convention"])
            .agent_authored(true)
            .ratings(2, 1)
            .build()
            .write(&skill_path)
            .unwrap();

        let parsed = PromptFile::parse(&skill_path).unwrap();
        assert_eq!(parsed.description, "Test skill");
        assert_eq!(parsed.tags, vec!["convention"]);
        assert!(parsed.agent_authored);
        assert_eq!(parsed.helpful, 2);
        assert_eq!(parsed.harmful, 1);
        assert!(parsed.body.contains("# My Skill"));
        assert!(parsed.body.contains("Some content here."));
    }

    #[test]
    fn write_empty_body() {
        let project = project();
        let skill_path = project.root().join("empty-body").join(SKILL_FILENAME);

        prompt_file("empty-body").description("Empty").body("").build().write(&skill_path).unwrap();

        let raw = std::fs::read_to_string(&skill_path).unwrap();
        assert!(raw.starts_with("---\n"));
        assert!(raw.contains("description: Empty"));
    }

    #[test]
    fn write_and_parse_roundtrip_with_triggers() {
        let project = project();
        let skill_path = project.root().join("rust-rules").join(SKILL_FILENAME);

        prompt_file("rust-rules")
            .description("Rust conventions")
            .body("Follow Rust conventions.")
            .user_invocable(false)
            .agent_invocable(false)
            .read_triggers(&["src/**/*.rs", "tests/**/*.rs"])
            .build()
            .write(&skill_path)
            .unwrap();

        let parsed = PromptFile::parse(&skill_path).unwrap();
        assert_eq!(parsed.description, "Rust conventions");
        assert!(!parsed.triggers.is_empty());
        assert!(parsed.triggers.matches_read("src/main.rs"));
        assert!(parsed.triggers.matches_read("tests/integration.rs"));
        assert!(!parsed.triggers.matches_read("README.md"));
        assert_eq!(parsed.triggers.patterns(), ["src/**/*.rs", "tests/**/*.rs"]);
    }

    #[test]
    fn write_rejects_empty_description() {
        let project = project();
        let skill_path = project.root().join("bad").join(SKILL_FILENAME);

        let result = prompt_file("bad").description("").build().write(&skill_path);
        assert!(matches!(result, Err(PromptFileError::MissingDescription { .. })));
    }

    #[test]
    fn write_rejects_no_activation_surface() {
        let project = project();
        let skill_path = project.root().join("noop").join(SKILL_FILENAME);

        let result = prompt_file("noop").user_invocable(false).agent_invocable(false).build().write(&skill_path);
        assert!(matches!(result, Err(PromptFileError::NoActivationSurface { .. })));
    }

    #[test]
    fn write_skips_default_frontmatter_fields() {
        let project = project();
        let skill_path = project.root().join("minimal").join(SKILL_FILENAME);

        prompt_file("minimal").build().write(&skill_path).unwrap();

        let raw = std::fs::read_to_string(&skill_path).unwrap();
        assert!(raw.contains("description: minimal skill"));
        assert!(!raw.contains("tags"));
        assert!(!raw.contains("agent_authored"));
        assert!(!raw.contains("agent-invocable"));
        assert!(!raw.contains("argument-hint"));
        assert!(!raw.contains("helpful"));
        assert!(!raw.contains("harmful"));
        assert!(!raw.contains("triggers"));
    }

    #[test]
    fn parse_globs_key() {
        let dir = project().skill(
            "ts-conventions",
            "---\ndescription: TS conventions\nglobs:\n  - \"src/**/*.ts\"\n  - \"src/**/*.tsx\"\n---\nUse strict TypeScript.",
        );

        let parsed = PromptFile::parse(&dir.root().join("ts-conventions").join(SKILL_FILENAME)).unwrap();
        assert_eq!(parsed.triggers.patterns(), ["src/**/*.ts", "src/**/*.tsx"]);
        assert!(parsed.triggers.matches_read("src/main.ts"));
        assert!(parsed.body.contains("Use strict TypeScript."));
    }

    #[test]
    fn parse_paths_key() {
        let dir = project().skill(
            "rust-rules",
            "---\ndescription: Rust rules\npaths:\n  - \"**/*.rs\"\n---\nFollow Rust conventions.",
        );

        let parsed = PromptFile::parse(&dir.root().join("rust-rules").join(SKILL_FILENAME)).unwrap();
        assert_eq!(parsed.triggers.patterns(), ["**/*.rs"]);
        assert!(parsed.triggers.matches_read("src/lib.rs"));
    }

    #[test]
    fn parse_merges_all_glob_sources() {
        let dir = project().skill(
            "merged-rules",
            "---\ndescription: Merged\ntriggers:\n  read:\n    - \"src/**/*.rs\"\nglobs:\n  - \"lib/**/*.ts\"\npaths:\n  - \"app/**/*.py\"\n---\nMerged rules.",
        );

        let parsed = PromptFile::parse(&dir.root().join("merged-rules").join(SKILL_FILENAME)).unwrap();
        assert!(parsed.triggers.matches_read("src/main.rs"));
        assert!(parsed.triggers.matches_read("lib/index.ts"));
        assert!(parsed.triggers.matches_read("app/main.py"));
    }

    #[test]
    fn parse_globs_as_activation_surface() {
        let dir = project()
            .file("globs-only.md", "---\ndescription: TS rules\nglobs:\n  - \"**/*.ts\"\n---\nTypeScript rules.");

        let parsed = PromptFile::parse(&dir.root().join("globs-only.md")).unwrap();
        assert_eq!(parsed.name, "globs-only");
        assert!(parsed.triggers.matches_read("src/index.ts"));
    }

    #[test]
    fn name_from_file_stem_for_non_skill_md() {
        let dir = project().file(
            "rust-conventions.md",
            "---\ndescription: Rust conventions\nglobs:\n  - \"**/*.rs\"\n---\nFollow Rust conventions.",
        );

        let parsed = PromptFile::parse(&dir.root().join("rust-conventions.md")).unwrap();
        assert_eq!(parsed.name, "rust-conventions");
    }

    #[test]
    fn empty_description_defaults_to_name() {
        let dir = project().file("my-rule.md", "---\nglobs:\n  - \"**/*.rs\"\n---\nRule body.");

        let parsed = PromptFile::parse(&dir.root().join("my-rule.md")).unwrap();
        assert_eq!(parsed.name, "my-rule");
        assert_eq!(parsed.description, "my-rule");
    }

    #[test]
    fn skill_file_defaults_user_invocable_true_when_missing() {
        let dir = project().skill("compat-skill", "---\ndescription: Claude-style skill\n---\nSkill body.");

        let parsed = PromptFile::parse(&dir.root().join("compat-skill").join(SKILL_FILENAME)).unwrap();
        assert!(parsed.user_invocable);
        assert!(parsed.agent_invocable);
    }

    #[test]
    fn non_skill_md_without_activation_surface_still_rejected() {
        let dir = project().file("noop.md", "---\ndescription: No activation\nagent-invocable: false\n---\nRule body.");

        let result = PromptFile::parse(&dir.root().join("noop.md"));
        assert!(matches!(result, Err(PromptFileError::NoActivationSurface { .. })));
    }
}
