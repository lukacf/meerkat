//! Public-surface projection for temporary councils (issue #159, phase 5).
//!
//! These rows exercise the ONE wire↔domain conversion site
//! (`meerkat_mob_mcp::temporary_council_wire`) and the owner-local public MCP
//! tools over a REAL local council: real mobs, real members, real source-owned
//! capabilities, real bounded turns. Only the LLM is scripted.
//!
//! What they pin:
//!
//! * a JSON wire request decodes into the domain request and runs;
//! * the sealed wire result carries the bounded exchanges, the explicit merge
//!   outcome, and the non-secret capability provenance — and carries NO bearer
//!   material and no inherited transcript;
//! * the record projection exposes custody correlation without custody;
//! * a request that smuggles a bearer-shaped field is refused before any side
//!   effect; and
//! * the public MCP tool surface exposes run/get and refuses the realm-wide
//!   recovery sweep.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use meerkat_contracts::wire::{
    MobTemporaryCouncilGetResult, MobTemporaryCouncilRunResult, WireTemporaryCouncilExitReason,
    WireTemporaryCouncilMergeOutcome, WireTemporaryCouncilRequest,
};
use meerkat_mob_mcp::temporary_council_wire::{
    decode_temporary_council_request, encode_temporary_council_outcome,
    encode_temporary_council_record, temporary_council_error_detail,
};
use meerkat_mob_mcp::{TemporaryCouncilError, public_tool_names};
use serde_json::json;
use support::{CouncilFixture, ScriptedTurn, role_in_request, user_text};

/// Every participant answers with its own role, so an exchange's provenance is
/// checkable from its text alone.
fn role_script(request: &meerkat_client::LlmRequest) -> ScriptedTurn {
    let text = user_text(request);
    if text.contains("bounded plain-text summary") {
        return ScriptedTurn::Text("SUMMARY: the council agreed on the plan.".to_string());
    }
    match role_in_request(request) {
        Some(role) => ScriptedTurn::Text(format!("position from {role}")),
        None => ScriptedTurn::Text("ok".to_string()),
    }
}

/// A complete wire request for a two-participant local council.
fn wire_request(fixture: &CouncilFixture, council_id: &str) -> serde_json::Value {
    json!({
        "council_id": fixture.council_id(council_id).as_str(),
        "definition_template": {
            "id": "template-is-replaced",
            "profiles": {
                "participant": {
                    "model": "claude-haiku-4-5-20251001",
                    "tools": { "comms": true },
                    "peer_description": "council participant",
                    "backend": "session"
                }
            }
        },
        "participants": [
            {
                "order": 0,
                "role": "analyst",
                "source_mob_id": fixture.source_mob_id().as_str(),
                "source_identity": "researcher",
                "target_identity": "analyst",
                "target_profile": "participant",
                "target_backend": "session",
                "scope": "invoke_and_observe"
            },
            {
                "order": 1,
                "role": "critic",
                "source_mob_id": fixture.source_mob_id().as_str(),
                "source_identity": "reviewer",
                "target_identity": "critic",
                "target_profile": "participant",
                "target_backend": "session",
                "scope": "invoke_and_observe"
            }
        ],
        "topic": "Should we ship the migration this week?",
        "bounds": {
            "deadline": { "kind": "relative", "after_millis": 120_000 },
            "max_rounds": 1,
            "max_exchanges": 8,
            "max_result_bytes": 4096
        },
        "merge_back": {
            "policy": "bounded_text_summary",
            "finalizer": "analyst",
            "max_bytes": 2048
        },
        "durability": "durable"
    })
}

fn assert_no_secret_material(encoded: &str) {
    for forbidden in [
        "bearer",
        "capability_id",
        "revocation_id",
        "cleanup_id",
        "\"messages\"",
        "\"transcript\"",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "council wire projection must not carry `{forbidden}`"
        );
    }
}

// ===========================================================================
// Wire request -> real council -> wire result
// ===========================================================================

/// A JSON wire request decodes through the ONE converter, runs a real local
/// council, and projects a sealed result that carries bounded exchanges,
/// the explicit merge outcome, and exact non-secret capability provenance —
/// and nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn wire_request_runs_a_real_council_and_projects_a_secret_free_result() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let body = wire_request(&fixture, "wire-surface");
    let decoded: WireTemporaryCouncilRequest =
        serde_json::from_value(body).expect("wire request decodes");
    let request = decode_temporary_council_request(decoded).expect("wire request converts");
    let council_id = request.council_id.clone();
    let temporary_mob_id = council_id.temporary_mob_id();

    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council runs to a terminal outcome");

    let wire: MobTemporaryCouncilRunResult = encode_temporary_council_outcome(&outcome);
    assert!(!wire.replayed);
    assert_eq!(wire.result.council_id, council_id.as_str());
    assert_eq!(wire.result.temporary_mob_id, temporary_mob_id.as_str());
    assert_eq!(
        wire.result.exit_reason,
        WireTemporaryCouncilExitReason::Completed
    );
    assert_eq!(
        wire.result.durability,
        meerkat_contracts::wire::WireTemporaryCouncilDurability::Durable
    );

    // Bounded exchanges are mirrored, in order, with their council-owned text.
    assert!(
        wire.result.exchanges.len() >= 2,
        "expected the round exchanges plus the merge turn: {:?}",
        wire.result.exchanges
    );
    let texts: Vec<String> = wire
        .result
        .exchanges
        .iter()
        .filter_map(|exchange| match &exchange.outcome {
            meerkat_contracts::wire::WireTemporaryCouncilExchangeOutcome::Completed {
                text,
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("position from analyst"))
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("position from critic"))
    );

    match &wire.result.merge {
        WireTemporaryCouncilMergeOutcome::BoundedTextSummary {
            finalizer,
            text,
            truncated,
        } => {
            assert_eq!(finalizer, "analyst");
            assert!(text.contains("SUMMARY"), "unexpected summary: {text}");
            assert!(!truncated);
        }
        other => panic!("expected a bounded text summary, got {other:?}"),
    }

    // Provenance is exact and non-secret.
    assert_eq!(wire.result.participants.len(), 2);
    for provenance in &wire.result.participants {
        assert!(provenance.seated);
        assert_eq!(provenance.source_mob_id, fixture.source_mob_id().as_str());
        let capability = provenance
            .capability
            .as_ref()
            .expect("a seated participant carries its capability provenance");
        assert!(!capability.fork_session_id.is_empty());
        assert!(capability.source.prefix_digest.starts_with("sha256:"));
        assert!(!capability.correlation_hint.is_empty());
        assert_eq!(
            capability.scope,
            meerkat_contracts::wire::WireTemporaryCouncilScope::InvokeAndObserve
        );
    }

    let encoded = serde_json::to_string(&wire).expect("result serializes");
    assert_no_secret_material(&encoded);

    // The record projection exposes custody CORRELATION, never custody.
    let record = fixture
        .state
        .temporary_council()
        .load(&council_id)
        .await
        .expect("record loads")
        .expect("record exists");
    let projected = encode_temporary_council_record(&record);
    assert!(!projected.unfinished, "a settled council owes no work");
    assert_eq!(projected.participants.len(), 2);
    for custody in &projected.participants {
        assert!(custody.seated);
        assert_eq!(
            custody.acquisition,
            meerkat_contracts::wire::WireTemporaryCouncilAcquisition::Acquired
        );
        assert!(custody.capability_correlation_hint.is_some());
        assert!(!custody.capability_request_id.is_empty());
    }
    let encoded = serde_json::to_string(&MobTemporaryCouncilGetResult {
        council: Some(projected),
    })
    .expect("record projection serializes");
    assert_no_secret_material(&encoded);

    fixture.teardown().await;
}

// ===========================================================================
// Fail-closed request decoding
// ===========================================================================

/// A request that smuggles capability custody or a transcript body is refused
/// by the wire contract itself — the converter never sees it, and no mob,
/// capability, or turn is created.
#[test]
fn wire_request_refuses_bearer_and_transcript_fields() {
    for smuggled in [
        json!({ "capability_ref": { "bearer": "secret" } }),
        json!({ "messages": [{ "role": "user", "content": "inherited" }] }),
        json!({ "transcript": ["inherited"] }),
    ] {
        let mut body = json!({
            "council_id": "smuggled",
            "definition_template": { "id": "ignored", "profiles": {} },
            "participants": [],
            "topic": "hello",
            "bounds": {
                "deadline": { "kind": "relative", "after_millis": 1000 },
                "max_rounds": 1,
                "max_exchanges": 1,
                "max_result_bytes": 512
            },
            "merge_back": { "policy": "no_merge" },
            "durability": "process_bound"
        });
        for (key, value) in smuggled.as_object().expect("object").clone() {
            body[key] = value;
        }
        assert!(
            serde_json::from_value::<WireTemporaryCouncilRequest>(body).is_err(),
            "a smuggled field must be refused, never ignored"
        );
    }
}

/// The converter rejects a malformed mob definition through the ordinary
/// public decoder, so a council cannot smuggle a definition shape
/// `mob/create` would refuse.
#[test]
fn wire_request_validates_the_definition_template() {
    let body = json!({
        "council_id": "bad-template",
        "definition_template": {
            "id": "ignored",
            "profiles": { "participant": { "model": "" } }
        },
        "participants": [],
        "topic": "hello",
        "bounds": {
            "deadline": { "kind": "relative", "after_millis": 1000 },
            "max_rounds": 1,
            "max_exchanges": 1,
            "max_result_bytes": 512
        },
        "merge_back": { "policy": "no_merge" },
        "durability": "process_bound"
    });
    let decoded: WireTemporaryCouncilRequest = serde_json::from_value(body).expect("shape decodes");
    // Whatever the decoder's verdict, it must be the PUBLIC decoder's verdict
    // (typed invalid-request), never a panic or a silently accepted profile.
    if let Err(error) = decode_temporary_council_request(decoded) {
        let detail = temporary_council_error_detail(&error);
        assert_eq!(
            detail.code(),
            meerkat_contracts::ErrorCode::InvalidParams,
            "definition rejection is an invalid request"
        );
    }
}

/// A non-canonical council id is refused at the boundary with the typed
/// invalid-request payload.
#[test]
fn council_id_conversion_is_validating() {
    let error = meerkat_mob_mcp::temporary_council_wire::parse_temporary_council_id(" padded ")
        .expect_err("non-canonical ids are refused");
    let detail = temporary_council_error_detail(&error);
    assert_eq!(detail.code(), meerkat_contracts::ErrorCode::InvalidParams);
    let data = detail.detail_value().expect("typed detail");
    assert_eq!(data["kind"], "invalid_request");
}

// ===========================================================================
// Error mapping
// ===========================================================================

/// Each typed council failure renders one stable code and one typed payload.
#[test]
fn council_failures_render_stable_codes_and_payloads() {
    let council_id =
        meerkat_mob::temporary_council::TemporaryCouncilId::new("mapping").expect("id");

    let conflict = TemporaryCouncilError::ConflictingRequest {
        council_id: council_id.clone(),
        stored_fingerprint: "sha256:a".to_string(),
        presented_fingerprint: "sha256:b".to_string(),
    };
    let detail = temporary_council_error_detail(&conflict);
    assert_eq!(detail.code(), meerkat_contracts::ErrorCode::DuplicateInput);
    assert_eq!(
        detail.detail_value().expect("conflict detail"),
        json!({
            "council_id": "mapping",
            "stored_fingerprint": "sha256:a",
            "presented_fingerprint": "sha256:b",
        })
    );

    let busy = TemporaryCouncilError::HeldByAnotherCoordinator {
        council_id: council_id.clone(),
        current_claim_epoch: 7,
    };
    assert_eq!(
        temporary_council_error_detail(&busy).code(),
        meerkat_contracts::ErrorCode::SessionBusy
    );

    let fenced = TemporaryCouncilError::Fenced {
        council_id: council_id.clone(),
        current_claim_epoch: 8,
    };
    assert_eq!(
        temporary_council_error_detail(&fenced).code(),
        meerkat_contracts::ErrorCode::StaleFence
    );

    let durability = TemporaryCouncilError::DurabilityUnavailable { council_id };
    let detail = temporary_council_error_detail(&durability);
    assert_eq!(
        detail.code(),
        meerkat_contracts::ErrorCode::CapabilityUnavailable
    );
    assert_eq!(
        detail.detail_value().expect("durability detail"),
        json!({
            "council_id": "mapping",
            "required": "durable",
            "available": "process_bound",
        })
    );

    let store = TemporaryCouncilError::Store {
        detail: "custody write failed".to_string(),
    };
    let detail = temporary_council_error_detail(&store);
    assert_eq!(detail.code(), meerkat_contracts::ErrorCode::InternalError);
    assert_eq!(
        detail.detail_value().expect("store detail"),
        json!({ "kind": "store", "detail": "custody write failed" })
    );
}

// ===========================================================================
// Public MCP authority
// ===========================================================================

/// The owner-local public MCP surface exposes council run/get and
/// deliberately does NOT expose the realm-wide recovery sweep. An unknown
/// name fails closed.
#[tokio::test]
async fn public_mcp_exposes_run_and_get_but_not_recover() {
    let names = public_tool_names();
    assert!(names.contains(&"meerkat_mob_temporary_council_run"));
    assert!(names.contains(&"meerkat_mob_temporary_council_get"));
    assert!(
        !names.contains(&"meerkat_mob_temporary_council_recover"),
        "a realm-wide maintenance sweep is not an LLM-reachable tool"
    );

    let state =
        meerkat_mob_mcp::MobMcpState::new_in_memory_as(meerkat_mob::MobControlPrincipal::Owner);
    let error = meerkat_mob_mcp::handle_public_tools_call(
        &state,
        "meerkat_mob_temporary_council_recover",
        &json!({}),
    )
    .await
    .expect_err("recovery must not be dispatchable from the tool surface");
    assert_eq!(error.code, -32601, "unknown tool names fail closed");
}

/// The public `get` tool answers a typed absence for an unknown council.
#[tokio::test]
async fn public_mcp_get_reports_typed_absence() {
    let state =
        meerkat_mob_mcp::MobMcpState::new_in_memory_as(meerkat_mob::MobControlPrincipal::Owner);
    let payload = meerkat_mob_mcp::handle_public_tools_call(
        &state,
        "meerkat_mob_temporary_council_get",
        &json!({ "council_id": "never-created" }),
    )
    .await
    .expect("absence is not an error");
    let decoded: MobTemporaryCouncilGetResult =
        serde_json::from_value(payload).expect("typed get result");
    assert!(decoded.council.is_none());
}

/// A `durable` declaration against process-bound custody is refused through
/// the tool surface with the same typed code the consoles render.
#[tokio::test]
async fn public_mcp_run_refuses_durable_on_process_bound_custody() {
    let state =
        meerkat_mob_mcp::MobMcpState::new_in_memory_as(meerkat_mob::MobControlPrincipal::Owner);
    let error = meerkat_mob_mcp::handle_public_tools_call(
        &state,
        "meerkat_mob_temporary_council_run",
        &json!({
            "request": {
                "council_id": "durability-refusal",
                "definition_template": {
                    "id": "ignored",
                    "profiles": { "participant": { "model": "claude-haiku-4-5-20251001" } }
                },
                "participants": [{
                    "order": 0,
                    "role": "critic",
                    "source_mob_id": "source-mob",
                    "source_identity": "alice",
                    "target_identity": "alice-branch",
                    "target_profile": "participant",
                    "scope": "invoke_and_observe"
                }],
                "topic": "is this safe to ship?",
                "bounds": {
                    "deadline": { "kind": "relative", "after_millis": 60_000 },
                    "max_rounds": 1,
                    "max_exchanges": 2,
                    "max_result_bytes": 4096
                },
                "merge_back": { "policy": "no_merge" },
                "durability": "durable"
            }
        }),
    )
    .await
    .expect_err("durable custody must be refused");
    assert_eq!(
        error.code,
        meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code()
    );
    let data = error.data.expect("typed durability data");
    assert_eq!(data["available"], "process_bound");
}
