//! The gateway host's retirement hook
//! (`GatewayHostComposition::identity_member_retired`, the identity
//! runtime's crate-private retired-member observer): which retires reach it,
//! with which incarnation, registration and cause, and with no Meta lock
//! held.
//!
//! An explicit or scoped retire releases the member: the identity stays a
//! member of Core's mob, active on the same session at the same generation,
//! and its next delivery materializes it again under a new registration.
//! Only a roster pass that no longer lists the identity ends its incarnation,
//! as `Unlisted`. A roster pass hands its retirements to the hook after the
//! bootstrap controller is released, also when a later step of the pass
//! fails, so a hook may refresh the roster itself. Replacements, reprofiles,
//! held aborts and shutdowns never reach it, and a hook error leaves the
//! retirement standing and reaches the error hook once.
//!
//! Each call names the exact Core registration the retire or release ended,
//! read before it. A registration still current after Core answered is not
//! named, and neither is one when the release ended none; a later
//! registration, even one on the same session before the hook returns, never
//! is the named one.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::identity_first::bridge::{
    BridgeDelivery, BridgeError, BridgeMemberReload, MemberRuntimeBinding, ResumeSessionOutcome,
    SessionBridge,
};
use crate::identity_first::core_test_bridge::{CoreReadsBridge, register_live};
use crate::identity_first::core_test_members::CoreTestMembers;
use crate::identity_first::runtime::RetiredMemberObserver;
use crate::identity_first::{
    AgentBuildDraft, AgentIdentity, AgentRuntimeId, ContinuityStore, DurabilityPolicy,
    DurableAgentSpec, HeldBootstrapStart, IdentityFirstRuntimeContext, IdentityLifecycleState,
    IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider,
    MutableRosterProvider, RetiredIdentityCause, RetiredIdentityMember, RosterProvider,
    ScopedRetireError, SessionSnapshot,
};
use crate::unified_runtime::types::ErrorEvent;

use meerkat_mob::MemberRuntimeRegistrationHook;
use meerkat_runtime::RuntimeSessionRegistrationWitness;

use super::held_identity_test_support::{SelectingHook, governed_runtime_with, member};

/// An identity runtime over `runtime`'s mob, its session bridge composed as
/// a gateway composes it: with the mob runtime's session service, so it
/// reads Core's registrations.
fn gateway_identity_runtime(
    runtime: &crate::UnifiedRuntime,
    continuity: Arc<dyn ContinuityStore>,
    instance: &str,
) -> Arc<IdentityRuntime> {
    let service = runtime
        .mob_runtime()
        .session_service()
        .cloned()
        .expect("a bootstrapped runtime has a session service");
    let bridge = crate::identity_first::MobSessionBridge::with_session_service(
        runtime.mob_handle(),
        service,
    );
    Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: continuity,
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: instance.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(Arc::new(bridge)),
        default_timeout: None,
    }))
}

/// The setup deadline of the fixture's held passes.
const HOLD: Duration = Duration::from_mins(1);

/// What the hook saw, in order.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<RetiredIdentityMember>>>);

impl Seen {
    fn calls(&self) -> Vec<RetiredIdentityMember> {
        self.0.lock().unwrap().clone()
    }
}

/// A hook that records each retired incarnation and answers `answer`.
fn recording(seen: &Seen, answer: Result<(), String>) -> RetiredMemberObserver {
    let seen = seen.clone();
    Arc::new(move |retired| {
        seen.0.lock().unwrap().push(retired);
        let answer = answer.clone();
        Box::pin(async move { answer })
    })
}

/// The host's Core registration hook in the fixture: it selects as the
/// governed host does ([`SelectingHook`]) and records each distinct
/// registration a member build was given
/// (`MemberRuntimeBuildIdentity::registration`), in order.
#[derive(Default)]
struct RecordingRegistrationHook {
    builds: Mutex<Vec<RuntimeSessionRegistrationWitness>>,
}

impl RecordingRegistrationHook {
    fn record(&self, identity: &meerkat_mob::MemberRuntimeBuildIdentity) {
        let mut builds = self.builds.lock().unwrap();
        if !builds.contains(identity.registration()) {
            builds.push(identity.registration().clone());
        }
    }

    /// The distinct registrations builds on `session` were given, in order.
    fn registrations_on(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> Vec<RuntimeSessionRegistrationWitness> {
        self.builds
            .lock()
            .unwrap()
            .iter()
            .filter(|registration| registration.session_id() == session)
            .cloned()
            .collect()
    }
}

#[async_trait::async_trait]
impl MemberRuntimeRegistrationHook for RecordingRegistrationHook {
    async fn select_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError> {
        self.record(identity);
        SelectingHook.select_destination(identity).await
    }

    async fn expected_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError> {
        self.record(identity);
        SelectingHook.expected_destination(identity).await
    }

    async fn verify_member_surface(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        surface: &[meerkat_core::ToolDef],
    ) -> Result<(), meerkat_mob::MobError> {
        SelectingHook.verify_member_surface(identity, surface).await
    }

    async fn prepare_member_surface(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, meerkat_mob::MobError> {
        SelectingHook
            .prepare_member_surface(identity, surface)
            .await
    }
}

/// A running roster of two members, bootstrapped held, with the recording
/// hook installed before the bootstrap. Its mob also declares a `reviewer`
/// profile, and its members register through the recording registration
/// hook.
struct Fixture {
    runtime: crate::UnifiedRuntime,
    context: Arc<IdentityFirstRuntimeContext>,
    roster: Arc<MutableRosterProvider>,
    kept: AgentIdentity,
    other: AgentIdentity,
    seen: Seen,
    builds: Arc<RecordingRegistrationHook>,
}

impl Fixture {
    async fn start(dir: &tempfile::TempDir, prefix: &str) -> Self {
        let continuity = Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
        let kept = AgentIdentity::parse(&format!("{prefix}-kept")).unwrap();
        let other = AgentIdentity::parse(&format!("{prefix}-other")).unwrap();
        let builds = Arc::new(RecordingRegistrationHook::default());
        let mut runtime = governed_runtime_with(
            dir,
            prefix,
            &["reviewer"],
            Arc::clone(&builds) as Arc<dyn MemberRuntimeRegistrationHook>,
        )
        .await;
        let identities = gateway_identity_runtime(
            &runtime,
            Arc::clone(&continuity) as Arc<dyn ContinuityStore>,
            prefix,
        );
        let seen = Seen::default();
        identities.set_retired_member_observer(Some(recording(&seen, Ok(()))));
        let roster = Arc::new(MutableRosterProvider::new(vec![
            member(&kept),
            member(&other),
        ]));
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            identities,
            Arc::clone(&roster) as Arc<dyn RosterProvider>,
            None,
            None,
            Some(runtime.mob_handle().definition().clone()),
        ));
        runtime.install_identity_first_context(Arc::clone(&context));
        runtime
            .bootstrap_installed_identity_first_context_held(
                &[member(&kept), member(&other)],
                tokio::time::Instant::now() + HOLD,
                Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
            )
            .await
            .expect("bootstrap the roster");
        assert!(seen.calls().is_empty(), "bootstrapping retired nothing");
        Self {
            runtime,
            context,
            roster,
            kept,
            other,
            seen,
            builds,
        }
    }

    fn identities(&self) -> &Arc<IdentityRuntime> {
        &self.context.runtime
    }

    async fn status(&self, identity: &AgentIdentity) -> crate::identity_first::IdentityStatus {
        self.identities()
            .status(identity)
            .await
            .expect("identity status")
    }
}

/// An explicit retire releases the member and reaches the hook once, cause
/// `Retire`, with the incarnation it released (its generation unchanged) and
/// with the identity's lifecycle lock released. The identity stays active on
/// the same session at the same generation. A second retire releases
/// nothing (the member is not materialized) and reaches the hook naming no
/// registration.
#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_retire_releases_the_member_and_reaches_the_hook() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-explicit").await;
    let before = fixture.status(&fixture.kept).await;
    assert_eq!(before.state, IdentityLifecycleState::Active);

    let lock_free = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&lock_free);
    let identities = Arc::downgrade(fixture.identities());
    let kept = fixture.kept.clone();
    let seen = fixture.seen.clone();
    let observer: RetiredMemberObserver = Arc::new(move |retired| {
        seen.0.lock().unwrap().push(retired);
        let observed = Arc::clone(&observed);
        let identities = identities.clone();
        let kept = kept.clone();
        Box::pin(async move {
            let identities = identities.upgrade().expect("the identity runtime");
            let free = identities
                .lifecycle_lock_for(&kept)
                .await
                .try_lock()
                .is_ok();
            *observed.lock().unwrap() = Some(free);
            Ok(())
        })
    });
    fixture
        .identities()
        .set_retired_member_observer(Some(observer));

    fixture
        .identities()
        .retire_tracked(&fixture.kept)
        .await
        .expect("the member retires");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let retired = &calls[0];
    assert_eq!(retired.identity, fixture.kept);
    assert_eq!(retired.cause, RetiredIdentityCause::Retire);
    assert_eq!(Some(&retired.member), before.member_id.as_ref());
    assert_eq!(retired.role, meerkat_mob::ProfileName::from("worker"));
    assert_eq!(
        Some(&retired.agent_runtime_id),
        before.agent_runtime_id.as_ref()
    );
    assert_eq!(retired.session_id, before.session_id);
    assert_eq!(
        Some(retired.generation),
        before.generation.map(meerkat_mob::Generation::get)
    );
    assert_eq!(
        *lock_free.lock().unwrap(),
        Some(true),
        "the hook runs with the identity's lifecycle lock released"
    );
    let after = fixture.status(&fixture.kept).await;
    assert_eq!(after.state, IdentityLifecycleState::Active, "{after:?}");
    assert_eq!(after.session_id, before.session_id, "the same session");
    assert_eq!(after.generation, before.generation, "the same generation");

    fixture
        .identities()
        .retire_tracked(&fixture.kept)
        .await
        .expect("a released member has nothing to release");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[1].cause, RetiredIdentityCause::Retire);
    assert_eq!(calls[1].generation, calls[0].generation);
    assert_eq!(
        calls[1].registration, None,
        "a release that ended no registration names none: {calls:?}"
    );
    let _ = fixture.runtime.shutdown().await;
}

/// A released identity the roster still lists stays a member through the
/// next roster pass, on the same session at the same generation, and the
/// hook saw only the one `Retire`: the pass retires nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_released_listed_identity_keeps_its_session_and_generation() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-return").await;
    let before = fixture.status(&fixture.kept).await;
    fixture
        .identities()
        .retire_tracked(&fixture.kept)
        .await
        .expect("the member is released");
    fixture
        .context
        .refresh_desired_topology_tracked()
        .await
        .expect("the roster pass runs");
    let after = fixture.status(&fixture.kept).await;
    assert_eq!(after.state, IdentityLifecycleState::Active, "{after:?}");
    assert_eq!(after.session_id, before.session_id, "the same session");
    assert_eq!(after.generation, before.generation, "the same generation");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].cause, RetiredIdentityCause::Retire);
    let _ = fixture.runtime.shutdown().await;
}

/// A roster pass that no longer lists an identity retires it as
/// `Unlisted`, once; a repeat pass retires and calls nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_unlisted_identity_reaches_the_hook_once() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-unlisted").await;
    let before = fixture.status(&fixture.other).await;
    assert!(fixture.roster.remove(&fixture.other));
    fixture
        .context
        .refresh_desired_topology_tracked()
        .await
        .expect("the roster pass runs");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].identity, fixture.other);
    assert_eq!(calls[0].cause, RetiredIdentityCause::Unlisted);
    assert_eq!(calls[0].session_id, before.session_id);
    fixture
        .context
        .refresh_desired_topology_tracked()
        .await
        .expect("the repeat pass runs");
    assert_eq!(fixture.seen.calls().len(), 1, "a repeat pass calls nothing");
    assert_eq!(
        fixture.status(&fixture.kept).await.state,
        IdentityLifecycleState::Active,
        "the listed member is untouched"
    );
    let _ = fixture.runtime.shutdown().await;
}

/// A boot pass that finds a live member Meta spawned for an identity the
/// roster no longer lists, which this runtime never registered, retires it
/// as `Unlisted` and hands it to the hook once, after the pass's bootstrap
/// controller is released.
#[tokio::test(flavor = "multi_thread")]
async fn a_boot_pass_hands_its_unlisted_retirement_to_the_hook() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-boot").await;
    let continuity = Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
    let rebooted = gateway_identity_runtime(
        &fixture.runtime,
        continuity as Arc<dyn ContinuityStore>,
        "retired-hook-boot-second",
    );
    let seen = Seen::default();
    let controller_free = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&controller_free);
    let probe = Arc::downgrade(&rebooted);
    let recorded = seen.clone();
    let observer: RetiredMemberObserver = Arc::new(move |retired| {
        recorded.0.lock().unwrap().push(retired);
        let observed = Arc::clone(&observed);
        let probe = probe.clone();
        Box::pin(async move {
            let runtime = probe.upgrade().expect("the identity runtime");
            *observed.lock().unwrap() = Some(runtime.bootstrap_controller_is_free());
            Ok(())
        })
    });
    rebooted.set_retired_member_observer(Some(observer));
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&rebooted),
        Arc::new(MutableRosterProvider::new(vec![member(&fixture.kept)])),
        None,
        None,
        Some(fixture.runtime.mob_handle().definition().clone()),
    );
    context
        .bootstrap_roster(&[member(&fixture.kept)])
        .await
        .expect("the boot pass runs");
    let calls = seen.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].identity, fixture.other);
    assert_eq!(calls[0].cause, RetiredIdentityCause::Unlisted);
    assert_eq!(
        *controller_free.lock().unwrap(),
        Some(true),
        "the hook runs after the bootstrap controller is released"
    );
    let _ = fixture.runtime.shutdown().await;
}

/// A hook that refreshes the roster from inside itself completes: no Meta
/// lock is held while it runs, and both passes finish.
#[tokio::test(flavor = "multi_thread")]
async fn a_hook_that_refreshes_the_roster_does_not_deadlock() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-reentrant").await;
    let context = Arc::downgrade(&fixture.context);
    let inner = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&inner);
    let seen = fixture.seen.clone();
    let observer: RetiredMemberObserver = Arc::new(move |retired| {
        seen.0.lock().unwrap().push(retired);
        let context = context.clone();
        let observed = Arc::clone(&observed);
        Box::pin(async move {
            let context = context.upgrade().expect("the identity context");
            let refreshed = context.refresh_desired_topology_tracked().await;
            *observed.lock().unwrap() = Some(refreshed.is_ok());
            Ok(())
        })
    });
    fixture
        .identities()
        .set_retired_member_observer(Some(observer));
    assert!(fixture.roster.remove(&fixture.other));
    tokio::time::timeout(
        Duration::from_mins(2),
        fixture.context.refresh_desired_topology_tracked(),
    )
    .await
    .expect("the outer pass does not deadlock")
    .expect("the outer pass runs");
    assert_eq!(*inner.lock().unwrap(), Some(true), "the inner pass ran");
    assert_eq!(fixture.seen.calls().len(), 1);
    let _ = fixture.runtime.shutdown().await;
}

/// Replacements, a pass that lists every live member and a shutdown never
/// reach the hook: each keeps or replaces the identity's member, and none
/// ends it.
#[tokio::test(flavor = "multi_thread")]
async fn replacements_listed_passes_and_shutdown_never_reach_the_hook() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-never").await;
    fixture
        .identities()
        .reload_member(&fixture.kept)
        .await
        .expect("the member reloads");
    fixture
        .identities()
        .respawn_tracked(&fixture.kept)
        .await
        .expect("the member respawns");
    fixture
        .identities()
        .reset_tracked(&fixture.other)
        .await
        .expect("the member resets");
    fixture
        .context
        .refresh_desired_topology_tracked()
        .await
        .expect("a pass that lists every member runs");
    assert!(
        fixture.seen.calls().is_empty(),
        "{:?}",
        fixture.seen.calls()
    );
    let _ = fixture.runtime.shutdown().await;
    assert!(fixture.seen.calls().is_empty(), "a shutdown calls nothing");
}

/// A held add the host refuses aborts its member before publication: no
/// incarnation of it was ever retired, so the hook is never called.
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_held_add_never_reaches_the_hook() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-held").await;
    let added = AgentIdentity::parse("retired-hook-held-added").unwrap();
    match fixture
        .context
        .add_member_held(
            member(&added),
            tokio::time::Instant::now() + HOLD,
            |_members, _completion| async { Err("the application refused".to_string()) },
        )
        .await
        .expect("the held add starts")
    {
        HeldBootstrapStart::Unsettled(settlement) => {
            settlement
                .settled()
                .await
                .expect_err("the host's refusal aborts the add");
        }
        HeldBootstrapStart::Activated(result) => {
            panic!("the refused add activated: {:?}", result.outcomes)
        }
    }
    assert!(
        fixture.seen.calls().is_empty(),
        "{:?}",
        fixture.seen.calls()
    );
    let _ = fixture.runtime.shutdown().await;
}

/// A hook error leaves the retirement standing: the retire answers `Ok`,
/// Core's member is released (the identity stays active), the error reaches
/// the error hook once as `RetiredMemberReleaseFailed`, and nothing is
/// retried.
#[tokio::test(flavor = "multi_thread")]
async fn a_hook_error_is_reported_apart_from_the_retirement() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-failure").await;
    let seen = Seen::default();
    fixture
        .identities()
        .set_retired_member_observer(Some(recording(&seen, Err("store busy".to_string()))));
    let reported = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&reported);
    let hook: crate::unified_runtime::ErrorHook = Arc::new(move |event| {
        sink.lock().unwrap().push(event);
        Box::pin(async {})
    });
    fixture.identities().set_error_hook(Some(hook));
    let before = fixture.status(&fixture.kept).await;
    fixture
        .identities()
        .retire_tracked(&fixture.kept)
        .await
        .expect("the retirement stands");
    assert_eq!(
        fixture.status(&fixture.kept).await.state,
        IdentityLifecycleState::Active,
        "a released member stays seated"
    );
    assert_eq!(seen.calls().len(), 1, "called once, never retried");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !reported.lock().unwrap().is_empty() || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        *reported.lock().unwrap(),
        vec![ErrorEvent::RetiredMemberReleaseFailed {
            identity: fixture.kept.to_string(),
            agent_runtime_id: before
                .agent_runtime_id
                .as_ref()
                .expect("a live runtime id")
                .to_string(),
            session_id: before.session_id.as_ref().map(ToString::to_string),
            cause: "retire".to_string(),
            error: "store busy".to_string(),
        }]
    );
    let _ = fixture.runtime.shutdown().await;
}

/// A scoped retire of the live member releases it and reaches the hook,
/// cause `Scoped`, while its scope names the current incarnation. A release
/// keeps the incarnation, so the same scope stays current: a second scoped
/// retire releases nothing and reaches the hook naming no registration. Once
/// a reset rotates the identity to its next incarnation the old scope is
/// stale, and neither the reset nor the stale scope calls the hook.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_retire_reaches_the_hook_while_its_scope_names_the_current_incarnation() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-scoped").await;
    let scope = fixture
        .identities()
        .capture_delivery_scope(&fixture.kept)
        .await
        .expect("the live scope");
    fixture
        .identities()
        .retire_at_scope_tracked(&fixture.kept, &scope)
        .await
        .expect("the scoped retire");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].cause, RetiredIdentityCause::Scoped);

    fixture
        .identities()
        .retire_at_scope_tracked(&fixture.kept, &scope)
        .await
        .expect("the scope still names the current incarnation");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[1].cause, RetiredIdentityCause::Scoped);
    assert_eq!(
        calls[1].registration, None,
        "a release that ended no registration names none: {calls:?}"
    );

    fixture
        .identities()
        .reset_tracked(&fixture.kept)
        .await
        .expect("the reset rotates the identity to its next incarnation");
    match fixture
        .identities()
        .retire_at_scope_tracked(&fixture.kept, &scope)
        .await
    {
        Err(ScopedRetireError::StaleScope { .. }) => {}
        other => panic!("an old scope is stale once the identity rotated: {other:?}"),
    }
    assert_eq!(
        fixture.seen.calls().len(),
        2,
        "neither the reset nor a stale scope calls the hook"
    );
    let _ = fixture.runtime.shutdown().await;
}

/// A retirement the roster pass made before a later step of the same pass
/// failed still reaches the hook, and the pass's error still reaches its
/// caller.
#[tokio::test]
async fn a_retirement_before_a_failed_step_still_reaches_the_hook() {
    let bridge = Arc::new(CoreReadsBridge::default());
    let identities = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "retired-hook-partial".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge.clone()),
        default_timeout: None,
    }));
    let seen = Seen::default();
    identities.set_retired_member_observer(Some(recording(&seen, Ok(()))));
    let gone = AgentIdentity::parse("retired-hook-partial-gone").unwrap();
    register_live(
        &identities,
        &bridge.core,
        member(&gone),
        1,
        meerkat_core::types::SessionId::new(),
    )
    .await
    .expect("a live registered member");
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&identities),
        Arc::new(MutableRosterProvider::new(Vec::new())),
        None,
        None,
        None,
    );
    // The pass retires the unlisted member, then fails reading Core's
    // durable members: this bridge does not answer that read.
    context
        .refresh_desired_topology()
        .await
        .expect_err("the pass fails after its first retire");
    let calls = seen.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].identity, gone);
    assert_eq!(calls[0].cause, RetiredIdentityCause::Unlisted);
    assert_eq!(calls[0].generation, 1);
}

/// The runtime machine that holds the fixture's member registrations.
fn fixture_machine(fixture: &Fixture) -> Arc<meerkat_runtime::MeerkatMachine> {
    crate::mob_handle_runtime::observed_runtime_adapter(
        fixture
            .runtime
            .mob_runtime()
            .session_service()
            .expect("a session service")
            .as_ref(),
    )
    .expect("the runtime machine")
}

/// A retire hands the hook the exact Core registration its release ended:
/// the machine's registration of the member's session right before the
/// retire. Once the hook runs, that registration is no longer current.
#[tokio::test(flavor = "multi_thread")]
async fn the_retired_member_carries_the_registration_it_held() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-registration").await;
    let machine = fixture_machine(&fixture);
    let session = fixture
        .status(&fixture.kept)
        .await
        .session_id
        .expect("the member's session");
    let held = machine
        .current_session_registration_witness(&session)
        .await
        .expect("the member's registration");
    fixture
        .identities()
        .retire_tracked(&fixture.kept)
        .await
        .expect("the member retires");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].registration.as_ref(), Some(&held), "{calls:?}");
    assert!(
        machine.session_registration_use(&held).await.is_none(),
        "the retired registration is no longer current"
    );
    let _ = fixture.runtime.shutdown().await;
}

/// A later registration made before the hook returns (here the hook's own
/// materialization of the released identity, on the same session at the
/// same generation) is another registration than the one the hook was
/// handed: a host that releases what it bound to that registration never
/// releases the later one.
#[tokio::test(flavor = "multi_thread")]
async fn a_successor_registered_before_the_hook_returns_holds_another_registration() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-successor").await;
    let machine = fixture_machine(&fixture);
    let session = fixture
        .status(&fixture.kept)
        .await
        .session_id
        .expect("the member's session");
    let successor = Arc::new(Mutex::new(None));
    let seen = fixture.seen.clone();
    let identities = Arc::downgrade(fixture.identities());
    let kept = fixture.kept.clone();
    let observed = Arc::clone(&successor);
    let hook_machine = Arc::clone(&machine);
    let hook_session = session.clone();
    let observer: RetiredMemberObserver = Arc::new(move |retired| {
        seen.0.lock().unwrap().push(retired);
        let identities = identities.clone();
        let kept = kept.clone();
        let observed = Arc::clone(&observed);
        let machine = Arc::clone(&hook_machine);
        let session = hook_session.clone();
        Box::pin(async move {
            let identities = identities
                .upgrade()
                .ok_or_else(|| "the identity runtime is gone".to_string())?;
            identities
                .materialize(&kept)
                .await
                .map_err(|error| error.to_string())?;
            let current = machine.current_session_registration_witness(&session).await;
            *observed.lock().unwrap() = current;
            Ok(())
        })
    });
    fixture
        .identities()
        .set_retired_member_observer(Some(observer));
    fixture
        .identities()
        .retire_tracked(&fixture.kept)
        .await
        .expect("the member retires");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let retired = calls[0]
        .registration
        .clone()
        .expect("the retired registration");
    let successor = successor
        .lock()
        .unwrap()
        .clone()
        .expect("the hook's materialization registered the same session again");
    assert_eq!(
        successor.session_id(),
        retired.session_id(),
        "the same session"
    );
    assert_ne!(successor, retired, "another registration");
    assert!(machine.session_registration_use(&retired).await.is_none());
    assert!(machine.session_registration_use(&successor).await.is_some());
    let after = fixture.status(&fixture.kept).await;
    assert_eq!(after.state, IdentityLifecycleState::Active, "{after:?}");
    let _ = fixture.runtime.shutdown().await;
}

/// A Core-reads bridge whose members hold real registrations on `machine`:
/// it reads them, and its retire or release ends the member's registration
/// only when `ends_registration`.
struct MachineReadsBridge {
    reads: CoreReadsBridge,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    ends_registration: bool,
}

#[async_trait::async_trait]
impl SessionBridge for MachineReadsBridge {
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.reads.create_session(identity, spec, draft).await
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        self.reads
            .resume_session(identity, spec, draft, session_id, resume_from_role)
            .await
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.reads.live_member(identity).await
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.reads.live_member_scope(identity).await
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.reads.ensure_member_live(identity).await
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.reads.member_binding_history().await
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.reads.last_member_binding(identity).await
    }

    async fn identity_management(
        &self,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        self.reads.identity_management(identity).await
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.reads.deliver_admitted(runtime_id, delivery).await
    }

    async fn checkpoint_session(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        self.reads.checkpoint_session(runtime_id, session_id).await
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        let session = self
            .reads
            .core
            .live_incarnation(runtime_id)
            .and_then(|(session, _)| session);
        self.reads.retire_member(runtime_id).await?;
        if self.ends_registration
            && let Some(session) = session
        {
            self.machine
                .unregister_session(&session)
                .await
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
        }
        Ok(())
    }

    async fn release_member(&self, identity: &AgentIdentity) -> Result<(), BridgeError> {
        let session = self
            .reads
            .core
            .live_member(identity)?
            .and_then(|binding| binding.session_id);
        self.reads.release_member(identity).await?;
        if self.ends_registration
            && let Some(session) = session
        {
            self.machine
                .unregister_session(&session)
                .await
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
        }
        Ok(())
    }

    async fn reload_member_registration(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<Option<BridgeMemberReload>, BridgeError> {
        self.reads.reload_member_registration(runtime_id).await
    }

    async fn member_registration(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<meerkat_runtime::RuntimeSessionRegistrationWitness> {
        self.machine
            .current_session_registration_witness(session_id)
            .await
    }
}

/// Retire (release) `prefix`'s one live registered member, bound to
/// `session`, through `bridge` (whose Core member reads are `core`) with the
/// recording hook installed, and answer what the hook saw.
async fn retire_one_through(
    bridge: Arc<dyn SessionBridge>,
    core: &CoreTestMembers,
    prefix: &str,
    session: meerkat_core::types::SessionId,
) -> Vec<RetiredIdentityMember> {
    let identities = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: prefix.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    }));
    let seen = Seen::default();
    identities.set_retired_member_observer(Some(recording(&seen, Ok(()))));
    let identity = AgentIdentity::parse(&format!("{prefix}-member")).unwrap();
    register_live(&identities, core, member(&identity), 1, session)
        .await
        .expect("a live registered member");
    identities
        .retire_tracked(&identity)
        .await
        .expect("the member is released");
    seen.calls()
}

/// The hook names only a registration Core's release ended. A registration
/// still current after the release answered was not ended by it and is not
/// named; neither is any registration when the bridge observes none.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_registration_the_retire_ended_is_named() {
    for (prefix, ends_registration) in [
        ("retired-hook-ended", true),
        ("retired-hook-still-current", false),
    ] {
        let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let bridge = Arc::new(MachineReadsBridge {
            reads: CoreReadsBridge::default(),
            machine: Arc::clone(&machine),
            ends_registration,
        });
        let session = meerkat_core::types::SessionId::new();
        machine
            .register_session(session.clone())
            .await
            .expect("the session registers");
        let held = machine
            .current_session_registration_witness(&session)
            .await
            .expect("the session's registration");
        let calls = retire_one_through(
            Arc::clone(&bridge) as Arc<dyn SessionBridge>,
            &bridge.reads.core,
            prefix,
            session,
        )
        .await;
        assert_eq!(calls.len(), 1, "{prefix}: {calls:?}");
        if ends_registration {
            assert_eq!(
                calls[0].registration.as_ref(),
                Some(&held),
                "{prefix}: {calls:?}"
            );
        } else {
            assert!(
                machine.session_registration_use(&held).await.is_some(),
                "{prefix}: the registration outlived the release"
            );
            assert_eq!(calls[0].registration, None, "{prefix}: {calls:?}");
        }
    }

    let bridge = Arc::new(CoreReadsBridge::default());
    let calls = retire_one_through(
        Arc::clone(&bridge) as Arc<dyn SessionBridge>,
        &bridge.core,
        "retired-hook-unobserved",
        meerkat_core::types::SessionId::new(),
    )
    .await;
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].registration, None, "{calls:?}");
}

/// End to end with the host's Core registration hook: each release hands
/// the hook the registration the registration hook was given for that
/// build, never another. The released identity, materialized again on the
/// same session at the same generation, is given a registration of its own
/// at that build, which its next retire then hands over.
#[tokio::test(flavor = "multi_thread")]
async fn each_retirement_hands_over_the_registration_its_build_was_given() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-build").await;
    let session = fixture
        .status(&fixture.kept)
        .await
        .session_id
        .expect("the member's session");
    let first = fixture.builds.registrations_on(&session);
    assert_eq!(first.len(), 1, "the first build: {first:?}");
    fixture
        .identities()
        .retire_tracked(&fixture.kept)
        .await
        .expect("the member is released");
    fixture
        .identities()
        .materialize(&fixture.kept)
        .await
        .expect("the released identity is materialized again");
    let both = fixture.builds.registrations_on(&session);
    assert_eq!(
        both.len(),
        2,
        "the second build on the same session: {both:?}"
    );
    assert_ne!(
        both[1], both[0],
        "the second build was given its own registration"
    );
    fixture
        .identities()
        .retire_tracked(&fixture.kept)
        .await
        .expect("the member is released again");
    let calls = fixture.seen.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(
        calls[0].registration.as_ref(),
        Some(&both[0]),
        "the first release hands over its build's registration"
    );
    assert_eq!(
        calls[1].registration.as_ref(),
        Some(&both[1]),
        "the second release hands over the second build's registration"
    );
    assert_eq!(
        calls[1].generation, calls[0].generation,
        "a release keeps the incarnation"
    );
    let _ = fixture.runtime.shutdown().await;
}

/// A reprofile releases the live member in the roster pass (it no longer
/// retires it) and never reaches the hook: the identity keeps its place in
/// the roster, seated on its session at its generation under its previous
/// role, and the role move this activation did not declare is held typed
/// until the host declares it.
#[tokio::test(flavor = "multi_thread")]
async fn a_reprofile_never_reaches_the_hook() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::start(&dir, "retired-hook-reprofile").await;
    let before = fixture.status(&fixture.kept).await;
    assert_eq!(before.state, IdentityLifecycleState::Active);
    let mut reprofiled = member(&fixture.kept);
    reprofiled.profile = "reviewer".into();
    fixture.roster.upsert(reprofiled);
    // The pass releases the live member, and its seat step holds the
    // undeclared role move typed.
    let _ = fixture.context.refresh_desired_topology_tracked().await;
    let after = fixture.status(&fixture.kept).await;
    assert_eq!(
        (
            after.agent_runtime_id.as_ref(),
            after.session_id.as_ref(),
            after.generation
        ),
        (
            before.agent_runtime_id.as_ref(),
            before.session_id.as_ref(),
            before.generation
        ),
        "the reprofile releases the member and keeps its incarnation: {before:?} -> {after:?}"
    );
    // Meta's spec names the new profile; Core still holds the member under
    // its previous role.
    assert_eq!(
        after.profile,
        Some(meerkat_mob::ProfileName::from("reviewer")),
        "{after:?}"
    );
    let kept = crate::member_comms_id::roster_member_id_for_identity(fixture.kept.as_str());
    let roles = fixture
        .runtime
        .mob_handle()
        .list_members()
        .await
        .into_iter()
        .filter(|member| member.agent_identity == kept)
        .map(|member| member.role.as_str().to_string())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["worker".to_string()], "{after:?}");
    assert_eq!(
        after.wake_refusal.as_ref().map(|refusal| refusal.remedy),
        Some(crate::identity_first::WakeRefusalRemedy::ResolveRoleMigration),
        "the undeclared role move is held typed: {after:?}"
    );
    let calls = fixture.seen.calls();
    assert!(calls.is_empty(), "{calls:?}");
    let _ = fixture.runtime.shutdown().await;
}
