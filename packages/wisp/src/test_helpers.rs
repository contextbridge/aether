use crate::settings::WISP_HOME_ENV_MUTEX;
use acp_utils::ElicitationSchema;
use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationParams};
use std::path::Path;
use tui::{Event, KeyCode, KeyEvent, KeyModifiers};

pub fn key(code: KeyCode) -> Event {
    Event::Key(key_event(code))
}

pub fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(modified_key_event(code, modifiers))
}

pub fn key_event(code: KeyCode) -> KeyEvent {
    modified_key_event(code, KeyModifiers::NONE)
}

pub fn modified_key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

pub fn elicitation_params(
    server: impl Into<String>,
    message: impl Into<String>,
    requested_schema: ElicitationSchema,
) -> ElicitationParams {
    ElicitationParams {
        server_name: server.into(),
        request: CreateElicitationRequestParams::FormElicitationParams {
            meta: None,
            message: message.into(),
            requested_schema,
        },
    }
}

pub fn url_elicitation_params(
    server: impl Into<String>,
    elicitation_id: impl Into<String>,
    url: impl Into<String>,
) -> ElicitationParams {
    url_elicitation_params_with_message(server, "Auth", elicitation_id, url)
}

pub fn url_elicitation_params_with_message(
    server: impl Into<String>,
    message: impl Into<String>,
    elicitation_id: impl Into<String>,
    url: impl Into<String>,
) -> ElicitationParams {
    ElicitationParams {
        server_name: server.into(),
        request: CreateElicitationRequestParams::UrlElicitationParams {
            meta: None,
            message: message.into(),
            url: url.into(),
            elicitation_id: elicitation_id.into(),
        },
    }
}

pub fn with_wisp_home(path: &Path, f: impl FnOnce()) {
    let _guard = WISP_HOME_ENV_MUTEX.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let old = std::env::var_os("WISP_HOME");
    unsafe { std::env::set_var("WISP_HOME", path) };
    f();
    if let Some(value) = old {
        unsafe { std::env::set_var("WISP_HOME", value) };
    } else {
        unsafe { std::env::remove_var("WISP_HOME") };
    }
}

#[allow(dead_code)]
pub const CUSTOM_TMTHEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>name</key>
    <string>Custom</string>
    <key>settings</key>
    <array>
        <dict>
            <key>settings</key>
            <dict>
                <key>foreground</key>
                <string>#112233</string>
                <key>background</key>
                <string>#000000</string>
            </dict>
        </dict>
    </array>
</dict>
</plist>"#;
