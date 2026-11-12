use crate::support::{Edges, choose, replying};
use aether_desktop::remotes::{InstanceState, RemoteSpec};
use aether_desktop::target::SessionTarget;
use aether_desktop::testing::{DesktopHarnessBuilder, launching_remote, static_remote};
use aether_desktop::threads::{Thread, ThreadStatus};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn the_cloud_section_lists_each_remote_below_the_local_projects_with_its_instances() {
    let mut harness = patchwave(0)
        .project("/alpha")
        .instance("patchwave", "microvm-a3f9", InstanceState::Running, Some("2026-01-07T13:00:00Z"))
        .instance("patchwave", "microvm-b71c", InstanceState::Suspended, None)
        .build();

    let mut top = |selector| harness.bounds(selector).unwrap_or_else(|| panic!("`{selector}` is painted")).top();
    assert!(top("section-local") < top("project-alpha"));
    assert!(top("project-alpha") < top("section-cloud"), "cloud is below local");
    assert!(top("section-cloud") < top("remote-patchwave"));
    assert!(top("remote-patchwave") < top("instance-row-microvm-a3f9"));
    assert!(top("instance-row-microvm-a3f9") < top("instance-row-microvm-b71c"));
    assert!(harness.shows("instance-row-microvm-a3f9-title", "a3f9"));
    assert!(harness.shows("instance-row-microvm-a3f9-subtitle", "1h 00m left"));
    assert!(harness.shows("instance-row-microvm-b71c-subtitle", "Suspended"));
    assert!(harness.bounds("instance-row-microvm-b71c-instance-suspended").is_some());
}

#[test]
fn the_cloud_section_is_hidden_without_remotes() {
    let mut harness = DesktopHarnessBuilder::default().project("/alpha").build();

    assert!(harness.bounds("section-local").is_some());
    assert!(harness.bounds("section-cloud").is_none());
}

#[test]
fn only_remotes_that_serve_an_agent_are_listed() {
    let terminal_only =
        RemoteSpec { shell: Some(vec!["ssh".to_owned(), "box".to_owned()]), ..RemoteSpec::named("box") };
    let mut harness = patchwave(0).remote(terminal_only).build();

    assert!(harness.bounds("remote-patchwave").is_some());
    assert!(harness.bounds("remote-box").is_none(), "a remote with no agent has nothing to start a thread on");
}

#[test]
fn an_instance_close_to_the_end_of_its_time_warns_how_little_it_has_left() {
    let mut harness = patchwave(0)
        .instance("patchwave", "microvm-a3f9", InstanceState::Running, Some("2026-01-07T12:05:00Z"))
        .build();

    assert!(harness.shows("instance-row-microvm-a3f9-subtitle", "5m left"));
}

#[test]
fn a_remotes_plus_launches_an_instance_and_lists_its_thread_under_the_remote() {
    let mut harness = patchwave(1).project("/alpha").build();

    harness.click("remote-patchwave-new-thread").unwrap();
    let thread = harness.connected_thread();

    let fleet = harness.fleet();
    harness.run_until(|_| !fleet.instances("patchwave").is_empty());

    assert_eq!(instance_of(thread).as_deref(), Some("instance-0"));
    assert_eq!(harness.remotes().state_of("patchwave", "instance-0"), Some(InstanceState::Running));
    let remote = harness.bounds("remote-patchwave").expect("the remote is painted");
    let row = harness.bounds("thread-row-0").expect("the thread's row is painted");
    assert!(remote.top() < row.top(), "the thread is listed under its remote");
    assert!(harness.bounds("instance-row-instance-0").is_none(), "the thread stands in for its instance");
    assert!(harness.shows("thread-row-0-subtitle", "ce-0 · Running"));
    assert_eq!(harness.projects(), [PathBuf::from("/alpha")], "the remote's checkout is not added as a project");
}

#[test]
fn a_static_remote_starts_threads_on_its_one_instance_without_launching() {
    let mut harness = DesktopHarnessBuilder::default()
        .script("/devbox", replying("cloud-session-0", "hi"))
        .remote(static_remote("devbox", "/devbox"))
        .build();
    assert!(harness.bounds("remote-devbox-empty").is_some(), "its one instance is not listed on its own");

    harness.click("remote-devbox-new-thread").unwrap();
    let thread = harness.connected_thread();

    assert_eq!(instance_of(thread).as_deref(), Some("devbox"));
    assert_eq!(harness.remotes().launches(), 0);
    assert!(harness.shows("thread-row-0-subtitle", "devbox"));
    assert!(harness.bounds("thread-row-0-stop-instance").is_none(), "a static remote cannot be stopped");
}

#[test]
fn the_new_thread_shortcut_starts_a_local_thread_even_while_a_cloud_thread_is_selected() {
    let mut harness = patchwave(1).agent("/alpha", replying("local-session", "hi")).build();
    harness.click("remote-patchwave-new-thread").unwrap();
    harness.connected_thread();

    harness.press("secondary-n");

    let thread = harness.selected_thread().unwrap();
    assert!(matches!(thread.target(), SessionTarget::Agent { .. }));
    assert_eq!(thread.target().cwd().map(Path::to_path_buf), Some("/alpha".into()));
    assert_eq!(harness.remotes().launches(), 1);
}

#[test]
fn the_new_cloud_thread_shortcut_starts_a_thread_on_the_first_remote() {
    let mut harness = patchwave(1).remote(launching_remote("aether", "/aether")).build();

    harness.press("secondary-alt-n");

    let thread = harness.connected_thread();
    assert_eq!(thread.target().remote_name().map(str::to_owned).as_deref(), Some("patchwave"));
}

#[test]
fn clicking_an_instance_no_thread_is_on_opens_a_thread_on_it() {
    let mut harness = patchwave(1).instance("patchwave", "microvm-a3f9", InstanceState::Running, None).build();

    harness.click("instance-row-microvm-a3f9").unwrap();

    let thread = harness.connected_thread();
    assert_eq!(instance_of(thread).as_deref(), Some("microvm-a3f9"));
    assert_eq!(harness.remotes().launches(), 0, "nothing new is launched");
    assert!(harness.bounds("instance-row-microvm-a3f9").is_none());
    assert!(harness.bounds("thread-row-0").is_some());
}

#[test]
fn a_cloud_thread_reconnects_to_the_instance_it_launched() {
    let mut harness = patchwave(2).build();
    harness.click("remote-patchwave-new-thread").unwrap();
    let thread = harness.connected_thread();

    harness.connector().kill_agents("/patchwave");
    harness.run_until(|_| thread.status().problem().is_some());
    harness.click("thread-row-0-reconnect").unwrap();
    harness.run_until(|_| thread.status() == ThreadStatus::Idle);

    assert_eq!(instance_of(thread).as_deref(), Some("instance-0"));
    assert_eq!(harness.remotes().launches(), 1);
}

#[test]
fn stopping_a_cloud_threads_instance_ends_the_thread_and_the_instance() {
    let mut harness = patchwave(1).build();
    harness.click("remote-patchwave-new-thread").unwrap();
    harness.connected_thread();

    harness.click("thread-row-0-stop-instance").unwrap();
    harness.run_until(|harness| harness.remotes().state_of("patchwave", "instance-0") == Some(InstanceState::Ended));

    assert_eq!(harness.threads().threads(), Vec::<Thread>::new());
    assert!(harness.bounds("thread-row-0").is_none());
    assert!(harness.bounds("instance-row-instance-0").is_none(), "the stopped instance is not listed");
    assert!(harness.bounds("remote-patchwave-empty").is_some());
}

#[test]
fn an_instance_is_suspended_and_resumed_from_its_rows_menu() {
    let mut harness = patchwave(0).instance("patchwave", "microvm-a3f9", InstanceState::Running, None).build();

    choose(&mut harness, "instance-row-microvm-a3f9", "menu-item-Suspend");
    harness
        .run_until(|harness| harness.remotes().state_of("patchwave", "microvm-a3f9") == Some(InstanceState::Suspended));
    let fleet = harness.fleet();
    harness.run_until(|_| fleet.instances("patchwave")[0].state == InstanceState::Suspended);
    assert!(harness.shows("instance-row-microvm-a3f9-subtitle", "Suspended"));

    choose(&mut harness, "instance-row-microvm-a3f9", "menu-item-Resume");
    harness.run_until(|_| fleet.instances("patchwave")[0].state == InstanceState::Running);

    assert_eq!(harness.remotes().state_of("patchwave", "microvm-a3f9"), Some(InstanceState::Running));
}

#[test]
fn a_remote_whose_instances_cannot_be_listed_says_why() {
    let mut harness = DesktopHarnessBuilder::default()
        .remote(launching_remote("broken", "/broken"))
        .failing("broken", "AWS credential setup failed")
        .build();

    assert!(harness.bounds("remote-broken-problem").is_some());
    let fleet = harness.fleet();
    assert_eq!(
        fleet.with_inventory(|inventory| inventory.problem("broken").map(str::to_owned)).as_deref(),
        Some("AWS credential setup failed")
    );
}

#[test]
fn a_remotes_file_that_cannot_be_read_says_why_and_keeps_the_remotes() {
    let mut harness = patchwave(0).build();

    fs::write(harness.remotes_file(), "{ not json").unwrap();
    harness.deactivate();
    harness.activate();

    assert!(harness.bounds("cloud-problem").is_some());
    assert!(harness.bounds("remote-patchwave").is_some(), "the remotes stay as they were");
}

fn patchwave(agents: usize) -> DesktopHarnessBuilder {
    let builder = DesktopHarnessBuilder::default().remote(launching_remote("patchwave", "/patchwave"));
    (0..agents)
        .fold(builder, |builder, index| builder.script("/patchwave", replying(&format!("cloud-session-{index}"), "hi")))
}

fn instance_of(thread: Thread) -> Option<String> {
    thread.target().instance().map(str::to_owned)
}
