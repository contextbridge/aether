use tui::{BorderedTextField, Cursor, Frame, ViewContext, display_width_text, truncate_text};

const INPUT_BOX_MAX_WIDTH: usize = 56;
pub(crate) const INPUT_BOX_INDENT: u16 = 2;

pub(crate) fn input_box_width(context: &ViewContext) -> usize {
    (context.size.width as usize).saturating_sub(usize::from(INPUT_BOX_INDENT)).min(INPUT_BOX_MAX_WIDTH)
}

pub(crate) fn input_box_frame(label: &str, value: &str, placeholder: &str, context: &ViewContext) -> Frame {
    let width = input_box_width(context);
    let input_width = width.saturating_sub(4);
    let visible_value = truncate_text(value, input_width);

    let mut field = BorderedTextField::new(label, value.to_string()).placeholder(placeholder);
    field.set_width(width);

    Frame::new(field.render_field(context, false))
        .with_cursor(Cursor::visible(1, 2 + display_width_text(&visible_value)))
        .indent(INPUT_BOX_INDENT)
}
