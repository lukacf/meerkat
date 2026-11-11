//! What a process knows about the issuer of OpenAI-family continuity
//! material, and what the replay owner admits from it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{OpenAiContinuityIssuer, ProviderMeta};

fn reasoning(id: &str, ciphertext: &str) -> AssistantBlock {
    AssistantBlock::Reasoning {
        text: String::new(),
        meta: Some(Box::new(ProviderMeta::OpenAi {
            id: id.to_string(),
            encrypted_content: Some(ciphertext.to_string()),
            phase: None,
            response_id: Some("resp_1".to_string()),
        })),
    }
}

fn answer(id: &str) -> AssistantBlock {
    AssistantBlock::Text {
        text: "Done.".to_string(),
        meta: Some(Box::new(ProviderMeta::OpenAiAssistantMessage {
            id: id.to_string(),
            phase: None,
            response_id: Some("resp_1".to_string()),
        })),
    }
}

fn signed_thinking() -> AssistantBlock {
    AssistantBlock::Reasoning {
        text: "plan".to_string(),
        meta: Some(Box::new(ProviderMeta::Anthropic {
            signature: "signature".to_string(),
        })),
    }
}

/// One response: a reasoning item and an answer.
fn response(tag: &str) -> Vec<AssistantBlock> {
    vec![
        reasoning(&format!("rs_{tag}"), &format!("ciphertext-{tag}")),
        answer(&format!("msg_{tag}")),
    ]
}

fn record(
    ledger: &OpenAiContinuityLedger,
    issuer: &OpenAiContinuityIssuer,
    step: &[AssistantBlock],
) {
    ledger.record_response(issuer, step.iter().filter_map(replay_provider_metadata));
}

const PUBLIC_API: OpenAiContinuityIssuer = OpenAiContinuityIssuer::OpenAiApi;
const COPILOT: OpenAiContinuityIssuer = OpenAiContinuityIssuer::Copilot;

fn server(id: &str) -> OpenAiContinuityIssuer {
    OpenAiContinuityIssuer::SelfHosted {
        server: id.to_string(),
    }
}

/// A response is known to have come from the issuer that returned it and
/// from no other; a ledger that received nothing knows nothing.
#[test]
fn a_ledger_knows_what_it_received_and_from_whom() {
    let ledger = OpenAiContinuityLedger::new();
    let step = response("a");
    assert!(
        !ledger.received_step_from(&PUBLIC_API, &step),
        "nothing yet"
    );

    record(&ledger, &PUBLIC_API, &step);
    assert!(ledger.received_step_from(&PUBLIC_API, &step));
    for other in [COPILOT, server("local")] {
        assert!(!ledger.received_step_from(&other, &step), "{other:?}");
    }
    assert!(
        !OpenAiContinuityLedger::new().received_step_from(&PUBLIC_API, &step),
        "another process received nothing"
    );

    let local = response("b");
    record(&ledger, &server("local-a"), &local);
    assert!(ledger.received_step_from(&server("local-a"), &local));
    assert!(
        !ledger.received_step_from(&server("local-b"), &local),
        "one self-hosted server is not another"
    );
}

/// The entry is the material, not its id: the same item id with another
/// ciphertext, response id or phase is other material.
#[test]
fn an_id_does_not_claim_other_material() {
    let ledger = OpenAiContinuityLedger::new();
    record(&ledger, &PUBLIC_API, &[reasoning("rs_1", "ciphertext-1")]);
    assert!(ledger.received_step_from(&PUBLIC_API, &[reasoning("rs_1", "ciphertext-1")]));
    assert!(
        !ledger.received_step_from(&PUBLIC_API, &[reasoning("rs_1", "other bytes")]),
        "the same id with another ciphertext"
    );
    let other_response = AssistantBlock::Reasoning {
        text: String::new(),
        meta: Some(Box::new(ProviderMeta::OpenAi {
            id: "rs_1".to_string(),
            encrypted_content: Some("ciphertext-1".to_string()),
            phase: None,
            response_id: Some("resp_2".to_string()),
        })),
    };
    assert!(
        !ledger.received_step_from(&PUBLIC_API, &[other_response]),
        "the same id and ciphertext under another response id"
    );
}

/// A step is known as a whole: one item this process did not receive makes
/// the step unknown, whichever items beside it are known.
#[test]
fn a_step_is_known_as_a_whole() {
    let ledger = OpenAiContinuityLedger::new();
    let step = response("a");
    record(&ledger, &PUBLIC_API, &step);
    let mut with_a_stranger = step.clone();
    with_a_stranger.push(answer("msg_never_received"));
    assert!(!ledger.received_step_from(&PUBLIC_API, &with_a_stranger));
    assert!(
        ledger.received_step_from(&PUBLIC_API, &step[..1]),
        "a part of a received step is received material"
    );
}

/// Byte-identical material two issuers both returned identifies neither's.
#[test]
fn material_two_issuers_returned_is_contested() {
    let ledger = OpenAiContinuityLedger::new();
    let step = response("a");
    record(&ledger, &PUBLIC_API, &step);
    record(&ledger, &COPILOT, &step);
    assert!(!ledger.received_step_from(&PUBLIC_API, &step));
    assert!(!ledger.received_step_from(&COPILOT, &step));
    record(&ledger, &PUBLIC_API, &step);
    assert!(
        !ledger.received_step_from(&PUBLIC_API, &step),
        "contested material stays contested"
    );
}

/// The ledger forgets a response at a time, oldest first, and a forgotten
/// response is unknown again.
#[test]
fn the_oldest_response_is_forgotten_whole() {
    let ledger = OpenAiContinuityLedger::with_capacity(2);
    let (first, second, third) = (response("1"), response("2"), response("3"));
    record(&ledger, &PUBLIC_API, &first);
    record(&ledger, &PUBLIC_API, &second);
    assert!(ledger.received_step_from(&PUBLIC_API, &first));
    record(&ledger, &PUBLIC_API, &third);
    assert!(
        !ledger.received_step_from(&PUBLIC_API, &first[..1])
            && !ledger.received_step_from(&PUBLIC_API, &first[1..]),
        "every item of the oldest response is forgotten"
    );
    assert!(ledger.received_step_from(&PUBLIC_API, &second));
    assert!(ledger.received_step_from(&PUBLIC_API, &third));
}

/// Metadata of another provider family is none of the ledger's material: it
/// is neither recorded nor required of a step.
#[test]
fn other_families_are_not_ledger_material() {
    let ledger = OpenAiContinuityLedger::new();
    record(&ledger, &PUBLIC_API, &[signed_thinking()]);
    let mut step = response("a");
    record(&ledger, &PUBLIC_API, &step);
    step.push(signed_thinking());
    assert!(
        ledger.received_step_from(&PUBLIC_API, &step),
        "a signed thinking block beside received OpenAI material does not make the step unknown"
    );
}

/// The process ledger is one ledger.
#[test]
fn the_process_ledger_is_shared() {
    let step = response("the_process_ledger_is_shared");
    record(&OpenAiContinuityLedger::process(), &COPILOT, &step);
    assert!(OpenAiContinuityLedger::process().received_step_from(&COPILOT, &step));
}
