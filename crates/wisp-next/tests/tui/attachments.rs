use super::support::*;

fn make_app_with_prompt_capabilities(
    prompt_capabilities: acp::PromptCapabilities,
) -> (App, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, command_rx) = AcpPromptHandle::recording();
    let app = App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        prompt_capabilities,
        session_capabilities: acp::SessionCapabilities::new(),
        config_options: Vec::new(),
        auth_methods: Vec::new(),
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir: std::path::PathBuf::from("."),
        settings: UiSettings::default(),
    });
    (app, command_rx)
}

fn make_app_with_caps_and_config(
    prompt_capabilities: acp::PromptCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
) -> (App, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, command_rx) = AcpPromptHandle::recording();
    let app = App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        prompt_capabilities,
        session_capabilities: acp::SessionCapabilities::new(),
        config_options,
        auth_methods: Vec::new(),
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir: std::path::PathBuf::from("."),
        settings: UiSettings::default(),
    });
    (app, command_rx)
}

fn make_failable_app_with_caps(
    prompt_capabilities: acp::PromptCapabilities,
) -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
    let app = App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        prompt_capabilities,
        session_capabilities: acp::SessionCapabilities::new(),
        config_options: vec![],
        auth_methods: vec![],
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir: std::path::PathBuf::from("."),
        settings: UiSettings::default(),
    });
    (app, fail_signal, command_rx)
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
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png");

    app.on_paste(img.to_str().unwrap());

    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "photo.png");
    assert!(app.composer().text().is_empty());
}

#[test]
fn paste_audio_path_adds_pending_media() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav");

    app.on_paste(audio.to_str().unwrap());

    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "note.wav");
}

#[test]
fn paste_ordinary_text_inserts_as_text() {
    let (mut app, _command_rx) = make_app();

    app.on_paste("hello world");

    assert!(app.composer().pending_media().is_empty());
    assert_eq!(app.composer().text(), "hello world");
}

#[test]
fn paste_non_media_file_falls_back_to_text() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let txt = create_temp_file(&tmp, "readme.txt", b"hello");

    app.on_paste(txt.to_str().unwrap());

    assert!(app.composer().pending_media().is_empty());
    assert!(!app.composer().text().is_empty());
}

#[test]
fn paste_multiple_dropped_files_adds_all() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "a.png", b"img");
    let audio = create_temp_file(&tmp, "b.wav", b"audio");
    let input = format!("{}\n{}", img.display(), audio.display());

    app.on_paste(&input);

    assert_eq!(app.composer().pending_media().len(), 2);
}

#[test]
fn duplicate_dropped_media_not_added_twice() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");
    let path_str = img.to_str().unwrap().to_string();

    app.on_paste(&path_str);
    app.on_paste(&path_str);

    assert_eq!(app.composer().pending_media().len(), 1);
}

#[test]
fn media_only_submit_sends_with_content_blocks() {
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { text, content, .. } => {
            assert!(text.is_empty(), "media-only send should have empty text");
            assert!(content.is_some(), "media-only send should have content blocks");
            assert!(!content.unwrap().is_empty());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_with_text_and_media_merges_both() {
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "describe this");
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { text, content, .. } => {
            assert_eq!(text, "describe this");
            assert!(content.is_some());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_clears_pending_media() {
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);
    command_rx.try_recv().unwrap();

    assert!(app.composer().pending_media().is_empty());
    assert!(app.composer().text().is_empty());
}

#[test]
fn backspace_on_empty_composer_removes_last_dropped_media() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img1 = create_temp_file(&tmp, "a.png", b"a");
    let img2 = create_temp_file(&tmp, "b.png", b"b");

    app.on_paste(img1.to_str().unwrap());
    app.on_paste(img2.to_str().unwrap());
    assert_eq!(app.composer().pending_media().len(), 2);

    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "a.png");

    app.on_key(key(KeyCode::Backspace));
    assert!(app.composer().pending_media().is_empty());
}

#[test]
fn backspace_does_not_remove_media_when_text_present() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "x");
    app.on_key(key(KeyCode::Backspace));

    assert_eq!(app.composer().pending_media().len(), 1);
    assert!(app.composer().text().is_empty());
}

#[test]
fn attachment_chips_render_in_layout() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.on_paste(img.to_str().unwrap());

    let layout = app.composer_mut().layout(80, &Theme::default());
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
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    assert!(!app.waiting_for_response());

    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("does not support image")));
}

#[test]
fn agent_rejects_audio_when_capability_missing() {
    let caps = acp::PromptCapabilities::new().image(true).audio(false);
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");

    app.on_paste(audio.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    assert!(!app.waiting_for_response());
}

#[test]
fn selected_model_rejects_image() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "gpt:no-vision",
        vec![model_select_option("gpt:no-vision", "GPT No Vision", false, false)],
    )];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("model selection does not support image")));
}

#[test]
fn missing_model_metadata_rejects_media() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config =
        vec![image_model_config("unknown-model", vec![model_select_option("known-model", "Known", true, true)])];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("missing prompt capability metadata")));
}

#[test]
fn supported_media_sends_blocks() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "claude:vision",
        vec![model_select_option("claude:vision", "Claude Vision", true, true)],
    )];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { content, .. } => {
            let blocks = content.expect("should have content blocks");
            assert!(blocks.iter().any(|b| matches!(b, acp::ContentBlock::Image(_))));
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn sync_prompt_failure_resets_busy_state() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_caps(media_caps());
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    fail_signal.store(true, Ordering::Relaxed);
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(!app.waiting_for_response(), "sync prompt failure should reset busy state");
    assert!(command_rx.try_recv().is_err(), "no prompt should be sent");

    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg.contains("Failed to send prompt")));
}

#[test]
fn text_only_submit_unaffected_by_media_capability_check() {
    let caps = acp::PromptCapabilities::new().image(false).audio(false);
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(caps);

    submit_prompt(&mut app, "hello");

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { text, content, .. } => {
            assert_eq!(text, "hello");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn submit_is_blocked_when_composer_empty_without_media() {
    let (mut app, mut command_rx) = make_app();

    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err());
}

#[test]
fn clear_command_also_clears_pending_media() {
    let (mut app, mut command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"img");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    assert!(app.composer().pending_media().is_empty());
    assert!(app.composer().text().is_empty());
}

#[test]
fn paste_with_file_uri_parses_correctly() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "image.png", b"img");
    let uri = format!("file://{}", img.display());

    app.on_paste(&uri);

    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "image.png");
}

#[test]
fn paste_with_percent_decoded_file_uri() {
    let (mut app, _command_rx) = make_app();
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "my image.png", b"png");
    let unencoded_path = img.to_str().unwrap();
    let encoded_path = unencoded_path.replace(' ', "%20");
    let uri = format!("file://{encoded_path}");

    app.on_paste(&uri);

    assert_eq!(app.composer().pending_media().len(), 1);
    assert_eq!(app.composer().pending_media()[0].display_name, "my image.png");
}

#[test]
fn selected_model_rejects_audio() {
    let caps = acp::PromptCapabilities::new().image(true).audio(true);
    let config = vec![image_model_config(
        "gpt:no-audio",
        vec![model_select_option("gpt:no-audio", "GPT No Audio", true, false)],
    )];
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");

    app.on_paste(audio.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
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
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
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
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let audio = create_temp_file(&tmp, "note.wav", b"fake wav data");

    app.on_paste(audio.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");
    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
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
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked — multi-select includes unsupported model");
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
    let (mut app, mut command_rx) = make_app_with_caps_and_config(caps, config);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { content, .. } => {
            let blocks = content.expect("should have content blocks");
            assert!(blocks.iter().any(|b| matches!(b, acp::ContentBlock::Image(_))));
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
}

#[test]
fn rejection_preserves_text_and_placeholders_in_transcript() {
    let caps = acp::PromptCapabilities::new().image(false).audio(false);
    let (mut app, mut command_rx) = make_app_with_prompt_capabilities(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "describe this image");
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(command_rx.try_recv().is_err(), "prompt should be blocked locally");

    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg == "describe this image"), "user text preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("image attachment")), "media placeholder preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("does not support image")), "error message shown");
}

#[test]
fn sync_failure_preserves_text_and_placeholders_in_transcript() {
    let caps = media_caps();
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_caps(caps);
    let tmp = TempDir::new().unwrap();
    let img = create_temp_file(&tmp, "photo.png", b"fake png data");

    app.on_paste(img.to_str().unwrap());
    type_text(&mut app, "describe this");
    fail_signal.store(true, Ordering::Relaxed);
    app.on_key(key(KeyCode::Enter));
    settle_tasks(&mut app);

    assert!(!app.waiting_for_response(), "sync failure should reset busy state");
    assert!(command_rx.try_recv().is_err(), "no prompt should be sent");

    let messages: Vec<_> = app
        .pending_items()
        .iter()
        .filter_map(|item| match item {
            HistoryItem::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(messages.iter().any(|msg| msg == "describe this"), "user text preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("image attachment")), "media placeholder preserved in transcript");
    assert!(messages.iter().any(|msg| msg.contains("Failed to send prompt")), "error message shown");
}

// ── Workspace move tests ──
