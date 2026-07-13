use llm::ToolDefinition;
use serde_json::json;

#[test]
fn tool_definition_serializes_parameters_as_structured_json() {
    let schema = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" }
        },
        "required": ["path"]
    });
    let tool = ToolDefinition::new("read_file", "Read a file", schema.clone());

    let serialized = serde_json::to_value(tool).expect("tool definition serializes");

    assert_eq!(serialized["parameters"], schema);
    assert!(serialized["parameters"].is_object());
}
