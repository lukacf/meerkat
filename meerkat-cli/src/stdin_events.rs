//! Stdin event reader for runtime-backed CLI sessions.
//!
//! Reads newline-delimited lines from stdin, auto-detects JSON/text,
//! and admits them as runtime-backed `ExternalEvent` inputs.

use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
use tokio::task::JoinHandle;

use meerkat_core::SessionId;
use meerkat_runtime::SessionServiceRuntimeExt as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdinLineFormat {
    Text,
    Json,
}

/// Parse a stdin line into an event body (auto-detect JSON/text).
///
/// Delegates to the shared `parse_plain_line()` from `meerkat-comms`.
pub fn parse_stdin_line(line: &str, format: StdinLineFormat) -> String {
    match format {
        StdinLineFormat::Text => line.to_string(),
        StdinLineFormat::Json => meerkat_comms::transport::plain_codec::parse_plain_line(line),
    }
}

fn make_stdin_external_event_input(
    body: String,
    format: StdinLineFormat,
) -> meerkat_runtime::Input {
    meerkat_runtime::Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
        header: meerkat_runtime::InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: meerkat_runtime::InputOrigin::External {
                source_name: "stdin".to_string(),
            },
            durability: meerkat_runtime::InputDurability::Durable,
            visibility: meerkat_runtime::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        event_type: "stdin".to_string(),
        // Text mode is literal by contract. JSON mode preserves structured JSON
        // when the parsed body is itself valid JSON.
        payload: match format {
            StdinLineFormat::Text => serde_json::json!({ "body": body }),
            StdinLineFormat::Json => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(parsed) => serde_json::json!({ "body": parsed }),
                Err(_) => serde_json::json!({ "body": body }),
            },
        },
        blocks: None,
    })
}

/// Spawn a background task that reads lines from stdin and admits them through
/// the runtime-backed external-event path.
///
/// The task exits cleanly on EOF or when the runtime stops accepting input.
pub fn spawn_stdin_reader(
    runtime_adapter: Arc<meerkat_runtime::RuntimeSessionAdapter>,
    session_id: SessionId,
    format: StdinLineFormat,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let reader = tokio::io::BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let body = parse_stdin_line(&line, format);
            if body.is_empty() {
                continue;
            }
            match runtime_adapter
                .accept_input(&session_id, make_stdin_external_event_input(body, format))
                .await
            {
                Ok(
                    meerkat_runtime::AcceptOutcome::Accepted { .. }
                    | meerkat_runtime::AcceptOutcome::Deduplicated { .. },
                ) => {}
                Ok(meerkat_runtime::AcceptOutcome::Rejected { reason }) => {
                    tracing::warn!("Stdin reader: runtime rejected stdin event: {reason}");
                }
                Ok(outcome) => {
                    tracing::warn!(
                        "Stdin reader: unexpected runtime accept outcome, exiting: {outcome:?}"
                    );
                    return;
                }
                Err(meerkat_runtime::RuntimeDriverError::Destroyed) => {
                    tracing::debug!("Stdin reader: runtime destroyed, exiting");
                    return;
                }
                Err(meerkat_runtime::RuntimeDriverError::NotReady { state }) => {
                    tracing::warn!(
                        "Stdin reader: runtime not ready for stdin event while in state {state}"
                    );
                }
                Err(err) => {
                    tracing::warn!("Stdin reader: runtime admission failed: {err}");
                    return;
                }
            }
        }
        tracing::debug!("Stdin reader: EOF, exiting");
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stdin_line_json_with_body() {
        assert_eq!(
            parse_stdin_line(r#"{"body":"hello"}"#, StdinLineFormat::Json),
            "hello"
        );
    }

    #[test]
    fn test_parse_stdin_line_plain_text() {
        assert_eq!(
            parse_stdin_line("plain text", StdinLineFormat::Text),
            "plain text"
        );
    }

    #[test]
    fn test_parse_stdin_line_json_without_body() {
        let result = parse_stdin_line(r#"{"event":"email","from":"john"}"#, StdinLineFormat::Json);
        assert!(result.contains("email"));
        assert!(result.contains("john"));
    }

    #[test]
    fn test_parse_stdin_line_empty() {
        assert_eq!(parse_stdin_line("", StdinLineFormat::Text), "");
    }

    #[test]
    fn test_make_stdin_external_event_input_uses_runtime_external_event_shape() {
        let input = make_stdin_external_event_input("hello".to_string(), StdinLineFormat::Text);
        let meerkat_runtime::Input::ExternalEvent(event) = input else {
            panic!("expected external event input");
        };
        assert_eq!(event.event_type, "stdin");
        assert_eq!(event.payload["body"], "hello");
        assert_eq!(event.blocks, None);
        assert!(matches!(
            event.header.source,
            meerkat_runtime::InputOrigin::External { ref source_name } if source_name == "stdin"
        ));
    }

    #[test]
    fn test_make_stdin_external_event_input_text_mode_preserves_json_looking_literal() {
        let input = make_stdin_external_event_input(
            r#"{"level":"info"}"#.to_string(),
            StdinLineFormat::Text,
        );
        let meerkat_runtime::Input::ExternalEvent(event) = input else {
            panic!("expected external event input");
        };
        assert_eq!(event.payload["body"], r#"{"level":"info"}"#);
    }

    #[test]
    fn test_make_stdin_external_event_input_json_mode_preserves_structured_json() {
        let input = make_stdin_external_event_input(
            r#"{"level":"info"}"#.to_string(),
            StdinLineFormat::Json,
        );
        let meerkat_runtime::Input::ExternalEvent(event) = input else {
            panic!("expected external event input");
        };
        assert_eq!(event.payload["body"]["level"], "info");
    }
}
