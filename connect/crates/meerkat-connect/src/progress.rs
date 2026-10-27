//! Bounded, provisional terminal output from one native session subscription.
//! This owns no turn, admission, cancellation, approval or terminal outcome.

use std::io::{self, Write};

use futures_util::StreamExt;
use meerkat_core::{AgentEvent, AssistantMessageId, SessionId, comms::EventStream};

const LIVE_LIMIT: usize = 32 * 1024;
const FINAL_LIMIT: usize = 64 * 1024;
const LIVE_PREFIX: &str = "Connect (in progress): ";

/// Poll only while the terminal is not presenting an approval. Dropping this
/// view only drops its event subscription; it cannot stop native work.
pub struct TurnProgress {
    session_id: SessionId,
    events: Option<EventStream>,
    message_id: Option<AssistantMessageId>,
    text: String,
    remaining: usize,
    line_open: bool,
    incomplete: bool,
}

impl TurnProgress {
    pub fn new(session_id: SessionId, events: EventStream) -> Self {
        Self {
            session_id,
            events: Some(events),
            message_id: None,
            text: String::new(),
            remaining: LIVE_LIMIT,
            line_open: false,
            incomplete: false,
        }
    }

    pub fn begin(&mut self, output: &mut impl Write) -> io::Result<()> {
        self.emit(
            "Connect is working. Ctrl-C stops this setup conversation.\n",
            output,
        )?;
        Ok(())
    }

    /// After closure, loss or the display limit, this remains pending. The
    /// presenter's completion future continues to own terminality; repeatedly
    /// polling a closed stream must not become a busy loop.
    pub async fn next(&mut self, output: &mut impl Write) -> io::Result<()> {
        let event = match &mut self.events {
            Some(events) => events.next().await,
            None => return std::future::pending().await,
        };
        let Some(event) = event else {
            return self.unavailable(output);
        };
        if event.source.session_id() == Some(&self.session_id) {
            self.observe(&event.payload, output)?;
        }
        Ok(())
    }

    fn observe(&mut self, event: &AgentEvent, output: &mut impl Write) -> io::Result<()> {
        match event {
            AgentEvent::TurnStarted {
                assistant_message_id,
                ..
            } => {
                self.pause_for_prompt(output)?;
                // Retries can restart the same native message. Keep preceding
                // text visibly provisional; do not concatenate it into a retry.
                self.message_id = *assistant_message_id;
                self.text.clear();
            }
            AgentEvent::TextDelta {
                delta,
                assistant_message_id,
            } if self.message_id.is_some() && *assistant_message_id == self.message_id => {
                let prefix = if self.line_open { "" } else { LIVE_PREFIX };
                let (text, truncated) =
                    display_prefix(delta, self.remaining.saturating_sub(prefix.len()));
                if !text.is_empty() {
                    if !self.emit(&format!("{prefix}{text}"), output)? {
                        return Ok(());
                    }
                    self.line_open = true;
                    self.text.push_str(&text);
                }
                if truncated {
                    self.unavailable(output)?;
                }
            }
            AgentEvent::ToolExecutionStarted { .. } => {
                self.pause_for_prompt(output)?;
                self.emit("Connect is running a setup tool.\n", output)?;
            }
            AgentEvent::Retrying {
                assistant_message_id,
                ..
            } if self.message_id.is_some() && *assistant_message_id == self.message_id => {
                self.pause_for_prompt(output)?;
                self.text.clear();
            }
            AgentEvent::StreamTruncated { .. } => self.unavailable(output)?,
            // In particular: no reasoning, raw tool names/arguments/results,
            // provider/auth errors, RunCompleted result or TextComplete copy.
            // A text-complete event is not a committed answer.
            _ => {}
        }
        Ok(())
    }

    fn emit(&mut self, text: &str, output: &mut impl Write) -> io::Result<bool> {
        if text.len() > self.remaining {
            self.unavailable(output)?;
            return Ok(false);
        }
        output.write_all(text.as_bytes())?;
        output.flush()?;
        self.remaining -= text.len();
        Ok(true)
    }

    fn unavailable(&mut self, output: &mut impl Write) -> io::Result<()> {
        self.events = None;
        if !self.incomplete {
            self.incomplete = true;
            self.pause_for_prompt(output)?;
            writeln!(
                output,
                "Live output paused. Waiting for the native result; no retry was started."
            )?;
            output.flush()?;
        }
        Ok(())
    }

    /// End a partial line before handing the sole input/output ownership to
    /// the approval presenter, or before reporting cancellation or completion.
    pub fn pause_for_prompt(&mut self, output: &mut impl Write) -> io::Result<()> {
        if self.line_open {
            writeln!(output)?;
            output.flush()?;
            self.line_open = false;
            self.remaining = self.remaining.saturating_sub(1);
        }
        Ok(())
    }

    /// Call only with text accepted by `terminal::committed_text` after the
    /// native completion handle settles. This comparison avoids duplicate
    /// printing; it does not establish commit, identity or execution state.
    pub fn finish_committed(&mut self, text: &str, output: &mut impl Write) -> io::Result<()> {
        self.events = None;
        self.pause_for_prompt(output)?;
        let (text, truncated) = display_prefix(text, FINAL_LIMIT);
        if !self.incomplete && !truncated && !self.text.is_empty() && self.text == text {
            writeln!(output, "Setup turn completed.")?;
        } else {
            writeln!(output, "Connect: {text}")?;
        }
        if truncated {
            writeln!(
                output,
                "The displayed answer reached the terminal limit. The complete result remains in native session history."
            )?;
        }
        output.flush()
    }
}

// Unlike sanitizing then truncating, this never allocates the complete model
// text or splits a UTF-8 character. The same escaping is used for both views.
fn display_prefix(text: &str, limit: usize) -> (String, bool) {
    let mut result = String::new();
    for character in text.chars() {
        let mut encoded = [0; 4];
        let visible = if character == '\u{1b}' {
            "[ESC]"
        } else if character == '\n' || character == '\t' || !character.is_control() {
            character.encode_utf8(&mut encoded)
        } else {
            continue;
        };
        if result.len() + visible.len() > limit {
            return (result, true);
        }
        result.push_str(visible);
    }
    (result, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{ContentBlock, event::StreamTruncationReason};

    fn progress() -> TurnProgress {
        TurnProgress::new(SessionId::new(), Box::pin(futures_util::stream::pending()))
    }

    fn message_id() -> AssistantMessageId {
        serde_json::from_str("\"01234567-89ab-4def-8123-456789abcdef\"").unwrap()
    }

    fn start(progress: &mut TurnProgress, output: &mut Vec<u8>) {
        progress
            .observe(
                &AgentEvent::TurnStarted {
                    turn_number: 1,
                    assistant_message_id: Some(message_id()),
                },
                output,
            )
            .unwrap();
    }

    #[test]
    fn only_identified_assistant_text_is_rendered_and_approval_gets_a_clean_line() {
        let mut progress = progress();
        let mut output = Vec::new();
        start(&mut progress, &mut output);
        for event in [
            AgentEvent::ReasoningDelta {
                delta: "hidden reasoning".into(),
                assistant_message_id: Some(message_id()),
            },
            AgentEvent::ReasoningComplete {
                content: "hidden reasoning complete".into(),
                assistant_message_id: Some(message_id()),
            },
            AgentEvent::ToolCallRequested {
                id: "private-id".into(),
                name: "secret-tool-name".into(),
                args: serde_json::json!({"token":"secret-argument"})
                    .try_into()
                    .unwrap(),
            },
            AgentEvent::ToolResultReceived {
                id: "private-id".into(),
                name: "secret-tool-name".into(),
                content: ContentBlock::text_vec("secret-tool-result".into()),
                is_error: true,
            },
            AgentEvent::TextComplete {
                content: "not a committed answer".into(),
                assistant_message_id: Some(message_id()),
            },
            AgentEvent::ExtractionFailed {
                session_id: SessionId::new(),
                last_output: "secret-extraction-output".into(),
                attempts: 1,
                reason: "raw-provider-error-with-credential".into(),
                request_usage: Vec::new(),
            },
            AgentEvent::TextDelta {
                delta: "unidentified text".into(),
                assistant_message_id: None,
            },
        ] {
            progress.observe(&event, &mut output).unwrap();
        }
        assert!(output.is_empty());
        progress
            .observe(
                &AgentEvent::TextDelta {
                    delta: "Hello\u{1b}[31m\r".into(),
                    assistant_message_id: Some(message_id()),
                },
                &mut output,
            )
            .unwrap();
        progress.pause_for_prompt(&mut output).unwrap();
        write!(output, "Approval prompt\n").unwrap();
        assert_eq!(
            String::from_utf8(output.clone()).unwrap(),
            "Connect (in progress): Hello[ESC][31m\nApproval prompt\n"
        );
        progress
            .finish_committed("Hello\u{1b}[31m\r", &mut output)
            .unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap().matches("Hello").count(),
            1
        );
    }

    #[tokio::test]
    async fn closed_and_lagged_streams_pause_once_then_stay_pending() {
        for lagged in [false, true] {
            let mut progress =
                TurnProgress::new(SessionId::new(), Box::pin(futures_util::stream::empty()));
            let mut output = Vec::new();
            if lagged {
                progress
                    .observe(
                        &AgentEvent::StreamTruncated {
                            reason: StreamTruncationReason::StreamLagged { dropped: 4 },
                        },
                        &mut output,
                    )
                    .unwrap();
            } else {
                progress.next(&mut output).await.unwrap();
            }
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(20),
                    progress.next(&mut output)
                )
                .await
                .is_err()
            );
            assert_eq!(
                String::from_utf8(output.clone())
                    .unwrap()
                    .matches("Live output paused")
                    .count(),
                1
            );
            assert!(
                !String::from_utf8(output.clone())
                    .unwrap()
                    .contains("completed")
            );
            progress
                .finish_committed("Canonical answer", &mut output)
                .unwrap();
            assert!(
                String::from_utf8(output)
                    .unwrap()
                    .ends_with("Connect: Canonical answer\n")
            );
        }
    }

    #[test]
    fn retry_restarts_display_comparison_and_output_is_bounded() {
        let mut progress = progress();
        let mut output = Vec::new();
        start(&mut progress, &mut output);
        progress
            .observe(
                &AgentEvent::TextDelta {
                    delta: "discarded draft".into(),
                    assistant_message_id: Some(message_id()),
                },
                &mut output,
            )
            .unwrap();
        start(&mut progress, &mut output);
        progress
            .observe(
                &AgentEvent::TextDelta {
                    delta: "x".repeat(LIVE_LIMIT * 3),
                    assistant_message_id: Some(message_id()),
                },
                &mut output,
            )
            .unwrap();
        assert!(progress.events.is_none());
        assert!(progress.text.len() <= LIVE_LIMIT);
        progress
            .finish_committed(&"\u{1b}".repeat(FINAL_LIMIT), &mut output)
            .unwrap();
        assert!(output.len() < LIVE_LIMIT + FINAL_LIMIT + 512);
        assert!(!output.contains(&0x1b));
    }
}
