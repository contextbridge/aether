//! Print the JSON schemas used by the TypeScript SDK as a single document, so codegen runs `cargo` once.

fn main() {
    let document = serde_json::json!({
        "AcpOptions": schemars::schema_for!(aether_cli::acp::AcpOptions),
        "AetherSettings": schemars::schema_for!(aether_project::AetherSettings),
        "HeadlessOptions": schemars::schema_for!(aether_cli::headless::HeadlessOptions),
        "EvalSpec": aether_evals::EvalSpec::schema(),
        "EvalOutcome": aether_evals::EvalOutcome::schema(),
        "EvalStreamEvent": aether_evals::EvalStreamEvent::schema(),
        "JudgeRubricResponse": schemars::schema_for!(aether_evals::JudgeRubricResponse),
    });
    println!("{}", serde_json::to_string_pretty(&document).expect("schema document serializes to JSON"));
}
