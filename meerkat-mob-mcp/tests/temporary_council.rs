//! Temporary-council orchestration core (issue #159, phase 4).
//!
//! Every row here drives real components: a real `MobMcpState` over explicitly
//! rooted durable custody, real source mobs with real members, real
//! source-owned forked-participant capabilities, a real temporary mob, real
//! wiring, and real bounded member turns. Only the LLM is scripted, and it is
//! scripted deterministically so replay and turn-count claims are checkable.
//!
//! # Parallel safety
//!
//! Every fixture mints a unique identity scope, so mob ids, council ids, and
//! the comms peer names derived from them are disjoint across concurrently
//! running tests. Each test also tears its mobs down explicitly. There is no
//! serializing lock and no reliance on process teardown: the suite is intended
//! to pass under a plain `cargo test -p meerkat-mob-mcp --test
//! temporary_council`.
//!
//! The mixed local + host-owned end-to-end path lives in the integration
//! e2e-fast lane. What this crate-level suite additionally proves:
//! [`a_remote_capability_acquired_but_never_seated_is_recovered_from_persisted_custody`]
//! drives a real HOST-routed capability reference through real recovery and
//! shows the persisted reference — not realm-local capability custody — is
//! what resolves it. The host-owned seating and release halves are covered at
//! the primitive level by
//! `meerkat-mob/tests/forked_participant_host_attached_spawn.rs` and
//! `meerkat-mob/tests/host_forked_participant_live_daemon.rs`, and the
//! coordinator adds no host-specific branch: it never inspects
//! `ForkedParticipantOwnerRoute` and routes every capability verb through
//! `MobHandle`, which owns the local/host decision.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::machines::temporary_council_lifecycle::TemporaryCouncilLifecycleState;
use meerkat_mob::temporary_council::{
    TemporaryCouncilAcquisition, TemporaryCouncilCleanupStatus, TemporaryCouncilDurability,
    TemporaryCouncilExchangeOutcome, TemporaryCouncilExitReason, TemporaryCouncilMergeOutcome,
    TemporaryCouncilMergePolicyKind,
};
use meerkat_mob::{AgentIdentity, ProfileName};
use meerkat_mob_mcp::MobMcpState;
use meerkat_mob_mcp::temporary_council::{
    MergeBackPolicy, TemporaryCouncilBounds, TemporaryCouncilDeadline, TemporaryCouncilError,
    TemporaryCouncilOutcome, TemporaryCouncilParticipantSpec, TemporaryCouncilRequest,
    TemporaryCouncilStructuredContract,
};
use support::{
    CouncilFixture, FlakyCapabilityStore, OneShotFailingCouncilStore, PanicOnceCouncilStore,
    ScriptedTurn, TurnGate, council_definition, council_definition_with_description, identity,
    role_in_request, user_text,
};

fn participant(
    fixture: &CouncilFixture,
    order: u32,
    role: &str,
    source: &str,
    target: &str,
) -> TemporaryCouncilParticipantSpec {
    TemporaryCouncilParticipantSpec::new(
        order,
        role,
        fixture.source_mob_id(),
        identity(source),
        identity(target),
        ProfileName::from("participant"),
    )
}

fn bounds(max_rounds: u32, secs: u64) -> TemporaryCouncilBounds {
    TemporaryCouncilBounds::relative(Duration::from_secs(secs), max_rounds, 4096)
}

/// The default script: every participant answers with its own role, so an
/// exchange's provenance is checkable from its text alone.
fn role_script(request: &meerkat_client::LlmRequest) -> ScriptedTurn {
    let text = user_text(request);
    if text.contains("strict JSON document") {
        return ScriptedTurn::Text("{\"verdict\":\"agreed\",\"confidence\":0.9}".to_string());
    }
    if text.contains("artifact you produced") {
        return ScriptedTurn::Text(
            "{\"uri\":\"blob://council/report\",\"media_type\":\"text/markdown\",\"byte_len\":42}"
                .to_string(),
        );
    }
    if text.contains("bounded plain-text summary") {
        return ScriptedTurn::Text("SUMMARY: the council agreed on the plan.".to_string());
    }
    match role_in_request(request) {
        Some(role) => ScriptedTurn::Text(format!("position from {role}")),
        None => ScriptedTurn::Text("ok".to_string()),
    }
}

fn two_participant_request(
    fixture: &CouncilFixture,
    id: &str,
    merge_back: MergeBackPolicy,
    max_rounds: u32,
    secs: u64,
) -> TemporaryCouncilRequest {
    TemporaryCouncilRequest::new(
        fixture.council_id(id),
        council_definition("template-is-replaced"),
        vec![
            participant(fixture, 0, "analyst", "researcher", "analyst"),
            participant(fixture, 1, "critic", "reviewer", "critic"),
        ],
        "Should we ship the migration this week?",
        bounds(max_rounds, secs),
        merge_back,
    )
}

fn completed_texts(outcome: &TemporaryCouncilOutcome) -> Vec<String> {
    outcome
        .result
        .exchanges
        .iter()
        .filter_map(|receipt| receipt.completed_text().map(str::to_string))
        .collect()
}

// ===========================================================================
// Happy path: local two-participant bounded discussion over shared custody
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn local_two_participant_discussion_runs_wires_merges_and_cleans_up() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(
        &fixture,
        "two-party",
        MergeBackPolicy::BoundedTextSummary {
            finalizer: identity("analyst"),
            max_bytes: 2048,
        },
        2,
        120,
    );
    let temporary_mob_id = request.council_id.temporary_mob_id();

    let outcome = fixture
        .state
        .temporary_council()
        .run(request.clone())
        .await
        .expect("council runs to a terminal outcome");

    assert!(!outcome.replayed);
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed,
        "the configured round schedule must run to completion"
    );
    assert_eq!(outcome.result.rounds_completed, 2);
    assert_eq!(outcome.result.temporary_mob_id, temporary_mob_id);

    // Four discussion exchanges (2 rounds x 2 participants) plus one merge turn.
    let texts = completed_texts(&outcome);
    assert_eq!(texts.len(), 5, "unexpected exchange set: {texts:?}");
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

    match &outcome.result.merge {
        TemporaryCouncilMergeOutcome::BoundedTextSummary {
            finalizer,
            text,
            truncated,
        } => {
            assert_eq!(finalizer, &identity("analyst"));
            assert!(text.contains("SUMMARY"), "unexpected summary: {text}");
            assert!(!truncated);
        }
        other => panic!("expected a bounded text summary, got {other:?}"),
    }

    // Provenance is present and non-secret.
    assert_eq!(outcome.result.participants.len(), 2);
    for provenance in &outcome.result.participants {
        assert!(provenance.seated);
        assert_eq!(provenance.source_mob_id, fixture.source_mob_id());
        // Issue #159 requires the result to be provenance-carrying: the exact
        // source transcript, its prefix digest, the owning route, the fork
        // session identity, and the granted scope/expiry/reuse — never a
        // capability bearer.
        let capability = provenance
            .capability
            .as_ref()
            .expect("a seated participant carries exact capability provenance");
        assert!(matches!(
            capability.owner_route,
            meerkat_mob::forked_participant::ForkedParticipantOwnerRoute::Local { .. }
        ));
        assert!(!capability.source_provenance.prefix_digest.is_empty());
        assert!(!capability.correlation_hint.is_empty());
        assert_eq!(
            capability.scope,
            meerkat_mob::forked_participant::ForkedParticipantOperationScope::InvokeAndObserve
        );
        assert_eq!(
            capability.reuse,
            meerkat_mob::forked_participant::ForkedParticipantReusePolicy::OneShot
        );
        assert!(capability.expires_at > outcome.result.concluded_at);
        assert_ne!(
            capability.fork_session_id, capability.source_provenance.source_session_id,
            "the branch is a distinct child session, not the source"
        );
    }

    // Cleanup: both members retired, the temporary mob is gone, no debt.
    assert!(
        outcome.cleanup.settled(),
        "cleanup debt: {:?}",
        outcome.cleanup
    );
    assert_eq!(outcome.cleanup.released_participants.len(), 2);
    assert!(
        fixture.state.handle_for(&temporary_mob_id).await.is_err(),
        "the temporary mob must not survive the council"
    );

    // The record is settled and carries the same immutable result.
    let record = fixture
        .state
        .temporary_council()
        .load(&request.council_id)
        .await
        .expect("load record")
        .expect("record present");
    assert_eq!(
        record.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::Settled
    );
    assert_eq!(record.result.as_ref(), Some(&outcome.result));

    fixture.teardown().await;
}

// ===========================================================================
// Live mid-flight observation: wiring, source disappearance, cancellation
// ===========================================================================

/// While the council is genuinely mid-turn, the temporary mob's roster shows a
/// FULL MESH between every seated participant. This reads live `MobMachine`
/// truth rather than trusting the coordinator's own wiring report.
#[tokio::test(flavor = "multi_thread")]
async fn full_mesh_wiring_is_observable_on_the_live_temporary_mob() {
    let gate = TurnGate::new();
    let gate_for_script = gate.clone();
    let fixture = CouncilFixture::new(move |request| {
        let text = user_text(request);
        if text.contains("Council topic:") {
            ScriptedTurn::Gated(gate_for_script.clone(), "gated position".to_string())
        } else {
            role_script(request)
        }
    });
    fixture
        .seed_source_mob(&["researcher", "reviewer", "scribe"])
        .await;

    let request = TemporaryCouncilRequest::new(
        fixture.council_id("mesh"),
        council_definition("template-is-replaced"),
        vec![
            participant(&fixture, 0, "analyst", "researcher", "analyst"),
            participant(&fixture, 1, "critic", "reviewer", "critic"),
            participant(&fixture, 2, "scribe", "scribe", "scribe"),
        ],
        "Mesh topology check",
        bounds(1, 240),
        MergeBackPolicy::NoMerge,
    );
    let temporary_mob_id = request.council_id.temporary_mob_id();

    let coordinator = fixture.state.temporary_council();
    let running = tokio::spawn(async move { coordinator.run(request).await });
    gate.wait_entered(1).await;

    let handle = fixture
        .state
        .handle_for(&temporary_mob_id)
        .await
        .expect("the temporary mob is live while the council runs");
    let roster = handle.list_members().await;
    let seated: Vec<_> = roster
        .iter()
        .filter(|entry| ["analyst", "critic", "scribe"].contains(&entry.agent_identity.as_str()))
        .collect();
    assert_eq!(seated.len(), 3, "all three participants must be seated");
    for entry in &seated {
        let peers: Vec<&str> = entry.wired_to.iter().map(AgentIdentity::as_str).collect();
        for other in ["analyst", "critic", "scribe"] {
            if other == entry.agent_identity.as_str() {
                continue;
            }
            assert!(
                peers.contains(&other),
                "{} is not wired to {other} (wired_to = {peers:?})",
                entry.agent_identity
            );
        }
    }

    gate.open();
    let outcome = running
        .await
        .expect("owned task joins")
        .expect("council runs");
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed
    );
    assert_eq!(outcome.result.exchanges.len(), 3);
    assert!(
        outcome.cleanup.settled(),
        "cleanup debt: {:?}",
        outcome.cleanup
    );

    fixture.teardown().await;
}

/// The source mob may disappear after its capabilities were created: the
/// council keeps running on the seated branches and still cleans up.
#[tokio::test(flavor = "multi_thread")]
async fn council_survives_the_source_mob_disappearing_after_capability_creation() {
    let gate = TurnGate::new();
    let gate_for_script = gate.clone();
    let fixture = CouncilFixture::new(move |request| {
        let text = user_text(request);
        if text.contains("Council topic:") {
            ScriptedTurn::Gated(gate_for_script.clone(), "gated position".to_string())
        } else {
            role_script(request)
        }
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request =
        two_participant_request(&fixture, "source-gone", MergeBackPolicy::NoMerge, 1, 240);
    let temporary_mob_id = request.council_id.temporary_mob_id();
    let coordinator = fixture.state.temporary_council();
    let running = tokio::spawn(async move { coordinator.run(request).await });
    gate.wait_entered(1).await;

    // Both capabilities are already created and seated; destroy the source.
    fixture
        .state
        .mob_destroy(&fixture.source_mob_id())
        .await
        .expect("destroy the source mob mid-council");

    gate.open();
    let outcome = running
        .await
        .expect("owned task joins")
        .expect("council runs");
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed
    );
    assert_eq!(outcome.result.exchanges.len(), 2);
    assert!(
        outcome.cleanup.settled(),
        "cleanup debt: {:?}",
        outcome.cleanup
    );
    assert!(fixture.state.handle_for(&temporary_mob_id).await.is_err());

    fixture.teardown().await;
}

/// Dropping the caller's await does NOT cancel the owned execution task: it
/// runs to its terminal, seals its result, and cleans up.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_caller_future_still_completes_the_council_and_cleanup() {
    let gate = TurnGate::new();
    let gate_for_script = gate.clone();
    let fixture = CouncilFixture::new(move |request| {
        let text = user_text(request);
        if text.contains("Council topic:") {
            ScriptedTurn::Gated(gate_for_script.clone(), "gated position".to_string())
        } else {
            role_script(request)
        }
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(
        &fixture,
        "cancelled-caller",
        MergeBackPolicy::NoMerge,
        1,
        240,
    );
    let id = request.council_id.clone();
    let temporary_mob_id = id.temporary_mob_id();

    {
        // The caller's future is dropped here, mid-flight.
        let coordinator = fixture.state.temporary_council();
        let mut running = Box::pin(coordinator.run(request));
        tokio::select! {
            _ = &mut running => panic!("the council cannot finish while the gate is closed"),
            () = gate.wait_entered(1) => {}
        }
    }

    gate.open();

    // The owned task continues without any awaiting caller.
    let coordinator = fixture.state.temporary_council();
    let mut settled = None;
    for _ in 0..600 {
        let record = coordinator.load(&id).await.expect("load record");
        if let Some(record) = record
            && record.machine_state.lifecycle_phase == TemporaryCouncilLifecycleState::Settled
        {
            settled = Some(record);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let settled = settled.expect("the owned task must settle the council after the caller left");
    let result = settled.result.expect("an immutable result is sealed");
    assert_eq!(result.exit_reason, TemporaryCouncilExitReason::Completed);
    assert_eq!(result.exchanges.len(), 2);
    assert!(
        settled.cleanup.expect("cleanup receipt").settled(),
        "cleanup must complete without an awaiting caller"
    );
    assert!(fixture.state.handle_for(&temporary_mob_id).await.is_err());

    fixture.teardown().await;
}

// ===========================================================================
// Bounds
// ===========================================================================

/// The round budget is exact: three rounds x two participants = six exchanges.
#[tokio::test(flavor = "multi_thread")]
async fn the_round_budget_bounds_the_discussion_exactly() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let outcome = fixture
        .state
        .temporary_council()
        .run(two_participant_request(
            &fixture,
            "round-budget",
            MergeBackPolicy::NoMerge,
            3,
            240,
        ))
        .await
        .expect("council runs");

    assert_eq!(outcome.result.rounds_completed, 3);
    assert_eq!(outcome.result.exchanges.len(), 6);
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed
    );
    for round in 0..3 {
        assert_eq!(
            outcome
                .result
                .exchanges
                .iter()
                .filter(|receipt| receipt.round == round)
                .count(),
            2,
            "each round runs exactly one exchange per participant"
        );
    }

    fixture.teardown().await;
}

/// The exchange budget cuts a longer round schedule short with a typed reason.
#[tokio::test(flavor = "multi_thread")]
async fn the_exchange_budget_cuts_the_schedule_short() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let mut request = two_participant_request(
        &fixture,
        "exchange-budget",
        MergeBackPolicy::NoMerge,
        4,
        240,
    );
    request.bounds.max_exchanges = 3;
    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council runs");

    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::MaxExchangesReached
    );
    assert_eq!(
        outcome
            .result
            .exchanges
            .iter()
            .filter(|receipt| receipt.completed_text().is_some())
            .count(),
        3,
        "the exchange budget is a hard cap on committed turns"
    );
    assert!(outcome.cleanup.settled());

    fixture.teardown().await;
}

/// The absolute deadline wraps every await and yields a typed partial outcome.
#[tokio::test(flavor = "multi_thread")]
async fn the_deadline_yields_a_typed_partial_outcome_and_still_cleans_up() {
    let gate = TurnGate::new();
    let gate_for_script = gate.clone();
    let fixture = CouncilFixture::new(move |request| {
        let text = user_text(request);
        if text.contains("Council topic:") {
            // Never released: the deadline must be what ends this council.
            ScriptedTurn::Gated(gate_for_script.clone(), "never".to_string())
        } else {
            role_script(request)
        }
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let mut request =
        two_participant_request(&fixture, "deadline", MergeBackPolicy::NoMerge, 2, 240);
    // Short relative deadline: seating happens first, then the gated turn
    // exhausts the remaining time.
    request.bounds.deadline = TemporaryCouncilDeadline::Relative {
        after: Duration::from_secs(20),
    };
    let temporary_mob_id = request.council_id.temporary_mob_id();

    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council reaches a terminal outcome");

    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::DeadlineExceeded,
        "unexpected exit: {:?}",
        outcome.result.exit_reason
    );
    assert!(
        outcome.result.exchanges.iter().any(|receipt| matches!(
            receipt.outcome,
            TemporaryCouncilExchangeOutcome::Failed { .. }
        )),
        "the timed-out exchange is recorded as a typed failure"
    );
    assert!(matches!(
        outcome.result.merge,
        TemporaryCouncilMergeOutcome::NoMerge { .. }
    ));
    assert!(
        outcome.cleanup.settled(),
        "cleanup debt: {:?}",
        outcome.cleanup
    );
    assert!(fixture.state.handle_for(&temporary_mob_id).await.is_err());
    gate.open();

    fixture.teardown().await;
}

// ===========================================================================
// Idempotency, conflict, and crash recovery
// ===========================================================================

/// Replaying the exact same request returns the sealed result and takes NO
/// additional model turns.
#[tokio::test(flavor = "multi_thread")]
async fn an_identical_replay_returns_the_sealed_result_without_extra_turns() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(&fixture, "replay", MergeBackPolicy::NoMerge, 1, 240);
    let first = fixture
        .state
        .temporary_council()
        .run(request.clone())
        .await
        .expect("first run");
    let calls_after_first = fixture.provider_calls();
    assert!(calls_after_first >= 2, "the first run must take real turns");

    let second = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("replay");

    assert!(second.replayed, "the second call must be a replay");
    assert_eq!(second.result, first.result, "the result is immutable");
    assert_eq!(
        fixture.provider_calls(),
        calls_after_first,
        "a replay must not issue another provider call"
    );

    fixture.teardown().await;
}

/// Two concurrent callers presenting the SAME request join one owned
/// execution: one council, one result, one set of model turns.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_identical_requests_join_a_single_owned_execution() {
    let gate = TurnGate::new();
    let gate_for_script = gate.clone();
    let fixture = CouncilFixture::new(move |request| {
        let text = user_text(request);
        if text.contains("Council topic:") {
            ScriptedTurn::Gated(gate_for_script.clone(), "gated position".to_string())
        } else {
            role_script(request)
        }
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request =
        two_participant_request(&fixture, "single-flight", MergeBackPolicy::NoMerge, 1, 240);
    let first_coordinator = fixture.state.temporary_council();
    let first_request = request.clone();
    let first = tokio::spawn(async move { first_coordinator.run(first_request).await });
    gate.wait_entered(1).await;

    // The second caller arrives while the first is still mid-flight.
    let second_coordinator = fixture.state.temporary_council();
    let second = tokio::spawn(async move { second_coordinator.run(request).await });

    gate.open();
    let first = first.await.expect("first joins").expect("first result");
    let second = second.await.expect("second joins").expect("second result");

    assert_eq!(
        first.result, second.result,
        "both callers observe the same immutable result"
    );
    assert!(
        !first.replayed && !second.replayed,
        "a joined caller observes the owned task's own outcome, not a replay"
    );
    assert_eq!(
        first.result.exchanges.len(),
        2,
        "a joined caller must not add exchanges"
    );
    assert_eq!(
        fixture.provider_calls(),
        2,
        "two callers must not double the council's model work"
    );

    fixture.teardown().await;
}

/// A different request under a bound council id is refused, and refusal does
/// not disturb the sealed result.
#[tokio::test(flavor = "multi_thread")]
async fn a_conflicting_request_under_a_bound_council_id_is_rejected() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(&fixture, "conflict", MergeBackPolicy::NoMerge, 1, 240);
    let first = fixture
        .state
        .temporary_council()
        .run(request.clone())
        .await
        .expect("first run");
    let calls_after_first = fixture.provider_calls();

    let mut conflicting = request.clone();
    conflicting.topic = "A materially different question".to_string();
    let error = fixture
        .state
        .temporary_council()
        .run(conflicting)
        .await
        .expect_err("a different request may not take a bound council id");
    match error {
        TemporaryCouncilError::ConflictingRequest {
            ref council_id,
            ref stored_fingerprint,
            ref presented_fingerprint,
        } => {
            assert_eq!(council_id, &request.council_id);
            assert_ne!(stored_fingerprint, presented_fingerprint);
        }
        other => panic!("expected a conflicting-request refusal, got {other:?}"),
    }
    assert_eq!(
        fixture.provider_calls(),
        calls_after_first,
        "a refused request must not run any turn"
    );

    let record = fixture
        .state
        .temporary_council()
        .load(&request.council_id)
        .await
        .expect("load")
        .expect("record present");
    assert_eq!(record.result.as_ref(), Some(&first.result));

    fixture.teardown().await;
}

/// A council seat may rename the source role, but it may not replace the
/// source member's execution profile. Exact binding equality covers model,
/// tools, MCP servers, skills, provider parameters, and resume overrides.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_profile_cannot_widen_the_source_execution_context() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let mut request =
        two_participant_request(&fixture, "profile-widen", MergeBackPolicy::NoMerge, 1, 240);
    let temporary_mob_id = request.council_id.temporary_mob_id();
    request
        .definition_template
        .profiles
        .get_mut(&ProfileName::from("participant"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .expect("inline participant profile")
        .model = "claude-opus-5".to_string();

    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("containment refusal is a sealed council outcome");
    match outcome.result.exit_reason {
        TemporaryCouncilExitReason::ParticipantSeatingFailed {
            participant_order,
            ref detail,
        } => {
            assert_eq!(participant_order, 0);
            assert!(detail.contains("would widen or alter"));
        }
        ref other => panic!("expected a profile-containment refusal, got {other:?}"),
    }
    assert!(outcome.result.exchanges.is_empty());
    assert_eq!(fixture.provider_calls(), 0, "no model work may begin");
    assert!(
        fixture
            .state
            .mob_handles_snapshot()
            .await
            .expect("mob snapshot")
            .into_iter()
            .all(|(mob_id, _)| mob_id != temporary_mob_id),
        "the temporary mob must not exist when profile containment fails"
    );

    fixture.teardown().await;
}

/// A coordinator that dies before sealing a result is recovered by a restarted
/// process as a typed interrupted terminal plus cleanup — never re-executed.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_style_restart_seals_an_interrupted_terminal_and_cleans_up() {
    // Fail the council custody commit that records the SECOND participant's
    // seating: by then a real temporary mob, two real capabilities, and two
    // real seated members exist, which is exactly the crash shape.
    let failing = Arc::new(std::sync::OnceLock::<Arc<OneShotFailingCouncilStore>>::new());
    let failing_slot = failing.clone();
    let fixture = CouncilFixture::new_with(role_script, move |state, root| {
        let path = MobMcpState::persistent_forked_participant_store_path(root);
        let inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore> = Arc::new(
            meerkat_mob::store::SqliteTemporaryCouncilStore::open(&path)
                .expect("open durable council custody"),
        );
        // Commit order: claim, then per participant {acquire-intent,
        // capability, seated}, then phase=Running. Index 7 is that last
        // advance: the first commit whose failure propagates out of the owned
        // task with two seated members and no sealed result — the crash shape.
        let store = Arc::new(OneShotFailingCouncilStore::new(inner, 7));
        let _ = failing_slot.set(store.clone());
        state.with_temporary_council_store(store)
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request =
        two_participant_request(&fixture, "interrupted", MergeBackPolicy::NoMerge, 1, 240);
    let id = request.council_id.clone();
    let temporary_mob_id = id.temporary_mob_id();

    let error = fixture
        .state
        .temporary_council()
        .run(request.clone())
        .await
        .expect_err("the injected custody fault aborts the owned task");
    assert!(
        matches!(error, TemporaryCouncilError::Store { .. }),
        "unexpected error: {error:?}"
    );
    assert!(failing.get().expect("store installed").fired());
    let calls_before_recovery = fixture.provider_calls();

    // Quiesce this process's mob actors before another process opens the same
    // durable stores; a real restart would have destroyed them with the
    // process.
    if let Ok(handle) = fixture.state.handle_for(&temporary_mob_id).await {
        handle.shutdown().await.expect("quiesce the temporary mob");
    }
    if let Ok(handle) = fixture.state.handle_for(&fixture.source_mob_id()).await {
        handle.shutdown().await.expect("quiesce the source mob");
    }

    // A restart is a DIFFERENT coordinator identity, so it may only take over
    // once the dead coordinator's lease has been observed expired.
    let restarted = fixture.restart_state();
    let held = restarted.temporary_council_store_for_tests();
    let reports = restarted
        .temporary_council()
        .recover_unfinished()
        .await
        .expect("a busy council is skipped rather than aborting the sweep");
    assert!(
        reports.is_empty(),
        "a live foreign claim is neither recovered nor reported as settled"
    );
    fixture.expire_claim_lease(&held, &id).await;

    let outcome = restarted
        .temporary_council()
        .run(request)
        .await
        .expect("same-id admission recovers the interrupted council");
    assert!(outcome.replayed);
    assert!(
        outcome.cleanup.settled(),
        "recovery cleanup debt: {:?}",
        outcome.cleanup
    );
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::CoordinatorInterrupted
    );

    let record = restarted
        .temporary_council()
        .load(&id)
        .await
        .expect("load")
        .expect("record present");
    assert_eq!(
        record.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::Settled
    );
    let result = record.result.expect("a terminal result is sealed");
    assert_eq!(
        result.exit_reason,
        TemporaryCouncilExitReason::CoordinatorInterrupted
    );
    assert!(matches!(
        result.merge,
        TemporaryCouncilMergeOutcome::NotAttempted { .. }
    ));
    assert_eq!(
        fixture.provider_calls(),
        calls_before_recovery,
        "recovery must never re-execute the council's model work"
    );
    assert!(
        restarted.handle_for(&temporary_mob_id).await.is_err(),
        "recovery must destroy the temporary mob"
    );

    fixture.teardown().await;
}

// ===========================================================================
// Partial failure and cleanup convergence
// ===========================================================================

/// A definition template with an unresolvable `ghost` profile, used to fail one
/// participant's attached spawn AFTER its capability was created.
fn definition_with_ghost_profile() -> meerkat_mob::MobDefinition {
    let mut definition = council_definition("template-is-replaced");
    definition.profiles.insert(
        ProfileName::from("ghost"),
        meerkat_mob::ProfileBinding::RealmRef {
            realm_profile: "no-such-realm-profile".to_string(),
        },
    );
    definition
}

fn partial_failure_request(fixture: &CouncilFixture, id: &str) -> TemporaryCouncilRequest {
    let mut second = participant(fixture, 1, "critic", "reviewer", "critic");
    second.target_profile = ProfileName::from("ghost");
    TemporaryCouncilRequest::new(
        fixture.council_id(id),
        definition_with_ghost_profile(),
        vec![
            participant(fixture, 0, "analyst", "researcher", "analyst"),
            second,
        ],
        "Partial seating check",
        bounds(1, 240),
        MergeBackPolicy::NoMerge,
    )
}

/// When one participant cannot be seated, the council reports a typed partial
/// outcome, retires the participants it DID seat, and explicitly revokes the
/// capability that was acquired but never attached.
#[tokio::test(flavor = "multi_thread")]
async fn a_partial_seating_failure_cleans_up_prior_participants_and_revokes() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = partial_failure_request(&fixture, "partial-seat");
    let id = request.council_id.clone();
    let temporary_mob_id = id.temporary_mob_id();

    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council reaches a terminal outcome");

    match &outcome.result.exit_reason {
        TemporaryCouncilExitReason::ParticipantSeatingFailed {
            participant_order, ..
        } => assert_eq!(*participant_order, 1),
        other => panic!("expected a seating failure, got {other:?}"),
    }
    match &outcome.result.merge {
        // NoMerge is observation only: it reports who was actually seated at
        // merge time, which is exactly the one participant that succeeded.
        TemporaryCouncilMergeOutcome::NoMerge {
            confirmed_participants,
        } => assert_eq!(confirmed_participants, &vec![identity("analyst")]),
        other => panic!("expected NoMerge, got {other:?}"),
    }
    assert!(outcome.result.exchanges.is_empty(), "no turn may run");

    assert_eq!(
        outcome.cleanup.released_participants,
        vec![0],
        "the seated participant is retired"
    );
    assert_eq!(
        outcome.cleanup.revoked_participants,
        Vec::<u32>::new(),
        "profile containment fails before acquiring the rejected participant (cleanup: {:?})",
        outcome.cleanup
    );
    assert!(
        outcome.cleanup.settled(),
        "cleanup debt: {:?}",
        outcome.cleanup
    );
    assert!(fixture.state.handle_for(&temporary_mob_id).await.is_err());

    fixture.teardown().await;
}

/// A cleanup obligation that fails is RETAINED as typed debt, and a later
/// recovery attempt converges it. The immutable result stays valid throughout.
#[tokio::test(flavor = "multi_thread")]
async fn retained_cleanup_debt_converges_on_retry() {
    // Fail every eligible revoker in the first cleanup pass for participant
    // slot 1. The coordinator deliberately tries source, temporary, and any
    // remaining managed handle before retaining debt; the retry then converges.
    let fixture = CouncilFixture::new_with(role_script, |state, _root| {
        let inner: Arc<dyn meerkat_mob::store::ForkedParticipantStore> =
            Arc::new(meerkat_mob::store::InMemoryForkedParticipantStore::new());
        state.with_forked_participant_store(Arc::new(
            FlakyCapabilityStore::failing_revocation_lookup(inner, ":p1", 3),
        ))
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = partial_failure_request(&fixture, "cleanup-debt");
    let id = request.council_id.clone();
    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council reaches a terminal outcome");

    assert!(outcome.cleanup.settled());
    assert!(outcome.cleanup.debts.is_empty());
    assert!(
        outcome.cleanup.temporary_mob_destroyed,
        "the mob is still destroyed even when a revocation is owed"
    );

    let record = fixture
        .state
        .temporary_council()
        .load(&id)
        .await
        .expect("load")
        .expect("record present");
    assert_eq!(
        record.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::Settled
    );
    let sealed = record.result.clone().expect("the result is sealed");
    assert_eq!(sealed, outcome.result, "the result survives failed cleanup");

    let reports = fixture
        .state
        .temporary_council()
        .recover_unfinished()
        .await
        .expect("cleanup retry sweep");
    assert!(reports.is_empty());

    let record = fixture
        .state
        .temporary_council()
        .load(&id)
        .await
        .expect("load")
        .expect("record present");
    assert_eq!(
        record.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::Settled
    );
    assert_eq!(
        record.result.as_ref(),
        Some(&sealed),
        "the immutable result is unchanged by the cleanup retry"
    );

    fixture.teardown().await;
}

/// A terminal turn failure stops the council with a typed exit reason and
/// still cleans up.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_participant_turn_yields_a_typed_partial_outcome() {
    let fixture = CouncilFixture::new(|request| {
        if role_in_request(request).as_deref() == Some("critic") {
            ScriptedTurn::Fail("scripted provider outage".to_string())
        } else {
            role_script(request)
        }
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request =
        two_participant_request(&fixture, "turn-failure", MergeBackPolicy::NoMerge, 2, 240);
    let temporary_mob_id = request.council_id.temporary_mob_id();
    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council reaches a terminal outcome");

    match &outcome.result.exit_reason {
        TemporaryCouncilExitReason::ExchangeFailed {
            round,
            target_identity,
            ..
        } => {
            assert_eq!(*round, 0);
            assert_eq!(target_identity, &identity("critic"));
        }
        other => panic!("expected a typed exchange failure, got {other:?}"),
    }
    assert_eq!(
        outcome
            .result
            .exchanges
            .iter()
            .filter(|receipt| receipt.completed_text().is_some())
            .count(),
        1,
        "only the analyst's turn committed"
    );
    assert!(
        outcome.cleanup.settled(),
        "cleanup debt: {:?}",
        outcome.cleanup
    );
    assert!(fixture.state.handle_for(&temporary_mob_id).await.is_err());

    fixture.teardown().await;
}

// ===========================================================================
// The five explicit merge-back policies
// ===========================================================================

/// A minimal but real JSON Schema contract for the structured merge tests.
fn verdict_contract() -> TemporaryCouncilStructuredContract {
    TemporaryCouncilStructuredContract {
        schema_id: "council.verdict".to_string(),
        schema_version: 1,
        json_schema: serde_json::json!({
            "type": "object",
            "required": ["verdict"],
            "properties": { "verdict": { "type": "string" } },
        }),
    }
}

async fn run_merge_policy(
    id: &str,
    policy: MergeBackPolicy,
    script: impl Fn(&meerkat_client::LlmRequest) -> ScriptedTurn + Send + Sync + 'static,
) -> (CouncilFixture, TemporaryCouncilOutcome) {
    let fixture = CouncilFixture::new(script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;
    let outcome = fixture
        .state
        .temporary_council()
        .run(two_participant_request(&fixture, id, policy, 1, 240))
        .await
        .expect("council reaches a terminal outcome");
    (fixture, outcome)
}

#[tokio::test(flavor = "multi_thread")]
async fn structured_merge_parses_strict_json() {
    let (_fixture, outcome) = run_merge_policy(
        "merge-structured",
        MergeBackPolicy::StructuredResult {
            finalizer: identity("analyst"),
            max_bytes: 2048,
            contract: verdict_contract(),
        },
        role_script,
    )
    .await;

    match &outcome.result.merge {
        TemporaryCouncilMergeOutcome::StructuredResult {
            finalizer,
            value,
            contract,
            ..
        } => {
            assert_eq!(finalizer, &identity("analyst"));
            assert_eq!(value["verdict"], serde_json::json!("agreed"));
            assert_eq!(contract.schema_id, "council.verdict");
            assert_eq!(contract.schema_version, 1);
            assert!(
                !contract.schema_digest.is_empty(),
                "the sealed result names the exact contract it validated against"
            );
        }
        other => panic!("expected a structured result, got {other:?}"),
    }
    assert!(outcome.cleanup.settled());

    _fixture.teardown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn structured_merge_reports_a_typed_failure_for_invalid_json() {
    let (_fixture, outcome) = run_merge_policy(
        "merge-bad-json",
        MergeBackPolicy::StructuredResult {
            finalizer: identity("analyst"),
            max_bytes: 2048,
            contract: verdict_contract(),
        },
        |request| {
            if user_text(request).contains("strict JSON document") {
                // Prose plus a fenced block: strict parsing must refuse it
                // rather than fishing the JSON out.
                ScriptedTurn::Text("Sure! ```json\n{\"verdict\":\"agreed\"}\n```".to_string())
            } else {
                role_script(request)
            }
        },
    )
    .await;

    match &outcome.result.merge {
        TemporaryCouncilMergeOutcome::Failed { policy, detail } => {
            assert_eq!(*policy, TemporaryCouncilMergePolicyKind::StructuredResult);
            assert!(
                detail.contains("strict JSON"),
                "unexpected detail: {detail}"
            );
        }
        other => panic!("expected a typed merge failure, got {other:?}"),
    }
    // A failed merge does not invalidate the discussion.
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed
    );
    assert!(outcome.cleanup.settled());

    _fixture.teardown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_merge_parses_a_typed_handle() {
    let (_fixture, outcome) = run_merge_policy(
        "merge-artifact",
        MergeBackPolicy::DurableArtifactReference {
            participant: identity("critic"),
            max_bytes: 2048,
        },
        role_script,
    )
    .await;

    match &outcome.result.merge {
        TemporaryCouncilMergeOutcome::DurableArtifactReference { participant, claim } => {
            assert_eq!(participant, &identity("critic"));
            assert_eq!(claim.uri, "blob://council/report");
            assert_eq!(claim.media_type.as_deref(), Some("text/markdown"));
            assert_eq!(claim.byte_len, Some(42));
        }
        other => panic!("expected a typed artifact handle, got {other:?}"),
    }

    _fixture.teardown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_merge_reports_a_typed_failure_for_a_non_handle() {
    let (_fixture, outcome) = run_merge_policy(
        "merge-artifact-bad",
        MergeBackPolicy::DurableArtifactReference {
            participant: identity("critic"),
            max_bytes: 2048,
        },
        |request| {
            if user_text(request).contains("artifact you produced") {
                ScriptedTurn::Text("I saved it somewhere.".to_string())
            } else {
                role_script(request)
            }
        },
    )
    .await;

    match &outcome.result.merge {
        TemporaryCouncilMergeOutcome::Failed { policy, .. } => assert_eq!(
            *policy,
            TemporaryCouncilMergePolicyKind::DurableArtifactReference
        ),
        other => panic!("expected a typed merge failure, got {other:?}"),
    }

    _fixture.teardown().await;
}

/// Selected-exchange merge reads exactly the sparse council exchange
/// sequences it was given, enforces the total byte cap, and never touches
/// inherited fork session history.
#[tokio::test(flavor = "multi_thread")]
async fn selected_exchange_merge_reads_sparse_sequences_under_a_byte_cap() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let outcome = fixture
        .state
        .temporary_council()
        .run(two_participant_request(
            &fixture,
            "merge-transcript",
            MergeBackPolicy::SelectedTranscript {
                participant: identity("analyst"),
                // Deliberately sparse and out of order, including a sequence
                // far past the end of a short discussion.
                exchange_sequences: vec![2, 0, 9999],
                max_bytes: 256,
            },
            2,
            240,
        ))
        .await
        .expect("council runs");

    match &outcome.result.merge {
        TemporaryCouncilMergeOutcome::SelectedTranscript {
            participant,
            exchange_sequences,
            excerpts,
            truncated,
        } => {
            assert_eq!(participant, &identity("analyst"));
            assert_eq!(
                exchange_sequences.len(),
                excerpts.len(),
                "every returned sequence has exactly one excerpt"
            );
            assert!(
                exchange_sequences.iter().all(|sequence| *sequence < 100),
                "an out-of-range sequence yields nothing, never a fabricated \
                 excerpt: {exchange_sequences:?}"
            );
            assert!(
                exchange_sequences.len() <= 2,
                "only the requested in-range sequences are read: {exchange_sequences:?}"
            );
            for excerpt in excerpts {
                assert_eq!(
                    excerpt.target_identity,
                    identity("analyst"),
                    "a selection may only read the named participant's own exchanges"
                );
            }
            let total: usize = excerpts.iter().map(|excerpt| excerpt.text.len()).sum();
            assert!(total <= 256, "the total byte cap is enforced (got {total})");
            let _ = truncated;
        }
        other => panic!("expected a selected exchange set, got {other:?}"),
    }
    // No merge turn is taken for this policy: 2 rounds x 2 participants only.
    assert_eq!(outcome.result.exchanges.len(), 4);

    fixture.teardown().await;
}

/// `NoMerge` carries provenance and confirmation only — no content at all.
#[tokio::test(flavor = "multi_thread")]
async fn no_merge_carries_only_provenance_and_confirmation() {
    let (_fixture, outcome) =
        run_merge_policy("merge-none", MergeBackPolicy::NoMerge, role_script).await;

    match &outcome.result.merge {
        TemporaryCouncilMergeOutcome::NoMerge {
            confirmed_participants,
        } => {
            assert_eq!(confirmed_participants.len(), 2);
            assert!(confirmed_participants.contains(&identity("analyst")));
            assert!(confirmed_participants.contains(&identity("critic")));
        }
        other => panic!("expected NoMerge, got {other:?}"),
    }
    // Exactly the discussion turns; no extra merge turn.
    assert_eq!(outcome.result.exchanges.len(), 2);

    _fixture.teardown().await;
}

// ===========================================================================
// Validation, containment, and non-leakage
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn invalid_requests_are_refused_before_any_side_effect() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;
    let coordinator = fixture.state.temporary_council();

    let cases: Vec<(&str, TemporaryCouncilRequest)> = vec![
        ("empty participants", {
            let mut request =
                two_participant_request(&fixture, "bad-empty", MergeBackPolicy::NoMerge, 1, 60);
            request.participants.clear();
            request
        }),
        ("duplicate target identity", {
            let mut request =
                two_participant_request(&fixture, "bad-dup", MergeBackPolicy::NoMerge, 1, 60);
            request.participants[1].target_identity = identity("analyst");
            request
        }),
        ("duplicate source", {
            let mut request =
                two_participant_request(&fixture, "bad-src", MergeBackPolicy::NoMerge, 1, 60);
            request.participants[1].source_identity = identity("researcher");
            request
        }),
        ("duplicate order", {
            let mut request =
                two_participant_request(&fixture, "bad-order", MergeBackPolicy::NoMerge, 1, 60);
            request.participants[1].order = 0;
            request
        }),
        ("unknown profile", {
            let mut request =
                two_participant_request(&fixture, "bad-profile", MergeBackPolicy::NoMerge, 1, 60);
            request.participants[1].target_profile = ProfileName::from("not-declared");
            request
        }),
        ("insufficient scope", {
            let mut request =
                two_participant_request(&fixture, "bad-scope", MergeBackPolicy::NoMerge, 1, 60);
            request.participants[1].scope =
                meerkat_mob::forked_participant::ForkedParticipantOperationScope::Observe;
            request
        }),
        ("deadline over the capability cap", {
            let mut request =
                two_participant_request(&fixture, "bad-deadline", MergeBackPolicy::NoMerge, 1, 60);
            request.bounds.deadline = TemporaryCouncilDeadline::Relative {
                after: Duration::from_secs(25 * 60 * 60),
            };
            request
        }),
        ("round budget over the cap", {
            let mut request =
                two_participant_request(&fixture, "bad-rounds", MergeBackPolicy::NoMerge, 1, 60);
            request.bounds.max_rounds = 10_000;
            request
        }),
        ("result byte cap over the cap", {
            let mut request =
                two_participant_request(&fixture, "bad-bytes", MergeBackPolicy::NoMerge, 1, 60);
            request.bounds.max_result_bytes = 10 * 1024 * 1024;
            request
        }),
        ("merge names a non-participant", {
            two_participant_request(
                &fixture,
                "bad-merge",
                MergeBackPolicy::BoundedTextSummary {
                    finalizer: identity("stranger"),
                    max_bytes: 1024,
                },
                1,
                60,
            )
        }),
        ("selected exchanges repeat a sequence", {
            two_participant_request(
                &fixture,
                "bad-indices",
                MergeBackPolicy::SelectedTranscript {
                    participant: identity("analyst"),
                    exchange_sequences: vec![0, 0],
                    max_bytes: 1024,
                },
                1,
                60,
            )
        }),
    ];

    for (label, request) in cases {
        let id = request.council_id.clone();
        let temporary_mob_id = id.temporary_mob_id();
        match coordinator.run(request).await {
            Err(TemporaryCouncilError::InvalidRequest { .. }) => {}
            Err(other) => panic!("{label}: expected InvalidRequest, got {other:?}"),
            Ok(_) => panic!("{label} must be refused"),
        }
        assert!(
            coordinator.load(&id).await.expect("load").is_none(),
            "{label} must not create durable council custody"
        );
        assert!(
            fixture.state.handle_for(&temporary_mob_id).await.is_err(),
            "{label} must not create a temporary mob"
        );
    }

    fixture.teardown().await;
}

/// The council record and result never carry a capability bearer token, an
/// auth binding, or a whole transcript.
#[tokio::test(flavor = "multi_thread")]
async fn records_and_results_carry_no_bearer_material_or_whole_transcript() {
    // Hold the capability store so the test can read the EXACT bearer tokens
    // the council minted, rather than guessing at their shape.
    let capabilities = Arc::new(meerkat_mob::store::InMemoryForkedParticipantStore::new());
    let capabilities_for_state = capabilities.clone();
    let fixture = CouncilFixture::new_with(role_script, move |state, _root| {
        state.with_forked_participant_store(capabilities_for_state)
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(
        &fixture,
        "no-leak",
        MergeBackPolicy::BoundedTextSummary {
            finalizer: identity("analyst"),
            max_bytes: 1024,
        },
        1,
        240,
    );
    let id = request.council_id.clone();
    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council runs");

    let bearer_tokens: Vec<String> =
        meerkat_mob::store::ForkedParticipantStore::list_all(capabilities.as_ref())
            .await
            .expect("list capability records")
            .into_iter()
            .map(|record| record.capability_id.expose_bearer_token().to_string())
            .collect();
    assert_eq!(bearer_tokens.len(), 2, "both capabilities were minted");

    let record = fixture
        .state
        .temporary_council()
        .load(&id)
        .await
        .expect("load")
        .expect("record present");
    let record_json = serde_json::to_string(&record).expect("encode record");
    let result_json = serde_json::to_string(&outcome.result).expect("encode result");

    // The RESULT is what a caller receives, and it must be safe to hand on:
    // no bearer token, and no credential-shaped field of any kind.
    for token in &bearer_tokens {
        assert!(
            !result_json.contains(token.as_str()),
            "the result must never carry a capability bearer token"
        );
    }
    for forbidden in ["auth_binding", "api_key", "bearer_token", "credential"] {
        for (label, json) in [("record", &record_json), ("result", &result_json)] {
            assert!(
                !json.contains(forbidden),
                "{label} must not carry '{forbidden}'"
            );
        }
    }

    // The RECORD deliberately holds the full immutable reference: that is the
    // holder-side custody issue #159 designs for, and it is the only thing
    // that makes a HOST-owned capability revocable after a crash. It lives in
    // the same realm-scoped, explicitly rooted database as capability custody.
    let held: Vec<String> = record
        .participants
        .iter()
        .filter_map(|participant| participant.capability_ref.as_ref())
        .map(|capability| capability.capability_id().expose_bearer_token().to_string())
        .collect();
    assert_eq!(held.len(), 2, "both references are held as custody");
    for token in &bearer_tokens {
        assert!(
            held.contains(token),
            "council custody holds the exact reference the owner minted"
        );
    }

    // A held reference must never be printable by accident.
    for participant in &record.participants {
        let capability = participant
            .capability_ref
            .as_ref()
            .expect("reference held before attach");
        let rendered = format!("{:?}", capability.capability_id());
        assert!(
            !rendered.contains(capability.capability_id().expose_bearer_token()),
            "capability Debug must stay redacted, got {rendered}"
        );
        assert!(
            participant
                .capability_request_id
                .as_str()
                .starts_with("council:no-leak-")
        );
    }

    // Result provenance carries the source transcript facts, not the bearer.
    for provenance in &outcome.result.participants {
        let capability = provenance.capability.as_ref().expect("seated provenance");
        assert!(!capability.source_provenance.prefix_digest.is_empty());
        assert!(
            !bearer_tokens.contains(&capability.correlation_hint),
            "a correlation hint is not a bearer token"
        );
    }

    // Only bounded exchange text is retained: no source transcript is copied.
    let source_prefix_leak = record
        .exchanges
        .iter()
        .filter_map(|receipt| receipt.completed_text())
        .any(|text| text.len() > 1024);
    assert!(!source_prefix_leak, "exchange text stays receiver-bounded");

    fixture.teardown().await;
}

/// The council never mutates the source members' sessions or policies: the
/// discussion happens on forked branches.
#[tokio::test(flavor = "multi_thread")]
async fn source_member_sessions_are_untouched_by_the_council() {
    use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};

    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let source = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle");
    let mut before = Vec::new();
    for member in ["researcher", "reviewer"] {
        let entry = source
            .get_member(&identity(member))
            .await
            .expect("member read")
            .expect("member seated");
        let session = entry
            .bridge_session_id()
            .expect("local member has a bridge session")
            .clone();
        let page = fixture
            .service
            .read_history(
                &session,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("history read");
        before.push((member, session, page.message_count));
    }

    fixture
        .state
        .temporary_council()
        .run(two_participant_request(
            &fixture,
            "source-untouched",
            MergeBackPolicy::NoMerge,
            2,
            240,
        ))
        .await
        .expect("council runs");

    for (member, session, count) in before {
        let page = fixture
            .service
            .read_history(
                &session,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("history read");
        assert_eq!(
            page.message_count, count,
            "the council must not append to source member {member}'s session"
        );
        let entry = source
            .get_member(&identity(member))
            .await
            .expect("member read")
            .expect("member still seated");
        assert_eq!(
            entry.bridge_session_id(),
            Some(&session),
            "the source member's session binding is unchanged"
        );
    }

    fixture.teardown().await;
}

// ===========================================================================
// Owned-task panic, and capability custody that does not depend on realm-local
// capability records
// ===========================================================================

/// A panic inside the owned execution task must still give every watcher a
/// typed terminal, release the single-flight registration, and leave the
/// record visible to durable recovery.
#[tokio::test(flavor = "multi_thread")]
async fn an_owned_task_panic_yields_a_typed_terminal_and_releases_the_registration() {
    let panicking = Arc::new(std::sync::OnceLock::<Arc<PanicOnceCouncilStore>>::new());
    let panicking_slot = panicking.clone();
    let fixture = CouncilFixture::new_with(role_script, move |state, root| {
        let path = MobMcpState::persistent_forked_participant_store_path(root);
        let inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore> = Arc::new(
            meerkat_mob::store::SqliteTemporaryCouncilStore::open(&path)
                .expect("open durable council custody"),
        );
        // Commit index 7 is the phase=Running advance: two participants are
        // seated and the temporary mob is live when the task dies.
        let store = Arc::new(PanicOnceCouncilStore::new(inner, 7));
        let _ = panicking_slot.set(store.clone());
        state.with_temporary_council_store(store)
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(&fixture, "panic", MergeBackPolicy::NoMerge, 1, 240);
    let id = request.council_id.clone();

    // The owned task is supervised: a panic inside it becomes the SAME typed
    // interrupted terminal a process crash would produce, sealed and cleaned
    // up before the joined caller is answered.
    let supervised = fixture
        .state
        .temporary_council()
        .run(request.clone())
        .await
        .expect("a panicking owned task is supervised into a typed terminal");
    assert_eq!(
        supervised.result.exit_reason,
        TemporaryCouncilExitReason::CoordinatorInterrupted,
        "a panic must never be reported as a completed council"
    );
    assert!(
        matches!(
            supervised.result.merge,
            TemporaryCouncilMergeOutcome::NotAttempted { .. }
        ),
        "a panicked council never applies its merge-back policy: {:?}",
        supervised.result.merge
    );
    assert!(
        supervised.cleanup.temporary_mob_destroyed,
        "the supervisor still runs bounded cleanup: {:?}",
        supervised.cleanup
    );
    assert!(panicking.get().expect("store installed").fired());

    // The registration is released, so the council id is not wedged, and the
    // sealed terminal replays — it is never re-executed.
    let calls_after_panic = fixture.provider_calls();
    let second = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("recovery converges the panicked council");
    assert!(second.replayed, "the recovered terminal is replayed");
    assert_eq!(
        second.result.exit_reason,
        TemporaryCouncilExitReason::CoordinatorInterrupted
    );
    assert_eq!(
        fixture.provider_calls(),
        calls_after_panic,
        "a panicked council must never be re-executed"
    );

    let record = fixture
        .state
        .temporary_council()
        .load(&id)
        .await
        .expect("load")
        .expect("the record survives the panic");
    assert_eq!(
        record.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::Settled
    );

    fixture.teardown().await;
}

/// Cleanup resolves the capability from the COUNCIL record's own custody, not
/// from realm-local capability custody.
///
/// The injected store fails every request-id lookup, which is exactly the
/// condition a HOST-owned capability creates: its owner record lives in the
/// remote host's store and this realm never held it. Revocation must still
/// converge, because the council persisted the full immutable reference before
/// the attach.
#[tokio::test(flavor = "multi_thread")]
async fn cleanup_revokes_from_council_custody_when_realm_capability_custody_cannot_answer() {
    let fixture = CouncilFixture::new_with(role_script, |state, _root| {
        let inner: Arc<dyn meerkat_mob::store::ForkedParticipantStore> =
            Arc::new(meerkat_mob::store::InMemoryForkedParticipantStore::new());
        // usize::MAX failures: realm-local custody can NEVER answer a
        // request-id lookup for an activated record.
        state.with_forked_participant_store(Arc::new(FlakyCapabilityStore::new(inner, usize::MAX)))
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let outcome = fixture
        .state
        .temporary_council()
        .run(partial_failure_request(&fixture, "ref-custody"))
        .await
        .expect("council reaches a terminal outcome");

    match &outcome.result.exit_reason {
        TemporaryCouncilExitReason::ParticipantSeatingFailed {
            participant_order, ..
        } => assert_eq!(*participant_order, 1),
        other => panic!("expected a seating failure, got {other:?}"),
    }
    assert_eq!(
        outcome.cleanup.revoked_participants,
        Vec::<u32>::new(),
        "profile containment rejects before acquiring the invalid participant (cleanup: {:?})",
        outcome.cleanup
    );
    assert!(
        outcome.cleanup.settled(),
        "no realm-local capability lookup is needed: {:?}",
        outcome.cleanup
    );

    let record = fixture
        .state
        .temporary_council()
        .load(&fixture.council_id("ref-custody"))
        .await
        .expect("load")
        .expect("record present");
    let rejected = record.participant(1).expect("participant 1 custody");
    assert!(rejected.capability_ref.is_none());
    assert_eq!(
        rejected.acquisition,
        meerkat_mob::temporary_council::TemporaryCouncilAcquisition::NotAttempted
    );

    fixture.teardown().await;
}

// ===========================================================================
// HOST-owned capability: crash recovery from persisted custody
// ===========================================================================

/// Re-route a real capability reference to a HOST owner route.
///
/// The reference's own serde shape is the only construction seam available
/// outside `meerkat-mob` (`new_source_owned` is crate-private), and it is the
/// same technique the mob-level capability tests use. Everything else about
/// the reference — bearer identity, fork session, provenance digest, scope,
/// expiry, reuse, revocation/cleanup ids — is the one the owner actually
/// minted.
fn rerouted_to_host(
    capability: &meerkat_mob::forked_participant::ForkedParticipantRef,
    realm: &str,
    host: &str,
) -> meerkat_mob::forked_participant::ForkedParticipantRef {
    let mut encoded = serde_json::to_value(capability).expect("serialize capability");
    encoded
        .as_object_mut()
        .expect("capability serializes as an object")
        .insert(
            "owner_route".to_string(),
            serde_json::json!({ "kind": "host", "realm_id": realm, "host_id": host }),
        );
    serde_json::from_value(encoded).expect("well-typed host-routed reference")
}

/// A coordinator that created a HOST-owned capability and died before seating
/// it is recovered from the council record's own custody.
///
/// This is the exact case realm-local capability custody cannot serve: the
/// owner's record lives in the remote host's store, so a request-id lookup in
/// this realm finds nothing. Recovery must therefore resolve the FULL
/// reference the coordinator persisted before the attach, present it to
/// revocation, and — when the owning host is not bound here — retain the
/// obligation as typed debt instead of dropping it and waiting for a TTL.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_capability_acquired_but_never_seated_is_recovered_from_persisted_custody() {
    use meerkat_mob::machines::temporary_council_lifecycle::{
        TemporaryCouncilLifecycleInput, TemporaryCouncilLifecycleMachineAuthority,
        TemporaryCouncilLifecycleMachineMutator,
    };
    use meerkat_mob::store::TemporaryCouncilRecord;
    use meerkat_mob::temporary_council::TemporaryCouncilParticipantCustody;

    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher"]).await;

    // Mint one REAL capability at the source owner, then re-route it to a host
    // that is not bound in this realm.
    let source = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle");
    let local = source
        .create_forked_participant(
            meerkat_mob::MobControlPrincipal::Owner,
            identity("researcher"),
            meerkat_mob::forked_participant::ForkedParticipantRequestId::new("remote-crash-p0")
                .expect("request id"),
            None,
            meerkat_mob::forked_participant::ForkedParticipantOperationScope::InvokeAndObserve,
            meerkat_mob::forked_participant::ForkedParticipantReusePolicy::OneShot,
            Duration::from_secs(600),
        )
        .await
        .expect("mint a real capability");
    let remote = rerouted_to_host(&local, "global", "host-b");

    // Write the crash shape: opened, one participant whose capability was
    // created and persisted, never seated, no result.
    let council_id = fixture.council_id("remote-crash");
    let fingerprint = "tcf1:sha256:remote-crash".to_string();
    let mut authority = TemporaryCouncilLifecycleMachineAuthority::new();
    TemporaryCouncilLifecycleMachineMutator::apply(
        &mut authority,
        TemporaryCouncilLifecycleInput::Open {
            request_fingerprint: fingerprint.clone(),
        },
    )
    .expect("open the council record");
    let now = chrono::Utc::now();
    let record = TemporaryCouncilRecord {
        council_id: council_id.clone(),
        request_fingerprint: fingerprint,
        temporary_mob_id: council_id.temporary_mob_id(),
        deadline: now + chrono::Duration::seconds(600),
        machine_state: authority.state().clone(),
        durability: meerkat_mob::temporary_council::TemporaryCouncilDurability::Durable,
        claim_lease_expires_at: now,
        participants: vec![TemporaryCouncilParticipantCustody {
            order: 0,
            role: "analyst".to_string(),
            source_mob_id: fixture.source_mob_id(),
            source_identity: identity("researcher"),
            target_identity: identity("analyst"),
            target_profile: ProfileName::from("participant"),
            scope:
                meerkat_mob::forked_participant::ForkedParticipantOperationScope::InvokeAndObserve,
            capability_request_id: council_id.capability_request_id(0).expect("request id"),
            capability_correlation_hint: Some(remote.capability_id().correlation_hint()),
            capability_ref: Some(remote.clone()),
            attachment_id: council_id.attachment_id(0).expect("attachment id"),
            acquisition: meerkat_mob::temporary_council::TemporaryCouncilAcquisition::Acquired,
            seated: false,
            seated_session_id: None,
        }],
        exchanges: Vec::new(),
        result: None,
        cleanup: None,
        revision: 0,
        created_at: now,
        updated_at: now,
    };
    fixture
        .state
        .temporary_council_store_for_tests()
        .insert_new(&record)
        .await
        .expect("write the crashed council record");
    fixture
        .state
        .mob_destroy(&fixture.source_mob_id())
        .await
        .expect("remove the exact owner handle before recovery");

    let reports = fixture
        .state
        .temporary_council()
        .recover_unfinished()
        .await
        .expect("recovery sweep");
    let report = reports
        .iter()
        .find(|report| report.council_id == council_id)
        .expect("the crashed council is swept");

    assert!(report.sealed_interrupted_result);
    assert!(
        !report.settled,
        "an unbound owning host cannot discharge the revocation yet"
    );
    assert_eq!(report.cleanup.debts.len(), 1, "{:?}", report.cleanup);
    let debt = &report.cleanup.debts[0];
    assert!(
        debt.subject.contains("remote-crash"),
        "the debt names the exact capability: {debt:?}"
    );
    assert!(
        debt.detail.contains("exact owner handle is unavailable"),
        "a Host route without its exact owner must not use a fallback mob: {debt:?}"
    );
    assert!(
        !debt.detail.contains("capability custody read failed"),
        "recovery must not depend on realm-local capability custody: {debt:?}"
    );
    assert!(
        fixture
            .state
            .forked_participant_store_for_tests()
            .load_by_request_id(&council_id.capability_request_id(0).expect("request id"))
            .await
            .expect("realm custody read")
            .is_none(),
        "this realm never held the remote owner's record"
    );

    // The obligation is durably retained, and a second sweep keeps it rather
    // than dropping it to a TTL backstop.
    let stored = fixture
        .state
        .temporary_council()
        .load(&council_id)
        .await
        .expect("load")
        .expect("record present");
    assert_eq!(
        stored.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::CleanupDebt
    );
    assert!(
        stored
            .participant(0)
            .expect("custody")
            .capability_ref
            .is_some(),
        "the reference stays held so a later retry can still revoke it"
    );

    let again = fixture
        .state
        .temporary_council()
        .recover_unfinished()
        .await
        .expect("second sweep");
    let again = again
        .iter()
        .find(|report| report.council_id == council_id)
        .expect("the obligation is retried, not dropped");
    assert!(!again.sealed_interrupted_result, "the result is immutable");
    assert!(!again.settled);
    // The automatic post-restore sweep may already have retried once, so the
    // contract asserted here is monotonicity, not an exact attempt count.
    assert!(
        again.cleanup.attempts > report.cleanup.attempts,
        "attempts are monotonic: {} then {}",
        report.cleanup.attempts,
        again.cleanup.attempts
    );

    fixture.teardown().await;
}

// ===========================================================================
// Durable coordinator claim, bounded cleanup, acquisition ambiguity,
// deadline-independent replay, explicit durability, and the typed structured
// contract
// ===========================================================================

/// A SECOND coordinator over the SAME durable custody may not execute a
/// council whose lease is live, and takes over only after observing expiry.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_coordinator_over_the_same_custody_is_fenced_until_the_lease_expires() {
    let gate = TurnGate::new();
    let gate_for_script = gate.clone();
    let fixture = CouncilFixture::new(move |request| {
        let text = user_text(request);
        if text.contains("Council topic:") {
            ScriptedTurn::Gated(gate_for_script.clone(), "gated position".to_string())
        } else {
            role_script(request)
        }
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request =
        two_participant_request(&fixture, "claim-fence", MergeBackPolicy::NoMerge, 1, 240);
    let id = request.council_id.clone();

    let first = {
        let state = fixture.state.clone();
        let request = request.clone();
        tokio::spawn(async move { state.temporary_council().run(request).await })
    };
    gate.wait_entered(1).await;

    // A different coordinator identity over the same SQLite custody must be
    // refused: process-local single-flight cannot span processes, so the
    // durable claim is what keeps exactly one executor.
    let second = fixture.restart_state();
    second.set_temporary_council_clock_offset(chrono::Duration::seconds(150));
    let error = second
        .temporary_council()
        .run(request.clone())
        .await
        .expect_err("a live lease fences a second coordinator");
    match error {
        TemporaryCouncilError::HeldByAnotherCoordinator {
            council_id,
            current_claim_epoch,
        } => {
            assert_eq!(council_id, id);
            assert_eq!(current_claim_epoch, 1);
        }
        other => panic!("expected a claim refusal, got {other:?}"),
    }

    gate.open();
    let outcome = first
        .await
        .expect("the owned task joins")
        .expect("the first coordinator completes its council");
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed
    );

    // Once the record is settled, a takeover attempt is refused as terminal
    // rather than granted — there is nothing left to execute.
    let record = second
        .temporary_council()
        .load(&id)
        .await
        .expect("load")
        .expect("record present");
    assert_eq!(
        record.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::Settled
    );
    let replay = second
        .temporary_council()
        .run(request)
        .await
        .expect("a settled council replays for any coordinator");
    assert!(replay.replayed);
    assert_eq!(replay.result, outcome.result);

    fixture.teardown().await;
}

/// A cleanup budget that is already spent must NOT hold the joined caller:
/// the sealed result publishes with an explicit pending receipt, and the
/// outstanding obligations converge on a later sweep.
#[tokio::test(flavor = "multi_thread")]
async fn an_exhausted_cleanup_budget_publishes_the_sealed_result_as_pending() {
    let fixture = CouncilFixture::new_with(role_script, |state, _root| {
        state.with_temporary_council_cleanup_budget(Duration::from_millis(0))
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request =
        two_participant_request(&fixture, "cleanup-budget", MergeBackPolicy::NoMerge, 1, 240);
    let id = request.council_id.clone();

    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("the council still returns its sealed result");

    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed,
        "the result is sealed BEFORE cleanup, so a starved budget cannot spoil it"
    );
    assert_eq!(
        outcome.cleanup.status(),
        TemporaryCouncilCleanupStatus::Pending
    );
    assert!(outcome.cleanup.budget_exhausted);
    assert!(
        !outcome.cleanup.debts.is_empty(),
        "the outstanding obligations are explicit: {:?}",
        outcome.cleanup
    );

    let record = fixture
        .state
        .temporary_council()
        .load(&id)
        .await
        .expect("load")
        .expect("record present");
    assert_eq!(
        record.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::CleanupDebt,
        "unfinished cleanup stays visible to the recovery sweep"
    );
    assert_eq!(record.result.as_ref(), Some(&outcome.result));

    // Give the budget back and let the ordinary retry path converge.
    fixture
        .state
        .set_temporary_council_cleanup_budget(Duration::from_secs(30));
    let reports = fixture
        .state
        .temporary_council()
        .recover_unfinished()
        .await
        .expect("cleanup retry sweep");
    let report = reports
        .iter()
        .find(|report| report.council_id == id)
        .expect("the pending obligation is retried");
    assert!(!report.sealed_interrupted_result, "the result is immutable");
    assert!(report.settled, "retry converges: {:?}", report.cleanup);

    fixture.teardown().await;
}

/// An exact replay must be answered from the sealed record even when the
/// council's absolute deadline is long past: a lost response retried later is
/// a retry, not a new admission.
///
/// The expiry is observed through the coordinator's injected clock rather than
/// by sleeping, so the first execution always runs with a safe deadline and
/// the replay always observes an unambiguously elapsed one.
#[tokio::test(flavor = "multi_thread")]
async fn an_exact_replay_after_the_absolute_deadline_returns_the_sealed_result() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    // A real absolute deadline, comfortably far away for the first run. It is
    // part of the request fingerprint, so the replay presents this exact value.
    let deadline = chrono::Utc::now() + chrono::Duration::minutes(10);
    let mut request = two_participant_request(
        &fixture,
        "replay-deadline",
        MergeBackPolicy::NoMerge,
        1,
        240,
    );
    request.bounds.deadline = TemporaryCouncilDeadline::Absolute { at: deadline };

    let first = fixture
        .state
        .temporary_council()
        .run(request.clone())
        .await
        .expect("the council completes inside its deadline");
    assert_eq!(
        first.result.exit_reason,
        TemporaryCouncilExitReason::Completed
    );
    let calls_after_first = fixture.provider_calls();

    // Move the coordinator's clock past the deadline. A NEW admission carrying
    // this request would now be refused outright by deadline validation; the
    // sealed record must still answer, because the replay lookup runs first.
    fixture
        .state
        .set_temporary_council_clock_offset(chrono::Duration::minutes(30));
    let mut control = two_participant_request(
        &fixture,
        "replay-deadline-fresh",
        MergeBackPolicy::NoMerge,
        1,
        240,
    );
    control.bounds.deadline = TemporaryCouncilDeadline::Absolute { at: deadline };
    assert!(
        matches!(
            fixture.state.temporary_council().run(control).await,
            Err(TemporaryCouncilError::InvalidRequest { .. })
        ),
        "the control: an unseen request carrying this same deadline is now refused"
    );

    let replay = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("an elapsed deadline must not turn a retry into a failure");
    assert!(replay.replayed, "a past deadline must not re-execute");
    assert_eq!(replay.result, first.result);
    assert_eq!(
        fixture.provider_calls(),
        calls_after_first,
        "a replay never takes another turn"
    );

    fixture
        .state
        .set_temporary_council_clock_offset(chrono::Duration::zero());
    fixture.teardown().await;
}

/// A custody commit that fails AFTER the create call must leave explicit
/// ambiguity, never a false absence, and the held reference must still let
/// cleanup revoke the capability.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_post_create_custody_commit_is_explicitly_ambiguous_and_still_revocable() {
    let failing = Arc::new(std::sync::OnceLock::<Arc<OneShotFailingCouncilStore>>::new());
    let failing_slot = failing.clone();
    let fixture = CouncilFixture::new_with(role_script, move |state, root| {
        let path = MobMcpState::persistent_forked_participant_store_path(root);
        let inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore> = Arc::new(
            meerkat_mob::store::SqliteTemporaryCouncilStore::open(&path)
                .expect("open durable council custody"),
        );
        // Commit order: 0 claim, 1 p0 acquire-intent, 2 p0 capability. Index 2
        // is the exact post-create commit: the capability EXISTS and the
        // coordinator is holding it when custody refuses the write.
        let store = Arc::new(OneShotFailingCouncilStore::new(inner, 2));
        let _ = failing_slot.set(store.clone());
        state.with_temporary_council_store(store)
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(
        &fixture,
        "commit-uncertain",
        MergeBackPolicy::NoMerge,
        1,
        240,
    );
    let id = request.council_id.clone();

    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("the coordinator seals a typed terminal");
    assert!(failing.get().expect("store installed").fired());
    assert!(
        matches!(
            outcome.result.exit_reason,
            TemporaryCouncilExitReason::ParticipantSeatingFailed {
                participant_order: 0,
                ..
            }
        ),
        "unexpected exit: {:?}",
        outcome.result.exit_reason
    );

    let record = fixture
        .state
        .temporary_council()
        .load(&id)
        .await
        .expect("load")
        .expect("record present");
    let custody = record.participant(0).expect("participant custody");
    assert_eq!(
        custody.acquisition,
        TemporaryCouncilAcquisition::Ambiguous,
        "a failed post-create commit is recorded as explicit ambiguity"
    );
    assert!(
        custody.capability_ref.is_some(),
        "the reference is what makes the ambiguity resolvable"
    );
    assert!(!custody.seated);

    // Cleanup used that exact reference to revoke, so nothing is left leaking.
    assert!(
        outcome.cleanup.revoked_participants.contains(&0),
        "the ambiguous capability is revoked, not abandoned: {:?}",
        outcome.cleanup
    );
    assert!(
        outcome.cleanup.settled(),
        "no debt remains: {:?}",
        outcome.cleanup
    );

    fixture.teardown().await;
}

/// Durable admission requires durable custody; process-bound execution is an
/// explicit, request-carried, result-carried opt-in.
#[tokio::test(flavor = "multi_thread")]
async fn process_bound_execution_is_an_explicit_opt_in_over_non_durable_custody() {
    let fixture = CouncilFixture::new_with(role_script, |state, _root| {
        state.with_temporary_council_store(Arc::new(
            meerkat_mob::store::InMemoryTemporaryCouncilStore::new(),
        ))
    });
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(&fixture, "durability", MergeBackPolicy::NoMerge, 1, 240);
    let id = request.council_id.clone();

    match fixture.state.temporary_council().run(request.clone()).await {
        Err(TemporaryCouncilError::DurabilityUnavailable { council_id }) => {
            assert_eq!(council_id, id);
        }
        Err(other) => panic!("expected a durability refusal, got {other:?}"),
        Ok(_) => panic!("a durable council must never silently run on process-bound custody"),
    }
    assert_eq!(
        fixture.provider_calls(),
        0,
        "the refusal lands before any side effect"
    );

    let outcome = fixture
        .state
        .temporary_council()
        .run(request.process_bound())
        .await
        .expect("an explicit process-bound council is admitted");
    assert_eq!(
        outcome.result.durability,
        TemporaryCouncilDurability::ProcessBound,
        "the result never claims crash recovery it does not have"
    );

    fixture.teardown().await;
}

/// A selection may only reach council-produced exchanges. Content the fork
/// INHERITED from its source (here, the source member's own system prompt)
/// can never be selected, whatever sequence is asked for.
#[tokio::test(flavor = "multi_thread")]
async fn a_selection_can_never_reach_content_inherited_from_the_source_session() {
    const MARKER: &str = "SOURCE-ONLY-PROMPT-MARKER-9f13";

    let fixture = CouncilFixture::new(role_script);
    fixture
        .seed_source_mob_with_description(&["researcher", "reviewer"], MARKER)
        .await;

    let mut request = two_participant_request(
        &fixture,
        "prefix-leak",
        MergeBackPolicy::SelectedTranscript {
            participant: identity("analyst"),
            // Sequence 0 is the LOWEST selectable index. If the selection
            // read raw fork history it would land in the inherited prefix.
            exchange_sequences: vec![0],
            max_bytes: 4096,
        },
        1,
        240,
    );
    request.definition_template =
        council_definition_with_description("template-is-replaced", MARKER);
    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council runs");

    let TemporaryCouncilMergeOutcome::SelectedTranscript { excerpts, .. } = &outcome.result.merge
    else {
        panic!(
            "expected a selected exchange set, got {:?}",
            outcome.result.merge
        );
    };
    assert_eq!(excerpts.len(), 1);
    let selected = &excerpts[0];
    assert_eq!(selected.sequence, 0);
    assert_eq!(selected.target_identity, identity("analyst"));
    assert_eq!(
        selected.text,
        outcome.result.exchanges[0]
            .completed_text()
            .expect("the first council exchange completed"),
        "sequence 0 is the first COUNCIL exchange, not an inherited message"
    );

    let serialized = serde_json::to_string(&outcome.result).expect("serialize the result");
    assert!(
        !serialized.contains(MARKER),
        "inherited source prompt content must never reach a council result"
    );
    let record = fixture
        .state
        .temporary_council()
        .load(&outcome.result.council_id)
        .await
        .expect("load")
        .expect("record present");
    let record_json = serde_json::to_string(&record).expect("serialize the record");
    assert!(
        !record_json.contains(MARKER),
        "inherited source prompt content must never reach council custody"
    );

    fixture.teardown().await;
}

/// A structured merge is validated against the caller's typed contract, and a
/// schema violation is a typed merge failure — not a sealed result.
#[tokio::test(flavor = "multi_thread")]
async fn a_structured_merge_that_violates_its_contract_is_a_typed_failure() {
    let (_fixture, outcome) = run_merge_policy(
        "merge-schema-violation",
        MergeBackPolicy::StructuredResult {
            finalizer: identity("analyst"),
            max_bytes: 2048,
            contract: TemporaryCouncilStructuredContract {
                schema_id: "council.verdict".to_string(),
                schema_version: 1,
                json_schema: serde_json::json!({
                    "type": "object",
                    "required": ["verdict"],
                    "properties": { "verdict": { "type": "integer" } },
                }),
            },
        },
        role_script,
    )
    .await;

    match &outcome.result.merge {
        TemporaryCouncilMergeOutcome::Failed { policy, detail } => {
            assert_eq!(*policy, TemporaryCouncilMergePolicyKind::StructuredResult);
            assert!(
                detail.contains("council.verdict"),
                "the failure names the contract it violated: {detail}"
            );
        }
        other => panic!("expected a typed schema failure, got {other:?}"),
    }
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed,
        "a failed merge does not invalidate the discussion"
    );

    _fixture.teardown().await;
}

/// A structured contract that is not a usable JSON Schema is refused at
/// validation time, before any side effect.
#[tokio::test(flavor = "multi_thread")]
async fn an_uncompilable_structured_contract_is_refused_before_any_side_effect() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;

    let request = two_participant_request(
        &fixture,
        "bad-schema",
        MergeBackPolicy::StructuredResult {
            finalizer: identity("analyst"),
            max_bytes: 2048,
            contract: TemporaryCouncilStructuredContract {
                schema_id: "council.verdict".to_string(),
                schema_version: 1,
                json_schema: serde_json::json!({ "type": "not-a-json-schema-type" }),
            },
        },
        1,
        240,
    );
    let id = request.council_id.clone();

    match fixture.state.temporary_council().run(request).await {
        Err(TemporaryCouncilError::InvalidRequest { .. }) => {}
        Err(other) => panic!("expected InvalidRequest, got {other:?}"),
        Ok(_) => panic!("an uncompilable contract must be refused"),
    }
    assert_eq!(fixture.provider_calls(), 0, "no turn was taken");
    assert!(
        fixture
            .state
            .temporary_council()
            .load(&id)
            .await
            .expect("load")
            .is_none(),
        "a refused request never persists a council record"
    );

    fixture.teardown().await;
}

/// Persistent restoration alone must converge unfinished councils: an
/// operator never has to remember to call a maintenance verb for a crashed
/// coordinator's obligations to be discharged.
#[tokio::test(flavor = "multi_thread")]
async fn restoration_alone_schedules_recovery_for_unfinished_councils() {
    use meerkat_mob::machines::temporary_council_lifecycle::{
        TemporaryCouncilLifecycleInput, TemporaryCouncilLifecycleMachineAuthority,
        TemporaryCouncilLifecycleMachineMutator,
    };
    use meerkat_mob::store::TemporaryCouncilRecord;

    let fixture = CouncilFixture::new(role_script);

    // A fresh state over the SAME durable custody, standing in for the
    // process that comes up after the crash. Nothing has restored it yet.
    let restarted = fixture.restart_state();
    let store = restarted.temporary_council_store_for_tests();

    let council_id = fixture.council_id("auto-recover");
    let fingerprint = "tcf1:sha256:auto-recover".to_string();
    let mut authority = TemporaryCouncilLifecycleMachineAuthority::new();
    TemporaryCouncilLifecycleMachineMutator::apply(
        &mut authority,
        TemporaryCouncilLifecycleInput::Open {
            request_fingerprint: fingerprint.clone(),
        },
    )
    .expect("open the council record");
    let now = chrono::Utc::now();
    store
        .insert_new(&TemporaryCouncilRecord {
            council_id: council_id.clone(),
            request_fingerprint: fingerprint,
            temporary_mob_id: council_id.temporary_mob_id(),
            deadline: now + chrono::Duration::seconds(600),
            machine_state: authority.state().clone(),
            durability: meerkat_mob::temporary_council::TemporaryCouncilDurability::Durable,
            // Already expired: the coordinator that held it is gone.
            claim_lease_expires_at: now - chrono::Duration::seconds(1),
            participants: Vec::new(),
            exchanges: Vec::new(),
            result: None,
            cleanup: None,
            revision: 0,
            created_at: now,
            updated_at: now,
        })
        .await
        .expect("write the crashed council record");

    // Any ordinary mob verb restores the state. NO council maintenance verb is
    // called anywhere in this test.
    let _ = restarted.mob_handles_snapshot().await;

    let mut settled = None;
    for _ in 0..400 {
        let record = store
            .load(&council_id)
            .await
            .expect("load")
            .expect("record present");
        if record.result.is_some() {
            settled = Some(record);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let record = settled.expect("restoration must converge the unfinished council on its own");
    assert_eq!(
        record.result.as_ref().expect("result sealed").exit_reason,
        TemporaryCouncilExitReason::CoordinatorInterrupted,
        "an abandoned council is sealed as interrupted, never re-executed"
    );
    assert_eq!(fixture.provider_calls(), 0, "recovery never takes a turn");

    fixture.teardown().await;
}
