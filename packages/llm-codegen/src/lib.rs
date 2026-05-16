#![doc = include_str!("../README.md")]

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

    // If the variant starts with a digit, prefix with underscore
    if result.starts_with(|c: char| c.is_ascii_digit()) {
        result.insert(0, '_');
    }

    result
}

fn emit_generated_source(ctx: &CodegenCtx) -> String {
    let mut out = String::with_capacity(64_000);
    emit_header(&mut out);
    emit_provider_enums(&mut out, &ctx.provider_models);
    emit_provider_impls(&mut out, &ctx.provider_models);
    emit_llm_model_enum(&mut out);
    emit_from_impls(&mut out);
    emit_llm_model_impl(&mut out);
    emit_display_impl(&mut out);
    emit_fromstr_impl(&mut out);
    out
}

fn emit_header(out: &mut String) {
    pushln(out, "// Auto-generated from models.dev — do not edit manually");
    pushln(out, "// Regenerated automatically by build.rs");
    blank(out);
    pushln(out, "use std::borrow::Cow;");
    pushln(out, "use std::sync::LazyLock;");
    pushln(out, "use crate::ReasoningEffort;");
    blank(out);
}

fn emit_provider_enums(out: &mut String, provider_models: &ProviderModels) {
    for cfg in PROVIDERS {
        let inner = cfg.inner_enum_name();
        pushln(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]");
        pushln(out, format!("pub enum {inner} {{"));
        for model in &provider_models[cfg.dev_id] {
            pushln(out, format!("    {},", model.variant_name));
        }
        pushln(out, "}");
        blank(out);
    }
}

fn emit_provider_impls(out: &mut String, provider_models: &ProviderModels) {
    for cfg in PROVIDERS {
        let models = &provider_models[cfg.dev_id];
        let enum_name = cfg.inner_enum_name();

        pushln(out, format!("impl {enum_name} {{"));

        // model_id — each model has a unique ID, no grouping needed
        pushln(out, "    #[allow(clippy::too_many_lines)]");
        pushln(out, "    fn model_id(self) -> &'static str {");
        pushln(out, "        match self {");
        for model in models {
            pushln(out, format!("            Self::{} => \"{}\",", model.variant_name, model.model_id));
        }
        pushln(out, "        }");
        pushln(out, "    }");
        blank(out);

        // display_name — group variants that share the same name
        pushln(out, "    #[allow(clippy::too_many_lines)]");
        pushln(out, "    fn display_name(self) -> &'static str {");
        pushln(out, "        match self {");
        emit_grouped_arms(out, models, |m| escape_rust_string(&m.display_name), |name| format!("\"{name}\""));
        pushln(out, "        }");
        pushln(out, "    }");
        blank(out);

        // context_window — group variants that share the same value
        pushln(out, "    fn context_window(self) -> u32 {");
        pushln(out, "        match self {");
        emit_grouped_arms(
            out,
            models,
            |m| m.context_window.to_string(),
            |val| format_number(val.parse::<u32>().unwrap()),
        );
        pushln(out, "        }");
        pushln(out, "    }");
        blank(out);

        // reasoning_levels — per-model reasoning level list
        pushln(out, "    pub fn reasoning_levels(self) -> &'static [ReasoningEffort] {");
        pushln(out, "        match self {");
        emit_grouped_arms(out, models, |m| m.reasoning_levels.join(","), format_reasoning_levels_rhs);
        pushln(out, "        }");
        pushln(out, "    }");
        blank(out);

        // supports_reasoning — derived from reasoning_levels
        pushln(out, "    pub fn supports_reasoning(self) -> bool {");
        pushln(out, "        !self.reasoning_levels().is_empty()");
        pushln(out, "    }");
        blank(out);

        // supports_prompt_caching — derived from models.dev cache pricing fields
        pushln(out, "    pub fn supports_prompt_caching(self) -> bool {");
        pushln(out, "        match self {");
        emit_grouped_arms(out, models, |m| m.supports_prompt_caching.to_string(), std::string::ToString::to_string);
        pushln(out, "        }");
        pushln(out, "    }");
        blank(out);

        for modality in ["image", "audio"] {
            pushln(out, format!("    pub fn supports_{modality}(self) -> bool {{"));
            pushln(out, "        match self {");
            let mod_owned = modality.to_string();
            emit_grouped_arms(
                out,
                models,
                |m| m.input_modalities.contains(&mod_owned).to_string(),
                std::string::ToString::to_string,
            );
            pushln(out, "        }");
            pushln(out, "    }");
            blank(out);
        }

        // ALL constant
        pushln(out, format!("    const ALL: &[{enum_name}] = &["));
        for model in models {
            pushln(out, format!("        Self::{},", model.variant_name));
        }
        pushln(out, "    ];");

        pushln(out, "}");
        blank(out);

        emit_from_str_impl(out, &enum_name, cfg.parser_name, models);
    }
}

fn emit_from_str_impl(out: &mut String, enum_name: &str, parser_name: &str, models: &[ModelInfo]) {
    pushln(out, format!("impl std::str::FromStr for {enum_name} {{"));
    pushln(out, "    type Err = String;");
    blank(out);
    pushln(out, "    #[allow(clippy::too_many_lines)]");
    pushln(out, "    fn from_str(s: &str) -> Result<Self, Self::Err> {");
    pushln(out, "        match s {");
    for model in models {
        pushln(out, format!("            \"{}\" => Ok(Self::{}),", model.model_id, model.variant_name));
    }
    pushln(out, format!("            _ => Err(format!(\"Unknown {parser_name} model: '{{s}}'\")),"));
    pushln(out, "        }");
    pushln(out, "    }");
    pushln(out, "}");
    blank(out);
}

/// Emit match arms grouped by value to avoid clippy `match_same_arms`.
///
/// `key_fn` extracts a grouping key from each model (e.g. `context_window` as string).
/// `fmt_val` formats the key into the match arm's RHS.
fn emit_grouped_arms(
    out: &mut String,
    models: &[ModelInfo],
    key_fn: impl Fn(&ModelInfo) -> String,
    fmt_val: impl Fn(&str) -> String,
) {
    // Group variants by value, preserving insertion order via BTreeMap
    let mut groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for model in models {
        groups.entry(key_fn(model)).or_default().push(&model.variant_name);
    }

    for (key, variants) in &groups {
        let rhs = fmt_val(key);
        if variants.len() == 1 {
            pushln(out, format!("            Self::{} => {rhs},", variants[0]));
        } else {
            let patterns: Vec<String> = variants.iter().map(|v| format!("Self::{v}")).collect();
            pushln(out, format!("            {} => {rhs},", patterns.join(" | ")));
        }
    }
}

/// Format the RHS of a `reasoning_levels()` match arm from a comma-joined key.
fn format_reasoning_levels_rhs(key: &str) -> String {
    if key.is_empty() {
        return "&[]".to_string();
    }
    let items: Vec<String> = key
        .split(',')
        .map(|l| {
            let variant = level_str_to_variant(l);
            format!("ReasoningEffort::{variant}")
        })
        .collect();
    format!("&[{}]", items.join(", "))
}

/// Map a reasoning level string to its `ReasoningEffort` variant name.
/// Panics at build time if an unknown level is encountered.
fn level_str_to_variant(level: &str) -> &'static str {
    match level {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Xhigh",
        other => panic!("Unknown reasoning level: {other}"),
    }
}

fn emit_llm_model_enum(out: &mut String) {
    pushln(out, "/// A model from a specific provider");
    pushln(out, "#[derive(Debug, Clone, PartialEq, Eq, Hash)]");
    pushln(out, "pub enum LlmModel {");
    for cfg in PROVIDERS {
        pushln(out, format!("    {provider}({provider}Model),", provider = cfg.enum_name));
    }
    for dyn_cfg in DYNAMIC_PROVIDERS {
        pushln(out, format!("    {}(String),", dyn_cfg.enum_name));
    }
    pushln(out, "}");
    blank(out);
}

fn emit_from_impls(out: &mut String) {
    for cfg in PROVIDERS {
        pushln(out, format!("impl From<{}Model> for LlmModel {{", cfg.enum_name));
        pushln(out, format!("    fn from(m: {}Model) -> Self {{ LlmModel::{}(m) }}", cfg.enum_name, cfg.enum_name));
        pushln(out, "}");
        blank(out);
    }
}

fn emit_llm_model_impl(out: &mut String) {
    pushln(out, "impl LlmModel {");
    emit_llm_model_id(out);
    emit_llm_display_name(out);
    emit_llm_provider(out);
    emit_llm_provider_display_name(out);
    emit_llm_context_window(out);
    emit_llm_required_env_var(out);
    emit_llm_all_required_env_vars(out);
    emit_llm_oauth_provider_id(out);
    emit_llm_reasoning_levels(out);
    emit_llm_supports_reasoning(out);
    emit_llm_supports_prompt_caching(out);
    for modality in ["image", "audio"] {
        emit_llm_supports_modality(out, modality);
    }
    emit_llm_all(out);
    pushln(out, "}");
    blank(out);
}

fn emit_llm_model_id(out: &mut String) {
    pushln(out, "    /// Raw model ID (e.g. `claude-opus-4-6`, `llama3.2`)");
    pushln(out, "    pub fn model_id(&self) -> Cow<'static, str> {");
    pushln(out, "        match self {");
    for cfg in PROVIDERS {
        if cfg.is_hybrid_dynamic {
            pushln(out, format!("            Self::{}(m) => m.model_id(),", cfg.enum_name));
        } else {
            pushln(out, format!("            Self::{}(m) => Cow::Borrowed(m.model_id()),", cfg.enum_name));
        }
    }
    pushln(out, format!("            {} => Cow::Owned(s.clone()),", dynamic_pattern_with_binding("s")));
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_display_name(out: &mut String) {
    pushln(out, "    /// Human-readable display name (e.g. `Claude Opus 4.6`)");
    pushln(out, "    pub fn display_name(&self) -> Cow<'static, str> {");
    pushln(out, "        match self {");
    for cfg in PROVIDERS {
        if cfg.is_hybrid_dynamic {
            pushln(out, format!("            Self::{}(m) => m.display_name(),", cfg.enum_name));
        } else {
            pushln(out, format!("            Self::{}(m) => Cow::Borrowed(m.display_name()),", cfg.enum_name));
        }
    }
    for dyn_cfg in DYNAMIC_PROVIDERS {
        pushln(
            out,
            format!(
                "            Self::{}(s) => Cow::Owned(format!(\"{} {{s}}\")),",
                dyn_cfg.enum_name, dyn_cfg.enum_name
            ),
        );
    }
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_provider(out: &mut String) {
    pushln(out, "    /// Provider identifier (e.g. `anthropic`)");
    pushln(out, "    pub fn provider(&self) -> &'static str {");
    pushln(out, "        match self {");
    for cfg in PROVIDERS {
        pushln(out, format!("            Self::{}(_) => \"{}\",", cfg.enum_name, cfg.parser_name));
    }
    for dyn_cfg in DYNAMIC_PROVIDERS {
        pushln(out, format!("            Self::{}(_) => \"{}\",", dyn_cfg.enum_name, dyn_cfg.parser_name));
    }
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_provider_display_name(out: &mut String) {
    pushln(out, "    /// Human-readable provider name (e.g. `AWS Bedrock`)");
    pushln(out, "    pub fn provider_display_name(&self) -> &'static str {");
    pushln(out, "        match self {");
    for cfg in PROVIDERS {
        pushln(out, format!("            Self::{}(_) => \"{}\",", cfg.enum_name, cfg.display_name));
    }
    for dyn_cfg in DYNAMIC_PROVIDERS {
        pushln(out, format!("            Self::{}(_) => \"{}\",", dyn_cfg.enum_name, dyn_cfg.display_name));
    }
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_context_window(out: &mut String) {
    pushln(out, "    /// Context window size in tokens (None for dynamic providers)");
    pushln(out, "    pub fn context_window(&self) -> Option<u32> {");
    pushln(out, "        match self {");
    for cfg in PROVIDERS {
        if cfg.is_hybrid_dynamic {
            pushln(out, format!("            Self::{}(m) => m.context_window(),", cfg.enum_name));
        } else {
            pushln(out, format!("            Self::{}(m) => Some(m.context_window()),", cfg.enum_name));
        }
    }
    pushln(out, format!("            {} => None,", dynamic_pattern_with_binding("_")));
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_required_env_var(out: &mut String) {
    pushln(out, "    /// Required env var for this model's provider (None for local providers)");
    pushln(out, "    pub fn required_env_var(&self) -> Option<&'static str> {");
    pushln(out, "        match self {");
    let mut none_arms: Vec<String> = Vec::new();
    for cfg in PROVIDERS {
        match cfg.env_var {
            Some(var) => pushln(out, format!("            Self::{}(_) => Some(\"{}\"),", cfg.enum_name, var)),
            None => none_arms.push(format!("Self::{}(_)", cfg.enum_name)),
        }
    }
    for dyn_cfg in DYNAMIC_PROVIDERS {
        none_arms.push(format!("Self::{}(_)", dyn_cfg.enum_name));
    }
    pushln(out, format!("            {} => None,", none_arms.join(" | ")));
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_all_required_env_vars(out: &mut String) {
    let vars: Vec<&str> = PROVIDERS.iter().filter_map(|cfg| cfg.env_var).collect();
    pushln(out, "    /// All provider API key env var names (deduplicated, static)");
    pushln(
        out,
        format!(
            "    pub const ALL_REQUIRED_ENV_VARS: &[&str] = &[{}];",
            vars.iter().map(|v| format!("\"{v}\"")).collect::<Vec<_>>().join(", ")
        ),
    );
    blank(out);
}

fn emit_llm_oauth_provider_id(out: &mut String) {
    pushln(out, "    /// OAuth provider ID if this model requires OAuth login (e.g. `\"codex\"`)");
    pushln(out, "    pub fn oauth_provider_id(&self) -> Option<&'static str> {");
    pushln(out, "        match self {");
    let mut none_arms: Vec<String> = Vec::new();
    for cfg in PROVIDERS {
        match cfg.oauth_provider_id {
            Some(id) => pushln(out, format!("            Self::{}(_) => Some(\"{}\"),", cfg.enum_name, id)),
            None => none_arms.push(format!("Self::{}(_)", cfg.enum_name)),
        }
    }
    for dyn_cfg in DYNAMIC_PROVIDERS {
        none_arms.push(format!("Self::{}(_)", dyn_cfg.enum_name));
    }
    pushln(out, format!("            {} => None,", none_arms.join(" | ")));
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_reasoning_levels(out: &mut String) {
    pushln(out, "    /// Reasoning levels supported by this model (empty if not a reasoning model)");
    pushln(out, "    pub fn reasoning_levels(&self) -> &'static [ReasoningEffort] {");
    pushln(out, "        match self {");
    for cfg in PROVIDERS {
        pushln(out, format!("            Self::{}(m) => m.reasoning_levels(),", cfg.enum_name));
    }
    pushln(out, format!("            {} => &[],", dynamic_pattern_with_binding("_")));
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_supports_reasoning(out: &mut String) {
    pushln(out, "    /// Whether this model supports reasoning/extended thinking");
    pushln(out, "    pub fn supports_reasoning(&self) -> bool {");
    pushln(out, "        !self.reasoning_levels().is_empty()");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_supports_prompt_caching(out: &mut String) {
    pushln(out, "    /// Whether this model supports provider-side prompt caching");
    pushln(out, "    pub fn supports_prompt_caching(&self) -> bool {");
    pushln(out, "        match self {");
    for cfg in PROVIDERS {
        pushln(out, format!("            Self::{}(m) => m.supports_prompt_caching(),", cfg.enum_name));
    }
    pushln(out, format!("            {} => false,", dynamic_pattern_with_binding("_")));
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_supports_modality(out: &mut String, modality: &str) {
    pushln(out, format!("    /// Whether this model supports {modality} input"));
    pushln(out, format!("    pub fn supports_{modality}(&self) -> bool {{"));
    pushln(out, "        match self {");
    for cfg in PROVIDERS {
        pushln(out, format!("            Self::{}(m) => m.supports_{modality}(),", cfg.enum_name));
    }
    pushln(out, format!("            {} => false,", dynamic_pattern_with_binding("_")));
    pushln(out, "        }");
    pushln(out, "    }");
    blank(out);
}

fn emit_llm_all(out: &mut String) {
    pushln(out, "    /// All catalog models (excludes dynamic providers)");
    pushln(out, "    pub fn all() -> &'static [LlmModel] {");
    pushln(out, "        static ALL: LazyLock<Vec<LlmModel>> = LazyLock::new(|| {");
    pushln(out, "            let mut v = Vec::new();");
    for cfg in PROVIDERS {
        if cfg.is_hybrid_dynamic {
            pushln(
                out,
                format!(
                    "            v.extend({inner}::ALL.iter().copied().map({outer}::Foundation).map(LlmModel::{variant}));",
                    inner = cfg.inner_enum_name(),
                    outer = cfg.outer_enum_name(),
                    variant = cfg.enum_name,
                ),
            );
        } else {
            pushln(
                out,
                format!(
                    "            v.extend({}Model::ALL.iter().copied().map(LlmModel::{}));",
                    cfg.enum_name, cfg.enum_name
                ),
            );
        }
    }
    pushln(out, "            v");
    pushln(out, "        });");
    pushln(out, "        &ALL");
    pushln(out, "    }");
}

fn emit_display_impl(out: &mut String) {
    pushln(out, "impl std::fmt::Display for LlmModel {");
    pushln(out, "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {");
    pushln(out, "        write!(f, \"{}:{}\", self.provider(), self.model_id())");
    pushln(out, "    }");
    pushln(out, "}");
    blank(out);
}

fn emit_fromstr_impl(out: &mut String) {
    pushln(out, "impl std::str::FromStr for LlmModel {");
    pushln(out, "    type Err = String;");
    blank(out);
    pushln(out, "    /// Parse a `provider:model` string into an `LlmModel`");
    pushln(out, "    fn from_str(s: &str) -> Result<Self, Self::Err> {");
    pushln(out, "        let (provider_str, model_str) = s.split_once(':').unwrap_or((s, \"\"));");
    pushln(out, "        match provider_str {");
    for cfg in PROVIDERS {
        pushln(
            out,
            format!(
                "            \"{}\" => model_str.parse::<{}Model>().map(Self::{}),",
                cfg.parser_name, cfg.enum_name, cfg.enum_name
            ),
        );
    }
    for dyn_cfg in DYNAMIC_PROVIDERS {
        pushln(
            out,
            format!(
                "            \"{}\" => Ok(Self::{}(model_str.to_string())),",
                dyn_cfg.parser_name, dyn_cfg.enum_name
            ),
        );
    }
    pushln(out, "            _ => Err(format!(\"Unknown provider: '{provider_str}'\")),");
    pushln(out, "        }");
    pushln(out, "    }");
    pushln(out, "}");
}

/// Build a combined `|` pattern for all dynamic providers with a binding variable.
/// e.g. `Self::Ollama(s) | Self::LlamaCpp(s)` or `Self::Ollama(_) | Self::LlamaCpp(_)`
fn dynamic_pattern_with_binding(binding: &str) -> String {
    DYNAMIC_PROVIDERS.iter().map(|d| format!("Self::{}({binding})", d.enum_name)).collect::<Vec<_>>().join(" | ")
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

fn escape_rust_string(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

fn emit_provider_docs(ctx: &CodegenCtx) -> HashMap<String, String> {
    let mut docs = HashMap::new();

    for cfg in PROVIDERS {
        let models = &ctx.provider_models[cfg.dev_id];
        let mut doc = String::new();

        pushln(&mut doc, format!("`{}` LLM provider.", cfg.display_name));
        blank(&mut doc);

        // Authentication
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

        // Supported models table
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

    // Dynamic providers
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

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use serde_json::json;
    use tempfile::NamedTempFile;

    #[test]
    fn generate_sorts_and_filters_models() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().expect("root object");
        let anthropic = root.get_mut("anthropic").and_then(Value::as_object_mut).expect("anthropic provider");

        anthropic.insert(
            "models".to_string(),
            json!({
                "b-model": {
                    "id": "b-model",
                    "name": "B Model",
                    "tool_call": true,
                    "limit": {"context": 2000, "output": 0}
                },
                "a-model": {
                    "id": "a-model",
                    "name": "A Model",
                    "tool_call": true,
                    "limit": {"context": 1000, "output": 0}
                },
                "alpha-latest": {
                    "id": "alpha-latest",
                    "name": "Alias",
                    "tool_call": true,
                    "limit": {"context": 500, "output": 0}
                },
                "no-tools": {
                    "id": "no-tools",
                    "name": "No Tools",
                    "tool_call": false,
                    "limit": {"context": 500, "output": 0}
                }
            }),
        );

        let source = generate_from_value(&data);
        // Provider-level FromStr: sorted model IDs
        let a_model = "\"a-model\" => Ok(Self::AModel),";
        let b_model = "\"b-model\" => Ok(Self::BModel),";
        let a_pos = source.find(a_model).expect("a-model parse arm");
        let b_pos = source.find(b_model).expect("b-model parse arm");
        assert!(a_pos < b_pos);
        // Aliases and non-tool-call models are excluded
        assert!(!source.contains("AlphaLatest"));
        assert!(!source.contains("NoTools"));
    }

    #[test]
    fn generate_contains_core_sections() {
        let source = generate_from_value(&minimal_models_dev_json());
        assert!(source.contains("pub enum LlmModel {"));
        assert!(source.contains("impl std::str::FromStr for LlmModel {"));
        assert!(source.contains("impl std::fmt::Display for LlmModel {"));
        assert!(source.contains("pub fn required_env_var(&self) -> Option<&'static str> {"));
    }

    #[test]
    fn generate_contains_dynamic_provider_arms() {
        let source = generate_from_value(&minimal_models_dev_json());
        assert!(source.contains("\"ollama\" => Ok(Self::Ollama(model_str.to_string())),"));
        assert!(source.contains("\"llamacpp\" => Ok(Self::LlamaCpp(model_str.to_string())),"));
        // Dynamic providers are combined with | for None-returning arms
        assert!(source.contains("Self::Ollama(_) | Self::LlamaCpp(_) => None,"));
    }

    #[test]
    fn generate_codex_is_catalog_provider() {
        let source = generate_from_value(&minimal_models_dev_json());
        // Codex is a catalog provider, not dynamic
        assert!(source.contains("pub enum CodexModel {"));
        assert!(source.contains("\"codex\" => model_str.parse::<CodexModel>().map(Self::Codex),"));
        assert!(source.contains("Self::Codex(m) => Some(m.context_window()),"));
    }

    #[test]
    fn generate_bedrock_is_hybrid_provider() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().unwrap();
        let bedrock = root.get_mut("amazon-bedrock").and_then(Value::as_object_mut).unwrap();

        bedrock.insert(
            "models".to_string(),
            json!({
                "anthropic.foo-v1:0": {
                    "id": "anthropic.foo-v1:0",
                    "name": "Foo Model",
                    "tool_call": true,
                    "limit": {"context": 200_000, "output": 0}
                }
            }),
        );

        let source = generate_from_value(&data);

        assert!(source.contains("pub enum BedrockFoundationModel {"));
        assert!(source.contains("AnthropicFooV10"));
        assert!(source.contains("impl std::str::FromStr for BedrockFoundationModel"));
        assert!(source.contains("\"anthropic.foo-v1:0\" => Ok(Self::AnthropicFooV10),"));
        assert!(source.contains("Unknown bedrock model"));

        // Wrapper enum is hand-written in catalog::bedrock — codegen must NOT emit it.
        assert!(!source.contains("pub enum BedrockModel {"));
        assert!(!source.contains("impl std::str::FromStr for BedrockModel"));
        assert!(!source.contains("fn is_bedrock_inference_profile_arn"));

        // LlmModel still delegates to the (hand-written) wrapper.
        assert!(source.contains(
            "BedrockFoundationModel::ALL.iter().copied().map(BedrockModel::Foundation).map(LlmModel::Bedrock)"
        ));
        assert!(source.contains("Self::Bedrock(m) => m.model_id(),"));
        assert!(source.contains("Self::Bedrock(m) => m.display_name(),"));
        assert!(source.contains("Self::Bedrock(m) => m.context_window(),"));
    }

    #[test]
    fn generate_non_hybrid_providers_keep_flat_enum() {
        let source = generate_from_value(&minimal_models_dev_json());
        assert!(source.contains("pub enum AnthropicModel {"));
        assert!(!source.contains("AnthropicFoundationModel"));
        assert!(!source.contains("Foundation(AnthropicModel)"));
        assert!(source.contains("Self::Anthropic(m) => Cow::Borrowed(m.model_id()),"));
        assert!(source.contains("Self::Anthropic(m) => Some(m.context_window()),"));
    }

    #[test]
    fn generate_oauth_provider_id_for_codex() {
        let source = generate_from_value(&minimal_models_dev_json());
        // Codex models return Some("codex") for oauth_provider_id
        assert!(source.contains("Self::Codex(_) => Some(\"codex\"),"));
        // Non-OAuth providers return None
        assert!(source.contains("pub fn oauth_provider_id(&self) -> Option<&'static str>"));
    }

    #[test]
    fn generate_delegates_to_provider_impls() {
        let source = generate_from_value(&minimal_models_dev_json());
        // LlmModel delegates to per-provider methods
        assert!(source.contains("Self::Anthropic(m) => Cow::Borrowed(m.model_id()),"));
        assert!(source.contains("Self::Anthropic(m) => Some(m.context_window()),"));
        // Provider-level FromStr is used by LlmModel::FromStr
        assert!(source.contains("\"anthropic\" => model_str.parse::<AnthropicModel>().map(Self::Anthropic),"));
    }

    #[test]
    fn generate_formats_large_numbers_with_separators() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().expect("root object");
        let anthropic = root.get_mut("anthropic").and_then(Value::as_object_mut).expect("anthropic provider");

        anthropic.insert(
            "models".to_string(),
            json!({
                "big-model": {
                    "id": "big-model",
                    "name": "Big Model",
                    "tool_call": true,
                    "limit": {"context": 200_000, "output": 0}
                }
            }),
        );

        let source = generate_from_value(&data);
        assert!(source.contains("200_000"));
        assert!(!source.contains("200000"));
    }

    #[test]
    fn generate_groups_identical_match_arms() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().expect("root object");
        let anthropic = root.get_mut("anthropic").and_then(Value::as_object_mut).expect("anthropic provider");

        anthropic.insert(
            "models".to_string(),
            json!({
                "model-a": {
                    "id": "model-a",
                    "name": "Same Name",
                    "tool_call": true,
                    "limit": {"context": 100_000, "output": 0}
                },
                "model-b": {
                    "id": "model-b",
                    "name": "Same Name",
                    "tool_call": true,
                    "limit": {"context": 100_000, "output": 0}
                }
            }),
        );

        let source = generate_from_value(&data);
        // Both context_window and display_name should combine arms
        assert!(source.contains("Self::ModelA | Self::ModelB => 100_000,"));
        assert!(source.contains("Self::ModelA | Self::ModelB => \"Same Name\","));
    }

    #[test]
    fn test_model_id_to_variant() {
        assert_eq!(model_id_to_variant("claude-sonnet-4-5-20250929"), "ClaudeSonnet4520250929");
        assert_eq!(model_id_to_variant("gemini-2.5-flash"), "Gemini25Flash");
        assert_eq!(model_id_to_variant("deepseek-chat"), "DeepseekChat");
        assert_eq!(model_id_to_variant("glm-4.5"), "Glm45");
    }

    #[test]
    fn test_model_id_to_variant_with_slash_and_colon() {
        assert_eq!(model_id_to_variant("anthropic/claude-opus-4.6"), "AnthropicClaudeOpus46");
        assert_eq!(model_id_to_variant("openai/gpt-5.1-codex-max"), "OpenaiGpt51CodexMax");
        assert_eq!(model_id_to_variant("deepseek/deepseek-r1:free"), "DeepseekDeepseekR1Free");
    }

    #[test]
    fn test_is_alias() {
        assert!(is_alias("claude-sonnet-4-5-latest"));
        assert!(is_alias("claude-3-7-sonnet-latest"));
        assert!(!is_alias("claude-sonnet-4-5-20250929"));
    }

    #[test]
    fn generate_contains_reasoning_levels_and_supports_reasoning() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().expect("root object");
        let anthropic = root.get_mut("anthropic").and_then(Value::as_object_mut).expect("anthropic provider");

        anthropic.insert(
            "models".to_string(),
            json!({
                "thinker": {
                    "id": "thinker",
                    "name": "Thinker",
                    "tool_call": true,
                    "reasoning": true,
                    "limit": {"context": 1000, "output": 0}
                },
                "fast": {
                    "id": "fast",
                    "name": "Fast",
                    "tool_call": true,
                    "reasoning": false,
                    "limit": {"context": 1000, "output": 0}
                }
            }),
        );

        let source = generate_from_value(&data);
        // Provider enum should have reasoning_levels method
        assert!(source.contains("pub fn reasoning_levels(self) -> &'static [ReasoningEffort] {"));
        // Thinker (anthropic) gets standard 3 levels
        assert!(
            source
                .contains("Self::Thinker => &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High],")
        );
        // Fast gets empty
        assert!(source.contains("Self::Fast => &[],"));
        // supports_reasoning delegates to reasoning_levels
        assert!(source.contains("pub fn supports_reasoning(self) -> bool {"));
        assert!(source.contains("!self.reasoning_levels().is_empty()"));
        // LlmModel level
        assert!(source.contains("pub fn reasoning_levels(&self) -> &'static [ReasoningEffort] {"));
        // Dynamic providers return empty
        assert!(source.contains("Self::Ollama(_) | Self::LlamaCpp(_) => &[],"));
    }

    #[test]
    fn generate_codex_gets_four_reasoning_levels() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().expect("root object");
        let openai = root.get_mut("openai").and_then(Value::as_object_mut).expect("openai provider");

        openai.insert(
            "models".to_string(),
            json!({
                "gpt-5.4-codex": {
                    "id": "gpt-5.4-codex",
                    "name": "GPT-5.4 Codex",
                    "tool_call": true,
                    "reasoning": true,
                    "limit": {"context": 200_000, "output": 0}
                }
            }),
        );

        let source = generate_from_value(&data);
        assert!(
            source.contains(
                "ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Xhigh"
            ),
            "Codex reasoning model should have 4 levels including Xhigh"
        );
    }

    #[test]
    fn generate_codex_overrides_gpt55_subscription_context_window() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().expect("root object");
        let openai = root.get_mut("openai").and_then(Value::as_object_mut).expect("openai provider");

        openai.insert(
            "models".to_string(),
            json!({
                "gpt-5.5": {
                    "id": "gpt-5.5",
                    "name": "GPT-5.5",
                    "tool_call": true,
                    "reasoning": true,
                    "limit": {"context": 1_050_000, "output": 128_000}
                }
            }),
        );

        let source = generate_from_value(&data);
        let codex_impl = generated_impl_block(&source, "CodexModel");
        let openai_impl = generated_impl_block(&source, "OpenaiModel");

        assert!(codex_impl.contains("Self::Gpt55 => 272_000,"));
        assert!(openai_impl.contains("Self::Gpt55 => 1_050_000,"));
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
    fn generate_derives_prompt_caching_from_cost_fields() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().expect("root object");
        let bedrock = root.get_mut("amazon-bedrock").and_then(Value::as_object_mut).expect("bedrock provider");

        bedrock.insert(
            "models".to_string(),
            json!({
                "anthropic.cached-v1:0": {
                    "id": "anthropic.cached-v1:0",
                    "name": "Cached",
                    "tool_call": true,
                    "limit": {"context": 200_000, "output": 0},
                    "cost": {"input": 3.0, "output": 15.0, "cache_read": 0.3, "cache_write": 3.75}
                },
                "anthropic.uncached-v1:0": {
                    "id": "anthropic.uncached-v1:0",
                    "name": "Uncached",
                    "tool_call": true,
                    "limit": {"context": 200_000, "output": 0},
                    "cost": {"input": 3.0, "output": 15.0}
                }
            }),
        );

        let source = generate_from_value(&data);
        let bedrock_impl = generated_impl_block(&source, "BedrockFoundationModel");

        assert!(bedrock_impl.contains("pub fn supports_prompt_caching(self) -> bool {"));
        assert!(bedrock_impl.contains("Self::AnthropicCachedV10 => true,"));
        assert!(bedrock_impl.contains("Self::AnthropicUncachedV10 => false,"));
        assert!(source.contains("Self::Bedrock(m) => m.supports_prompt_caching(),"));
    }

    fn generate_from_value(data: &Value) -> String {
        let tmp = NamedTempFile::new().expect("temp file");
        let json = serde_json::to_string(data).expect("serialize fixture");
        std::fs::write(tmp.path(), json).expect("write fixture");
        generate(tmp.path()).expect("codegen succeeds").rust_source
    }

    fn generated_impl_block<'a>(source: &'a str, enum_name: &str) -> &'a str {
        let start_marker = format!("impl {enum_name} {{");
        let start = source.find(&start_marker).expect("provider impl block start");
        let rest = &source[start..];
        let end_marker = format!("impl std::str::FromStr for {enum_name}");
        let end = rest.find(&end_marker).expect("provider impl block end");
        &rest[..end]
    }
    fn minimal_models_dev_json() -> Value {
        let mut root = serde_json::Map::new();
        for cfg in PROVIDERS {
            let json_key = cfg.json_key();
            root.entry(json_key.to_string()).or_insert_with(|| {
                json!({
                    "id": json_key,
                    "name": json_key,
                    "env": [],
                    "models": {}
                })
            });
            for &extra in cfg.extra_source_ids {
                root.entry(extra.to_string()).or_insert_with(|| {
                    json!({
                        "id": extra,
                        "name": extra,
                        "env": [],
                        "models": {}
                    })
                });
            }
        }
        Value::Object(root)
    }

    #[test]
    fn extra_source_ids_merges_models_into_provider() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().unwrap();

        // Add a model to the zai-coding-plan extra source that doesn't exist in zai
        let extra = root.get_mut("zai-coding-plan").unwrap().as_object_mut().unwrap();
        extra.insert(
            "models".to_string(),
            json!({
                "extra-model": {
                    "id": "extra-model",
                    "name": "Extra Model",
                    "tool_call": true,
                    "limit": {"context": 4000, "output": 0}
                }
            }),
        );

        let source = generate_from_value(&data);
        // The extra model should appear under ZAi
        assert!(source.contains("\"extra-model\" => Ok(Self::ExtraModel),"));
    }

    #[test]
    fn extra_source_ids_does_not_duplicate_existing_models() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().unwrap();

        // Add same model ID to both zai and zai-coding-plan
        let zai = root.get_mut("zai").unwrap().as_object_mut().unwrap();
        zai.insert(
            "models".to_string(),
            json!({
                "shared-model": {
                    "id": "shared-model",
                    "name": "Shared Model",
                    "tool_call": true,
                    "limit": {"context": 1000, "output": 0}
                }
            }),
        );
        let extra = root.get_mut("zai-coding-plan").unwrap().as_object_mut().unwrap();
        extra.insert(
            "models".to_string(),
            json!({
                "shared-model": {
                    "id": "shared-model",
                    "name": "Shared Model Duplicate",
                    "tool_call": true,
                    "limit": {"context": 2000, "output": 0}
                }
            }),
        );

        let source = generate_from_value(&data);
        let from_str_matches = source.matches("\"shared-model\" => Ok(Self::SharedModel),").count();
        assert_eq!(from_str_matches, 1);
    }

    #[test]
    fn generate_emits_provider_docs() {
        let mut data = minimal_models_dev_json();
        let root = data.as_object_mut().unwrap();
        let anthropic = root.get_mut("anthropic").and_then(Value::as_object_mut).unwrap();

        anthropic.insert(
            "models".to_string(),
            json!({
                "claude-test": {
                    "id": "claude-test",
                    "name": "Claude Test",
                    "tool_call": true,
                    "reasoning": true,
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

        // Dynamic providers get a short doc
        let ollama_doc = &output.provider_docs["ollama"];
        assert!(ollama_doc.contains("`Ollama` LLM provider."));
        assert!(ollama_doc.contains("any model name at runtime"));
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
            // Should not panic for any known variant
            let _ = level_str_to_variant(effort.as_str());
        }
    }
}
