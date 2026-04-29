use mcp_utils::client::RawMcpServerConfig;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub enum McpSourceSpec {
    File { path: String, proxy: bool },
    Inline { servers: BTreeMap<String, RawMcpServerConfig> },
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum McpSourceSpecInput {
    Path(String),
    Object(McpSourceSpecObject),
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum McpSourceSpecObject {
    File {
        path: String,
        #[serde(default)]
        proxy: bool,
    },
    Inline {
        servers: BTreeMap<String, RawMcpServerConfig>,
    },
}

impl<'de> Deserialize<'de> for McpSourceSpec {
    fn deserialize<T: Deserializer<'de>>(deserializer: T) -> Result<Self, T::Error> {
        match Deserialize::deserialize(deserializer)? {
            McpSourceSpecInput::Path(path) => Ok(Self::File { path, proxy: false }),
            McpSourceSpecInput::Object(McpSourceSpecObject::File { path, proxy }) => Ok(Self::File { path, proxy }),
            McpSourceSpecInput::Object(McpSourceSpecObject::Inline { servers }) => Ok(Self::Inline { servers }),
        }
    }
}

impl Serialize for McpSourceSpec {
    fn serialize<T: Serializer>(&self, serializer: T) -> Result<T::Ok, T::Error> {
        match self {
            Self::File { path, proxy: false } => serializer.serialize_str(path),
            Self::File { path, proxy } => {
                Serialize::serialize(&McpSourceSpecObject::File { path: path.clone(), proxy: *proxy }, serializer)
            }
            Self::Inline { servers } => {
                Serialize::serialize(&McpSourceSpecObject::Inline { servers: servers.clone() }, serializer)
            }
        }
    }
}

impl schemars::JsonSchema for McpSourceSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "McpSourceSpec".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let object_schema = generator.subschema_for::<McpSourceSpecObject>().to_value();
        schemars::Schema::try_from(serde_json::json!({
            "description": "MCP config source — either a file path string or a typed file or inline object.",
            "oneOf": [
                { "type": "string" },
                object_schema
            ]
        }))
        .expect("mcp source schema must be valid")
    }
}

impl McpSourceSpec {
    pub fn file(path: impl Into<String>) -> Self {
        Self::File { path: path.into(), proxy: false }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File { path, .. } => Some(path.as_str()),
            Self::Inline { .. } => None,
        }
    }
}
