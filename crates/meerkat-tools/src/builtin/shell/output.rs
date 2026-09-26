//! Bounded capture and compact rendering of shell command output.
//!
//! Foreground calls and background jobs share one policy. Each stream keeps
//! its head and its tail within a character cap. A cut moves to a line
//! boundary when the line it lands in fits the cap, so neither side starts or
//! ends mid-line. The marker between the two sides names the omitted lines,
//! the line where the head ends and the line where the tail starts, so the
//! model can page the middle with a narrower command.

use std::collections::VecDeque;

use tokio::io::AsyncReadExt;

/// Character caps for one command's streams: stdout gets the configured cap
/// and stderr half of it. Each keeps at least one character per side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputCaps {
    pub(super) stdout_chars: usize,
    pub(super) stderr_chars: usize,
}

impl OutputCaps {
    pub(super) fn from_max_output_chars(max_output_chars: usize) -> Self {
        let stdout_chars = max_output_chars.max(2);
        Self {
            stdout_chars,
            stderr_chars: (stdout_chars / 2).max(2),
        }
    }
}

/// Bytes captured per side of a stream for a character cap: a UTF-8
/// character is at most four bytes.
pub(super) fn capture_bytes_for_chars(max_chars: usize) -> usize {
    max_chars.saturating_mul(4)
}

fn is_utf8_continuation(byte: u8) -> bool {
    (byte & 0b1100_0000) == 0b1000_0000
}

/// Drop leading UTF-8 continuation bytes so a tail cut mid-character decodes
/// cleanly.
fn skip_utf8_continuation(bytes: &[u8]) -> &[u8] {
    let skip = bytes
        .iter()
        .take(3)
        .take_while(|byte| is_utf8_continuation(**byte))
        .count();
    &bytes[skip..]
}

/// Drop a character that a cut left incomplete at the end of `bytes`, so a
/// head cut mid-character decodes cleanly. Invalid bytes are left alone.
fn trim_partial_utf8_suffix(bytes: &[u8]) -> &[u8] {
    for back in 1..=bytes.len().min(4) {
        let index = bytes.len() - back;
        let byte = bytes[index];
        if is_utf8_continuation(byte) {
            continue;
        }
        let width = match byte {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => return bytes,
        };
        return if width > back { &bytes[..index] } else { bytes };
    }
    bytes
}

struct TailBuffer {
    buffer: VecDeque<u8>,
    max_bytes: usize,
}

impl TailBuffer {
    fn new(max_bytes: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            max_bytes,
        }
    }

    fn extend(&mut self, data: &[u8]) {
        if self.max_bytes == 0 {
            return;
        }

        if data.len() >= self.max_bytes {
            self.buffer.clear();
            self.buffer
                .extend(data[data.len() - self.max_bytes..].iter().copied());
            return;
        }

        let overflow = (self.buffer.len() + data.len()).saturating_sub(self.max_bytes);
        if overflow > 0 {
            self.buffer.drain(0..overflow);
        }
        self.buffer.extend(data.iter().copied());
    }

    fn into_vec(self) -> Vec<u8> {
        self.buffer.into_iter().collect()
    }
}

/// Incremental capture of one stream: its first and last `side_bytes` bytes,
/// plus totals counted over every byte. Memory stays bounded however long
/// the stream runs.
pub(super) struct HeadTailCapture {
    side_bytes: usize,
    head: Vec<u8>,
    tail: TailBuffer,
    total_bytes: u64,
    total_chars: u64,
    total_newlines: u64,
    last_byte: Option<u8>,
}

impl HeadTailCapture {
    pub(super) fn new(side_bytes: usize) -> Self {
        Self {
            side_bytes,
            head: Vec::new(),
            tail: TailBuffer::new(side_bytes),
            total_bytes: 0,
            total_chars: 0,
            total_newlines: 0,
            last_byte: None,
        }
    }

    pub(super) fn push(&mut self, data: &[u8]) {
        let Some(&last_byte) = data.last() else {
            return;
        };
        self.total_bytes = self.total_bytes.saturating_add(data.len() as u64);
        // One pass counts characters (UTF-8 lead bytes) and line ends.
        let (chars, newlines) = data.iter().fold((0u64, 0u64), |(chars, newlines), byte| {
            (
                chars + u64::from(!is_utf8_continuation(*byte)),
                newlines + u64::from(*byte == b'\n'),
            )
        });
        self.total_chars = self.total_chars.saturating_add(chars);
        self.total_newlines = self.total_newlines.saturating_add(newlines);
        self.last_byte = Some(last_byte);
        let head_room = self.side_bytes.saturating_sub(self.head.len());
        self.head
            .extend_from_slice(&data[..head_room.min(data.len())]);
        // The tail sees every byte, so it always holds the stream's last
        // `side_bytes`, even where they overlap the head.
        self.tail.extend(data);
    }

    pub(super) fn finish(self) -> CapturedStream {
        let cut = self.total_bytes > self.head.len() as u64;
        CapturedStream {
            head: self.head,
            tail: if cut {
                self.tail.into_vec()
            } else {
                Vec::new()
            },
            total_bytes: self.total_bytes,
            total_chars: self.total_chars,
            total_newlines: self.total_newlines,
            ends_with_newline: self.last_byte == Some(b'\n'),
        }
    }
}

/// Read a stream to its end, keeping its first and last `side_bytes` bytes
/// and its totals.
pub(super) async fn read_stream_head_tail<R>(
    mut reader: R,
    side_bytes: usize,
) -> std::io::Result<CapturedStream>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut capture = HeadTailCapture::new(side_bytes);
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        capture.push(&chunk[..read]);
    }
    Ok(capture.finish())
}

/// One stream's output as captured: the first bytes, the last bytes, and
/// totals over the whole stream. When the stream fit in `head`, `tail` is
/// empty. Characters are counted as UTF-8 lead bytes.
#[derive(Debug, Default)]
pub(super) struct CapturedStream {
    head: Vec<u8>,
    tail: Vec<u8>,
    total_bytes: u64,
    total_chars: u64,
    total_newlines: u64,
    ends_with_newline: bool,
}

impl CapturedStream {
    /// Whether the stream ran past the head buffer.
    fn is_cut(&self) -> bool {
        self.total_bytes > self.head.len() as u64
    }

    fn head_bytes(&self) -> &[u8] {
        if self.is_cut() {
            trim_partial_utf8_suffix(&self.head)
        } else {
            &self.head
        }
    }

    fn tail_bytes(&self) -> &[u8] {
        skip_utf8_continuation(&self.tail)
    }

    /// Whether the captured bytes hold invalid UTF-8. A character split by a
    /// capture cut is not invalid output and does not count.
    pub(super) fn lossy(&self) -> bool {
        std::str::from_utf8(self.head_bytes()).is_err()
            || std::str::from_utf8(self.tail_bytes()).is_err()
    }

    fn total_lines(&self) -> u64 {
        self.total_newlines
            .saturating_add(u64::from(self.total_bytes > 0 && !self.ends_with_newline))
    }

    /// The stream bounded to `max_chars` characters of output.
    ///
    /// Output within the cap is returned whole. Longer output keeps up to
    /// half the cap from its start and the rest from its end, with a marker
    /// between them. Losing the head of a diff or a file is as harmful as
    /// losing its tail, so neither end is dropped.
    pub(super) fn bounded_text(&self, max_chars: usize) -> String {
        let head_text = String::from_utf8_lossy(self.head_bytes());
        if !self.is_cut() && head_text.chars().count() <= max_chars {
            return head_text.into_owned();
        }
        let head_budget = max_chars / 2;
        let tail_budget = max_chars - head_budget;
        let tail_text;
        let (tail_source, tail_source_starts_stream) = if self.is_cut() {
            tail_text = String::from_utf8_lossy(self.tail_bytes());
            (tail_text.as_ref(), false)
        } else {
            (head_text.as_ref(), true)
        };
        let head = cut_head(&head_text, head_budget, !self.is_cut());
        let (tail, tail_starts_mid_line) =
            cut_tail(tail_source, tail_budget, tail_source_starts_stream);
        let marker = self.omission_marker(head, tail, tail_starts_mid_line);
        let separator = if head.is_empty() || head.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        format!("{head}{separator}{marker}\n{tail}")
    }

    fn omission_marker(&self, head: &str, tail: &str, tail_starts_mid_line: bool) -> String {
        let total_lines = self.total_lines();
        let head_ends_mid_line = !head.is_empty() && !head.ends_with('\n');
        let head_end_line = count_newlines(head) + u64::from(head_ends_mid_line);
        let tail_lines =
            count_newlines(tail) + u64::from(!tail.is_empty() && !tail.ends_with('\n'));
        let tail_start_line = total_lines.saturating_sub(tail_lines) + 1;
        let first_omitted = if head_ends_mid_line {
            head_end_line
        } else {
            head_end_line + 1
        };
        let last_omitted = if tail_starts_mid_line {
            tail_start_line
        } else {
            tail_start_line.saturating_sub(1)
        }
        .max(first_omitted);
        let shown_chars = (head.chars().count() + tail.chars().count()) as u64;
        let omitted_chars = self.total_chars.saturating_sub(shown_chars);
        let total_chars = self.total_chars;
        let omitted = line_range(first_omitted, last_omitted);
        if !head_ends_mid_line && !tail_starts_mid_line {
            let omitted_lines = last_omitted - first_omitted + 1;
            let lines = if omitted_lines == 1 { "line" } else { "lines" };
            return format!(
                "[... {omitted} of {total_lines} omitted ({omitted_lines} {lines}, {omitted_chars} of {total_chars} characters); the head ends at line {head_end_line} and the tail starts at line {tail_start_line}. To see them, re-run the command piped through `sed -n '{first_omitted},{last_omitted}p'`, or narrow it with `grep -n`. ...]"
            );
        }
        let head_end = if head_ends_mid_line { "inside" } else { "at" };
        let tail_start = if tail_starts_mid_line { "inside" } else { "at" };
        format!(
            "[... {omitted_chars} of {total_chars} characters omitted from {omitted} of {total_lines}; the head ends {head_end} line {head_end_line} and the tail starts {tail_start} line {tail_start_line}, because a line there is longer than the cap. To see the omitted part, narrow the command, for example with `grep -n PATTERN`, or split long lines with `fold -w 200`. ...]"
        )
    }
}

fn count_newlines(text: &str) -> u64 {
    text.matches('\n').count() as u64
}

fn line_range(first: u64, last: u64) -> String {
    if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}-{last}")
    }
}

/// Byte offset just past the first `chars` characters of `text`.
fn byte_offset_after_chars(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(offset, _)| offset)
}

/// The shown head: at most `budget` characters from the start of `text`.
/// When the cut lands inside a line that itself fits the budget, the head
/// ends at the end of the previous line instead. `text_is_whole_stream`
/// says whether `text` runs to the end of the stream.
fn cut_head(text: &str, budget: usize, text_is_whole_stream: bool) -> &str {
    let cut = byte_offset_after_chars(text, budget);
    let shown = &text[..cut];
    if cut == text.len() || shown.is_empty() || shown.ends_with('\n') {
        return shown;
    }
    let Some(line_start) = shown.rfind('\n').map(|index| index + 1) else {
        // The first line alone is longer than the budget.
        return shown;
    };
    let line_end = match text[cut..].find('\n') {
        Some(index) => cut + index + 1,
        None if text_is_whole_stream => text.len(),
        // The line runs past the captured head, far beyond the budget.
        None => return shown,
    };
    if text[line_start..line_end].chars().count() <= budget {
        &text[..line_start]
    } else {
        shown
    }
}

/// The shown tail: at most `budget` characters from the end of `text`, and
/// whether it starts inside a line. When the cut lands inside a line that
/// itself fits the budget, the tail starts at the next line instead.
/// `text_starts_stream` says whether `text` begins at the stream's start.
fn cut_tail(text: &str, budget: usize, text_starts_stream: bool) -> (&str, bool) {
    let total = text.chars().count();
    if total <= budget {
        return (text, !text_starts_stream);
    }
    let start = byte_offset_after_chars(text, total - budget);
    let shown = &text[start..];
    if text[..start].ends_with('\n') {
        return (shown, false);
    }
    let Some(newline) = shown.find('\n') else {
        return (shown, true);
    };
    let next_line = start + newline + 1;
    if next_line == text.len() {
        // The last line alone is longer than the budget.
        return (shown, true);
    }
    let line_start = match text[..start].rfind('\n') {
        Some(index) => index + 1,
        None if text_starts_stream => 0,
        // The line starts before the captured tail, far beyond the budget.
        None => return (shown, true),
    };
    if text[line_start..next_line].chars().count() <= budget {
        (&text[next_line..], false)
    } else {
        (shown, true)
    }
}

/// The status phrase for a process that ran to an exit status.
pub(super) fn exit_status_phrase(exit_code: Option<i32>, duration_secs: f64) -> String {
    match exit_code {
        Some(code) => format!("exit code {code} ({duration_secs:.1}s)"),
        None => format!("terminated by a signal, no exit code ({duration_secs:.1}s)"),
    }
}

/// Append a command's streams to `text`: stdout as is, then stderr under a
/// `[stderr]` line, each only when non-empty, or `(no output)` when both are
/// empty.
pub(super) fn push_streams(text: &mut String, stdout: &str, stderr: &str) {
    if stdout.is_empty() && stderr.is_empty() {
        text.push_str("\n(no output)");
        return;
    }
    if !stdout.is_empty() {
        text.push('\n');
        text.push_str(stdout.trim_end_matches('\n'));
    }
    if !stderr.is_empty() {
        text.push_str("\n[stderr]\n");
        text.push_str(stderr.trim_end_matches('\n'));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn capture(bytes: &[u8], side_bytes: usize) -> CapturedStream {
        let mut capture = HeadTailCapture::new(side_bytes);
        // Feed in uneven chunks so the head/tail split does not line up with
        // a single write.
        for chunk in bytes.chunks(777) {
            capture.push(chunk);
        }
        capture.finish()
    }

    fn numbered_lines(range: std::ops::Range<usize>, line: impl Fn(usize) -> String) -> String {
        range.map(line).collect::<Vec<_>>().concat()
    }

    /// The text after the marker line.
    fn tail_of(bounded: &str) -> &str {
        let marker = bounded.find("[... ").unwrap();
        let after = &bounded[marker..];
        &after[after.find("...]\n").unwrap() + 5..]
    }

    /// The text before the marker line.
    fn head_of(bounded: &str) -> &str {
        &bounded[..bounded.find("[... ").unwrap()]
    }

    #[test]
    fn output_within_the_cap_is_returned_whole() {
        let captured = capture(b"short output\n", 1024);
        assert_eq!(captured.bounded_text(100), "short output\n");
        assert_eq!(CapturedStream::default().bounded_text(100), "");
    }

    #[test]
    fn long_output_keeps_both_ends_cut_at_line_boundaries() {
        let text = numbered_lines(0..1000, |i| format!("line {i}\n"));
        let captured = capture(text.as_bytes(), text.len());
        let bounded = captured.bounded_text(200);
        let head = head_of(&bounded);
        let tail = tail_of(&bounded);
        assert!(head.starts_with("line 0\nline 1\n"), "{bounded}");
        assert!(head.ends_with('\n'), "head must end on a line: {head:?}");
        assert!(
            tail.starts_with("line "),
            "tail must start a line: {tail:?}"
        );
        assert!(tail.ends_with("line 999\n"), "{bounded}");
        assert!(head.chars().count() <= 100 && tail.chars().count() <= 100);
    }

    #[test]
    fn marker_names_the_omitted_lines_and_where_each_side_stops() {
        // 1000 lines of 9 characters ("line NNN\n").
        let text = numbered_lines(0..1000, |i| format!("line {i:03}\n"));
        let captured = capture(text.as_bytes(), 256);
        assert!(captured.is_cut());
        let bounded = captured.bounded_text(100);
        // 50 characters per side hold 5 whole lines each.
        assert_eq!(
            head_of(&bounded),
            numbered_lines(0..5, |i| format!("line {i:03}\n"))
        );
        assert_eq!(
            tail_of(&bounded),
            numbered_lines(995..1000, |i| format!("line {i:03}\n"))
        );
        assert!(
            bounded.contains(
                "[... lines 6-995 of 1000 omitted (990 lines, 8910 of 9000 characters); \
                 the head ends at line 5 and the tail starts at line 996. \
                 To see them, re-run the command piped through `sed -n '6,995p'`"
            ),
            "{bounded}"
        );
    }

    #[test]
    fn marker_line_numbers_match_the_output_without_a_trailing_newline() {
        let mut text = numbered_lines(1..101, |i| format!("row {i:03}\n"));
        text.push_str("last");
        let captured = capture(text.as_bytes(), text.len());
        let bounded = captured.bounded_text(40);
        let tail = tail_of(&bounded);
        // The tail holds rows 99 and 100 and "last", which is line 101.
        assert_eq!(tail, "row 099\nrow 100\nlast");
        assert!(
            bounded.contains("[... lines 3-98 of 101 omitted (96 lines,"),
            "{bounded}"
        );
        assert!(
            bounded.contains("the head ends at line 2 and the tail starts at line 99."),
            "{bounded}"
        );
    }

    #[test]
    fn a_line_longer_than_the_cap_is_cut_mid_line_and_the_marker_says_so() {
        let text = format!("{}\n", "x".repeat(10_000));
        let captured = capture(text.as_bytes(), 1024);
        let bounded = captured.bounded_text(100);
        assert_eq!(head_of(&bounded), format!("{}\n", "x".repeat(50)));
        assert_eq!(tail_of(&bounded), format!("{}\n", "x".repeat(49)));
        assert!(
            bounded.contains(
                "[... 9901 of 10001 characters omitted from line 1 of 1; \
                 the head ends inside line 1 and the tail starts inside line 1"
            ),
            "{bounded}"
        );
    }

    #[test]
    fn a_long_line_at_the_cut_does_not_empty_either_side() {
        // Short lines, then one line longer than the cap straddling each cut.
        let long = "y".repeat(5_000);
        let text = format!("a\nb\n{long}\n{long}\nc\nd\n");
        let captured = capture(text.as_bytes(), text.len());
        let bounded = captured.bounded_text(100);
        let head = head_of(&bounded);
        let tail = tail_of(&bounded);
        assert!(head.starts_with("a\nb\nyyy"), "{head:?}");
        assert_eq!(head.trim_end_matches('\n').chars().count(), 50);
        assert!(tail.ends_with("yyy\nc\nd\n"), "{tail:?}");
        assert!(
            bounded.contains("the head ends inside line 3 and the tail starts inside line 4"),
            "{bounded}"
        );
    }

    #[test]
    fn capture_keeps_head_and_tail_of_a_stream_larger_than_both() {
        let text = numbered_lines(0..20_000, |i| format!("row {i}\n"));
        let captured = capture(text.as_bytes(), 400);
        assert_eq!(captured.total_bytes, text.len() as u64);
        assert_eq!(captured.head.len(), 400);
        assert_eq!(captured.tail.len(), 400);
        let bounded = captured.bounded_text(100);
        assert!(bounded.starts_with("row 0\n"), "{bounded}");
        assert!(bounded.ends_with("row 19999\n"), "{bounded}");
        assert!(bounded.contains("of 20000 omitted"), "{bounded}");
    }

    #[test]
    fn capture_fills_the_tail_when_the_stream_barely_exceeds_the_head() {
        // Regression: the tail used to receive only bytes past the head, so a
        // stream just over the cap showed a short tail.
        let text = numbered_lines(0..3000, |i| format!("{i:05}\n"));
        let captured = capture(text.as_bytes(), 16_000);
        assert_eq!(captured.tail.len(), 16_000);
        let bounded = captured.bounded_text(4_000);
        let tail = tail_of(&bounded);
        assert_eq!(tail.chars().count(), 1_998, "tail keeps whole lines");
        assert!(tail.ends_with("02999\n"));
    }

    #[tokio::test]
    async fn stream_reader_leaves_tail_empty_when_the_stream_fits() {
        let captured = read_stream_head_tail(&b"small"[..], 16).await.unwrap();
        assert!(captured.tail.is_empty());
        assert_eq!(captured.bounded_text(100), "small");
    }

    #[test]
    fn head_tail_never_splits_multibyte_characters() {
        let text = "🎉".repeat(500);
        for max_chars in [2, 3, 10, 99] {
            let captured = capture(text.as_bytes(), text.len());
            let bounded = captured.bounded_text(max_chars);
            assert!(!bounded.contains('\u{FFFD}'), "{bounded}");
        }
        // A tail captured mid-character drops the partial prefix.
        let captured = capture(text.as_bytes(), 9);
        assert!(!captured.lossy());
        assert!(!captured.bounded_text(4).contains('\u{FFFD}'));
    }

    #[test]
    fn valid_utf8_cut_mid_character_at_the_head_is_not_lossy() {
        // Regression: the head buffer is cut at a byte count, which can land
        // inside a three-byte box-drawing character. Valid output was then
        // flagged as invalid UTF-8.
        let text = "\u{251c}\u{2500}\u{2500} dep v1.0.0\n".repeat(20_000);
        let side_bytes = capture_bytes_for_chars(40_000);
        assert!(text.len() > side_bytes);
        let captured = capture(text.as_bytes(), side_bytes);
        assert!(
            std::str::from_utf8(&captured.head).is_err(),
            "fixture must split a character at the head cut"
        );
        assert!(!captured.lossy(), "valid UTF-8 must not be flagged lossy");
        let bounded = captured.bounded_text(40_000);
        assert!(!bounded.contains('\u{FFFD}'));
    }

    #[test]
    fn genuinely_invalid_utf8_is_still_lossy() {
        let mut bytes = b"ok ".to_vec();
        bytes.push(0xFF);
        bytes.extend_from_slice(b" done\n");
        assert!(capture(&bytes, 1024).lossy());
        // A truncated character at the true end of a stream that fit is
        // invalid output, not a capture cut.
        let mut truncated = "box \u{251c}".as_bytes().to_vec();
        truncated.pop();
        assert!(capture(&truncated, 1024).lossy());
    }

    #[test]
    fn trim_partial_utf8_suffix_drops_only_an_incomplete_last_character() {
        let text = "a\u{251c}";
        let bytes = text.as_bytes();
        assert_eq!(trim_partial_utf8_suffix(bytes), bytes);
        assert_eq!(trim_partial_utf8_suffix(&bytes[..bytes.len() - 1]), b"a");
        assert_eq!(trim_partial_utf8_suffix(&bytes[..2]), b"a");
        assert_eq!(trim_partial_utf8_suffix(b"plain"), b"plain");
        assert_eq!(trim_partial_utf8_suffix(&[0xFF_u8]), &[0xFF_u8]);
        assert_eq!(trim_partial_utf8_suffix(&[]), &[] as &[u8]);
    }

    #[test]
    fn output_caps_give_stderr_half_and_never_zero() {
        assert_eq!(
            OutputCaps::from_max_output_chars(40_000),
            OutputCaps {
                stdout_chars: 40_000,
                stderr_chars: 20_000,
            }
        );
        let tiny = OutputCaps::from_max_output_chars(0);
        assert!(tiny.stdout_chars >= 2 && tiny.stderr_chars >= 2);
    }

    #[test]
    fn push_streams_renders_compactly() {
        let mut text = String::from("status");
        push_streams(&mut text, "out\n", "err\n");
        assert_eq!(text, "status\nout\n[stderr]\nerr");
        let mut empty = String::from("status");
        push_streams(&mut empty, "", "");
        assert_eq!(empty, "status\n(no output)");
    }
}
