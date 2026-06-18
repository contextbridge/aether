use aether_evals::{EvalRunError, FakeAgent, Task, Workspace};

#[tokio::test]
async fn fake_agent_passes_file_assertion() -> Result<(), EvalRunError> {
    let run = Task::new("Write 'Hello, World!' to hello.txt", Workspace::empty()?)
        .run(&FakeAgent::writes_file("hello.txt", "Hello, World!"))
        .await?;

    assert!(run.workspace().join("hello.txt").exists(), "{}", run.failure_context());

    Ok(())
}

#[tokio::test]
async fn fake_agent_failure_context_describes_missing_file() -> Result<(), EvalRunError> {
    let run = Task::new("Create missing.txt", Workspace::empty()?).run(&FakeAgent::success()).await?;

    assert!(!run.workspace().join("missing.txt").exists());
    let context = run.failure_context();
    assert!(context.contains("Create missing.txt"));
    assert!(context.contains("Task completed successfully"));

    Ok(())
}

#[tokio::test]
async fn tool_call_assertion_works_in_rust_test() -> Result<(), EvalRunError> {
    let run =
        Task::new("Run a bash command", Workspace::empty()?).run(&FakeAgent::with_tool_call("bash", "success")).await?;

    assert_eq!(run.transcript().tool_call_count("bash"), 1, "{}", run.failure_context());

    Ok(())
}
