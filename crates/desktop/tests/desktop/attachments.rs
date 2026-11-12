use crate::support::texts;
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::{FakeAgent, FakeAgentRequests};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::Thread;
use agent_client_protocol::v2::schema as acp;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn a_pasted_screenshot_is_attached_and_sent_with_the_prompt() {
    let (mut harness, _, mut requests, _workspace) = seeing();

    harness.platform().copy_image(PNG);
    harness.press("secondary-v");
    assert!(harness.bounds("composer-attachment-0").is_some());
    assert!(harness.shows("composer-text", ""));
    harness.press("enter");

    let prompt = harness.next(&mut requests.prompt).prompt;
    assert!(prompt.iter().any(|block| matches!(block, acp::ContentBlock::Image(_))), "{prompt:?}");
    assert!(harness.bounds("composer-attachment-0").is_none(), "sending clears the attachments");
}

#[test]
fn copied_files_attach_their_media_and_mention_the_rest() {
    let (mut harness, _, _, workspace) = seeing();
    let photo = write(&workspace, "photo.png", PNG);
    let notes = write(&workspace, "notes.md", b"# notes");

    harness.platform().copy_files(vec![photo, notes]);
    harness.press("secondary-v");

    assert!(harness.shows("composer-text", "@notes.md "));
    assert!(harness.bounds("composer-attachment-0-thumbnail").is_some(), "the photo is attached");
    assert!(harness.bounds("composer-attachment-1").is_none(), "the notes are only mentioned");
}

#[test]
fn copied_text_still_pastes_as_text() {
    let (mut harness, _, _, _workspace) = seeing();

    harness.platform().copy_text("/tmp/report.pdf");
    harness.press("secondary-v");

    assert!(harness.shows("composer-text", "/tmp/report.pdf"));
    assert!(harness.bounds("composer-attachment-0").is_none());
}

#[test]
fn files_dropped_on_the_thread_are_attached() {
    let (mut harness, _, _, workspace) = seeing();
    let photo = write(&workspace, "photo.png", PNG);
    let clip = write(&workspace, "clip.wav", b"RIFF");

    harness.drop_files("transcript", vec![photo, clip]).unwrap();

    assert!(harness.bounds("composer-attachment-0").is_some());
    assert!(harness.bounds("composer-attachment-1").is_some());
    assert!(harness.bounds("composer-attachment-0-thumbnail").is_some(), "an image previews itself");
    assert!(harness.bounds("composer-attachment-1-thumbnail").is_none(), "a sound has no preview");
}

#[test]
fn files_dropped_on_the_changes_are_not_attached() {
    let (mut harness, _, _, workspace) = seeing();
    let photo = write(&workspace, "photo.png", PNG);
    harness.press("secondary-2");

    harness.drop_files("view-switch", vec![photo]).unwrap();
    harness.click("tab-chat").unwrap();

    assert!(harness.bounds("composer-attachment-0").is_none());
}

#[test]
fn a_dropped_folder_is_named_for_a_local_agent_to_open() {
    let (mut harness, _, _, workspace) = seeing();
    let folder = workspace.path().join("assets");
    fs::create_dir_all(&folder).unwrap();

    drop_files(&mut harness, vec![folder.clone()]);

    assert!(harness.shows("composer-text", &format!("{} ", folder.display())));
}

#[test]
fn the_paperclip_attaches_the_chosen_files() {
    let (mut harness, _, _, workspace) = seeing();
    let photo = write(&workspace, "photo.png", PNG);

    harness.click("attach-files").unwrap();
    let prompt = harness.platform().answer_path_prompt(Some(vec![photo])).expect("the paperclip asks for files");

    assert!(!prompt.directories && prompt.multiple, "{prompt:?}");
    harness.run_until_painted("composer-attachment-0");
}

#[test]
fn an_attachment_comes_off_with_its_remove_button_or_backspace() {
    let (mut harness, _, _, workspace) = seeing();
    let paths = vec![write(&workspace, "one.png", PNG), write(&workspace, "two.png", PNG)];
    drop_files(&mut harness, paths);

    harness.click("composer-attachment-0-remove").unwrap();
    assert!(harness.bounds("composer-attachment-1").is_none());
    harness.press("backspace");

    assert!(harness.bounds("composer-attachment-0").is_none());
}

#[test]
fn a_sent_prompt_shows_its_attachments_and_its_image_opens_full_size() {
    let workspace = TempDir::new().unwrap();
    let agent = image_capable().prompt_reply("Nice.");
    let (mut harness, thread) = DesktopHarnessBuilder::default().agent(workspace.path(), agent).start(workspace.path());
    let photo = write(&workspace, "photo.png", PNG);
    let notes = write(&workspace, "notes.md", b"# notes");

    drop_files(&mut harness, vec![photo, notes]);
    harness.type_text("compare");
    harness.press("enter");
    harness.run_until(|_| texts(thread).iter().any(|text| text == "Nice."));

    assert!(harness.bounds("message-attachment-0-1").is_some(), "the mentioned file is shown with the prompt");
    assert!(harness.bounds("message-attachment-0-2").is_some(), "so is the image");
    harness.click("message-attachment-0-2").unwrap();
    assert!(harness.bounds("image-preview").is_some(), "clicking the image opens it full size");
    harness.click("image-preview").unwrap();
    assert!(harness.bounds("image-preview").is_none(), "clicking again closes it");
}

#[test]
fn an_image_the_agent_cannot_take_comes_back_to_the_composer() {
    let workspace = TempDir::new().unwrap();
    let agent = FakeAgent::new("alpha-session");
    let (mut harness, thread) = DesktopHarnessBuilder::default().agent(workspace.path(), agent).start(workspace.path());
    drop_files(&mut harness, vec![write(&workspace, "photo.png", PNG)]);

    harness.type_text("what is this?");
    harness.press("enter");
    harness.run_until(|_| texts(thread).iter().any(|text| text.ends_with("does not support image input.")));

    assert!(harness.shows("composer-text", "what is this?"));
    assert!(harness.bounds("composer-attachment-0").is_some());
}

#[test]
fn a_dropped_folder_is_refused_for_a_remote_agent() {
    let workspace = TempDir::new().unwrap();
    let remote = RemoteServerInfo { cwd: "/vm/workspace".into(), session_id: None };
    let agent = FakeAgent::new("vm-session").remote_server(&remote);
    let (mut harness, thread) = DesktopHarnessBuilder::default().agent(workspace.path(), agent).start(workspace.path());
    let folder = workspace.path().join("assets");
    fs::create_dir_all(&folder).unwrap();

    drop_files(&mut harness, vec![folder]);

    assert!(harness.shows("composer-text", ""));
    assert!(noticed(thread, "Can't attach folders or binary files to a remote agent: assets"));
}

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn seeing() -> (DesktopHarness, Thread, FakeAgentRequests, TempDir) {
    let workspace = TempDir::new().unwrap();
    let (agent, requests) = image_capable().capture();
    let (harness, thread) = DesktopHarnessBuilder::default().agent(workspace.path(), agent).start(workspace.path());
    (harness, thread, requests, workspace)
}

fn image_capable() -> FakeAgent {
    let images = acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new());
    FakeAgent::new("alpha-session")
        .capabilities(acp::AgentCapabilities::new().session(acp::SessionCapabilities::new().prompt(images)))
}

fn write(workspace: &TempDir, name: &str, contents: &[u8]) -> PathBuf {
    let path = workspace.path().join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn drop_files(harness: &mut DesktopHarness, paths: Vec<PathBuf>) {
    harness.drop_files("composer", paths).unwrap();
}

fn noticed(thread: Thread, notice: &str) -> bool {
    texts(thread).iter().any(|text| text.ends_with(notice))
}
