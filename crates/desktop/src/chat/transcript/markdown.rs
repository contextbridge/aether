use crate::platform::Platform;
use crate::ui::ActiveTheme;
use clankerdiff_markdown::{MarkdownBlock, MarkdownStream};
use clankerdiff_rouge::{MarkdownOutput, markdown_blocks};
use clankerdiff_rouge_shell::rouge::core::{Styled, View, WindowHandle, div, dynamic, each, view};
use clankerdiff_rouge_shell::rouge::reactive::{Callback, Memo, Stored, memo, stored};
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
    let theme = ActiveTheme::current().review_signal();
    let emit = Callback::from(move |output: MarkdownOutput| match output {
        MarkdownOutput::CopyCode(code) => window.copy_text(code),
        MarkdownOutput::OpenLink(url) => platform.with(|platform| platform.open(&url)),
        MarkdownOutput::Event(_) | MarkdownOutput::ThemeChanged(_) => {}
    });
    view! {
        <div flex col gap=12.0 min_w=0.0>
            <each items={move || blocks.get().into_iter().enumerate().collect::<Vec<(usize, MarkdownBlock)>>()}
                  key={|(index, _): &(usize, MarkdownBlock)| *index} let:block>
                <dynamic>{block.with(|(_, block)| markdown_blocks(std::slice::from_ref(block), theme, true, emit))}</dynamic>
            </each>
        </div>
    }
}
