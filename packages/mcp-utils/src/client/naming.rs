pub const SERVERNAME_DELIMITER: &str = "__";

pub(crate) fn create_namespaced_tool_name(server_name: &str, tool_name: &str) -> String {
    format!("{server_name}{SERVERNAME_DELIMITER}{tool_name}")
}

pub fn split_on_server_name(namespaced_name: &str) -> Option<(&str, &str)> {
    namespaced_name.split_once(SERVERNAME_DELIMITER)
}
