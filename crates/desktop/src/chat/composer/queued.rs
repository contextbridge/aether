use crate::ui::tokens::ICON_SIZE_SMALL;
use crate::ui::{Icon, IconName, UiStyles, UiTheme};
use clankerdiff_rouge_shell::rouge::core::{AnyView, Div, Styled, View, div, text, view};
use client_core::session_core::QueuedPrompt;

pub(crate) fn queued_prompts(prompts: &[QueuedPrompt]) -> Div {
    let muted = UiTheme::current().text_muted;
    let shown = if prompts.len() > MAX_QUEUED_ROWS + 1 { MAX_QUEUED_ROWS } else { prompts.len() };
    let rows: Vec<AnyView> = prompts[..shown]
        .iter()
        .map(|prompt| {
            let first = prompt.submission.text.lines().next().unwrap_or_default().to_owned();
            view! {
                <div flex items_center gap=8.0 min_w=0.0>
                    {Icon::new(IconName::ChevronRight, muted).size(ICON_SIZE_SMALL)}
                    <div flex_none><text small muted>"Queued"</text></div>
                    <div flex_1 min_w=0.0 overflow_hidden><text small secondary no_wrap>{first}</text></div>
                </div>
            }
            .into_any()
        })
        .collect();
    let more = (shown < prompts.len()).then(|| {
        view! { <div><text small muted>{format!("{} more queued", prompts.len() - shown)}</text></div> }
    });
    view! {
        <div test_id="queued-prompts" flex col gap=4.0>
            {..rows}
            {more}
        </div>
    }
}

const MAX_QUEUED_ROWS: usize = 3;
