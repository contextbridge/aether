use crate::session_config_view::SessionConfigView;
use crate::settings::{ContextUsageDisplay, ResolvedStatusLineSettings, StatusLineSegmentConfig, StatusLineStyle};
use crate::theme::Theme;
use crate::workspace_status::WorkspaceStatus;
use crate::wrap::{truncate_spans, truncate_to_width};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::SessionConfigOption;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use utils::ReasoningEffort;

/// Everything the status line reads out of `App`, borrowed for one frame.
///
/// Gathering it in one place is what keeps `App` from having to expose a getter
/// per segment.
pub struct StatusLineModel<'a> {
    pub settings: &'a ResolvedStatusLineSettings,
    pub config_options: &'a [SessionConfigOption],
    pub workspace: &'a WorkspaceStatus,
    pub agent_name: &'a str,
    pub content_padding: usize,
    pub context_usage: Option<ContextUsageDisplay>,
    pub unhealthy_servers: usize,
    pub waiting_for_response: bool,
    pub exit_confirmation: bool,
}

/// The bottom status bar: workspace on the left, session state on the right.
///
/// Built once per frame and rendered by reference, so measuring its height does
/// not mean rendering every segment a second time.
pub struct StatusLine {
    left: Line<'static>,
    right: Line<'static>,
}

impl StatusLine {
    pub fn new(model: &StatusLineModel<'_>, theme: &Theme) -> Self {
        let settings = model.settings;
        let mut left = render_segments(model, &settings.left, theme);
        left.insert(0, Span::raw(" ".repeat(model.content_padding)));
        let right = if model.exit_confirmation {
            vec![Span::styled("Ctrl-C again to exit", Style::new().fg(theme.warning))]
        } else {
            render_segments(model, &settings.right, theme)
        };
        // The alignment rides on the line itself, so rendering it is a borrow
        // rather than a copy into a `Paragraph`.
        Self { left: Line::from(left), right: Line::from(right).right_aligned() }
    }

    /// Rows this status line needs: one when both halves fit side by side, two
    /// when they must stack.
    pub fn height(&self, width: u16) -> u16 {
        if width == 0 {
            return 1;
        }
        if self.fits_on_one_row(usize::from(width)) { 1 } else { 2 }
    }

    /// Whether both halves fit on one row.
    fn fits_on_one_row(&self, width: usize) -> bool {
        self.right.width() == 0 || self.left.width() + 1 + self.right.width() <= width
    }
}

impl Widget for &StatusLine {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = usize::from(area.width);
        let clipped = |line: &Line<'static>| Line { spans: truncate_spans(&line.spans, width), ..line.clone() };

        if self.fits_on_one_row(width) {
            if self.right.width() == 0 {
                clipped(&self.left).render(area, buf);
            } else {
                (&self.left).render(area, buf);
                (&self.right).render(area, buf);
            }
            return;
        }

        // Too wide for one row: stack, or clip to the left half when there is
        // only one row to work with.
        let [left_row, right_row] = Layout::vertical([Constraint::Length(1); 2]).areas(area);
        clipped(&self.left).render(left_row, buf);
        clipped(&self.right).render(right_row, buf);
    }
}

/// Renders each configured segment, joining the non-empty ones with the
/// separator so no separator ever dangles beside a hidden segment.
fn render_segments(
    model: &StatusLineModel<'_>,
    segments: &[StatusLineSegmentConfig],
    theme: &Theme,
) -> Vec<Span<'static>> {
    let config = SessionConfigView::new(model.config_options);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for segment in segments {
        let segment_spans = render_segment(segment, model, &config, theme);
        if segment_spans.is_empty() {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::styled(model.settings.separator.clone(), Style::new().fg(theme.text_secondary)));
        }
        spans.extend(segment_spans);
    }
    spans
}

fn render_segment(
    segment: &StatusLineSegmentConfig,
    model: &StatusLineModel<'_>,
    config: &SessionConfigView<'_>,
    theme: &Theme,
) -> Vec<Span<'static>> {
    match segment {
        StatusLineSegmentConfig::Cwd { max_width } => {
            vec![styled(clamp(&model.workspace.display_dir, *max_width), theme.text_secondary)]
        }
        StatusLineSegmentConfig::GitRef => model
            .workspace
            .git_ref
            .as_deref()
            .map(|git_ref| vec![styled(git_ref.to_string(), theme.success)])
            .unwrap_or_default(),
        StatusLineSegmentConfig::Agent => vec![styled(model.agent_name.to_string(), theme.info)],
        StatusLineSegmentConfig::Mode => config
            .current_display_name(ConfigOptionId::Mode)
            .map(|mode| vec![styled(mode, theme.text_secondary)])
            .unwrap_or_default(),
        StatusLineSegmentConfig::Model { max_width } => config
            .current_display_name(ConfigOptionId::Model)
            .map(|model| vec![styled(clamp(&model, *max_width), theme.success)])
            .unwrap_or_default(),
        StatusLineSegmentConfig::Reasoning => {
            let levels = config.reasoning_levels();
            if levels.is_empty() {
                return Vec::new();
            }
            let effort = config.reasoning_effort();
            vec![styled(reasoning_bar(effort, &levels), reasoning_color(effort, &levels, theme))]
        }
        StatusLineSegmentConfig::Context => model
            .context_usage
            .map(|usage| vec![styled(context_bar(usage), context_color(usage, theme))])
            .unwrap_or_default(),
        StatusLineSegmentConfig::ServerHealth => {
            let count = model.unhealthy_servers;
            if model.waiting_for_response || count == 0 {
                return Vec::new();
            }
            let message =
                if count == 1 { "1 server needs auth".to_string() } else { format!("{count} servers unhealthy") };
            vec![styled(message, theme.warning)]
        }
        StatusLineSegmentConfig::Text { value, style } => {
            vec![styled(value.clone(), style.map_or(theme.text_secondary, |style| semantic_color(style, theme)))]
        }
    }
}

fn styled(text: String, color: Color) -> Span<'static> {
    Span::styled(text, Style::new().fg(color))
}

fn clamp(text: &str, max_width: Option<u16>) -> String {
    max_width.map_or_else(|| text.to_string(), |width| truncate_to_width(text, usize::from(width)))
}

fn semantic_color(style: StatusLineStyle, theme: &Theme) -> Color {
    match style {
        StatusLineStyle::Primary => theme.text_primary,
        StatusLineStyle::Secondary => theme.text_secondary,
        StatusLineStyle::Muted => theme.muted,
        StatusLineStyle::Info => theme.info,
        StatusLineStyle::Success => theme.success,
        StatusLineStyle::Warning => theme.warning,
        StatusLineStyle::Error => theme.error,
    }
}

/// A `[■■·]` gauge with `filled` of `total` slots lit.
fn slot_bar(filled: usize, total: usize) -> String {
    let slots: String = (0..total).map(|index| if index < filled { '■' } else { '·' }).collect();
    format!("[{slots}]")
}

fn context_bar(usage: ContextUsageDisplay) -> String {
    const TOTAL: u32 = 3;
    let filled = (usage.used_tokens.saturating_mul(TOTAL) + usage.limit_tokens / 2) / usage.limit_tokens.max(1);
    format!(
        "ctx {} {} / {}",
        slot_bar((filled as usize).min(TOTAL as usize), TOTAL as usize),
        format_tokens(usage.used_tokens),
        format_tokens(usage.limit_tokens)
    )
}

fn context_color(usage: ContextUsageDisplay, theme: &Theme) -> Color {
    let used_pct = usage.used_ratio() * 100.0;
    if used_pct >= 86.0 {
        theme.error
    } else if used_pct >= 71.0 {
        theme.warning
    } else {
        theme.text_secondary
    }
}

fn format_tokens(count: u32) -> String {
    match count {
        count if count < 1_000 => count.to_string(),
        count if count < 1_000_000 => format_with_unit(f64::from(count) / 1_000.0, "k"),
        count => format_with_unit(f64::from(count) / 1_000_000.0, "M"),
    }
}

fn format_with_unit(value: f64, unit: &str) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded - rounded.trunc()).abs() < f64::EPSILON {
        format!("{rounded:.0}{unit}")
    } else {
        format!("{rounded:.1}{unit}")
    }
}

fn reasoning_bar(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> String {
    let label = effort.map_or("none", ReasoningEffort::as_str);
    format!("{label} {}", slot_bar(filled_slots(effort, levels), levels.len()))
}

fn reasoning_color(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort], theme: &Theme) -> Color {
    let filled = filled_slots(effort, levels);
    let total = levels.len();
    if total == 0 {
        theme.text_secondary
    } else if effort == Some(ReasoningEffort::Max) {
        theme.warning
    } else if filled * 3 <= total {
        theme.text_secondary
    } else if filled * 3 <= total * 2 {
        theme.info
    } else {
        theme.success
    }
}

fn filled_slots(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> usize {
    effort.map_or(0, |effort| levels.iter().filter(|&&level| level <= effort).count())
}
