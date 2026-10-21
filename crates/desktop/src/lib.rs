//! Native desktop client for running and managing many concurrent Aether agent threads.

mod app;
mod changes;
mod chat;
pub mod cli;
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
mod window_chrome;

pub use app::{
    DesktopApp, DesktopConfig, OpenThreadSwitcher, SelectNextThread, SelectPreviousThread, ShowChanges, ShowChat,
};
pub use menus::{About, Hide, HideOthers, Minimize, Quit, ShowAll, Zoom};
pub use sidebar::SIDEBAR_WIDTH;
pub use theme::{DesktopTheme, ThemePair};

use aether_sessions::SessionStore;
use clankerdiff_gpui::{DiffViewer, load_default_fonts};
use cli::Cli;
use gpui::{App, AppContext, Bounds, Entity, Pixels, Size, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use registry::{Registry, default_state_file};
use std::sync::Arc;
use target::{AgentCommandLine, LocalConnector};

/// Runs the desktop app until its last window closes or it is quit.
///
/// # Panics
///
/// Panics if the bundled fonts cannot be loaded or the main window cannot open.
pub fn run(cli: Cli) {
    let runtime = runtime::build();
    let handle = runtime.handle().clone();
    gpui_platform::application().run(move |cx: &mut App| {
        runtime::install(cx, handle);
        load_default_fonts(cx).expect("failed to load the bundled fonts");
        init(cx);
        run_loop::install(cx);
        let state_file = default_state_file();
        if state_file.is_none() {
            tracing::warn!("no home directory, so nothing is remembered between runs");
        }
        let registry = cx.new(|cx| Registry::load(state_file, cx));
        let bounds = Bounds::centered(None, window_size(&registry, cx), cx);
        let config = DesktopConfig {
            connector: Arc::new(LocalConnector),
            agent: AgentCommandLine::new(cli.agent),
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
