#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Identities created at runtime, served by a Meta gateway's identity
//! runtime.
//!
//! An identity created at runtime is an identity entry of Core's mob whose
//! record is its row in the mob's identity store, not the host's roster. The
//! runtime serves it as any identity. A roster pass registers each one it
//! has not registered yet from that row, and a door that names one this
//! runtime has not registered yet registers it from its row first, so a send
//! by name, a dispatch and a schedule fire go through the identity's door. A
//! delivery door reads the row again, so a change Core made to it is served
//! at once, and a delivery to one Core holds no member of is answered from
//! Core's entry. The lifecycle verbs, the scoped tool grants, the topology
//! provider, the warm and the bootstrap status treat it as a configured
//! identity, and its first build carries the host customizer's tools. A
//! name Core holds for no identity, a removed identity's name included,
//! stays unknown.
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
use meerkat_meta::identity_first::contracts::{AgentCustomizer, RosterProvider, TopologyProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, BoundGovernedInput,
    CustomizerError, DurableAgentSpec, GovernedBindContext, GovernedBindError, GovernedBindRefusal,
    GovernedInputBinder, GovernedInputError, GovernedSettlement, IdentityRuntimeError,
    LocalExternalToolOverlay, ManagedPeerEdge, MutableRosterProvider, RosterContext, RosterError,
    TopologyContext, TopologyError,
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

[profiles.analyst]
model = "gpt-5.4"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.analyst.tools]
comms = true
builtins = true
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
struct DeclaredRoster(DurableAgentSpec);

#[async_trait]
impl RosterProvider for DeclaredRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![self.0.clone()])
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
/// every request answers. A `pinned` client reports [`controller`] as its
/// selection; one that is not reports none, so each request names the model
/// its member was built with.
#[derive(Clone)]
struct RecordingClient {
    asked: mpsc::UnboundedSender<String>,
    requests: Arc<Mutex<Vec<String>>>,
    pinned: bool,
}

impl meerkat_client::LlmClient for RecordingClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.pinned.then(controller)
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
    /// The identity the host's roster declares.
    declared: DurableAgentSpec,
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
    boot_composed(state, mob_id, customizer, None).await
}

/// [`boot_with`] with the host's `topology` provider, when it composes one.
async fn boot_composed(
    state: &Path,
    mob_id: &str,
    customizer: Option<Arc<dyn AgentCustomizer>>,
    topology: Option<Arc<dyn TopologyProvider>>,
) -> Rig {
    boot_host(
        state,
        definition(mob_id),
        declared_spec(),
        customizer,
        topology,
        true,
    )
    .await
}

/// Boot the gateway's runtime composition on `state` for `definition`, with
/// a roster that declares `declared` alone, the host's `customizer` and
/// `topology` provider when it composes them, and a members' model that is
/// `pinned` to [`controller`] or not.
async fn boot_host(
    state: &Path,
    definition: MobDefinition,
    declared: DurableAgentSpec,
    customizer: Option<Arc<dyn AgentCustomizer>>,
    topology: Option<Arc<dyn TopologyProvider>>,
    pinned: bool,
) -> Rig {
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(definition)
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(DeclaredRoster(declared.clone())))
        .identity_runtime_instance_id("meta-created-identities")
        .default_llm_client(Arc::new(RecordingClient {
            asked: asked_tx,
            requests: Arc::clone(&requests),
            pinned,
        }));
    if let Some(customizer) = customizer {
        builder = builder.agent_customizer(customizer);
    }
    if let Some(topology) = topology {
        builder = builder.topology_provider(topology);
    }
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    Rig {
        runtime,
        asked,
        requests,
        declared,
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
            roster_provider: Arc::new(MutableRosterProvider::new(vec![self.declared.clone()])),
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

    /// The whole model requests so far.
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// `name`'s row in the mob's identity store, rendered: its revision,
    /// presence, spec and owned edges.
    async fn row(&self, name: &str) -> String {
        match self
            .runtime
            .mob_handle()
            .identity_record(&meerkat_mob::AgentIdentity::from(name))
            .await
            .expect("the row reads")
        {
            Some(meerkat_mob::CreatedIdentityObservation::Created(record)) => format!(
                "{} {:?} {:?} {:?}",
                record.revision, record.presence, record.spec, record.owned_edges
            ),
            other => panic!("{name} has a row: {other:?}"),
        }
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

/// A member mutation of an identity created at runtime, named before any
/// roster pass registered it, is answered by the identity's door, which
/// registers it from its row and serves the mutation as a configured
/// identity's: `meta/reload_member` answers its outcome and
/// `meta/retire_member` releases the member. Core still holds its member
/// seated, and a send still reaches it.
///
/// RED before Meta served created identities: `meta/reload_member` and
/// `meta/retire_member` were refused `-32001` (unknown identity) before any
/// door; and before the verbs served them, `identity_store_owned`.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_mutation_of_an_identity_created_at_runtime_is_answered_by_its_door() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;

    let retire = rig
        .rpc("meta/retire_member", json!({"member_id": CREATED}))
        .await;
    assert!(
        retire["error"].is_null(),
        "the identity's door serves the retire: {retire:#?}"
    );
    assert!(
        rig.registered(CREATED).await,
        "the door registered the identity from its row"
    );
    assert!(
        rig.seated(CREATED),
        "the retire released the member and kept it seated"
    );
    assert!(
        rig.turn(CREATED, "CREATED-AFTER-MUTATION").await,
        "the created identity still serves its sends"
    );

    let reload = rig.verb("meta/reload_member", CREATED).await;
    assert!(
        reload["error"].is_null() && !reload["result"].is_null(),
        "the identity's door serves the reload: {reload:#?}"
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

/// The offline identity reset (`meta_gateway identity-reset`) resets a
/// store-owned identity as any identity: it leaves its row in the first-boot
/// manifest, and the next start's roster pass carries it out. After that
/// start the agent is on a fresh session at the next generation with
/// nothing built, its name and its row are unchanged, the manifest row is
/// settled, and its next delivery runs on the fresh session without the old
/// conversation.
///
/// RED before the pass carried such a row out: the reset refused the
/// identity (`identity_store_owned`) and wrote nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_offline_identity_reset_gives_a_store_owned_identity_a_fresh_life_at_the_next_start() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let mut rig = boot(temp.path(), &mob_id).await;
    rig.create(CREATED).await;
    assert!(rig.turn(CREATED, "BEFORE-OFFLINE-RESET").await);
    let before = rig.life(CREATED).await;
    let row_before = rig.row(CREATED).await;
    rig.shutdown_for_reopen().await;

    let report = meerkat_meta::storage_migrate_native::reset_identity(
        &meerkat_meta::storage_migrate_native::IdentityResetRequest::new(temp.path(), CREATED)
            .with_apply(),
    )
    .await;
    assert!(
        report.refusal.is_none() && report.written,
        "the offline reset takes the store-owned identity and writes its row: {report:?}"
    );
    let manifest = temp.path().join("storage-migrate-native.first-boot.json");
    assert!(manifest.exists(), "the first-boot manifest holds the row");

    let mut rig = boot(temp.path(), &mob_id).await;
    let after = rig.life(CREATED).await;
    assert_ne!(after.session, before.session, "a fresh session");
    assert!(
        after.generation > before.generation,
        "at a later generation: {:?} after {:?}",
        after.generation,
        before.generation
    );
    assert!(
        rig.requests().is_empty(),
        "the start carried the reset out without building a member"
    );
    assert_eq!(
        rig.row(CREATED).await,
        row_before,
        "the name and the row are unchanged"
    );
    assert!(
        !manifest.exists(),
        "the start settled the manifest row and removed the empty manifest"
    );
    assert!(
        rig.turn(CREATED, "AFTER-OFFLINE-RESET").await,
        "the agent takes its next delivery"
    );
    let delivered = rig.life(CREATED).await;
    assert_eq!(
        (delivered.session, delivered.generation),
        (after.session, after.generation),
        "on the fresh session"
    );
    let request = rig
        .request_with("AFTER-OFFLINE-RESET")
        .expect("the delivery's turn");
    assert!(
        !request.contains("BEFORE-OFFLINE-RESET"),
        "the fresh conversation holds nothing of the old one"
    );
    rig.shutdown().await;
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

// ---------------------------------------------------------------------------
// One agent model: a store-owned identity is served as any identity
// ---------------------------------------------------------------------------

/// The verbs of the table, by the method a caller names: each takes the
/// identity's name, and `retire_at_scope` is the Rust door (a scope captured
/// from the identity, then the scoped retire).
const VERBS: &[&str] = &[
    "meta/retire",
    "meta/retire_member",
    "retire_at_scope",
    "meta/reload_member",
    "meta/identity/warm",
    "meta/respawn",
    "meta/respawn_member",
    "meta/reset",
    "meta/delete_identity",
];

/// What a caller observes of an identity between two reads of its status.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Life {
    session: Option<String>,
    generation: Option<u64>,
    state: String,
}

/// What a caller observes of one verb on one identity.
#[derive(Debug, PartialEq, Eq)]
struct VerbEffect {
    /// The verb answered a result, not an error.
    accepted: bool,
    /// The identity's member is on the session it was on before the verb.
    session_kept: bool,
    /// How many generations the verb advanced the identity's member.
    generations: u64,
    /// The identity's state right after the verb.
    state: String,
    /// The identity took a delivery after the verb, in this process.
    delivers: bool,
}

/// What Core's roster holds of a member.
struct Seat {
    role: ProfileName,
    labels: BTreeMap<String, String>,
    wired_to: std::collections::BTreeSet<meerkat_mob::AgentIdentity>,
}

impl Rig {
    /// The identity's status as a caller reads it.
    async fn life(&self, name: &str) -> Life {
        let status = self
            .runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .status(&id(name))
            .await
            .unwrap_or_else(|error| panic!("{name}: the status reads: {error}"));
        Life {
            session: status.session_id.map(|session| session.to_string()),
            generation: status.generation.map(meerkat_mob::Generation::get),
            state: format!("{:?}", status.state),
        }
    }

    /// Core's roster entry of `name`'s member: its role, its labels and the
    /// members it is wired to.
    async fn member(&self, name: &str) -> Seat {
        let entry = self
            .runtime
            .mob_handle()
            .get_member(&meerkat_meta::member_comms_id::mob_member_id(name))
            .await
            .expect("Core's roster reads")
            .unwrap_or_else(|| panic!("Core holds a member of {name}"));
        Seat {
            role: entry.role,
            labels: entry.labels,
            wired_to: entry.wired_to,
        }
    }

    /// Ask `verb` of `name` and answer the JSON-RPC response (the Rust door's
    /// answer is rendered as one). A verb Core refuses for a cause that
    /// clears on its own (the member's last turn is still settling) is asked
    /// again, bounded.
    async fn verb(&self, verb: &str, name: &str) -> Value {
        let mut response = self.verb_once(verb, name).await;
        for _ in 0..40 {
            let settling = response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("work in flight"));
            if !settling {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            response = self.verb_once(verb, name).await;
        }
        response
    }

    async fn verb_once(&self, verb: &str, name: &str) -> Value {
        match verb {
            "retire_at_scope" => {
                let identities = self
                    .runtime
                    .identity_runtime()
                    .expect("identity runtime installed");
                let scope = match identities.capture_delivery_scope(&id(name)).await {
                    Ok(scope) => scope,
                    Err(error) => return json!({"error": {"message": format!("{error:?}")}}),
                };
                match identities.retire_at_scope_tracked(&id(name), &scope).await {
                    Ok(_) => json!({"result": {"identity": name}}),
                    Err(error) => json!({"error": {"message": format!("{error:?}")}}),
                }
            }
            "meta/retire_member" | "meta/reload_member" | "meta/respawn_member" => {
                self.rpc(verb, json!({"member_id": name})).await
            }
            _ => self.rpc(verb, json!({"identity": name})).await,
        }
    }

    /// Run one turn of `name` with `marker`: `false` when the send was
    /// refused or the member's model was never asked. The model answers at
    /// once, and the turn is given a moment to settle; a verb that still
    /// finds it settling is asked again ([`Self::verb`]).
    async fn turn(&mut self, name: &str, marker: &str) -> bool {
        let sent = self.send(name, marker).await;
        if !sent["error"].is_null() || !self.asked(marker).await {
            eprintln!("{name}: the turn {marker:?} was not asked: {sent}");
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        true
    }

    /// Run `verb` on `name`, a member that ran a turn before, and answer what
    /// a caller observes of it, with the verb's response.
    async fn observe_verb(&mut self, verb: &str, name: &str) -> (VerbEffect, Value) {
        assert!(
            self.turn(name, &format!("BEFORE {verb} {name}")).await,
            "{verb} {name}: the member runs a turn before the verb"
        );
        let before = self.life(name).await;
        let response = self.verb(verb, name).await;
        let after = self.life(name).await;
        let delivers = self.turn(name, &format!("AFTER {verb} {name}")).await;
        (
            VerbEffect {
                accepted: response["error"].is_null(),
                session_kept: before.session == after.session,
                generations: after
                    .generation
                    .unwrap_or_default()
                    .saturating_sub(before.generation.unwrap_or_default()),
                state: after.state,
                delivers,
            },
            response,
        )
    }
}

/// Every lifecycle verb acts on a store-owned identity as on a configured
/// one: for each verb the configured identity and a store-owned one, each a
/// member that ran a turn, answer the same (accepted, the session kept or
/// not, the generations advanced, the state right after), and each takes its
/// next delivery in this process, with no restart.
///
/// RED before the verbs served both: every verb refused the store-owned
/// identity (`identity_store_owned`), and changed nothing of it.
#[tokio::test(flavor = "multi_thread")]
async fn meta_verbs_act_on_a_store_owned_identity_as_on_a_configured_one() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    let mut differing = Vec::new();
    for verb in VERBS {
        let created = format!(
            "keeper-{}",
            verb.rsplit('/').next().unwrap().replace('_', "-")
        );
        rig.create(&created).await;
        let (configured, configured_response) = rig.observe_verb(verb, DECLARED).await;
        let (store_owned, store_owned_response) = rig.observe_verb(verb, &created).await;
        assert!(
            configured.accepted && configured.delivers,
            "{verb}: the configured identity takes the verb and its next delivery: \
             {configured:?} {configured_response:#?}"
        );
        if configured != store_owned {
            differing.push(format!(
                "{verb}: configured {configured:?}, store-owned {store_owned:?} \
                 ({store_owned_response})"
            ));
        }
        assert!(
            rig.seated(&created),
            "{verb}: Core holds the store-owned identity's member seated after the verb"
        );
    }
    assert!(
        differing.is_empty(),
        "each verb has one effect, whichever record holds the identity's spec:\n{}",
        differing.join("\n")
    );
    rig.shutdown().await;
}

/// The kinds of the events `events` holds now, in order.
fn event_kinds<Event: Clone + std::fmt::Debug>(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
) -> Vec<String> {
    let mut kinds = Vec::new();
    while let Ok(event) = events.try_recv() {
        let rendered = format!("{event:?}");
        kinds.push(
            rendered
                .split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap_or_default()
                .to_string(),
        );
    }
    kinds
}

/// A respawn gives a store-owned identity what it gives a configured one,
/// fact by fact as a caller observes them: the same session, the next
/// generation, the state right after and after the next delivery, the
/// events its subscribers get, and a next turn that is offered the host
/// customizer's tool.
///
/// RED before the respawn served both: the store-owned identity's respawn
/// was refused (`identity_store_owned`).
#[tokio::test(flavor = "multi_thread")]
async fn a_respawn_gives_a_store_owned_identity_what_it_gives_a_configured_one() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot_with(
        temp.path(),
        &fresh_mob_id(),
        Some(Arc::new(ToolCustomizer) as Arc<dyn AgentCustomizer>),
    )
    .await;
    rig.create(CREATED).await;
    let mut observed = Vec::new();
    for name in [DECLARED, CREATED] {
        let before_marker = format!("BEFORE-RESPAWN {name}");
        assert!(
            rig.turn(name, &before_marker).await,
            "{name}: a turn before"
        );
        let mut events = rig
            .runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .subscribe(&id(name))
            .await
            .expect("the identity's events");
        let before = rig.life(name).await;
        let respawned = rig.verb("meta/respawn", name).await;
        assert!(
            respawned["error"].is_null(),
            "{name}: the respawn is served: {respawned:#?}"
        );
        let after = rig.life(name).await;
        let after_marker = format!("AFTER-RESPAWN {name}");
        assert!(
            rig.turn(name, &after_marker).await,
            "{name}: the respawned member takes its next delivery"
        );
        let delivered = rig.life(name).await;
        let offered = rig
            .request_with(&after_marker)
            .is_some_and(|request| offers_tool(&request));
        observed.push((
            name,
            (
                before.session == after.session,
                after
                    .generation
                    .unwrap_or_default()
                    .saturating_sub(before.generation.unwrap_or_default()),
                respawned["result"]["session_id"] == json!(after.session),
                respawned["result"]["generation"] == json!(after.generation),
                after.state,
                delivered.state,
                event_kinds(&mut events),
                offered,
            ),
        ));
    }
    assert_eq!(
        observed[0].1, observed[1].1,
        "a respawn's facts are the same for {} and {}",
        observed[0].0, observed[1].0
    );
    assert!(
        observed[1].1.0 && observed[1].1.1 == 1 && observed[1].1.7,
        "the store-owned identity keeps its session, advances one generation and is offered \
         the customizer's tool: {:?}",
        observed[1].1
    );
    rig.shutdown().await;
}

/// A respawn seats a store-owned identity's successor with its row's
/// current spec: after Core's update moved the row's profile and labels, the
/// respawned member runs the row's profile with the row's labels, on its own
/// session, and the identity runtime serves the row's spec.
///
/// RED before the respawn served store-owned identities: the respawn was
/// refused (`identity_store_owned`), and the seat kept the labels it was
/// seated with.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawn_seats_a_store_owned_identity_with_its_rows_current_spec() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    assert!(rig.turn(CREATED, "BEFORE-ROW-CHANGE").await);
    let before = rig.life(CREATED).await;

    let mut labels = created_labels();
    labels.insert("tier".to_string(), "gold".to_string());
    rig.update(
        CREATED,
        meerkat_mob::CreatedIdentitySpec::new(CREATED, "analyst")
            .with_display_name(format!("Created {CREATED}"))
            .with_labels(labels.clone()),
    )
    .await;

    let respawned = rig.verb("meta/respawn", CREATED).await;
    assert!(
        respawned["error"].is_null(),
        "the respawn is served: {respawned:#?}"
    );
    let member = rig.member(CREATED).await;
    assert_eq!(
        (member.role.as_str(), &member.labels),
        ("analyst", &labels),
        "the successor is seated with the row's current profile and labels"
    );
    let after = rig.life(CREATED).await;
    assert_eq!(after.session, before.session, "on its own session");
    let status = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed")
        .status(&id(CREATED))
        .await
        .expect("the status reads");
    assert_eq!(
        (
            status.profile.as_ref().map(ProfileName::as_str),
            &status.labels
        ),
        (Some("analyst"), &labels),
        "the identity runtime serves the row's current spec"
    );
    assert!(
        rig.turn(CREATED, "AFTER-ROW-RESPAWN").await,
        "the successor takes its next delivery"
    );
    let request = rig
        .request_with("AFTER-ROW-RESPAWN")
        .expect("the successor's turn");
    let before_request = rig
        .request_with("BEFORE-ROW-CHANGE")
        .expect("the turn before the row changed");
    assert_ne!(
        tool_names(&request),
        tool_names(&before_request),
        "the successor is built with the row's profile's tools, which the profile it was \
         created under does not offer"
    );
    rig.shutdown().await;
}

/// The names of the tools `request` offers, sorted.
fn tool_names(request: &str) -> Vec<String> {
    let request: Value = serde_json::from_str(request).unwrap_or_default();
    let mut names: Vec<String> = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

/// A delivery's completion is observed for a store-owned identity as for a
/// configured one: `meta/wait_for_completion` on the cursor a send answered
/// ends `completed` for both.
///
/// RED before the identity's event stream was tracked for a member that
/// carries no `agent_identity` label: the store-owned identity's cursor
/// never moved, and the wait ended `timed_out`.
#[tokio::test(flavor = "multi_thread")]
async fn a_deliverys_completion_is_observed_for_a_store_owned_identity_as_for_a_configured_one() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    let mut outcomes = Vec::new();
    for name in [DECLARED, CREATED] {
        // The first turn materializes the member; the wait is on the next.
        assert!(rig.turn(name, &format!("WARM {name}")).await);
        let cursor = rig
            .rpc(
                "meta/wait_for_completion",
                json!({"identity": name, "timeout_ms": 10_000}),
            )
            .await;
        let sent = rig.send(name, &format!("OBSERVED {name}")).await;
        assert!(sent["error"].is_null(), "{name}: {sent:#?}");
        let waited = rig
            .rpc(
                "meta/wait_for_completion",
                json!({
                    "identity": name,
                    "after": cursor["result"]["completion_cursor"],
                    "timeout_ms": 10_000,
                }),
            )
            .await;
        outcomes.push((
            name,
            cursor["result"]["outcome"].clone(),
            waited["result"]["outcome"].clone(),
        ));
    }
    assert_eq!(
        outcomes,
        vec![
            (DECLARED, json!("completed"), json!("completed")),
            (CREATED, json!("completed"), json!("completed")),
        ],
        "each identity's first turn and its next delivery are observed completed"
    );
    rig.shutdown().await;
}

/// A store-owned identity whose name the member-id encoding changes (the
/// usual identity shape, with a `:`) is served under its own name: Core
/// holds it under the comms-safe member id the name encodes to, and its
/// deliveries, its respawn and its status go through that mapping.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_owned_identity_with_an_encoded_name_is_served_under_its_name() {
    const NAMED: &str = "team:keeper";
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    let member = meerkat_meta::member_comms_id::mob_member_id(NAMED);
    assert_ne!(member.as_str(), NAMED, "the encoding changes the name");
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(created_spec(member.as_str())),
        member.as_str(),
    )
    .await;
    assert!(
        rig.turn(NAMED, "ENCODED-BEFORE").await,
        "a delivery to its name runs"
    );
    let before = rig.life(NAMED).await;
    let respawned = rig.verb("meta/respawn", NAMED).await;
    assert!(
        respawned["error"].is_null(),
        "the respawn is served: {respawned:#?}"
    );
    let after = rig.life(NAMED).await;
    assert_eq!(after.session, before.session, "on its own session");
    assert!(
        after.generation > before.generation,
        "at the next generation"
    );
    assert!(
        rig.turn(NAMED, "ENCODED-AFTER").await,
        "it takes its next delivery"
    );
    rig.shutdown().await;
}

/// The peer rule the rows' owned edges are admitted under.
fn peer_rule() -> meerkat_mob::OwnedEdgeAdmission {
    meerkat_mob::OwnedEdgeAdmission::rule("row-peers").expect("a peer rule id")
}

/// Whether Core holds `a` and `b` wired.
async fn wired(rig: &Rig, a: &str, b: &str) -> bool {
    rig.member(a)
        .await
        .wired_to
        .contains(&meerkat_meta::member_comms_id::mob_member_id(b))
}

/// A reset gives a store-owned identity a fresh session and keeps its name
/// and its edges: Core holds the same identity entry, seated at the next
/// generation on a new session, its row is unchanged, the edge its row owns
/// is wired again, and it takes its next delivery on the new session. The
/// fresh life is seated with its row's current spec.
///
/// RED before the reset served store-owned identities: the reset was refused
/// (`identity_store_owned`).
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_gives_a_store_owned_identity_a_fresh_session_and_keeps_its_name_and_edges() {
    const PEER: &str = "keeper-peer";
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(PEER).await;
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(created_spec(CREATED))
            .with_peers(vec![meerkat_mob::OwnedIdentityEdge::new(PEER, peer_rule())]),
        CREATED,
    )
    .await;
    assert!(wired(&rig, CREATED, PEER).await, "the row's edge is wired");
    assert!(rig.turn(CREATED, "BEFORE-RESET").await);
    let mut labels = created_labels();
    labels.insert("tier".to_string(), "gold".to_string());
    rig.update(CREATED, created_spec(CREATED).with_labels(labels.clone()))
        .await;
    let before = rig.life(CREATED).await;

    let reset = rig.verb("meta/reset", CREATED).await;
    assert!(reset["error"].is_null(), "the reset is served: {reset:#?}");
    let after = rig.life(CREATED).await;
    assert!(
        after.session.is_some() && after.session != before.session,
        "a fresh session: {before:?} -> {after:?}"
    );
    assert_eq!(
        after.generation,
        before.generation.map(|generation| generation + 1),
        "the next generation"
    );
    assert_eq!(
        reset["result"]["session_id"],
        json!(after.session),
        "the reset answers the fresh session: {reset:#?}"
    );
    match rig
        .runtime
        .mob_handle()
        .identity_entry(&meerkat_mob::AgentIdentity::from(CREATED))
    {
        Ok(IdentityEntryRead::Entry(entry)) => assert!(
            entry.origin == meerkat_mob::IdentityEntryOrigin::Created
                && matches!(entry.standing, EntryStanding::Seated(_)),
            "the same store-owned entry, seated: {entry:?}"
        ),
        other => panic!("{CREATED} stays an identity entry: {other:?}"),
    }
    let row = rig
        .runtime
        .mob_handle()
        .identity_record(&meerkat_mob::AgentIdentity::from(CREATED))
        .await
        .expect("the row reads");
    assert!(
        matches!(
            &row,
            Some(meerkat_mob::CreatedIdentityObservation::Created(record))
                if record.presence == meerkat_mob::CreatedIdentityPresence::Present
        ),
        "the row stays present: {row:?}"
    );
    assert_eq!(
        rig.member(CREATED).await.labels,
        labels,
        "the fresh life is seated with the row's current labels"
    );
    assert!(
        wired(&rig, CREATED, PEER).await,
        "the row's owned edge is wired again after the reset"
    );
    assert!(rig.registered(CREATED).await, "the name stays served");
    assert!(
        rig.turn(CREATED, "AFTER-RESET").await,
        "the fresh life takes its next delivery"
    );
    let request = rig.request_with("AFTER-RESET").expect("the fresh turn");
    assert!(
        !request.contains("BEFORE-RESET"),
        "the fresh session holds none of the old conversation: {request}"
    );
    rig.shutdown().await;
}

/// `meta/delete_identity` gives an agent a fresh life and keeps it, whichever
/// record holds its spec: after it the configured identity and the
/// store-owned one are each still served and listed under the same name, on
/// a new session at the next generation, and take their next delivery. The
/// store-owned identity's row stays present: nothing is removed or archived.
///
/// RED before the verb served store-owned identities: the delete was refused
/// (`identity_store_owned`).
#[tokio::test(flavor = "multi_thread")]
async fn after_delete_identity_an_agent_is_still_listed_under_its_name_on_a_new_session() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    let mut observed = Vec::new();
    for name in [DECLARED, CREATED] {
        let marker = format!("BEFORE-DELETE {name}");
        assert!(rig.turn(name, &marker).await, "{name}: a turn before");
        let before = rig.life(name).await;
        let deleted = rig.verb("meta/delete_identity", name).await;
        assert!(
            deleted["error"].is_null(),
            "{name}: the delete is served: {deleted:#?}"
        );
        let after = rig.life(name).await;
        let listed = rig
            .runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .statuses()
            .await
            .expect("the statuses read")
            .iter()
            .any(|status| status.identity.as_str() == name);
        let marker = format!("AFTER-DELETE {name}");
        let delivers = rig.turn(name, &marker).await;
        observed.push((
            name,
            (
                listed,
                rig.registered(name).await,
                after.session.is_some() && after.session != before.session,
                after
                    .generation
                    .unwrap_or_default()
                    .saturating_sub(before.generation.unwrap_or_default()),
                delivers,
            ),
        ));
    }
    assert_eq!(
        observed[0].1,
        (true, true, true, 1, true),
        "{}: still listed, a new session, the next generation, the next delivery",
        observed[0].0
    );
    assert_eq!(
        observed[1].1, observed[0].1,
        "{} gets what {} gets",
        observed[1].0, observed[0].0
    );
    let row = rig
        .runtime
        .mob_handle()
        .identity_record(&meerkat_mob::AgentIdentity::from(CREATED))
        .await
        .expect("the row reads");
    assert!(
        matches!(
            &row,
            Some(meerkat_mob::CreatedIdentityObservation::Created(record))
                if record.presence == meerkat_mob::CreatedIdentityPresence::Present
        ),
        "the store-owned identity's row stays present: {row:?}"
    );
    rig.shutdown().await;
}

/// The host's topology provider: it records the identities each call names
/// and the specs its context lists, and answers the edges the row set.
#[derive(Default)]
struct RecordingTopology {
    targets: Mutex<Vec<Vec<String>>>,
    listed: Mutex<Vec<Vec<String>>>,
    edges: Mutex<Vec<ManagedPeerEdge>>,
}

#[async_trait]
impl TopologyProvider for RecordingTopology {
    async fn compute_edges(
        &self,
        target_identities: &[AgentIdentity],
        context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        self.targets.lock().unwrap().push(
            target_identities
                .iter()
                .map(|identity| identity.as_str().to_string())
                .collect(),
        );
        self.listed.lock().unwrap().push(
            context
                .roster
                .iter()
                .map(|spec| spec.identity.as_str().to_string())
                .collect(),
        );
        Ok(self.edges.lock().unwrap().clone())
    }
}

/// A topology provider sees every identity the runtime serves: after a
/// store-owned identity is created, the next pass names it among the
/// provider's target identities and lists its spec in the provider's
/// context, beside the configured identity, and an edge the provider answers
/// between the two is wired.
///
/// RED before the provider saw store-owned identities: its input was the
/// host's roster alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_topology_provider_sees_every_identity() {
    let temp = tempfile::tempdir().unwrap();
    let topology = Arc::new(RecordingTopology::default());
    let rig = boot_composed(
        temp.path(),
        &fresh_mob_id(),
        None,
        Some(Arc::clone(&topology) as Arc<dyn TopologyProvider>),
    )
    .await;
    rig.create(CREATED).await;
    *topology.edges.lock().unwrap() =
        vec![ManagedPeerEdge::new(id(DECLARED), id(CREATED)).expect("an edge")];

    rig.roster_pass().await;
    let targets = topology.targets.lock().unwrap().last().cloned();
    let listed = topology.listed.lock().unwrap().last().cloned();
    for (what, names) in [("target identities", targets), ("context", listed)] {
        let names = names.expect("the pass asked the provider");
        assert!(
            names.iter().any(|name| name == DECLARED) && names.iter().any(|name| name == CREATED),
            "the provider's {what} name the configured and the store-owned identity: {names:?}"
        );
    }
    assert!(
        wired(&rig, DECLARED, CREATED).await,
        "the edge the provider answered between the two is wired"
    );
    rig.shutdown().await;
}

/// The managed topology leaves a peer edge a store-owned identity's row
/// declares, and prunes one no owner declares: under a topology provider
/// that answers no edge, the row's owned edge stays wired after a pass, and
/// a plain edge wired between the store-owned identity and the configured
/// one, which neither the provider, the definition nor a row declares, is
/// unwired, as one between two configured identities is.
///
/// RED before the topology treated every identity alike: no edge of a
/// store-owned identity was ever pruned.
#[tokio::test(flavor = "multi_thread")]
async fn the_managed_topology_keeps_a_rows_owned_edge_and_prunes_an_undeclared_one() {
    const PEER: &str = "keeper-peer";
    let temp = tempfile::tempdir().unwrap();
    let topology = Arc::new(RecordingTopology::default());
    let rig = boot_composed(
        temp.path(),
        &fresh_mob_id(),
        None,
        Some(Arc::clone(&topology) as Arc<dyn TopologyProvider>),
    )
    .await;
    rig.create(PEER).await;
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(created_spec(CREATED))
            .with_peers(vec![meerkat_mob::OwnedIdentityEdge::new(PEER, peer_rule())]),
        CREATED,
    )
    .await;
    rig.runtime
        .mob_handle()
        .wire(
            meerkat_meta::member_comms_id::mob_member_id(CREATED),
            meerkat_meta::member_comms_id::mob_member_id(DECLARED),
        )
        .await
        .expect("a plain edge between the store-owned and the configured identity");
    assert!(wired(&rig, CREATED, PEER).await && wired(&rig, CREATED, DECLARED).await);

    rig.roster_pass().await;
    assert!(
        wired(&rig, CREATED, PEER).await,
        "the edge the row declares stays wired"
    );
    assert!(
        !wired(&rig, CREATED, DECLARED).await,
        "the edge no owner declares is pruned"
    );
    rig.shutdown().await;
}

/// The first build of a new store-owned identity carries the host
/// customizer's tools: an identity created with a first message is built by
/// Core for its kickoff, before any roster pass or door named it, and that
/// kickoff turn is offered the tool.
///
/// RED before Core asked the host's build preparer about that spawn: the
/// kickoff turn offered no customizer tool (only the member's next build
/// carried it).
#[tokio::test(flavor = "multi_thread")]
async fn the_first_build_of_a_new_identity_carries_customizer_tools() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot_with(
        temp.path(),
        &fresh_mob_id(),
        Some(Arc::new(ToolCustomizer) as Arc<dyn AgentCustomizer>),
    )
    .await;
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(created_spec(CREATED).with_initial_message(
            meerkat_core::ContentInput::Text("FIRST-MESSAGE".to_string()),
        )),
        CREATED,
    )
    .await;
    assert!(
        rig.asked("FIRST-MESSAGE").await,
        "the kickoff turn of the new identity runs"
    );
    let request = rig
        .request_with("FIRST-MESSAGE")
        .expect("the kickoff turn's request");
    assert!(
        offers_tool(&request),
        "the first build's turn is offered the host customizer's tool: {request}"
    );
    rig.shutdown().await;
}

/// The build of a store-owned identity that Core wakes before any roster
/// pass or door named it carries the host customizer's tools too: the turn
/// that build runs next is offered the tool.
///
/// RED before that build attached the identity's tool dispatcher: it offered
/// no customizer tool (only the member's next build carried it).
#[tokio::test(flavor = "multi_thread")]
async fn the_first_wake_of_a_new_identity_carries_customizer_tools() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot_with(
        temp.path(),
        &fresh_mob_id(),
        Some(Arc::new(ToolCustomizer) as Arc<dyn AgentCustomizer>),
    )
    .await;
    rig.create(CREATED).await;
    assert!(
        !rig.registered(CREATED).await,
        "no roster pass or door named the new identity yet"
    );
    rig.runtime
        .mob_handle()
        .ensure_member_live(&meerkat_mob::AgentIdentity::from(CREATED))
        .await
        .expect("Core wakes the new identity's member: its first build");
    let generation = rig.life(CREATED).await.generation;

    assert!(
        rig.turn(CREATED, "FIRST-BUILD-TURN").await,
        "the first build runs a turn"
    );
    assert_eq!(
        rig.life(CREATED).await.generation,
        generation,
        "the turn ran on the first build's incarnation"
    );
    let request = rig
        .request_with("FIRST-BUILD-TURN")
        .expect("the first build's turn");
    assert!(
        offers_tool(&request),
        "the first build's turn is offered the host customizer's tool: {request}"
    );
    rig.shutdown().await;
}

/// A complete roster's pass never retires a store-owned identity: the host's
/// roster does not list it, and after two passes Core still holds its member
/// seated on the same session at the same generation, the runtime serves it,
/// and it takes a delivery.
#[tokio::test(flavor = "multi_thread")]
async fn meta_roster_pass_never_retires_a_store_owned_identity() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    assert!(rig.turn(CREATED, "BEFORE-PASSES").await);
    let before = rig.life(CREATED).await;
    rig.roster_pass().await;
    rig.roster_pass().await;
    assert!(rig.seated(CREATED), "Core holds the member seated");
    assert!(rig.registered(CREATED).await, "the runtime serves it");
    let after = rig.life(CREATED).await;
    assert_eq!(
        (after.session, after.generation),
        (before.session, before.generation),
        "the passes moved nothing of it"
    );
    assert!(rig.turn(CREATED, "AFTER-PASSES").await);
    rig.shutdown().await;
}

/// The host's customizer of the start row: it counts the builds it composes.
#[derive(Default)]
struct CountingCustomizer {
    builds: AtomicU64,
}

#[async_trait]
impl AgentCustomizer for CountingCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        if spec.identity.as_str() != DECLARED {
            self.builds.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// A start serves the store-owned identities without building one: after a
/// restart over a state directory that holds three of them, each is served
/// and tracked by the bootstrap status, no build of one was composed, no
/// model was asked, and Core holds each member unmaterialized, through the
/// start's pass and a second one.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_builds_no_member_of_a_store_owned_identity() {
    const NAMES: [&str; 3] = ["keeper-one", "keeper-two", "keeper-three"];
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot(temp.path(), &mob_id).await;
    for name in NAMES {
        rig.create(name).await;
    }
    rig.shutdown_for_reopen().await;

    let customizer = Arc::new(CountingCustomizer::default());
    let rig = boot_with(
        temp.path(),
        &mob_id,
        Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>),
    )
    .await;
    rig.roster_pass().await;
    let bootstrap = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed")
        .identity_bootstrap_status();
    for name in NAMES {
        assert!(rig.registered(name).await, "{name} is served");
        assert!(
            bootstrap.identities.contains_key(&id(name)),
            "{name} is tracked by the bootstrap status: {bootstrap:?}"
        );
        assert!(
            rig.runtime
                .mob_handle()
                .member_unmaterialized(&meerkat_mob::AgentIdentity::from(name)),
            "{name}: Core holds the member unmaterialized"
        );
    }
    assert_eq!(
        customizer.builds.load(Ordering::SeqCst),
        0,
        "no build of a store-owned identity was composed"
    );
    assert!(
        rig.requests.lock().unwrap().is_empty(),
        "no model was asked"
    );
    rig.shutdown().await;
}

/// An instruction activation reaches a store-owned member as it reaches a
/// configured one: `meta/member_activate_instruction` appends the activation
/// (`applied`) and `meta/member_instruction_activations` reads it back, for
/// both, and neither wakes or rebuilds anything else.
#[tokio::test(flavor = "multi_thread")]
async fn an_instruction_activation_reaches_a_store_owned_member_as_a_configured_one() {
    const BODY: &str = "Answer in one sentence.";
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    let mut observed = Vec::new();
    for name in [DECLARED, CREATED] {
        let marker = format!("BEFORE-ACTIVATION {name}");
        assert!(rig.turn(name, &marker).await, "{name}: a turn before");
        let activated = rig
            .rpc(
                "meta/member_activate_instruction",
                json!({
                    "identity": name,
                    "activation": {
                        "revision": {
                            "namespace": "host",
                            "key": "persona",
                            "revision_id": "r1",
                            "content_sha256":
                                meerkat_core::InstructionContentDigest::for_body(BODY).as_str(),
                        },
                        "activation_id": "a1",
                        "expectation": {"kind": "absent"},
                        "body": BODY,
                    },
                }),
            )
            .await;
        let page = rig
            .rpc(
                "meta/member_instruction_activations",
                json!({"identity": name, "namespace": "host", "key": "persona"}),
            )
            .await;
        observed.push((
            name,
            (
                activated["error"].clone(),
                activated["result"]["receipt"]["disposition"].clone(),
                page["error"].clone(),
                page["result"]["page"]["records"]
                    .as_array()
                    .map(|records| {
                        records
                            .iter()
                            .map(|record| record["identity"]["activation_id"].clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            ),
        ));
    }
    assert_eq!(
        observed[0].1,
        (
            Value::Null,
            json!("applied"),
            Value::Null,
            vec![json!("a1")]
        ),
        "{}: the activation is applied and read back",
        observed[0].0
    );
    assert_eq!(
        observed[1].1, observed[0].1,
        "{} gets what {} gets",
        observed[1].0, observed[0].0
    );
    rig.shutdown().await;
}

/// A status never says that the host's verbs refuse a store-owned identity:
/// its `intent_managed` reads false, as a configured identity's does, in the
/// Rust status and in `meta/status_identity`.
///
/// RED before the verbs served store-owned identities: it read true.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_owned_identitys_status_reads_no_host_verb_refusal() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    for name in [DECLARED, CREATED] {
        let status = rig
            .runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .status(&id(name))
            .await
            .expect("the status reads");
        assert!(!status.intent_managed, "{name}: {status:?}");
        let wire = rig
            .rpc("meta/status_identity", json!({"identity": name}))
            .await;
        assert_eq!(
            wire["result"]["intent_managed"],
            json!(false),
            "{name}: {wire:#?}"
        );
    }
    rig.shutdown().await;
}

/// A held add of a name a store-owned identity holds adds nothing: it is
/// refused `ExistingContinuity`, as the add of a configured identity that
/// has continuity is, for a seated store-owned identity and for one that was
/// never seated (its host prepares it first).
///
/// RED before the add served both: it answered `identity_store_owned`.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_add_of_a_name_a_store_owned_identity_holds_is_refused_existing_continuity() {
    const PENDING: &str = "keeper-pending";
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(created_spec(PENDING))
            .with_preparation(meerkat_mob::IdentityPreparationMode::Required),
        PENDING,
    )
    .await;
    let context = rig
        .runtime
        .identity_first_context()
        .expect("identity-first context installed");
    for name in [CREATED, PENDING] {
        let refused = context
            .add_member_held(
                DurableAgentSpec {
                    identity: id(name),
                    ..declared_spec()
                },
                tokio::time::Instant::now() + Duration::from_mins(1),
                |_members, _completion| async { Ok(()) },
            )
            .await;
        assert!(
            matches!(
                &refused,
                Err(IdentityRuntimeError::HeldBootstrapRefused(
                    meerkat_meta::identity_first::HeldBootstrapRefusal::ExistingContinuity {
                        identity
                    }
                )) if identity.as_str() == name
            ),
            "{name}: the name is taken: {:?}",
            refused
                .as_ref()
                .map(|_| "started")
                .map_err(ToString::to_string)
        );
    }
    rig.shutdown().await;
}

/// A held add of a name a removed identity held adds nothing either: Core
/// keeps a removed identity's name (its tombstone), and the add is refused
/// with Core's own answer for such a name.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_add_of_a_removed_identitys_name_is_refused_with_cores_answer() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    rig.create(CREATED).await;
    rig.remove(CREATED).await;
    let context = rig
        .runtime
        .identity_first_context()
        .expect("identity-first context installed");
    let refused = context
        .add_member_held(
            DurableAgentSpec {
                identity: id(CREATED),
                ..declared_spec()
            },
            tokio::time::Instant::now() + Duration::from_mins(1),
            |_members, _completion| async { Ok(()) },
        )
        .await;
    assert!(
        matches!(
            &refused,
            Err(IdentityRuntimeError::HeldBootstrapRefused(
                meerkat_meta::identity_first::HeldBootstrapRefusal::ExistingContinuity { identity }
            )) if identity.as_str() == CREATED
        ),
        "the removed name is still held: the add answers ExistingContinuity: {:?}",
        refused
            .as_ref()
            .map(|_| "started")
            .map_err(ToString::to_string)
    );
    assert!(!rig.seated(CREATED), "nothing is seated under the name");
    rig.shutdown().await;
}

// ---------------------------------------------------------------------------
// One host build step: a store-owned agent is built from the host's inputs
// as a configured one
// ---------------------------------------------------------------------------

/// The model the host's customizer of the build rows gives every build.
const HOST_MODEL: &str = "gpt-5.4";
/// The system prompt it gives every build.
const HOST_PROMPT: &str = "HOST-SYSTEM-PROMPT: keep the desk's record.";
/// The instruction section it appends to every build.
const HOST_INSTRUCTION: &str = "HOST-INSTRUCTION: cite the workspace record.";
/// The profile of the build rows whose resume mask names the model.
const MASKED: &str = "masked";
/// The label the host's customizer of the build rows sets on every build:
/// its key and its value.
const HOST_LABEL: (&str, &str) = ("host_tier", "gold");

/// The labels of a created agent's row that carries the host's label.
fn host_labelled_row() -> BTreeMap<String, String> {
    let mut labels = created_labels();
    labels.insert(HOST_LABEL.0.to_string(), HOST_LABEL.1.to_string());
    labels
}

/// The spec of an agent created on `profile` whose row carries the host's
/// label: what a host that labels its members in its customizer creates.
fn host_labelled_spec(name: &str, profile: &str) -> meerkat_mob::CreatedIdentitySpec {
    meerkat_mob::CreatedIdentitySpec::new(name, profile)
        .with_display_name(format!("Created {name}"))
        .with_labels(host_labelled_row())
}

/// The application context the host's customizer gives every build.
fn host_context() -> Value {
    json!({"desk": "north", "host": true})
}

/// The provider parameters the host's customizer gives every build. The
/// routing key is the host's own, so no identity's default is derived.
fn host_provider_params() -> meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
    use meerkat_core::lifecycle::run_primitive::{
        OpenAiProviderTag, ProviderParamsOverride, ProviderTag,
    };
    ProviderParamsOverride {
        temperature: Some(0.25),
        provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
            prompt_cache_key: Some("host-build-rows".to_string()),
            ..Default::default()
        })),
        ..Default::default()
    }
}

/// The host's customizer of the build rows: it sets every field of the
/// draft, the same for every identity, and records each build it composes
/// by the identity's name.
#[derive(Default)]
struct ComposingCustomizer {
    builds: Mutex<Vec<String>>,
}

impl ComposingCustomizer {
    /// How many builds of `name` the customizer composed.
    fn builds_of(&self, name: &str) -> usize {
        self.builds
            .lock()
            .unwrap()
            .iter()
            .filter(|built| built.as_str() == name)
            .count()
    }
}

#[async_trait]
impl AgentCustomizer for ComposingCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        self.builds
            .lock()
            .unwrap()
            .push(spec.identity.as_str().to_string());
        draft.model = Some(HOST_MODEL.to_string());
        draft.system_prompt = Some(HOST_PROMPT.to_string());
        draft
            .additional_instructions
            .push(HOST_INSTRUCTION.to_string());
        draft
            .labels
            .insert(HOST_LABEL.0.to_string(), HOST_LABEL.1.to_string());
        draft.app_context = Some(host_context());
        draft.provider_params = Some(host_provider_params());
        draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(HostTools));
        Ok(())
    }
}

/// The build rows' definition: the worker profile, and a profile whose
/// resume mask names the model, so a rebuild on a durable session takes the
/// profile's model over the session's own.
fn build_rows_definition(mob_id: &str) -> MobDefinition {
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

[profiles.masked]
model = "gpt-5.5"
resume_overrides = ["model", "provider"]
external_addressable = true
runtime_mode = "turn_driven"

[profiles.masked.tools]
comms = true
"#
    ))
    .expect("parse the definition")
}

/// Boot the build rows' host on `state`: the configured identity on
/// `profile`, the customizer that sets every draft field, and a members'
/// model that reports no selection of its own, so each request names the
/// model its member was built with.
async fn boot_build_rows(
    state: &Path,
    mob_id: &str,
    profile: &str,
) -> (Rig, Arc<ComposingCustomizer>) {
    let customizer = Arc::new(ComposingCustomizer::default());
    let rig = boot_host(
        state,
        build_rows_definition(mob_id),
        DurableAgentSpec {
            profile: ProfileName::from(profile),
            ..declared_spec()
        },
        Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>),
        None,
        false,
    )
    .await;
    (rig, customizer)
}

/// What one build of an identity's member took from the host's build step,
/// read from the facts that carry it: the turn's model request, Core's
/// roster entry and the session's durable build state.
#[derive(Debug, Clone, PartialEq)]
struct BuildInputs {
    /// The model the build's turn asked.
    model: Value,
    /// The model override Core's roster entry holds for the member.
    model_override: Option<String>,
    /// The model and the provider parameters of the profile override Core's
    /// roster entry holds for the member.
    profile_override: Option<(String, Value)>,
    /// Whether the turn's request carries the host's system prompt.
    system_prompt: bool,
    /// Whether it carries the host's instruction section.
    instruction: bool,
    /// The sampling temperature and the provider parameters it carries.
    temperature: Value,
    provider_params: Value,
    /// The application context the session's durable build state holds.
    context: Option<Value>,
    /// Whether the turn was offered the host's tool.
    tool: bool,
}

impl Rig {
    /// The inputs the build that ran the turn `marker` of `name` took.
    async fn build_inputs(&self, name: &str, marker: &str) -> BuildInputs {
        let rendered = self
            .request_with(marker)
            .unwrap_or_else(|| panic!("{name}: the turn {marker:?} was asked"));
        let request: Value = serde_json::from_str(&rendered).expect("a rendered model request");
        let entry = self
            .runtime
            .mob_handle()
            .get_member(&meerkat_meta::member_comms_id::mob_member_id(name))
            .await
            .expect("Core's roster reads")
            .unwrap_or_else(|| panic!("Core holds a member of {name}"));
        let session = self
            .life(name)
            .await
            .session
            .unwrap_or_else(|| panic!("{name} is bound to a session"));
        let session_id =
            meerkat_core::types::SessionId::parse(&session).expect("the member's session id");
        let stored = meerkat_mob::MobSessionService::load_persisted_session(
            &**self
                .runtime
                .mob_runtime()
                .session_service()
                .expect("the runtime's session service"),
            &session_id,
        )
        .await
        .expect("the member's session reads")
        .unwrap_or_else(|| panic!("{name}: the session {session} is stored"));
        BuildInputs {
            model: request["model"].clone(),
            model_override: entry.effective_model_override.clone(),
            profile_override: entry.effective_profile_override.as_ref().map(|profile| {
                (
                    profile.model.clone(),
                    serde_json::to_value(&profile.provider_params)
                        .expect("the profile's provider parameters render"),
                )
            }),
            system_prompt: rendered.contains(HOST_PROMPT),
            instruction: rendered.contains(HOST_INSTRUCTION),
            temperature: request["temperature"].clone(),
            provider_params: request["provider_params"].clone(),
            context: stored
                .try_build_state()
                .expect("the session's build state reads")
                .and_then(|state| state.app_context),
            tool: offers_tool(&rendered),
        }
    }
}

/// Fail unless `inputs`, a build of `name`, took every input the host's
/// customizer sets.
fn assert_host_inputs(name: &str, inputs: &BuildInputs) {
    assert_eq!(
        inputs.model,
        json!(HOST_MODEL),
        "{name}: the build runs the host's model: {inputs:#?}"
    );
    assert!(
        inputs.system_prompt && inputs.instruction,
        "{name}: the build carries the host's system prompt and instruction: {inputs:#?}"
    );
    assert_eq!(
        inputs.context,
        Some(host_context()),
        "{name}: the build carries the host's context: {inputs:#?}"
    );
    assert_eq!(
        inputs
            .profile_override
            .as_ref()
            .map(|(_, params)| params["temperature"].clone()),
        Some(json!(0.25)),
        "{name}: the build carries the host's provider parameters: {inputs:#?}"
    );
    assert!(
        inputs.tool,
        "{name}: the build is offered the host's tool: {inputs:#?}"
    );
}

/// A store-owned agent is built from the host's build step as a configured
/// agent on the same profile is: the model, the system prompt and its
/// instruction section, the provider parameters, the context and the tools
/// the host's customizer sets reach the first build of an agent created
/// with a first message (its kickoff spawn) and of one created Dormant and
/// then sent to, and each equals the configured agent's field by field. The
/// host's customizer composes each of those first builds once.
///
/// Each created agent's row carries the label the host's customizer sets,
/// as it must for the agent to be seated.
///
/// RED before Core asked the host to compose a store-owned seat: both
/// created agents were built on the profile's own model with no host system
/// prompt, instruction, provider parameters or context (only the host's tool
/// reached them).
#[tokio::test(flavor = "multi_thread")]
async fn a_store_owned_agent_is_built_from_the_hosts_inputs_as_a_configured_one() {
    const KICKED: &str = "keeper-kicked";
    const SEATED: &str = "keeper-seated";
    let temp = tempfile::tempdir().unwrap();
    let (mut rig, customizer) = boot_build_rows(temp.path(), &fresh_mob_id(), "worker").await;

    assert!(
        rig.turn(DECLARED, "BUILD-CONFIGURED").await,
        "the configured agent runs a turn"
    );
    let configured = rig.build_inputs(DECLARED, "BUILD-CONFIGURED").await;
    assert_host_inputs(DECLARED, &configured);

    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(
            host_labelled_spec(KICKED, "worker").with_initial_message(
                meerkat_core::ContentInput::Text("BUILD-KICKOFF".to_string()),
            ),
        ),
        KICKED,
    )
    .await;
    assert!(
        rig.asked("BUILD-KICKOFF").await,
        "the kickoff turn of the agent created with a first message runs"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let kicked = rig.build_inputs(KICKED, "BUILD-KICKOFF").await;

    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(host_labelled_spec(SEATED, "worker")),
        SEATED,
    )
    .await;
    assert!(
        rig.turn(SEATED, "BUILD-SEATED").await,
        "the agent created Dormant runs its first turn"
    );
    let seated = rig.build_inputs(SEATED, "BUILD-SEATED").await;

    assert_eq!(
        kicked, configured,
        "an agent created with a first message is built from the host's inputs as the \
         configured agent is"
    );
    assert_eq!(
        seated, configured,
        "an agent created Dormant and then sent to is built from the host's inputs as the \
         configured agent is"
    );
    assert_eq!(
        (customizer.builds_of(KICKED), customizer.builds_of(SEATED)),
        (1, 1),
        "the host's customizer composes each first build once"
    );
    rig.shutdown().await;
}

/// A reset leaves a store-owned agent's build inputs as they were: the
/// first build of its fresh life takes from the host's build step what the
/// first build of its first life took, field by field, on a new session.
///
/// The agent's row carries the label the host's customizer sets, as it
/// must for the agent to be seated.
///
/// RED before Core asked the host to compose a store-owned seat: neither
/// life took the host's model, system prompt, instruction, provider
/// parameters or context.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_store_owned_agent_is_built_from_the_host_inputs_of_its_first_life() {
    let temp = tempfile::tempdir().unwrap();
    let (mut rig, _customizer) = boot_build_rows(temp.path(), &fresh_mob_id(), "worker").await;
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(host_labelled_spec(CREATED, "worker")),
        CREATED,
    )
    .await;
    assert!(
        rig.turn(CREATED, "FIRST-LIFE").await,
        "the first life runs a turn"
    );
    let first = rig.build_inputs(CREATED, "FIRST-LIFE").await;
    assert_host_inputs(CREATED, &first);
    let before = rig.life(CREATED).await;

    let reset = rig.verb("meta/reset", CREATED).await;
    assert!(reset["error"].is_null(), "the reset is served: {reset:#?}");
    let after = rig.life(CREATED).await;
    assert!(
        after.session.is_some() && after.session != before.session,
        "the reset seats a fresh session: {before:?} -> {after:?}"
    );
    assert!(
        rig.turn(CREATED, "SECOND-LIFE").await,
        "the fresh life runs a turn"
    );
    let second = rig.build_inputs(CREATED, "SECOND-LIFE").await;
    assert_eq!(
        second, first,
        "the fresh life is built from the host's inputs as the first life was"
    );
    rig.shutdown().await;
}

/// Under a profile whose resume mask names the model, a store-owned agent
/// and a configured agent on that profile build with the same model, the
/// host's, in the process that first built them and again after a restart
/// of the runtime on the same state directory, where each is rebuilt on its
/// durable session and the mask takes the profile's model over the
/// session's own.
///
/// RED before Core asked the host to compose a store-owned seat at every
/// build: the created agent was built on the profile's own model.
#[tokio::test(flavor = "multi_thread")]
async fn under_a_resume_model_mask_a_store_owned_agent_builds_with_the_configured_agents_model() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let (mut rig, _customizer) = boot_build_rows(temp.path(), &mob_id, MASKED).await;
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(host_labelled_spec(CREATED, MASKED)),
        CREATED,
    )
    .await;
    assert!(rig.turn(DECLARED, "MASK-CONFIGURED-FIRST").await);
    assert!(rig.turn(CREATED, "MASK-CREATED-FIRST").await);
    let configured = rig.build_inputs(DECLARED, "MASK-CONFIGURED-FIRST").await;
    let created = rig.build_inputs(CREATED, "MASK-CREATED-FIRST").await;
    assert_eq!(
        configured.model,
        json!(HOST_MODEL),
        "the configured agent's first build runs the host's model: {configured:#?}"
    );
    assert_eq!(
        (
            &created.model,
            &created.model_override,
            &created.profile_override
        ),
        (
            &configured.model,
            &configured.model_override,
            &configured.profile_override
        ),
        "the created agent's first build runs the configured agent's model"
    );
    rig.shutdown_for_reopen().await;

    let (mut rig, customizer) = boot_build_rows(temp.path(), &mob_id, MASKED).await;
    assert!(rig.turn(DECLARED, "MASK-CONFIGURED-AGAIN").await);
    assert!(rig.turn(CREATED, "MASK-CREATED-AGAIN").await);
    let configured = rig.build_inputs(DECLARED, "MASK-CONFIGURED-AGAIN").await;
    let created = rig.build_inputs(CREATED, "MASK-CREATED-AGAIN").await;
    assert_eq!(
        configured.model,
        json!(HOST_MODEL),
        "the configured agent's rebuild runs the host's model: {configured:#?}"
    );
    assert_eq!(
        (
            &created.model,
            &created.model_override,
            &created.profile_override
        ),
        (
            &configured.model,
            &configured.model_override,
            &configured.profile_override
        ),
        "the created agent's rebuild after the restart runs the configured agent's model"
    );
    assert!(
        customizer.builds_of(CREATED) >= 1,
        "the host is asked again at the rebuild of the created agent"
    );
    rig.shutdown().await;
}

// ---------------------------------------------------------------------------
// A label the host's customizer sets: a created agent's row must carry it
// ---------------------------------------------------------------------------

/// The subject the label rows' access rules are decided for.
const LABEL_SUBJECT: &str = "alice@example.test";

/// The access configuration of the label rows. Everyone may send to any
/// agent, except to one that carries the host's label: the deny rule keyed
/// on that label. Only an agent that carries it may be viewed: the allow
/// rule keyed on it.
fn label_access() -> meerkat_meta::AccessControlConfig {
    let host_labelled = BTreeMap::from([(HOST_LABEL.0.to_string(), HOST_LABEL.1.to_string())]);
    meerkat_meta::AccessControlConfig {
        approvers: Default::default(),
        enabled: true,
        admins: Vec::new(),
        groups: BTreeMap::new(),
        rules: vec![
            meerkat_meta::AccessRule {
                id: "send-any".to_string(),
                actions: vec!["agent.send".to_string()],
                ..meerkat_meta::AccessRule::default()
            },
            meerkat_meta::AccessRule {
                id: "no-send-to-host-labelled".to_string(),
                effect: meerkat_meta::AccessEffect::Deny,
                actions: vec!["agent.send".to_string()],
                match_labels: host_labelled.clone(),
                ..meerkat_meta::AccessRule::default()
            },
            meerkat_meta::AccessRule {
                id: "view-host-labelled".to_string(),
                actions: vec!["agent.view".to_string()],
                match_labels: host_labelled,
                ..meerkat_meta::AccessRule::default()
            },
        ],
    }
}

/// What [`label_access`] decides about the member `seat` of `name`, read
/// from Core's roster entry as the console reads a member's attributes:
/// whether a send to it is allowed, and whether it may be viewed.
fn label_decisions(name: &str, seat: &Seat) -> (bool, bool) {
    let principal = meerkat_meta::AccessPrincipal {
        subject: Some(LABEL_SUBJECT.to_string()),
        groups: Default::default(),
    };
    let resource = meerkat_meta::AccessResource {
        identity: Some(name),
        agent_id: None,
        role: Some(seat.role.as_str()),
        labels: Some(&seat.labels),
    };
    let config = label_access();
    let allowed = |action: &str| {
        meerkat_meta::evaluate_access(&config, &principal, action, &resource).is_allow()
    };
    (allowed("agent.send"), allowed("agent.view"))
}

/// The decisions for a member that carries the host's label: the deny holds
/// (no send), and the allow rule matches (view).
const HOST_LABELLED: (bool, bool) = (false, true);

/// What the identity's status shows of its seat: its state, and the reason
/// its build is refused for, when one stands.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SeatStatus {
    state: String,
    refused: Option<String>,
}

impl SeatStatus {
    /// Whether the status shows the label refusal: it names the host's
    /// label key and the remedy (the row), and no label value.
    fn shows_the_label_refusal(&self) -> bool {
        self.refused.as_deref().is_some_and(names_the_label_key)
    }
}

/// Whether `reason` names the host's label key and the remedy (the label
/// belongs in the agent's row), and not the label's value.
fn names_the_label_key(reason: &str) -> bool {
    reason.contains(&format!("added [{}]", HOST_LABEL.0))
        && reason.contains("row")
        && !reason.contains(HOST_LABEL.1)
}

impl Rig {
    async fn seat_status(&self, name: &str) -> SeatStatus {
        let status = self
            .runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .status(&id(name))
            .await
            .unwrap_or_else(|error| panic!("{name}: the status reads: {error}"));
        SeatStatus {
            state: status.state.wire_str().to_string(),
            refused: status
                .customizer_tools_pending
                .map(|pending| pending.reason),
        }
    }

    /// Change `name`'s row through Core's store-owned update, from `from`
    /// to `to`, as the operation `operation_id`.
    async fn update_from(
        &self,
        operation_id: &str,
        from: &meerkat_mob::CreatedIdentitySpec,
        to: meerkat_mob::CreatedIdentitySpec,
    ) {
        let name = to.identity.to_string();
        let updated = self
            .runtime
            .mob_handle()
            .update_store_owned_identity(
                meerkat_mob::StoreOwnedIdentityUpdate::new(
                    operation(operation_id),
                    meerkat_mob::IdentityChangeRequesterInput::host("console"),
                    to,
                    from.spec_digest().expect("the row's digest"),
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

    /// Whether a build created the session Core binds `name`'s member to:
    /// the session is stored.
    async fn session_built(&self, name: &str) -> bool {
        let Some(session) = self.life(name).await.session else {
            return false;
        };
        let session_id =
            meerkat_core::types::SessionId::parse(&session).expect("the member's session id");
        meerkat_mob::MobSessionService::load_persisted_session(
            &**self
                .runtime
                .mob_runtime()
                .session_service()
                .expect("the runtime's session service"),
            &session_id,
        )
        .await
        .expect("the member's session reads")
        .is_some()
    }
}

/// A label the host's customizer sets on every build reaches a configured
/// agent's seat, where the access rules keyed on it decide: the deny holds
/// and the allow rule matches. A created agent whose row lacks that label is
/// refused its seat instead of being seated without it: created Dormant and
/// then sent to, the send is refused with a reason that names the label key
/// and the remedy and no value, nothing is built and no turn runs; created
/// with a first message, its kickoff does not run and Core holds no member
/// of it. The host's customizer was asked about each.
///
/// RED before the rule: both created agents were built and ran, on seats
/// that did not carry the host's label, so the deny rule missed them.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_agent_whose_row_lacks_a_host_label_is_refused_its_seat_and_not_built() {
    const KICKED: &str = "keeper-kicked";
    const SEATED: &str = "keeper-seated";
    let temp = tempfile::tempdir().unwrap();
    let (mut rig, customizer) = boot_build_rows(temp.path(), &fresh_mob_id(), "worker").await;

    assert!(
        rig.turn(DECLARED, "LABEL-CONFIGURED").await,
        "the configured agent runs a turn"
    );
    let configured = rig.member(DECLARED).await;
    assert_eq!(
        configured.labels.get(HOST_LABEL.0).map(String::as_str),
        Some(HOST_LABEL.1),
        "the configured agent's seat carries the host's label: {:?}",
        configured.labels
    );
    assert_eq!(
        label_decisions(DECLARED, &configured),
        HOST_LABELLED,
        "the deny holds for the configured agent, and the allow rule matches it"
    );

    rig.create(SEATED).await;
    let sent = rig.send(SEATED, "LABEL-SEATED").await;
    assert!(
        !sent["error"].is_null() && names_the_label_key(&sent["error"].to_string()),
        "the send to an agent whose row lacks the host's label is refused, naming the label \
         key and the remedy: {sent}"
    );
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(created_spec(KICKED).with_initial_message(
            meerkat_core::ContentInput::Text("LABEL-KICKOFF".to_string()),
        )),
        KICKED,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        rig.request_with("LABEL-SEATED").is_none() && rig.request_with("LABEL-KICKOFF").is_none(),
        "no turn of a refused agent runs"
    );
    assert!(
        !rig.session_built(SEATED).await,
        "nothing is built for the agent created Dormant: no build created its session"
    );
    assert!(
        !rig.seated(KICKED),
        "Core holds no member of the agent created with a first message"
    );
    assert!(
        customizer.builds_of(SEATED) >= 1 && customizer.builds_of(KICKED) >= 1,
        "the host's customizer was asked about each refused agent"
    );
    rig.shutdown().await;
}

/// A created agent whose row carries the label the host's customizer sets
/// is built, its seat carries the label, and the access rules keyed on it
/// decide as for a configured agent: the deny holds and the allow rule
/// matches.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_agent_whose_row_carries_the_host_label_is_built_and_the_deny_holds() {
    let temp = tempfile::tempdir().unwrap();
    let (mut rig, _customizer) = boot_build_rows(temp.path(), &fresh_mob_id(), "worker").await;
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(host_labelled_spec(CREATED, "worker")),
        CREATED,
    )
    .await;
    assert!(
        rig.turn(CREATED, "LABEL-IN-THE-ROW").await,
        "the agent whose row carries the host's label is built and runs"
    );
    let created = rig.member(CREATED).await;
    assert_eq!(
        created.labels,
        host_labelled_row(),
        "its seat carries its row's labels, the host's among them"
    );
    assert_eq!(
        label_decisions(CREATED, &created),
        HOST_LABELLED,
        "the deny holds for the created agent, and the allow rule matches it"
    );
    assert_eq!(rig.seat_status(CREATED).await.refused, None);
    rig.shutdown().await;
}

/// A created agent refused its seat for a label its row lacks is visible as
/// such, and its row and a seat of its member are all that has to change
/// for it to be built: no restart and no store edit. Its status shows the
/// reason, naming the label key and the remedy, with the state `broken`
/// for one refused at its first wake and `dormant` for one owed its seat.
/// An update of its row that adds the label does not change the seat Core
/// holds, so the next send is still refused; a respawn, or a reset, then
/// seats its member with the row's labels, and it is built at its next
/// wake: its seat carries the label and the deny holds. An agent created
/// with a first message, which Core holds no member of, is seated by the
/// update itself and runs its kickoff. An archive and a removal of a
/// refused agent are served.
///
/// RED before the rule: the agents were built without the label.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_created_agent_shows_the_reason_and_is_seated_once_its_row_carries_the_label() {
    const KICKED: &str = "keeper-kicked";
    const SEATED: &str = "keeper-seated";
    const ARCHIVED: &str = "keeper-archived";
    const REMOVED: &str = "keeper-removed";
    const RESET: &str = "keeper-reset";
    let temp = tempfile::tempdir().unwrap();
    let (mut rig, _customizer) = boot_build_rows(temp.path(), &fresh_mob_id(), "worker").await;

    // Created Dormant: refused at its first wake.
    rig.create(SEATED).await;
    let sent = rig.send(SEATED, "REFUSED-WAKE").await;
    assert!(!sent["error"].is_null(), "the send is refused: {sent}");
    let refused = rig.seat_status(SEATED).await;
    assert!(
        refused.shows_the_label_refusal(),
        "the status shows the refusal's reason: {refused:?}"
    );
    assert_eq!(refused.state, "broken", "Core holds the member Broken");

    rig.update(SEATED, host_labelled_spec(SEATED, "worker"))
        .await;
    let again = rig.send(SEATED, "AFTER-UPDATE").await;
    assert!(
        !again["error"].is_null() && names_the_label_key(&again["error"].to_string()),
        "the update leaves the seat Core holds as it was, so the send is still refused: \
         {again}"
    );
    assert_eq!(
        rig.seat_status(SEATED).await,
        refused,
        "the status still shows the refusal"
    );
    let respawned = rig.verb("meta/respawn", SEATED).await;
    assert!(
        respawned["error"].is_null(),
        "the respawn that seats the member with its row's labels is served: {respawned:#?}"
    );
    assert!(
        rig.turn(SEATED, "AFTER-RESPAWN").await,
        "the agent is built once its seat carries the label"
    );
    let seat = rig.member(SEATED).await;
    assert_eq!(seat.labels, host_labelled_row());
    assert_eq!(label_decisions(SEATED, &seat), HOST_LABELLED);
    let cleared = rig.seat_status(SEATED).await;
    assert_eq!(
        (cleared.state.as_str(), &cleared.refused),
        ("active", &None),
        "the refusal is gone from the status"
    );

    // Created with a first message: owed its seat, seated by the update.
    let kicked = created_spec(KICKED).with_initial_message(meerkat_core::ContentInput::Text(
        "KICKOFF-ONCE-LABELLED".to_string(),
    ));
    rig.create_request(
        meerkat_mob::CreatedIdentityRequest::new(kicked.clone()),
        KICKED,
    )
    .await;
    assert!(!rig.seated(KICKED), "the agent is owed its seat");
    let owed = rig.seat_status(KICKED).await;
    assert!(
        owed.shows_the_label_refusal(),
        "the status shows the refusal's reason: {owed:?}"
    );
    assert_eq!(owed.state, "dormant", "Core holds no member of it");
    rig.update_from(
        "update-kicked",
        &kicked,
        kicked.clone().with_labels(host_labelled_row()),
    )
    .await;
    assert!(
        rig.asked("KICKOFF-ONCE-LABELLED").await,
        "the update seats the agent, and its kickoff runs"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let seat = rig.member(KICKED).await;
    assert_eq!(seat.labels, host_labelled_row());
    assert_eq!(label_decisions(KICKED, &seat), HOST_LABELLED);
    assert_eq!(rig.seat_status(KICKED).await.refused, None);

    // A refused agent is archived, and another removed.
    for name in [ARCHIVED, REMOVED, RESET] {
        rig.create(name).await;
        let sent = rig.send(name, &format!("REFUSED {name}")).await;
        assert!(
            !sent["error"].is_null(),
            "{name}: the send is refused: {sent}"
        );
    }
    rig.archive(ARCHIVED).await;
    rig.remove(REMOVED).await;
    assert!(
        !rig.seated(ARCHIVED) && !rig.seated(REMOVED),
        "neither refused agent keeps a member"
    );

    // A reset seats a refused agent's member with its row's labels, as a
    // respawn does.
    rig.update(RESET, host_labelled_spec(RESET, "worker")).await;
    let reset = rig.verb("meta/reset", RESET).await;
    assert!(reset["error"].is_null(), "the reset is served: {reset:#?}");
    assert!(
        rig.turn(RESET, "AFTER-RESET-LABELLED").await,
        "the agent is built once a reset seated it with the label"
    );
    let seat = rig.member(RESET).await;
    assert_eq!(seat.labels, host_labelled_row());
    assert_eq!(label_decisions(RESET, &seat), HOST_LABELLED);
    rig.shutdown().await;
}
