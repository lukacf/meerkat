//! The public MCP doors an identity entry is reached through: the keyed
//! member send asks the same external admission as the unkeyed one, and the
//! door that appends to a member's session as its host reaches an entry only
//! with the `AdminHost` control scope.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use serde_json::Value;
use std::collections::BTreeSet;

/// The owner's console state over an ephemeral runtime, with its continuation
/// services bound (nothing drains them), and a mob that holds the identity
/// entry `entry-1` and the worker `worker-1`, both on a profile that admits
/// external work.
struct EntryMob {
    state: Arc<MobMcpState>,
    mob_id: MobId,
}

impl EntryMob {
    async fn new(mob: &str) -> Self {
        let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let state = MobMcpState::from_acquired_runtime_adapter(
            Arc::new(crate::LocalSessionService::new_over_runtime_for_tests(
                Arc::clone(&runtime),
            )),
            Some(runtime),
            MobControlPrincipal::Owner,
        )
        .into_shared();
        let inbox = meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
            meerkat_runtime::InMemoryRuntimeStore::new(),
        ));
        state
            .bind_continuations(inbox, &meerkat::ContinuationHostBindings::default())
            .expect("bind the state's continuations");
        crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": mob,
                    "profiles": {
                        "worker": {
                            "model": "gpt-5.4",
                            "external_addressable": true,
                            "tools": { "comms": true }
                        }
                    }
                }
            }),
        )
        .await
        .expect("create the mob");
        let mob_id = MobId::from(mob);
        let mut entry = SpawnMemberSpec::new("worker", AgentIdentity::from("entry-1"));
        entry.runtime_mode = Some(MobRuntimeMode::TurnDriven);
        state
            .mob_spawn_spec(&mob_id, entry.with_entry_custody(true))
            .await
            .expect("the host seats its identity entry");
        crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_mob_spawn",
            &json!({
                "mob_id": mob,
                "profile": "worker",
                "agent_identity": "worker-1",
                "runtime_mode": "turn_driven"
            }),
        )
        .await
        .expect("spawn the worker");
        Self { state, mob_id }
    }

    /// A console serving the principal `principal`, which holds exactly
    /// `scopes` on the mob.
    async fn scoped(&self, principal: &str, scopes: &[ControlScope]) -> Arc<MobMcpState> {
        let id = meerkat_core::auth::PrincipalId::new(principal).expect("principal id");
        let handle = self.state.handle_for(&self.mob_id).await.expect("handle");
        handle
            .grant_scopes(
                MobControlPrincipal::Owner,
                id.clone(),
                scopes.iter().copied().collect::<BTreeSet<_>>(),
                None,
            )
            .await
            .expect("the owner grants the scopes");
        let scoped = MobMcpState::new_in_memory_as(MobControlPrincipal::External(id));
        scoped.mob_insert_handle(self.mob_id.clone(), handle).await;
        scoped
    }

    async fn keyed_status(&self, member: &str, key: &str) -> meerkat::ContinuationStatus {
        self.state
            .continuation_service()
            .expect("bound")
            .continuation_status(
                &meerkat::ContinuationOwner::Member {
                    mob_id: self.mob_id.to_string(),
                    identity: member.to_string(),
                },
                &meerkat::ContinuationKey::new(format!(
                    "{MEMBER_SEND_CONTINUATION_KEY_PREFIX}{key}"
                ))
                .expect("key"),
            )
            .await
            .expect("status")
    }
}

fn send_args(mob: &MobId, member: &str, content: &str, key: Option<&str>) -> Value {
    let mut args = json!({
        "mob_id": mob.as_str(),
        "agent_identity": member,
        "content": content,
    });
    if let Some(key) = key {
        args["idempotency_key"] = json!(key);
    }
    args
}

fn append_args(mob: &MobId, member: &str, key: &str) -> Value {
    json!({
        "mob_id": mob.as_str(),
        "agent_identity": member,
        "text": "door context",
        "source": "mob",
        "idempotency_key": key,
    })
}

fn assert_identity_member(result: &Result<Value, McpToolError>, member: &str, context: &str) {
    let error = match result {
        Err(error) => error,
        Ok(value) => panic!("{context}: refused as an identity member, got {value}"),
    };
    assert_eq!(
        error.data,
        Some(json!({
            "kind": "identity_member",
            "member_id": member,
            "retryable": false
        })),
        "{context}: {error:?}"
    );
}

/// The door passed its admission: whatever the call answers after it, it is
/// neither an identity member refusal nor a scope denial.
fn assert_passed_the_door(result: &Result<Value, McpToolError>, context: &str) {
    if let Err(error) = result {
        assert_ne!(
            error.data.as_ref().map(|data| data["kind"].clone()),
            Some(json!("identity_member")),
            "{context}: {error:?}"
        );
        assert_ne!(
            error.code,
            meerkat_contracts::ErrorCode::ScopeDenied.jsonrpc_code(),
            "{context}: {error:?}"
        );
    }
}

/// The public keyed member send asks the same external admission as the
/// unkeyed one: to an identity entry it is refused `identity_member`, with
/// exactly the unkeyed send's answer, and nothing is committed under its key.
#[tokio::test]
async fn a_keyed_public_member_send_to_an_identity_entry_is_refused_like_the_unkeyed_send() {
    let mob = EntryMob::new("public-keyed-entry").await;
    let unkeyed = crate::public_mcp::handle_public_tools_call(
        &mob.state,
        "meerkat_mob_member_send",
        &send_args(&mob.mob_id, "entry-1", "unkeyed", None),
    )
    .await;
    assert_identity_member(&unkeyed, "entry-1", "the unkeyed send");
    let keyed = crate::public_mcp::handle_public_tools_call(
        &mob.state,
        "meerkat_mob_member_send",
        &send_args(&mob.mob_id, "entry-1", "keyed", Some("entry-key")),
    )
    .await;
    let (keyed, unkeyed) = (
        keyed.expect_err("the keyed send to an entry is refused"),
        unkeyed.expect_err("refused"),
    );
    assert_eq!(
        (keyed.code, &keyed.message, &keyed.data),
        (unkeyed.code, &unkeyed.message, &unkeyed.data),
        "the keyed send answers exactly as the unkeyed send"
    );
    assert_eq!(
        mob.keyed_status("entry-1", "entry-key").await,
        meerkat::ContinuationStatus::NotCommitted,
        "nothing is committed under the key"
    );
}

/// The public `meerkat_mob_append_system_context` appends to the member's
/// session as its host: on an identity entry a SendCommand principal without
/// `AdminHost` is refused `identity_member` before anything is appended, a
/// principal that holds `AdminHost` passes the door, the owner appends, and
/// on a worker the door is unchanged.
#[tokio::test]
async fn the_public_append_system_context_reaches_an_identity_entry_only_with_admin_host() {
    let mob = EntryMob::new("public-append-entry").await;
    let scoped = mob
        .scoped("append-operator", &[ControlScope::SendCommand])
        .await;
    let append = |state: &Arc<MobMcpState>, member: &str, key: &str| {
        let state = Arc::clone(state);
        let args = append_args(&mob.mob_id, member, key);
        async move {
            crate::public_mcp::handle_public_tools_call(
                &state,
                "meerkat_mob_append_system_context",
                &args,
            )
            .await
        }
    };
    assert_identity_member(
        &append(&scoped, "entry-1", "scoped-entry").await,
        "entry-1",
        "a SendCommand principal appends nothing to an entry",
    );
    assert_passed_the_door(
        &append(&scoped, "worker-1", "scoped-worker").await,
        "the door is unchanged for a worker",
    );
    let host = mob
        .scoped(
            "append-host",
            &[ControlScope::SendCommand, ControlScope::AdminHost],
        )
        .await;
    assert_passed_the_door(
        &append(&host, "entry-1", "host-entry").await,
        "AdminHost reaches the entry",
    );
    let owner = append(&mob.state, "entry-1", "owner-entry")
        .await
        .expect("the owner appends to its entry");
    assert_eq!(owner["status"], "applied", "{owner}");
}

/// The host's own keyed send reaches its identity entry as internal work:
/// it commits where the external keyed door refuses the same entry, and a
/// console principal without `AdminHost` is refused `identity_member` at the
/// host door before anything is committed.
#[tokio::test]
async fn the_hosts_keyed_send_reaches_its_identity_entry_as_internal_work() {
    let mob = EntryMob::new("host-keyed-entry").await;
    let entry = AgentIdentity::from("entry-1");
    let external = mob
        .state
        .mob_member_send_keyed(
            &mob.mob_id,
            entry.clone(),
            ContentInput::from("external"),
            HandlingMode::Queue,
            "external-key",
        )
        .await
        .expect_err("the external keyed door refuses an entry");
    assert!(
        matches!(&external, MobError::IdentityMember { member_id } if member_id == &entry),
        "{external:?}"
    );

    let receipt = mob
        .state
        .mob_member_send_keyed_as_host(
            &mob.mob_id,
            entry.clone(),
            ContentInput::from("host"),
            HandlingMode::Queue,
            "host-key",
        )
        .await
        .expect("the host's keyed send to its entry commits");
    assert_eq!(
        receipt.status,
        meerkat_contracts::WireContinuationStatus::Pending,
        "nothing drains it here"
    );
    assert!(
        matches!(
            mob.keyed_status("entry-1", "host-key").await,
            meerkat::ContinuationStatus::Pending { .. }
        ),
        "the host's send is committed for its entry"
    );

    let scoped = mob
        .scoped("host-operator", &[ControlScope::SendCommand])
        .await;
    let refused = scoped
        .mob_member_send_keyed_as_host(
            &mob.mob_id,
            entry.clone(),
            ContentInput::from("scoped"),
            HandlingMode::Queue,
            "scoped-key",
        )
        .await
        .expect_err("a principal without AdminHost is refused at the host door");
    assert!(
        matches!(&refused, MobError::IdentityMember { member_id } if member_id == &entry),
        "{refused:?}"
    );
    assert_eq!(
        mob.keyed_status("entry-1", "scoped-key").await,
        meerkat::ContinuationStatus::NotCommitted,
        "nothing is committed for the refused send"
    );
}
