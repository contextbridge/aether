pub mod agents;
mod app;
mod artifact_review;
mod changes;
mod chat;
pub mod cli;
pub mod clock;
mod elicitation;
pub mod history;
mod menus;
pub mod registry;
pub mod remotes;
mod settings;
pub mod sidebar;
pub mod target;
#[cfg(feature = "testing")]
pub mod testing;
mod theme;
mod thread_view;
pub mod threads;
pub mod ui;

pub use app::{
    DesktopApp, DesktopConfig, OpenSettings, OpenThreadSwitcher, SelectNextThread, SelectPreviousThread, ShowChanges,
    ShowChat,
};
pub use clankerdiff_ui_core::{AppearanceMode, ThemePreference};
pub use theme::DesktopTheme;

use aether_sessions::SessionStore;
use agents::{Agents, default_agents_file};
use clankerdiff_gpui::{DiffViewer, MarkdownReviewer};
use clankerdiff_gpui_shell::{run_loop, runtime};
use clankerdiff_remote::CommandRemotes;
use cli::Cli;
use clock::SystemClock;
use gpui::{App, AppContext, Bounds, Entity, Pixels, Size, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use registry::{Registry, default_state_file};
use rustls::crypto::aws_lc_rs;
use std::sync::Arc;
use target::ProcessTransport;

pub fn run(cli: Cli) {
    aws_lc_rs::default_provider().install_default().expect("failed to install the Rustls AWS-LC crypto provider");
    let runtime = runtime::build();
    let handle = runtime.handle().clone();
    gpui_platform::application().run(move |cx: &mut App| {
        runtime::install(cx, handle);
        ui::load_fonts(cx).expect("failed to load the bundled fonts");
        init(cx);
        run_loop::install(cx);
        let state_file = default_state_file();
        if state_file.is_none() {
            tracing::warn!("no home directory, so nothing is remembered between runs");
        }
        let registry = cx.new(|cx| Registry::load(state_file, cx));
        let bounds = Bounds::centered(None, window_size(&registry, cx), cx);
        let config = DesktopConfig {
            transport: Arc::new(ProcessTransport),
            clock: Arc::new(SystemClock),
            agents: Agents::new(cli.agent, default_agents_file()),
            remotes: Arc::new(CommandRemotes),
            remotes_file: remotes::default_file(),
            sessions: SessionStore::new().inspect_err(|error| tracing::warn!(%error, "showing no history")).ok(),
            registry,
        };
        cx.open_window(window_options(bounds), |window, cx| cx.new(|cx| DesktopApp::new(config, window, cx)))
            .expect("failed to open the main window");
        cx.activate(true);
    });
    runtime.shutdown_background();
}

const APP_ID: &str = "aether-desktop";
const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1280.0), px(840.0));
const MIN_WINDOW_SIZE: Size<Pixels> = size(px(640.0), px(480.0));

fn init(cx: &mut App) {
    cx.set_app_identity(APP_ID, menus::APP_NAME);
    gpui_base::init(cx);
    ui::InterfaceSize::init(ui::interface_size::DEFAULT_INTERFACE_SIZE, cx);
    DiffViewer::bind_keys(cx);
    MarkdownReviewer::bind_keys(cx);
    DesktopApp::bind_keys(cx);
    menus::install(cx);
}

fn window_size(registry: &Entity<Registry>, cx: &App) -> Size<Pixels> {
    registry.read(cx).state().window_size.map_or(DEFAULT_WINDOW_SIZE, |size| size.max(&MIN_WINDOW_SIZE))
}

fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(menus::APP_NAME.into()),
            appears_transparent: cfg!(target_os = "macos"),
            ..TitlebarOptions::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(MIN_WINDOW_SIZE),
        app_id: Some(APP_ID.to_owned()),
        ..WindowOptions::default()
    }
}
