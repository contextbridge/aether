use aether_evals::{EvalFilesReport, EvalSpecLoadOptions, ResolvedEvalSpec, WorkspaceRetention, run_eval_specs};
use std::num::NonZeroUsize;

#[tokio::test]
async fn declarative_spec_runs_example_eval() {
    let dir = tempfile::tempdir().expect("temp eval dir");
    let spec_path = dir.path().join("edit-notes.eval.json");
    let agent_command = serde_json::to_string(&vec!["/usr/local/bin/aether-eval-agent"]).expect("command serialize");
    std::fs::write(
        &spec_path,
        format!(
            r#"{{
              "docker": {{ "image": "aether-sandbox:latest" }},
              "agent": {{ "command": {agent_command} }},
              "name": "edits_notes_file",
              "task": {{
                "prompt": "Use the coding MCP tools to update notes.txt. Read the file first, then call coding__edit_file exactly once to replace only the first 'alpha' with 'beta'. Do not replace the second 'alpha'.",
                "workspace": {{ "files": {{ "notes.txt": "alpha\nalpha\n" }} }}
              }},
              "expect": {{
                "toolCalls": {{
                  "coding__read_file": {{ "atLeast": 1 }},
                  "coding__edit_file": {{ "exactly": 1 }}
                }},
                "files": {{ "notes.txt": "beta\nalpha\n" }}
              }}
            }}"#
        ),
    )
    .expect("write eval fixture");

    let cases =
        ResolvedEvalSpec::load(EvalSpecLoadOptions { paths: vec![spec_path], filter: None }).expect("eval file loads");
    let evals = run_eval_specs(cases, WorkspaceRetention::Discard, NonZeroUsize::new(1).unwrap())
        .await
        .expect("eval file runs");
    let report = EvalFilesReport { evals };

    let outcome = report.evals.first().expect("one eval outcome");
    let context = outcome.failure_context.as_deref().unwrap_or("");
    assert!(outcome.passed, "eval `{}` failed: {:?}\n{context}", outcome.name, outcome.failures);
}
