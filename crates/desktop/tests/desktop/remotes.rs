use crate::support::{choose, replying};
use aether_desktop::remotes::{InstanceState, RemoteSpec};
use aether_desktop::target::SessionTarget;
use aether_desktop::testing::{DesktopHarnessBuilder, launching_remote, static_remote};
use aether_desktop::threads::{Thread, ThreadStatus};
use gpui::{Entity, TestAppContext};
use std::fs;
use std::path::{Path, PathBuf};

#[gpui::test]
fn the_cloud_section_lists_each_remote_below_the_local_projects_with_its_instances(cx: &mut TestAppContext) {
    let harness = patchwave(0)
        .project("/alpha")
        .instance("patchwave", "microvm-a3f9", InstanceState::Running, Some("2026-01-07T13:00:00Z"))
        .instance("patchwave", "microvm-b71c", InstanceState::Suspended, None)
        .build(cx);

    let mut top = |selector| harness.bounds(cx, selector).unwrap_or_else(|| panic!("`{selector}` is painted")).top();
    assert!(top("section-local") < top("project-alpha"));
    assert!(top("project-alpha") < top("section-cloud"), "cloud is below local");
    assert!(top("section-cloud") < top("remote-patchwave"));
    assert!(top("remote-patchwave") < top("instance-row-microvm-a3f9"));
    assert!(top("instance-row-microvm-a3f9") < top("instance-row-microvm-b71c"));
    assert!(harness.shows(cx, "instance-row-microvm-a3f9-title", "a3f9"));
    assert!(harness.shows(cx, "instance-row-microvm-a3f9-subtitle", "1h 00m left"));
    assert!(harness.shows(cx, "instance-row-microvm-b71c-subtitle", "Suspended"));
    assert!(harness.bounds(cx, "instance-row-microvm-b71c-instance-suspended").is_some());
}

#[gpui::test]
fn the_cloud_section_is_hidden_without_remotes(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().project("/alpha").build(cx);

    assert!(harness.bounds(cx, "section-local").is_some());
    assert!(harness.bounds(cx, "section-cloud").is_none());
}

#[gpui::test]
fn only_remotes_that_serve_an_agent_are_listed(cx: &mut TestAppContext) {
    let terminal_only =
        RemoteSpec { shell: Some(vec!["ssh".to_owned(), "box".to_owned()]), ..RemoteSpec::named("box") };
    let harness = patchwave(0).remote(terminal_only).build(cx);

    assert!(harness.bounds(cx, "remote-patchwave").is_some());
    assert!(harness.bounds(cx, "remote-box").is_none(), "a remote with no agent has nothing to start a thread on");
}

#[gpui::test]
fn an_instance_close_to_the_end_of_its_time_warns_how_little_it_has_left(cx: &mut TestAppContext) {
    let harness = patchwave(0)
        .instance("patchwave", "microvm-a3f9", InstanceState::Running, Some("2026-01-07T12:05:00Z"))
        .build(cx);

    assert!(harness.shows(cx, "instance-row-microvm-a3f9-subtitle", "5m left"));
}

#[gpui::test]
fn a_remotes_plus_launches_an_instance_and_lists_its_thread_under_the_remote(cx: &mut TestAppContext) {
    let harness = patchwave(1).project("/alpha").build(cx);

    harness.click(cx, "remote-patchwave-new-thread").unwrap();
    let thread = harness.connected_thread(cx);

    let fleet = harness.fleet(cx);
    harness.run_until(cx, |cx| !fleet.read(cx).instances("patchwave").is_empty());

    assert_eq!(instance_of(&thread, cx).as_deref(), Some("instance-0"));
    assert_eq!(harness.remotes().state_of("patchwave", "instance-0"), Some(InstanceState::Running));
    let remote = harness.bounds(cx, "remote-patchwave").expect("the remote is painted");
    let row = harness.bounds(cx, "thread-row-0").expect("the thread's row is painted");
    assert!(remote.top() < row.top(), "the thread is listed under its remote");
    assert!(harness.bounds(cx, "instance-row-instance-0").is_none(), "the thread stands in for its instance");
    assert!(harness.shows(cx, "thread-row-0-subtitle", "ce-0 · Running"));
    assert_eq!(harness.projects(cx), [PathBuf::from("/alpha")], "the remote's checkout is not added as a project");
}

#[gpui::test]
fn a_static_remote_starts_threads_on_its_one_instance_without_launching(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .script("/devbox", replying("cloud-session-0", "hi"))
        .remote(static_remote("devbox", "/devbox"))
        .build(cx);
    assert!(harness.bounds(cx, "remote-devbox-empty").is_some(), "its one instance is not listed on its own");

    harness.click(cx, "remote-devbox-new-thread").unwrap();
    let thread = harness.connected_thread(cx);

    assert_eq!(instance_of(&thread, cx).as_deref(), Some("devbox"));
    assert_eq!(harness.remotes().launches(), 0);
    assert!(harness.shows(cx, "thread-row-0-subtitle", "devbox"));
    assert!(harness.bounds(cx, "thread-row-0-stop-instance").is_none(), "a static remote cannot be stopped");
}

#[gpui::test]
fn the_new_thread_shortcut_starts_a_local_thread_even_while_a_cloud_thread_is_selected(cx: &mut TestAppContext) {
    let harness = patchwave(1).agent("/alpha", replying("local-session", "hi")).build(cx);
    harness.click(cx, "remote-patchwave-new-thread").unwrap();
    harness.connected_thread(cx);

    harness.simulate_keystrokes(cx, "secondary-n");

    let thread = harness.selected_thread(cx).unwrap();
    assert!(matches!(thread.read_with(cx, |thread, _| thread.target().clone()), SessionTarget::Agent { .. }));
    assert_eq!(thread.read_with(cx, |thread, _| thread.target().cwd().map(Path::to_path_buf)), Some("/alpha".into()));
    assert_eq!(harness.remotes().launches(), 1);
}

#[gpui::test]
fn the_new_cloud_thread_shortcut_starts_a_thread_on_the_first_remote(cx: &mut TestAppContext) {
    let harness = patchwave(1).remote(launching_remote("aether", "/aether")).build(cx);

    harness.simulate_keystrokes(cx, "secondary-alt-n");

    let thread = harness.connected_thread(cx);
    assert_eq!(
        thread.read_with(cx, |thread, _| thread.target().remote_name().map(str::to_owned)).as_deref(),
        Some("patchwave")
    );
}

#[gpui::test]
fn clicking_an_instance_no_thread_is_on_opens_a_thread_on_it(cx: &mut TestAppContext) {
    let harness = patchwave(1).instance("patchwave", "microvm-a3f9", InstanceState::Running, None).build(cx);

    harness.click(cx, "instance-row-microvm-a3f9").unwrap();

    let thread = harness.connected_thread(cx);
    assert_eq!(instance_of(&thread, cx).as_deref(), Some("microvm-a3f9"));
    assert_eq!(harness.remotes().launches(), 0, "nothing new is launched");
    assert!(harness.bounds(cx, "instance-row-microvm-a3f9").is_none());
    assert!(harness.bounds(cx, "thread-row-0").is_some());
}

#[gpui::test]
fn a_cloud_thread_reconnects_to_the_instance_it_launched(cx: &mut TestAppContext) {
    let harness = patchwave(2).build(cx);
    harness.click(cx, "remote-patchwave-new-thread").unwrap();
    let thread = harness.connected_thread(cx);

    harness.connector().kill_agents("/patchwave");
    harness.run_until(cx, |cx| thread.read(cx).status().problem().is_some());
    harness.click(cx, "thread-row-0-reconnect").unwrap();
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(instance_of(&thread, cx).as_deref(), Some("instance-0"));
    assert_eq!(harness.remotes().launches(), 1);
}

#[gpui::test]
fn stopping_a_cloud_threads_instance_ends_the_thread_and_the_instance(cx: &mut TestAppContext) {
    let harness = patchwave(1).build(cx);
    harness.click(cx, "remote-patchwave-new-thread").unwrap();
    harness.connected_thread(cx);

    harness.click(cx, "thread-row-0-stop-instance").unwrap();
    harness.run_until(cx, |_| harness.remotes().state_of("patchwave", "instance-0") == Some(InstanceState::Ended));

    assert!(harness.threads(cx).read_with(cx, |threads, _| threads.threads().is_empty()));
    assert!(harness.bounds(cx, "thread-row-0").is_none());
    assert!(harness.bounds(cx, "instance-row-instance-0").is_none(), "the stopped instance is not listed");
    assert!(harness.bounds(cx, "remote-patchwave-empty").is_some());
}

#[gpui::test]
fn an_instance_is_suspended_and_resumed_from_its_rows_menu(cx: &mut TestAppContext) {
    let harness = patchwave(0).instance("patchwave", "microvm-a3f9", InstanceState::Running, None).build(cx);

    choose(&harness, cx, "instance-row-microvm-a3f9", "menu-suspend");
    harness
        .run_until(cx, |_| harness.remotes().state_of("patchwave", "microvm-a3f9") == Some(InstanceState::Suspended));
    let fleet = harness.fleet(cx);
    harness.run_until(cx, |cx| fleet.read(cx).instances("patchwave")[0].state == InstanceState::Suspended);
    assert!(harness.shows(cx, "instance-row-microvm-a3f9-subtitle", "Suspended"));

    choose(&harness, cx, "instance-row-microvm-a3f9", "menu-resume");
    harness.run_until(cx, |cx| fleet.read(cx).instances("patchwave")[0].state == InstanceState::Running);

    assert_eq!(harness.remotes().state_of("patchwave", "microvm-a3f9"), Some(InstanceState::Running));
}

#[gpui::test]
fn a_remote_whose_instances_cannot_be_listed_says_why(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .remote(launching_remote("broken", "/broken"))
        .failing("broken", "AWS credential setup failed")
        .build(cx);

    assert!(harness.bounds(cx, "remote-broken-problem").is_some());
    let fleet = harness.fleet(cx);
    assert_eq!(
        fleet.read_with(cx, |fleet, _| fleet.inventory().problem("broken").map(str::to_owned)).as_deref(),
        Some("AWS credential setup failed")
    );
}

#[gpui::test]
fn a_remotes_file_that_cannot_be_read_says_why_and_keeps_the_remotes(cx: &mut TestAppContext) {
    let harness = patchwave(0).build(cx);

    fs::write(harness.remotes_file(), "{ not json").unwrap();
    harness.deactivate(cx);
    harness.activate(cx);

    assert!(harness.bounds(cx, "cloud-problem").is_some());
    assert!(harness.bounds(cx, "remote-patchwave").is_some(), "the remotes stay as they were");
}

fn patchwave(agents: usize) -> DesktopHarnessBuilder {
    let builder = DesktopHarnessBuilder::default().remote(launching_remote("patchwave", "/patchwave"));
    (0..agents)
        .fold(builder, |builder, index| builder.script("/patchwave", replying(&format!("cloud-session-{index}"), "hi")))
}

fn instance_of(thread: &Entity<Thread>, cx: &TestAppContext) -> Option<String> {
    thread.read_with(cx, |thread, _| thread.target().instance().map(str::to_owned))
}
