use aether_desktop::{
    About, OpenThreadSwitcher, Quit, SelectNextThread, SelectPreviousThread, testing::DesktopHarnessBuilder,
};
use gpui::{Action, Keystroke, OwnedMenuItem, TestAppContext};

#[gpui::test]
fn app_menu_offers_about_and_quit(cx: &mut TestAppContext) {
    DesktopHarnessBuilder::default().build(cx);

    let menus = cx.update(|cx| cx.get_menus()).expect("menus are installed");
    let app_menu = menus.first().expect("an app menu");
    let actions: Vec<&dyn Action> = app_menu
        .items
        .iter()
        .filter_map(|item| match item {
            OwnedMenuItem::Action { action, .. } => Some(action.as_ref()),
            _ => None,
        })
        .collect();

    assert!(actions.iter().any(|action| action.partial_eq(&About)));
    assert!(actions.iter().any(|action| action.partial_eq(&Quit)));
}

#[gpui::test]
fn secondary_q_quits(cx: &mut TestAppContext) {
    DesktopHarnessBuilder::default().build(cx);

    let keystroke = Keystroke::parse("secondary-q").expect("valid keystroke");
    let bindings = cx.update(|cx| cx.all_bindings_for_input(&[keystroke]));

    assert!(bindings.iter().any(|binding| binding.action().partial_eq(&Quit)));
}

#[gpui::test]
fn the_threads_menu_offers_the_switcher_and_cycling(cx: &mut TestAppContext) {
    DesktopHarnessBuilder::default().build(cx);

    let menus = cx.update(|cx| cx.get_menus()).expect("menus are installed");
    let threads = menus.iter().find(|menu| menu.name == "Threads").expect("a Threads menu");
    let offers = |expected: &dyn Action| {
        threads
            .items
            .iter()
            .any(|item| matches!(item, OwnedMenuItem::Action { action, .. } if action.partial_eq(expected)))
    };

    assert!(offers(&OpenThreadSwitcher));
    assert!(offers(&SelectNextThread));
    assert!(offers(&SelectPreviousThread));
}
