use schemars::Schema;
use schemars::transform::{RemoveRefSiblings, Transform, transform_subschemas};
use serde_json::{Map, Value, json};

pub fn normalize_for_moonshot(schema: &mut Schema) {
    RemoveRefSiblings::default().transform(schema);
    MoonshotMfjsTransformer.transform(schema);
}

/// Drops the `null` alternative from properties that are not `required`.
///
/// Optional properties can already be omitted, so the null alternative is redundant.
/// Xiaomi `MiMo`'s server-side tool-call parser corrupts streamed tool calls and leaks
/// them into text when a parameter's type is a union.
pub fn normalize_for_xiaomi(schema: &mut Schema) {
    OptionalNullStripper.transform(schema);
}

struct OptionalNullStripper;

impl Transform for OptionalNullStripper {
    fn transform(&mut self, schema: &mut Schema) {
        if let Some(obj) = schema.as_object_mut() {
            let required = obj.get("required").and_then(Value::as_array).cloned().unwrap_or_default();
            if let Some(Value::Object(properties)) = obj.get_mut("properties") {
                properties
                    .iter_mut()
                    .filter(|(name, _)| !required.iter().any(|r| *r == name.as_str()))
                    .filter_map(|(_, property)| property.as_object_mut())
                    .for_each(remove_null_alternative);
            }
        }

        transform_subschemas(self, schema);
    }
}

struct MoonshotMfjsTransformer;

impl Transform for MoonshotMfjsTransformer {
    fn transform(&mut self, schema: &mut Schema) {
        if let Some(obj) = schema.as_object_mut() {
            obj.remove("$schema");
            obj.remove("title");
            obj.remove("format");
            obj.remove("$comment");

            if matches!(obj.get("type"), Some(Value::Array(_))) {
                let Some(Value::Array(types)) = obj.remove("type") else { unreachable!() };
                let any_of: Vec<Value> = types.into_iter().map(|t| json!({ "type": t })).collect();
                obj.insert("anyOf".to_string(), Value::Array(any_of));
            }
        }

        transform_subschemas(self, schema);
    }
}

fn remove_null_alternative(property: &mut Map<String, Value>) {
    if let Some(Value::Array(types)) = property.get_mut("type")
        && types.len() > 1
    {
        types.retain(|t| t != "null");
        if types.len() == 1 {
            let only = types.remove(0);
            property.insert("type".to_string(), only);
        }
    }

    if let Some(Value::Array(variants)) = property.get_mut("anyOf")
        && variants.len() > 1
    {
        variants.retain(|variant| *variant != json!({ "type": "null" }));
        if variants.len() == 1
            && let Some(Value::Array(mut variants)) = property.remove("anyOf")
            && let Value::Object(only) = variants.remove(0)
        {
            for (key, value) in only {
                property.entry(key).or_insert(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_from_value(value: Value) -> Schema {
        Schema::try_from(value).unwrap()
    }

    fn schema_to_value(schema: &Schema) -> Value {
        Value::from(schema.clone())
    }

    #[test]
    fn normalize_for_xiaomi_drops_null_from_optional_property_types() {
        let mut schema = schema_from_value(json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "description": {"type": ["string", "null"], "description": "What it does"},
                "value": {"type": ["string", "number", "null"]},
                "cleared": {"type": ["string", "null"]}
            },
            "required": ["command", "cleared"]
        }));
        normalize_for_xiaomi(&mut schema);
        assert_eq!(
            schema_to_value(&schema),
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "description": {"type": "string", "description": "What it does"},
                    "value": {"type": ["string", "number"]},
                    "cleared": {"type": ["string", "null"]}
                },
                "required": ["command", "cleared"]
            })
        );
    }

    #[test]
    fn normalize_for_xiaomi_recurses_into_nested_objects() {
        let mut schema = schema_from_value(json!({
            "type": "object",
            "properties": {
                "edits": {"type": "array", "items": {"$ref": "#/$defs/Edit"}}
            },
            "$defs": {
                "Edit": {
                    "type": "object",
                    "properties": {"replace_all": {"type": ["boolean", "null"]}}
                }
            }
        }));
        normalize_for_xiaomi(&mut schema);
        assert_eq!(schema_to_value(&schema)["$defs"]["Edit"]["properties"]["replace_all"], json!({"type": "boolean"}));
    }

    #[test]
    fn normalize_for_xiaomi_unwraps_optional_any_of_null() {
        let mut schema = schema_from_value(json!({
            "type": "object",
            "properties": {
                "edit": {"description": "An edit", "anyOf": [{"$ref": "#/$defs/Edit"}, {"type": "null"}]},
                "mode": {"anyOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}]},
                "target": {"anyOf": [{"$ref": "#/$defs/Edit"}, {"type": "null"}]}
            },
            "required": ["target"]
        }));
        normalize_for_xiaomi(&mut schema);
        let properties = &schema_to_value(&schema)["properties"];
        assert_eq!(properties["edit"], json!({"description": "An edit", "$ref": "#/$defs/Edit"}));
        assert_eq!(properties["mode"], json!({"anyOf": [{"type": "string"}, {"type": "integer"}]}));
        assert_eq!(properties["target"], json!({"anyOf": [{"$ref": "#/$defs/Edit"}, {"type": "null"}]}));
    }

    #[test]
    fn sanitize_removes_dollar_schema() {
        let mut schema = schema_from_value(json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "string"
        }));
        MoonshotMfjsTransformer.transform(&mut schema);
        assert_eq!(schema_to_value(&schema), json!({"type": "string"}));
    }

    #[test]
    fn sanitize_removes_title_format_comment() {
        let mut schema = schema_from_value(json!({
            "title": "MySchema",
            "type": "string",
            "format": "uri",
            "$comment": "internal note"
        }));
        MoonshotMfjsTransformer.transform(&mut schema);
        assert_eq!(schema_to_value(&schema), json!({"type": "string"}));
    }

    #[test]
    fn sanitize_preserves_description() {
        let mut schema = schema_from_value(json!({
            "type": "string",
            "description": "A useful field"
        }));
        MoonshotMfjsTransformer.transform(&mut schema);
        assert_eq!(schema_to_value(&schema), json!({"type": "string", "description": "A useful field"}));
    }

    #[test]
    fn sanitize_recurses_into_properties() {
        let mut schema = schema_from_value(json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "title": "Name",
                    "format": "email"
                }
            }
        }));
        MoonshotMfjsTransformer.transform(&mut schema);
        assert_eq!(
            schema_to_value(&schema),
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                }
            })
        );
    }

    #[test]
    fn sanitize_rewrites_nullable_type_array_to_anyof() {
        let mut schema = schema_from_value(json!({"type": ["string", "null"]}));
        MoonshotMfjsTransformer.transform(&mut schema);
        assert_eq!(schema_to_value(&schema), json!({"anyOf": [{"type": "string"}, {"type": "null"}]}));
    }

    #[test]
    fn sanitize_preserves_scalar_type() {
        let mut schema = schema_from_value(json!({"type": "string"}));
        MoonshotMfjsTransformer.transform(&mut schema);
        assert_eq!(schema_to_value(&schema), json!({"type": "string"}));
    }

    #[test]
    fn sanitize_rewrites_type_array_recursively() {
        let mut schema = schema_from_value(json!({
            "$defs": {
                "NullableString": {"type": ["string", "null"]}
            },
            "type": "object"
        }));
        MoonshotMfjsTransformer.transform(&mut schema);
        assert_eq!(
            schema_to_value(&schema),
            json!({
                "$defs": {
                    "NullableString": {
                        "anyOf": [{"type": "string"}, {"type": "null"}]
                    }
                },
                "type": "object"
            })
        );
    }

    #[test]
    fn full_pipeline_lsp_check_errors_schema() {
        let input = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "LspDiagnosticsInput",
            "$defs": {
                "LspDiagnosticsInput": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "scope": {"type": "string", "const": "workspace"}
                            },
                            "required": ["scope"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "scope": {"type": "string", "const": "file"},
                                "filePath": {"type": "string", "description": "Absolute path"}
                            },
                            "required": ["scope", "filePath"]
                        }
                    ]
                }
            },
            "type": "object",
            "properties": {
                "input": {
                    "$ref": "#/$defs/LspDiagnosticsInput",
                    "description": "Wrapped discriminated union request"
                }
            },
            "required": ["input"]
        });

        let mut schema = schema_from_value(input);
        normalize_for_moonshot(&mut schema);
        let result = schema_to_value(&schema);

        let result_str = serde_json::to_string(&result).unwrap();
        assert!(!result_str.contains("\"$schema\""));
        assert!(!result_str.contains("\"title\""));
        assert!(!result_str.contains("\"format\""));

        let input_prop = result.pointer("/properties/input").unwrap().as_object().unwrap();
        assert!(input_prop.contains_key("allOf"), "$ref should be wrapped in allOf: {input_prop:?}");
        assert!(!input_prop.contains_key("$ref"), "$ref should have been moved: {input_prop:?}");
        assert_eq!(result["type"], "object");
    }

    #[test]
    fn full_pipeline_ref_with_no_siblings_untouched() {
        let mut schema = schema_from_value(json!({
            "$defs": {"Foo": {"type": "string"}},
            "type": "object",
            "properties": {"value": {"$ref": "#/$defs/Foo"}}
        }));
        normalize_for_moonshot(&mut schema);
        let result = schema_to_value(&schema);
        assert_eq!(result.pointer("/properties/value/$ref").unwrap(), "#/$defs/Foo");
    }

    #[test]
    fn full_pipeline_nullable_in_property() {
        let mut schema = schema_from_value(json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": ["string", "null"],
                    "description": "Optional name",
                    "format": "uri"
                }
            }
        }));
        normalize_for_moonshot(&mut schema);
        let result = schema_to_value(&schema);

        let name_prop = result.pointer("/properties/name").unwrap();
        assert!(name_prop.get("anyOf").is_some(), "Should have anyOf: {name_prop}");
        assert!(name_prop.get("type").is_none(), "type array should be removed: {name_prop}");
        assert!(name_prop.get("format").is_none(), "format should be removed: {name_prop}");
        assert!(name_prop.get("description").is_some(), "description should be preserved: {name_prop}");
    }

    #[test]
    fn full_pipeline_ref_with_sibling_description() {
        let mut schema = schema_from_value(json!({
            "$defs": {"Foo": {"type": "string"}},
            "type": "object",
            "properties": {
                "value": {"$ref": "#/$defs/Foo", "description": "A foo value"}
            }
        }));
        normalize_for_moonshot(&mut schema);
        let result = schema_to_value(&schema);

        let value_prop = result.pointer("/properties/value").unwrap().as_object().unwrap();
        assert!(value_prop.contains_key("allOf"));
        assert!(value_prop.contains_key("description"));
        assert!(!value_prop.contains_key("$ref"));
    }
}
