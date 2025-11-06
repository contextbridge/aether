use crate::agent::{AgentError, Result, substitute_parameters};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub enum Prompt {
    Text(String),
    File {
        path: String,
        ancestors: bool,
        args: Option<HashMap<String, String>>,
    },
    SystemEnv,
}

impl Prompt {
    pub fn text(str: &str) -> Self {
        Self::Text(str.to_string())
    }

    pub fn file(path: &str, ancestors: bool) -> Self {
        Self::File {
            path: path.to_string(),
            ancestors,
            args: None,
        }
    }

    pub fn file_with_args(path: &str, ancestors: bool, args: HashMap<String, String>) -> Self {
        Self::File {
            path: path.to_string(),
            ancestors,
            args: Some(args),
        }
    }

    pub fn agents_md() -> Self {
        Self::File {
            path: "AGENTS.md".to_string(),
            ancestors: true,
            args: None,
        }
    }

    pub fn system_env() -> Self {
        Self::SystemEnv
    }

    /// Resolve this SystemPrompt to a String
    pub fn build(&self) -> Result<String> {
        match self {
            Prompt::Text(text) => Ok(text.clone()),
            Prompt::File {
                path,
                ancestors,
                args,
            } => {
                let content = if *ancestors {
                    Self::resolve_file_with_ancestors(path)?
                } else {
                    Self::resolve_file(&PathBuf::from(path))?
                };

                Ok(substitute_parameters(&content, args))
            }
            Prompt::SystemEnv => Self::resolve_system_env(),
        }
    }

    /// Resolve multiple SystemPrompts and join them with double newlines
    pub fn build_all(prompts: &[Prompt]) -> Result<String> {
        let content: Result<Vec<_>> = prompts.iter().map(|p| p.build()).collect();
        Ok(content?.join("\n\n"))
    }

    fn resolve_file(path: &Path) -> Result<String> {
        fs::read_to_string(path).map_err(|e| {
            AgentError::IoError(format!("Failed to read file '{}': {e}", path.display()))
        })
    }

    fn resolve_file_with_ancestors(filename: &str) -> Result<String> {
        let mut prompt = Vec::new();
        let mut current_dir = env::current_dir()
            .map_err(|e| AgentError::IoError(format!("Failed to get current directory: {e}")))?;

        loop {
            let file_path = current_dir.join(filename);
            if file_path.exists() && file_path.is_file() {
                let content = Self::resolve_file(&file_path)?;
                prompt.push(content);
            }

            match current_dir.parent() {
                Some(parent) => {
                    // Stop before root (/)
                    if parent.parent().is_none() {
                        break;
                    }
                    current_dir = parent.to_path_buf();
                }
                None => break,
            }
        }

        if prompt.is_empty() {
            return Err(AgentError::IoError(format!(
                "No '{filename}' files found in directory tree"
            )));
        }

        // Want root -> CWD (i.e. general --> specific prompt)
        prompt.reverse();
        Ok(prompt.join("\n\n"))
    }

    fn resolve_system_env() -> Result<String> {
        let cwd = env::current_dir()
            .map_err(|e| AgentError::IoError(format!("Failed to get current directory: {e}")))?;

        let os_version = Command::new("uname")
            .arg("-a")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|version| {
                let version = version.trim();
                if !version.is_empty() {
                    Some(format!("OS Version: {version}"))
                } else {
                    None
                }
            });

        let is_git_repo = if cwd.join(".git").exists() {
            "Yes"
        } else {
            "No"
        };

        let mut lines = vec![
            format!("Working directory: {}", cwd.display()),
            format!("Platform: {}", env::consts::OS),
            format!("Today's date: {}", chrono::Local::now().format("%Y-%m-%d")),
            format!("Is directory a git repo: {}", is_git_repo),
        ];

        if let Some(os) = os_version {
            lines.push(os);
        }

        Ok(format!("<env>\n{}\n</env>", lines.join("\n")))
    }
}
