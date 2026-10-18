use crate::Result;
use crate::catalog::LlmModel;
#[cfg(feature = "bedrock")]
use crate::providers::bedrock::BedrockProvider;
#[cfg(feature = "codex")]
use crate::providers::codex::CodexProvider;
use crate::providers::generic::{self, GenericProvider};
use crate::{
    LlmError, ProviderConnectionConfig, ProviderConnectionOverrides, StreamingModelProvider,
    alloyed::AlloyedModelProvider,
};
use futures::future::BoxFuture;
use std::collections::{HashMap, HashSet};
use std::future::{Future, ready};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::OnceCell;

#[doc = include_str!("docs/parser.md")]
pub struct ModelProviderParser {
    factories: HashMap<String, CreateProviderFn>,
}

impl Default for ModelProviderParser {
    /// Create a parser with all built-in providers registered
    fn default() -> Self {
        let mut parser = Self { factories: HashMap::new() };

        for config in generic::BUILT_INS {
            parser = parser.with_generic_provider(config);
        }

        #[cfg(feature = "bedrock")]
        {
            parser = parser.register(
                "bedrock",
                |connection| async move { Ok(BedrockProvider::new(connection).await) },
                BedrockProvider::with_model,
            );
        }

        parser
    }
}

impl ModelProviderParser {
    #[cfg(feature = "codex")]
    pub fn with_codex_provider(self, codex: CodexProvider) -> Self {
        self.register_sync(
            "codex",
            move |connection| Ok(codex.clone().with_connection(connection)),
            CodexProvider::with_model,
        )
    }

    pub fn with_generic_provider(self, config: &'static generic::ProviderConfig) -> Self {
        self.register_sync(
            config.provider.parser_name(),
            move |connection| GenericProvider::from_env_with_connection(config, connection),
            GenericProvider::with_model,
        )
    }

    /// Parse a model specification string and create a provider instance.
    ///
    /// Returns both the provider and an `LlmModel` describing the identity
    /// of the first (or only) provider in the spec.
    ///
    /// # Format
    ///
    /// - `"provider:model"` - Single provider (e.g., "anthropic:claude-3.5-sonnet")
    /// - `"provider1:model1,provider2:model2"` - Multiple providers create an `AlloyedModelProvider`
    ///
    pub async fn parse(
        &self,
        models_str: &str,
        connections: &ProviderConnectionOverrides,
    ) -> Result<(Box<dyn StreamingModelProvider>, LlmModel)> {
        let provider_model_pairs: Vec<&str> = models_str.split(',').map(str::trim).collect();
        if provider_model_pairs.is_empty() {
            return Err(LlmError::EmptyModelSpec);
        }

        let bedrock_has_inference_profile_arn = connections.config_for("bedrock").inference_profile_arn.is_some();
        let mut seen_bedrock = false;
        let mut seen_request_model_providers = HashSet::new();
        let mut providers = Vec::new();
        let mut first_identity: Option<LlmModel> = None;

        for pair in provider_model_pairs {
            let (provider_name, model) = pair.split_once(':').unwrap_or((pair, ""));

            if provider_name == "bedrock" && bedrock_has_inference_profile_arn {
                if seen_bedrock {
                    return Err(LlmError::DuplicateProvider {
                        provider: "bedrock".to_string(),
                        field: "inferenceProfileArn".to_string(),
                    });
                }
                seen_bedrock = true;
            }

            let connection = connections.config_for(provider_name);
            if connection.request_model.is_some() && !seen_request_model_providers.insert(provider_name) {
                return Err(LlmError::DuplicateProvider {
                    provider: provider_name.to_string(),
                    field: "requestModel".to_string(),
                });
            }

            let factory = self
                .factories
                .get(provider_name)
                .ok_or_else(|| LlmError::UnknownProvider { provider: provider_name.to_string() })?;
            providers.push(factory(model, connection).await?);

            if first_identity.is_none() {
                first_identity = Some(pair.parse::<LlmModel>().map_err(LlmError::InvalidModelSpec)?);
            }
        }

        let identity = first_identity.ok_or(LlmError::EmptyModelSpec)?;

        let provider: Box<dyn StreamingModelProvider> = if providers.len() == 1 {
            providers.into_iter().next().ok_or(LlmError::EmptyModelSpec)?
        } else {
            Box::new(AlloyedModelProvider::new(providers))
        };

        Ok((provider, identity))
    }
}

type CreateProviderFn = Box<
    dyn Fn(&str, ProviderConnectionConfig) -> BoxFuture<'static, Result<Box<dyn StreamingModelProvider>>> + Send + Sync,
>;

struct ProviderPool<T, U> {
    connect: U,
    providers: Mutex<HashMap<ProviderConnectionConfig, Arc<OnceCell<T>>>>,
}

impl<T, U, V> ProviderPool<T, U>
where
    T: Clone,
    U: Fn(ProviderConnectionConfig) -> V,
    V: Future<Output = Result<T>>,
{
    async fn connect(&self, config: ProviderConnectionConfig) -> Result<T> {
        let cell = Arc::clone(
            self.providers.lock().unwrap_or_else(PoisonError::into_inner).entry(config.clone()).or_default(),
        );
        cell.get_or_try_init(|| (self.connect)(config)).await.cloned()
    }
}

impl ModelProviderParser {
    fn register<T, U, V>(mut self, name: &str, connect: U, with_model: fn(T, &str) -> T) -> Self
    where
        T: StreamingModelProvider + Clone + 'static,
        U: Fn(ProviderConnectionConfig) -> V + Send + Sync + 'static,
        V: Future<Output = Result<T>> + Send + 'static,
    {
        let pool = Arc::new(ProviderPool { connect, providers: Mutex::default() });
        let create: CreateProviderFn = Box::new(move |model, config| {
            let (pool, model) = (Arc::clone(&pool), model.to_string());
            Box::pin(async move {
                let provider = pool.connect(config).await?;
                Ok(Box::new(with_model(provider, &model)) as Box<dyn StreamingModelProvider>)
            })
        });
        self.factories.insert(name.to_string(), create);
        self
    }

    fn register_sync<T: StreamingModelProvider + Clone + 'static>(
        self,
        name: &str,
        connect: impl Fn(ProviderConnectionConfig) -> Result<T> + Send + Sync + 'static,
        with_model: fn(T, &str) -> T,
    ) -> Self {
        self.register(name, move |config| ready(connect(config)), with_model)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[tokio::test]
    async fn test_parse_llamacpp() {
        let result = parse("llamacpp").await;
        assert!(result.is_ok());
        let (_, model) = result.unwrap();
        assert_eq!(model, LlmModel::LlamaCpp(String::new()));
    }

    #[tokio::test]
    async fn test_parse_anthropic() {
        let result = parse("anthropic:claude-sonnet-4-6").await;
        match result {
            Ok((_, model)) => {
                assert_eq!(model, LlmModel::Anthropic(crate::catalog::AnthropicModel::ClaudeSonnet46));
            }
            Err(e) => {
                let err = e.to_string();
                assert!(
                    err.contains("API")
                        || err.contains("ANTHROPIC")
                        || err.contains("credentials")
                        || err.contains("JSON"),
                    "Should fail on API key or credentials, not parsing. Got: {err}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_parse_ollama() {
        let result = parse("ollama:llama3.2").await;
        assert!(result.is_ok());
        let (_, model) = result.unwrap();
        assert_eq!(model, LlmModel::Ollama("llama3.2".to_string()));
    }

    #[tokio::test]
    async fn test_parse_openai() {
        let result = parse("openai:gpt-4.1").await;
        if let Err(e) = result {
            let err = e.to_string();
            assert!(err.contains("API") || err.contains("OPENAI"), "Should fail on API key, not parsing. Got: {err}");
        }
    }

    #[tokio::test]
    async fn test_parse_openrouter() {
        let result = parse("openrouter:google/gemini-2.5-flash").await;
        if let Err(e) = result {
            let err = e.to_string();
            assert!(err.contains("API") || err.contains("OPENROUTER"), "Should fail on API key, not parsing");
        }
    }

    #[tokio::test]
    async fn test_parse_gemini() {
        let result = parse("gemini:gemini-2.5-flash").await;
        if let Err(e) = result {
            let err = e.to_string();
            assert!(err.contains("API") || err.contains("GEMINI"), "Should fail on API key, not parsing");
        }
    }

    #[tokio::test]
    async fn test_parse_provider_without_model() {
        let result = parse("anthropic").await;
        assert!(result.is_err());
    }

    #[cfg(feature = "bedrock")]
    #[tokio::test]
    async fn test_parse_rejects_bedrock_inference_profile_arn() {
        let spec = "bedrock:arn:aws:bedrock:us-west-2:000000000000:inference-profile/us.anthropic.claude-opus-4-7";

        let error = match parse(spec).await {
            Ok(_) => panic!("Bedrock ARN should be rejected"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("providers.bedrock.inferenceProfileArn"), "{error}");
    }

    #[cfg(feature = "bedrock")]
    #[tokio::test]
    async fn test_parse_rejects_bedrock_application_inference_profile_arn() {
        let spec = "bedrock:arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000";

        let error = match parse(spec).await {
            Ok(_) => panic!("Bedrock ARN should be rejected"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("providers.bedrock.inferenceProfileArn"), "{error}");
    }

    #[tokio::test]
    async fn test_parse_rejects_repeated_request_model_provider() {
        let parser = ModelProviderParser::default();
        let connections = ProviderConnectionOverrides::new(BTreeMap::from([(
            "azure-foundry".to_string(),
            crate::ProviderConnectionOverride {
                base_url: Some("http://127.0.0.1:8787".to_string()),
                auth_mode: Some(crate::ProviderAuthMode::None),
                request_model: Some("production-coding".to_string()),
                inference_profile_arn: None,
                idle_timeout_secs: None,
            },
        )]));

        let Err(error) = parser.parse("azure-foundry:gpt-5.5,azure-foundry:gpt-5.5", &connections).await else {
            panic!("repeated request-model provider should be rejected");
        };
        assert!(error.to_string().contains("providers.azure-foundry.requestModel"));
    }

    #[tokio::test]
    async fn test_parse_unknown_provider() {
        let result = parse("unknown:model").await;
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("Unknown provider"));
        }
    }

    #[tokio::test]
    async fn test_parse_single_provider() {
        let result = parse("llamacpp").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_parse_multiple_providers() {
        let result = parse("llamacpp,ollama:llama3.2").await;
        assert!(result.is_ok());
        let (_, model) = result.unwrap();
        assert_eq!(model, LlmModel::LlamaCpp(String::new()));
    }

    #[tokio::test]
    async fn test_parse_with_spaces() {
        let result = parse("llamacpp , ollama:llama3.2").await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_parser_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ModelProviderParser>();
    }

    async fn parse(spec: &str) -> Result<(Box<dyn StreamingModelProvider>, LlmModel)> {
        ModelProviderParser::default().parse(spec, &ProviderConnectionOverrides::default()).await
    }
}
