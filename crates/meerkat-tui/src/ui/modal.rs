//! Modal dialogs: the escalation (approval) prompt, the resume picker and
//! the model picker.
//!
//! Modals are driven by plain data so the UI stays decoupled from the approval
//! engine and session modules.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as BorderBlock, Borders, Clear, Paragraph};

use super::markdown::{truncate_to_width, wrap_text};
use super::render::fmt_ctx_tokens;
use crate::endpoints::describe_endpoint;
use crate::models::{ModelEntry, ModelGroup, ModelSource};

/// Maximum lines of the args preview shown in the escalation dialog.
const MAX_ARGS_LINES: usize = 10;

/// Identifier of a pending escalation, assigned by the approval engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EscalationId(pub u64);

/// The user's answer to an escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationAnswer {
    /// Allow this call only.
    AllowOnce,
    /// Allow and persist a narrow allow rule.
    AllowAndAddRule,
    /// Deny this call.
    Deny,
    /// Deny and stop the turn.
    Stop,
}

/// What the escalation dialog shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationPrompt {
    /// The helper making the call (`None`: the session's own agent).
    pub agent: Option<String>,
    /// Tool name.
    pub tool: String,
    /// Human-readable preview of the arguments.
    pub args_preview: String,
    /// Classifier explanation, if any.
    pub hint: Option<String>,
    /// Why the policy asks without the classifier (a protected path), if so.
    pub reason: Option<String>,
}

/// Session picker for `/resume` without a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePicker {
    /// `(session ref, label)` pairs, newest first.
    pub items: Vec<(String, String)>,
    /// Selected index.
    pub selected: usize,
}

impl ResumePicker {
    /// Move the selection up.
    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection down.
    pub fn down(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
    }

    /// The selected session ref.
    pub fn selected_ref(&self) -> Option<&str> {
        self.items.get(self.selected).map(|(r, _)| r.as_str())
    }
}

/// Model picker for `/model` without an id: one group per endpoint (the
/// current endpoint and model first), narrowed by a type-to-filter query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPicker {
    /// Every endpoint's models, in display order.
    pub groups: Vec<ModelGroup>,
    /// Case-insensitive substring filter over `endpoint:id` and display name.
    pub filter: String,
    /// Selected index into the filtered models (across groups).
    pub selected: usize,
}

impl ModelPicker {
    /// A picker over `groups` with nothing filtered and the first model selected.
    pub fn new(groups: Vec<ModelGroup>) -> Self {
        Self {
            groups,
            filter: String::new(),
            selected: 0,
        }
    }

    fn matches(needle: &str, group: &ModelGroup, entry: &ModelEntry) -> bool {
        needle.is_empty()
            || format!("{}:{}", group.endpoint, entry.id)
                .to_lowercase()
                .contains(needle)
            || entry
                .display_name
                .as_deref()
                .is_some_and(|name| name.to_lowercase().contains(needle))
    }

    /// The models matching the filter with their group, in display order.
    pub fn filtered(&self) -> impl Iterator<Item = (&ModelGroup, &ModelEntry)> {
        let needle = self.filter.to_lowercase();
        self.groups.iter().flat_map(move |group| {
            let needle = needle.clone();
            group
                .models
                .iter()
                .filter(move |entry| Self::matches(&needle, group, entry))
                .map(move |entry| (group, entry))
        })
    }

    /// Move the selection up.
    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection down.
    pub fn down(&mut self) {
        if self.selected + 1 < self.filtered().count() {
            self.selected += 1;
        }
    }

    /// Append a character to the filter and select the first match.
    pub fn push_filter(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    /// Remove the filter's last character and select the first match.
    pub fn pop_filter(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }

    /// The selected model as `endpoint:id`, `None` when nothing matches.
    pub fn selected_ref(&self) -> Option<String> {
        self.filtered()
            .nth(self.selected)
            .map(|(group, entry)| format!("{}:{}", group.endpoint, entry.id))
    }

    /// Warnings of every group (shown as notices when the picker cannot open).
    pub fn warnings(groups: &[ModelGroup]) -> impl Iterator<Item = String> + '_ {
        groups.iter().filter_map(|group| {
            group
                .warning
                .as_ref()
                .map(|warning| format!("{}: {warning}", group.endpoint))
        })
    }
}

/// An open modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    /// A tool call awaiting a human decision.
    Escalation {
        /// Escalation id to answer.
        id: EscalationId,
        /// What to show.
        prompt: EscalationPrompt,
        /// How many more escalations are queued behind this one.
        queued_behind: usize,
    },
    /// Session picker.
    Resume(ResumePicker),
    /// Model picker.
    Model(ModelPicker),
}

/// Center a `width`×`height` rectangle inside `area` (clamped to fit).
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Draw `modal` centered over `area`.
pub fn draw_modal(frame: &mut Frame<'_>, area: Rect, modal: &Modal) {
    let width = area.width.saturating_sub(4).clamp(20, 80);
    let inner_w = usize::from(width.saturating_sub(2));
    let (title, lines) = match modal {
        Modal::Escalation {
            prompt,
            queued_behind,
            ..
        } => {
            let title = if *queued_behind > 0 {
                format!(" Approval needed (+{queued_behind} queued) ")
            } else {
                " Approval needed ".to_string()
            };
            (title, escalation_lines(prompt, inner_w))
        }
        Modal::Resume(picker) => (
            " Resume session ".to_string(),
            picker_lines(picker, inner_w, usize::from(area.height.saturating_sub(4))),
        ),
        Modal::Model(picker) => (
            " Select model ".to_string(),
            model_picker_lines(picker, inner_w, usize::from(area.height.saturating_sub(4))),
        ),
    };
    let height = (lines.len() as u16).saturating_add(2);
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    let block = BorderBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(title);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

fn escalation_lines(prompt: &EscalationPrompt, width: usize) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = Vec::new();
    if let Some(agent) = &prompt.agent {
        lines.push(Line::from(vec![
            Span::styled("Helper: ", dim),
            Span::styled(
                agent.clone(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("Tool: ", dim),
        Span::styled(
            prompt.tool.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));
    let args = wrap_text(&prompt.args_preview, width);
    let extra = args.len().saturating_sub(MAX_ARGS_LINES);
    lines.extend(args.into_iter().take(MAX_ARGS_LINES).map(Line::from));
    if extra > 0 {
        lines.push(Line::styled(format!("… ({extra} more lines)"), dim));
    }
    if let Some(reason) = &prompt.reason {
        lines.push(Line::default());
        lines.extend(
            wrap_text(&format!("Policy: {reason}"), width)
                .into_iter()
                .map(|l| Line::styled(l, Style::default().fg(Color::Yellow))),
        );
    }
    if let Some(hint) = &prompt.hint {
        lines.push(Line::default());
        lines.extend(
            wrap_text(&format!("Classifier: {hint}"), width)
                .into_iter()
                .map(|l| Line::styled(l, Style::default().fg(Color::Yellow))),
        );
    }
    lines.push(Line::default());
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    lines.push(Line::from(vec![
        Span::styled("[y]", key),
        Span::raw(" allow once  "),
        Span::styled("[a]", key),
        Span::raw(" allow + rule  "),
        Span::styled("[n]", key),
        Span::raw(" deny  "),
        Span::styled("[s]", key),
        Span::raw(" stop"),
    ]));
    lines
}

fn picker_lines(picker: &ResumePicker, width: usize, max_rows: usize) -> Vec<Line<'static>> {
    if picker.items.is_empty() {
        return vec![Line::styled(
            "no sessions found (Esc to close)",
            Style::default().add_modifier(Modifier::DIM),
        )];
    }
    let rows = max_rows.max(1);
    let start = picker.selected.saturating_sub(rows.saturating_sub(1));
    picker
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, (id, label))| {
            let text = truncate_to_width(&format!("{id}  {label}"), width);
            if i == picker.selected {
                Line::styled(text, Style::default().add_modifier(Modifier::REVERSED))
            } else {
                Line::from(text)
            }
        })
        .collect()
}

/// Filter line, then per endpoint a header, its warning and its models,
/// then a key hint. At most `max_lines` lines; the list scrolls to keep the
/// selection visible.
fn model_picker_lines(picker: &ModelPicker, width: usize, max_lines: usize) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut head = vec![if picker.filter.is_empty() {
        Line::styled("filter: (type to filter)", dim)
    } else {
        Line::from(vec![
            Span::styled("filter: ", dim),
            Span::raw(truncate_to_width(&picker.filter, width.saturating_sub(8))),
        ])
    }];
    let footer = Line::styled("↑/↓ move · Enter select · Esc close", dim);
    let needle = picker.filter.to_lowercase();
    let id_width = picker
        .filtered()
        .map(|(_, entry)| entry.id.chars().count())
        .max()
        .unwrap_or(0)
        .min(width / 2);
    let mut body: Vec<Line<'static>> = Vec::new();
    let mut selected_line = None;
    let mut index = 0;
    for group in &picker.groups {
        let matching: Vec<&ModelEntry> = group
            .models
            .iter()
            .filter(|entry| ModelPicker::matches(&needle, group, entry))
            .collect();
        if !needle.is_empty() && matching.is_empty() {
            continue;
        }
        body.push(Line::styled(
            truncate_to_width(&group_header(group), width),
            Style::default().fg(Color::Cyan),
        ));
        if let Some(warning) = &group.warning {
            body.extend(
                wrap_text(warning, width)
                    .into_iter()
                    .map(|l| Line::styled(l, Style::default().fg(Color::Yellow))),
            );
        } else if matching.is_empty() {
            body.push(Line::styled("no models", dim));
        }
        for entry in matching {
            let text = truncate_to_width(&model_row(entry, id_width), width);
            if index == picker.selected {
                selected_line = Some(body.len());
                body.push(Line::styled(
                    text,
                    Style::default().add_modifier(Modifier::REVERSED),
                ));
            } else if entry.source == ModelSource::Current {
                body.push(Line::styled(
                    text,
                    Style::default().add_modifier(Modifier::BOLD),
                ));
            } else {
                body.push(Line::from(text));
            }
            index += 1;
        }
    }
    if index == 0 && !needle.is_empty() {
        body.push(Line::styled("no models match", dim));
    }
    let rows = max_lines.saturating_sub(head.len() + 1).max(1);
    let start = selected_line.map_or(0, |line| line.saturating_sub(rows.saturating_sub(1)));
    head.extend(body.into_iter().skip(start).take(rows));
    head.push(footer);
    head
}

/// `── litellm (openai dialect, http://localhost:4000) ──`.
fn group_header(group: &ModelGroup) -> String {
    format!(
        "── {} ({}) ──",
        group.endpoint,
        describe_endpoint(group.dialect, group.base_url.as_deref())
    )
}

/// `id  Display Name  200k ctx  current`, with the id padded to `id_width`.
fn model_row(entry: &ModelEntry, id_width: usize) -> String {
    let mut row = format!("{:<id_width$}", entry.id);
    if let Some(name) = &entry.display_name {
        row.push_str("  ");
        row.push_str(name);
    }
    if let Some(window) = entry.context_window {
        row.push_str("  ");
        row.push_str(&fmt_ctx_tokens(window));
        row.push_str(" ctx");
    }
    if entry.source == ModelSource::Current {
        row.push_str("  current");
    }
    row.trim_end().to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn picker_navigation_is_clamped() {
        let mut p = ResumePicker {
            items: vec![("a".into(), "x".into()), ("b".into(), "y".into())],
            selected: 0,
        };
        p.up();
        assert_eq!(p.selected_ref(), Some("a"));
        p.down();
        p.down();
        assert_eq!(p.selected_ref(), Some("b"));
    }

    fn model(id: &str, name: Option<&str>, source: ModelSource) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            display_name: name.map(Into::into),
            source,
            context_window: None,
            max_output_tokens: None,
        }
    }

    fn group(endpoint: &str, models: Vec<ModelEntry>) -> ModelGroup {
        ModelGroup {
            endpoint: endpoint.into(),
            dialect: crate::endpoints::WireDialect::Anthropic,
            base_url: None,
            models,
            warning: None,
        }
    }

    fn sample_models() -> ModelPicker {
        ModelPicker::new(vec![
            group(
                "anthropic",
                vec![
                    model(
                        "claude-sonnet-4-6",
                        Some("Claude Sonnet 4.6"),
                        ModelSource::Current,
                    ),
                    model(
                        "claude-opus-4-6",
                        Some("Claude Opus 4.6"),
                        ModelSource::Provider,
                    ),
                ],
            ),
            group(
                "litellm",
                vec![model("claude-haiku-4-5", None, ModelSource::Provider)],
            ),
        ])
    }

    #[test]
    fn model_picker_filters_by_endpoint_id_and_display_name() {
        let mut p = sample_models();
        for c in "OPUS".chars() {
            p.push_filter(c);
        }
        assert_eq!(
            p.selected_ref().as_deref(),
            Some("anthropic:claude-opus-4-6")
        );
        assert_eq!(p.filtered().count(), 1);
        p.filter.clear();
        for c in "4.6".chars() {
            p.push_filter(c);
        }
        let ids: Vec<&str> = p.filtered().map(|(_, e)| e.id.as_str()).collect();
        assert_eq!(ids, vec!["claude-sonnet-4-6", "claude-opus-4-6"]);
        p.filter.clear();
        for c in "litellm:".chars() {
            p.push_filter(c);
        }
        assert_eq!(
            p.selected_ref().as_deref(),
            Some("litellm:claude-haiku-4-5")
        );
    }

    #[test]
    fn model_picker_navigation_spans_groups_and_clamps() {
        let mut p = sample_models();
        p.down();
        p.down();
        p.down();
        assert_eq!(
            p.selected_ref().as_deref(),
            Some("litellm:claude-haiku-4-5")
        );
        p.push_filter('o');
        assert_eq!(p.selected, 0);
        p.down();
        assert_eq!(
            p.selected_ref().as_deref(),
            Some("anthropic:claude-opus-4-6")
        );
        p.push_filter('x');
        assert_eq!(p.selected_ref(), None);
        p.pop_filter();
        assert_eq!(
            p.selected_ref().as_deref(),
            Some("anthropic:claude-sonnet-4-6")
        );
        p.up();
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn model_row_shows_name_window_and_current() {
        let mut entry = model(
            "gemini-3.1-pro-preview",
            Some("Gemini 3.1 Pro"),
            ModelSource::Current,
        );
        entry.context_window = Some(1_048_576);
        assert_eq!(
            model_row(&entry, 24),
            "gemini-3.1-pro-preview    Gemini 3.1 Pro  1M ctx  current"
        );
        let bare = model("gpt-5.4", None, ModelSource::Provider);
        assert_eq!(model_row(&bare, 7), "gpt-5.4");
    }

    #[test]
    fn group_header_names_endpoint_dialect_and_url() {
        let mut g = group("litellm", Vec::new());
        g.dialect = crate::endpoints::WireDialect::OpenAi;
        g.base_url = Some("http://localhost:4000".into());
        assert_eq!(
            group_header(&g),
            "── litellm (openai dialect, http://localhost:4000) ──"
        );
    }

    #[test]
    fn centered_rect_fits() {
        let r = centered(Rect::new(0, 0, 10, 10), 20, 4);
        assert_eq!(r, Rect::new(0, 3, 10, 4));
    }
}
