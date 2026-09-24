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
