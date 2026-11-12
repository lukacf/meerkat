//! What a process knows about the issuer of OpenAI-family continuity
//! material, and what the replay owner admits from it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{OpenAiContinuityIssuer, OpenAiContinuityService, ProviderMeta};
use serde_json::json;

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

/// A hosted-tool item: its provider-issued id is in its content and it
/// carries no metadata.
fn hosted_search(id: &str) -> AssistantBlock {
    AssistantBlock::ServerToolContent {
        id: Some(id.to_string()),
        kind: crate::ServerToolKind::WebSearch,
        content: json!({"type": "web_search_call", "id": id, "status": "completed"}),
        meta: None,
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

fn gemini_signed() -> AssistantBlock {
    AssistantBlock::Text {
        text: "answer".to_string(),
        meta: Some(Box::new(ProviderMeta::Gemini {
            thought_signature: "signature".to_string(),
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

fn public_api() -> OpenAiContinuityIssuer {
    OpenAiContinuityIssuer::at_base_url(
        OpenAiContinuityService::OpenAiApi,
        "https://api.openai.com",
    )
}

fn copilot() -> OpenAiContinuityIssuer {
    OpenAiContinuityIssuer::at_base_url(
        OpenAiContinuityService::Copilot,
        "https://api.githubcopilot.com",
    )
}

fn server(id: &str) -> OpenAiContinuityIssuer {
    OpenAiContinuityIssuer::self_hosted_server(id)
}

fn metadata_of(block: &AssistantBlock) -> &ProviderMeta {
    replay_provider_metadata(block).expect("the block carries metadata")
}

fn target_of(
    issuer: OpenAiContinuityIssuer,
    ledger: &Arc<OpenAiContinuityLedger>,
) -> OpenAiContinuityTarget {
    OpenAiContinuityTarget::new(issuer, Arc::clone(ledger))
}

// ---------------------------------------------------------------------------
// The issuer
// ---------------------------------------------------------------------------

/// An issuer is a kind of service and the endpoint it is reached at. Two
/// kinds at one address, and one kind at two addresses, are different
/// issuers; the same address written with another case of scheme or host,
/// or with a trailing slash, is the same one.
#[test]
fn an_issuer_is_a_service_kind_at_an_endpoint() {
    use OpenAiContinuityService::{AzureOpenAi, OpenAiApi, SelfHosted};
    let at = OpenAiContinuityIssuer::at_base_url;
    assert_eq!(public_api().service(), OpenAiApi);
    assert_eq!(
        at(OpenAiApi, "https://api.openai.com"),
        at(OpenAiApi, " HTTPS://API.OpenAI.com/ "),
    );
    assert_ne!(
        at(OpenAiApi, "https://api.openai.com"),
        at(OpenAiApi, "https://gateway.example/openai"),
        "the public API behind another base URL"
    );
    assert_ne!(
        at(AzureOpenAi, "https://one.openai.azure.com"),
        at(AzureOpenAi, "https://two.openai.azure.com"),
        "two Azure OpenAI resources"
    );
    assert_ne!(
        at(OpenAiApi, "https://host.example"),
        at(SelfHosted, "https://host.example"),
        "a self-hosted server is not the public API at the same address"
    );
    assert_ne!(
        at(OpenAiApi, "https://host.example/v1/A"),
        at(OpenAiApi, "https://host.example/v1/a"),
        "a path is case-sensitive"
    );
    assert_eq!(server("local"), server("local"));
    assert_ne!(server("local-a"), server("local-b"));
    assert_ne!(
        server("local"),
        at(SelfHosted, "local"),
        "a declared server is not a base URL of the same text"
    );
}

// ---------------------------------------------------------------------------
// The ledger
// ---------------------------------------------------------------------------

/// A response is known to have come from the issuer that returned it and
/// from no other; a ledger that received nothing knows nothing.
#[test]
fn a_ledger_knows_what_it_received_and_from_whom() {
    let ledger = OpenAiContinuityLedger::new();
    let step = response("a");
    assert!(
        !ledger.received_step_from(&public_api(), &step),
        "nothing yet"
    );

    ledger.record_step(&public_api(), &step);
    assert!(ledger.received_step_from(&public_api(), &step));
    for other in [copilot(), server("local")] {
        assert!(!ledger.received_step_from(&other, &step), "{other:?}");
    }
    assert!(
        !OpenAiContinuityLedger::new().received_step_from(&public_api(), &step),
        "another process received nothing"
    );

    let local = response("b");
    ledger.record_step(&server("local-a"), &local);
    assert!(ledger.received_step_from(&server("local-a"), &local));
    assert!(
        !ledger.received_step_from(&server("local-b"), &local),
        "one self-hosted server is not another"
    );
    // Both issuers are known to the ledger now, each for its own response:
    // neither returned the other's.
    assert!(!ledger.received_step_from(&server("local-a"), &step));
    assert!(!ledger.received_step_from(&public_api(), &local));
}

/// The entry is the material, not its id: the same item id with another
/// ciphertext or under another response id is other material.
#[test]
fn an_id_does_not_claim_other_material() {
    let ledger = OpenAiContinuityLedger::new();
    ledger.record_step(&public_api(), &[reasoning("rs_1", "ciphertext-1")]);
    assert!(ledger.received_step_from(&public_api(), &[reasoning("rs_1", "ciphertext-1")]));
    assert!(
        !ledger.received_step_from(&public_api(), &[reasoning("rs_1", "other bytes")]),
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
        !ledger.received_step_from(&public_api(), &[other_response]),
        "the same id and ciphertext under another response id"
    );
}

/// A hosted-tool item is material by its content, with or without metadata:
/// a step that holds one the process did not receive is unknown, and so is
/// one whose item content differs from what was received.
#[test]
fn a_hosted_tool_item_is_material_by_its_content() {
    let ledger = OpenAiContinuityLedger::new();
    let mut step = response("a");
    step.insert(1, hosted_search("ws_1"));
    ledger.record_step(&public_api(), &step);
    assert!(ledger.received_step_from(&public_api(), &step));

    let mut other_item = step.clone();
    other_item[1] = hosted_search("ws_2");
    assert!(
        !ledger.received_step_from(&public_api(), &other_item),
        "another hosted-tool item beside received metadata"
    );
    assert!(
        !ledger.received_step_from(&public_api(), &[hosted_search("ws_never")]),
        "a hosted-tool item alone, never received"
    );
    assert!(ledger.received_step_from(&public_api(), &[hosted_search("ws_1")]));
}

/// A step is known as a whole: one piece the process did not receive makes
/// the step unknown. A step that carries no material proves nothing, and
/// blocks removed from a received step leave received material.
#[test]
fn a_step_is_known_as_a_whole() {
    let ledger = OpenAiContinuityLedger::new();
    let step = response("a");
    ledger.record_step(&public_api(), &step);
    let mut with_a_stranger = step.clone();
    with_a_stranger.push(answer("msg_never_received"));
    assert!(!ledger.received_step_from(&public_api(), &with_a_stranger));
    assert!(
        ledger.received_step_from(&public_api(), &step[..1]),
        "a received step with a block removed"
    );
    let plain = [AssistantBlock::Text {
        text: "no material".to_string(),
        meta: None,
    }];
    assert!(!ledger.received_step_from(&public_api(), &plain));
    assert!(!ledger.received_step_from(&public_api(), &[]));
}

/// Byte-identical material two issuers both returned identifies neither's.
#[test]
fn material_two_issuers_returned_is_contested() {
    let ledger = OpenAiContinuityLedger::new();
    let step = response("a");
    ledger.record_step(&public_api(), &step);
    ledger.record_step(&copilot(), &step);
    assert!(!ledger.received_step_from(&public_api(), &step));
    assert!(!ledger.received_step_from(&copilot(), &step));
    ledger.record_step(&public_api(), &step);
    assert!(
        !ledger.received_step_from(&public_api(), &step),
        "contested material stays contested"
    );
}

/// The ledger keeps two generations and forgets a generation at a time:
/// with generations of two responses it knows the last two to four, never
/// drops a single response, and a response it forgot is unknown again.
#[test]
fn a_generation_is_forgotten_whole() {
    let ledger = OpenAiContinuityLedger::with_generation(2);
    let steps: Vec<_> = (1..=6).map(|n| response(&n.to_string())).collect();
    let known = |ledger: &OpenAiContinuityLedger| -> Vec<bool> {
        steps
            .iter()
            .map(|step| ledger.received_step_from(&public_api(), step))
            .collect()
    };
    for step in &steps[..4] {
        ledger.record_step(&public_api(), step);
    }
    assert_eq!(
        known(&ledger),
        [false, false, true, true, false, false],
        "the fourth response filled a generation: the first generation is forgotten whole"
    );
    ledger.record_step(&public_api(), &steps[4]);
    assert_eq!(
        known(&ledger),
        [false, false, true, true, true, false],
        "a response into a fresh generation forgets nothing"
    );
    ledger.record_step(&public_api(), &steps[5]);
    assert_eq!(known(&ledger), [false, false, false, false, true, true]);
}

/// A response is recorded whole, however much material it carries, and is
/// then known like any other, as a whole and by any of its pieces.
#[test]
fn a_large_response_is_recorded_whole() {
    let ledger = OpenAiContinuityLedger::new();
    let large: Vec<_> = (0..500).map(|n| answer(&format!("msg_{n}"))).collect();
    ledger.record_step(&public_api(), &large);
    assert!(ledger.received_step_from(&public_api(), &large));
    assert!(ledger.received_step_from(&public_api(), &large[..1]));
}

/// A generation is bounded by the pieces it holds as well as by its
/// responses, so a response that carries very much closes it sooner. With
/// generations of three responses a generation is full at 192 pieces: a
/// response of 200 closes its generation alone, and the third response after
/// it forgets it. (Counted by responses alone it would share a generation
/// with the next two and still be known.)
#[test]
fn a_generation_full_of_pieces_closes() {
    let ledger = OpenAiContinuityLedger::with_generation(3);
    let large: Vec<_> = (0..200).map(|n| answer(&format!("msg_{n}"))).collect();
    ledger.record_step(&public_api(), &large);
    assert!(ledger.received_step_from(&public_api(), &large));
    for tag in ["a", "b"] {
        ledger.record_step(&public_api(), &response(tag));
    }
    assert!(ledger.received_step_from(&public_api(), &large));
    ledger.record_step(&public_api(), &response("c"));
    assert!(!ledger.received_step_from(&public_api(), &large));
    for tag in ["a", "b", "c"] {
        assert!(ledger.received_step_from(&public_api(), &response(tag)));
    }
}

/// A stream progress event kept in a step is not material: it is never
/// replayed, so it is neither recorded nor required of a step.
#[test]
fn a_stream_progress_event_is_not_material() {
    let progress = |sequence: u64| AssistantBlock::ServerToolContent {
        id: Some("ws_1".to_string()),
        kind: crate::ServerToolKind::WebSearch,
        content: json!({
            "type": "response.web_search_call.searching",
            "item_id": "ws_1",
            "sequence_number": sequence,
        }),
        meta: None,
    };
    let ledger = OpenAiContinuityLedger::new();
    ledger.record_step(&public_api(), &[progress(1)]);
    assert!(
        !ledger.received_step_from(&public_api(), &[progress(1)]),
        "a step of progress events alone carries no material"
    );
    let mut step = response("a");
    ledger.record_step(&public_api(), &step);
    step.push(progress(7));
    assert!(
        ledger.received_step_from(&public_api(), &step),
        "a progress event the ledger never saw does not make its step unknown"
    );
}

/// The text of the block that opens with `header`, braces matched.
fn block_of<'a>(source: &'a str, header: &str) -> &'a str {
    let start = source.find(header).expect("the block exists");
    let mut depth = 0_usize;
    for (offset, character) in source[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("the block closes");
}

/// The process shares one ledger, so its lock covers lookups and inserts of
/// digests only: a step is serialized and hashed before the lock is taken.
/// The code keeps that shape: the lock is taken in one place, what runs
/// under it sees digests and no material, and each use hands the lock one
/// call on keys that were made before.
#[test]
fn nothing_is_hashed_under_the_ledger_lock() {
    let source = include_str!("../transcript_replay.rs");
    let source = &source[..source.find("#[cfg(test)]").unwrap_or(source.len())];
    assert_eq!(
        source.matches(".lock()").count(),
        1,
        "the ledger's lock is taken in one place"
    );
    let under_lock = block_of(source, "impl OpenAiContinuityLedgerInner {");
    let hashing = [
        "OpenAiContinuityMaterial",
        "continuity_keys",
        "AssistantBlock",
        "ProviderMeta",
        "serde_json",
        "Sha256",
    ];
    for name in hashing {
        assert!(
            !under_lock.contains(name),
            "what runs under the lock does not name {name}"
        );
    }
    let uses: Vec<&str> = source
        .match_indices("self.with_inner(")
        .map(|(at, _)| {
            let call = &source[at..];
            &call[..call.find(';').unwrap_or(call.len())]
        })
        .collect();
    assert_eq!(uses.len(), 2, "record and look up: {uses:?}");
    for call in uses {
        assert!(
            call.starts_with("self.with_inner(|inner| inner."),
            "the lock is handed one call on the ledger's state: {call}"
        );
        for name in hashing.iter().chain(&[".key(", "of_block"]) {
            assert!(
                !call.contains(name),
                "nothing is hashed inside the lock: {call}"
            );
        }
    }
}

/// Metadata of another provider family is none of the ledger's material: it
/// is neither recorded nor required of a step.
#[test]
fn other_families_are_not_ledger_material() {
    let ledger = OpenAiContinuityLedger::new();
    ledger.record_step(&public_api(), &[signed_thinking()]);
    assert!(!ledger.received_step_from(&public_api(), &[signed_thinking()]));
    let mut step = response("a");
    ledger.record_step(&public_api(), &step);
    step.push(signed_thinking());
    assert!(
        ledger.received_step_from(&public_api(), &step),
        "a signed thinking block beside received OpenAI material does not make the step unknown"
    );
}

/// The process ledger is one ledger.
#[test]
fn the_process_ledger_is_shared() {
    let step = response("the_process_ledger_is_shared");
    OpenAiContinuityLedger::process().record_step(&copilot(), &step);
    assert!(OpenAiContinuityLedger::process().received_step_from(&copilot(), &step));
}

/// A panic while the ledger is locked does not take the ledger from the
/// other sessions of the process.
#[test]
fn a_poisoned_lock_is_recovered() {
    let ledger = Arc::new(OpenAiContinuityLedger::new());
    let step = response("a");
    ledger.record_step(&public_api(), &step);
    let poisoner = Arc::clone(&ledger);
    let panicked = std::thread::spawn(move || {
        let _held = poisoner.inner.lock();
        panic!("a session panics while it holds the ledger");
    })
    .join();
    assert!(panicked.is_err());
    assert!(ledger.received_step_from(&public_api(), &step));
    ledger.record_step(&public_api(), &response("b"));
    assert!(ledger.received_step_from(&public_api(), &response("b")));
}

// ---------------------------------------------------------------------------
// The owner: which metadata of a step a target is sent
// ---------------------------------------------------------------------------

/// OpenAI-family metadata is admitted to an OpenAI-wire target only when
/// the process received the step from the service the target is: not when
/// it received it from another service, not when it did not receive it, and
/// not when the target names no service.
#[test]
fn openai_metadata_is_admitted_only_from_the_targets_own_service() {
    let ledger = Arc::new(OpenAiContinuityLedger::new());
    let step = response("a");
    let metadata = metadata_of(&step[0]);
    let admission = |openai: Option<&OpenAiContinuityTarget>| {
        ReplayStepAdmission::of(ReplayWireFamily::OpenAi, openai, &step)
    };

    assert!(
        !admission(Some(&target_of(public_api(), &ledger))).admits(metadata),
        "not received: unknown issuer"
    );
    assert!(!admission(Some(&target_of(public_api(), &ledger))).admits_provider_items());
    ledger.record_step(&public_api(), &step);
    assert!(
        admission(Some(&target_of(public_api(), &ledger))).admits(metadata),
        "received from the target's service"
    );
    assert!(admission(Some(&target_of(public_api(), &ledger))).admits_provider_items());
    for other in [
        OpenAiContinuityIssuer::at_base_url(
            OpenAiContinuityService::OpenAiApi,
            "https://gateway.example",
        ),
        OpenAiContinuityIssuer::at_base_url(
            OpenAiContinuityService::ChatGptBackend,
            "https://api.openai.com",
        ),
        copilot(),
        server("local"),
    ] {
        let other_admission = admission(Some(&target_of(other.clone(), &ledger)));
        assert!(
            !other_admission.admits(metadata) && !other_admission.admits_provider_items(),
            "received from another service than {other:?}"
        );
    }
    assert!(
        !admission(None).admits(metadata) && !admission(None).admits_provider_items(),
        "a target that names no service"
    );
    assert!(
        !admission(Some(&target_of(
            public_api(),
            &Arc::new(OpenAiContinuityLedger::new())
        )))
        .admits(metadata),
        "another process"
    );
}

/// A step is admitted as a whole: one item the process did not receive
/// takes the admission of every item of the step with it.
#[test]
fn a_step_is_admitted_as_a_whole() {
    let ledger = Arc::new(OpenAiContinuityLedger::new());
    let step = response("a");
    ledger.record_step(&public_api(), &step);
    let target = target_of(public_api(), &ledger);
    let whole = ReplayStepAdmission::of(ReplayWireFamily::OpenAi, Some(&target), &step);
    assert!(step.iter().all(|block| whole.admits(metadata_of(block))));

    let mut altered = step.clone();
    altered[0] = reasoning("rs_a", "other bytes");
    let partial = ReplayStepAdmission::of(ReplayWireFamily::OpenAi, Some(&target), &altered);
    assert!(
        altered
            .iter()
            .all(|block| !partial.admits(metadata_of(block))),
        "the received message beside the unknown reasoning item is not admitted either"
    );

    let mut with_unknown_item = step.clone();
    with_unknown_item.push(hosted_search("ws_never"));
    let unknown_item =
        ReplayStepAdmission::of(ReplayWireFamily::OpenAi, Some(&target), &with_unknown_item);
    assert!(
        !unknown_item.admits(metadata_of(&step[0])) && !unknown_item.admits_provider_items(),
        "an unknown hosted-tool item takes the step's metadata with it"
    );
}

/// Metadata of another wire family is never admitted, whatever the ledger
/// knows, and Anthropic and Gemini metadata, which records no issuer, is
/// admitted to its own wire family as before, ledger or no ledger.
#[test]
fn the_wire_family_still_decides_first() {
    let ledger = Arc::new(OpenAiContinuityLedger::new());
    let step = vec![
        reasoning("rs_a", "ciphertext-a"),
        signed_thinking(),
        gemini_signed(),
    ];
    ledger.record_step(&public_api(), &step);
    let target = target_of(public_api(), &ledger);
    let families = [
        ReplayWireFamily::OpenAi,
        ReplayWireFamily::Anthropic,
        ReplayWireFamily::Gemini,
    ];
    for target_family in families {
        let admission = ReplayStepAdmission::of(target_family, Some(&target), &step);
        for (block, family) in step.iter().zip(families) {
            assert_eq!(
                admission.admits(metadata_of(block)),
                family == target_family,
                "{family:?} metadata to a {target_family:?} target"
            );
        }
        let without_ledger = ReplayStepAdmission::of(target_family, None, &step);
        assert!(
            !without_ledger.admits(metadata_of(&step[0])),
            "OpenAI metadata to a {target_family:?} target that names no service"
        );
        for (block, family) in step.iter().zip(families).skip(1) {
            assert_eq!(
                without_ledger.admits(metadata_of(block)),
                family == target_family,
                "{family:?} metadata needs no ledger"
            );
        }
        assert_eq!(
            without_ledger.admits_provider_items(),
            target_family != ReplayWireFamily::OpenAi,
            "{target_family:?} hosted-tool items"
        );
    }
}

fn openai_target() -> ReplayTarget {
    ReplayTarget::new(
        ReplayWireFamily::OpenAi,
        true,
        false,
        true,
        ReplayToolResultProjection::PreserveBlocks,
        ReplayReasoningProjection::ProviderNative,
    )
}

fn metadata_dispositions(plan: &ReplayPlan) -> Vec<ReplayDisposition> {
    plan.entries()
        .filter(|entry| matches!(entry.subject, ReplaySubject::ProviderMetadata { .. }))
        .map(|entry| entry.disposition)
        .collect()
}

fn assistant(blocks: Vec<AssistantBlock>) -> Message {
    Message::BlockAssistant(crate::BlockAssistantMessage::new(
        blocks,
        crate::StopReason::EndTurn,
    ))
}

/// The plan decides each step once: in a transcript of a stored step and a
/// received one, the stored step's metadata is omitted and the received
/// step's is the provider's own, and a plan built without a service omits
/// both.
#[test]
fn the_plan_admits_received_steps_and_omits_the_rest() {
    let ledger = Arc::new(OpenAiContinuityLedger::new());
    let (stored, received) = (response("stored"), response("received"));
    ledger.record_step(&public_api(), &received);
    let messages = [assistant(stored.clone()), assistant(received.clone())];
    let target = target_of(public_api(), &ledger);
    let plan = ReplayPlan::build_with_openai_continuity(&messages, openai_target(), Some(&target))
        .expect("plan");
    assert_eq!(
        metadata_dispositions(&plan),
        [
            ReplayDisposition::Omit,
            ReplayDisposition::Omit,
            ReplayDisposition::ProviderNative,
            ReplayDisposition::ProviderNative,
        ]
    );
    assert!(
        !plan
            .step_admission(ReplayMessageIndex(0))
            .admits(metadata_of(&stored[0]))
    );
    assert!(
        plan.step_admission(ReplayMessageIndex(1))
            .admits(metadata_of(&received[0]))
    );

    let unnamed = ReplayPlan::build(&messages, openai_target()).expect("plan");
    assert_eq!(
        metadata_dispositions(&unnamed),
        [ReplayDisposition::Omit; 4]
    );
}

/// A provider adapter that never asks the plan and keeps OpenAI-family
/// metadata on a step the plan did not admit does not pass the plan's
/// validation: the admission is an input of the plan, not a relaxation of
/// its checks.
#[test]
fn a_projection_that_keeps_unadmitted_metadata_is_refused() {
    let ledger = Arc::new(OpenAiContinuityLedger::new());
    let stored = vec![answer("msg_stored")];
    let messages = [assistant(stored.clone())];
    let target = target_of(public_api(), &ledger);
    let plan = ReplayPlan::build_with_openai_continuity(&messages, openai_target(), Some(&target))
        .expect("plan");
    let subject = ReplaySubject::AssistantBlock {
        message: ReplayMessageIndex(0),
        block: ReplayAssistantBlockIndex(0),
    };
    let metadata_subject = ReplaySubject::ProviderMetadata {
        message: ReplayMessageIndex(0),
        block: ReplayAssistantBlockIndex(0),
    };

    // The adapter that forgot: it hands the block back with its metadata.
    let mut application = plan.application();
    application
        .record_message(
            ReplaySubject::Message(ReplayMessageIndex(0)),
            &messages[0],
            None,
        )
        .expect("message");
    application
        .record_assistant(subject, &stored[0], Some(&stored[0]))
        .expect("the payload is unchanged");
    assert!(matches!(
        application.record_provider_metadata(
            metadata_subject,
            metadata_of(&stored[0]),
            &stored[0],
            Some(&stored[0]),
        ),
        Err(ReplayPlanError::ProjectionMismatch { .. })
    ));

    // And one that records the dropped form but returns the transcript
    // untouched: the final validation compares the projected transcript
    // with the plan.
    assert!(
        plan.validate_projected(&messages, &messages, recorded_as_dropped(&plan, &messages))
            .is_err()
    );
}

/// `message` with the blocks `blocks` in place of its own.
fn with_blocks(message: &Message, blocks: Vec<AssistantBlock>) -> Message {
    let Message::BlockAssistant(step) = message else {
        panic!("an assistant step");
    };
    let mut step = step.clone();
    step.blocks = blocks;
    Message::BlockAssistant(step)
}

/// The same holds for a hosted-tool item, whose provider-issued id is in
/// its content and not in metadata: a projection that keeps the item of a
/// step the plan did not admit is refused, although every block's metadata
/// is in the dropped form. The same projection of a step the process
/// received from the target is accepted.
#[test]
fn a_projection_that_keeps_an_unadmitted_hosted_tool_item_is_refused() {
    let stored = vec![hosted_search("ws_stored"), answer("msg_stored")];
    let messages = [assistant(stored.clone())];
    for received in [false, true] {
        let ledger = Arc::new(OpenAiContinuityLedger::new());
        if received {
            ledger.record_step(&public_api(), &stored);
        }
        let target = target_of(public_api(), &ledger);
        let plan =
            ReplayPlan::build_with_openai_continuity(&messages, openai_target(), Some(&target))
                .expect("plan");
        let admission = plan.step_admission(ReplayMessageIndex(0));
        assert_eq!(admission.admits_provider_items(), received);
        // Every block in place, its metadata as the plan asks.
        let kept = [with_blocks(
            &messages[0],
            stored
                .iter()
                .cloned()
                .map(|block| admission.strip_foreign_metadata(block))
                .collect(),
        )];
        assert_eq!(
            plan.validate_projected(&messages, &kept, recorded_as_dropped(&plan, &messages))
                .is_ok(),
            received,
            "received {received}: a kept hosted-tool item passes only in an admitted step"
        );
    }
}

/// An application of `plan` that recorded every entry as the plan asks, so
/// only the final projection check remains.
fn recorded_as_dropped<'a>(plan: &'a ReplayPlan, messages: &[Message]) -> ReplayApplication<'a> {
    let mut application = plan.application();
    for (index, message) in messages.iter().enumerate() {
        let index = ReplayMessageIndex(index);
        application
            .record_message(ReplaySubject::Message(index), message, None)
            .expect("message");
        let Message::BlockAssistant(step) = message else {
            continue;
        };
        for (block_index, block) in step.blocks.iter().enumerate() {
            let block_index = ReplayAssistantBlockIndex(block_index);
            let stripped = plan
                .step_admission(index)
                .strip_foreign_metadata(block.clone());
            application
                .record_assistant(
                    ReplaySubject::AssistantBlock {
                        message: index,
                        block: block_index,
                    },
                    block,
                    Some(&stripped),
                )
                .expect("block");
            if let Some(metadata) = replay_provider_metadata(block) {
                application
                    .record_provider_metadata(
                        ReplaySubject::ProviderMetadata {
                            message: index,
                            block: block_index,
                        },
                        metadata,
                        block,
                        Some(&stripped),
                    )
                    .expect("metadata");
            }
        }
    }
    application
}

/// Anthropic and Gemini plans are what they were: built without any OpenAI
/// service, they keep their own family's metadata and omit the others'.
#[test]
fn anthropic_and_gemini_plans_are_unchanged() {
    let messages = [assistant(vec![
        reasoning("rs_a", "ciphertext-a"),
        signed_thinking(),
        gemini_signed(),
    ])];
    for (family, expected) in [
        (
            ReplayWireFamily::Anthropic,
            [
                ReplayDisposition::Omit,
                ReplayDisposition::ProviderNative,
                ReplayDisposition::Omit,
            ],
        ),
        (
            ReplayWireFamily::Gemini,
            [
                ReplayDisposition::Omit,
                ReplayDisposition::Omit,
                ReplayDisposition::ProviderNative,
            ],
        ),
    ] {
        let target = ReplayTarget::new(
            family,
            true,
            false,
            true,
            ReplayToolResultProjection::PreserveBlocks,
            ReplayReasoningProjection::ProviderNative,
        );
        let plan = ReplayPlan::build(&messages, target).expect("plan");
        assert_eq!(metadata_dispositions(&plan), expected, "{family:?}");
    }
}

/// Stripping by wire family alone: an OpenAI-wire target that names no
/// service keeps no OpenAI metadata, and the other families keep their own.
#[test]
fn stripping_by_family_keeps_no_openai_metadata() {
    let stripped = ReplayWireFamily::OpenAi.strip_foreign_metadata(reasoning("rs_a", "bytes"));
    assert!(replay_provider_metadata(&stripped).is_none());
    let kept = ReplayWireFamily::Anthropic.strip_foreign_metadata(signed_thinking());
    assert!(replay_provider_metadata(&kept).is_some());
    let foreign = ReplayWireFamily::Anthropic.strip_foreign_metadata(reasoning("rs_a", "bytes"));
    assert!(replay_provider_metadata(&foreign).is_none());
    let gemini = ReplayWireFamily::Gemini.strip_foreign_metadata(gemini_signed());
    assert!(replay_provider_metadata(&gemini).is_some());
}
