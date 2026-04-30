use crate::error::CliError;
use aether_core::agent_spec::AgentSpec;
use aether_project::AgentCatalog;

pub fn resolve_agent_spec(catalog: &AgentCatalog, agent_name: Option<&str>) -> Result<AgentSpec, CliError> {
    match agent_name {
        Some(name) => catalog.resolve(name).map_err(|e| CliError::AgentError(e.to_string())),

        None => {
            if let Some(selected) = catalog.default_agent() {
                catalog.resolve(&selected.name).map_err(|e| CliError::AgentError(e.to_string()))
            } else {
                let model = "anthropic:claude-sonnet-4-5".parse().map_err(|e: String| CliError::ModelError(e))?;

                Ok(AgentSpec::default_spec(&model, None, Vec::new()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_project::{AetherSettings, AetherSettingsSource};
    use std::path::PathBuf;

    fn write_file(dir: &std::path::Path, path: &str, content: &str) {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
    }

    fn setup_catalog(settings_json: &str) -> (tempfile::TempDir, AgentCatalog) {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "PROMPT.md", "Be helpful");
        write_file(dir.path(), ".aether/settings.json", settings_json);
        let config =
            AetherSettings::load(dir.path(), [AetherSettingsSource::File(PathBuf::from(".aether/settings.json"))])
                .unwrap();
        let catalog = AgentCatalog::from_settings(dir.path(), config).unwrap();
        (dir, catalog)
    }

    #[test]
    fn resolve_with_explicit_name() {
        let (_dir, catalog) = setup_catalog(
            r#"{"agents": [
                {"name": "first", "description": "First", "model": "anthropic:claude-sonnet-4-5", "userInvocable": true, "prompts": [{"type":"file","path":"PROMPT.md"}]},
                {"name": "second", "description": "Second", "model": "anthropic:claude-sonnet-4-5", "userInvocable": true, "prompts": [{"type":"file","path":"PROMPT.md"}]}
            ]}"#,
        );
        let spec = resolve_agent_spec(&catalog, Some("second")).unwrap();
        assert_eq!(spec.name, "second");
    }

    #[test]
    fn resolve_auto_selects_first_user_invocable() {
        let (_dir, catalog) = setup_catalog(
            r#"{"agents": [
                {"name": "internal", "description": "Internal", "model": "anthropic:claude-sonnet-4-5", "agentInvocable": true, "prompts": [{"type":"file","path":"PROMPT.md"}]},
                {"name": "visible", "description": "Visible", "model": "anthropic:claude-sonnet-4-5", "userInvocable": true, "prompts": [{"type":"file","path":"PROMPT.md"}]}
            ]}"#,
        );
        let spec = resolve_agent_spec(&catalog, None).unwrap();
        assert_eq!(spec.name, "visible");
    }

    #[test]
    fn resolve_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = AgentCatalog::empty(dir.path().to_path_buf());
        let spec = resolve_agent_spec(&catalog, None).unwrap();
        assert_eq!(spec.name, "__default__");
    }

    #[test]
    fn resolve_unknown_name_errors() {
        let (_dir, catalog) = setup_catalog(
            r#"{"agents": [
                {"name": "alpha", "description": "Alpha", "model": "anthropic:claude-sonnet-4-5", "userInvocable": true, "prompts": [{"type":"file","path":"PROMPT.md"}]}
            ]}"#,
        );
        let result = resolve_agent_spec(&catalog, Some("nonexistent"));
        assert!(result.is_err());
    }
}
