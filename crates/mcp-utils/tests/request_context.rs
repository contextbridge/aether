use mcp_utils::{
    request_context::{AgentIdentity, GatewayRequestContext, RequestContextError, TOOL_CONTEXT_KEY},
    tool_exposure::{DeferredToolRules, ToolExposure},
    tool_policy::{ToolFilter, ToolMatcher},
};
use rmcp::model::{MetaObject, Tool};
use serde_json::json;

#[test]
fn context_roundtrip_preserves_unrelated_metadata_and_replaces_reserved_policy() {
    let context = context();
    let mut meta = MetaObject::default();
    meta.insert("traceparent".into(), json!("trace"));
    meta.insert("protocolVersion".into(), json!("2026-07-28"));
    meta.insert(TOOL_CONTEXT_KEY.into(), json!({"untrusted": true}));
    context.merge_into(&mut meta).unwrap();
    assert_eq!(GatewayRequestContext::from_meta(Some(&meta)).unwrap(), context);
    assert_eq!(GatewayRequestContext::from_json(&serde_json::to_string(&context).unwrap()).unwrap(), context);
    assert_eq!(meta["traceparent"], "trace");
    assert_eq!(meta["protocolVersion"], "2026-07-28");
    assert_ne!(AgentIdentity::new(), context.identity);
}

#[test]
fn context_fails_closed_for_missing_malformed_or_invalid_identity() {
    assert_eq!(GatewayRequestContext::from_meta(None), Err(RequestContextError::Missing));
    for value in [json!({}), json!({"identity": {"runtime_id": "bad"}}), json!(null)] {
        let mut meta = MetaObject::default();
        meta.insert(TOOL_CONTEXT_KEY.into(), value);
        assert_eq!(GatewayRequestContext::from_meta(Some(&meta)), Err(RequestContextError::Invalid));
    }
    let mut invalid = context();
    invalid.identity.runtime_id = uuid::Uuid::nil();
    assert!(invalid.validate().is_err());
    invalid = context();
    invalid.server_alias = "vm__tools".into();
    assert!(invalid.validate().is_err());
}

#[test]
fn filters_intersect_across_all_three_naming_domains() {
    let mut context = context();
    context.agent_tools.allow = vec![ToolMatcher::name("vm_tools__coding__read_*")];
    context.server_tools.allow = vec![ToolMatcher::name("coding__*")];
    let deployment =
        ToolFilter { allow: vec![ToolMatcher::name("read_*")], deny: vec![ToolMatcher::name("read_secret")] };
    assert!(context.allows("coding", &deployment, &tool("read_file", None)));
    assert!(!context.allows("coding", &deployment, &tool("read_secret", None)));
    assert!(!context.allows("coding", &deployment, &tool("edit_file", None)));
    assert!(!context.allows("other", &deployment, &tool("read_file", None)));
    context.server_alias = "other_vm".into();
    assert!(!context.allows("coding", &deployment, &tool("read_file", None)));
    context = self::context();
    context.agent_tools.allow = vec![ToolMatcher::name("vm_tools__coding__edit_file")];
    context.server_tools.allow = vec![ToolMatcher::name("coding__read_file")];
    for name in ["read_file", "edit_file"] {
        assert!(!context.allows("coding", &ToolFilter::default(), &tool(name, None)));
    }
}

#[test]
fn annotations_are_or_matchers_and_missing_annotations_do_not_match() {
    let mut context = context();
    context.server_tools.allow = vec![ToolMatcher::read_only(), ToolMatcher::name("coding__*")];
    let deployment = ToolFilter::default();
    assert!(context.allows("coding", &deployment, &tool("edit", None)));
    assert!(context.allows("linear", &deployment, &tool("read", Some(true))));
    assert!(!context.allows("linear", &deployment, &tool("read", None)));
    context.server_tools.deny = vec![ToolMatcher::name("coding__edit")];
    assert!(!context.allows("coding", &deployment, &tool("edit", Some(true))));
}

#[test]
fn cli_visibility_requires_allowance_and_deferral_and_preserves_nested_names() {
    let mut context = context();
    let deployment = ToolFilter::default();
    let tool = tool("issues__get", None);
    assert!(context.allows("linear", &deployment, &tool));
    assert!(!context.cli_visible("linear", &deployment, &tool));
    context.defer_tools = ToolExposure::Deferred(DeferredToolRules::new(&["linear__*"], &[]));
    assert!(context.cli_visible("linear", &deployment, &tool));
    context.server_tools.deny = vec![ToolMatcher::name("linear__issues__get")];
    assert!(!context.cli_visible("linear", &deployment, &tool));
    assert!(!context.allows("invalid__alias", &deployment, &tool));
}

fn context() -> GatewayRequestContext {
    GatewayRequestContext {
        identity: AgentIdentity::new(),
        execution_task: None,
        server_alias: "vm_tools".into(),
        agent_tools: ToolFilter::default(),
        server_tools: ToolFilter::default(),
        defer_tools: ToolExposure::default(),
    }
}

fn tool(name: &str, read_only: Option<bool>) -> Tool {
    let mut value = json!({"name": name, "inputSchema": {"type": "object"}});
    if let Some(read_only) = read_only {
        value["annotations"] = json!({"readOnlyHint": read_only});
    }
    serde_json::from_value(value).unwrap()
}
