//! Native desktop client for running and managing many concurrent Aether agent threads.

pub mod agents;
mod app;
mod changes;
mod chat;
pub mod cli;
pub mod clock;
mod elicitation;
mod file_index;
mod fuzzy;
pub mod history;
mod menus;
mod notification;
pub mod registry;
mod run_loop;
mod runtime;
pub mod sidebar;
mod switcher;
pub mod target;
#[cfg(feature = "testing")]
pub mod testing;
mod theme;
mod thread_view;
pub mod threads;
pub mod ui;

pub use app::{
    DesktopApp, DesktopConfig, OpenThreadSwitcher, SelectNextThread, SelectPreviousThread, ShowChanges, ShowChat,
};
pub use menus::{About, Hide, HideOthers, Minimize, Quit, ShowAll, Zoom};
pub use theme::{DesktopTheme, ThemePair};

use aether_sessions::SessionStore;
use agents::{Agents, default_agents_file};
use clankerdiff_gpui::DiffViewer;
use cli::Cli;
use clock::SystemClock;
use gpui::{App, AppContext, Bounds, Entity, Pixels, Size, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use registry::{Registry, default_state_file};
use rustls::crypto::aws_lc_rs;
use std::sync::Arc;
use target::AgentConnector;

/// Runs the desktop app until its last window closes or it is quit.
///
/// # Panics
///
/// Panics if the TLS crypto provider cannot be installed, the bundled fonts cannot be loaded or
/// the main window cannot open.
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
            connector: Arc::new(AgentConnector),
            clock: Arc::new(SystemClock),
            agents: Agents::new(cli.agent, default_agents_file()),
            sessions: SessionStore::new().inspect_err(|error| tracing::warn!(%error, "showing no history")).ok(),
            registry,
        };
        cx.open_window(window_options(bounds), |window, cx| cx.new(|cx| DesktopApp::new(config, window, cx)))
            .expect("failed to open the main window");
        cx.activate(true);
    });
    // A blocking shutdown would stall quitting on agent connections that are still running.
    runtime.shutdown_background();
}

/// How the app identifies itself to the window system and the notification center.
const APP_ID: &str = "aether-desktop";
const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1280.0), px(840.0));
const MIN_WINDOW_SIZE: Size<Pixels> = size(px(640.0), px(480.0));

/// Registers the key bindings, actions and menus every window relies on.
fn init(cx: &mut App) {
    cx.set_app_identity(APP_ID, menus::APP_NAME);
    gpui_base::init(cx);
    ui::InterfaceSize::init(ui::interface_size::DEFAULT_INTERFACE_SIZE, cx);
    DiffViewer::bind_keys(cx);
    // Bound before the menus are installed so that the menus can show the shortcuts.
    DesktopApp::bind_keys(cx);
    menus::install(cx);
}

/// The size the main window had when the app last ran, or else the default size.
fn window_size(registry: &Entity<Registry>, cx: &App) -> Size<Pixels> {
    registry.read(cx).window_size().map_or(DEFAULT_WINDOW_SIZE, |size| size.max(&MIN_WINDOW_SIZE))
}

fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some(menus::APP_NAME.into()), ..TitlebarOptions::default() }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(MIN_WINDOW_SIZE),
        app_id: Some(APP_ID.to_owned()),
        ..WindowOptions::default()
    }
}
