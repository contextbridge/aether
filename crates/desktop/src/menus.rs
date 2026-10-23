use crate::app::{OpenThreadSwitcher, SelectNextThread, SelectPreviousThread, ShowChanges, ShowChat};
use crate::sidebar::{NewThread, ToggleSidebar};
use crate::ui::interface_size::{DecreaseInterfaceSize, IncreaseInterfaceSize, ResetInterfaceSize};
use gpui::{App, Menu, MenuItem};

pub(crate) const APP_NAME: &str = "Aether";

/// Installs the standard app, edit and window menus around the View and Threads menus.
pub(crate) fn install(cx: &mut App) {
    let view = Menu {
        name: "View".into(),
        disabled: false,
        items: vec![
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::separator(),
            MenuItem::action("Bigger", IncreaseInterfaceSize),
            MenuItem::action("Smaller", DecreaseInterfaceSize),
            MenuItem::action("Actual Size", ResetInterfaceSize),
        ],
    };
    let threads = Menu {
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
    };
    clankerdiff_gpui_shell::menus::install(cx, APP_NAME, env!("CARGO_PKG_VERSION"), vec![view, threads]);
}
