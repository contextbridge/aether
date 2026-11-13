pub mod agents;
mod app;
mod artifact_review;
mod chat;
pub mod cli;
pub mod clock;
mod elicitation;
mod file_picker;
mod files;
pub mod history;
mod menus;
pub mod platform;
pub mod registry;
pub mod remotes;
mod settings;
pub mod sidebar;
pub mod target;
mod tasks;
#[cfg(feature = "testing")]
pub mod testing;
mod theme;
mod thread_view;
pub mod threads;
pub mod ui;

pub use app::{
    DesktopApp, DesktopConfig, NewCloudThread, NewThread, OpenFile, OpenSettings, OpenThreadSwitcher, SelectNextThread,
    SelectPreviousThread, ShowChat, ShowFiles, ToggleSidebar,
};
pub use clankerdiff_ui_core::{AppearanceMode, ThemePreference};
pub use ui::ActiveReviewTheme;

use aether_sessions::SessionStore;
use agents::{Agents, default_agents_file};
use clankerdiff_remote::CommandRemotes;
use cli::Cli;
use clock::SystemClock;
use platform::SystemPlatform;
use registry::{State, WindowSize, default_state_file};
use rouge::{App, WindowOptions};
use rouge_widgets::bundled_fonts;
use rustls::crypto::aws_lc_rs;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use target::ProcessTransport;
use tokio::runtime::Builder;

pub fn run(cli: Cli) {
    aws_lc_rs::default_provider().install_default().expect("failed to install the Rustls AWS-LC crypto provider");
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to start the desktop Tokio runtime");
    let state_file = default_state_file();
    if state_file.is_none() {
        tracing::warn!("no home directory, so nothing is remembered between runs");
    }
    let (saved, flush) = registry::load(state_file, runtime.handle());
    let options = window_options(&saved.state());
    let config = DesktopConfig {
        transport: Arc::new(ProcessTransport),
        clock: Arc::new(SystemClock),
        agents: Agents::new(cli.agent, default_agents_file()),
        remotes: Arc::new(CommandRemotes),
        remotes_file: remotes::default_file(),
        sessions: SessionStore::new().inspect_err(|error| tracing::warn!(%error, "showing no history")).ok(),
        registry: saved,
        platform: Rc::new(SystemPlatform),
    };
    let app = App::new()
        .fonts(bundled_fonts())
        .tokio(runtime.handle().clone())
        .menus(menus::all())
        .window(options, move || DesktopApp::new(config).view());
    if let Err(error) = app.run() {
        eprintln!("{}: {error}", menus::APP_NAME);
    }
    runtime.block_on(async {
        let _ = tokio::time::timeout(SHUTDOWN_FLUSH, flush).await;
    });
    runtime.shutdown_background();
}

pub const DEFAULT_WINDOW_SIZE: WindowSize = WindowSize { width: 1280.0, height: 840.0 };
pub const MIN_WINDOW_SIZE: WindowSize = WindowSize { width: 640.0, height: 480.0 };

const SHUTDOWN_FLUSH: Duration = Duration::from_secs(2);

#[must_use]
pub fn window_size(state: &State) -> WindowSize {
    state.window_size.map_or(DEFAULT_WINDOW_SIZE, |size| size.at_least(MIN_WINDOW_SIZE))
}

fn window_options(state: &State) -> WindowOptions {
    let size = window_size(state);
    WindowOptions::default()
        .title(menus::APP_NAME)
        .size(size.width, size.height)
        .min_size(MIN_WINDOW_SIZE.width, MIN_WINDOW_SIZE.height)
}
