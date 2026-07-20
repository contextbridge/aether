use super::support::*;
use std::path::{Path, PathBuf};

fn make_app_with_prompt_capabilities(prompt_capabilities: acp::PromptCapabilities) -> TestUi {
    TestUiBuilder::new().prompt_capabilities(prompt_capabilities).build()
}

fn make_app_with_caps_and_config(
    prompt_capabilities: acp::PromptCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
) -> TestUi {
    TestUiBuilder::new().prompt_capabilities(prompt_capabilities).config_options(config_options).build()
}

fn make_failable_app_with_caps(prompt_capabilities: acp::PromptCapabilities) -> TestUi {
    TestUiBuilder::new().prompt_capabilities(prompt_capabilities).build()
}

fn media_caps() -> acp::PromptCapabilities {
    acp::PromptCapabilities::new().image(true).audio(true)
}

fn model_select_option(
    value: &str,
    name: &str,
    supports_image: bool,
    supports_audio: bool,
) -> acp::SessionConfigSelectOption {
    acp::SessionConfigSelectOption::new(value.to_string(), name.to_string())
        .meta(SelectOptionMeta { reasoning_levels: vec![], supports_image, supports_audio }.into_meta())
}

fn create_temp_file(dir: &TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, content).unwrap();
    p
}

fn image_model_config(current: &str, options: Vec<acp::SessionConfigSelectOption>) -> acp::SessionConfigOption {
    acp::SessionConfigOption::select(
        ConfigOptionId::Model.as_str().to_string(),
        "Model".to_string(),
        current.to_string(),
        options,
    )
    .category(acp::SessionConfigOptionCategory::Model)
}

fn grouped_model_config(current: &str, groups: Vec<acp::SessionConfigSelectGroup>) -> acp::SessionConfigOption {
    let mut option = acp::SessionConfigOption::select(
        ConfigOptionId::Model.as_str().to_string(),
        "Model".to_string(),
        current.to_string(),
        Vec::<acp::SessionConfigSelectOption>::new(),
    )
    .category(acp::SessionConfigOptionCategory::Model);
    if let acp::SessionConfigKind::Select(select) = &mut option.kind {
        select.options = acp::SessionConfigSelectOptions::Grouped(groups);
    }
    option
}

fn make_select_group(
    id: &str,
    name: &str,
    options: Vec<acp::SessionConfigSelectOption>,
) -> acp::SessionConfigSelectGroup {
    acp::SessionConfigSelectGroup::new(acp::SessionConfigGroupId::new(id.to_string()), name.to_string(), options)
}

#[test]
fn paste_image_path_adds_pending_media() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png");

    app.paste(img.to_str().unwrap());

    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().pending_media()[0].display_name, "photo.png");
    assert!(app.app().composer().text().is_empty());
}

#[test]
fn paste_audio_path_adds_pending_media() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav");

    app.paste(audio.to_str().unwrap());

    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().pending_media()[0].display_name, "note.wav");
}

#[test]
fn paste_ordinary_text_inserts_as_text() {
    let mut app = make_app();

    app.paste("hello world");

    assert!(app.app().composer().pending_media().is_empty());
    assert_eq!(app.app().composer().text(), "hello world");
}

#[test]
fn paste_non_media_file_falls_back_to_text() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let txt = create_temp_file(&tmp, "readme.txt", b"hello");

    app.paste(txt.to_str().unwrap());

    assert!(app.app().composer().pending_media().is_empty());
    assert!(!app.app().composer().text().is_empty());
}

#[test]
fn paste_nonexistent_path_remains_as_text() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("photo.png");
    let input = missing.to_str().unwrap();

    app.paste(input);

    assert!(app.app().composer().pending_media().is_empty());
    assert_eq!(app.app().composer().text(), input);
}

#[test]
fn paste_nonexistent_file_uri_remains_as_text() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("note.wav");
    let uri = format!("file://{}", missing.display());

    app.paste(&uri);

    assert!(app.app().composer().pending_media().is_empty());
    assert_eq!(app.app().composer().text(), uri);
}

#[test]
fn paste_directory_path_remains_as_text() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    // A directory with a media-like extension is the dangerous case.
    let dir = tmp.path().join("assets.png");
    std::fs::create_dir(&dir).unwrap();
    let input = dir.to_str().unwrap();

    app.paste(input);

    assert!(app.app().composer().pending_media().is_empty());
    assert_eq!(app.app().composer().text(), input);
}

#[test]
fn paste_multiple_nonexistent_paths_remain_as_text() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.png");
    let b = tmp.path().join("b.wav");
    let input = format!("{}\n{}", a.display(), b.display());

    app.paste(&input);

    assert!(app.app().composer().pending_media().is_empty());
    assert_eq!(app.app().composer().text(), input);
}

#[test]
fn paste_existing_regular_file_becomes_media_and_clears_text() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png");

    app.paste(img.to_str().unwrap());

    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().pending_media()[0].display_name, "photo.png");
    assert!(app.app().composer().text().is_empty(), "a real regular media file consumes the pasted path");
}

#[test]
fn paste_mixed_valid_and_missing_media_keeps_text_and_adds_nothing() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let valid = create_temp_file(&tmp, "photo.png", b"img");
    let missing = tmp.path().join("gone.wav");
    let input = format!("{}\n{}", valid.display(), missing.display());

    app.paste(&input);

    assert!(app.app().composer().pending_media().is_empty());
    assert_eq!(app.app().composer().text(), input);
}

#[test]
fn paste_mixed_valid_media_and_directory_keeps_text_and_adds_nothing() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let valid = create_temp_file(&tmp, "photo.png", b"img");
    let dir = tmp.path().join("clips.png");
    std::fs::create_dir(&dir).unwrap();
    let input = format!("{}\n{}", valid.display(), dir.display());

    app.paste(&input);

    assert!(app.app().composer().pending_media().is_empty());
    assert_eq!(app.app().composer().text(), input);
}

#[test]
fn paste_multiple_dropped_files_adds_all() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "a.png", b"img");
    let audio = create_temp_file(&tmp, "b.wav", b"audio");
    let input = format!("{}\n{}", img.display(), audio.display());

    app.paste(&input);

    assert_eq!(app.app().composer().pending_media().len(), 2);
}

#[test]
fn duplicate_dropped_media_not_added_twice() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");
    let path_str = img.to_str().unwrap().to_string();

    app.paste(&path_str);
    app.paste(&path_str);

    assert_eq!(app.app().composer().pending_media().len(), 1);
}

#[test]
fn media_only_submit_sends_with_content_blocks() {
    let mut app = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    let cmd = app.next_agent_command().unwrap();
    match cmd {
        AgentCommand::Prompt { text, content, .. } => {
            assert!(text.is_empty(), "media-only send should have empty text");
            assert!(content.is_some(), "media-only send should have content blocks");
            assert!(!content.unwrap().is_empty());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_with_text_and_media_merges_both() {
    let mut app = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.type_text("describe this");
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    let cmd = app.next_agent_command().unwrap();
    match cmd {
        AgentCommand::Prompt { text, content, .. } => {
            assert_eq!(text, "describe this");
            assert!(content.is_some());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_clears_pending_media() {
    let mut app = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();
    app.next_agent_command().unwrap();

    assert!(app.app().composer().pending_media().is_empty());
    assert!(app.app().composer().text().is_empty());
}

#[test]
fn backspace_on_empty_composer_removes_last_dropped_media() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img1 = create_temp_file(&tmp, "a.png", b"a");
    let img2 = create_temp_file(&tmp, "b.png", b"b");

    app.paste(img1.to_str().unwrap());
    app.paste(img2.to_str().unwrap());
    assert_eq!(app.app().composer().pending_media().len(), 2);

    app.key(key(KeyCode::Backspace));
    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().pending_media()[0].display_name, "a.png");

    app.key(key(KeyCode::Backspace));
    assert!(app.app().composer().pending_media().is_empty());
}

#[test]
fn backspace_does_not_remove_media_when_text_present() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.paste(img.to_str().unwrap());
    app.type_text("x");
    app.key(key(KeyCode::Backspace));

    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert!(app.app().composer().text().is_empty());
}

#[test]
fn attachment_chips_render_in_layout() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.paste(img.to_str().unwrap());

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
fn agent_rejects_image_when_capability_missing() {
    let caps = acp::PromptCapabilities::new().image(false).audio(true);
    let mut app = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");
    assert!(!app.app().waiting_for_response());

    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| msg.contains("does not support image")));
}

#[test]
fn agent_rejects_audio_when_capability_missing() {
    let caps = acp::PromptCapabilities::new().image(true).audio(false);
    let mut app = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");
    app.executor_mut().filesystem_mut().write_file(&audio, b"fake wav data");

    app.paste(audio.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");
    assert!(!app.app().waiting_for_response());
}

#[test]
fn selected_model_rejects_image() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "gpt:no-vision",
        vec![model_select_option("gpt:no-vision", "GPT No Vision", false, false)],
    )];
    let mut app = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");
    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support image")));
}

#[test]
fn missing_model_metadata_rejects_media() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config =
        vec![image_model_config("unknown-model", vec![model_select_option("known-model", "Known", true, true)])];
    let mut app = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");
    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| msg.contains("missing prompt capability metadata")));
}

#[test]
fn supported_media_sends_blocks() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "claude:vision",
        vec![model_select_option("claude:vision", "Claude Vision", true, true)],
    )];
    let mut app = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    let cmd = app.next_agent_command().unwrap();
    match cmd {
        AgentCommand::Prompt { content, .. } => {
            let blocks = content.expect("should have content blocks");
            assert!(blocks.iter().any(|b| matches!(b, acp::ContentBlock::Image(_))));
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn sync_prompt_failure_resets_busy_state() {
    let mut app = make_failable_app_with_caps(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();
    let _ = app.next_agent_command().expect("prompt should be recorded before its completion fails");
    app.deliver_result(CommandResult::Failed {
        command: FailedCommand::Prompt,
        error: "connection closed".to_string(),
    });

    assert!(!app.app().waiting_for_response(), "failed prompt should reset busy state");
    assert!(app.next_command().is_none(), "no follow-up prompt should be sent");

    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| msg.contains("Failed to send prompt")));
}

#[test]
fn text_only_submit_unaffected_by_media_capability_check() {
    let caps = acp::PromptCapabilities::new().image(false).audio(false);
    let mut app = make_app_with_prompt_capabilities(caps);

    app.submit("hello");

    let cmd = app.next_agent_command().unwrap();
    match cmd {
        AgentCommand::Prompt { text, content, .. } => {
            assert_eq!(text, "hello");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_is_blocked_when_composer_empty_without_media() {
    let mut app = make_app();

    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none());
}

#[test]
fn clear_command_also_clears_pending_media() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.paste(img.to_str().unwrap());
    app.type_text("/clear");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    assert!(app.app().composer().pending_media().is_empty());
    assert!(app.app().composer().text().is_empty());
}

#[test]
fn paste_with_file_uri_parses_correctly() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "image.png", b"img");
    let uri = format!("file://{}", img.display());

    app.paste(&uri);

    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().pending_media()[0].display_name, "image.png");
}

#[test]
fn paste_with_percent_decoded_file_uri() {
    let mut app = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "my image.png", b"png");
    let unencoded_path = img.to_str().unwrap();
    let encoded_path = unencoded_path.replace(' ', "%20");
    let uri = format!("file://{encoded_path}");

    app.paste(&uri);

    assert_eq!(app.app().composer().pending_media().len(), 1);
    assert_eq!(app.app().composer().pending_media()[0].display_name, "my image.png");
}

#[test]
fn selected_model_rejects_audio() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "gpt:no-audio",
        vec![model_select_option("gpt:no-audio", "GPT No Audio", true, false)],
    )];
    let mut app = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");
    app.executor_mut().filesystem_mut().write_file(&audio, b"fake wav data");

    app.paste(audio.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");
    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support audio")));
}

#[test]
fn selected_model_rejects_image_grouped() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let groups = vec![make_select_group(
        "g1",
        "Group 1",
        vec![model_select_option("grouped:no-vision", "No Vision", false, true)],
    )];
    let config = vec![grouped_model_config("grouped:no-vision", groups)];
    let mut app = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");
    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support image")));
}

#[test]
fn selected_model_rejects_audio_grouped() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let groups = vec![make_select_group(
        "g1",
        "Group 1",
        vec![model_select_option("grouped:no-audio", "No Audio", true, false)],
    )];
    let config = vec![grouped_model_config("grouped:no-audio", groups)];
    let mut app = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");
    app.executor_mut().filesystem_mut().write_file(&audio, b"fake wav data");

    app.paste(audio.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");
    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support audio")));
}

#[test]
fn comma_separated_multi_model_rejects_image() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let groups = vec![
        make_select_group(
            "g1",
            "Vision Models",
            vec![model_select_option("claude:sonnet", "Claude Sonnet", true, true)],
        ),
        make_select_group(
            "g2",
            "Text Models",
            vec![model_select_option("gpt:text-only", "GPT Text Only", false, false)],
        ),
    ];
    let config = vec![grouped_model_config("claude:sonnet,gpt:text-only", groups)];
    let mut app = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked — multi-select includes unsupported model");
}

#[test]
fn comma_separated_multi_model_sends_when_all_support_media() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let groups = vec![
        make_select_group(
            "g1",
            "Vision Models",
            vec![model_select_option("claude:sonnet", "Claude Sonnet", true, true)],
        ),
        make_select_group("g2", "Reasoning", vec![model_select_option("deepseek:r1", "DeepSeek R1", true, true)]),
    ];
    let config = vec![grouped_model_config("claude:sonnet,deepseek:r1", groups)];
    let mut app = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    let cmd = app.next_agent_command().unwrap();
    match cmd {
        AgentCommand::Prompt { content, .. } => {
            let blocks = content.expect("should have content blocks");
            assert!(blocks.iter().any(|b| matches!(b, acp::ContentBlock::Image(_))));
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn rejection_preserves_text_and_placeholders_in_transcript() {
    let caps = acp::PromptCapabilities::new().image(false).audio(false);
    let mut app = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.type_text("describe this image");
    app.key(key(KeyCode::Enter));
    app.settle_tasks();

    assert!(app.next_command().is_none(), "prompt should be blocked locally");

    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| *msg == "describe this image"), "user text preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("image attachment")), "media placeholder preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("does not support image")), "error message shown");
}

#[test]
fn sync_failure_preserves_text_and_placeholders_in_transcript() {
    let caps = media_caps();
    let mut app = make_failable_app_with_caps(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");
    app.executor_mut().filesystem_mut().write_file(&img, b"fake png data");

    app.paste(img.to_str().unwrap());
    app.type_text("describe this");
    app.key(key(KeyCode::Enter));
    app.settle_tasks();
    let _ = app.next_agent_command().expect("prompt should be recorded before its completion fails");
    app.deliver_result(CommandResult::Failed {
        command: FailedCommand::Prompt,
        error: "connection closed".to_string(),
    });

    assert!(!app.app().waiting_for_response(), "failed prompt should reset busy state");
    assert!(app.next_command().is_none(), "no follow-up prompt should be sent");

    let messages: Vec<_> = message_texts(&app).collect();
    assert!(messages.iter().any(|msg| *msg == "describe this"), "user text preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("image attachment")), "media placeholder preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("Failed to send prompt")), "error message shown");
}

const ONE_MIB: usize = 1024 * 1024;
const TEN_MIB: usize = 10 * 1024 * 1024;

fn write_temp(name: &str, bytes: &[u8]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn attach(path: PathBuf, display_name: &str) -> Vec<PromptAttachment> {
    vec![PromptAttachment { path, display_name: display_name.to_string() }]
}

fn text_of(block: &acp::ContentBlock) -> &str {
    match block {
        acp::ContentBlock::Resource(resource) => match &resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(contents) => &contents.text,
            _ => panic!("resource block is not text"),
        },
        _ => panic!("expected a text resource block, got {block:?}"),
    }
}

fn mime_of(block: &acp::ContentBlock) -> &str {
    match block {
        acp::ContentBlock::Image(image) => &image.mime_type,
        acp::ContentBlock::Audio(audio) => &audio.mime_type,
        acp::ContentBlock::Resource(resource) => match &resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(contents) => {
                contents.mime_type.as_deref().unwrap_or("")
            }
            _ => "",
        },
        _ => "",
    }
}

#[test]
fn classify_attachment_detects_whitelisted_images() {
    assert_eq!(classify_attachment(Path::new("photo.png")), AttachmentKind::Image);
    assert_eq!(classify_attachment(Path::new("photo.jpg")), AttachmentKind::Image);
    assert_eq!(classify_attachment(Path::new("photo.jpeg")), AttachmentKind::Image);
    assert_eq!(classify_attachment(Path::new("photo.gif")), AttachmentKind::Image);
    assert_eq!(classify_attachment(Path::new("photo.webp")), AttachmentKind::Image);
}

#[test]
fn classify_attachment_excludes_image_mime_outside_whitelist() {
    assert_eq!(classify_attachment(Path::new("icon.svg")), AttachmentKind::Unsupported);
    assert_eq!(classify_attachment(Path::new("photo.bmp")), AttachmentKind::Unsupported);
}

#[test]
fn classify_attachment_detects_whitelisted_audio() {
    assert_eq!(classify_attachment(Path::new("note.wav")), AttachmentKind::Audio);
    assert_eq!(classify_attachment(Path::new("note.mp3")), AttachmentKind::Audio);
    assert_eq!(classify_attachment(Path::new("note.ogg")), AttachmentKind::Audio);
}

#[test]
fn classify_attachment_detects_text() {
    assert_eq!(classify_attachment(Path::new("readme.txt")), AttachmentKind::Text);
}

#[test]
fn classify_attachment_unknown_extension_is_unsupported() {
    assert_eq!(classify_attachment(Path::new("data.xyz")), AttachmentKind::Unsupported);
}

#[test]
fn image_at_ten_mib_limit_is_accepted() {
    let (_dir, path) = write_temp("photo.png", &vec![0u8; TEN_MIB]);
    let outcome = build_attachments(&attach(path, "photo.png"));

    assert_eq!(outcome.blocks.len(), 1);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert!(matches!(outcome.blocks[0], acp::ContentBlock::Image(_)));
    assert_eq!(mime_of(&outcome.blocks[0]), "image/png");
    assert_eq!(outcome.placeholders, vec!["[image attachment: photo.png]"]);
}

#[test]
fn image_above_ten_mib_is_rejected() {
    let (_dir, path) = write_temp("photo.png", &vec![0u8; TEN_MIB + 1]);
    let outcome = build_attachments(&attach(path, "photo.png"));

    assert!(outcome.blocks.is_empty());
    assert!(outcome.placeholders.is_empty());
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.warnings[0], "Skipped photo.png: file too large (max 10485760)");
}

#[test]
fn audio_at_ten_mib_limit_is_accepted() {
    let (_dir, path) = write_temp("note.wav", &vec![0u8; TEN_MIB]);
    let outcome = build_attachments(&attach(path, "note.wav"));

    assert_eq!(outcome.blocks.len(), 1);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert!(matches!(outcome.blocks[0], acp::ContentBlock::Audio(_)));
    assert_eq!(mime_of(&outcome.blocks[0]), "audio/wav");
    assert_eq!(outcome.placeholders, vec!["[audio attachment: note.wav]"]);
}

#[test]
fn audio_above_ten_mib_is_rejected() {
    let (_dir, path) = write_temp("note.wav", &vec![0u8; TEN_MIB + 1]);
    let outcome = build_attachments(&attach(path, "note.wav"));

    assert!(outcome.blocks.is_empty());
    assert!(outcome.placeholders.is_empty());
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.warnings[0], "Skipped note.wav: file too large (max 10485760)");
}

#[test]
fn text_just_under_one_mib_is_not_truncated() {
    let body = "x".repeat(ONE_MIB - 1);
    let (_dir, path) = write_temp("notes.txt", body.as_bytes());
    let outcome = build_attachments(&attach(path, "notes.txt"));

    assert_eq!(outcome.blocks.len(), 1);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(text_of(&outcome.blocks[0]).len(), ONE_MIB - 1);
    assert_eq!(text_of(&outcome.blocks[0]), &body);
}

#[test]
fn text_exactly_one_mib_is_not_truncated() {
    let body = "x".repeat(ONE_MIB);
    let (_dir, path) = write_temp("notes.txt", body.as_bytes());
    let outcome = build_attachments(&attach(path, "notes.txt"));

    assert_eq!(outcome.blocks.len(), 1);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(text_of(&outcome.blocks[0]).len(), ONE_MIB);
}

#[test]
fn text_just_over_one_mib_is_truncated_with_warning() {
    let body = "x".repeat(ONE_MIB + 1);
    let (_dir, path) = write_temp("notes.txt", body.as_bytes());
    let outcome = build_attachments(&attach(path, "notes.txt"));

    assert_eq!(outcome.blocks.len(), 1);
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.warnings[0], "Truncated notes.txt to 1048576 bytes");
    let text = text_of(&outcome.blocks[0]);
    assert_eq!(text.len(), ONE_MIB);
    assert_eq!(text, &body[..ONE_MIB]);
}

#[test]
fn text_far_over_one_mib_embeds_only_one_mib() {
    let body = "x".repeat(ONE_MIB * 4);
    let (_dir, path) = write_temp("big.txt", body.as_bytes());
    let outcome = build_attachments(&attach(path, "big.txt"));

    assert_eq!(outcome.blocks.len(), 1);
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(text_of(&outcome.blocks[0]).len(), ONE_MIB);
}

#[test]
fn text_truncated_across_multibyte_boundary_stays_valid_utf8() {
    let body = "€".repeat((ONE_MIB / 3) + 2);
    let (_dir, path) = write_temp("multibyte.txt", body.as_bytes());
    let outcome = build_attachments(&attach(path, "multibyte.txt"));

    assert_eq!(outcome.blocks.len(), 1);
    assert_eq!(outcome.warnings.len(), 1);
    // ONE_MIB is not a multiple of three, so truncation lands mid-codepoint; the
    // result must back up to the last complete UTF-8 boundary rather than panic.
    let valid_len = ONE_MIB - (ONE_MIB % 3);
    assert_eq!(text_of(&outcome.blocks[0]), &body[..valid_len]);
}

#[test]
fn oversized_file_with_early_invalid_utf8_is_rejected() {
    // An invalid byte well before the truncation point is a real error, not a
    // truncation-induced incomplete sequence, so it must not be accepted as a prefix.
    let mut body = vec![b'a'; ONE_MIB + 10];
    body[100] = 0xff;
    let (_dir, path) = write_temp("bad.txt", &body);
    let outcome = build_attachments(&attach(path, "bad.txt"));

    assert!(outcome.blocks.is_empty(), "invalid UTF-8 must not be accepted as a truncated prefix");
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.warnings[0], "Skipped binary or non-UTF8 file: bad.txt");
}

#[test]
fn svg_is_embedded_as_text_not_an_image() {
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let (_dir, path) = write_temp("icon.svg", svg.as_bytes());
    let outcome = build_attachments(&attach(path, "icon.svg"));

    assert_eq!(outcome.blocks.len(), 1);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert!(matches!(outcome.blocks[0], acp::ContentBlock::Resource(_)));
    assert_eq!(text_of(&outcome.blocks[0]), svg);
    assert_eq!(mime_of(&outcome.blocks[0]), "image/svg+xml");
    assert!(outcome.placeholders.is_empty());
}

#[test]
fn unsupported_audio_mime_is_embedded_as_text_not_audio() {
    let flac = b"fake but valid UTF-8 audio metadata";
    let (_dir, path) = write_temp("track.flac", flac);
    let outcome = build_attachments(&attach(path, "track.flac"));

    assert_eq!(outcome.blocks.len(), 1);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert!(matches!(outcome.blocks[0], acp::ContentBlock::Resource(_)), "audio/flac is not a whitelisted media type");
    assert!(mime_of(&outcome.blocks[0]).starts_with("audio/"), "kept its audio/* MIME but stayed a text resource");
    assert_eq!(text_of(&outcome.blocks[0]), std::str::from_utf8(flac).unwrap());
    assert!(outcome.placeholders.is_empty());
}

#[test]
fn unsupported_image_mime_binary_is_skipped_with_warning() {
    let (_dir, path) = write_temp("raster.bmp", &[0x42, 0x4d, 0xff, 0x00]);
    let outcome = build_attachments(&attach(path, "raster.bmp"));

    assert!(outcome.blocks.is_empty(), "image/bmp is not whitelisted, and the binary body is non-UTF-8");
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.warnings[0], "Skipped binary or non-UTF8 file: raster.bmp");
}

#[test]
fn non_utf8_file_is_skipped_with_warning() {
    let (_dir, path) = write_temp("data.bin", &[0xff, 0xfe, 0xfd]);
    let outcome = build_attachments(&attach(path, "data.bin"));

    assert!(outcome.blocks.is_empty());
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.warnings[0], "Skipped binary or non-UTF8 file: data.bin");
}

#[test]
fn unsupported_extension_text_is_embedded_as_resource() {
    let (_dir, path) = write_temp("notes.xyz", b"hello world");
    let outcome = build_attachments(&attach(path, "notes.xyz"));

    assert_eq!(outcome.blocks.len(), 1);
    assert!(outcome.warnings.is_empty());
    assert!(matches!(outcome.blocks[0], acp::ContentBlock::Resource(_)));
    assert_eq!(text_of(&outcome.blocks[0]), "hello world");
}

#[test]
fn multi_attachment_preserves_order_with_truncated_text_between_media() {
    let dir = TempDir::new().unwrap();
    let img = create_temp_file(&dir, "photo.png", b"png bytes");
    let big = create_temp_file(&dir, "big.txt", &vec![b'a'; ONE_MIB + 5]);
    let wav = create_temp_file(&dir, "note.wav", b"wav bytes");
    let attachments = vec![
        PromptAttachment { path: img, display_name: "photo.png".to_string() },
        PromptAttachment { path: big, display_name: "big.txt".to_string() },
        PromptAttachment { path: wav, display_name: "note.wav".to_string() },
    ];

    let outcome = build_attachments(&attachments);

    assert_eq!(outcome.blocks.len(), 3);
    assert!(matches!(outcome.blocks[0], acp::ContentBlock::Image(_)));
    assert!(matches!(outcome.blocks[1], acp::ContentBlock::Resource(_)));
    assert!(matches!(outcome.blocks[2], acp::ContentBlock::Audio(_)));
    // The truncated text block carries no placeholder; image and audio do, in order.
    assert_eq!(outcome.placeholders, vec!["[image attachment: photo.png]", "[audio attachment: note.wav]"]);
    // The truncation warning is emitted without dropping the truncated block.
    assert_eq!(outcome.warnings, vec!["Truncated big.txt to 1048576 bytes"]);
    assert_eq!(text_of(&outcome.blocks[1]).len(), ONE_MIB);
}

#[test]
fn image_and_audio_blocks_carry_base64_of_the_file_bytes() {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let image_bytes = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xde, 0xad, 0xbe, 0xef];
    let (_image_dir, image_path) = write_temp("photo.png", &image_bytes);
    let audio_bytes = *b"RIFF fake wav payload with non-text bytes \xff\x01\x02";
    let (_audio_dir, audio_path) = write_temp("note.wav", &audio_bytes);

    let outcome = build_attachments(&[
        PromptAttachment { path: image_path, display_name: "photo.png".to_string() },
        PromptAttachment { path: audio_path, display_name: "note.wav".to_string() },
    ]);

    assert_eq!(outcome.blocks.len(), 2);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);

    let decoded_image = match &outcome.blocks[0] {
        acp::ContentBlock::Image(image) => {
            assert_eq!(image.mime_type, "image/png");
            BASE64.decode(&image.data).unwrap()
        }
        other => panic!("expected image block, got {other:?}"),
    };
    assert_eq!(decoded_image, image_bytes);

    let decoded_audio = match &outcome.blocks[1] {
        acp::ContentBlock::Audio(audio) => {
            assert_eq!(audio.mime_type, "audio/wav");
            BASE64.decode(&audio.data).unwrap()
        }
        other => panic!("expected audio block, got {other:?}"),
    };
    assert_eq!(decoded_audio, audio_bytes);
}
