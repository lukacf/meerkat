//! Multi-host mobs phase 5 — the §11 control-scope enforcement matrix
//! (design-enforcement §4.2, rows T-M1..T-M15; ADJ-P5-10..17).
//!
//! Every row drives verbs through the PRODUCTION principal-binding seam
//! (ADJ-P5-10): `handle.with_command_authority(CommandAuthority::principal(..))`
//! on a handle clone — the exact seam the v2 bearer-token resolver lands on.
//! Denials are asserted TYPED (`MobError::ScopeDenied(ScopeDenial)`), never
//! via display strings.
//!
//! Deny rows deliberately target nonexistent members/hosts: chokepoint (a)
//! fires BEFORE the handler body, so a denied principal learns nothing about
//! roster state and the row needs no arrange step. Allow rows arrange real
//! members as the owner first.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/live_plane.rs"]
mod live_support;
mod support;

use std::collections::BTreeSet;

use meerkat_mob::machines::mob_machine::ControlScope;
use meerkat_mob::{MobControlPrincipal, MobError, ScopeDenial};
use support::{ControllingMob, control_principal, create_controlling_mob};

// ───────────────────────── helpers ─────────────────────────

fn expect_scope_denied<T: std::fmt::Debug, E>(
    result: Result<T, E>,
    required: ControlScope,
    presented: &[ControlScope],
) where
    E: std::fmt::Debug + Into<MobError>,
{
    match result {
        Err(error) => match error.into() {
            MobError::ScopeDenied(denial) => {
                assert_eq!(
                    denial.required, required,
                    "denied scope names the verb's class"
                );
                let expected: BTreeSet<ControlScope> = presented.iter().copied().collect();
                assert_eq!(
                    denial.presented, expected,
                    "presented set is the caller's own effective (post-expiry) set"
                );
            }
            other => panic!("expected ScopeDenied({required:?}), got {other:?}"),
        },
        Ok(value) => panic!("expected ScopeDenied({required:?}), got Ok({value:?})"),
    }
}

fn assert_not_scope_denied<T, E>(result: &Result<T, E>, context: &str)
where
    E: std::fmt::Debug,
{
    if let Err(error) = result {
        let rendered = format!("{error:?}");
        assert!(
            !rendered.contains("ScopeDenied"),
            "{context}: expected gate passage (any post-gate outcome), got {rendered}"
        );
    }
}

/// `list_members` projects the committed machine WATCH, which propagates
/// asynchronously after a spawn commit replies — converge the arrange
/// snapshot on it so before/after comparisons race nothing.
async fn members_settled(
    mob: &ControllingMob,
    expected: usize,
) -> Vec<meerkat_mob::runtime::MobMemberListEntry> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let members = mob.handle.list_members().await;
        if members.len() == expected {
            return members;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "member projection did not settle to {expected} members"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

fn local_spec(member: &str) -> meerkat_mob::SpawnMemberSpec {
    // Session backend + turn-driven: deterministic local members through the
    // fixture's TestClient factory, no autonomous kickoff churn.
    meerkat_mob::SpawnMemberSpec::new("worker", member)
        .with_backend(meerkat_mob::MobBackendKind::Session)
        .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven)
}

async fn spawn_local(mob: &ControllingMob, member: &str) {
    mob.handle
        .spawn_spec(local_spec(member))
        .await
        .expect("owner arranges a local member");
}

fn identity(name: &str) -> meerkat_mob::AgentIdentity {
    meerkat_mob::AgentIdentity::from(name)
}

/// The grantable scopes with representative verbs. Phase 6 gives
/// `ReadHistory` a real MobCommand (`MemberHistory`, DEC-P6E-21); phase 6b
/// gives `Live` its verb family (`MemberLiveOpen`/`Close`/`Status`/
/// `Control`, DEC-P6B-C1/C2) — T-C8d: `Live` grants exactly the live family
/// and no other scope grants any live verb.
const GRANTABLE: &[ControlScope] = &[
    ControlScope::List,
    ControlScope::ReadHistory,
    ControlScope::SubscribeEvents,
    ControlScope::SendCommand,
    ControlScope::Cancel,
    ControlScope::Retire,
    ControlScope::WireTopology,
    ControlScope::AdminHost,
    ControlScope::AdminGrants,
    ControlScope::Live,
];

/// Drive scope S's representative verb as `principal`, returning the raw
/// outcome classification: Ok / ScopeDenied(required) / other typed error.
enum VerbOutcome {
    Passed,
    ScopeDenied(ScopeDenial),
    OtherError(String),
}

async fn drive_representative_verb(
    mob: &ControllingMob,
    principal: &str,
    scope: ControlScope,
    unique: &str,
) -> VerbOutcome {
    let handle = mob.handle_as(principal);
    fn classify<T>(result: Result<T, MobError>) -> VerbOutcome {
        match result {
            Ok(_) => VerbOutcome::Passed,
            Err(MobError::ScopeDenied(denial)) => VerbOutcome::ScopeDenied(denial),
            Err(other) => VerbOutcome::OtherError(other.to_string()),
        }
    }
    match scope {
        // List → member-status projection (ProjectMemberStatus, actor-routed).
        ControlScope::List => classify(
            handle
                .member_status(&identity("matrix-member"))
                .await
                .map(|_| ()),
        ),
        // ReadHistory → member transcript page (MemberHistory, phase 6).
        ControlScope::ReadHistory => classify(
            handle
                .member_history(
                    control_principal(principal),
                    identity("matrix-member"),
                    None,
                    Some(1),
                )
                .await
                .map(|_| ()),
        ),
        // SubscribeEvents → mob event poll (PollEvents, actor-routed).
        ControlScope::SubscribeEvents => classify(handle.events().poll(0, 8).await.map(|_| ())),
        // SendCommand → member spawn (drives the roster).
        ControlScope::SendCommand => classify(
            handle
                .spawn_spec(local_spec(&format!("spawned-{unique}")))
                .await
                .map(|_| ()),
        ),
        // Cancel → force-cancel.
        ControlScope::Cancel => {
            classify(handle.force_cancel_member(identity("matrix-member")).await)
        }
        // Retire → retire.
        ControlScope::Retire => {
            classify(handle.retire(identity(&format!("victim-{unique}"))).await)
        }
        // WireTopology → wire two members.
        ControlScope::WireTopology => classify(
            handle
                .wire(identity("matrix-member"), identity("matrix-peer"))
                .await,
        ),
        // AdminHost → revoke a host (A9: exactly bind/revoke hosts). An
        // unknown host id still proves gate passage via a typed
        // non-ScopeDenied error.
        ControlScope::AdminHost => classify(handle.revoke_host("no-such-host").await),
        // AdminGrants → the grants read verb (self-gated, §17.2:815).
        ControlScope::AdminGrants => classify(
            handle
                .grants(control_principal(principal))
                .await
                .map(|_| ()),
        ),
        // Live → the dedicated live point read (MemberLiveStatus, phase 6b).
        // Post-gate outcomes (MemberNotFound / LiveTransportUnavailable) are
        // gate-passage proofs like AdminHost's unknown-host row.
        ControlScope::Live => classify(
            handle
                .member_live_status(
                    control_principal(principal),
                    identity("matrix-member"),
                    None,
                )
                .await
                .map(|_| ()),
        ),
    }
}

// ───────────────────────── T-M1 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn each_scope_grants_exactly_its_verbs() {
    let mob = create_controlling_mob("scope-m1").await;
    spawn_local(&mob, "matrix-member").await;
    spawn_local(&mob, "matrix-peer").await;

    for (index, granted) in GRANTABLE.iter().copied().enumerate() {
        let principal = format!("m1-p{index}");
        mob.grant(&principal, &[granted], None).await;

        for (verb_index, verb_scope) in GRANTABLE.iter().copied().enumerate() {
            let unique = format!("{index}-{verb_index}");
            if verb_scope == ControlScope::Retire && granted == ControlScope::Retire {
                // Retire's allow row needs a live victim.
                spawn_local(&mob, &format!("victim-{unique}")).await;
            }
            let outcome = drive_representative_verb(&mob, &principal, verb_scope, &unique).await;
            if verb_scope == granted {
                match outcome {
                    VerbOutcome::Passed => {}
                    // A post-gate typed error that is provably not
                    // ScopeDenied still proves the gate admitted the verb.
                    VerbOutcome::OtherError(_) => {}
                    VerbOutcome::ScopeDenied(denial) => {
                        panic!("granted {granted:?} must pass its own verb, denied: {denial:?}")
                    }
                }
            } else {
                match outcome {
                    VerbOutcome::ScopeDenied(denial) => {
                        assert_eq!(
                            denial.required, verb_scope,
                            "denial names the verb's required scope"
                        );
                        assert_eq!(
                            denial.presented,
                            BTreeSet::from([granted]),
                            "denial presents the caller's own granted set"
                        );
                    }
                    VerbOutcome::Passed => {
                        panic!("{granted:?}-only principal must be denied at {verb_scope:?}'s verb")
                    }
                    VerbOutcome::OtherError(error) => panic!(
                        "{granted:?}-only principal at {verb_scope:?}'s verb must deny \
                         TYPED at the gate, before any handler error; got {error}"
                    ),
                }
            }
        }
    }
}

// ───────────────────────── T-M2 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn send_command_drives_but_cannot_read() {
    let mob = create_controlling_mob("scope-m2").await;
    mob.grant("worker-bee", &[ControlScope::SendCommand], None)
        .await;
    let handle = mob.handle_as("worker-bee");

    // Drives: spawn succeeds.
    handle
        .spawn_spec(local_spec("bee-child"))
        .await
        .expect("SendCommand grants the spawn verb");

    // Cannot read the roster projection.
    expect_scope_denied(
        handle.member_status(&identity("bee-child")).await,
        ControlScope::List,
        &[ControlScope::SendCommand],
    );

    // Cannot read grants.
    expect_scope_denied(
        handle.grants(control_principal("worker-bee")).await,
        ControlScope::AdminGrants,
        &[ControlScope::SendCommand],
    );

    // Phase 6 (ADJ-P5-13 realized): the ReadHistory verb exists — work-only
    // cannot read a member transcript.
    expect_scope_denied(
        handle
            .member_history(
                control_principal("worker-bee"),
                identity("bee-child"),
                None,
                None,
            )
            .await,
        ControlScope::ReadHistory,
        &[ControlScope::SendCommand],
    );
}

// ───────────────────────── T-E8 (phase 6, ADJ-P5-18) ─────────────────────────

/// The projection trio (`ProjectMemberList` / `MemberMachineProjection` /
/// `QueryPhase`) classifies `List` at chokepoint (a) and replies through
/// Result channels: a non-owner principal without `List` gets a TYPED
/// `ScopeDenied` — never fake lifecycle/phase data (the pre-phase-6
/// closed-channel→WATCH laundering shape is dead).
///
/// `QueryPhase` is the publicly reachable member of the trio
/// (`MobHandle::status`); the `ProjectMemberList`/`MemberMachineProjection`
/// Result-channel conversions are additionally pinned by the events lane's
/// in-crate rows (their reply channels are not public verbs).
#[tokio::test(flavor = "multi_thread")]
async fn projection_trio_denies_typed_for_non_owner_principal() {
    let mob = create_controlling_mob("scope-e8").await;
    spawn_local(&mob, "e8-member").await;

    // No grant at all: every trio read denies typed with required=List.
    let bare = mob.handle_as("e8-nobody");
    expect_scope_denied(bare.status().await, ControlScope::List, &[]);

    // A denial is a typed Err — the caller can NEVER mistake it for a
    // lifecycle phase (no laundering into fake phase data).
    match bare.status().await {
        Err(MobError::ScopeDenied(_)) => {}
        other => panic!("denied status must be a typed ScopeDenied Err, got {other:?}"),
    }

    // With the List grant the same verb passes the gate and serves the real
    // phase.
    mob.grant("e8-reader", &[ControlScope::List], None).await;
    let reader = mob.handle_as("e8-reader");
    reader
        .status()
        .await
        .expect("List-granted principal reads the lifecycle phase");
    reader
        .member_status(&identity("e8-member"))
        .await
        .expect("List-granted principal reads member status");
}

/// Phase 6 hard cancel is `Cancel`-scoped at chokepoint (a): a read-only
/// principal cannot hard-cancel, and the denial is typed.
#[tokio::test(flavor = "multi_thread")]
async fn hard_cancel_requires_cancel_scope() {
    let mob = create_controlling_mob("scope-e8-hc").await;
    spawn_local(&mob, "hc-member").await;
    mob.grant("hc-reader", &[ControlScope::List], None).await;

    expect_scope_denied(
        mob.handle_as("hc-reader")
            .hard_cancel_member(
                control_principal("hc-reader"),
                identity("hc-member"),
                "denied attempt",
            )
            .await,
        ControlScope::Cancel,
        &[ControlScope::List],
    );
}

// ─────────────────── T-C8a..T-C8c (phase 6b, DL8/ADJ-P6B-11) ───────────────────

/// T-C8a — owner-implicit-full: with ZERO grant rows, all four live verbs
/// pass chokepoint (a) and reach their post-gate outcomes (here the local
/// no-gateway `LiveTransportUnavailable` / channel misses — any typed
/// non-ScopeDenied error proves gate passage).
#[tokio::test(flavor = "multi_thread")]
async fn owner_passes_the_live_family_with_zero_grants() {
    let mob = create_controlling_mob("scope-c8a").await;
    spawn_local(&mob, "live-member").await;

    assert_not_scope_denied(
        &mob.handle
            .member_live_open(
                MobControlPrincipal::Owner,
                identity("live-member"),
                None,
                None,
            )
            .await,
        "owner at member_live_open",
    );
    assert_not_scope_denied(
        &mob.handle
            .member_live_close(
                MobControlPrincipal::Owner,
                identity("live-member"),
                "chan-x".to_string(),
            )
            .await,
        "owner at member_live_close",
    );
    assert_not_scope_denied(
        &mob.handle
            .member_live_status(MobControlPrincipal::Owner, identity("live-member"), None)
            .await,
        "owner at member_live_status",
    );
    assert_not_scope_denied(
        &mob.handle
            .member_live_control(
                MobControlPrincipal::Owner,
                identity("live-member"),
                "chan-x".to_string(),
                meerkat_mob::runtime::bridge_protocol::BridgeLiveControlVerb::CommitInput,
            )
            .await,
        "owner at member_live_control",
    );
}

/// T-C8b — a non-owner principal granted exactly `[Live]` drives a proxied
/// open END-TO-END (bootstrap issuance IS the scope-gated act, DL8) and is
/// admitted at the other three verbs. Merge-gated on the member arms +
/// scripted factory (seams S3/S4).
#[tokio::test(flavor = "multi_thread")]
async fn live_granted_principal_opens_end_to_end() {
    let _guard = support::REAL_COMMS_TEST_LOCK.lock().await;
    let (listener, local_addr) = live_support::bind_live_listener().await;
    let mut opts = support::HostFixtureOptions::named("scope-c8b-host-b").with_member_build();
    opts.live_endpoint = Some(format!("ws://{local_addr}"));
    opts.resolvable_providers = vec![
        meerkat_core::Provider::Anthropic,
        meerkat_core::Provider::OpenAI,
    ];
    opts.member_llm_client = Some(support::scripted_member_client_completing("c8b-done"));
    let fixture = support::spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn live member-host fixture");
    let _plane = live_support::install_live_plane(
        &fixture,
        listener,
        None,
        live_support::scripted_realtime_factory(),
    )
    .await;
    let mob = support::create_controlling_mob_with_definition(
        "scope-c8b",
        support::add_realtime_worker_profile,
    )
    .await;
    let report = mob.bind_fixture(&fixture).await;
    mob.spawn_placed("rt-worker", "c8b-member", &report.host_id)
        .await
        .expect("realtime member materializes on host B");

    mob.grant("live-operator", &[ControlScope::Live], None)
        .await;
    let operator = mob.handle_as("live-operator");

    let open = operator
        .member_live_open(
            control_principal("live-operator"),
            identity("c8b-member"),
            None,
            None,
        )
        .await
        .expect("Live grant admits the proxied open end-to-end");
    assert!(
        !open.channel_id.is_empty(),
        "bootstrap issued to the grantee"
    );

    assert_not_scope_denied(
        &operator
            .member_live_status(
                control_principal("live-operator"),
                identity("c8b-member"),
                None,
            )
            .await,
        "Live grant at member_live_status",
    );
    assert_not_scope_denied(
        &operator
            .member_live_control(
                control_principal("live-operator"),
                identity("c8b-member"),
                open.channel_id.clone(),
                meerkat_mob::runtime::bridge_protocol::BridgeLiveControlVerb::CommitInput,
            )
            .await,
        "Live grant at member_live_control",
    );
    assert_not_scope_denied(
        &operator
            .member_live_close(
                control_principal("live-operator"),
                identity("c8b-member"),
                open.channel_id,
            )
            .await,
        "Live grant at member_live_close",
    );

    fixture.shutdown().await;
}

/// T-C8c — a non-owner principal with every ADJACENT grant but NOT `Live`
/// is denied typed at all four verbs BEFORE any dispatch: no bridge command
/// is built, no token is ever minted (DL8's row 10 + denied-command-
/// mutates-nothing), pinned by the S3b served-count staying zero.
#[tokio::test(flavor = "multi_thread")]
async fn live_family_denies_without_live_scope_and_mutates_nothing() {
    let _guard = support::REAL_COMMS_TEST_LOCK.lock().await;
    let mut opts = support::HostFixtureOptions::named("scope-c8c-host-b").with_member_build();
    // Live-CAPABLE bind fact: the deny must come from chokepoint (a), not
    // the capability gate.
    opts.live_endpoint = Some("wss://live.example.test:8443".to_string());
    opts.member_llm_client = Some(support::scripted_member_client_completing("c8c-done"));
    let fixture = support::spawn_host_daemon_fixture(opts)
        .await
        .expect("spawn live-capable host fixture");
    let mob = create_controlling_mob("scope-c8c").await;
    let report = mob.bind_fixture(&fixture).await;
    mob.spawn_placed("worker", "c8c-member", &report.host_id)
        .await
        .expect("worker materializes on host B");

    let adjacent = [
        ControlScope::SendCommand,
        ControlScope::SubscribeEvents,
        ControlScope::ReadHistory,
    ];
    mob.grant("almost-live", &adjacent, None).await;
    let handle = mob.handle_as("almost-live");

    expect_scope_denied(
        handle
            .member_live_open(
                control_principal("almost-live"),
                identity("c8c-member"),
                None,
                None,
            )
            .await,
        ControlScope::Live,
        &adjacent,
    );
    expect_scope_denied(
        handle
            .member_live_close(
                control_principal("almost-live"),
                identity("c8c-member"),
                "chan-x".to_string(),
            )
            .await,
        ControlScope::Live,
        &adjacent,
    );
    expect_scope_denied(
        handle
            .member_live_status(
                control_principal("almost-live"),
                identity("c8c-member"),
                None,
            )
            .await,
        ControlScope::Live,
        &adjacent,
    );
    expect_scope_denied(
        handle
            .member_live_control(
                control_principal("almost-live"),
                identity("c8c-member"),
                "chan-x".to_string(),
                meerkat_mob::runtime::bridge_protocol::BridgeLiveControlVerb::CommitInput,
            )
            .await,
        ControlScope::Live,
        &adjacent,
    );

    assert_eq!(
        fixture.live_commands_served(),
        0,
        "denied live verbs dispatched NOTHING — no bridge command, no token \
         minted (S3b pin)"
    );

    fixture.shutdown().await;
}

// ───────────────────────── T-M3 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn subscribe_without_cancel_watches_but_cannot_stop() {
    let mob = create_controlling_mob("scope-m3").await;
    spawn_local(&mob, "watched").await;
    mob.grant("watcher", &[ControlScope::SubscribeEvents], None)
        .await;
    let handle = mob.handle_as("watcher");

    let events = handle
        .events()
        .poll(0, 64)
        .await
        .expect("SubscribeEvents grants the event poll verb");
    assert!(
        !events.is_empty(),
        "a spawned mob has recorded lifecycle events to observe"
    );

    expect_scope_denied(
        handle.force_cancel_member(identity("watched")).await,
        ControlScope::Cancel,
        &[ControlScope::SubscribeEvents],
    );
}

// ───────────────────────── T-M4 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn wire_topology_distinct_from_send_command() {
    let mob = create_controlling_mob("scope-m4").await;
    spawn_local(&mob, "left").await;
    spawn_local(&mob, "right").await;

    mob.grant("wirer", &[ControlScope::WireTopology], None)
        .await;
    mob.grant("driver", &[ControlScope::SendCommand], None)
        .await;

    let wirer = mob.handle_as("wirer");
    wirer
        .wire(identity("left"), identity("right"))
        .await
        .expect("WireTopology grants the wire verb");
    expect_scope_denied(
        wirer
            .spawn_spec(local_spec("wirer-child"))
            .await
            .map(|_| ()),
        ControlScope::SendCommand,
        &[ControlScope::WireTopology],
    );

    let driver = mob.handle_as("driver");
    driver
        .spawn_spec(local_spec("driver-child"))
        .await
        .expect("SendCommand grants the spawn verb");
    expect_scope_denied(
        driver
            .wire(identity("left"), identity("driver-child"))
            .await,
        ControlScope::WireTopology,
        &[ControlScope::SendCommand],
    );
}

// ───────────────────────── T-M5 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn admin_host_distinct_from_everything() {
    let mob = create_controlling_mob("scope-m5").await;
    mob.grant("hostadmin", &[ControlScope::AdminHost], None)
        .await;
    mob.grant("grantadmin", &[ControlScope::AdminGrants], None)
        .await;

    // AdminHost passes the host verbs (unknown host ⇒ typed non-ScopeDenied
    // error proves gate passage), and is denied everything else.
    let hostadmin = mob.handle_as("hostadmin");
    let revoke = hostadmin.revoke_host("no-such-host").await;
    assert_not_scope_denied(&revoke, "AdminHost at revoke_host");
    expect_scope_denied(
        hostadmin.grants(control_principal("hostadmin")).await,
        ControlScope::AdminGrants,
        &[ControlScope::AdminHost],
    );
    expect_scope_denied(
        hostadmin.retire(identity("nobody")).await,
        ControlScope::Retire,
        &[ControlScope::AdminHost],
    );

    // AdminGrants does not confer host administration (A9's split is
    // load-bearing).
    let grantadmin = mob.handle_as("grantadmin");
    expect_scope_denied(
        grantadmin.revoke_host("no-such-host").await,
        ControlScope::AdminHost,
        &[ControlScope::AdminGrants],
    );
}

// ───────────────────────── T-M6 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn owner_implicit_full_scope_with_zero_grants() {
    let mob = create_controlling_mob("scope-m6").await;
    spawn_local(&mob, "matrix-member").await;
    spawn_local(&mob, "matrix-peer").await;

    // The launch-bound owner handle passes every representative verb without
    // any grant row existing.
    let owner = &mob.handle;
    owner
        .member_status(&identity("matrix-member"))
        .await
        .expect("owner passes List verbs");
    owner
        .events()
        .poll(0, 8)
        .await
        .expect("owner passes SubscribeEvents verbs");
    owner
        .spawn_spec(local_spec("owner-child"))
        .await
        .expect("owner passes SendCommand verbs");
    assert_not_scope_denied(
        &owner.force_cancel_member(identity("matrix-member")).await,
        "owner at force_cancel",
    );
    owner
        .wire(identity("matrix-member"), identity("matrix-peer"))
        .await
        .expect("owner passes WireTopology verbs");
    assert_not_scope_denied(
        &owner.revoke_host("no-such-host").await,
        "owner at revoke_host",
    );
    owner
        .retire(identity("owner-child"))
        .await
        .expect("owner passes Retire verbs");

    // Full scope comes from identity, never from a synthesized grant row.
    let grants = owner
        .grants(MobControlPrincipal::Owner)
        .await
        .expect("owner passes AdminGrants verbs");
    assert!(
        grants.is_empty(),
        "owner-implicit-full must not materialize grant records, got {grants:?}"
    );
}

// ───────────────────────── T-M7 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn default_deny_covers_every_operator_verb() {
    let mob = create_controlling_mob("scope-m7").await;

    for (index, scope) in GRANTABLE.iter().copied().enumerate() {
        let outcome =
            drive_representative_verb(&mob, "stranger", scope, &format!("m7-{index}")).await;
        match outcome {
            VerbOutcome::ScopeDenied(denial) => {
                assert_eq!(denial.required, scope);
                assert!(
                    denial.presented.is_empty(),
                    "an ungranted principal presents the empty set, got {:?}",
                    denial.presented
                );
            }
            VerbOutcome::Passed => {
                panic!("ungranted principal must be denied at {scope:?}'s verb")
            }
            VerbOutcome::OtherError(error) => panic!(
                "ungranted principal at {scope:?}'s verb must deny typed at the \
                 gate, got {error}"
            ),
        }
    }
}

// ───────────────────────── T-M8 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn denied_command_mutates_nothing() {
    let mob = create_controlling_mob("scope-m8").await;
    spawn_local(&mob, "stable-a").await;
    spawn_local(&mob, "stable-b").await;

    let before_members = members_settled(&mob, 2).await;
    let before_grants = mob
        .handle
        .grants(MobControlPrincipal::Owner)
        .await
        .expect("owner grants read");

    let stranger = mob.handle_as("stranger");
    expect_scope_denied(
        stranger.retire(identity("stable-a")).await,
        ControlScope::Retire,
        &[],
    );
    expect_scope_denied(
        stranger
            .wire(identity("stable-a"), identity("stable-b"))
            .await,
        ControlScope::WireTopology,
        &[],
    );

    let after_members = mob.handle.list_members().await;
    let after_grants = mob
        .handle
        .grants(MobControlPrincipal::Owner)
        .await
        .expect("owner grants read");
    assert_eq!(
        before_members.len(),
        after_members.len(),
        "denied retire/wire must not change the roster"
    );
    assert_eq!(
        before_members
            .iter()
            .map(|entry| entry.agent_identity.to_string())
            .collect::<BTreeSet<_>>(),
        after_members
            .iter()
            .map(|entry| entry.agent_identity.to_string())
            .collect::<BTreeSet<_>>(),
    );
    assert_eq!(before_grants, after_grants, "denials record no grant rows");
}

// ───────────────────────── T-M9 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn admin_grants_gates_the_grant_verbs_and_chains() {
    let mob = create_controlling_mob("scope-m9").await;
    spawn_local(&mob, "cancel-target").await;

    // A principal without AdminGrants is denied at all three grant verbs.
    let stranger = mob.handle_as("p1");
    expect_scope_denied(
        stranger
            .grant_scopes(
                control_principal("p1"),
                support::principal_id("p2"),
                BTreeSet::from([ControlScope::Cancel]),
                None,
            )
            .await,
        ControlScope::AdminGrants,
        &[],
    );
    expect_scope_denied(
        stranger
            .revoke_scopes(control_principal("p1"), support::principal_id("p2"), None)
            .await,
        ControlScope::AdminGrants,
        &[],
    );
    expect_scope_denied(
        stranger.grants(control_principal("p1")).await,
        ControlScope::AdminGrants,
        &[],
    );

    // Owner grants AdminGrants to P1; P1 grants Cancel to P2; P2 cancels.
    mob.grant("p1", &[ControlScope::AdminGrants], None).await;
    let p1 = mob.handle_as("p1");
    p1.grant_scopes(
        control_principal("p1"),
        support::principal_id("p2"),
        BTreeSet::from([ControlScope::Cancel]),
        None,
    )
    .await
    .expect("AdminGrants principal can grant");

    let p2 = mob.handle_as("p2");
    assert_not_scope_denied(
        &p2.force_cancel_member(identity("cancel-target")).await,
        "P2 with delegated Cancel",
    );

    // P1 revokes; P2 is denied again.
    let removed = p1
        .revoke_scopes(control_principal("p1"), support::principal_id("p2"), None)
        .await
        .expect("AdminGrants principal can revoke");
    assert!(removed, "full revoke removes the grant record");
    expect_scope_denied(
        p2.force_cancel_member(identity("cancel-target")).await,
        ControlScope::Cancel,
        &[],
    );
}

// ───────────────────────── T-M10 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn revoke_all_restores_default_deny_and_partial_keeps_expiry() {
    let mob = create_controlling_mob("scope-m10").await;
    spawn_local(&mob, "observed").await;

    let expiry = Some(u64::MAX - 1);
    mob.grant(
        "p10",
        &[
            ControlScope::List,
            ControlScope::Cancel,
            ControlScope::ReadHistory,
        ],
        expiry,
    )
    .await;
    let p10 = mob.handle_as("p10");
    p10.member_status(&identity("observed"))
        .await
        .expect("List granted");

    // Partial revoke: Cancel goes, List stays, expiry retained (mirrors the
    // machine contract at multi_host_machine.rs grant-lifecycle lane).
    let removed = mob
        .handle
        .revoke_scopes(
            MobControlPrincipal::Owner,
            support::principal_id("p10"),
            Some(BTreeSet::from([ControlScope::Cancel])),
        )
        .await
        .expect("owner partial revoke");
    assert!(!removed, "partial revoke keeps the grant record");

    p10.member_status(&identity("observed"))
        .await
        .expect("List survives the partial revoke");
    expect_scope_denied(
        p10.force_cancel_member(identity("observed")).await,
        ControlScope::Cancel,
        &[ControlScope::List, ControlScope::ReadHistory],
    );
    let records = mob
        .handle
        .grants(MobControlPrincipal::Owner)
        .await
        .expect("owner grants read");
    let record = records
        .iter()
        .find(|record| record.principal == "p10")
        .expect("p10 grant record");
    assert_eq!(
        record.expires_at_ms, expiry,
        "partial revoke retains expiry"
    );
    assert_eq!(
        record.scopes,
        BTreeSet::from([ControlScope::List, ControlScope::ReadHistory]),
    );

    // Revoke the rest: default-deny restored; a second revoke is an
    // idempotent no-op (`removed: false`) — ADJ-P5-3.
    let removed = mob
        .handle
        .revoke_scopes(
            MobControlPrincipal::Owner,
            support::principal_id("p10"),
            None,
        )
        .await
        .expect("owner full revoke");
    assert!(removed, "full revoke removes the record");
    expect_scope_denied(
        p10.member_status(&identity("observed")).await,
        ControlScope::List,
        &[],
    );
    let removed_again = mob
        .handle
        .revoke_scopes(
            MobControlPrincipal::Owner,
            support::principal_id("p10"),
            None,
        )
        .await
        .expect("revoking an absent grant is a no-op, not an error");
    assert!(!removed_again, "absent-grant revoke reports removed: false");
}

// ───────────────────────── T-M11 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn expired_grant_denies_at_the_seam() {
    let mob = create_controlling_mob("scope-m11").await;
    spawn_local(&mob, "observed").await;

    // Expiry is data: the write path accepts a past expiry verbatim
    // (ADJ-P5-7); the enforcement seam makes it inert. No sleeping.
    mob.grant("expired-p", &[ControlScope::List], Some(1)).await;
    expect_scope_denied(
        mob.handle_as("expired-p")
            .member_status(&identity("observed"))
            .await,
        ControlScope::List,
        &[],
    );

    mob.grant("live-p", &[ControlScope::List], Some(u64::MAX - 1))
        .await;
    mob.handle_as("live-p")
        .member_status(&identity("observed"))
        .await
        .expect("a far-future expiry is live");
}

// ───────────────────────── T-M12 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn grant_durability_and_expired_stays_expired_across_restart() {
    let mob = create_controlling_mob("scope-m12").await;
    spawn_local(&mob, "observed").await;

    mob.grant("durable-p", &[ControlScope::List], None).await;
    mob.grant("expired-p", &[ControlScope::Cancel], Some(1))
        .await;

    mob.handle_as("durable-p")
        .member_status(&identity("observed"))
        .await
        .expect("pre-restart: durable grant enforces");
    expect_scope_denied(
        mob.handle_as("expired-p")
            .force_cancel_member(identity("observed"))
            .await,
        ControlScope::Cancel,
        &[],
    );

    let mob = mob.restart().await;

    // Raw records replay unchanged — both rows visible with verbatim expiry.
    let records = mob
        .handle
        .grants(MobControlPrincipal::Owner)
        .await
        .expect("post-restart grants read");
    let durable = records
        .iter()
        .find(|record| record.principal == "durable-p")
        .expect("durable grant survives restart");
    assert_eq!(durable.expires_at_ms, None);
    let expired = records
        .iter()
        .find(|record| record.principal == "expired-p")
        .expect("expired grant record replays raw (no expired flag, ADJ-P5-4)");
    assert_eq!(expired.expires_at_ms, Some(1), "expiry replays verbatim");

    // Enforcement: a restored mob's live grant still works; its expired
    // grant stays expired (§8:268).
    mob.handle_as("durable-p")
        .member_status(&identity("observed"))
        .await
        .expect("post-restart: durable grant still enforces");
    expect_scope_denied(
        mob.handle_as("expired-p")
            .force_cancel_member(identity("observed"))
            .await,
        ControlScope::Cancel,
        &[],
    );
}

// ───────────────────────── T-M13 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn rotation_does_not_clear_grants() {
    let mob = create_controlling_mob("scope-m13").await;
    spawn_local(&mob, "observed").await;

    mob.grant("rot-p", &[ControlScope::List], None).await;
    mob.handle
        .rotate_supervisor()
        .await
        .expect("supervisor rotation on the controlling fixture");

    // Grants are principal→mob facts, not epoch-scoped (§8:268).
    let records = mob
        .handle
        .grants(MobControlPrincipal::Owner)
        .await
        .expect("grants read after rotation");
    assert!(
        records.iter().any(|record| record.principal == "rot-p"),
        "rotation must not clear grant records"
    );
    mob.handle_as("rot-p")
        .member_status(&identity("observed"))
        .await
        .expect("the granted verb still passes after rotation");
}

// ───────────────────────── T-M14 ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn unresolvable_principal_fails_closed_at_the_boundary() {
    // Surface parse: empty / control-character principal strings are rejected
    // by the ONE validated boundary type before any command is constructed.
    assert!(meerkat_core::auth::PrincipalId::new("").is_err());
    assert!(meerkat_core::auth::PrincipalId::new("   ").is_err());
    assert!(meerkat_core::auth::PrincipalId::new("evil\u{0007}bell").is_err());

    // An Unresolved principal resolves to zero scopes, always: every verb
    // denies with the empty presented set.
    let mob = create_controlling_mob("scope-m14").await;
    let unresolved =
        mob.handle
            .clone()
            .with_command_authority(meerkat_mob::CommandAuthority::principal(
                MobControlPrincipal::Unresolved,
            ));
    expect_scope_denied(
        unresolved.retire(identity("nobody")).await,
        ControlScope::Retire,
        &[],
    );
    expect_scope_denied(
        unresolved.events().poll(0, 8).await.map(|_| ()),
        ControlScope::SubscribeEvents,
        &[],
    );
}

// ───────────────────────── T-M15 ─────────────────────────

#[test]
fn scope_denied_wire_detail_is_snake_case_and_ordered() {
    use meerkat_contracts::wire::{WireControlScope, WireScopeDeniedDetail};

    let detail = WireScopeDeniedDetail {
        required: WireControlScope::WireTopology,
        presented: vec![WireControlScope::List, WireControlScope::SendCommand],
    };
    let value = serde_json::to_value(&detail).expect("detail serializes");
    assert_eq!(
        value,
        serde_json::json!({
            "required": "wire_topology",
            "presented": ["list", "send_command"],
        }),
        "snake_case scope names; presented preserves ControlScope order"
    );

    let decoded: WireScopeDeniedDetail = serde_json::from_value(value).expect("detail round-trips");
    assert_eq!(decoded, detail);

    // deny_unknown_fields: an extra key is a decode reject, not a silent drop.
    assert!(
        serde_json::from_value::<WireScopeDeniedDetail>(serde_json::json!({
            "required": "list",
            "presented": [],
            "expired": true,
        }))
        .is_err(),
        "unknown fields are rejected (ADJ-P5-16)"
    );
}

// ─────────── conversion pin: the denial and its wire detail agree ───────────

#[test]
fn scope_denial_converts_to_wire_detail_ordered() {
    use meerkat_contracts::wire::{WireControlScope, WireScopeDeniedDetail};

    let denial = ScopeDenial {
        required: ControlScope::Retire,
        presented: BTreeSet::from([ControlScope::WireTopology, ControlScope::List]),
    };
    let detail = WireScopeDeniedDetail::from(&denial);
    assert_eq!(detail.required, WireControlScope::Retire);
    assert_eq!(
        detail.presented,
        vec![WireControlScope::List, WireControlScope::WireTopology],
        "wire Vec is BTreeSet-ordered (deterministic)"
    );
}

// ───────────────────────── T-M16 ─────────────────────────

/// Destroy scrubs the grant family (DEC-P5P-7 step 3): grant rows are
/// principal-keyed — not member/host-lifecycle tied — so the destroy sweep
/// is their ONLY cleaner. A destroyed mob must leave zero grant records in
/// the runtime-metadata store (leaked rows would re-apply through
/// `recover_operator_grants` if a destroyed store were ever re-opened).
#[tokio::test(flavor = "multi_thread")]
async fn destroy_scrubs_grant_records_from_the_metadata_store() {
    let mob = create_controlling_mob("scope-m16").await;

    mob.grant("scrubbed-p", &[ControlScope::List], None).await;
    mob.grant("scrubbed-q", &[ControlScope::Cancel], Some(1))
        .await;
    let before = mob
        .storage_metadata
        .list_mob_operator_grants(&mob.mob_id)
        .await
        .expect("pre-destroy grant rows read");
    assert_eq!(before.len(), 2, "both grant rows persisted before destroy");

    mob.handle.destroy().await.expect("destroy completes clean");

    let after = mob
        .storage_metadata
        .list_mob_operator_grants(&mob.mob_id)
        .await
        .expect("post-destroy grant rows read");
    assert!(
        after.is_empty(),
        "destroy must scrub every grant record (found {})",
        after.len()
    );
}
