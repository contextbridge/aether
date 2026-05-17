use super::App;
use crate::components::status_line::StatusLine;
use crate::settings;
use tui::{Component, Frame, ViewContext};

pub fn build_frame(app: &mut App, context: &ViewContext) -> Frame {
    if let Some(ref mut overlay) = app.settings_overlay {
        let overlay_frame = overlay.build_frame(context);
        return Frame::vstack(vec![overlay_frame, make_status_line(app).render(context)]);
    }

    if app.screen_router.is_full_screen_mode() {
        return app.screen_router.render(context);
    }

    let conv_frame = app.conversation_screen.render(context);
    Frame::vstack(vec![conv_frame, make_status_line(app).render(context)])
}

fn make_status_line(app: &App) -> StatusLine<'_> {
    StatusLine {
        workspace_status: &app.workspace_status,
        agent_name: &app.agent_name,
        config_options: &app.config_options,
        context_usage: app.context_usage,
        waiting_for_response: app.conversation_screen.is_waiting(),
        unhealthy_server_count: settings::unhealthy_server_count(&app.server_statuses),
        content_padding: app.content_padding,
        exit_confirmation_active: app.exit_confirmation_active(),
    }
}
