use foxfire_tui::command::{HostCommand, HostCommandResult};
use foxfire_tui::runtime::{Host, TuiHost};
use foxfire_tui::settings::{ThemeSettings, UiSettings};
use foxfire_tui::theme::{ThemeApplicationError, ThemeLoadError};

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
