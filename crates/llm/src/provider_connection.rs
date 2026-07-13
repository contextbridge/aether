use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderAuthMode {
    #[default]
    Default,
    None,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderConnectionConfig {
    pub base_url: Option<String>,
    pub auth_mode: ProviderAuthMode,
    pub request_model: Option<String>,
    pub inference_profile_arn: Option<String>,
}

#[doc = include_str!("docs/provider_connection_override.md")]
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderConnectionOverride {
    /// Base URL override for the provider's API endpoint.
    #[serde(default, rename = "url", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Authentication mode. `default` uses the provider's normal credential
    /// chain; `none` disables auth, for local or unauthenticated servers.
    #[serde(default, rename = "auth", skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<ProviderAuthMode>,
    /// Provider-specific model or deployment target sent in requests without changing catalog identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_model: Option<String>,
    /// AWS Bedrock application inference profile ARN to route requests through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_profile_arn: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ProviderConnectionOverrides {
    providers: BTreeMap<String, ProviderConnectionOverride>,
}

impl ProviderConnectionConfig {
    pub fn from_override(value: ProviderConnectionOverride) -> Self {
        Self {
            base_url: value.base_url,
            auth_mode: value.auth_mode.unwrap_or_default(),
            request_model: value.request_model,
            inference_profile_arn: value.inference_profile_arn,
        }
    }
}

impl ProviderConnectionOverride {
    pub fn url(url: impl Into<String>) -> Self {
        Self { base_url: Some(url.into()), ..Self::default() }
    }

    pub fn auth(auth_mode: ProviderAuthMode) -> Self {
        Self { auth_mode: Some(auth_mode), ..Self::default() }
    }

    pub fn request_model(model: impl Into<String>) -> Self {
        Self { request_model: Some(model.into()), ..Self::default() }
    }

    pub fn inference_profile_arn(arn: impl Into<String>) -> Self {
        Self { inference_profile_arn: Some(arn.into()), ..Self::default() }
    }

    pub fn merge(&mut self, override_value: Self) {
        if override_value.base_url.is_some() {
            self.base_url = override_value.base_url;
        }
        if override_value.auth_mode.is_some() {
            self.auth_mode = override_value.auth_mode;
        }
        if override_value.request_model.is_some() {
            self.request_model = override_value.request_model;
        }
        if override_value.inference_profile_arn.is_some() {
            self.inference_profile_arn = override_value.inference_profile_arn;
        }
    }
}

impl ProviderConnectionOverrides {
    pub fn new(providers: BTreeMap<String, ProviderConnectionOverride>) -> Self {
        Self { providers }
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn merge(&mut self, overrides: ProviderConnectionOverrides) {
        for (provider, override_value) in overrides.providers {
            self.providers
                .entry(provider)
                .and_modify(|existing| existing.merge(override_value.clone()))
                .or_insert(override_value);
        }
    }

    pub fn config_for(&self, provider: &str) -> ProviderConnectionConfig {
        self.providers.get(provider).cloned().map(ProviderConnectionConfig::from_override).unwrap_or_default()
    }

    pub fn into_inner(self) -> BTreeMap<String, ProviderConnectionOverride> {
        self.providers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_and_merges_request_model() {
        let mut first: ProviderConnectionOverrides =
            serde_json::from_str(r#"{"azure-foundry":{"requestModel":"first"}}"#).unwrap();
        first.merge(ProviderConnectionOverrides::new(BTreeMap::from([(
            "azure-foundry".to_string(),
            ProviderConnectionOverride::request_model("second"),
        )])));

        assert_eq!(first.config_for("azure-foundry").request_model.as_deref(), Some("second"));
    }
    #[test]
    fn deserializes_bedrock_inference_profile_arn() {
        let overrides: ProviderConnectionOverrides = serde_json::from_str(
            r#"{"bedrock":{"inferenceProfileArn":"arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000"}}"#,
        )
        .unwrap();

        let config = overrides.config_for("bedrock");

        assert_eq!(
            config.inference_profile_arn.as_deref(),
            Some("arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000")
        );
    }

    #[test]
    fn merge_replaces_inference_profile_arn() {
        let mut first = ProviderConnectionOverride::inference_profile_arn("arn:first");

        first.merge(ProviderConnectionOverride::inference_profile_arn("arn:second"));

        assert_eq!(first.inference_profile_arn.as_deref(), Some("arn:second"));
    }

    #[test]
    fn provider_overrides_merge_inference_profile_arn() {
        let mut first = ProviderConnectionOverrides::new(BTreeMap::from([(
            "bedrock".to_string(),
            ProviderConnectionOverride::inference_profile_arn("arn:first"),
        )]));
        let second = ProviderConnectionOverrides::new(BTreeMap::from([(
            "bedrock".to_string(),
            ProviderConnectionOverride::inference_profile_arn("arn:second"),
        )]));

        first.merge(second);

        assert_eq!(first.config_for("bedrock").inference_profile_arn.as_deref(), Some("arn:second"));
    }
}
