mod form;

pub use form::{
    FieldError, FieldInput, FormError, FormField, FormFieldKind, SelectOption, accept_content, form_fields,
};

use crate::notifications::AETHER_META_NAMESPACE;
use agent_client_protocol::schema::v2::{
    self as acp, CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse, Meta, SessionId,
};
use rmcp::model::{self as mcp, ElicitRequestParams, ElicitResult};
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum ElicitationConversionError {
    #[error("unsupported {0}")]
    Unsupported(&'static str),
    #[error("legacy MCP enum has a different number of values and titles")]
    MismatchedEnumTitles,
    #[error("elicitation could not be converted: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn map_mcp_elicitation_request_to_acp(
    server_name: &str,
    session_id: &SessionId,
    request: &ElicitRequestParams,
) -> Result<CreateElicitationRequest, ElicitationConversionError> {
    let scope = || acp::ElicitationSessionScope::new(session_id.clone());
    let (request, meta) = match request {
        ElicitRequestParams::FormElicitationParams { meta, message, requested_schema } => (
            CreateElicitationRequest::new(
                acp::ElicitationFormMode::new(scope(), map_mcp_elicitation_schema_to_acp(requested_schema)?),
                message.clone(),
            ),
            meta,
        ),
        ElicitRequestParams::UrlElicitationParams { meta, message, url, elicitation_id } => (
            CreateElicitationRequest::new(
                acp::ElicitationUrlMode::new(
                    scope(),
                    build_scoped_elicitation_id(session_id, server_name, elicitation_id),
                    url.clone(),
                ),
                message.clone(),
            ),
            meta,
        ),
        _ => return Err(ElicitationConversionError::Unsupported("MCP elicitation request variant")),
    };
    Ok(request.meta(map_mcp_meta_to_acp(meta.as_ref(), server_name)))
}

pub fn map_acp_elicitation_response_to_mcp(
    response: CreateElicitationResponse,
) -> Result<ElicitResult, ElicitationConversionError> {
    let mut result = match response.action {
        acp::ElicitationAction::Accept(accept) => {
            let mut result = ElicitResult::new(mcp::ElicitationAction::Accept);
            result.content = accept.content.map(serde_json::to_value).transpose()?;
            result
        }
        acp::ElicitationAction::Decline => ElicitResult::new(mcp::ElicitationAction::Decline),
        acp::ElicitationAction::Cancel => ElicitResult::new(mcp::ElicitationAction::Cancel),
        _ => return Err(ElicitationConversionError::Unsupported("ACP elicitation response action")),
    };
    result.meta = response.meta.map(mcp::MetaObject::from);
    Ok(result)
}

pub fn build_acp_elicitation_completion_notification(
    session_id: &SessionId,
    server_name: &str,
    elicitation_id: &str,
) -> CompleteElicitationNotification {
    CompleteElicitationNotification::new(build_scoped_elicitation_id(session_id, server_name, elicitation_id))
}

/// The MCP server a converted elicitation originated from, if any.
pub fn source_mcp_server_name(meta: Option<&Meta>) -> Option<&str> {
    meta.and_then(|meta| meta.get(AETHER_META_NAMESPACE))
        .and_then(Value::as_object)
        .and_then(|source| source.get("mcpServer"))
        .and_then(Value::as_str)
}

fn map_mcp_elicitation_schema_to_acp(
    schema: &mcp::ElicitationSchema,
) -> Result<acp::ElicitationSchema, ElicitationConversionError> {
    let mut schema = serde_json::to_value(schema)?;
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.values_mut().try_for_each(title_legacy_enum)?;
    }
    Ok(serde_json::from_value(schema)?)
}

fn title_legacy_enum(property: &mut Value) -> Result<(), ElicitationConversionError> {
    let Some(property) = property.as_object_mut() else { return Ok(()) };
    let Some(Value::Array(titles)) = property.remove("enumNames") else { return Ok(()) };
    let values = match property.remove("enum") {
        Some(Value::Array(values)) if values.len() == titles.len() => values,
        _ => return Err(ElicitationConversionError::MismatchedEnumTitles),
    };
    let options = values.into_iter().zip(titles).map(|(value, title)| json!({ "const": value, "title": title }));
    property.insert("oneOf".to_string(), options.collect());
    Ok(())
}

fn map_mcp_meta_to_acp(meta: Option<&mcp::RequestMetaObject>, server_name: &str) -> Meta {
    let mut meta: Meta =
        meta.map(|meta| meta.iter().map(|(key, value)| (key.clone(), value.clone())).collect()).unwrap_or_default();
    let source = meta.entry(AETHER_META_NAMESPACE.to_string()).or_insert_with(|| json!({}));
    if !source.is_object() {
        *source = json!({});
    }
    source["mcpServer"] = json!(server_name);
    meta
}

fn build_scoped_elicitation_id(session_id: &SessionId, server_name: &str, elicitation_id: &str) -> String {
    json!([session_id.0.as_ref(), server_name, elicitation_id]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v2::{
        ElicitationAction, ElicitationMode, ElicitationPropertySchema, ElicitationScope,
    };
    use serde_json::Map;

    #[test]
    fn form_request_converts_with_session_scope_and_source_server() {
        let request = ElicitRequestParams::FormElicitationParams {
            meta: Some(mcp::RequestMetaObject::from(Map::from_iter([("ui".to_string(), json!("artifactReview"))]))),
            message: "Review the plan".to_string(),
            requested_schema: rmcp::model::ElicitationSchema::builder().required_bool("approved").build().unwrap(),
        };

        let converted = map_mcp_elicitation_request_to_acp("review", &SessionId::new("session-1"), &request).unwrap();

        let ElicitationMode::Form(form) = converted.mode else { panic!("expected form mode") };
        let ElicitationScope::Session(scope) = form.scope else { panic!("expected session scope") };
        assert_eq!(&*scope.session_id.0, "session-1");
        assert!(form.requested_schema.properties.contains_key("approved"));
        assert_eq!(converted.message, "Review the plan");
        assert_eq!(converted.meta.as_ref().and_then(|meta| meta.get("ui")), Some(&json!("artifactReview")));
        assert_eq!(source_mcp_server_name(converted.meta.as_ref()), Some("review"));
    }

    #[test]
    fn every_mcp_property_kind_converts_to_the_matching_acp_schema() {
        let requested_schema = serde_json::from_value(json!({
            "type": "object",
            "title": "Settings",
            "description": "Pick settings",
            "properties": {
                "email": { "type": "string", "title": "Email", "format": "email", "minLength": 3, "maxLength": 50, "default": "a@b.c" },
                "ratio": { "type": "number", "description": "Ratio", "minimum": 0.5, "maximum": 2.5, "default": 1.5 },
                "count": { "type": "integer", "minimum": 1, "maximum": 9, "default": 3 },
                "enabled": { "type": "boolean", "default": true },
                "color": { "type": "string", "enum": ["red", "blue"], "default": "red" },
                "size": { "type": "string", "oneOf": [{ "const": "s", "title": "Small" }, { "const": "l", "title": "Large" }] },
                "tags": { "type": "array", "items": { "type": "string", "enum": ["a", "b"] }, "minItems": 1, "maxItems": 2, "default": ["a"] },
                "labels": { "type": "array", "items": { "anyOf": [{ "const": "x", "title": "X" }] } },
                "legacy": { "type": "string", "enum": ["p", "q"], "enumNames": ["P", "Q"] }
            },
            "required": ["count", "email"]
        }))
        .unwrap();
        let request =
            ElicitRequestParams::FormElicitationParams { meta: None, message: "Configure".into(), requested_schema };

        let converted = map_mcp_elicitation_request_to_acp("settings", &SessionId::new("session-1"), &request).unwrap();
        let ElicitationMode::Form(form) = converted.mode else { panic!("expected form mode") };

        assert_eq!(
            serde_json::to_value(&form.requested_schema).unwrap(),
            json!({
                "type": "object",
                "title": "Settings",
                "description": "Pick settings",
                "properties": {
                    "email": { "type": "string", "title": "Email", "format": "email", "minLength": 3, "maxLength": 50, "default": "a@b.c" },
                    "ratio": { "type": "number", "description": "Ratio", "minimum": 0.5, "maximum": 2.5, "default": 1.5 },
                    "count": { "type": "integer", "minimum": 1, "maximum": 9, "default": 3 },
                    "enabled": { "type": "boolean", "default": true },
                    "color": { "type": "string", "enum": ["red", "blue"], "default": "red" },
                    "size": { "type": "string", "oneOf": [{ "const": "s", "title": "Small" }, { "const": "l", "title": "Large" }] },
                    "tags": { "type": "array", "items": { "type": "string", "enum": ["a", "b"] }, "minItems": 1, "maxItems": 2, "default": ["a"] },
                    "labels": { "type": "array", "items": { "anyOf": [{ "const": "x", "title": "X" }] } },
                    "legacy": { "type": "string", "oneOf": [{ "const": "p", "title": "P" }, { "const": "q", "title": "Q" }] }
                },
                "required": ["count", "email"]
            })
        );
    }

    #[test]
    fn legacy_enum_names_convert_to_titled_acp_options() {
        let mut legacy = rmcp::model::LegacyEnumSchema::new(vec!["small".into(), "large".into()]);
        legacy.enum_names = Some(vec!["Small".into(), "Large".into()]);
        legacy.default = Some("large".into());
        let request = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Pick a size".to_string(),
            requested_schema: rmcp::model::ElicitationSchema::builder()
                .required_property(
                    "size",
                    rmcp::model::PrimitiveSchemaDefinition::Enum(rmcp::model::EnumSchema::Legacy(legacy)),
                )
                .build()
                .unwrap(),
        };

        let converted = map_mcp_elicitation_request_to_acp("catalog", &SessionId::new("session-1"), &request).unwrap();
        let ElicitationMode::Form(form) = converted.mode else { panic!("expected form mode") };
        let Some(ElicitationPropertySchema::String(size)) = form.requested_schema.properties.get("size") else {
            panic!("expected string property")
        };

        assert_eq!(size.default.as_deref(), Some("large"));
        assert_eq!(
            size.one_of
                .as_ref()
                .unwrap()
                .iter()
                .map(|option| (option.value.as_str(), option.title.as_str()))
                .collect::<Vec<_>>(),
            vec![("small", "Small"), ("large", "Large")]
        );
    }

    #[test]
    fn legacy_enum_with_mismatched_titles_is_rejected() {
        let mut legacy = rmcp::model::LegacyEnumSchema::new(vec!["small".into(), "large".into()]);
        legacy.enum_names = Some(vec!["Small".into()]);
        let request = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Pick a size".to_string(),
            requested_schema: rmcp::model::ElicitationSchema::builder()
                .required_property(
                    "size",
                    rmcp::model::PrimitiveSchemaDefinition::Enum(rmcp::model::EnumSchema::Legacy(legacy)),
                )
                .build()
                .unwrap(),
        };

        let error = map_mcp_elicitation_request_to_acp("catalog", &SessionId::new("session-1"), &request).unwrap_err();

        assert!(matches!(error, ElicitationConversionError::MismatchedEnumTitles), "{error}");
    }

    #[test]
    fn url_ids_are_namespaced_per_session_and_mcp_server() {
        let request = ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "Authorize".to_string(),
            url: "https://example.com/oauth".to_string(),
            elicitation_id: "oauth".to_string(),
        };

        let alpha = map_mcp_elicitation_request_to_acp("alpha", &SessionId::new("session-1"), &request).unwrap();
        let bravo = map_mcp_elicitation_request_to_acp("bravo", &SessionId::new("session-1"), &request).unwrap();
        let other_session =
            map_mcp_elicitation_request_to_acp("alpha", &SessionId::new("session-2"), &request).unwrap();
        let ElicitationMode::Url(alpha_url) = alpha.mode else { panic!("expected URL mode") };
        let ElicitationMode::Url(bravo_url) = bravo.mode else { panic!("expected URL mode") };
        let ElicitationMode::Url(other_session_url) = other_session.mode else { panic!("expected URL mode") };

        assert_ne!(alpha_url.elicitation_id, bravo_url.elicitation_id);
        assert_ne!(alpha_url.elicitation_id, other_session_url.elicitation_id);
        assert_eq!(
            build_acp_elicitation_completion_notification(&SessionId::new("session-1"), "alpha", "oauth")
                .elicitation_id,
            alpha_url.elicitation_id
        );
        assert_eq!(alpha_url.url, "https://example.com/oauth");
    }

    #[test]
    fn url_id_namespacing_is_unambiguous() {
        let first = build_acp_elicitation_completion_notification(&SessionId::new("session"), "alpha:bravo", "oauth");
        let second = build_acp_elicitation_completion_notification(&SessionId::new("session"), "alpha", "bravo:oauth");

        assert_ne!(first, second);
    }

    #[test]
    fn responses_convert_explicitly_and_unknown_actions_fail() {
        let accept = CreateElicitationResponse::new(acp::ElicitationAcceptAction::new().content(
            std::collections::BTreeMap::from([
                ("approved".to_string(), acp::ElicitationContentValue::Boolean(true)),
                ("count".to_string(), acp::ElicitationContentValue::Integer(3)),
                ("name".to_string(), acp::ElicitationContentValue::String("Ada".to_string())),
                (
                    "tags".to_string(),
                    acp::ElicitationContentValue::StringArray(vec!["rust".to_string(), "acp".to_string()]),
                ),
            ]),
        ))
        .meta(Map::from_iter([("traceId".to_string(), json!("trace-1"))]));
        let result = map_acp_elicitation_response_to_mcp(accept).unwrap();
        assert_eq!(result.action, rmcp::model::ElicitationAction::Accept);
        assert_eq!(
            result.content,
            Some(json!({ "approved": true, "count": 3, "name": "Ada", "tags": ["rust", "acp"] }))
        );
        assert_eq!(result.meta.as_ref().and_then(|meta| meta.get("traceId")), Some(&json!("trace-1")));

        let decline =
            map_acp_elicitation_response_to_mcp(CreateElicitationResponse::new(ElicitationAction::Decline)).unwrap();
        assert_eq!(decline.action, rmcp::model::ElicitationAction::Decline);

        let unknown = CreateElicitationResponse::new(acp::OtherElicitationAction::new(
            "_defer",
            std::collections::BTreeMap::new(),
        ));
        assert!(map_acp_elicitation_response_to_mcp(unknown).is_err());
    }
}
