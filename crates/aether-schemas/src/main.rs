use aether_cli::acp::AcpOptions;
use aether_cli::headless::HeadlessOptions;
use aether_core::core::AgentEvent;
use aether_evals::{GitSpec, JudgeCriterionSpec, JudgeRubricResponse, JudgeSummary};
use aether_project::AetherSettings;
use llm::SessionUsageEvent;
use utils::schema_document::SchemaDocument;

fn main() {
    SchemaDocument::default()
        .output::<AgentEvent>()
        .output::<SessionUsageEvent>()
        .output::<JudgeSummary>()
        .input::<JudgeRubricResponse>()
        .input::<JudgeCriterionSpec>()
        .input::<GitSpec>()
        .input::<AetherSettings>()
        .input::<AcpOptions>()
        .input::<HeadlessOptions>()
        .print();
}
