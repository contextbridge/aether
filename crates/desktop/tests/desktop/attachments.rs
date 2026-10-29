use crate::support::texts;
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::{FakeAgent, FakeAgentRequests};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::Thread;
use agent_client_protocol::schema::v2 as acp;
use gpui::{
    ClipboardEntry, ClipboardItem, ClipboardString, Entity, ExternalPaths, FileDropEvent, Image, ImageFormat,
    TestAppContext,
};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[gpui::test]
fn a_pasted_screenshot_is_attached_and_sent_with_the_prompt(cx: &mut TestAppContext) {
    let (harness, _, mut requests, _workspace) = seeing(cx);

    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, PNG.to_vec())));
    harness.simulate_keystrokes(cx, "secondary-v");
    assert!(harness.bounds(cx, "composer-attachment-0").is_some());
    assert!(harness.shows(cx, "composer-text", ""));
    harness.simulate_keystrokes(cx, "enter");

    let prompt = harness.next(cx, &mut requests.prompt).prompt;
    assert!(prompt.iter().any(|block| matches!(block, acp::ContentBlock::Image(_))), "{prompt:?}");
    assert!(harness.bounds(cx, "composer-attachment-0").is_none(), "sending clears the attachments");
}

#[gpui::test]
fn copied_files_attach_their_media_and_mention_the_rest(cx: &mut TestAppContext) {
    let (harness, _, _, workspace) = seeing(cx);
    let photo = write(&workspace, "photo.png", PNG);
    let notes = write(&workspace, "notes.md", b"# notes");

    cx.write_to_clipboard(ClipboardItem {
        entries: vec![
            ClipboardEntry::ExternalPaths(ExternalPaths(vec![photo, notes].into())),
            ClipboardEntry::String(ClipboardString::new("photo.png\nnotes.md".into())),
        ],
    });
    harness.simulate_keystrokes(cx, "secondary-v");

    assert!(harness.shows(cx, "composer-text", "@notes.md "));
    assert!(harness.bounds(cx, "composer-attachment-0-thumbnail").is_some(), "the photo is attached");
    assert!(harness.bounds(cx, "composer-attachment-1").is_none(), "the notes are only mentioned");
}

#[gpui::test]
fn copied_text_still_pastes_as_text(cx: &mut TestAppContext) {
    let (harness, _, _, _workspace) = seeing(cx);

    cx.write_to_clipboard(ClipboardItem::new_string("/tmp/report.pdf".into()));
    harness.simulate_keystrokes(cx, "secondary-v");

    assert!(harness.shows(cx, "composer-text", "/tmp/report.pdf"));
    assert!(harness.bounds(cx, "composer-attachment-0").is_none());
}

#[gpui::test]
fn a_clipboard_image_in_a_format_agents_cannot_read_is_refused(cx: &mut TestAppContext) {
    let (harness, thread, _, _workspace) = seeing(cx);

    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Tiff, vec![0x49, 0x49])));
    harness.simulate_keystrokes(cx, "secondary-v");

    assert!(harness.bounds(cx, "composer-attachment-0").is_none());
    assert!(noticed(&thread, cx, "Can't attach TIFF images; paste a PNG, JPEG, GIF or WebP instead."));
    assert!(harness.bounds(cx, "transcript-error-0").is_some(), "the refusal shows as an error");
}

#[gpui::test]
fn files_dropped_on_the_thread_are_attached(cx: &mut TestAppContext) {
    let (harness, _, _, workspace) = seeing(cx);
    let photo = write(&workspace, "photo.png", PNG);
    let clip = write(&workspace, "clip.wav", b"RIFF");
    let position = harness.center(cx, "transcript").unwrap();

    harness.simulate_event(cx, FileDropEvent::Entered { position, paths: ExternalPaths(vec![photo, clip].into()) });
    harness.simulate_event(cx, FileDropEvent::Submit { position });

    assert!(harness.bounds(cx, "composer-attachment-0").is_some());
    assert!(harness.bounds(cx, "composer-attachment-1").is_some());
    assert!(harness.bounds(cx, "composer-attachment-0-thumbnail").is_some(), "an image previews itself");
    assert!(harness.bounds(cx, "composer-attachment-1-thumbnail").is_none(), "a sound has no preview");
}

#[gpui::test]
fn files_dropped_on_the_changes_are_not_attached(cx: &mut TestAppContext) {
    let (harness, _, _, workspace) = seeing(cx);
    let photo = write(&workspace, "photo.png", PNG);
    harness.simulate_keystrokes(cx, "secondary-2");
    let position = harness.center(cx, "view-switch").unwrap();

    harness.simulate_event(cx, FileDropEvent::Entered { position, paths: ExternalPaths(vec![photo].into()) });
    harness.simulate_event(cx, FileDropEvent::Submit { position });
    harness.click(cx, "tab-chat").unwrap();

    assert!(harness.bounds(cx, "composer-attachment-0").is_none());
}

#[gpui::test]
fn a_dropped_folder_is_named_for_a_local_agent_to_open(cx: &mut TestAppContext) {
    let (harness, _, _, workspace) = seeing(cx);
    let folder = workspace.path().join("assets");
    fs::create_dir_all(&folder).unwrap();

    drop_files(&harness, cx, vec![folder.clone()]);

    assert!(harness.shows(cx, "composer-text", &format!("{} ", folder.display())));
}

#[gpui::test]
fn the_paperclip_attaches_the_chosen_files(cx: &mut TestAppContext) {
    let (harness, _, _, workspace) = seeing(cx);
    let photo = write(&workspace, "photo.png", PNG);

    harness.click(cx, "attach-files").unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && options.multiple);
        Some(vec![photo])
    });
    cx.run_until_parked();

    assert!(harness.bounds(cx, "composer-attachment-0").is_some());
}

#[gpui::test]
fn an_attachment_comes_off_with_its_remove_button_or_backspace(cx: &mut TestAppContext) {
    let (harness, _, _, workspace) = seeing(cx);
    let paths = vec![write(&workspace, "one.png", PNG), write(&workspace, "two.png", PNG)];
    drop_files(&harness, cx, paths);

    harness.click(cx, "composer-attachment-0-remove").unwrap();
    assert!(harness.bounds(cx, "composer-attachment-1").is_none());
    harness.simulate_keystrokes(cx, "backspace");

    assert!(harness.bounds(cx, "composer-attachment-0").is_none());
}

#[gpui::test]
fn a_sent_prompt_shows_its_attachments_and_its_image_opens_full_size(cx: &mut TestAppContext) {
    let workspace = TempDir::new().unwrap();
    let agent = image_capable().prompt_reply("Nice.");
    let (harness, thread) = DesktopHarnessBuilder::default().agent(workspace.path(), agent).start(cx, workspace.path());
    let photo = write(&workspace, "photo.png", PNG);
    let notes = write(&workspace, "notes.md", b"# notes");

    drop_files(&harness, cx, vec![photo, notes]);
    harness.simulate_input(cx, "compare");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |cx| texts(thread.read(cx)).iter().any(|text| text == "Nice."));

    assert!(harness.bounds(cx, "message-attachment-0-1").is_some(), "the mentioned file is shown with the prompt");
    assert!(harness.bounds(cx, "message-attachment-0-2").is_some(), "so is the image");
    harness.click(cx, "message-attachment-0-2").unwrap();
    assert!(harness.bounds(cx, "image-preview").is_some(), "clicking the image opens it full size");
    harness.click(cx, "image-preview").unwrap();
    assert!(harness.bounds(cx, "image-preview").is_none(), "clicking again closes it");
}

#[gpui::test]
fn an_image_the_agent_cannot_take_comes_back_to_the_composer(cx: &mut TestAppContext) {
    let workspace = TempDir::new().unwrap();
    let agent = FakeAgent::new("alpha-session");
    let (harness, thread) = DesktopHarnessBuilder::default().agent(workspace.path(), agent).start(cx, workspace.path());
    drop_files(&harness, cx, vec![write(&workspace, "photo.png", PNG)]);

    harness.simulate_input(cx, "what is this?");
    harness.simulate_keystrokes(cx, "enter");
    harness
        .run_until(cx, |cx| texts(thread.read(cx)).iter().any(|text| text.ends_with("does not support image input.")));

    assert!(harness.shows(cx, "composer-text", "what is this?"));
    assert!(harness.bounds(cx, "composer-attachment-0").is_some());
}

#[gpui::test]
fn a_dropped_folder_is_refused_for_a_remote_agent(cx: &mut TestAppContext) {
    let workspace = TempDir::new().unwrap();
    let remote = RemoteServerInfo { cwd: "/vm/workspace".into(), session_id: None };
    let agent = FakeAgent::new("vm-session").remote_server(&remote);
    let (harness, thread) = DesktopHarnessBuilder::default().agent(workspace.path(), agent).start(cx, workspace.path());
    let folder = workspace.path().join("assets");
    fs::create_dir_all(&folder).unwrap();

    drop_files(&harness, cx, vec![folder]);

    assert!(harness.shows(cx, "composer-text", ""));
    assert!(noticed(&thread, cx, "Can't attach folders or binary files to a remote agent: assets"));
}

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn seeing(cx: &mut TestAppContext) -> (DesktopHarness, Entity<Thread>, FakeAgentRequests, TempDir) {
    let workspace = TempDir::new().unwrap();
    let (agent, requests) = image_capable().capture();
    let (harness, thread) = DesktopHarnessBuilder::default().agent(workspace.path(), agent).start(cx, workspace.path());
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

fn drop_files(harness: &DesktopHarness, cx: &mut TestAppContext, paths: Vec<PathBuf>) {
    let position = harness.center(cx, "composer").unwrap();
    harness.simulate_event(cx, FileDropEvent::Entered { position, paths: ExternalPaths(paths.into()) });
    harness.simulate_event(cx, FileDropEvent::Submit { position });
}

fn noticed(thread: &Entity<Thread>, cx: &TestAppContext, notice: &str) -> bool {
    cx.read(|cx| texts(thread.read(cx)).iter().any(|text| text.ends_with(notice)))
}
