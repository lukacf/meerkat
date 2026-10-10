//! Outbound peer requests a live delegation worker sent and has no answer
//! for yet, read from the worker session's committed rows.
//!
//! A worker asked to "ask another member" completes its turn with "I asked
//! them": comms requests have no built-in wait, so the member's answer is a
//! later turn's input. The voice layer must know that answer is still
//! pending when the "I asked" result lands (Turbo S S102 r2 voiced an
//! invented one). Every fact here is typed: a request is a successful tool
//! result whose output deserializes to the comms contract's
//! [`CommsSendResult::PeerRequestSent`] receipt, for a tool call the worker's
//! own turn made (its assistant row carries the delegation's interaction
//! id), and its answer is a committed comms notice of kind
//! [`CommsNoticeKind::ResponseTerminal`] carrying the same request id. No
//! text is read for meaning, and a request another turn of the session left
//! unanswered never counts against this result.

use std::collections::{HashMap, HashSet};

use meerkat_contracts::CommsSendResult;
use meerkat_core::types::{
    AssistantBlock, CommsNoticeKind, Message, SystemNoticeBlock, text_content,
};

/// Label for a member the request names no display label for.
const UNNAMED_PEER_LABEL: &str = "the other member";

/// The comms tool output that carries a send receipt (`{"status": "sent",
/// "kind": ..., "receipt": ...}`); only the typed receipt is read.
#[derive(serde::Deserialize)]
struct SentToolOutput {
    receipt: CommsSendResult,
}

/// The display labels of the members whose peer requests, sent by tool calls
/// of the turn identified by `interaction` (the delegation's interaction
/// id, stamped on its assistant rows), have no committed terminal response
/// in `messages`, in request order, each named once.
///
/// The label is the request's own `display_name` argument (diagnostic only;
/// routing used the peer id), or [`UNNAMED_PEER_LABEL`] without one.
pub(super) fn awaiting_peer_replies(messages: &[Message], interaction: &str) -> Vec<String> {
    let mut labels_by_call: HashMap<&str, Option<String>> = HashMap::new();
    let mut requests: Vec<(String, Option<String>)> = Vec::new();
    let mut answered: HashSet<String> = HashSet::new();
    for message in messages {
        match message {
            Message::BlockAssistant(assistant)
                if assistant
                    .identity
                    .interaction_id
                    .is_some_and(|id| id.to_string() == interaction) =>
            {
                for block in &assistant.blocks {
                    if let AssistantBlock::ToolUse { id, args, .. } = block {
                        labels_by_call.insert(id.as_str(), display_label(args.get()));
                    }
                }
            }
            Message::ToolResults { results, .. } => {
                for result in results.iter().filter(|result| !result.is_error) {
                    let Some(label) = labels_by_call.get(result.tool_use_id.as_str()) else {
                        // Not a call of this turn.
                        continue;
                    };
                    let Ok(SentToolOutput {
                        receipt: CommsSendResult::PeerRequestSent { request_id, .. },
                    }) = serde_json::from_str(&text_content(&result.content))
                    else {
                        continue;
                    };
                    requests.push((request_id, label.clone()));
                }
            }
            Message::SystemNotice(notice) => {
                for block in &notice.blocks {
                    if let SystemNoticeBlock::Comms {
                        kind: CommsNoticeKind::ResponseTerminal,
                        request_id: Some(request_id),
                        ..
                    } = block
                    {
                        answered.insert(request_id.clone());
                    }
                }
            }
            _ => {}
        }
    }
    let mut labels = Vec::new();
    for (request_id, label) in requests {
        if answered.contains(&request_id) {
            continue;
        }
        let label = label.unwrap_or_else(|| UNNAMED_PEER_LABEL.to_string());
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}

/// A tool call's non-empty `display_name` argument.
fn display_label(args: &str) -> Option<String> {
    let args: serde_json::Value = serde_json::from_str(args).ok()?;
    let label = args.get("display_name")?.as_str()?.trim();
    (!label.is_empty()).then(|| label.to_string())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "focused row-shape tests use explicit assertion messages"
)]
mod tests {
    use super::*;
    use meerkat_core::types::{
        BlockAssistantMessage, ContentBlock, SystemNoticeDirection, SystemNoticeKind,
        SystemNoticeMessage, ToolResult,
    };

    const REQUEST: &str = "06f6c345-2214-44f7-b37c-c87a7d75b27b";
    const TURN: uuid::Uuid = uuid::Uuid::from_u128(0xf063_3e40);
    const OTHER_TURN: uuid::Uuid = uuid::Uuid::from_u128(0x0bad);

    fn turn() -> String {
        TURN.to_string()
    }

    fn send_request_call(call_id: &str, display_name: Option<&str>) -> Message {
        send_request_call_in(TURN, call_id, display_name)
    }

    fn send_request_call_in(
        interaction: uuid::Uuid,
        call_id: &str,
        display_name: Option<&str>,
    ) -> Message {
        let mut args = serde_json::json!({
            "peer_id": "3649ce6c-4112-593c-9abc-e88726b7b583",
            "intent": "checksum_token",
            "params": {"subject": "what_time_do_you_think_it_is"},
            "handling_mode": "queue",
        });
        if let Some(display_name) = display_name {
            args["display_name"] = serde_json::json!(display_name);
        }
        let mut assistant = BlockAssistantMessage::snapshot(vec![AssistantBlock::ToolUse {
            id: call_id.to_string(),
            name: "send_request".to_string(),
            args: serde_json::value::to_raw_value(&args).expect("raw args"),
            meta: None,
        }]);
        assistant.identity.interaction_id =
            Some(meerkat_core::interaction::InteractionId(interaction));
        Message::BlockAssistant(assistant)
    }

    fn sent_receipt(call_id: &str, request_id: &str, is_error: bool) -> Message {
        let output = serde_json::json!({
            "status": "sent",
            "kind": "peer_request",
            "receipt": {
                "kind": "peer_request_sent",
                "envelope_id": request_id,
                "interaction_id": request_id,
                "request_id": request_id,
                "stream_reserved": true,
                "delivery": {"durably_resolved": {"outcome": "accepted"}},
            },
        });
        Message::ToolResults {
            results: vec![ToolResult {
                host_metadata: Default::default(),
                tool_use_id: call_id.to_string(),
                content: vec![ContentBlock::Text {
                    text: output.to_string(),
                }],
                is_error,
                settlement_failures: Vec::new(),
            }],
            created_at: meerkat_core::types::message_timestamp_now(),
        }
    }

    fn response_notice(kind: CommsNoticeKind, request_id: &str) -> Message {
        Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::Comms,
            None,
            SystemNoticeBlock::Comms {
                kind,
                direction: SystemNoticeDirection::Incoming,
                peer: None,
                sender_taint: None,
                request_id: Some(request_id.to_string()),
                intent: None,
                status: Some("completed".to_string()),
                summary: None,
                payload: None,
                content: Vec::new(),
            },
        ))
    }

    /// S102 r2's shape at result time: the worker sent one request and its
    /// answer has not been committed.
    #[test]
    fn a_sent_request_without_a_terminal_response_is_awaiting() {
        let rows = [
            send_request_call("call_1", Some("analyst-pemberton")),
            sent_receipt("call_1", REQUEST, false),
        ];
        assert_eq!(awaiting_peer_replies(&rows, &turn()), ["analyst-pemberton"]);
    }

    /// Once the terminal response for that request id is committed, nothing
    /// is awaiting; a progress response does not answer it.
    #[test]
    fn only_a_terminal_response_to_the_same_request_answers_it() {
        let call = send_request_call("call_1", Some("analyst-pemberton"));
        let sent = sent_receipt("call_1", REQUEST, false);
        let progress = response_notice(CommsNoticeKind::ResponseProgress, REQUEST);
        let other = response_notice(CommsNoticeKind::ResponseTerminal, "another-request");
        let terminal = response_notice(CommsNoticeKind::ResponseTerminal, REQUEST);
        assert_eq!(
            awaiting_peer_replies(&[call.clone(), sent.clone(), progress, other], &turn()),
            ["analyst-pemberton"]
        );
        assert!(awaiting_peer_replies(&[call, sent, terminal], &turn()).is_empty());
    }

    /// A failed send and any non-request tool output are not outstanding
    /// requests; a request without a display label is still counted.
    #[test]
    fn failed_sends_and_other_outputs_are_not_requests() {
        let failed = [
            send_request_call("call_1", Some("analyst-pemberton")),
            sent_receipt("call_1", REQUEST, true),
        ];
        assert!(awaiting_peer_replies(&failed, &turn()).is_empty());
        let unrelated = Message::ToolResults {
            results: vec![ToolResult {
                host_metadata: Default::default(),
                tool_use_id: "call_2".to_string(),
                content: vec![ContentBlock::Text {
                    text: "{\"status\":\"sent\",\"kind\":\"peer_request\"}".to_string(),
                }],
                is_error: false,
                settlement_failures: Vec::new(),
            }],
            created_at: meerkat_core::types::message_timestamp_now(),
        };
        assert!(awaiting_peer_replies(&[unrelated], &turn()).is_empty());
        let unnamed = [
            send_request_call("call_3", None),
            sent_receipt("call_3", REQUEST, false),
        ];
        assert_eq!(
            awaiting_peer_replies(&unnamed, &turn()),
            [UNNAMED_PEER_LABEL]
        );
    }

    /// A request another turn of the same session sent and never had
    /// answered does not make this turn's result "awaiting".
    #[test]
    fn requests_from_other_turns_never_count() {
        let rows = [
            send_request_call_in(OTHER_TURN, "call_old", Some("analyst-pemberton")),
            sent_receipt("call_old", "an-old-request", false),
        ];
        assert!(awaiting_peer_replies(&rows, &turn()).is_empty());
        assert_eq!(
            awaiting_peer_replies(&rows, &OTHER_TURN.to_string()),
            ["analyst-pemberton"]
        );
    }
}
