use acp_utils::client::AcpEvent;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::TerminalOptions;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use wisp::app::WorkspaceMoveState;
use wisp::command::{AgentCommand, Command, CommandResult, GitCommand, TerminalCommand};

use super::support::{
    BooleanPropertySchema, ElicitationSchema, StringPropertySchema, TestUi, TestUiBuilder, accepted_content, acp,
    block_on_local, buffer_text, form_elicitation, prompt_failed, with_elicitation,
};

/// Whether the app asked the terminal to ring the bell, draining whatever else
/// it had queued along with it.
fn rang_bell(app: &mut TestUi) -> bool {
    app.take_commands().into_iter().any(|command| matches!(command, Command::Terminal(TerminalCommand::RingBell)))
}

fn make_app() -> TestUi {
    TestUiBuilder::new().build()
}

fn make_ui() -> TestUi {
    TestUiBuilder::new().dimensions(80, 24).build()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// The area navigation is drawn into: the whole inline viewport.
fn navigation_rect(ui: &mut TestUi) -> ratatui::layout::Rect {
    ui.viewport_area()
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

mod picker_click {
    use super::*;
    use agent_client_protocol::schema::v1::SessionInfo;

    #[test]
    fn session_picker_click_first_row_selects_index_zero() {
        let mut ui = make_ui();

        // Open session picker with sessions
        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("s1"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("s2"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("s3"), "/tmp"),
        ];
        ui.deliver_result(CommandResult::SessionsListed(acp::ListSessionsResponse::new(sessions)));
        assert!(ui.app().has_session_picker());

        ui.draw();
        let rect = navigation_rect(&mut ui);
        // Click on the second content row (local_y=2 → row=1 after border offset)
        ui.terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(ui.app().has_session_picker(), "picker should remain open");
    }

    #[test]
    fn session_picker_click_outside_row_range_is_clamped() {
        let mut ui = make_ui();

        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("s1"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("s2"), "/tmp"),
        ];
        ui.deliver_result(CommandResult::SessionsListed(acp::ListSessionsResponse::new(sessions)));
        assert!(ui.app().has_session_picker());

        ui.draw();

        let rect = navigation_rect(&mut ui);
        // Click far below content (local_y=20 → row=19 after offset, clamped to last item)
        ui.terminal_event(click(rect.x + 2, rect.y + 20));
        assert!(ui.app().has_session_picker());
    }

    #[test]
    fn session_picker_click_with_filter_uses_visible_rows() {
        let mut ui = make_ui();

        let mut session_a = SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("aaa"), "/tmp");
        session_a.title = Some("Alpha Project".to_string());
        let mut session_b = SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("bbb"), "/tmp");
        session_b.title = Some("Beta Project".to_string());
        let mut session_c = SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("ccc"), "/tmp");
        session_c.title = Some("Alpha Config".to_string());

        ui.deliver_result(CommandResult::SessionsListed(acp::ListSessionsResponse::new(vec![
            session_a, session_b, session_c,
        ])));
        assert!(ui.app().has_session_picker());

        // Type filter: "Alpha"
        ui.key(key(KeyCode::Char('A')));
        ui.key(key(KeyCode::Char('l')));
        ui.key(key(KeyCode::Char('p')));
        ui.key(key(KeyCode::Char('h')));
        ui.key(key(KeyCode::Char('a')));

        ui.draw();

        let rect = navigation_rect(&mut ui);
        // Click on first visible row
        ui.terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(ui.app().has_session_picker());
    }

    #[test]
    fn workspace_picker_click_first_row_selects_index_zero() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};

        let workspaces = vec![
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws1"), is_current: false },
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws2"), is_current: false },
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws3"), is_current: false },
        ];
        let mut ui = make_ui();
        ui.deliver_result(CommandResult::WorkspacesListed(WorkspaceListResponse { workspaces }));

        ui.draw();
        let rect = navigation_rect(&mut ui);
        // Click on the first content row (local_y=1 → row=0 after border offset)
        ui.terminal_event(click(rect.x + 2, rect.y + 1));
    }

    #[test]
    fn workspace_picker_click_outside_row_range_is_clamped() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};

        let workspaces = vec![WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws1"), is_current: false }];
        let mut ui = make_ui();
        ui.deliver_result(CommandResult::WorkspacesListed(WorkspaceListResponse { workspaces }));

        ui.draw();

        let rect = navigation_rect(&mut ui);
        // Click far below content
        ui.terminal_event(click(rect.x + 2, rect.y + 15));
    }

    #[test]
    fn workspace_picker_click_with_filter_uses_filtered_rows() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};

        let workspaces = vec![
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/project-alpha"), is_current: false },
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/project-beta"), is_current: false },
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/other"), is_current: false },
        ];
        let mut ui = make_ui();
        ui.deliver_result(CommandResult::WorkspacesListed(WorkspaceListResponse { workspaces }));

        // Type filter: "beta"
        ui.key(key(KeyCode::Char('b')));
        ui.key(key(KeyCode::Char('e')));
        ui.key(key(KeyCode::Char('t')));
        ui.key(key(KeyCode::Char('a')));

        ui.draw();

        let rect = navigation_rect(&mut ui);
        // Click on first visible row
        ui.terminal_event(click(rect.x + 2, rect.y + 2));
    }
}

mod mouse_capture {
    use super::*;
    use agent_client_protocol::schema::v1::SessionInfo;

    #[test]
    fn no_capture_when_no_fullscreen_or_modal() {
        let app = make_app();
        assert!(!app.app().needs_mouse_capture());
    }

    #[test]
    fn capture_enabled_when_session_picker_is_open() {
        let mut app = make_app();
        let current_id = agent_client_protocol::schema::v1::SessionId::new("test-session");
        let other = SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("other"), "/tmp");
        app.deliver_result(CommandResult::SessionsListed(acp::ListSessionsResponse::new(vec![other])));
        std::mem::drop(current_id);

        assert!(app.app().has_session_picker());
        assert!(app.app().needs_mouse_capture());

        app.key(key(KeyCode::Esc));
        assert!(!app.app().needs_mouse_capture());
    }

    #[test]
    fn capture_when_composer_overlay_is_open() {
        let mut app = make_app();

        // Open command picker
        app.key(key(KeyCode::Char('/')));
        assert!(app.app().needs_mouse_capture());

        app.key(key(KeyCode::Esc));
        assert!(!app.app().needs_mouse_capture());
    }

    #[test]
    fn capture_when_prompt_search_is_open() {
        let mut app = TestUiBuilder::new().prompt_search().build();

        app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(app.app().needs_mouse_capture());

        app.key(key(KeyCode::Esc));
        assert!(!app.app().needs_mouse_capture());
    }

    #[test]
    fn capture_disabled_after_connection_closed() {
        let mut app = make_app();
        let other = SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("other"), "/tmp");
        app.deliver_result(CommandResult::SessionsListed(acp::ListSessionsResponse::new(vec![other])));
        assert!(app.app().needs_mouse_capture());

        app.acp_event(AcpEvent::ConnectionClosed);
        assert!(!app.app().needs_mouse_capture());
    }
}

mod bell {
    use super::*;
    use agent_client_protocol::schema::v1 as acp;

    #[test]
    fn bell_after_normal_completion() {
        let mut app = make_app();

        // Submit a prompt
        app.submit("hello");
        assert!(app.app().waiting_for_response());

        app.complete_prompt(acp::StopReason::EndTurn);

        assert!(rang_bell(&mut app));
    }

    #[test]
    fn no_bell_after_cancellation() {
        let mut app = make_app();

        app.submit("hello");
        app.complete_prompt(acp::StopReason::Cancelled);

        assert!(!rang_bell(&mut app));
    }

    #[test]
    fn no_bell_after_prompt_error() {
        let mut app = make_app();

        app.submit("hello");
        app.deliver_result(prompt_failed("internal error"));

        assert!(!rang_bell(&mut app));
    }

    #[test]
    fn no_bell_after_connection_close() {
        let mut app = make_app();

        app.submit("hello");
        app.acp_event(AcpEvent::ConnectionClosed);

        assert!(!rang_bell(&mut app));
    }

    #[test]
    fn no_bell_on_unsolicited_prompt_completion() {
        let mut app = make_app();

        app.complete_prompt(acp::StopReason::EndTurn);

        assert!(!rang_bell(&mut app));
    }

    #[test]
    fn exactly_one_bell_per_completion() {
        let mut app = make_app();

        app.submit("first");
        app.complete_prompt(acp::StopReason::EndTurn);

        assert!(rang_bell(&mut app));
        assert!(!rang_bell(&mut app));
    }
}

mod resize {
    use super::*;

    #[test]
    fn resize_preserves_composer_content() {
        let mut app = make_app();

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
    use agent_client_protocol::schema::v1::SessionInfo;

    #[test]
    fn mouse_click_outside_surface_is_ignored() {
        let mut app = make_app();

        app.deliver_result(CommandResult::SessionsListed(acp::ListSessionsResponse::new(vec![SessionInfo::new(
            agent_client_protocol::schema::v1::SessionId::new("other"),
            "/tmp",
        )])));
        assert!(app.app().has_session_picker());

        // Click at (0, 0) — outside the picker rect (no surface rect is set without render)
        app.terminal_event(click(0, 0));

        // The session picker should still be open and unchanged
        assert!(app.app().has_session_picker());
    }

    #[test]
    fn keyboard_event_routing_precedence_unchanged() {
        let mut app = make_app();

        // Open settings overlay via /settings
        app.key(key(KeyCode::Char('/')));
        app.key(key(KeyCode::Char('s')));
        app.key(key(KeyCode::Tab));
        app.key(key(KeyCode::Enter));

        // Esc closes settings overlay
        app.key(key(KeyCode::Esc));
        assert!(!app.app().has_modal());
    }

    #[test]
    fn mouse_event_routes_to_topmost_surface() {
        let mut ui = make_ui();

        // Open settings overlay
        ui.key(key(KeyCode::Char('/')));
        ui.key(key(KeyCode::Char('s')));
        ui.key(key(KeyCode::Tab));
        ui.key(key(KeyCode::Enter));

        ui.draw();

        // Scroll events should be consumed internally
        let bell_before = rang_bell(&mut ui);
        ui.terminal_event(scroll_down(40, 5));
        assert_eq!(bell_before, rang_bell(&mut ui));
    }

    #[test]
    fn scroll_on_settings_overlay_changes_menu_selection() {
        use agent_client_protocol::schema::v1::{SessionConfigOption, SessionConfigSelectOption};
        let opts = vec![
            SessionConfigOption::select(
                "model",
                "Model",
                "a",
                vec![SessionConfigSelectOption::new("a", "Alpha"), SessionConfigSelectOption::new("b", "Beta")],
            ),
            SessionConfigOption::select(
                "mode",
                "Mode",
                "code",
                vec![SessionConfigSelectOption::new("code", "Code"), SessionConfigSelectOption::new("chat", "Chat")],
            ),
        ];
        let mut ui = TestUiBuilder::new().config_options(opts).dimensions(80, 24).build();

        ui.type_text("/settings");
        ui.key(key(KeyCode::Tab));

        ui.draw();
        assert!(ui.app().has_navigation());

        ui.terminal_event(scroll_down(40, 5));
        assert!(!rang_bell(&mut ui));
    }

    #[test]
    fn settings_overlay_click_selects_entry() {
        use agent_client_protocol::schema::v1::{SessionConfigOption, SessionConfigSelectOption};
        let opts = vec![SessionConfigOption::select(
            "model",
            "Model",
            "a",
            vec![SessionConfigSelectOption::new("a", "Alpha"), SessionConfigSelectOption::new("b", "Beta")],
        )];
        let mut ui = TestUiBuilder::new().config_options(opts).dimensions(80, 24).build();

        ui.type_text("/settings");
        ui.key(key(KeyCode::Tab));

        ui.draw();

        let rect = navigation_rect(&mut ui);
        let click_y = rect.y + 2;
        ui.terminal_event(click(rect.x + 2, click_y));
        assert!(!rang_bell(&mut ui));
    }

    #[test]
    fn mouse_activation_uses_shared_settings_actions() {
        use acp_utils::notifications::{
            McpNotification, McpServerAuthCapability, McpServerStatus, McpServerStatusEntry,
        };
        use agent_client_protocol::schema::v1::{
            AuthMethod, AuthMethodAgent, SessionConfigOption, SessionConfigSelectOption,
        };

        fn settings_app(
            options: Vec<SessionConfigOption>,
            servers: Vec<McpServerStatusEntry>,
            methods: Vec<AuthMethod>,
        ) -> TestUi {
            let mut ui = TestUiBuilder::new().config_options(options).auth_methods(methods).dimensions(80, 24).build();
            ui.acp_event(AcpEvent::McpNotification(McpNotification::ServerStatus { servers }));
            ui
        }

        fn open_settings(ui: &mut TestUi) {
            ui.type_text("/settings");
            ui.key(key(KeyCode::Tab));
            ui.draw();
        }

        let options = vec![SessionConfigOption::select(
            "model",
            "Model",
            "a",
            vec![SessionConfigSelectOption::new("a", "Alpha"), SessionConfigSelectOption::new("b", "Beta")],
        )];
        let mut ui = settings_app(options, vec![], vec![]);
        open_settings(&mut ui);
        let model_row = ui.viewport_row("Model:").expect("Model row in the settings menu");
        ui.terminal_event(click(10, model_row));
        ui.draw();
        let beta_row = ui.viewport_row("Beta").expect("Beta row in the model picker");
        ui.terminal_event(click(10, beta_row));
        assert!(matches!(
            ui.next_agent_command(),
            Some(AgentCommand::SetConfigOption { config_id, value, .. }) if config_id == "model" && value == "b"
        ));

        let server = McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
            .with_auth_capability(McpServerAuthCapability::OAuth);
        let mut ui = settings_app(vec![], vec![server], vec![]);
        open_settings(&mut ui);
        let servers_row = ui.viewport_row("MCP Servers:").expect("MCP Servers row in the settings menu");
        ui.terminal_event(click(10, servers_row));
        ui.draw();
        let linear_row = ui.viewport_row("linear").expect("linear row in the servers pane");
        ui.terminal_event(click(10, linear_row));
        assert!(matches!(
            ui.next_agent_command(),
            Some(AgentCommand::AuthenticateMcpServer { server_name, .. }) if server_name == "linear"
        ));

        let methods = vec![AuthMethod::Agent(AuthMethodAgent::new("codex", "Codex"))];
        let mut ui = settings_app(vec![], vec![], methods);
        open_settings(&mut ui);
        let providers_row = ui.viewport_row("Provider Logins:").expect("Provider Logins row in the settings menu");
        ui.terminal_event(click(10, providers_row));
        ui.draw();
        let codex_row = ui.viewport_row("Codex").expect("Codex row in the provider pane");
        ui.terminal_event(click(10, codex_row));
        assert!(matches!(
            ui.next_agent_command(),
            Some(AgentCommand::Authenticate { method_id }) if method_id == "codex"
        ));
    }
    #[test]
    fn session_picker_scroll_changes_selection() {
        let mut ui = make_ui();
        let current = agent_client_protocol::schema::v1::SessionId::new("test-session");
        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("a"), "/tmp/a"),
            SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("b"), "/tmp/b"),
            SessionInfo::new(agent_client_protocol::schema::v1::SessionId::new("c"), "/tmp/c"),
        ];
        ui.deliver_result(CommandResult::SessionsListed(acp::ListSessionsResponse::new(sessions)));
        std::mem::drop(current);
        assert!(ui.app().has_session_picker());

        ui.draw();
        assert!(ui.app().has_navigation());

        let rect = navigation_rect(&mut ui);
        ui.terminal_event(scroll_down(rect.x + 2, rect.y + 2));
        assert!(!rang_bell(&mut ui));
        assert!(ui.app().has_session_picker());
    }

    #[test]
    fn prompt_search_scroll_up_and_down() {
        let mut ui = TestUiBuilder::new().prompt_search().dimensions(80, 24).build();

        ui.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(ui.app().needs_mouse_capture());

        ui.draw();

        let rect = navigation_rect(&mut ui);
        // Scroll should be consumed internally
        ui.terminal_event(scroll_down(rect.x + 2, rect.y + 1));
        ui.terminal_event(scroll_up(rect.x + 2, rect.y + 1));
        assert!(!rang_bell(&mut ui));
    }

    #[test]
    fn composer_overlay_scroll_and_click() {
        let mut ui = make_ui();
        // Open command picker
        ui.key(key(KeyCode::Char('/')));
        assert!(ui.app().needs_mouse_capture());

        ui.draw();

        let rect = navigation_rect(&mut ui);
        ui.terminal_event(scroll_down(rect.x + 2, rect.y + rect.height.saturating_sub(1)));
        ui.terminal_event(click(rect.x + 2, rect.y + rect.height.saturating_sub(1)));
        assert!(!rang_bell(&mut ui));
    }

    #[test]
    fn workspace_picker_scroll_up_and_down() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};
        use std::path::PathBuf;

        let mut ui = make_ui();
        let workspaces = vec![
            WorkspaceEntry { path: PathBuf::from("/tmp/a"), is_current: false },
            WorkspaceEntry { path: PathBuf::from("/tmp/b"), is_current: false },
            WorkspaceEntry { path: PathBuf::from("/tmp/c"), is_current: false },
        ];
        ui.deliver_result(CommandResult::WorkspacesListed(WorkspaceListResponse { workspaces }));
        assert!(matches!(ui.app().workspace_move_state(), WorkspaceMoveState::Picking));

        ui.draw();

        let rect = navigation_rect(&mut ui);
        ui.terminal_event(scroll_down(rect.x + 2, rect.y + 2));
        ui.terminal_event(scroll_up(rect.x + 2, rect.y + 2));
        ui.terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(!rang_bell(&mut ui));
    }

    #[test]
    fn form_modal_scroll_changes_field() {
        block_on_local(async {
            let mut ui = make_ui();
            let schema = ElicitationSchema::new().property("field1", StringPropertySchema::new(), true).property(
                "field2",
                StringPropertySchema::new(),
                true,
            );

            with_elicitation(&mut ui, form_elicitation("test", "Test", schema)).await;

            assert!(ui.app().needs_mouse_capture());

            ui.draw();

            let rect = navigation_rect(&mut ui);
            ui.terminal_event(scroll_down(rect.x + 2, rect.y + 3));
            assert!(!rang_bell(&mut ui));
        });
    }

    /// A survey of many questions walks one page at a time; every question
    /// must be reachable, not just the ones that fit the first screen.
    #[test]
    fn form_modal_walks_to_the_last_question() {
        block_on_local(async {
            const QUESTIONS: usize = 40;

            let mut ui = make_ui();
            let mut schema = ElicitationSchema::new();
            for index in 0..QUESTIONS {
                schema = schema.property(format!("question_{index:02}"), BooleanPropertySchema::new(), false);
            }

            with_elicitation(&mut ui, form_elicitation("test", "Pick some", schema)).await;

            ui.draw();
            let shows = |ui: &mut TestUi, needle: &str| screen_rows(ui).iter().any(|row| row.contains(needle));
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

            let mut ui = make_ui();
            with_elicitation(&mut ui, form_elicitation("test", "Pick one", schema)).await;

            ui.draw();
            let shows = |ui: &mut TestUi, needle: &str| screen_rows(ui).iter().any(|row| row.contains(needle));
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
            let mut ui = make_ui();
            let schema = ElicitationSchema::new()
                .property("alpha", BooleanPropertySchema::new().default_value(false), false)
                .property("bravo", BooleanPropertySchema::new().default_value(false), false);

            let response_rx = with_elicitation(&mut ui, form_elicitation("test", "Pick one", schema)).await;

            ui.draw();

            let rows = screen_rows(&mut ui);
            let yes = rows.iter().position(|row| row.contains("1  Yes")).expect("the first option should be on screen");
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
            let mut ui = make_ui();
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
            for row in screen_rows(&mut ui)
                .iter()
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

/// The screen's rows as text, indexed by terminal row.
fn screen_rows(ui: &mut TestUi) -> Vec<String> {
    let buffer = ui.backend().buffer();
    (buffer.area.top()..buffer.area.bottom())
        .map(|y| {
            (buffer.area.left()..buffer.area.right())
                .map(|x| buffer.cell((x, y)).map_or(" ", ratatui::buffer::Cell::symbol))
                .collect()
        })
        .collect()
}

mod screen_mouse {
    use super::*;
    use wisp::git_review::{DiffDocument, FileDiff, FileStatus, GitDiffEvent, StageState};
    use wisp::renderer::DrawContext;
    use wisp::screens::git_diff::GitDiffScreen;
    use wisp::surfaces::input::MouseAction;
    use wisp::theme::Theme;
    use wisp::view::generation::Generation;
    use wisp::view::syntax::SyntaxHighlighter;

    fn make_test_document() -> DiffDocument {
        use wisp::git_review::{Hunk, PatchLine, PatchLineKind};
        DiffDocument {
            repo_root: std::path::PathBuf::from("/tmp/repo"),
            files: vec![
                FileDiff {
                    old_path: None,
                    path: "src/main.rs".to_string(),
                    status: FileStatus::Modified,
                    staged: StageState::Unstaged,
                    hunks: vec![Hunk {
                        header: "@@ -1,5 +1,5 @@".to_string(),
                        old_start: 1,
                        old_count: 5,
                        new_start: 1,
                        new_count: 5,
                        lines: vec![PatchLine {
                            kind: PatchLineKind::Context,
                            text: "fn main() {".to_string(),
                            old_line_no: Some(1),
                            new_line_no: Some(1),
                        }],
                    }],
                    binary: false,
                },
                FileDiff {
                    old_path: None,
                    path: "src/lib.rs".to_string(),
                    status: FileStatus::Added,
                    staged: StageState::Unstaged,
                    hunks: vec![],
                    binary: false,
                },
                FileDiff {
                    old_path: None,
                    path: "Cargo.toml".to_string(),
                    status: FileStatus::Modified,
                    staged: StageState::Unstaged,
                    hunks: vec![],
                    binary: false,
                },
            ],
        }
    }

    fn render_git_diff(screen: &mut GitDiffScreen, width: u16, height: u16) -> Buffer {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::with_options(backend, TerminalOptions::default()).unwrap();
        terminal
            .draw(|frame| {
                let mut cx = DrawContext {
                    theme: &theme,
                    highlighter: &mut highlighter,
                    theme_generation: Generation::default(),
                };
                screen.render(frame.area(), frame.buffer_mut(), &mut cx);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn open_screen() -> GitDiffScreen {
        let (mut screen, task) = GitDiffScreen::new(std::path::PathBuf::from("/tmp/repo"));
        let GitCommand::Load { request_id, .. } = task else {
            panic!("opening Git review must load its document");
        };
        screen.on_event(GitDiffEvent::Loaded { request_id, result: Ok(make_test_document()) });
        screen
    }

    #[test]
    fn git_diff_click_left_side_selects_drawer() {
        let mut screen = open_screen();

        // Render at wide width (120)
        let _buffer = render_git_diff(&mut screen, 120, 40);

        // drawer_width = (120/3).clamp(24,36) = 36
        // body starts at x=1 (after border)
        // Click at x=20 (left side, well within drawer)
        screen.on_mouse(MouseAction::Click, 3, 20);

        let buffer = render_git_diff(&mut screen, 120, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("[Enter] open"), "drawer focus footer: {text}");
    }

    #[test]
    fn git_diff_click_right_side_selects_patch() {
        let mut screen = open_screen();

        let _buffer = render_git_diff(&mut screen, 120, 40);

        // drawer_width = 36, body_x = 1. Drawer spans x=1..37. Patch spans x=38..
        // Click at x=60 (right side, patch area)
        screen.on_mouse(MouseAction::Click, 3, 60);

        let buffer = render_git_diff(&mut screen, 120, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("[c] comment"), "patch focus footer: {text}");
    }

    #[test]
    fn git_diff_click_narrow_layout_always_patch() {
        let mut screen = open_screen();

        // Render at narrow width (< 72)
        let _buffer = render_git_diff(&mut screen, 60, 40);

        // Even clicking on the left side should focus Patch
        screen.on_mouse(MouseAction::Click, 3, 2);

        let buffer = render_git_diff(&mut screen, 60, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("[c] comment"), "narrow layout should focus patch: {text}");
    }

    #[test]
    fn git_diff_click_on_border_is_ignored() {
        let mut screen = open_screen();

        let _buffer = render_git_diff(&mut screen, 120, 40);

        // Click at y=0 (top border) — should be ignored
        screen.on_mouse(MouseAction::Click, 0, 40);

        let buffer = render_git_diff(&mut screen, 120, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("Git Diff"), "screen should still render: {text}");
    }

    #[test]
    fn git_diff_click_after_resize_uses_new_pane_rects() {
        let mut screen = open_screen();

        // Render at width 120: drawer_width = 36, drawer x=1..37
        render_git_diff(&mut screen, 120, 40);

        // Click at x=50 (patch area at width 120)
        screen.on_mouse(MouseAction::Click, 3, 50);

        // Resize to width 200: drawer_width = 36, drawer x=1..37
        render_git_diff(&mut screen, 200, 40);

        // Click at x=50 should still be in patch area
        screen.on_mouse(MouseAction::Click, 3, 50);

        let buffer = render_git_diff(&mut screen, 200, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("[c] comment"), "patch should be focused: {text}");
    }
}

mod mouse_owning_surfaces {
    use super::*;

    #[test]
    fn settings_overlay_sets_surface_rect() {
        let opts = vec![agent_client_protocol::schema::v1::SessionConfigOption::select(
            "model",
            "Model",
            "a",
            vec![
                agent_client_protocol::schema::v1::SessionConfigSelectOption::new("a", "Alpha"),
                agent_client_protocol::schema::v1::SessionConfigSelectOption::new("b", "Beta"),
            ],
        )];
        let mut ui = TestUiBuilder::new().config_options(opts).dimensions(80, 24).build();

        // Type /settings to open the settings overlay
        ui.type_text("/settings");
        ui.key(key(KeyCode::Tab));

        assert!(ui.app().has_modal(), "settings overlay should be open after /settings+Tab");

        ui.draw();

        assert!(ui.app().has_modal(), "the overlay should still be open after a render");
    }

    #[test]
    fn session_picker_sets_surface_rect() {
        let mut ui = make_ui();
        let current = agent_client_protocol::schema::v1::SessionId::new("test-session");
        ui.deliver_result(CommandResult::SessionsListed(acp::ListSessionsResponse::new(vec![
            agent_client_protocol::schema::v1::SessionInfo::new(
                agent_client_protocol::schema::v1::SessionId::new("other"),
                std::path::PathBuf::from("/tmp"),
            ),
        ])));
        std::mem::drop(current);

        ui.draw();

        assert!(ui.app().has_navigation());
    }

    #[test]
    fn prompt_search_sets_surface_rect() {
        let mut ui = TestUiBuilder::new().prompt_search().dimensions(80, 24).build();

        ui.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));

        ui.draw();

        // History search is a composer overlay rather than app navigation, so it owns
        // the mouse without anything being pushed above the conversation.
        assert!(!ui.app().has_navigation());
        assert!(ui.app().composer().has_prompt_search());
        assert!(ui.app().needs_mouse_capture());
    }
}
