use aether_evals::{Agent, FakeAgent, Task, Transcript, TranscriptError};

#[tokio::test]
async fn tool_call_assertion_works_in_rust_test() -> Result<(), TranscriptError> {
    let prompt = "Run a bash command";
    let trace = Transcript::from_stream(FakeAgent::with_tool_call("bash", "success").run(Task::new(prompt))).await?;
    assert_eq!(trace.tool_call_count("bash"), 1);

    Ok(())
}
