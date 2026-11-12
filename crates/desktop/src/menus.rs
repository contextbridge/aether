use crate::app::{
    NEW_CLOUD_THREAD_KEYS, NEW_THREAD_KEYS, NEXT_THREAD_KEYS, NewCloudThread, NewThread, OPEN_FILE_KEYS, OpenFile,
    OpenSettings, OpenThreadSwitcher, PREVIOUS_THREAD_KEYS, SETTINGS_KEYS, SHOW_CHAT_KEYS, SHOW_FILES_KEYS,
    SWITCHER_KEYS, SelectNextThread, SelectPreviousThread, ShowChat, ShowFiles, TOGGLE_SIDEBAR_KEYS, ToggleSidebar,
    chord,
};
use crate::ui::interface_size::SizeAction;
use clankerdiff_rouge_shell::rouge::{Menu, MenuItem};

pub(crate) const APP_NAME: &str = "Aether";

pub(crate) fn all() -> Vec<Menu> {
    let view = Menu::new(
        "View",
        vec![
            MenuItem::action("Toggle Sidebar", ToggleSidebar).shortcut(chord(TOGGLE_SIDEBAR_KEYS)),
            MenuItem::Separator,
            MenuItem::action("Bigger", SizeAction::Increase),
            MenuItem::action("Smaller", SizeAction::Decrease),
            MenuItem::action("Actual Size", SizeAction::Reset),
        ],
    );
    let threads = Menu::new(
        "Threads",
        vec![
            MenuItem::action("New Thread", NewThread).shortcut(chord(NEW_THREAD_KEYS)),
            MenuItem::action("New Cloud Thread", NewCloudThread).shortcut(chord(NEW_CLOUD_THREAD_KEYS)),
            MenuItem::action("Switch Thread…", OpenThreadSwitcher).shortcut(chord(SWITCHER_KEYS)),
            MenuItem::Separator,
            MenuItem::action("Next Thread", SelectNextThread).shortcut(chord(NEXT_THREAD_KEYS)),
            MenuItem::action("Previous Thread", SelectPreviousThread).shortcut(chord(PREVIOUS_THREAD_KEYS)),
            MenuItem::Separator,
            MenuItem::action("Show Chat", ShowChat).shortcut(chord(SHOW_CHAT_KEYS)),
            MenuItem::action("Show Files", ShowFiles).shortcut(chord(SHOW_FILES_KEYS)),
            MenuItem::action("Open File…", OpenFile).shortcut(chord(OPEN_FILE_KEYS)),
        ],
    );
    let app = vec![MenuItem::action("Settings…", OpenSettings).shortcut(chord(SETTINGS_KEYS))];
    clankerdiff_rouge_shell::menus::build(APP_NAME, app, vec![view, threads])
}
