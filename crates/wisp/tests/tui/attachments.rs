use super::support::*;

#[test]
fn remote_paste_never_attaches_even_when_the_path_exists_locally() {
    let mut ui = TestUiBuilder::new().remote_workspace().build();
    let image = paste_media(&mut ui, "photo.png");
    let image = image.to_str().unwrap();
    assert_eq!(ui.app().composer().pending_media().len(), 0);
    assert_eq!(ui.app().composer().text(), image);
    ui.key(key(KeyCode::Enter));
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::Prompt { text, content: None, .. }) if text == image));
    assert!(ui.take_commands().is_empty());
}

#[test]
fn pasting_an_existing_media_path_or_file_uri_attaches_it() {
    let tmp = TestWorkspace::new();
    let file = |name: &str| tmp.write(name, b"media").display().to_string();
    for (name, payload) in [
        ("photo.png", file("photo.png")),
        ("note.wav", file("note.wav")),
        ("image.png", format!("file://{}", file("image.png"))),
        ("my image.png", format!("file://{}", file("my image.png")).replace(' ', "%20")),
    ] {
        let mut ui = TestUi::new();

        ui.paste(&payload);

        let attached: Vec<_> = ui.app().composer().pending_media().iter().map(|media| &media.display_name).collect();
        assert_eq!(attached, [name], "pasting {name} should attach it");
        assert_eq!(ui.app().composer().text(), "", "an attached path is consumed");
    }
}

#[test]
fn pastes_that_are_not_all_existing_media_files_stay_text() {
    let tmp = TestWorkspace::new();
    let photo = tmp.write("photo.png", b"img").display().to_string();
    let readme = tmp.write("readme.txt", b"hello").display().to_string();
    let missing = tmp.path("gone.wav").display().to_string();
    // A directory with a media-like extension is the dangerous case.
    let directory = tmp.path("clips.png");
    std::fs::create_dir(&directory).unwrap();
    let directory = directory.display().to_string();
    for input in [
        "hello world".to_string(),
        readme,
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
    let mut app = TestUiBuilder::new().prompt_capabilities(media_caps()).build();
    submit_media(&mut app, "photo.png");

    let cmd = app.next_agent_command();
    assert!(
        matches!(&cmd, Some(AgentCommand::Prompt { text, content: Some(blocks), .. })
            if text.is_empty() && !blocks.is_empty()),
        "media-only send should carry content blocks and no text, got {cmd:?}"
    );
}

#[test]
fn submit_with_text_and_media_merges_both() {
    let mut app = TestUiBuilder::new().prompt_capabilities(media_caps()).build();
    paste_media(&mut app, "photo.png");
    app.type_text("describe this");
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    let cmd = app.next_agent_command();
    assert!(
        matches!(&cmd, Some(AgentCommand::Prompt { text, content: Some(_), .. }) if text == "describe this"),
        "{cmd:?}"
    );
}

#[test]
fn submit_clears_pending_media() {
    let mut app = TestUiBuilder::new().prompt_capabilities(media_caps()).build();
    submit_media(&mut app, "photo.png");
    app.next_agent_command().unwrap();

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
fn media_is_sent_only_when_the_agent_and_every_selected_model_support_it() {
    let no_image = acp::PromptCapabilities::new().audio(acp::PromptAudioCapabilities::new());
    let no_audio = acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new());
    let cases = [
        (no_image, vec![], "photo.png", Some("ACP agent does not support image input")),
        (no_audio, vec![], "note.wav", Some("ACP agent does not support audio input")),
        (media_caps(), models("gpt", &[("gpt", false, false)]), "photo.png", Some("selection does not support image")),
        (media_caps(), models("gpt", &[("gpt", true, false)]), "note.wav", Some("selection does not support audio")),
        (media_caps(), models("unknown", &[("known", true, true)]), "photo.png", Some("missing prompt capability")),
        (media_caps(), grouped("a", &[("a", false, true)]), "photo.png", Some("selection does not support image")),
        (media_caps(), grouped("a", &[("a", true, false)]), "note.wav", Some("selection does not support audio")),
        (media_caps(), grouped("a,b", &[("a", true, true), ("b", false, false)]), "photo.png", Some("support image")),
        (media_caps(), models("a", &[("a", true, true)]), "photo.png", None),
        (media_caps(), grouped("a,b", &[("a", true, true), ("b", true, true)]), "photo.png", None),
    ];
    for (capabilities, config, file, rejection) in cases {
        let mut ui = TestUiBuilder::new().prompt_capabilities(capabilities).config_options(config).build();
        submit_media(&mut ui, file);

        if let Some(notice) = rejection {
            assert!(ui.next_command().is_none(), "{file} should be blocked locally");
            assert!(!ui.app().waiting_for_response());
            let messages: Vec<_> = message_texts(&ui).collect();
            assert!(messages.iter().any(|message| message.contains(notice)), "{notice:?} not in {messages:?}");
        } else {
            let command = ui.next_agent_command();
            assert!(
                matches!(&command, Some(AgentCommand::Prompt { content: Some(blocks), .. })
                    if blocks.iter().any(|block| matches!(block, acp::ContentBlock::Image(_)))),
                "{file} should be sent, got {command:?}"
            );
        }
    }
}

#[test]
fn text_only_submit_unaffected_by_media_capability_check() {
    let caps = acp::PromptCapabilities::new().image(None).audio(None);
    let mut app = TestUiBuilder::new().prompt_capabilities(caps).build();

    app.submit("hello");

    let cmd = app.next_agent_command();
    assert!(matches!(&cmd, Some(AgentCommand::Prompt { text, content: None, .. }) if text == "hello"), "{cmd:?}");
}

#[test]
fn submit_is_blocked_when_composer_empty_without_media() {
    let mut app = TestUi::new();

    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none());
}

#[test]
fn clear_command_also_clears_pending_media() {
    let mut app = TestUi::new();
    paste_media(&mut app, "photo.png");
    app.run_command("/clear");
    let _ = app.next_agent_command().unwrap();

    assert_eq!(app.app().composer().pending_media().len(), 0);
    assert_eq!(app.app().composer().text(), "");
}

#[test]
fn unsupported_media_returns_the_text_to_the_composer() {
    let caps = acp::PromptCapabilities::new().image(None).audio(None);
    let mut app = TestUiBuilder::new().prompt_capabilities(caps).build();
    let img = paste_media(&mut app, "photo.png");
    app.type_text("describe this image");
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");

    assert_eq!(app.app().composer().text(), "describe this image");
    assert_eq!(pending_media_paths(&app), [&img]);
    let messages: Vec<_> = message_texts(&app).collect();
    assert!(
        messages.iter().all(|msg| !msg.contains("describe this image")),
        "an unsent prompt stays out of the transcript"
    );
    assert!(messages.iter().any(|msg| msg.contains("does not support image")), "error message shown");
}

#[test]
fn sync_failure_returns_the_text_to_the_composer() {
    let mut app = TestUiBuilder::new().prompt_capabilities(media_caps()).build();
    app.reject_prompts(PromptRejection::Failed("connection closed".to_string()));
    let img = paste_media(&mut app, "photo.png");
    app.type_text("describe this");
    app.key(key(KeyCode::Enter));
    app.settle_tasks();
    let _ = app.next_agent_command().expect("prompt should be recorded before its completion fails");

    assert!(!app.app().waiting_for_response(), "failed prompt should reset busy state");
    assert!(app.next_command().is_none(), "no follow-up prompt should be sent");

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
    let tmp = TestWorkspace::new();
    let path = tmp.write(name, b"media bytes");
    ui.executor_mut().filesystem_mut().write_file(&path, b"media bytes");
    ui.paste(path.to_str().unwrap());
    path
}

fn submit_media(ui: &mut TestUi, name: &str) {
    paste_media(ui, name);
    ui.key(key(KeyCode::Enter));
    ui.settle_tasks();
}

fn models(current: &str, offered: &[(&str, bool, bool)]) -> Vec<acp::SessionConfigOption> {
    vec![model_picker(
        current,
        offered.iter().map(|&(value, image, audio)| model(value, image, audio)).collect::<Vec<_>>(),
    )]
}

fn grouped(current: &str, offered: &[(&str, bool, bool)]) -> Vec<acp::SessionConfigOption> {
    let groups: Vec<_> =
        offered.iter().map(|&(value, image, audio)| option_group(value, vec![model(value, image, audio)])).collect();
    vec![model_picker(current, groups)]
}

fn model(value: &str, supports_image: bool, supports_audio: bool) -> acp::SessionConfigSelectOption {
    model_choice(value, value, SelectOptionMeta { supports_image, supports_audio, ..Default::default() })
}
