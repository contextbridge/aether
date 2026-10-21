#![cfg(feature = "testing")]

use acp_utils::client::{AcpClientError, AcpClientHandle};
use acp_utils::testing::FakeAgent;
use client_core::runtime::CommandDispatcher;
use tokio::task::LocalSet;
use wisp::command::{Command, CommandResult, HostCommand, HostCommandResult};
use wisp::runtime::TuiHost;
use wisp::settings::{ThemeSettings, UiSettings};
use wisp::theme::{ThemeApplicationError, ThemeLoadError};

#[tokio::test]
async fn theme_load_failure_is_returned_as_a_typed_result() -> Result<(), AcpClientError> {
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?, TuiHost);
    dispatcher.dispatch(Command::Host(HostCommand::ApplyTheme {
        settings: Box::new(UiSettings {
            theme: ThemeSettings::File { file: "../invalid.json".into() },
            ..UiSettings::default()
        }),
    }));
    assert!(matches!(dispatcher.next_result().await,
        Some(CommandResult::Host(HostCommandResult::ThemeApplied(Err(ThemeApplicationError::Load(ThemeLoadError::InvalidFile(file))))))
        if file == "../invalid.json"
    ));
    assert!(!dispatcher.has_pending_tasks());
    Ok(())
}

async fn disconnected_client() -> Result<AcpClientHandle, AcpClientError> {
    LocalSet::new()
        .run_until(async {
            let client = FakeAgent::default().build().await?;
            client.handle.disconnect().await;
            Ok(client.handle)
        })
        .await
}
