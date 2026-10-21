use crate::app::{OpenThreadSwitcher, SelectNextThread, SelectPreviousThread, ShowChanges, ShowChat};
use crate::run_loop;
use crate::sidebar::NewThread;
use gpui::{App, Menu, MenuItem, OsAction, PromptLevel, SystemMenuType, Window, actions};
use gpui_base::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};

actions!(
    aether_desktop,
    [
        /// Shows the app name and version.
        About,
        /// Hides the app (macOS).
        Hide,
        /// Hides every other app (macOS).
        HideOthers,
        /// Minimizes the active window.
        Minimize,
        /// Quits the app, stopping every agent it started.
        Quit,
        /// Shows every hidden app (macOS).
        ShowAll,
        /// Zooms (maximizes or restores) the active window.
        Zoom,
    ]
);

pub(crate) const APP_NAME: &str = "Aether";

pub(crate) fn install(cx: &mut App) {
    cx.on_action(|_: &About, cx| with_active_window(cx, show_about));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &Minimize, cx| with_active_window(cx, |window, _| window.minimize_window()));
    cx.on_action(|_: &Quit, cx| run_loop::stop(cx));
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Zoom, cx| with_active_window(cx, |window, _| window.zoom_window()));
    cx.set_menus(build());
}

fn build() -> Vec<Menu> {
    vec![
        Menu {
            name: APP_NAME.into(),
            disabled: false,
            items: vec![
                MenuItem::action(format!("About {APP_NAME}"), About),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(format!("Hide {APP_NAME}"), Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action(format!("Quit {APP_NAME}"), Quit),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "Threads".into(),
            disabled: false,
            items: vec![
                MenuItem::action("New Thread", NewThread),
                MenuItem::action("Switch Thread…", OpenThreadSwitcher),
                MenuItem::separator(),
                MenuItem::action("Next Thread", SelectNextThread),
                MenuItem::action("Previous Thread", SelectPreviousThread),
                MenuItem::separator(),
                MenuItem::action("Show Chat", ShowChat),
                MenuItem::action("Show Changes", ShowChanges),
            ],
        },
        Menu {
            name: "Window".into(),
            disabled: false,
            items: vec![MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)],
        },
    ]
}

fn show_about(window: &mut Window, cx: &mut App) {
    let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
    // The prompt is informational, so which button closed it does not matter.
    drop(window.prompt(PromptLevel::Info, APP_NAME, Some(&version), &["OK"], cx));
}

fn with_active_window(cx: &mut App, update: impl FnOnce(&mut Window, &mut App)) {
    if let Some(window) = cx.active_window() {
        // The window can close between the menu click and this update; there is nothing to do then.
        let _ = window.update(cx, |_, window, cx| update(window, cx));
    }
}
