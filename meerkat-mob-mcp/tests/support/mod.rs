//! Real-component fixtures for the temporary-council lane (issue #159).
//!
//! Everything here composes production types: a real
//! `PersistentSessionService` over a real `AgentFactory`, a real `MobMcpState`
//! with explicitly rooted durable custody, real mobs, real members, and real
//! source-owned forked-participant capabilities. The only substitution is the
//! LLM itself, which is a deterministic scripted client — the council's own
//! machinery is never stubbed.

#![allow(dead_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::manual_assert
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_mob::temporary_council::TemporaryCouncilId;
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobControlPrincipal, MobDefinition, MobId, ProfileBinding,
    ProfileName,
};
use meerkat_mob_mcp::MobMcpState;

// ===========================================================================
// Deterministic scripted LLM
// ===========================================================================

/// What the scripted client should do for one turn.
pub enum ScriptedTurn {
    /// Emit this exact assistant text.
    Text(String),
    /// Fail the provider call, so the member turn fails terminally.
    Fail(String),
    /// Block until the gate opens, then emit this text.
    ///
    /// This is how a test observes a council WHILE it is mid-flight: real
    /// member turns are genuinely in progress, so the temporary mob, its
    /// wiring, and its capability attachments are all live and readable.
    Gated(Arc<TurnGate>, String),
}

/// A release gate a scripted turn blocks on.
#[derive(Default)]
pub struct TurnGate {
    open: std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>,
    entered: AtomicUsize,
}

impl TurnGate {
    pub fn new() -> Arc<Self> {
        let (tx, _rx) = tokio::sync::watch::channel(false);
        Arc::new(Self {
            open: std::sync::Mutex::new(Some(tx)),
            entered: AtomicUsize::new(0),
        })
    }

    fn receiver(&self) -> tokio::sync::watch::Receiver<bool> {
        self.open
            .lock()
            .unwrap()
            .as_ref()
            .expect("gate sender retained")
            .subscribe()
    }

    /// Release every blocked turn, now and in the future.
    pub fn open(&self) {
        let guard = self.open.lock().unwrap();
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(true);
        }
    }

    /// How many turns have reached the gate.
    pub fn entered(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }

    /// Wait until at least `count` turns have reached the gate.
    pub async fn wait_entered(&self, count: usize) {
        for _ in 0..600 {
            if self.entered() >= count {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!(
            "only {} turn(s) reached the gate, expected {count}",
            self.entered()
        );
    }

    /// Block the current task until [`Self::open`] is called.
    pub async fn wait(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let mut rx = self.receiver();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }
}

type Script = Arc<dyn Fn(&LlmRequest) -> ScriptedTurn + Send + Sync>;

/// A deterministic LLM whose reply is a pure function of the request.
///
/// Councils need distinguishable, request-dependent participant voices, which
/// `TestClient`'s fixed "ok" cannot provide. Every reply here is derived from
/// the request text, so a replay that takes no turns is observable as an
/// unchanged call count.
pub struct ScriptedCouncilClient {
    script: Script,
    calls: Arc<AtomicUsize>,
}

impl ScriptedCouncilClient {
    pub fn new(script: impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static) -> Self {
        Self {
            script: Arc::new(script),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Shared counter of provider calls actually issued.
    pub fn calls(&self) -> Arc<AtomicUsize> {
        self.calls.clone()
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedCouncilClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let events = match (self.script)(request) {
            ScriptedTurn::Text(text) => vec![
                LlmEvent::TextDelta {
                    delta: text,
                    meta: None,
                },
                LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Anthropic,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                },
                LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ],
            ScriptedTurn::Fail(reason) => {
                return Box::pin(futures::stream::iter(vec![Err(LlmError::InvalidRequest {
                    message: reason,
                })]));
            }
            ScriptedTurn::Gated(gate, text) => {
                let model = request.model.clone();
                let released = async move {
                    gate.entered.fetch_add(1, Ordering::SeqCst);
                    let mut rx = gate.receiver();
                    while !*rx.borrow_and_update() {
                        if rx.changed().await.is_err() {
                            break;
                        }
                    }
                    vec![
                        LlmEvent::TextDelta {
                            delta: text,
                            meta: None,
                        },
                        LlmEvent::UsageUpdate {
                            usage: meerkat_core::TurnUsage::host_declared(
                                meerkat_core::Provider::Anthropic,
                                &model,
                                meerkat_core::Usage::default(),
                            ),
                        },
                        LlmEvent::Done {
                            outcome: meerkat_client::LlmDoneOutcome::Success {
                                stop_reason: meerkat_core::StopReason::EndTurn,
                            },
                        },
                    ]
                };
                return Box::pin(futures::StreamExt::flat_map(
                    futures::stream::once(released),
                    |events| futures::stream::iter(events.into_iter().map(Ok)),
                ));
            }
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Concatenated user-channel text of a request.
pub fn user_text(request: &LlmRequest) -> String {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            meerkat_core::Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text of the last user-channel message in a request.
pub fn last_user_text(request: &LlmRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            meerkat_core::Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The council role named by a round prompt ("You are '<role>'").
pub fn role_in_request(request: &LlmRequest) -> Option<String> {
    let haystack = user_text(request);
    let marker = "You are '";
    let start = haystack.rfind(marker)? + marker.len();
    let rest = &haystack[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

// ===========================================================================
// Real session service + MobMcpState
// ===========================================================================

pub struct CouncilFixture {
    /// Per-fixture identity scope.
    ///
    /// Every mob id, council id, and therefore every derived comms peer name
    /// is scoped by this, so two tests running CONCURRENTLY in one process can
    /// never collide on a process-global inproc route. The suite is parallel
    /// safe by construction, not by a serializing lock.
    pub scope: String,
    pub state: Arc<MobMcpState>,
    pub service: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    pub runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    pub calls: Arc<AtomicUsize>,
    pub root: std::path::PathBuf,
    pub temp: tempfile::TempDir,
}

fn persistent_service(
    root: &std::path::Path,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    client: Arc<dyn LlmClient>,
) -> Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>> {
    let project_root = root.join("project-root");
    let context_root = root.join("context-root");
    for dir in [&project_root, &context_root] {
        std::fs::create_dir_all(dir).expect("create project/context root");
        std::fs::write(
            dir.join("AGENTS.md"),
            "# Temporary council fixture\n\nDeterministic scripted harness.\n",
        )
        .expect("write AGENTS.md");
    }
    let factory = meerkat::AgentFactory::new(root.join("factory-store"))
        .user_config_root(root.join("user-config"))
        .runtime_root(root.join("runtime-root"))
        .project_root(project_root.clone())
        .context_root(context_root)
        .builtins(false)
        .comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    builder.default_llm_client = Some(client);
    let store = Arc::new(meerkat_store::JsonlStore::new(root.join("sessions-jsonl")));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));
    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::default());
    Arc::new(
        meerkat_session::PersistentSessionService::new(
            builder,
            32,
            store_dyn,
            runtime_store,
            blob_store,
        )
        .with_event_projection(
            Arc::new(meerkat_session::event_store::FileEventStore::new(
                project_root.join(".rkat").join("events"),
            )),
            Arc::new(meerkat_session::projector::SessionProjector::new(
                project_root.join(".rkat"),
            )),
        ),
    )
}

impl CouncilFixture {
    /// Build a fixture with an explicitly rooted durable realm custody.
    pub fn new(script: impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static) -> Self {
        Self::new_with(script, |state, _root| state)
    }

    /// [`Self::new`] with a chance to inject custom durable stores BEFORE the
    /// persistent root is installed (a caller-supplied store is never replaced
    /// by the root's default upgrade).
    pub fn new_with(
        script: impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static,
        customize: impl FnOnce(MobMcpState, &std::path::Path) -> MobMcpState,
    ) -> Self {
        let temp = tempfile::tempdir().expect("council temp dir");
        let root = temp.path().to_path_buf();
        let scope = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let client = Arc::new(ScriptedCouncilClient::new(script));
        let calls = client.calls();
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let service = persistent_service(&root, runtime_store.clone(), client);
        let state_root = root.join("state");
        let state = customize(
            MobMcpState::new(service.clone(), MobControlPrincipal::Owner),
            &state_root,
        );
        let state = state
            .try_with_persistent_storage_root(Some(state_root))
            .expect("open rooted council + capability custody")
            .into_shared();
        Self {
            scope,
            state,
            service,
            runtime_store,
            calls,
            root,
            temp,
        }
    }

    /// This fixture's unique source mob id.
    pub fn source_mob_id(&self) -> MobId {
        MobId::from(format!("council-source-{}", self.scope))
    }

    /// A council id scoped to this fixture.
    pub fn council_id(&self, label: &str) -> TemporaryCouncilId {
        TemporaryCouncilId::new(format!("{label}-{}", self.scope)).expect("canonical council id")
    }

    /// Create this fixture's source mob and seat `members` as real local,
    /// turn-driven members.
    pub async fn seed_source_mob(&self, members: &[&str]) {
        self.seed_source_mob_with_description(members, "council participant")
            .await;
    }

    /// [`Self::seed_source_mob`] with an explicit peer description, which lands
    /// in the source member's own system prompt and is therefore inherited by
    /// every fork taken from it.
    pub async fn seed_source_mob_with_description(&self, members: &[&str], description: &str) {
        let mob_id = self.source_mob_id();
        self.state
            .mob_create_definition(council_definition_with_description(
                mob_id.as_str(),
                description,
            ))
            .await
            .expect("create source mob");
        for member in members {
            self.state
                .mob_spawn(
                    &mob_id,
                    ProfileName::from("participant"),
                    identity(member),
                    // Turn-driven: an autonomous kickoff turn would make
                    // provider-call counts non-deterministic.
                    Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                    Some(MobBackendKind::Session),
                    None,
                )
                .await
                .unwrap_or_else(|error| panic!("spawn source member {member}: {error}"));
        }
    }

    /// Destroy every mob this fixture still owns.
    ///
    /// Called explicitly at the end of each test rather than left to process
    /// teardown: a live mob keeps its inproc comms routes registered, and the
    /// suite must not depend on the process exiting to release them.
    pub async fn teardown(&self) {
        let handles = self.state.mob_handles_snapshot().await.unwrap_or_default();
        for (mob_id, handle) in handles {
            if self.state.mob_destroy(&mob_id).await.is_err() {
                let _ = handle.shutdown().await;
            }
        }
    }

    /// Path of the explicitly rooted realm custody database this fixture uses.
    pub fn realm_custody_path(&self) -> std::path::PathBuf {
        MobMcpState::persistent_forked_participant_store_path(&self.root.join("state"))
    }

    /// Rebuild the state over the SAME durable stores, the way a restarted
    /// process would. The session service and runtime store are retained
    /// because replacing them would model data loss, not a cold restart.
    pub fn restart_state(&self) -> Arc<MobMcpState> {
        MobMcpState::new(self.service.clone(), MobControlPrincipal::Owner)
            .try_with_persistent_storage_root(Some(self.root.join("state")))
            .expect("reopen rooted council + capability custody")
            .into_shared()
    }

    /// Age the persisted coordinator lease of `council_id` past its deadline.
    ///
    /// This is the ONE thing a test cannot get for free from a restarted
    /// process: real wall-clock time passing while the dead coordinator holds
    /// a lease. It rewrites only the lease deadline, so the takeover still
    /// goes through the machine's ordinary observed-expiry path.
    pub async fn expire_claim_lease(
        &self,
        store: &Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
        council_id: &meerkat_mob::temporary_council::TemporaryCouncilId,
    ) {
        let mut record = store
            .load(council_id)
            .await
            .expect("load the council record")
            .expect("the council record exists");
        record.claim_lease_expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        store
            .commit(&record)
            .await
            .expect("age the coordinator lease");
    }

    pub fn provider_calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

// ===========================================================================
// Definitions
// ===========================================================================

pub fn participant_profile(description: &str) -> meerkat_mob::Profile {
    meerkat_mob::Profile {
        model: "claude-haiku-4-5-20251001".to_string(),
        provider: None,
        self_hosted_server_id: None,
        image_generation_provider: None,
        auto_compact_threshold: None,
        resume_overrides: Vec::new(),
        skills: Vec::new(),
        tools: meerkat_mob::ToolConfig {
            comms: true,
            ..meerkat_mob::ToolConfig::default()
        },
        peer_description: description.to_string(),
        external_addressable: false,
        backend: Some(meerkat_mob::MobBackendKind::Session),
        runtime_mode: meerkat_mob::MobRuntimeMode::AutonomousHost,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
    }
}

/// An explicit mob definition with a single `participant` profile.
pub fn council_definition(mob_id: &str) -> MobDefinition {
    council_definition_with_description(mob_id, "council participant")
}

/// [`council_definition`] with an explicit participant peer description.
pub fn council_definition_with_description(mob_id: &str, description: &str) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(participant_profile(description))),
    );
    let mut definition = MobDefinition::explicit(MobId::from(mob_id));
    definition.profiles = profiles;
    definition
}

pub fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

// ===========================================================================
// Fault-injection wrappers over the injectable durable seams
// ===========================================================================

/// A council store that fails one chosen commit exactly once.
///
/// This injects a fault at the DURABLE SEAM, not inside the coordinator: the
/// coordinator's own logic is fully exercised, and the resulting record is
/// exactly the shape a process crash leaves behind.
pub struct OneShotFailingCouncilStore {
    inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
    fail_at_commit: usize,
    commits: AtomicUsize,
    fired: std::sync::atomic::AtomicBool,
}

impl OneShotFailingCouncilStore {
    pub fn new(
        inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
        fail_at_commit: usize,
    ) -> Self {
        Self {
            inner,
            fail_at_commit,
            commits: AtomicUsize::new(0),
            fired: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl meerkat_mob::store::TemporaryCouncilStore for OneShotFailingCouncilStore {
    fn durability(&self) -> meerkat_mob::temporary_council::TemporaryCouncilStoreDurability {
        self.inner.durability()
    }

    async fn insert_new(
        &self,
        record: &meerkat_mob::store::TemporaryCouncilRecord,
    ) -> Result<meerkat_mob::store::TemporaryCouncilRecord, meerkat_mob::store::MobStoreError> {
        self.inner.insert_new(record).await
    }

    async fn load(
        &self,
        council_id: &meerkat_mob::temporary_council::TemporaryCouncilId,
    ) -> Result<Option<meerkat_mob::store::TemporaryCouncilRecord>, meerkat_mob::store::MobStoreError>
    {
        self.inner.load(council_id).await
    }

    async fn commit(
        &self,
        record: &meerkat_mob::store::TemporaryCouncilRecord,
    ) -> Result<meerkat_mob::store::TemporaryCouncilRecord, meerkat_mob::store::MobStoreError> {
        let index = self.commits.fetch_add(1, Ordering::SeqCst);
        if index == self.fail_at_commit && !self.fired.swap(true, Ordering::SeqCst) {
            return Err(meerkat_mob::store::MobStoreError::WriteFailed(
                "injected council custody write fault".to_string(),
            ));
        }
        self.inner.commit(record).await
    }

    async fn list_all(
        &self,
    ) -> Result<Vec<meerkat_mob::store::TemporaryCouncilRecord>, meerkat_mob::store::MobStoreError>
    {
        self.inner.list_all().await
    }
}

/// Persists an ambiguous capability custody record, then pauses before
/// panicking the coordinator task. This models a lost durable-store completion
/// followed by process death, while giving an integration test one deterministic
/// point to remove the source mob before supervised recovery begins.
pub struct AmbiguousThenPanicCouncilStore {
    inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
    fail_at_commit: usize,
    commits: AtomicUsize,
    gate: Arc<TurnGate>,
}

impl AmbiguousThenPanicCouncilStore {
    pub fn new(
        inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
        fail_at_commit: usize,
        gate: Arc<TurnGate>,
    ) -> Self {
        Self {
            inner,
            fail_at_commit,
            commits: AtomicUsize::new(0),
            gate,
        }
    }
}

#[async_trait::async_trait]
impl meerkat_mob::store::TemporaryCouncilStore for AmbiguousThenPanicCouncilStore {
    fn durability(&self) -> meerkat_mob::temporary_council::TemporaryCouncilStoreDurability {
        self.inner.durability()
    }

    async fn insert_new(
        &self,
        record: &meerkat_mob::store::TemporaryCouncilRecord,
    ) -> Result<meerkat_mob::store::TemporaryCouncilRecord, meerkat_mob::store::MobStoreError> {
        self.inner.insert_new(record).await
    }

    async fn load(
        &self,
        council_id: &meerkat_mob::temporary_council::TemporaryCouncilId,
    ) -> Result<Option<meerkat_mob::store::TemporaryCouncilRecord>, meerkat_mob::store::MobStoreError>
    {
        self.inner.load(council_id).await
    }

    async fn commit(
        &self,
        record: &meerkat_mob::store::TemporaryCouncilRecord,
    ) -> Result<meerkat_mob::store::TemporaryCouncilRecord, meerkat_mob::store::MobStoreError> {
        let index = self.commits.fetch_add(1, Ordering::SeqCst);
        if index == self.fail_at_commit {
            let _ = self.inner.commit(record).await?;
            return Err(meerkat_mob::store::MobStoreError::WriteFailed(
                "injected council custody completion loss".to_string(),
            ));
        }
        if index == self.fail_at_commit + 1 {
            let mut ambiguous = record.clone();
            ambiguous.revision = self
                .inner
                .load(&record.council_id)
                .await?
                .ok_or_else(|| {
                    meerkat_mob::store::MobStoreError::ReadFailed(
                        "injected custody record disappeared before ambiguity repair".to_string(),
                    )
                })?
                .revision;
            self.inner.commit(&ambiguous).await?;
            self.gate.wait().await;
            panic!("injected coordinator crash after ambiguous capability custody");
        }
        self.inner.commit(record).await
    }

    async fn list_all(
        &self,
    ) -> Result<Vec<meerkat_mob::store::TemporaryCouncilRecord>, meerkat_mob::store::MobStoreError>
    {
        self.inner.list_all().await
    }
}

/// A capability store with two independent, precisely-placed fault knobs.
///
/// * `request_id_failures` fails `load_by_request_id` for ACTIVATED records.
///   `usize::MAX` models the condition a HOST-owned capability creates: this
///   realm's custody simply never holds the owner's record.
/// * `capability_id_failures` fails `load_by_capability_id` for ACTIVATED
///   records belonging to `capability_id_fault_slot`, which is the exact
///   lookup an owner-side revocation performs. Gating on the slot keeps the
///   fault off the attach path, so the resulting cleanup debt is real and
///   lands where the test says it does.
pub struct FlakyCapabilityStore {
    inner: Arc<dyn meerkat_mob::store::ForkedParticipantStore>,
    request_id_failures: AtomicUsize,
    capability_id_failures: AtomicUsize,
    capability_id_fault_slot: Option<String>,
}

impl FlakyCapabilityStore {
    /// Fail `load_by_request_id` for activated records `failures` times.
    pub fn new(
        inner: Arc<dyn meerkat_mob::store::ForkedParticipantStore>,
        failures: usize,
    ) -> Self {
        Self {
            inner,
            request_id_failures: AtomicUsize::new(failures),
            capability_id_failures: AtomicUsize::new(0),
            capability_id_fault_slot: None,
        }
    }

    /// Fail the owner-side revocation lookup for one participant slot suffix
    /// (for example `":p1"`) `failures` times.
    pub fn failing_revocation_lookup(
        inner: Arc<dyn meerkat_mob::store::ForkedParticipantStore>,
        slot_suffix: &str,
        failures: usize,
    ) -> Self {
        Self {
            inner,
            request_id_failures: AtomicUsize::new(0),
            capability_id_failures: AtomicUsize::new(failures),
            capability_id_fault_slot: Some(slot_suffix.to_string()),
        }
    }

    /// Inner store, for tests that need the exact bearer material.
    pub fn inner(&self) -> Arc<dyn meerkat_mob::store::ForkedParticipantStore> {
        self.inner.clone()
    }

    fn take(counter: &AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                (remaining > 0).then(|| remaining.saturating_sub(1))
            })
            .is_ok()
    }
}

#[async_trait::async_trait]
impl meerkat_mob::store::ForkedParticipantStore for FlakyCapabilityStore {
    async fn insert_reserved(
        &self,
        record: &meerkat_mob::store::ForkedParticipantRecord,
    ) -> Result<meerkat_mob::store::ForkedParticipantRecord, meerkat_mob::store::MobStoreError>
    {
        self.inner.insert_reserved(record).await
    }

    async fn load_by_capability_id(
        &self,
        capability_id: &meerkat_mob::forked_participant::ForkedParticipantCapabilityId,
    ) -> Result<
        Option<meerkat_mob::store::ForkedParticipantRecord>,
        meerkat_mob::store::MobStoreError,
    > {
        let loaded = self.inner.load_by_capability_id(capability_id).await?;
        if let (Some(slot), Some(record)) =
            (self.capability_id_fault_slot.as_ref(), loaded.as_ref())
            && record.sidecar.capability_ref.is_some()
            && record.request_id.as_str().ends_with(slot.as_str())
            && Self::take(&self.capability_id_failures)
        {
            return Err(meerkat_mob::store::MobStoreError::ReadFailed(
                "injected owner-side revocation lookup fault".to_string(),
            ));
        }
        Ok(loaded)
    }

    async fn load_by_request_id(
        &self,
        request_id: &meerkat_mob::forked_participant::ForkedParticipantRequestId,
    ) -> Result<
        Option<meerkat_mob::store::ForkedParticipantRecord>,
        meerkat_mob::store::MobStoreError,
    > {
        let loaded = self.inner.load_by_request_id(request_id).await?;
        let activated = loaded
            .as_ref()
            .is_some_and(|record| record.sidecar.capability_ref.is_some());
        if activated && Self::take(&self.request_id_failures) {
            return Err(meerkat_mob::store::MobStoreError::ReadFailed(
                "injected capability custody read fault".to_string(),
            ));
        }
        Ok(loaded)
    }

    async fn load_by_fork_session_id(
        &self,
        fork_session_id: &meerkat_core::SessionId,
    ) -> Result<
        Option<meerkat_mob::store::ForkedParticipantRecord>,
        meerkat_mob::store::MobStoreError,
    > {
        self.inner.load_by_fork_session_id(fork_session_id).await
    }

    async fn load_exact(
        &self,
        capability: &meerkat_mob::forked_participant::ForkedParticipantRef,
    ) -> Result<meerkat_mob::store::ForkedParticipantRecord, meerkat_mob::store::MobStoreError>
    {
        self.inner.load_exact(capability).await
    }

    async fn commit(
        &self,
        record: &meerkat_mob::store::ForkedParticipantRecord,
    ) -> Result<meerkat_mob::store::ForkedParticipantRecord, meerkat_mob::store::MobStoreError>
    {
        self.inner.commit(record).await
    }

    async fn list_all(
        &self,
    ) -> Result<Vec<meerkat_mob::store::ForkedParticipantRecord>, meerkat_mob::store::MobStoreError>
    {
        self.inner.list_all().await
    }
}

/// A council store that PANICS on one chosen commit.
///
/// Models an owned execution task that dies mid-flight without unwinding into
/// a typed error: the coordinator's watchers must still receive a typed
/// terminal, the single-flight registration must be released, and durable
/// recovery must still see the record.
pub struct PanicOnceCouncilStore {
    inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
    panic_at_commit: usize,
    commits: AtomicUsize,
    fired: std::sync::atomic::AtomicBool,
}

impl PanicOnceCouncilStore {
    pub fn new(
        inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore>,
        panic_at_commit: usize,
    ) -> Self {
        Self {
            inner,
            panic_at_commit,
            commits: AtomicUsize::new(0),
            fired: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl meerkat_mob::store::TemporaryCouncilStore for PanicOnceCouncilStore {
    fn durability(&self) -> meerkat_mob::temporary_council::TemporaryCouncilStoreDurability {
        self.inner.durability()
    }

    async fn insert_new(
        &self,
        record: &meerkat_mob::store::TemporaryCouncilRecord,
    ) -> Result<meerkat_mob::store::TemporaryCouncilRecord, meerkat_mob::store::MobStoreError> {
        self.inner.insert_new(record).await
    }

    async fn load(
        &self,
        council_id: &TemporaryCouncilId,
    ) -> Result<Option<meerkat_mob::store::TemporaryCouncilRecord>, meerkat_mob::store::MobStoreError>
    {
        self.inner.load(council_id).await
    }

    async fn commit(
        &self,
        record: &meerkat_mob::store::TemporaryCouncilRecord,
    ) -> Result<meerkat_mob::store::TemporaryCouncilRecord, meerkat_mob::store::MobStoreError> {
        let index = self.commits.fetch_add(1, Ordering::SeqCst);
        if index == self.panic_at_commit && !self.fired.swap(true, Ordering::SeqCst) {
            panic!("injected council coordinator panic");
        }
        self.inner.commit(record).await
    }

    async fn list_all(
        &self,
    ) -> Result<Vec<meerkat_mob::store::TemporaryCouncilRecord>, meerkat_mob::store::MobStoreError>
    {
        self.inner.list_all().await
    }
}
