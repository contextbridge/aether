#![doc = include_str!("../README.md")]

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::path::Path;

type ModelsDevData = HashMap<String, ProviderData>;
type ContextWindowOverride = fn(&str, u32) -> u32;

#[derive(Debug, Deserialize)]
struct ProviderData {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    env: Vec<String>,
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
    #[allow(dead_code)]
    cost: Option<CostData>,
    #[serde(default)]
    limit: Option<LimitData>,
    #[serde(default)]
    modalities: Option<ModalitiesData>,
}

#[derive(Debug, Deserialize, Default)]
struct ModalitiesData {
    #[serde(default)]
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
    #[serde(default)]
    #[allow(dead_code)]
    output: u32,
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
    /// Only include models whose ID passes this filter (None = include all)
    model_filter: Option<fn(&str) -> bool>,
    /// Provider-specific generated context window override
    context_window_override: Option<ContextWindowOverride>,
    /// Our Rust enum name (e.g. "Gemini")
    enum_name: &'static str,
    /// Our internal provider name used for parsing (e.g. "gemini")
    parser_name: &'static str,
    /// Human-readable provider name (e.g. "AWS Bedrock")
    display_name: &'static str,
    /// Env var our code actually checks (None for providers with complex credential chains)
    env_var: Option<&'static str>,
    /// OAuth provider ID for providers that require OAuth login (e.g. "codex")
    oauth_provider_id: Option<&'static str>,
    /// Default reasoning levels for models that support reasoning (empty = use standard 3)
    default_reasoning_levels: &'static [&'static str],
    /// When true, the inner catalog enum is named `{Enum}FoundationModel` and
    /// `LlmModel::{Enum}` carries a hand-written `{Enum}Model` wrapper (defined
    /// outside of codegen) that adds a `Profile(String)` fall-through plus any
    /// provider-specific parsing policy. Used for Bedrock to accept arbitrary
    /// inference profile IDs at runtime while keeping ARNs out of model identity.
    is_hybrid_dynamic: bool,
}

impl ProviderConfig {
    /// Shorthand for providers with default `source_dev_id`, `model_filter`, and `oauth_provider_id`.
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
            model_filter: None,
            context_window_override: None,
            enum_name,
            parser_name,
            display_name,
            env_var,
            oauth_provider_id: None,
            default_reasoning_levels: &["low", "medium", "high"],
            is_hybrid_dynamic: false,
        }
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
    /// Human-readable provider name (e.g. "Ollama")
    display_name: &'static str,
}

const PROVIDERS: &[ProviderConfig] = &[
    ProviderConfig::standard("anthropic", "Anthropic", "anthropic", "Anthropic", Some("ANTHROPIC_API_KEY")),
    ProviderConfig {
        dev_id: "codex",
        source_dev_id: Some("openai"),
        extra_source_ids: &[],
        model_filter: Some(|id| id.contains("codex") || id.starts_with("gpt-5.") || id == "gpt-5"),
        context_window_override: Some(codex_subscription_context_window),
        enum_name: "Codex",
        parser_name: "codex",
        display_name: "Codex",
        env_var: None,
        oauth_provider_id: Some("codex"),
        default_reasoning_levels: &["low", "medium", "high", "xhigh"],
        is_hybrid_dynamic: false,
    },
    ProviderConfig::standard("deepseek", "DeepSeek", "deepseek", "DeepSeek", Some("DEEPSEEK_API_KEY")),
    ProviderConfig::standard("google", "Gemini", "gemini", "Gemini", Some("GEMINI_API_KEY")),
    ProviderConfig::standard("moonshotai", "Moonshot", "moonshot", "Moonshot", Some("MOONSHOT_API_KEY")),
    ProviderConfig::standard("openai", "Openai", "openai", "OpenAI", Some("OPENAI_API_KEY")),
    ProviderConfig::standard("openrouter", "OpenRouter", "openrouter", "OpenRouter", Some("OPENROUTER_API_KEY")),
    ProviderConfig {
        extra_source_ids: &["zai-coding-plan"],
        ..ProviderConfig::standard("zai", "ZAi", "zai", "ZAI", Some("ZAI_API_KEY"))
    },
    ProviderConfig {
        is_hybrid_dynamic: true,
        ..ProviderConfig::standard("amazon-bedrock", "Bedrock", "bedrock", "AWS Bedrock", None)
    },
];

const DYNAMIC_PROVIDERS: &[DynamicProviderConfig] = &[
    DynamicProviderConfig { enum_name: "Ollama", parser_name: "ollama", display_name: "Ollama" },
    DynamicProviderConfig { enum_name: "LlamaCpp", parser_name: "llamacpp", display_name: "LlamaCpp" },
];

const CODEX_SUBSCRIPTION_CONTEXT_WINDOW: u32 = 272_000;

fn codex_subscription_context_window(model_id: &str, default_context_window: u32) -> u32 {
    match model_id {
        "gpt-5.5" | "gpt-5.4" | "gpt-5.4-mini" | "gpt-5.3-codex" | "gpt-5.2" | "codex-auto-review" => {
            CODEX_SUBSCRIPTION_CONTEXT_WINDOW
        }
        _ => default_context_window,
    }
}

#[derive(Debug, Clone)]
struct ModelInfo {
    variant_name: String,
    model_id: String,
    display_name: String,
    context_window: u32,
    reasoning_levels: Vec<String>,
    input_modalities: Vec<String>,
    supports_prompt_caching: bool,
}

type ProviderModels = BTreeMap<&'static str, Vec<ModelInfo>>;

struct CodegenCtx {
    provider_models: ProviderModels,
}

/// Output of the code generator.
pub struct GeneratedOutput {
    /// The generated Rust source (for `generated.rs`).
    pub rust_source: String,
    /// Per-provider markdown documentation keyed by provider identifier.
    ///
    /// Keys are provider `dev_ids` (e.g. `"anthropic"`, `"ollama"`) and values
    /// are markdown strings suitable for `#![doc = include_str!(...)]`.
    pub provider_docs: HashMap<String, String>,
}

/// Run the codegen, returning the generated Rust source and per-provider docs.
pub fn generate(models_json_path: &Path) -> Result<GeneratedOutput, String> {
    let json_bytes = std::fs::read_to_string(models_json_path).map_err(|e| format!("read: {e}"))?;
    let data: ModelsDevData = serde_json::from_str(&json_bytes).map_err(|e| format!("parse: {e}"))?;

    let provider_models = build_provider_models(&data)?;
    let ctx = CodegenCtx { provider_models };
    Ok(GeneratedOutput { rust_source: emit_generated_source(&ctx), provider_docs: emit_provider_docs(&ctx) })
}

fn build_provider_models(data: &ModelsDevData) -> Result<ProviderModels, String> {
    let mut provider_models = ProviderModels::new();

    for cfg in PROVIDERS {
        let json_key = cfg.json_key();
        let provider_data =
            data.get(json_key).ok_or_else(|| format!("Provider '{json_key}' not found in models.dev data"))?;

        let mut models: Vec<ModelInfo> = collect_models_from(cfg, &provider_data.models);

        for &extra_key in cfg.extra_source_ids {
            if let Some(extra_data) = data.get(extra_key) {
                let extra = collect_models_from(cfg, &extra_data.models);
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

fn collect_models_from(cfg: &ProviderConfig, models: &HashMap<String, ModelData>) -> Vec<ModelInfo> {
    models
        .values()
        .filter(|m| m.tool_call == Some(true))
        .filter(|m| !is_alias(&m.id))
        .filter(|m| cfg.model_filter.is_none_or(|f| f(&m.id)))
        .map(|m| {
            let reasoning_levels = if m.reasoning.unwrap_or(false) {
                cfg.default_reasoning_levels.iter().map(|s| (*s).to_string()).collect()
            } else {
                Vec::new()
            };
            let input_modalities =
                m.modalities.as_ref().map_or_else(|| vec!["text".to_string()], |md| md.input.clone());
            let source_context_window = m.limit.as_ref().map_or(0, |l| l.context);
            let context_window = cfg.context_window_override.map_or(source_context_window, |override_context_window| {
                override_context_window(&m.id, source_context_window)
            });
            ModelInfo {
                variant_name: model_id_to_variant(&m.id),
                model_id: m.id.clone(),
                display_name: m.name.clone(),
                context_window,
                reasoning_levels,
                input_modalities,
                supports_prompt_caching: m.cost.as_ref().is_some_and(CostData::has_prompt_caching),
            }
        })
        .collect()
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
    let provider_enums = emit_provider_enums(&ctx.provider_models);
    let provider_impls = emit_provider_impls(&ctx.provider_models);
    let llm_model_enum = emit_llm_model_enum();
    let from_impls = emit_from_impls();
    let llm_model_impl = emit_llm_model_impl();
    let display_impl = emit_display_impl();
    let fromstr_impl = emit_fromstr_impl();

    let file_tokens = quote! {
        use std::borrow::Cow;
        use std::sync::LazyLock;
        use crate::ReasoningEffort;

        #provider_enums
        #provider_impls
        #llm_model_enum
        #from_impls
        #llm_model_impl
        #display_impl
        #fromstr_impl
    };

    let file: syn::File = syn::parse2(file_tokens).expect("generated tokens parse as Rust");
    let formatted = prettyplease::unparse(&file);
    format!(
        "// Auto-generated from models.dev — do not edit manually\n// Regenerated automatically by build.rs\n\n{formatted}"
    )
}

fn emit_provider_enums(provider_models: &ProviderModels) -> TokenStream {
    let enums = PROVIDERS.iter().map(|cfg| {
        let inner = format_ident!("{}", cfg.inner_enum_name());
        let variants = provider_models[cfg.dev_id].iter().map(|m| format_ident!("{}", m.variant_name));
        quote! {
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum #inner {
                #(#variants,)*
            }
        }
    });
    quote! { #(#enums)* }
}

fn emit_provider_impls(provider_models: &ProviderModels) -> TokenStream {
    let impls = PROVIDERS.iter().map(|cfg| {
        let models = &provider_models[cfg.dev_id];
        let enum_ident = format_ident!("{}", cfg.inner_enum_name());

        let model_id_arms = models.iter().map(|m| {
            let v = format_ident!("{}", m.variant_name);
            let id = &m.model_id;
            quote! { Self::#v => #id, }
        });

        let display_name_arms = grouped_arms(
            models,
            |m| m.display_name.clone(),
            |m| {
                let s = &m.display_name;
                quote! { #s }
            },
        );

        let context_window_arms =
            grouped_arms(models, |m| m.context_window, |m| num_lit_with_underscores(m.context_window));

        let reasoning_levels_arms = emit_reasoning_levels_arms(models);

        let prompt_caching_arms = grouped_arms(
            models,
            |m| m.supports_prompt_caching,
            |m| {
                let b = m.supports_prompt_caching;
                quote! { #b }
            },
        );

        let modality_methods = ["image", "audio"].iter().map(|modality| {
            let method = format_ident!("supports_{}", modality);
            let mod_owned = (*modality).to_string();
            let arms = grouped_arms(models, move |m| m.input_modalities.contains(&mod_owned), {
                let mod_owned = (*modality).to_string();
                move |m| {
                    let b = m.input_modalities.contains(&mod_owned);
                    quote! { #b }
                }
            });
            quote! {
                #[allow(clippy::too_many_lines)]
                pub fn #method(self) -> bool {
                    match self { #arms }
                }
            }
        });

        let all_variants = models.iter().map(|m| format_ident!("{}", m.variant_name));

        let from_str_impl = emit_from_str_impl(&enum_ident, cfg.parser_name, models);

        quote! {
            impl #enum_ident {
                #[allow(clippy::too_many_lines)]
                fn model_id(self) -> &'static str {
                    match self { #(#model_id_arms)* }
                }

                #[allow(clippy::too_many_lines)]
                fn display_name(self) -> &'static str {
                    match self { #display_name_arms }
                }

                #[allow(clippy::too_many_lines)]
                fn context_window(self) -> u32 {
                    match self { #context_window_arms }
                }

                #[allow(clippy::too_many_lines)]
                pub fn reasoning_levels(self) -> &'static [ReasoningEffort] {
                    match self { #reasoning_levels_arms }
                }

                pub fn supports_reasoning(self) -> bool {
                    !self.reasoning_levels().is_empty()
                }

                #[allow(clippy::too_many_lines)]
                pub fn supports_prompt_caching(self) -> bool {
                    match self { #prompt_caching_arms }
                }

                #(#modality_methods)*

                const ALL: &[#enum_ident] = &[#(Self::#all_variants),*];
            }

            #from_str_impl
        }
    });
    quote! { #(#impls)* }
}

fn emit_from_str_impl(enum_ident: &proc_macro2::Ident, parser_name: &str, models: &[ModelInfo]) -> TokenStream {
    let arms = models.iter().map(|m| {
        let id = &m.model_id;
        let v = format_ident!("{}", m.variant_name);
        quote! { #id => Ok(Self::#v), }
    });
    let err_msg = format!("Unknown {parser_name} model: '{{s}}'");
    quote! {
        impl std::str::FromStr for #enum_ident {
            type Err = String;

            #[allow(clippy::too_many_lines)]
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    #(#arms)*
                    _ => Err(format!(#err_msg)),
                }
            }
        }
    }
}

/// Emit match arms grouped by value to avoid clippy `match_same_arms`.
fn grouped_arms<K, R>(
    models: &[ModelInfo],
    key_fn: impl Fn(&ModelInfo) -> K,
    rhs_fn: impl Fn(&ModelInfo) -> R,
) -> TokenStream
where
    K: Eq + Ord,
    R: ToTokens,
{
    let mut groups: BTreeMap<K, Vec<&ModelInfo>> = BTreeMap::new();
    for m in models {
        groups.entry(key_fn(m)).or_default().push(m);
    }
    let arms = groups.values().map(|members| {
        let pats = members.iter().map(|m| {
            let v = format_ident!("{}", m.variant_name);
            quote! { Self::#v }
        });
        let rhs = rhs_fn(members[0]);
        quote! { #(#pats)|* => #rhs, }
    });
    quote! { #(#arms)* }
}

fn emit_reasoning_levels_arms(models: &[ModelInfo]) -> TokenStream {
    grouped_arms(
        models,
        |m| m.reasoning_levels.clone(),
        |m| {
            if m.reasoning_levels.is_empty() {
                quote! { &[] }
            } else {
                let items = m.reasoning_levels.iter().map(|l| {
                    let variant = format_ident!("{}", level_str_to_variant(l));
                    quote! { ReasoningEffort::#variant }
                });
                quote! { &[#(#items),*] }
            }
        },
    )
}

/// Map a reasoning level string to its `ReasoningEffort` variant name.
fn level_str_to_variant(level: &str) -> &'static str {
    match level {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Xhigh",
        other => panic!("Unknown reasoning level: {other}"),
    }
}

fn emit_llm_model_enum() -> TokenStream {
    let catalog_variants = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        let inner = format_ident!("{}Model", cfg.enum_name);
        quote! { #v(#inner) }
    });
    let dynamic_variants = DYNAMIC_PROVIDERS.iter().map(|d| {
        let v = format_ident!("{}", d.enum_name);
        quote! { #v(String) }
    });
    quote! {
        /// A model from a specific provider
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum LlmModel {
            #(#catalog_variants,)*
            #(#dynamic_variants,)*
        }
    }
}

fn emit_from_impls() -> TokenStream {
    let impls = PROVIDERS.iter().map(|cfg| {
        let outer = format_ident!("{}Model", cfg.enum_name);
        let v = format_ident!("{}", cfg.enum_name);
        quote! {
            impl From<#outer> for LlmModel {
                fn from(m: #outer) -> Self {
                    LlmModel::#v(m)
                }
            }
        }
    });
    quote! { #(#impls)* }
}

fn emit_llm_model_impl() -> TokenStream {
    let model_id = emit_llm_model_id();
    let display_name = emit_llm_display_name();
    let provider = emit_llm_provider();
    let provider_display_name = emit_llm_provider_display_name();
    let context_window = emit_llm_context_window();
    let required_env_var = emit_llm_required_env_var();
    let all_required_env_vars = emit_llm_all_required_env_vars();
    let oauth_provider_id = emit_llm_oauth_provider_id();
    let reasoning_levels = emit_llm_reasoning_levels();
    let supports_reasoning = emit_llm_supports_reasoning();
    let supports_prompt_caching = emit_llm_supports_prompt_caching();
    let modality_methods = ["image", "audio"].iter().map(|m| emit_llm_supports_modality(m));
    let all = emit_llm_all();

    quote! {
        impl LlmModel {
            #model_id
            #display_name
            #provider
            #provider_display_name
            #context_window
            #required_env_var
            #all_required_env_vars
            #oauth_provider_id
            #reasoning_levels
            #supports_reasoning
            #supports_prompt_caching
            #(#modality_methods)*
            #all
        }
    }
}

fn emit_llm_model_id() -> TokenStream {
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        if cfg.is_hybrid_dynamic {
            quote! { Self::#v(m) => m.model_id(), }
        } else {
            quote! { Self::#v(m) => Cow::Borrowed(m.model_id()), }
        }
    });
    let dyn_pats = dynamic_pattern_with_binding("s");
    quote! {
        /// Raw model ID (e.g. `claude-opus-4-6`, `llama3.2`)
        pub fn model_id(&self) -> Cow<'static, str> {
            match self {
                #(#catalog_arms)*
                #dyn_pats => Cow::Owned(s.clone()),
            }
        }
    }
}

fn emit_llm_display_name() -> TokenStream {
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        if cfg.is_hybrid_dynamic {
            quote! { Self::#v(m) => m.display_name(), }
        } else {
            quote! { Self::#v(m) => Cow::Borrowed(m.display_name()), }
        }
    });
    let dyn_arms = DYNAMIC_PROVIDERS.iter().map(|d| {
        let v = format_ident!("{}", d.enum_name);
        let fmt = format!("{} {{s}}", d.enum_name);
        quote! { Self::#v(s) => Cow::Owned(format!(#fmt)), }
    });
    quote! {
        /// Human-readable display name (e.g. `Claude Opus 4.6`)
        pub fn display_name(&self) -> Cow<'static, str> {
            match self {
                #(#catalog_arms)*
                #(#dyn_arms)*
            }
        }
    }
}

fn emit_llm_provider() -> TokenStream {
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        let name = cfg.parser_name;
        quote! { Self::#v(_) => #name, }
    });
    let dyn_arms = DYNAMIC_PROVIDERS.iter().map(|d| {
        let v = format_ident!("{}", d.enum_name);
        let name = d.parser_name;
        quote! { Self::#v(_) => #name, }
    });
    quote! {
        /// Provider identifier (e.g. `anthropic`)
        pub fn provider(&self) -> &'static str {
            match self {
                #(#catalog_arms)*
                #(#dyn_arms)*
            }
        }
    }
}

fn emit_llm_provider_display_name() -> TokenStream {
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        let name = cfg.display_name;
        quote! { Self::#v(_) => #name, }
    });
    let dyn_arms = DYNAMIC_PROVIDERS.iter().map(|d| {
        let v = format_ident!("{}", d.enum_name);
        let name = d.display_name;
        quote! { Self::#v(_) => #name, }
    });
    quote! {
        /// Human-readable provider name (e.g. `AWS Bedrock`)
        pub fn provider_display_name(&self) -> &'static str {
            match self {
                #(#catalog_arms)*
                #(#dyn_arms)*
            }
        }
    }
}

fn emit_llm_context_window() -> TokenStream {
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        if cfg.is_hybrid_dynamic {
            quote! { Self::#v(m) => m.context_window(), }
        } else {
            quote! { Self::#v(m) => Some(m.context_window()), }
        }
    });
    let dyn_pats = dynamic_pattern_with_binding("_");
    quote! {
        /// Context window size in tokens (None for dynamic providers)
        pub fn context_window(&self) -> Option<u32> {
            match self {
                #(#catalog_arms)*
                #dyn_pats => None,
            }
        }
    }
}

fn emit_llm_required_env_var() -> TokenStream {
    let mut some_arms = Vec::new();
    let mut none_pats = Vec::new();
    for cfg in PROVIDERS {
        let v = format_ident!("{}", cfg.enum_name);
        match cfg.env_var {
            Some(var) => some_arms.push(quote! { Self::#v(_) => Some(#var), }),
            None => none_pats.push(quote! { Self::#v(_) }),
        }
    }
    for d in DYNAMIC_PROVIDERS {
        let v = format_ident!("{}", d.enum_name);
        none_pats.push(quote! { Self::#v(_) });
    }
    quote! {
        /// Required env var for this model's provider (None for local providers)
        pub fn required_env_var(&self) -> Option<&'static str> {
            match self {
                #(#some_arms)*
                #(#none_pats)|* => None,
            }
        }
    }
}

fn emit_llm_all_required_env_vars() -> TokenStream {
    let vars = PROVIDERS.iter().filter_map(|cfg| cfg.env_var);
    quote! {
        /// All provider API key env var names (deduplicated, static)
        pub const ALL_REQUIRED_ENV_VARS: &[&str] = &[#(#vars),*];
    }
}

fn emit_llm_oauth_provider_id() -> TokenStream {
    let mut some_arms = Vec::new();
    let mut none_pats = Vec::new();
    for cfg in PROVIDERS {
        let v = format_ident!("{}", cfg.enum_name);
        match cfg.oauth_provider_id {
            Some(id) => some_arms.push(quote! { Self::#v(_) => Some(#id), }),
            None => none_pats.push(quote! { Self::#v(_) }),
        }
    }
    for d in DYNAMIC_PROVIDERS {
        let v = format_ident!("{}", d.enum_name);
        none_pats.push(quote! { Self::#v(_) });
    }
    quote! {
        /// OAuth provider ID if this model requires OAuth login (e.g. `"codex"`)
        pub fn oauth_provider_id(&self) -> Option<&'static str> {
            match self {
                #(#some_arms)*
                #(#none_pats)|* => None,
            }
        }
    }
}

fn emit_llm_reasoning_levels() -> TokenStream {
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        quote! { Self::#v(m) => m.reasoning_levels(), }
    });
    let dyn_pats = dynamic_pattern_with_binding("_");
    quote! {
        /// Reasoning levels supported by this model (empty if not a reasoning model)
        pub fn reasoning_levels(&self) -> &'static [ReasoningEffort] {
            match self {
                #(#catalog_arms)*
                #dyn_pats => &[],
            }
        }
    }
}

fn emit_llm_supports_reasoning() -> TokenStream {
    quote! {
        /// Whether this model supports reasoning/extended thinking
        pub fn supports_reasoning(&self) -> bool {
            !self.reasoning_levels().is_empty()
        }
    }
}

fn emit_llm_supports_prompt_caching() -> TokenStream {
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        quote! { Self::#v(m) => m.supports_prompt_caching(), }
    });
    let dyn_pats = dynamic_pattern_with_binding("_");
    quote! {
        /// Whether this model supports provider-side prompt caching
        pub fn supports_prompt_caching(&self) -> bool {
            match self {
                #(#catalog_arms)*
                #dyn_pats => false,
            }
        }
    }
}

fn emit_llm_supports_modality(modality: &str) -> TokenStream {
    let method = format_ident!("supports_{}", modality);
    let doc = format!(" Whether this model supports {modality} input");
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let v = format_ident!("{}", cfg.enum_name);
        quote! { Self::#v(m) => m.#method(), }
    });
    let dyn_pats = dynamic_pattern_with_binding("_");
    quote! {
        #[doc = #doc]
        pub fn #method(&self) -> bool {
            match self {
                #(#catalog_arms)*
                #dyn_pats => false,
            }
        }
    }
}

fn emit_llm_all() -> TokenStream {
    let pushes = PROVIDERS.iter().map(|cfg| {
        let inner = format_ident!("{}", cfg.inner_enum_name());
        let outer = format_ident!("{}", cfg.outer_enum_name());
        let v = format_ident!("{}", cfg.enum_name);
        if cfg.is_hybrid_dynamic {
            quote! {
                v.extend(#inner::ALL.iter().copied().map(#outer::Foundation).map(LlmModel::#v));
            }
        } else {
            quote! {
                v.extend(#inner::ALL.iter().copied().map(LlmModel::#v));
            }
        }
    });
    quote! {
        /// All catalog models (excludes dynamic providers)
        pub fn all() -> &'static [LlmModel] {
            static ALL: LazyLock<Vec<LlmModel>> = LazyLock::new(|| {
                let mut v = Vec::new();
                #(#pushes)*
                v
            });
            &ALL
        }
    }
}

fn emit_display_impl() -> TokenStream {
    quote! {
        impl std::fmt::Display for LlmModel {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}:{}", self.provider(), self.model_id())
            }
        }
    }
}

fn emit_fromstr_impl() -> TokenStream {
    let catalog_arms = PROVIDERS.iter().map(|cfg| {
        let name = cfg.parser_name;
        let outer = format_ident!("{}Model", cfg.enum_name);
        let v = format_ident!("{}", cfg.enum_name);
        quote! { #name => model_str.parse::<#outer>().map(Self::#v), }
    });
    let dyn_arms = DYNAMIC_PROVIDERS.iter().map(|d| {
        let name = d.parser_name;
        let v = format_ident!("{}", d.enum_name);
        quote! { #name => Ok(Self::#v(model_str.to_string())), }
    });
    quote! {
        impl std::str::FromStr for LlmModel {
            type Err = String;

            /// Parse a `provider:model` string into an `LlmModel`
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let (provider_str, model_str) = s.split_once(':').unwrap_or((s, ""));
                match provider_str {
                    #(#catalog_arms)*
                    #(#dyn_arms)*
                    _ => Err(format!("Unknown provider: '{provider_str}'")),
                }
            }
        }
    }
}

/// Build a `Self::Ollama(b) | Self::LlamaCpp(b)` pattern for all dynamic providers.
fn dynamic_pattern_with_binding(binding: &str) -> TokenStream {
    let binding_ident = if binding == "_" {
        quote! { _ }
    } else {
        let b = format_ident!("{}", binding);
        quote! { #b }
    };
    let pats = DYNAMIC_PROVIDERS.iter().map(|d| {
        let v = format_ident!("{}", d.enum_name);
        quote! { Self::#v(#binding_ident) }
    });
    quote! { #(#pats)|* }
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
            }
        }
        blank(&mut doc);

        pushln(&mut doc, "# Supported models");
        blank(&mut doc);
        pushln(&mut doc, "| Model ID | Name | Context | Reasoning | Image | Audio |");
        pushln(&mut doc, "|----------|------|---------|-----------|-------|-------|");
        for model in models {
            let ctx_str = format_context_window(model.context_window);
            let reasoning = if model.reasoning_levels.is_empty() { "" } else { "yes" };
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

    // ── Helper unit tests ────────────────────────────────────────────────────

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
    fn codex_subscription_context_window_overrides_known_codex_models() {
        for model_id in ["gpt-5.5", "gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex", "gpt-5.2", "codex-auto-review"] {
            assert_eq!(codex_subscription_context_window(model_id, 1_050_000), 272_000);
        }
    }

    #[test]
    fn codex_subscription_context_window_leaves_unknown_models_unchanged() {
        assert_eq!(codex_subscription_context_window("gpt-5.3-codex-spark", 128_000), 128_000);
        assert_eq!(codex_subscription_context_window("some-future-model", 400_000), 400_000);
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
    fn build_assigns_codex_four_reasoning_levels() {
        let mut data = minimal_models_dev_json();
        insert_models(
            &mut data,
            "openai",
            json!({
                "gpt-5.4-codex": {
                    "id": "gpt-5.4-codex", "name": "GPT-5.4 Codex", "tool_call": true, "reasoning": true,
                    "limit": {"context": 200_000, "output": 0}
                }
            }),
        );

        let models = build_from_value(&data);
        let codex_model = models["codex"].iter().find(|m| m.model_id == "gpt-5.4-codex").unwrap();
        assert_eq!(codex_model.reasoning_levels, vec!["low", "medium", "high", "xhigh"]);
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

    // ── Markdown docs ────────────────────────────────────────────────────────

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
        provider.insert("models".to_string(), models);
    }

    fn minimal_models_dev_json() -> Value {
        let mut root = serde_json::Map::new();
        for cfg in PROVIDERS {
            let json_key = cfg.json_key();
            root.entry(json_key.to_string())
                .or_insert_with(|| json!({"id": json_key, "name": json_key, "env": [], "models": {}}));
            for &extra in cfg.extra_source_ids {
                root.entry(extra.to_string())
                    .or_insert_with(|| json!({"id": extra, "name": extra, "env": [], "models": {}}));
            }
        }
        Value::Object(root)
    }
}
