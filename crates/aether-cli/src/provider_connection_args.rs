use std::collections::BTreeMap;
use std::str::FromStr;

use llm::{ProviderAuthMode, ProviderConnectionOverride, ProviderConnectionOverrides};

#[derive(Clone, Debug, Default, clap::Args)]
pub struct ProviderConnectionArgs {
    #[arg(
        long = "provider",
        value_name = "PROVIDER.url=URL|PROVIDER.auth=default|none|bedrock.inference-profile-arn=ARN"
    )]
    pub providers: Vec<ProviderArg>,
}

impl ProviderConnectionArgs {
    pub fn into_overrides(self) -> ProviderConnectionOverrides {
        let mut providers = BTreeMap::new();
        for arg in self.providers {
            providers.entry(arg.provider).or_insert_with(ProviderConnectionOverride::default).merge(arg.connection);
        }
        ProviderConnectionOverrides::new(providers)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderArg {
    provider: String,
    connection: ProviderConnectionOverride,
}

impl FromStr for ProviderArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (key, setting) = split_key_value(value)?;
        let (provider, field) = key
            .split_once('.')
            .ok_or_else(|| "provider override must be PROVIDER.url=URL, PROVIDER.auth=default|none, or bedrock.inference-profile-arn=ARN".to_string())?;

        validate_provider(provider)?;
        if setting.trim().is_empty() {
            return Err("provider value cannot be empty".to_string());
        }

        let connection = match field {
            "url" => {
                validate_url(setting)?;
                ProviderConnectionOverride::url(setting)
            }
            "auth" => ProviderConnectionOverride::auth(parse_auth_mode(setting)?),
            "inference-profile-arn" => {
                if provider != "bedrock" {
                    return Err("inference-profile-arn is only supported for the bedrock provider".to_string());
                }
                ProviderConnectionOverride::inference_profile_arn(setting)
            }
            _ => return Err("provider override field must be url, auth, or inference-profile-arn".to_string()),
        };

        Ok(Self { provider: provider.to_string(), connection })
    }
}

fn split_key_value(value: &str) -> Result<(&str, &str), String> {
    value.split_once('=').ok_or_else(|| "provider override must be PROVIDER.FIELD=VALUE".to_string())
}

fn validate_provider(provider: &str) -> Result<(), String> {
    if provider.trim().is_empty() {
        return Err("provider name cannot be empty".to_string());
    }
    Ok(())
}

fn validate_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("invalid provider URL: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(format!("provider URL must use http or https, got {scheme}")),
    }
}

fn parse_auth_mode(value: &str) -> Result<ProviderAuthMode, String> {
    match value {
        "default" => Ok(ProviderAuthMode::Default),
        "none" => Ok(ProviderAuthMode::None),
        _ => Err("provider auth mode must be default or none".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_url() {
        let arg: ProviderArg = "bedrock.url=http://127.0.0.1:8787".parse().unwrap();
        assert_eq!(arg.provider, "bedrock");
        assert_eq!(arg.connection.base_url.as_deref(), Some("http://127.0.0.1:8787"));
    }

    #[test]
    fn parses_provider_auth_modes() {
        assert_eq!(
            "bedrock.auth=none".parse::<ProviderArg>().unwrap().connection.auth_mode,
            Some(ProviderAuthMode::None)
        );
        assert_eq!(
            "bedrock.auth=default".parse::<ProviderArg>().unwrap().connection.auth_mode,
            Some(ProviderAuthMode::Default)
        );
    }

    #[test]
    fn combines_repeated_provider_overrides() {
        let args = ProviderConnectionArgs {
            providers: vec![
                "bedrock.url=http://127.0.0.1:8787".parse().unwrap(),
                "bedrock.auth=none".parse().unwrap(),
                "bedrock.inference-profile-arn=arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000"
                    .parse()
                    .unwrap(),
            ],
        };

        let config = args.into_overrides().config_for("bedrock");

        assert_eq!(config.base_url.as_deref(), Some("http://127.0.0.1:8787"));
        assert_eq!(config.auth_mode, ProviderAuthMode::None);
        assert_eq!(
            config.inference_profile_arn.as_deref(),
            Some("arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000")
        );
    }

    #[test]
    fn parses_bedrock_inference_profile_arn() {
        let arg: ProviderArg =
            "bedrock.inference-profile-arn=arn:aws:bedrock:us-west-2:000000000000:inference-profile/us.anthropic.claude-sonnet-4-5-20250929-v1:0"
                .parse()
                .unwrap();

        assert_eq!(arg.provider, "bedrock");
        assert_eq!(
            arg.connection.inference_profile_arn.as_deref(),
            Some(
                "arn:aws:bedrock:us-west-2:000000000000:inference-profile/us.anthropic.claude-sonnet-4-5-20250929-v1:0"
            )
        );
    }

    #[test]
    fn rejects_invalid_values() {
        assert!("bedrock".parse::<ProviderArg>().is_err());
        assert!("bedrock.url".parse::<ProviderArg>().is_err());
        assert!(".url=http://127.0.0.1:8787".parse::<ProviderArg>().is_err());
        assert!("bedrock.url=".parse::<ProviderArg>().is_err());
        assert!("bedrock.url=file:///tmp/proxy".parse::<ProviderArg>().is_err());
        assert!("bedrock.auth=disabled".parse::<ProviderArg>().is_err());
        assert!("bedrock.region=us-west-2".parse::<ProviderArg>().is_err());
        assert!("openai.inference-profile-arn=arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000".parse::<ProviderArg>().is_err());
    }
}
