use tui::{
    BorderedTextField, Combobox, Cursor, Frame, Line, Searchable, Style, ViewContext, display_width_text,
    pad_text_to_width, truncate_text,
};

pub(crate) const SEARCH_BOX_MAX_WIDTH: usize = 56;
pub(crate) const SEARCH_BOX_INDENT: u16 = 2;

pub(crate) fn render_two_column_items<T: Searchable + Send + Sync + 'static>(
    combobox: &Combobox<T>,
    context: &ViewContext,
    max_label_width: usize,
    label_fn: impl Fn(&T) -> String,
    meta_fn: impl Fn(&T) -> String,
) -> Vec<Line> {
    let max_label_width = combobox
        .matches()
        .iter()
        .map(|entry| display_width_text(&label_fn(entry)))
        .max()
        .unwrap_or(0)
        .min(max_label_width);

    combobox.render_items(context, |entry, is_selected, ctx| {
        let full_label = label_fn(entry);
        let label = truncate_text(&full_label, max_label_width);
        let padded = pad_text_to_width(&label, max_label_width);
        let meta = meta_fn(entry);
        let line_text = if meta.is_empty() { padded.to_string() } else { format!("{padded}  {meta}") };
        let truncated = truncate_text(&line_text, ctx.size.width as usize);

        if is_selected {
            ctx.theme.selected_row_line(truncated)
        } else {
            let boundary = truncated.floor_char_boundary(padded.len().min(truncated.len()));
            let mut line = Line::new(&truncated[..boundary]);
            if truncated.len() > boundary {
                line.push_with_style(&truncated[boundary..], Style::fg(ctx.theme.muted()));
            }
            line
        }
    })
}

pub(crate) fn boxed_search_field(label: &str, value: &str, placeholder: &str, context: &ViewContext) -> Frame {
    let width = (context.size.width as usize).saturating_sub(usize::from(SEARCH_BOX_INDENT)).min(SEARCH_BOX_MAX_WIDTH);
    let input_width = width.saturating_sub(4);
    let visible_value = truncate_text(value, input_width);

    let mut field = BorderedTextField::new(label, value.to_string()).placeholder(placeholder);
    field.set_width(width);

    Frame::new(field.render_field(context, false))
        .with_cursor(Cursor::visible(1, 2 + display_width_text(&visible_value)))
        .indent(SEARCH_BOX_INDENT)
}
