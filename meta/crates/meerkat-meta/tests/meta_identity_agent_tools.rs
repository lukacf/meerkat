#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The identity tools: what an agent calls to create and change long-term
//! agents (`identity_create`, `identity_update`, `identity_archive`,
//! `identity_unarchive`, `identity_remove`, `identity_list`,
//! `identity_directory`).
//!
//! The tools are one surface of the identity family and decide nothing: a
//! tool parses its arguments into the family's wire request, attributes the
//! requester from the session its call ran in, names the operation and
//! calls the family. So these rows run on a scripted family that records
//! what it was called with and answers what the row scripts; what the
//! host's rules and Core's change path do with a request is the family's
//! own suite.
//!
//! Each row boots the gateway's runtime composition on a temporary state
//! directory (persistent stores, a roster provider that declares two
//! agents) with a model that calls the tool its turn's message names, and
//! reads back what the model was offered, what the family was called with
//! and what the tool answered.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::{ContentInput, HandlingMode, StopReason, text_content};
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_creation::{
    AgentChangeState, AgentEntered, AgentOutcome, AgentPage, AgentProvenance, AgentReadiness,
    AgentRecord, AgentRef, AgentSet, AgentSpec, AgentSummary, ChangeRequester, ChangesHold,
    ChangesHoldRequest, ConsentSummary, CreateAgents, DirectoryPage, DirectoryPeer,
    DirectoryProfile, DirectoryVerdict, GetAgent, IDENTITY_ARCHIVE, IDENTITY_CREATE,
    IDENTITY_DIRECTORY, IDENTITY_GET, IDENTITY_LIST, IDENTITY_REMOVE, IDENTITY_TOOLS,
    IDENTITY_UNARCHIVE, IDENTITY_UPDATE, IdentityAgentTools, IdentityChangeCall,
    IdentityChangeSpec, IdentityCreationPolicy, IdentityFamily, IdentityFamilyOptions,
    IdentityOutcome, IdentityRead, IdentityReadAnswer, IdentityRefusal, IdentityRefusalKind,
    IdentityToolAnswer, IdentityToolCallSite, IdentityToolCaller, InstructionOutcome, ListAgents,
    OperationId, ReadDirectory, Requester, RequesterKind, Sha256Digest, UpdateAgent,
    WithdrawRequest, Withdrawn, core_member_name, identity_tool_catalog, identity_tool_defs,
    is_identity_tool,
};
use meerkat_meta::identity_creation::{
    AgentSchedules, KeyedSendNotices, OperationState, ScheduleEffect, ScheduleRef,
    SchedulesFollowed,
};
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, MutableRosterProvider, RosterContext,
    RosterError,
};
use meerkat_mob::{MobDefinition, MobHandle, ProfileName, SpawnMemberSpec};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// The declared agent whose profile has mob tools: its spawn grant.
const LEAD: &str = "lead-a";
/// The declared agent whose profile has no mob tools.
const PLAIN: &str = "plain-a";
/// The profile with mob tools.
const MAKER: &str = "maker";
/// The profile without them.
const WORKER: &str = "worker";
/// The label that makes a member private to a person.
const PRIVATE_TO: &str = "console.private_to";
/// Bounds a failure only; a passing row never waits for it.
const TURN_BOUND: Duration = Duration::from_secs(90);

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.{MAKER}]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.{MAKER}.tools]
comms = true
mob = true

[profiles.{WORKER}]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.{WORKER}.tools]
comms = true
"#
    ))
    .expect("parse the definition")
}

fn fresh_mob_id() -> String {
    format!(
        "meta-identity-agent-tools-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn declared(name: &str, profile: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(name).unwrap(),
        profile: ProfileName::from(profile),
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

fn roster() -> Vec<DurableAgentSpec> {
    vec![declared(LEAD, MAKER), declared(PLAIN, WORKER)]
}

/// The host's roster: [`LEAD`] and [`PLAIN`].
struct DeclaredRoster;

#[async_trait]
impl RosterProvider for DeclaredRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(roster())
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
            serde_json::json!({"realm": "meta-identity-agent-tools", "account": "scripted"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "scripted-profile".into(),
        "scripted-transport".into(),
    )
}

/// One tool result as a model request carried it.
#[derive(Debug, Clone, PartialEq)]
struct ToolAnswer {
    content: Value,
    is_error: bool,
}

/// One model request: what it was offered and asked.
#[derive(Debug, Clone)]
struct Asked {
    /// The names of the tools the request offers.
    tools: Vec<String>,
    /// The text of the newest user message.
    text: String,
    /// The tool results that follow that message, in order.
    answers: Vec<ToolAnswer>,
    /// Whether the model ended the turn on this request.
    ended: bool,
}

/// A call the turn's message asks the model to make: `[[call N TOOL]]ARGS
/// [[end]]` makes the model call `TOOL` with `ARGS`, `N` times, each in a
/// round of its own.
struct Directive {
    times: usize,
    tool: String,
    arguments: Value,
}

fn directive(text: &str) -> Option<Directive> {
    let (_, rest) = text.split_once("[[call ")?;
    let (head, rest) = rest.split_once("]]")?;
    let (times, tool) = head.split_once(' ')?;
    let (arguments, _) = rest.split_once("[[end]]")?;
    Some(Directive {
        times: times.parse().ok()?,
        tool: tool.to_owned(),
        arguments: serde_json::from_str(arguments).ok()?,
    })
}

/// The message that makes the model call `tool` with `arguments`, `times`
/// times, marked `mark`.
fn call_message(mark: &str, times: usize, tool: &str, arguments: &Value) -> String {
    format!("{mark} [[call {times} {tool}]]{arguments}[[end]]")
}

/// The members' model. It makes the calls its turn's message names, each
/// with the call id `fc_0`, as a provider that numbers a response's calls
/// from zero does, and records every request.
#[derive(Clone, Default)]
struct ScriptedModel {
    asked: Arc<Mutex<Vec<Asked>>>,
}

impl meerkat_client::LlmClient for ScriptedModel {
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
        let mut text = String::new();
        let mut answers = Vec::new();
        for message in &request.messages {
            match message {
                Message::User(user) => {
                    text = text_content(&user.content);
                    answers.clear();
                }
                Message::ToolResults { results, .. } => {
                    answers.extend(results.iter().map(|result| {
                        ToolAnswer {
                            content: serde_json::from_str(&result.text_content())
                                .unwrap_or_else(|_| Value::String(result.text_content())),
                            is_error: result.is_error,
                        }
                    }));
                }
                _ => {}
            }
        }
        let call = directive(&text).filter(|directive| answers.len() < directive.times);
        self.asked.lock().unwrap().push(Asked {
            tools: request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
            text,
            answers,
            ended: call.is_none(),
        });
        let (lead, stop) = match call {
            Some(directive) => (
                LlmEvent::ToolCallComplete {
                    id: "fc_0".to_string(),
                    name: directive.tool,
                    args: directive.arguments,
                    meta: None,
                },
                StopReason::ToolUse,
            ),
            None => (
                LlmEvent::TextDelta {
                    delta: "noted".to_string(),
                    meta: None,
                },
                StopReason::EndTurn,
            ),
        };
        let [usage, done] = llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, stop);
        Box::pin(async_stream::stream! {
            yield Ok(lead);
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

/// What the scripted family was called with.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a row's record of one call, compared whole"
)]
enum Seen {
    Change {
        requester: ChangeRequester,
        operation_id: String,
        change: IdentityChangeSpec,
    },
    Read {
        requester: ChangeRequester,
        read: IdentityRead,
    },
}

/// A family that decides nothing: it records each call and answers what the
/// row scripted, or `done` for the agents the change names.
#[derive(Default)]
struct ScriptedFamily {
    seen: Mutex<Vec<Seen>>,
    outcome: Mutex<Option<IdentityOutcome>>,
    refuse_reads: Mutex<Option<IdentityRefusal>>,
}

impl ScriptedFamily {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn forget(&self) {
        self.seen.lock().unwrap().clear();
    }

    /// Answer every later change with `outcome`.
    fn answer(&self, outcome: IdentityOutcome) {
        *self.outcome.lock().unwrap() = Some(outcome);
    }

    fn operations(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .filter_map(|seen| match seen {
                Seen::Change { operation_id, .. } => Some(operation_id),
                Seen::Read { .. } => None,
            })
            .collect()
    }
}

fn names_of(change: &IdentityChangeSpec) -> Vec<String> {
    match change {
        IdentityChangeSpec::Create(create) => create
            .agents
            .iter()
            .map(|agent| agent.name.clone())
            .collect(),
        IdentityChangeSpec::Update(update) => vec![update.name.clone()],
        IdentityChangeSpec::Archive(agent)
        | IdentityChangeSpec::Unarchive(agent)
        | IdentityChangeSpec::Remove(agent)
        | IdentityChangeSpec::Adopt(agent) => vec![agent.name.clone()],
        _ => Vec::new(),
    }
}

fn done(operation_id: &OperationId, names: &[String]) -> IdentityOutcome {
    IdentityOutcome::Done {
        operation_id: operation_id.clone(),
        agents: names
            .iter()
            .map(|name| AgentOutcome::new(name.clone(), AgentChangeState::Committed))
            .collect(),
    }
}

#[async_trait]
impl IdentityFamily for ScriptedFamily {
    async fn change(
        &self,
        _mob: &MobHandle,
        requester: &ChangeRequester,
        call: IdentityChangeCall,
    ) -> IdentityOutcome {
        self.seen.lock().unwrap().push(Seen::Change {
            requester: requester.clone(),
            operation_id: call.operation_id.as_str().to_owned(),
            change: call.change.clone(),
        });
        match self.outcome.lock().unwrap().clone() {
            Some(outcome) => outcome,
            None => done(&call.operation_id, &names_of(&call.change)),
        }
    }

    async fn withdraw(
        &self,
        _mob: &MobHandle,
        _requester: &ChangeRequester,
        _request: WithdrawRequest,
    ) -> Result<Withdrawn, IdentityRefusal> {
        Err(IdentityRefusal::new(IdentityRefusalKind::DecisionNotFound))
    }

    async fn changes_hold(
        &self,
        _mob: &MobHandle,
        _requester: &ChangeRequester,
        _request: ChangesHoldRequest,
    ) -> Result<ChangesHold, IdentityRefusal> {
        Ok(ChangesHold::new(false))
    }

    async fn read(
        &self,
        _mob: &MobHandle,
        requester: &ChangeRequester,
        read: IdentityRead,
    ) -> Result<IdentityReadAnswer, IdentityRefusal> {
        self.seen.lock().unwrap().push(Seen::Read {
            requester: requester.clone(),
            read: read.clone(),
        });
        if let Some(refusal) = self.refuse_reads.lock().unwrap().clone() {
            return Err(refusal);
        }
        match read {
            // The host's own read of the mob's agents: one agent another
            // agent asked for, with its provenance.
            IdentityRead::List(_) => {
                let mut page = AgentPage::default();
                page.agents = vec![hosts_view_of_scout()];
                page.next = Some("next-agent".to_owned());
                Ok(IdentityReadAnswer::List(page))
            }
            // The host's own read of that agent in full.
            IdentityRead::Get(_) => {
                let mut record = AgentRecord::new(hosts_view_of_scout());
                record.instructions = vec!["Find the open questions.".to_owned()];
                record.context = Some(json!({"desk": "north"}));
                record.labels = BTreeMap::from([("team".to_owned(), "research".to_owned())]);
                record.peers = vec![PLAIN.to_owned()];
                Ok(IdentityReadAnswer::Get(Box::new(record)))
            }
            _ => {
                let mut page = DirectoryPage::default();
                page.profiles = vec![
                    DirectoryProfile::new(WORKER, DirectoryVerdict::Allowed),
                    DirectoryProfile::new(
                        MAKER,
                        DirectoryVerdict::NeedsDecision {
                            lines: vec!["Creates an agent that can create agents.".to_owned()],
                        },
                    ),
                ];
                // A peer whose name the host's roster lists as well, as the
                // host's read says it.
                let mut peer = DirectoryPeer::new(
                    PLAIN,
                    WORKER,
                    DirectoryVerdict::NotAllowed {
                        reason: Some("outside your team".to_owned()),
                    },
                );
                peer.roster_listing_superseded = true;
                page.peers = vec![peer];
                page.next = Some("next-entry".to_owned());
                Ok(IdentityReadAnswer::Directory(page))
            }
        }
    }
}

/// One agent as the scripted family's host read shows it: another agent
/// asked for it, so it carries its provenance and an operation that agent
/// holds open for it, whose id tells who asked; and the host's roster
/// lists its name as well, which the host's read says. The host made it
/// internal only and gave it a runtime mode of its own, which every read
/// says.
fn hosts_view_of_scout() -> AgentSummary {
    let mut agent = AgentSummary::new("scout", WORKER, sha("scout"), AgentReadiness::Unseated);
    agent.display_name = Some("Scout".to_owned());
    agent.spec_revision = Some(3);
    agent.archived = true;
    agent.roster_listing_superseded = true;
    agent.addressability = Some(AgentAddressability::InternalOnly);
    agent.runtime_mode_override = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    agent.provenance = Some(AgentProvenance::new(
        AgentEntered::Created,
        RequesterKind::Agent,
        "another-agent",
        1_700_000_000_000,
        ConsentSummary::Decided,
    ));
    agent.open_operations = vec![
        OperationId::for_request(
            &Requester::Agent {
                identity: "another-agent".to_owned(),
            },
            "held-request-1",
        )
        .expect("a request id"),
    ];
    agent
}

/// One booted gateway runtime, its members' model and the scripted family.
struct Rig {
    runtime: meerkat_meta::UnifiedRuntime,
    model: ScriptedModel,
    family: Arc<ScriptedFamily>,
}

/// Boot the gateway's runtime composition on `state` for the mob `mob_id`.
/// No identity family is attached: the launch has no identity policy.
async fn boot(state: &Path, mob_id: &str) -> Rig {
    let model = ScriptedModel::default();
    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition(mob_id))
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(DeclaredRoster))
            .identity_runtime_instance_id("meta-identity-agent-tools")
            .default_llm_client(Arc::new(model.clone()))
            .build(),
    )
    .await
    .expect("build the gateway-shaped UnifiedRuntime");
    Rig {
        runtime,
        model,
        family: Arc::new(ScriptedFamily::default()),
    }
}

/// [`boot`], with the scripted family attached as a launch with an identity
/// policy attaches its family.
async fn boot_with_family(state: &Path, mob_id: &str) -> Rig {
    let rig = boot(state, mob_id).await;
    rig.attach_family();
    rig
}

fn sha(text: &str) -> Sha256Digest {
    Sha256Digest::of(text.as_bytes())
}

fn has_identity_tools(asked: &Asked) -> bool {
    IDENTITY_TOOLS
        .iter()
        .all(|tool| asked.tools.iter().any(|offered| offered == tool))
}

fn has_no_identity_tool(asked: &Asked) -> bool {
    !asked.tools.iter().any(|offered| is_identity_tool(offered))
}

impl Rig {
    /// Attach the scripted family as a launch whose identity policy is
    /// present and enabled attaches its family.
    fn attach_family(&self) {
        self.runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .attach_identity_family(self.family.clone());
    }

    /// Attach the scripted family as a launch without an enabled identity
    /// policy attaches its family: it serves reads and refuses every
    /// change.
    fn attach_reads(&self) {
        self.runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .attach_identity_reads(self.family.clone());
    }

    /// One JSON-RPC call through the dispatcher the stdio gateway serves.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: self
                .runtime
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: Arc::new(MutableRosterProvider::new(roster())),
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

    /// The model requests whose turn's message carries `mark`, once that
    /// turn ended.
    async fn turn(&self, mark: &str) -> Vec<Asked> {
        tokio::time::timeout(TURN_BOUND, async {
            loop {
                let asked: Vec<Asked> = self
                    .model
                    .asked
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|asked| asked.text.contains(mark))
                    .cloned()
                    .collect();
                if asked.last().is_some_and(|asked| asked.ended) {
                    return asked;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the turn marked {mark} ends"))
    }

    /// Send `content` to the agent `identity` by name, and answer its
    /// turn's model requests.
    async fn send(&self, identity: &str, mark: &str, content: &str) -> Vec<Asked> {
        let sent = self
            .rpc(
                "meta/send",
                json!({"identity": identity, "content": content}),
            )
            .await;
        assert!(
            sent["error"].is_null(),
            "the send to {identity} is taken: {sent:#}"
        );
        self.turn(mark).await
    }

    /// Have `identity` call `tool` with `arguments` once, and answer what
    /// the tool answered.
    async fn tool(&self, identity: &str, mark: &str, tool: &str, arguments: Value) -> ToolAnswer {
        let asked = self
            .send(identity, mark, &call_message(mark, 1, tool, &arguments))
            .await;
        let last = asked.last().expect("the turn asked the model");
        assert_eq!(
            last.answers.len(),
            1,
            "the call of {tool} answered once: {last:?}"
        );
        last.answers[0].clone()
    }

    /// The session of the agent `identity`'s member.
    async fn session_of(&self, identity: &str) -> meerkat_core::types::SessionId {
        let member = core_member_name(identity).expect("an agent name");
        self.runtime
            .mob_handle()
            .get_member(&member)
            .await
            .expect("the roster reads")
            .unwrap_or_else(|| panic!("{identity} is a member"))
            .bridge_session_id()
            .cloned()
            .unwrap_or_else(|| panic!("{identity} has a session"))
    }

    /// The identity tools over the scripted family, called directly.
    fn tools(&self) -> IdentityAgentTools {
        IdentityAgentTools::new(self.family.clone(), self.runtime.mob_handle())
    }

    /// Create the agent `name` of `profile` through Core's own create, as a
    /// family would: Core seats it dormant.
    async fn create(&self, name: &str, profile: &str) {
        let created = self
            .runtime
            .mob_handle()
            .create_store_owned_identities(
                meerkat_mob::StoreOwnedIdentityCreate::new(
                    meerkat_mob::IdentityOperation::new(
                        format!("create-{name}"),
                        meerkat_core::approval::decision::Sha256Digest::of(name.as_bytes())
                            .as_str(),
                    ),
                    meerkat_mob::IdentityChangeRequesterInput::host("console"),
                    vec![meerkat_mob::CreatedIdentityRequest::new(
                        meerkat_mob::CreatedIdentitySpec::new(name, profile),
                    )],
                ),
                meerkat_mob::IdentityChangeConsentInput::Policy(
                    meerkat_mob::IdentityPolicyOutcome::Allow,
                ),
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
    }

    /// Spawn the worker member `name` of `profile` with `labels`, run one
    /// turn on it and answer that turn's model requests.
    async fn worker_turn(
        &self,
        name: &str,
        profile: &str,
        labels: &[(&str, &str)],
        mark: &str,
    ) -> Vec<Asked> {
        let identity = meerkat_mob::AgentIdentity::from(name);
        let labels = labels
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>();
        Box::pin(
            self.runtime
                .mob_handle()
                .spawn_spec(SpawnMemberSpec::new(profile, identity.clone()).with_labels(labels)),
        )
        .await
        .unwrap_or_else(|error| panic!("{name} is spawned: {error}"));
        let member = self
            .runtime
            .mob_handle()
            .member(&identity)
            .await
            .expect("the member's handle");
        let turn = member
            .start_turn(
                ContentInput::Text(mark.to_string()),
                HandlingMode::Queue,
                meerkat_mob::MemberTurnOptions::new(),
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("{name} takes a turn: {error}"));
        turn.wait()
            .await
            .unwrap_or_else(|error| panic!("{name}'s turn ends: {error}"));
        self.turn(mark).await
    }

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }

    /// Shut down before a reopen over the same state directory in this
    /// process, until the durable child root is released.
    async fn shutdown_for_reopen(self) {
        let released = |outcome: &meerkat_meta::ChildMobStandDownOutcome| {
            matches!(
                outcome,
                meerkat_meta::ChildMobStandDownOutcome::Released { .. }
                    | meerkat_meta::ChildMobStandDownOutcome::NotConfigured
            )
        };
        let mut outcome = self.runtime.shutdown().await.child_mob_stand_down;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !released(&outcome) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            outcome = self
                .runtime
                .mob_runtime()
                .stand_down_child_mobs(deadline)
                .await;
        }
        assert!(
            released(&outcome),
            "the durable child root is released before the reopen: {outcome:?}"
        );
    }
}

/// The caller the rows call the tools directly as: [`LEAD`], from its
/// member's session.
async fn lead(rig: &Rig) -> IdentityToolCaller {
    IdentityToolCaller::new(LEAD, rig.session_of(LEAD).await)
}

const RUN: &str = "run-1";

async fn call(rig: &Rig, tool: &str, arguments: Value) -> IdentityToolAnswer {
    rig.tools()
        .call(
            &lead(rig).await,
            &IdentityToolCallSite::in_round(RUN, 0, "fc_0"),
            tool,
            &arguments.to_string(),
        )
        .await
        .unwrap_or_else(|fault| panic!("{tool} answers: {fault}"))
}

/// The identity tools are in an agent's tool list exactly while the mob's
/// identity family takes changes (its launch has an identity policy, and
/// the policy is enabled) and the agent's profile has mob tools (its spawn
/// grant). Otherwise they are absent: not listed and then refused. A launch
/// without an enabled policy still has a family, which serves reads to the
/// host and refuses every change: its agents are offered no identity tool.
/// A family that comes to take changes after an agent was built reaches
/// that agent's next request.
#[tokio::test(flavor = "multi_thread")]
async fn identity_tools_mount_with_a_policy_and_a_spawn_grant_and_not_without() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;

    let asked = rig.send(LEAD, "no-family", "no-family").await;
    assert!(
        has_no_identity_tool(&asked[0]),
        "with no identity family an agent with mob tools is offered no identity tool: {:?}",
        asked[0].tools
    );
    assert!(
        asked[0].tools.iter().any(|tool| tool == "mob_spawn_member"),
        "the agent has its mob tools: {:?}",
        asked[0].tools
    );

    rig.attach_reads();
    let asked = rig.send(LEAD, "no-policy", "no-policy").await;
    assert!(
        has_no_identity_tool(&asked[0]),
        "a family that serves reads only (no policy, or one not enabled) mounts no identity \
         tool: {:?}",
        asked[0].tools
    );

    rig.attach_family();
    let asked = rig.send(LEAD, "with-policy", "with-policy").await;
    assert!(
        has_identity_tools(&asked[0]),
        "with a policy an agent with mob tools is offered the identity tools: {:?}",
        asked[0].tools
    );
    let offered: Vec<&str> = asked[0]
        .tools
        .iter()
        .map(String::as_str)
        .filter(|tool| is_identity_tool(tool))
        .collect();
    assert_eq!(offered, IDENTITY_TOOLS, "each once, in their order");

    let asked = rig.send(PLAIN, "no-grant", "no-grant").await;
    assert!(
        has_no_identity_tool(&asked[0]),
        "an agent whose profile has no mob tools holds no spawn grant and is offered none: {:?}",
        asked[0].tools
    );
    rig.shutdown().await;
}

/// A member private to a person is offered no identity tool, whatever its
/// profile: it would carry that person's conversation into a shared agent's
/// instructions or first message. A member of the same profile that is
/// private to no one is offered them.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_private_to_a_person_is_offered_no_identity_tools() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_family(state.path(), &fresh_mob_id()).await;

    let asked = rig
        .worker_turn("side", MAKER, &[(PRIVATE_TO, "alice")], "private-turn")
        .await;
    assert!(
        has_no_identity_tool(&asked[0]),
        "a private member is offered no identity tool: {:?}",
        asked[0].tools
    );
    assert!(
        asked[0].tools.iter().any(|tool| tool == "mob_spawn_member"),
        "the private member has its mob tools: {:?}",
        asked[0].tools
    );

    let asked = rig.worker_turn("shared", MAKER, &[], "shared-turn").await;
    assert!(
        has_identity_tools(&asked[0]),
        "a member of the same profile private to no one is offered them: {:?}",
        asked[0].tools
    );
    rig.shutdown().await;
}

/// The tools are in the FIRST request of an agent's first run: after a wake
/// of a dormant agent, of a newly created one, and after a restart restored
/// the mob. A tool list that changed under an agent in mid conversation
/// would not be acceptable as the ordinary path.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_request_of_a_run_offers_the_identity_tools_after_a_wake_and_a_restore() {
    let state = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot_with_family(state.path(), &mob_id).await;

    let asked = rig.send(LEAD, "first-wake", "first-wake").await;
    assert!(
        has_identity_tools(&asked[0]),
        "the first request after a declared agent's first wake: {:?}",
        asked[0].tools
    );

    rig.create("keeper", MAKER).await;
    let asked = rig.send("keeper", "created-wake", "created-wake").await;
    assert!(
        has_identity_tools(&asked[0]),
        "the first request of a newly created agent's first run: {:?}",
        asked[0].tools
    );
    rig.shutdown_for_reopen().await;

    let rig = boot_with_family(state.path(), &mob_id).await;
    for (agent, mark) in [(LEAD, "restored-lead"), ("keeper", "restored-keeper")] {
        let asked = rig.send(agent, mark, mark).await;
        assert!(
            has_identity_tools(&asked[0]),
            "the first request of {agent} after the restart: {:?}",
            asked[0].tools
        );
    }
    rig.shutdown().await;
}

/// Each tool, called by an agent's model in a real turn, reaches the family
/// with the wire request its arguments spell, as the member whose session
/// made the call, and answers the family's outcome, without the operation
/// id the family names it under.
#[tokio::test(flavor = "multi_thread")]
async fn each_identity_tool_reaches_the_family_with_its_wire_request() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_family(state.path(), &fresh_mob_id()).await;
    let digest = sha("spec-of-scout");

    let mut scout = AgentSpec::new("scout", WORKER);
    scout.display_name = Some("Scout".to_owned());
    scout.instructions = vec!["Find the open questions.".to_owned()];
    scout.context = Some(json!({"topic": "tides"}));
    scout.labels = BTreeMap::from([("area".to_owned(), "research".to_owned())]);
    scout.first_message = Some(ContentInput::Text("Start with the tide tables.".to_owned()));
    scout.peers = vec![LEAD.to_owned(), "mapper".to_owned()];
    let mut create = CreateAgents::new(vec![scout, AgentSpec::new("mapper", WORKER)]);
    create.request_id = Some("team-1".to_owned());

    let mut set = AgentSet::default();
    set.display_name = Some("Scout II".to_owned());
    set.instructions = Some(vec!["Find the open questions, then rank them.".to_owned()]);
    set.context = Some(json!({"topic": "currents"}));
    set.labels = Some(BTreeMap::from([("area".to_owned(), "field".to_owned())]));
    set.labels_remove = vec!["draft".to_owned()];
    set.profile = Some(MAKER.to_owned());
    let mut update = UpdateAgent::new("scout", digest.clone());
    update.set = set;
    update.peers_add = vec!["mapper".to_owned()];
    update.peers_remove = vec![LEAD.to_owned()];
    let one = AgentRef::new("scout", digest.clone());

    let changes = [
        (
            IDENTITY_CREATE,
            json!({
                "request_id": "team-1",
                "agents": [
                    {
                        "name": "scout",
                        "profile": WORKER,
                        "display_name": "Scout",
                        "instructions": ["Find the open questions."],
                        "context": {"topic": "tides"},
                        "labels": {"area": "research"},
                        "first_message": "Start with the tide tables.",
                        "peers": [LEAD, "mapper"],
                    },
                    {"name": "mapper", "profile": WORKER},
                ],
            }),
            IdentityChangeSpec::Create(create),
        ),
        (
            IDENTITY_UPDATE,
            json!({
                "name": "scout",
                "expected_spec_digest": digest.as_str(),
                "set": {
                    "display_name": "Scout II",
                    "instructions": ["Find the open questions, then rank them."],
                    "context": {"topic": "currents"},
                    "labels": {"area": "field"},
                    "labels_remove": ["draft"],
                    "profile": MAKER,
                },
                "peers_add": ["mapper"],
                "peers_remove": [LEAD],
            }),
            IdentityChangeSpec::Update(update),
        ),
        (
            IDENTITY_ARCHIVE,
            json!({"name": "scout", "expected_spec_digest": digest.as_str()}),
            IdentityChangeSpec::Archive(one.clone()),
        ),
        (
            IDENTITY_UNARCHIVE,
            json!({"name": "scout", "expected_spec_digest": digest.as_str()}),
            IdentityChangeSpec::Unarchive(one.clone()),
        ),
        (
            IDENTITY_REMOVE,
            json!({"name": "scout", "expected_spec_digest": digest.as_str()}),
            IdentityChangeSpec::Remove(one),
        ),
    ];
    let session = {
        // The agent's member is seated dormant with its session bound.
        rig.send(LEAD, "seat-lead", "seat-lead").await;
        rig.session_of(LEAD).await
    };
    let expected_requester = ChangeRequester::agent_session(LEAD, session);

    for (tool, arguments, expected) in changes {
        rig.family.forget();
        let answer = rig
            .tool(LEAD, &format!("turn-{tool}"), tool, arguments)
            .await;
        let seen = rig.family.seen();
        let [
            Seen::Change {
                requester,
                operation_id,
                change,
            },
        ] = seen.as_slice()
        else {
            panic!("{tool} called the family's change once: {seen:?}");
        };
        assert_eq!(
            change, &expected,
            "{tool} hands the family its wire request"
        );
        assert_eq!(
            requester, &expected_requester,
            "{tool} asks as the member whose session made the call"
        );
        let names = names_of(&expected);
        assert!(
            OperationId::from_recorded(operation_id).is_some(),
            "{tool} hands the family an operation id: {operation_id}"
        );
        // The family's outcome, without its operation id: that is the
        // host's handle on the change, and the agent that asked is not
        // shown it (`an_agents_change_answer_names_no_operation_id`).
        assert_eq!(
            answer.content,
            json!({
                "outcome": "done",
                "agents": names
                    .iter()
                    .map(|name| json!({"name": name, "state": "committed"}))
                    .collect::<Vec<_>>(),
            }),
            "{tool} answers the family's outcome, without its operation id"
        );
        assert!(!answer.is_error, "a done outcome is no error result");
    }

    for (tool, arguments, expected) in [
        (
            IDENTITY_LIST,
            json!({"profile": WORKER, "include_archived": true, "after": "mapper"}),
            IdentityRead::List({
                let mut list = ListAgents::default();
                list.profile = Some(WORKER.to_owned());
                list.include_archived = true;
                list.after = Some("mapper".to_owned());
                list
            }),
        ),
        (
            IDENTITY_DIRECTORY,
            json!({"query": "sco", "after": "mapper"}),
            IdentityRead::Directory({
                let mut directory = ReadDirectory::default();
                directory.query = Some("sco".to_owned());
                directory.after = Some("mapper".to_owned());
                directory
            }),
        ),
    ] {
        rig.family.forget();
        let answer = rig
            .tool(LEAD, &format!("turn-{tool}"), tool, arguments)
            .await;
        assert_eq!(
            rig.family.seen(),
            vec![Seen::Read {
                requester: expected_requester.clone(),
                read: expected,
            }],
            "{tool} hands the family its read, as the calling member"
        );
        let expected_answer = if tool == IDENTITY_LIST {
            // The family's page, without the agent's provenance and without
            // the operation another requester holds open for it; the two
            // members the host set are read back.
            json!({
                "agents": [{
                    "name": "scout",
                    "display_name": "Scout",
                    "profile": WORKER,
                    "spec_digest": sha("scout").as_str(),
                    "spec_revision": 3,
                    "archived": true,
                    "readiness": "unseated",
                    "addressability": "internal_only",
                    "runtime_mode_override": "turn_driven",
                }],
                "next": "next-agent",
            })
        } else {
            // Each entry carries its verdict flat, beside its own members.
            json!({
                "profiles": [
                    {"profile": WORKER, "verdict": "allowed"},
                    {
                        "profile": MAKER,
                        "verdict": "needs_decision",
                        "lines": ["Creates an agent that can create agents."],
                    },
                ],
                "peers": [{
                    "name": PLAIN,
                    "profile": WORKER,
                    "verdict": "not_allowed",
                    "reason": "outside your team",
                }],
                "next": "next-entry",
            })
        };
        assert_eq!(answer.content, expected_answer, "{tool} answers the page");
        assert!(!answer.is_error);
    }

    // An agent that lists the mob's agents is not shown who asked for
    // another agent, when, or under which consent, nor an operation another
    // requester holds open for it, nor whether the host's roster lists the
    // agent's name as well: the host's read keeps that.
    let listed = call(&rig, IDENTITY_LIST, json!({})).await.text();
    for hidden in [
        "provenance",
        "another-agent",
        "consent",
        "at_ms",
        "decided",
        "open_operations",
        "held-request-1",
        "roster_listing_superseded",
    ] {
        assert!(
            !listed.contains(hidden),
            "identity_list names no `{hidden}`: {listed}"
        );
    }
    let directory = call(&rig, IDENTITY_DIRECTORY, json!({})).await.text();
    assert!(
        !directory.contains("roster_listing_superseded"),
        "identity_directory does not say where a peer's record lives: {directory}"
    );
    rig.shutdown().await;
}

/// Who asks is the member whose session made the call, never an argument.
/// The arguments are closed: a requester, a member only the host plane
/// writes, and every parameter that is not in this release are refused as
/// unknown, naming the argument, and the family is never called.
#[tokio::test(flavor = "multi_thread")]
async fn the_requester_is_the_calling_session_and_an_argument_naming_one_is_refused() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_family(state.path(), &fresh_mob_id()).await;
    rig.send(LEAD, "seat-lead", "seat-lead").await;
    let digest = sha("spec");
    let agent = json!({"name": "scout", "profile": WORKER});
    let one = json!({"name": "scout", "expected_spec_digest": digest.as_str()});
    let with = |base: &Value, path: &[&str], member: &str, value: Value| {
        let mut arguments = base.clone();
        let mut at = &mut arguments;
        for step in path {
            at = match step.parse::<usize>() {
                Ok(index) => &mut at[index],
                Err(_) => &mut at[*step],
            };
        }
        at[member] = value;
        arguments
    };
    let create = json!({"agents": [agent]});
    let update = with(&one, &[], "set", json!({}));

    let forged = with(
        &create,
        &[],
        "requester",
        json!({"kind": "agent", "identity": PLAIN}),
    );
    let answer = rig.tool(LEAD, "forged", IDENTITY_CREATE, forged).await;
    assert_eq!(answer.content["outcome"], "refused");
    assert_eq!(answer.content["kind"], "invalid_request");
    assert_eq!(answer.content["problem"], "malformed");
    assert_eq!(answer.content["field"], "requester");
    assert_eq!(answer.content["retryable"], false);
    assert!(answer.is_error, "a refusal is an error result");
    assert!(
        rig.family.seen().is_empty(),
        "a call with a forged requester never reaches the family"
    );

    for (tool, arguments, field) in [
        (
            IDENTITY_CREATE,
            with(&create, &[], "proposer", json!("ops")),
            "proposer",
        ),
        (
            IDENTITY_CREATE,
            with(
                &create,
                &[],
                "source_evidence",
                json!({"attestor": "a", "subject": "b", "source_ref": "c", "observed_at_ms": 1}),
            ),
            "source_evidence",
        ),
        (
            IDENTITY_CREATE,
            with(&create, &[], "mob_id", json!("other")),
            "mob_id",
        ),
        (
            IDENTITY_CREATE,
            with(&create, &["agents", "0"], "grants", json!([])),
            "agents[0].grants",
        ),
        (
            IDENTITY_CREATE,
            with(&create, &["agents", "0"], "schedules", json!([])),
            "agents[0].schedules",
        ),
        (
            IDENTITY_CREATE,
            with(
                &create,
                &["agents", "0"],
                "fork_from",
                json!({"name": LEAD}),
            ),
            "agents[0].fork_from",
        ),
        (
            IDENTITY_CREATE,
            with(&create, &["agents", "0"], "placement", json!({})),
            "agents[0].placement",
        ),
        (
            IDENTITY_CREATE,
            with(&create, &["agents", "0"], "tools", json!(["shell"])),
            "agents[0].tools",
        ),
        (
            IDENTITY_UPDATE,
            with(&update, &["set"], "grants_add", json!([])),
            "set.grants_add",
        ),
        (
            IDENTITY_UPDATE,
            with(
                &update,
                &["set"],
                "runtime_mode_override",
                json!("turn_driven"),
            ),
            "set.runtime_mode_override",
        ),
        (
            IDENTITY_UPDATE,
            with(&update, &["set"], "addressability", json!("internal_only")),
            "set.addressability",
        ),
        (
            IDENTITY_CREATE,
            with(
                &create,
                &["agents", "0"],
                "addressability",
                json!("internal_only"),
            ),
            "agents[0].addressability",
        ),
        (
            IDENTITY_CREATE,
            with(
                &create,
                &["agents", "0"],
                "runtime_mode_override",
                json!("turn_driven"),
            ),
            "agents[0].runtime_mode_override",
        ),
        (
            IDENTITY_UPDATE,
            with(&update, &[], "proposer", json!("ops")),
            "proposer",
        ),
        (
            IDENTITY_ARCHIVE,
            with(&one, &[], "source_evidence", json!({})),
            "source_evidence",
        ),
        (
            IDENTITY_UNARCHIVE,
            with(&one, &[], "requester", json!(PLAIN)),
            "requester",
        ),
        (
            IDENTITY_REMOVE,
            with(&one, &[], "mob_id", json!("m")),
            "mob_id",
        ),
        (IDENTITY_LIST, json!({"mob_id": "m"}), "mob_id"),
        (IDENTITY_DIRECTORY, json!({"requester": PLAIN}), "requester"),
        (IDENTITY_CREATE, json!({}), "agents"),
        (
            IDENTITY_CREATE,
            json!({"agents": [{"name": "scout"}]}),
            "agents[0].profile",
        ),
        (
            IDENTITY_CREATE,
            json!({"agents": [{"name": "scout", "profile": WORKER, "instructions": "one"}]}),
            "agents[0].instructions",
        ),
        (
            IDENTITY_CREATE,
            json!({"agents": [{"name": "scout", "profile": WORKER, "peers": [7]}]}),
            "agents[0].peers[0]",
        ),
        (
            IDENTITY_UPDATE,
            json!({"name": "scout"}),
            "expected_spec_digest",
        ),
        (
            IDENTITY_ARCHIVE,
            json!({"name": "scout", "expected_spec_digest": "sha256:short"}),
            "expected_spec_digest",
        ),
        (
            IDENTITY_LIST,
            json!({"include_archived": "yes"}),
            "include_archived",
        ),
    ] {
        let answer = call(&rig, tool, arguments.clone()).await;
        assert_eq!(
            (
                answer.content["outcome"].as_str(),
                answer.content["kind"].as_str(),
                answer.content["field"].as_str(),
            ),
            (Some("refused"), Some("invalid_request"), Some(field)),
            "{tool} with {arguments}: {}",
            answer.content
        );
        assert!(answer.refused);
        assert!(
            answer.content["detail"]
                .as_str()
                .is_some_and(|detail| !detail.is_empty()),
            "the refusal says what to send instead"
        );
    }
    assert!(
        rig.family.seen().is_empty(),
        "no refused call reached the family: {:?}",
        rig.family.seen()
    );

    // A null member is a member left out, at every depth.
    let answer = call(
        &rig,
        IDENTITY_CREATE,
        json!({
            "request_id": null,
            "agents": [{"name": "scout", "profile": WORKER, "display_name": null, "peers": null}],
        }),
    )
    .await;
    assert_eq!(answer.content["outcome"], "done", "{}", answer.content);
    rig.shutdown().await;
}

/// A call that gives no `request_id` gets its operation from where it ran:
/// its mob, session, run, and its round and call id within the run. The
/// same call dispatched again names the same operation; another round of
/// the same run, another run and another session name others, though the
/// provider's call id is the same in all of them. A `request_id` names the
/// operation by the requester alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_without_a_request_id_derives_its_operation_from_where_it_ran() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_family(state.path(), &fresh_mob_id()).await;
    let arguments = json!({"agents": [{"name": "scout", "profile": WORKER}]});

    // In a real run: two rounds of one run, then another run, each call
    // with the provider's call id `fc_0`.
    rig.send(
        LEAD,
        "two-rounds",
        &call_message("two-rounds", 2, IDENTITY_CREATE, &arguments),
    )
    .await;
    rig.send(
        LEAD,
        "next-run",
        &call_message("next-run", 1, IDENTITY_CREATE, &arguments),
    )
    .await;
    let operations = rig.family.operations();
    assert_eq!(operations.len(), 3, "three calls reached the family");
    assert_eq!(
        operations.iter().collect::<BTreeSet<_>>().len(),
        3,
        "two rounds of one run and a later run are three operations, with one call id: \
         {operations:?}"
    );
    for operation in &operations {
        let derived = operation
            .strip_prefix("agent.")
            .and_then(|rest| rest.split_once(".call-"));
        assert!(
            derived.is_some_and(|(requester, call)| requester.len() == 32 && call.len() == 64),
            "a derived operation id is the requester's, with a derived request id: {operation}"
        );
    }

    // Called directly, with the place of the call given.
    rig.family.forget();
    let caller = lead(&rig).await;
    let tools = rig.tools();
    let at = |site: IdentityToolCallSite<'static>, caller: IdentityToolCaller| {
        let tools = tools.clone();
        let arguments = arguments.to_string();
        async move {
            tools
                .call(&caller, &site, IDENTITY_CREATE, &arguments)
                .await
                .expect("the tool answers")
        }
    };
    let first = at(
        IdentityToolCallSite::in_round(RUN, 0, "fc_0"),
        caller.clone(),
    )
    .await;
    let again = at(
        IdentityToolCallSite::in_round(RUN, 0, "fc_0"),
        caller.clone(),
    )
    .await;
    assert_eq!(
        first, again,
        "the same call dispatched again reads the same state"
    );
    at(
        IdentityToolCallSite::in_round(RUN, 1, "fc_0"),
        caller.clone(),
    )
    .await;
    at(
        IdentityToolCallSite::in_round("run-2", 0, "fc_0"),
        caller.clone(),
    )
    .await;
    at(
        IdentityToolCallSite::in_round(RUN, 0, "fc_1"),
        caller.clone(),
    )
    .await;
    at(
        IdentityToolCallSite::in_round(RUN, 0, "fc_0"),
        IdentityToolCaller::new(LEAD, meerkat_core::types::SessionId::new()),
    )
    .await;
    let operations = rig.family.operations();
    assert_eq!(operations[0], operations[1], "one operation for one call");
    assert_eq!(
        operations.iter().collect::<BTreeSet<_>>().len(),
        5,
        "another round, another run, another call id and another session are other \
         operations: {operations:?}"
    );

    // With a request id the operation is the requester's request, wherever
    // the call ran.
    rig.family.forget();
    let named = json!({"request_id": "team-1", "agents": [{"name": "scout", "profile": WORKER}]});
    for site in [
        IdentityToolCallSite::in_round(RUN, 0, "fc_0"),
        IdentityToolCallSite::in_round("run-9", 4, "fc_3"),
        IdentityToolCallSite::outside_a_run("host-call"),
    ] {
        tools
            .call(&caller, &site, IDENTITY_CREATE, &named.to_string())
            .await
            .expect("the tool answers");
    }
    let expected = OperationId::for_request(
        &Requester::Agent {
            identity: LEAD.to_owned(),
        },
        "team-1",
    )
    .expect("a request id");
    assert_eq!(
        rig.family.operations(),
        vec![expected.as_str().to_owned(); 3],
        "a request id names the operation by the requester"
    );

    // Outside a run there is no place to derive from: the call must give a
    // request id, and nothing reaches the family.
    rig.family.forget();
    let answer = tools
        .call(
            &caller,
            &IdentityToolCallSite::outside_a_run("host-call"),
            IDENTITY_CREATE,
            &arguments.to_string(),
        )
        .await
        .expect("the tool answers");
    assert_eq!(
        (
            answer.content["kind"].as_str(),
            answer.content["field"].as_str()
        ),
        (Some("invalid_request"), Some("request_id")),
        "{}",
        answer.content
    );
    assert!(rig.family.seen().is_empty());
    rig.shutdown().await;
}

/// The family's typed outcome as every surface writes it, without its
/// operation id: what the agent that asked reads of its own change.
fn without_operation_id(outcome: &IdentityOutcome) -> Value {
    let mut read = outcome.to_json();
    assert!(
        read.as_object_mut()
            .expect("an outcome is an object")
            .remove("operation_id")
            .is_some(),
        "the family's outcome carries the operation id: {outcome:?}"
    );
    read
}

/// The family's outcome is the tool's answer, member for member, without
/// the operation id, which an agent is not shown: a held decision, a
/// pending preparation, an update's instruction outcome and a refusal with
/// its kind's details. Only a refusal is an error result. An agent that
/// names itself reaches the family as requester and target alike, and the
/// family's `self_change_refused` is what the tool answers.
#[tokio::test(flavor = "multi_thread")]
async fn the_familys_outcome_is_the_tools_answer_without_its_operation_id() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_family(state.path(), &fresh_mob_id()).await;
    rig.send(LEAD, "seat-lead", "seat-lead").await;
    let digest = sha("read");
    let current = sha("current");
    let operation = OperationId::for_request(
        &Requester::Agent {
            identity: LEAD.to_owned(),
        },
        "r-1",
    )
    .unwrap();
    let update = json!({
        "request_id": "r-1",
        "name": "scout",
        "expected_spec_digest": digest.as_str(),
        "set": {"instructions": ["Rank them."]},
    });

    let held = IdentityOutcome::PendingDecision {
        operation_id: operation.clone(),
        decision_id: "decision-7".to_owned(),
        deadline_ms: 1_800_000_000_000,
    };
    rig.family.answer(held.clone());
    let answer = call(&rig, IDENTITY_UPDATE, update.clone()).await;
    assert_eq!(answer.content, without_operation_id(&held));
    assert_eq!(
        answer.content,
        json!({
            "outcome": "pending_decision",
            "decision_id": "decision-7",
            "deadline_ms": 1_800_000_000_000_u64,
        })
    );
    assert!(!answer.refused, "a held decision is no error result");

    let preparing = IdentityOutcome::PendingPreparation {
        operation_id: operation.clone(),
        agents: vec![AgentOutcome::new(
            "scout",
            AgentChangeState::PendingPreparation,
        )],
    };
    rig.family.answer(preparing.clone());
    let answer = call(&rig, IDENTITY_UPDATE, update.clone()).await;
    assert_eq!(answer.content, without_operation_id(&preparing));
    assert_eq!(answer.content["outcome"], "pending_preparation");
    assert!(!answer.refused);

    let mut agent = AgentOutcome::new("scout", AgentChangeState::Committed);
    agent.spec_digest = Some(current.clone());
    agent.spec_revision = Some(2);
    let updated = IdentityOutcome::Done {
        operation_id: operation.clone(),
        agents: vec![agent],
    }
    .with_instruction_outcome("scout", InstructionOutcome::AppliesAtNextRun);
    rig.family.answer(updated.clone());
    let answer = call(&rig, IDENTITY_UPDATE, update.clone()).await;
    assert_eq!(answer.content, without_operation_id(&updated));
    assert_eq!(
        answer.content["agents"][0]["instructions"],
        json!({"kind": "applies_at_next_run"}),
        "an update that changed the instructions carries the typed instruction outcome"
    );
    assert_eq!(answer.content["agents"][0]["spec_digest"], current.as_str());

    let stale = IdentityRefusal::new(IdentityRefusalKind::StaleSpec {
        current_digest: current.clone(),
        current_revision: Some(2),
    })
    .at("expected_spec_digest");
    rig.family.answer(IdentityOutcome::Refused(stale.clone()));
    let answer = call(&rig, IDENTITY_UPDATE, update.clone()).await;
    let mut expected = stale.to_refused();
    expected["outcome"] = json!("refused");
    assert_eq!(
        answer.content, expected,
        "a refusal goes through to_refused"
    );
    assert_eq!(answer.content["kind"], "stale_spec");
    assert_eq!(answer.content["current_digest"], current.as_str());
    assert_eq!(answer.content["field"], "expected_spec_digest");
    assert_eq!(answer.content["retryable"], false);
    assert!(answer.refused, "a refusal is an error result");

    // A refusal that names an argument keeps its path: a first message in
    // a governed realm, which the family refuses before anything commits.
    let governed = IdentityRefusal::new(IdentityRefusalKind::Unsupported {
        feature: "a first message in a governed realm".to_owned(),
    })
    .at("agents[0].first_message");
    rig.family
        .answer(IdentityOutcome::Refused(governed.clone()));
    let answer = call(
        &rig,
        IDENTITY_CREATE,
        json!({"agents": [{"name": "scout", "profile": WORKER, "first_message": "Begin."}]}),
    )
    .await;
    let mut expected = governed.to_refused();
    expected["outcome"] = json!("refused");
    assert_eq!(answer.content, expected);
    assert_eq!(answer.content["kind"], "unsupported");
    assert_eq!(answer.content["field"], "agents[0].first_message");
    assert!(answer.refused);

    // An agent naming itself: the tool hands the family the calling
    // session's identity as requester, so the family's own refusal is the
    // answer for every change of one agent.
    let own = IdentityRefusal::new(IdentityRefusalKind::SelfChangeRefused).at("name");
    rig.family.answer(IdentityOutcome::Refused(own.clone()));
    for tool in [
        IDENTITY_UPDATE,
        IDENTITY_ARCHIVE,
        IDENTITY_UNARCHIVE,
        IDENTITY_REMOVE,
    ] {
        rig.family.forget();
        let answer = call(
            &rig,
            tool,
            json!({"name": LEAD, "expected_spec_digest": digest.as_str()}),
        )
        .await;
        let seen = rig.family.seen();
        let [
            Seen::Change {
                requester, change, ..
            },
        ] = seen.as_slice()
        else {
            panic!("{tool} called the family once: {seen:?}");
        };
        assert_eq!(
            requester.recorded().agent_identity(),
            Some(LEAD),
            "{tool}: the requester is the calling agent"
        );
        assert_eq!(
            names_of(change),
            vec![LEAD.to_owned()],
            "{tool}: so is the target"
        );
        assert_eq!(answer.content["kind"], "self_change_refused", "{tool}");
        assert_eq!(answer.content["field"], "name", "{tool}");
        assert!(answer.refused);
    }

    // A refused read is a refused outcome.
    *rig.family.refuse_reads.lock().unwrap() = Some(IdentityRefusal::new(
        IdentityRefusalKind::ProfileNotAllowed {
            profile: WORKER.to_owned(),
        },
    ));
    let answer = call(&rig, IDENTITY_LIST, json!({"profile": WORKER})).await;
    assert_eq!(answer.content["outcome"], "refused");
    assert_eq!(answer.content["kind"], "profile_not_allowed");
    assert!(answer.refused);
    rig.shutdown().await;
}

/// The reads never wake an agent: after `identity_list`, `identity_get` and
/// `identity_directory`, called in a real turn, a dormant agent the mob
/// holds has still built nothing, and its model was asked nothing.
#[tokio::test(flavor = "multi_thread")]
async fn identity_list_and_identity_directory_wake_nothing() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_family(state.path(), &fresh_mob_id()).await;
    rig.create("keeper", WORKER).await;
    let keeper = rig.session_of("keeper").await;
    let unborn = |rig: &Rig| rig.runtime.mob_handle().session_unborn(&keeper);
    assert!(
        unborn(&rig),
        "a created agent is seated dormant: nothing is built"
    );

    for (tool, arguments) in [
        (IDENTITY_LIST, json!({})),
        (IDENTITY_GET, json!({"name": "keeper"})),
        (IDENTITY_DIRECTORY, json!({})),
    ] {
        let answer = rig
            .tool(LEAD, &format!("read-{tool}"), tool, arguments)
            .await;
        assert!(!answer.is_error, "{tool} answers: {answer:?}");
    }
    assert!(
        unborn(&rig),
        "after the reads the dormant agent has still built nothing"
    );
    let asked = rig.model.asked.lock().unwrap().clone();
    assert!(
        asked
            .iter()
            .all(|asked| asked.text.contains("read-identity_")),
        "only the reading agent's model was asked anything: {asked:?}"
    );
    rig.shutdown().await;
}

/// The identity tools declare themselves as tools that may wait behind the
/// deferred catalog, under one source, so a session with an exact catalog
/// carries their definitions only once it loaded them.
#[test]
fn the_identity_tools_may_wait_behind_the_deferred_catalog() {
    let catalog = identity_tool_catalog();
    assert_eq!(
        catalog
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect::<Vec<_>>(),
        IDENTITY_TOOLS
    );
    for entry in &catalog {
        assert_eq!(entry.plane, meerkat_core::ToolPlaneClass::Session);
        assert!(entry.callability.is_callable());
        let meerkat_core::ToolCatalogDeferredEligibility::DeferredEligible { provenance } =
            &entry.deferred_eligibility
        else {
            panic!("{} may wait behind the deferred catalog", entry.tool.name);
        };
        assert_eq!(provenance.kind, meerkat_core::types::ToolSourceKind::Mob);
        assert_eq!(provenance.source_id.to_string(), "identity");
        assert_eq!(entry.tool.provenance.as_ref(), Some(provenance));
    }
    assert_eq!(
        meerkat_core::select_catalog_mode_from_snapshot(true, &catalog, &[]),
        meerkat_core::ToolCatalogMode::Deferred,
        "in a session whose catalog is exact, the identity tools alone are deferred"
    );
}

fn schema_of(tool: &str) -> Value {
    identity_tool_defs()
        .iter()
        .find(|def| def.name == tool)
        .unwrap_or_else(|| panic!("{tool} is defined"))
        .input_schema
        .clone()
}

fn property_names(schema: &Value) -> BTreeSet<String> {
    schema["properties"]
        .as_object()
        .expect("an object schema")
        .keys()
        .cloned()
        .collect()
}

fn member_names(value: &impl serde::Serialize) -> BTreeSet<String> {
    serde_json::to_value(value)
        .unwrap()
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect()
}

/// Every schema is the family's wire request, member for member, without
/// the two members only the host plane writes and without the two only a
/// host sets (an agent reads them back and does not set them), and closed
/// at every depth: a member added to a wire type fails this row until the
/// tool names it or is told to leave it out.
#[test]
fn every_schema_names_the_wire_request_without_the_host_plane() {
    let host_plane: BTreeSet<String> = ["proposer", "source_evidence"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    // What a host's create and update set and an agent's do not.
    let host_sets: BTreeSet<String> = ["addressability", "runtime_mode_override"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let evidence = meerkat_meta::identity_creation::SourceEvidence::new("a", "b", "c", 1);
    let digest = sha("spec");

    let mut agent = AgentSpec::new("scout", WORKER);
    agent.display_name = Some("Scout".to_owned());
    agent.instructions = vec!["brief".to_owned()];
    agent.context = Some(json!({}));
    agent.labels = BTreeMap::from([("k".to_owned(), "v".to_owned())]);
    agent.first_message = Some(ContentInput::Text("go".to_owned()));
    agent.peers = vec![LEAD.to_owned()];
    agent.addressability = Some(AgentAddressability::InternalOnly);
    agent.runtime_mode_override = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    let mut create = CreateAgents::new(vec![agent.clone()]);
    create.request_id = Some("r".to_owned());
    create.proposer = Some("p".to_owned());
    create.source_evidence = Some(evidence.clone());
    let schema = schema_of(IDENTITY_CREATE);
    assert_eq!(
        &property_names(&schema) | &host_plane,
        member_names(&create),
        "identity_create"
    );
    assert_eq!(
        &property_names(&schema["properties"]["agents"]["items"]) | &host_sets,
        member_names(&agent),
        "an agent of identity_create"
    );
    assert!(
        property_names(&schema["properties"]["agents"]["items"]).is_disjoint(&host_sets),
        "an agent of identity_create takes neither member a host sets"
    );

    let mut set = AgentSet::default();
    set.display_name = Some("Scout".to_owned());
    set.instructions = Some(vec!["brief".to_owned()]);
    set.context = Some(json!({}));
    set.labels = Some(BTreeMap::from([("k".to_owned(), "v".to_owned())]));
    set.labels_remove = vec!["k".to_owned()];
    set.profile = Some(WORKER.to_owned());
    set.addressability = Some(AgentAddressability::InternalOnly);
    set.runtime_mode_override = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    let mut update = UpdateAgent::new("scout", digest.clone());
    update.request_id = Some("r".to_owned());
    update.set = set.clone();
    update.peers_add = vec![LEAD.to_owned()];
    update.peers_remove = vec![PLAIN.to_owned()];
    update.proposer = Some("p".to_owned());
    update.source_evidence = Some(evidence.clone());
    let schema = schema_of(IDENTITY_UPDATE);
    assert_eq!(
        &property_names(&schema) | &host_plane,
        member_names(&update),
        "identity_update"
    );
    assert_eq!(
        &property_names(&schema["properties"]["set"]) | &host_sets,
        member_names(&set),
        "the set of identity_update"
    );
    assert!(
        property_names(&schema["properties"]["set"]).is_disjoint(&host_sets),
        "the set of identity_update takes neither member a host sets"
    );

    let mut one = AgentRef::new("scout", digest);
    one.request_id = Some("r".to_owned());
    one.proposer = Some("p".to_owned());
    one.source_evidence = Some(evidence);
    for tool in [IDENTITY_ARCHIVE, IDENTITY_UNARCHIVE, IDENTITY_REMOVE] {
        assert_eq!(
            &property_names(&schema_of(tool)) | &host_plane,
            member_names(&one),
            "{tool}"
        );
    }

    let mut list = ListAgents::default();
    list.profile = Some(WORKER.to_owned());
    list.after = Some("a".to_owned());
    assert_eq!(
        property_names(&schema_of(IDENTITY_LIST)),
        member_names(&list),
        "identity_list"
    );
    let mut directory = ReadDirectory::default();
    directory.query = Some("q".to_owned());
    directory.after = Some("a".to_owned());
    assert_eq!(
        property_names(&schema_of(IDENTITY_DIRECTORY)),
        member_names(&directory),
        "identity_directory"
    );

    fn closed(schema: &Value, at: &str) {
        if schema["type"] == "object" && schema.get("properties").is_some() {
            assert_eq!(
                schema["additionalProperties"],
                json!(false),
                "{at} is closed"
            );
            for (name, member) in schema["properties"].as_object().unwrap() {
                assert!(
                    member["description"]
                        .as_str()
                        .is_some_and(|doc| !doc.is_empty()),
                    "{at}.{name} says what it is"
                );
                closed(member, &format!("{at}.{name}"));
            }
        }
        if let Some(items) = schema.get("items") {
            closed(items, &format!("{at}[]"));
        }
    }
    for tool in IDENTITY_TOOLS {
        closed(&schema_of(tool), tool);
    }
}

/// What a model must be told, in the words the product settled on: these
/// sentences are in the descriptions, and no description says what is not
/// in this release.
#[test]
fn the_descriptions_say_what_a_model_must_know() {
    let text = |tool: &str| {
        identity_tool_defs()
            .iter()
            .find(|def| def.name == tool)
            .unwrap_or_else(|| panic!("{tool} is defined"))
            .description
            .clone()
    };
    let create = text(IDENTITY_CREATE);
    for sentence in [
        "is the same kind of agent as one the host configured",
        "You are recorded as who asked for it. You do not own it, it does not report to you, \
         and it is not limited to your tools.",
        "a `profile` of the mob, which gives its model, tools and skills",
        "nothing is wired to you unless you list your own name",
        "a kickoff with no requester",
        "That turn has the same tools as any other turn of the agent.",
        "A create with a first message answers once the new agent's session exists; the first \
         run continues after the answer.",
        "In a governed realm an agent is created without a first message: a create that gives \
         one is refused, and the agent's first input comes through the host's own ingress.",
        "created together or not at all",
        "Do not repeat the call with new arguments",
        "If you are a long-term agent yourself, a notice reaches you when it is decided; in \
         every case the identical call with the same `request_id` returns the current state.",
        "A name that was removed is never available again",
        // An agent the host prepares, in plain words: what it is until the
        // host settles it, that no tool settles it, and where it is read
        // (`an_agent_reads_that_an_agent_waits_for_its_hosts_preparation`).
        "- `pending_preparation`: created, and at least one agent (its `state` is \
         `pending_preparation`) waits for the host to prepare it.",
        "Until the host settles that, the agent has no member, takes no message and runs \
         nothing; its first message runs once when it is settled.",
        "No tool settles it, and nothing you send speeds it up: identity_list and identity_get \
         read it as `readiness: \"preparation_pending\"`, and the identical call with the same \
         `request_id` returns where it stands.",
        "`refused`",
    ] {
        assert!(
            create.contains(sentence),
            "identity_create says: {sentence}"
        );
    }
    let update = text(IDENTITY_UPDATE);
    for sentence in [
        // A move into a profile the host prepares, and an update of an agent
        // that waits: the same row.
        "A move into a profile whose agents the host prepares is refused `unsupported` at \
         `set.profile`: create the agent in that profile instead.",
        "An update of an agent that waits for its host's preparation changes its record only: \
         the agent still waits.",
        "The result's `outcome` is `done`, `pending_decision` or `refused`,",
        "It reaches the agent at its next run; a run that starts within moments of the change \
         may still use the earlier instructions; earlier instructions are never rewritten.",
        "`applies_at_next_run`",
        "`applies_after_compaction`",
        "`applies_at_next_session`",
        "`unchanged`",
        "You cannot change yourself",
        "`stale_spec` with `current_digest`",
    ] {
        assert!(
            update.contains(sentence),
            "identity_update says: {sentence}"
        );
    }
    assert!(text(IDENTITY_ARCHIVE).contains("identity_unarchive brings it back"));
    assert!(text(IDENTITY_ARCHIVE).contains("You cannot archive yourself"));
    assert!(text(IDENTITY_UNARCHIVE).contains("You cannot unarchive yourself"));
    assert!(text(IDENTITY_REMOVE).contains("its name can never be used again"));
    assert!(text(IDENTITY_REMOVE).contains("use identity_archive when the agent may be needed"));
    // An archive and a removal owe an agent created at run time what
    // leaving the roster owes a configured one: both texts say so.
    for tool in [IDENTITY_ARCHIVE, IDENTITY_REMOVE] {
        assert!(
            text(tool).contains(
                "the agent's last conversation is harvested as for any agent that leaves"
            ),
            "{tool} says the agent's last conversation is harvested"
        );
    }
    assert!(text(IDENTITY_LIST).contains("it never wakes an agent"));
    // What the list shows is what the family lists, not what the caller may
    // create: `identity_list_shows_an_agent_of_a_profile_the_caller_may_not_create`.
    assert!(text(IDENTITY_LIST).contains("every agent that is not private to a person"));
    assert!(
        text(IDENTITY_LIST)
            .contains("An agent of a profile you may not create agents of is listed too")
    );
    assert!(!text(IDENTITY_LIST).contains("your grant covers"));
    // An agent that waits for its host's preparation is read in plain words
    // by the two reads, and an update never answers a preparation.
    assert!(
        text(IDENTITY_LIST).contains(
            "`preparation_pending` (it waits for its host to prepare it: no member, no message \
             taken, nothing runs until the host settles that; no tool of yours does)"
        ),
        "identity_list says what `preparation_pending` is"
    );
    assert!(
        text(IDENTITY_GET).contains(
            "An agent that waits for its host's preparation reads `readiness: \
             \"preparation_pending\"`: it takes no message until the host settles that."
        ),
        "identity_get says how an agent that waits reads"
    );
    assert!(
        !update.contains("`pending_preparation`"),
        "an update answers no preparation: its description names no such outcome"
    );
    // The settle of a preparation is the host's: there is no tool for it,
    // and no description names the host's method as something to call.
    assert_eq!(IDENTITY_TOOLS.len(), 8);
    assert!(!IDENTITY_TOOLS.iter().any(|tool| tool.contains("settle")));
    for tool in IDENTITY_TOOLS {
        assert!(
            !text(tool).contains("settle_preparation"),
            "{tool} names no settle method"
        );
    }
    // The two members a host sets are read back by the two reads and set by
    // no tool: the create and the update do not name them.
    for tool in [IDENTITY_LIST, IDENTITY_GET] {
        assert!(
            text(tool).contains(
                "`addressability` and `runtime_mode_override` are read back only: the host \
                 sets them, and identity_create and identity_update do not take them."
            ),
            "{tool} says the two members are read back and not set here"
        );
    }
    for tool in [IDENTITY_CREATE, IDENTITY_UPDATE] {
        let all = format!("{} {}", text(tool), schema_of(tool));
        for absent in ["addressability", "runtime_mode"] {
            assert!(!all.contains(absent), "{tool} names no `{absent}`");
        }
    }
    // A name the caller is shown no agent under: one answer, whatever the
    // reason (`identity_list_shows_an_agent_of_a_profile_the_caller_may_not_create`).
    for tool in [
        IDENTITY_UPDATE,
        IDENTITY_ARCHIVE,
        IDENTITY_UNARCHIVE,
        IDENTITY_REMOVE,
    ] {
        assert!(
            text(tool).contains(
                "A `name` that no agent holds, and an agent you are not shown (one private to \
                 a person), both answer `identity_unknown`."
            ),
            "{tool} says what a name it is shown no agent under answers"
        );
        // `an_agent_changes_no_agent_the_hosts_configuration_declares`.
        assert!(
            text(tool).contains(
                "An agent that the host's configuration declares is changed there, not here \
                 (`owner_read_only`)."
            ),
            "{tool} says a configured agent takes no change here"
        );
    }
    assert!(
        text(IDENTITY_DIRECTORY)
            .contains("the agents of your mob that are neither archived nor private to a person")
    );
    // No text has an agent act for another: who asks is the caller.
    for tool in IDENTITY_TOOLS {
        let lower = text(tool).to_lowercase();
        for absent in ["on behalf of", "for another agent", "as another agent"] {
            assert!(!lower.contains(absent), "{tool} names no `{absent}`");
        }
    }
    assert!(text(IDENTITY_DIRECTORY).contains("it never wakes an agent"));
    for tool in IDENTITY_TOOLS {
        let all = format!("{} {}", text(tool), schema_of(tool)).to_lowercase();
        for absent in [
            "grants",
            "schedule",
            "fork_from",
            "mob_id",
            "identity_reset",
            "review",
            "retire ",
            "provenance",
            "next turn",
        ] {
            assert!(!all.contains(absent), "{tool} does not say `{absent}`");
        }
    }
}

/// The pinned text of every tool: its description and its argument schema,
/// as a model reads them. They are product text, read and settled like
/// product text, so a change here is deliberate: run the row with
/// `UPDATE_IDENTITY_TOOL_GOLDENS=1` to print the new pins and paste them
/// into `identity_tool_goldens.rs`.
#[test]
fn every_identity_tool_text_is_pinned() {
    let defs = identity_tool_defs();
    assert_eq!(
        defs.iter()
            .map(|def| def.name.to_string())
            .collect::<Vec<_>>(),
        IDENTITY_TOOLS
    );
    if std::env::var_os("UPDATE_IDENTITY_TOOL_GOLDENS").is_some() {
        for def in defs.iter() {
            println!("=== {} description\n{}", def.name, def.description);
            println!(
                "=== {} schema\n{}",
                def.name,
                serde_json::to_string_pretty(&def.input_schema).unwrap()
            );
        }
        println!("=== end");
        return;
    }
    assert_eq!(goldens::PINS.len(), defs.len());
    for (def, (name, description, schema)) in defs.iter().zip(goldens::PINS) {
        assert_eq!(def.name, *name);
        assert_eq!(
            def.description, *description,
            "the description of {name} changed"
        );
        assert_eq!(
            serde_json::to_string_pretty(&def.input_schema).unwrap(),
            *schema,
            "the argument schema of {name} changed"
        );
    }
}

#[path = "support/identity_tool_goldens.rs"]
mod goldens;

/// The identity policy of the end-to-end row: agents of the profile with
/// mob tools may create, change, archive and remove agents of the worker
/// profile, and wire them to any agent.
fn policy() -> Arc<IdentityCreationPolicy> {
    Arc::new(
        IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "peers": {"default": "allow"},
            "approval": {"rules": [{
                "change": ["create", "descriptive", "peers", "archive", "remove"],
                "requester": {"kind": "agent", "profile": [MAKER]},
                "target": {"profile": [WORKER]},
                "outcome": {"kind": "allow"},
            }]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// End to end, on the real identity family under a host's policy: an agent
/// looks up what it may name, creates a long-term agent, lists it, changes
/// it, is refused a stale digest and a change of itself, archives it (a
/// message to it is then refused), brings it back and removes it (its name
/// is then never available again). Each step is one tool call the agent's
/// model makes in a real turn, and each answer is the family's own.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_creates_changes_archives_and_removes_an_agent_under_the_hosts_policy() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;
    assert!(
        rig.runtime
            .attach_identity_policy(policy(), IdentityFamilyOptions::new()),
        "the launch attaches its identity policy"
    );
    let done = |answer: &ToolAnswer, what: &str| {
        assert_eq!(
            answer.content["outcome"], "done",
            "{what}: {}",
            answer.content
        );
        assert!(!answer.is_error, "{what}");
        answer.content["agents"][0].clone()
    };
    let refused = |answer: &ToolAnswer, kind: &str, what: &str| {
        assert_eq!(
            (
                answer.content["outcome"].as_str(),
                answer.content["kind"].as_str()
            ),
            (Some("refused"), Some(kind)),
            "{what}: {}",
            answer.content
        );
        assert!(answer.is_error, "{what}");
    };

    // What the agent may name.
    let directory = rig
        .tool(LEAD, "e2e-directory", IDENTITY_DIRECTORY, json!({}))
        .await;
    assert!(!directory.is_error, "{directory:?}");
    let verdict_of = |profile: &str| {
        directory.content["profiles"]
            .as_array()
            .expect("the profiles")
            .iter()
            .find(|entry| entry["profile"] == profile)
            .unwrap_or_else(|| panic!("{profile} is in the directory: {}", directory.content))["verdict"]
            .clone()
    };
    assert_eq!(
        verdict_of(WORKER),
        "allowed",
        "the policy allows the worker profile"
    );
    assert_eq!(verdict_of(MAKER), "not_allowed", "and no other");

    // Create: the agent exists, dormant, with nothing built.
    let created = rig
        .tool(
            LEAD,
            "e2e-create",
            IDENTITY_CREATE,
            json!({
                "request_id": "scout-1",
                "agents": [{
                    "name": "scout",
                    "profile": WORKER,
                    "display_name": "Scout",
                    "instructions": ["Find the open questions."],
                }],
            }),
        )
        .await;
    let agent = done(&created, "the create");
    assert_eq!(agent["name"], "scout");
    let first_digest = agent["spec_digest"]
        .as_str()
        .expect("a spec digest")
        .to_owned();
    let scout = rig.session_of("scout").await;
    assert!(
        rig.runtime.mob_handle().session_unborn(&scout),
        "the new agent is seated dormant: nothing is built"
    );

    // The same request again reads the same state and creates nothing.
    let again = rig
        .tool(
            LEAD,
            "e2e-create-again",
            IDENTITY_CREATE,
            json!({
                "request_id": "scout-1",
                "agents": [{
                    "name": "scout",
                    "profile": WORKER,
                    "display_name": "Scout",
                    "instructions": ["Find the open questions."],
                }],
            }),
        )
        .await;
    assert_eq!(
        done(&again, "the same request again")["spec_digest"],
        first_digest.as_str()
    );

    // List: the agent, with the digest a change sends, and no provenance.
    let listed = rig.tool(LEAD, "e2e-list", IDENTITY_LIST, json!({})).await;
    let entry = listed.content["agents"]
        .as_array()
        .expect("the agents")
        .iter()
        .find(|entry| entry["name"] == "scout")
        .unwrap_or_else(|| panic!("the new agent is listed: {}", listed.content))
        .clone();
    assert_eq!(entry["spec_digest"], first_digest.as_str());
    assert_eq!(entry["profile"], WORKER);
    // The agent that asked is itself listed, as any agent: what the list
    // never says is that it asked for this one.
    let said = listed.content.to_string();
    assert!(
        entry.get("provenance").is_none()
            && ["provenance", "requester", "consent", "at_ms"]
                .iter()
                .all(|hidden| !said.contains(hidden)),
        "the list does not say who asked for the agent: {said}"
    );

    // Update: a new digest; the earlier one is then stale.
    let update = |digest: &str| {
        json!({
            "name": "scout",
            "expected_spec_digest": digest,
            "set": {"display_name": "Scout II", "instructions": ["Find and rank the open questions."]},
        })
    };
    let updated = rig
        .tool(LEAD, "e2e-update", IDENTITY_UPDATE, update(&first_digest))
        .await;
    let second_digest = done(&updated, "the update")["spec_digest"]
        .as_str()
        .expect("a spec digest")
        .to_owned();
    assert_ne!(second_digest, first_digest, "the update moved the spec");
    let stale = rig
        .tool(LEAD, "e2e-stale", IDENTITY_UPDATE, update(&first_digest))
        .await;
    refused(
        &stale,
        "stale_spec",
        "an update that names the earlier digest",
    );
    assert_eq!(stale.content["current_digest"], second_digest.as_str());

    // An agent never changes itself.
    let own = rig
        .tool(
            LEAD,
            "e2e-self",
            IDENTITY_ARCHIVE,
            json!({"name": LEAD, "expected_spec_digest": second_digest}),
        )
        .await;
    refused(
        &own,
        "self_change_refused",
        "an archive of the calling agent",
    );

    // Archive: the agent is kept, and a message to it is refused.
    let one = |digest: &str| json!({"name": "scout", "expected_spec_digest": digest});
    let archived = rig
        .tool(LEAD, "e2e-archive", IDENTITY_ARCHIVE, one(&second_digest))
        .await;
    let state_of = |agent: &Value| agent["state"].as_str().unwrap_or_default().to_owned();
    assert!(
        ["archived", "archiving"].contains(&state_of(&done(&archived, "the archive")).as_str()),
        "{}",
        archived.content
    );
    let sent = rig
        .rpc(
            "meta/send",
            json!({"identity": "scout", "content": "are you there"}),
        )
        .await;
    assert!(
        !sent["error"].is_null(),
        "a message to an archived agent is refused: {sent:#}"
    );

    // Unarchive, then remove: the name is never available again.
    let restored = rig
        .tool(
            LEAD,
            "e2e-unarchive",
            IDENTITY_UNARCHIVE,
            one(&second_digest),
        )
        .await;
    assert_eq!(state_of(&done(&restored, "the unarchive")), "restored");
    let removed = rig
        .tool(LEAD, "e2e-remove", IDENTITY_REMOVE, one(&second_digest))
        .await;
    assert!(
        ["removed", "retiring"].contains(&state_of(&done(&removed, "the removal")).as_str()),
        "{}",
        removed.content
    );
    let reused = rig
        .tool(
            LEAD,
            "e2e-reuse",
            IDENTITY_CREATE,
            json!({"agents": [{"name": "scout", "profile": WORKER}]}),
        )
        .await;
    refused(
        &reused,
        "name_taken",
        "a create of the removed agent's name",
    );
    assert_eq!(
        reused.content["field"], "agents[0].name",
        "the refusal names the argument: {}",
        reused.content
    );
    rig.shutdown().await;
}

/// An agent that holds the grant creates a long-term agent with its tool,
/// and the created agent exists after a restart with no roster listing: the
/// agent that asked lists it again, with the same spec digest, and the mob
/// holds its seat.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_an_agents_tool_created_exists_after_a_restart() {
    let state = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot(state.path(), &mob_id).await;
    assert!(
        rig.runtime
            .attach_identity_policy(policy(), IdentityFamilyOptions::new())
    );
    let created = rig
        .tool(
            LEAD,
            "restart-create",
            IDENTITY_CREATE,
            json!({"agents": [{"name": "keeper", "profile": WORKER, "display_name": "Keeper"}]}),
        )
        .await;
    assert_eq!(created.content["outcome"], "done", "{}", created.content);
    let digest = created.content["agents"][0]["spec_digest"].clone();
    rig.shutdown_for_reopen().await;

    let rig = boot(state.path(), &mob_id).await;
    assert!(
        rig.runtime
            .attach_identity_policy(policy(), IdentityFamilyOptions::new())
    );
    let listed = rig
        .tool(LEAD, "restart-list", IDENTITY_LIST, json!({}))
        .await;
    let entry = listed.content["agents"]
        .as_array()
        .expect("the agents")
        .iter()
        .find(|entry| entry["name"] == "keeper")
        .unwrap_or_else(|| {
            panic!(
                "the created agent is listed after the restart: {}",
                listed.content
            )
        })
        .clone();
    assert_eq!(
        entry["spec_digest"], digest,
        "with the spec it was created with"
    );
    assert_eq!(entry["display_name"], "Keeper");
    let keeper = core_member_name("keeper").expect("an agent name");
    assert!(
        rig.runtime
            .mob_handle()
            .get_member(&keeper)
            .await
            .expect("the roster reads")
            .is_some(),
        "the mob holds the created agent's seat after the restart"
    );
    rig.shutdown().await;
}

/// An agent is refused, typed, a change the host's rules do not give it:
/// a create of a profile no rule allows it, and an archive and a removal of
/// an agent of such a profile. Nothing is written: the agent it may not act
/// on is unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_is_refused_a_change_the_hosts_rules_do_not_give_it() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;
    assert!(
        rig.runtime
            .attach_identity_policy(policy(), IdentityFamilyOptions::new())
    );
    // An agent of the profile the rules do not cover, created by the host.
    rig.create("warden", MAKER).await;
    let listed = rig
        .tool(LEAD, "denied-list", IDENTITY_LIST, json!({}))
        .await;
    let digest = listed.content["agents"]
        .as_array()
        .expect("the agents")
        .iter()
        .find(|entry| entry["name"] == "warden")
        .unwrap_or_else(|| panic!("the host's agent is listed: {}", listed.content))["spec_digest"]
        .clone();

    let denied = [
        (
            IDENTITY_CREATE,
            json!({"agents": [{"name": "second-lead", "profile": MAKER}]}),
        ),
        (
            IDENTITY_ARCHIVE,
            json!({"name": "warden", "expected_spec_digest": digest}),
        ),
        (
            IDENTITY_REMOVE,
            json!({"name": "warden", "expected_spec_digest": digest}),
        ),
    ];
    for (tool, arguments) in denied {
        let answer = rig
            .tool(LEAD, &format!("denied-{tool}"), tool, arguments)
            .await;
        assert_eq!(
            (
                answer.content["outcome"].as_str(),
                answer.content["kind"].as_str()
            ),
            (Some("refused"), Some("change_denied")),
            "{tool} of a profile no rule gives the agent: {}",
            answer.content
        );
        assert!(answer.is_error, "{tool}");
    }

    let after = rig
        .tool(LEAD, "denied-after", IDENTITY_LIST, json!({}))
        .await;
    let names: Vec<&str> = after.content["agents"]
        .as_array()
        .expect("the agents")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(
        names.contains(&"warden"),
        "the agent is still there: {names:?}"
    );
    assert!(
        !names.contains(&"second-lead"),
        "the refused create made nothing: {names:?}"
    );
    rig.shutdown().await;
}

/// The names `identity_list` answered, in the answer's order.
fn listed_names(answer: &ToolAnswer) -> Vec<String> {
    assert!(!answer.is_error, "the list answers: {}", answer.content);
    answer.content["agents"]
        .as_array()
        .unwrap_or_else(|| panic!("the agents: {}", answer.content))
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
        .collect()
}

/// `identity_list` shows an agent the long-term agents of its mob whatever
/// their profile: the list is no function of what the calling agent may
/// create. An agent of a profile the host's rules let the caller create no
/// agent of (the directory says `not_allowed`) is listed like any other,
/// alone and under its profile's filter, with the digest a change of it
/// would send. A worker member holds no entry of the list, private to a
/// person or not: a member private to a person is never listed.
#[tokio::test(flavor = "multi_thread")]
async fn identity_list_shows_an_agent_of_a_profile_the_caller_may_not_create() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;
    assert!(
        rig.runtime
            .attach_identity_policy(policy(), IdentityFamilyOptions::new())
    );
    // A shared agent of the profile the rules give the caller no create of.
    rig.create("warden", MAKER).await;
    // Two worker members, one private to a person and one shared: neither
    // is a long-term agent.
    rig.worker_turn("side", WORKER, &[(PRIVATE_TO, "alice")], "seen-private")
        .await;
    rig.worker_turn("temp", WORKER, &[], "seen-shared").await;

    let directory = rig
        .tool(LEAD, "seen-directory", IDENTITY_DIRECTORY, json!({}))
        .await;
    let verdict = directory.content["profiles"]
        .as_array()
        .expect("the profiles")
        .iter()
        .find(|entry| entry["profile"] == MAKER)
        .unwrap_or_else(|| panic!("the profile is in the directory: {}", directory.content))
        ["verdict"]
        .clone();
    assert_eq!(
        verdict, "not_allowed",
        "the caller may create no agent of the profile"
    );

    let listed = rig.tool(LEAD, "seen-list", IDENTITY_LIST, json!({})).await;
    let names = listed_names(&listed);
    for name in ["warden", LEAD, PLAIN] {
        assert!(
            names.iter().any(|listed| listed == name),
            "{name} is listed: {names:?}"
        );
    }
    let warden = listed.content["agents"]
        .as_array()
        .expect("the agents")
        .iter()
        .find(|entry| entry["name"] == "warden")
        .expect("the agent's entry");
    assert_eq!(warden["profile"], MAKER);
    assert!(
        warden["spec_digest"]
            .as_str()
            .is_some_and(|digest| !digest.is_empty()),
        "with its spec digest: {warden}"
    );
    for worker in ["side", "temp"] {
        assert!(
            !names.iter().any(|listed| listed == worker),
            "a worker member, private to a person or shared, is no entry of the list: {names:?}"
        );
    }

    let makers = rig
        .tool(
            LEAD,
            "seen-profile",
            IDENTITY_LIST,
            json!({"profile": MAKER}),
        )
        .await;
    let makers: BTreeSet<String> = listed_names(&makers).into_iter().collect();
    assert_eq!(
        makers,
        BTreeSet::from([LEAD.to_owned(), "warden".to_owned()]),
        "the profile filter keeps the agents of that profile"
    );

    // The directory's peers follow the same rule: the shared agent is an
    // entry, a worker member is none.
    let peers: Vec<&str> = directory.content["peers"]
        .as_array()
        .expect("the peers")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(
        peers.contains(&"warden"),
        "the agent is a peer entry: {peers:?}"
    );
    for worker in ["side", "temp"] {
        assert!(
            !peers.contains(&worker),
            "a worker member is no peer entry: {peers:?}"
        );
    }

    // A change of a name the caller is shown no agent under answers
    // `identity_unknown` at the name, whatever the reason.
    for (tool, name) in [
        (IDENTITY_ARCHIVE, "nobody"),
        (IDENTITY_REMOVE, "side"),
        (IDENTITY_UNARCHIVE, "temp"),
    ] {
        let answer = rig
            .tool(
                LEAD,
                &format!("seen-unknown-{tool}"),
                tool,
                json!({"name": name, "expected_spec_digest": warden["spec_digest"]}),
            )
            .await;
        assert_eq!(
            (
                answer.content["outcome"].as_str(),
                answer.content["kind"].as_str(),
                answer.content["field"].as_str(),
            ),
            (Some("refused"), Some("identity_unknown"), Some("name")),
            "{tool} of {name}: {}",
            answer.content
        );
        assert!(answer.is_error, "{tool} of {name}");
    }
    rig.shutdown().await;
}

/// An agent its host's configuration declares takes no run-time change from
/// an agent's tools, though the host's rules allow the change for its
/// profile: an update, an archive, an unarchive and a removal each answer
/// `owner_read_only` at the name, and the agent is unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_changes_no_agent_the_hosts_configuration_declares() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;
    assert!(
        rig.runtime
            .attach_identity_policy(policy(), IdentityFamilyOptions::new())
    );
    let digest_of = |listed: &ToolAnswer| {
        listed.content["agents"]
            .as_array()
            .expect("the agents")
            .iter()
            .find(|entry| entry["name"] == PLAIN)
            .unwrap_or_else(|| panic!("the declared agent is listed: {}", listed.content))["spec_digest"]
            .clone()
    };
    let listed = rig
        .tool(LEAD, "declared-list", IDENTITY_LIST, json!({}))
        .await;
    let digest = digest_of(&listed);

    let changes = [
        (
            IDENTITY_UPDATE,
            json!({
                "name": PLAIN,
                "expected_spec_digest": digest,
                "set": {"display_name": "Renamed"},
            }),
        ),
        (
            IDENTITY_ARCHIVE,
            json!({"name": PLAIN, "expected_spec_digest": digest}),
        ),
        (
            IDENTITY_UNARCHIVE,
            json!({"name": PLAIN, "expected_spec_digest": digest}),
        ),
        (
            IDENTITY_REMOVE,
            json!({"name": PLAIN, "expected_spec_digest": digest}),
        ),
    ];
    for (tool, arguments) in changes {
        let answer = rig
            .tool(LEAD, &format!("declared-{tool}"), tool, arguments)
            .await;
        assert_eq!(
            (
                answer.content["outcome"].as_str(),
                answer.content["kind"].as_str(),
                answer.content["field"].as_str(),
            ),
            (Some("refused"), Some("owner_read_only"), Some("name")),
            "{tool} of an agent the configuration declares: {}",
            answer.content
        );
        assert!(answer.is_error, "{tool}");
    }

    let after = rig
        .tool(LEAD, "declared-after", IDENTITY_LIST, json!({}))
        .await;
    assert_eq!(digest_of(&after), digest, "the declared agent is unchanged");
    rig.shutdown().await;
}

/// What a read of an agent's list or record must never carry: who asked for
/// the agent, when, under which consent, or an operation another requester
/// holds open for it.
const NEVER_SHOWN_TO_AN_AGENT: [&str; 9] = [
    "provenance",
    "another-agent",
    "requester",
    "consent",
    "at_ms",
    "decided",
    "open_operations",
    "held-request-1",
    "roster_listing_superseded",
];

/// `identity_get` is one of the identity tools: listed with the others, with
/// a closed schema that takes the agent's name and nothing else (the wire
/// read's own members), and a description that says what it shows, what it
/// does not, and what a name the caller is shown no agent under answers.
#[test]
fn identity_get_is_one_of_the_identity_tools() {
    assert!(
        IDENTITY_TOOLS.contains(&IDENTITY_GET),
        "the identity tools: {IDENTITY_TOOLS:?}"
    );
    assert!(is_identity_tool(IDENTITY_GET));
    let schema = schema_of(IDENTITY_GET);
    assert_eq!(
        property_names(&schema),
        member_names(&GetAgent::new("scout")),
        "the schema's members are the wire read's"
    );
    assert_eq!(schema["required"], json!(["name"]));
    assert_eq!(schema["additionalProperties"], json!(false));
    let text = identity_tool_defs()
        .iter()
        .find(|def| def.name == IDENTITY_GET)
        .expect("identity_get is defined")
        .description
        .clone();
    for sentence in [
        "it never wakes an agent",
        "`instructions`, `context`, `labels` and `peers`",
        "You are not shown who asked for the agent",
        "A `name` that no agent holds, and an agent you are not shown (one private to a \
         person), both answer `identity_unknown`.",
    ] {
        assert!(text.contains(sentence), "identity_get says: {sentence}");
    }
}

/// `identity_get` hands the family its wire read as the calling member and
/// answers the agent's record without what only the host's read shows: who
/// asked for the agent, when, under which consent, and the operations
/// another requester holds open for it. Its arguments are closed: who asks
/// is no argument, and nothing but the name is one.
#[tokio::test(flavor = "multi_thread")]
async fn identity_get_hands_the_family_its_read_and_shows_an_agent_no_provenance() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_family(state.path(), &fresh_mob_id()).await;
    rig.send(LEAD, "seat-lead", "seat-lead").await;
    let session = rig.session_of(LEAD).await;

    rig.family.forget();
    let answer = rig
        .tool(LEAD, "turn-get", IDENTITY_GET, json!({"name": "scout"}))
        .await;
    assert_eq!(
        rig.family.seen(),
        vec![Seen::Read {
            requester: ChangeRequester::agent_session(LEAD, session),
            read: IdentityRead::Get(GetAgent::new("scout")),
        }],
        "identity_get hands the family its read, as the calling member"
    );
    assert_eq!(
        answer.content,
        json!({
            "name": "scout",
            "display_name": "Scout",
            "profile": WORKER,
            "spec_digest": sha("scout").as_str(),
            "spec_revision": 3,
            "archived": true,
            "readiness": "unseated",
            "addressability": "internal_only",
            "runtime_mode_override": "turn_driven",
            "instructions": ["Find the open questions."],
            "context": {"desk": "north"},
            "labels": {"team": "research"},
            "peers": [PLAIN],
        }),
        "the family's record, without what only the host's read shows"
    );
    assert!(!answer.is_error);
    let shown = answer.content.to_string();
    for hidden in NEVER_SHOWN_TO_AN_AGENT {
        assert!(
            !shown.contains(hidden),
            "identity_get names no `{hidden}`: {shown}"
        );
    }

    rig.family.forget();
    for (arguments, field) in [
        (json!({"name": "scout", "requester": PLAIN}), "requester"),
        (json!({"name": "scout", "mob_id": "m"}), "mob_id"),
        (
            json!({"name": "scout", "include_provenance": true}),
            "include_provenance",
        ),
        (json!({}), "name"),
        (json!({"name": 7}), "name"),
    ] {
        let answer = call(&rig, IDENTITY_GET, arguments.clone()).await;
        assert_eq!(
            (
                answer.content["outcome"].as_str(),
                answer.content["kind"].as_str(),
                answer.content["field"].as_str(),
            ),
            (Some("refused"), Some("invalid_request"), Some(field)),
            "{arguments}: {}",
            answer.content
        );
        assert!(answer.refused, "{arguments}");
    }
    assert!(
        rig.family.seen().is_empty(),
        "a read the schema refuses never reaches the family: {:?}",
        rig.family.seen()
    );
    rig.shutdown().await;
}

/// End to end, on the real identity family under a host's policy: an agent
/// reads one agent's record by name. It is shown the fields the host's read
/// of the agent has (with the digest its list entry has), and never who
/// asked for the agent. A name the caller is shown no agent under answers
/// `identity_unknown` at the name in one shape, whether no agent holds the
/// name or its member is private to a person. The read wakes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn identity_get_reads_one_agent_and_answers_unknown_for_a_name_it_is_not_shown() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;
    assert!(
        rig.runtime
            .attach_identity_policy(policy(), IdentityFamilyOptions::new())
    );
    // An agent the calling agent asked for itself, with what a record holds.
    let created = rig
        .tool(
            LEAD,
            "get-create",
            IDENTITY_CREATE,
            json!({"agents": [{
                "name": "scout",
                "profile": WORKER,
                "display_name": "Scout",
                "instructions": ["Find the open questions."],
                "context": {"desk": "north"},
                "peers": [LEAD],
            }]}),
        )
        .await;
    assert_eq!(created.content["outcome"], "done", "{}", created.content);
    let digest = created.content["agents"][0]["spec_digest"].clone();
    let scout = rig.session_of("scout").await;
    // An agent the host asked for, of a profile the caller may create no
    // agent of, and a member private to a person.
    rig.create("warden", MAKER).await;
    rig.worker_turn("side", WORKER, &[(PRIVATE_TO, "alice")], "get-private")
        .await;

    let record = rig
        .tool(LEAD, "get-scout", IDENTITY_GET, json!({"name": "scout"}))
        .await;
    assert!(!record.is_error, "{record:?}");
    let listed = rig.tool(LEAD, "get-list", IDENTITY_LIST, json!({})).await;
    let entry = listed.content["agents"]
        .as_array()
        .expect("the agents")
        .iter()
        .find(|entry| entry["name"] == "scout")
        .expect("the agent's list entry")
        .clone();
    for (member, value) in entry.as_object().expect("an entry") {
        assert_eq!(
            &record.content[member], value,
            "the record carries the list entry's `{member}`"
        );
    }
    assert_eq!(record.content["spec_digest"], digest);
    assert_eq!(
        record.content["instructions"],
        json!(["Find the open questions."])
    );
    assert_eq!(record.content["context"], json!({"desk": "north"}));
    assert_eq!(record.content["peers"], json!([LEAD]));
    assert!(record.content["labels"].is_object(), "{}", record.content);
    let shown = record.content.to_string();
    for hidden in [
        "provenance",
        "requester",
        "consent",
        "at_ms",
        "open_operations",
    ] {
        assert!(
            !shown.contains(hidden),
            "an agent is not shown `{hidden}` of an agent it asked for itself: {shown}"
        );
    }
    assert!(
        rig.runtime.mob_handle().session_unborn(&scout),
        "the read built nothing of the dormant agent"
    );

    // An agent the host asked for, of a profile the caller may not create:
    // its record is read like any other, and says nothing of who asked.
    let warden = rig
        .tool(LEAD, "get-warden", IDENTITY_GET, json!({"name": "warden"}))
        .await;
    assert!(!warden.is_error, "{warden:?}");
    assert_eq!(warden.content["name"], "warden");
    assert_eq!(warden.content["profile"], MAKER);
    let shown = warden.content.to_string();
    for hidden in ["provenance", "requester", "consent", "at_ms", "console"] {
        assert!(
            !shown.contains(hidden),
            "an agent is not shown `{hidden}` of the host's agent: {shown}"
        );
    }

    // A name no agent holds and a member private to a person answer alike.
    let unknown = |name: &'static str, mark: &'static str| {
        let rig = &rig;
        async move {
            let answer = rig
                .tool(LEAD, mark, IDENTITY_GET, json!({"name": name}))
                .await;
            assert!(answer.is_error, "{name}: {answer:?}");
            answer.content
        }
    };
    let nobody = unknown("nobody", "get-nobody").await;
    let private = unknown("side", "get-side").await;
    for (name, answer) in [("nobody", &nobody), ("side", &private)] {
        assert_eq!(
            (
                answer["outcome"].as_str(),
                answer["kind"].as_str(),
                answer["field"].as_str(),
            ),
            (Some("refused"), Some("identity_unknown"), Some("name")),
            "a read of {name}: {answer}"
        );
    }
    assert_eq!(
        nobody, private,
        "nothing tells a member private to a person from a name no agent holds"
    );

    // The caller's own record is read too, and so is an archived agent's.
    let own = rig
        .tool(LEAD, "get-own", IDENTITY_GET, json!({"name": LEAD}))
        .await;
    assert!(!own.is_error, "{own:?}");
    assert_eq!(own.content["name"], LEAD);
    assert_eq!(own.content["profile"], MAKER);
    let archived = rig
        .tool(
            LEAD,
            "get-archive",
            IDENTITY_ARCHIVE,
            json!({"name": "scout", "expected_spec_digest": digest}),
        )
        .await;
    assert_eq!(archived.content["outcome"], "done", "{}", archived.content);
    let record = rig
        .tool(LEAD, "get-archived", IDENTITY_GET, json!({"name": "scout"}))
        .await;
    assert!(!record.is_error, "{record:?}");
    assert_eq!(record.content["archived"], true, "{}", record.content);
    rig.shutdown().await;
}

/// A launch's schedules as its host gives them to the family: two schedules
/// target the agent, whoever wrote them, and each change of the agent takes
/// both with it.
struct TwoSchedules;

impl TwoSchedules {
    fn both(effect: ScheduleEffect) -> SchedulesFollowed {
        SchedulesFollowed::new(
            effect,
            vec![
                ScheduleRef::new("sched-nightly", Some("nightly digest".to_owned())),
                ScheduleRef::new("sched-weekly", None),
            ],
            false,
        )
    }
}

#[async_trait]
impl AgentSchedules for TwoSchedules {
    async fn follow(
        &self,
        _mob_id: &str,
        _agent: &str,
        effect: ScheduleEffect,
    ) -> SchedulesFollowed {
        Self::both(effect)
    }

    async fn planned(
        &self,
        _mob_id: &str,
        _agent: &str,
        effect: ScheduleEffect,
    ) -> SchedulesFollowed {
        Self::both(effect)
    }
}

/// An agent that archives, brings back or removes an agent is told what
/// became of the schedules that target it and how many there are, and not
/// which: a schedule that targets an agent may be anybody's, so the list of
/// schedules is the host's to read. On the real family, with a launch whose
/// schedules follow the agent: each answer's `agents[0].schedules` is the
/// effect and the count, and no answer names a schedule.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "waits for the family to scope the schedules list by requester: until that change is on this branch the family answers every requester the per-schedule list"]
async fn an_agent_is_told_what_became_of_an_agents_schedules_not_which() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;
    assert!(
        rig.runtime.attach_identity_policy(
            policy(),
            IdentityFamilyOptions::new().with_schedules(Arc::new(TwoSchedules)),
        ),
        "the launch attaches its identity policy and its schedules"
    );
    let created = rig
        .tool(
            LEAD,
            "schedules-create",
            IDENTITY_CREATE,
            json!({"agents": [{"name": "scout", "profile": WORKER}]}),
        )
        .await;
    assert_eq!(created.content["outcome"], "done", "{}", created.content);
    let digest = created.content["agents"][0]["spec_digest"]
        .as_str()
        .expect("a spec digest")
        .to_owned();
    let one = json!({"name": "scout", "expected_spec_digest": digest});

    for (tool, effect) in [
        (IDENTITY_ARCHIVE, "paused"),
        (IDENTITY_UNARCHIVE, "resumed"),
        (IDENTITY_REMOVE, "ended"),
    ] {
        let answer = rig
            .tool(LEAD, &format!("schedules-{tool}"), tool, one.clone())
            .await;
        assert_eq!(
            answer.content["outcome"], "done",
            "{tool}: {}",
            answer.content
        );
        assert_eq!(
            answer.content["agents"][0]["schedules"],
            json!({"effect": effect, "count": 2}),
            "{tool} tells an agent the effect and the count, and no list: {}",
            answer.content
        );
        let said = answer.content.to_string();
        for hidden in [
            "sched-nightly",
            "nightly digest",
            "sched-weekly",
            "schedule_id",
        ] {
            assert!(
                !said.contains(hidden),
                "{tool} names no schedule to an agent (`{hidden}`): {said}"
            );
        }
    }
    rig.shutdown().await;
}

/// The identity policy of the preparation row: the host prepares every
/// agent of the worker profile before its first use, and agents of the
/// profile with mob tools may create agents of both profiles, change them
/// and move them to another profile.
fn prepared_policy() -> Arc<IdentityCreationPolicy> {
    Arc::new(
        IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "peers": {"default": "allow"},
            "preparation": {WORKER: {"mode": "required"}},
            "approval": {"rules": [{
                "change": ["create", "descriptive", "profile"],
                "requester": {"kind": "agent", "profile": [MAKER]},
                "target": {"profile": [WORKER, MAKER]},
                "outcome": {"kind": "allow"},
            }]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// The first message of the agent the preparation row creates. An agent's
/// tool call writes it as JSON, where its quotes are escaped, so this text
/// as it stands is in a model request only when the new agent itself is
/// asked it.
const KICKOFF: &str = r#"Chart the "tides" first."#;

/// What an agent reads of an agent its host prepares, on the real family
/// under a policy that names a preparation for the worker profile. Its
/// create answers `pending_preparation` and is no error result; until the
/// host settles it the agent has no member, takes no message, and its first
/// message is asked of no model; `identity_list` and `identity_get` read it
/// `preparation_pending`; the same create again answers the same; an update
/// changes its record and leaves it waiting; and the agent that asked is
/// offered no tool that settles it. A move of another agent into the
/// prepared profile is refused `unsupported` at `set.profile`. Once the
/// host settles it, its first message runs once, the same create answers
/// `done`, and the list reads it as any agent.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_reads_that_an_agent_waits_for_its_hosts_preparation() {
    use meerkat_mob::{EntryStanding, IdentityEntryRead, UnseatedEntryCause};

    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;
    assert!(
        rig.runtime
            .attach_identity_policy(prepared_policy(), IdentityFamilyOptions::new()),
        "the launch attaches its identity policy"
    );
    let member = core_member_name("scout").expect("an agent name");
    let waits = || {
        matches!(
            rig.runtime.mob_handle().identity_entry(&member),
            Ok(IdentityEntryRead::Entry(entry))
                if entry.standing
                    == EntryStanding::Unseated(UnseatedEntryCause::PreparationPending)
        )
    };
    let entry_of = |listed: &ToolAnswer, name: &str| {
        listed.content["agents"]
            .as_array()
            .expect("the agents")
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap_or_else(|| panic!("{name} is listed: {}", listed.content))
            .clone()
    };

    // The create: committed, and the agent waits for its host.
    let create = json!({
        "request_id": "scout-1",
        "agents": [{"name": "scout", "profile": WORKER, "first_message": KICKOFF}],
    });
    let created = rig
        .tool(LEAD, "prep-create", IDENTITY_CREATE, create.clone())
        .await;
    assert_eq!(
        created.content["outcome"], "pending_preparation",
        "{}",
        created.content
    );
    assert_eq!(
        created.content["agents"][0]["state"], "pending_preparation",
        "{}",
        created.content
    );
    assert!(
        !created.is_error,
        "an agent that waits for its host is no error result"
    );
    let digest = created.content["agents"][0]["spec_digest"]
        .as_str()
        .expect("a spec digest")
        .to_owned();

    // Until the host settles it: no member, no message taken, nothing run.
    assert!(waits(), "Core holds the agent for its preparation");
    assert!(
        !rig.runtime
            .mob_handle()
            .list_members_including_retiring()
            .await
            .iter()
            .any(|entry| entry.agent_identity == member),
        "an agent that waits has no member"
    );
    let sent = rig
        .rpc(
            "meta/send",
            json!({"identity": "scout", "content": "are you there"}),
        )
        .await;
    assert!(
        !sent["error"].is_null(),
        "an agent that waits takes no message: {sent:#}"
    );

    // The two reads say so, and show the agent that asked no operation.
    let listed = rig.tool(LEAD, "prep-list", IDENTITY_LIST, json!({})).await;
    assert_eq!(
        entry_of(&listed, "scout")["readiness"],
        "preparation_pending",
        "{}",
        listed.content
    );
    let read = rig
        .tool(LEAD, "prep-get", IDENTITY_GET, json!({"name": "scout"}))
        .await;
    assert_eq!(
        read.content["readiness"], "preparation_pending",
        "{}",
        read.content
    );
    for answer in [&listed, &read] {
        let said = answer.content.to_string();
        assert!(
            !answer.is_error && !said.contains("open_operations") && !said.contains("operation_id"),
            "a read of an agent that waits names no operation: {said}"
        );
    }

    // The same create again reads where the agent stands.
    let again = rig
        .tool(LEAD, "prep-create-again", IDENTITY_CREATE, create.clone())
        .await;
    assert_eq!(
        again.content, created.content,
        "the same create answers the same while the agent waits"
    );

    // An update changes the record and settles nothing.
    let updated = rig
        .tool(
            LEAD,
            "prep-update",
            IDENTITY_UPDATE,
            json!({
                "name": "scout",
                "expected_spec_digest": digest,
                "set": {"display_name": "Scout"},
            }),
        )
        .await;
    assert_eq!(updated.content["outcome"], "done", "{}", updated.content);
    assert!(!updated.is_error);
    let read = rig
        .tool(
            LEAD,
            "prep-get-updated",
            IDENTITY_GET,
            json!({"name": "scout"}),
        )
        .await;
    assert_eq!(
        (
            read.content["display_name"].as_str(),
            read.content["readiness"].as_str()
        ),
        (Some("Scout"), Some("preparation_pending")),
        "an update changes the record of an agent that waits, and it still waits: {}",
        read.content
    );

    // A move of an agent into the prepared profile is refused.
    let walker = rig
        .tool(
            LEAD,
            "prep-walker",
            IDENTITY_CREATE,
            json!({"agents": [{"name": "walker", "profile": MAKER}]}),
        )
        .await;
    assert_eq!(walker.content["outcome"], "done", "{}", walker.content);
    let moved = rig
        .tool(
            LEAD,
            "prep-move",
            IDENTITY_UPDATE,
            json!({
                "name": "walker",
                "expected_spec_digest": walker.content["agents"][0]["spec_digest"],
                "set": {"profile": WORKER},
            }),
        )
        .await;
    assert_eq!(
        (
            moved.content["outcome"].as_str(),
            moved.content["kind"].as_str(),
            moved.content["field"].as_str()
        ),
        (Some("refused"), Some("unsupported"), Some("set.profile")),
        "a move into a profile the host prepares is refused: {}",
        moved.content
    );
    assert!(moved.is_error);
    assert_eq!(
        entry_of(
            &rig.tool(LEAD, "prep-list-walker", IDENTITY_LIST, json!({}))
                .await,
            "walker"
        )["profile"],
        MAKER,
        "the agent stays on its profile"
    );

    // Nothing the agent that asked did settled it: it is offered no tool
    // for that, the agent still waits, and its first message was asked of
    // no model.
    {
        let asked = rig.model.asked.lock().unwrap();
        assert!(
            asked
                .iter()
                .all(|asked| asked.tools.iter().all(|tool| !tool.contains("settle"))),
            "no agent is offered a tool that settles a preparation"
        );
        assert!(
            asked.iter().all(|asked| !asked.text.contains(KICKOFF)),
            "the first message of an agent that waits is asked of no model"
        );
    }
    assert!(waits(), "the agent still waits for its host");

    // The host settles it: the agent is seated and its first message runs.
    let settled = rig
        .rpc(
            "meta/identity/settle_preparation",
            json!({"request_id": "settle-scout", "name": "scout"}),
        )
        .await;
    assert!(
        settled["error"].is_null() && settled["result"]["outcome"] == "done",
        "the host settles the preparation: {settled:#}"
    );
    rig.turn(KICKOFF).await;
    let after = rig
        .tool(LEAD, "prep-create-after", IDENTITY_CREATE, create)
        .await;
    assert_eq!(
        (
            after.content["outcome"].as_str(),
            after.content["agents"][0]["state"].as_str(),
            after.content["agents"][0]["seat"].as_str()
        ),
        (Some("done"), Some("committed"), Some("seated")),
        "the same create answers done once the agent is settled: {}",
        after.content
    );
    let listed = rig
        .tool(LEAD, "prep-list-after", IDENTITY_LIST, json!({}))
        .await;
    let readiness = entry_of(&listed, "scout")["readiness"].clone();
    assert!(
        readiness == "live" || readiness == "dormant",
        "a settled agent reads as any agent: {}",
        listed.content
    );
    assert_eq!(
        rig.model
            .asked
            .lock()
            .unwrap()
            .iter()
            .filter(|asked| asked.text.contains(KICKOFF) && asked.ended)
            .count(),
        1,
        "the first message ran once"
    );
    rig.shutdown().await;
}

/// An operation id is the host's handle on a change. No identity tool takes
/// one and no description tells an agent to quote one, so a change's answer
/// names none to the agent that asked: a `done`, a held decision and an
/// agent that waits for its host's preparation each answer their outcome
/// without it, as the model reads the tool result and as a direct call
/// answers. A held change still names its decision and its deadline, and
/// the description says in words that a person decides. The family is
/// handed the operation as before, and the host reads it with its id.
#[tokio::test(flavor = "multi_thread")]
async fn an_agents_change_answer_names_no_operation_id() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_family(state.path(), &fresh_mob_id()).await;
    let asker = Requester::Agent {
        identity: LEAD.to_owned(),
    };
    let operation = OperationId::for_request(&asker, "r-1").unwrap();
    let create = json!({"request_id": "r-1", "agents": [{"name": "scout", "profile": WORKER}]});
    for (token, scripted) in [
        // The scripted family's own answer: `done` for the agent named.
        ("done", None),
        (
            "pending_decision",
            Some(IdentityOutcome::PendingDecision {
                operation_id: operation.clone(),
                decision_id: "decision-7".to_owned(),
                deadline_ms: 1_800_000_000_000,
            }),
        ),
        (
            "pending_preparation",
            Some(IdentityOutcome::PendingPreparation {
                operation_id: operation.clone(),
                agents: vec![AgentOutcome::new(
                    "scout",
                    AgentChangeState::PendingPreparation,
                )],
            }),
        ),
    ] {
        if let Some(outcome) = scripted {
            rig.family.answer(outcome);
        }
        rig.family.forget();
        // In a real turn, which is what the model reads; then directly.
        let read = rig
            .tool(
                LEAD,
                &format!("no-id-{token}"),
                IDENTITY_CREATE,
                create.clone(),
            )
            .await;
        let direct = call(&rig, IDENTITY_CREATE, create.clone()).await;
        assert_eq!(
            rig.family.operations(),
            vec![operation.as_str().to_owned(); 2],
            "{token}: the family is handed the operation"
        );
        assert!(
            !read.is_error && !direct.refused,
            "{token} is no error result"
        );
        assert_eq!(
            read.content, direct.content,
            "{token}: one answer however it is called"
        );
        assert_eq!(read.content["outcome"], token, "{}", read.content);
        let said = read.content.to_string();
        assert!(
            !said.contains("operation_id") && !said.contains(operation.as_str()),
            "{token}: the answer names no operation id: {said}"
        );
        if token == "pending_decision" {
            assert_eq!(
                read.content,
                json!({
                    "outcome": "pending_decision",
                    "decision_id": "decision-7",
                    "deadline_ms": 1_800_000_000_000_u64,
                }),
                "a held change names its decision and its deadline"
            );
        }
    }
    // What a held change is, the description says in words; and no tool's
    // text or arguments name an operation id.
    for def in identity_tool_defs().iter() {
        let all = format!("{} {}", def.description, def.input_schema).to_lowercase();
        assert!(
            !all.contains("operation_id") && !all.contains("operation id"),
            "{} names no operation id",
            def.name
        );
        if def.name == IDENTITY_CREATE {
            assert!(
                def.description.contains(
                    "a person must decide (`decision_id`, `deadline_ms`). Nothing is created yet."
                ),
                "identity_create says what a held change is"
            );
        }
    }
    rig.shutdown().await;

    // On the real family under a host's policy: the same, for the first
    // answer and for the same request asked again.
    let state = tempfile::tempdir().unwrap();
    let rig = boot(state.path(), &fresh_mob_id()).await;
    assert!(
        rig.runtime
            .attach_identity_policy(policy(), IdentityFamilyOptions::new()),
        "the launch attaches its identity policy"
    );
    let operation = OperationId::for_request(&asker, "real-1").unwrap();
    for mark in ["no-id-real", "no-id-real-again"] {
        let answer = rig
            .tool(
                LEAD,
                mark,
                IDENTITY_CREATE,
                json!({"request_id": "real-1", "agents": [{"name": "scout", "profile": WORKER}]}),
            )
            .await;
        assert_eq!(answer.content["outcome"], "done", "{}", answer.content);
        let said = answer.content.to_string();
        assert!(
            !said.contains("operation_id") && !said.contains(operation.as_str()),
            "{mark}: the answer names no operation id: {said}"
        );
    }
    rig.host_reads_operation(&operation, "done").await;
    rig.shutdown().await;
}

/// The approver id the notice row's policy names.
const APPROVERS: &str = "team:owners";
/// The chat person the launch's approver map lets decide for [`APPROVERS`].
const OWNER: &str = "U123";

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms")
}

/// [`boot`], on a launch whose approver map lets the chat person [`OWNER`]
/// decide for [`APPROVERS`] through a declared ingress.
async fn boot_with_approvers(state: &Path, mob_id: &str) -> Rig {
    let model = ScriptedModel::default();
    let access: meerkat_meta::AccessControlConfig = toml::from_str(
        r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "chat", subject = "U123" }]
"#,
    )
    .expect("the approver map");
    let ingresses: meerkat_meta::DecisionIngressDeclarations =
        serde_json::from_value(json!({"ingresses": [
            {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
        ]}))
        .expect("ingress declarations");
    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition(mob_id))
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(DeclaredRoster))
            .identity_runtime_instance_id("meta-identity-agent-tools")
            .default_llm_client(Arc::new(model.clone()))
            .decision_ingresses(ingresses)
            .access_controller(meerkat_meta::AccessController::new(access).expect("controller"))
            .build(),
    )
    .await
    .expect("build the gateway-shaped UnifiedRuntime");
    Rig {
        runtime,
        model,
        family: Arc::new(ScriptedFamily::default()),
    }
}

impl Rig {
    /// Approve the held decision `decision_id` as the person the approver
    /// map names: a host channel shows it to its approvers first.
    async fn approve(&self, decision_id: &str) {
        use meerkat_core::approval::decision::{
            ApproverId, DecisionId, DecisionReference, PresentationAudience, PresentationRecord,
        };
        struct NoNotices;
        #[async_trait]
        impl meerkat_meta::DecisionNoticeSink for NoNotices {
            async fn deliver(&self, _line: String) -> bool {
                true
            }
        }
        let runtime = &self.runtime;
        let owner = runtime.durable_decisions().owner().clone();
        let row = owner
            .get(&DecisionId::new(decision_id).expect("a decision id"))
            .expect("the decision is read");
        owner
            .present(
                &row.decision_id,
                &PresentationRecord::complete(
                    PresentationAudience::Approver {
                        approver_id: ApproverId::new(APPROVERS).expect("approver"),
                    },
                    &row.descriptor.rendering,
                    None,
                    DecisionReference::new("chat:owners").expect("channel"),
                    DecisionReference::new(format!("message-{}", now_ms())).expect("message"),
                    now_ms(),
                ),
            )
            .expect("present");
        let connection = meerkat_meta::StdioDecisionConnection::new(
            runtime.durable_decisions().realm(),
            Arc::new(NoNotices) as Arc<dyn meerkat_meta::DecisionNoticeSink>,
        );
        let line = json!({"jsonrpc": "2.0", "id": "decide", "method": "meta/decisions/decide",
        "params": {
            "decision_id": decision_id,
            "approve": true,
            "presented_digest": row.descriptor.subject_digest.to_string(),
            "attestation": {
                "ingress": "chat",
                "subject": OWNER,
                "scope": "chat-session-1",
                "event_ref": format!("chat:event:{decision_id}"),
                "observed_at_ms": now_ms(),
            },
        }})
        .to_string();
        let answer: Value = serde_json::from_str(
            &connection
                .handle_line(runtime, &line)
                .await
                .expect("a decision method answers"),
        )
        .expect("answer JSON");
        assert!(answer["error"].is_null(), "{answer:#}");
    }

    /// The host's read of the operation `operation` on the real family: its
    /// outcome is `outcome`, and it carries the operation's id, which is the
    /// host's handle on the change.
    async fn host_reads_operation(&self, operation: &OperationId, outcome: &str) {
        let family = self.runtime.identity_family().expect("the family");
        let read = family
            .read(
                &self.runtime.mob_handle(),
                &ChangeRequester::host("rpc"),
                IdentityRead::Operation(operation.clone()),
            )
            .await;
        let Ok(IdentityReadAnswer::Operation(OperationState::Outcome(read))) = read else {
            panic!("the host reads the operation {operation:?}");
        };
        assert_eq!(read.token(), outcome, "{read:?}");
        assert_eq!(
            read.to_json()["operation_id"],
            operation.as_str(),
            "the host's read of an operation keeps its id"
        );
    }
}

/// A person decided a change an agent asked for, and it was carried out:
/// the notice the agent reads says so with the typed outcome and names the
/// decision, and it names no operation id, neither the notice's own nor the
/// one inside the outcome it carries. On the real family, with the launch's
/// notices sent as the gateway sends them (a keyed send to the agent's own
/// name): for a create that ends `done`, and for one whose agent then waits
/// for its host's preparation. The host's read of the same operation keeps
/// the id.
#[tokio::test(flavor = "multi_thread")]
async fn the_notice_of_a_carried_out_change_names_no_operation_id_to_the_agent_that_asked() {
    let state = tempfile::tempdir().unwrap();
    let rig = boot_with_approvers(state.path(), &fresh_mob_id()).await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "peers": {"default": "allow"},
        "preparation": {WORKER: {"mode": "required"}},
        "approval": {"rules": [{
            "change": ["create"],
            "requester": {"kind": "agent", "profile": [MAKER]},
            "target": {"profile": [WORKER, MAKER]},
            "outcome": {"kind": "require_approval", "approver": {"id": APPROVERS}},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    // The route the gateway gives a launch's notices: the identity runtime,
    // asked when a notice is delivered. The row takes it back before the
    // runtime stops.
    let route = Arc::new(Mutex::new(rig.runtime.identity_runtime()));
    let asked_route = Arc::clone(&route);
    assert!(
        rig.runtime.attach_identity_policy(
            Arc::new(policy),
            IdentityFamilyOptions::new().with_decision_notices(Arc::new(KeyedSendNotices::new(
                move || asked_route.lock().unwrap().clone()
            ))),
        ),
        "the launch attaches its identity policy and its notices"
    );
    let asker = Requester::Agent {
        identity: LEAD.to_owned(),
    };
    for (name, profile, outcome) in [
        ("walker", MAKER, "done"),
        ("scout", WORKER, "pending_preparation"),
    ] {
        let request_id = format!("held-{name}");
        let operation = OperationId::for_request(&asker, &request_id).unwrap();
        let held = rig
            .tool(
                LEAD,
                &format!("notice-ask-{name}"),
                IDENTITY_CREATE,
                json!({"request_id": request_id, "agents": [{"name": name, "profile": profile}]}),
            )
            .await;
        assert_eq!(
            held.content["outcome"], "pending_decision",
            "{}",
            held.content
        );
        let decision_id = held.content["decision_id"]
            .as_str()
            .expect("a decision id")
            .to_owned();
        rig.approve(&decision_id).await;

        // The notice starts a turn of the agent that asked: its text is
        // what that agent's model reads.
        let told = rig
            .turn(&format!("The change you asked for (create of \"{name}\")"))
            .await;
        let text = told.last().expect("the notice's turn").text.clone();
        assert!(
            text.contains("was approved and carried out") && text.contains(&decision_id),
            "the notice says what was decided and names the decision: {text}"
        );
        assert!(
            text.contains(&format!("\"outcome\":\"{outcome}\"")),
            "the notice carries the typed outcome `{outcome}`: {text}"
        );
        assert!(
            !text.contains("operation_id") && !text.contains(operation.as_str()),
            "the notice an agent reads names no operation id: {text}"
        );
        rig.host_reads_operation(&operation, outcome).await;
    }
    route.lock().unwrap().take();
    rig.shutdown().await;
}
