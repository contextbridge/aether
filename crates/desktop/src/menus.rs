use crate::app::{OpenSettings, OpenThreadSwitcher, SelectNextThread, SelectPreviousThread, ShowChat, ShowFiles};
use crate::sidebar::{NewCloudThread, NewThread, ToggleSidebar};
use crate::ui::interface_size::{DecreaseInterfaceSize, IncreaseInterfaceSize, ResetInterfaceSize};
use gpui::{App, Menu, MenuItem};

pub(crate) const APP_NAME: &str = "Aether";

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
            MenuItem::action("New Cloud Thread", NewCloudThread),
            MenuItem::action("Switch Thread…", OpenThreadSwitcher),
            MenuItem::separator(),
            MenuItem::action("Next Thread", SelectNextThread),
            MenuItem::action("Previous Thread", SelectPreviousThread),
            MenuItem::separator(),
            MenuItem::action("Show Chat", ShowChat),
            MenuItem::action("Show Files", ShowFiles),
        ],
    };
    let app = vec![MenuItem::action("Settings…", OpenSettings)];
    clankerdiff_gpui_shell::menus::install(cx, APP_NAME, env!("CARGO_PKG_VERSION"), app, vec![view, threads]);
}
