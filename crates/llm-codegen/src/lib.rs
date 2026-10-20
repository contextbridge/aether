#![doc = include_str!("../README.md")]

use proc_macro2::{Ident, TokenStream};
use quote::{ToTokens, format_ident, quote};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::Path;
use utils::ReasoningEffort;

/// Output of the code generator.
pub struct GeneratedOutput {
    /// The generated Rust source (for `generated.rs`).
    pub rust_source: String,
    /// Per-provider markdown documentation keyed by provider identifier.
    ///
    /// Keys are provider `dev_ids` (e.g. `"anthropic"`, `"ollama"`) and values
    /// are markdown strings suitable for `#![doc = include_str!(...)]`.
    pub provider_docs: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("read: {0}")]
    Read(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("Provider '{0}' not found in models.dev data")]
    ProviderNotFound(String),
    #[error("Configured model '{model_id}' was not found in provider '{provider_id}'")]
    ConfiguredModelNotFound { provider_id: String, model_id: String },
    #[error("Configured model '{model_id}' is duplicated for provider '{provider_id}'")]
    DuplicateConfiguredModel { provider_id: String, model_id: String },
    #[error("Configured model '{model_id}' is not tool-capable in provider '{provider_id}'")]
    ConfiguredModelUnavailable { provider_id: String, model_id: String },
    #[error("Model '{model_id}' declares unsupported reasoning effort '{effort}'")]
    UnsupportedReasoningEffort { model_id: String, effort: String },
    #[error("Model '{model_id}' declares unsupported wire shape '{shape}'")]
    UnsupportedWireShape { model_id: String, shape: String },
    #[error("Model '{model_id}' must declare both an endpoint and wire shape")]
    IncompleteTransport { model_id: String },
}

pub fn generate(models_json_path: &Path) -> Result<GeneratedOutput, CodegenError> {
    let json_bytes = std::fs::read_to_string(models_json_path)?;
    let data: ModelsDevData = serde_json::from_str(&json_bytes)?;

    let provider_models = build_provider_models(&data)?;
    let ctx = CodegenCtx { provider_models };
    Ok(GeneratedOutput { rust_source: emit_generated_source(&ctx), provider_docs: emit_provider_docs(&ctx) })
}

type ModelsDevData = HashMap<String, ProviderData>;

#[derive(Debug, Deserialize)]
struct ProviderData {
    #[serde(default)]
    models: HashMap<String, ModelData>,
}

#[derive(Debug, Deserialize)]
struct ModelData {
    id: String,
    name: String,
    #[serde(default)]
    tool_call: Option<bool>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    reasoning_options: Vec<ReasoningOption>,
    #[serde(default)]
    cost: Option<CostData>,
    #[serde(default)]
    limit: Option<LimitData>,
    #[serde(default)]
    modalities: Option<ModalitiesData>,
    #[serde(default)]
    provider: Option<ModelProviderData>,
}

/// Per-model transport override.
#[derive(Debug, Deserialize)]
struct ModelProviderData {
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    shape: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReasoningOption {
    Effort { values: Vec<Option<String>> },
    Toggle,
    BudgetTokens,
}

#[derive(Debug, Deserialize, Default)]
struct ModalitiesData {
    #[serde(default)]
    input: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct CostData {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    cache_read: Option<f64>,
    #[serde(default)]
    cache_write: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct LimitData {
    #[serde(default)]
    context: u32,
}

impl CostData {
    fn has_prompt_caching(&self) -> bool {
        self.cache_read.is_some() || self.cache_write.is_some()
    }
}

/// Provider configuration for codegen, for catalog and local providers alike.
struct ProviderConfig {
    /// Unique provider key used in `provider_models` map (e.g. "codex")
    dev_id: &'static str,
    /// models.dev provider ID to read models from (defaults to `dev_id` when `None`)
    source_dev_id: Option<&'static str>,
    /// Additional models.dev keys whose models are merged into this provider
    extra_source_ids: &'static [&'static str],
    /// When set, the provider exposes only these models (e.g. subscription-gated providers).
    explicit_models: Option<&'static [&'static str]>,
    /// Context window applied to every model.
    explicit_context_window: Option<u32>,
    /// Our Rust enum name (e.g. "Gemini")
    enum_name: &'static str,
    /// Our internal provider name used for parsing (e.g. "gemini")
    parser_name: &'static str,
    /// OpenTelemetry `GenAI` semantic-convention provider name.
    genai_provider_name: &'static str,
    /// Human-readable provider name (e.g. "AWS Bedrock")
    display_name: &'static str,
    /// Env var our code actually checks (None for providers with complex credential chains)
    env_var: Option<&'static str>,
    /// OAuth provider ID for providers that require OAuth login (e.g. "codex")
    oauth_provider_id: Option<&'static str>,
    /// Fallback levels when source metadata does not declare granular efforts.
    fallback_reasoning_levels: &'static [ReasoningEffort],
    /// Which models can turn reasoning off.
    reasoning_off: ReasoningOff,
    /// When false (subscription-billed providers), models carry no pricing.
    per_token_pricing: bool,
    /// When true, a model's `provider.api`/`provider.shape` metadata is read as a
    /// per-model transport override. Off elsewhere because most providers publish
    /// unrelated data (npm package names) under the same key.
    use_model_transport: bool,
    /// Transports for models whose metadata declares none.
    transport_overrides: &'static [TransportOverride],
    model_ids: ModelIds,
}

/// How `LlmModel::{Enum}` identifies a provider's models.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelIds {
    /// A generated `{Enum}Model` catalog enum.
    Catalog,
    /// A generated `{Enum}FoundationModel` catalog enum inside a hand-written
    /// `{Enum}Model` wrapper (defined outside of codegen) that adds a
    /// `Profile(String)` fall-through plus any provider-specific parsing policy.
    /// Used for Bedrock to accept arbitrary inference profile IDs at runtime
    /// while keeping ARNs out of model identity.
    Hybrid,
    /// A model name supplied at runtime (`LlmModel::{Enum}(String)`), with no
    /// catalog enum.
    Runtime,
}

/// Mirrors `ReasoningOff` in `llm::catalog`.
#[derive(Debug, Clone, Copy)]
enum ReasoningOff {
    Always,
    Effort,
    ModelTransport,
    Never,
}

/// Mirrors `llm::ReasoningDisabledSupport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasoningDisabledSupport {
    Unsupported,
    Effort,
    Toggle,
}

/// Mirrors `llm::ReasoningControl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasoningControl {
    Effort,
    Budget,
    BudgetAndEffort,
}

struct TransportOverride {
    matches: ModelMatch,
    transport: TransportInfo,
}

enum ModelMatch {
    Prefixes(&'static [&'static str]),
    /// IDs containing the text anywhere, so inference-profile IDs such as
    /// `us.anthropic.claude-…` match along with their foundation model.
    Contains(&'static str),
}

impl TransportOverride {
    fn for_model(&self, model_id: &str) -> Option<TransportInfo> {
        let matches = match self.matches {
            ModelMatch::Prefixes(prefixes) => prefixes.iter().any(|prefix| model_id.starts_with(prefix)),
            ModelMatch::Contains(text) => model_id.contains(text),
        };
        matches.then(|| self.transport.clone())
    }
}

impl ProviderConfig {
    /// Shorthand for providers with default `source_dev_id`, `explicit_models`, and `oauth_provider_id`.
    const fn standard(
        dev_id: &'static str,
        enum_name: &'static str,
        parser_name: &'static str,
        display_name: &'static str,
        env_var: Option<&'static str>,
    ) -> Self {
        Self {
            dev_id,
            source_dev_id: None,
            extra_source_ids: &[],
            explicit_models: None,
            explicit_context_window: None,
            enum_name,
            parser_name,
            genai_provider_name: parser_name,
            display_name,
            env_var,
            oauth_provider_id: None,
            fallback_reasoning_levels: &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High],
            reasoning_off: ReasoningOff::Always,
            per_token_pricing: true,
            use_model_transport: false,
            transport_overrides: &[],
            model_ids: ModelIds::Catalog,
        }
    }

    const fn local(
        enum_name: &'static str,
        parser_name: &'static str,
        genai_provider_name: &'static str,
        display_name: &'static str,
    ) -> Self {
        Self {
            genai_provider_name,
            reasoning_off: ReasoningOff::Never,
            model_ids: ModelIds::Runtime,
            ..Self::standard(parser_name, enum_name, parser_name, display_name, None)
        }
    }

    fn is_local(&self) -> bool {
        self.model_ids == ModelIds::Runtime
    }

    /// Inner catalog-enum name. For hybrid providers the outer `{enum_name}Model`
    /// is a wrapper; the catalog enum is `{enum_name}FoundationModel`.
    fn inner_enum_name(&self) -> String {
        if self.model_ids == ModelIds::Hybrid {
            format!("{}FoundationModel", self.enum_name)
        } else {
            format!("{}Model", self.enum_name)
        }
    }

    /// Outer enum name as referenced by `LlmModel::{enum_name}(...)`.
    fn outer_enum_name(&self) -> String {
        format!("{}Model", self.enum_name)
    }

    /// The models.dev key to look up in the JSON data.
    fn json_key(&self) -> &'static str {
        self.source_dev_id.unwrap_or(self.dev_id)
    }
}

const PROVIDERS: &[ProviderConfig] = &[
    ProviderConfig::standard("anthropic", "Anthropic", "anthropic", "Anthropic", Some("ANTHROPIC_API_KEY")),
    ProviderConfig {
        source_dev_id: Some("azure"),
        genai_provider_name: "azure.ai.openai",
        reasoning_off: ReasoningOff::Effort,
        ..ProviderConfig::standard(
            "azure-foundry",
            "AzureFoundry",
            "azure-foundry",
            "Microsoft Foundry",
            Some("AZURE_OPENAI_API_KEY"),
        )
    },
    ProviderConfig {
        source_dev_id: Some("openai"),
        explicit_models: Some(CODEX_SUBSCRIPTION_MODELS),
        explicit_context_window: Some(272_000),
        genai_provider_name: "openai",
        oauth_provider_id: Some("codex"),
        fallback_reasoning_levels: &[
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
        ],
        per_token_pricing: false,
        ..ProviderConfig::standard("codex", "Codex", "codex", "Codex", None)
    },
    ProviderConfig {
        reasoning_off: ReasoningOff::Effort,
        ..ProviderConfig::standard("deepseek", "DeepSeek", "deepseek", "DeepSeek", Some("DEEPSEEK_API_KEY"))
    },
    ProviderConfig {
        source_dev_id: Some("fireworks-ai"),
        reasoning_off: ReasoningOff::Effort,
        ..ProviderConfig::standard("fireworks", "Fireworks", "fireworks", "Fireworks AI", Some("FIREWORKS_API_KEY"))
    },
    ProviderConfig {
        genai_provider_name: "gcp.gemini",
        ..ProviderConfig::standard("google", "Gemini", "gemini", "Gemini", Some("GEMINI_API_KEY"))
    },
    ProviderConfig {
        genai_provider_name: "moonshot_ai",
        reasoning_off: ReasoningOff::Effort,
        ..ProviderConfig::standard("moonshotai", "Moonshot", "moonshot", "Moonshot", Some("MOONSHOT_API_KEY"))
    },
    ProviderConfig::standard("openai", "Openai", "openai", "OpenAI", Some("OPENAI_API_KEY")),
    ProviderConfig::standard("openrouter", "OpenRouter", "openrouter", "OpenRouter", Some("OPENROUTER_API_KEY")),
    ProviderConfig::standard("xiaomi", "Xiaomi", "xiaomi", "Xiaomi", Some("XIAOMI_API_KEY")),
    ProviderConfig {
        extra_source_ids: &["zai-coding-plan"],
        reasoning_off: ReasoningOff::Effort,
        ..ProviderConfig::standard("zai", "ZAi", "zai", "ZAI", Some("ZAI_API_KEY"))
    },
    ProviderConfig {
        genai_provider_name: "aws.bedrock",
        reasoning_off: ReasoningOff::ModelTransport,
        use_model_transport: true,
        transport_overrides: &[
            TransportOverride {
                matches: ModelMatch::Prefixes(&["us.openai.gpt-6-", "us.openai.gpt-6.1-sol"]),
                transport: TransportInfo::OpenAiResponses {
                    base_url_template: Cow::Borrowed("https://bedrock-runtime.${AWS_REGION}.amazonaws.com/openai/v1"),
                },
            },
            TransportOverride {
                matches: ModelMatch::Contains("anthropic.claude-"),
                transport: TransportInfo::AnthropicMessages,
            },
        ],
        model_ids: ModelIds::Hybrid,
        ..ProviderConfig::standard("amazon-bedrock", "Bedrock", "bedrock", "AWS Bedrock", None)
    },
    ProviderConfig::local("Ollama", "ollama", "ollama", "Ollama"),
    ProviderConfig::local("LlamaCpp", "llamacpp", "llama.cpp", "LlamaCpp"),
];

const CODEX_SUBSCRIPTION_MODELS: &[&str] = &[
    "gpt-6.1-sol",
    "gpt-6-sol",
    "gpt-6-astra",
    "gpt-6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.2",
];

#[derive(Debug, Clone)]
struct ModelInfo {
    variant_name: String,
    model_id: String,
    display_name: String,
    context_window: u32,
    reasoning: ReasoningMetadata,
    input_modalities: Vec<String>,
    pricing: Option<CostData>,
    supports_prompt_caching: bool,
    transport: Option<TransportInfo>,
}

#[derive(Debug, Clone)]
struct ReasoningMetadata {
    levels: Vec<ReasoningEffort>,
    disabled_support: ReasoningDisabledSupport,
    control: ReasoningControl,
}

#[derive(Default)]
struct ListedReasoningOptions<'a> {
    efforts: Option<Vec<&'a str>>,
    effort_none: bool,
    toggle: bool,
    budget_tokens: bool,
}

impl<'a> ListedReasoningOptions<'a> {
    fn new(options: &'a [ReasoningOption]) -> Self {
        let mut listed = Self::default();
        for option in options {
            match option {
                ReasoningOption::Effort { values } => {
                    let efforts = listed.efforts.get_or_insert_with(Vec::new);
                    for value in values.iter().flatten() {
                        match value.as_str() {
                            "none" => listed.effort_none = true,
                            "default" => {}
                            effort => efforts.push(effort),
                        }
                    }
                }
                ReasoningOption::Toggle => listed.toggle = true,
                ReasoningOption::BudgetTokens => listed.budget_tokens = true,
            }
        }
        listed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransportInfo {
    OpenAiResponses { base_url_template: Cow<'static, str> },
    AnthropicMessages,
}

type ProviderModels = BTreeMap<&'static str, Vec<ModelInfo>>;

struct CodegenCtx {
    provider_models: ProviderModels,
}

/// Providers with a fixed model list read from models.dev.
fn catalog_providers() -> impl Iterator<Item = &'static ProviderConfig> {
    PROVIDERS.iter().filter(|cfg| !cfg.is_local())
}

fn build_provider_models(data: &ModelsDevData) -> Result<ProviderModels, CodegenError> {
    let mut provider_models = ProviderModels::new();

    for cfg in catalog_providers() {
        let json_key = cfg.json_key();
        let provider_data = data.get(json_key).ok_or_else(|| CodegenError::ProviderNotFound(json_key.to_string()))?;

        validate_provider_config(cfg, provider_data)?;
        let mut models: Vec<ModelInfo> = collect_models_from(cfg, &provider_data.models)?;

        for &extra_key in cfg.extra_source_ids {
            if let Some(extra_data) = data.get(extra_key) {
                let extra = collect_models_from(cfg, &extra_data.models)?;
                let existing_ids: std::collections::HashSet<String> =
                    models.iter().map(|m| m.model_id.clone()).collect();
                models.extend(extra.into_iter().filter(|m| !existing_ids.contains(&m.model_id)));
            }
        }

        models.sort_by(|a, b| a.model_id.cmp(&b.model_id));
        provider_models.insert(cfg.dev_id, models);
    }

    Ok(provider_models)
}

fn validate_provider_config(cfg: &ProviderConfig, provider: &ProviderData) -> Result<(), CodegenError> {
    let Some(explicit_models) = cfg.explicit_models else {
        return Ok(());
    };
    let mut seen = HashSet::new();
    for &model_id in explicit_models {
        if !seen.insert(model_id) {
            return Err(CodegenError::DuplicateConfiguredModel {
                provider_id: cfg.dev_id.to_string(),
                model_id: model_id.to_string(),
            });
        }
        let Some(model) = provider.models.get(model_id) else {
            return Err(CodegenError::ConfiguredModelNotFound {
                provider_id: cfg.dev_id.to_string(),
                model_id: model_id.to_string(),
            });
        };
        if model.tool_call != Some(true) {
            return Err(CodegenError::ConfiguredModelUnavailable {
                provider_id: cfg.dev_id.to_string(),
                model_id: model_id.to_string(),
            });
        }
    }
    Ok(())
}

fn collect_models_from(
    cfg: &ProviderConfig,
    models: &HashMap<String, ModelData>,
) -> Result<Vec<ModelInfo>, CodegenError> {
    models
        .values()
        .filter(|m| m.tool_call == Some(true))
        .filter(|m| !is_alias(&m.id))
        .filter(|m| cfg.explicit_models.is_none_or(|ids| ids.contains(&m.id.as_str())))
        .map(|m| {
            let input_modalities =
                m.modalities.as_ref().map_or_else(|| vec!["text".to_string()], |md| md.input.clone());
            let context_window =
                cfg.explicit_context_window.unwrap_or_else(|| m.limit.as_ref().map_or(0, |l| l.context));
            let pricing = m.cost.clone().filter(|_| cfg.per_token_pricing);
            Ok(ModelInfo {
                variant_name: model_id_to_variant(&m.id),
                model_id: m.id.clone(),
                display_name: m.name.clone(),
                context_window,
                reasoning: reasoning_for_model(cfg, m)?,
                input_modalities,
                supports_prompt_caching: m.cost.as_ref().is_some_and(CostData::has_prompt_caching),
                pricing,
                transport: transport_for_model(cfg, m)?,
            })
        })
        .collect()
}

fn transport_for_model(cfg: &ProviderConfig, model: &ModelData) -> Result<Option<TransportInfo>, CodegenError> {
    if !cfg.use_model_transport {
        return Ok(None);
    }
    let (api, shape) =
        model.provider.as_ref().map_or((None, None), |provider| (provider.api.as_ref(), provider.shape.as_deref()));

    match (api, shape) {
        (None, None) => Ok(cfg.transport_overrides.iter().find_map(|transport| transport.for_model(&model.id))),
        (Some(base_url_template), Some("responses")) => {
            Ok(Some(TransportInfo::OpenAiResponses { base_url_template: Cow::Owned(base_url_template.clone()) }))
        }
        (_, Some(shape)) if shape != "responses" => {
            Err(CodegenError::UnsupportedWireShape { model_id: model.id.clone(), shape: shape.to_string() })
        }
        _ => Err(CodegenError::IncompleteTransport { model_id: model.id.clone() }),
    }
}

fn reasoning_for_model(cfg: &ProviderConfig, model: &ModelData) -> Result<ReasoningMetadata, CodegenError> {
    let listed = ListedReasoningOptions::new(&model.reasoning_options);
    let reasoning = model.reasoning.unwrap_or(false);
    let disabled_support = if !reasoning || cfg.explicit_models.is_some() {
        ReasoningDisabledSupport::Unsupported
    } else if listed.effort_none {
        ReasoningDisabledSupport::Effort
    } else if listed.toggle {
        ReasoningDisabledSupport::Toggle
    } else {
        ReasoningDisabledSupport::Unsupported
    };

    let control = match (listed.budget_tokens, listed.efforts.as_ref().is_some_and(|efforts| !efforts.is_empty())) {
        (true, true) => ReasoningControl::BudgetAndEffort,
        (true, false) => ReasoningControl::Budget,
        (false, _) => ReasoningControl::Effort,
    };

    let levels = if reasoning { reasoning_levels(cfg, model, listed.efforts, disabled_support)? } else { Vec::new() };
    Ok(ReasoningMetadata { levels, disabled_support, control })
}

fn reasoning_levels(
    cfg: &ProviderConfig,
    model: &ModelData,
    efforts: Option<Vec<&str>>,
    disabled_support: ReasoningDisabledSupport,
) -> Result<Vec<ReasoningEffort>, CodegenError> {
    let mut levels = match efforts {
        None => cfg.fallback_reasoning_levels.to_vec(),
        Some(efforts) => efforts
            .into_iter()
            .map(|effort| {
                effort.parse::<ReasoningEffort>().ok().filter(|effort| effort.is_enabled()).ok_or_else(|| {
                    CodegenError::UnsupportedReasoningEffort { model_id: model.id.clone(), effort: effort.to_string() }
                })
            })
            .collect::<Result<_, _>>()?,
    };
    if disabled_support != ReasoningDisabledSupport::Unsupported {
        levels.push(ReasoningEffort::Disabled);
    }
    Ok(ReasoningEffort::selectable_levels().iter().copied().filter(|level| levels.contains(level)).collect())
}

/// Returns true for "latest" alias IDs that just point to another model
fn is_alias(id: &str) -> bool {
    id.ends_with("-latest")
}

/// Convert a model ID like "claude-sonnet-4-5-20250929" into a `PascalCase` variant name.
/// Treats `-`, `.`, `/`, and `:` as word separators.
fn model_id_to_variant(id: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = true;

    for ch in id.chars() {
        if ch == '-' || ch == '.' || ch == '/' || ch == ':' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }

    if result.starts_with(|c: char| c.is_ascii_digit()) {
        result.insert(0, '_');
    }

    result
}

fn emit_generated_source(ctx: &CodegenCtx) -> String {
    let provider_enum = emit_provider_enum();
    let catalog_enums = emit_catalog_enums(&ctx.provider_models);
    let llm_model = emit_llm_model();

    let file_tokens = quote! {
        #provider_enum
        #catalog_enums
        #llm_model
    };

    let file: syn::File = syn::parse2(file_tokens).expect("generated tokens parse as Rust");
    let formatted = prettyplease::unparse(&file);
    format!(
        "// Auto-generated from models.dev — do not edit manually\n// Regenerated automatically by build.rs\n\n{formatted}"
    )
}

fn emit_provider_enum() -> TokenStream {
    let variants: Vec<_> = PROVIDERS.iter().map(|cfg| format_ident!("{}", cfg.enum_name)).collect();
    let info = PROVIDERS.iter().map(|cfg| {
        let (parser_name, genai_provider_name, display_name, is_local) =
            (cfg.parser_name, cfg.genai_provider_name, cfg.display_name, cfg.is_local());
        let required_env_var = option_tokens(cfg.env_var);
        let oauth_provider_id = option_tokens(cfg.oauth_provider_id);
        let reasoning_off = variant_ident(cfg.reasoning_off);
        quote! {
            ProviderInfo {
                parser_name: #parser_name,
                genai_provider_name: #genai_provider_name,
                display_name: #display_name,
                required_env_var: #required_env_var,
                oauth_provider_id: #oauth_provider_id,
                is_local: #is_local,
                reasoning_off: ReasoningOff::#reasoning_off,
            }
        }
    });

    quote! {
        /// Typed provider identifier — covers both catalog providers
        /// (`Anthropic`, `Codex`, …) and dynamic providers whose model name is
        /// user-supplied (`Ollama`, `LlamaCpp`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Provider {
            #(#variants,)*
        }

        impl Provider {
            /// All providers — catalog and dynamic — in declaration order.
            pub const ALL: &[Provider] = &[#(Self::#variants),*];

            const INFO: &[ProviderInfo] = &[#(#info,)*];
        }
    }
}

fn emit_catalog_enums(provider_models: &ProviderModels) -> TokenStream {
    let enums = catalog_providers().map(|cfg| {
        let inner = format_ident!("{}", cfg.inner_enum_name());
        let models = &provider_models[cfg.dev_id];
        let variants: Vec<_> = models.iter().map(|m| format_ident!("{}", m.variant_name)).collect();
        let entries = models.iter().map(emit_catalog_entry);
        quote! {
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum #inner {
                #(#variants,)*
            }

            impl #inner {
                const ALL: &[Self] = &[#(Self::#variants),*];

                #[allow(clippy::large_stack_arrays, clippy::unreadable_literal)]
                const ENTRIES: &[CatalogEntry] = &[#(#entries,)*];

                fn entry(self) -> &'static CatalogEntry {
                    &Self::ENTRIES[self as usize]
                }
            }
        }
    });
    quote! { #(#enums)* }
}

fn emit_catalog_entry(model: &ModelInfo) -> TokenStream {
    let (id, display_name, supports_prompt_caching) =
        (&model.model_id, &model.display_name, model.supports_prompt_caching);
    let context_window = num_lit_with_underscores(model.context_window);
    let reasoning_levels = model.reasoning.levels.iter().copied().map(variant_ident);
    let disabled_support = variant_ident(model.reasoning.disabled_support);
    let reasoning_control = variant_ident(model.reasoning.control);
    let pricing = option_tokens(model.pricing.as_ref().map(|pricing| {
        let (input, output) = (pricing.input, pricing.output);
        let (cache_read, cache_write) = (option_tokens(pricing.cache_read), option_tokens(pricing.cache_write));
        quote! {
            ModelPricing {
                input_per_million: #input,
                output_per_million: #output,
                cache_read_per_million: #cache_read,
                cache_write_per_million: #cache_write,
            }
        }
    }));
    let supports_image = model.input_modalities.iter().any(|modality| modality == "image");
    let supports_audio = model.input_modalities.iter().any(|modality| modality == "audio");
    let transport = option_tokens(model.transport.as_ref().map(|transport| match transport {
        TransportInfo::OpenAiResponses { base_url_template } => {
            let base_url_template = base_url_template.as_ref();
            quote! { ModelTransport::OpenAiResponses { base_url_template: #base_url_template } }
        }
        TransportInfo::AnthropicMessages => quote! { ModelTransport::AnthropicMessages },
    }));
    quote! {
        CatalogEntry {
            id: #id,
            display_name: #display_name,
            context_window: #context_window,
            reasoning_levels: &[#(ReasoningEffort::#reasoning_levels),*],
            disabled_support: ReasoningDisabledSupport::#disabled_support,
            supports_prompt_caching: #supports_prompt_caching,
            reasoning_control: ReasoningControl::#reasoning_control,
            pricing: #pricing,
            supports_image: #supports_image,
            supports_audio: #supports_audio,
            transport: #transport,
        }
    }
}

fn emit_llm_model() -> TokenStream {
    let variants = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        let outer = format_ident!("{}", cfg.outer_enum_name());
        match cfg.model_ids {
            ModelIds::Catalog | ModelIds::Hybrid => quote! { #v(#outer) },
            ModelIds::Runtime => quote! { #v(String) },
        }
    });
    let from_impls = catalog_providers().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        let outer = format_ident!("{}", cfg.outer_enum_name());
        quote! {
            impl From<#outer> for LlmModel {
                fn from(m: #outer) -> Self {
                    LlmModel::#v(m)
                }
            }
        }
    });
    let pushes = PROVIDERS.iter().filter_map(|cfg| {
        let inner = format_ident!("{}", cfg.inner_enum_name());
        let outer = format_ident!("{}", cfg.outer_enum_name());
        let v = format_ident!("{}", cfg.enum_name);
        match cfg.model_ids {
            ModelIds::Catalog => Some(quote! { v.extend(#inner::ALL.iter().copied().map(LlmModel::#v)); }),
            ModelIds::Hybrid => {
                Some(quote! { v.extend(#inner::ALL.iter().copied().map(#outer::Foundation).map(LlmModel::#v)); })
            }
            ModelIds::Runtime => None,
        }
    });
    let provider_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        quote! { Self::#v(_) => Provider::#v, }
    });
    let source_arms = PROVIDERS.iter().filter_map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        match cfg.model_ids {
            ModelIds::Catalog => Some(quote! { Self::#v(m) => ModelSource::Catalog(m.entry()), }),
            ModelIds::Hybrid => Some(quote! { Self::#v(m) => m.source(), }),
            ModelIds::Runtime => None,
        }
    });
    let runtime_pats = PROVIDERS.iter().filter(|cfg| cfg.is_local()).map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        quote! { Self::#v(id) }
    });

    quote! {
        /// A model from a specific provider
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum LlmModel {
            #(#variants,)*
        }

        #(#from_impls)*

        impl LlmModel {
            /// All catalog models (excludes dynamic providers)
            pub fn all() -> &'static [LlmModel] {
                static ALL: LazyLock<Vec<LlmModel>> = LazyLock::new(|| {
                    let mut v = Vec::new();
                    #(#pushes)*
                    v
                });
                &ALL
            }

            /// Typed provider identifier.
            pub fn provider_enum(&self) -> Provider {
                match self { #(#provider_arms)* }
            }

            fn source(&self) -> ModelSource<'_> {
                match self {
                    #(#source_arms)*
                    #(#runtime_pats)|* => ModelSource::Runtime(id),
                }
            }
        }
    }
}

/// Emit `Some(value)` or `None` (`quote` emits a bare value or nothing for an `Option`).
fn option_tokens(value: Option<impl ToTokens>) -> TokenStream {
    value.map_or_else(|| quote! { None }, |value| quote! { Some(#value) })
}

/// Emit the variant name of a fieldless enum whose `Debug` derive prints it.
fn variant_ident(variant: impl std::fmt::Debug) -> Ident {
    format_ident!("{}", format!("{variant:?}"))
}

/// Emit a `u32` literal with underscore separators (e.g. `200_000`).
fn num_lit_with_underscores(n: u32) -> TokenStream {
    format_number(n).parse().expect("formatted number parses as a token")
}

/// Format a number with underscore separators (e.g. `200000` → `200_000`).
fn format_number(n: u32) -> String {
    let s = n.to_string();
    if s.len() <= 4 {
        return s;
    }
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            result.push('_');
        }
        result.push(ch);
    }
    result
}

fn emit_provider_docs(ctx: &CodegenCtx) -> BTreeMap<String, String> {
    PROVIDERS
        .iter()
        .map(|cfg| {
            let doc = if cfg.is_local() {
                local_provider_doc(cfg)
            } else {
                catalog_provider_doc(cfg, &ctx.provider_models[cfg.dev_id])
            };
            (cfg.dev_id.to_string(), doc)
        })
        .collect()
}

fn catalog_provider_doc(cfg: &ProviderConfig, models: &[ModelInfo]) -> String {
    let mut doc = String::new();

    pushln(&mut doc, format!("`{}` LLM provider.", cfg.display_name));
    blank(&mut doc);

    pushln(&mut doc, "# Authentication");
    blank(&mut doc);
    match cfg.env_var {
        Some(var) => pushln(&mut doc, format!("Set the `{var}` environment variable.")),
        None if cfg.oauth_provider_id.is_some() => {
            pushln(&mut doc, "This provider uses OAuth authentication.");
        }
        None => {
            pushln(&mut doc, "Uses the default AWS credential chain (environment variables, config files, IAM roles).");
            pushln(
                &mut doc,
                "Models served from a dedicated endpoint also accept a Bedrock API key in `AWS_BEARER_TOKEN_BEDROCK`.",
            );
        }
    }
    blank(&mut doc);

    pushln(&mut doc, "# Supported models");
    blank(&mut doc);
    pushln(&mut doc, "| Model ID | Name | Context | Reasoning | Image | Audio |");
    pushln(&mut doc, "|----------|------|---------|-----------|-------|-------|");
    for model in models {
        let ctx_str = format_context_window(model.context_window);
        let reasoning = if model.reasoning.levels.iter().any(|level| level.is_enabled()) { "yes" } else { "" };
        let image = if model.input_modalities.contains(&"image".to_string()) { "yes" } else { "" };
        let audio = if model.input_modalities.contains(&"audio".to_string()) { "yes" } else { "" };
        pushln(
            &mut doc,
            format!(
                "| `{}` | `{}` | `{}` | {} | {} | {} |",
                model.model_id, model.display_name, ctx_str, reasoning, image, audio
            ),
        );
    }

    for model in models.iter().filter(|model| model.reasoning.disabled_support != ReasoningDisabledSupport::Unsupported)
    {
        pushln(
            &mut doc,
            format!("Model `{}` advertises `disabled` reasoning (subject to adapter support).", model.model_id),
        );
    }
    push_transport_section(&mut doc, models);
    doc
}

fn local_provider_doc(cfg: &ProviderConfig) -> String {
    let mut doc = String::new();
    pushln(&mut doc, format!("`{}` LLM provider.", cfg.display_name));
    blank(&mut doc);
    pushln(&mut doc, format!("This provider accepts any model name at runtime (e.g. `{}:my-model`).", cfg.parser_name));
    pushln(&mut doc, "No API key is required.");
    doc
}

/// Document the models that do not use the provider's default transport.
fn push_transport_section(doc: &mut String, models: &[ModelInfo]) {
    let overridden: Vec<(&ModelInfo, &TransportInfo)> =
        models.iter().filter_map(|model| Some((model, model.transport.as_ref()?))).collect();
    if overridden.is_empty() {
        return;
    }

    blank(doc);
    pushln(doc, "# Models with a dedicated transport");
    blank(doc);
    pushln(doc, "These models are served by their own API and wire protocol rather than");
    pushln(doc, "the provider's default. `${VAR}` placeholders are resolved at request time.");
    blank(doc);
    pushln(doc, "| Model ID | API | Wire shape |");
    pushln(doc, "|----------|-----|------------|");
    for (model, transport) in overridden {
        let (api, shape) = match transport {
            TransportInfo::OpenAiResponses { base_url_template } => (base_url_template.as_ref(), "responses"),
            TransportInfo::AnthropicMessages => ("InvokeModelWithResponseStream", "messages"),
        };
        pushln(doc, format!("| `{}` | `{api}` | `{shape}` |", model.model_id));
    }
}

/// Format a token count as human-readable (e.g. `1_000_000` → `1M`, `200_000` → `200k`).
fn format_context_window(tokens: u32) -> String {
    if tokens == 0 {
        return "unknown".to_string();
    }
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000 && tokens.is_multiple_of(1_000) {
        format!("{}k", tokens / 1_000)
    } else {
        format_number(tokens)
    }
}

fn pushln(out: &mut String, line: impl AsRef<str>) {
    writeln!(out, "{}", line.as_ref()).expect("writing to String should not fail");
}

fn blank(out: &mut String) {
    pushln(out, "");
}
