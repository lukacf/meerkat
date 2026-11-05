//! Text wrapping and markdown rendering.
//!
//! Everything here produces pre-wrapped lines so that the transcript can compute
//! exact heights without asking ratatui to wrap. The markdown renderer is a single
//! linear pass over `pulldown-cmark` events: it is called on every update of a
//! streaming assistant block, so it never allocates per character beyond the
//! output and tolerates partial input (unclosed fences, emphasis, tables).

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Number of spaces a tab expands to.
const TAB_WIDTH: usize = 4;

/// Markdown element styles. Kept as constants/functions so a theme can replace them later.
pub mod style {
    use super::{Color, HeadingLevel, Modifier, Style};

    /// Plain paragraph text.
    pub const TEXT: Style = Style::new();
    /// Level-1 heading.
    pub const H1: Style = Style::new()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::UNDERLINED);
    /// Level-2 heading.
    pub const H2: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    /// Level 3+ headings.
    pub const H3: Style = Style::new().add_modifier(Modifier::BOLD);
    /// `*emphasis*`.
    pub const EMPHASIS: Style = Style::new().add_modifier(Modifier::ITALIC);
    /// `**strong**`.
    pub const STRONG: Style = Style::new().add_modifier(Modifier::BOLD);
    /// `~~strikethrough~~`.
    pub const STRIKETHROUGH: Style = Style::new().add_modifier(Modifier::CROSSED_OUT);
    /// `` `inline code` ``.
    pub const INLINE_CODE: Style = Style::new().fg(Color::Yellow);
    /// Code block contents.
    pub const CODE_BLOCK: Style = Style::new().fg(Color::LightYellow);
    /// Code block left gutter.
    pub const CODE_GUTTER: Style = Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM);
    /// Text inside a block quote (patched over the inline style).
    pub const QUOTE: Style = Style::new().add_modifier(Modifier::DIM);
    /// Block-quote `│` bar.
    pub const QUOTE_BAR: Style = Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM);
    /// Link text.
    pub const LINK: Style = Style::new()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);
    /// The ` (url)` suffix after link text.
    pub const LINK_URL: Style = Style::new().add_modifier(Modifier::DIM);
    /// List bullets and numbers.
    pub const LIST_MARKER: Style = Style::new().fg(Color::Cyan);
    /// Horizontal rule.
    pub const RULE: Style = Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM);
    /// Table header row.
    pub const TABLE_HEADER: Style = Style::new().add_modifier(Modifier::BOLD);
    /// Table header separator.
    pub const TABLE_SEPARATOR: Style = Style::new().add_modifier(Modifier::DIM);

    /// Style for a heading of `level`.
    pub const fn heading(level: HeadingLevel) -> Style {
        match level {
            HeadingLevel::H1 => H1,
            HeadingLevel::H2 => H2,
            _ => H3,
        }
    }
}

/// Bullet glyphs by list nesting depth (cycled).
const BULLETS: [&str; 3] = ["• ", "◦ ", "▪ "];
/// Gutter drawn left of code-block lines.
const CODE_GUTTER: &str = "▎ ";
/// Block-quote prefix.
const QUOTE_BAR: &str = "│ ";
/// Column separator for tables.
const TABLE_SEP: &str = " │ ";

/// Render markdown into wrapped, styled lines at most `width` columns wide.
///
/// Paragraphs, headings and list items are word-wrapped (single newlines inside a
/// paragraph are kept as line breaks, which matches how models format chat output).
/// Code blocks are never wrapped; over-long lines are clipped by ratatui. Partial
/// markdown (as seen while streaming) renders as well as it parses and never fails.
/// Always returns at least one line.
pub fn render_markdown(md: &str, width: u16) -> Vec<Line<'static>> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut r = Renderer::new(usize::from(width));
    for event in Parser::new_ext(md, opts) {
        r.event(event);
    }
    r.finish()
}

/// A styled fragment of one word.
type Part = (String, Style);

/// Inline token awaiting wrapping.
#[derive(Debug)]
enum Tok {
    Word { parts: Vec<Part>, width: usize },
    Space,
    Break,
}

/// Block-level container that contributes a line prefix.
#[derive(Debug)]
enum Container {
    Quote,
    List {
        next: Option<u64>,
    },
    Item {
        marker: String,
        width: usize,
        marker_pending: bool,
    },
}

#[derive(Debug, Default)]
struct TableState {
    rows: Vec<Vec<String>>,
    header_rows: usize,
    cell: Option<String>,
    in_head: bool,
}

#[derive(Debug)]
struct Renderer {
    width: usize,
    out: Vec<Line<'static>>,
    containers: Vec<Container>,
    toks: Vec<Tok>,
    word: Vec<Part>,
    word_width: usize,
    inline: Vec<Style>,
    links: Vec<(String, usize)>,
    code: Option<String>,
    table: Option<TableState>,
    /// Emit a blank separator line before the next output line, with the prefix
    /// of the first `n` containers.
    pending_blank: Option<usize>,
}

impl Renderer {
    fn new(width: usize) -> Self {
        Self {
            width,
            out: Vec::new(),
            containers: Vec::new(),
            toks: Vec::new(),
            word: Vec::new(),
            word_width: 0,
            inline: Vec::new(),
            links: Vec::new(),
            code: None,
            table: None,
            pending_blank: None,
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if let Some(code) = self.code.take() {
            self.emit_code(&code);
        }
        self.flush_inline();
        self.flush_pending_markers();
        if self.out.is_empty() {
            self.out.push(Line::default());
        }
        self.out
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => self.inline_code(&code),
            Event::InlineMath(t) | Event::DisplayMath(t) | Event::InlineHtml(t) => self.text(&t),
            Event::Html(html) => {
                let html = html.strip_suffix('\n').unwrap_or(&html);
                self.text(html);
                self.toks.push(Tok::Break);
            }
            Event::FootnoteReference(label) => self.text(&format!("[^{label}]")),
            Event::SoftBreak | Event::HardBreak => {
                if let Some(cell) = self.table.as_mut().and_then(|t| t.cell.as_mut()) {
                    cell.push(' ');
                } else {
                    self.end_word();
                    self.toks.push(Tok::Break);
                }
            }
            Event::Rule => {
                self.flush_inline();
                let avail = self.avail();
                self.emit_line(vec![Span::styled("─".repeat(avail), style::RULE)]);
                self.mark_blank();
            }
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.flush_inline(),
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.inline.push(style::heading(level));
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.containers.push(Container::Quote);
            }
            Tag::CodeBlock(_) => {
                self.flush_inline();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush_inline();
                self.containers.push(Container::List { next: start });
            }
            Tag::Item => {
                self.flush_inline();
                let depth = self
                    .containers
                    .iter()
                    .filter(|c| matches!(c, Container::List { .. }))
                    .count();
                let marker = match self.containers.last_mut() {
                    Some(Container::List { next: Some(n) }) => {
                        let m = format!("{n}. ");
                        *n = n.saturating_add(1);
                        m
                    }
                    _ => BULLETS[depth.saturating_sub(1) % BULLETS.len()].to_string(),
                };
                let width = display_width(&marker);
                self.containers.push(Container::Item {
                    marker,
                    width,
                    marker_pending: true,
                });
            }
            Tag::Table(_) => {
                self.flush_inline();
                self.table = Some(TableState::default());
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cell = Some(String::new());
                }
            }
            Tag::Emphasis => self.inline.push(style::EMPHASIS),
            Tag::Strong => self.inline.push(style::STRONG),
            Tag::Strikethrough => self.inline.push(style::STRIKETHROUGH),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.inline.push(style::LINK);
                let start = self.toks.len();
                self.links.push((dest_url.into_string(), start));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => {
                self.flush_inline();
                self.mark_blank();
            }
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.inline.pop();
                self.mark_blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.containers.pop();
                self.mark_blank();
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.emit_code(&code);
                }
                self.mark_blank();
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.containers.pop();
                let nested = self
                    .containers
                    .iter()
                    .any(|c| matches!(c, Container::Item { .. }));
                if !nested {
                    self.mark_blank();
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                self.flush_pending_markers();
                self.containers.pop();
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.emit_table(&t);
                }
                self.mark_blank();
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = false;
                    t.header_rows = t.rows.len();
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut()
                    && let Some(cell) = t.cell.take()
                {
                    let cell = cell.trim().to_string();
                    match t.rows.last_mut() {
                        Some(row) => row.push(cell),
                        None => t.rows.push(vec![cell]),
                    }
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline.pop();
            }
            TagEnd::Link | TagEnd::Image => {
                self.inline.pop();
                if let Some((url, start)) = self.links.pop() {
                    self.link_suffix(&url, start);
                }
            }
            _ => {}
        }
    }

    // ---- inline content -------------------------------------------------

    fn current_style(&self) -> Style {
        let mut s = self
            .inline
            .iter()
            .fold(style::TEXT, |acc, st| acc.patch(*st));
        if self
            .containers
            .iter()
            .any(|c| matches!(c, Container::Quote))
        {
            s = s.patch(style::QUOTE);
        }
        s
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = self.code.as_mut() {
            code.push_str(text);
            return;
        }
        if let Some(cell) = self.table.as_mut().and_then(|t| t.cell.as_mut()) {
            cell.push_str(text);
            return;
        }
        let st = self.current_style();
        for ch in text.chars() {
            match ch {
                '\n' => {
                    self.end_word();
                    self.toks.push(Tok::Break);
                }
                c if c.is_whitespace() => {
                    self.end_word();
                    if !matches!(self.toks.last(), Some(Tok::Space)) {
                        self.toks.push(Tok::Space);
                    }
                }
                c => self.push_char(c, st),
            }
        }
    }

    fn inline_code(&mut self, code: &str) {
        if let Some(cell) = self.table.as_mut().and_then(|t| t.cell.as_mut()) {
            cell.push_str(code);
            return;
        }
        let st = self.current_style().patch(style::INLINE_CODE);
        // Spaces inside a code span do not break lines.
        for ch in code.chars() {
            self.push_char(ch, st);
        }
    }

    fn push_char(&mut self, ch: char, st: Style) {
        let ch = match ch {
            '\t' => ' ',
            c if c.is_control() => '\u{fffd}',
            c => c,
        };
        self.word_width += ch.width().unwrap_or(0);
        match self.word.last_mut() {
            Some((s, last)) if *last == st => s.push(ch),
            _ => self.word.push((ch.to_string(), st)),
        }
    }

    fn end_word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        self.toks.push(Tok::Word {
            parts: std::mem::take(&mut self.word),
            width: std::mem::take(&mut self.word_width),
        });
    }

    fn link_suffix(&mut self, url: &str, start: usize) {
        self.end_word();
        let text: String = self
            .toks
            .get(start..)
            .unwrap_or_default()
            .iter()
            .filter_map(|t| match t {
                Tok::Word { parts, .. } => {
                    Some(parts.iter().map(|(s, _)| s.as_str()).collect::<String>())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        if url.is_empty() || text == url || url.strip_prefix("mailto:") == Some(text.as_str()) {
            return;
        }
        self.toks.push(Tok::Space);
        let st = self.current_style().patch(style::LINK_URL);
        self.push_char('(', st);
        for ch in url.chars() {
            self.push_char(ch, st);
        }
        self.push_char(')', st);
        self.end_word();
    }

    /// Wrap pending inline tokens and emit them.
    fn flush_inline(&mut self) {
        self.end_word();
        if self.toks.is_empty() {
            return;
        }
        let toks = std::mem::take(&mut self.toks);
        let avail = self.avail();
        for line in wrap_tokens(toks, avail) {
            let spans = line
                .into_iter()
                .map(|(s, st)| Span::styled(s, st))
                .collect();
            self.emit_line(spans);
        }
    }

    // ---- block content --------------------------------------------------

    fn emit_code(&mut self, code: &str) {
        let body = code.strip_suffix('\n').unwrap_or(code);
        for raw in body.split('\n') {
            let clean = sanitize(raw);
            self.emit_line(vec![
                Span::styled(CODE_GUTTER, style::CODE_GUTTER),
                Span::styled(clean, style::CODE_BLOCK),
            ]);
        }
    }

    fn emit_table(&mut self, t: &TableState) {
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0usize; cols];
        for row in &t.rows {
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(display_width(&sanitize(cell)));
            }
        }
        let avail = self.avail();
        for (i, row) in t.rows.iter().enumerate() {
            let text = widths
                .iter()
                .enumerate()
                .map(|(c, w)| {
                    let cell = sanitize(row.get(c).map_or("", String::as_str));
                    let pad = w.saturating_sub(display_width(&cell));
                    format!("{cell}{}", " ".repeat(pad))
                })
                .collect::<Vec<_>>()
                .join(TABLE_SEP);
            let header = i < t.header_rows;
            let st = if header {
                style::TABLE_HEADER
            } else {
                style::TEXT
            };
            self.emit_line(vec![Span::styled(
                truncate_to_width(text.trim_end(), avail),
                st,
            )]);
            if header && i + 1 == t.header_rows {
                let sep = widths
                    .iter()
                    .map(|w| "─".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                self.emit_line(vec![Span::styled(
                    truncate_to_width(&sep, avail),
                    style::TABLE_SEPARATOR,
                )]);
            }
        }
    }

    /// Emit a bare marker line for items that produced no content (e.g. `- ` while streaming).
    fn flush_pending_markers(&mut self) {
        if matches!(
            self.containers.last(),
            Some(Container::Item {
                marker_pending: true,
                ..
            })
        ) {
            self.emit_line(Vec::new());
        }
    }

    fn mark_blank(&mut self) {
        self.pending_blank = Some(self.containers.len());
    }

    /// Columns available for content after the container prefix.
    fn avail(&self) -> usize {
        let prefix: usize = self
            .containers
            .iter()
            .map(|c| match c {
                Container::Quote => display_width(QUOTE_BAR),
                Container::List { .. } => 0,
                Container::Item { width, .. } => *width,
            })
            .sum();
        self.width.saturating_sub(prefix).max(1)
    }

    /// Build the prefix for the first `depth` containers. With `consume`, pending
    /// item markers are printed (once); otherwise items contribute spaces.
    fn prefix(&mut self, depth: usize, consume: bool) -> Vec<Span<'static>> {
        let quote = self
            .containers
            .iter()
            .any(|c| matches!(c, Container::Quote));
        let mut spans = Vec::new();
        let mut pad = String::new();
        for c in self.containers.iter_mut().take(depth) {
            match c {
                Container::Quote => {
                    if !pad.is_empty() {
                        spans.push(Span::raw(std::mem::take(&mut pad)));
                    }
                    spans.push(Span::styled(QUOTE_BAR, style::QUOTE_BAR));
                }
                Container::List { .. } => {}
                Container::Item {
                    marker,
                    width,
                    marker_pending,
                } => {
                    if consume && *marker_pending {
                        *marker_pending = false;
                        if !pad.is_empty() {
                            spans.push(Span::raw(std::mem::take(&mut pad)));
                        }
                        let mut st = style::LIST_MARKER;
                        if quote {
                            st = st.patch(style::QUOTE);
                        }
                        spans.push(Span::styled(marker.clone(), st));
                    } else {
                        pad.extend(std::iter::repeat_n(' ', *width));
                    }
                }
            }
        }
        if !pad.is_empty() {
            spans.push(Span::raw(pad));
        }
        spans
    }

    fn emit_line(&mut self, content: Vec<Span<'static>>) {
        if let Some(depth) = self.pending_blank.take()
            && !self.out.is_empty()
        {
            let depth = depth.min(self.containers.len());
            let mut blank = self.prefix(depth, false);
            // Trailing padding on a blank line is noise.
            if matches!(blank.last(), Some(s) if s.content.trim().is_empty() && s.style == Style::default())
            {
                blank.pop();
            }
            self.out.push(Line::from(blank));
        }
        let mut spans = self.prefix(self.containers.len(), true);
        spans.extend(content);
        self.out.push(Line::from(spans));
    }
}

/// Greedy word wrap of inline tokens into lines of at most `avail` columns.
fn wrap_tokens(toks: Vec<Tok>, avail: usize) -> Vec<Vec<Part>> {
    let avail = avail.max(1);
    let mut lines = Vec::new();
    let mut cur: Vec<Part> = Vec::new();
    let mut cur_w = 0usize;
    let mut space = false;
    for tok in toks {
        match tok {
            Tok::Space => space = cur_w > 0,
            Tok::Break => {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
                space = false;
            }
            Tok::Word { parts, width } => {
                let sp = usize::from(space && cur_w > 0);
                space = false;
                if cur_w + sp + width <= avail {
                    if sp == 1 {
                        cur.push((" ".to_string(), Style::default()));
                    }
                    cur.extend(parts);
                    cur_w += sp + width;
                    continue;
                }
                if cur_w > 0 {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                if width <= avail {
                    cur = parts;
                    cur_w = width;
                    continue;
                }
                for (text, st) in parts {
                    for ch in text.chars() {
                        let cw = ch.width().unwrap_or(0);
                        if cur_w + cw > avail && cur_w > 0 {
                            lines.push(std::mem::take(&mut cur));
                            cur_w = 0;
                        }
                        match cur.last_mut() {
                            Some((s, last)) if *last == st => s.push(ch),
                            _ => cur.push((ch.to_string(), st)),
                        }
                        cur_w += cw;
                    }
                }
            }
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Replace characters that would corrupt the terminal when written into a cell.
///
/// Tabs expand to spaces, carriage returns are dropped and every other control
/// character (including ESC) becomes `�`. Newlines are preserved.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => out.push('\n'),
            '\t' => out.extend(std::iter::repeat_n(' ', TAB_WIDTH)),
            '\r' => {}
            c if c.is_control() => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out
}

/// Sanitize `text` and greedily word-wrap every line to at most `width` columns.
///
/// Words longer than `width` are hard-broken. An empty input yields one empty line.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let clean = sanitize(text);
    let mut out = Vec::new();
    for line in clean.split('\n') {
        wrap_line(line, width, &mut out);
    }
    out
}

/// Display width of a string in terminal columns.
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate `s` to at most `width` columns, appending `…` when shortened.
pub fn truncate_to_width(s: &str, width: usize) -> String {
    if display_width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw + 1 > width {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

/// Truncate `s` to at most `max_chars` characters (char-boundary safe), appending `...`.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((byte, _)) => format!("{}...", &s[..byte]),
        None => s.to_string(),
    }
}

fn push_trimmed(out: &mut Vec<String>, cur: &mut String) {
    let trimmed_len = cur.trim_end_matches(' ').len();
    cur.truncate(trimmed_len);
    out.push(std::mem::take(cur));
}

fn wrap_line(line: &str, width: usize, out: &mut Vec<String>) {
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in line.split_inclusive(' ') {
        let word_w = display_width(word);
        let visible_w = display_width(word.trim_end_matches(' '));
        if cur_w + visible_w <= width {
            cur.push_str(word);
            cur_w += word_w;
            continue;
        }
        if cur.trim_end_matches(' ').is_empty() {
            cur.clear();
        } else {
            push_trimmed(out, &mut cur);
        }
        cur_w = 0;
        if visible_w > width {
            for ch in word.chars() {
                let cw = ch.width().unwrap_or(0);
                if cur_w + cw > width && !cur.is_empty() {
                    push_trimmed(out, &mut cur);
                    cur_w = 0;
                }
                cur.push(ch);
                cur_w += cw;
            }
        } else {
            cur.push_str(word);
            cur_w = word_w;
        }
    }
    push_trimmed(out, &mut cur);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn wraps_on_word_boundaries() {
        assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
    }

    #[test]
    fn hard_breaks_long_words() {
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn keeps_blank_lines_and_leading_indent() {
        assert_eq!(wrap_text("a\n\n  b", 10), vec!["a", "", "  b"]);
    }

    #[test]
    fn sanitizes_control_characters() {
        assert_eq!(sanitize("a\tb\r\n\x1b[31m"), "a    b\n\u{fffd}[31m");
    }

    #[test]
    fn wide_characters_respect_width() {
        let lines = wrap_text("日本語テキスト", 6);
        assert!(lines.iter().all(|l| display_width(l) <= 6));
        assert_eq!(lines.concat(), "日本語テキスト");
    }

    #[test]
    fn truncation_helpers() {
        assert_eq!(truncate_to_width("abcdef", 4), "abc…");
        assert_eq!(truncate_to_width("abc", 4), "abc");
        assert_eq!(truncate_chars("héllo", 2), "hé...");
        assert_eq!(truncate_chars("hi", 2), "hi");
    }

    #[test]
    fn render_markdown_wraps() {
        assert_eq!(render_markdown("one two three", 7).len(), 2);
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn md(src: &str, width: u16) -> Vec<String> {
        texts(&render_markdown(src, width))
    }

    fn span_style(lines: &[Line<'_>], needle: &str) -> Style {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains(needle))
            .map(|s| s.style)
            .unwrap_or_else(|| panic!("no span containing {needle:?}"))
    }

    #[test]
    fn headings_are_styled_and_distinguished() {
        let lines = render_markdown("# Title\n\n## Sub\n\n### Small\n\nbody", 40);
        assert_eq!(
            texts(&lines),
            vec!["Title", "", "Sub", "", "Small", "", "body"]
        );
        let h1 = span_style(&lines, "Title");
        let h2 = span_style(&lines, "Sub");
        assert!(h1.add_modifier.contains(Modifier::BOLD));
        assert!(h2.add_modifier.contains(Modifier::BOLD));
        assert_ne!(h1, h2);
        assert_eq!(h1, style::H1);
        assert_eq!(h2, style::H2);
    }

    #[test]
    fn paragraph_wraps_at_width_20() {
        let out = md(
            "The quick brown fox jumps over the lazy dog and keeps running far away.",
            20,
        );
        assert_eq!(
            out,
            vec![
                "The quick brown fox",
                "jumps over the lazy",
                "dog and keeps",
                "running far away.",
            ]
        );
        assert!(out.iter().all(|l| display_width(l) <= 20));
    }

    #[test]
    fn inline_styles() {
        let lines = render_markdown("a *em* **strong** ~~gone~~ `code here`", 80);
        assert_eq!(texts(&lines), vec!["a em strong gone code here"]);
        assert!(
            span_style(&lines, "em")
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            span_style(&lines, "strong")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            span_style(&lines, "gone")
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
        assert_eq!(span_style(&lines, "code here").fg, style::INLINE_CODE.fg);
    }

    #[test]
    fn nested_lists_indent() {
        let out = md(
            "- one\n  - inner a\n  - inner b\n- two\n\n1. first\n2. second",
            40,
        );
        assert_eq!(
            out,
            vec![
                "• one",
                "  ◦ inner a",
                "  ◦ inner b",
                "• two",
                "",
                "1. first",
                "2. second",
            ]
        );
    }

    #[test]
    fn list_items_wrap_under_their_text() {
        let out = md("- alpha beta gamma delta", 12);
        assert_eq!(out, vec!["• alpha beta", "  gamma", "  delta"]);
    }

    #[test]
    fn fenced_code_is_not_wrapped_and_keeps_indentation() {
        let src = "```rust\nfn main() {\n    let long_variable_name = 1234567890;\n}\n```\nafter";
        let lines = render_markdown(src, 20);
        let out = texts(&lines);
        assert_eq!(
            out,
            vec![
                "▎ fn main() {",
                "▎     let long_variable_name = 1234567890;",
                "▎ }",
                "",
                "after",
            ]
        );
        assert_eq!(span_style(&lines, "fn main").fg, style::CODE_BLOCK.fg);
    }

    #[test]
    fn indented_code_block() {
        assert_eq!(
            md("text\n\n    indented\n      more", 40),
            vec!["text", "", "▎ indented", "▎   more"]
        );
    }

    #[test]
    fn unclosed_fence_while_streaming_does_not_panic() {
        let full =
            "Intro **bold\n\n```py\nprint('x')\n  y = [1,\n\n> quote *open\n| a | b |\n|---|";
        for end in 0..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            for w in [0, 1, 5, 40] {
                assert!(!render_markdown(&full[..end], w).is_empty());
            }
        }
        let out = md("```\nstill streaming", 40);
        assert_eq!(out, vec!["▎ still streaming"]);
    }

    #[test]
    fn links_show_text_and_dim_url() {
        let lines = render_markdown("see [docs](https://x.dev/a) and <https://y.dev>", 80);
        assert_eq!(
            texts(&lines),
            vec!["see docs (https://x.dev/a) and https://y.dev"]
        );
        assert!(
            span_style(&lines, "(https://x.dev/a)")
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert_eq!(span_style(&lines, "docs").fg, style::LINK.fg);
    }

    #[test]
    fn block_quote_rule_and_table() {
        let out = md(
            "> quoted text\n\n---\n\n| a | bb |\n|---|---|\n| 1 | 2 |",
            20,
        );
        assert_eq!(
            out,
            vec![
                "│ quoted text",
                "",
                "────────────────────",
                "",
                "a │ bb",
                "──┼───",
                "1 │ 2",
            ]
        );
        let lines = render_markdown("> q", 20);
        assert!(span_style(&lines, "q").add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn zero_width_never_panics() {
        for src in ["", "word", "# h\n- a\n  - b\n\n```\ncode\n```", "> > deep"] {
            let lines = render_markdown(src, 0);
            assert!(!lines.is_empty());
        }
        assert_eq!(md("ab", 0), vec!["a", "b"]);
    }

    #[test]
    fn soft_breaks_are_kept_and_control_chars_sanitized() {
        assert_eq!(md("one\ntwo", 40), vec!["one", "two"]);
        assert_eq!(md("a\u{1b}[31mb", 40), vec!["a\u{fffd}[31mb"]);
    }
}
