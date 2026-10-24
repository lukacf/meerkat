use crate::error::LlmError;
use crate::types::{LlmDoneOutcome, LlmEvent, LlmStream};
use futures::StreamExt;

pub fn ensure_terminal_done(mut stream: LlmStream<'_>) -> LlmStream<'_> {
    Box::pin(async_stream::stream! {
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    if matches!(event, LlmEvent::Done { .. }) {
                        yield Ok(event);
                        return;
                    }
                    yield Ok(event);
                }
                Err(error) => {
                    yield Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Error { error },
                    });
                    return;
                }
            }
        }

        yield Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Error {
                error: LlmError::IncompleteResponse {
                    message: "Stream ended without Done event".to_string(),
                },
            },
        });
    })
}

/// Splits a streamed response body into lines and decodes each complete
/// line as strict UTF-8.
///
/// Network chunks can end anywhere, including inside a multi-byte
/// character, so a chunk is never decoded on its own: its bytes are kept
/// until a line is complete, and only whole lines are decoded. A complete
/// line that is not valid UTF-8 is a [`LlmError::StreamParseError`];
/// nothing is ever replaced with U+FFFD.
#[derive(Debug, Default)]
pub struct Utf8LineBuffer {
    bytes: Vec<u8>,
    /// How many leading bytes are known to hold no `\n`. The next search
    /// starts there, so a long line that arrives in many chunks has each of
    /// its bytes searched once.
    scanned: usize,
}

impl Utf8LineBuffer {
    /// An empty buffer with room for `capacity` bytes.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            scanned: 0,
        }
    }

    /// Append one network chunk.
    pub fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
    }

    /// The next complete line, without its `\n`, or `None` while no
    /// complete line is buffered.
    pub fn next_line(&mut self) -> Option<Result<String, LlmError>> {
        let Some(offset) = self.bytes[self.scanned..]
            .iter()
            .position(|byte| *byte == b'\n')
        else {
            self.scanned = self.bytes.len();
            return None;
        };
        let end = self.scanned + offset;
        self.scanned = 0;
        let mut line: Vec<u8> = self.bytes.drain(..=end).collect();
        line.pop();
        Some(decode_stream_line(line))
    }

    /// At the end of the stream: the bytes after the last `\n`, decoded
    /// strictly as one final line, or `None` when nothing is left. A
    /// character cut off by the end of the stream is an error, like any
    /// other invalid line.
    pub fn finish(&mut self) -> Result<Option<String>, LlmError> {
        let rest = self.take_partial_line();
        if rest.is_empty() {
            return Ok(None);
        }
        decode_stream_line(rest).map(Some)
    }

    /// At the end of the stream, for a reader that only reports what was
    /// left: the bytes after the last `\n`, undecoded, leaving the buffer
    /// empty. [`Self::finish`] is the strict form that decodes them as one
    /// final line.
    pub fn take_partial_line(&mut self) -> Vec<u8> {
        self.scanned = 0;
        std::mem::take(&mut self.bytes)
    }
}

fn decode_stream_line(line: Vec<u8>) -> Result<String, LlmError> {
    String::from_utf8(line).map_err(|error| LlmError::StreamParseError {
        message: format!(
            "stream line is not valid UTF-8 at byte {}",
            error.utf8_error().valid_up_to()
        ),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod utf8_line_tests {
    use super::*;

    const TEXT: &str = "data: don\u{2019}t panic \u{1F642}\n\ndata: next\n";

    /// Lines come out whole and exact however the bytes were chunked, a
    /// chunk boundary inside a character included.
    #[test]
    fn lines_are_decoded_whole_whatever_the_chunk_boundaries() {
        let expected = vec![
            "data: don\u{2019}t panic \u{1F642}".to_owned(),
            String::new(),
            "data: next".to_owned(),
        ];
        let bytes = TEXT.as_bytes();
        for chunk_len in 1..=bytes.len() {
            let mut lines = Utf8LineBuffer::with_capacity(8);
            let mut decoded = Vec::new();
            for chunk in bytes.chunks(chunk_len) {
                lines.push(chunk);
                while let Some(line) = lines.next_line() {
                    decoded.push(line.expect("valid line"));
                }
            }
            assert_eq!(decoded, expected, "chunks of {chunk_len} bytes");
            assert_eq!(lines.finish().expect("nothing left"), None);
        }
    }

    /// A complete line that is not valid UTF-8 is a stream parse error, and
    /// so is a final line cut off inside a character; nothing is replaced.
    #[test]
    fn invalid_or_truncated_lines_are_stream_parse_errors() {
        let mut lines = Utf8LineBuffer::default();
        lines.push(b"data: bad \xFF byte\ndata: ok\n");
        assert!(matches!(
            lines.next_line(),
            Some(Err(LlmError::StreamParseError { .. }))
        ));
        assert_eq!(
            lines.next_line().map(|line| line.expect("valid line")),
            Some("data: ok".to_owned())
        );

        let mut truncated = Utf8LineBuffer::default();
        truncated.push("data: don".as_bytes());
        truncated.push(&"\u{2019}".as_bytes()[..2]);
        assert!(truncated.next_line().is_none());
        assert!(matches!(
            truncated.finish(),
            Err(LlmError::StreamParseError { .. })
        ));

        let mut tail = Utf8LineBuffer::default();
        tail.push("data: last \u{1F642}".as_bytes());
        assert_eq!(
            tail.finish().expect("a whole final line"),
            Some("data: last \u{1F642}".to_owned())
        );
        assert_eq!(tail.finish().expect("nothing left"), None);
    }

    /// The bytes after the last line break come back undecoded for a reader
    /// that only reports them; the buffer is empty after, and reads on.
    #[test]
    fn a_partial_line_is_taken_undecoded() {
        let mut lines = Utf8LineBuffer::default();
        lines.push(b"data: one\ndata: cut \xE2\x80");
        assert_eq!(
            lines.next_line().map(|line| line.expect("valid line")),
            Some("data: one".to_owned())
        );
        assert!(lines.next_line().is_none());
        assert_eq!(lines.take_partial_line(), b"data: cut \xE2\x80".to_vec());
        assert_eq!(lines.finish().expect("nothing left"), None);
        lines.push(b"next\n");
        assert_eq!(
            lines.next_line().map(|line| line.expect("valid line")),
            Some("next".to_owned())
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::StopReason;

    #[tokio::test]
    async fn test_ensure_terminal_done_converts_error_to_done() {
        let inner = Box::pin(futures::stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: "hi".to_string(),
                meta: None,
            }),
            Err(LlmError::ConnectionReset),
        ]));

        let mut stream = ensure_terminal_done(inner);
        let mut saw_done = false;

        while let Some(item) = stream.next().await {
            let event = item.expect("wrapper should not yield Err");
            match event {
                LlmEvent::TextDelta { .. } => {}
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error { error },
                } => {
                    assert!(matches!(error, LlmError::ConnectionReset));
                    saw_done = true;
                }
                other => panic!("Unexpected event: {other:?}"),
            }
        }

        assert!(saw_done, "Expected terminal Done event");
    }

    #[tokio::test]
    async fn test_ensure_terminal_done_appends_incomplete_done() {
        let inner = Box::pin(futures::stream::iter([Ok(LlmEvent::TextDelta {
            delta: "hi".to_string(),
            meta: None,
        })]));

        let mut stream = ensure_terminal_done(inner);
        let mut done_count = 0;

        while let Some(item) = stream.next().await {
            let event = item.expect("wrapper should not yield Err");
            if let LlmEvent::Done { outcome } = event {
                done_count += 1;
                match outcome {
                    LlmDoneOutcome::Error { error } => {
                        assert!(matches!(error, LlmError::IncompleteResponse { .. }));
                    }
                    LlmDoneOutcome::Success { .. } => panic!("Expected error outcome"),
                }
            }
        }

        assert_eq!(done_count, 1, "Expected exactly one Done event");
    }

    #[tokio::test]
    async fn test_ensure_terminal_done_stops_after_success_done() {
        let inner = Box::pin(futures::stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: "hi".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            }),
            Ok(LlmEvent::TextDelta {
                delta: "later".to_string(),
                meta: None,
            }),
        ]));

        let mut stream = ensure_terminal_done(inner);
        let mut done_count = 0;
        let mut saw_later_delta = false;

        while let Some(item) = stream.next().await {
            let event = item.expect("wrapper should not yield Err");
            match event {
                LlmEvent::TextDelta { delta, .. } => {
                    if delta == "later" {
                        saw_later_delta = true;
                    }
                }
                LlmEvent::Done { outcome } => {
                    done_count += 1;
                    assert!(matches!(outcome, LlmDoneOutcome::Success { .. }));
                }
                _ => {}
            }
        }

        assert_eq!(done_count, 1, "Expected exactly one Done event");
        assert!(
            !saw_later_delta,
            "Expected stream to stop yielding events after Done"
        );
    }
}
