use super::support::*;

#[test]
fn a_remote_workspace_attaches_pasted_media_too() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::remote_workspace).build();

    paste_media(&mut ui, "photo.png");

    let attached: Vec<_> = ui.app().composer().pending_media().iter().map(|media| &media.display_name).collect();
    assert_eq!(attached, ["photo.png"]);
    assert_eq!(ui.app().composer().text(), "");
}

#[test]
fn pasting_an_existing_media_path_in_any_form_a_terminal_drops_attaches_it() {
    let tmp = TestWorkspace::new();
    let file = |name: &str| tmp.write(name, b"media").display().to_string();
    for (name, payload) in [
        ("photo.png", file("photo.png")),
        ("note.wav", file("note.wav")),
        ("image.png", format!("file://{}", file("image.png"))),
        ("my image.png", format!("file://{}", file("my image.png")).replace(' ', "%20")),
        ("my image.png", format!("'{}'", file("my image.png"))),
        ("my image.png", format!("\"{}\"", file("my image.png"))),
        ("my image.png", file("my image.png").replace(' ', "\\ ")),
    ] {
        let mut ui = TestUi::new();

        ui.paste(&payload);

        let attached: Vec<_> = ui.app().composer().pending_media().iter().map(|media| &media.display_name).collect();
        assert_eq!(attached, [name], "pasting {name} should attach it");
        assert_eq!(ui.app().composer().text(), "", "an attached path is consumed");
    }
}

#[test]
fn pasting_a_text_files_path_mentions_it() {
    let tmp = TestWorkspace::new();
    let readme = tmp.write("readme.txt", b"hello").display().to_string();
    let mut ui = TestUi::new();

    ui.paste(&readme);

    assert_eq!(ui.app().composer().pending_media().len(), 0);
    assert_eq!(ui.app().composer().text(), "@readme.txt ");
}

#[test]
fn pastes_that_are_not_all_existing_files_stay_text() {
    let tmp = TestWorkspace::new();
    let photo = tmp.write("photo.png", b"img").display().to_string();
    let missing = tmp.path("gone.wav").display().to_string();
    // A directory with a media-like extension is the dangerous case.
    let directory = tmp.path("clips.png");
    std::fs::create_dir(&directory).unwrap();
    let directory = directory.display().to_string();
    for input in [
        "hello world".to_string(),
        missing.clone(),
        format!("file://{missing}"),
        directory.clone(),
        format!("{}\n{missing}", tmp.path("a.png").display()),
        format!("{photo}\n{missing}"),
        format!("{photo}\n{directory}"),
    ] {
        let mut ui = TestUi::new();

        ui.paste(&input);

        assert_eq!(ui.app().composer().pending_media().len(), 0, "{input:?} should attach nothing");
        assert_eq!(ui.app().composer().text(), input);
    }
}

#[test]
fn paste_multiple_dropped_files_adds_all() {
    let mut app = TestUi::new();
    let tmp = TestWorkspace::new();
    let img = tmp.write("a.png", b"img");
    let audio = tmp.write("b.wav", b"audio");
    let input = format!("{}\n{}", img.display(), audio.display());

    app.paste(&input);

    assert_eq!(app.app().composer().pending_media().len(), 2);
}

#[test]
fn duplicate_dropped_media_not_added_twice() {
    let mut app = TestUi::new();
    let tmp = TestWorkspace::new();
    let img = tmp.write("photo.png", b"img");
    let path_str = img.to_str().unwrap().to_string();

    app.paste(&path_str);
    app.paste(&path_str);

    assert_eq!(app.app().composer().pending_media().len(), 1);
}

#[test]
fn media_only_submit_sends_with_content_blocks() {
    let mut app = TestUiBuilder::new().session(|session| session.prompt_capabilities(media_caps())).build();
    submit_media(&mut app, "photo.png");

    let request = sent_prompt(&mut app);
    assert_eq!(prompt_text(&request), None, "media-only send should carry no text");
    assert!(!request.prompt.is_empty(), "media-only send should carry content blocks");
}

#[test]
fn submit_with_text_and_media_merges_both() {
    let mut app = TestUiBuilder::new().session(|session| session.prompt_capabilities(media_caps())).build();
    paste_media(&mut app, "photo.png");
    app.type_text("describe this");
    app.key(key(KeyCode::Enter));

    let request = sent_prompt(&mut app);
    assert_eq!(prompt_text(&request), Some("describe this"));
    assert_eq!(request.prompt.len(), 2, "the media follows the text");
}

#[test]
fn submit_clears_pending_media() {
    let mut app = TestUiBuilder::new().session(|session| session.prompt_capabilities(media_caps())).build();
    submit_media(&mut app, "photo.png");

    assert_eq!(app.app().composer().pending_media().len(), 0);
    assert_eq!(app.app().composer().text(), "");
}

#[test]
fn backspace_on_empty_composer_removes_last_dropped_media() {
    let mut app = TestUi::new();
    paste_media(&mut app, "a.png");
    paste_media(&mut app, "b.png");
    assert_eq!(app.app().composer().pending_media().len(), 2);

    app.key(key(KeyCode::Backspace));
    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().pending_media()[0].display_name, "a.png");

    app.key(key(KeyCode::Backspace));
    assert_eq!(app.app().composer().pending_media().len(), 0);
}

#[test]
fn backspace_does_not_remove_media_when_text_present() {
    let mut app = TestUi::new();
    paste_media(&mut app, "photo.png");
    app.type_text("x");
    app.key(key(KeyCode::Backspace));

    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().text(), "");
}

#[test]
fn attachment_chips_render_in_layout() {
    let mut app = TestUi::new();
    paste_media(&mut app, "photo.png");

    let layout = app.composer_layout(80);
    let text: String = layout
        .lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("attached image: photo.png"));
}

#[test]
fn text_only_submit_unaffected_by_media_capability_check() {
    let caps = acp::PromptCapabilities::new().image(None).audio(None);
    let mut app = TestUiBuilder::new().session(|session| session.prompt_capabilities(caps)).build();

    app.submit("hello");

    assert_eq!(sent_prompt(&mut app).prompt, vec![acp::ContentBlock::from("hello")]);
}

#[test]
fn submit_is_blocked_when_composer_empty_without_media() {
    let mut app = TestUi::new();

    app.key(key(KeyCode::Enter));

    assert!(!sent_any_prompt(&mut app));
}

#[test]
fn clear_command_also_clears_pending_media() {
    let mut app = TestUi::new();
    paste_media(&mut app, "photo.png");
    app.run_command("/clear");

    assert_eq!(app.app().composer().pending_media().len(), 0);
    assert_eq!(app.app().composer().text(), "");
}

#[test]
fn sync_failure_returns_the_text_to_the_composer() {
    let mut app = TestUiBuilder::new().session(|session| session.prompt_capabilities(media_caps())).build();
    app.refuse_prompts(agent_error("connection closed"));
    let img = paste_media(&mut app, "photo.png");
    app.type_text("describe this");
    app.key(key(KeyCode::Enter));
    sent_prompt(&mut app);

    assert!(!app.app().core().waiting_for_response(), "failed prompt should reset busy state");
    assert!(!sent_any_prompt(&mut app), "no follow-up prompt should be sent");

    assert_eq!(app.app().composer().text(), "describe this");
    assert_eq!(pending_media_paths(&app), [&img]);
    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().all(|msg| !msg.contains("describe this")), "an unsent prompt stays out of the transcript");
    assert!(messages.iter().any(|msg| msg.contains("Failed to send prompt")), "error message shown");
}

fn media_caps() -> acp::PromptCapabilities {
    acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new()).audio(acp::PromptAudioCapabilities::new())
}

fn pending_media_paths(app: &TestUi) -> Vec<&PathBuf> {
    app.app().composer().pending_media().iter().map(|attachment| &attachment.path).collect()
}

fn paste_media(ui: &mut TestUi, name: &str) -> PathBuf {
    let path = ui.workspace().join(name);
    std::fs::write(&path, b"media bytes").unwrap();
    ui.paste(path.to_str().unwrap());
    path
}

fn submit_media(ui: &mut TestUi, name: &str) {
    paste_media(ui, name);
    ui.key(key(KeyCode::Enter));
}
