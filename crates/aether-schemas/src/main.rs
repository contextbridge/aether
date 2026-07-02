//! Print the JSON schemas used by the TypeScript SDK as a single document, so codegen runs `cargo` once.

fn main() {
    let document = serde_json::json!({
        "AcpOptions": schemars::schema_for!(aether_cli::acp::AcpOptions),
        "AetherSettings": schemars::schema_for!(aether_project::AetherSettings),
        "HeadlessOptions": schemars::schema_for!(aether_cli::headless::HeadlessOptions),
        "AgentEvent": schemars::schema_for!(aether_core::events::AgentEvent),
        "ContextUsage": schemars::schema_for!(aether_core::events::ContextUsage),
        "JudgeRubricResponse": schemars::schema_for!(aether_evals::JudgeRubricResponse),
        "JudgeSummary": schemars::schema_for!(aether_evals::JudgeSummary),
        "JudgeCriterionSpec": schemars::schema_for!(aether_evals::JudgeCriterionSpec),
        "ReasoningEffort": schemars::schema_for!(utils::ReasoningEffort),
    });
    println!("{}", serde_json::to_string_pretty(&document).expect("schema document serializes to JSON"));
}
