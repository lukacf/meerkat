//! Gate 0 — durable representation contracts for the model-routing handoff log.
//!
//! These tests pin the *representation* before any behavior exists: the record
//! shapes, the waiting/terminal split, the idempotence and conflict rules, the
//! authenticated HeadCanonical carrier, and the compatibility evidence for
//! carrying the log inside the existing v3 session envelope.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::image_generation::{
    SwitchTurnDenialReason, SwitchTurnDuration, SwitchTurnIntent, SwitchTurnOrigin,
    SwitchTurnReasonTextDisposition, SwitchTurnRequestId,
};
use meerkat_core::lifecycle::identifiers::RunId;
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::session::model_routing_control::{
    ModelRoutingControlAppendError, ModelRoutingControlAppendOutcome,
    ModelRoutingIntentRecordDisposition, SessionModelRoutingControlHistory,
    SessionModelRoutingControlRecord,
};
use meerkat_core::{Message, Session, SessionLlmIdentity, UserMessage};

fn new_request_id() -> SwitchTurnRequestId {
    SwitchTurnRequestId::new(uuid::Uuid::new_v4())
}

fn user_message() -> Message {
    Message::User(UserMessage::text("hello".to_string()))
}

fn intent(model: &str) -> SwitchTurnIntent {
    SwitchTurnIntent {
        target_model: ModelId::new(model),
        duration: SwitchTurnDuration::UntilChanged,
        origin: SwitchTurnOrigin::Model {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        },
    }
}

fn identity(model: &str) -> Box<SessionLlmIdentity> {
    Box::new(SessionLlmIdentity {
        model: model.to_string(),
        provider: meerkat_core::Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    })
}

fn requested(
    request_id: &SwitchTurnRequestId,
    run: &RunId,
    model: &str,
) -> SessionModelRoutingControlRecord {
    SessionModelRoutingControlRecord::request(*request_id, run.clone(), intent(model))
        .expect("until-changed model-origin intent is a durable handoff")
}

fn realized(
    request_id: &SwitchTurnRequestId,
    run: &RunId,
    model: &str,
) -> SessionModelRoutingControlRecord {
    SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
        request_id: *request_id,
        originating_run_id: run.clone(),
        intent: intent(model),
        applied_identity: identity(model),
    }
}

// ---------------------------------------------------------------------------
// Record shape and serde
// ---------------------------------------------------------------------------

#[test]
fn every_record_variant_round_trips_through_json() {
    let request_id = new_request_id();
    let run = RunId::new();
    let variants = vec![
        requested(&request_id, &run, "claude-opus-5"),
        SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
            request_id,
            originating_run_id: run.clone(),
            intent: intent("claude-opus-5"),
            applied_identity: identity("claude-opus-5"),
        },
        SessionModelRoutingControlRecord::ModelRoutingIntentDenied {
            request_id,
            originating_run_id: run.clone(),
            intent: intent("claude-opus-5"),
            reason: SwitchTurnDenialReason::UnsupportedModel,
        },
        SessionModelRoutingControlRecord::ModelRoutingIntentAbandoned {
            request_id,
            originating_run_id: run.clone(),
            intent: intent("claude-opus-5"),
        },
    ];

    for record in variants {
        let encoded = serde_json::to_value(&record).expect("record serializes");
        let decoded: SessionModelRoutingControlRecord =
            serde_json::from_value(encoded.clone()).expect("record deserializes");
        assert_eq!(
            decoded, record,
            "record must round-trip losslessly: {encoded}"
        );
        assert_eq!(
            decoded.disposition(),
            record.disposition(),
            "disposition must survive the round-trip"
        );
    }
}

#[test]
fn a_record_with_an_unknown_field_is_rejected() {
    let mut encoded = serde_json::to_value(requested(&new_request_id(), &RunId::new(), "model-a"))
        .expect("record serializes");
    encoded
        .as_object_mut()
        .expect("record is an object")
        .insert("future_semantic_fact".to_string(), json_true());

    let error = serde_json::from_value::<SessionModelRoutingControlRecord>(encoded)
        .expect_err("unknown durable record fields must fail closed");
    assert!(
        error.to_string().contains("unknown field"),
        "strict decode must name the unknown field: {error}"
    );
}

/// A request naming no model can never be realized.
///
/// The generated record constructors already refuse an empty target, so
/// admitting one durably would mint committed state that machine authority can
/// never accept — a request permanently owed and permanently unrealizable.
#[test]
fn a_request_with_an_empty_target_model_is_typed_refused() {
    let request_id = new_request_id();
    let error = SessionModelRoutingControlRecord::request(request_id, RunId::new(), intent(""))
        .expect_err("an empty target names no routing change");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::EmptyTargetModel { request_id: refused }
                if refused == request_id
        ),
        "the refusal must name the empty target and the request it belongs to: {error:?}"
    );
}

/// The constructor is not the only ingress: a persisted document is decoded
/// straight into a history. Both paths must agree, or a log could carry a
/// record the live path would have refused.
#[test]
fn a_persisted_envelope_with_an_empty_target_model_is_refused() {
    let request_id = new_request_id();
    let run = RunId::new();
    let mut encoded =
        serde_json::to_value(requested(&request_id, &run, "model-a")).expect("record serializes");
    encoded
        .as_object_mut()
        .expect("record is an object")
        .get_mut("intent")
        .expect("record carries its intent")
        .as_object_mut()
        .expect("intent is an object")
        .insert("target_model".to_string(), serde_json::json!(""));

    let record = serde_json::from_value::<SessionModelRoutingControlRecord>(encoded)
        .expect("the record shape itself is still well formed");
    let error = SessionModelRoutingControlHistory::from_records(vec![record])
        .expect_err("a persisted empty target must not decode into a usable history");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::EmptyTargetModel { request_id: refused }
                if refused == request_id
        ),
        "materializing a persisted log must refuse the empty target: {error:?}"
    );
}

/// The mirror of `awaiting_decision`, and the input to crash-window
/// reconciliation: only the newest record per request, and only when it is
/// terminal.
#[test]
fn settled_terminals_reports_only_the_newest_terminal_per_request() {
    let settled_id = new_request_id();
    let pending_id = new_request_id();
    let run = RunId::new();
    let history = SessionModelRoutingControlHistory::from_records(vec![
        requested(&settled_id, &run, "model-b"),
        requested(&pending_id, &run, "model-c"),
        realized(&settled_id, &run, "model-b"),
    ])
    .expect("a coherent log materializes");

    let settled: Vec<_> = history.settled_terminals().collect();
    assert_eq!(settled.len(), 1, "only one request has terminalized");
    assert_eq!(settled[0].request_id(), &settled_id);
    assert_eq!(
        settled[0].disposition(),
        ModelRoutingIntentRecordDisposition::Realized,
        "the newest record is the one reconciliation must read"
    );

    let awaiting: Vec<_> = history.awaiting_decision().collect();
    assert_eq!(awaiting.len(), 1, "the other request is still owed");
    assert_eq!(
        awaiting[0].request_id(),
        &pending_id,
        "a settled request must never appear as still owed"
    );
}

#[test]
fn record_tag_names_the_model_routing_domain() {
    let request_id = new_request_id();
    let run = RunId::new();
    let records = [
        (
            requested(&request_id, &run, "gpt-5.5"),
            "model_routing_intent_requested",
        ),
        (
            realized(&request_id, &run, "gpt-5.5"),
            "model_routing_intent_realized",
        ),
        (
            SessionModelRoutingControlRecord::ModelRoutingIntentDenied {
                request_id,
                originating_run_id: run.clone(),
                intent: intent("gpt-5.5"),
                reason: SwitchTurnDenialReason::UnsupportedModel,
            },
            "model_routing_intent_denied",
        ),
        (
            SessionModelRoutingControlRecord::ModelRoutingIntentAbandoned {
                request_id,
                originating_run_id: run,
                intent: intent("gpt-5.5"),
            },
            "model_routing_intent_abandoned",
        ),
    ];
    for (record, expected_tag) in records {
        let encoded = serde_json::to_value(&record).expect("record serializes");
        assert_eq!(
            encoded["record"], expected_tag,
            "durable tags must use stable model-routing nouns"
        );
        assert!(encoded["record"].is_string(), "record tags are strings");
    }
}

#[test]
fn record_disposition_distinguishes_waiting_from_terminal() {
    assert!(ModelRoutingIntentRecordDisposition::Requested.is_awaiting_decision());
    assert!(!ModelRoutingIntentRecordDisposition::Requested.is_terminal());
    for terminal in [
        ModelRoutingIntentRecordDisposition::Realized,
        ModelRoutingIntentRecordDisposition::Denied,
        ModelRoutingIntentRecordDisposition::Abandoned,
    ] {
        assert!(terminal.is_terminal(), "{terminal:?} must be terminal");
        assert!(
            !terminal.is_awaiting_decision(),
            "{terminal:?} must not read as still waiting"
        );
    }
}

// ---------------------------------------------------------------------------
// Append rules: idempotence, conflict, terminality
// ---------------------------------------------------------------------------

#[test]
fn exact_duplicate_request_is_idempotent() {
    let request_id = new_request_id();
    let run = RunId::new();
    let mut history = SessionModelRoutingControlHistory::new();

    assert_eq!(
        history
            .append(requested(&request_id, &run, "claude-opus-5"))
            .expect("first request appends"),
        ModelRoutingControlAppendOutcome::Appended
    );
    assert_eq!(
        history
            .append(requested(&request_id, &run, "claude-opus-5"))
            .expect("identical request is idempotent"),
        ModelRoutingControlAppendOutcome::AlreadyRecorded
    );
    assert_eq!(
        history.len(),
        1,
        "an idempotent append must not grow the log"
    );
}

#[test]
fn same_request_id_different_target_is_typed_conflict() {
    let request_id = new_request_id();
    let run = RunId::new();
    let mut history = SessionModelRoutingControlHistory::new();
    history
        .append(requested(&request_id, &run, "claude-opus-5"))
        .expect("first request appends");

    let error = history
        .append(requested(&request_id, &run, "gpt-5.5"))
        .expect_err("a conflicting target must be refused");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::ConflictingIntent { .. }
        ),
        "expected ConflictingIntent, got {error:?}"
    );
    assert_eq!(history.len(), 1, "a refused append must not mutate the log");
}

#[test]
fn same_request_id_different_originating_run_is_typed_conflict() {
    let request_id = new_request_id();
    let mut history = SessionModelRoutingControlHistory::new();
    history
        .append(requested(&request_id, &RunId::new(), "claude-opus-5"))
        .expect("first request appends");

    let error = history
        .append(requested(&request_id, &RunId::new(), "claude-opus-5"))
        .expect_err("a different originating run must be refused");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::ConflictingIntent { .. }
        ),
        "expected ConflictingIntent, got {error:?}"
    );
}

#[test]
fn terminal_record_requires_a_committed_request() {
    let request_id = new_request_id();
    let mut history = SessionModelRoutingControlHistory::new();
    let error = history
        .append(
            SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
                request_id,
                originating_run_id: RunId::new(),
                intent: intent("claude-opus-5"),
                applied_identity: identity("claude-opus-5"),
            },
        )
        .expect_err("a terminal for an unknown request must be refused");
    assert!(
        matches!(error, ModelRoutingControlAppendError::UnknownRequest { .. }),
        "expected UnknownRequest, got {error:?}"
    );
}

#[test]
fn exact_terminal_replay_is_idempotent_but_a_different_terminal_is_refused() {
    let request_id = new_request_id();
    let run = RunId::new();
    let mut history = SessionModelRoutingControlHistory::new();
    history
        .append(requested(&request_id, &run, "claude-opus-5"))
        .expect("request appends");

    let realized = SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
        request_id,
        originating_run_id: run.clone(),
        intent: intent("claude-opus-5"),
        applied_identity: identity("claude-opus-5"),
    };
    assert_eq!(
        history.append(realized.clone()).expect("realized appends"),
        ModelRoutingControlAppendOutcome::Appended
    );
    // Crash-retry replays the identical realized record: never a second
    // rotation, never a duplicate row.
    assert_eq!(
        history
            .append(realized)
            .expect("identical realized replay is idempotent"),
        ModelRoutingControlAppendOutcome::AlreadyRecorded
    );

    let error = history
        .append(SessionModelRoutingControlRecord::ModelRoutingIntentDenied {
            request_id,
            originating_run_id: run,
            intent: intent("claude-opus-5"),
            reason: SwitchTurnDenialReason::UnsupportedModel,
        })
        .expect_err("a second, different terminal must be refused");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::AfterTerminal {
                disposition: ModelRoutingIntentRecordDisposition::Realized,
                ..
            }
        ),
        "expected AfterTerminal(Realized), got {error:?}"
    );
}

#[test]
fn abandoned_is_an_idempotent_terminal_and_cannot_overwrite_a_settled_request() {
    let request_id = new_request_id();
    let run = RunId::new();
    let mut history = SessionModelRoutingControlHistory::new();
    history
        .append(requested(&request_id, &run, "claude-opus-5"))
        .expect("request appends");
    let abandoned = SessionModelRoutingControlRecord::ModelRoutingIntentAbandoned {
        request_id,
        originating_run_id: run.clone(),
        intent: intent("claude-opus-5"),
    };
    assert_eq!(
        history
            .append(abandoned.clone())
            .expect("archive terminal appends"),
        ModelRoutingControlAppendOutcome::Appended
    );
    assert_eq!(
        history
            .append(abandoned)
            .expect("exact archive retry converges"),
        ModelRoutingControlAppendOutcome::AlreadyRecorded
    );
    let error = history
        .append(SessionModelRoutingControlRecord::ModelRoutingIntentDenied {
            request_id,
            originating_run_id: run,
            intent: intent("claude-opus-5"),
            reason: SwitchTurnDenialReason::UnsupportedModel,
        })
        .expect_err("a denial cannot overwrite the archive terminal");
    assert!(matches!(
        error,
        ModelRoutingControlAppendError::AfterTerminal {
            disposition: ModelRoutingIntentRecordDisposition::Abandoned,
            ..
        }
    ));
}

#[test]
fn a_request_that_is_not_an_until_changed_model_switch_is_unrepresentable() {
    let request_id = new_request_id();
    let finite = SwitchTurnIntent {
        target_model: ModelId::new("claude-opus-5"),
        duration: SwitchTurnDuration::Finite {
            duration: meerkat_core::image_generation::FiniteScopedTurnDuration::OneTurn,
        },
        origin: SwitchTurnOrigin::Model {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        },
    };
    let error = SessionModelRoutingControlRecord::request(request_id, RunId::new(), finite)
        .expect_err("a finite scoped override is not a durable handoff");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::UnsupportedIntent { .. }
        ),
        "expected UnsupportedIntent, got {error:?}"
    );

    let user_origin = SwitchTurnIntent {
        target_model: ModelId::new("claude-opus-5"),
        duration: SwitchTurnDuration::UntilChanged,
        origin: SwitchTurnOrigin::User {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        },
    };
    let error = SessionModelRoutingControlRecord::request(request_id, RunId::new(), user_origin)
        .expect_err("a user-origin switch has its own live control surface");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::UnsupportedIntent { .. }
        ),
        "expected UnsupportedIntent, got {error:?}"
    );
}

#[test]
fn awaiting_decision_reports_only_undecided_requests() {
    let waiting = new_request_id();
    let settled = new_request_id();
    let run = RunId::new();
    let mut history = SessionModelRoutingControlHistory::new();
    history
        .append(requested(&settled, &run, "claude-opus-5"))
        .expect("settled request appends");
    history
        .append(
            SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
                request_id: settled,
                originating_run_id: run.clone(),
                intent: intent("claude-opus-5"),
                applied_identity: identity("claude-opus-5"),
            },
        )
        .expect("settled request terminalizes");
    history
        .append(requested(&waiting, &run, "gpt-5.5"))
        .expect("waiting request appends");

    let awaiting: Vec<_> = history
        .awaiting_decision()
        .map(|record| *record.request_id())
        .collect();
    assert_eq!(awaiting, vec![waiting]);
    assert_eq!(
        history.disposition_of(&settled),
        Some(ModelRoutingIntentRecordDisposition::Realized)
    );
    assert_eq!(
        history.disposition_of(&waiting),
        Some(ModelRoutingIntentRecordDisposition::Requested)
    );
}

#[test]
fn from_records_revalidates_a_persisted_log() {
    let request_id = new_request_id();
    let run = RunId::new();
    let corrupt = vec![
        requested(&request_id, &run, "claude-opus-5"),
        requested(&request_id, &run, "gpt-5.5"),
    ];
    let error = SessionModelRoutingControlHistory::from_records(corrupt)
        .expect_err("an incoherent persisted log must not enter memory");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::ConflictingIntent { .. }
        ),
        "expected ConflictingIntent, got {error:?}"
    );
}

fn invalid_persisted_histories() -> Vec<(&'static str, Vec<SessionModelRoutingControlRecord>)> {
    let duplicate_id = new_request_id();
    let duplicate_run = RunId::new();
    let duplicate = requested(&duplicate_id, &duplicate_run, "claude-opus-5");

    let orphan_id = new_request_id();
    let orphan_run = RunId::new();

    let resurrected_id = new_request_id();
    let resurrected_run = RunId::new();

    let scoped_id = new_request_id();
    vec![
        ("duplicate", vec![duplicate.clone(), duplicate]),
        (
            "orphan terminal",
            vec![realized(&orphan_id, &orphan_run, "claude-opus-5")],
        ),
        (
            "settled request resurrection",
            vec![
                requested(&resurrected_id, &resurrected_run, "claude-opus-5"),
                realized(&resurrected_id, &resurrected_run, "claude-opus-5"),
                requested(&resurrected_id, &resurrected_run, "claude-opus-5"),
            ],
        ),
        (
            "scoped intent",
            vec![
                SessionModelRoutingControlRecord::ModelRoutingIntentRequested {
                    request_id: scoped_id,
                    originating_run_id: RunId::new(),
                    intent: SwitchTurnIntent {
                        target_model: ModelId::new("claude-opus-5"),
                        duration: SwitchTurnDuration::Finite {
                            duration:
                                meerkat_core::image_generation::FiniteScopedTurnDuration::OneTurn,
                        },
                        origin: SwitchTurnOrigin::Model {
                            reason: SwitchTurnReasonTextDisposition::NotProvided,
                        },
                    },
                },
            ],
        ),
    ]
}

#[test]
fn history_deserialize_refuses_every_incoherent_sequence() {
    for (case, records) in invalid_persisted_histories() {
        let encoded = serde_json::to_value(records).expect("records serialize");
        let result = serde_json::from_value::<SessionModelRoutingControlHistory>(encoded);
        assert!(result.is_err(), "{case} must fail strict history decode");
    }
}

#[test]
fn session_head_deserialize_transitively_refuses_every_incoherent_sequence() {
    let mut session = Session::new();
    session.push(user_message());
    let head = meerkat_core::session_store::SessionHead::from_session(
        &session,
        meerkat_core::session_store::TranscriptStrandId::root(),
        0,
    )
    .expect("head projects");

    for (case, records) in invalid_persisted_histories() {
        let mut encoded = serde_json::to_value(&head).expect("head serializes");
        encoded.as_object_mut().expect("head is an object").insert(
            "model_routing_control".to_string(),
            serde_json::to_value(records).expect("records serialize"),
        );
        let result = serde_json::from_value::<meerkat_core::session_store::SessionHead>(encoded);
        assert!(result.is_err(), "{case} must fail SessionHead decode");
    }
}

// ---------------------------------------------------------------------------
// Session envelope: round-trip, compatibility, fork
// ---------------------------------------------------------------------------

fn session_with_owed_handoff() -> (Session, SwitchTurnRequestId) {
    let mut session = Session::new();
    session.push(user_message());
    let request_id = new_request_id();
    session
        .append_model_routing_control_record(requested(&request_id, &RunId::new(), "claude-opus-5"))
        .expect("request appends to the session");
    (session, request_id)
}

#[test]
fn session_envelope_round_trips_a_populated_handoff_log() {
    let (session, request_id) = session_with_owed_handoff();
    let encoded = serde_json::to_value(&session).expect("session serializes");
    assert!(
        encoded.get("model_routing_control").is_some(),
        "a populated handoff log must be persisted in the envelope"
    );
    assert_eq!(
        encoded["model_routing_control"],
        serde_json::to_value(session.model_routing_control().records())
            .expect("record sequence serializes"),
        "the in-memory Box must not add a wrapper to the durable record sequence"
    );

    let decoded: Session = serde_json::from_value(encoded).expect("session deserializes");
    assert_eq!(
        decoded.model_routing_control(),
        session.model_routing_control(),
        "the committed handoff log must round-trip losslessly"
    );
    assert_eq!(
        decoded.model_routing_control().disposition_of(&request_id),
        Some(ModelRoutingIntentRecordDisposition::Requested)
    );
}

#[test]
fn an_empty_handoff_log_keeps_the_envelope_byte_identical() {
    // Compatibility evidence (option a): absent/default is byte-compatible, so
    // every already-persisted v3 document is untouched and no version bump or
    // importer is required for existing state.
    let mut session = Session::new();
    session.push(user_message());
    let encoded = serde_json::to_value(&session).expect("session serializes");
    assert!(
        encoded.get("model_routing_control").is_none(),
        "an empty handoff log must not appear in the persisted envelope"
    );
    assert_eq!(
        encoded["version"],
        meerkat_core::SESSION_VERSION,
        "carrying the handoff log must not change the envelope version"
    );
}

#[test]
fn a_pre_handoff_envelope_still_deserializes() {
    // Compatibility evidence (option a): a new reader accepts old v3 state.
    let mut session = Session::new();
    session.push(user_message());
    let mut encoded = serde_json::to_value(&session).expect("session serializes");
    encoded
        .as_object_mut()
        .expect("envelope is an object")
        .remove("model_routing_control");

    let decoded: Session =
        serde_json::from_value(encoded).expect("a pre-handoff envelope must still deserialize");
    assert!(
        decoded.model_routing_control().is_empty(),
        "a pre-handoff document owes no handoff"
    );
}

#[test]
fn an_old_reader_fails_closed_on_a_populated_handoff_log() {
    // Compatibility evidence (option a): the old reader's envelope shape is
    // `deny_unknown_fields`, so a document that actually carries an owed
    // handoff is refused rather than silently stripped.
    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "snake_case", deny_unknown_fields)]
    #[allow(dead_code)]
    struct PreHandoffSessionEnvelope {
        version: u32,
        id: meerkat_core::SessionId,
        messages: Vec<Message>,
        created_at: std::time::SystemTime,
        updated_at: std::time::SystemTime,
        #[serde(default)]
        metadata: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        usage: meerkat_core::Usage,
    }

    let (session, _) = session_with_owed_handoff();
    let encoded = serde_json::to_value(&session).expect("session serializes");
    let error = serde_json::from_value::<PreHandoffSessionEnvelope>(encoded)
        .expect_err("an old reader must fail closed on an owed handoff it cannot honour");
    assert!(
        error.to_string().contains("model_routing_control"),
        "the refusal must name the unknown handoff field, got: {error}"
    );

    let mut plain = Session::new();
    plain.push(user_message());
    let plain_encoded = serde_json::to_value(&plain).expect("session serializes");
    serde_json::from_value::<PreHandoffSessionEnvelope>(plain_encoded)
        .expect("an old reader still accepts a document with no owed handoff");
}

#[test]
fn a_fork_does_not_inherit_an_owed_handoff() {
    let (session, request_id) = session_with_owed_handoff();
    for forked in [session.fork(), session.fork_at(1)] {
        assert!(
            forked.model_routing_control().is_empty(),
            "a fork is a new identity with its own run lineage and inherits no owed handoff"
        );
        assert_eq!(
            forked.model_routing_control().disposition_of(&request_id),
            None
        );
    }
    assert_eq!(
        session.model_routing_control().disposition_of(&request_id),
        Some(ModelRoutingIntentRecordDisposition::Requested),
        "forking must not settle the parent's owed handoff"
    );
}

// ---------------------------------------------------------------------------
// HeadCanonical: the log is carried and authenticated
// ---------------------------------------------------------------------------

#[test]
fn the_real_session_envelope_refuses_an_unknown_field() {
    // Under the chosen compatibility option, production `deny_unknown_fields`
    // on the session envelope IS the version gate. Assert it on the real type,
    // not on a replica: if the attribute is ever dropped, this must go red.
    let mut session = Session::new();
    session.push(user_message());
    let mut encoded = serde_json::to_value(&session).expect("session serializes");
    encoded
        .as_object_mut()
        .expect("envelope is an object")
        .insert("a_field_this_binary_does_not_know".to_string(), json_true());

    let error = serde_json::from_value::<Session>(encoded)
        .expect_err("the session envelope must refuse an unknown field");
    assert!(
        error
            .to_string()
            .contains("a_field_this_binary_does_not_know"),
        "the refusal must name the unknown field, got: {error}"
    );
}

fn json_true() -> serde_json::Value {
    serde_json::Value::Bool(true)
}

#[test]
fn a_persisted_log_carrying_a_duplicate_record_is_refused() {
    // Decode must be the inverse of encode. Silently collapsing a duplicate
    // would make the decoded value disagree with the bytes it came from.
    let request_id = new_request_id();
    let run = RunId::new();
    let record = requested(&request_id, &run, "claude-opus-5");
    let error =
        SessionModelRoutingControlHistory::from_records(vec![record.clone(), record.clone()])
            .expect_err("a duplicated persisted record must fail closed");
    assert!(
        matches!(
            error,
            ModelRoutingControlAppendError::DuplicateRecord { .. }
        ),
        "expected DuplicateRecord, got {error:?}"
    );

    // And the same duplicate arriving through the durable envelope is refused
    // rather than normalized.
    let mut session = Session::new();
    session.push(user_message());
    let mut encoded = serde_json::to_value(&session).expect("session serializes");
    encoded
        .as_object_mut()
        .expect("envelope is an object")
        .insert(
            "model_routing_control".to_string(),
            serde_json::to_value(vec![record.clone(), record]).expect("records serialize"),
        );
    let error = serde_json::from_value::<Session>(encoded)
        .expect_err("a duplicated persisted record must fail closed at the envelope");
    assert!(
        error.to_string().contains("duplicate"),
        "the refusal must name the duplicate, got: {error}"
    );
}

#[test]
fn a_committed_log_must_extend_its_predecessor() {
    let first = new_request_id();
    let second = new_request_id();
    let run = RunId::new();

    let mut committed = SessionModelRoutingControlHistory::new();
    committed
        .append(requested(&first, &run, "claude-opus-5"))
        .expect("request appends");

    let mut extended = committed.clone();
    extended
        .append(requested(&second, &run, "gpt-5.5"))
        .expect("second request appends");
    assert!(
        extended.extends(&committed),
        "adding a record must be a legal extension"
    );
    assert!(
        committed.extends(&committed),
        "an unchanged log extends itself"
    );

    let empty = SessionModelRoutingControlHistory::new();
    assert!(
        !empty.extends(&committed),
        "dropping a committed record must not read as an extension"
    );

    let mut divergent = SessionModelRoutingControlHistory::new();
    divergent
        .append(requested(&second, &run, "gpt-5.5"))
        .expect("request appends");
    assert!(
        !divergent.extends(&committed),
        "rewriting a committed record must not read as an extension"
    );
}

#[test]
fn recovery_adoption_takes_the_committed_handoff_log_from_the_durable_head() {
    // `adopt_recovered_head_state` is the one seam that REPLACES rather than
    // appends the log. A local tail the durable head never acknowledged is by
    // definition uncommitted, so recovery must adopt the head's log — and a
    // handoff the head DOES carry must not be lost.
    let request_id = new_request_id();
    let run = RunId::new();

    let mut head = Session::new();
    head.push(user_message());
    head.append_model_routing_control_record(requested(&request_id, &run, "claude-opus-5"))
        .expect("head owes a handoff");

    let mut recovered = Session::new();
    recovered.push(user_message());
    assert!(recovered.model_routing_control().is_empty());

    recovered
        .adopt_recovered_head_state(&head)
        .expect("recovery adopts the durable head");
    assert_eq!(
        recovered
            .model_routing_control()
            .disposition_of(&request_id),
        Some(ModelRoutingIntentRecordDisposition::Requested),
        "recovery must adopt a handoff the durable head committed"
    );

    // And the reverse: an uncommitted local tail is dropped, not retained.
    let mut local_tail = Session::new();
    local_tail.push(user_message());
    local_tail
        .append_model_routing_control_record(requested(&new_request_id(), &RunId::new(), "gpt-5.5"))
        .expect("local tail owes a handoff");

    let mut bare_head = Session::new();
    bare_head.push(user_message());
    local_tail
        .adopt_recovered_head_state(&bare_head)
        .expect("recovery adopts the durable head");
    assert!(
        local_tail.model_routing_control().is_empty(),
        "a tail the durable head never acknowledged must not survive recovery"
    );
}

// ---------------------------------------------------------------------------
// WholeBlob: the sealed artifact carries and restores the log
// ---------------------------------------------------------------------------

#[test]
fn the_whole_blob_artifact_round_trips_an_owed_handoff() {
    let (session, request_id) = session_with_owed_handoff();
    let artifact = session
        .to_persisted_artifact()
        .expect("WholeBlob artifact serializes");
    let bytes = artifact.into_bytes();

    let decoded = Session::from_persisted_bytes(&bytes).expect("WholeBlob artifact decodes");
    assert_eq!(
        decoded.model_routing_control(),
        session.model_routing_control(),
        "the WholeBlob artifact must carry the committed handoff log losslessly"
    );
    assert_eq!(
        decoded.model_routing_control().disposition_of(&request_id),
        Some(ModelRoutingIntentRecordDisposition::Requested)
    );
}

#[test]
fn the_whole_blob_byte_identity_changes_when_a_handoff_is_owed() {
    // The physical WholeBlob digest is the store's identity for the document.
    // An owed handoff must change it, or a store could serve a stale body that
    // silently drops the handoff.
    let mut plain = Session::new();
    plain.push(user_message());
    let plain_bytes = plain
        .to_persisted_artifact()
        .expect("artifact serializes")
        .into_bytes();

    let mut owed = plain.clone();
    owed.append_model_routing_control_record(requested(
        &new_request_id(),
        &RunId::new(),
        "claude-opus-5",
    ))
    .expect("request appends");
    let owed_bytes = owed
        .to_persisted_artifact()
        .expect("artifact serializes")
        .into_bytes();

    assert_ne!(
        plain_bytes, owed_bytes,
        "an owed handoff must change the WholeBlob body"
    );
}

mod head_row {
    use super::*;
    use meerkat_core::session_store::{SessionHead, TranscriptStrandId, session_head_cas_token};

    #[test]
    fn head_carries_the_committed_handoff_log() {
        let (session, request_id) = session_with_owed_handoff();
        let head = SessionHead::from_session(&session, TranscriptStrandId::root(), 0)
            .expect("head projects from the session");
        assert_eq!(
            head.model_routing_control.disposition_of(&request_id),
            Some(ModelRoutingIntentRecordDisposition::Requested),
            "the physical head must carry the committed handoff log"
        );
    }

    #[test]
    fn an_empty_handoff_log_keeps_the_head_row_byte_identical() {
        // Compatibility evidence: every head written before the handoff log
        // existed keeps its exact persisted shape, so no head-row migration is
        // required.
        let mut session = Session::new();
        session.push(user_message());
        let head = SessionHead::from_session(&session, TranscriptStrandId::root(), 0)
            .expect("head projects from the session");
        let encoded = serde_json::to_value(&head).expect("head serializes");
        assert!(
            encoded.get("model_routing_control").is_none(),
            "an empty handoff log must not appear in the persisted head row"
        );
        assert!(
            session_head_cas_token(&head).is_ok(),
            "an empty handoff log must not disturb head CAS derivation"
        );
    }
}
