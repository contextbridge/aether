use crate::platform::Platform;
use crate::ui::ActiveReviewTheme;
use rouge::core::{Styled, View, WindowHandle, div, dynamic, each, view};
use rouge::markdown::{MarkdownBlock, MarkdownStream};
use rouge::reactive::{Memo, Stored, memo, stored};
use rouge::widgets::{MarkdownAction, MarkdownPaint, markdown};
use std::rc::Rc;

pub(crate) fn reply(text: Memo<String>, window: WindowHandle, platform: Stored<Rc<dyn Platform>>) -> impl View {
    let stream = stored((String::new(), MarkdownStream::new()));
    let blocks = memo(move || {
        text.with(|text| {
            stream.update(|(seen, stream)| {
                match text.strip_prefix(seen.as_str()) {
                    Some("") => {}
                    Some(appended) => drop(stream.push(appended)),
                    None => drop(stream.replace(text.as_str())),
                }
                text.clone_into(seen);
                stream.document().blocks().to_vec()
            })
        })
    });
    let theme = ActiveReviewTheme::current().review_signal();
    let paint = MarkdownPaint::new(
        theme.select(|theme| theme.markdown),
        theme.select(|theme| theme.syntax.clone()),
        move |action| match action {
            MarkdownAction::CopyCode(code) => window.copy_text(code),
            MarkdownAction::OpenLink(url) => platform.with(|platform| platform.open(&url)),
        },
    );
    view! {
        <div flex col gap=12.0 min_w=0.0>
            <each items={move || blocks.get().into_iter().enumerate().collect::<Vec<(usize, MarkdownBlock)>>()}
                  key={|(index, _): &(usize, MarkdownBlock)| *index} let:block>
                <dynamic>{block.with(|(_, block)| markdown(std::slice::from_ref(block), paint))}</dynamic>
            </each>
        </div>
    }
}
