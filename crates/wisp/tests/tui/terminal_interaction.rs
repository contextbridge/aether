use super::support::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use utils::ReasoningEffort;

const MODELS: &[(&str, &str)] = &[("a", "Alpha"), ("b", "Beta")];

#[test]
fn remote_checkout_actions_are_blocked_without_losing_text() {
    let mut ui = TestUiBuilder::new().remote_workspace().dimensions(120, 24).build();
    ui.type_text("draft");
    ui.key(key(KeyCode::Char('@')));
    assert!(!ui.app().full_screen_active());
    assert!(!ui.app().composer().has_completion());
    assert!(ui.take_commands().is_empty());
    ui.assert_conversation_contains("unavailable for remote workspaces");
    assert!(ui.app().composer().text().starts_with("draft"));
}

#[test]
fn remote_git_review_opens() {
    use client_core::command::GitReviewCommand;
    let mut ui = TestUiBuilder::new().remote_workspace().dimensions(120, 24).build();
    ui.key(ctrl('g'));
    assert_eq!(ui.count_commands(|command| matches!(command, Command::GitReview(GitReviewCommand::Open { .. }))), 1);
    ui.settle_tasks();
    ui.assert_viewport_contains("Git Diff");
}

#[test]
fn remote_pasted_paths_remain_text_and_settings_remain_local() {
    let mut ui = TestUiBuilder::new().remote_workspace().dimensions(120, 24).build();
    ui.terminal_event(crossterm::event::Event::Paste("/server/screenshot.png".into()));
    assert_eq!(ui.app().composer().text(), "/server/screenshot.png");
    ui.key(key(KeyCode::Enter));
    assert!(
        matches!(ui.next_agent_command(), Some(AgentCommand::Prompt { text, .. }) if text == "/server/screenshot.png")
    );
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.run_command("/settings");
    assert!(ui.app().has_modal());
    assert_eq!(ui.count_commands(|command| matches!(command, Command::Host(HostCommand::ListThemes))), 1);
}

mod picker_click {
    use super::*;

    #[test]
    fn session_picker_click_selects_the_visible_row_under_the_pointer() {
        for (filter, row, resumed) in [("", 2, "bbb"), ("Alpha", 2, "ccc"), ("", 0, "aaa"), ("", 10, "ccc")] {
            let mut ui = TestUi::with_dimensions(80, 24);
            ui.open_session_picker(vec![
                session_info("aaa", "/tmp", "Alpha Project", ""),
                session_info("bbb", "/tmp", "Beta Project", ""),
                session_info("ccc", "/tmp", "Alpha Config", ""),
            ]);
            ui.type_text(filter);
            ui.draw();
            ui.terminal_event(click(2, row));
            assert!(ui.app().has_session_picker(), "a click selects without resuming");

            ui.key(key(KeyCode::Enter));
            assert_eq!(
                resumed_session(&mut ui).as_deref(),
                Some(resumed),
                "clicking row {row} under filter {filter:?}"
            );
        }
    }

    #[test]
    fn session_picker_click_requests_the_clicked_session_preview() {
        let mut ui = TestUiBuilder::new().session_preview().dimensions(120, 24).build();
        ui.open_session_picker(vec![
            session_info("sess-1", "/tmp/one", "Session One", ""),
            session_info("sess-2", "/tmp/two", "Session Two", ""),
        ]);
        assert!(matches!(ui.next_agent_command(), Some(AgentCommand::SessionPreview { .. })));

        let row = ui.viewport_row("Session Two").expect("second session row");
        ui.terminal_event(click(2, row));

        let command = ui.next_agent_command();
        assert!(
            matches!(&command, Some(AgentCommand::SessionPreview { session_id }) if session_id == "sess-2"),
            "expected a preview of the clicked session, got {command:?}"
        );
    }

    #[test]
    fn model_selector_click_clamps_reasoning_to_the_clicked_model() {
        let levels =
            |levels: &[ReasoningEffort]| SelectOptionMeta { reasoning_levels: levels.to_vec(), ..Default::default() };
        let opus = model_choice(
            "anthropic:opus",
            "Anthropic / Opus",
            levels(&[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High]),
        );
        let chat = model_choice("deepseek:chat", "DeepSeek / Chat", levels(&[ReasoningEffort::Low]));
        let mut ui = TestUiBuilder::new()
            .config_options(vec![
                model_picker("anthropic:opus", vec![opus, chat]),
                reasoning_option("high", &["low", "medium", "high"]),
            ])
            .dimensions(80, 24)
            .build();
        open_settings_row(&mut ui, "Model");

        ui.click_text("Chat");
        ui.key(key(KeyCode::Esc));

        assert_sets_config_option(&mut ui, "model", "anthropic:opus,deepseek:chat");
        assert_sets_config_option(&mut ui, "reasoning_effort", "low");
    }

    #[test]
    fn workspace_picker_click_moves_to_the_visible_row_under_the_pointer() {
        for (filter, row, moved) in [("", 2, Some("/tmp/beta")), ("other", 1, Some("/tmp/other")), ("", 0, None)] {
            let mut ui = TestUiBuilder::new().workspace_move().dimensions(80, 24).build();
            let paths = ["/tmp/alpha", "/tmp/beta", "/tmp/other"];
            open_workspace_picker(&mut ui, paths.map(|path| workspace_entry(path, false)).to_vec());
            ui.type_text(filter);
            ui.draw();
            ui.terminal_event(click(2, row));
            assert_eq!(
                moved_workspace(&mut ui).as_deref(),
                moved.map(Path::new),
                "clicking row {row} under filter {filter:?}"
            );
        }
    }
}

#[test]
fn mouse_owning_surfaces_scroll_their_own_selection() {
    let surfaces: [(&str, Step, Step); 5] = [
        (
            "settings",
            |ui| ui.run_command("/settings"),
            |ui| {
                ui.key(key(KeyCode::Enter));
                ui.assert_viewport_contains("Beta");
            },
        ),
        (
            "session picker",
            |ui| ui.open_session_picker(["a", "b", "c"].map(|id| acp::SessionInfo::new(id, "/tmp")).to_vec()),
            |ui| {
                ui.key(key(KeyCode::Enter));
                assert_eq!(resumed_session(ui).as_deref(), Some("b"));
            },
        ),
        (
            "workspace picker",
            |ui| open_workspace_picker(ui, ["/tmp/a", "/tmp/b"].map(|path| workspace_entry(path, false)).to_vec()),
            |ui| {
                ui.key(key(KeyCode::Enter));
                assert_eq!(moved_workspace(ui).as_deref(), Some(Path::new("/tmp/b")));
            },
        ),
        (
            "command picker",
            |ui| ui.type_text("/"),
            |ui| {
                ui.key(key(KeyCode::Tab));
                assert!(matches!(ui.next_agent_command(), Some(AgentCommand::ListSessions)));
            },
        ),
        (
            "prompt search",
            |ui| search(ui, "h", vec![hit("hello", 0, 1), hit("hey", 0, 1)]),
            |ui| assert_eq!(ui.app().composer().text(), "hey"),
        ),
    ];
    let build = || {
        TestUiBuilder::new()
            .config_options(vec![select("model", "Model", "a", MODELS)])
            .workspace_move()
            .prompt_search()
            .dimensions(80, 24)
            .build()
    };
    for (surface, open, assert_second_selected) in surfaces {
        let mut ui = build();
        assert!(!ui.app().needs_mouse_capture(), "the conversation leaves the mouse to the terminal");
        open(&mut ui);
        assert!(ui.app().needs_mouse_capture(), "{surface} should capture the mouse");
        ui.key(key(KeyCode::Esc));
        assert!(!ui.app().needs_mouse_capture(), "closing {surface} should release the mouse");

        let mut ui = build();
        open(&mut ui);
        ui.draw();
        for scroll in [scroll_down(2, 2), scroll_down(2, 2), scroll_up(2, 2)] {
            ui.terminal_event(scroll);
        }
        assert_eq!(bells(&mut ui), 0, "{surface}");
        assert!(ui.app().needs_mouse_capture(), "{surface} should stay open");
        assert_second_selected(&mut ui);
    }
}

#[test]
fn mouse_capture_is_released_when_the_connection_closes() {
    let mut ui = TestUi::new();
    ui.open_session_picker(vec![acp::SessionInfo::new("other", "/tmp")]);
    assert!(ui.app().needs_mouse_capture());

    ui.acp_event(AcpEvent::ConnectionClosed);
    assert!(!ui.app().needs_mouse_capture());
}

#[test]
fn bell_rings_once_when_a_submitted_turn_ends_normally() {
    let scenarios: [(&str, Step, usize); 5] = [
        ("normal completion", |ui| ui.turn("hello", "done"), 1),
        (
            "cancellation",
            |ui| {
                ui.submit("hello");
                ui.complete_prompt(acp::StopReason::Cancelled);
            },
            0,
        ),
        (
            "prompt error",
            |ui| {
                ui.reject_prompts(PromptRejection::Failed("internal error".to_string()));
                ui.submit("hello");
            },
            0,
        ),
        (
            "connection close",
            |ui| {
                ui.submit("hello");
                ui.acp_event(AcpEvent::ConnectionClosed);
            },
            0,
        ),
        ("unsolicited completion", |ui| ui.complete_prompt(acp::StopReason::EndTurn), 0),
    ];
    for (scenario, run, expected) in scenarios {
        let mut ui = TestUi::new();
        run(&mut ui);
        assert_eq!(bells(&mut ui), expected, "{scenario}");
        assert_eq!(bells(&mut ui), 0, "{scenario} must not ring again");
    }
}

mod resize {
    use super::*;

    #[test]
    fn resize_preserves_composer_content() {
        let mut app = TestUi::new();

        app.terminal_event(crossterm::event::Event::Resize(120, 30));

        app.type_text("hello resize");
        assert_eq!(app.app().composer().text(), "hello resize");
    }

    #[test]
    fn resize_no_duplicate_history() {
        let mut ui = TestUiBuilder::new().dimensions(80, 20).build();

        ui.submit("before resize");

        ui.draw();
        ui.terminal_event(crossterm::event::Event::Resize(120, 30));
        ui.draw();

        let viewport = buffer_text(ui.backend().buffer());
        assert_eq!(viewport.matches("before resize").count(), 1);
    }
}

mod event_routing {
    use super::*;

    #[test]
    fn mouse_activation_uses_shared_settings_actions() {
        use agent_client_protocol::schema::v2::{AuthMethod, AuthMethodAgent, SessionConfigOption};

        fn settings_app(
            options: Vec<SessionConfigOption>,
            servers: Vec<McpServerStatusEntry>,
            methods: Vec<AuthMethod>,
        ) -> TestUi {
            let mut ui = TestUiBuilder::new().config_options(options).auth_methods(methods).dimensions(80, 24).build();
            ui.acp_event(mcp_notification(servers));
            ui
        }

        let mut ui = settings_app(vec![select("model", "Model", "a", MODELS)], vec![], vec![]);
        open_settings_row(&mut ui, "Model");
        ui.click_text("Beta");
        assert_sets_config_option(&mut ui, "model", "b");

        let server = McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
            .with_auth_capability(McpServerAuthCapability::OAuth);
        let mut ui = settings_app(vec![], vec![server], vec![]);
        open_settings_row(&mut ui, "MCP Servers");
        ui.click_text("linear");
        assert_authenticates_mcp_server(&mut ui, "linear");

        let methods = vec![AuthMethod::Agent(AuthMethodAgent::new("codex", "Codex"))];
        let mut ui = settings_app(vec![], vec![], methods);
        open_settings_row(&mut ui, "Provider Logins");
        ui.click_text("Codex");
        assert!(matches!(
            ui.next_agent_command(),
            Some(AgentCommand::Authenticate { method_id }) if method_id == "codex"
        ));
    }

    #[test]
    fn form_modal_scroll_answers_the_choice_in_focus() {
        block_on_local(async {
            let mut ui = TestUi::with_dimensions(80, 24);
            let choices =
                StringPropertySchema::new().enum_values(vec!["first".into(), "second".into(), "third".into()]);
            let schema = ElicitationSchema::new().property("choice", choices, false);
            let response_rx = with_elicitation(&mut ui, form_elicitation("test", "Pick one", schema)).await;
            assert!(ui.app().needs_mouse_capture());

            ui.draw();
            for scroll in [scroll_down(2, 3), scroll_down(2, 3), scroll_up(2, 3)] {
                ui.terminal_event(scroll);
            }
            ui.key(key(KeyCode::Enter));

            let response = response_rx.await.unwrap();
            assert_eq!(accepted_content(&response), serde_json::json!({ "choice": "second" }));
        });
    }

    /// A survey of many questions walks one page at a time; every question
    /// must be reachable, not just the ones that fit the first screen.
    #[test]
    fn form_modal_walks_to_the_last_question() {
        block_on_local(async {
            const QUESTIONS: usize = 40;

            let mut ui = TestUi::with_dimensions(80, 24);
            let mut schema = ElicitationSchema::new();
            for index in 0..QUESTIONS {
                schema = schema.property(format!("question_{index:02}"), BooleanPropertySchema::new(), false);
            }

            with_elicitation(&mut ui, form_elicitation("test", "Pick some", schema)).await;

            ui.draw();
            let shows = |ui: &mut TestUi, needle: &str| buffer_text(ui.backend().buffer()).contains(needle);
            assert!(shows(&mut ui, "question_00"), "the first question starts on screen");
            assert!(!shows(&mut ui, "question_39"), "the last question starts on a later page");

            for _ in 1..QUESTIONS {
                ui.key(key(KeyCode::Tab));
            }
            ui.draw();

            assert!(shows(&mut ui, "question_39"), "Tab reaches the last question");
            assert!(shows(&mut ui, "✓"), "the review tab is reachable at the end of the strip");
        });
    }

    /// A page with more options than the modal is tall has to scroll, or the
    /// options below the fold can never be reached.
    #[test]
    fn form_modal_scrolls_the_focused_option_into_view() {
        block_on_local(async {
            const OPTIONS: usize = 40;

            let values = (0..OPTIONS).map(|index| format!("option_{index:02}")).collect::<Vec<_>>();
            let schema =
                ElicitationSchema::new().property("choice", StringPropertySchema::new().enum_values(values), false);

            let mut ui = TestUi::with_dimensions(80, 24);
            with_elicitation(&mut ui, form_elicitation("test", "Pick one", schema)).await;

            ui.draw();
            let shows = |ui: &mut TestUi, needle: &str| buffer_text(ui.backend().buffer()).contains(needle);
            assert!(shows(&mut ui, "option_00"), "the first option starts on screen");
            assert!(!shows(&mut ui, "option_39"), "the last option starts below the fold");

            for _ in 1..OPTIONS {
                ui.key(key(KeyCode::Down));
            }
            ui.draw();

            assert!(shows(&mut ui, "option_39"), "the focused option scrolled into view");
            assert!(!shows(&mut ui, "option_00"), "the options above it scrolled away");
        });
    }

    /// The modal is drawn centred inside the viewport, so its options sit an
    /// arbitrary number of rows down. A click has to hit the option under the
    /// pointer rather than one counted from the top of the screen.
    #[test]
    fn form_modal_click_answers_the_option_under_the_pointer() {
        block_on_local(async {
            let mut ui = TestUi::with_dimensions(80, 24);
            let schema = ElicitationSchema::new()
                .property("alpha", BooleanPropertySchema::new().default_value(false), false)
                .property("bravo", BooleanPropertySchema::new().default_value(false), false);

            let response_rx = with_elicitation(&mut ui, form_elicitation("test", "Pick one", schema)).await;

            ui.draw();

            let screen = buffer_text(ui.backend().buffer());
            let yes =
                screen.lines().position(|row| row.contains("1  Yes")).expect("the first option should be on screen");
            ui.terminal_event(click(20, u16::try_from(yes).unwrap()));

            // Two questions make this a wizard: Enter walks alpha → bravo →
            // review, and only on the review page does it submit.
            ui.key(key(KeyCode::Enter));
            ui.key(key(KeyCode::Enter));
            ui.key(key(KeyCode::Enter));

            let response = response_rx.await.unwrap();
            assert_eq!(accepted_content(&response), serde_json::json!({ "alpha": true, "bravo": false }));
        });
    }

    /// An option is as many rows as it was drawn on, so clicking a wrapped
    /// continuation row hits the option rather than falling between two.
    #[test]
    fn form_modal_click_hits_every_row_an_option_occupies() {
        block_on_local(async {
            let mut ui = TestUi::with_dimensions(80, 24);
            let schema = ElicitationSchema::new().property(
                "choice",
                StringPropertySchema::new().enum_values(vec![
                    "a-very-long-option-title-that-certainly-wraps-onto-a-second-row".to_string(),
                    "short".to_string(),
                ]),
                false,
            );

            let response_rx = with_elicitation(&mut ui, form_elicitation("test", "Pick one", schema)).await;

            ui.draw();

            let needle = "certainly-wraps";
            for row in buffer_text(ui.backend().buffer())
                .lines()
                .enumerate()
                .filter(|(_, row)| row.contains(needle) || row.contains("onto-a-second-row"))
                .map(|(row, _)| row)
                .collect::<Vec<_>>()
            {
                ui.terminal_event(click(20, u16::try_from(row).unwrap()));
            }
            ui.key(key(KeyCode::Enter));

            let response = response_rx.await.unwrap();
            assert_eq!(
                accepted_content(&response),
                serde_json::json!({
                    "choice": "a-very-long-option-title-that-certainly-wraps-onto-a-second-row"
                })
            );
        });
    }
}

mod screen_mouse {
    use super::*;
    use ratatui::layout::Rect;
    use wisp::git_review::FileDiff;
    use wisp::screens::git_diff::GitDiffScreen;
    use wisp::view::input::MouseAction;

    fn render_git_diff(screen: &mut GitDiffScreen, width: u16, height: u16) -> Buffer {
        render_surface(Buffer::empty(Rect::new(0, 0, width, height)), |buffer, cx| {
            screen.render(buffer.area, buffer, cx)
        })
    }

    fn open_screen() -> GitDiffScreen {
        git_diff_screen(vec![
            FileDiff::from_texts("src/main.rs", "fn old() {}\n", "fn main() {}\n").unwrap(),
            FileDiff::from_texts("src/lib.rs", "", "pub fn library() {}\n").unwrap(),
            FileDiff::from_texts("Cargo.toml", "old\n", "new\n").unwrap(),
        ])
    }

    fn assert_drawer_focus(buffer: &Buffer, focused: bool) {
        let accent = Theme::default().accent;
        let selected = (1..buffer.area.bottom() - 2)
            .map(|y| &buffer[(1, y)])
            .find(|cell| cell.bg == accent)
            .expect("drawer has a selected row");
        assert_eq!(
            selected.modifier.contains(ratatui::style::Modifier::BOLD),
            focused,
            "selected drawer row is bold only while focused:\n{}",
            buffer_text(buffer)
        );
    }

    #[test]
    fn git_diff_click_left_side_selects_drawer() {
        let mut screen = open_screen();

        screen.on_ui_event(&UiEvent::Key(key(KeyCode::Tab)));
        let before = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&before, false);

        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (20, 3))).is_empty());

        let buffer = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&buffer, true);
    }

    #[test]
    fn git_diff_click_right_side_selects_patch() {
        let mut screen = open_screen();

        let before = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&before, true);

        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (60, 3))).is_empty());

        let buffer = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&buffer, false);
    }

    #[test]
    fn git_diff_click_narrow_layout_always_patch() {
        let mut screen = open_screen();

        render_git_diff(&mut screen, 60, 40);

        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (2, 3))).is_empty());

        screen.on_ui_event(&UiEvent::Key(key(KeyCode::Char('c'))));
        let buffer = render_git_diff(&mut screen, 60, 40);
        assert!(buffer_text(&buffer).contains("save"), "patch focus must allow drafting a comment");
    }

    #[test]
    fn git_diff_click_on_border_is_ignored() {
        let mut screen = open_screen();

        let before = render_git_diff(&mut screen, 120, 40);

        // Click at y=0 (top border) — should be ignored
        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (40, 0))).is_empty());

        let buffer = render_git_diff(&mut screen, 120, 40);
        assert_eq!(buffer, before, "border clicks must not alter selection or focus");
    }

    #[test]
    fn git_diff_click_after_resize_uses_new_pane_rects() {
        let mut screen = open_screen();

        render_git_diff(&mut screen, 60, 40);
        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (20, 3))).is_empty());
        screen.on_ui_event(&UiEvent::Key(key(KeyCode::Char('c'))));
        let draft = render_git_diff(&mut screen, 60, 40);
        assert!(buffer_text(&draft).contains("save"));
        screen.on_ui_event(&UiEvent::Key(key(KeyCode::Esc)));

        let before = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&before, false);
        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (20, 3))).is_empty());
        let after = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&after, true);
    }
}

mod refocus_click {
    use agent_client_protocol::schema::v2::{AuthMethod, AuthMethodAgent};
    use crossterm::event::Event;

    use super::*;

    fn codex_logged_in_via_browser() -> TestUi {
        let methods = vec![AuthMethod::Agent(AuthMethodAgent::new("codex", "Codex"))];
        let mut ui = TestUiBuilder::new().auth_methods(methods).dimensions(80, 24).build();
        open_settings_row(&mut ui, "Provider Logins");

        ui.click_text("Codex");
        ui.terminal_event(Event::FocusLost);
        ui.deliver_result(CommandResult::AuthenticationCompleted {
            method_id: "codex".into(),
            result: Ok(acp::LoginAuthResponse::new()),
        });
        ui.draw();
        ui.take_commands();
        ui
    }

    #[test]
    fn click_after_refocusing_can_log_in_again() {
        let mut ui = codex_logged_in_via_browser();
        ui.terminal_event(Event::FocusGained);
        ui.click_text("Codex");
        ui.click_text("Codex");
        let logins = ui.count_commands(|command| matches!(command, Command::Agent(AgentCommand::Authenticate { .. })));
        assert_eq!(logins, 1);
    }

    #[test]
    fn click_after_refocusing_from_the_keyboard_is_not_swallowed() {
        let mut ui = codex_logged_in_via_browser();
        ui.terminal_event(Event::FocusGained);
        ui.key(key(KeyCode::Up));
        ui.click_text("Codex");
        let logins = ui.count_commands(|command| matches!(command, Command::Agent(AgentCommand::Authenticate { .. })));
        assert_eq!(logins, 1);
    }
}

fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> crossterm::event::Event {
    crossterm::event::Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
}

fn scroll_down(column: u16, row: u16) -> crossterm::event::Event {
    mouse_event(MouseEventKind::ScrollDown, column, row)
}

fn scroll_up(column: u16, row: u16) -> crossterm::event::Event {
    mouse_event(MouseEventKind::ScrollUp, column, row)
}

fn click(column: u16, row: u16) -> crossterm::event::Event {
    mouse_event(MouseEventKind::Down(MouseButton::Left), column, row)
}

fn resumed_session(ui: &mut TestUi) -> Option<String> {
    ui.take_command_where(|command| match command {
        Command::Agent(AgentCommand::ResumeSession { session_id, .. }) => Some(session_id.0.to_string()),
        _ => None,
    })
}

fn moved_workspace(ui: &mut TestUi) -> Option<PathBuf> {
    ui.take_command_where(|command| match command {
        Command::Agent(AgentCommand::MoveWorkspace { target: WorkspaceMoveTarget::Existing { path }, .. }) => {
            Some(path)
        }
        _ => None,
    })
}
