//! A scoped tool grant to a member identity is decided against the mob's
//! live roster when it commits ([`MobHandle::grant_member_tool`]): the mob's
//! actor commits it in its command order with the retirements that end an
//! identity's grants, so a grant never outlives a retirement whose roster
//! read came before it.

use super::*;
use crate::runtime::{MemberToolGrant, MemberToolGrantError};
use meerkat_core::tool_grant::{ToolGrantIssuer, ToolGrantSubject, ToolGrantTarget};
use meerkat_core::types::ToolName;
use meerkat_runtime::tool_grant::{ToolGrantStatus, ToolGrantWindow};

/// Bounds a hang only: a passing row never waits for it.
const STEP: Duration = Duration::from_secs(30);

fn shell_grant() -> MemberToolGrant {
    MemberToolGrant {
        target: ToolGrantTarget::Tool {
            name: ToolName::from("shell"),
        },
        issuer: ToolGrantIssuer::operator(),
        reason: Some("maintenance".to_string()),
        window: ToolGrantWindow::For(Duration::from_mins(10)),
    }
}

async fn seat(handle: &MobHandle, identity: &AgentIdentity) -> SessionId {
    tokio::time::timeout(
        STEP,
        handle.spawn(ProfileName::from("lead"), identity.clone(), None),
    )
    .await
    .expect("the spawn answers within the bound")
    .expect("the member spawns")
    .bridge_session_id()
    .expect("a session-backed member")
    .clone()
}

async fn retire(handle: &MobHandle, identity: &AgentIdentity) {
    tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("the retirement answers within the bound")
        .expect("the member retires");
}

/// Both orders of a grant and a retirement of the same identity. Retire
/// first, after a gate read that found the member: the commit is refused as
/// an unknown member, the refusal the read gives one, and no grant exists.
/// Grant first, then retire: the retirement's end revokes the grant, and the
/// identity seated again in the same incarnation finds no active grant.
#[tokio::test]
async fn a_member_grant_never_outlives_a_retirement() {
    let (handle, _service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;
    let runtime = handle
        .runtime_adapter
        .clone()
        .expect("a runtime-backed mob holds grants");
    let worker = AgentIdentity::from("worker-1");
    let subject = ToolGrantSubject::member(
        handle.definition.id.as_str(),
        &handle.live_incarnation(),
        worker.as_str(),
    );

    seat(&handle, &worker).await;
    handle
        .member_grant_gate(&worker)
        .await
        .expect("the gate read finds the member");
    retire(&handle, &worker).await;
    let refused =
        tokio::time::timeout(STEP, handle.grant_member_tool(&worker, shell_grant(), None))
            .await
            .expect("the grant answers within the bound")
            .expect_err("a grant committed after the retirement is refused");
    assert!(
        matches!(
            &refused,
            MemberToolGrantError::Mob(MobError::MemberNotFound(identity)) if identity == &worker
        ),
        "the refusal is the unknown member's: {refused:?}"
    );
    assert!(
        runtime
            .list_tool_grants(&subject, None)
            .await
            .expect("the grants read")
            .grants
            .is_empty(),
        "a refused grant leaves no row"
    );

    seat(&handle, &worker).await;
    let issued = tokio::time::timeout(STEP, handle.grant_member_tool(&worker, shell_grant(), None))
        .await
        .expect("the grant answers within the bound")
        .expect("a seated identity is granted");
    retire(&handle, &worker).await;
    seat(&handle, &worker).await;
    let listed = runtime
        .list_tool_grants(&subject, None)
        .await
        .expect("the grants read")
        .grants;
    assert!(
        listed
            .iter()
            .all(|grant| grant.status != ToolGrantStatus::Active),
        "the retirement ended the grant: {listed:?}"
    );
    assert!(
        listed.iter().any(|grant| {
            grant.record.grant_id == issued.record.grant_id
                && grant.status == ToolGrantStatus::Revoked
        }),
        "{listed:?}"
    );
}

/// A grant through the handle to a seated identity, with the session that
/// serves it, completes within the bound and arms that session's deadline
/// driver for the grant's deadline: the commit inside the mob's actor waits
/// on the runtime only, never on the actor or a handle of it.
#[tokio::test]
async fn a_member_grant_with_a_deadline_completes_and_arms_the_serving_session() {
    let (handle, _service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;
    let runtime = handle
        .runtime_adapter
        .clone()
        .expect("a runtime-backed mob holds grants");
    let worker = AgentIdentity::from("worker-1");
    let session_id = seat(&handle, &worker).await;

    let issued = tokio::time::timeout(
        STEP,
        handle.grant_member_tool(&worker, shell_grant(), Some(&session_id)),
    )
    .await
    .expect("a grant through the handle completes")
    .expect("a seated identity is granted");
    assert_eq!(issued.status, ToolGrantStatus::Active);
    assert_eq!(
        runtime
            .tool_grant_deadlines()
            .armed_deadline(&session_id, &issued.record.subject.key()),
        Some(issued.expires_at_ms),
        "the serving session's deadline driver is armed for the grant"
    );
}
