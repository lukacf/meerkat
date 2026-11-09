#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Identities created at runtime, served by a Meta gateway's identity
//! runtime.
//!
//! An identity created at runtime is a created entry of Core's mob: its
//! record is its row in the mob's identity store (store-owned), not the
//! host's roster. The roster pass registers each one from that row, and a
//! door that names one this runtime has not registered yet registers it from
//! its row first, so a send by name, a dispatch and a schedule fire go
//! through the identity's door, as for any registered identity. Core's
//! MobMachine drives a store-owned identity's lifecycle, so the identity's
//! door refuses Meta's lifecycle verbs on it, typed and final, and changes
//! nothing. A name Core holds for no identity, a removed identity's name
//! included, stays unknown.
//!
//! Each row boots the gateway's runtime composition on a temporary state
//! directory (persistent stores, the state directory's identity substrate, a
//! roster provider that lists one declared identity) with a model that
//! records what each member's turn was asked, and creates its identities
//! through Core's store-owned create.

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, MutableRosterProvider, RosterContext,
    RosterError,
};
use meerkat_mob::{EntryStanding, IdentityEntryRead, MobDefinition, ProfileName};
use serde_json::{Value, json};
use tokio::sync::mpsc;

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// The identity the host's roster declares.
const DECLARED: &str = "host-a";
/// The identity each row creates at runtime.
const CREATED: &str = "keeper";
/// Bounds a failure only; a passing row never waits for it.
const TURN_BOUND: Duration = Duration::from_secs(90);

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("parse the definition")
}

fn fresh_mob_id() -> String {
    format!(
        "meta-created-identities-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn declared_spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(DECLARED).unwrap(),
        profile: ProfileName::from("worker"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// The host's roster: the declared identity alone. It never lists an
/// identity created at runtime.
struct DeclaredRoster;

#[async_trait]
impl RosterProvider for DeclaredRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![declared_spec()])
    }
}

/// The members' model: each request's last message (past Core's system
/// notices) is sent on the row's channel, and every request answers.
#[derive(Clone)]
struct RecordingClient {
    asked: mpsc::UnboundedSender<String>,
}

impl meerkat_client::LlmClient for RecordingClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        if let Some(last) = request
            .messages
            .iter()
            .rev()
            .find(|message| !matches!(message, Message::SystemNotice(_)))
        {
            let _ = self
                .asked
                .send(serde_json::to_string(last).unwrap_or_default());
        }
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "noted".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// One booted gateway runtime and what its members' model was asked.
struct Rig {
    runtime: meerkat_meta::UnifiedRuntime,
    asked: mpsc::UnboundedReceiver<String>,
}

/// Boot the gateway's runtime composition on `state` for the mob `mob_id`.
async fn boot(state: &Path, mob_id: &str) -> Rig {
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition(mob_id))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(DeclaredRoster))
        .identity_runtime_instance_id("meta-created-identities")
        .default_llm_client(Arc::new(RecordingClient { asked: asked_tx }));
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    Rig { runtime, asked }
}

fn operation(name: &str) -> meerkat_mob::IdentityOperation {
    meerkat_mob::IdentityOperation::new(
        name,
        meerkat_core::approval::decision::Sha256Digest::of(name.as_bytes()).as_str(),
    )
}

fn allow() -> meerkat_mob::IdentityChangeConsentInput<'static> {
    meerkat_mob::IdentityChangeConsentInput::Policy(meerkat_mob::IdentityPolicyOutcome::Allow)
}

/// The label every identity a row creates carries in its row's spec.
fn created_labels() -> BTreeMap<String, String> {
    BTreeMap::from([("custody".to_string(), "store".to_string())])
}

impl Rig {
    /// One JSON-RPC call through the dispatcher the stdio gateway serves.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: self
                .runtime
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: Arc::new(MutableRosterProvider::new(vec![declared_spec()])),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let raw = meerkat_meta::handle_unified_rpc_json(
            &self.runtime,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string(),
            Duration::from_mins(1),
            None,
            Some(&ctx),
        )
        .await;
        serde_json::from_str(&raw).expect("json-rpc response")
    }

    /// Create `name` at runtime through Core's store-owned create: Core seats
    /// it Dormant, and its row in the mob's identity store holds its spec.
    async fn create(&self, name: &str) {
        let created = self
            .runtime
            .mob_handle()
            .create_store_owned_identities(
                meerkat_mob::StoreOwnedIdentityCreate::new(
                    operation(&format!("create-{name}")),
                    meerkat_mob::IdentityChangeRequesterInput::host("console"),
                    vec![meerkat_mob::CreatedIdentityRequest::new(
                        meerkat_mob::CreatedIdentitySpec::new(name, "worker")
                            .with_display_name(format!("Created {name}"))
                            .with_labels(created_labels()),
                    )],
                ),
                allow(),
            )
            .await
            .map(|created| created.change);
        assert!(
            matches!(
                created,
                Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
            ),
            "Core creates {name}: {created:?}"
        );
        match self
            .runtime
            .mob_handle()
            .identity_entry(&meerkat_mob::AgentIdentity::from(name))
        {
            Ok(IdentityEntryRead::Entry(entry)) => assert_eq!(
                entry.origin,
                meerkat_mob::IdentityEntryOrigin::Created,
                "{name} is a created entry"
            ),
            other => panic!("{name} is an identity entry: {other:?}"),
        }
    }

    /// Remove `name` through Core's removal: Core retires its member and
    /// tombstones its name.
    async fn remove(&self, name: &str) {
        let removed = self
            .runtime
            .mob_handle()
            .remove_identity(
                meerkat_mob::IdentityRemoval::new(
                    operation(&format!("remove-{name}")),
                    meerkat_mob::IdentityChangeRequesterInput::host("console"),
                    name,
                ),
                allow(),
            )
            .await;
        assert!(
            matches!(
                removed,
                Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
            ),
            "Core removes {name}: {removed:?}"
        );
    }

    /// Whether the identity runtime has registered `name`. A plain read: it
    /// registers nothing.
    async fn registered(&self, name: &str) -> bool {
        self.runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .contains(&AgentIdentity::parse(name).unwrap())
            .await
    }

    /// Run the roster pass a host's refresh runs.
    async fn roster_pass(&self) {
        self.runtime
            .identity_first_context()
            .expect("identity-first context installed")
            .refresh_desired_topology()
            .await
            .expect("the roster pass runs");
    }

    /// Whether Core holds `name`'s member seated.
    fn seated(&self, name: &str) -> bool {
        matches!(
            self.runtime
                .mob_handle()
                .identity_entry(&meerkat_mob::AgentIdentity::from(name)),
            Ok(IdentityEntryRead::Entry(entry)) if matches!(entry.standing, EntryStanding::Seated(_))
        )
    }

    /// Wait until a member's turn is asked `marker`; `false` when none is
    /// within the bound.
    async fn asked(&mut self, marker: &str) -> bool {
        tokio::time::timeout(TURN_BOUND, async {
            while let Some(asked) = self.asked.recv().await {
                if asked.contains(marker) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }

    /// Send `marker` to `identity` by name through `meta/send`, and answer
    /// the response.
    async fn send(&self, identity: &str, marker: &str) -> Value {
        self.rpc(
            "meta/send",
            json!({"identity": identity, "content": marker}),
        )
        .await
    }

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }

    /// Shut down before a reopen over the same state directory in this
    /// process: the shutdown stands the durable child root down within its
    /// terminal window, which a loaded host can outrun, and a reopen of a
    /// root still claimed is refused `AlreadyClaimed`. Stand down again,
    /// bounded, until the root is released.
    async fn shutdown_for_reopen(self) {
        let mut outcome = self.runtime.shutdown().await.child_mob_stand_down;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !child_root_released(&outcome) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            outcome = self
                .runtime
                .mob_runtime()
                .stand_down_child_mobs(deadline)
                .await;
        }
        assert!(
            child_root_released(&outcome),
            "the durable child root is released before the reopen: {outcome:?}"
        );
    }
}

fn child_root_released(outcome: &meerkat_meta::ChildMobStandDownOutcome) -> bool {
    matches!(
        outcome,
        meerkat_meta::ChildMobStandDownOutcome::Released { .. }
            | meerkat_meta::ChildMobStandDownOutcome::NotConfigured
    )
}

/// A send by name to an identity created at runtime, through `meta/send`
/// and `meta/send_message`, goes through the identity's door and reaches its
/// member, created after this process's roster pass ran: the door registers
/// it from its row first.
///
/// RED before Meta served created identities: both sends were refused
/// `-32001` (unknown identity), and nothing reached the member.
#[tokio::test(flavor = "multi_thread")]
async fn a_send_by_name_to_an_identity_created_at_runtime_is_delivered_through_its_door() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;

    let sent = rig.send(CREATED, "CREATED-SEND-ONE").await;
    assert!(
        sent["error"].is_null(),
        "meta/send goes through the identity's door: {sent:#?}"
    );
    assert!(
        rig.asked("CREATED-SEND-ONE").await,
        "the send reaches the created identity's member"
    );
    assert!(
        rig.registered(CREATED).await,
        "the door registered the created identity"
    );

    let message = rig
        .rpc(
            "meta/send_message",
            json!({"member_id": CREATED, "message": "CREATED-SEND-TWO"}),
        )
        .await;
    assert!(
        message["error"].is_null(),
        "meta/send_message goes through the identity's door: {message:#?}"
    );
    assert!(
        rig.asked("CREATED-SEND-TWO").await,
        "meta/send_message reaches the created identity's member"
    );
    rig.shutdown().await;
}

/// A dispatch to an identity created at runtime goes through the identity's
/// door and reaches its member.
///
/// RED before Meta served created identities: the dispatch was refused
/// `-32001`.
#[tokio::test(flavor = "multi_thread")]
async fn a_dispatch_to_an_identity_created_at_runtime_is_delivered_through_its_door() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;

    let dispatched = rig
        .rpc(
            "meta/dispatch",
            json!({"identity": CREATED, "dispatch_input": {"content": "CREATED-DISPATCH"}}),
        )
        .await;
    assert!(
        dispatched["error"].is_null(),
        "meta/dispatch goes through the identity's door: {dispatched:#?}"
    );
    assert!(
        rig.asked("CREATED-DISPATCH").await,
        "the dispatch reaches the created identity's member"
    );
    rig.shutdown().await;
}

/// A member mutation of an identity created at runtime is answered by the
/// identity's door: Core's MobMachine drives a store-owned identity's
/// lifecycle, so Meta's lifecycle verbs refuse it, typed and final
/// (`identity_store_owned`), and nothing changes. Core still holds its member
/// seated, and a send still reaches it.
///
/// RED before Meta served created identities: `meta/reload_member` and
/// `meta/retire_member` were refused `-32001` (unknown identity) before any
/// door.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_mutation_of_an_identity_created_at_runtime_is_answered_by_its_door() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;

    let reload = rig
        .rpc("meta/reload_member", json!({"member_id": CREATED}))
        .await;
    assert!(reload["result"].is_null(), "{reload:#?}");
    assert_eq!(
        (
            reload["error"]["code"].clone(),
            reload["error"]["data"].clone()
        ),
        (
            json!(-32603),
            json!({"kind": "identity_store_owned", "identity": CREATED, "retryable": false})
        ),
        "the identity's door refuses the reload, typed and final: {reload:#?}"
    );

    let retire = rig
        .rpc("meta/retire_member", json!({"member_id": CREATED}))
        .await;
    assert!(retire["result"].is_null(), "{retire:#?}");
    assert_eq!(
        retire["error"]["code"],
        json!(-32000),
        "meta/retire_member answers the identity door's refusal: {retire:#?}"
    );
    let message = retire["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("retire_member (identity) failed") && message.contains("store-owned"),
        "the identity's door refused the retire: {retire:#?}"
    );

    assert!(
        rig.seated(CREATED),
        "nothing retired the created identity's member"
    );
    let sent = rig.send(CREATED, "CREATED-AFTER-MUTATION").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(
        rig.asked("CREATED-AFTER-MUTATION").await,
        "the created identity still serves its sends"
    );
    rig.shutdown().await;
}

/// The roster pass registers an identity created at runtime from its row in
/// the mob's identity store, with that row's spec, though the host's roster
/// does not list it; after a restart over the same state directory the
/// boot's roster pass registers it again, and a send by name reaches it.
///
/// RED before Meta served created identities: no roster pass registered it,
/// before or after the restart.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_created_at_runtime_is_registered_by_the_roster_pass_and_again_after_a_restart()
{
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot(temp.path(), &mob_id).await;
    rig.create(CREATED).await;
    assert!(
        !rig.registered(CREATED).await,
        "no door named the created identity yet"
    );

    rig.roster_pass().await;
    assert!(
        rig.registered(CREATED).await,
        "the roster pass registers the identity created at runtime"
    );
    let registered = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed")
        .registered_spec(&AgentIdentity::parse(CREATED).unwrap())
        .await
        .expect("the created identity's registered row");
    assert_eq!(
        (
            registered.spec.profile.as_str(),
            registered.spec.labels.clone(),
            registered
                .spec
                .display_name
                .as_ref()
                .map(|name| name.as_str().to_string()),
        ),
        (
            "worker",
            created_labels(),
            Some(format!("Created {CREATED}"))
        ),
        "the registered row is the spec of the identity's row in the store"
    );
    assert!(
        rig.registered(DECLARED).await,
        "the roster's own identity stays registered"
    );
    rig.shutdown_for_reopen().await;

    let mut rig = boot(temp.path(), &mob_id).await;
    assert!(
        rig.registered(CREATED).await,
        "the boot's roster pass registers the created identity again after the restart"
    );
    let sent = rig.send(CREATED, "CREATED-AFTER-RESTART").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(
        rig.asked("CREATED-AFTER-RESTART").await,
        "a send by name reaches the created identity after the restart"
    );
    rig.shutdown().await;
}

/// A name Core holds for no identity stays unknown (`-32001`): a name never
/// used, and the name of an identity created at runtime and removed since,
/// whose registration the next roster pass forgets.
///
/// RED before Meta served created identities: the first send to the created
/// identity was refused `-32001`.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_no_identity_holds_stays_unknown_and_a_removed_created_identity_is_forgotten() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    let sent = rig.send(CREATED, "CREATED-BEFORE-REMOVAL").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(rig.asked("CREATED-BEFORE-REMOVAL").await);

    rig.remove(CREATED).await;
    rig.roster_pass().await;
    assert!(
        !rig.registered(CREATED).await,
        "the roster pass forgets the removed identity's registration"
    );
    for name in [CREATED, "nobody"] {
        let refused = rig.send(name, "NEVER-DELIVERED").await;
        assert!(refused["result"].is_null(), "{name}: {refused:#?}");
        assert_eq!(
            (
                refused["error"]["code"].clone(),
                refused["error"]["data"].clone()
            ),
            (json!(-32001), Value::Null),
            "{name} is unknown: {refused:#?}"
        );
    }
    rig.shutdown().await;
}
