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

        let yaml = serde_yml::to_string(&frontmatter)?;
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

        let frontmatter: PromptFrontmatter = serde_yml::from_str(yaml_str)?;

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
    fn new(glob_patterns: Vec<String>) -> Result<Self, PromptFileError> {
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
    Yaml(#[from] serde_yml::Error),
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
    use tempfile::TempDir;

    fn minimal_frontmatter(description: &str) -> PromptFrontmatter {
        PromptFrontmatter {
            description: description.to_string(),
            name: None,
            user_invocable: None,
            agent_invocable: None,
            argument_hint: None,
            tags: vec![],
            triggers: None,
            globs: vec![],
            paths: vec![],
            agent_authored: false,
            helpful: 0,
            harmful: 0,
        }
    }

    #[test]
    fn frontmatter_serde_roundtrip() {
        let fm = minimal_frontmatter("A simple skill");

        let yaml = serde_yml::to_string(&fm).unwrap();
        let parsed: PromptFrontmatter = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(parsed.description, "A simple skill");
        assert!(parsed.tags.is_empty());
        assert!(!parsed.agent_authored);
    }

    #[test]
    fn frontmatter_serde_with_all_fields() {
        let mut fm = minimal_frontmatter("A full skill");
        fm.tags = vec!["convention".to_string(), "testing".to_string()];
        fm.agent_authored = true;
        fm.helpful = 5;
        fm.harmful = 2;

        let yaml = serde_yml::to_string(&fm).unwrap();
        let parsed: PromptFrontmatter = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(parsed.description, "A full skill");
        assert_eq!(parsed.tags, vec!["convention", "testing"]);
        assert!(parsed.agent_authored);
        assert_eq!(parsed.helpful, 5);
        assert_eq!(parsed.harmful, 2);
    }

    #[test]
    fn backward_compat_old_frontmatter() {
        let yaml = "description: An old skill\n";
        let parsed: PromptFrontmatter = serde_yml::from_str(yaml).unwrap();
        assert_eq!(parsed.description, "An old skill");
        assert!(parsed.tags.is_empty());
        assert!(!parsed.agent_authored);
        assert_eq!(parsed.helpful, 0);
        assert_eq!(parsed.harmful, 0);
    }

    #[test]
    fn confidence() {
        let pf = |helpful, harmful| PromptFile {
            name: String::new(),
            description: "test".to_string(),
            body: String::new(),
            path: PathBuf::new(),
            user_invocable: false,
            agent_invocable: false,
            argument_hint: None,
            tags: vec![],
            triggers: PromptTriggers::default(),
            agent_authored: true,
            helpful,
            harmful,
        };

        assert!((pf(0, 0).confidence() - 0.0).abs() < f64::EPSILON);
        assert!((pf(7, 1).confidence() - 7.0 / 9.0).abs() < f64::EPSILON);
        assert!((pf(0, 5).confidence() - 0.0).abs() < f64::EPSILON);
        assert!((pf(3, 0).confidence() - 3.0 / 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_frontmatter_from_string() {
        let content = "---\ndescription: Test skill\ntags:\n  - rust\nagent_authored: true\nhelpful: 3\nharmful: 1\n---\n# My Skill\n\nSome content here.";
        let (fm, body) = PromptFile::parse_frontmatter(content).unwrap();
        assert_eq!(fm.description, "Test skill");
        assert_eq!(fm.tags, vec!["rust"]);
        assert!(fm.agent_authored);
        assert_eq!(fm.helpful, 3);
        assert_eq!(fm.harmful, 1);
        assert!(body.contains("# My Skill"));
        assert!(body.contains("Some content here."));
    }

    #[test]
    fn write_and_parse_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("my-skill").join(SKILL_FILENAME);

        let prompt = PromptFile {
            name: "my-skill".to_string(),
            description: "Test skill".to_string(),
            body: "# My Skill\n\nSome content here.".to_string(),
            path: skill_path.clone(),
            user_invocable: false,
            agent_invocable: true,
            argument_hint: None,
            tags: vec!["convention".to_string()],
            triggers: PromptTriggers::default(),
            agent_authored: true,
            helpful: 2,
            harmful: 1,
        };
        prompt.write(&skill_path).unwrap();

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
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("empty-body").join(SKILL_FILENAME);

        let prompt = PromptFile {
            name: "empty-body".to_string(),
            description: "Empty".to_string(),
            body: String::new(),
            path: skill_path.clone(),
            user_invocable: false,
            agent_invocable: true,
            argument_hint: None,
            tags: vec![],
            triggers: PromptTriggers::default(),
            agent_authored: true,
            helpful: 0,
            harmful: 0,
        };
        prompt.write(&skill_path).unwrap();

        let raw = std::fs::read_to_string(&skill_path).unwrap();
        assert!(raw.starts_with("---\n"));
        assert!(raw.contains("description: Empty"));
    }

    #[test]
    fn write_and_parse_roundtrip_with_triggers() {
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("rust-rules").join(SKILL_FILENAME);

        let triggers = PromptTriggers::new(vec!["src/**/*.rs".to_string(), "tests/**/*.rs".to_string()]).unwrap();

        let prompt = PromptFile {
            name: "rust-rules".to_string(),
            description: "Rust conventions".to_string(),
            body: "Follow Rust conventions.".to_string(),
            path: skill_path.clone(),
            user_invocable: false,
            agent_invocable: false,
            argument_hint: None,
            tags: vec![],
            triggers,
            agent_authored: false,
            helpful: 0,
            harmful: 0,
        };
        prompt.write(&skill_path).unwrap();

        let parsed = PromptFile::parse(&skill_path).unwrap();
        assert_eq!(parsed.description, "Rust conventions");
        assert!(!parsed.triggers.is_empty());
        assert!(parsed.triggers.matches_read("src/main.rs"));
        assert!(parsed.triggers.matches_read("tests/integration.rs"));
        assert!(!parsed.triggers.matches_read("README.md"));
        assert_eq!(parsed.triggers.patterns(), &["src/**/*.rs", "tests/**/*.rs"]);
    }

    #[test]
    fn write_rejects_empty_description() {
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("bad").join(SKILL_FILENAME);

        let prompt = PromptFile {
            name: "bad".to_string(),
            description: String::new(),
            body: "content".to_string(),
            path: skill_path.clone(),
            user_invocable: true,
            agent_invocable: false,
            argument_hint: None,
            tags: vec![],
            triggers: PromptTriggers::default(),
            agent_authored: true,
            helpful: 0,
            harmful: 0,
        };
        let result = prompt.write(&skill_path);
        assert!(matches!(result, Err(PromptFileError::MissingDescription { .. })));
    }

    #[test]
    fn write_rejects_no_activation_surface() {
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("noop").join(SKILL_FILENAME);

        let prompt = PromptFile {
            name: "noop".to_string(),
            description: "Does nothing".to_string(),
            body: "content".to_string(),
            path: skill_path.clone(),
            user_invocable: false,
            agent_invocable: false,
            argument_hint: None,
            tags: vec![],
            triggers: PromptTriggers::default(),
            agent_authored: true,
            helpful: 0,
            harmful: 0,
        };
        let result = prompt.write(&skill_path);
        assert!(matches!(result, Err(PromptFileError::NoActivationSurface { .. })));
    }

    #[test]
    fn skip_serializing_defaults() {
        let fm = minimal_frontmatter("Minimal");

        let yaml = serde_yml::to_string(&fm).unwrap();
        assert!(!yaml.contains("tags"));
        assert!(!yaml.contains("agent_authored"));
        assert!(!yaml.contains("helpful"));
        assert!(!yaml.contains("harmful"));
    }

    #[test]
    fn parse_globs_key() {
        let content = r#"---
description: TS conventions
globs:
  - "src/**/*.ts"
  - "src/**/*.tsx"
---
Use strict TypeScript."#;
        let (fm, body) = PromptFile::parse_frontmatter(content).unwrap();
        assert_eq!(fm.globs, vec!["src/**/*.ts", "src/**/*.tsx"]);
        assert!(fm.triggers.is_none());
        assert!(body.contains("Use strict TypeScript."));
    }

    #[test]
    fn parse_paths_key() {
        let content = r#"---
description: Rust rules
paths:
  - "**/*.rs"
---
Follow Rust conventions."#;
        let (fm, _) = PromptFile::parse_frontmatter(content).unwrap();
        assert_eq!(fm.paths, vec!["**/*.rs"]);
        assert!(fm.triggers.is_none());
    }

    #[test]
    fn parse_merges_all_glob_sources() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("merged-rules").join(SKILL_FILENAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"---
description: Merged
triggers:
  read:
    - "src/**/*.rs"
globs:
  - "lib/**/*.ts"
paths:
  - "app/**/*.py"
---
Merged rules."#,
        )
        .unwrap();

        let parsed = PromptFile::parse(&path).unwrap();
        assert!(parsed.triggers.matches_read("src/main.rs"));
        assert!(parsed.triggers.matches_read("lib/index.ts"));
        assert!(parsed.triggers.matches_read("app/main.py"));
    }

    #[test]
    fn parse_globs_as_activation_surface() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("globs-only.md");
        std::fs::write(
            &path,
            r#"---
description: TS rules
globs:
  - "**/*.ts"
---
TypeScript rules."#,
        )
        .unwrap();

        let parsed = PromptFile::parse(&path).unwrap();
        assert_eq!(parsed.name, "globs-only");
        assert!(parsed.triggers.matches_read("src/index.ts"));
    }

    #[test]
    fn name_from_file_stem_for_non_skill_md() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("rust-conventions.md");
        std::fs::write(
            &path,
            r#"---
description: Rust conventions
globs:
  - "**/*.rs"
---
Follow Rust conventions."#,
        )
        .unwrap();

        let parsed = PromptFile::parse(&path).unwrap();
        assert_eq!(parsed.name, "rust-conventions");
    }

    #[test]
    fn empty_description_defaults_to_name() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("my-rule.md");
        std::fs::write(
            &path,
            r#"---
globs:
  - "**/*.rs"
---
Rule body."#,
        )
        .unwrap();

        let parsed = PromptFile::parse(&path).unwrap();
        assert_eq!(parsed.name, "my-rule");
        assert_eq!(parsed.description, "my-rule");
    }

    #[test]
    fn skill_file_defaults_user_invocable_true_when_missing() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("compat-skill").join(SKILL_FILENAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r"---
description: Claude-style skill
---
Skill body.",
        )
        .unwrap();

        let parsed = PromptFile::parse(&path).unwrap();
        assert!(parsed.user_invocable);
        assert!(parsed.agent_invocable);
    }

    #[test]
    fn non_skill_md_without_activation_surface_still_rejected() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("noop.md");
        std::fs::write(
            &path,
            r"---
description: No activation
agent-invocable: false
---
Rule body.",
        )
        .unwrap();

        let result = PromptFile::parse(&path);
        assert!(matches!(result, Err(PromptFileError::NoActivationSurface { .. })));
    }
}
