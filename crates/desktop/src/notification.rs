use crate::threads::{Thread, ThreadEvent};
use gpui::{App, SystemNotification};

pub(crate) fn show(thread: &Thread, event: &ThreadEvent, title: String, cx: &App) {
    let Some(session_id) = thread.session_id() else {
        return;
    };
    let body = match event {
        ThreadEvent::NeedsInput => {
            thread.elicitation().map_or_else(|| NEEDS_INPUT.to_owned(), |(_, elicitation)| elicitation.question())
        }
        _ => TURN_FINISHED.to_owned(),
    };
    cx.show_system_notification(SystemNotification {
        tag: session_id.0.clone().into(),
        title: title.into(),
        body: body.into(),
        actions: Vec::new(),
    });
}

const TURN_FINISHED: &str = "The agent finished its turn";
const NEEDS_INPUT: &str = "The agent needs your input";
