//! Drawing: transcript blocks, input box, status bar and modal overlay.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as BorderBlock, Borders, Paragraph};

use super::markdown::{display_width, render_markdown, sanitize, truncate_to_width, wrap_text};
use super::modal::draw_modal;
use super::transcript::{
    Block, BlockId, DiffView, HelperFold, HelperState, NoticeLevel, ToolState, args_preview,
    result_line_count,
};
use super::{App, HistorySearch, StatusInfo};

/// Maximum visible input lines.
pub const MAX_INPUT_LINES: usize = 6;
/// Most lines of the helper panel under the input.
pub const MAX_PANEL_LINES: usize = 6;
/// Maximum diff lines shown under a tool block.
const MAX_DIFF_LINES: usize = 40;
/// Braille spinner frames.
pub(crate) const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// Milliseconds per spinner frame.
pub(crate) const SPINNER_FRAME_MS: u128 = 80;
/// Status-bar segment separator.
const SEP: &str = " │ ";

/// Draw the whole application.
pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    draw_at(frame, app, Instant::now());
}

/// [`draw`] with an explicit clock (for deterministic tests).
pub fn draw_at(frame: &mut Frame<'_>, app: &mut App, now: Instant) {
    let area = frame.area();
    let input_rows = if app.search().is_some() {
        1
    } else {
        app.input.lines().len().clamp(1, MAX_INPUT_LINES) as u16
    };
    let panel = app.helper_panel();
    let panel_rows = panel.len().min(MAX_PANEL_LINES) as u16;
    let [transcript_area, input_area, panel_area, status_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(input_rows + 1),
        Constraint::Length(panel_rows),
        Constraint::Length(1),
    ])
    .areas(area);

    match app.helper_view() {
        Some(header) => draw_helper_view(frame, app, header, transcript_area),
        None => draw_transcript(frame, app, transcript_area),
    }
    draw_input(frame, app, input_area, now);
    if !panel.is_empty() {
        draw_panel(frame, app, &panel, panel_area);
    }
    // The paragraph does not wrap, so an over-long status bar is clipped.
    frame.render_widget(
        Paragraph::new(status_bar(&app.status, now))
            .style(Style::default().add_modifier(Modifier::REVERSED)),
        status_area,
    );
    if let Some(modal) = &app.modal {
        draw_modal(frame, area, modal);
    }
}

fn draw_transcript(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let height = usize::from(area.height);
    app.set_viewport_height(height);
    let total = app.transcript.ensure_layout(area.width);
    let start = app.scroll.window(total, height);
    let mut lines = app.transcript.visible_lines(start, height);
    let rows = app.transcript.visible_blocks(start, height);
    if let Some(sel) = app.selection() {
        lines = lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| sel.highlight(start + i, line))
            .collect();
    }
    app.set_transcript_rows(area, start, rows);
    frame.render_widget(Paragraph::new(lines), area);
}

/// A helper's full view in place of the transcript: its header text as the
/// title line, then its blocks, scrolled by the view's own scroll state.
fn draw_helper_view(frame: &mut Frame<'_>, app: &mut App, header: BlockId, area: Rect) {
    let height = usize::from(area.height);
    app.set_viewport_height(height.saturating_sub(1));
    // No transcript rows: clicks and selection do not reach the transcript.
    app.set_transcript_rows(area, 0, Vec::new());
    let w = usize::from(area.width).max(4);
    let title = match app.transcript.block(header) {
        Some(Block::Helper(fold)) => helper_line(fold, w, "◂ ", "  Esc back"),
        _ => Line::default(),
    };
    let body = app.transcript.helper_view_lines(header, area.width);
    let body_height = height.saturating_sub(1);
    let start = app
        .helper_view_scroll()
        .map_or(0, |scroll| scroll.window(body.len(), body_height));
    let mut lines = Vec::with_capacity(height);
    lines.push(title.patch_style(Modifier::REVERSED));
    lines.extend(body.into_iter().skip(start).take(body_height));
    frame.render_widget(Paragraph::new(lines), area);
}

/// The helper panel under the input: one line per helper, indented by
/// nesting depth, the selected one reversed; past [`MAX_PANEL_LINES`]
/// helpers a window around the selection and `… and N more`.
fn draw_panel(frame: &mut Frame<'_>, app: &App, panel: &[BlockId], area: Rect) {
    let w = usize::from(area.width).max(4);
    let selected = app
        .panel_selection()
        .and_then(|id| panel.iter().position(|p| *p == id));
    let (first, shown, more) = panel_window(panel.len(), selected);
    let mut lines: Vec<Line<'static>> = panel
        .iter()
        .enumerate()
        .skip(first)
        .take(shown)
        .filter_map(|(i, id)| match app.transcript.block(*id) {
            Some(Block::Helper(fold)) => {
                let indent = "  ".repeat(fold.depth);
                let line = helper_line(fold, w, &indent, "");
                Some(if selected == Some(i) {
                    line.patch_style(Modifier::REVERSED)
                } else {
                    line
                })
            }
            _ => None,
        })
        .collect();
    if more > 0 {
        lines.push(Line::styled(
            format!("  … and {more} more"),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Which panel entries are drawn for `len` helpers with the entry
/// `selected`: `(first, count, more)`. Up to [`MAX_PANEL_LINES`] helpers
/// all show; beyond that one line fewer, around the selection, and a line
/// saying how many more there are.
pub fn panel_window(len: usize, selected: Option<usize>) -> (usize, usize, usize) {
    if len <= MAX_PANEL_LINES {
        return (0, len, 0);
    }
    let shown = MAX_PANEL_LINES - 1;
    let first = selected
        .unwrap_or(0)
        .saturating_sub(shown - 1)
        .min(len - shown);
    (first, shown, len - shown)
}

fn draw_input(frame: &mut Frame<'_>, app: &App, area: Rect, now: Instant) {
    let title = if app.quit_armed(now) {
        " press Ctrl-C again to quit "
    } else if app.quit_confirm_armed(now) {
        " press Ctrl-D (or /quit) again to quit anyway "
    } else if app.chord_pending(now) {
        " C-x- "
    } else if app.search().is_some() {
        " Ctrl-R older · Enter accept · Esc cancel "
    } else if app.helper_view().is_some() {
        " Esc back to the transcript · PgUp/PgDn scroll · Enter sends typed text "
    } else if app.panel_selection().is_some() {
        " Enter opens the helper (empty input; with text Enter sends) · Esc deselects "
    } else if app.has_recallable() {
        " Esc recall all · ↑ edit last queued · Ctrl-X Ctrl-S send now "
    } else if app.is_running() {
        " Enter queues · Ctrl-X Ctrl-S interrupts and sends · Esc/Ctrl-C interrupts "
    } else {
        ""
    };
    let block = BorderBlock::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some((search, corpus)) = app.search() {
        let (line, cursor_col) = search_line(search, corpus, usize::from(inner.width));
        frame.render_widget(Paragraph::new(line), inner);
        if app.modal.is_none() {
            let x = inner.x + cursor_col.min(usize::from(u16::MAX)) as u16;
            frame.set_cursor_position(Position::new(
                x.min(inner.right().saturating_sub(1)),
                inner.y,
            ));
        }
        return;
    }

    let prefix_w = 2usize;
    let avail = usize::from(inner.width).saturating_sub(prefix_w).max(1);
    let (row, _) = app.input.cursor();
    let rows = usize::from(inner.height).max(1);
    let first = row.saturating_sub(rows - 1);
    let cursor_dc = app.input.cursor_display_col();
    let hscroll = cursor_dc.saturating_sub(avail - 1);

    let prompt_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let lines: Vec<Line<'static>> = app
        .input
        .lines()
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(i, raw)| {
            let clean = sanitize(raw);
            let shown = if i == row {
                truncate_to_width(&skip_columns(&clean, hscroll), avail)
            } else {
                truncate_to_width(&clean, avail)
            };
            let prefix = if i == 0 { "› " } else { "  " };
            Line::from(vec![Span::styled(prefix, prompt_style), Span::raw(shown)])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);

    if app.modal.is_none() {
        let x = inner.x + (prefix_w + cursor_dc - hscroll) as u16;
        let y = inner.y + (row - first) as u16;
        frame.set_cursor_position(Position::new(x.min(inner.right().saturating_sub(1)), y));
    }
}

/// Flatten an entry for one-line display (newlines shown as `⏎`).
fn flatten(s: &str) -> String {
    sanitize(s).replace('\n', "⏎")
}

/// The reverse-search prompt line, `(reverse-i-search)`query': match`, with the
/// matched substring highlighted, and the cursor column (end of the query).
pub fn search_line(
    search: &HistorySearch,
    corpus: &[String],
    width: usize,
) -> (Line<'static>, usize) {
    let label = if search.failing() {
        "(failing reverse-i-search)`"
    } else {
        "(reverse-i-search)`"
    };
    let query = flatten(&search.query);
    let cursor_col = display_width(label) + display_width(&query);
    let head = format!("{label}{query}': ");
    let label_style = Style::default().fg(Color::Cyan);
    let mut spans = vec![Span::styled(head.clone(), label_style)];
    let avail = width.saturating_sub(display_width(&head));
    if let Some(entry) = search.current(corpus)
        && avail > 0
    {
        let range = super::search::find_smart_case(entry, &search.query).unwrap_or(0..0);
        let before = flatten(entry.get(..range.start).unwrap_or(""));
        let mid = flatten(entry.get(range.clone()).unwrap_or(""));
        let after = flatten(entry.get(range.end..).unwrap_or(""));
        // Keep the match visible: elide the start of `before` when it is too long.
        let mid_w = display_width(&mid).min(avail);
        let before_room = avail - mid_w;
        let before = if display_width(&before) > before_room {
            if before_room == 0 {
                String::new()
            } else {
                let keep = before_room - 1;
                let skip = display_width(&before) - keep;
                format!("…{}", skip_columns(&before, skip))
            }
        } else {
            before
        };
        let mid = truncate_to_width(&mid, avail.saturating_sub(display_width(&before)));
        let after = truncate_to_width(
            &after,
            avail.saturating_sub(display_width(&before) + display_width(&mid)),
        );
        spans.push(Span::raw(before));
        spans.push(Span::styled(
            mid,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(after));
    }
    (Line::from(spans), cursor_col)
}

fn skip_columns(s: &str, cols: usize) -> String {
    let mut skipped = 0;
    let mut out = String::new();
    for ch in s.chars() {
        if skipped < cols {
            skipped += display_width(ch.encode_utf8(&mut [0; 4]));
            continue;
        }
        out.push(ch);
    }
    out
}

/// Compact token count: `950`, `12.3k`, `1.2M`.
pub fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Short token count for the context gauge: `950`, `8.5k`, `84k`, `1M`, `1.2M`.
pub(crate) fn fmt_ctx_tokens(n: u64) -> String {
    let (value, unit) = match n {
        0..1_000 => return n.to_string(),
        1_000..10_000 => (format!("{:.1}", n as f64 / 1_000.0), "k"),
        10_000..1_000_000 => (format!("{:.0}", n as f64 / 1_000.0), "k"),
        _ => (format!("{:.1}", n as f64 / 1_000_000.0), "M"),
    };
    let value = value.strip_suffix(".0").unwrap_or(&value);
    format!("{value}{unit}")
}

/// Context-gauge level at which the segment turns yellow (percent).
pub const CONTEXT_WARN_PERCENT: u64 = 70;
/// Context-gauge level at which the segment turns red (percent).
pub const CONTEXT_CRITICAL_PERCENT: u64 = 90;

/// The context-gauge segment: `ctx 42% (84k/200k)` (with `~` when estimated),
/// `ctx 84k` when the window is unknown, `None` when usage is unknown.
pub fn context_segment(status: &StatusInfo) -> Option<Span<'static>> {
    let used = status.context_used?;
    let approx = if status.context_approx || status.window_approx {
        "~"
    } else {
        ""
    };
    let Some(window) = status.context_window.filter(|w| *w > 0) else {
        return Some(Span::raw(format!("ctx {approx}{}", fmt_ctx_tokens(used))));
    };
    let percent = used.saturating_mul(100) / window;
    let style = if percent >= CONTEXT_CRITICAL_PERCENT {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if percent >= CONTEXT_WARN_PERCENT {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    Some(Span::styled(
        format!(
            "ctx {approx}{percent}% ({}/{})",
            fmt_ctx_tokens(used),
            fmt_ctx_tokens(window)
        ),
        style,
    ))
}

/// Spinner glyph for the elapsed time since `since` (first frame when `None`).
fn spinner_frame(since: Option<Instant>, now: Instant) -> char {
    let elapsed = since.map_or(0, |s| now.saturating_duration_since(s).as_millis());
    SPINNER
        .get((elapsed / SPINNER_FRAME_MS) as usize % SPINNER.len())
        .copied()
        .unwrap_or('⠋')
}

/// Style of the `auto:` segment for a display state.
fn auto_style(state: &str) -> Style {
    if state.eq_ignore_ascii_case("paused") {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
            .remove_modifier(Modifier::REVERSED)
    } else if state.eq_ignore_ascii_case("off") {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default()
    }
}

/// The styled status bar (drawn on a reversed background).
pub fn status_bar(status: &StatusInfo, now: Instant) -> Line<'static> {
    let mut segs: Vec<Span<'static>> = Vec::new();
    if !status.provider.is_empty() || !status.model.is_empty() {
        segs.push(Span::raw(format!("{}/{}", status.provider, status.model)));
    }
    if !status.session_short.is_empty() {
        segs.push(Span::raw(format!("session {}", status.session_short)));
    }
    segs.push(Span::raw(format!(
        "↑{} ↓{}",
        fmt_tokens(status.input_tokens),
        fmt_tokens(status.output_tokens)
    )));
    segs.extend(super::cost::cost_segment_text(&status.cost, &status.prices).map(Span::raw));
    segs.extend(context_segment(status));
    if !status.auto_state.is_empty() {
        let (c, cm, t, tm) = status.denials;
        let text = if status.auto_state.eq_ignore_ascii_case("off") {
            format!("auto: {}", status.auto_state)
        } else {
            format!("auto: {} (deny {c}/{cm}, {t}/{tm})", status.auto_state)
        };
        segs.push(Span::styled(text, auto_style(&status.auto_state)));
    }
    if status.classifying {
        let frame = spinner_frame(status.running_since, now);
        segs.push(Span::styled(
            format!("{frame} classifying…"),
            Style::default().fg(Color::Magenta),
        ));
    }
    if let Some(since) = status.running_since {
        let elapsed = now.saturating_duration_since(since);
        let frame = spinner_frame(Some(since), now);
        segs.push(Span::raw(format!("{frame} running {}s", elapsed.as_secs())));
    }
    if status.queued > 0 {
        segs.push(Span::raw(format!("queued {}", status.queued)));
    }
    if status.helpers > 0 {
        segs.push(Span::styled(
            format!(
                "{} helper{}",
                status.helpers,
                if status.helpers == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::Magenta),
        ));
    }
    if status.helpers > 0 || status.helper_panel {
        segs.push(Span::raw("ctrl+↓ helpers"));
    }
    let mut spans = Vec::with_capacity(segs.len() * 2 + 1);
    spans.push(Span::raw(" "));
    for (i, seg) in segs.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(SEP));
        }
        spans.push(seg);
    }
    Line::from(spans)
}

/// The status-bar text (unstyled [`status_bar`]).
pub fn status_line(status: &StatusInfo, now: Instant) -> String {
    status_bar(status, now)
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

/// Style for one line of a unified diff.
pub fn diff_line_style(line: &str) -> Style {
    let dim = Style::default().add_modifier(Modifier::DIM);
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
        dim.add_modifier(Modifier::BOLD)
    } else if line.starts_with("@@") {
        dim.fg(Color::Cyan)
    } else if line.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::default().fg(Color::Red)
    } else if line.starts_with('\\') || line.starts_with("index ") {
        dim
    } else {
        Style::default()
    }
}

fn prefixed(
    text: &str,
    width: usize,
    first: &str,
    rest: &str,
    style: Style,
    out: &mut Vec<Line<'static>>,
) {
    let prefix_w = display_width(first).max(display_width(rest));
    for (i, l) in wrap_text(text, width.saturating_sub(prefix_w))
        .into_iter()
        .enumerate()
    {
        let p = if i == 0 { first } else { rest };
        out.push(Line::from(vec![
            Span::styled(p.to_string(), style),
            Span::styled(l, style),
        ]));
    }
}

/// Lay out one block at `width` (including a trailing spacer line).
pub fn block_lines(block: &Block, width: u16) -> Vec<Line<'static>> {
    let w = usize::from(width).max(4);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = Vec::new();
    match block {
        Block::User { text } => {
            let style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
            prefixed(text, w, "› ", "  ", style, &mut out);
        }
        Block::Queued { text, label, .. } => {
            let style = Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM);
            let shown = format!("{text}  · {}", label.text());
            prefixed(&shown, w, "› ", "  ", style, &mut out);
        }
        Block::Assistant { md } => {
            for (i, line) in render_markdown(md, (w - 2) as u16).into_iter().enumerate() {
                let p = if i == 0 { "• " } else { "  " };
                let mut spans = vec![Span::raw(p)];
                spans.extend(line.spans);
                out.push(Line::from(spans));
            }
        }
        Block::Reasoning { text, collapsed } => {
            let style = dim.add_modifier(Modifier::ITALIC);
            if *collapsed {
                let n = text.chars().count();
                out.push(Line::styled(
                    truncate_to_width(&format!("✻ thinking ({n} chars, Ctrl-O to expand)"), w),
                    style.fg(Color::Magenta),
                ));
            } else {
                out.push(Line::styled("✻ thinking", style.fg(Color::Magenta)));
                prefixed(text, w, "  ", "  ", style, &mut out);
            }
        }
        Block::Tool {
            name,
            args,
            state,
            result_preview,
            diff,
            note,
            depth,
            collapsed,
        } => {
            let indent = "  ".repeat(*depth);
            let (marker, mut status, color) = match state {
                ToolState::Requested => ("○", String::new(), Color::DarkGray),
                ToolState::Running { .. } => ("◐", " running…".to_string(), Color::Yellow),
                ToolState::Done {
                    is_error: false,
                    duration_ms,
                } => ("✓", format!(" ({duration_ms}ms)"), Color::Green),
                ToolState::Done {
                    is_error: true,
                    duration_ms,
                } => ("✗", format!(" ({duration_ms}ms)"), Color::Red),
                ToolState::Denied { reason } => ("⊘", format!(" denied: {reason}"), Color::Red),
            };
            let has_body = result_preview.is_some() || diff.is_some();
            if has_body {
                status.push_str(&tool_toggle_hint(
                    *collapsed,
                    result_preview.as_deref(),
                    diff.as_ref(),
                ));
            }
            let header = format!("{indent}{marker} {name}({})", args_preview(args));
            let header_w = w.saturating_sub(display_width(&status));
            out.push(Line::from(vec![
                Span::styled(
                    truncate_to_width(&sanitize(&header), header_w),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(truncate_to_width(&sanitize(&status), w), dim),
            ]));
            if let Some(note) = note {
                out.push(Line::styled(
                    truncate_to_width(&sanitize(&format!("{indent}  {note}")), w),
                    dim,
                ));
            }
            if !*collapsed {
                tool_body(
                    &indent,
                    result_preview.as_deref(),
                    diff.as_ref(),
                    w,
                    &mut out,
                );
            }
        }
        Block::Approval {
            tool,
            tier,
            allowed,
            reason,
        } => {
            let (marker, verb, color) = if *allowed {
                ("✓", "allowed", Color::Green)
            } else {
                ("⊘", "blocked", Color::Red)
            };
            let mut text = format!("{marker} {verb} {tool} [{tier}]");
            if let Some(r) = reason {
                text.push_str(": ");
                text.push_str(r);
            }
            prefixed(&text, w, "", "  ", Style::default().fg(color), &mut out);
        }
        Block::Notice { level, text } => {
            let style = match level {
                NoticeLevel::Info => dim,
                NoticeLevel::Warn => Style::default().fg(Color::Yellow),
                NoticeLevel::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            };
            prefixed(text, w, "", "", style, &mut out);
        }
        Block::Helper(fold) => out.push(helper_header(fold, w)),
    }
    out.push(Line::default());
    out
}

/// A helper's one-line header in the transcript: `▸ [scout · model] ⠋
/// running · 4 tool calls · grep "fn resolve"` (`▾` when expanded), cut to
/// the width.
fn helper_header(fold: &HelperFold, w: usize) -> Line<'static> {
    let indent = "  ".repeat(fold.depth.saturating_sub(1));
    let marker = if fold.expanded { "▾ " } else { "▸ " };
    helper_line(fold, w, &format!("{indent}{marker}"), "")
}

/// A helper's header text after `lead`: its tag, then its status, then
/// `tail` (dim), cut to the width.
fn helper_line(fold: &HelperFold, w: usize, lead: &str, tail: &str) -> Line<'static> {
    let head = format!("{lead}{} ", sanitize(&fold.tag()));
    let status = sanitize(&fold.status_text());
    let status_style = match fold.state {
        HelperState::Running => Style::default().fg(Color::Yellow),
        HelperState::Done => Style::default().fg(Color::Green),
        HelperState::Failed => Style::default().fg(Color::Red),
        HelperState::Cancelled => Style::default().fg(Color::Yellow),
        HelperState::Unmeasured => Style::default().add_modifier(Modifier::DIM),
    };
    let head = truncate_to_width(&head, w);
    let rest = w.saturating_sub(display_width(&head));
    let status = truncate_to_width(&status, rest);
    let rest = rest.saturating_sub(display_width(&status));
    Line::from(vec![
        Span::styled(
            head,
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(status, status_style),
        Span::styled(
            truncate_to_width(tail, rest),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])
}

/// The hint after a tool header with output: `▸ 12 lines` / `▸ +3 -1` when
/// collapsed (click or Ctrl-T to expand), `▾` when expanded.
fn tool_toggle_hint(collapsed: bool, preview: Option<&str>, diff: Option<&DiffView>) -> String {
    if !collapsed {
        return " ▾".to_string();
    }
    let mut parts = Vec::new();
    if let Some(preview) = preview {
        let n = result_line_count(preview);
        parts.push(format!("{n} line{}", if n == 1 { "" } else { "s" }));
    }
    if let Some(d) = diff {
        parts.push(format!("+{} -{}", d.added, d.removed));
    }
    format!(" ▸ {}", parts.join(", "))
}

/// The expanded body of a tool block: the result preview, then the diff.
fn tool_body(
    indent: &str,
    preview: Option<&str>,
    diff: Option<&DiffView>,
    w: usize,
    out: &mut Vec<Line<'static>>,
) {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let body_prefix = format!("{indent}  │ ");
    if let Some(preview) = preview {
        prefixed(preview, w, &body_prefix, &body_prefix, dim, out);
    }
    if let Some(d) = diff {
        out.push(Line::styled(
            truncate_to_width(
                &format!("{indent}  {} (+{} -{})", d.path, d.added, d.removed),
                w,
            ),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        let total = d.unified.lines().count();
        for l in d.unified.lines().take(MAX_DIFF_LINES) {
            out.push(Line::styled(
                truncate_to_width(&sanitize(&format!("{indent}  {l}")), w),
                diff_line_style(l),
            ));
        }
        if total > MAX_DIFF_LINES {
            out.push(Line::styled(
                format!("{indent}  … {} more lines", total - MAX_DIFF_LINES),
                dim,
            ));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn token_formatting() {
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(12_345), "12.3k");
        assert_eq!(fmt_tokens(1_200_000), "1.2M");
    }

    #[test]
    fn status_line_segments() {
        let now = Instant::now();
        let mut s = StatusInfo::default();
        s.set_model("anthropic", "claude-sonnet-4-6");
        s.set_session("abc12345-ffff");
        s.set_tokens(1500, 20);
        s.set_auto("active", (1, 3, 4, 20));
        s.running_since = Some(now);
        s.queued = 2;
        let line = status_line(&s, now);
        assert_eq!(
            line,
            " anthropic/claude-sonnet-4-6 │ session abc12345 │ ↑1.5k ↓20 │ auto: active (deny 1/3, 4/20) │ ⠋ running 0s │ queued 2"
        );
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn tool_block_layout() {
        let mut block = Block::Tool {
            name: "read_file".into(),
            args: serde_json::json!({"path": "a.rs"}),
            state: ToolState::Done {
                is_error: false,
                duration_ms: 3,
            },
            result_preview: Some("fn main() {}".into()),
            diff: None,
            note: None,
            depth: 0,
            collapsed: true,
        };
        let lines: Vec<String> = block_lines(&block, 60).iter().map(line_text).collect();
        assert_eq!(
            lines,
            vec![
                "✓ read_file({\"path\":\"a.rs\"}) (3ms) ▸ 1 line".to_string(),
                String::new(),
            ]
        );
        if let Block::Tool { collapsed, .. } = &mut block {
            *collapsed = false;
        }
        let lines: Vec<String> = block_lines(&block, 40).iter().map(line_text).collect();
        assert_eq!(
            lines,
            vec![
                "✓ read_file({\"path\":\"a.rs\"}) (3ms) ▾".to_string(),
                "  │ fn main() {}".to_string(),
                String::new(),
            ]
        );
    }

    #[test]
    fn collapsed_tool_hint_counts_lines_and_diff() {
        let preview = (0..30)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut t = super::super::transcript::Transcript::new();
        t.apply_agent_event(&meerkat_core::AgentEvent::ToolCallRequested {
            id: "g".into(),
            name: "grep".into(),
            args: super::super::transcript::test_support::test_args(serde_json::json!({})),
        });
        t.apply_agent_event(&meerkat_core::AgentEvent::ToolExecutionCompleted {
            id: "g".into(),
            name: "grep".into(),
            is_error: false,
            duration_ms: 2,
        });
        t.apply_agent_event(&super::super::transcript::test_support::test_tool_result(
            "g", &preview,
        ));
        let lines = t.ensure_layout(80);
        assert_eq!(lines, 2, "collapsed: header + spacer");
        let header = line_text(&t.visible_lines(0, 1)[0]);
        assert!(header.ends_with("(2ms) ▸ 30 lines"), "{header}");

        let diff = DiffView {
            path: "x".into(),
            unified: "+a".into(),
            added: 3,
            removed: 1,
        };
        assert_eq!(
            tool_toggle_hint(true, Some("ok"), Some(&diff)),
            " ▸ 1 line, +3 -1"
        );
        assert_eq!(tool_toggle_hint(true, None, Some(&diff)), " ▸ +3 -1");
        assert_eq!(tool_toggle_hint(false, None, Some(&diff)), " ▾");
    }

    #[test]
    fn tool_without_output_has_no_hint() {
        let block = Block::Tool {
            name: "shell".into(),
            args: serde_json::json!({}),
            state: ToolState::Running {
                since: Instant::now(),
            },
            result_preview: None,
            diff: None,
            note: None,
            depth: 0,
            collapsed: true,
        };
        let header = line_text(&block_lines(&block, 60)[0]);
        assert_eq!(header, "◐ shell({}) running…");
    }

    #[test]
    fn collapsed_reasoning_is_one_line() {
        let block = Block::Reasoning {
            text: "a\nb\nc".into(),
            collapsed: true,
        };
        assert_eq!(block_lines(&block, 60).len(), 2);
    }

    fn seg_style(line: &Line<'_>, needle: &str) -> Style {
        line.spans
            .iter()
            .find(|s| s.content.contains(needle))
            .map(|s| s.style)
            .unwrap_or_else(|| panic!("no segment {needle:?}"))
    }

    #[test]
    fn status_bar_auto_states_and_classifying() {
        let now = Instant::now();
        let mut s = StatusInfo::default();
        s.set_auto("PAUSED", (3, 3, 5, 20));
        s.set_classifying(true);
        let bar = status_bar(&s, now);
        assert_eq!(seg_style(&bar, "auto: PAUSED").bg, Some(Color::Yellow));
        assert!(status_line(&s, now).ends_with("auto: PAUSED (deny 3/3, 5/20) │ ⠋ classifying…"));
        s.set_auto("off", (0, 3, 0, 20));
        s.set_classifying(false);
        let bar = status_bar(&s, now);
        assert!(
            seg_style(&bar, "auto: off")
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(status_line(&s, now).ends_with("│ auto: off"));
    }

    #[test]
    fn context_token_formatting() {
        assert_eq!(fmt_ctx_tokens(950), "950");
        assert_eq!(fmt_ctx_tokens(8_500), "8.5k");
        assert_eq!(fmt_ctx_tokens(84_000), "84k");
        assert_eq!(fmt_ctx_tokens(84_400), "84k");
        assert_eq!(fmt_ctx_tokens(200_000), "200k");
        assert_eq!(fmt_ctx_tokens(1_000_000), "1M");
        assert_eq!(fmt_ctx_tokens(1_250_000), "1.2M");
    }

    #[test]
    fn context_gauge_text_and_thresholds() {
        let now = Instant::now();
        let mut s = StatusInfo::default();
        s.set_tokens(1500, 20);
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20");
        s.context_used = Some(84_000);
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20 │ ctx 84k");
        s.context_window = Some(200_000);
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20 │ ctx 42% (84k/200k)");
        let bar = status_bar(&s, now);
        assert_eq!(seg_style(&bar, "ctx"), Style::default());
        s.window_approx = true;
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20 │ ctx ~42% (84k/200k)");
        s.window_approx = false;
        s.context_approx = true;
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20 │ ctx ~42% (84k/200k)");
        s.context_used = Some(139_000);
        assert_eq!(seg_style(&status_bar(&s, now), "ctx").fg, None);
        s.context_used = Some(140_000);
        assert_eq!(
            seg_style(&status_bar(&s, now), "ctx").fg,
            Some(Color::Yellow)
        );
        s.context_used = Some(180_000);
        assert_eq!(seg_style(&status_bar(&s, now), "ctx").fg, Some(Color::Red));
        // A zero window is treated as unknown.
        s.context_window = Some(0);
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20 │ ctx ~180k");
    }

    #[test]
    fn cost_segment_follows_the_tokens() {
        use crate::approval::rules::PricingSection;
        use crate::pricing::PriceTable;
        use std::collections::BTreeMap;
        use std::sync::Arc;
        let now = Instant::now();
        let mut s = StatusInfo::default();
        s.set_tokens(1500, 20);
        s.context_used = Some(84_000);
        let usage = |input| meerkat_core::Usage {
            input_tokens: input,
            ..meerkat_core::Usage::default()
        };
        s.cost.record("m", &usage(1_000));
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20 │ $? │ ctx 84k");
        s.prices = Arc::new(PriceTable::new(BTreeMap::from([(
            "m".to_string(),
            PricingSection {
                input: Some(420.0),
                output: Some(1.0),
                ..PricingSection::default()
            },
        )])));
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20 │ $0.42 │ ctx 84k");
        s.cost.record("unpriced", &usage(1));
        assert_eq!(status_line(&s, now), " ↑1.5k ↓20 │ ~$0.42 │ ctx 84k");
    }

    #[test]
    fn diff_lines_are_colored_and_capped() {
        use super::super::transcript::DiffView;
        let mut unified =
            String::from("--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n ctx\n-old\n+new\n");
        for i in 0..50 {
            unified.push_str(&format!("+l{i}\n"));
        }
        let block = Block::Tool {
            name: "edit_file".into(),
            args: serde_json::json!({}),
            state: ToolState::Done {
                is_error: false,
                duration_ms: 1,
            },
            result_preview: None,
            diff: Some(DiffView {
                path: "x.rs".into(),
                unified,
                added: 51,
                removed: 1,
            }),
            note: None,
            depth: 0,
            collapsed: false,
        };
        let lines = block_lines(&block, 60);
        let find = |t: &str| {
            lines
                .iter()
                .find(|l| line_text(l) == t)
                .unwrap_or_else(|| panic!("missing {t:?}"))
        };
        assert_eq!(find("  +new").style.fg, Some(Color::Green));
        assert_eq!(find("  -old").style.fg, Some(Color::Red));
        assert_eq!(find("  @@ -1,2 +1,2 @@").style.fg, Some(Color::Cyan));
        assert!(
            find("  --- a/x.rs")
                .style
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(find("  +++ b/x.rs").style.fg.is_none());
        assert!(
            find("  … 16 more lines")
                .style
                .add_modifier
                .contains(Modifier::DIM)
        );
        // header + path line + 40 diff lines + trailer + spacer
        assert_eq!(lines.len(), 44);
    }
}
