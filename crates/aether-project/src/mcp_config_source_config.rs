use mcp_utils::client::McpServerConfig;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use utils::{PathOrObject, ResourcePath, is_false, string_or_object_schema};

#[derive(Debug, Clone, PartialEq)]
pub enum McpSourceSpec {
    File(McpFileSpec),
    Inline { servers: BTreeMap<String, McpServerConfig> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpFileSpec {
    pub path: ResourcePath,
    pub defer_tools: bool,
    pub optional: bool,
}

impl McpFileSpec {
    pub fn new(path: impl Into<ResourcePath>) -> Self {
        Self { path: path.into(), defer_tools: false, optional: false }
    }

    #[must_use]
    pub fn defer_tools(mut self) -> Self {
        self.defer_tools = true;
        self
    }

    #[must_use]
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

impl McpSourceSpec {
    pub fn file(path: impl Into<ResourcePath>) -> Self {
        Self::File(McpFileSpec::new(path))
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File(file) => Some(file.path.as_authored()),
            Self::Inline { .. } => None,
        }
    }
}

impl From<McpFileSpec> for McpSourceSpec {
    fn from(spec: McpFileSpec) -> Self {
        Self::File(spec)
    }
}

impl<'de> Deserialize<'de> for McpSourceSpec {
    fn deserialize<T: Deserializer<'de>>(deserializer: T) -> Result<Self, T::Error> {
        Ok(match PathOrObject::<McpSourceSpecObject>::deserialize(deserializer)? {
            PathOrObject::Path(path) => Self::File(McpFileSpec::new(path)),
            PathOrObject::Object(McpSourceSpecObject::File { path, defer_tools, optional }) => {
                Self::File(McpFileSpec { path, defer_tools, optional })
            }
            PathOrObject::Object(McpSourceSpecObject::Inline { servers }) => Self::Inline { servers },
        })
    }
}

impl Serialize for McpSourceSpec {
    fn serialize<T: Serializer>(&self, serializer: T) -> Result<T::Ok, T::Error> {
        match self {
            Self::File(McpFileSpec { path, defer_tools: false, optional: false }) => path.serialize(serializer),
            Self::File(McpFileSpec { path, defer_tools, optional }) => Serialize::serialize(
                &McpSourceSpecObject::File { path: path.clone(), defer_tools: *defer_tools, optional: *optional },
                serializer,
            ),
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
        string_or_object_schema(
            include_str!("docs/mcp_source_spec.md"),
            &generator.subschema_for::<McpSourceSpecObject>().to_value(),
        )
    }
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum McpSourceSpecObject {
    File {
        path: ResourcePath,
        #[serde(rename = "deferTools", default, skip_serializing_if = "is_false")]
        defer_tools: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        optional: bool,
    },
    Inline {
        servers: BTreeMap<String, McpServerConfig>,
    },
}
