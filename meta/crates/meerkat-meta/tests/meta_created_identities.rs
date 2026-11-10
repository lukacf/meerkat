#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Identities created at runtime, served by a Meta gateway's identity
//! runtime.
//!
//! An identity created at runtime is a created entry of Core's mob: its
//! record is its row in the mob's identity store (store-owned), not the
//! host's roster. The roster pass registers each one from that row, and a
//! door that names one this runtime has not registered yet registers it from
//! its row first, so a send by name, a dispatch and a schedule fire go
//! through the identity's door, as for any registered identity. A delivery
//! door reads the row again, so a change Core made to it is served at once,
//! and a delivery to one Core holds no member of is answered from Core's
//! entry. Core's MobMachine drives a store-owned identity's lifecycle, so the
//! identity's door refuses Meta's lifecycle verbs on it, typed and final, and
//! changes nothing. A name Core holds for no identity, a removed identity's
//! name included, stays unknown.
//!
//! Each row boots the gateway's runtime composition on a temporary state
//! directory (persistent stores, the state directory's identity substrate, a
//! roster provider that lists one declared identity) with a model that
//! records what each member's turn was asked, and creates its identities
//! through Core's store-owned create.

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::{StopReason, ToolCallView, ToolDef};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, RosterProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, BoundGovernedInput,
    CustomizerError, DurableAgentSpec, GovernedBindContext, GovernedBindError, GovernedBindRefusal,
    GovernedInputBinder, GovernedInputError, GovernedSettlement, IdentityRuntimeError,
    LocalExternalToolOverlay, MutableRosterProvider, RosterContext, RosterError,
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
/// The tool the host's customizer gives every build it composes.
const TOOL: &str = "lookup_workspace";
/// What the governed row's binder answers: it binds nothing.
const BIND_REFUSED: &str = "the row's host binds nothing";
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

/// The controller the members' model reports, as a governed member's
/// selection owner does: the composition pins it, so a governed input
/// reaches its host's bind.
fn controller() -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: "gpt-5.5".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm": "meta-created-identities", "account": "recording"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "recording-profile".into(),
        "recording-transport".into(),
    )
}

/// The members' model: each request's last message (past Core's system
/// notices) is sent on the row's channel, each whole request is kept, and
/// every request answers.
#[derive(Clone)]
struct RecordingClient {
    asked: mpsc::UnboundedSender<String>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl meerkat_client::LlmClient for RecordingClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(controller())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(request).unwrap_or_default());
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

/// The host's tools for one build: [`TOOL`].
struct HostTools;

#[async_trait]
impl AgentToolDispatcher for HostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef {
            audience: Default::default(),
            name: TOOL.into(),
            description: "Look up the workspace record.".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
        })]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(meerkat_core::ToolResult {
            host_metadata: Default::default(),
            tool_use_id: call.id.to_string(),
            content: vec![],
            is_error: false,
            settlement_failures: Vec::new(),
        }
        .into())
    }
}

/// The host's customizer: every build it composes gets [`TOOL`].
struct ToolCustomizer;

#[async_trait]
impl AgentCustomizer for ToolCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(HostTools));
        Ok(())
    }
}

/// The trusted host's binder of the governed row: it records the registered
/// row each bind is handed, and refuses, so nothing is submitted.
#[derive(Default)]
struct RefusingBinder {
    rows: Mutex<Vec<DurableAgentSpec>>,
}

#[async_trait]
impl GovernedInputBinder for RefusingBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        _input: meerkat_runtime::Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        self.rows.lock().unwrap().push(context.spec.clone());
        Err(GovernedBindRefusal::denied(BIND_REFUSED).into())
    }

    fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {
        panic!("nothing is submitted, so no admission is left unresolved");
    }
}

/// One booted gateway runtime and what its members' model was asked.
struct Rig {
    runtime: meerkat_meta::UnifiedRuntime,
    asked: mpsc::UnboundedReceiver<String>,
    requests: Arc<Mutex<Vec<String>>>,
}

/// Boot the gateway's runtime composition on `state` for the mob `mob_id`.
async fn boot(state: &Path, mob_id: &str) -> Rig {
    boot_with(state, mob_id, None).await
}

/// [`boot`] with the host's agent `customizer`, when it composes one.
async fn boot_with(
    state: &Path,
    mob_id: &str,
    customizer: Option<Arc<dyn AgentCustomizer>>,
) -> Rig {
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(definition(mob_id))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(DeclaredRoster))
        .identity_runtime_instance_id("meta-created-identities")
        .default_llm_client(Arc::new(RecordingClient {
            asked: asked_tx,
            requests: Arc::clone(&requests),
        }));
    if let Some(customizer) = customizer {
        builder = builder.agent_customizer(customizer);
    }
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    Rig {
        runtime,
        asked,
        requests,
    }
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

/// The spec every identity a row creates is created with.
fn created_spec(name: &str) -> meerkat_mob::CreatedIdentitySpec {
    meerkat_mob::CreatedIdentitySpec::new(name, "worker")
        .with_display_name(format!("Created {name}"))
        .with_labels(created_labels())
}

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

/// Whether a rendered model request offers [`TOOL`]: the tool definitions it
/// carries name it, whatever its transcript mentions.
fn offers_tool(request: &str) -> bool {
    serde_json::from_str::<Value>(request)
        .is_ok_and(|request| request["tools"].to_string().contains(TOOL))
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
        self.create_request(
            meerkat_mob::CreatedIdentityRequest::new(created_spec(name)),
            name,
        )
        .await;
    }

    /// Create `name` at runtime from `request` through Core's store-owned
    /// create.
    async fn create_request(&self, request: meerkat_mob::CreatedIdentityRequest, name: &str) {
        let created = self
            .runtime
            .mob_handle()
            .create_store_owned_identities(
                meerkat_mob::StoreOwnedIdentityCreate::new(
                    operation(&format!("create-{name}")),
                    meerkat_mob::IdentityChangeRequesterInput::host("console"),
                    vec![request],
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

    /// Change `name`'s row through Core's store-owned update, from the spec
    /// [`Self::create`] created it with to `spec`.
    async fn update(&self, name: &str, spec: meerkat_mob::CreatedIdentitySpec) {
        let digest = created_spec(name).spec_digest().expect("the row's digest");
        let updated = self
            .runtime
            .mob_handle()
            .update_store_owned_identity(
                meerkat_mob::StoreOwnedIdentityUpdate::new(
                    operation(&format!("update-{name}")),
                    meerkat_mob::IdentityChangeRequesterInput::host("console"),
                    spec,
                    digest,
                ),
                allow(),
            )
            .await
            .map(|updated| updated.change);
        assert!(
            matches!(
                updated,
                Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
            ),
            "Core updates {name}: {updated:?}"
        );
    }

    /// Archive `name` through Core's archive: Core retires its member and
    /// keeps its name and its record until its unarchive.
    async fn archive(&self, name: &str) {
        let archived = self
            .runtime
            .mob_handle()
            .archive_identity(
                meerkat_mob::IdentityArchiveChange::new(
                    operation(&format!("archive-{name}")),
                    meerkat_mob::IdentityChangeRequesterInput::host("console"),
                    name,
                ),
                allow(),
            )
            .await;
        assert!(
            matches!(
                archived,
                Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
            ),
            "Core archives {name}: {archived:?}"
        );
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

    /// The first whole model request that carried `marker`.
    fn request_with(&self, marker: &str) -> Option<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.contains(marker))
            .cloned()
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

/// The `(code, data)` of a JSON-RPC error response.
fn error_of(response: &Value) -> (Value, Value) {
    (
        response["error"]["code"].clone(),
        response["error"]["data"].clone(),
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

/// `meta/send_message` as the first door that names an identity created at
/// runtime registers it from its row itself, and its message reaches the
/// member through the identity's door: no other door ran first.
#[tokio::test(flavor = "multi_thread")]
async fn a_send_message_as_the_first_door_registers_an_identity_created_at_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    assert!(
        !rig.registered(CREATED).await,
        "no door named the created identity yet"
    );

    let message = rig
        .rpc(
            "meta/send_message",
            json!({"member_id": CREATED, "message": "CREATED-MESSAGE-FIRST"}),
        )
        .await;
    assert!(
        message["error"].is_null(),
        "meta/send_message goes through the identity's door: {message:#?}"
    );
    assert!(
        rig.asked("CREATED-MESSAGE-FIRST").await,
        "the message reaches the created identity's member"
    );
    assert!(
        rig.registered(CREATED).await,
        "meta/send_message registered the created identity from its row"
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

/// A delivery door reads the row of an identity created at runtime again,
/// so Core's update of its row is served at once: once Core updates it to
/// internal only, `meta/send` (the external door) refuses it `-32002` (not
/// addressable), and nothing reaches its member, with no roster pass in
/// between.
///
/// RED before the doors read the row again: the send was delivered under
/// the registration the first send made, until a roster pass.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_core_updates_to_internal_only_is_refused_at_once_by_the_external_send_door() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    let sent = rig.send(CREATED, "BEFORE-UPDATE").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(rig.asked("BEFORE-UPDATE").await);

    rig.update(
        CREATED,
        created_spec(CREATED)
            .with_addressability(meerkat_mob::IdentityAddressability::InternalOnly),
    )
    .await;
    let refused = rig.send(CREATED, "AFTER-UPDATE").await;
    assert!(refused["result"].is_null(), "{refused:#?}");
    assert_eq!(
        error_of(&refused),
        (json!(-32002), Value::Null),
        "the external send door refuses the identity Core made internal only: {refused:#?}"
    );
    assert!(
        rig.request_with("AFTER-UPDATE").is_none(),
        "nothing reached the member"
    );
    rig.shutdown().await;
}

/// A send or a dispatch to an identity created at runtime that Core archived
/// is answered from Core's entry: final, typed (`identity_archived`), and it
/// names the archive's way back, Core's `unarchive_identity`. Nothing
/// reaches a member.
///
/// RED before the refusal read Core's entry: both answered `-32603` "in
/// state Dormant", with no kind.
#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_to_an_archived_identity_created_at_runtime_is_refused_typed_naming_its_unarchive()
 {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    let sent = rig.send(CREATED, "BEFORE-ARCHIVE").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(rig.asked("BEFORE-ARCHIVE").await);

    rig.archive(CREATED).await;
    for (method, params) in [
        (
            "meta/send",
            json!({"identity": CREATED, "content": "AFTER-ARCHIVE"}),
        ),
        (
            "meta/dispatch",
            json!({"identity": CREATED, "dispatch_input": {"content": "AFTER-ARCHIVE"}}),
        ),
    ] {
        let refused = rig.rpc(method, params).await;
        assert!(refused["result"].is_null(), "{method}: {refused:#?}");
        assert_eq!(
            error_of(&refused),
            (
                json!(-32603),
                json!({
                    "kind": "identity_archived",
                    "identity": CREATED,
                    "remedy": "unarchive_identity",
                    "retryable": false,
                })
            ),
            "{method}: the archived identity is refused typed and final: {refused:#?}"
        );
        assert!(
            refused["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("unarchive_identity")),
            "{method}: the refusal names the unarchive: {refused:#?}"
        );
    }
    assert!(
        rig.request_with("AFTER-ARCHIVE").is_none(),
        "nothing reached a member"
    );
    rig.shutdown().await;
}

/// After a restart, the roster pass registers an identity created at
/// runtime that Core archived (Core keeps its name and its record), and a
/// send to it answers Core's archive, typed and final.
///
/// RED before the refusal read Core's entry: the send answered `-32603` "in
/// state Dormant", with no kind.
#[tokio::test(flavor = "multi_thread")]
async fn a_send_to_an_identity_created_at_runtime_archived_before_a_restart_is_refused_typed() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot(temp.path(), &mob_id).await;
    rig.create(CREATED).await;
    rig.archive(CREATED).await;
    rig.shutdown_for_reopen().await;

    let rig = boot(temp.path(), &mob_id).await;
    let refused = rig.send(CREATED, "AFTER-RESTART").await;
    assert!(refused["result"].is_null(), "{refused:#?}");
    assert_eq!(
        error_of(&refused),
        (
            json!(-32603),
            json!({
                "kind": "identity_archived",
                "identity": CREATED,
                "remedy": "unarchive_identity",
                "retryable": false,
            })
        ),
        "the archived identity is refused typed and final after the restart: {refused:#?}"
    );
    assert!(
        rig.request_with("AFTER-RESTART").is_none(),
        "nothing reached a member"
    );
    rig.shutdown().await;
}

/// A send to an identity created at runtime that awaits its host's
/// preparation (Core seats no member of it until the host settles it) is
/// refused retryable (`-32052`, `identity_wake_retryable`): the same send,
/// made again once Core seats it, is delivered. Nothing reaches a member.
///
/// RED before the refusal read Core's entry: the send answered a final
/// `-32603` "in state Dormant".
#[tokio::test(flavor = "multi_thread")]
async fn a_send_to_an_identity_created_at_runtime_awaiting_its_preparation_is_refused_retryable() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(created_spec(CREATED))
            .with_preparation(meerkat_mob::IdentityPreparationMode::Required),
        CREATED,
    )
    .await;

    let refused = rig.send(CREATED, "BEFORE-PREPARATION").await;
    assert!(refused["result"].is_null(), "{refused:#?}");
    assert_eq!(
        error_of(&refused),
        (
            json!(meerkat_meta::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE),
            json!({
                "kind": "identity_wake_retryable",
                "identity": CREATED,
                "session_id": null,
                "retryable": true,
            })
        ),
        "the identity awaiting its preparation is refused retryable: {refused:#?}"
    );
    assert!(
        rig.request_with("BEFORE-PREPARATION").is_none(),
        "nothing reached a member"
    );
    rig.shutdown().await;
}

/// The governed door (a trusted host's governed input, the door Connect's
/// governed submit uses) registers an identity created at runtime from its
/// row first, as the other delivery doors do: the input reaches the host's
/// bind, handed the identity's registered row, and the host's refusal ends
/// it (nothing is submitted).
///
/// RED before the governed door registered on demand: it refused the name
/// as unknown (`UnknownIdentity`) before any bind.
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_send_to_an_identity_created_at_runtime_registers_it_and_reaches_the_hosts_bind()
{
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    let binder = Arc::new(RefusingBinder::default());

    let sent = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed")
        .send_governed_with_turn_ticket(
            &id(CREATED),
            None,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "CREATED-GOVERNED",
                None,
            )),
            binder.clone(),
        )
        .await;
    assert!(
        matches!(
            &sent,
            Err(IdentityRuntimeError::GovernedInput(GovernedInputError::Refused(refusal)))
                if refusal.detail == BIND_REFUSED
        ),
        "the governed send reaches the host's bind: {sent:?}"
    );
    assert!(
        rig.registered(CREATED).await,
        "the governed door registered the created identity from its row"
    );
    let rows = binder.rows.lock().unwrap().clone();
    assert_eq!(
        rows.iter()
            .map(|row| (row.identity.as_str().to_string(), row.labels.clone()))
            .collect::<Vec<_>>(),
        vec![(CREATED.to_string(), created_labels())],
        "the bind was handed the identity's registered row, its row's spec"
    );
    rig.shutdown().await;
}

/// The reads of one identity register an identity created at runtime from
/// its row first, as its doors do: its status, its member health, its event
/// subscription and the capture of its delivery scope each answer for it,
/// never as an unknown identity, each on an identity no other door named.
///
/// RED before the reads registered on demand: each answered the name as an
/// unknown identity.
#[tokio::test(flavor = "multi_thread")]
async fn the_reads_of_one_identity_register_an_identity_created_at_runtime_on_demand() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    for name in [
        "keeper-status",
        "keeper-health",
        "keeper-events",
        "keeper-scope",
    ] {
        rig.create(name).await;
    }
    let identity_runtime = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed");

    let status = identity_runtime.status(&id("keeper-status")).await;
    assert!(
        status
            .as_ref()
            .is_ok_and(|status| status.labels == created_labels()),
        "the status read answers the created identity from its row: {status:?}"
    );
    let health = identity_runtime.member_health(&id("keeper-health")).await;
    assert!(
        health.is_ok(),
        "the health read answers the created identity: {health:?}"
    );
    let subscribed = identity_runtime.subscribe(&id("keeper-events")).await;
    assert!(
        subscribed.is_ok(),
        "the event subscription answers the created identity: {:?}",
        subscribed.err()
    );
    let scope = identity_runtime
        .capture_delivery_scope(&id("keeper-scope"))
        .await;
    assert!(
        !matches!(
            &scope,
            Err(meerkat_meta::identity_first::ScopedDeliveryError::Rejected { detail })
                if detail.starts_with("unknown identity")
        ),
        "the scope capture answers the created identity, never as unknown: {scope:?}"
    );
    for name in [
        "keeper-status",
        "keeper-health",
        "keeper-events",
        "keeper-scope",
    ] {
        assert!(
            rig.registered(name).await,
            "{name}: the read registered the created identity from its row"
        );
    }
    rig.shutdown().await;
}

/// A send to an identity created at runtime wakes its member with the host
/// customizer's tools, as a roster identity's member is woken: the door
/// registers the identity from its row, the registration keeps the
/// identity's customizer tool dispatcher, which Meta's spawn customizer
/// attaches to the member's build, and the build preparer publishes the
/// tools into it before Core provisions the member. The woken turn is
/// offered the tool.
///
/// RED before a created registration kept the dispatcher: the build carried
/// none, and the turn offered no customizer tool.
#[tokio::test(flavor = "multi_thread")]
async fn a_send_wakes_an_identity_created_at_runtime_with_the_customizer_tools() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot_with(
        temp.path(),
        &fresh_mob_id(),
        Some(Arc::new(ToolCustomizer) as Arc<dyn AgentCustomizer>),
    )
    .await;
    rig.create(CREATED).await;

    let sent = rig.send(CREATED, "CREATED-CUSTOMIZED").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(
        rig.asked("CREATED-CUSTOMIZED").await,
        "the send reaches the created identity's member"
    );
    let request = rig
        .request_with("CREATED-CUSTOMIZED")
        .expect("the woken turn's request");
    assert!(
        offers_tool(&request),
        "the woken turn is offered the host customizer's tool: {request}"
    );
    rig.shutdown().await;
}

/// A member of an identity created at runtime that Core wakes on its own,
/// before any roster pass or door named the identity, is registered from its
/// row by Meta's build preparer, which waits on no lifecycle lock: the
/// identity runtime serves it as a registered identity from then on.
///
/// RED before the preparer registered a created member: the identity stayed
/// unregistered, and its build was prepared as no identity's.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_core_wakes_registers_its_identity_created_at_runtime_from_its_row() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot_with(
        temp.path(),
        &fresh_mob_id(),
        Some(Arc::new(ToolCustomizer) as Arc<dyn AgentCustomizer>),
    )
    .await;
    rig.create(CREATED).await;
    assert!(
        !rig.registered(CREATED).await,
        "no door named the created identity yet"
    );

    rig.runtime
        .mob_handle()
        .ensure_member_live(&meerkat_mob::AgentIdentity::from(CREATED))
        .await
        .expect("Core wakes the created identity's member");
    assert!(
        rig.registered(CREATED).await,
        "the build preparer registered the created identity from its row"
    );
    rig.shutdown().await;
}

/// The offline identity reset (`meta_gateway identity-reset`) refuses an
/// identity created at runtime as store-owned (`identity_store_owned`), as
/// every Meta lifecycle verb refuses it, and writes no first-boot row: Core's
/// MobMachine drives it.
///
/// RED before the reset named store ownership: it refused the identity as
/// one whose management kind this build does not know.
#[tokio::test(flavor = "multi_thread")]
async fn the_offline_identity_reset_refuses_an_identity_created_at_runtime_as_store_owned() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let mut rig = boot(temp.path(), &mob_id).await;
    rig.create(CREATED).await;
    let sent = rig.send(CREATED, "BEFORE-RESET").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(rig.asked("BEFORE-RESET").await);
    rig.shutdown_for_reopen().await;

    let report = meerkat_meta::storage_migrate_native::reset_identity(
        &meerkat_meta::storage_migrate_native::IdentityResetRequest::new(temp.path(), CREATED)
            .with_apply(),
    )
    .await;
    let refusal = serde_json::to_value(report.refusal.as_ref().expect("the reset refuses"))
        .expect("the refusal renders");
    assert_eq!(
        refusal["kind"],
        json!("identity_store_owned"),
        "the reset refuses the store-owned identity: {report:?}"
    );
    assert!(
        !report.written,
        "a refused reset writes nothing: {report:?}"
    );
}

/// A created entry whose name the member-id encoding maps to another member
/// id is not registered by the roster pass, under its own name or under the
/// name its member id decodes to: `mk--keeper` starts with the reserved
/// marker without being an encoding, so its member id decodes to `keeper`,
/// whose doors would act on another member (`keeper`) than Core's entry. A
/// send by either name through the identity runtime's door stays unknown,
/// nothing reaches a member, and Core holds no entry under either name's
/// member id. (The RPC doors refuse the marked name at their ingress.)
///
/// RED before the pass skipped such a name: the roster pass registered the
/// entry as `keeper`.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_name_the_member_id_encoding_maps_to_another_member_is_not_registered() {
    const ODD: &str = "mk--keeper";
    const DECODED: &str = "keeper";
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(ODD).await;

    rig.roster_pass().await;
    let identity_runtime = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed");
    for name in [ODD, DECODED] {
        assert!(
            !rig.registered(name).await,
            "{name}: the roster pass registers nothing for the created entry {ODD}"
        );
        let refused = identity_runtime
            .send(
                &id(name),
                &meerkat_core::ContentInput::Text("NEVER-DELIVERED".to_string()),
            )
            .await;
        assert!(
            matches!(&refused, Err(IdentityRuntimeError::UnknownIdentity(unknown)) if unknown.as_str() == name),
            "{name}: the name stays unknown to the identity runtime's door: {refused:?}"
        );
    }
    assert!(
        rig.request_with("NEVER-DELIVERED").is_none(),
        "nothing reached a member"
    );
    for name in [ODD, DECODED] {
        let member = meerkat_meta::member_comms_id::mob_member_id(name);
        assert!(
            matches!(
                rig.runtime.mob_handle().identity_entry(&member),
                Ok(IdentityEntryRead::NotAnEntry { worker: None })
            ),
            "{name}: Core holds no entry under its member id {member}"
        );
    }
    rig.shutdown().await;
}
