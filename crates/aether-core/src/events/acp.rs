use acp_utils::AETHER_TOOL_NAME_META_KEY;

pub fn aether_tool_name_meta(name: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert(AETHER_TOOL_NAME_META_KEY.to_string(), name.to_string().into());
    meta
}

pub fn parse_tool_call_chunk(chunk: &str) -> serde_json::Value {
    serde_json::from_str(chunk).unwrap_or_else(|_| serde_json::Value::String(chunk.to_string()))
}

pub fn mcp_tool_name(namespaced_name: &str) -> &str {
    namespaced_name.split("__").last().unwrap_or(namespaced_name)
}

pub fn humanize_tool_name(name: &str) -> String {
    let mut result = mcp_tool_name(name).replace('_', " ");
    if let Some(first) = result.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_tool_names() {
        assert_eq!(humanize_tool_name("coding__read_file"), "Read file");
        assert_eq!(humanize_tool_name("read_file"), "Read file");
        assert_eq!(humanize_tool_name("bash"), "Bash");
        assert_eq!(humanize_tool_name("plugins__coding__read_file"), "Read file");
    }

    #[test]
    fn strips_server_prefixes_from_tool_names() {
        assert_eq!(mcp_tool_name("subagents__spawn_subagent"), "spawn_subagent");
        assert_eq!(mcp_tool_name("plugins__coding__read_file"), "read_file");
        assert_eq!(mcp_tool_name("bash"), "bash");
    }
}
