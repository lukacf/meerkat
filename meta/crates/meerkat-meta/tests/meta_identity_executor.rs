#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The executor seam of identity changes reaches Core's change path.
//!
//! `CoreChangeExecutor::commit` commits a normalized identity request under
//! a consent, as a requester, in a mob. Each row boots the gateway's runtime
//! composition on a temporary state directory (persistent stores, a roster
//! provider that lists one declared identity), normalizes a request against
//! a policy that allows it, and commits it through the seam with the mob's
//! handle the runtime holds. What the rows read back, they read from Core:
//! its identity entries, its receipts, and a delivery by name.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_creation::{
    ADOPT_NOT_BUILT, AdoptIdentitySpec, AgentChangeState, AgentSeat, ArchiveIdentitySpec,
    ChangeCommit, ChangeCommitted, ChangeRequester, CoreChangeExecutor, CreateAgents,
    CreateIdentitySpec, IdentityChangeExecutor, IdentityCreationPolicy, IdentityEntryFacts,
    IdentityOutcome, InstructionOutcome, InstructionStep, InstructionsChanged, NormalizedRequest,
    OperationId, RemoveIdentitySpec, RequestContext, RequestEnvelope, RequesterFacts, Sha256Digest,
    UPDATE_NOT_BUILT, UnarchiveIdentitySpec, UpdateIdentitySpec, committed_outcome,
    consent_of_plan, normalize_adopt, normalize_archive, normalize_create, normalize_remove,
    normalize_unarchive, normalize_update,
};
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
/// The identity the rows create through the seam.
const CREATED: &str = "keeper";
/// The host surface the rows ask on.
const SURFACE: &str = "ops";
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
        "meta-identity-executor-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(name).unwrap(),
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
/// identity the seam creates.
struct DeclaredRoster;

#[async_trait]
impl RosterProvider for DeclaredRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![spec(DECLARED)])
    }
}

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
            serde_json::json!({"realm": "meta-identity-executor", "account": "recording"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "recording-profile".into(),
        "recording-transport".into(),
    )
}

/// The members' model: each request's last message is sent on the row's
/// channel, each whole request is kept, and every request answers.
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

/// The instruction step the rows' executor is given: it records each agent
/// it is told was removed, and each update it is asked about.
#[derive(Default)]
struct RecordingStep {
    removed: Mutex<Vec<String>>,
    updated: Mutex<Vec<String>>,
}

#[async_trait]
impl InstructionStep for RecordingStep {
    async fn after_update(
        &self,
        _mob: &meerkat_mob::MobHandle,
        change: &InstructionsChanged,
    ) -> InstructionOutcome {
        self.updated.lock().unwrap().push(change.identity.clone());
        InstructionOutcome::AppliesAtNextRun
    }

    async fn after_removal(&self, _mob: &meerkat_mob::MobHandle, identity: &str) {
        self.removed.lock().unwrap().push(identity.to_owned());
    }
}

/// One booted gateway runtime and what its members' model was asked.
struct Rig {
    runtime: meerkat_meta::UnifiedRuntime,
    asked: mpsc::UnboundedReceiver<String>,
    requests: Arc<Mutex<Vec<String>>>,
    step: Arc<RecordingStep>,
}

/// Boot the gateway's runtime composition on `state` for the mob `mob_id`.
/// The launch names no identity policy.
async fn boot(state: &Path, mob_id: &str) -> Rig {
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition(mob_id))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(DeclaredRoster))
        .identity_runtime_instance_id("meta-identity-executor")
        .default_llm_client(Arc::new(RecordingClient {
            asked: asked_tx,
            requests: Arc::clone(&requests),
        }));
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    Rig {
        runtime,
        asked,
        requests,
        step: Arc::new(RecordingStep::default()),
    }
}

/// The policy the rows normalize under: every change is allowed. The seam
/// reads no policy; the rows need one only to make a normalized request.
fn allow_all() -> IdentityCreationPolicy {
    IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive", "remove", "archive", "adopt"],
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled")
}

fn host() -> ChangeRequester {
    ChangeRequester::host(SURFACE)
}

/// A normalized create of `specs`, asked by `requester`.
fn create_request(requester: &RequesterFacts, specs: &[DurableAgentSpec]) -> NormalizedRequest {
    let profiles = BTreeSet::from(["worker".to_owned()]);
    let peers = BTreeMap::new();
    let specs: Vec<CreateIdentitySpec> =
        specs.iter().cloned().map(CreateIdentitySpec::new).collect();
    normalize_create(
        &allow_all(),
        &RequestContext {
            requester,
            profiles: &profiles,
            peers: &peers,
        },
        RequestEnvelope::default(),
        &specs,
    )
    .expect("the create is allowed")
}

/// What normalization knows of the identity `name` at the spec `digest`.
fn entry(name: &str, digest: &Sha256Digest) -> IdentityEntryFacts {
    IdentityEntryFacts::new(name, digest.clone(), Some(1), "worker", BTreeMap::new())
}

fn id(name: &str) -> meerkat_mob::AgentIdentity {
    meerkat_mob::AgentIdentity::from(name)
}

impl Rig {
    fn mob(&self) -> meerkat_mob::MobHandle {
        self.runtime.mob_handle()
    }

    /// The agents the instruction step was told were removed, in order.
    fn removed(&self) -> Vec<String> {
        self.step.removed.lock().unwrap().clone()
    }

    /// Commit `normalized` through the seam as `requester`'s request
    /// `request_id`, under the consent its plan gives.
    async fn commit(
        &self,
        requester: &ChangeRequester,
        request_id: &str,
        normalized: &NormalizedRequest,
    ) -> ChangeCommitted {
        self.commit_of(requester, request_id, normalized, false)
            .await
    }

    /// [`Self::commit`], of an agent its host configures when `configured`.
    async fn commit_of(
        &self,
        requester: &ChangeRequester,
        request_id: &str,
        normalized: &NormalizedRequest,
        configured: bool,
    ) -> ChangeCommitted {
        let mob = self.mob();
        let operation_id =
            OperationId::for_request(&requester.recorded(), request_id).expect("a request id");
        let consent = consent_of_plan(&normalized.plan).expect("the policy allows the change");
        CoreChangeExecutor::new()
            .with_instruction_step(Arc::clone(&self.step) as Arc<dyn InstructionStep>)
            .commit(
                ChangeCommit::new(&mob, &operation_id, requester, consent, &normalized.request)
                    .of_configured_target(configured),
            )
            .await
            .expect("the seam answers a typed outcome")
    }

    /// Create `name` through the seam, as the host, and answer its spec
    /// digest after the create.
    async fn create(&self, name: &str) -> Sha256Digest {
        let committed = self
            .commit(
                &host(),
                &format!("create-{name}"),
                &create_request(&RequesterFacts::host(SURFACE), &[spec(name)]),
            )
            .await;
        let IdentityOutcome::Done { agents, .. } = &committed.outcome else {
            panic!("the create of {name} is done: {:?}", committed.outcome);
        };
        agents[0]
            .spec_digest
            .clone()
            .expect("a created agent has a spec digest")
    }

    /// Where Core holds `name`'s member, when `name` is an identity entry.
    fn standing(&self, name: &str) -> Option<EntryStanding> {
        match self.mob().identity_entry(&id(name)) {
            Ok(IdentityEntryRead::Entry(entry)) => Some(entry.standing),
            _ => None,
        }
    }

    fn seated(&self, name: &str) -> bool {
        matches!(self.standing(name), Some(EntryStanding::Seated(_)))
    }

    /// The outcome Core recorded for `requester`'s request `request_id`.
    async fn recorded(
        &self,
        requester: &ChangeRequester,
        request_id: &str,
    ) -> Option<IdentityOutcome> {
        let operation_id =
            OperationId::for_request(&requester.recorded(), request_id).expect("a request id");
        committed_outcome(&self.mob(), &operation_id)
            .await
            .expect("Core's receipt is read")
    }

    /// One JSON-RPC call through the dispatcher the stdio gateway serves.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: self
                .runtime
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: Arc::new(MutableRosterProvider::new(vec![spec(DECLARED)])),
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

    /// Send `marker` to `identity` by name through `meta/send`.
    async fn send(&self, identity: &str, marker: &str) -> Value {
        self.rpc(
            "meta/send",
            json!({"identity": identity, "content": marker}),
        )
        .await
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

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }

    /// Shut down before a reopen over the same state directory in this
    /// process: stand the durable child root down, bounded, until it is
    /// released.
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

/// The kind token and the request field of a refused commit.
fn refused(committed: &ChangeCommitted) -> (String, Option<String>) {
    let refusal = committed
        .outcome
        .refusal()
        .unwrap_or_else(|| panic!("the change is refused: {:?}", committed.outcome));
    (refusal.kind_token().to_owned(), refusal.field.clone())
}

/// A create through the seam reaches Core: the agent is an identity entry
/// of the mob, seated dormant at spec revision 1, under an operation id
/// namespaced by its requester. No roster lists it. After a restart of the
/// runtime over the same state the agent exists, seated, Core's receipt
/// answers the same outcome, and a send by name reaches it.
///
/// RED before the seam reached Core: the create answered `unsupported` and
/// Core held no entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_agent_exists_after_a_restart_with_no_roster_listing() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot(temp.path(), &mob_id).await;
    let written: CreateAgents = serde_json::from_value(json!({
        "request_id": "create-keeper",
        "agents": [{
            "name": CREATED,
            "profile": "worker",
            "display_name": "The keeper",
            "instructions": ["Keep the records."],
        }],
    }))
    .expect("a create request as a surface writes it");
    let created: Vec<DurableAgentSpec> = written
        .to_create_specs()
        .expect("the agents are well formed")
        .into_iter()
        .map(|agent| agent.spec)
        .collect();
    let request = create_request(&RequesterFacts::host(SURFACE), &created);

    let committed = rig.commit(&host(), "create-keeper", &request).await;
    assert!(!committed.replayed, "the create committed now");
    let IdentityOutcome::Done {
        operation_id,
        agents,
    } = &committed.outcome
    else {
        panic!("the create is done: {:?}", committed.outcome);
    };
    assert!(
        operation_id.as_str().starts_with("host.")
            && operation_id.as_str().ends_with(".create-keeper"),
        "the operation id is the requester's: {operation_id}"
    );
    assert_eq!(agents.len(), 1);
    assert_eq!(
        (
            agents[0].name.as_str(),
            agents[0].state,
            agents[0].spec_revision,
            agents[0].seat,
        ),
        (
            CREATED,
            AgentChangeState::Committed,
            Some(1),
            Some(AgentSeat::Seated)
        ),
        "{agents:?}"
    );
    assert!(agents[0].spec_digest.is_some(), "{agents:?}");
    assert!(
        agents[0].session_id.is_some(),
        "the dormant seat is bound to a session: {agents:?}"
    );
    assert!(rig.seated(CREATED), "Core seats the agent");
    let record = rig
        .mob()
        .identity_record(&id(CREATED))
        .await
        .expect("Core's record is read");
    let Some(meerkat_mob::CreatedIdentityObservation::Created(record)) = record else {
        panic!("Core holds the agent's record: {record:?}");
    };
    let row = record.spec.expect("the record holds the spec");
    assert_eq!(
        (
            record.revision,
            row.display_name.as_deref(),
            row.additional_instructions.as_slice(),
        ),
        (
            1,
            Some("The keeper"),
            ["Keep the records.".to_owned()].as_slice()
        ),
        "the record holds the spec the request gave"
    );
    assert_eq!(
        rig.recorded(&host(), "create-keeper").await.as_ref(),
        Some(&committed.outcome),
        "Core's receipt answers the operation's outcome"
    );
    rig.shutdown_for_reopen().await;

    let mut rig = boot(temp.path(), &mob_id).await;
    assert!(
        rig.seated(CREATED),
        "after the restart Core holds the agent seated, and no roster lists it"
    );
    assert_eq!(
        rig.recorded(&host(), "create-keeper")
            .await
            .map(|outcome| outcome
                .agents()
                .iter()
                .map(|agent| agent.name.clone())
                .collect::<Vec<_>>()),
        Some(vec![CREATED.to_owned()]),
        "the receipt survives the restart"
    );
    let sent = rig.send(CREATED, "KEEPER-AFTER-RESTART").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(
        rig.asked("KEEPER-AFTER-RESTART").await,
        "a send by name reaches the created agent after the restart"
    );
    rig.shutdown().await;
}

/// The same operation id with the same request replays: Core answers the
/// committed outcome and writes nothing again. Another request under the
/// same id is refused `operation_reused`, and nothing of it exists. The
/// same request id of another requester is another operation.
///
/// RED before the seam reached Core: every commit answered `unsupported`.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_operation_id_replays_and_another_digest_is_operation_reused() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = RequesterFacts::host(SURFACE);
    let request = create_request(&facts, &[spec(CREATED)]);

    let first = rig.commit(&host(), "op-1", &request).await;
    assert!(
        matches!(first.outcome, IdentityOutcome::Done { .. }),
        "{first:?}"
    );
    assert!(!first.replayed);

    let again = rig.commit(&host(), "op-1", &request).await;
    assert!(again.replayed, "the same id and request replay: {again:?}");
    assert_eq!(
        again.outcome, first.outcome,
        "the replay answers the committed outcome"
    );
    let record = rig
        .mob()
        .identity_record(&id(CREATED))
        .await
        .expect("Core's record is read");
    assert!(
        matches!(
            &record,
            Some(meerkat_mob::CreatedIdentityObservation::Created(record)) if record.revision == 1
        ),
        "the replay wrote nothing: {record:?}"
    );

    let other = create_request(&facts, &[spec("warden")]);
    let reused = rig.commit(&host(), "op-1", &other).await;
    assert_eq!(
        refused(&reused),
        ("operation_reused".to_owned(), None),
        "another request under the same id: {reused:?}"
    );
    assert!(
        rig.standing("warden").is_none(),
        "nothing of the refused request exists"
    );

    let console = ChangeRequester::host("console");
    let by_console = rig
        .commit(
            &console,
            "op-1",
            &create_request(&RequesterFacts::console(), &[spec("warden")]),
        )
        .await;
    assert!(
        matches!(by_console.outcome, IdentityOutcome::Done { .. }) && !by_console.replayed,
        "the same request id of another requester is another operation: {by_console:?}"
    );
    assert!(rig.seated("warden"));
    rig.shutdown().await;
}

/// An archive, an unarchive and a removal through the seam reach Core, each
/// decided against the spec the requester read: the archived agent has no
/// member and a send to it is refused; a stale spec digest is refused
/// `stale_spec` at `expected_spec_digest` with the current one; the
/// unarchive seats it again; the removal ends it, and its name is never an
/// agent again.
///
/// RED before the seam reached Core: each answered `unsupported`.
#[tokio::test(flavor = "multi_thread")]
async fn an_archive_an_unarchive_and_a_removal_reach_core() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let digest = rig.create(CREATED).await;
    let facts = RequesterFacts::host(SURFACE);
    let policy = allow_all();
    let target = entry(CREATED, &digest);

    let stale = Sha256Digest::of(b"an earlier spec");
    let stale_archive = normalize_archive(
        &policy,
        &facts,
        RequestEnvelope::default(),
        &entry(CREATED, &stale),
        &ArchiveIdentitySpec::new(CREATED, stale.clone()),
    )
    .expect("the archive is allowed");
    let moved = rig.commit(&host(), "archive-stale", &stale_archive).await;
    assert_eq!(
        refused(&moved),
        (
            "stale_spec".to_owned(),
            Some("expected_spec_digest".to_owned())
        ),
        "{moved:?}"
    );
    assert_eq!(
        moved.outcome.refusal().expect("a refusal").to_data()["current_digest"],
        digest.as_str(),
        "the refusal names the current spec digest"
    );
    assert!(rig.seated(CREATED), "a refused archive changes nothing");

    let archive = normalize_archive(
        &policy,
        &facts,
        RequestEnvelope::default(),
        &target,
        &ArchiveIdentitySpec::new(CREATED, digest.clone()),
    )
    .expect("the archive is allowed");
    let archived = rig.commit(&host(), "archive-keeper", &archive).await;
    let states: Vec<AgentChangeState> = archived
        .outcome
        .agents()
        .iter()
        .map(|agent| agent.state)
        .collect();
    assert!(
        matches!(
            states.as_slice(),
            [AgentChangeState::Archived | AgentChangeState::Archiving]
        ),
        "the archive is done: {archived:?}"
    );
    assert!(
        matches!(
            rig.standing(CREATED),
            Some(EntryStanding::Unseated(
                meerkat_mob::UnseatedEntryCause::Archived
            ))
        ),
        "Core holds the agent archived, with no member: {:?}",
        rig.standing(CREATED)
    );
    let sent = rig.send(CREATED, "NEVER-DELIVERED").await;
    assert!(
        !sent["error"].is_null(),
        "a send to the archived agent is refused: {sent:#?}"
    );
    let replayed = rig.commit(&host(), "archive-keeper", &archive).await;
    assert!(replayed.replayed, "{replayed:?}");

    let unarchive = normalize_unarchive(
        &policy,
        &facts,
        RequestEnvelope::default(),
        &target,
        &UnarchiveIdentitySpec::new(CREATED, digest.clone()),
    )
    .expect("the unarchive is allowed");
    let restored = rig.commit(&host(), "unarchive-keeper", &unarchive).await;
    assert_eq!(
        restored
            .outcome
            .agents()
            .iter()
            .map(|agent| agent.state)
            .collect::<Vec<_>>(),
        [AgentChangeState::Restored],
        "{restored:?}"
    );
    assert!(rig.seated(CREATED), "the unarchive seats the agent again");
    assert!(
        rig.removed().is_empty(),
        "an archive and an unarchive tell the instruction step nothing: an archived agent \
         keeps what the step holds"
    );
    let twice = rig.commit(&host(), "unarchive-again", &unarchive).await;
    assert_eq!(
        refused(&twice),
        ("not_archived".to_owned(), Some("name".to_owned())),
        "{twice:?}"
    );

    let remove = normalize_remove(
        &policy,
        &facts,
        RequestEnvelope::default(),
        &target,
        &RemoveIdentitySpec::new(CREATED, digest.clone()),
    )
    .expect("the removal is allowed");
    let removed = rig.commit(&host(), "remove-keeper", &remove).await;
    let states: Vec<AgentChangeState> = removed
        .outcome
        .agents()
        .iter()
        .map(|agent| agent.state)
        .collect();
    assert!(
        matches!(
            states.as_slice(),
            [AgentChangeState::Removed | AgentChangeState::Retiring]
        ),
        "the removal is done: {removed:?}"
    );
    assert!(
        matches!(
            rig.mob().identity_entry(&id(CREATED)),
            Ok(IdentityEntryRead::Removed)
        ),
        "Core holds the name removed"
    );
    assert_eq!(
        rig.removed(),
        [CREATED],
        "the instruction step is told of the removal"
    );
    let removed_again = rig.commit(&host(), "remove-keeper", &remove).await;
    assert!(removed_again.replayed, "{removed_again:?}");
    assert_eq!(
        rig.removed(),
        [CREATED, CREATED],
        "a replay of the removal tells it again"
    );
    assert!(
        rig.step.updated.lock().unwrap().is_empty(),
        "no change of the row was an update"
    );
    let reborn = rig
        .commit(
            &host(),
            "create-keeper-again",
            &create_request(&facts, &[spec(CREATED)]),
        )
        .await;
    assert_eq!(
        reborn.outcome.refusal().map(|refusal| refusal.to_data()),
        Some(json!({"kind": "name_taken", "by": "removed"})),
        "a removed name is never an agent again: {reborn:?}"
    );
    rig.shutdown().await;
}

/// What the seam refuses before Core is called, for every requester: a
/// member private to a person, a change of an agent its host configures
/// other than its adoption, and a requester the request does not record.
/// Nothing is written and Core holds no receipt.
///
/// RED before the seam reached Core: a removal of a configured agent
/// answered `owner_read_only` already, but the allowed create of the last
/// step answered `unsupported`.
#[tokio::test(flavor = "multi_thread")]
async fn the_seam_refuses_a_private_member_and_a_configured_target_before_core() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let agent_facts = RequesterFacts::agent(DECLARED, "worker", BTreeMap::new());
    let session = meerkat_core::SessionId::new();

    let private = ChangeRequester::agent_session(DECLARED, session).private_to_a_person(true);
    let by_private = rig
        .commit(
            &private,
            "private-create",
            &create_request(&agent_facts, &[spec(CREATED)]),
        )
        .await;
    assert_eq!(
        refused(&by_private),
        ("private_requester".to_owned(), None),
        "{by_private:?}"
    );
    assert!(rig.standing(CREATED).is_none(), "nothing was created");
    assert!(
        rig.recorded(&private, "private-create").await.is_none(),
        "Core holds no receipt of the refused change"
    );

    let digest = Sha256Digest::of(b"the declared spec");
    let policy = allow_all();
    let host_facts = RequesterFacts::host(SURFACE);
    let target = entry(DECLARED, &digest);
    let remove = normalize_remove(
        &policy,
        &host_facts,
        RequestEnvelope::default(),
        &target,
        &RemoveIdentitySpec::new(DECLARED, digest.clone()),
    )
    .expect("the removal is allowed");
    let archive = normalize_archive(
        &policy,
        &host_facts,
        RequestEnvelope::default(),
        &target,
        &ArchiveIdentitySpec::new(DECLARED, digest.clone()),
    )
    .expect("the archive is allowed");
    for (request_id, request) in [("remove-declared", &remove), ("archive-declared", &archive)] {
        let read_only = rig.commit_of(&host(), request_id, request, true).await;
        assert_eq!(
            refused(&read_only),
            ("owner_read_only".to_owned(), Some("name".to_owned())),
            "{request_id}: {read_only:?}"
        );
        let detail = read_only.outcome.refusal().expect("a refusal").detail();
        assert!(
            detail.contains("configuration") && detail.contains("adopt"),
            "the detail names both ways: {detail}"
        );
    }
    assert!(
        matches!(
            rig.mob().identity_entry(&id(DECLARED)),
            Ok(IdentityEntryRead::Entry(_))
        ),
        "the configured agent keeps its name: it was not removed"
    );

    let other = rig
        .commit(
            &ChangeRequester::host("console"),
            "create-as-console",
            &create_request(&host_facts, &[spec(CREATED)]),
        )
        .await;
    assert_eq!(
        refused(&other),
        ("invalid_request".to_owned(), Some("requester".to_owned())),
        "the request records another requester: {other:?}"
    );
    assert!(rig.standing(CREATED).is_none(), "nothing was created");

    let allowed = rig
        .commit(
            &host(),
            "create-as-host",
            &create_request(&host_facts, &[spec(CREATED)]),
        )
        .await;
    assert!(
        matches!(allowed.outcome, IdentityOutcome::Done { .. }),
        "the same create by its own requester commits: {allowed:?}"
    );
    rig.shutdown().await;
}

/// An agent requester's change is admitted by its ordinary spawn grant,
/// which the seam reads through Core: a session that holds no grant in the
/// mob is refused `profile_not_allowed` by Core's admission, and an
/// identity whose current grant admits no spawn is refused `grant_revoked`
/// when it is admitted from its recorded identity. Nothing is created.
///
/// RED before the seam reached Core: both answered `unsupported`.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_requester_is_admitted_by_cores_grant_admission() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = RequesterFacts::agent(DECLARED, "worker", BTreeMap::new());
    let request = create_request(&facts, &[spec(CREATED)]);

    let by_session = rig
        .commit(
            &ChangeRequester::agent_session(DECLARED, meerkat_core::SessionId::new()),
            "agent-create",
            &request,
        )
        .await;
    assert_eq!(
        by_session
            .outcome
            .refusal()
            .map(|refusal| refusal.to_data()),
        Some(json!({"kind": "profile_not_allowed", "profile": "worker"})),
        "{by_session:?}"
    );

    let by_record = rig
        .commit(
            &ChangeRequester::agent_recorded(DECLARED),
            "agent-create-recorded",
            &request,
        )
        .await;
    assert_eq!(
        refused(&by_record),
        ("grant_revoked".to_owned(), None),
        "{by_record:?}"
    );
    assert!(rig.standing(CREATED).is_none(), "nothing was created");
    rig.shutdown().await;
}

/// A create that names placement is refused `unsupported` at its field
/// before Core is called, and an update and an adoption answer
/// `unsupported` with their own features until their bodies are filled.
#[tokio::test(flavor = "multi_thread")]
async fn what_the_seam_does_not_carry_yet_is_refused_unsupported() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = RequesterFacts::host(SURFACE);

    let mut placed = spec("placed");
    placed.placement = Some(meerkat_contracts::WireHostRef("member-host-1".to_owned()));
    let refused_placed = rig
        .commit(&host(), "create-placed", &create_request(&facts, &[placed]))
        .await;
    assert_eq!(
        refused(&refused_placed),
        ("unsupported".to_owned(), Some("agents[0]".to_owned())),
        "{refused_placed:?}"
    );
    assert!(rig.standing("placed").is_none(), "nothing was created");

    let digest = rig.create(CREATED).await;
    let policy = allow_all();
    let target = entry(CREATED, &digest);
    let profiles = BTreeSet::from(["worker".to_owned()]);
    let peers = BTreeMap::new();
    let update: UpdateIdentitySpec = serde_json::from_value(json!({
        "identity": CREATED,
        "expected_spec_digest": digest.as_str(),
        "set": {"additional_instructions": ["Keep them in order."]},
    }))
    .expect("an update");
    let update = normalize_update(
        &policy,
        &RequestContext {
            requester: &facts,
            profiles: &profiles,
            peers: &peers,
        },
        RequestEnvelope::default(),
        &target,
        &update,
    )
    .expect("the update is allowed");
    let adopt = normalize_adopt(
        &policy,
        &facts,
        RequestEnvelope::default(),
        &target,
        &AdoptIdentitySpec::new(CREATED, digest.clone()),
    )
    .expect("the adoption is allowed");
    for (request_id, request, feature) in [
        ("update-keeper", &update, UPDATE_NOT_BUILT),
        ("adopt-keeper", &adopt, ADOPT_NOT_BUILT),
    ] {
        let answered = rig.commit(&host(), request_id, request).await;
        assert_eq!(
            answered.outcome.refusal().map(|refusal| refusal.to_data()),
            Some(json!({"kind": "unsupported", "feature": feature})),
            "{request_id}: {answered:?}"
        );
    }
    rig.shutdown().await;
}

/// A create of an agent whose profile the host's policy prepares is
/// committed pending its preparation: the create answers
/// `pending_preparation`, Core holds the agent with no member, a send to it
/// is refused, and Core's receipt answers the same outcome. The same create
/// with no preparation given seats its agent.
///
/// RED before the seam reached Core: the create answered `unsupported`.
#[tokio::test(flavor = "multi_thread")]
async fn a_create_under_a_required_preparation_answers_pending_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "preparation": {"worker": {"mode": "required"}},
        "approval": {"rules": [{"change": ["create"], "outcome": {"kind": "allow"}}]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    let facts = RequesterFacts::host(SURFACE);
    let request = create_request(&facts, &[spec(CREATED)]);
    let mob = rig.mob();
    let requester = host();
    let operation_id =
        OperationId::for_request(&requester.recorded(), "create-prepared").expect("a request id");
    let consent = consent_of_plan(&request.plan).expect("the policy allows the change");
    let committed = CoreChangeExecutor::new()
        .commit(
            ChangeCommit::new(&mob, &operation_id, &requester, consent, &request.request)
                .with_preparations(policy.preparations()),
        )
        .await
        .expect("the seam answers a typed outcome");
    let IdentityOutcome::PendingPreparation { agents, .. } = &committed.outcome else {
        panic!(
            "the create is committed pending its preparation: {:?}",
            committed.outcome
        );
    };
    assert_eq!(
        agents
            .iter()
            .map(|agent| (agent.name.as_str(), agent.state, agent.seat))
            .collect::<Vec<_>>(),
        [(CREATED, AgentChangeState::PendingPreparation, None)],
        "{agents:?}"
    );
    assert!(
        matches!(
            rig.standing(CREATED),
            Some(EntryStanding::Unseated(
                meerkat_mob::UnseatedEntryCause::PreparationPending
            ))
        ),
        "Core holds the agent with no member until it is prepared: {:?}",
        rig.standing(CREATED)
    );
    let sent = rig.send(CREATED, "NEVER-DELIVERED").await;
    assert!(
        !sent["error"].is_null(),
        "a send to an agent that is not prepared is refused: {sent:#?}"
    );
    assert_eq!(
        rig.recorded(&requester, "create-prepared").await.as_ref(),
        Some(&committed.outcome),
        "Core's receipt answers the operation's outcome"
    );

    let unprepared = rig
        .commit(
            &requester,
            "create-unprepared",
            &create_request(&facts, &[spec("warden")]),
        )
        .await;
    assert!(
        matches!(unprepared.outcome, IdentityOutcome::Done { .. }),
        "with no preparation given the agent is seated: {unprepared:?}"
    );
    assert!(rig.seated("warden"));
    rig.shutdown().await;
}

/// A launch that names no identity policy changes nothing a host or an
/// agent can observe: no identity write method creates an agent, a
/// member's turn is offered no identity tool, the status of the configured
/// agent carries none of the record facts, and Core holds no agent but the
/// configured one.
#[tokio::test(flavor = "multi_thread")]
async fn a_launch_without_the_policy_changes_nothing_observable() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;

    for method in [
        "meta/identity/create",
        "meta/identity/update",
        "meta/identity/archive",
        "meta/identity/unarchive",
        "meta/identity/remove",
        "meta/identity/adopt",
    ] {
        let answered = rig
            .rpc(
                method,
                json!({
                    "request_id": "guard-1",
                    "identity": CREATED,
                    "identities": [{"spec": {"identity": CREATED, "profile": "worker"}}],
                }),
            )
            .await;
        assert!(
            !answered["error"].is_null(),
            "{method} changes nothing without a policy: {answered:#?}"
        );
    }
    assert!(
        rig.standing(CREATED).is_none(),
        "Core holds no agent a write method made"
    );
    let entries = rig
        .mob()
        .identity_entries(None, 16)
        .expect("Core's entries are read");
    assert_eq!(
        entries
            .entries
            .iter()
            .map(|entry| entry.identity.to_string())
            .collect::<Vec<_>>(),
        [DECLARED],
        "Core holds the configured agent alone"
    );

    let sent = rig.send(DECLARED, "GUARD-TURN").await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(
        rig.asked("GUARD-TURN").await,
        "the configured agent runs a turn"
    );
    let request: Value = serde_json::from_str(
        &rig.request_with("GUARD-TURN")
            .expect("the turn's model request"),
    )
    .expect("the request is JSON");
    let offered = request["tools"].to_string();
    assert!(
        !offered.contains("identity_"),
        "a member is offered no identity tool: {offered}"
    );

    let status = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed")
        .status(&AgentIdentity::parse(DECLARED).unwrap())
        .await
        .expect("the configured agent's status");
    let status = serde_json::to_value(&status).expect("the status is JSON");
    for member in [
        "spec_digest",
        "spec_revision",
        "archived",
        "preparation_pending",
        "provenance",
        "open_operations",
        "record",
    ] {
        assert!(
            status.get(member).is_none(),
            "the status carries no `{member}`: {status:#}"
        );
    }
    rig.shutdown().await;
}
