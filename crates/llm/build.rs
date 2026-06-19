use std::path::Path;

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let models_json = manifest_dir.join("models.json");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let output_path = Path::new(&out_dir).join("generated.rs");

    println!("cargo::rerun-if-changed={}", models_json.display());

    let output = llm_codegen::generate(&models_json).unwrap_or_else(|e| {
        panic!("Codegen failed: {e}");
    });

    std::fs::write(&output_path, &output.rust_source).unwrap();

    let docs_dir = Path::new(&out_dir).join("docs");
    std::fs::create_dir_all(&docs_dir).unwrap();
    for (provider_id, markdown) in &output.provider_docs {
        std::fs::write(docs_dir.join(format!("{provider_id}.md")), markdown).unwrap();
    }

    // Combine deepseek + moonshot + zai into a single doc for the openai_compatible module
    let mut combined = String::from(
        "`OpenAI`-compatible LLM providers.\n\nShared infrastructure for providers whose APIs are compatible with the `OpenAI` chat completions format.\n\n",
    );
    for key in ["deepseek", "moonshotai", "zai"] {
        if let Some(doc) = output.provider_docs.get(key) {
            combined.push_str(doc);
            combined.push('\n');
        }
    }
    std::fs::write(docs_dir.join("openai_compatible.md"), combined).unwrap();

    // Combine ollama + llamacpp into a single doc for the local module
    let mut local = String::from(
        "Local LLM providers.\n\nProviders that run models on the local machine without requiring API keys.\n\n",
    );
    for key in ["ollama", "llamacpp"] {
        if let Some(doc) = output.provider_docs.get(key) {
            local.push_str(doc);
            local.push('\n');
        }
    }
    std::fs::write(docs_dir.join("local.md"), local).unwrap();
}
