//! Request-only context (`RuntimeTurnMetadata::request_only_context`) lives
//! only in the process that admitted it: it is not part of the prompt replay
//! identity, a stored input never holds it, an input recovered after a
//! restart carries none, and a person's steer that joins their running turn
//! carries its own into the turn.
//!
//! These rows drive the durable steer rows' scripted runner through the
//! runtime ingress, the generated machine and the runtime loop.
use super::*;
use meerkat_core::lifecycle::run_primitive::TurnRequestContext;
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

/// A person the host authenticated: the opaque, qualified requester Core
/// carries and never interprets.
fn person(subject: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        subject,
        TrustDomainId::new("oidc-issuer:https://issuer.example").expect("trust domain"),
    )
    .expect("qualified requester")
}

fn with_requester(mut input: Input, requester: PrincipalRef) -> Input {
    input.header_mut().requester = Some(requester);
    input
}

/// `input`, a prompt, with exactly `sections` as its request-only context.
fn with_sections(mut input: Input, sections: &[&str]) -> Input {
    let Input::Prompt(prompt) = &mut input else {
        panic!("only a prompt carries caller turn metadata here");
    };
    prompt
        .turn_metadata
        .get_or_insert_with(Default::default)
        .request_only_context = sections
        .iter()
        .map(|section| TurnRequestContext::new(*section).expect("non-empty section"))
        .collect();
    input
}

fn texts(contexts: &[TurnRequestContext]) -> Vec<&str> {
    contexts.iter().map(TurnRequestContext::as_str).collect()
}

/// Two sends that differ only in the request-only context they recall are the
/// same prompt: the replay identity leaves it out, so a retry that recalls
/// another section replays the original instead of conflicting with it.
#[test]
fn request_only_context_is_not_part_of_the_replay_identity() {
    let send = |sections: &[&str]| {
        let mut input = with_sections(
            Input::Prompt(crate::input::PromptInput::new("the same words", None)),
            sections,
        );
        input.header_mut().idempotency_key =
            Some(crate::identifiers::IdempotencyKey::new("one-send"));
        input
    };
    let identity = |input: &Input| {
        crate::input_state::PromptReplayIdentity::from_input(input)
            .expect("replay identity")
            .expect("a keyed prompt has a replay identity")
    };
    let original = identity(&send(&["the first section"]));
    assert_eq!(
        original,
        identity(&send(&["another section"])),
        "a retry that recalls another section is the same prompt"
    );
    assert_eq!(
        original,
        identity(&send(&[])),
        "a retry that recalls nothing is the same prompt"
    );
}

/// A stored input never holds its request-only context, and the input the
/// runtime recovers after a restart runs with none: the recovered run recalls
/// nothing rather than replay a section from the store.
#[tokio::test]
async fn a_restart_drops_request_only_context() {
    let dir = tempfile::tempdir().expect("store dir");
    let store: Arc<dyn crate::store::RuntimeStore> = Arc::new(
        crate::store::sqlite::SqliteRuntimeStore::new(dir.path().join("runtime.sqlite"))
            .expect("sqlite runtime store"),
    );
    let first = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let session_id = first.session_id.clone();
    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
    // A requester-less run keeps the runtime busy, so the person's prompt
    // stays queued across the restart.
    let busy = first.start_busy_turn().await;
    let prompt = with_sections(
        with_requester(
            Input::Prompt(crate::input::PromptInput::new("queued for a", None)),
            person("a"),
        ),
        &["a's section"],
    );
    let queued = prompt.id().clone();
    first.admit(prompt).await;
    let stored = store
        .load_input_state(&runtime_id, &queued)
        .await
        .expect("load row")
        .expect("row persisted");
    let Some(Input::Prompt(stored_prompt)) = stored.state.persisted_input.as_ref() else {
        panic!("the queued prompt's payload is stored");
    };
    assert!(
        stored_prompt
            .turn_metadata
            .as_ref()
            .is_none_or(|metadata| metadata.request_only_context.is_empty()),
        "a stored input never holds request-only context"
    );

    // The process stops with the run in flight; the store keeps both rows.
    std::mem::forget(first);
    let restarted = DurableSteerRig::with_adapter_for_session(
        Arc::new(
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("persistent machine"),
        ),
        session_id,
    )
    .await;
    for expected_calls in 1..=3 {
        if restarted.phase(&queued).await == Some(InputLifecycleState::Consumed)
            && restarted.phase(&busy).await == Some(InputLifecycleState::Consumed)
        {
            break;
        }
        restarted.wait_for_apply_calls(expected_calls).await;
        restarted.script.step(RunnerStep::Finish);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    restarted
        .wait_for_phase(&queued, InputLifecycleState::Consumed)
        .await;
    let recovered = restarted
        .script
        .primitive_metadata()
        .into_iter()
        .find(|(contributors, _)| contributors.contains(&queued))
        .expect("a run after the restart carried the recovered prompt");
    assert!(
        recovered
            .1
            .as_ref()
            .is_none_or(|metadata| metadata.request_only_context.is_empty()),
        "the recovered input runs with no request-only context"
    );
}

/// A person's steer that joins their running turn carries its request-only
/// context into the turn: the durable delivery the runtime prepares for the
/// running run holds it, beside the request-only context the run started
/// with. No follow-up turn runs.
#[tokio::test]
async fn a_same_requester_join_carries_its_request_only_context() {
    let rig = DurableSteerRig::ephemeral().await;
    let first = with_sections(
        with_requester(
            Input::Prompt(crate::input::PromptInput::new(
                "a's prompt keeps the runtime busy",
                None,
            )),
            person("a"),
        ),
        &["a's section"],
    );
    let first_id = first.id().clone();
    let (outcome, _completion) = rig
        .adapter
        .accept_input_with_completion(&rig.session_id, first)
        .await
        .expect("a's prompt is accepted");
    assert!(outcome.is_accepted());
    tokio::time::timeout(Duration::from_secs(5), rig.script.apply_started.notified())
        .await
        .expect("a's run starts");
    let started = rig.script.primitive_metadata();
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0]
            .1
            .as_ref()
            .map(|metadata| texts(&metadata.request_only_context)),
        Some(vec!["a's section"]),
        "the run starts with its batch's request-only context"
    );

    let steer = with_sections(
        with_requester(text_prompt_steer("from a"), person("a")),
        &["a's section", "a's newer section"],
    );
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_applied_durable(1).await;
    assert_eq!(rig.script.applied_durable(), vec![steer_id.clone()]);
    let prepared = rig.script.prepared_request_only_context();
    assert_eq!(
        prepared
            .iter()
            .map(|contexts| texts(contexts))
            .collect::<Vec<_>>(),
        vec![vec!["a's section", "a's newer section"]],
        "the joining steer carries its request-only context into the running turn"
    );

    rig.script.step(RunnerStep::Finish);
    for input_id in [&first_id, &steer_id] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "the steer joined the running run; no follow-up turn ran"
    );
}
