use crate::chat::attachment::{Attachment, chip, chip_icon, thumbnail};
use crate::threads::{Thread, ThreadStatus};
use crate::ui::{
    ButtonVariant, Icon, IconName, SurfaceExt, UiStyles, UiTheme, backdrop, button, disclosure, icon_button,
};
use clankerdiff_rouge::{ReviewHandle, diff_view};
use clankerdiff_rouge_shell::rouge::core::{
    AnyView, Color, Cursor, Div, Fit, ImageSource, Styled, View, WindowHandle, div, image, pct, px, text, view,
};
use clankerdiff_rouge_shell::rouge::reactive::{Callback, Signal};
use std::sync::Arc;

pub(crate) fn prompt(
    row: usize,
    body: String,
    attachments: &[(usize, Attachment)],
    on_open: Callback<(Arc<[u8]>,)>,
) -> Div {
    let attachments: Vec<AnyView> = attachments
        .iter()
        .map(|(block, attachment)| {
            let id = format!("message-attachment-{row}-{block}");
            match attachment {
                Attachment::Image(bytes) => {
                    let bytes = bytes.clone();
                    let opened = bytes.clone();
                    thumbnail(id, ImageSource::Bytes(bytes), THUMBNAIL_SIZE)
                        .cursor(Cursor::Pointer)
                        .on_click(move |event| {
                            event.stop_propagation();
                            on_open.call(opened.clone());
                        })
                        .into_any()
                }
                Attachment::File { icon, name } => chip(id, chip_icon(*icon), name).into_any(),
            }
        })
        .collect();
    let body = (!body.is_empty()).then(|| view! { <text reading>{body}</text> });
    let files = (!attachments.is_empty()).then(|| view! { <div flex flex_wrap gap=8.0>{..attachments}</div> });
    view! {
        <div test_id={format!("prompt-{row}")} max_w=pct(PROMPT_MAX_WIDTH) px=16.0 py=8.0 flex col gap=8.0 surface
             bg=UiTheme::current().elevated>
            {body}
            {files}
        </div>
    }
}

pub(crate) fn message_actions(row: usize, text: String, hovered: Signal<bool>, window: WindowHandle) -> Div {
    view! {
        <div pt=4.0 flex opacity={move || if hovered.get() { 1.0 } else { 0.0 }}>
            <icon_button id={format!("copy-message-{row}")} icon=IconName::Copy label="Copy"
                         on_click={move || window.copy_text(text.clone())}/>
        </div>
    }
}

pub(crate) fn image_preview(bytes: Arc<[u8]>, on_close: Callback) -> impl View {
    view! {
        <backdrop id="image-preview" on_dismiss=on_close>
            <div w=pct(100.0) h=pct(100.0) min_h=0.0 cursor=Cursor::Pointer on_click={move |_| on_close.call()}>
                {image(ImageSource::Bytes(bytes)).fit(Fit::Contain).size_full()}
            </div>
        </backdrop>
    }
}

pub(crate) fn full_diff(handle: &ReviewHandle, on_close: Callback) -> impl View + use<> {
    view! {
        <backdrop id="full-diff" on_dismiss=on_close>
            <div test_id="full-diff-panel" w=pct(100.0) h=pct(100.0) min_h=0.0 flex col overflow_hidden floating>
                <div flex justify_end p=4.0>
                    <icon_button id="close-full-diff" icon=IconName::X label="Close" on_click=on_close/>
                </div>
                <div flex_1 min_h=0.0>{diff_view(*handle)}</div>
            </div>
        </backdrop>
    }
}

pub(crate) fn thinking_block(
    thought: String,
    streaming: bool,
    index: usize,
    expanded: bool,
    on_toggle: Callback,
) -> AnyView {
    let theme = UiTheme::current();
    let body = expanded.then(|| {
        view! {
            <div test_id={format!("thinking-text-{index}")} pl=12.0 border_l=px(2.0) border_color=theme.border_subtle>
                <text body muted>{thought}</text>
            </div>
        }
    });
    view! {
        <div flex col gap=8.0>
            <disclosure id={format!("thinking-{index}")} expanded on_toggle>
                <text body muted>{if streaming { "Thinking…" } else { "Thought" }}</text>
            </disclosure>
            {body}
        </div>
    }
    .into_any()
}

pub(crate) fn notice(text: &str) -> AnyView {
    line(IconName::Info, UiTheme::current().text_muted.get_untracked(), text).into_any()
}

pub(crate) fn error(index: usize, text: &str) -> AnyView {
    line(IconName::TriangleAlert, UiTheme::current().destructive.get_untracked(), text)
        .test_id(format!("transcript-error-{index}"))
        .into_any()
}

pub(crate) fn reconnect_banner(thread: Thread, status: ThreadStatus) -> Option<Div> {
    let problem = status.problem()?;
    let warning = UiTheme::current().warning.get_untracked();
    Some(view! {
        <div px=12.0 py=8.0 flex items_center gap=8.0 surface bg=UiTheme::current().elevated>
            {line(IconName::TriangleAlert, warning, problem).flex_1().min_w(0.0)}
            {reconnect_button(thread)}
        </div>
    })
}

pub(crate) fn reconnect_button(thread: Thread) -> AnyView {
    view! {
        <button id="transcript-reconnect" label="Reconnect" icon=IconName::RotateCw variant=ButtonVariant::Primary
                on_click={move || thread.reconnect()}/>
    }
}

const THUMBNAIL_SIZE: f32 = 96.0;

const PROMPT_MAX_WIDTH: f32 = 85.0;

fn line(icon: IconName, color: Color, label: &str) -> Div {
    view! {
        <div flex items_start gap=8.0>
            {Icon::new(icon, color)}
            <div flex_1 min_w=0.0><text small color=color>{label.to_owned()}</text></div>
        </div>
    }
}
