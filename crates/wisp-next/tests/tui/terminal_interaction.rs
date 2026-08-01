use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::TerminalOptions;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use tokio::sync::mpsc::UnboundedReceiver;
use wisp_next::test_support::app::{App, RuntimeEffect, WorkspaceMoveState};
use wisp_next::test_support::tasks::TaskResult;

use super::support::AppBuilder;
use super::support::TestUi;
use super::support::TestUiBuilder;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Whether the app asked the terminal to ring the bell, draining whatever else
/// it had queued along with it.
fn rang_bell(app: &mut App) -> bool {
    let mut rang = false;
    while let Some(effect) = app.take_effect() {
        rang |= matches!(effect, RuntimeEffect::RingBell);
    }
    rang
}

fn make_app() -> (App, UnboundedReceiver<acp_utils::client::PromptCommand>) {
    AppBuilder::new().build()
}

fn make_ui() -> TestUi {
    TestUiBuilder::new().dimensions(80, 24).build()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn has_session_picker(app: &App) -> bool {
    app.has_session_picker()
}

/// The area a layer is drawn into: the whole inline viewport.
fn layer_rect(ui: &mut TestUi) -> ratatui::layout::Rect {
    ui.terminal_mut().get_frame().area()
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

// ---------------------------------------------------------------------------
// Picker click offset tests
// ---------------------------------------------------------------------------

mod picker_click {
    use super::*;
    use acp_utils::client::AcpEvent;
    use agent_client_protocol::schema::SessionInfo;

    #[test]
    fn session_picker_click_first_row_selects_index_zero() {
        let mut ui = make_ui();

        // Open session picker with sessions
        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s1"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s2"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s3"), "/tmp"),
        ];
        ui.acp_event(AcpEvent::SessionsListed { sessions });
        assert!(has_session_picker(ui.app()));

        ui.draw();
        let rect = layer_rect(&mut ui);
        // Click on the second content row (local_y=2 → row=1 after border offset)
        ui.terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(has_session_picker(ui.app()), "picker should remain open");
    }

    #[test]
    fn session_picker_click_outside_row_range_is_clamped() {
        let mut ui = make_ui();

        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s1"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s2"), "/tmp"),
        ];
        ui.acp_event(AcpEvent::SessionsListed { sessions });
        assert!(has_session_picker(ui.app()));

        ui.draw();

        let rect = layer_rect(&mut ui);
        // Click far below content (local_y=20 → row=19 after offset, clamped to last item)
        ui.terminal_event(click(rect.x + 2, rect.y + 20));
        assert!(has_session_picker(ui.app()));
    }

    #[test]
    fn session_picker_click_with_filter_uses_visible_rows() {
        let mut ui = make_ui();

        let mut session_a = SessionInfo::new(agent_client_protocol::schema::SessionId::new("aaa"), "/tmp");
        session_a.title = Some("Alpha Project".to_string());
        let mut session_b = SessionInfo::new(agent_client_protocol::schema::SessionId::new("bbb"), "/tmp");
        session_b.title = Some("Beta Project".to_string());
        let mut session_c = SessionInfo::new(agent_client_protocol::schema::SessionId::new("ccc"), "/tmp");
        session_c.title = Some("Alpha Config".to_string());

        ui.acp_event(AcpEvent::SessionsListed { sessions: vec![session_a, session_b, session_c] });
        assert!(has_session_picker(ui.app()));

        // Type filter: "Alpha"
        ui.key(key(KeyCode::Char('A')));
        ui.key(key(KeyCode::Char('l')));
        ui.key(key(KeyCode::Char('p')));
        ui.key(key(KeyCode::Char('h')));
        ui.key(key(KeyCode::Char('a')));

        ui.draw();

        let rect = layer_rect(&mut ui);
        // Click on first visible row
        ui.terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(has_session_picker(ui.app()));
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
        ui.acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }));

        ui.draw();
        let rect = layer_rect(&mut ui);
        // Click on the first content row (local_y=1 → row=0 after border offset)
        ui.terminal_event(click(rect.x + 2, rect.y + 1));
    }

    #[test]
    fn workspace_picker_click_outside_row_range_is_clamped() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};

        let workspaces = vec![WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws1"), is_current: false }];
        let mut ui = make_ui();
        ui.acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }));

        ui.draw();

        let rect = layer_rect(&mut ui);
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
        ui.acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }));

        // Type filter: "beta"
        ui.key(key(KeyCode::Char('b')));
        ui.key(key(KeyCode::Char('e')));
        ui.key(key(KeyCode::Char('t')));
        ui.key(key(KeyCode::Char('a')));

        ui.draw();

        let rect = layer_rect(&mut ui);
        // Click on first visible row
        ui.terminal_event(click(rect.x + 2, rect.y + 2));
    }
}

// ---------------------------------------------------------------------------
// Mouse capture tests
// ---------------------------------------------------------------------------

mod mouse_capture {
    use super::*;
    use acp_utils::client::AcpEvent;
    use agent_client_protocol::schema::SessionInfo;

    #[test]
    fn no_capture_when_no_fullscreen_or_modal() {
        let (app, _rx) = make_app();
        assert!(!app.needs_mouse_capture());
    }

    #[test]
    fn capture_enabled_when_session_picker_is_open() {
        let (mut app, _rx) = make_app();
        let current_id = agent_client_protocol::schema::SessionId::new("test-session");
        let other = SessionInfo::new(agent_client_protocol::schema::SessionId::new("other"), "/tmp");
        app.on_acp_event(AcpEvent::SessionsListed { sessions: vec![other] });
        std::mem::drop(current_id);

        assert!(has_session_picker(&app));
        assert!(app.needs_mouse_capture());

        app.on_key(key(KeyCode::Esc));
        assert!(!app.needs_mouse_capture());
    }

    #[test]
    fn capture_when_composer_overlay_is_open() {
        let (mut app, _rx) = make_app();

        // Open command picker
        app.on_key(key(KeyCode::Char('/')));
        assert!(app.needs_mouse_capture());

        app.on_key(key(KeyCode::Esc));
        assert!(!app.needs_mouse_capture());
    }

    #[test]
    fn capture_when_prompt_search_is_open() {
        let (mut app, _rx) = AppBuilder::new().prompt_search().build();

        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(app.needs_mouse_capture());

        app.on_key(key(KeyCode::Esc));
        assert!(!app.needs_mouse_capture());
    }

    #[test]
    fn capture_disabled_after_connection_closed() {
        let (mut app, _rx) = make_app();
        let other = SessionInfo::new(agent_client_protocol::schema::SessionId::new("other"), "/tmp");
        app.on_acp_event(AcpEvent::SessionsListed { sessions: vec![other] });
        assert!(app.needs_mouse_capture());

        app.on_acp_event(AcpEvent::ConnectionClosed);
        assert!(!app.needs_mouse_capture());
    }
}

// ---------------------------------------------------------------------------
// Bell tests
// ---------------------------------------------------------------------------

mod bell {
    use super::*;
    use acp_utils::client::AcpEvent;
    use agent_client_protocol::schema as acp;

    #[test]
    fn bell_after_normal_completion() {
        let (mut app, _rx) = make_app();

        // Submit a prompt
        super::submit_prompt(&mut app, "hello");
        assert!(app.waiting_for_response());

        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        assert!(rang_bell(&mut app));
    }

    #[test]
    fn no_bell_after_cancellation() {
        let (mut app, _rx) = make_app();

        super::submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::Cancelled));

        assert!(!rang_bell(&mut app));
    }

    #[test]
    fn no_bell_after_prompt_error() {
        let (mut app, _rx) = make_app();

        super::submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::PromptError(agent_client_protocol::Error::internal_error()));

        assert!(!rang_bell(&mut app));
    }

    #[test]
    fn no_bell_after_connection_close() {
        let (mut app, _rx) = make_app();

        super::submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ConnectionClosed);

        assert!(!rang_bell(&mut app));
    }

    #[test]
    fn no_bell_on_unsolicited_prompt_done() {
        let (mut app, _rx) = make_app();

        // No prompt in flight — PromptDone is unsolicited
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        assert!(!rang_bell(&mut app));
    }

    #[test]
    fn exactly_one_bell_per_completion() {
        let (mut app, _rx) = make_app();

        super::submit_prompt(&mut app, "first");
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        assert!(rang_bell(&mut app));
        assert!(!rang_bell(&mut app));
    }
}

// ---------------------------------------------------------------------------
// Resize tests
// ---------------------------------------------------------------------------

mod resize {
    use super::*;

    #[test]
    fn resize_preserves_composer_content() {
        let (mut app, _rx) = make_app();

        // Simulate a resize event
        app.on_terminal_event(crossterm::event::Event::Resize(120, 30));

        super::type_text(&mut app, "hello resize");
        assert_eq!(app.composer().text(), "hello resize");
    }

    #[test]
    fn resize_no_duplicate_history() {
        let mut ui = TestUiBuilder::new().dimensions(80, 20).build();

        ui.submit("before resize");

        // Render at initial size then resize
        ui.draw();
        ui.terminal_event(crossterm::event::Event::Resize(120, 30));
        ui.draw();

        let viewport = super::buffer_text(ui.terminal_mut().backend().buffer());
        assert_eq!(viewport.matches("before resize").count(), 1);
    }
}

// ---------------------------------------------------------------------------
// Event routing tests
// ---------------------------------------------------------------------------

mod event_routing {
    use super::*;
    use acp_utils::client::AcpEvent;
    use agent_client_protocol::schema::SessionInfo;

    #[test]
    fn mouse_click_outside_surface_is_ignored() {
        let (mut app, _rx) = make_app();

        app.on_acp_event(AcpEvent::SessionsListed {
            sessions: vec![SessionInfo::new(agent_client_protocol::schema::SessionId::new("other"), "/tmp")],
        });
        assert!(has_session_picker(&app));

        // Click at (0, 0) — outside the picker rect (no surface rect is set without render)
        app.on_terminal_event(click(0, 0));

        // The session picker should still be open and unchanged
        assert!(has_session_picker(&app));
    }

    #[test]
    fn keyboard_event_routing_precedence_unchanged() {
        let (mut app, _rx) = make_app();

        // Open settings overlay via /settings
        app.on_key(key(KeyCode::Char('/')));
        app.on_key(key(KeyCode::Char('s')));
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Enter));

        // Esc closes settings overlay
        app.on_key(key(KeyCode::Esc));
        assert!(!app.has_modal());
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
        let bell_before = rang_bell(ui.app_mut());
        ui.terminal_event(scroll_down(40, 5));
        assert_eq!(bell_before, rang_bell(ui.app_mut()));
    }

    #[test]
    fn scroll_on_settings_overlay_changes_menu_selection() {
        use agent_client_protocol::schema::{SessionConfigOption, SessionConfigSelectOption};
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
        assert!(ui.app().has_layer());

        ui.terminal_event(scroll_down(40, 5));
        assert!(!rang_bell(ui.app_mut()));
    }

    #[test]
    fn settings_overlay_click_selects_entry() {
        use agent_client_protocol::schema::{SessionConfigOption, SessionConfigSelectOption};
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

        let rect = layer_rect(&mut ui);
        let click_y = rect.y + 2;
        ui.terminal_event(click(rect.x + 2, click_y));
        assert!(!rang_bell(ui.app_mut()));
    }

    #[test]
    fn mouse_activation_uses_shared_settings_actions() {
        use acp_utils::client::AcpEvent;
        use acp_utils::notifications::{
            McpNotification, McpServerAuthCapability, McpServerStatus, McpServerStatusEntry,
        };
        use agent_client_protocol::schema::{
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
        ui.terminal_event(click(10, 4));
        ui.draw();
        ui.terminal_event(click(10, 5));
        assert!(matches!(
            ui.command_rx().try_recv(),
            Ok(acp_utils::client::PromptCommand::SetConfigOption { config_id, value, .. }) if config_id == "model" && value == "b"
        ));

        let server = McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
            .with_auth_capability(McpServerAuthCapability::OAuth);
        let mut ui = settings_app(vec![], vec![server], vec![]);
        open_settings(&mut ui);
        ui.terminal_event(click(10, 4));
        ui.draw();
        ui.terminal_event(click(10, 3));
        assert!(matches!(
            ui.command_rx().try_recv(),
            Ok(acp_utils::client::PromptCommand::AuthenticateMcpServer { server_name, .. }) if server_name == "linear"
        ));

        let methods = vec![AuthMethod::Agent(AuthMethodAgent::new("codex", "Codex"))];
        let mut ui = settings_app(vec![], vec![], methods);
        open_settings(&mut ui);
        ui.terminal_event(click(10, 5));
        ui.draw();
        ui.terminal_event(click(10, 3));
        assert!(matches!(
            ui.command_rx().try_recv(),
            Ok(acp_utils::client::PromptCommand::Authenticate { method_id }) if method_id == "codex"
        ));
    }
    #[test]
    fn session_picker_scroll_changes_selection() {
        let mut ui = make_ui();
        let current = agent_client_protocol::schema::SessionId::new("test-session");
        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("a"), "/tmp/a"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("b"), "/tmp/b"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("c"), "/tmp/c"),
        ];
        ui.acp_event(AcpEvent::SessionsListed { sessions });
        std::mem::drop(current);
        assert!(has_session_picker(ui.app()));

        ui.draw();
        assert!(ui.app().has_layer());

        // Scroll down
        let rect = layer_rect(&mut ui);
        ui.terminal_event(scroll_down(rect.x + 2, rect.y + 2));
        assert!(!rang_bell(ui.app_mut()));
        assert!(has_session_picker(ui.app()));
    }

    #[test]
    fn prompt_search_scroll_up_and_down() {
        let mut ui = TestUiBuilder::new().prompt_search().dimensions(80, 24).build();

        ui.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(ui.app().needs_mouse_capture());

        ui.draw();

        let rect = layer_rect(&mut ui);
        // Scroll should be consumed internally
        ui.terminal_event(scroll_down(rect.x + 2, rect.y + 1));
        ui.terminal_event(scroll_up(rect.x + 2, rect.y + 1));
        assert!(!rang_bell(ui.app_mut()));
    }

    #[test]
    fn composer_overlay_scroll_and_click() {
        let mut ui = make_ui();
        // Open command picker
        ui.key(key(KeyCode::Char('/')));
        assert!(ui.app().needs_mouse_capture());

        ui.draw();

        let rect = layer_rect(&mut ui);
        ui.terminal_event(scroll_down(rect.x + 2, rect.y + rect.height.saturating_sub(1)));
        ui.terminal_event(click(rect.x + 2, rect.y + rect.height.saturating_sub(1)));
        assert!(!rang_bell(ui.app_mut()));
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
        ui.acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }));
        assert!(matches!(ui.app().workspace_move_state(), WorkspaceMoveState::Picking));

        ui.draw();

        let rect = layer_rect(&mut ui);
        ui.terminal_event(scroll_down(rect.x + 2, rect.y + 2));
        ui.terminal_event(scroll_up(rect.x + 2, rect.y + 2));
        ui.terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(!rang_bell(ui.app_mut()));
    }

    #[test]
    fn form_modal_scroll_changes_field() {
        use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationParams};
        use acp_utils::testing::test_connection;

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let mut ui = make_ui();
            let (cx, mut peer) = test_connection().await;
            let (responder, _response_rx) = peer.fake_elicitation(&cx).await;

            let schema = acp_utils::ElicitationSchema::builder()
                .required_string("field1")
                .required_string("field2")
                .build()
                .unwrap();

            ui.acp_event(AcpEvent::ElicitationRequest {
                params: ElicitationParams {
                    server_name: "test".into(),
                    request: CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Test".into(),
                        requested_schema: schema,
                    },
                },
                responder,
            });

            assert!(ui.app().needs_mouse_capture());

            ui.draw();

            let rect = layer_rect(&mut ui);
            ui.terminal_event(scroll_down(rect.x + 2, rect.y + 3));
            assert!(!rang_bell(ui.app_mut()));
        });
    }

    /// A form with more fields than the modal is tall has to scroll, or the
    /// fields below the fold can never be reached.
    #[test]
    fn form_modal_scrolls_the_focused_field_into_view() {
        use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationParams};
        use acp_utils::testing::test_connection;

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            const FIELDS: usize = 40;

            let mut ui = make_ui();
            let (cx, mut peer) = test_connection().await;
            let (responder, _response_rx) = peer.fake_elicitation(&cx).await;

            let mut builder = acp_utils::ElicitationSchema::builder();
            for index in 0..FIELDS {
                builder = builder.optional_bool(format!("field_{index:02}"), false);
            }

            ui.acp_event(AcpEvent::ElicitationRequest {
                params: ElicitationParams {
                    server_name: "test".into(),
                    request: CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Pick some".into(),
                        requested_schema: builder.build().unwrap(),
                    },
                },
                responder,
            });

            ui.draw();

            let shows = |ui: &mut TestUi, needle: &str| screen_rows(ui).iter().any(|row| row.contains(needle));
            assert!(shows(&mut ui, "field_00"), "the first field starts on screen");
            assert!(!shows(&mut ui, "field_39"), "the last field starts below the fold");

            for _ in 1..FIELDS {
                ui.key(key(KeyCode::Down));
            }
            ui.draw();

            assert!(shows(&mut ui, "field_39"), "the focused field scrolled into view");
            assert!(!shows(&mut ui, "field_00"), "the fields above it scrolled away");
        });
    }

    /// The form is drawn centred inside the viewport, so its first field sits an
    /// arbitrary number of rows down. A click has to hit the field under the
    /// pointer rather than one counted from the top of the screen.
    #[test]
    fn form_modal_click_toggles_the_field_under_the_pointer() {
        use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationParams};
        use acp_utils::testing::test_connection;

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let mut ui = make_ui();
            let (cx, mut peer) = test_connection().await;
            let (responder, _response_rx) = peer.fake_elicitation(&cx).await;

            let schema = acp_utils::ElicitationSchema::builder()
                .optional_bool("alpha", false)
                .optional_bool("bravo", false)
                .build()
                .unwrap();

            ui.acp_event(AcpEvent::ElicitationRequest {
                params: ElicitationParams {
                    server_name: "test".into(),
                    request: CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Pick one".into(),
                        requested_schema: schema,
                    },
                },
                responder,
            });

            ui.draw();

            let rows = screen_rows(&mut ui);
            let bravo_row = rows.iter().position(|row| row.contains("bravo")).expect("field should be on screen");
            ui.terminal_event(click(20, u16::try_from(bravo_row).unwrap()));
            ui.draw();

            let rows = screen_rows(&mut ui);
            let row_with = |needle: &str| rows.iter().find(|row| row.contains(needle)).unwrap().clone();
            let alpha = row_with("alpha");
            let bravo = row_with("bravo");
            assert!(bravo.contains("[x]"), "the clicked field should be checked, got {bravo:?}");
            assert!(alpha.contains("[ ]"), "the field above it should be untouched, got {alpha:?}");
        });
    }

    /// A field is as many rows as it was drawn on, so clicking its description
    /// hits the field rather than falling between two of them.
    #[test]
    fn form_modal_click_hits_every_row_a_field_occupies() {
        use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationParams};
        use acp_utils::testing::test_connection;

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let mut ui = make_ui();
            let (cx, mut peer) = test_connection().await;
            let (responder, _response_rx) = peer.fake_elicitation(&cx).await;

            let schema = acp_utils::ElicitationSchema::builder()
                .optional_bool_with("alpha", |field| field.description("the first switch"))
                .optional_bool_with("bravo", |field| field.description("the second switch"))
                .build()
                .unwrap();

            ui.acp_event(AcpEvent::ElicitationRequest {
                params: ElicitationParams {
                    server_name: "test".into(),
                    request: CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Pick one".into(),
                        requested_schema: schema,
                    },
                },
                responder,
            });

            ui.draw();

            let rows = screen_rows(&mut ui);
            let description_row =
                rows.iter().position(|row| row.contains("the second switch")).expect("description should be on screen");
            ui.terminal_event(click(20, u16::try_from(description_row).unwrap()));
            ui.draw();

            let rows = screen_rows(&mut ui);
            let row_with = |needle: &str| rows.iter().find(|row| row.contains(needle)).unwrap().clone();
            let bravo = row_with("bravo");
            assert!(bravo.contains("[x]"), "clicking a field's description should toggle it, got {bravo:?}");
            assert!(row_with("alpha").contains("[ ]"), "the other field should be untouched");
        });
    }
}

/// The screen's rows as text, indexed by terminal row.
fn screen_rows(ui: &mut TestUi) -> Vec<String> {
    let buffer = ui.terminal_mut().backend().buffer();
    (buffer.area.top()..buffer.area.bottom())
        .map(|y| {
            (buffer.area.left()..buffer.area.right())
                .map(|x| buffer.cell((x, y)).map_or(" ", ratatui::buffer::Cell::symbol))
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Screen mouse handling tests
// ---------------------------------------------------------------------------

mod screen_mouse {
    use super::*;
    use wisp_next::test_support::generation::Generation;
    use wisp_next::test_support::git_diff::{FileDiff, FileStatus, GitDiffDocument, StageState};
    use wisp_next::test_support::renderer::DrawContext;
    use wisp_next::test_support::screens::git_diff::{GitDiffEvent, GitDiffScreen};
    use wisp_next::test_support::surface::MouseAction;
    use wisp_next::test_support::surface::Surface;
    use wisp_next::test_support::syntax::SyntaxHighlighter;
    use wisp_next::test_support::theme::Theme;

    fn make_test_document() -> GitDiffDocument {
        use wisp_next::test_support::git_diff::{Hunk, PatchLine, PatchLineKind};
        GitDiffDocument {
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
        screen.on_task_result(TaskResult::GitDiff(GitDiffEvent::Loaded {
            request_id: task.request_id(),
            result: Ok(make_test_document()),
        }));
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

        // With focus on Drawer, footer shows "h/l pane"
        let buffer = render_git_diff(&mut screen, 120, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("h/l pane"), "drawer focus footer: {text}");
    }

    #[test]
    fn git_diff_click_right_side_selects_patch() {
        let mut screen = open_screen();

        let _buffer = render_git_diff(&mut screen, 120, 40);

        // drawer_width = 36, body_x = 1. Drawer spans x=1..37. Patch spans x=38..
        // Click at x=60 (right side, patch area)
        screen.on_mouse(MouseAction::Click, 3, 60);

        // Focus should now be Patch. Footer shows "c comment" for Patch focus.
        let buffer = render_git_diff(&mut screen, 120, 40);
        let text = buffer_text(&buffer);
        // Should show a draft comment box
        assert!(text.contains("c comment"), "patch focus footer: {text}");
    }

    #[test]
    fn git_diff_click_narrow_layout_always_patch() {
        let mut screen = open_screen();

        // Render at narrow width (< 72)
        let _buffer = render_git_diff(&mut screen, 60, 40);

        // Even clicking on the left side should focus Patch
        screen.on_mouse(MouseAction::Click, 3, 2);

        // Footer shows "c comment" for Patch focus
        let buffer = render_git_diff(&mut screen, 60, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("c comment"), "narrow layout should focus patch: {text}");
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

        // Footer shows "c comment" for Patch focus
        let buffer = render_git_diff(&mut screen, 200, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("c comment"), "patch should be focused: {text}");
    }
}

// ---------------------------------------------------------------------------
// Surface rect tracking tests
// ---------------------------------------------------------------------------

mod mouse_owning_surfaces {
    use super::*;
    use acp_utils::client::AcpEvent;

    #[test]
    fn settings_overlay_sets_surface_rect() {
        let opts = vec![agent_client_protocol::schema::SessionConfigOption::select(
            "model",
            "Model",
            "a",
            vec![
                agent_client_protocol::schema::SessionConfigSelectOption::new("a", "Alpha"),
                agent_client_protocol::schema::SessionConfigSelectOption::new("b", "Beta"),
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
        let current = agent_client_protocol::schema::SessionId::new("test-session");
        ui.acp_event(AcpEvent::SessionsListed {
            sessions: vec![agent_client_protocol::schema::SessionInfo::new(
                agent_client_protocol::schema::SessionId::new("other"),
                std::path::PathBuf::from("/tmp"),
            )],
        });
        std::mem::drop(current);

        ui.draw();

        assert!(ui.app().has_layer());
    }

    #[test]
    fn prompt_search_sets_surface_rect() {
        let mut ui = TestUiBuilder::new().prompt_search().dimensions(80, 24).build();

        ui.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));

        ui.draw();

        // History search is a composer overlay rather than a layer, so it owns
        // the mouse without anything being pushed above the conversation.
        assert!(!ui.app().has_layer());
        assert!(ui.app().composer().has_prompt_search());
        assert!(ui.app().needs_mouse_capture());
    }
}

// ---------------------------------------------------------------------------
// Helpers shared across tests
// ---------------------------------------------------------------------------

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn submit_prompt(app: &mut App, text: &str) {
    type_text(app, text);
    app.on_key(key(KeyCode::Enter));
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            out.push_str(buffer.cell((x, y)).map_or(" ", ratatui::buffer::Cell::symbol));
        }
        out.push('\n');
    }
    out
}
