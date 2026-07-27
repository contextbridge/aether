use std::fmt::Write as _;

use thiserror::Error;

/// Executable per-model transport strategy.
///
/// A model without an override uses its provider's default transport. Each
/// variant here is a complete routing decision implemented by a runtime provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelTransport {
    /// `OpenAI` Responses API at a dedicated endpoint template.
    OpenAiResponses { base_url_template: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TemplateError {
    #[error("template variable '{name}' could not be resolved in '{template}'")]
    UnresolvedVariable { name: String, template: String },
    #[error("template '{template}' has an unterminated '${{' placeholder")]
    UnterminatedPlaceholder { template: String },
}

/// Expand `${VAR}` placeholders in an endpoint template.
///
/// `lookup` resolves a variable name to its value; returning `None` fails the
/// expansion rather than substituting an empty string, so a misconfigured
/// environment surfaces as an error instead of a malformed URL.
pub fn expand_api_template(template: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(TemplateError::UnterminatedPlaceholder { template: template.to_string() });
        };
        let name = &after[..end];
        let value = lookup(name).ok_or_else(|| TemplateError::UnresolvedVariable {
            name: name.to_string(),
            template: template.to_string(),
        })?;
        let _ = out.write_str(&value);
        rest = &after[end + 1..];
    }

    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v).to_string())
    }

    #[test]
    fn expands_a_single_variable() {
        let expanded = expand_api_template(
            "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1",
            lookup(&[("AWS_REGION", "us-west-2")]),
        )
        .unwrap();

        assert_eq!(expanded, "https://bedrock-mantle.us-west-2.api.aws/openai/v1");
    }

    #[test]
    fn leaves_templates_without_placeholders_untouched() {
        let expanded = expand_api_template("https://bedrock-mantle.us-east-1.api.aws/v1", lookup(&[])).unwrap();

        assert_eq!(expanded, "https://bedrock-mantle.us-east-1.api.aws/v1");
    }

    #[test]
    fn expands_multiple_variables() {
        let expanded = expand_api_template(
            "https://${HOST}/v1/projects/${PROJECT}/end",
            lookup(&[("HOST", "example.com"), ("PROJECT", "p-1")]),
        )
        .unwrap();

        assert_eq!(expanded, "https://example.com/v1/projects/p-1/end");
    }

    #[test]
    fn unresolved_variable_is_an_error() {
        let error = expand_api_template("https://x.${AWS_REGION}.aws/v1", lookup(&[])).unwrap_err();

        assert_eq!(
            error,
            TemplateError::UnresolvedVariable {
                name: "AWS_REGION".to_string(),
                template: "https://x.${AWS_REGION}.aws/v1".to_string(),
            }
        );
    }

    #[test]
    fn unterminated_placeholder_is_an_error() {
        let error = expand_api_template("https://x.${AWS_REGION/v1", lookup(&[])).unwrap_err();

        assert!(matches!(error, TemplateError::UnterminatedPlaceholder { .. }));
    }
}
