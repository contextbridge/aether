use aether_evals::{EvalRunOptions, run_eval_files};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

#[tokio::test]
async fn declarative_spec_runs_example_eval() {
    let dir = tempfile::tempdir().expect("temp eval dir");
    let spec_path = dir.path().join("edit-notes.eval.json");
    let settings_path = aether_repo_root().join(".aether/settings.json");
    std::fs::write(
        &spec_path,
        format!(
            r#"{{
              "docker": {{ "image": "aether-sandbox:latest" }},
              "settings": {},
              "name": "edits_notes_file",
              "prompt": "Use the coding MCP tools to update notes.txt. Read the file first, then call coding__edit_file exactly once to replace only the first 'alpha' with 'beta'. Do not replace the second 'alpha'.",
              "workspace": {{ "files": {{ "notes.txt": "alpha\nalpha\n" }} }},
              "expect": {{
                "toolCalls": {{
                  "coding__read_file": {{ "atLeast": 1 }},
                  "coding__edit_file": {{ "exactly": 1 }}
                }},
                "files": {{ "notes.txt": "beta\nalpha\n" }}
              }}
            }}"#,
            serde_json::to_string(&settings_path).unwrap()
        ),
    )
    .expect("write eval fixture");

    let report = run_eval_files(EvalRunOptions {
        paths: vec![spec_path],
        filter: None,
        max_concurrency: NonZeroUsize::new(1).unwrap(),
    })
    .await
    .expect("eval file runs");

    let outcome = report.evals.first().expect("one eval outcome");
    let context = outcome.failure_context.as_deref().unwrap_or("");
    assert!(outcome.passed, "eval `{}` failed: {:?}\n{context}", outcome.name, outcome.failures);
}

fn aether_repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("packages/internal-evals should live under the repository root")
        .to_path_buf()
}
