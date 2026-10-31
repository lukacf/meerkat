//! The structured presentation of a rendering: a card of labelled fields,
//! lists (the consequences among them) and quoted free text.
//!
//! The card is parsed from the canonical rendering, never from the request
//! JSON, so it can show nothing the subject digest does not cover. Parsing
//! accepts exactly the `render_v1` layout and value encoding, and refuses
//! anything else.
//!
//! Every form the card offers for display is the canonical text itself:
//! quoted values keep their quotes and escapes, and text lines keep their
//! escapes, exactly as `render_v1` wrote them. So a hidden character, a line
//! break or markup in a value shows as its visible escape, never raw. The
//! value a segment or a text stands for is behind the separately named
//! `*_not_for_display` accessors, for comparing with a request only.
//!
//! Every byte of the rendering belongs to one block of the card, and a
//! block's display lines are exactly its bytes. So a presenter that
//! displays every block covers every canonical part, and the coverage of a
//! presentation that leaves blocks out counts only the blocks it displayed.

use std::fmt;
use std::num::NonZeroU32;
use std::ops::Range;

use super::escape::{is_markup, unescape_line, unquote};
use super::parts::{CanonicalPart, canonical_parts, part_ranges};
use super::render::labels;

/// One segment of a field's or a list item's value, exactly as `render_v1`
/// wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueSegment {
    /// A word the renderer wrote: a keyword, a number or a digest.
    Word(String),
    /// A quoted value as the renderer wrote it: its quotes, its escaped
    /// markup and its `\u{XXXX}` escapes. This is the form to display.
    Quoted(String),
}

impl ValueSegment {
    /// The segment as the renderer wrote it: the form to display.
    pub fn shown(&self) -> &str {
        let (Self::Word(text) | Self::Quoted(text)) = self;
        text
    }
}

/// A field's or a list item's value: words and quoted values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardValue {
    segments: Vec<ValueSegment>,
}

impl CardValue {
    /// The value's segments, as the renderer wrote them.
    pub fn segments(&self) -> &[ValueSegment] {
        &self.segments
    }

    /// The value's one word, when it is exactly one word.
    pub fn word(&self) -> Option<&str> {
        match self.segments.as_slice() {
            [ValueSegment::Word(word)] => Some(word),
            _ => None,
        }
    }

    /// The value's one quoted segment as the renderer wrote it, with its
    /// quotes and escapes, when it is exactly one: the form to display.
    pub fn quoted(&self) -> Option<&str> {
        match self.segments.as_slice() {
            [ValueSegment::Quoted(text)] => Some(text),
            _ => None,
        }
    }

    /// The text the one quoted segment stands for, decoded: the exact value
    /// the request carries. Not for display: decoding brings back the
    /// hidden characters, line breaks and markup the rendering escaped. For
    /// comparing with a request only.
    pub fn decoded_not_for_display(&self) -> Option<String> {
        self.quoted().and_then(unquote).map(|(decoded, _)| decoded)
    }
}

/// The value as the renderer wrote it: quoted segments with their quotes
/// and escapes.
impl fmt::Display for CardValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, segment) in self.segments.iter().enumerate() {
            if index > 0 {
                formatter.write_str(" ")?;
            }
            formatter.write_str(segment.shown())?;
        }
        Ok(())
    }
}

/// One entry of a card.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CardEntry {
    /// A labelled field.
    Field {
        /// The label.
        label: String,
        /// The value.
        value: CardValue,
    },
    /// A labelled list.
    List {
        /// The label.
        label: String,
        /// The items, in order.
        items: Vec<CardValue>,
    },
    /// Labelled free text, line by line.
    Text {
        /// The label.
        label: String,
        /// The lines as the renderer wrote them, with their escapes: the
        /// form to display. An empty line is empty.
        lines: Vec<String>,
    },
}

impl CardEntry {
    /// The entry's label.
    pub fn label(&self) -> &str {
        match self {
            Self::Field { label, .. } | Self::List { label, .. } | Self::Text { label, .. } => {
                label
            }
        }
    }

    /// The entry's lines exactly as the renderer wrote them: what a
    /// presenter displays.
    pub fn display_lines(&self) -> Vec<String> {
        match self {
            Self::Field { label, value } => vec![format!("{label}: {value}")],
            Self::List { label, items } => {
                let mut lines = Vec::with_capacity(items.len() + 1);
                lines.push(format!("{label} ({}):", items.len()));
                lines.extend(items.iter().map(|item| format!("- {item}")));
                lines
            }
            Self::Text { label, lines } => {
                let mut shown = Vec::with_capacity(lines.len() + 1);
                let unit = if lines.len() == 1 { "line" } else { "lines" };
                shown.push(format!("{label} ({} {unit}):", lines.len()));
                shown.extend(lines.iter().map(|line| {
                    if line.is_empty() {
                        ">".to_owned()
                    } else {
                        format!("> {line}")
                    }
                }));
                shown
            }
        }
    }

    /// A text entry's text, decoded, its lines joined with line breaks: the
    /// exact text the request carries. Not for display (see
    /// `CardValue::decoded_not_for_display`). `None` for a field or a list.
    pub fn decoded_text_not_for_display(&self) -> Option<String> {
        let Self::Text { lines, .. } = self else {
            return None;
        };
        let decoded: Option<Vec<String>> = lines.iter().map(|line| unescape_line(line)).collect();
        decoded.map(|lines| lines.join("\n"))
    }
}

/// One block of a card: its entries, the first being its heading, and the
/// bytes of the rendering it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardBlock {
    entries: Vec<CardEntry>,
    bytes: Range<usize>,
}

impl CardBlock {
    /// The block's entries.
    pub fn entries(&self) -> &[CardEntry] {
        &self.entries
    }

    /// The block's heading: its first entry.
    pub fn heading(&self) -> Option<&CardEntry> {
        self.entries.first()
    }

    /// The bytes of the rendering the block shows, the empty line after it
    /// included.
    pub fn bytes(&self) -> Range<usize> {
        self.bytes.clone()
    }

    /// The block's lines exactly as the renderer wrote them, without the
    /// empty line after the block: what a presenter displays.
    pub fn display_lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .flat_map(CardEntry::display_lines)
            .collect()
    }
}

/// Why a text is not a canonical rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CardError {
    /// The line, from 1; 0 for the text as a whole.
    pub line: usize,
    /// What is wrong.
    pub reason: &'static str,
}

impl CardError {
    fn at(line: usize, reason: &'static str) -> Self {
        Self { line, reason }
    }
}

impl fmt::Display for CardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "not a canonical identity rendering (line {}): {}",
            self.line, self.reason
        )
    }
}

impl std::error::Error for CardError {}

/// A structured card, parsed from a canonical rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredCard {
    rendering: String,
    blocks: Vec<CardBlock>,
}

impl StructuredCard {
    /// Parse a canonical rendering into its card.
    pub fn parse(rendering: &str) -> Result<Self, CardError> {
        if !rendering.ends_with('\n') {
            return Err(CardError::at(0, "the last line has no line break"));
        }
        let lines = lines_with_offsets(rendering);
        let mut blocks = Vec::new();
        let mut entries = Vec::new();
        let mut block_start = 0;
        let mut index = 0;
        while let Some(&(start, line)) = lines.get(index) {
            if line.is_empty() {
                let separates = !entries.is_empty()
                    && lines
                        .get(index + 1)
                        .is_some_and(|(_, next)| !next.is_empty());
                if !separates {
                    return Err(CardError::at(
                        index + 1,
                        "an empty line that does not separate two blocks",
                    ));
                }
                let end = start + 1;
                blocks.push(CardBlock {
                    entries: std::mem::take(&mut entries),
                    bytes: block_start..end,
                });
                block_start = end;
                index += 1;
                continue;
            }
            let (entry, consumed) = parse_entry(&lines, index)?;
            entries.push(entry);
            index += consumed;
        }
        if entries.is_empty() {
            return Err(CardError::at(0, "the rendering has no block"));
        }
        blocks.push(CardBlock {
            entries,
            bytes: block_start..rendering.len(),
        });
        let titled =
            blocks
                .first()
                .and_then(CardBlock::heading)
                .is_some_and(|heading| match heading {
                    CardEntry::Field { label, value } => {
                        label == labels::TITLE && value.word() == Some(labels::VERSION)
                    }
                    CardEntry::List { .. } | CardEntry::Text { .. } => false,
                });
        if !titled {
            return Err(CardError::at(
                1,
                "the first line is not the render-v1 title",
            ));
        }
        Ok(Self {
            rendering: rendering.to_owned(),
            blocks,
        })
    }

    /// The card's blocks, in order.
    pub fn blocks(&self) -> &[CardBlock] {
        &self.blocks
    }

    /// The canonical parts a presentation of the whole card covers, for the
    /// part size `max_part_bytes`: every part.
    pub fn covered_parts(&self, max_part_bytes: Option<NonZeroU32>) -> Vec<CanonicalPart> {
        let every: Vec<usize> = (0..self.blocks.len()).collect();
        self.covered_parts_of(&every, max_part_bytes)
    }

    /// The canonical parts a presentation that displayed the blocks at
    /// `shown` covers, for the part size `max_part_bytes`: those whose every
    /// byte belongs to a displayed block. Blocks it did not display count
    /// for nothing.
    pub fn covered_parts_of(
        &self,
        shown: &[usize],
        max_part_bytes: Option<NonZeroU32>,
    ) -> Vec<CanonicalPart> {
        let shown: Vec<Range<usize>> = shown
            .iter()
            .filter_map(|index| self.blocks.get(*index))
            .map(CardBlock::bytes)
            .collect();
        part_ranges(&self.rendering, max_part_bytes)
            .into_iter()
            .zip(canonical_parts(&self.rendering, max_part_bytes))
            .filter(|(range, _)| is_covered(range, &shown))
            .map(|(_, part)| part)
            .collect()
    }
}

fn lines_with_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        lines.push((start, line.strip_suffix('\n').unwrap_or(line)));
        start += line.len();
    }
    lines
}

fn is_covered(range: &Range<usize>, shown: &[Range<usize>]) -> bool {
    let mut position = range.start;
    while position < range.end {
        let reach = shown
            .iter()
            .filter(|block| block.start <= position && position < block.end)
            .map(|block| block.end)
            .max();
        match reach {
            Some(end) => position = end,
            None => return false,
        }
    }
    true
}

/// A label: ASCII letters, digits and single inner spaces.
fn is_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with(' ')
        && !label.ends_with(' ')
        && !label.contains("  ")
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b' ')
}

/// A count as the renderer writes it: decimal, no leading zero.
fn parse_count(text: &str) -> Option<usize> {
    let count: usize = text.parse().ok()?;
    (count.to_string() == text).then_some(count)
}

fn is_word_char(character: char) -> bool {
    character.is_ascii_graphic() && character != '"' && character != '\\' && !is_markup(character)
}

/// A field's or an item's value: words and quoted values separated by single
/// spaces.
fn parse_value(text: &str) -> Option<CardValue> {
    let mut segments = Vec::new();
    let mut rest = text;
    loop {
        if rest.starts_with('"') {
            let (_, after) = unquote(rest)?;
            let shown = rest.get(..rest.len() - after.len())?;
            segments.push(ValueSegment::Quoted(shown.to_owned()));
            rest = after;
        } else {
            let end = rest.find(' ').unwrap_or(rest.len());
            let word = rest.get(..end)?;
            if word.is_empty() || !word.chars().all(is_word_char) {
                return None;
            }
            segments.push(ValueSegment::Word(word.to_owned()));
            rest = rest.get(end..)?;
        }
        if rest.is_empty() {
            return Some(CardValue { segments });
        }
        rest = rest.strip_prefix(' ')?;
        if rest.is_empty() {
            return None;
        }
    }
}

/// One text line: `>` for an empty line, else `> ` and the escaped line.
/// Returns the escaped line as written, once it decodes canonically.
fn parse_text_line(line: &str) -> Option<String> {
    if line == ">" {
        return Some(String::new());
    }
    let escaped = line.strip_prefix("> ")?;
    if escaped.is_empty() {
        return None;
    }
    unescape_line(escaped)?;
    Some(escaped.to_owned())
}

/// The entry at `lines[index]`, and how many lines it takes.
fn parse_entry(lines: &[(usize, &str)], index: usize) -> Result<(CardEntry, usize), CardError> {
    let number = index + 1;
    let Some(&(_, line)) = lines.get(index) else {
        return Err(CardError::at(number, "a missing line"));
    };
    if let Some((label, value)) = line.split_once(": ") {
        if !is_label(label) {
            return Err(CardError::at(number, "a field label that is not canonical"));
        }
        let value = parse_value(value)
            .ok_or_else(|| CardError::at(number, "a field value that is not canonical"))?;
        return Ok((
            CardEntry::Field {
                label: label.to_owned(),
                value,
            },
            1,
        ));
    }
    let Some((label, count)) = line
        .strip_suffix("):")
        .and_then(|head| head.rsplit_once(" ("))
    else {
        return Err(CardError::at(
            number,
            "a line that is no field, list or text",
        ));
    };
    if !is_label(label) {
        return Err(CardError::at(number, "a label that is not canonical"));
    }
    let singular = count.strip_suffix(" line");
    let plural = count.strip_suffix(" lines");
    if let Some(digits) = singular.or(plural) {
        let lines_count = parse_count(digits)
            .ok_or_else(|| CardError::at(number, "a line count that is not canonical"))?;
        if lines_count == 0 || (lines_count == 1) != singular.is_some() {
            return Err(CardError::at(number, "a line count that is not canonical"));
        }
        let mut text = Vec::with_capacity(lines_count);
        for offset in 1..=lines_count {
            let shown = lines
                .get(index + offset)
                .and_then(|&(_, text_line)| parse_text_line(text_line))
                .ok_or_else(|| {
                    CardError::at(number + offset, "a text line that is not canonical")
                })?;
            text.push(shown);
        }
        return Ok((
            CardEntry::Text {
                label: label.to_owned(),
                lines: text,
            },
            lines_count + 1,
        ));
    }
    let items_count = parse_count(count)
        .ok_or_else(|| CardError::at(number, "an item count that is not canonical"))?;
    let mut items = Vec::with_capacity(items_count);
    for offset in 1..=items_count {
        let item = lines
            .get(index + offset)
            .and_then(|&(_, item_line)| item_line.strip_prefix("- "))
            .and_then(parse_value)
            .ok_or_else(|| CardError::at(number + offset, "a list item that is not canonical"))?;
        items.push(item);
    }
    Ok((
        CardEntry::List {
            label: label.to_owned(),
            items,
        },
        items_count + 1,
    ))
}
