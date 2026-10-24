use crate::support::texts;
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::{FakeAgent, FakeAgentRequests};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::Thread;
use agent_client_protocol::schema::v2 as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
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
    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
    harness.simulate_keystrokes(cx, "enter");

    assert_eq!(sent(&harness, cx, &mut requests), [Sent::Image("image/png".into(), PNG.to_vec())]);
    assert!(harness.bounds(cx, "composer-attachment-0").is_none(), "sending clears the attachments");
}

#[gpui::test]
fn copied_files_attach_media_and_mention_text(cx: &mut TestAppContext) {
    let (harness, _, mut requests, workspace) = seeing(cx);
    let photo = write(&workspace, "photo.png", PNG);
    let notes = write(&workspace, "notes.md", b"# notes");

    cx.write_to_clipboard(ClipboardItem {
        entries: vec![
            ClipboardEntry::ExternalPaths(ExternalPaths(vec![photo, notes].into())),
            ClipboardEntry::String(ClipboardString::new("photo.png\nnotes.md".into())),
        ],
    });
    harness.simulate_keystrokes(cx, "secondary-v");
    assert_eq!(harness.composer_text(cx).as_deref(), Some("@notes.md "));
    harness.simulate_input(cx, "compare");
    harness.simulate_keystrokes(cx, "enter");

    let sent = sent(&harness, cx, &mut requests);
    assert_eq!(sent[0], Sent::Text("@notes.md compare".into()));
    assert!(sent.contains(&Sent::Image("image/png".into(), PNG.to_vec())), "{sent:?}");
    assert!(
        sent.iter()
            .any(|block| matches!(block, Sent::Resource(uri, text) if uri.starts_with("file://") && text == "# notes"))
    );
}

#[gpui::test]
fn copied_text_still_pastes_as_text(cx: &mut TestAppContext) {
    let (harness, _, _, _workspace) = seeing(cx);

    cx.write_to_clipboard(ClipboardItem::new_string("/tmp/report.pdf".into()));
    harness.simulate_keystrokes(cx, "secondary-v");

    assert_eq!(harness.composer_text(cx).as_deref(), Some("/tmp/report.pdf"));
    assert!(harness.bounds(cx, "composer-attachment-0").is_none());
}

#[gpui::test]
fn a_clipboard_image_in_a_format_agents_cannot_read_is_refused(cx: &mut TestAppContext) {
    let (harness, thread, _, _workspace) = seeing(cx);

    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Tiff, vec![0x49, 0x49])));
    harness.simulate_keystrokes(cx, "secondary-v");

    assert!(harness.bounds(cx, "composer-attachment-0").is_none());
    assert!(noticed(&thread, cx, "Can't attach TIFF images; paste a PNG, JPEG, GIF or WebP instead."));
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
}

#[gpui::test]
fn a_dropped_folder_is_named_for_a_local_agent_to_open(cx: &mut TestAppContext) {
    let (harness, _, _, workspace) = seeing(cx);
    let folder = workspace.path().join("assets");
    fs::create_dir_all(&folder).unwrap();

    drop_files(&harness, cx, vec![folder.clone()]);

    assert_eq!(harness.composer_text(cx), Some(format!("{} ", folder.display())));
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
fn an_image_the_agent_cannot_take_comes_back_to_the_composer(cx: &mut TestAppContext) {
    let workspace = TempDir::new().unwrap();
    let agent = FakeAgent::default().new_session_response(acp::NewSessionResponse::new("alpha-session"));
    let (harness, thread) = harness_in(cx, &workspace, agent);
    drop_files(&harness, cx, vec![write(&workspace, "photo.png", PNG)]);

    harness.simulate_input(cx, "what is this?");
    harness.simulate_keystrokes(cx, "enter");
    harness
        .run_until(cx, |cx| texts(thread.read(cx)).iter().any(|text| text.ends_with("does not support image input.")));

    assert_eq!(harness.composer_text(cx).as_deref(), Some("what is this?"));
    assert!(harness.bounds(cx, "composer-attachment-0").is_some());
}

#[gpui::test]
fn a_remote_agent_gets_file_contents_but_no_local_paths(cx: &mut TestAppContext) {
    let workspace = TempDir::new().unwrap();
    let remote = RemoteServerInfo { cwd: "/vm/workspace".into(), session_id: None };
    let (agent, mut requests) = FakeAgent::default()
        .new_session_response(acp::NewSessionResponse::new("vm-session"))
        .remote_server(&remote)
        .capture();
    let (harness, thread) = harness_in(cx, &workspace, agent);
    let folder = workspace.path().join("assets");
    fs::create_dir_all(&folder).unwrap();
    let notes = write(&workspace, "notes.md", b"# notes");

    drop_files(&harness, cx, vec![folder]);
    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
    assert!(noticed(&thread, cx, "Can't attach folders or binary files to a remote agent: assets"));
    drop_files(&harness, cx, vec![notes]);
    harness.simulate_keystrokes(cx, "enter");

    let sent = sent(&harness, cx, &mut requests);
    assert_eq!(sent[1], Sent::Resource("attachment:///notes.md".into(), "# notes".into()));
}

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

#[derive(Debug, PartialEq, Eq)]
enum Sent {
    Text(String),
    Image(String, Vec<u8>),
    Resource(String, String),
}

fn seeing(cx: &mut TestAppContext) -> (DesktopHarness, Entity<Thread>, FakeAgentRequests, TempDir) {
    let workspace = TempDir::new().unwrap();
    let images = acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new());
    let (agent, requests) = FakeAgent::default()
        .capabilities(acp::AgentCapabilities::new().session(acp::SessionCapabilities::new().prompt(images)))
        .new_session_response(acp::NewSessionResponse::new("alpha-session"))
        .capture();
    let (harness, thread) = harness_in(cx, &workspace, agent);
    (harness, thread, requests, workspace)
}

fn harness_in(cx: &mut TestAppContext, workspace: &TempDir, agent: FakeAgent) -> (DesktopHarness, Entity<Thread>) {
    let harness = DesktopHarnessBuilder::default().agent(workspace.path(), agent).build(cx);
    let thread = harness.start_thread(cx, workspace.path());
    (harness, thread)
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

fn sent(harness: &DesktopHarness, cx: &mut TestAppContext, requests: &mut FakeAgentRequests) -> Vec<Sent> {
    harness.run_until(cx, |_| !requests.prompt.is_empty());
    let (prompt, _) = requests.prompt.try_recv().unwrap();
    prompt
        .prompt
        .iter()
        .map(|block| match block {
            acp::ContentBlock::Text(text) => Sent::Text(text.text.clone()),
            acp::ContentBlock::Image(image) => {
                Sent::Image(image.mime_type.0.to_string(), BASE64.decode(&image.data).unwrap())
            }
            acp::ContentBlock::Resource(acp::EmbeddedResource {
                resource: acp::EmbeddedResourceResource::TextResourceContents(contents),
                ..
            }) => Sent::Resource(contents.uri.clone(), contents.text.clone()),
            other => panic!("unexpected block {other:?}"),
        })
        .collect()
}
