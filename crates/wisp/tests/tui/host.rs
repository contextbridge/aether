use wisp::command::{HostCommand, HostCommandResult};
use wisp::runtime::{Host, TuiHost};
use wisp::settings::{ThemeSettings, UiSettings};
use wisp::theme::{ThemeApplicationError, ThemeLoadError};

#[tokio::test]
async fn theme_load_failure_is_returned_as_a_typed_result() {
    let mut host = TuiHost::default();
    let settings =
        UiSettings { theme: ThemeSettings::File { file: "../invalid.json".into() }, ..UiSettings::default() };

    host.execute(HostCommand::ApplyTheme { settings: Box::new(settings) });

    assert!(matches!(host.next().await,
        HostCommandResult::ThemeApplied(Err(ThemeApplicationError::Load(ThemeLoadError::InvalidFile(file))))
        if file == "../invalid.json"
    ));
    host.shutdown().await;
}
