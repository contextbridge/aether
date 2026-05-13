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
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderConnectionOverride {
    #[serde(default, rename = "url", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, rename = "auth", skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<ProviderAuthMode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ProviderConnectionOverrides {
    providers: BTreeMap<String, ProviderConnectionOverride>,
}

impl ProviderConnectionConfig {
    pub fn from_override(value: ProviderConnectionOverride) -> Self {
        Self { base_url: value.base_url, auth_mode: value.auth_mode.unwrap_or_default() }
    }
}

impl ProviderConnectionOverride {
    pub fn url(url: impl Into<String>) -> Self {
        Self { base_url: Some(url.into()), auth_mode: None }
    }

    pub fn auth(auth_mode: ProviderAuthMode) -> Self {
        Self { base_url: None, auth_mode: Some(auth_mode) }
    }

    pub fn merge(&mut self, override_value: Self) {
        if override_value.base_url.is_some() {
            self.base_url = override_value.base_url;
        }
        if override_value.auth_mode.is_some() {
            self.auth_mode = override_value.auth_mode;
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
