use super::support::*;

#[test]
fn a_remote_workspace_attaches_pasted_media_too() {
    let mut ui = with_media(&["photo.png"]).session(SessionCoreConfigBuilder::remote_workspace).build();

    paste_media(&mut ui, "photo.png");

    let attached: Vec<_> = ui.app().composer().pending_media().iter().map(|media| &media.display_name).collect();
    assert_eq!(attached, ["photo.png"]);
    assert_eq!(ui.app().composer().text(), "");
}

#[test]
fn pasting_an_existing_media_path_in_any_form_a_terminal_drops_attaches_it() {
    type Form = fn(String) -> String;
    let forms: [(&str, Form); 7] = [
        ("photo.png", |path| path),
        ("note.wav", |path| path),
        ("image.png", |path| format!("file://{path}")),
        ("my image.png", |path| format!("file://{path}").replace(' ', "%20")),
        ("my image.png", |path| format!("'{path}'")),
        ("my image.png", |path| format!("\"{path}\"")),
        ("my image.png", |path| path.replace(' ', "\\ ")),
    ];
    for (name, form) in forms {
        let mut ui = with_media(&[name]).build();

        ui.paste(&form(workspace_path(&ui, name)));

        let attached: Vec<_> = ui.app().composer().pending_media().iter().map(|media| &media.display_name).collect();
        assert_eq!(attached, [name], "pasting {name} should attach it");
        assert_eq!(ui.app().composer().text(), "", "an attached path is consumed");
    }
}

#[test]
fn pasting_a_text_files_path_mentions_it() {
    let mut ui = TestUiBuilder::new().file("readme.txt", b"hello").build();

    ui.paste(&workspace_path(&ui, "readme.txt"));

    assert_eq!(ui.app().composer().pending_media().len(), 0);
    assert_eq!(ui.app().composer().text(), "@readme.txt ");
}

#[test]
fn pastes_that_are_not_all_existing_files_stay_text() {
    // A directory with a media-like extension is the dangerous case.
    let inputs: [fn(&TestUi) -> String; 7] = [
        |_| "hello world".to_string(),
        |ui| workspace_path(ui, "gone.wav"),
        |ui| format!("file://{}", workspace_path(ui, "gone.wav")),
        |ui| workspace_path(ui, "clips.png"),
        |ui| format!("{}\n{}", workspace_path(ui, "a.png"), workspace_path(ui, "gone.wav")),
        |ui| format!("{}\n{}", workspace_path(ui, "photo.png"), workspace_path(ui, "gone.wav")),
        |ui| format!("{}\n{}", workspace_path(ui, "photo.png"), workspace_path(ui, "clips.png")),
    ];
    for input in inputs {
        let mut ui = with_media(&["photo.png", "clips.png/frame.png"]).build();
        let input = input(&ui);

        ui.paste(&input);

        assert_eq!(ui.app().composer().pending_media().len(), 0, "{input:?} should attach nothing");
        assert_eq!(ui.app().composer().text(), input);
    }
}

#[test]
fn paste_multiple_dropped_files_adds_all() {
    let mut app = with_media(&["a.png", "b.wav"]).build();

    app.paste(&format!("{}\n{}", workspace_path(&app, "a.png"), workspace_path(&app, "b.wav")));

    assert_eq!(app.app().composer().pending_media().len(), 2);
}

#[test]
fn duplicate_dropped_media_not_added_twice() {
    let mut app = with_media(&["photo.png"]).build();

    paste_media(&mut app, "photo.png");
    paste_media(&mut app, "photo.png");

    assert_eq!(app.app().composer().pending_media().len(), 1);
}

#[test]
fn submit_sends_pasted_media_with_the_prompt() {
    let mut app = with_media(&["photo.png"]).session(|session| session.prompt_capabilities(media_caps())).build();
    paste_media(&mut app, "photo.png");
    app.submit("describe this");

    let request = sent_prompt(&mut app);
    assert!(request.prompt.iter().any(|block| matches!(block, acp::ContentBlock::Image(_))), "{:?}", request.prompt);
}

#[test]
fn submit_clears_pending_media() {
    let mut app = with_media(&["photo.png"]).session(|session| session.prompt_capabilities(media_caps())).build();
    submit_media(&mut app, "photo.png");

    assert_eq!(app.app().composer().pending_media().len(), 0);
    assert_eq!(app.app().composer().text(), "");
}

#[test]
fn backspace_on_empty_composer_removes_last_dropped_media() {
    let mut app = with_media(&["a.png", "b.png"]).build();
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
    let mut app = with_media(&["photo.png"]).build();
    paste_media(&mut app, "photo.png");
    app.type_text("x");
    app.key(key(KeyCode::Backspace));

    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().text(), "");
}

#[test]
fn attachment_chips_render_in_layout() {
    let mut app = with_media(&["photo.png"]).build();
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
fn submit_is_blocked_when_composer_empty_without_media() {
    let mut app = TestUi::new();

    app.key(key(KeyCode::Enter));

    assert!(!sent_any_prompt(&mut app));
}

#[test]
fn clear_command_also_clears_pending_media() {
    let mut app = with_media(&["photo.png"]).build();
    paste_media(&mut app, "photo.png");
    app.run_command("/clear");

    assert_eq!(app.app().composer().pending_media().len(), 0);
    assert_eq!(app.app().composer().text(), "");
}

#[test]
fn sync_failure_returns_the_text_to_the_composer() {
    let mut app = with_media(&["photo.png"]).session(|session| session.prompt_capabilities(media_caps())).build();
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

/// A scenario whose workspace holds a media file at each of `paths`.
fn with_media(paths: &[&str]) -> TestUiBuilder {
    paths.iter().fold(TestUiBuilder::new(), |builder, path| builder.file(path, b"media bytes"))
}

fn workspace_path(ui: &TestUi, name: &str) -> String {
    ui.workspace().join(name).display().to_string()
}

/// Pastes the path of the workspace file `name`, as a terminal drops it.
fn paste_media(ui: &mut TestUi, name: &str) -> PathBuf {
    let path = ui.workspace().join(name);
    ui.paste(path.to_str().unwrap());
    path
}

fn submit_media(ui: &mut TestUi, name: &str) {
    paste_media(ui, name);
    ui.key(key(KeyCode::Enter));
}
