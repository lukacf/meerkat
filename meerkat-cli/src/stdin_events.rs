//! Stdin event reader for CLI host mode.
//!
//! Reads newline-delimited lines from stdin, auto-detects JSON/text,
//! and injects them as `PlainEvent` items via the `EventInjector` trait.

use meerkat_core::PlainEventSource;
use meerkat_core::event_injector::{EventInjector, EventInjectorError};
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
use tokio::task::JoinHandle;

/// Parse a stdin line into an event body (auto-detect JSON/text).
///
/// Delegates to the shared `parse_plain_line()` from `meerkat-comms`.
pub fn parse_stdin_line(line: &str) -> String {
    meerkat_comms::transport::plain_codec::parse_plain_line(line)
}

/// Spawn a background task that reads lines from stdin and injects them as events.
///
/// The task exits cleanly on EOF or when the injector's inbox is closed.
/// On inbox full, it logs a warning and drops the line (backpressure).
pub fn spawn_stdin_reader(injector: Arc<dyn EventInjector>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let reader = tokio::io::BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let body = parse_stdin_line(&line);
            if body.is_empty() {
                continue;
            }
            match injector.inject(body, PlainEventSource::Stdin) {
                Ok(()) => {}
                Err(EventInjectorError::Full) => {
                    tracing::warn!("Stdin reader: inbox full, dropping event");
                }
                Err(EventInjectorError::Closed) => {
                    tracing::debug!("Stdin reader: inbox closed, exiting");
                    return;
                }
            }
        }
        tracing::debug!("Stdin reader: EOF, exiting");
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stdin_line_json_with_body() {
        assert_eq!(parse_stdin_line(r#"{"body":"hello"}"#), "hello");
    }

    #[test]
    fn test_parse_stdin_line_plain_text() {
        assert_eq!(parse_stdin_line("plain text"), "plain text");
    }

    #[test]
    fn test_parse_stdin_line_json_without_body() {
        let result = parse_stdin_line(r#"{"event":"email","from":"john"}"#);
        assert!(result.contains("email"));
        assert!(result.contains("john"));
    }

    #[test]
    fn test_parse_stdin_line_empty() {
        assert_eq!(parse_stdin_line(""), "");
    }
}
