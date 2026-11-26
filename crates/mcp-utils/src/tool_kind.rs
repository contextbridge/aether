pub use agent_client_protocol::v2::schema::ToolKind;
use rmcp::model::{MetaObject, Tool};

pub fn tool_kind_meta(kind: ToolKind) -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert(TOOL_KIND_KEY.into(), serde_json::to_value(kind).expect("ACP tool kinds serialize to JSON"));
    meta
}

pub fn declared_tool_kind(tool: &Tool) -> Option<ToolKind> {
    let value = tool.meta.as_ref()?.get(TOOL_KIND_KEY)?.as_str()?;
    let kind = serde_json::from_value(value.into()).ok()?;
    (!matches!(kind, ToolKind::Unknown(_))).then_some(kind)
}

const TOOL_KIND_KEY: &str = "aether-agent.io/tool-kind";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_unrecognized_tool_kinds() {
        let mut meta = MetaObject::new();
        meta.insert("aether-agent.io/tool-kind".into(), "not_a_kind".into());
        let tool = Tool::default().with_meta(meta);

        assert_eq!(declared_tool_kind(&tool), None);
    }
}
