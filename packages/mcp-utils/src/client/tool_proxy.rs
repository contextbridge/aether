use super::McpError;
use super::mcp_client::McpClient;
use llm::ToolDefinition;
use rmcp::{RoleClient, service::RunningService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{create_dir_all, remove_dir_all, write};

/// Resolved proxy call returned by [`ToolProxy::resolve_call`].
#[derive(Debug)]
pub struct ResolvedCall {
    pub server: String,
    pub tool: String,
    pub arguments: Option<Map<String, Value>>,
}

/// A tool-proxy that wraps multiple servers behind a single `call_tool`.
pub struct ToolProxy {
    name: String,
    members: HashSet<String>,
    tool_dir: PathBuf,
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

impl ToolProxy {
    pub fn new(name: String, members: HashSet<String>, tool_dir: PathBuf) -> Self {
        Self { name, members, tool_dir }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn members(&self) -> &HashSet<String> {
        &self.members
    }

    /// Whether `server_name` is a nested member of this proxy.
    pub fn contains_server(&self, server_name: &str) -> bool {
        self.members.contains(server_name)
    }

    /// Parse and validate a proxy `call_tool` invocation.
    pub fn resolve_call(&self, arguments_json: &str) -> super::Result<ResolvedCall> {
        let args: ProxyCallArgs = serde_json::from_str(arguments_json)?;
        if !self.contains_server(&args.server) {
            return Err(McpError::ServerNotFound(format!(
                "Server '{}' is not part of proxy '{}'",
                args.server, self.name
            )));
        }
        Ok(ResolvedCall { server: args.server, tool: args.tool, arguments: args.arguments })
    }

    pub fn tool_dir(&self) -> &Path {
        &self.tool_dir
    }

    /// Register a new member server (e.g. after late OAuth registration).
    pub fn add_member(&mut self, server_name: String) {
        self.members.insert(server_name);
    }

    /// Returns the directory for a tool-proxy's tool definitions.
    ///
    /// Uses `$AETHER_HOME/tool-proxy/<name>` or `~/.aether/tool-proxy/<name>`.
    pub fn dir(name: &str) -> Result<PathBuf, McpError> {
        let base = super::aether_home().ok_or_else(|| McpError::Other("Home directory not set".into()))?;
        Ok(Self::dir_in_home(&base, name))
    }

    pub fn dir_in_home(home: &Path, name: &str) -> PathBuf {
        home.join("tool-proxy").join(name)
    }

    /// Clean up the tool directory for a proxy, removing all tool files.
    pub async fn clean_dir(tool_dir: &Path) -> Result<(), McpError> {
        if tool_dir.exists() {
            remove_dir_all(tool_dir)
                .await
                .map_err(|e| McpError::Other(format!("Failed to clean tool-proxy dir: {e}")))?;
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
    pub fn call_tool_definition(proxy_name: &str) -> ToolDefinition {
        let schema = Self::call_tool_schema();
        let namespaced_name = format!("{proxy_name}__call_tool");
        ToolDefinition {
            name: namespaced_name,
            description: "Execute a tool on a nested MCP server. Browse the tool-proxy directory to discover available tools first.".to_string(),
            parameters: Value::Object((*schema).clone()).to_string(),
            server: Some(proxy_name.to_string()),
        }
    }

    /// Discover tools from a connected MCP server and write them as JSON files
    /// to `tool_dir/<server_name>/`, replacing any existing files for that server.
    pub async fn write_tools_to_dir(
        server_name: &str,
        client: &RunningService<RoleClient, McpClient>,
        tool_dir: &Path,
    ) -> Result<(), McpError> {
        let tools_response = client.list_tools(None).await.map_err(|e| {
            McpError::ToolDiscoveryFailed(format!("Failed to list tools for nested server '{server_name}': {e}"))
        })?;

        Self::write_tool_entries_to_dir(server_name, &tools_response.tools, tool_dir).await
    }

    /// Write tool entries to `tool_dir/<server_name>/`, removing any stale files first.
    pub(super) async fn write_tool_entries_to_dir(
        server_name: &str,
        tools: &[rmcp::model::Tool],
        tool_dir: &Path,
    ) -> Result<(), McpError> {
        let server_dir = tool_dir.join(server_name);
        if server_dir.exists() {
            remove_dir_all(&server_dir).await?;
        }
        create_dir_all(&server_dir).await?;

        for tool in tools {
            let entry = ToolFileEntry {
                name: tool.name.to_string(),
                description: tool.description.clone().unwrap_or_default().to_string(),
                server: server_name.to_string(),
                parameters: Value::Object((*tool.input_schema).clone()),
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
            .and_then(|info| info.server_info.description.as_deref().filter(|s| !s.is_empty()).map(ToString::to_string))
            .unwrap_or_else(|| server_name.to_string())
    }

    /// Build proxy instructions describing the tool directory and connected servers.
    pub fn build_instructions(tool_dir: &Path, server_descriptions: &[(String, String)]) -> String {
        use std::fmt::Write;

        let mut instructions = format!(
            "You are connected to a set of MCP servers, whose tools are available at `{tool_dir}`.\n\
             Each subdirectory in `{tool_dir}` represents a MCP server you're connected. And each subdir contains tool definitions in the form of JSON files.\n\
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
}

/// A tool definition written to disk for agent browsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFileEntry {
    pub name: String,
    pub description: String,
    pub server: String,
    pub parameters: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
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
        };

        let json_str = serde_json::to_string_pretty(&entry).unwrap();
        let deserialized: ToolFileEntry = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.name, "create_issue");
        assert_eq!(deserialized.server, "github");
        assert_eq!(deserialized.description, "Create a GitHub issue");
    }

    #[test]
    fn call_tool_schema_is_valid() {
        let schema = ToolProxy::call_tool_schema();
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
        let dir = ToolProxy::dir("proxy").unwrap();
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
    fn call_tool_definition_has_correct_name_and_server() {
        let def = ToolProxy::call_tool_definition("myproxy");
        assert_eq!(def.name, "myproxy__call_tool");
        assert_eq!(def.server, Some("myproxy".to_string()));
        assert!(def.description.contains("Execute a tool"));
    }

    #[test]
    fn build_proxy_instructions_includes_tool_dir_and_servers() {
        let tool_dir = std::path::Path::new("/tmp/tool-proxy/test");
        let descriptions =
            vec![("math".to_string(), "Math tools".to_string()), ("git".to_string(), "Git tools".to_string())];
        let instr = ToolProxy::build_instructions(tool_dir, &descriptions);
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
        };
        std::fs::write(server_dir.join("old_tool.json"), serde_json::to_string_pretty(&old_entry).unwrap()).unwrap();
        assert!(server_dir.join("old_tool.json").exists());

        let tools: Vec<rmcp::model::Tool> =
            vec![rmcp::model::Tool::new("new_tool", "New tool", Arc::new(serde_json::Map::new()))];
        ToolProxy::write_tool_entries_to_dir("my-server", &tools, &tool_dir).await.unwrap();

        assert!(!server_dir.join("old_tool.json").exists(), "stale file should be removed");
        assert!(server_dir.join("new_tool.json").exists(), "new file should be written");
    }

    fn make_proxy(members: &[&str]) -> ToolProxy {
        let members: HashSet<String> = members.iter().map(std::string::ToString::to_string).collect();
        ToolProxy::new("myproxy".to_string(), members, PathBuf::from("/tmp/tool-proxy/myproxy"))
    }

    #[test]
    fn tool_proxy_contains_server() {
        let proxy = make_proxy(&["math", "git"]);
        assert!(proxy.contains_server("math"));
        assert!(proxy.contains_server("git"));
        assert!(!proxy.contains_server("unknown"));
    }

    #[test]
    fn tool_proxy_resolve_call_success() {
        let proxy = make_proxy(&["math"]);
        let json = r#"{"server":"math","tool":"add","arguments":{"a":1,"b":2}}"#;
        let call = proxy.resolve_call(json).unwrap();
        assert_eq!(call.server, "math");
        assert_eq!(call.tool, "add");
        assert!(call.arguments.is_some());
        assert_eq!(call.arguments.unwrap().get("a").unwrap(), 1);
    }

    #[test]
    fn tool_proxy_resolve_call_unknown_server() {
        let proxy = make_proxy(&["math"]);
        let json = r#"{"server":"unknown","tool":"add","arguments":{}}"#;
        let err = proxy.resolve_call(json).unwrap_err();
        assert!(err.to_string().contains("not part of proxy"));
    }

    #[test]
    fn tool_proxy_accessors() {
        let proxy = make_proxy(&["math"]);
        assert_eq!(proxy.name(), "myproxy");
        assert_eq!(proxy.tool_dir(), Path::new("/tmp/tool-proxy/myproxy"));
    }

    #[test]
    fn tool_proxy_add_member() {
        let mut proxy = make_proxy(&["math"]);
        assert!(!proxy.contains_server("git"));
        proxy.add_member("git".to_string());
        assert!(proxy.contains_server("git"));
    }
}
