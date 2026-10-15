#![doc = include_str!("../README.md")]

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::Path;

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

/// Provider configuration for codegen (catalog providers with known model lists)
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
    fallback_reasoning_levels: &'static [&'static str],
    /// When true, a model's `provider.api`/`provider.shape` metadata is read as a
    /// per-model transport override. Off elsewhere because most providers publish
    /// unrelated data (npm package names) under the same key.
    use_model_transport: bool,
    /// When true, the provider uses the shared OpenAI-compatible chat transport.
    uses_openai_compatible_api: bool,
    /// When true, the inner catalog enum is named `{Enum}FoundationModel` and
    /// `LlmModel::{Enum}` carries a hand-written `{Enum}Model` wrapper (defined
    /// outside of codegen) that adds a `Profile(String)` fall-through plus any
    /// provider-specific parsing policy. Used for Bedrock to accept arbitrary
    /// inference profile IDs at runtime while keeping ARNs out of model identity.
    is_hybrid_dynamic: bool,
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
            fallback_reasoning_levels: &["low", "medium", "high"],
            use_model_transport: false,
            uses_openai_compatible_api: false,
            is_hybrid_dynamic: false,
        }
    }

    const fn openai_compatible(
        dev_id: &'static str,
        enum_name: &'static str,
        parser_name: &'static str,
        display_name: &'static str,
        env_var: &'static str,
    ) -> Self {
        let mut config = Self::standard(dev_id, enum_name, parser_name, display_name, Some(env_var));
        config.uses_openai_compatible_api = true;
        config
    }

    /// Inner catalog-enum name. For hybrid providers the outer `{enum_name}Model`
    /// is a wrapper; the catalog enum is `{enum_name}FoundationModel`.
    fn inner_enum_name(&self) -> String {
        if self.is_hybrid_dynamic {
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

/// Dynamic provider — model name is user-supplied at runtime, no fixed enum
#[allow(clippy::struct_field_names)]
struct DynamicProviderConfig {
    /// Rust variant name in `LlmModel` (e.g. "Ollama")
    enum_name: &'static str,
    /// Parser name used in "provider:model" strings (e.g. "ollama")
    parser_name: &'static str,
    /// OpenTelemetry `GenAI` semantic-convention provider name.
    genai_provider_name: &'static str,
    /// Human-readable provider name (e.g. "Ollama")
    display_name: &'static str,
}

const PROVIDERS: &[ProviderConfig] = &[
    ProviderConfig::standard("anthropic", "Anthropic", "anthropic", "Anthropic", Some("ANTHROPIC_API_KEY")),
    ProviderConfig {
        source_dev_id: Some("azure"),
        genai_provider_name: "azure.ai.openai",
        ..ProviderConfig::openai_compatible(
            "azure-foundry",
            "AzureFoundry",
            "azure-foundry",
            "Microsoft Foundry",
            "AZURE_OPENAI_API_KEY",
        )
    },
    ProviderConfig {
        source_dev_id: Some("openai"),
        explicit_models: Some(CODEX_SUBSCRIPTION_MODELS),
        explicit_context_window: Some(272_000),
        genai_provider_name: "openai",
        oauth_provider_id: Some("codex"),
        fallback_reasoning_levels: &["low", "medium", "high", "xhigh"],
        ..ProviderConfig::standard("codex", "Codex", "codex", "Codex", None)
    },
    ProviderConfig::openai_compatible("deepseek", "DeepSeek", "deepseek", "DeepSeek", "DEEPSEEK_API_KEY"),
    ProviderConfig {
        source_dev_id: Some("fireworks-ai"),
        ..ProviderConfig::openai_compatible("fireworks", "Fireworks", "fireworks", "Fireworks AI", "FIREWORKS_API_KEY")
    },
    ProviderConfig {
        genai_provider_name: "gcp.gemini",
        ..ProviderConfig::standard("google", "Gemini", "gemini", "Gemini", Some("GEMINI_API_KEY"))
    },
    ProviderConfig {
        genai_provider_name: "moonshot_ai",
        ..ProviderConfig::openai_compatible("moonshotai", "Moonshot", "moonshot", "Moonshot", "MOONSHOT_API_KEY")
    },
    ProviderConfig::standard("openai", "Openai", "openai", "OpenAI", Some("OPENAI_API_KEY")),
    ProviderConfig::standard("openrouter", "OpenRouter", "openrouter", "OpenRouter", Some("OPENROUTER_API_KEY")),
    ProviderConfig::standard("xiaomi", "Xiaomi", "xiaomi", "Xiaomi", Some("XIAOMI_API_KEY")),
    ProviderConfig {
        extra_source_ids: &["zai-coding-plan"],
        ..ProviderConfig::openai_compatible("zai", "ZAi", "zai", "ZAI", "ZAI_API_KEY")
    },
    ProviderConfig {
        genai_provider_name: "aws.bedrock",
        use_model_transport: true,
        is_hybrid_dynamic: true,
        ..ProviderConfig::standard("amazon-bedrock", "Bedrock", "bedrock", "AWS Bedrock", None)
    },
];

const DYNAMIC_PROVIDERS: &[DynamicProviderConfig] = &[
    DynamicProviderConfig {
        enum_name: "Ollama",
        parser_name: "ollama",
        genai_provider_name: "ollama",
        display_name: "Ollama",
    },
    DynamicProviderConfig {
        enum_name: "LlamaCpp",
        parser_name: "llamacpp",
        genai_provider_name: "llama.cpp",
        display_name: "LlamaCpp",
    },
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
    reasoning_levels: Vec<String>,
    disabled_support: &'static str,
    input_modalities: Vec<String>,
    pricing: Option<CostData>,
    supports_prompt_caching: bool,
    transport: Option<TransportInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransportInfo {
    OpenAiResponses { base_url_template: String },
}

type ProviderModels = BTreeMap<&'static str, Vec<ModelInfo>>;

struct CodegenCtx {
    provider_models: ProviderModels,
}

/// Output of the code generator.
pub struct GeneratedOutput {
    /// The generated Rust source (for `generated.rs`).
    pub rust_source: String,
    /// Provider documentation keys for the shared OpenAI-compatible module.
    pub openai_compatible_provider_ids: Vec<&'static str>,
    /// Per-provider markdown documentation keyed by provider identifier.
    ///
    /// Keys are provider `dev_ids` (e.g. `"anthropic"`, `"ollama"`) and values
    /// are markdown strings suitable for `#![doc = include_str!(...)]`.
    pub provider_docs: HashMap<String, String>,
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

/// Run the codegen, returning the generated Rust source and per-provider docs.
pub fn generate(models_json_path: &Path) -> Result<GeneratedOutput, CodegenError> {
    let json_bytes = std::fs::read_to_string(models_json_path)?;
    let data: ModelsDevData = serde_json::from_str(&json_bytes)?;

    let provider_models = build_provider_models(&data)?;
    let ctx = CodegenCtx { provider_models };
    let openai_compatible_provider_ids =
        PROVIDERS.iter().filter(|config| config.uses_openai_compatible_api).map(|config| config.dev_id).collect();
    Ok(GeneratedOutput {
        rust_source: emit_generated_source(&ctx),
        openai_compatible_provider_ids,
        provider_docs: emit_provider_docs(&ctx),
    })
}

fn build_provider_models(data: &ModelsDevData) -> Result<ProviderModels, CodegenError> {
    let mut provider_models = ProviderModels::new();

    for cfg in PROVIDERS {
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

        if cfg.dev_id == "amazon-bedrock" {
            for model in &mut models {
                if (model.model_id.starts_with("us.openai.gpt-6-") || model.model_id == "us.openai.gpt-6.1-sol")
                    && model.transport.is_none()
                {
                    model.transport = Some(TransportInfo::OpenAiResponses {
                        base_url_template: "https://bedrock-runtime.${AWS_REGION}.amazonaws.com/openai/v1".to_string(),
                    });
                }
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
            let reasoning_levels =
                if m.reasoning.unwrap_or(false) { reasoning_levels_for_model(cfg, m)? } else { Vec::new() };
            let input_modalities =
                m.modalities.as_ref().map_or_else(|| vec!["text".to_string()], |md| md.input.clone());
            let context_window =
                cfg.explicit_context_window.unwrap_or_else(|| m.limit.as_ref().map_or(0, |l| l.context));
            let pricing = if cfg.dev_id == "codex" { None } else { m.cost.clone() };
            Ok(ModelInfo {
                variant_name: model_id_to_variant(&m.id),
                model_id: m.id.clone(),
                display_name: m.name.clone(),
                context_window,
                reasoning_levels,
                disabled_support: disabled_support_for_model(cfg, m),
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
    let Some(provider) = &model.provider else {
        return Ok(None);
    };

    match (&provider.api, provider.shape.as_deref()) {
        (None, None) => Ok(None),
        (Some(base_url_template), Some("responses")) => {
            Ok(Some(TransportInfo::OpenAiResponses { base_url_template: base_url_template.clone() }))
        }
        (_, Some(shape)) if shape != "responses" => {
            Err(CodegenError::UnsupportedWireShape { model_id: model.id.clone(), shape: shape.to_string() })
        }
        _ => Err(CodegenError::IncompleteTransport { model_id: model.id.clone() }),
    }
}

fn disabled_support_for_model(cfg: &ProviderConfig, model: &ModelData) -> &'static str {
    if !model.reasoning.unwrap_or(false) || cfg.explicit_models.is_some() {
        return "Unsupported";
    }
    if model.reasoning_options.iter().any(|option| {
        matches!(option,
            ReasoningOption::Effort { values } if values.iter().any(|value| value.as_deref() == Some("none"))
        )
    }) {
        "Effort"
    } else if model.reasoning_options.iter().any(|option| matches!(option, ReasoningOption::Toggle)) {
        "Toggle"
    } else {
        "Unsupported"
    }
}

fn reasoning_levels_for_model(cfg: &ProviderConfig, model: &ModelData) -> Result<Vec<String>, CodegenError> {
    let values = model
        .reasoning_options
        .iter()
        .filter_map(|option| match option {
            ReasoningOption::Effort { values } => Some(values),
            ReasoningOption::Toggle | ReasoningOption::BudgetTokens => None,
        })
        .collect::<Vec<_>>();
    let mut levels = Vec::new();
    if values.is_empty() {
        levels.extend(cfg.fallback_reasoning_levels.iter().map(|level| (*level).to_string()));
    } else {
        for effort in values.into_iter().flatten().filter_map(|value| value.as_deref()) {
            if matches!(effort, "none" | "default") {
                continue;
            }
            let parsed =
                effort.parse::<utils::ReasoningEffort>().ok().filter(|effort| effort.is_enabled()).ok_or_else(
                    || CodegenError::UnsupportedReasoningEffort {
                        model_id: model.id.clone(),
                        effort: effort.to_string(),
                    },
                )?;
            levels.push(parsed.as_str().to_string());
        }
    }
    if disabled_support_for_model(cfg, model) != "Unsupported" {
        levels.push("disabled".to_string());
    }
    Ok(utils::ReasoningEffort::selectable_levels()
        .iter()
        .filter(|level| levels.iter().any(|value| value == level.as_str()))
        .map(|level| level.as_str().to_string())
        .collect())
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
    let variants: Vec<_> = PROVIDERS
        .iter()
        .map(|cfg| cfg.enum_name)
        .chain(DYNAMIC_PROVIDERS.iter().map(|d| d.enum_name))
        .map(|name| format_ident!("{name}"))
        .collect();
    let catalog_info = PROVIDERS.iter().map(|cfg| {
        let (parser_name, genai_provider_name, display_name) =
            (cfg.parser_name, cfg.genai_provider_name, cfg.display_name);
        let required_env_var = option_tokens(cfg.env_var);
        let oauth_provider_id = option_tokens(cfg.oauth_provider_id);
        let reasoning_off = if cfg.uses_openai_compatible_api {
            format_ident!("Effort")
        } else if cfg.use_model_transport {
            format_ident!("ModelTransport")
        } else {
            format_ident!("Always")
        };
        quote! {
            ProviderInfo {
                parser_name: #parser_name,
                genai_provider_name: #genai_provider_name,
                display_name: #display_name,
                required_env_var: #required_env_var,
                oauth_provider_id: #oauth_provider_id,
                is_local: false,
                reasoning_off: ReasoningOff::#reasoning_off,
            }
        }
    });
    let dynamic_info = DYNAMIC_PROVIDERS.iter().map(|d| {
        let (parser_name, genai_provider_name, display_name) = (d.parser_name, d.genai_provider_name, d.display_name);
        quote! {
            ProviderInfo {
                parser_name: #parser_name,
                genai_provider_name: #genai_provider_name,
                display_name: #display_name,
                required_env_var: None,
                oauth_provider_id: None,
                is_local: true,
                reasoning_off: ReasoningOff::Never,
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

            const INFO: &[ProviderInfo] = &[#(#catalog_info,)* #(#dynamic_info,)*];
        }
    }
}

fn emit_catalog_enums(provider_models: &ProviderModels) -> TokenStream {
    let enums = PROVIDERS.iter().map(|cfg| {
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
    let reasoning_levels = model.reasoning_levels.iter().map(|level| format_ident!("{}", level_str_to_variant(level)));
    let disabled_support = format_ident!("{}", model.disabled_support);
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
            quote! { ModelTransport::OpenAiResponses { base_url_template: #base_url_template } }
        }
    }));
    quote! {
        CatalogEntry {
            id: #id,
            display_name: #display_name,
            context_window: #context_window,
            reasoning_levels: &[#(ReasoningEffort::#reasoning_levels),*],
            disabled_support: ReasoningDisabledSupport::#disabled_support,
            supports_prompt_caching: #supports_prompt_caching,
            pricing: #pricing,
            supports_image: #supports_image,
            supports_audio: #supports_audio,
            transport: #transport,
        }
    }
}

fn emit_llm_model() -> TokenStream {
    let catalog_variants = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        let outer = format_ident!("{}", cfg.outer_enum_name());
        quote! { #v(#outer) }
    });
    let dynamic_variants = DYNAMIC_PROVIDERS.iter().map(|d| format_ident!("{}", d.enum_name));
    let from_impls = PROVIDERS.iter().map(|cfg| {
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
    let env_vars = PROVIDERS.iter().filter_map(|cfg| cfg.env_var);
    let pushes = PROVIDERS.iter().map(|cfg| {
        let inner = format_ident!("{}", cfg.inner_enum_name());
        let outer = format_ident!("{}", cfg.outer_enum_name());
        let v = format_ident!("{}", cfg.enum_name);
        if cfg.is_hybrid_dynamic {
            quote! { v.extend(#inner::ALL.iter().copied().map(#outer::Foundation).map(LlmModel::#v)); }
        } else {
            quote! { v.extend(#inner::ALL.iter().copied().map(LlmModel::#v)); }
        }
    });
    let provider_arms =
        PROVIDERS.iter().map(|cfg| cfg.enum_name).chain(DYNAMIC_PROVIDERS.iter().map(|d| d.enum_name)).map(|name| {
            let v = format_ident!("{name}");
            quote! { Self::#v(_) => Provider::#v, }
        });
    let source_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        if cfg.is_hybrid_dynamic {
            quote! { Self::#v(m) => m.source(), }
        } else {
            quote! { Self::#v(m) => ModelSource::Catalog(m.entry()), }
        }
    });
    let dynamic_pats = DYNAMIC_PROVIDERS.iter().map(|d| {
        let v = format_ident!("{}", d.enum_name);
        quote! { Self::#v(id) }
    });

    quote! {
        /// A model from a specific provider
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum LlmModel {
            #(#catalog_variants,)*
            #(#dynamic_variants(String),)*
        }

        #(#from_impls)*

        impl LlmModel {
            /// All provider API key env var names (deduplicated, static)
            pub const ALL_REQUIRED_ENV_VARS: &[&str] = &[#(#env_vars),*];

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
                    #(#dynamic_pats)|* => ModelSource::Runtime(id),
                }
            }
        }
    }
}

/// Emit `Some(value)` or `None` (`quote` emits a bare value or nothing for an `Option`).
fn option_tokens(value: Option<impl ToTokens>) -> TokenStream {
    value.map_or_else(|| quote! { None }, |value| quote! { Some(#value) })
}

/// Map a reasoning level string to its `ReasoningEffort` variant name
/// (the serialized name with the first letter capitalized).
fn level_str_to_variant(level: &str) -> String {
    let canonical =
        level.parse::<utils::ReasoningEffort>().unwrap_or_else(|_| panic!("Unknown reasoning level: {level}")).as_str();
    let mut variant = canonical.to_string();
    variant[..1].make_ascii_uppercase();
    variant
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

fn emit_provider_docs(ctx: &CodegenCtx) -> HashMap<String, String> {
    let mut docs = HashMap::new();

    for cfg in PROVIDERS {
        let models = &ctx.provider_models[cfg.dev_id];
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
                pushln(
                    &mut doc,
                    "Uses the default AWS credential chain (environment variables, config files, IAM roles).",
                );
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
            let reasoning = if model.reasoning_levels.iter().any(|level| level != "disabled") { "yes" } else { "" };
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

        for model in models.iter().filter(|model| model.disabled_support != "Unsupported") {
            pushln(
                &mut doc,
                format!("Model `{}` advertises `disabled` reasoning (subject to adapter support).", model.model_id),
            );
        }
        push_transport_section(&mut doc, models);

        docs.insert(cfg.dev_id.to_string(), doc);
    }

    for dyn_cfg in DYNAMIC_PROVIDERS {
        let mut doc = String::new();
        pushln(&mut doc, format!("`{}` LLM provider.", dyn_cfg.display_name));
        blank(&mut doc);
        pushln(
            &mut doc,
            format!("This provider accepts any model name at runtime (e.g. `{}:my-model`).", dyn_cfg.parser_name),
        );
        pushln(&mut doc, "No API key is required.");
        docs.insert(dyn_cfg.parser_name.to_string(), doc);
    }

    docs
}

/// Document the models that do not use the provider's default endpoint.
fn push_transport_section(doc: &mut String, models: &[ModelInfo]) {
    let overridden: Vec<&ModelInfo> = models.iter().filter(|m| m.transport.is_some()).collect();
    if overridden.is_empty() {
        return;
    }

    blank(doc);
    pushln(doc, "# Models with a dedicated endpoint");
    blank(doc);
    pushln(doc, "These models are served from their own endpoint and wire protocol");
    pushln(doc, "rather than the provider's default. `${VAR}` placeholders are resolved");
    pushln(doc, "at request time.");
    blank(doc);
    pushln(doc, "| Model ID | Endpoint | Wire shape |");
    pushln(doc, "|----------|----------|------------|");
    for model in overridden {
        let transport = model.transport.as_ref().expect("filtered to models with a transport");
        let (api, shape) = match transport {
            TransportInfo::OpenAiResponses { base_url_template } => (base_url_template.as_str(), "responses"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use serde_json::json;
    use tempfile::NamedTempFile;

    #[test]
    fn model_id_to_variant_pascal_cases_segments() {
        assert_eq!(model_id_to_variant("claude-sonnet-4-5-20250929"), "ClaudeSonnet4520250929");
        assert_eq!(model_id_to_variant("gemini-2.5-flash"), "Gemini25Flash");
        assert_eq!(model_id_to_variant("deepseek-chat"), "DeepseekChat");
        assert_eq!(model_id_to_variant("glm-4.5"), "Glm45");
    }

    #[test]
    fn model_id_to_variant_handles_slash_and_colon() {
        assert_eq!(model_id_to_variant("anthropic/claude-opus-4.6"), "AnthropicClaudeOpus46");
        assert_eq!(model_id_to_variant("openai/gpt-5.1-codex-max"), "OpenaiGpt51CodexMax");
        assert_eq!(model_id_to_variant("deepseek/deepseek-r1:free"), "DeepseekDeepseekR1Free");
    }

    #[test]
    fn is_alias_detects_latest_suffix() {
        assert!(is_alias("claude-sonnet-4-5-latest"));
        assert!(is_alias("claude-3-7-sonnet-latest"));
        assert!(!is_alias("claude-sonnet-4-5-20250929"));
    }

    #[test]
    fn build_uses_explicit_context_windows_for_codex_models() {
        let data = minimal_models_dev_json();

        let models = build_from_value(&data);
        let window = |id: &str| models["codex"].iter().find(|model| model.model_id == id).unwrap().context_window;
        for model_id in [
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
        ] {
            assert_eq!(window(model_id), 272_000);
        }
    }

    #[test]
    fn transport_override_is_preserved_from_model_metadata() {
        let mut data = minimal_models_dev_json();
        insert_models(
            &mut data,
            "amazon-bedrock",
            json!({
                "with-transport": {
                    "id": "with-transport", "name": "With Transport", "tool_call": true,
                    "limit": {"context": 1000, "output": 0},
                    "provider": {
                        "npm": "@ai-sdk/amazon-bedrock/mantle",
                        "api": "https://example.${AWS_REGION}.api.aws/openai/v1",
                        "shape": "responses"
                    }
                },
                "without-transport": {
                    "id": "without-transport", "name": "Without Transport", "tool_call": true,
                    "limit": {"context": 1000, "output": 0}
                }
            }),
        );

        let models = build_from_value(&data);
        let transport =
            |id: &str| models["amazon-bedrock"].iter().find(|m| m.model_id == id).unwrap().transport.clone();

        assert_eq!(
            transport("with-transport"),
            Some(TransportInfo::OpenAiResponses {
                base_url_template: "https://example.${AWS_REGION}.api.aws/openai/v1".to_string(),
            })
        );
        assert_eq!(transport("without-transport"), None);
    }

    #[test]
    fn transport_override_with_only_an_npm_package_is_ignored() {
        let mut data = minimal_models_dev_json();
        anthropic_models(
            &mut data,
            json!({
                "npm-only": {
                    "id": "npm-only", "name": "Npm Only", "tool_call": true,
                    "limit": {"context": 1000, "output": 0},
                    "provider": {"npm": "@ai-sdk/anthropic"}
                }
            }),
        );

        let models = build_from_value(&data);

        assert_eq!(models["anthropic"].iter().find(|m| m.model_id == "npm-only").unwrap().transport, None);
    }

    #[test]
    fn unknown_wire_shape_is_rejected() {
        let mut data = minimal_models_dev_json();
        insert_models(
            &mut data,
            "amazon-bedrock",
            json!({
                "weird": {
                    "id": "weird", "name": "Weird", "tool_call": true,
                    "limit": {"context": 1000, "output": 0},
                    "provider": {"api": "https://example.com/v1", "shape": "telepathy"}
                }
            }),
        );
        let parsed: ModelsDevData = serde_json::from_value(data).expect("parse fixture");

        let error = build_provider_models(&parsed).unwrap_err();

        assert!(
            matches!(error, CodegenError::UnsupportedWireShape { ref model_id, ref shape }
                if model_id == "weird" && shape == "telepathy"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn incomplete_bedrock_transport_is_rejected() {
        let mut data = minimal_models_dev_json();
        insert_models(
            &mut data,
            "amazon-bedrock",
            json!({
                "incomplete": {
                    "id": "incomplete", "name": "Incomplete", "tool_call": true,
                    "limit": {"context": 1000, "output": 0},
                    "provider": {"api": "https://example.com/v1"}
                }
            }),
        );
        let parsed: ModelsDevData = serde_json::from_value(data).expect("parse fixture");

        let error = build_provider_models(&parsed).unwrap_err();

        assert!(matches!(error, CodegenError::IncompleteTransport { ref model_id } if model_id == "incomplete"));
    }

    #[test]
    fn format_context_window_formats_correctly() {
        assert_eq!(format_context_window(1_000_000), "1M");
        assert_eq!(format_context_window(200_000), "200k");
        assert_eq!(format_context_window(8_000), "8k");
        assert_eq!(format_context_window(0), "unknown");
    }

    #[test]
    fn level_str_to_variant_covers_all_reasoning_efforts() {
        for effort in utils::ReasoningEffort::all() {
            let _ = level_str_to_variant(effort.as_str());
        }
    }

    #[test]
    fn build_sorts_models_and_filters_aliases_and_non_tool_call() {
        let mut data = minimal_models_dev_json();
        anthropic_models(
            &mut data,
            json!({
                "b-model": {"id": "b-model", "name": "B Model", "tool_call": true, "limit": {"context": 2000, "output": 0}},
                "a-model": {"id": "a-model", "name": "A Model", "tool_call": true, "limit": {"context": 1000, "output": 0}},
                "alpha-latest": {"id": "alpha-latest", "name": "Alias", "tool_call": true, "limit": {"context": 500, "output": 0}},
                "no-tools": {"id": "no-tools", "name": "No Tools", "tool_call": false, "limit": {"context": 500, "output": 0}}
            }),
        );

        let models = build_from_value(&data);
        let ids: Vec<&str> = models["anthropic"].iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, vec!["a-model", "b-model"]);
    }

    #[test]
    fn build_extra_source_ids_merges_unique_models_into_provider() {
        let mut data = minimal_models_dev_json();
        zai_extra_models(
            &mut data,
            json!({
                "extra-model": {"id": "extra-model", "name": "Extra Model", "tool_call": true, "limit": {"context": 4000, "output": 0}}
            }),
        );

        let models = build_from_value(&data);
        assert!(models["zai"].iter().any(|m| m.model_id == "extra-model"));
    }

    #[test]
    fn build_extra_source_ids_does_not_duplicate_existing_models() {
        let mut data = minimal_models_dev_json();
        let shared = json!({
            "shared-model": {"id": "shared-model", "name": "Shared Model", "tool_call": true, "limit": {"context": 1000, "output": 0}}
        });
        insert_models(&mut data, "zai", shared.clone());
        insert_models(&mut data, "zai-coding-plan", shared);

        let models = build_from_value(&data);
        let count = models["zai"].iter().filter(|m| m.model_id == "shared-model").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn build_derives_reasoning_levels_from_source_metadata() {
        let mut data = minimal_models_dev_json();
        anthropic_models(
            &mut data,
            json!({
                "claude-test": {
                    "id": "claude-test", "name": "Claude Test", "tool_call": true, "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["low", "high", "max"]}],
                    "limit": {"context": 200_000, "output": 0}
                }
            }),
        );

        let models = build_from_value(&data);
        let model = models["anthropic"].iter().find(|model| model.model_id == "claude-test").unwrap();
        assert_eq!(model.reasoning_levels, ["low", "high", "max"]);
    }

    #[test]
    fn generate_rejects_unknown_reasoning_effort_metadata() {
        let mut data = minimal_models_dev_json();
        anthropic_models(
            &mut data,
            json!({
                "claude-test": {
                    "id": "claude-test", "name": "Claude Test", "tool_call": true, "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["ultra"]}],
                    "limit": {"context": 200_000, "output": 0}
                }
            }),
        );
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), serde_json::to_string(&data).unwrap()).unwrap();
        assert!(matches!(generate(tmp.path()), Err(CodegenError::UnsupportedReasoningEffort { .. })));
    }

    #[test]
    fn build_preserves_model_pricing_and_omits_codex_subscription_pricing() {
        let mut data = minimal_models_dev_json();
        anthropic_models(
            &mut data,
            json!({
                "priced": {
                    "id": "priced", "name": "Priced", "tool_call": true,
                    "limit": {"context": 200_000, "output": 0},
                    "cost": {"input": 3.0, "output": 15.0, "cache_read": 0.3, "cache_write": 3.75}
                }
            }),
        );
        insert_models(
            &mut data,
            "openai",
            json!({
                "gpt-5.5": {
                    "id": "gpt-5.5", "name": "GPT-5.5", "tool_call": true,
                    "limit": {"context": 1_050_000, "output": 128_000},
                    "cost": {"input": 1.25, "output": 10.0, "cache_read": 0.125}
                }
            }),
        );

        let models = build_from_value(&data);
        let priced = models["anthropic"].iter().find(|model| model.model_id == "priced").unwrap();
        assert_eq!(priced.pricing.as_ref().map(|pricing| pricing.input), Some(3.0));
        assert_eq!(priced.pricing.as_ref().map(|pricing| pricing.output), Some(15.0));
        assert_eq!(priced.pricing.as_ref().and_then(|pricing| pricing.cache_read), Some(0.3));
        assert_eq!(priced.pricing.as_ref().and_then(|pricing| pricing.cache_write), Some(3.75));

        let codex = models["codex"].iter().find(|model| model.model_id == "gpt-5.5").unwrap();
        assert_eq!(codex.pricing, None);
        assert!(codex.supports_prompt_caching);
    }

    #[test]
    fn build_derives_prompt_caching_from_cost_fields() {
        let mut data = minimal_models_dev_json();
        insert_models(
            &mut data,
            "amazon-bedrock",
            json!({
                "cached": {
                    "id": "cached", "name": "Cached", "tool_call": true,
                    "limit": {"context": 200_000, "output": 0},
                    "cost": {"input": 3.0, "output": 15.0, "cache_read": 0.3, "cache_write": 3.75}
                },
                "uncached": {
                    "id": "uncached", "name": "Uncached", "tool_call": true,
                    "limit": {"context": 200_000, "output": 0},
                    "cost": {"input": 3.0, "output": 15.0}
                }
            }),
        );

        let models = build_from_value(&data);
        let bedrock = &models["amazon-bedrock"];
        let cached = bedrock.iter().find(|m| m.model_id == "cached").unwrap();
        let uncached = bedrock.iter().find(|m| m.model_id == "uncached").unwrap();
        assert!(cached.supports_prompt_caching);
        assert!(!uncached.supports_prompt_caching);
    }

    #[test]
    fn build_assigns_codex_model_specific_reasoning_levels() {
        let mut data = minimal_models_dev_json();
        insert_models(
            &mut data,
            "openai",
            json!({
                "gpt-5.6-sol": {
                    "id": "gpt-5.6-sol", "name": "GPT-5.6 Sol", "tool_call": true, "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["none", "low", "medium", "high", "xhigh", "max"]}],
                    "limit": {"context": 200_000, "output": 0}
                },
                "gpt-5.6-luna": {
                    "id": "gpt-5.6-luna", "name": "GPT-5.6 Luna", "tool_call": true, "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["none", "low", "medium", "high", "xhigh", "max"]}],
                    "limit": {"context": 200_000, "output": 0}
                },
                "gpt-5.4": {
                    "id": "gpt-5.4", "name": "GPT-5.4", "tool_call": true, "reasoning": true,
                    "limit": {"context": 200_000, "output": 0}
                }
            }),
        );

        let models = build_from_value(&data);
        let levels = |id: &str| models["codex"].iter().find(|m| m.model_id == id).unwrap().reasoning_levels.clone();
        assert_eq!(levels("gpt-5.6-sol"), vec!["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(levels("gpt-5.6-luna"), vec!["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(levels("gpt-5.4"), vec!["low", "medium", "high", "xhigh"]);
    }

    #[test]
    fn build_applies_codex_subscription_context_window_override() {
        let mut data = minimal_models_dev_json();
        insert_models(
            &mut data,
            "openai",
            json!({
                "gpt-5.5": {
                    "id": "gpt-5.5", "name": "GPT-5.5", "tool_call": true, "reasoning": true,
                    "limit": {"context": 1_050_000, "output": 128_000}
                }
            }),
        );

        let models = build_from_value(&data);
        let codex = models["codex"].iter().find(|m| m.model_id == "gpt-5.5").unwrap();
        let openai = models["openai"].iter().find(|m| m.model_id == "gpt-5.5").unwrap();
        assert_eq!(codex.context_window, 272_000);
        assert_eq!(openai.context_window, 1_050_000);
    }

    #[test]
    fn generate_uses_codex_subscription_model_ids() {
        let mut data = minimal_models_dev_json();
        insert_models(
            &mut data,
            "openai",
            json!({
                "gpt-6.1-sol": {
                    "id": "gpt-6.1-sol", "name": "GPT-6.1 Sol", "tool_call": true, "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high", "xhigh", "max"]}],
                    "limit": {"context": 1_050_000, "output": 128_000}
                },
                "gpt-5.1-codex": {
                    "id": "gpt-5.1-codex", "name": "GPT-5.1 Codex", "tool_call": true, "reasoning": true,
                    "limit": {"context": 400_000, "output": 128_000}
                },
                "gpt-5.6": {
                    "id": "gpt-5.6", "name": "GPT-5.6 Sol", "tool_call": true, "reasoning": true,
                    "limit": {"context": 1_050_000, "output": 128_000}
                },
                "gpt-5.6-sol": {
                    "id": "gpt-5.6-sol", "name": "GPT-5.6 Sol", "tool_call": true, "reasoning": true,
                    "limit": {"context": 1_050_000, "output": 128_000}
                },
                "gpt-5.6-terra": {
                    "id": "gpt-5.6-terra", "name": "GPT-5.6 Terra", "tool_call": true, "reasoning": true,
                    "limit": {"context": 1_050_000, "output": 128_000}
                },
                "gpt-5.6-luna": {
                    "id": "gpt-5.6-luna", "name": "GPT-5.6 Luna", "tool_call": true, "reasoning": true,
                    "limit": {"context": 1_050_000, "output": 128_000}
                }
            }),
        );

        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), serde_json::to_string(&data).unwrap()).unwrap();
        let output = generate(tmp.path()).unwrap();

        let codex_doc = &output.provider_docs["codex"];
        assert!(!codex_doc.contains("`gpt-5.6`"));
        assert!(!codex_doc.contains("`gpt-5.1-codex`"));
        assert!(codex_doc.contains("| `gpt-6.1-sol` | `GPT-6.1 Sol` | `272k` |"));
        assert!(codex_doc.contains("| `gpt-5.6-sol` | `GPT-5.6 Sol` | `272k` |"));
        assert!(codex_doc.contains("| `gpt-5.6-terra` | `GPT-5.6 Terra` | `272k` |"));
        assert!(codex_doc.contains("| `gpt-5.6-luna` | `GPT-5.6 Luna` | `272k` |"));

        let openai_doc = &output.provider_docs["openai"];
        assert!(openai_doc.contains("`gpt-5.6`"));
        assert!(openai_doc.contains("`gpt-5.1-codex`"));
        assert!(openai_doc.contains("`gpt-5.6-sol`"));
    }

    #[test]
    fn generate_emits_provider_docs() {
        let mut data = minimal_models_dev_json();
        anthropic_models(
            &mut data,
            json!({
                "claude-test": {
                    "id": "claude-test", "name": "Claude Test", "tool_call": true, "reasoning": true,
                    "limit": {"context": 200_000, "output": 0},
                    "modalities": {"input": ["text", "image"]}
                }
            }),
        );

        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), serde_json::to_string(&data).unwrap()).unwrap();
        let output = generate(tmp.path()).unwrap();

        let anthropic_doc = &output.provider_docs["anthropic"];
        assert!(anthropic_doc.contains("`Anthropic` LLM provider."));
        assert!(anthropic_doc.contains("`ANTHROPIC_API_KEY`"));
        assert!(anthropic_doc.contains("| `claude-test` | `Claude Test` | `200k` | yes | yes |  |"));

        let ollama_doc = &output.provider_docs["ollama"];
        assert!(ollama_doc.contains("`Ollama` LLM provider."));
        assert!(ollama_doc.contains("any model name at runtime"));
    }

    #[test]
    fn generate_routes_only_existing_us_gpt_6_profiles_to_runtime() {
        let mut data = minimal_models_dev_json();
        let mut models = serde_json::Map::new();
        for version in ["6-luna", "6-sol", "6-astra", "6.1-sol"] {
            for prefix in ["us", "global"] {
                let id = format!("{prefix}.openai.gpt-{version}");
                models.insert(
                    id.clone(),
                    json!({
                        "id": id, "name": format!("GPT-{version} ({prefix})"), "tool_call": true,
                        "limit": {"context": 1_050_000, "output": 128_000}
                    }),
                );
            }
        }
        models.insert(
            "openai.gpt-6-astra".to_string(),
            json!({
                "id": "openai.gpt-6-astra", "name": "GPT-6 Astra", "tool_call": true,
                "provider": {"api": "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1", "shape": "responses"}
            }),
        );
        insert_models(&mut data, "amazon-bedrock", serde_json::Value::Object(models));
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), serde_json::to_string(&data).unwrap()).unwrap();
        let docs = generate(tmp.path()).unwrap().provider_docs.remove("amazon-bedrock").unwrap();

        for version in ["6-luna", "6-sol", "6-astra", "6.1-sol"] {
            let id = format!("openai.gpt-{version}");
            assert!(docs.contains(&format!(
                "| `us.{id}` | `https://bedrock-runtime.${{AWS_REGION}}.amazonaws.com/openai/v1` | `responses` |"
            )));
            assert!(!docs.contains(&format!("| `global.{id}` | `https://bedrock-runtime.")));
        }
        assert!(docs.contains(
            "| `openai.gpt-6-astra` | `https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1` | `responses` |"
        ));
        assert!(!docs.contains("| `openai.gpt-6-luna` |"));
        assert!(!docs.contains("| `openai.gpt-6-sol` |"));
    }

    #[test]
    fn generate_preserves_disabled_separately_from_default() {
        let mut data = minimal_models_dev_json();
        data["openai"]["models"]["gpt-5.4"]["reasoning_options"] = json!([
            {"type": "effort", "values": [null, "default", "none", "low", "high"]}
        ]);
        anthropic_models(
            &mut data,
            json!({
                "claude-toggle": {
                    "id": "claude-toggle", "name": "Toggle", "tool_call": true, "reasoning": true,
                    "reasoning_options": [{"type": "toggle"}, {"type": "effort", "values": ["low", "high"]}]
                }
            }),
        );
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), serde_json::to_string(&data).unwrap()).unwrap();
        let output = generate(tmp.path()).unwrap();
        assert!(output.rust_source.contains("ReasoningEffort::Disabled"));
        assert_default_is_not_capability(&output.rust_source);
        assert!(output.provider_docs["openai"].contains("disabled"));
        assert!(output.provider_docs["anthropic"].contains("disabled"));
        assert!(!output.provider_docs["codex"].contains("disabled"));
    }

    #[test]
    fn generate_reasoning_capability_edge_cases() {
        let mut data = minimal_models_dev_json();
        for (id, reasoning, options, disabled) in [
            ("toggle", true, json!([{"type": "toggle"}]), true),
            (
                "both",
                true,
                json!([{"type": "toggle"}, {"type": "effort", "values": ["none", "high", "none", "low", "low"]}]),
                true,
            ),
            ("defaults", true, json!([{"type": "effort", "values": [null, "default"]}]), false),
            ("budget", true, json!([{"type": "budget_tokens"}]), false),
            ("missing", true, json!([]), false),
            ("plain", false, json!([{"type": "toggle"}]), false),
        ] {
            data["anthropic"]["models"] = json!({id: {"id": id, "name": id, "reasoning": reasoning, "tool_call": true, "reasoning_options": options}});
            let tmp = NamedTempFile::new().unwrap();
            std::fs::write(tmp.path(), serde_json::to_string(&data).unwrap()).unwrap();
            let output = generate(tmp.path()).unwrap();
            assert_eq!(output.provider_docs["anthropic"].contains("advertises `disabled`"), disabled, "{id}");
            assert_default_is_not_capability(&output.rust_source);
            if id == "both" {
                assert!(output.rust_source.contains("ReasoningDisabledSupport::Effort"));
                assert!(!output.rust_source.contains("ReasoningEffort::Disabled, ReasoningEffort::Disabled"));
            }
        }
    }

    fn assert_default_is_not_capability(source: &str) {
        assert!(!source.contains("ReasoningEffort::Default"));
    }

    fn build_from_value(data: &Value) -> ProviderModels {
        let parsed: ModelsDevData = serde_json::from_value(data.clone()).expect("parse fixture");
        build_provider_models(&parsed).expect("build provider models")
    }

    fn anthropic_models(data: &mut Value, models: Value) {
        insert_models(data, "anthropic", models);
    }

    fn zai_extra_models(data: &mut Value, models: Value) {
        insert_models(data, "zai-coding-plan", models);
    }

    fn insert_models(data: &mut Value, provider_key: &str, models: Value) {
        let provider = data.as_object_mut().unwrap().get_mut(provider_key).unwrap().as_object_mut().unwrap();
        let target = provider.get_mut("models").unwrap().as_object_mut().unwrap();
        let Value::Object(models) = models else {
            panic!("models fixture must be an object");
        };
        target.extend(models);
    }

    fn minimal_models_dev_json() -> Value {
        let mut root = serde_json::Map::new();
        for cfg in PROVIDERS {
            let json_key = cfg.json_key();
            root.entry(json_key.to_string()).or_insert_with(|| json!({"models": {}}));
            for &extra in cfg.extra_source_ids {
                root.entry(extra.to_string()).or_insert_with(|| json!({"models": {}}));
            }
        }
        let openai = root.get_mut("openai").unwrap()["models"].as_object_mut().unwrap();
        for &id in CODEX_SUBSCRIPTION_MODELS {
            openai.insert(
                id.to_string(),
                json!({
                    "id": id,
                    "name": id,
                    "tool_call": true,
                    "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high", "xhigh"]}],
                    "limit": {"context": 1_050_000, "output": 0}
                }),
            );
        }
        Value::Object(root)
    }
}
