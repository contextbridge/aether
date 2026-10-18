use llm_codegen::{CodegenError, GeneratedOutput, generate};
use serde_json::Value;
use serde_json::json;
use tempfile::NamedTempFile;

#[test]
fn generated_variants_pascal_case_model_ids() {
    let mut data = minimal_models_dev_json();
    for (id, variant) in [
        ("claude-sonnet-4-5-20250929", "ClaudeSonnet4520250929"),
        ("gemini-2.5-flash", "Gemini25Flash"),
        ("deepseek-chat", "DeepseekChat"),
        ("glm-4.5", "Glm45"),
        ("anthropic/claude-opus-4.6", "AnthropicClaudeOpus46"),
        ("openai/gpt-5.1-codex-max", "OpenaiGpt51CodexMax"),
        ("deepseek/deepseek-r1:free", "DeepseekDeepseekR1Free"),
    ] {
        data["anthropic"]["models"] = json!({id: {"id": id, "name": id, "tool_call": true}});
        let output = generate_from_value(&data).unwrap();
        assert!(output.rust_source.contains(&format!("pub enum AnthropicModel {{\n    {variant},\n}}")), "{id}");
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

    let output = generate_from_value(&data).unwrap();
    let entry = catalog_entry(&output, "BedrockFoundationModel", "with-transport");
    assert!(entry.contains("transport: Some(ModelTransport::OpenAiResponses"));
    assert!(entry.contains("https://example.${AWS_REGION}.api.aws/openai/v1"));
    assert!(catalog_entry(&output, "BedrockFoundationModel", "without-transport").contains("transport: None"));
    assert!(
        output.provider_docs["amazon-bedrock"]
            .contains("| `with-transport` | `https://example.${AWS_REGION}.api.aws/openai/v1` | `responses` |")
    );
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

    let output = generate_from_value(&data).unwrap();
    assert!(catalog_entry(&output, "AnthropicModel", "npm-only").contains("transport: None"));
    assert!(!output.provider_docs["anthropic"].contains("# Models with a dedicated endpoint"));
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
    let error = generate_from_value(&data).err().expect("generation must fail");

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
    let error = generate_from_value(&data).err().expect("generation must fail");

    assert!(matches!(error, CodegenError::IncompleteTransport { ref model_id } if model_id == "incomplete"));
}

#[test]
fn generated_docs_format_context_windows() {
    let mut data = minimal_models_dev_json();
    for (window, label) in [(1_000_000, "1M"), (200_000, "200k"), (8_000, "8k"), (0, "unknown")] {
        data["anthropic"]["models"] = json!({"test": {
            "id": "test", "name": "Test", "tool_call": true, "limit": {"context": window}
        }});
        let output = generate_from_value(&data).unwrap();
        assert!(output.provider_docs["anthropic"].contains(&format!("| `test` | `Test` | `{label}` |")));
    }
}

#[test]
fn generate_sorts_models_and_filters_aliases_and_non_tool_call() {
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

    let output = generate_from_value(&data).unwrap();
    let docs = &output.provider_docs["anthropic"];
    let rows: Vec<_> = docs.lines().filter(|line| line.starts_with("| `")).collect();
    assert_eq!(rows, ["| `a-model` | `A Model` | `1k` |  |  |  |", "| `b-model` | `B Model` | `2k` |  |  |  |"]);
    assert!(output.rust_source.contains("pub enum AnthropicModel {\n    AModel,\n    BModel,\n}"));
    assert!(!output.rust_source.contains("alpha-latest"));
    assert!(!output.rust_source.contains("no-tools"));
}

#[test]
fn generate_extra_source_ids_merges_unique_models_into_provider() {
    let mut data = minimal_models_dev_json();
    zai_extra_models(
        &mut data,
        json!({
            "extra-model": {"id": "extra-model", "name": "Extra Model", "tool_call": true, "limit": {"context": 4000, "output": 0}}
        }),
    );

    let output = generate_from_value(&data).unwrap();
    assert!(output.provider_docs["zai"].contains("| `extra-model` | `Extra Model` | `4k` |"));
    assert!(catalog_entry(&output, "ZAiModel", "extra-model").contains("context_window: 4000"));
}

#[test]
fn generate_extra_source_ids_does_not_duplicate_existing_models() {
    let mut data = minimal_models_dev_json();
    let shared = json!({
        "shared-model": {"id": "shared-model", "name": "Shared Model", "tool_call": true, "limit": {"context": 1000, "output": 0}}
    });
    insert_models(&mut data, "zai", shared.clone());
    insert_models(&mut data, "zai-coding-plan", shared);

    let output = generate_from_value(&data).unwrap();
    assert_eq!(output.provider_docs["zai"].matches("| `shared-model` |").count(), 1);
    assert_eq!(output.rust_source.matches("id: \"shared-model\"").count(), 1);
}

#[test]
fn generate_derives_reasoning_levels_from_source_metadata() {
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

    let output = generate_from_value(&data).unwrap();
    assert_eq!(reasoning_levels(&output, "AnthropicModel", "claude-test"), ["Low", "High", "Max"]);
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
    assert!(matches!(generate_from_value(&data), Err(CodegenError::UnsupportedReasoningEffort { .. })));
}

#[test]
fn generate_preserves_model_pricing_and_omits_codex_subscription_pricing() {
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

    let output = generate_from_value(&data).unwrap();
    let priced = catalog_entry(&output, "AnthropicModel", "priced");
    for field in [
        "input_per_million: 3f64",
        "output_per_million: 15f64",
        "cache_read_per_million: Some(0.3f64)",
        "cache_write_per_million: Some(3.75f64)",
    ] {
        assert!(priced.contains(field), "{priced}");
    }
    let codex = catalog_entry(&output, "CodexModel", "gpt-5.5");
    assert!(codex.contains("pricing: None"));
    assert!(codex.contains("supports_prompt_caching: true"));
}

#[test]
fn generate_derives_prompt_caching_from_cost_fields() {
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

    let output = generate_from_value(&data).unwrap();
    assert!(catalog_entry(&output, "BedrockFoundationModel", "cached").contains("supports_prompt_caching: true"));
    assert!(catalog_entry(&output, "BedrockFoundationModel", "uncached").contains("supports_prompt_caching: false"));
}

#[test]
fn generate_assigns_codex_model_specific_reasoning_levels() {
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

    let output = generate_from_value(&data).unwrap();
    assert_eq!(reasoning_levels(&output, "CodexModel", "gpt-5.6-sol"), ["Low", "Medium", "High", "Xhigh", "Max"]);
    assert_eq!(reasoning_levels(&output, "CodexModel", "gpt-5.6-luna"), ["Low", "Medium", "High", "Xhigh", "Max"]);
    assert_eq!(reasoning_levels(&output, "CodexModel", "gpt-5.4"), ["Low", "Medium", "High", "Xhigh"]);
}

#[test]
fn generate_applies_codex_subscription_context_window_override() {
    let output = generate_from_value(&minimal_models_dev_json()).unwrap();
    for &id in CODEX_MODELS {
        assert!(catalog_entry(&output, "CodexModel", id).contains("context_window: 272_000"), "{id}");
        assert!(catalog_entry(&output, "OpenaiModel", id).contains("context_window: 1_050_000"), "{id}");
    }
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

    let output = generate_from_value(&data).unwrap();

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

    let output = generate_from_value(&data).unwrap();

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
    let docs = generate_from_value(&data).unwrap().provider_docs.remove("amazon-bedrock").unwrap();

    for version in ["6-luna", "6-sol", "6-astra", "6.1-sol"] {
        let id = format!("openai.gpt-{version}");
        assert!(docs.contains(&format!(
            "| `us.{id}` | `https://bedrock-runtime.${{AWS_REGION}}.amazonaws.com/openai/v1` | `responses` |"
        )));
        assert!(!docs.contains(&format!("| `global.{id}` | `https://bedrock-runtime.")));
    }
    assert!(
        docs.contains(
            "| `openai.gpt-6-astra` | `https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1` | `responses` |"
        )
    );
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
    let output = generate_from_value(&data).unwrap();
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
        let output = generate_from_value(&data).unwrap();
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

fn generate_from_value(data: &Value) -> Result<GeneratedOutput, CodegenError> {
    let tmp = NamedTempFile::new().unwrap();
    serde_json::to_writer(tmp.as_file(), data).unwrap();
    generate(tmp.path())
}

fn catalog_entry(output: &GeneratedOutput, catalog: &str, id: &str) -> String {
    let implementation = output.rust_source.split_once(&format!("impl {catalog} {{")).unwrap().1;
    let implementation = implementation.split("\n}\n").next().unwrap();
    implementation
        .split("CatalogEntry {")
        .find(|entry| entry.trim_start().starts_with(&format!("id: {id:?},")))
        .unwrap_or_else(|| panic!("missing {catalog} entry for {id}"))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn reasoning_levels(output: &GeneratedOutput, catalog: &str, id: &str) -> Vec<String> {
    let entry = catalog_entry(output, catalog, id);
    let levels = entry.split_once("reasoning_levels: &[").unwrap().1.split(']').next().unwrap();
    levels.split(',').filter_map(|level| level.trim().strip_prefix("ReasoningEffort::").map(str::to_string)).collect()
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
    for provider in [
        "anthropic",
        "azure",
        "deepseek",
        "fireworks-ai",
        "google",
        "moonshotai",
        "openai",
        "openrouter",
        "xiaomi",
        "zai",
        "zai-coding-plan",
        "amazon-bedrock",
    ] {
        root.insert(provider.to_string(), json!({"models": {}}));
    }
    let openai = root.get_mut("openai").unwrap()["models"].as_object_mut().unwrap();
    for &id in CODEX_MODELS {
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
const CODEX_MODELS: &[&str] = &[
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
