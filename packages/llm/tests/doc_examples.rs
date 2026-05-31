//! Ensures the JSON examples embedded in config doc comments stay valid.
//! These `.md` files are rendered into the auto-generated settings reference.

use aether_doctest::assert_json_examples;
use llm::ProviderConnectionOverride;

#[test]
fn provider_connection_override_examples_parse() {
    assert_json_examples::<ProviderConnectionOverride>(
        "provider_connection_override.md",
        include_str!("../src/docs/provider_connection_override.md"),
    );
}
