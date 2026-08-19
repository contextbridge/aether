use mcp_utils::client::{
    CatalogTool, DeferredToolRules, PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME, ServerCatalogEntry, ToolAnnotationMatcher,
    ToolCatalog, ToolExposure, ToolExposureKind, ToolFilter, ToolMatcher, ToolRoute,
};
use mcp_utils::status::{McpServerAuthCapability, McpServerStatus};
use rmcp::model::{Tool, ToolAnnotations};
use serde_json::json;
use std::sync::Arc;

fn tool(name: &str) -> Tool {
    let schema = json!({
        "type": "object",
        "properties": { "value": { "type": "string" } },
        "required": ["value"]
    });
    Tool::new(name.to_string(), format!("{name} description"), Arc::new(schema.as_object().unwrap().clone()))
}

fn connected_entry(name: &str, exposure: ToolExposure, tools: &[Tool], filter: &ToolFilter) -> ServerCatalogEntry {
    ServerCatalogEntry::new(
        name,
        format!("{name} server"),
        Some(format!("{name} instructions")),
        McpServerStatus::Connected { tool_count: tools.len() },
        McpServerAuthCapability::OAuth,
        exposure,
        tools,
        filter,
    )
}

#[test]
fn catalog_projects_visibility_filtering_instructions_and_routes_consistently() {
    assert_eq!(PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME, "progressive-discovery");
    let tools = vec![
        tool("bash"),
        tool("read").with_annotations(ToolAnnotations::new().read_only(true).open_world(false)),
        tool("write"),
    ];
    let filter = ToolFilter {
        allow: vec![ToolMatcher::name("coding__*")],
        deny: vec![ToolMatcher::annotations(ToolAnnotationMatcher {
            read_only: Some(true),
            ..ToolAnnotationMatcher::default()
        })],
    };
    let exposure = ToolExposure::Deferred(DeferredToolRules::new(&["read", "write"], &["bash"]));
    let mut catalog = ToolCatalog::new();
    catalog.upsert_server(connected_entry("coding", exposure, &tools, &filter));
    catalog.set_progressive_discovery_instructions(Some("Discover deferred tools".to_string()));

    let tools = catalog.tools();
    assert_eq!(
        tools.model_visible.iter().map(|tool| tool.definition().name.as_str()).collect::<Vec<_>>(),
        ["coding__bash"]
    );
    assert_eq!(tools.deferred.into_iter().map(CatalogTool::local_name).collect::<Vec<_>>(), ["write"]);
    let server_tools = catalog.tools_for("coding").unwrap();
    assert_eq!(server_tools.model_visible.len(), 1);
    assert_eq!(server_tools.deferred.len(), 1);
    assert!(catalog.tools_for("missing").is_none());
    assert_eq!(catalog.discoverable_deferred_servers()[0].name, "coding");
    assert_eq!(catalog.model_instructions().get("coding").map(String::as_str), Some("coding instructions"));
    assert_eq!(
        catalog.model_instructions().get(PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME).map(String::as_str),
        Some("Discover deferred tools")
    );
    assert!(catalog.route_permitted(&ToolRoute::ModelVisible { namespaced_name: "coding__bash".into() }));
    assert!(!catalog.route_permitted(&ToolRoute::Deferred { server: "coding".into(), tool: "bash".into() }));
    assert!(catalog.route_permitted(&ToolRoute::Deferred { server: "coding".into(), tool: "write".into() }));
    assert!(!catalog.route_permitted(&ToolRoute::Deferred { server: "coding".into(), tool: "read".into() }));
}

#[test]
fn catalog_preserves_names_schema_annotations_metadata_and_order() {
    let tools =
        vec![tool("first").with_annotations(ToolAnnotations::new().read_only(true).idempotent(true)), tool("second")];
    let mut catalog = ToolCatalog::new();
    catalog.upsert_server(connected_entry("alpha", ToolExposure::ModelVisible, &tools, &ToolFilter::default()));
    catalog.upsert_server(connected_entry("beta", ToolExposure::deferred_all(), &tools, &ToolFilter::default()));

    let first = catalog.tool("alpha__first").unwrap();
    assert_eq!(first.namespaced_name(), "alpha__first");
    assert_eq!(first.local_name(), "first");
    assert_eq!(first.definition().server.as_deref(), Some("alpha"));
    assert_eq!(first.definition().description, "first description");
    assert_eq!(first.definition().parameters["required"], json!(["value"]));
    let annotations = first.definition().annotations.as_ref().unwrap();
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(first.exposure(), ToolExposureKind::ModelVisible);
    assert!(first.allowed());
    assert_eq!(catalog.servers().iter().map(ServerCatalogEntry::name).collect::<Vec<_>>(), ["alpha", "beta"]);
    assert_eq!(
        catalog.servers()[0].tools().iter().map(CatalogTool::local_name).collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(catalog.servers()[0].description(), "alpha server");
    assert_eq!(catalog.servers()[0].auth_capability(), McpServerAuthCapability::OAuth);
}

#[test]
fn disconnected_servers_retain_status_without_exposing_tools_or_instructions() {
    let tools = vec![tool("hidden")];
    let mut catalog = ToolCatalog::new();
    catalog.upsert_server(ServerCatalogEntry::new(
        "remote",
        "Remote",
        Some("not visible".to_string()),
        McpServerStatus::NeedsOAuth,
        McpServerAuthCapability::OAuth,
        ToolExposure::deferred_all(),
        &tools,
        &ToolFilter::default(),
    ));
    catalog.set_progressive_discovery_instructions(Some("not visible".to_string()));

    let tools = catalog.tools();
    assert!(tools.model_visible.is_empty());
    assert!(tools.deferred.is_empty());
    assert!(catalog.discoverable_deferred_servers().is_empty());
    assert!(catalog.model_instructions().is_empty());
    assert!(!catalog.route_permitted(&ToolRoute::Deferred { server: "remote".into(), tool: "hidden".into() }));
    assert!(matches!(catalog.servers()[0].status(), McpServerStatus::NeedsOAuth));
    assert!(catalog.servers()[0].status_entry().can_authenticate());
}

#[test]
fn replacement_preserves_server_position_and_removal_is_atomic() {
    let mut catalog = ToolCatalog::new();
    catalog.upsert_server(connected_entry("first", ToolExposure::ModelVisible, &[tool("old")], &ToolFilter::default()));
    catalog.upsert_server(connected_entry(
        "second",
        ToolExposure::ModelVisible,
        &[tool("other")],
        &ToolFilter::default(),
    ));
    catalog.upsert_server(connected_entry("first", ToolExposure::ModelVisible, &[tool("new")], &ToolFilter::default()));

    assert_eq!(catalog.servers().iter().map(ServerCatalogEntry::name).collect::<Vec<_>>(), ["first", "second"]);
    assert!(catalog.tool("first__old").is_none());
    assert!(catalog.tool("first__new").is_some());

    let removed = catalog.remove_server("first").unwrap();
    assert_eq!(removed.name(), "first");
    assert_eq!(catalog.servers().iter().map(ServerCatalogEntry::name).collect::<Vec<_>>(), ["second"]);
}
