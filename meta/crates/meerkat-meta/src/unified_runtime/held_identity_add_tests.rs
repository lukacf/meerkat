//! One fresh member added, held before its publication, to a running
//! identity roster (`IdentityFirstRuntimeContext::add_member_held`).
//!
//! The other members keep running untouched. The add's one terminal
//! decision (its deadline, its caller's loss, the host's completion)
//! decides the added member alone, and an aborted add settles its member's
//! cleanup without touching the roster. The host lists the member from
//! inside its completion: a roster pass waits for the add and never
//! embodies the member unheld, and one that reads the listing of an
//! aborted add keeps the member refused. A member embodied outside the add
//! refuses it, an identity Core already spawned (live or retired) is not
//! added again, a host-adopted identity is refused typed, an added member
//! retires like any other, and a lazy roster adds its member at once. An add
//! still completing cuts a shutdown's in-flight drain at its bound, before
//! the drain stops the mob, and holds a shutdown before its Stop, by which
//! point the shutdown has closed the cross-mob control listener.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::identity_first::{
    AgentIdentity, ContinuityStore, HeldBootstrapRefusal, HeldBootstrapStart,
    HeldCompletionOutcome, IdentityFirstRuntimeContext, IdentityLifecycleState,
    IdentityRuntimeError, LocalContinuityStore, MutableRosterProvider, RestoreOutcome,
    RosterProvider,
};

use super::held_identity_test_support::{governed_runtime, identity_runtime, member};

/// The setup deadline of an add that is meant to activate.
const HOLD: Duration = Duration::from_mins(1);

/// A running roster of one member, bootstrapped held, whose roster provider
/// lists that member: a test lists the member it adds from inside the add's
/// completion, as a host does.
struct Fixture {
    runtime: crate::UnifiedRuntime,
    context: Arc<IdentityFirstRuntimeContext>,
    roster: Arc<MutableRosterProvider>,
    running: AgentIdentity,
    added: AgentIdentity,
    running_session: meerkat_core::types::SessionId,
}

impl Fixture {
    async fn start(dir: &tempfile::TempDir, prefix: &str) -> Self {
        let continuity = Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
        let running = AgentIdentity::parse(&format!("{prefix}-running")).unwrap();
        let added = AgentIdentity::parse(&format!("{prefix}-added")).unwrap();
        let mut runtime = governed_runtime(dir, prefix).await;
        let identity_runtime = identity_runtime(
            &runtime,
            Arc::clone(&continuity) as Arc<dyn ContinuityStore>,
            prefix,
        );
        let roster = Arc::new(MutableRosterProvider::new(vec![member(&running)]));
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            identity_runtime,
            Arc::clone(&roster) as Arc<dyn RosterProvider>,
            None,
            None,
            Some(runtime.mob_handle().definition().clone()),
        ));
        runtime.install_identity_first_context(Arc::clone(&context));
        let restored = runtime
            .bootstrap_installed_identity_first_context_held(
                &[member(&running)],
                tokio::time::Instant::now() + HOLD,
                Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
            )
            .await
            .expect("bootstrap the running member");
        assert!(
            matches!(
                restored.outcomes.get(&running),
                Some(RestoreOutcome::Created { .. })
            ),
            "{:?}",
            restored.outcomes
        );
        let running_session = context
            .runtime
            .status(&running)
            .await
            .expect("running status")
            .session_id
            .expect("running session");
        Self {
            runtime,
            context,
            roster,
            running,
            added,
            running_session,
        }
    }

    async fn state(&self, identity: &AgentIdentity) -> Option<IdentityLifecycleState> {
        self.context
            .runtime
            .status(identity)
            .await
            .ok()
            .map(|status| status.state)
    }

    /// Whether Core's roster holds `identity`'s member.
    async fn core_holds_member(&self, identity: &AgentIdentity) -> bool {
        self.runtime
            .mob_handle()
            .get_member(&crate::member_comms_id::roster_member_id_for_identity(
                identity.as_str(),
            ))
            .await
            .expect("Core's roster read")
            .is_some()
    }

    async fn members(&self) -> usize {
        self.runtime.mob_handle().list_members().await.len()
    }

    /// The running member is untouched: still active on its own session.
    async fn assert_running_untouched(&self) {
        let status = self
            .context
            .runtime
            .status(&self.running)
            .await
            .expect("running status");
        assert_eq!(status.state, IdentityLifecycleState::Active, "{status:?}");
        assert_eq!(status.session_id.as_ref(), Some(&self.running_session));
    }

    /// The added member was not published: Core's roster holds only the
    /// running member, and the added one is not active.
    async fn assert_added_not_published(&self) {
        assert_eq!(
            self.members().await,
            1,
            "only the running member is published"
        );
        assert!(!self.core_holds_member(&self.added).await);
        assert_ne!(
            self.state(&self.added).await,
            Some(IdentityLifecycleState::Active)
        );
    }

    async fn add(
        &self,
        deadline: tokio::time::Instant,
        refusal: Option<&'static str>,
    ) -> Result<HeldBootstrapStart, IdentityRuntimeError> {
        let expected = self.added.clone();
        let roster = Arc::clone(&self.roster);
        let listed = member(&self.added);
        self.context
            .add_member_held(
                member(&self.added),
                deadline,
                move |members, _completion| async move {
                    let held: Vec<_> = members.into_iter().map(|member| member.identity).collect();
                    if held != vec![expected] {
                        return Err(format!("unexpected held inventory: {held:?}"));
                    }
                    match refusal {
                        Some(reason) => Err(reason.to_string()),
                        None => {
                            // The host lists the member it completed.
                            roster.upsert(listed);
                            Ok(())
                        }
                    }
                },
            )
            .await
    }
}

impl Fixture {
    /// The host adopts the running member through Core: Core then holds an
    /// identity intent row for it.
    async fn adopt_running(&self) {
        let handle = self.runtime.mob_handle();
        let core_identity =
            crate::member_comms_id::roster_member_id_for_identity(self.running.as_str());
        let entry = handle
            .get_member(&core_identity)
            .await
            .expect("Core's roster read")
            .expect("a live member to adopt");
        let request = meerkat_mob::AdoptMemberIdentityDeclaration {
            mob_id: handle.definition().id.clone(),
            agent_identity: core_identity,
            request_id: meerkat_mob::IdentityAdoptionId::new("held-add-adoption")
                .expect("valid adoption id"),
            precondition: meerkat_mob::IdentityAdoptionPrecondition::ExpectedAbsent,
            declaration_scope: meerkat_mob::IdentityDeclarationScopeId::new("held-add-host")
                .expect("valid declaration scope"),
            declaration_revision: 1,
            session: meerkat_mob::DesiredSessionTarget {
                session_id: self.running_session.clone(),
                lineage_id: meerkat_core::SessionLineageId::for_session(&self.running_session),
                lineage_generation: meerkat_core::SessionGeneration::INITIAL,
                authority_policy: meerkat_mob::DesiredSessionAuthorityPolicy::RequireExisting,
            },
            member: meerkat_mob::IdentityProfileMemberDeclaration {
                profile_name: entry.role.clone(),
                profile_override: None,
                model_override: None,
                external_addressable_override: None,
                context: None,
                labels: Some(entry.labels.clone()),
                additional_instructions: None,
                system_prompt_override: None,
                tool_access_policy: None,
                auth_binding: None,
                budget_limits: None,
                runtime_mode: None,
                required_env_keys: Vec::new(),
                required_local_callback_tools: Vec::new(),
                execution: meerkat_mob::DesiredExecution::ControllingSession,
            },
            wiring_custody: Default::default(),
            owned_wiring: Default::default(),
            convergence: meerkat_mob::IdentityConvergenceMode::Drain { max_wait_ms: 1_000 },
        };
        let adopted = handle
            .adopt_member_identity_declaration(request)
            .await
            .expect("Core adopts the live member");
        assert!(
            matches!(
                adopted.adoption,
                meerkat_mob::IdentityAdoptionOutcome::Adopted { .. }
            ),
            "{:?}",
            adopted.adoption
        );
    }
}

fn activated(start: HeldBootstrapStart) -> crate::identity_first::RestoreFlowResult {
    match start {
        HeldBootstrapStart::Activated(result) => result,
        HeldBootstrapStart::Unsettled(_) => panic!("the held add did not activate"),
    }
}

async fn settled_error(start: HeldBootstrapStart) -> IdentityRuntimeError {
    match start {
        HeldBootstrapStart::Unsettled(settlement) => settlement
            .settled()
            .await
            .expect_err("an aborted add settles as an error"),
        HeldBootstrapStart::Activated(result) => {
            panic!("the held add activated: {:?}", result.outcomes)
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_is_added_held_while_the_roster_keeps_running() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add").await;
    let result = activated(
        fixture
            .add(tokio::time::Instant::now() + HOLD, None)
            .await
            .expect("the held add starts"),
    );
    assert_eq!(result.outcomes.len(), 1, "{:?}", result.outcomes);
    assert!(
        matches!(
            result.outcomes.get(&fixture.added),
            Some(RestoreOutcome::Created { .. })
        ),
        "{:?}",
        result.outcomes
    );
    assert!(result.managed_edges.is_empty());
    assert_eq!(
        fixture.state(&fixture.added).await,
        Some(IdentityLifecycleState::Active)
    );
    assert!(fixture.core_holds_member(&fixture.added).await);
    assert_eq!(fixture.members().await, 2);
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_add_the_deadline_ends_settles_its_member_and_leaves_the_roster_alone() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-deadline").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let start = fixture
        .context
        .add_member_held(
            member(&fixture.added),
            deadline,
            |_members, completion| async move {
                // The host is still completing at the deadline.
                completion.aborted().await;
                Err("aborted while completing".to_string())
            },
        )
        .await
        .expect("the held add starts");
    assert!(tokio::time::Instant::now() >= deadline);
    match settled_error(start).await {
        IdentityRuntimeError::HeldBootstrapAborted {
            refusal: HeldBootstrapRefusal::DeadlineElapsed,
            completion: HeldCompletionOutcome::Refused(_),
        } => {}
        other => panic!("the add ends at its deadline: {other:?}"),
    }
    fixture.assert_added_not_published().await;
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_add_whose_caller_goes_away_aborts_its_member() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-caller").await;
    let (asked_tx, asked_rx) = tokio::sync::oneshot::channel();
    let (aborted_tx, aborted_rx) = tokio::sync::oneshot::channel();
    let context = Arc::clone(&fixture.context);
    let added = member(&fixture.added);
    let caller = tokio::spawn(async move {
        context
            .add_member_held(
                added,
                tokio::time::Instant::now() + HOLD,
                move |_members, completion| async move {
                    let _ = asked_tx.send(());
                    completion.aborted().await;
                    let _ = aborted_tx.send(());
                    Err("the caller went away".to_string())
                },
            )
            .await
    });
    asked_rx.await.expect("the host is asked to complete");
    caller.abort();
    assert!(caller.await.is_err(), "the caller was cancelled");
    tokio::time::timeout(Duration::from_secs(10), aborted_rx)
        .await
        .expect("the host sees the abort")
        .expect("abort signal");
    // The tracked operation still owns the member's abort and cleanup.
    fixture.context.runtime.join_foreground_operations().await;
    fixture.assert_added_not_published().await;
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_add_the_host_refuses_aborts_its_member() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-refused").await;
    let start = fixture
        .add(
            tokio::time::Instant::now() + HOLD,
            Some("refused by the host"),
        )
        .await
        .expect("the held add starts");
    match settled_error(start).await {
        IdentityRuntimeError::HeldBootstrapAborted {
            refusal: HeldBootstrapRefusal::HostRefused(reason),
            completion: HeldCompletionOutcome::Refused(_),
        } if reason == "refused by the host" => {}
        other => panic!("the host's refusal aborts the add: {other:?}"),
    }
    fixture.assert_added_not_published().await;
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

/// The host lists the member from inside its completion, while the add
/// holds the bootstrap controller. A roster pass started then reads the
/// listing only after the add settled: it waits, and it reuses the member
/// the add held instead of embodying one unheld.
#[tokio::test(flavor = "multi_thread")]
async fn a_roster_pass_waits_for_a_held_add() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-pass").await;
    let (listed_tx, listed_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let context = Arc::clone(&fixture.context);
    let roster = Arc::clone(&fixture.roster);
    let added = member(&fixture.added);
    let listed = added.clone();
    let adding = tokio::spawn(async move {
        context
            .add_member_held(
                added,
                tokio::time::Instant::now() + HOLD,
                move |_members, _completion| async move {
                    roster.upsert(listed);
                    let _ = listed_tx.send(());
                    let _ = release_rx.await;
                    Ok(())
                },
            )
            .await
    });
    listed_rx
        .await
        .expect("the host lists the member from its completion");
    // A roster pass over the provider's roster, which now lists both
    // members, while the host is still completing.
    let refreshing = {
        let context = Arc::clone(&fixture.context);
        tokio::spawn(async move { context.refresh_desired_topology().await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !refreshing.is_finished(),
        "a roster pass never runs inside a held add"
    );
    let _ = release_tx.send(());
    let result = activated(
        adding
            .await
            .expect("adding task")
            .expect("the held add starts"),
    );
    let Some(RestoreOutcome::Created { binding: held, .. }) = result.outcomes.get(&fixture.added)
    else {
        panic!("the add created the member: {:?}", result.outcomes);
    };
    let refreshed = refreshing
        .await
        .expect("refresh task")
        .expect("the roster pass completes");
    assert!(refreshed.outcomes.contains_key(&fixture.added));
    let status = fixture
        .context
        .runtime
        .status(&fixture.added)
        .await
        .expect("added status");
    assert_eq!(status.state, IdentityLifecycleState::Active, "{status:?}");
    assert_eq!(
        status.session_id.as_ref(),
        held.session_id.as_ref(),
        "the pass kept the member the add held"
    );
    assert_eq!(
        status.agent_runtime_id.as_ref(),
        Some(&held.agent_runtime_id)
    );
    assert_eq!(fixture.members().await, 2);
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

/// A host that listed the member from its completion and then saw the add
/// abort unlists it. A roster pass that reads the listing first still never
/// embodies the member: the aborted member owes a fresh held completion and
/// the pass refuses it.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_listed_by_an_aborted_add_stays_broken_until_unlisted() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-unlist").await;
    let roster = Arc::clone(&fixture.roster);
    let listed = member(&fixture.added);
    let start = fixture
        .context
        .add_member_held(
            member(&fixture.added),
            tokio::time::Instant::now() + HOLD,
            move |_members, _completion| async move {
                roster.upsert(listed);
                Err("refused after listing".to_string())
            },
        )
        .await
        .expect("the held add starts");
    assert!(matches!(
        settled_error(start).await,
        IdentityRuntimeError::HeldBootstrapAborted { .. }
    ));
    // The pass reads the listing before the host unlists the member.
    let refreshed = fixture
        .context
        .refresh_desired_topology()
        .await
        .expect("the roster pass completes");
    assert!(
        matches!(
            refreshed.outcomes.get(&fixture.added),
            Some(RestoreOutcome::Refused(_))
        ),
        "{:?}",
        refreshed.outcomes
    );
    fixture.assert_added_not_published().await;
    assert!(
        fixture.roster.remove(&fixture.added),
        "the host unlists the aborted member"
    );
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_embodied_outside_the_add_refuses_it() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-race").await;
    // Between the add's freshness read and its creation, another path
    // registers and materializes the member, unheld. The competitor is
    // bounded, so the fixture never waits on one an implementation excludes.
    let competitor = Arc::clone(&fixture.context.runtime);
    let competing_spec = member(&fixture.added);
    let competing = fixture.added.clone();
    fixture
        .context
        .runtime
        .set_held_after_freshness_hook(Box::new(move || {
            Box::pin(async move {
                competitor.register(competing_spec).await;
                let _ = tokio::time::timeout(
                    Duration::from_secs(10),
                    competitor.materialize(&competing),
                )
                .await;
            })
        }));
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    let result = fixture
        .context
        .add_member_held(
            member(&fixture.added),
            tokio::time::Instant::now() + HOLD,
            move |_members, _completion| async move {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await;
    if fixture.core_holds_member(&fixture.added).await && asked.load(Ordering::SeqCst) == 0 {
        // The competitor created the member: the add refuses it (the pass's
        // terminal decision is an `Abort`, settled in the tracked operation),
        // never asks the host, and never refuses the valid member.
        let refusal = match result {
            Err(error) => error,
            Ok(start) => settled_error(start).await,
        };
        match refusal {
            IdentityRuntimeError::HeldBootstrapRefused(HeldBootstrapRefusal::NotHeld {
                identity,
            }) => assert_eq!(identity, fixture.added),
            other => panic!("a member embodied outside the add was accepted: {other:?}"),
        }
        assert_eq!(
            fixture.state(&fixture.added).await,
            Some(IdentityLifecycleState::Active)
        );
    } else {
        // An implementation that excluded the competitor held the member.
        activated(result.expect("the held add starts"));
    }
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

/// Freshness is Core's member history, as for a held roster bootstrap: an
/// identity Core spawned is not added again, whether its member is live or
/// retired.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_core_spawned_is_not_added_again() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-again").await;
    activated(
        fixture
            .add(tokio::time::Instant::now() + HOLD, None)
            .await
            .expect("the held add starts"),
    );
    assert_eq!(
        fixture.state(&fixture.added).await,
        Some(IdentityLifecycleState::Active)
    );
    let refused_live =
        |result: Result<HeldBootstrapStart, IdentityRuntimeError>, what: &str| match result {
            Err(IdentityRuntimeError::HeldBootstrapRefused(
                HeldBootstrapRefusal::ExistingContinuity { identity },
            )) => assert_eq!(identity, fixture.added, "{what}"),
            other => panic!("{what}: an identity Core spawned is added again: {other:?}"),
        };
    refused_live(
        fixture.add(tokio::time::Instant::now() + HOLD, None).await,
        "live member",
    );
    // Retired, the identity keeps its Core history: still not fresh.
    fixture
        .context
        .runtime
        .retire_tracked(&fixture.added)
        .await
        .expect("the added member retires");
    refused_live(
        fixture.add(tokio::time::Instant::now() + HOLD, None).await,
        "retired member",
    );
    assert_eq!(fixture.members().await, 1);
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

/// An identity Core holds an identity intent row for is the host's: the
/// held add refuses it typed, never asks the host, and moves nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_adopted_identity_is_not_added() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-adopted").await;
    fixture.adopt_running().await;
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    match fixture
        .context
        .add_member_held(
            member(&fixture.running),
            tokio::time::Instant::now() + HOLD,
            move |_members, _completion| async move {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
    {
        Err(IdentityRuntimeError::IdentityHostAdopted { identity }) => {
            assert_eq!(identity, fixture.running);
        }
        other => panic!("a host-adopted identity is added: {other:?}"),
    }
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the host was never asked");
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_added_member_retires_and_settles() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-retire").await;
    activated(
        fixture
            .add(tokio::time::Instant::now() + HOLD, None)
            .await
            .expect("the held add starts"),
    );
    fixture
        .context
        .runtime
        .retire_tracked(&fixture.added)
        .await
        .expect("the added member retires");
    assert_eq!(
        fixture.state(&fixture.added).await,
        Some(IdentityLifecycleState::Dormant)
    );
    assert_eq!(
        fixture.members().await,
        1,
        "the added member left meerkat's roster"
    );
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_add_refuses_a_member_it_cannot_hold() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-initial").await;
    let mut spec = member(&fixture.added);
    spec.initial_message = Some("hello".into());
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    match fixture
        .context
        .add_member_held(
            spec,
            tokio::time::Instant::now() + HOLD,
            move |_members, _completion| async move {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
    {
        Err(IdentityRuntimeError::HeldBootstrapRefused(HeldBootstrapRefusal::InitialMessage {
            identity,
        })) => assert_eq!(identity, fixture.added),
        other => panic!("a member with an initial message is held: {other:?}"),
    }
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the host was never asked");
    assert_eq!(fixture.members().await, 1);
    fixture.assert_running_untouched().await;
    let _ = fixture.runtime.shutdown().await;
}

/// In lazy mode, too, the add embodies its member at once, held, while the
/// rest of the lazy roster stays dormant. The add publishes no bootstrap
/// generation: the bootstrap status does not list the added member.
#[tokio::test(flavor = "multi_thread")]
async fn a_lazy_roster_adds_its_member_held_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let continuity = Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
    let running = AgentIdentity::parse("held-add-lazy-running").unwrap();
    let added = AgentIdentity::parse("held-add-lazy-added").unwrap();
    let mut runtime = governed_runtime(&dir, "held-add-lazy").await;
    let identity_runtime = identity_runtime(
        &runtime,
        Arc::clone(&continuity) as Arc<dyn ContinuityStore>,
        "held-add-lazy",
    );
    let roster = Arc::new(MutableRosterProvider::new(vec![member(&running)]));
    let context = Arc::new(IdentityFirstRuntimeContext::new_with_lazy_materialization(
        identity_runtime,
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        None,
        Some(runtime.mob_handle().definition().clone()),
        true,
    ));
    runtime.install_identity_first_context(Arc::clone(&context));
    let restored = runtime
        .bootstrap_installed_identity_first_context(&[member(&running)])
        .await
        .expect("bootstrap the lazy roster");
    assert!(
        matches!(
            restored.outcomes.get(&running),
            Some(RestoreOutcome::Dormant { .. })
        ),
        "{:?}",
        restored.outcomes
    );

    let listing = Arc::clone(&roster);
    let listed = member(&added);
    let result = activated(
        context
            .add_member_held(
                member(&added),
                tokio::time::Instant::now() + HOLD,
                move |_members, _completion| async move {
                    listing.upsert(listed);
                    Ok(())
                },
            )
            .await
            .expect("the held add starts"),
    );
    assert!(
        matches!(
            result.outcomes.get(&added),
            Some(RestoreOutcome::Created { .. })
        ),
        "{:?}",
        result.outcomes
    );
    let added_status = context.runtime.status(&added).await.expect("added status");
    assert_eq!(added_status.state, IdentityLifecycleState::Active);
    let running_status = context
        .runtime
        .status(&running)
        .await
        .expect("running status");
    assert_eq!(running_status.state, IdentityLifecycleState::Dormant);
    assert_eq!(
        runtime.mob_handle().list_members().await.len(),
        1,
        "only the added member is embodied"
    );
    let bootstrap = context.runtime.identity_bootstrap_status();
    assert!(bootstrap.identities.contains_key(&running), "{bootstrap:?}");
    assert!(!bootstrap.identities.contains_key(&added), "{bootstrap:?}");
    let _ = runtime.shutdown().await;
}

/// A graceful shutdown's in-flight drain joins identity foreground
/// operations before it stops the mob, bounded by its window. An add whose
/// host is still completing holds the bootstrap controller past that
/// bound, so the drain is cut there, before its Stop: the mob keeps running,
/// and the add settles in the shutdown that follows.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_add_still_completing_cuts_the_in_flight_drain_before_its_stop() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-drain").await;
    let (asked_tx, asked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let context = Arc::clone(&fixture.context);
    let added = member(&fixture.added);
    let adding = tokio::spawn(async move {
        context
            .add_member_held(
                added,
                tokio::time::Instant::now() + HOLD,
                move |_members, _completion| async move {
                    let _ = asked_tx.send(());
                    let _ = release_rx.await;
                    Ok(())
                },
            )
            .await
    });
    asked_rx.await.expect("the host is asked to complete");

    let window = Duration::from_millis(500);
    let report = tokio::time::timeout(
        window + Duration::from_secs(10),
        fixture
            .runtime
            .drain_in_flight_turns(std::time::Instant::now() + window),
    )
    .await
    .expect("the drain ends at its bound, not when the add does");
    assert_eq!(
        report.outcome,
        crate::InFlightDrainOutcome::IdentityForegroundBusy,
        "{report:?}"
    );
    assert!(
        report.waited >= window && report.waited < window + Duration::from_secs(5),
        "the drain is cut at its bound: {report:?}"
    );
    assert_eq!(
        fixture
            .runtime
            .mob_handle()
            .status()
            .await
            .expect("mob status"),
        meerkat_mob::MobState::Running,
        "the cut drain never stopped the mob"
    );

    let _ = release_tx.send(());
    let _ = fixture.runtime.shutdown().await;
    let added = tokio::time::timeout(Duration::from_secs(30), adding)
        .await
        .expect("the add settles in the shutdown")
        .expect("adding task");
    if let Ok(HeldBootstrapStart::Unsettled(settlement)) = added {
        tokio::time::timeout(Duration::from_secs(30), settlement.settled())
            .await
            .expect("the add's settlement ends in the shutdown")
            .ok();
    }
}

/// A shutdown closes the cross-mob control listener before it stops the
/// mob, as the in-flight drain does. An add whose host is still completing
/// holds the shutdown before its Stop: the mob is still running there, and
/// the listener already refuses connections, so no late wire or inject
/// reaches a mob that is about to stop.
#[tokio::test(flavor = "multi_thread")]
async fn a_shutdown_closes_cross_mob_control_before_it_stops_the_mob() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "held-add-control").await;
    let address = fixture
        .runtime
        .start_control_listener(
            &crate::runtime::cross_mob_control::ControlListenAddr::parse("tcp://127.0.0.1:0")
                .expect("a control address"),
        )
        .await
        .expect("the control listener binds");
    let socket = address
        .strip_prefix("tcp://")
        .expect("a tcp listener")
        .to_string();
    tokio::net::TcpStream::connect(&socket)
        .await
        .expect("the listener accepts connections before the shutdown");

    let (asked_tx, asked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let context = Arc::clone(&fixture.context);
    let added = member(&fixture.added);
    let adding = tokio::spawn(async move {
        context
            .add_member_held(
                added,
                tokio::time::Instant::now() + HOLD,
                move |_members, _completion| async move {
                    let _ = asked_tx.send(());
                    let _ = release_rx.await;
                    Ok(())
                },
            )
            .await
    });
    asked_rx.await.expect("the host is asked to complete");

    let runtime = &fixture.runtime;
    let socket = socket.as_str();
    tokio::join!(
        async {
            let _ = runtime.shutdown().await;
        },
        async move {
            tokio::time::timeout(Duration::from_secs(30), async {
                while tokio::net::TcpStream::connect(socket).await.is_ok() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the shutdown closes the control listener");
            assert_eq!(
                runtime.mob_handle().status().await.expect("mob status"),
                meerkat_mob::MobState::Running,
                "the listener closed while the add holds the shutdown, before its Stop"
            );
            let _ = release_tx.send(());
        }
    );
    let added = tokio::time::timeout(Duration::from_secs(30), adding)
        .await
        .expect("the add settles in the shutdown")
        .expect("adding task");
    if let Ok(HeldBootstrapStart::Unsettled(settlement)) = added {
        tokio::time::timeout(Duration::from_secs(30), settlement.settled())
            .await
            .expect("the add's settlement ends in the shutdown")
            .ok();
    }
}
