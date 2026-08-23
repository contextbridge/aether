use agent_client_protocol::schema::v1::{
    ElicitationContentValue, ElicitationPropertySchema, ElicitationSchema, EnumOption, MultiSelectItems,
    MultiSelectPropertySchema, StringPropertySchema,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use std::collections::BTreeMap;
use unicode_width::UnicodeWidthStr;

use crate::surfaces::input::is_composed_char;
use crate::theme::Theme;
use crate::view::edit_buffer::{EditBuffer, apply_edit_key};
use crate::view::selection::{Direction, scroll_into_view, step_clamped};
use crate::view::widgets::{KeyHint, RowsView, render_vertical_scrollbar, rows_and_track, visible_window};
use crate::view::wrap::{as_u16, fit_line, wrap_line};

/// One question per page, with a tab strip tracking progress and a review
/// page at the end. Forms of a single field skip the chrome: the question and
/// its control stand alone and `Enter` submits.
pub(super) struct FormModal {
    server_name: String,
    message: String,
    fields: Vec<FormField>,
    /// The question on screen; [`Self::review`] is the summary page.
    page: usize,
    confirming_cancel: bool,
    validation_error: Option<String>,
    body: BodyView,
}

/// The scrolling middle of the modal, where the page is drawn.
///
/// A frame records what it laid out here, so a click lands on the control the
/// user actually pointed at and the next frame scrolls against rows that
/// really exist — neither has to recompute how tall a row was drawn.
#[derive(Default)]
struct BodyView {
    /// First body row on screen.
    scroll: usize,
    /// Where the rows were drawn, for hit-testing a click.
    area: Rect,
    /// What each rendered row is: only option and review rows answer a click,
    /// everything else is padding, headings, descriptions, the tab strip, and
    /// input borders.
    rows: Vec<RowSite>,
    /// Where each tab cell was drawn and the page it picks, so a click can
    /// jump between questions.
    strip_cells: Vec<Rect>,
    strip_pages: Vec<usize>,
}

#[derive(Clone, Copy)]
enum RowSite {
    Inert,
    Option { option: usize },
    Review { field: usize },
}

pub(super) struct FormField {
    name: String,
    label: String,
    description: Option<String>,
    required: bool,
    /// Whether the user answered, so tab state, the progress counter, and the
    /// discard guard can tell an answer from an untouched default.
    touched: bool,
    kind: FormFieldKind,
}

pub(super) enum FormFieldKind {
    Text(EditBuffer),
    Integer(EditBuffer),
    Number(EditBuffer),
    Boolean(bool),
    Single { options: Vec<SelectOption>, selected: usize },
    Multi { options: Vec<SelectOption>, selected: Vec<bool>, cursor: usize },
}

/// A field's display label and its optional description.
type Labelling = (String, Option<String>);

pub(super) struct SelectOption {
    value: String,
    title: String,
}

pub(super) enum FormAction {
    None,
    Cancel,
    Accept(BTreeMap<String, ElicitationContentValue>),
}

/// The rows of one page, with everything a click or the terminal cursor needs
/// to know about where they landed.
#[derive(Default)]
struct PageRows {
    lines: Vec<Line<'static>>,
    sites: Vec<RowSite>,
    /// Row index of the tab strip, with each cell's column and page.
    strip: Option<(usize, Vec<TabCell>)>,
    /// Row index of the input's text row, with the cursor column inside it.
    input: Option<(usize, usize)>,
    /// The row to keep in view while scrolling.
    focus: Option<usize>,
}

#[derive(Clone, Copy)]
struct TabCell {
    page: usize,
    column: u16,
    width: u16,
}

impl PageRows {
    fn row(&mut self, line: Line<'static>) {
        self.site(line, RowSite::Inert);
    }

    fn site(&mut self, line: Line<'static>, site: RowSite) {
        self.lines.push(line);
        self.sites.push(site);
    }

    fn blank(&mut self) {
        self.row(Line::raw(""));
    }
}

impl FormModal {
    pub(super) fn new(server_name: String, message: String, schema: &ElicitationSchema) -> Option<Self> {
        let required: Vec<&str> = schema.required.as_deref().unwrap_or(&[]).iter().map(String::as_str).collect();
        let fields = schema
            .properties
            .iter()
            .map(|(name, prop)| FormField::from_schema(name, prop, &required))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            server_name,
            message,
            fields,
            page: 0,
            confirming_cancel: false,
            validation_error: None,
            body: BodyView::default(),
        })
    }

    pub(super) fn server_name(&self) -> &str {
        &self.server_name
    }

    /// The summary page, which only a multi-question form has.
    fn review(&self) -> usize {
        self.fields.len()
    }

    fn wizard(&self) -> bool {
        self.fields.len() > 1
    }

    fn on_review(&self) -> bool {
        self.wizard() && self.page == self.review()
    }

    fn last_page(&self) -> usize {
        if self.wizard() { self.review() } else { 0 }
    }

    fn choice_page(&self) -> bool {
        self.fields.get(self.page).is_some_and(|field| {
            matches!(field.kind, FormFieldKind::Boolean(_) | FormFieldKind::Single { .. } | FormFieldKind::Multi { .. })
        })
    }

    fn multi_page(&self) -> bool {
        self.fields.get(self.page).is_some_and(|field| matches!(field.kind, FormFieldKind::Multi { .. }))
    }

    fn focused_kind(&mut self) -> Option<&mut FormFieldKind> {
        self.fields.get_mut(self.page).map(|field| &mut field.kind)
    }

    /// Marks the focused field answered and retires any stale complaint
    /// about it.
    fn touch(&mut self) {
        if let Some(field) = self.fields.get_mut(self.page) {
            field.touched = true;
        }
        self.validation_error = None;
    }

    fn touched_count(&self) -> usize {
        self.fields.iter().filter(|field| field.touched).count()
    }

    fn text_buffer(&mut self) -> Option<&mut EditBuffer> {
        match self.focused_kind()? {
            FormFieldKind::Text(buffer) | FormFieldKind::Integer(buffer) | FormFieldKind::Number(buffer) => Some(buffer),
            _ => None,
        }
    }
}

impl FormModal {
    pub(super) fn on_key(&mut self, key: KeyEvent) -> FormAction {
        // The guard swallows whatever answers it: `y`/`Esc` discard, anything
        // else goes back to the form without side effects.
        if self.confirming_cancel {
            self.confirming_cancel = false;
            return match key.code {
                KeyCode::Esc | KeyCode::Char('y' | 'Y') => FormAction::Cancel,
                _ => FormAction::None,
            };
        }
        match key.code {
            KeyCode::Esc => self.escape(),
            KeyCode::Enter => self.advance(),
            KeyCode::Tab => {
                self.move_page(Direction::Forward);
                FormAction::None
            }
            KeyCode::BackTab => {
                self.move_page(Direction::Backward);
                FormAction::None
            }
            KeyCode::Up | KeyCode::Left => self.vertical(Direction::Backward),
            KeyCode::Down | KeyCode::Right => self.vertical(Direction::Forward),
            KeyCode::Char(' ') if !is_composed_char(key) => self.space(),
            KeyCode::Char('a' | 'A') if !is_composed_char(key) && self.multi_page() => self.toggle_all(),
            KeyCode::Char(digit @ '1'..='9') if !is_composed_char(key) && self.choice_page() => self.pick_digit(digit),
            _ => self.type_key(key),
        }
    }

    fn escape(&mut self) -> FormAction {
        // Losing several answered questions to a stray Escape deserves one
        // confirmation; a form barely started goes immediately.
        if self.touched_count() > 1 {
            self.confirming_cancel = true;
            FormAction::None
        } else {
            FormAction::Cancel
        }
    }

    fn advance(&mut self) -> FormAction {
        if self.wizard() && self.page < self.review() {
            self.move_page(Direction::Forward);
            return FormAction::None;
        }
        self.submit()
    }

    fn move_page(&mut self, direction: Direction) {
        self.goto_page(step_clamped(self.page, direction, 1, self.last_page()));
    }

    fn goto_page(&mut self, page: usize) {
        self.page = page;
        self.validation_error = None;
        self.body.scroll = 0;
    }

    /// Vertical motion — an arrow key or a mouse wheel — belongs to the
    /// control in focus: it answers on a choice page and walks questions
    /// everywhere else.
    pub(super) fn vertical(&mut self, direction: Direction) -> FormAction {
        if self.choice_page() {
            let answered = self.focused_kind().is_some_and(|kind| step_option(kind, direction));
            if answered {
                self.touch();
            }
        } else {
            self.move_page(direction);
        }
        FormAction::None
    }

    fn space(&mut self) -> FormAction {
        let toggled = self.focused_kind().is_some_and(toggle_at_cursor);
        if toggled {
            self.touch();
        }
        FormAction::None
    }

    fn toggle_all(&mut self) -> FormAction {
        let changed = self.focused_kind().is_some_and(select_all);
        if changed {
            self.touch();
        }
        FormAction::None
    }

    fn pick_digit(&mut self, digit: char) -> FormAction {
        let index = (digit.to_digit(10).unwrap_or(1) - 1) as usize;
        let answered = self.focused_kind().is_some_and(|kind| set_option(kind, index));
        if answered {
            self.touch();
        }
        FormAction::None
    }

    fn type_key(&mut self, key: KeyEvent) -> FormAction {
        let edited = self.text_buffer().is_some_and(|buffer| {
            let before = buffer.text().to_string();
            apply_edit_key(buffer, key);
            buffer.text() != before
        });
        if edited {
            self.touch();
        }
        FormAction::None
    }

    pub(super) fn paste(&mut self, text: &str) {
        if let Some(buffer) = self.text_buffer()
            && !text.is_empty()
        {
            buffer.insert_paste(text);
            self.touch();
        }
    }

    /// Focuses whatever the pointer lands on: a tab cell picks its page, an
    /// option row answers, a review row reopens its question.
    pub(super) fn click(&mut self, column: u16, row: u16) {
        if let Some(page) = self
            .body
            .strip_cells
            .iter()
            .zip(self.body.strip_pages.iter())
            .find_map(|(cell, &page)| cell.contains(Position::new(column, row)).then_some(page))
        {
            self.goto_page(page);
            return;
        }
        let Some(site) = self.site_at(row) else { return };
        match site {
            RowSite::Option { option } => {
                let answered = self.focused_kind().is_some_and(|kind| set_option(kind, option));
                if answered {
                    self.touch();
                }
            }
            RowSite::Review { field } => self.goto_page(field),
            RowSite::Inert => {}
        }
    }

    fn site_at(&self, row: u16) -> Option<RowSite> {
        let area = self.body.area;
        if row < area.y || row >= area.bottom() {
            return None;
        }
        self.body.rows.get(self.body.scroll + usize::from(row - area.y)).copied()
    }

    fn submit(&mut self) -> FormAction {
        let mut content = BTreeMap::new();
        let mut first_error = None;
        for (index, field) in self.fields.iter().enumerate() {
            match field.value() {
                Ok(Some(value)) => {
                    content.insert(field.name.clone(), value);
                }
                Ok(None) => {}
                Err(error) => {
                    first_error.get_or_insert((index, error));
                }
            }
        }
        match first_error {
            None => FormAction::Accept(content),
            Some((index, error)) => {
                // Send the user to the first thing standing between them and
                // submitting, with the complaint right beside it.
                self.goto_page(index);
                self.validation_error = Some(error);
                FormAction::None
            }
        }
    }

    /// The keys this page answers, for whichever footer is drawing them.
    pub(super) fn hints(&self) -> Vec<KeyHint> {
        if self.confirming_cancel {
            return vec![("y", "discard".into()), ("n", "keep".into())];
        }
        let enter = if self.on_review() || !self.wizard() {
            "submit"
        } else if self.page + 1 == self.review() {
            "review"
        } else {
            "next"
        };
        let mut hints = Vec::new();
        if self.choice_page() {
            hints.push(("↑↓", if self.multi_page() { "move" } else { "choose" }.into()));
            if self.multi_page() {
                hints.push(("Space", "toggle".into()));
            } else if self.wizard() {
                hints.push(("1-9", "pick".into()));
            }
        }
        if self.wizard() && !self.on_review() {
            hints.push(("Tab", "next".into()));
        }
        hints.push(("Enter", enter.into()));
        hints.push(("Esc", "cancel".into()));
        hints
    }
}

impl FormModal {
    /// The rows the current page wants at `width`, for drawing and for
    /// measuring the modal around them.
    fn page_rows(&self, theme: &Theme, width: u16) -> PageRows {
        let mut rows = PageRows::default();

        if !self.message.is_empty() {
            let headline =
                Line::styled(self.message.clone(), Style::new().fg(theme.heading).add_modifier(Modifier::BOLD));
            for line in wrap_line(headline, width) {
                rows.row(line);
            }
        }

        if self.wizard() {
            rows.blank();
            let strip = self.strip_line(theme, width);
            rows.row(strip.line);
            rows.strip = Some((rows.lines.len() - 1, strip.cells));
            rows.blank();
        }

        if self.on_review() {
            self.review_rows(theme, width, &mut rows);
        } else if let Some(field) = self.fields.get(self.page) {
            question_rows(field, theme, width, &mut rows);
        }

        if let Some(error) = &self.validation_error {
            rows.blank();
            for line in wrap_line(Line::styled(error.clone(), Style::new().fg(theme.error)), width) {
                rows.row(line);
            }
        }

        if self.confirming_cancel {
            rows.blank();
            let question =
                Span::styled(format!("Discard {} answers?", self.touched_count()), Style::new().fg(theme.warning));
            let how = Span::styled("  y discard · any other key keep", Style::new().fg(theme.muted));
            for line in wrap_line(Line::from(vec![question, how]), width) {
                rows.row(line);
            }
        }

        rows.blank();
        rows
    }

    fn review_rows(&self, theme: &Theme, width: u16, rows: &mut PageRows) {
        rows.blank();
        let headline =
            Line::styled("Review your answers", Style::new().fg(theme.text_primary).add_modifier(Modifier::BOLD));
        for line in wrap_line(headline, width) {
            rows.row(line);
        }
        rows.blank();

        let label_width =
            self.fields.iter().map(|field| field.label.width()).max().unwrap_or(0).min(usize::from(width) * 2 / 5);
        for (index, field) in self.fields.iter().enumerate() {
            let value = field.display_value();
            let value = if value.is_empty() { "—".to_string() } else { value };
            let style =
                if field.value().is_err() { Style::new().fg(theme.error) } else { Style::new().fg(theme.text_primary) };
            let prefix =
                vec![Span::styled(format!("{:<label_width$}  ", field.label), Style::new().fg(theme.text_secondary))];
            for line in wrapped_row(&prefix, &value, style, width) {
                rows.site(line, RowSite::Review { field: index });
            }
        }
    }

    /// The tab strip: every page a same-width cell on one grid, the current
    /// page filled with the accent, answered pages brighter than untouched
    /// ones, and the progress counter right-aligned in whatever is left.
    fn strip_line(&self, theme: &Theme, width: u16) -> Strip {
        const TITLED_CELL_LIMIT: usize = 6;
        let pages = self.fields.len();
        let titled = pages <= TITLED_CELL_LIMIT;
        let counter = format!("{} / {}", self.touched_count(), pages);
        let counter_width = counter.width();

        let attempt = |numbered: bool| {
            (0..=pages)
                .map(|page| {
                    if page == pages {
                        "✓".to_string()
                    } else if numbered {
                        (page + 1).to_string()
                    } else {
                        self.fields[page].label.clone()
                    }
                })
                .collect::<Vec<_>>()
        };

        let available = usize::from(width);
        let show_counter = counter_width + 1 < available;
        let budget = if show_counter { available - counter_width - 1 } else { available };

        let mut chosen = attempt(!titled);
        if strip_width(&chosen) > budget {
            chosen = attempt(true);
        }

        let cell = cell_width(&chosen);
        let full_width = strip_width(&chosen);
        let shown = if full_width <= budget {
            0..pages + 1
        } else {
            // Keep the current page in view with a neighbour either side when
            // the budget allows, marking both cut ends with an ellipsis.
            let ellipses = 4;
            let capacity = (budget.saturating_sub(ellipses) + 1) / (cell + 1);
            let capacity = capacity.clamp(1, pages + 1);
            let start = self.page.saturating_sub(capacity / 2).min(pages + 1 - capacity);
            start..start + capacity
        };

        let mut spans = Vec::new();
        let mut cells = Vec::new();
        let mut column: usize = 0;
        let dots = Style::new().fg(theme.muted);

        let left_cut = shown.start > 0;
        if left_cut {
            spans.push(Span::styled("…", dots));
            spans.push(Span::raw(" "));
            column += 2;
        }
        for page in shown.clone() {
            if page != shown.start {
                spans.push(Span::raw(" "));
                column += 1;
            }
            let style = if page == self.page {
                Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD)
            } else if page < pages && self.fields[page].value().is_err() {
                Style::new().fg(theme.error)
            } else if page < pages && self.fields[page].touched {
                Style::new().fg(theme.text_secondary)
            } else {
                Style::new().fg(theme.muted)
            };
            let label_width = cell - 2;
            spans.push(Span::styled(format!(" {:^label_width$} ", chosen[page]), style));
            cells.push(TabCell { page, column: as_u16(column), width: as_u16(cell) });
            column += cell;
        }
        let right_cut = shown.end <= pages;
        if right_cut {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("…", dots));
            column += 2;
        }
        if show_counter {
            let pad = available.saturating_sub(column + counter_width + 1);
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(counter, Style::new().fg(theme.muted)));
        }

        Strip { line: Line::from(spans), cells }
    }

    /// The rows the current page wants, so the modal can be sized to its
    /// content before anything is drawn.
    pub(super) fn content_height(&self, theme: &Theme, width: u16) -> usize {
        self.page_rows(theme, width).lines.len()
    }

    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) -> Option<Position> {
        let (rows_area, track_area) = rows_and_track(area, true);
        let rows_area =
            Rect { x: rows_area.x.saturating_add(1), width: rows_area.width.saturating_sub(1), ..rows_area };

        let page = self.page_rows(theme, rows_area.width);
        let height = usize::from(rows_area.height);
        let scroll = page
            .focus
            .map_or(self.body.scroll, |focus| scroll_into_view(self.body.scroll, focus, height))
            .min(page.lines.len().saturating_sub(height));

        self.body.area = rows_area;
        self.body.rows = page.sites;
        self.body.scroll = scroll;
        self.body.strip_cells = Vec::new();
        self.body.strip_pages = Vec::new();
        if let Some((row, cells)) = page.strip
            && let Some(y) = row.checked_sub(scroll).filter(|y| *y < height)
        {
            for cell in cells {
                self.body.strip_cells.push(Rect::new(
                    rows_area.x.saturating_add(cell.column),
                    rows_area.y + as_u16(y),
                    cell.width,
                    1,
                ));
                self.body.strip_pages.push(cell.page);
            }
        }

        RowsView::from_lines(page.lines.iter().skip(scroll).cloned()).render(rows_area, buf);
        if page.lines.len() > height {
            render_vertical_scrollbar(track_area, buf, page.lines.len(), scroll);
        }

        let (row, column) = page.input?;
        let y = row.checked_sub(scroll).filter(|y| *y < height)?;
        Some(Position::new(rows_area.x.saturating_add(as_u16(column)), rows_area.y + as_u16(y)))
    }
}

struct Strip {
    line: Line<'static>,
    cells: Vec<TabCell>,
}

/// Total width of every cell plus the gutters between them.
fn strip_width(labels: &[String]) -> usize {
    labels.len().saturating_sub(1) + labels.len() * cell_width(labels)
}

/// Every cell is as wide as the widest label, padded one column either side.
fn cell_width(labels: &[String]) -> usize {
    labels.iter().map(|label| label.width()).max().unwrap_or(0) + 2
}

/// Steps a choice control's cursor; reports whether the field's answer moved
/// with it, since a multi-select's cursor roams without answering.
fn step_option(kind: &mut FormFieldKind, direction: Direction) -> bool {
    match kind {
        FormFieldKind::Boolean(value) => {
            *value = !*value;
            true
        }
        FormFieldKind::Single { options, selected } if !options.is_empty() => {
            *selected = step_clamped(*selected, direction, 1, options.len() - 1);
            true
        }
        FormFieldKind::Multi { options, cursor, .. } if !options.is_empty() => {
            *cursor = step_clamped(*cursor, direction, 1, options.len() - 1);
            false
        }
        _ => false,
    }
}

/// Picks `index` outright: single answers jump to it, multi-selects land
/// their cursor on it and toggle it, so a number key always does what it
/// shows.
fn set_option(kind: &mut FormFieldKind, index: usize) -> bool {
    match kind {
        FormFieldKind::Boolean(value) => {
            let picked = index == 0;
            let changed = picked != *value;
            *value = picked;
            changed
        }
        FormFieldKind::Single { options, selected } if index < options.len() => {
            let changed = *selected != index;
            *selected = index;
            changed
        }
        FormFieldKind::Multi { options, selected, cursor } if index < options.len() => {
            *cursor = index;
            selected[index] = !selected[index];
            true
        }
        _ => false,
    }
}

fn toggle_at_cursor(kind: &mut FormFieldKind) -> bool {
    if let FormFieldKind::Multi { selected, cursor, .. } = kind
        && let Some(flag) = selected.get_mut(*cursor)
    {
        *flag = !*flag;
        true
    } else {
        false
    }
}

fn select_all(kind: &mut FormFieldKind) -> bool {
    if let FormFieldKind::Multi { options, selected, .. } = kind {
        let all = !options.is_empty() && selected.iter().all(|flag| *flag);
        for flag in selected.iter_mut() {
            *flag = !all;
        }
        true
    } else {
        false
    }
}

/// The question on screen: its label (marked when required), its description,
/// and the control that answers it.
fn question_rows(field: &FormField, theme: &Theme, width: u16, rows: &mut PageRows) {
    rows.blank();
    let mut headline =
        Line::styled(field.label.clone(), Style::new().fg(theme.text_primary).add_modifier(Modifier::BOLD));
    if field.required {
        headline.spans.push(Span::styled(" *", Style::new().fg(theme.error)));
    }
    for line in wrap_line(headline, width) {
        rows.row(line);
    }
    if let Some(description) = &field.description {
        for line in wrap_line(Line::styled(description.clone(), Style::new().fg(theme.muted)), width) {
            rows.row(line);
        }
    }

    match &field.kind {
        FormFieldKind::Multi { options, selected, cursor } => {
            option_rows(options, *cursor, Some(selected), theme, width, rows);
        }
        FormFieldKind::Single { options, selected } => option_rows(options, *selected, None, theme, width, rows),
        FormFieldKind::Boolean(value) => {
            option_rows(&boolean_options(), boolean_index(*value), None, theme, width, rows);
        }
        FormFieldKind::Text(buffer) | FormFieldKind::Integer(buffer) | FormFieldKind::Number(buffer) => {
            input_rows(buffer, theme, width, rows);
        }
    }
}

/// The option rows of a choice page: an ordinal column, the checkbox column
/// for multi-selects, and the cursor row filled with the accent across the
/// full grid width.
fn option_rows(
    options: &[SelectOption],
    cursor: usize,
    checked: Option<&[bool]>,
    theme: &Theme,
    width: u16,
    rows: &mut PageRows,
) {
    rows.blank();
    let number_width = options.len().max(1).to_string().len();
    for (index, option) in options.iter().enumerate() {
        let focused = index == cursor;
        if focused {
            rows.focus = rows.focus.or(Some(rows.lines.len()));
        }

        let tail = match checked {
            Some(flags) if flags.get(index).copied().unwrap_or(false) => format!("[x] {}", option.title),
            Some(_) => format!("[ ] {}", option.title),
            None => option.title.clone(),
        };

        let prefix = vec![Span::styled(format!("{:>number_width$}  ", index + 1), Style::new().fg(theme.muted))];
        for mut line in wrapped_row(&prefix, &tail, Style::new().fg(theme.text_primary), width) {
            if focused {
                let highlight = Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD);
                for span in &mut line.spans {
                    span.style = highlight;
                }
                line = fit_line(line, usize::from(width), highlight);
            }
            rows.site(line, RowSite::Option { option: index });
        }
    }
}

/// A text or number answer: an inset one-line input with a real terminal
/// cursor, sized to the page.
fn input_rows(buffer: &EditBuffer, theme: &Theme, width: u16, rows: &mut PageRows) {
    rows.blank();
    let border = Style::new().fg(theme.muted);
    let rule = "─".repeat(usize::from(width.saturating_sub(2)));
    rows.row(Line::from(vec![
        Span::styled("╭", border),
        Span::styled(rule.clone(), border),
        Span::styled("╮", border),
    ]));

    let content = usize::from(width.saturating_sub(4));
    let (visible, cursor_column) = visible_window(buffer, content.max(1));
    let text_style = Style::new().fg(theme.text_primary).bg(theme.code_bg);
    let padding = content.saturating_sub(visible.width());
    rows.row(Line::from(vec![
        Span::styled("│ ", border),
        Span::styled(visible, text_style),
        Span::styled(" ".repeat(padding), text_style),
        Span::styled(" │", border),
    ]));
    rows.input = Some((rows.lines.len() - 1, 2 + cursor_column));
    rows.focus = Some(rows.lines.len() - 1);
    rows.row(Line::from(vec![Span::styled("╰", border), Span::styled(rule, border), Span::styled("╯", border)]));
}

/// A row whose first columns are fixed (an ordinal, a checkbox, a label) and
/// whose tail wraps onto continuation rows aligned under where the tail
/// began, so every wrapped row still hits what it describes.
fn wrapped_row(prefix: &[Span<'static>], tail: &str, tail_style: Style, width: u16) -> Vec<Line<'static>> {
    let prefix_width: usize = prefix.iter().map(Span::width).sum();
    let tail_width = usize::from(width).saturating_sub(prefix_width).max(1);
    let indent = " ".repeat(prefix_width);
    wrap_line(Line::from(Span::styled(tail.to_string(), tail_style)), as_u16(tail_width))
        .into_iter()
        .enumerate()
        .map(|(index, mut tail_row)| {
            let mut spans = if index == 0 { prefix.to_vec() } else { vec![Span::raw(indent.clone())] };
            spans.append(&mut tail_row.spans);
            Line::from(spans)
        })
        .collect()
}

/// A boolean as the two options it really is.
fn boolean_options() -> Vec<SelectOption> {
    vec![
        SelectOption { value: "true".into(), title: "Yes".into() },
        SelectOption { value: "false".into(), title: "No".into() },
    ]
}

fn boolean_index(value: bool) -> usize {
    usize::from(!value)
}

impl FormField {
    fn from_schema(name: &str, prop: &ElicitationPropertySchema, required: &[&str]) -> Option<Self> {
        use FormFieldKind::{Boolean, Integer, Number};
        let (labelling, kind) = match prop {
            ElicitationPropertySchema::Boolean(value) => (
                labelling(value.title.as_deref(), value.description.as_deref(), name),
                Boolean(value.default.unwrap_or(false)),
            ),
            ElicitationPropertySchema::Integer(value) => (
                labelling(value.title.as_deref(), value.description.as_deref(), name),
                Integer(stringified_default(value.default).into()),
            ),
            ElicitationPropertySchema::Number(value) => (
                labelling(value.title.as_deref(), value.description.as_deref(), name),
                Number(stringified_default(value.default).into()),
            ),
            ElicitationPropertySchema::String(value) => {
                (labelling(value.title.as_deref(), value.description.as_deref(), name), string_kind(value))
            }
            ElicitationPropertySchema::Array(value) => {
                (labelling(value.title.as_deref(), value.description.as_deref(), name), multi_kind(value)?)
            }
            _ => return None,
        };
        let (label, description) = labelling;
        Some(Self {
            name: name.to_string(),
            label,
            description,
            required: required.contains(&name),
            touched: false,
            kind,
        })
    }

    fn value(&self) -> Result<Option<ElicitationContentValue>, String> {
        let missing = || format!("{} is required", self.label);
        match &self.kind {
            FormFieldKind::Text(value) => {
                if self.required && value.is_empty() {
                    Err(missing())
                } else {
                    Ok((!value.is_empty()).then(|| ElicitationContentValue::String(value.text().to_string())))
                }
            }
            FormFieldKind::Integer(value) => {
                if value.is_empty() {
                    return if self.required { Err(missing()) } else { Ok(None) };
                }
                let invalid = || format!("{} must be an integer", self.label);
                value
                    .text()
                    .parse::<i64>()
                    .map(ElicitationContentValue::Integer)
                    .map(Some)
                    .map_err(|_| invalid())
            }
            FormFieldKind::Number(value) => {
                if value.is_empty() {
                    return if self.required { Err(missing()) } else { Ok(None) };
                }
                let invalid = || format!("{} must be a number", self.label);
                value
                    .text()
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite())
                    .map(ElicitationContentValue::Number)
                    .map(Some)
                    .ok_or_else(invalid)
            }
            FormFieldKind::Boolean(value) => Ok(Some(ElicitationContentValue::Boolean(*value))),
            FormFieldKind::Single { options, selected } => {
                let value = options.get(*selected).map(|option| option.value.clone());
                if self.required && value.is_none() {
                    Err(missing())
                } else {
                    Ok(value.map(ElicitationContentValue::String))
                }
            }
            FormFieldKind::Multi { options, selected, .. } => Ok(Some(ElicitationContentValue::StringArray(
                options
                    .iter()
                    .zip(selected)
                    .filter(|(_, selected)| **selected)
                    .map(|(option, _)| option.value.clone())
                    .collect(),
            ))),
        }
    }

    fn display_value(&self) -> String {
        match &self.kind {
            FormFieldKind::Text(value) | FormFieldKind::Integer(value) | FormFieldKind::Number(value) => {
                value.text().to_string()
            }
            FormFieldKind::Boolean(value) => if *value { "Yes" } else { "No" }.to_string(),
            FormFieldKind::Single { options, selected } => {
                options.get(*selected).map(|o| o.title.clone()).unwrap_or_default()
            }
            FormFieldKind::Multi { options, selected, .. } => options
                .iter()
                .zip(selected)
                .filter(|(_, selected)| **selected)
                .map(|(opt, _)| opt.title.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// A field's display label and description: the schema's own title and
/// description, falling back to the property name.
fn labelling(title: Option<&str>, description: Option<&str>, name: &str) -> Labelling {
    (title.unwrap_or(name).to_string(), description.map(str::to_string))
}

fn string_kind(schema: &StringPropertySchema) -> FormFieldKind {
    if let Some(options) = &schema.one_of {
        single(options_from_enum_options(options), schema.default.as_deref())
    } else if let Some(options) = &schema.enum_values {
        single(options_from_strings(options), schema.default.as_deref())
    } else {
        FormFieldKind::Text(schema.default.clone().unwrap_or_default().into())
    }
}

fn multi_kind(schema: &MultiSelectPropertySchema) -> Option<FormFieldKind> {
    let options = match &schema.items {
        MultiSelectItems::String(items) => options_from_strings(&items.values),
        MultiSelectItems::Titled(items) => options_from_enum_options(&items.options),
        _ => return None,
    };
    Some(multi(options, schema.default.as_deref()))
}

/// A single-select focused on `default`, or on the first option when the
/// default names an option that is not offered.
fn single(options: Vec<SelectOption>, default: Option<&str>) -> FormFieldKind {
    let selected = default.and_then(|value| options.iter().position(|o| o.value == value)).unwrap_or(0);
    FormFieldKind::Single { options, selected }
}

fn multi(options: Vec<SelectOption>, defaults: Option<&[String]>) -> FormFieldKind {
    let defaults = defaults.unwrap_or(&[]);
    let selected = options.iter().map(|o| defaults.contains(&o.value)).collect();
    FormFieldKind::Multi { options, selected, cursor: 0 }
}

fn options_from_strings(values: &[String]) -> Vec<SelectOption> {
    values.iter().map(|s| SelectOption { value: s.clone(), title: s.clone() }).collect()
}

fn options_from_enum_options(items: &[EnumOption]) -> Vec<SelectOption> {
    items.iter().map(|item| SelectOption { value: item.value.clone(), title: item.title.clone() }).collect()
}

fn stringified_default<T: std::fmt::Display>(default: Option<T>) -> String {
    default.map(|value| value.to_string()).unwrap_or_default()
}

#[cfg(test)]
pub(super) fn permission_like_schema() -> ElicitationSchema {
    ElicitationSchema::new().property(
        "decision",
        StringPropertySchema::new().enum_values(vec!["allow".into(), "deny".into()]).default_value("deny"),
        true,
    )
}

#[cfg(test)]
#[allow(clippy::absolute_paths, clippy::similar_names)]
mod tests {
    use super::*;
    use crate::testing::{buffer_text, row_containing};
    use agent_client_protocol::schema::v1::{BooleanPropertySchema, IntegerPropertySchema, NumberPropertySchema};
    use crossterm::event::KeyModifiers;
    use serde_json::Value;

    const WIDTH: u16 = 64;
    const HEIGHT: u16 = 24;

    fn test_modal(server_name: String, message: String, schema: &ElicitationSchema) -> FormModal {
        FormModal::new(server_name, message, schema).unwrap()
    }

    fn required(name: &str, property: impl Into<ElicitationPropertySchema>) -> ElicitationSchema {
        ElicitationSchema::new().property(name, property, true)
    }

    fn optional(name: &str, property: impl Into<ElicitationPropertySchema>) -> ElicitationSchema {
        ElicitationSchema::new().property(name, property, false)
    }

    fn multi_schema() -> ElicitationSchema {
        required("tags", MultiSelectPropertySchema::new(vec!["fast".into(), "reliable".into(), "cheap".into()]))
    }

    fn survey_schema() -> ElicitationSchema {
        ElicitationSchema::new()
            .property("team", StringPropertySchema::new().title("Team"), true)
            .property(
                "urgency",
                StringPropertySchema::new()
                    .one_of(vec![EnumOption::new("low", "Low"), EnumOption::new("high", "High")]),
                false,
            )
            .property("notify", BooleanPropertySchema::new().default_value(false), false)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn draw(form: &mut FormModal) -> String {
        let mut buffer = Buffer::empty(Rect::new(0, 0, WIDTH, HEIGHT));
        form.render(buffer.area, &mut buffer, &Theme::default());
        buffer_text(&buffer)
    }

    /// The middle of the first cell containing `needle`, for a click.
    fn cell_of(form: &mut FormModal, needle: &str) -> (u16, u16) {
        let mut buffer = Buffer::empty(Rect::new(0, 0, WIDTH, HEIGHT));
        form.render(buffer.area, &mut buffer, &Theme::default());
        for y in buffer.area.top()..buffer.area.bottom() {
            let row: String = (buffer.area.left()..buffer.area.right())
                .map(|x| buffer.cell((x, y)).map_or_else(|| " ".to_string(), |cell| cell.symbol().to_string()))
                .collect();
            if let Some(byte) = row.find(needle) {
                let column = row[..byte].width() + needle.width() / 2;
                return (as_u16(column), y);
            }
        }
        panic!("{needle:?} should be on screen");
    }

    fn submitted(form: &mut FormModal) -> Value {
        for _ in 0..16 {
            if let FormAction::Accept(content) = form.on_key(key(KeyCode::Enter)) {
                return serde_json::to_value(content).expect("elicitation content serializes");
            }
        }
        panic!("Enter never submitted the form");
    }

    #[test]
    fn parses_string_field_with_title_and_description() {
        let schema =
            required("name", StringPropertySchema::new().title("Your Name").description("Enter your full name"));
        let form = test_modal("test".into(), String::new(), &schema);
        assert_eq!(form.fields.len(), 1);
        assert_eq!(form.fields[0].label, "Your Name");
        assert_eq!(form.fields[0].description.as_deref(), Some("Enter your full name"));
        assert!(form.fields[0].required);
        assert!(matches!(form.fields[0].kind, FormFieldKind::Text(_)));
    }

    #[test]
    fn parses_boolean_field_with_default() {
        let schema = optional("approved", BooleanPropertySchema::new().default_value(true));
        let form = test_modal("test".into(), String::new(), &schema);
        assert_eq!(form.fields.len(), 1);
        assert!(matches!(form.fields[0].kind, FormFieldKind::Boolean(true)));
    }

    #[test]
    fn parses_integer_and_number_fields() {
        let schema = ElicitationSchema::new()
            .property("age", IntegerPropertySchema::new().minimum(0).maximum(150), true)
            .property("rating", NumberPropertySchema::new().minimum(0.0).maximum(5.0), true);
        let form = test_modal("test".into(), String::new(), &schema);
        assert_eq!(form.fields.len(), 2);
        assert!(matches!(form.fields[0].kind, FormFieldKind::Integer(_)));
        assert!(matches!(form.fields[1].kind, FormFieldKind::Number(_)));
    }

    #[test]
    fn integer_field_respects_default() {
        let schema = required("count", IntegerPropertySchema::new().minimum(0).maximum(100).default_value(42));
        let form = test_modal("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Integer(value) => assert_eq!(value, "42"),
            _ => panic!("expected Integer"),
        }
    }

    #[test]
    fn number_field_respects_default() {
        let schema = required("score", NumberPropertySchema::new().minimum(0.0).maximum(100.0).default_value(2.5));
        let form = test_modal("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Number(value) => {
                let parsed: f64 = value.parse().unwrap();
                assert!((parsed - 2.5).abs() < 0.001, "expected ~2.5, got {}", value.text());
            }
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn string_field_respects_default() {
        let schema = required("greeting", StringPropertySchema::new().default_value("hello"));
        let form = test_modal("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Text(value) => assert_eq!(value, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn parses_single_select_enum_from_schema() {
        let schema = required(
            "color",
            StringPropertySchema::new().enum_values(vec!["red".into(), "green".into(), "blue".into()]),
        );
        let form = test_modal("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Single { options, selected } => {
                assert_eq!(options.len(), 3);
                assert_eq!(options[0].value, "red");
                assert_eq!(*selected, 0);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn parses_single_select_with_default() {
        let schema = required(
            "color",
            StringPropertySchema::new().enum_values(vec!["red".into(), "green".into()]).default_value("green"),
        );
        let form = test_modal("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Single { options, selected } => {
                assert_eq!(*selected, 1);
                assert_eq!(options[1].value, "green");
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn parses_titled_single_select_with_const_titles() {
        let schema = required(
            "size",
            StringPropertySchema::new().one_of(vec![
                EnumOption::new("s", "Small"),
                EnumOption::new("m", "Medium"),
                EnumOption::new("l", "Large"),
            ]),
        );
        let form = test_modal("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Single { options, .. } => {
                assert_eq!(options.len(), 3);
                assert_eq!(options[0].title, "Small");
                assert_eq!(options[0].value, "s");
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn parses_multi_select_enum() {
        let schema = multi_schema();
        let form = test_modal("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Multi { options, selected, cursor } => {
                assert_eq!(options.len(), 3);
                assert_eq!(selected.len(), 3);
                assert_eq!(*cursor, 0);
                assert!(selected.iter().all(|s| !*s));
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn parses_multi_select_with_defaults() {
        let schema = required(
            "tags",
            MultiSelectPropertySchema::new(vec!["fast".into(), "reliable".into(), "cheap".into()])
                .default_value(vec!["reliable".to_string()]),
        );
        let form = test_modal("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Multi { selected, .. } => {
                assert!(!selected[0]);
                assert!(selected[1]);
                assert!(!selected[2]);
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn empty_schema_produces_no_fields() {
        let schema = ElicitationSchema::new();
        let form = test_modal("test".into(), String::new(), &schema);
        assert!(form.fields.is_empty());
    }

    #[test]
    fn text_field_default_submits_as_a_string() {
        let schema = optional("greeting", StringPropertySchema::new().default_value("hello"));
        let mut form = test_modal("test".into(), String::new(), &schema);
        assert_eq!(submitted(&mut form), serde_json::json!({"greeting": "hello"}));
    }

    #[test]
    fn number_field_default_submits_as_a_parsed_number() {
        let schema = optional("count", IntegerPropertySchema::new().minimum(0).maximum(100).default_value(42));
        let mut form = test_modal("test".into(), String::new(), &schema);
        assert_eq!(submitted(&mut form), serde_json::json!({"count": 42}));
    }

    #[test]
    fn boolean_and_single_select_defaults_submit_as_typed_values() {
        let schema = ElicitationSchema::new()
            .property("approved", BooleanPropertySchema::new().default_value(true), false)
            .property(
                "color",
                StringPropertySchema::new().enum_values(vec!["red".into(), "green".into()]).default_value("green"),
                false,
            );
        let mut form = test_modal("test".into(), String::new(), &schema);
        assert_eq!(submitted(&mut form), serde_json::json!({"approved": true, "color": "green"}));
    }

    #[test]
    fn multi_select_defaults_submit_as_the_selected_array() {
        let schema = required(
            "tags",
            MultiSelectPropertySchema::new(vec!["fast".into(), "reliable".into(), "cheap".into()])
                .default_value(vec!["reliable".to_string()]),
        );
        let mut form = test_modal("test".into(), String::new(), &schema);
        assert_eq!(submitted(&mut form), serde_json::json!({"tags": ["reliable"]}));
    }

    #[test]
    fn accept_produces_correct_json() {
        let schema = ElicitationSchema::new()
            .property("name", StringPropertySchema::new().title("Name"), false)
            .property("approved", BooleanPropertySchema::new().default_value(true), false)
            .property(
                "color",
                StringPropertySchema::new().enum_values(vec!["red".into(), "green".into()]).default_value("green"),
                false,
            );
        let mut form = test_modal("test".into(), "Test".into(), &schema);
        let value = submitted(&mut form);
        let object = value.as_object().unwrap();
        assert!(!object.contains_key("name"), "empty optional text should be omitted");
        assert_eq!(object["approved"], Value::Bool(true));
        assert_eq!(object["color"], Value::String("green".into()));
    }

    #[test]
    fn submit_rejects_required_field() {
        let schema = required("name", StringPropertySchema::new());
        let mut form = test_modal("test".into(), String::new(), &schema);
        assert!(matches!(form.on_key(key(KeyCode::Enter)), FormAction::None));
    }

    #[test]
    fn permission_like_form_submit_returns_default_deny() {
        let schema = permission_like_schema();
        let mut form = test_modal("coding".into(), "Allow bash: rm -rf /tmp?".into(), &schema);
        assert_eq!(submitted(&mut form)["decision"], "deny");
    }

    #[test]
    fn integer_field_rejects_fractional_input() {
        let schema = optional("count", IntegerPropertySchema::new());
        let mut form = test_modal("test".into(), String::new(), &schema);

        form.on_key(key(KeyCode::Char('1')));
        form.on_key(key(KeyCode::Char('.')));
        form.on_key(key(KeyCode::Char('5')));

        assert!(matches!(form.on_key(key(KeyCode::Enter)), FormAction::None));
    }

    #[test]
    fn number_field_rejects_non_numeric_input_and_preserves_number_type() {
        let schema = optional("score", NumberPropertySchema::new().minimum(0.0).maximum(5.0));
        let mut form = test_modal("test".into(), String::new(), &schema);

        form.on_key(key(KeyCode::Char('4')));
        form.on_key(key(KeyCode::Char('a')));
        assert!(matches!(form.on_key(key(KeyCode::Enter)), FormAction::None), "letters block submit");

        form.on_key(key(KeyCode::Backspace));
        assert_eq!(submitted(&mut form), serde_json::json!({"score": 4.0}));
    }

    #[test]
    fn single_field_form_submits_directly_on_enter() {
        let schema = permission_like_schema();
        let mut form = test_modal("coding".into(), "Allow bash?".into(), &schema);
        assert!(matches!(form.on_key(key(KeyCode::Enter)), FormAction::Accept(_)));
    }

    #[test]
    fn enter_walks_pages_and_submits_from_review() {
        let schema = survey_schema();
        // `properties` arrives as a sorted map, so the pages run notify, team,
        // urgency — alphabetical by name.
        let mut form = test_modal("survey".into(), "Help us route this".into(), &schema);

        form.on_key(key(KeyCode::Down));
        assert!(matches!(form.on_key(key(KeyCode::Enter)), FormAction::None), "the notify page advances");
        assert!(draw(&mut form).contains('╭'), "the team question has an input");
        for character in "team".chars() {
            form.on_key(key(KeyCode::Char(character)));
        }
        assert!(matches!(form.on_key(key(KeyCode::Enter)), FormAction::None), "the team page advances");
        assert!(draw(&mut form).contains("1  Low"), "the urgency question is a choice page");
        form.on_key(key(KeyCode::Char('2')));
        assert!(matches!(form.on_key(key(KeyCode::Enter)), FormAction::None), "the last question advances to review");
        assert!(draw(&mut form).contains("Review your answers"), "the review page");

        let FormAction::Accept(content) = form.on_key(key(KeyCode::Enter)) else {
            panic!("a complete form submits from review")
        };
        let value = serde_json::to_value(content).unwrap();
        assert_eq!(value["notify"], true);
        assert_eq!(value["team"], "team");
        assert_eq!(value["urgency"], "high");
    }

    #[test]
    fn tab_and_backtab_walk_pages_without_wraparound() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);

        form.on_key(key(KeyCode::BackTab));
        assert!(draw(&mut form).contains("1  Yes"), "BackTab stops at the first page");
        for _ in 0..3 {
            form.on_key(key(KeyCode::Tab));
        }
        assert!(draw(&mut form).contains("Review your answers"), "Tab reaches the review page");
        form.on_key(key(KeyCode::Tab));
        assert!(draw(&mut form).contains("Review your answers"), "Tab stops at the review page");
    }

    #[test]
    fn submit_failure_jumps_to_the_first_invalid_field() {
        let schema = ElicitationSchema::new().property("first", StringPropertySchema::new(), true).property(
            "second",
            StringPropertySchema::new(),
            true,
        );
        let mut form = test_modal("test".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Tab));
        assert!(matches!(form.on_key(key(KeyCode::Enter)), FormAction::None));
        let screen = draw(&mut form);
        assert!(screen.contains("first is required"), "the complaint sits beside the question: {screen}");
        assert!(screen.contains('╭'), "the first unanswered question is back on screen: {screen}");
    }

    #[test]
    fn arrows_answer_choice_pages_immediately() {
        let schema = ElicitationSchema::new()
            .property(
                "urgency",
                StringPropertySchema::new()
                    .one_of(vec![EnumOption::new("low", "Low"), EnumOption::new("high", "High")]),
                false,
            )
            .property("notify", BooleanPropertySchema::new().default_value(false), false);
        let mut form = test_modal("survey".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Tab));
        assert!(draw(&mut form).contains("1  Low"), "the urgency question is a choice page");

        form.on_key(key(KeyCode::Down));
        assert!(draw(&mut form).contains("1 / 2"), "answering marks the page answered");
        assert_eq!(submitted(&mut form)["urgency"], "high", "Down answers High");

        let mut form = test_modal("survey".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Down));
        form.on_key(key(KeyCode::Up));
        assert_eq!(submitted(&mut form)["urgency"], "low", "Up answers Low");
    }

    #[test]
    fn boolean_page_answers_through_its_two_options() {
        let schema = optional("notify", BooleanPropertySchema::new().default_value(false));
        let mut form = test_modal("test".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Down));
        assert_eq!(submitted(&mut form)["notify"], true, "Down selects Yes");
        form.on_key(key(KeyCode::Char('2')));
        assert_eq!(submitted(&mut form)["notify"], false, "2 selects No");
        form.on_key(key(KeyCode::Char('1')));
        assert_eq!(submitted(&mut form)["notify"], true, "1 selects Yes");
    }

    #[test]
    fn digit_keys_pick_single_and_toggle_multi_options() {
        let schema = ElicitationSchema::new()
            .property(
                "color",
                StringPropertySchema::new().enum_values(vec!["red".into(), "green".into(), "blue".into()]),
                false,
            )
            .property("tags", MultiSelectPropertySchema::new(vec!["fast".into(), "reliable".into()]), true);
        let mut form = test_modal("test".into(), String::new(), &schema);

        form.on_key(key(KeyCode::Char('3')));
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Char('2')));
        let value = submitted(&mut form);
        assert_eq!(value["color"], "blue");
        assert_eq!(value["tags"], serde_json::json!(["reliable"]), "2 toggles the second checkbox");
    }
    #[test]
    fn multi_select_cursor_moves_without_answering() {
        let schema = ElicitationSchema::new()
            .property(
                "tags",
                MultiSelectPropertySchema::new(vec!["fast".into(), "reliable".into(), "cheap".into()]),
                true,
            )
            .property("verify", BooleanPropertySchema::new().default_value(false), false);
        let mut form = test_modal("test".into(), String::new(), &schema);

        form.on_key(key(KeyCode::Down));
        assert!(draw(&mut form).contains("0 / 2"), "moving the cursor is not an answer");
        form.on_key(key(KeyCode::Char(' ')));
        assert!(draw(&mut form).contains("1 / 2"), "toggling is");
        form.on_key(key(KeyCode::Down));
        form.on_key(key(KeyCode::Char(' ')));
        assert_eq!(submitted(&mut form)["tags"], serde_json::json!(["reliable", "cheap"]));
    }

    #[test]
    fn multi_select_space_toggles_the_focused_option() {
        let schema = multi_schema();
        let mut form = test_modal("test".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Char(' ')));
        assert_eq!(submitted(&mut form)["tags"], serde_json::json!(["fast"]));
    }

    #[test]
    fn multi_select_up_saturates_at_zero() {
        let schema = multi_schema();
        let mut form = test_modal("test".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Up));
        form.on_key(key(KeyCode::Char(' ')));
        assert_eq!(submitted(&mut form)["tags"], serde_json::json!(["fast"]));
    }

    #[test]
    fn select_all_flips_every_checkbox() {
        let schema = multi_schema();
        let mut form = test_modal("test".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Char('a')));
        assert_eq!(submitted(&mut form)["tags"], serde_json::json!(["fast", "reliable", "cheap"]));
        form.on_key(key(KeyCode::Char('a')));
        assert_eq!(submitted(&mut form)["tags"], serde_json::json!([]), "select-all clears when everything is on");
    }

    #[test]
    fn digits_still_type_into_text_fields() {
        let schema = optional("name", StringPropertySchema::new());
        let mut form = test_modal("test".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Char('4')));
        form.on_key(key(KeyCode::Char('2')));
        assert_eq!(submitted(&mut form)["name"], "42");
    }

    #[test]
    fn esc_confirms_when_more_than_one_answer_would_be_lost() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Down));
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Char('t')));

        assert!(matches!(form.on_key(key(KeyCode::Esc)), FormAction::None), "the first Esc arms the guard");
        assert!(draw(&mut form).contains("Discard 2 answers?"));

        assert!(matches!(form.on_key(key(KeyCode::Char('n'))), FormAction::None), "n keeps the answers");
        let screen = draw(&mut form);
        assert!(!screen.contains("Discard"), "the guard is gone: {screen}");
        assert!(screen.contains('╭'), "the form stays on the team question: {screen}");

        form.on_key(key(KeyCode::Esc));
        assert!(matches!(form.on_key(key(KeyCode::Esc)), FormAction::Cancel), "Esc twice discards");
    }

    #[test]
    fn esc_cancels_immediately_when_barely_anything_was_answered() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Down));
        assert!(matches!(form.on_key(key(KeyCode::Esc)), FormAction::Cancel), "one answer needs no ceremony");
    }

    #[test]
    fn wizard_pages_show_a_tab_strip_and_progress_counter() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), "Help us route this".into(), &schema);

        let screen = draw(&mut form);
        assert!(screen.contains("notify"), "the first question is the page headline: {screen}");
        assert!(screen.contains("0 / 3"), "the progress counter starts at zero: {screen}");
        assert!(screen.contains("✓"), "the review tab is the last cell: {screen}");

        form.on_key(key(KeyCode::Down));
        form.on_key(key(KeyCode::Tab));
        let screen = draw(&mut form);
        assert!(screen.contains("1 / 3"), "answering advances the counter: {screen}");
        assert!(screen.contains("Team *"), "the required question marks itself: {screen}");
    }

    #[test]
    fn single_question_forms_skip_the_tab_strip() {
        let schema = permission_like_schema();
        let mut form = test_modal("coding".into(), "Allow bash: rm -rf /tmp?".into(), &schema);
        let screen = draw(&mut form);
        assert!(!screen.contains("/ 1"), "no progress counter for one question: {screen}");
        assert!(screen.contains("Allow bash: rm -rf /tmp?"));
        assert!(screen.contains("deny"), "the default answer is highlighted: {screen}");
    }

    #[test]
    fn choice_pages_list_every_option_with_its_ordinal() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Tab));
        let screen = draw(&mut form);
        assert!(screen.contains("1  Low"), "options are vertical with ordinals: {screen}");
        assert!(screen.contains("2  High"));
    }

    #[test]
    fn text_pages_draw_an_input_and_place_the_terminal_cursor() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Char('a')));
        form.on_key(key(KeyCode::Char('b')));

        let mut buffer = Buffer::empty(Rect::new(0, 0, WIDTH, HEIGHT));
        let cursor = form.render(buffer.area, &mut buffer, &Theme::default()).expect("text pages own the cursor");
        let screen = buffer_text(&buffer);
        assert!(screen.contains("│ ab"), "the input shows what was typed: {screen}");
        assert_eq!(cursor.y, row_containing(&buffer, "│ ab").unwrap(), "the cursor sits on the input row");
        assert_eq!(cursor.x, 5, "the cursor sits after the border, the space, and the typed text");
    }

    #[test]
    fn review_page_lists_every_answer_and_marks_missing_ones() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);
        for _ in 0..3 {
            form.on_key(key(KeyCode::Tab));
        }
        let screen = draw(&mut form);
        assert!(screen.contains("Review your answers"));
        assert!(screen.contains("Team"));
        assert!(screen.contains("—"), "the unanswered required question has no value");
    }

    #[test]
    fn clicking_an_option_answers_the_field_under_the_pointer() {
        let schema = ElicitationSchema::new()
            .property("alpha", BooleanPropertySchema::new().default_value(false), false)
            .property("bravo", BooleanPropertySchema::new().default_value(false), false);
        let mut form = test_modal("test".into(), String::new(), &schema);

        let (column, yes) = cell_of(&mut form, "1  Yes");
        form.click(column, yes);
        assert!(draw(&mut form).contains("1 / 2"), "clicking Yes answers alpha");

        form.on_key(key(KeyCode::Tab));
        let (column, no) = cell_of(&mut form, "2  No");
        form.click(column, no);
        assert!(draw(&mut form).contains("1 / 2"), "clicking the answer already in place changes nothing");
        assert_eq!(submitted(&mut form), serde_json::json!({ "alpha": true, "bravo": false }));
    }

    #[test]
    fn clicking_a_tab_or_a_review_row_jumps_to_its_page() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);

        let (column, row) = cell_of(&mut form, "✓");
        form.click(column, row);
        assert!(draw(&mut form).contains("Review your answers"), "the review tab jumps to the summary");

        let (column, row) = cell_of(&mut form, "urgency");
        form.click(column, row);
        assert!(draw(&mut form).contains("1  Low"), "a review row reopens its question");
    }

    #[test]
    fn long_forms_window_the_tab_strip_around_the_current_page() {
        let mut schema = ElicitationSchema::new();
        for index in 0..40 {
            schema = schema.property(format!("field_{index:02}"), BooleanPropertySchema::new(), false);
        }
        let mut form = test_modal("survey".into(), String::new(), &schema);

        for _ in 0..20 {
            form.on_key(key(KeyCode::Tab));
        }
        let screen = draw(&mut form);
        assert!(screen.contains('…'), "the strip marks its cut ends: {screen}");
        assert!(screen.contains("0 / 40"), "the counter counts answers, not pages: {screen}");
        assert!(screen.contains("field_20"), "the current question is on screen");

        let (column, row) = cell_of(&mut form, " 18 ");
        form.click(column, row);
        assert!(draw(&mut form).contains("field_17"), "the strip stays clickable while windowed");
    }

    #[test]
    fn scrolling_answers_on_choice_pages_and_walks_pages_elsewhere() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);

        form.vertical(Direction::Forward);
        assert!(draw(&mut form).contains("1  Yes"), "vertical motion on a choice page answers instead of paging");
        form.on_key(key(KeyCode::Tab));
        form.vertical(Direction::Forward);
        assert!(draw(&mut form).contains("1  Low"), "vertical motion on a text page walks questions");
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Tab));
        assert!(draw(&mut form).contains("Review your answers"), "the review page is reachable");
        form.vertical(Direction::Backward);
        assert!(draw(&mut form).contains("1  Low"), "vertical motion on the review page walks back through questions");
    }

    #[test]
    fn hints_follow_the_page_in_focus() {
        let schema = survey_schema();
        let mut form = test_modal("survey".into(), String::new(), &schema);

        let hints = |form: &FormModal| form.hints().iter().map(|(key, _)| *key).collect::<Vec<_>>();
        assert_eq!(hints(&form), vec!["↑↓", "1-9", "Tab", "Enter", "Esc"], "a choice page picks");
        form.on_key(key(KeyCode::Tab));
        assert_eq!(hints(&form), vec!["Tab", "Enter", "Esc"], "a text page types");
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Tab));
        assert_eq!(hints(&form), vec!["Enter", "Esc"], "the review page submits");
    }
}
