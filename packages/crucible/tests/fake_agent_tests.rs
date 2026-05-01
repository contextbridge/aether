use crucible::{EvalRunError, FakeAgent, Workspace, run_eval};

#[tokio::test]
async fn fake_agent_passes_file_assertion() -> Result<(), EvalRunError> {
    let report = run_eval(
        &FakeAgent::writes_file("hello.txt", "Hello, World!"),
        "Write 'Hello, World!' to hello.txt",
        Workspace::empty()?,
    )
    .await?;

    assert!(report.path("hello.txt").exists(), "{}", report.failure_context());

    Ok(())
}

#[tokio::test]
async fn fake_agent_failure_context_describes_missing_file() -> Result<(), EvalRunError> {
    let report = run_eval(&FakeAgent::success(), "Create missing.txt", Workspace::empty()?).await?;

    assert!(!report.path("missing.txt").exists());
    let context = report.failure_context();
    assert!(context.contains("Create missing.txt"));
    assert!(context.contains("Task completed successfully"));

    Ok(())
}

#[tokio::test]
async fn tool_call_assertion_works_in_rust_test() -> Result<(), EvalRunError> {
    let report =
        run_eval(&FakeAgent::with_tool_call("bash", "success"), "Run a bash command", Workspace::empty()?).await?;

    assert_eq!(report.tool_call_count("bash"), 1, "{}", report.failure_context());

    Ok(())
}
