//! Native notifications for threads that did something while the user was not watching them.

use crate::elicitation::question;
use crate::threads::{Thread, ThreadEvent};
use gpui::{App, SystemNotification};

/// Posts a notification titled `title` announcing `event` on `thread`.
///
/// The notification is tagged with the thread's session, so it replaces any notification the
/// thread posted before, and a click on it can find the thread again. A thread whose session has
/// not opened yet has done nothing to announce, and posts nothing.
pub(crate) fn show(thread: &Thread, event: &ThreadEvent, title: String, cx: &App) {
    let Some(session_id) = thread.session_id() else {
        return;
    };
    let body = match event {
        ThreadEvent::TurnFinished => TURN_FINISHED.to_owned(),
        ThreadEvent::NeedsInput => thread.elicitation().map_or_else(|| NEEDS_INPUT.to_owned(), question),
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
