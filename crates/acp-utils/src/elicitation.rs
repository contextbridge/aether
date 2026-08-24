use crate::notifications::AETHER_META_NAMESPACE;
use agent_client_protocol::schema::v1::{
    self as acp, CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse, Meta, SessionId,
};
use rmcp::model::{self as mcp, ElicitRequestParams, ElicitResult};
use serde_json::{Map, Number, Value, json};

#[derive(Debug, thiserror::Error)]
pub enum ElicitationConversionError {
    #[error("unsupported {0}")]
    Unsupported(&'static str),
    #[error("legacy MCP enum has a different number of values and titles")]
    MismatchedEnumTitles,
    #[error("ACP elicitation response contains a non-finite number")]
    NonFiniteNumber,
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
            result.content = accept.content.map(map_acp_elicitation_content_to_mcp).transpose()?;
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
    let required = schema.required.as_deref().unwrap_or_default();
    schema.properties.iter().try_fold(
        acp::ElicitationSchema::new()
            .title(owned_string(schema.title.as_deref()))
            .description(owned_string(schema.description.as_deref())),
        |converted, (name, property)| {
            Ok(converted.property(name, map_mcp_primitive_schema_to_acp(property)?, required.contains(name)))
        },
    )
}

fn map_mcp_primitive_schema_to_acp(
    property: &mcp::PrimitiveSchemaDefinition,
) -> Result<acp::ElicitationPropertySchema, ElicitationConversionError> {
    match property {
        mcp::PrimitiveSchemaDefinition::String(schema) => Ok(map_mcp_string_schema_to_acp(schema)?.into()),
        mcp::PrimitiveSchemaDefinition::Number(schema) => Ok(acp::NumberPropertySchema::new()
            .title(owned_string(schema.title.as_deref()))
            .description(owned_string(schema.description.as_deref()))
            .minimum(schema.minimum)
            .maximum(schema.maximum)
            .default_value(schema.default)
            .into()),
        mcp::PrimitiveSchemaDefinition::Integer(schema) => Ok(acp::IntegerPropertySchema::new()
            .title(owned_string(schema.title.as_deref()))
            .description(owned_string(schema.description.as_deref()))
            .minimum(schema.minimum)
            .maximum(schema.maximum)
            .default_value(schema.default)
            .into()),
        mcp::PrimitiveSchemaDefinition::Boolean(schema) => Ok(acp::BooleanPropertySchema::new()
            .title(owned_string(schema.title.as_deref()))
            .description(owned_string(schema.description.as_deref()))
            .default_value(schema.default)
            .into()),
        mcp::PrimitiveSchemaDefinition::Enum(schema) => map_mcp_enum_schema_to_acp(schema),
        _ => Err(ElicitationConversionError::Unsupported("MCP elicitation schema variant")),
    }
}

fn map_mcp_string_schema_to_acp(
    schema: &mcp::StringSchema,
) -> Result<acp::StringPropertySchema, ElicitationConversionError> {
    let mut converted = acp::StringPropertySchema::new()
        .title(owned_string(schema.title.as_deref()))
        .description(owned_string(schema.description.as_deref()))
        .min_length(schema.min_length)
        .max_length(schema.max_length)
        .default_value(schema.default.clone());
    if let Some(format) = schema.format {
        converted = converted.format(map_mcp_string_format_to_acp(format)?);
    }
    Ok(converted)
}

fn map_mcp_string_format_to_acp(format: mcp::StringFormat) -> Result<acp::StringFormat, ElicitationConversionError> {
    match format {
        mcp::StringFormat::Email => Ok(acp::StringFormat::Email),
        mcp::StringFormat::Uri => Ok(acp::StringFormat::Uri),
        mcp::StringFormat::Date => Ok(acp::StringFormat::Date),
        mcp::StringFormat::DateTime => Ok(acp::StringFormat::DateTime),
        _ => Err(ElicitationConversionError::Unsupported("MCP elicitation string format")),
    }
}

fn map_mcp_enum_schema_to_acp(
    schema: &mcp::EnumSchema,
) -> Result<acp::ElicitationPropertySchema, ElicitationConversionError> {
    match schema {
        mcp::EnumSchema::Single(mcp::SingleSelectEnumSchema::Untitled(schema)) => Ok(build_acp_single_select_schema(
            schema.title.as_deref(),
            schema.description.as_deref(),
            schema.default.clone(),
        )
        .enum_values(schema.enum_.clone())
        .into()),
        mcp::EnumSchema::Single(mcp::SingleSelectEnumSchema::Titled(schema)) => Ok(build_acp_single_select_schema(
            schema.title.as_deref(),
            schema.description.as_deref(),
            schema.default.clone(),
        )
        .one_of(map_mcp_enum_options_to_acp(&schema.one_of))
        .into()),
        mcp::EnumSchema::Multi(mcp::MultiSelectEnumSchema::Untitled(schema)) => {
            Ok(acp::MultiSelectPropertySchema::new(schema.items.enum_.clone())
                .title(owned_string(schema.title.as_deref()))
                .description(owned_string(schema.description.as_deref()))
                .min_items(schema.min_items)
                .max_items(schema.max_items)
                .default_value(schema.default.clone())
                .into())
        }
        mcp::EnumSchema::Multi(mcp::MultiSelectEnumSchema::Titled(schema)) => {
            Ok(acp::MultiSelectPropertySchema::titled(map_mcp_enum_options_to_acp(&schema.items.any_of))
                .title(owned_string(schema.title.as_deref()))
                .description(owned_string(schema.description.as_deref()))
                .min_items(schema.min_items)
                .max_items(schema.max_items)
                .default_value(schema.default.clone())
                .into())
        }
        mcp::EnumSchema::Legacy(schema) => {
            let (enum_values, one_of) = match &schema.enum_names {
                Some(titles) if titles.len() == schema.enum_.len() => (
                    None,
                    Some(
                        schema
                            .enum_
                            .iter()
                            .zip(titles)
                            .map(|(value, title)| acp::EnumOption::new(value, title))
                            .collect::<Vec<_>>(),
                    ),
                ),
                Some(_) => return Err(ElicitationConversionError::MismatchedEnumTitles),
                None => (Some(schema.enum_.clone()), None),
            };
            Ok(build_acp_single_select_schema(
                schema.title.as_deref(),
                schema.description.as_deref(),
                schema.default.clone(),
            )
            .enum_values(enum_values)
            .one_of(one_of)
            .into())
        }
        _ => Err(ElicitationConversionError::Unsupported("MCP elicitation schema variant")),
    }
}

fn build_acp_single_select_schema(
    title: Option<&str>,
    description: Option<&str>,
    default: Option<String>,
) -> acp::StringPropertySchema {
    acp::StringPropertySchema::new()
        .title(title.map(str::to_owned))
        .description(description.map(str::to_owned))
        .default_value(default)
}

fn map_mcp_enum_options_to_acp(options: &[mcp::ConstTitle]) -> Vec<acp::EnumOption> {
    options.iter().map(|option| acp::EnumOption::new(&option.const_, &option.title)).collect()
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

fn map_acp_elicitation_content_to_mcp(
    content: std::collections::BTreeMap<String, acp::ElicitationContentValue>,
) -> Result<Value, ElicitationConversionError> {
    Ok(Value::Object(
        content
            .into_iter()
            .map(|(name, value)| map_acp_elicitation_content_value_to_mcp(value).map(|value| (name, value)))
            .collect::<Result<Map<_, _>, _>>()?,
    ))
}

fn map_acp_elicitation_content_value_to_mcp(
    value: acp::ElicitationContentValue,
) -> Result<Value, ElicitationConversionError> {
    match value {
        acp::ElicitationContentValue::String(value) => Ok(Value::String(value)),
        acp::ElicitationContentValue::Integer(value) => Ok(Value::Number(value.into())),
        acp::ElicitationContentValue::Number(value) => {
            Number::from_f64(value).map(Value::Number).ok_or(ElicitationConversionError::NonFiniteNumber)
        }
        acp::ElicitationContentValue::Boolean(value) => Ok(Value::Bool(value)),
        acp::ElicitationContentValue::StringArray(values) => {
            Ok(Value::Array(values.into_iter().map(Value::String).collect()))
        }
        _ => Err(ElicitationConversionError::Unsupported("ACP elicitation response content")),
    }
}

fn owned_string(value: Option<&str>) -> Option<String> {
    value.map(str::to_owned)
}

fn build_scoped_elicitation_id(session_id: &SessionId, server_name: &str, elicitation_id: &str) -> String {
    json!([session_id.0.as_ref(), server_name, elicitation_id]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        ElicitationAction, ElicitationMode, ElicitationPropertySchema, ElicitationScope,
    };

    #[test]
    fn form_request_converts_with_session_scope_and_source_server() {
        let request = ElicitRequestParams::FormElicitationParams {
            meta: Some(mcp::RequestMetaObject::from(Map::from_iter([("ui".to_string(), json!("planReview"))]))),
            message: "Review the plan".to_string(),
            requested_schema: rmcp::model::ElicitationSchema::builder().required_bool("approved").build().unwrap(),
        };

        let converted = map_mcp_elicitation_request_to_acp("plan", &SessionId::new("session-1"), &request).unwrap();

        let ElicitationMode::Form(form) = converted.mode else { panic!("expected form mode") };
        let ElicitationScope::Session(scope) = form.scope else { panic!("expected session scope") };
        assert_eq!(&*scope.session_id.0, "session-1");
        assert!(form.requested_schema.properties.contains_key("approved"));
        assert_eq!(converted.message, "Review the plan");
        assert_eq!(converted.meta.as_ref().and_then(|meta| meta.get("ui")), Some(&json!("planReview")));
        assert_eq!(source_mcp_server_name(converted.meta.as_ref()), Some("plan"));
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
        let first = build_scoped_elicitation_id(&SessionId::new("session"), "alpha:bravo", "oauth");
        let second = build_scoped_elicitation_id(&SessionId::new("session"), "alpha", "bravo:oauth");

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
