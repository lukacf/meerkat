//! Multi-host mobs phase 3 — the RESPAWN VERB on a PLACED member (ADJ-24:
//! placed respawn = re-materialization, realized by `handle_respawn_placed`):
//! respawn admission preview → retire with the FULL remote release → kickoff
//! clear → replacement through the ordinary remote spawn-exec ladder,
//! re-materializing at the replacement tuple (generation bump, SAME
//! identity) → receipt from the committed roster entry.
//!
//! Placement immutability rides the same verb: a spawn customizer can
//! neither RE-PLACE a placed member mid-respawn nor pin it to an explicit
//! runtime binding — both fail typed BEFORE the retire, leaving the live
//! incarnation untouched.
//!
//! Assertions are on PUBLIC machine-fact carriers (the host's durable
//! binding record, the member service census, the roster projection),
//! never on actor internals.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;

use meerkat_core::HandlingMode;
use meerkat_mob::AgentIdentity;
use meerkat_mob::runtime::{
    SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec, SpawnSource,
};
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, create_controlling_mob,
    create_controlling_mob_with_customizer, spawn_host_daemon_fixture,
};

// ===========================================================================
// ADJ-24 — respawn verb on a placed member: full remote release, then the
// replacement re-materializes at the replacement tuple (generation bump,
// same identity); the receipt names the committed roster entry
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn respawn_verb_rematerializes_placed_member_at_replacement_tuple() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("respawn-verb-host").with_member_build(),
    )
    .await
    .expect("member-build fixture");
    let controlling = create_controlling_mob("respawn-verb").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");
    let before = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("pre-respawn materialized row");
    let old_session = meerkat_core::SessionId::parse(&before.session_id).expect("old session id");

    // The verb under test.
    let receipt = controlling
        .handle
        .respawn(AgentIdentity::from("b2"), None)
        .await
        .expect("respawn verb re-materializes the placed member");
    assert_eq!(
        receipt.identity,
        AgentIdentity::from("b2"),
        "the receipt names the SAME identity"
    );

    // Host-side machine facts: exactly one materialized row for b2, at a
    // REPLACEMENT tuple — generation bumped, tuple strictly above the old
    // incarnation's — naming a FRESH session that exists on the member
    // service, while the retired incarnation's session is fully released
    // (no live runtime survives the remote release).
    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    assert_eq!(
        record.materialized.len(),
        1,
        "exactly one materialized incarnation per identity"
    );
    let after = record
        .materialized
        .get("b2")
        .expect("post-respawn materialized row");
    assert!(
        after.generation > before.generation,
        "the replacement bumps the generation: {} !> {}",
        after.generation,
        before.generation
    );
    assert!(
        (after.generation, after.fence_token) > (before.generation, before.fence_token),
        "the replacement tuple sorts strictly above the retired tuple: {:?} !> {:?}",
        (after.generation, after.fence_token),
        (before.generation, before.fence_token)
    );
    assert_ne!(
        after.session_id, before.session_id,
        "the replacement is a FRESH materialization, never a rebind of the released session"
    );
    assert!(
        fixture.member_session_exists(&after.session_id).await,
        "the replacement session exists on the member host"
    );
    assert!(
        !fixture
            .member_session_service()
            .has_live_session(&old_session)
            .await
            .unwrap_or(true),
        "no live runtime survives the retired incarnation's remote release"
    );

    // Controlling-side: the roster lists b2 exactly once (the committed
    // replacement the receipt was built from), and the entry is USABLE —
    // delivery reaches the replacement on the member host.
    let members = controlling.handle.list_members().await;
    assert_eq!(
        members
            .iter()
            .filter(|entry| entry.agent_identity == "b2")
            .count(),
        1,
        "exactly one committed roster entry for the replacement"
    );
    let member = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("replacement member handle");
    member
        .send("post-respawn turn", HandlingMode::Queue)
        .await
        .expect("the replacement serves through its host");

    fixture.shutdown().await;
}

// ===========================================================================
// Placement immutability — a customizer can neither re-place nor bind a
// placed member mid-respawn; both fail typed BEFORE the retire, leaving the
// live incarnation untouched
// ===========================================================================

/// Customizer that tampers with a placed respawn per the selected mode.
struct TamperingCustomizer {
    mode: std::sync::Mutex<Tamper>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tamper {
    /// Leave specs alone (spawn setup must succeed untampered).
    None,
    /// Re-place the member onto a different host id.
    Replace,
    /// Pin the member to an explicit runtime binding.
    Bind,
}

impl SpawnMemberCustomizer for TamperingCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        if ctx.spawn_source != SpawnSource::Respawn {
            return Ok(());
        }
        match *self.mode.lock().expect("tamper mode lock") {
            Tamper::None => {}
            Tamper::Replace => {
                spec.placement = Some(meerkat_mob::machines::mob_machine::HostId(
                    "1c0ffee0-0000-5000-8000-000000000000".to_string(),
                ));
            }
            Tamper::Bind => {
                spec.binding = Some(meerkat_mob::RuntimeBinding::Session);
            }
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn respawn_customizer_cannot_replace_or_bind_a_placed_member() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("respawn-immut-host").with_member_build(),
    )
    .await
    .expect("member-build fixture");
    let customizer = Arc::new(TamperingCustomizer {
        mode: std::sync::Mutex::new(Tamper::None),
    });
    let controlling =
        create_controlling_mob_with_customizer("respawn-immut", Some(customizer.clone())).await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");
    let before = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("materialized row");

    for tamper in [Tamper::Replace, Tamper::Bind] {
        *customizer.mode.lock().expect("tamper mode lock") = tamper;
        let error = controlling
            .handle
            .respawn(AgentIdentity::from("b2"), None)
            .await
            .expect_err("a tampering customizer must fail the respawn typed");
        assert!(
            !format!("{error:?}").is_empty(),
            "placement/binding tampering surfaces typed"
        );

        // The guard fires BEFORE the retire: the live incarnation is
        // untouched — same materialized row on the host, same live session,
        // roster intact.
        let row = fixture
            .host_binding_record(controlling.mob_id.as_ref())
            .await
            .materialized
            .get("b2")
            .cloned()
            .expect("the incarnation survives the refused respawn");
        assert_eq!(
            (row.generation, row.fence_token, row.session_id.clone()),
            (
                before.generation,
                before.fence_token,
                before.session_id.clone()
            ),
            "the refused respawn mutates nothing host-side"
        );
        assert!(
            fixture
                .member_session_service()
                .has_live_session(
                    &meerkat_core::SessionId::parse(&row.session_id).expect("session id"),
                )
                .await
                .unwrap_or(false),
            "the live incarnation keeps serving after the refused respawn"
        );
        assert!(
            controlling
                .handle
                .list_members()
                .await
                .iter()
                .any(|entry| entry.agent_identity == "b2"),
            "the roster entry survives the refused respawn"
        );
    }

    // Untampered respawn through the SAME customizer converges (the seam
    // itself is not the gate — the tampering is).
    *customizer.mode.lock().expect("tamper mode lock") = Tamper::None;
    controlling
        .handle
        .respawn(AgentIdentity::from("b2"), None)
        .await
        .expect("an untampered respawn through the customizer seam converges");

    fixture.shutdown().await;
}

// ===========================================================================
// Placed profile check on respawn: the successor's resolved profile is
// checked before anything is retired, so a refused respawn keeps the live
// incarnation
// ===========================================================================

/// Admits every placed profile until it is told to refuse, recording the
/// source of each check.
struct RespawnProfileJudge {
    refuse: std::sync::atomic::AtomicBool,
    seen: std::sync::Mutex<Vec<SpawnSource>>,
}

impl SpawnMemberCustomizer for RespawnProfileJudge {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        _spec: &mut SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        Ok(())
    }

    fn check_placed_profile(
        &self,
        ctx: &SpawnCustomizationContext,
        _profile: &meerkat_mob::Profile,
    ) -> Result<(), meerkat_mob::MobError> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ctx.spawn_source);
        if self.refuse.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(meerkat_mob::MobError::WiringError(
                "refused by the respawn judge".to_string(),
            ));
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_placed_respawn_keeps_the_member() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("respawn-profile-host").with_member_build(),
    )
    .await
    .expect("member-build fixture");
    let judge = Arc::new(RespawnProfileJudge {
        refuse: std::sync::atomic::AtomicBool::new(false),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let controlling =
        create_controlling_mob_with_customizer("respawn-profile", Some(judge.clone())).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");
    let before = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("materialized row");

    judge
        .refuse
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = controlling
        .handle
        .respawn(AgentIdentity::from("b2"), None)
        .await
        .expect_err("the judge refuses the placed respawn");

    let seen = judge
        .seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        seen.last(),
        Some(&SpawnSource::Respawn),
        "the respawn asks with its source: {seen:?}"
    );
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("refused by the respawn judge") && !rendered.contains("SpawnAfterRetire"),
        "the judge's refusal reaches the caller before any retirement: {error:?}"
    );
    assert!(
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b2"),
        "the roster entry survives the refused respawn"
    );
    let after = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("the host row survives the refused respawn");
    assert_eq!(
        (
            after.generation,
            after.fence_token,
            after.session_id.clone()
        ),
        (
            before.generation,
            before.fence_token,
            before.session_id.clone()
        ),
        "the live incarnation is kept"
    );
    assert!(
        fixture
            .member_session_service()
            .has_live_session(&meerkat_core::SessionId::parse(&after.session_id).expect("session"))
            .await
            .unwrap_or(false),
        "and keeps serving"
    );

    fixture.shutdown().await;
}

/// Admits the first check of each placed respawn, the one before the
/// retirement, and refuses the second, the placed lane's own check of the
/// successor.
struct SecondRespawnCheckJudge {
    respawn_checks: std::sync::atomic::AtomicUsize,
}

impl SpawnMemberCustomizer for SecondRespawnCheckJudge {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        _spec: &mut SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        Ok(())
    }

    fn check_placed_profile(
        &self,
        ctx: &SpawnCustomizationContext,
        _profile: &meerkat_mob::Profile,
    ) -> Result<(), meerkat_mob::MobError> {
        if ctx.spawn_source == SpawnSource::Respawn
            && self
                .respawn_checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 1
        {
            return Err(meerkat_mob::MobError::WiringError(
                "refused by the lane's own check".to_string(),
            ));
        }
        Ok(())
    }
}

/// The placed lane checks a respawn's successor again after the retirement:
/// a refusal there ends the respawn with the refusal as its cause, and no
/// successor runs on the host.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_respawn_checks_its_successor_again_in_the_lane() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("respawn-lane-check-host").with_member_build(),
    )
    .await
    .expect("member-build fixture");
    let judge = Arc::new(SecondRespawnCheckJudge {
        respawn_checks: std::sync::atomic::AtomicUsize::new(0),
    });
    let controlling =
        create_controlling_mob_with_customizer("respawn-lane-check", Some(judge.clone())).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");

    let error = controlling
        .handle
        .respawn(AgentIdentity::from("b2"), None)
        .await
        .expect_err("the lane's own check refuses the successor");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("SpawnAfterRetireWithCause")
            && rendered.contains("refused by the lane's own check"),
        "the lane's refusal is the respawn's cause: {error:?}"
    );
    assert_eq!(
        judge
            .respawn_checks
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the respawn checked its successor before the retirement and again in the lane"
    );
    assert!(
        !controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b2"),
        "no successor joined the roster"
    );
    assert!(
        fixture
            .host_binding_record(controlling.mob_id.as_ref())
            .await
            .materialized
            .get("b2")
            .is_none(),
        "no successor runs on the host"
    );

    fixture.shutdown().await;
}
