use crate::client::aether_home;

use super::McpError;
use super::config::ToolExposure;
use super::connection::convert_tool_annotations;
use super::mcp_client::McpClient;
use llm::{ToolAnnotations, ToolDefinition};
use rmcp::{RoleClient, service::RunningService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{create_dir_all, remove_dir_all, write};

/// The reserved server name of Aether's shared MCP tool proxy.
pub const PROXY_SERVER_NAME: &str = "proxy";

/// The namespaced name of the proxy's `call_tool` virtual tool.
pub const PROXY_CALL_TOOL_NAME: &str = "proxy__call_tool";

/// Resolved proxy call returned by [`resolve_call`].
#[derive(Debug)]
pub struct ResolvedCall {
    pub server: String,
    pub tool: String,
    pub arguments: Option<Map<String, Value>>,
}

/// Parse a proxy `call_tool` invocation.
pub fn resolve_call(arguments_json: &str) -> super::Result<ResolvedCall> {
    let args: ProxyCallArgs = serde_json::from_str(arguments_json)?;
    Ok(ResolvedCall { server: args.server, tool: args.tool, arguments: args.arguments })
}

/// Returns the directory for the tool-proxy's tool definitions.
///
/// Uses `$AETHER_HOME/tool-proxy/proxy` or `~/.aether/tool-proxy/proxy`.
pub fn dir() -> Result<PathBuf, McpError> {
    let base = aether_home().ok_or_else(|| McpError::Other("Home directory not set".into()))?;
    Ok(dir_in_home(&base))
}

pub fn dir_in_home(home: &Path) -> PathBuf {
    home.join("tool-proxy").join(PROXY_SERVER_NAME)
}

/// Clean up the tool directory for the proxy, removing all tool files.
pub async fn clean_dir(tool_dir: &Path) -> Result<(), McpError> {
    if tool_dir.exists() {
        remove_dir_all(tool_dir).await.map_err(|e| McpError::Other(format!("Failed to clean tool-proxy dir: {e}")))?;
    }
    Ok(())
}

/// Build the `call_tool` JSON schema used by the proxy's virtual tool.
pub fn call_tool_schema() -> Arc<Map<String, Value>> {
    let schema = schemars::schema_for!(ProxyCallArgs);
    let value = serde_json::to_value(schema).expect("schema serialization cannot fail");
    Arc::new(value.as_object().expect("schema is always an object").clone())
}

/// Build a `ToolDefinition` for the proxy's `call_tool` virtual tool.
pub fn call_tool_definition() -> ToolDefinition {
    let schema = call_tool_schema();
    ToolDefinition::new(
        PROXY_CALL_TOOL_NAME,
        "Execute a tool on a nested MCP server when it is not exposed directly. Browse the tool-proxy directory to discover available tools first.",
        Value::Object((*schema).clone()),
    )
    .with_server(PROXY_SERVER_NAME.to_string())
}

/// Write the exposure's proxied tool entries to `tool_dir/<server_name>/`,
/// removing any stale files first. Directly exposed tools are omitted.
pub(super) async fn write_tool_entries_to_dir(
    server_name: &str,
    tools: &[rmcp::model::Tool],
    exposure: &ToolExposure,
    tool_dir: &Path,
) -> Result<(), McpError> {
    let server_dir = tool_dir.join(server_name);
    if server_dir.exists() {
        remove_dir_all(&server_dir).await?;
    }
    create_dir_all(&server_dir).await?;

    for tool in tools.iter().filter(|tool| !exposure.is_direct_tool(&tool.name)) {
        let entry = ToolFileEntry {
            name: tool.name.to_string(),
            description: tool.description.clone().unwrap_or_default().to_string(),
            server: server_name.to_string(),
            parameters: Value::Object((*tool.input_schema).clone()),
            annotations: tool.annotations.as_ref().map(convert_tool_annotations),
        };

        let file_path = server_dir.join(format!("{}.json", tool.name));
        let json = serde_json::to_string_pretty(&entry)?;
        write(&file_path, json).await?;
    }

    Ok(())
}

/// Extract a one-line description for a nested server from its peer info.
///
/// Uses `server_info.description`, falling back to the server name.
pub fn extract_server_description(client: &RunningService<RoleClient, McpClient>, server_name: &str) -> String {
    client
        .peer_info()
        .and_then(|info| {
            info.server_info
                .as_ref()
                .and_then(|server_info| server_info.description.as_deref())
                .filter(|description| !description.is_empty())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| server_name.to_string())
}

/// Build proxy instructions describing the tool directory and connected servers.
pub fn build_instructions(tool_dir: &Path, server_descriptions: &[(String, String)]) -> String {
    use std::fmt::Write;

    let mut instructions = format!(
        "Tools that are not exposed directly are available through connected MCP servers at `{tool_dir}`.\n\
         Each subdirectory in `{tool_dir}` represents a connected MCP server and contains JSON tool definitions.\n\
         Browse or grep the directory to discover tools, then use `call_tool` to execute them.",
        tool_dir = tool_dir.display()
    );

    if !server_descriptions.is_empty() {
        instructions.push_str("\n\n## Connected Servers\n");
        for (name, desc) in server_descriptions {
            let _ = writeln!(instructions, "- **{name}**: {desc}");
        }
    }

    instructions
}

/// A tool definition written to disk for agent browsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFileEntry {
    pub name: String,
    pub description: String,
    pub server: String,
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

/// Parsed arguments from a proxy `call_tool` invocation.
#[derive(Deserialize, JsonSchema)]
struct ProxyCallArgs {
    /// The server name (directory name in the tool-proxy folder)
    server: String,
    /// The tool name (file name without .json)
    tool: String,
    /// Arguments to pass to the tool
    arguments: Option<Map<String, Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::config::ToolProxyRules;
    use crate::client::naming::create_namespaced_tool_name;
    use rmcp::model::{Tool, ToolAnnotations};
    use serde_json::json;

    #[test]
    fn tool_file_entry_serialization() {
        let entry = ToolFileEntry {
            name: "create_issue".to_string(),
            description: "Create a GitHub issue".to_string(),
            server: "github".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "title": { "type": "string" }
                },
                "required": ["repo", "title"]
            }),
            annotations: None,
        };

        let json_str = serde_json::to_string_pretty(&entry).unwrap();
        let deserialized: ToolFileEntry = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.name, "create_issue");
        assert_eq!(deserialized.server, "github");
        assert_eq!(deserialized.description, "Create a GitHub issue");
    }

    #[test]
    fn call_tool_schema_is_valid() {
        let schema = call_tool_schema();
        assert_eq!(schema.get("type").unwrap(), "object");

        let properties = schema.get("properties").unwrap().as_object().unwrap();
        assert!(properties.contains_key("server"));
        assert!(properties.contains_key("tool"));
        assert!(properties.contains_key("arguments"));

        let required = schema.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 2);
        let required_names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(required_names.contains(&"server"));
        assert!(required_names.contains(&"tool"));
    }

    #[test]
    fn tool_proxy_dir_appends_correct_suffix() {
        let dir = dir().unwrap();
        assert!(
            dir.ends_with("tool-proxy/proxy"),
            "Expected path to end with tool-proxy/proxy, got: {}",
            dir.display()
        );
    }

    #[test]
    fn write_and_read_tool_files() {
        let tmp = tempfile::tempdir().unwrap();
        let tool_dir = tmp.path().to_path_buf();
        let server_dir = tool_dir.join("test-server");
        std::fs::create_dir_all(&server_dir).unwrap();

        let entry = ToolFileEntry {
            name: "my_tool".to_string(),
            description: "Does stuff".to_string(),
            server: "test-server".to_string(),
            parameters: json!({"type": "object", "properties": {}}),
            annotations: None,
        };

        let file_path = server_dir.join("my_tool.json");
        let json = serde_json::to_string_pretty(&entry).unwrap();
        std::fs::write(&file_path, &json).unwrap();

        let contents = std::fs::read_to_string(&file_path).unwrap();
        let parsed: ToolFileEntry = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed.name, "my_tool");
        assert_eq!(parsed.server, "test-server");
    }

    #[test]
    fn call_tool_definition_uses_proxy_name_constants() {
        let def = call_tool_definition();
        assert_eq!(def.name, PROXY_CALL_TOOL_NAME);
        assert_eq!(def.server, Some(PROXY_SERVER_NAME.to_string()));
        assert!(def.description.contains("Execute a tool"));
        assert_eq!(PROXY_CALL_TOOL_NAME, create_namespaced_tool_name(PROXY_SERVER_NAME, "call_tool"));
    }

    #[test]
    fn build_proxy_instructions_includes_tool_dir_and_servers() {
        let tool_dir = std::path::Path::new("/tmp/tool-proxy/test");
        let descriptions =
            vec![("math".to_string(), "Math tools".to_string()), ("git".to_string(), "Git tools".to_string())];
        let instr = build_instructions(tool_dir, &descriptions);
        assert!(instr.contains("/tmp/tool-proxy/test"));
        assert!(instr.contains("call_tool"));
        assert!(instr.contains("## Connected Servers"));
        assert!(instr.contains("**math**"));
        assert!(instr.contains("**git**"));
    }

    #[tokio::test]
    async fn write_tool_entries_to_dir_removes_stale_files() {
        let tmp = tempfile::tempdir().unwrap();
        let tool_dir = tmp.path().to_path_buf();
        let server_dir = tool_dir.join("my-server");
        std::fs::create_dir_all(&server_dir).unwrap();

        let old_entry = ToolFileEntry {
            name: "old_tool".to_string(),
            description: "Old tool".to_string(),
            server: "my-server".to_string(),
            parameters: json!({"type": "object", "properties": {}}),
            annotations: None,
        };
        std::fs::write(server_dir.join("old_tool.json"), serde_json::to_string_pretty(&old_entry).unwrap()).unwrap();
        assert!(server_dir.join("old_tool.json").exists());

        let tools: Vec<Tool> = vec![Tool::new("new_tool", "New tool", Arc::new(serde_json::Map::new()))];
        write_tool_entries_to_dir("my-server", &tools, &ToolExposure::proxied_all(), &tool_dir).await.unwrap();

        assert!(!server_dir.join("old_tool.json").exists(), "stale file should be removed");
        assert!(server_dir.join("new_tool.json").exists(), "new file should be written");
    }

    #[tokio::test]
    async fn write_tool_entries_to_dir_omits_direct_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = [
            Tool::new("bash", "Shell", Arc::new(serde_json::Map::new())),
            Tool::new("lsp_hover", "Hover", Arc::new(serde_json::Map::new())),
            Tool::new("read_file", "Read", Arc::new(serde_json::Map::new())),
        ];
        let exposure = ToolExposure::Proxied(ToolProxyRules::new(&[], &["bash", "lsp_*"]));
        write_tool_entries_to_dir("coding", &tools, &exposure, tmp.path()).await.unwrap();

        let server_dir = tmp.path().join("coding");
        assert!(!server_dir.join("bash.json").exists());
        assert!(!server_dir.join("lsp_hover.json").exists());
        assert!(server_dir.join("read_file.json").exists());
    }

    #[tokio::test]
    async fn write_tool_entries_to_dir_preserves_annotations() {
        let tmp = tempfile::tempdir().unwrap();
        let tool_dir = tmp.path().to_path_buf();
        let tools = [Tool::new("read", "Read", Arc::new(serde_json::Map::new()))
            .with_annotations(ToolAnnotations::new().read_only(true).open_world(false))];

        write_tool_entries_to_dir("my-server", &tools, &ToolExposure::proxied_all(), &tool_dir).await.unwrap();

        let contents = std::fs::read_to_string(tool_dir.join("my-server/read.json")).unwrap();
        let parsed: ToolFileEntry = serde_json::from_str(&contents).unwrap();
        let annotations = parsed.annotations.expect("annotations should be written");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
    }

    #[test]
    fn tool_proxy_resolve_call_success() {
        let json = r#"{"server":"math","tool":"add","arguments":{"a":1,"b":2}}"#;
        let call = resolve_call(json).unwrap();
        assert_eq!(call.server, "math");
        assert_eq!(call.tool, "add");
        assert!(call.arguments.is_some());
        assert_eq!(call.arguments.unwrap().get("a").unwrap(), 1);
    }
}
