//! Ensures the JSON examples embedded in config doc comments stay valid.
//! These `.md` files are rendered into the auto-generated settings reference.

use aether_doctest::assert_json_examples;
use mcp_utils::client::McpServerConfig;

#[test]
fn mcp_server_config_examples_parse() {
    assert_json_examples::<McpServerConfig>("mcp_server_config.md", include_str!("../src/docs/mcp_server_config.md"));
}
