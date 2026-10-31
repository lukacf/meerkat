//! Multi-host mobs phase 3 — the REAL two-hosts-in-one-process spawn ladder
//! (design-fold-ins-tests §3.2/§3.3 rows over a member-build
//! `HostDaemonFixture`): the §7.3 B2 walkthrough, the B2→B21 upcall spawn,
//! and the fold-in end-to-end rows (budget carrier, sealed policy both
//! directions, workgraph gate legs (ii)/(iii), taint relay to placed
//! members, revival with zero bridge traffic is `host_member_revival.rs`).
//!
//! Kept DISTINCT from `remote_spawn_materialization.rs` (scripted host):
//! these rows pin the REAL member-host serving surface end-to-end. Scripted
//! twins of T-L3/T-L4/T-L5 live in the materialization file (controlling
//! side) and `host_materialize_serving.rs` (host side) — noted merges.
//!
//! Serialized under `REAL_COMMS_TEST_LOCK`; notify-driven waits only.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_core::HandlingMode;
use meerkat_mob::AgentIdentity;
use meerkat_mob::MobError;
use meerkat_mob::MobSessionService as _;
use meerkat_mob::machines::mob_machine::MobSpawnMemberAdmissionKind;
use meerkat_mob::{
    DeclaredEntry, DeclaredEntryOutcome, DeclaredRoster, EntryStanding, IdentityEntryOrigin,
    IdentityEntryRead, UnseatedEntryCause,
};
use support::{
    HostFixtureOptions, OneShotToolCallClient, REAL_COMMS_TEST_LOCK, create_controlling_mob,
    placed_spawn_spec, spawn_host_daemon_fixture,
};

const WAIT: Duration = Duration::from_secs(30);

/// Wait (bounded, yield-driven) until `predicate` holds.
async fn wait_until<F, Fut>(what: &str, mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(WAIT, async {
        loop {
            if predicate().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

// ===========================================================================
// T-L1 — §7.3 B2 full walkthrough: placement → MaterializeMember → Host B
// builds → ack → remote commit → member turn runs ON HOST B
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn b2_walkthrough_materializes_on_host_b_and_turns_run_there() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("ladder-host-b").with_member_build())
            .await
            .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("ladder-walkthrough").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits from the real host ack");

    // Host-side durable truth: exactly one materialized row for b2, and the
    // session it names EXISTS on Host B's realm-local service...
    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    assert_eq!(record.materialized.len(), 1, "one materialized row for b2");
    let row = record
        .materialized
        .get("b2")
        .expect("materialized row keyed by agent identity");
    assert!(
        fixture.member_session_exists(&row.session_id).await,
        "the ack session must exist on Host B's member service"
    );
    // ...and does NOT exist on the controlling service (the remote binding
    // is machine truth, never a local session — §19.L5 gotcha 12).
    let controlling_view = controlling
        .service
        .load_persisted_session(
            &meerkat_core::SessionId::parse(&row.session_id).expect("session id parses"),
        )
        .await
        .expect("controlling service read");
    assert!(
        controlling_view.is_none(),
        "the member session must not exist on the controlling host"
    );

    // Roster committed; failures empty is pinned by the typed Ok above.
    assert!(
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b2"),
        "b2 committed to the roster"
    );

    // Deliver one input: the turn runs on Host B (TestClient); the typed
    // delivery receipt through the ack path IS the observation.
    let member = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("member handle");
    member
        .send("hello b2", HandlingMode::Queue)
        .await
        .expect("member turn on Host B acks through the delivery path");

    fixture.shutdown().await;
}

// ===========================================================================
// T-L2 — B2→B21 via upcall (§15.9 exit criterion): B2's forwarding
// dispatcher spawns B21, default placement = B2's host (DEC-P3F-8)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn b2_upcall_spawns_b21_on_its_own_host_by_default() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    // B2's first turn fires exactly one spawn_member tool call with NO
    // placement arg — the controlling receiver must resolve the default to
    // B2's own placement.
    let one_shot = OneShotToolCallClient::new(
        "spawn_member",
        serde_json::json!({ "profile": "worker", "member_id": "b21" }),
    );
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        member_llm_client: Some(one_shot.clone()),
        ..HostFixtureOptions::named("ladder-upcall-host-b").with_member_build()
    })
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("ladder-upcall").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("B2 materializes on Host B");

    // Kick B2: its scripted turn invokes the forwarding spawn_member tool →
    // MemberOperatorRequest → admission → controlling spawns B21 placed on
    // Host B.
    let member = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("b2 handle");
    member
        .send("spawn your helper", HandlingMode::Queue)
        .await
        .expect("B2 turn delivers");

    // Notify-driven convergence on the PUBLIC carriers: roster + Host B rows.
    wait_until("b21 to join the roster via upcall", || async {
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b21")
    })
    .await;

    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    let b21_row = record
        .materialized
        .get("b21")
        .expect("B21 defaults to B2's host — the materialized row lands on Host B");
    assert!(
        fixture.member_session_exists(&b21_row.session_id).await,
        "B21's session exists on Host B"
    );
    assert!(one_shot.fired(), "the upcall rode the member tool seam");

    fixture.shutdown().await;
}

// ===========================================================================
// T-F1 — budget seed end-to-end (ADJ-1): the digest-covered overlay budget
// lands in Host B's session build state; the wire pin lives in
// remote_spawn_materialization.rs (I12)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn spawn_budget_lands_in_host_b_session_build_state() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("ladder-budget-host").with_member_build(),
    )
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("ladder-budget").await;
    let report = controlling.bind_fixture(&fixture).await;

    let limits = meerkat_core::BudgetLimits {
        max_tokens: None,
        max_duration: None,
        max_turn_duration: None,
        max_tool_calls: Some(1),
    };
    let mut spec = placed_spawn_spec("worker", "b2", &report.host_id);
    spec.budget_limits = Some(limits.clone());
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("budgeted placed spawn commits");

    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    let row = record.materialized.get("b2").expect("b2 materialized row");
    // Durable spec row carries the overlay budget (revival recomposes it for
    // free — the T-F3 pin in host_member_revival.rs)...
    assert_eq!(
        row.spec.overlay.budget_limits.as_ref(),
        Some(&limits),
        "the stored spec's overlay is the single budget carrier"
    );
    // ...and the BUILT session's durable build state carries the applied
    // value (`apply_spawn_budget_limits` → CreateSessionRequest.build).
    let session = fixture
        .member_session_service()
        .load_persisted_session(
            &meerkat_core::SessionId::parse(&row.session_id).expect("session id parses"),
        )
        .await
        .expect("read member session")
        .expect("member session exists");
    assert_eq!(
        session.build_state().and_then(|state| state.budget_limits),
        Some(limits),
        "the member session was built with the overlay budget"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T-F2 — sealed policy shipped + persisted, containment BOTH directions
// (§18 O3): a restricted parent's Inherit-resolved child is never
// unrestricted, on either host
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn resolved_policy_ships_sealed_and_contains_children_on_host_b() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("ladder-policy-host").with_member_build(),
    )
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("ladder-policy").await;
    let report = controlling.bind_fixture(&fixture).await;

    let deny =
        meerkat_core::ops::ToolAccessPolicy::DenyList(["shell_execute"].into_iter().collect());
    let mut spec = placed_spawn_spec("worker", "restricted-b2", &report.host_id);
    spec.tool_access_policy = Some(deny.clone());
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("restricted placed spawn commits");

    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    let row = record
        .materialized
        .get("restricted-b2")
        .expect("materialized row");
    // Wire carried the RESOLVED policy, never Inherit (serde-unrepresentable;
    // end-to-end pin over the stored spec bytes).
    let overlay_policy = serde_json::to_value(&row.spec.overlay.tool_access_policy)
        .expect("overlay policy serializes");
    assert!(
        !overlay_policy
            .to_string()
            .to_lowercase()
            .contains("inherit"),
        "no inherit token may survive on the wire, got {overlay_policy}"
    );
    // Host B persisted the resolved policy into the member session's own
    // metadata (DEC-P3F-2: the existing factory persist path, no new seam).
    let session = fixture
        .member_session_service()
        .load_persisted_session(
            &meerkat_core::SessionId::parse(&row.session_id).expect("session id parses"),
        )
        .await
        .expect("read member session")
        .expect("member session exists");
    let metadata = session
        .session_metadata()
        .expect("member session has typed metadata");
    assert_eq!(
        metadata.tooling.tool_access_policy,
        Some(deny.clone()),
        "SessionMetadata.tooling.tool_access_policy equals the shipped resolved policy"
    );

    // Containment the OTHER direction (same policy, local spawn): the local
    // child persists the same effective policy — no host pair yields an
    // Inherit-derived unrestricted child.
    let mut local = meerkat_mob::SpawnMemberSpec::new("worker", "restricted-local");
    local.tool_access_policy = Some(deny.clone());
    controlling
        .handle
        .spawn_spec(local)
        .await
        .expect("restricted local spawn commits");
    let local_session_id = controlling
        .handle
        .resolve_bridge_session_id(&AgentIdentity::from("restricted-local"))
        .await
        .expect("local member has a bridge session");
    let local_session = controlling
        .service
        .load_persisted_session(&local_session_id)
        .await
        .expect("read local session")
        .expect("local session exists");
    assert_eq!(
        local_session
            .session_metadata()
            .expect("local metadata")
            .tooling
            .tool_access_policy,
        Some(deny),
        "local twin persists the identical effective policy (containment parity)"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T-F5 legs (ii)/(iii) — workgraph placement gate: the same profile that is
// DENIED remotely (pinned in remote_spawn_materialization.rs I3) is allowed
// locally; a builtins-only profile is allowed remotely
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn workgraph_gate_allows_local_and_builtins_remote() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("ladder-workgraph-host").with_member_build(),
    )
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("ladder-workgraph").await;
    let report = controlling.bind_fixture(&fixture).await;

    // (ii) same workgraph profile LOCAL → allowed.
    controlling
        .handle
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new("workgrapher", "wg-local"))
        .await
        .expect("workgraph profile spawns locally");

    // (iii) builtins-only (workgraph=false) REMOTE → allowed; the denied
    // combination never builds, so no skill can lie about it (gotcha 13
    // unrepresentable).
    controlling
        .spawn_placed("builtins-worker", "builtins-remote", &report.host_id)
        .await
        .expect("builtins-only profile materializes remotely");
    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    let row = record
        .materialized
        .get("builtins-remote")
        .expect("builtins member materialized on Host B");
    assert!(
        !row.spec.profile.tools.workgraph,
        "the shipped spec asserts no workgraph tools"
    );
    assert!(row.spec.profile.tools.builtins);

    // (i) remote deny twin, through the REAL host this time (same ladder
    // choke-point as the scripted pin — A4).
    let denied = controlling
        .spawn_placed("workgrapher", "wg-remote", &report.host_id)
        .await
        .expect_err("explicit workgraph profile must deny remotely");
    assert!(
        matches!(
            denied,
            MobError::SpawnMemberAdmissionDenied {
                admission: MobSpawnMemberAdmissionKind::NonPortableWorkgraphTools,
            }
        ),
        "expected NonPortableWorkgraphTools, got {denied:?}"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T-F6 — taint relay to placed members (§18 O8): declare → placed member's
// runtime installs it; lapse-on-rematerialize; fail-closed after revoke
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn taint_relay_reaches_placed_member_and_lapses_on_rematerialize() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("ladder-taint-host").with_member_build(),
    )
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("ladder-taint").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");

    // Relay arm rides the placed binding derivation (member pubkey @ host
    // acceptor address): a typed Ok IS the delivery pin — the member-side
    // receiver acks the DeclareMemberOutboundTaint command.
    let taint = meerkat_core::comms::SenderContentTaint::Tainted;
    controlling
        .handle
        .declare_member_outbound_taint(AgentIdentity::from("b2"), Some(taint))
        .await
        .expect("taint declaration relays to the placed member");

    // Lapse on rematerialize: retire + respawn at a new generation, then the
    // declaration must be GONE (handle-side semantics symmetric across
    // hosts) — re-declaring succeeds against the new incarnation.
    controlling
        .handle
        .retire(AgentIdentity::from("b2"))
        .await
        .expect("retire placed b2");
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("respawn b2 at a new generation");
    controlling
        .handle
        .declare_member_outbound_taint(
            AgentIdentity::from("b2"),
            Some(meerkat_core::comms::SenderContentTaint::Tainted),
        )
        .await
        .expect("re-declaration lands on the new incarnation");

    // Fail-closed twin: taint to a member whose host was REVOKED is a typed
    // error, never a silent drop.
    controlling
        .handle
        .revoke_host(&report.host_id)
        .await
        .expect("revoke host");
    let refused = controlling
        .handle
        .declare_member_outbound_taint(
            AgentIdentity::from("b2"),
            Some(meerkat_core::comms::SenderContentTaint::Tainted),
        )
        .await;
    assert!(
        refused.is_err(),
        "taint to a revoked-host member must fail typed, got {refused:?}"
    );

    fixture.shutdown().await;
}

// ===========================================================================
// T-L10 leg — tools.mob=false placed member exposes NO upcall tools: its
// one-shot spawn_member call dispatches NOTHING (dispatcher not mounted)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn mobless_profile_mounts_no_forwarding_dispatcher() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let one_shot = OneShotToolCallClient::new(
        "spawn_member",
        serde_json::json!({ "profile": "worker", "member_id": "never-spawned" }),
    );
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        member_llm_client: Some(one_shot),
        ..HostFixtureOptions::named("ladder-mobless-host").with_member_build()
    })
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("ladder-mobless").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("quiet-worker", "b2", &report.host_id)
        .await
        .expect("quiet placed spawn commits");
    let member = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("b2 handle");
    member
        .send("try to spawn", HandlingMode::Queue)
        .await
        .expect("turn delivers (the tool call fails member-side, the turn completes)");

    // The upcall tool does not exist for this member: no B21-class roster
    // entry may ever appear.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "never-spawned"),
        "a tools.mob=false member must not be able to upcall-spawn"
    );

    fixture.shutdown().await;
}

/// A host whose member substrate carries a registration hook: records each
/// member it is asked about and selects (or refuses) that member's own
/// session document as its destination.
struct PlacedMemberHost {
    seen: std::sync::Mutex<Vec<String>>,
    refuse: bool,
}

#[async_trait::async_trait]
impl meerkat_mob::MemberRuntimeRegistrationHook for PlacedMemberHost {
    async fn select_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, MobError> {
        self.seen
            .lock()
            .unwrap()
            .push(identity.member().member.clone());
        if self.refuse {
            return Err(MobError::WiringError(
                "host refused the placed member".into(),
            ));
        }
        let owner = meerkat_core::auth::PrincipalRef::new(
            meerkat_core::auth::PrincipalKind::SharedAgent,
            identity.member().member.clone(),
        )
        .unwrap();
        Ok(meerkat_mob::MemberDestinationSelection {
            logical_owner: owner.clone(),
            context: identity.session_document().clone(),
            context_generation: 1,
            audience: meerkat_mob::AudienceRef::Principal { principal: owner },
        })
    }

    async fn expected_destination(
        &self,
        _identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, MobError> {
        Err(MobError::WiringError("not used by these tests".into()))
    }

    async fn verify_member_surface(
        &self,
        _identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        _surface: &[meerkat_core::ToolDef],
    ) -> Result<(), MobError> {
        Ok(())
    }
}

async fn placed_spawn_through_hook(
    name: &str,
    refuse: bool,
) -> (
    Result<meerkat_mob::SpawnResult, MobError>,
    std::sync::Arc<PlacedMemberHost>,
) {
    let host = std::sync::Arc::new(PlacedMemberHost {
        seen: std::sync::Mutex::new(Vec::new()),
        refuse,
    });
    let fixture = spawn_host_daemon_fixture(HostFixtureOptions {
        member_runtime_registration_hook: Some(host.clone()),
        ..HostFixtureOptions::named(name).with_member_build()
    })
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob(name).await;
    let report = controlling.bind_fixture(&fixture).await;
    let result = controlling
        .spawn_placed("worker", "placed", &report.host_id)
        .await;
    (result, host)
}

/// A host-placed member registers through the serving host's hook: the hook
/// selects the placed member's destination, and the placed spawn does not
/// commit. (The fixture's member substrate installs no governed work host,
/// which a scoped destination requires; this test asserts the refusal, not
/// its remote reason.)
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_member_registers_through_the_host_hook() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (result, host) = placed_spawn_through_hook("ladder-hook-host", false).await;
    assert!(
        result.is_err(),
        "the placed spawn does not commit an unscoped member"
    );
    assert!(
        host.seen
            .lock()
            .unwrap()
            .iter()
            .any(|member| member == "placed"),
        "the serving host's hook selected the placed member's destination"
    );
}

/// A host hook that refuses a placed member refuses the placed spawn: no
/// host-placed member bypasses the host's selection.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_hook_refusal_refuses_the_placed_member() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (result, host) = placed_spawn_through_hook("ladder-hook-refusing-host", true).await;
    assert!(result.is_err(), "a refused placed member does not commit");
    assert!(
        host.seen
            .lock()
            .unwrap()
            .iter()
            .any(|member| member == "placed"),
        "the refusal came from the serving host's hook"
    );
}

// ===========================================================================
// A declared placed identity: hosted on its first use, created by a placed
// spawn holding the entry's custody, and hosted on its first use again after
// its retirement, across restarts
// ===========================================================================

/// The origin and standing of the identity entry `identity` is, or `None`
/// when it is no entry.
fn entry_of(
    handle: &meerkat_mob::MobHandle,
    identity: &AgentIdentity,
) -> Option<(IdentityEntryOrigin, EntryStanding)> {
    if let IdentityEntryRead::Entry(view) = handle
        .identity_entry(identity)
        .expect("the entry read answers")
    {
        Some((view.origin, view.standing))
    } else {
        None
    }
}

/// A placed identity the host declares is hosted on its first use: the
/// apply records the entry unseated (`HostedOnFirstUse`) and seats nothing,
/// and the host's placed spawn holding the entry's custody creates its
/// member on the member host, whose remote commit seats it as the Declared
/// entry. A restart replays it Seated and Declared. Its retirement keeps the
/// entry hosted on its next first use (its session lives on its host), and
/// a restart replays the same.
///
/// RED: replay a retired placed incarnation's retirement as a local one
/// (pass `hosted_incarnation: false` from the replay of its `MemberRetired`
/// row): after the second restart the entry reads owed its seat
/// (`SeatOwed`).
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_placed_identity_is_hosted_on_its_first_use_across_restarts() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("ladder-entry-host").with_member_build(),
    )
    .await
    .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("ladder-placed-entry").await;
    let report = controlling.bind_fixture(&fixture).await;
    let identity = AgentIdentity::from("placed-entry");
    let hosted = Some((
        IdentityEntryOrigin::Declared,
        EntryStanding::Unseated(UnseatedEntryCause::HostedOnFirstUse),
    ));

    let declared = controlling
        .handle
        .apply_declared_roster(DeclaredRoster::partial([DeclaredEntry::new(
            placed_spawn_spec("worker", "placed-entry", &report.host_id),
        )]))
        .await
        .expect("the apply runs");
    assert!(
        matches!(
            declared.entries.get(&identity),
            Some(DeclaredEntryOutcome::HostedOnFirstUse)
        ),
        "{declared:?}"
    );
    assert_eq!(entry_of(&controlling.handle, &identity), hosted);

    controlling
        .handle
        .spawn_spec(
            placed_spawn_spec("worker", "placed-entry", &report.host_id).with_entry_custody(true),
        )
        .await
        .expect("the placed spawn holding the entry's custody commits");
    assert!(
        matches!(
            entry_of(&controlling.handle, &identity),
            Some((IdentityEntryOrigin::Declared, EntryStanding::Seated(_)))
        ),
        "the remote commit seats the entry"
    );

    let controlling = controlling.restart().await;
    assert!(
        matches!(
            entry_of(&controlling.handle, &identity),
            Some((IdentityEntryOrigin::Declared, EntryStanding::Seated(_)))
        ),
        "the replay seats the entry again with its placed carrier"
    );

    controlling
        .handle
        .retire(identity.clone())
        .await
        .expect("the placed member's retirement converges on its host");
    assert_eq!(
        entry_of(&controlling.handle, &identity),
        hosted,
        "the retirement keeps the entry, hosted on its next first use"
    );

    let controlling = controlling.restart().await;
    assert_eq!(
        entry_of(&controlling.handle, &identity),
        hosted,
        "the replay of the retirement keeps it so"
    );

    fixture.shutdown().await;
}
