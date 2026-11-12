#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A change of an agent's instructions reaches the agent as one appended
//! system instruction at its next run, and nothing rewrites what its
//! conversation already holds.
//!
//! An agent's instructions are part of its spec: a configured agent's come
//! from the host's roster, a store-owned agent's from its row in the mob's
//! identity store. A session's first build puts them into its first prompt,
//! and a resume authors nothing. The identity runtime therefore keeps one
//! operator instruction record per agent whose instructions changed, in the
//! platform's reserved namespace, and Meerkat's mob delivers it: recorded in
//! the agent's session without waking it, applied at the agent's next run
//! start as one appended System entry.
//!
//! Each row boots the gateway's runtime composition on a temporary state
//! directory (persistent stores, the state directory's identity substrate, a
//! roster the row can edit) with a model that records every request it is
//! sent, and reads the agent's durable transcript after each step.

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::runtime::{
    InstructionNextSessionReason, InstructionOutcome, InstructionsChanged,
};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, MutableRosterProvider,
};
use meerkat_mob::{MobDefinition, ProfileName};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// The agent the host's configuration declares.
const CONFIGURED: &str = "host-a";
/// The agent a row creates at run time, store-owned.
const CREATED: &str = "keeper";
/// The namespace of the records the identity runtime keeps.
const NAMESPACE: &str = "meerkat.identity.instructions";
/// The first line of an operator instruction entry row.
const ENTRY_HEADER: &str = "[meerkat-operator-instructions-v1]";
/// What a record that carries a text starts with.
const TEXT_PREAMBLE: &str = "This is the current, complete text of your standing instructions \
for this role. It replaces every earlier version of them in this conversation, including the \
version in your first prompt. The rest of your system prompt is unchanged.";
/// The whole body of a record that carries no text.
const WITHDRAWN_BODY: &str = "Your standing instructions for this role have been withdrawn. \
Every earlier version of them in this conversation, including the version in your first prompt, \
no longer applies. The rest of your system prompt is unchanged.";
/// A turn whose content holds this waits for the row's release.
const HOLD: &str = "HOLD-THIS-RUN";
/// A turn whose content holds this calls `fork_off` once.
const FORK_OFF: &str = "CALL-FORK-OFF";
/// The member a `fork_off` call names, and the task it hands it.
const FORK_CHILD: &str = "fork-child";
const FORK_TASK: &str = "FORK-TASK-MARKER: carry on";
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

[profiles.prefix]
model = "gemini-3.5-flash"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.prefix.tools]
comms = true

[profiles.forker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.forker.tools]
comms = true
mob = true
"#
    ))
    .expect("parse the definition")
}

fn fresh_mob_id() -> String {
    format!(
        "meta-instruction-append-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn text(sections: &[&str]) -> Vec<String> {
    sections.iter().map(ToString::to_string).collect()
}

/// The configured agent's spec with `instructions`, on `profile`.
fn spec_on(profile: &str, instructions: &[&str]) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(CONFIGURED).unwrap(),
        profile: ProfileName::from(profile),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: text(instructions),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

fn configured_spec(instructions: &[&str]) -> DurableAgentSpec {
    spec_on("worker", instructions)
}

/// The store-owned agent's spec with `instructions`.
fn created_spec(instructions: &[&str]) -> meerkat_mob::CreatedIdentitySpec {
    created_spec_on("worker", instructions)
}

/// The store-owned agent's spec with `instructions`, on `profile`.
fn created_spec_on(profile: &str, instructions: &[&str]) -> meerkat_mob::CreatedIdentitySpec {
    meerkat_mob::CreatedIdentitySpec::new(CREATED, profile)
        .with_additional_instructions(text(instructions))
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
            serde_json::json!({"realm": "meta-instruction-append", "account": "recording"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "recording-profile".into(),
        "recording-transport".into(),
    )
}

/// The members' model: each request's last message (past Core's system
/// notices) is sent on the row's channel, each whole request is kept, and
/// every request answers. A request whose last message holds [`HOLD`] waits
/// until the row releases it; one that holds [`FORK_OFF`] answers with that
/// tool call once.
#[derive(Clone)]
struct RecordingClient {
    asked: mpsc::UnboundedSender<String>,
    requests: Arc<Mutex<Vec<Value>>>,
    released: watch::Receiver<bool>,
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
            .push(serde_json::to_value(request).unwrap_or_default());
        let last = request
            .messages
            .iter()
            .rev()
            .find(|message| !matches!(message, Message::SystemNotice(_)))
            .map(|last| serde_json::to_string(last).unwrap_or_default())
            .unwrap_or_default();
        let held = last.contains(HOLD);
        // A turn asked to fork answers with that one tool call, then ends.
        let answered = matches!(request.messages.last(), Some(Message::ToolResults { .. }));
        let call = if answered {
            None
        } else if last.contains(FORK_OFF) {
            Some((
                "fork_off",
                serde_json::json!({"member_id": FORK_CHILD, "task": FORK_TASK}),
            ))
        } else {
            None
        };
        let _ = self.asked.send(last);
        let [usage, done] = llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            if call.is_some() {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
        );
        let mut released = self.released.clone();
        Box::pin(async_stream::stream! {
            if held {
                let _ = released.wait_for(|released| *released).await;
            }
            if let Some((name, args)) = call {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: format!("call-{name}"),
                    name: name.to_string(),
                    args,
                    meta: None,
                });
            } else {
                yield Ok(LlmEvent::TextDelta { delta: "noted".to_string(), meta: None });
            }
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
    roster: Arc<MutableRosterProvider>,
    asked: mpsc::UnboundedReceiver<String>,
    requests: Arc<Mutex<Vec<Value>>>,
    release: watch::Sender<bool>,
}

/// Boot the gateway's runtime composition on `state` for the mob `mob_id`
/// with `roster` as the host's configuration.
async fn boot(state: &Path, mob_id: &str, roster: Vec<DurableAgentSpec>) -> Rig {
    boot_with(state, mob_id, roster, |builder| builder).await
}

/// [`boot`] with `configure` applied to the builder.
async fn boot_with(
    state: &Path,
    mob_id: &str,
    roster: Vec<DurableAgentSpec>,
    configure: impl FnOnce(UnifiedRuntimeBuilder) -> UnifiedRuntimeBuilder,
) -> Rig {
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let (release, released) = watch::channel(false);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let roster = Arc::new(MutableRosterProvider::new(roster));
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition(mob_id))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(roster.clone())
        .identity_runtime_instance_id("meta-instruction-append")
        .default_llm_client(Arc::new(RecordingClient {
            asked: asked_tx,
            requests: Arc::clone(&requests),
            released,
        }));
    let runtime = Box::pin(configure(builder).build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    Rig {
        runtime,
        roster,
        asked,
        requests,
        release,
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

fn host() -> meerkat_mob::IdentityChangeRequesterInput {
    meerkat_mob::IdentityChangeRequesterInput::host("console")
}

/// The operator instruction entry rows among `messages`, each as its
/// content.
fn entries(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message["role"] == "system")
        .filter_map(|message| message["content"].as_str())
        .filter(|content| content.starts_with(ENTRY_HEADER))
        .map(ToString::to_string)
        .collect()
}

/// The body a record of `sections` carries.
fn body_of(sections: &[&str]) -> String {
    format!("{TEXT_PREAMBLE}\n\n{}", sections.join("\n\n"))
}

/// Whether `entry`, an entry row with one change, carries exactly `body`:
/// the row ends with the body's length, an empty line, the body and a line
/// end.
fn carries_body(entry: &str, body: &str) -> bool {
    entry.ends_with(&format!("body_bytes={}\n\n{body}\n", body.len()))
}

/// Whether `entry` carries the whole text `sections` under the preamble.
fn carries(entry: &str, sections: &[&str]) -> bool {
    carries_body(entry, &body_of(sections))
}

impl Rig {
    /// One JSON-RPC call through the dispatcher the stdio gateway serves.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: self
                .runtime
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: self.roster.clone(),
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

    /// Send `marker` to `identity` and wait until its turn was asked it and
    /// the turn's rows are in the agent's durable transcript.
    async fn turn(&mut self, identity: &str, marker: &str) {
        let sent = self
            .rpc(
                "meta/send",
                json!({"identity": identity, "content": marker}),
            )
            .await;
        assert!(sent["error"].is_null(), "the send is admitted: {sent:#?}");
        assert!(self.asked(marker).await, "the turn is asked {marker}");
        self.settled(identity, marker).await;
    }

    /// Wait until `identity`'s durable transcript holds `marker` and the
    /// assistant row that answers it.
    async fn settled(&self, identity: &str, marker: &str) {
        let deadline = tokio::time::Instant::now() + TURN_BOUND;
        loop {
            let transcript = self.transcript(identity).await;
            let asked = transcript
                .iter()
                .position(|row| row["role"] == "user" && row.to_string().contains(marker));
            if asked.is_some_and(|asked| {
                transcript[asked..]
                    .iter()
                    .any(|row| row["role"] == "block_assistant")
            }) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the turn asked {marker} is in {identity}'s transcript: {transcript:#?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The first whole model request whose last message holds `marker`.
    fn request_asked(&self, marker: &str) -> Value {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| {
                request["messages"]
                    .as_array()
                    .and_then(|messages| {
                        messages
                            .iter()
                            .rev()
                            .find(|message| message["role"] != "system_notice")
                    })
                    .is_some_and(|last| last.to_string().contains(marker))
            })
            .cloned()
            .unwrap_or_else(|| panic!("a model request asked {marker}"))
    }

    /// The messages of the model request that asked `marker`.
    fn request_messages(&self, marker: &str) -> Vec<Value> {
        self.request_asked(marker)["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn mob(&self) -> meerkat_mob::MobHandle {
        self.runtime.mob_handle()
    }

    fn member(name: &str) -> meerkat_mob::AgentIdentity {
        meerkat_meta::member_comms_id::mob_member_id(name)
    }

    /// Whether Core holds `name`'s member without a loaded session.
    fn dormant(&self, name: &str) -> bool {
        self.mob().member_unmaterialized(&Self::member(name))
    }

    /// The rows of `name`'s durable transcript, as its store holds them.
    async fn transcript(&self, name: &str) -> Vec<Value> {
        let session_id = self
            .mob()
            .resolve_bridge_session_id(&Self::member(name))
            .await
            .expect("the member has a session binding");
        let session = self
            .runtime
            .mob_runtime()
            .session_service()
            .expect("the mob's session service")
            .load_persisted_session_tolerating_tail(&session_id)
            .await
            .expect("the session reads")
            .expect("the session is stored");
        session
            .messages()
            .iter()
            .map(|message| serde_json::to_value(message).expect("a transcript row"))
            .collect()
    }

    /// `meta/instruction_status` for `name`'s slot and member.
    async fn status(&self, name: &str) -> Value {
        let status = self
            .rpc(
                "meta/instruction_status",
                json!({"namespace": NAMESPACE, "key": name, "identity": name}),
            )
            .await;
        assert!(status["error"].is_null(), "the status reads: {status:#?}");
        status["result"].clone()
    }

    /// The mob's record of `name`'s instructions, if it holds one.
    async fn record(&self, name: &str) -> Option<Value> {
        self.status(name).await["desired"]
            .as_array()
            .and_then(|records| records.first().cloned())
    }

    /// Wait until `name`'s session recorded the mob's current snapshot.
    async fn recorded(&self, name: &str) {
        let deadline = tokio::time::Instant::now() + TURN_BOUND;
        loop {
            let status = self.status(name).await;
            let delivery = status["members"][0]["delivery"].to_string();
            if delivery.contains("\"current\"") {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name}'s session records its snapshot: {status:#?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Edit the configured agent's instructions in the host's configuration
    /// and run the roster pass a host's refresh runs.
    async fn configure(&self, instructions: &[&str]) {
        self.roster.upsert(configured_spec(instructions));
        self.roster_pass().await;
    }

    async fn roster_pass(&self) {
        self.runtime
            .identity_first_context()
            .expect("identity-first context installed")
            .refresh_desired_topology()
            .await
            .expect("the roster pass runs");
    }

    /// `meta/reset` of `name`: its next life starts on a fresh conversation.
    async fn reset(&self, name: &str) {
        let reset = self.rpc("meta/reset", json!({"identity": name})).await;
        assert!(
            reset["error"].is_null(),
            "the reset is carried out: {reset:#?}"
        );
    }

    /// Create the store-owned agent with `instructions` through Core.
    async fn create(&self, instructions: &[&str]) {
        self.create_spec(created_spec(instructions)).await;
    }

    /// Create the store-owned agent from `spec` through Core.
    async fn create_spec(&self, spec: meerkat_mob::CreatedIdentitySpec) {
        let created = self
            .mob()
            .create_store_owned_identities(
                meerkat_mob::StoreOwnedIdentityCreate::new(
                    operation(&format!("create-{CREATED}")),
                    host(),
                    vec![meerkat_mob::CreatedIdentityRequest::new(spec)],
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
            "Core creates {CREATED}: {created:?}"
        );
    }

    /// Change the store-owned agent's row from `previous` to `current`
    /// through Core's update, as operation `name`.
    async fn update(&self, name: &str, previous: &[&str], current: &[&str]) {
        self.update_spec(name, created_spec(previous), created_spec(current))
            .await;
    }

    /// Change the store-owned agent's row from `previous` to `current`
    /// through Core's update, as operation `name`.
    async fn update_spec(
        &self,
        name: &str,
        previous: meerkat_mob::CreatedIdentitySpec,
        current: meerkat_mob::CreatedIdentitySpec,
    ) {
        let digest = previous.spec_digest().expect("the row's digest");
        let updated = self
            .mob()
            .update_store_owned_identity(
                meerkat_mob::StoreOwnedIdentityUpdate::new(
                    operation(name),
                    host(),
                    current,
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
            "Core updates {CREATED}: {updated:?}"
        );
    }

    /// What the update of the store-owned agent's row from `previous` to
    /// `current` calls after its commit.
    async fn after_update(&self, previous: &[&str], current: &[&str]) -> InstructionOutcome {
        self.runtime
            .identity_runtime()
            .expect("identity runtime installed")
            .after_update(
                &self.mob(),
                &InstructionsChanged {
                    identity: CREATED.to_string(),
                    previous: text(previous),
                    current: text(current),
                },
            )
            .await
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

/// A configured agent, live and idle: its configuration's instructions
/// change, and its next run's request carries one appended entry with the
/// whole new text. The first prompt and every earlier row are the same
/// bytes as before.
///
/// RED before the identity runtime recorded changed instructions: the next
/// request carried no entry, and the agent kept the text of its first
/// prompt until a fresh session.
#[tokio::test(flavor = "multi_thread")]
async fn a_configured_agents_changed_instructions_are_appended_at_its_next_run() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    let before = rig.transcript(CONFIGURED).await;
    assert!(
        before[0]["content"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("Answer in ALPHA style.")),
        "the first prompt holds the text the session was built with"
    );
    assert!(entries(&before).is_empty(), "no entry before a change");

    rig.configure(&["Answer in BETA style."]).await;
    rig.recorded(CONFIGURED).await;
    assert!(
        !rig.dormant(CONFIGURED),
        "the agent stays loaded: nothing was restarted"
    );
    rig.turn(CONFIGURED, "SECOND-TURN").await;

    let request = entries(&rig.request_messages("SECOND-TURN"));
    assert_eq!(
        request.len(),
        1,
        "one entry reaches the model: {request:#?}"
    );
    assert!(
        carries(&request[0], &["Answer in BETA style."]),
        "the entry carries the whole current text: {}",
        request[0]
    );
    let after = rig.transcript(CONFIGURED).await;
    assert_eq!(
        after[..before.len()],
        before[..],
        "every earlier row is byte-identical"
    );
    assert_eq!(entries(&after).len(), 1, "one entry row was appended");
    rig.shutdown().await;
}

/// A store-owned agent, live and idle: Core updates its row's instructions,
/// the update's after-commit step answers `applies_at_next_run`, and its
/// next run's request carries the entry. Earlier rows are untouched.
///
/// RED before the after-commit step recorded the change: it answered
/// `unchanged`, and the next request carried no entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_owned_agents_updated_instructions_are_appended_at_its_next_run() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;
    let before = rig.transcript(CREATED).await;

    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    assert_eq!(
        rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
            .await,
        InstructionOutcome::AppliesAtNextRun
    );
    rig.recorded(CREATED).await;
    rig.turn(CREATED, "SECOND-TURN").await;

    let request = entries(&rig.request_messages("SECOND-TURN"));
    assert_eq!(
        request.len(),
        1,
        "one entry reaches the model: {request:#?}"
    );
    assert!(
        carries(&request[0], &["Keep BETA records."]),
        "the entry carries the whole current text: {}",
        request[0]
    );
    let after = rig.transcript(CREATED).await;
    assert_eq!(
        after[..before.len()],
        before[..],
        "every earlier row is byte-identical"
    );

    rig.turn(CONFIGURED, "OTHER-AGENT-TURN").await;
    assert!(
        entries(&rig.request_messages("OTHER-AGENT-TURN")).is_empty(),
        "the record targets the changed agent alone: another agent reads no entry"
    );
    rig.shutdown().await;
}

/// An agent whose session no build created yet needs no record: the change
/// answers `applies_at_next_run`, nothing is recorded, and its first build
/// puts the current text into its first prompt.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_never_built_takes_the_current_text_at_its_first_build() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    assert_eq!(
        rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
            .await,
        InstructionOutcome::AppliesAtNextRun
    );
    assert_eq!(
        rig.record(CREATED).await,
        None,
        "no record for an agent whose session no build created"
    );

    rig.turn(CREATED, "FIRST-TURN").await;
    let transcript = rig.transcript(CREATED).await;
    assert!(
        transcript[0]["content"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("Keep BETA records.")
                && !prompt.contains("Keep ALPHA records.")),
        "the first prompt is built from the current text"
    );
    assert!(entries(&transcript).is_empty(), "and no entry is appended");
    rig.shutdown().await;
}

/// A change of a store-owned agent's row that no update step reported: the
/// next door that reads the row again finds its instructions changed and
/// records the change, so the run after it carries the entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_door_that_reads_a_changed_row_records_the_change() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;

    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    assert_eq!(
        rig.record(CREATED).await,
        None,
        "nothing reported the change"
    );
    rig.turn(CREATED, "SECOND-TURN").await;
    let record = rig.record(CREATED).await;
    assert_eq!(
        record.as_ref().map(|record| record["revision"].clone()),
        Some(json!(1)),
        "the door that read the changed row recorded the change: {record:#?}"
    );
    rig.recorded(CREATED).await;
    rig.turn(CREATED, "THIRD-TURN").await;
    let request = entries(&rig.request_messages("THIRD-TURN"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(carries(&request[0], &["Keep BETA records."]));
    rig.shutdown().await;
}

/// The same change twice writes one record revision and appends one entry:
/// a repeat of the after-commit step, and a second door that reads the
/// changed row, find the record current.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_change_twice_is_recorded_and_appended_once() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;

    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    assert_eq!(
        rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
            .await,
        InstructionOutcome::AppliesAtNextRun
    );
    assert_eq!(
        rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
            .await,
        InstructionOutcome::AppliesAtNextRun,
        "a replay of the step finds the record current and answers as the step did"
    );
    let record = rig.record(CREATED).await.expect("the agent's record");
    assert_eq!(record["revision"], json!(1), "one revision: {record:#?}");

    rig.recorded(CREATED).await;
    rig.turn(CREATED, "SECOND-TURN").await;
    rig.turn(CREATED, "THIRD-TURN").await;
    assert_eq!(
        rig.record(CREATED).await.expect("the agent's record")["revision"],
        json!(1),
        "the doors that read the changed row wrote nothing more"
    );
    assert_eq!(
        entries(&rig.transcript(CREATED).await).len(),
        1,
        "one entry row in the transcript"
    );
    rig.shutdown().await;
}

/// The first change supersedes nothing; a second change is the record's
/// next revision, and its entry supersedes the first entry's revision. Both
/// entry rows stay in the transcript.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_change_supersedes_the_first_and_the_first_supersedes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;

    rig.configure(&["Answer in BETA style."]).await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-TURN").await;
    rig.configure(&["Answer in GAMMA style."]).await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "THIRD-TURN").await;

    let record = rig.record(CONFIGURED).await.expect("the agent's record");
    assert_eq!(record["revision"], json!(2), "{record:#?}");
    let rows = entries(&rig.transcript(CONFIGURED).await);
    assert_eq!(rows.len(), 2, "both entries stay: {rows:#?}");
    assert!(
        rows[0].contains("revision=1\nsupersedes=null\n")
            && carries(&rows[0], &["Answer in BETA style."]),
        "the first entry supersedes nothing: {}",
        rows[0]
    );
    assert!(
        rows[1].contains("revision=2\nsupersedes=1\n")
            && carries(&rows[1], &["Answer in GAMMA style."]),
        "the second entry supersedes the first: {}",
        rows[1]
    );
    rig.shutdown().await;
}

/// An agent that is not loaded: a change of its instructions wakes nothing
/// and builds nothing. The mob records the change in its stored session,
/// and its next run applies it.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_wakes_no_agent_and_its_next_run_applies_it() {
    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let roster = vec![configured_spec(&["Answer in ALPHA style."])];
    let mut rig = boot(temp.path(), &mob, roster.clone()).await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.shutdown_for_reopen().await;

    let mut rig = boot(temp.path(), &mob, roster).await;
    assert!(rig.dormant(CONFIGURED), "the restart loads no agent");
    rig.configure(&["Answer in BETA style."]).await;
    rig.recorded(CONFIGURED).await;
    assert!(
        rig.dormant(CONFIGURED),
        "the change and its record woke nothing"
    );
    assert!(
        rig.requests.lock().unwrap().is_empty(),
        "no session was built and no model was asked"
    );

    rig.turn(CONFIGURED, "SECOND-TURN").await;
    let request = entries(&rig.request_messages("SECOND-TURN"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(carries(&request[0], &["Answer in BETA style."]));
    rig.shutdown().await;
}

/// A run in flight is not touched: a change made while it runs appends
/// nothing to it, and the next run start carries the entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_in_flight_is_untouched_and_the_next_run_start_carries_the_entry() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;

    let held = format!("{HOLD} the held run");
    let sent = rig
        .rpc(
            "meta/send",
            json!({"identity": CONFIGURED, "content": held}),
        )
        .await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(rig.asked(HOLD).await, "the held run is in flight");
    rig.configure(&["Answer in BETA style."]).await;
    rig.release.send_replace(true);
    rig.settled(CONFIGURED, HOLD).await;
    assert!(
        entries(&rig.request_messages(HOLD)).is_empty(),
        "the run in flight read no entry"
    );

    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "NEXT-TURN").await;
    assert_eq!(
        entries(&rig.request_messages("NEXT-TURN")).len(),
        1,
        "the next run start carries the entry"
    );
    let transcript = rig.transcript(CONFIGURED).await;
    let position = |needle: &str| {
        transcript
            .iter()
            .position(|row| row.to_string().contains(needle))
            .unwrap_or_else(|| panic!("{needle} is in the transcript"))
    };
    let (held_turn, entry, next_turn) = (
        position(HOLD),
        position(ENTRY_HEADER),
        position("NEXT-TURN"),
    );
    assert!(
        held_turn < entry && entry < next_turn,
        "the entry sits after the held run and before the next run's input: held {held_turn}, \
         entry {entry}, next {next_turn}"
    );
    assert_eq!(
        transcript[held_turn + 1]["role"],
        "block_assistant",
        "nothing was appended inside the held run"
    );
    rig.shutdown().await;
}

/// A delivery submitted right after a change, with no wait for the mob's
/// delivery: the row records whether that run carried the entry (it may run
/// on the earlier instructions), and pins that the run after it does.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_submitted_right_after_a_change_may_precede_its_entry() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;

    rig.configure(&["Answer in BETA style."]).await;
    rig.turn(CONFIGURED, "RACING-TURN").await;
    let racing = entries(&rig.request_messages("RACING-TURN")).len();
    eprintln!(
        "MEASURED: the run submitted right after the change carried {racing} entr{}",
        if racing == 1 { "y" } else { "ies" }
    );
    assert!(racing <= 1, "never more than one entry");

    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "LATER-TURN").await;
    assert_eq!(
        entries(&rig.request_messages("LATER-TURN")).len(),
        1,
        "the entry is in the conversation by the run after it"
    );
    assert_eq!(entries(&rig.transcript(CONFIGURED).await).len(), 1);
    rig.shutdown().await;
}

/// A restart with no change writes nothing: an agent the mob holds no
/// record of gets none (its build text is not appended again), and an agent
/// with a record keeps its revision and its one entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_with_no_change_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let alpha = vec![configured_spec(&["Answer in ALPHA style."])];
    let mut rig = boot(temp.path(), &mob, alpha.clone()).await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.shutdown_for_reopen().await;

    let mut rig = boot(temp.path(), &mob, alpha).await;
    assert_eq!(
        rig.record(CONFIGURED).await,
        None,
        "no record of an agent whose instructions never changed"
    );
    rig.turn(CONFIGURED, "SECOND-TURN").await;
    assert!(
        entries(&rig.transcript(CONFIGURED).await).is_empty(),
        "the build text is not appended after a restart"
    );

    rig.configure(&["Answer in BETA style."]).await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "THIRD-TURN").await;
    rig.shutdown_for_reopen().await;

    let beta = vec![configured_spec(&["Answer in BETA style."])];
    let mut rig = boot(temp.path(), &mob, beta).await;
    assert_eq!(
        rig.record(CONFIGURED).await.expect("the record survives")["revision"],
        json!(1),
        "the restart wrote no new revision"
    );
    rig.turn(CONFIGURED, "FOURTH-TURN").await;
    assert_eq!(
        entries(&rig.transcript(CONFIGURED).await).len(),
        1,
        "the entry is not appended again"
    );
    rig.shutdown().await;
}

/// A text that changed while the gateway was down, for an agent the mob
/// holds a record of: the start writes the record's next revision,
/// wakes nothing, and the agent's next run carries the text.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_made_while_the_gateway_was_down_is_recorded_at_start() {
    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let mut rig = boot(
        temp.path(),
        &mob,
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.configure(&["Answer in BETA style."]).await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-TURN").await;
    rig.shutdown_for_reopen().await;

    let gamma = vec![configured_spec(&["Answer in GAMMA style."])];
    let mut rig = boot(temp.path(), &mob, gamma).await;
    let record = rig.record(CONFIGURED).await.expect("the agent's record");
    assert_eq!(
        record["revision"],
        json!(2),
        "the start wrote the changed text: {record:#?}"
    );
    assert!(rig.dormant(CONFIGURED), "the start woke nothing");
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "THIRD-TURN").await;
    let request = entries(&rig.request_messages("THIRD-TURN"));
    assert!(
        request
            .last()
            .is_some_and(|entry| carries(entry, &["Answer in GAMMA style."])),
        "the next run carries the changed text: {request:#?}"
    );
    rig.shutdown().await;
}

/// A text that changed while the gateway was down, for an agent the mob
/// holds no record of yet: at the agent's first registration after the
/// start the text is compared with the sections its session was built
/// with, the change is recorded, and nothing is woken. The session's record
/// of its build sections holds through a restart and a resumed run, so the
/// comparison is the same after them.
///
/// RED before a session's recorded build sections could be read (the
/// session service's `recorded_build_instructions`, and a resumed build
/// that keeps them): nothing said such a session was built with another
/// text, and nothing was written.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_change_made_while_the_gateway_was_down_is_recorded_at_start() {
    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let alpha = vec![configured_spec(&["Answer in ALPHA style."])];
    let mut rig = boot(temp.path(), &mob, alpha.clone()).await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.shutdown_for_reopen().await;

    let mut rig = boot(temp.path(), &mob, alpha).await;
    rig.turn(CONFIGURED, "RESUMED-TURN").await;
    assert_eq!(
        rig.record(CONFIGURED).await,
        None,
        "no change, no record, after a restart and a resumed run"
    );
    rig.shutdown_for_reopen().await;

    let beta = vec![configured_spec(&["Answer in BETA style."])];
    let mut rig = boot(temp.path(), &mob, beta).await;
    let record = rig.record(CONFIGURED).await;
    assert_eq!(
        record.as_ref().map(|record| record["revision"].clone()),
        Some(json!(1)),
        "the start recorded the first change: {record:#?}"
    );
    assert!(rig.dormant(CONFIGURED), "the start woke nothing");
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-TURN").await;
    let request = entries(&rig.request_messages("SECOND-TURN"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(carries(&request[0], &["Answer in BETA style."]));
    rig.shutdown().await;
}

/// An instruction change reaches a store-owned agent's next run exactly as
/// it reaches a configured agent's: the same text for both gives entries
/// that differ only in the agent's name, with the same body and digest.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_owned_agent_gets_the_same_entry_as_a_configured_one() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.create(&["Answer in ALPHA style."]).await;
    rig.turn(CONFIGURED, "CONFIGURED-FIRST").await;
    rig.turn(CREATED, "CREATED-FIRST").await;

    rig.configure(&["Answer in BETA style."]).await;
    rig.update(
        "update-one",
        &["Answer in ALPHA style."],
        &["Answer in BETA style."],
    )
    .await;
    assert_eq!(
        rig.after_update(&["Answer in ALPHA style."], &["Answer in BETA style."])
            .await,
        InstructionOutcome::AppliesAtNextRun
    );
    rig.recorded(CONFIGURED).await;
    rig.recorded(CREATED).await;
    rig.turn(CONFIGURED, "CONFIGURED-SECOND").await;
    rig.turn(CREATED, "CREATED-SECOND").await;

    let configured = entries(&rig.request_messages("CONFIGURED-SECOND"));
    let created = entries(&rig.request_messages("CREATED-SECOND"));
    assert_eq!((configured.len(), created.len()), (1, 1));
    assert_eq!(
        configured[0].replace(CONFIGURED, "AGENT"),
        created[0].replace(CREATED, "AGENT"),
        "the two entries differ in the agent's name alone"
    );
    let (configured, created) = (
        rig.record(CONFIGURED).await.expect("the configured record"),
        rig.record(CREATED).await.expect("the store-owned record"),
    );
    assert_eq!(
        (
            configured["revision"].clone(),
            configured["content_sha256"].clone()
        ),
        (
            created["revision"].clone(),
            created["content_sha256"].clone()
        ),
        "one revision each, of the same body"
    );
    rig.shutdown().await;
}

/// The removal of an agent revokes its record, once; an archive keeps it,
/// and the agent that comes back from its archive still runs on its changed
/// instructions with nothing written again.
#[tokio::test(flavor = "multi_thread")]
async fn a_removal_revokes_the_record_and_an_archive_keeps_it() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;
    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
        .await;
    rig.recorded(CREATED).await;
    rig.turn(CREATED, "SECOND-TURN").await;

    let archived = rig
        .mob()
        .archive_identity(
            meerkat_mob::IdentityArchiveChange::new(operation("archive"), host(), CREATED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            archived,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{archived:?}"
    );
    let record = rig.record(CREATED).await.expect("the record stays");
    assert_eq!(
        (record["revision"].clone(), record["effect"].clone()),
        (json!(1), json!("active")),
        "an archive leaves the record as it is: {record:#?}"
    );
    let unarchived = rig
        .mob()
        .unarchive_identity(
            meerkat_mob::IdentityArchiveChange::new(operation("unarchive"), host(), CREATED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            unarchived,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{unarchived:?}"
    );
    rig.turn(CREATED, "THIRD-TURN").await;
    let request = entries(&rig.request_messages("THIRD-TURN"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(
        carries(&request[0], &["Keep BETA records."]),
        "the agent still runs on its changed instructions"
    );
    assert_eq!(
        rig.record(CREATED).await.expect("the record")["revision"],
        json!(1),
        "nothing was written again"
    );

    let removed = rig
        .mob()
        .remove_identity(
            meerkat_mob::IdentityRemoval::new(operation("remove"), host(), CREATED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            removed,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{removed:?}"
    );
    let identities = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed");
    identities.after_removal(&rig.mob(), CREATED).await;
    let record = rig.record(CREATED).await.expect("the revoked record");
    assert_eq!(
        (record["revision"].clone(), record["effect"].clone()),
        (json!(2), json!("absent")),
        "the removal revoked the record: {record:#?}"
    );
    identities.after_removal(&rig.mob(), CREATED).await;
    assert_eq!(
        rig.record(CREATED).await.expect("the revoked record")["revision"],
        json!(2),
        "a replay of the removal's step revokes nothing again"
    );
    rig.shutdown().await;
}

/// A removal that committed in Core while the step after it never ran (the
/// process stopped in between, and a replayed removal is answered from
/// Core's receipt without repeating the step): the next start revokes the
/// removed agent's record, once, from what the mob holds. A second start
/// writes nothing, and an archived agent's record stays as it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_revokes_the_record_of_an_agent_removed_without_its_step() {
    const REMOVED: &str = "removed-one";
    const ARCHIVED: &str = "archived-one";
    let spec = |name: &str, instructions: &str| {
        meerkat_mob::CreatedIdentitySpec::new(name, "worker")
            .with_additional_instructions(vec![instructions.to_string()])
    };
    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let mut rig = boot(temp.path(), &mob, vec![configured_spec(&[])]).await;
    let created = rig
        .mob()
        .create_store_owned_identities(
            meerkat_mob::StoreOwnedIdentityCreate::new(
                operation("create-two"),
                host(),
                [REMOVED, ARCHIVED]
                    .iter()
                    .map(|name| {
                        meerkat_mob::CreatedIdentityRequest::new(spec(name, "Keep ALPHA records."))
                    })
                    .collect(),
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
        "Core creates the agents: {created:?}"
    );
    let identities = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed");
    for name in [REMOVED, ARCHIVED] {
        rig.turn(name, &format!("FIRST-TURN-OF-{name}")).await;
        rig.update_spec(
            &format!("update-{name}"),
            spec(name, "Keep ALPHA records."),
            spec(name, "Keep BETA records."),
        )
        .await;
        assert_eq!(
            identities
                .after_update(
                    &rig.mob(),
                    &InstructionsChanged {
                        identity: name.to_string(),
                        previous: text(&["Keep ALPHA records."]),
                        current: text(&["Keep BETA records."]),
                    },
                )
                .await,
            InstructionOutcome::AppliesAtNextRun,
            "{name}'s change is recorded"
        );
        rig.recorded(name).await;
    }
    drop(identities);

    // The removal commits in Core; the step after it does not run.
    let removed = rig
        .mob()
        .remove_identity(
            meerkat_mob::IdentityRemoval::new(operation("remove"), host(), REMOVED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            removed,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{removed:?}"
    );
    let archived = rig
        .mob()
        .archive_identity(
            meerkat_mob::IdentityArchiveChange::new(operation("archive"), host(), ARCHIVED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            archived,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{archived:?}"
    );
    let record = rig.record(REMOVED).await.expect("the record");
    assert_eq!(
        (record["revision"].clone(), record["effect"].clone()),
        (json!(1), json!("active")),
        "the removal's step did not run: {record:#?}"
    );
    rig.shutdown_for_reopen().await;

    let rig = boot(temp.path(), &mob, vec![configured_spec(&[])]).await;
    let record = rig
        .record(REMOVED)
        .await
        .expect("the removed agent's record");
    assert_eq!(
        (record["revision"].clone(), record["effect"].clone()),
        (json!(2), json!("absent")),
        "the start revoked the removed agent's record: {record:#?}"
    );
    let record = rig
        .record(ARCHIVED)
        .await
        .expect("the archived agent's record");
    assert_eq!(
        (record["revision"].clone(), record["effect"].clone()),
        (json!(1), json!("active")),
        "an archived agent keeps its record: {record:#?}"
    );
    rig.shutdown_for_reopen().await;

    let rig = boot(temp.path(), &mob, vec![configured_spec(&[])]).await;
    assert_eq!(
        rig.record(REMOVED).await.expect("the revoked record")["revision"],
        json!(2),
        "a second start revokes nothing again"
    );
    let record = rig
        .record(ARCHIVED)
        .await
        .expect("the archived agent's record");
    assert_eq!(
        (record["revision"].clone(), record["effect"].clone()),
        (json!(1), json!("active")),
        "an archived agent keeps its record: {record:#?}"
    );
    rig.shutdown().await;
}

/// A reset right after an edit of the configuration that no roster pass
/// read: the fresh conversation is built from the edited text, and nothing
/// presents the text the record held before the edit as the current one.
///
/// RED while a reset kept the record: the fresh conversation was born with
/// the record's earlier text appended as "the current, complete text",
/// after a first prompt built from the edited text.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_after_an_unread_edit_never_presents_the_earlier_text_as_current() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.configure(&["Answer in BETA style."]).await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-TURN").await;

    // The host edits its configuration and resets before any roster pass.
    rig.roster
        .upsert(configured_spec(&["Answer in GAMMA style."]));
    rig.reset(CONFIGURED).await;
    rig.turn(CONFIGURED, "AFTER-RESET").await;
    let prompt = rig.transcript(CONFIGURED).await[0]["content"]
        .as_str()
        .expect("the first prompt")
        .to_string();
    assert!(
        prompt.contains("Answer in GAMMA style.") && !prompt.contains("Answer in BETA style."),
        "the fresh conversation's first prompt is built from the edited text"
    );
    let request = entries(&rig.request_messages("AFTER-RESET"));
    assert!(
        request
            .iter()
            .all(|entry| !carries(entry, &["Answer in BETA style."])),
        "nothing presents the earlier text as the current one: {request:#?}"
    );
    rig.shutdown().await;
}

/// A host that replaces a registered agent's spec through the runtime
/// (`IdentityRuntime::update_spec`) with other instructions: the change is
/// recorded like one a roster pass reads, and the agent's next run carries
/// it.
///
/// RED while the runtime's own spec update replaced the registered spec
/// without the comparison: nothing was recorded while the process ran.
#[tokio::test(flavor = "multi_thread")]
async fn a_spec_update_through_the_runtime_records_the_changed_instructions() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;

    rig.roster
        .upsert(configured_spec(&["Answer in BETA style."]));
    let updated = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed")
        .update_spec(configured_spec(&["Answer in BETA style."]))
        .await;
    assert!(updated.is_ok(), "the runtime takes the spec: {updated:?}");
    let record = rig.record(CONFIGURED).await;
    assert_eq!(
        record.as_ref().map(|record| record["revision"].clone()),
        Some(json!(1)),
        "the spec update recorded the change: {record:#?}"
    );
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-TURN").await;
    let request = entries(&rig.request_messages("SECOND-TURN"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(carries(&request[0], &["Answer in BETA style."]));
    rig.shutdown().await;
}

/// A reset of an agent whose instructions changed revokes its record: the
/// fresh conversation's first prompt is the current, complete text, and
/// nothing is appended to it. A later change is the record's next revision
/// and reaches the fresh conversation as one entry. The same for a
/// configured agent and for one created at run time.
///
/// RED while a reset kept the record: the fresh conversation held the text
/// twice, in its first prompt and as an appended entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_of_an_edited_agent_revokes_its_record_and_appends_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.create(&["Answer in ALPHA style."]).await;
    rig.turn(CONFIGURED, "CONFIGURED-FIRST").await;
    rig.turn(CREATED, "CREATED-FIRST").await;
    rig.configure(&["Answer in BETA style."]).await;
    rig.update(
        "update-one",
        &["Answer in ALPHA style."],
        &["Answer in BETA style."],
    )
    .await;
    rig.after_update(&["Answer in ALPHA style."], &["Answer in BETA style."])
        .await;
    for name in [CONFIGURED, CREATED] {
        rig.recorded(name).await;
        rig.turn(name, &format!("{name}-SECOND")).await;
        assert_eq!(
            entries(&rig.transcript(name).await).len(),
            1,
            "{name} ran on its changed instructions"
        );

        rig.reset(name).await;
        let record = rig.record(name).await.expect("the revoked record");
        assert_eq!(
            (record["revision"].clone(), record["effect"].clone()),
            (json!(2), json!("absent")),
            "{name}'s reset revoked its record: {record:#?}"
        );
        rig.turn(name, &format!("{name}-AFTER-RESET")).await;
        let transcript = rig.transcript(name).await;
        assert!(
            transcript[0]["content"]
                .as_str()
                .is_some_and(|prompt| prompt.contains("Answer in BETA style.")
                    && !prompt.contains("Answer in ALPHA style.")),
            "{name}'s fresh conversation is built from the current text"
        );
        assert!(
            entries(&transcript).is_empty(),
            "nothing is appended to {name}'s fresh conversation: {transcript:#?}"
        );
    }

    rig.configure(&["Answer in GAMMA style."]).await;
    let record = rig.record(CONFIGURED).await.expect("the record");
    assert_eq!(
        (record["revision"].clone(), record["effect"].clone()),
        (json!(3), json!("active")),
        "a later change is the record's next revision: {record:#?}"
    );
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "CONFIGURED-THIRD").await;
    let request = entries(&rig.request_messages("CONFIGURED-THIRD"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(carries(&request[0], &["Answer in GAMMA style."]));
    rig.shutdown().await;
}

/// An agent the host's configuration drops and lists again with other
/// instructions, in later roster passes of the same process: the listing
/// records the listed text against the conversation the agent comes back
/// on, and the agent runs on it. The comparison is not tied to the
/// process's first roster pass.
///
/// The agent's first run after the listing loads its conversation again,
/// and the mob delivers the record to it then: that run carries the entry
/// when the delivery lands before it starts, and the run after it carries
/// it in any case.
///
/// RED while only a replaced registration and the first pass were compared:
/// the agent came back on its conversation with the dropped text.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_listed_again_with_other_instructions_runs_on_them() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;

    rig.roster
        .remove(&AgentIdentity::parse(CONFIGURED).expect("an identity"));
    rig.roster_pass().await;
    rig.configure(&["Answer in GAMMA style."]).await;
    let record = rig.record(CONFIGURED).await;
    assert_eq!(
        record
            .as_ref()
            .map(|record| (record["revision"].clone(), record["effect"].clone())),
        Some((json!(1), json!("active"))),
        "the listing recorded the listed text: {record:#?}"
    );

    rig.turn(CONFIGURED, "FIRST-AFTER-RELISTING").await;
    assert!(
        entries(&rig.request_messages("FIRST-AFTER-RELISTING")).len() <= 1,
        "the first run carries the entry or none"
    );
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-AFTER-RELISTING").await;
    let request = entries(&rig.request_messages("SECOND-AFTER-RELISTING"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(
        carries(&request[0], &["Answer in GAMMA style."]),
        "the relisted agent runs on the listed text: {}",
        request[0]
    );
    rig.shutdown().await;
}

/// An agent listed again with the text its conversation was built with,
/// after a change of it was recorded in this process: the listing is
/// compared with the mob's record as it is now, not with what the process
/// read at its start, so the listed text is recorded as the record's next
/// revision and the agent runs on it.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_listed_again_with_its_first_text_gets_it_recorded() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.configure(&["Answer in BETA style."]).await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-TURN").await;

    rig.roster
        .remove(&AgentIdentity::parse(CONFIGURED).expect("an identity"));
    rig.roster_pass().await;
    rig.configure(&["Answer in ALPHA style."]).await;
    let record = rig.record(CONFIGURED).await.expect("the agent's record");
    assert_eq!(
        (record["revision"].clone(), record["effect"].clone()),
        (json!(2), json!("active")),
        "the listing recorded the listed text: {record:#?}"
    );
    rig.turn(CONFIGURED, "FIRST-AFTER-RELISTING").await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-AFTER-RELISTING").await;
    let request = entries(&rig.request_messages("SECOND-AFTER-RELISTING"));
    assert!(
        request
            .last()
            .is_some_and(|entry| carries(entry, &["Answer in ALPHA style."])),
        "the relisted agent runs on the listed text: {request:#?}"
    );
    rig.shutdown().await;
}

/// An edit of an archived agent, which has a conversation and no seated
/// member: the change is recorded, nothing is woken, and the agent runs on
/// it after its unarchive.
///
/// The agent's first run after the unarchive loads its conversation again,
/// and the mob delivers the record to it then: that run carries the entry
/// when the delivery lands before it starts, and the run after it carries
/// it in any case.
///
/// RED while an agent with no seated member was read as never built: the
/// step answered `applies_at_next_run`, wrote nothing, and the unarchived
/// agent ran on its earlier text.
#[tokio::test(flavor = "multi_thread")]
async fn an_edit_of_an_archived_agent_reaches_it_after_its_unarchive() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;
    let archived = rig
        .mob()
        .archive_identity(
            meerkat_mob::IdentityArchiveChange::new(operation("archive"), host(), CREATED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            archived,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{archived:?}"
    );

    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    assert_eq!(
        rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
            .await,
        InstructionOutcome::AppliesAtNextRun
    );
    let record = rig.record(CREATED).await;
    assert_eq!(
        record
            .as_ref()
            .map(|record| (record["revision"].clone(), record["effect"].clone())),
        Some((json!(1), json!("active"))),
        "the change of the archived agent is recorded: {record:#?}"
    );

    let unarchived = rig
        .mob()
        .unarchive_identity(
            meerkat_mob::IdentityArchiveChange::new(operation("unarchive"), host(), CREATED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            unarchived,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{unarchived:?}"
    );
    rig.turn(CREATED, "FIRST-AFTER-UNARCHIVE").await;
    assert!(
        entries(&rig.request_messages("FIRST-AFTER-UNARCHIVE")).len() <= 1,
        "the first run carries the entry or none"
    );
    rig.recorded(CREATED).await;
    rig.turn(CREATED, "SECOND-AFTER-UNARCHIVE").await;
    let request = entries(&rig.request_messages("SECOND-AFTER-UNARCHIVE"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(
        carries(&request[0], &["Keep BETA records."]),
        "the unarchived agent runs on the change: {}",
        request[0]
    );
    rig.shutdown().await;
}

/// A change of an archived agent's row that nothing reported before the
/// gateway stopped: the next start compares the agent against the
/// conversation its archive kept, records the change, and the agent runs on
/// it after its unarchive.
#[tokio::test(flavor = "multi_thread")]
async fn an_archived_agents_change_made_before_a_stop_is_recorded_at_start() {
    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let mut rig = boot(temp.path(), &mob, vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;
    let archived = rig
        .mob()
        .archive_identity(
            meerkat_mob::IdentityArchiveChange::new(operation("archive"), host(), CREATED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            archived,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{archived:?}"
    );
    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    assert_eq!(
        rig.record(CREATED).await,
        None,
        "nothing reported the change"
    );
    rig.shutdown_for_reopen().await;

    let mut rig = boot(temp.path(), &mob, vec![configured_spec(&[])]).await;
    let record = rig.record(CREATED).await;
    assert_eq!(
        record
            .as_ref()
            .map(|record| (record["revision"].clone(), record["effect"].clone())),
        Some((json!(1), json!("active"))),
        "the start recorded the archived agent's change: {record:#?}"
    );
    let unarchived = rig
        .mob()
        .unarchive_identity(
            meerkat_mob::IdentityArchiveChange::new(operation("unarchive"), host(), CREATED),
            allow(),
        )
        .await;
    assert!(
        matches!(
            unarchived,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "{unarchived:?}"
    );
    rig.turn(CREATED, "FIRST-AFTER-UNARCHIVE").await;
    rig.recorded(CREATED).await;
    rig.turn(CREATED, "SECOND-AFTER-UNARCHIVE").await;
    let request = entries(&rig.request_messages("SECOND-AFTER-UNARCHIVE"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(
        carries(&request[0], &["Keep BETA records."]),
        "the unarchived agent runs on the change: {}",
        request[0]
    );
    rig.shutdown().await;
}

/// An update whose change a delivery door recorded first (the door read the
/// changed row between the update's commit and its step): the step still
/// answers when the change applies, and writes nothing again.
///
/// RED while the step answered `unchanged` whenever the record already held
/// the text: an update that did change the instructions told its caller
/// nothing had changed.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_whose_change_a_door_recorded_first_answers_when_it_applies() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;
    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    rig.turn(CREATED, "DOOR-TURN").await;
    assert_eq!(
        rig.record(CREATED).await.expect("the door's record")["revision"],
        json!(1),
        "the door that read the changed row recorded it"
    );

    assert_eq!(
        rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
            .await,
        InstructionOutcome::AppliesAtNextRun,
        "the update's step answers when its change applies"
    );
    assert_eq!(
        rig.record(CREATED).await.expect("the record")["revision"],
        json!(1),
        "and writes nothing again"
    );
    rig.shutdown().await;
}

/// An update that leaves the sections that count as they are (other fields
/// of the row changed, or only an empty section came or went) records
/// nothing and answers `unchanged`.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_that_leaves_the_sections_as_they_are_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;
    assert_eq!(
        rig.after_update(&["Keep ALPHA records.", ""], &["Keep ALPHA records."])
            .await,
        InstructionOutcome::Unchanged
    );
    assert_eq!(rig.record(CREATED).await, None, "nothing is recorded");
    rig.turn(CREATED, "SECOND-TURN").await;
    assert!(
        entries(&rig.transcript(CREATED).await).is_empty(),
        "and nothing is appended"
    );
    rig.shutdown().await;
}

/// A change of a store-owned agent's row that nothing reported before the
/// gateway stopped: the next start compares the agent as it compares a
/// configured one, records the change, wakes nothing, and the agent's next
/// run carries the text.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_owned_agents_change_made_before_a_stop_is_recorded_at_start() {
    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let mut rig = boot(temp.path(), &mob, vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;
    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    assert_eq!(
        rig.record(CREATED).await,
        None,
        "nothing reported the change"
    );
    rig.shutdown_for_reopen().await;

    let mut rig = boot(temp.path(), &mob, vec![configured_spec(&[])]).await;
    let record = rig.record(CREATED).await;
    assert_eq!(
        record.as_ref().map(|record| record["revision"].clone()),
        Some(json!(1)),
        "the start recorded the change: {record:#?}"
    );
    assert!(rig.dormant(CREATED), "the start woke nothing");
    rig.recorded(CREATED).await;
    rig.turn(CREATED, "SECOND-TURN").await;
    let request = entries(&rig.request_messages("SECOND-TURN"));
    assert_eq!(request.len(), 1, "{request:#?}");
    assert!(carries(&request[0], &["Keep BETA records."]));
    rig.shutdown().await;
}

/// A restart of an agent whose conversation was built with sections its
/// host's build customizer added after the spec's own: its recorded build
/// sections start with the spec's, so nothing is written at start.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_of_an_agent_built_with_customizer_sections_writes_nothing() {
    use meerkat_meta::identity_first::contracts::AgentCustomizer;
    use meerkat_meta::identity_first::{AgentBuildContext, AgentBuildDraft, CustomizerError};

    struct SectionCustomizer;

    #[async_trait::async_trait]
    impl AgentCustomizer for SectionCustomizer {
        async fn customize_build(
            &self,
            _context: &AgentBuildContext,
            _spec: &DurableAgentSpec,
            draft: &mut AgentBuildDraft,
        ) -> Result<(), CustomizerError> {
            draft
                .additional_instructions
                .push("A SECTION THE HOST'S CUSTOMIZER ADDS.".to_string());
            Ok(())
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let alpha = vec![configured_spec(&["Answer in ALPHA style."])];
    let mut rig = boot_with(temp.path(), &mob, alpha.clone(), |builder| {
        builder.agent_customizer(Arc::new(SectionCustomizer))
    })
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.shutdown_for_reopen().await;

    let mut rig = boot_with(temp.path(), &mob, alpha, |builder| {
        builder.agent_customizer(Arc::new(SectionCustomizer))
    })
    .await;
    assert_eq!(
        rig.record(CONFIGURED).await,
        None,
        "the customizer's sections are no change of the agent's instructions"
    );
    rig.turn(CONFIGURED, "SECOND-TURN").await;
    assert!(
        entries(&rig.transcript(CONFIGURED).await).is_empty(),
        "nothing is appended"
    );
    rig.shutdown().await;
}

/// A host cannot write or revoke in the namespaces the platform keeps for
/// agents' own instructions, typed, and still reads the records there.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_write_to_the_reserved_namespaces_is_refused_and_their_records_are_readable() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.configure(&["Answer in BETA style."]).await;

    for namespace in [NAMESPACE, "meerkat.identity.anything"] {
        let set = rig
            .rpc(
                "meta/set_instruction",
                json!({
                    "namespace": namespace,
                    "key": CONFIGURED,
                    "revision": 7,
                    "body": "A host's own text.",
                    "target": {"kind": "mob"},
                }),
            )
            .await;
        assert_eq!(
            (
                set["error"]["code"].clone(),
                set["error"]["data"]["instruction_set_code"].clone()
            ),
            (json!(-32602), json!("invalid_request")),
            "a host write to {namespace} is refused: {set:#?}"
        );
        let revoke = rig
            .rpc(
                "meta/revoke_instruction",
                json!({"namespace": namespace, "key": CONFIGURED, "revision": 7}),
            )
            .await;
        assert_eq!(
            (
                revoke["error"]["code"].clone(),
                revoke["error"]["data"]["instruction_set_code"].clone()
            ),
            (json!(-32602), json!("invalid_request")),
            "a host revoke in {namespace} is refused: {revoke:#?}"
        );
    }
    let record = rig.record(CONFIGURED).await.expect("the agent's record");
    assert_eq!(
        (
            record["namespace"].clone(),
            record["key"].clone(),
            record["revision"].clone(),
            record["effect"].clone(),
            record["target"].clone()
        ),
        (
            json!(NAMESPACE),
            json!(CONFIGURED),
            json!(1),
            json!("active"),
            json!({"kind": "member", "identity": CONFIGURED})
        ),
        "the status read shows the agent's record, untouched: {record:#?}"
    );

    let own = rig
        .rpc(
            "meta/set_instruction",
            json!({
                "namespace": "app.example",
                "key": "tone",
                "revision": 1,
                "body": "Answer formally.",
                "target": {"kind": "mob"},
            }),
        )
        .await;
    assert!(
        own["error"].is_null(),
        "a host's own namespace is its own: {own:#?}"
    );
    rig.shutdown().await;
}

/// More than 64 agents each hold their own appended instruction: the mob
/// takes one record per agent whatever their number (one agent's snapshot
/// is what is bounded), and each agent's next run carries its own text and
/// nobody else's.
#[tokio::test(flavor = "multi_thread")]
async fn more_than_64_agents_each_get_their_own_appended_instruction() {
    const AGENTS: usize = 66;
    let names = (0..AGENTS)
        .map(|agent| format!("agent-{agent:02}"))
        .collect::<Vec<_>>();
    let spec = |name: &str, instructions: String| {
        meerkat_mob::CreatedIdentitySpec::new(name, "worker")
            .with_additional_instructions(vec![instructions])
    };
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    let created = rig
        .mob()
        .create_store_owned_identities(
            meerkat_mob::StoreOwnedIdentityCreate::new(
                operation("create-many"),
                host(),
                names
                    .iter()
                    .map(|name| {
                        meerkat_mob::CreatedIdentityRequest::new(spec(
                            name,
                            format!("First text of {name}."),
                        ))
                    })
                    .collect(),
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
        "Core creates the agents: {created:?}"
    );
    for name in &names {
        rig.turn(name, &format!("FIRST-TURN-OF-{name}.")).await;
    }

    let identities = rig
        .runtime
        .identity_runtime()
        .expect("identity runtime installed");
    for name in &names {
        let (previous, current) = (
            format!("First text of {name}."),
            format!("Changed text of {name}."),
        );
        rig.update_spec(
            &format!("update-{name}"),
            spec(name, previous.clone()),
            spec(name, current.clone()),
        )
        .await;
        assert_eq!(
            identities
                .after_update(
                    &rig.mob(),
                    &InstructionsChanged {
                        identity: name.clone(),
                        previous: vec![previous],
                        current: vec![current],
                    },
                )
                .await,
            InstructionOutcome::AppliesAtNextRun,
            "{name}'s change is recorded"
        );
    }
    let records = rig
        .rpc("meta/instruction_status", json!({"namespace": NAMESPACE}))
        .await["result"]["desired"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(records, AGENTS, "one record per agent");

    for name in &names {
        rig.recorded(name).await;
        let marker = format!("SECOND-TURN-OF-{name}.");
        rig.turn(name, &marker).await;
        let request = entries(&rig.request_messages(&marker));
        assert_eq!(request.len(), 1, "{name} reads one entry: {request:#?}");
        assert!(
            carries(&request[0], &[&format!("Changed text of {name}.")]),
            "{name} reads its own text: {}",
            request[0]
        );
    }
    rig.shutdown().await;
}

/// The record's body is the fixed preamble and the spec's own sections:
/// nothing of the profile's prompt, and nothing a host's customizer adds to
/// a build.
#[tokio::test(flavor = "multi_thread")]
async fn the_record_carries_the_specs_sections_only() {
    use meerkat_meta::identity_first::contracts::AgentCustomizer;
    use meerkat_meta::identity_first::{AgentBuildContext, AgentBuildDraft, CustomizerError};

    struct SectionCustomizer;

    #[async_trait::async_trait]
    impl AgentCustomizer for SectionCustomizer {
        async fn customize_build(
            &self,
            _context: &AgentBuildContext,
            _spec: &DurableAgentSpec,
            draft: &mut AgentBuildDraft,
        ) -> Result<(), CustomizerError> {
            draft
                .additional_instructions
                .push("A SECTION THE HOST'S CUSTOMIZER ADDS.".to_string());
            Ok(())
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot_with(
        temp.path(),
        &fresh_mob_id(),
        vec![configured_spec(&[
            "Answer in ALPHA style.",
            "Cite sources.",
        ])],
        |builder| builder.agent_customizer(Arc::new(SectionCustomizer)),
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    let prompt = rig.transcript(CONFIGURED).await[0]["content"]
        .as_str()
        .expect("the first prompt")
        .to_string();
    assert!(
        prompt.contains("A SECTION THE HOST'S CUSTOMIZER ADDS.")
            && prompt.contains("Answer in ALPHA style."),
        "the first prompt holds the spec's and the customizer's sections"
    );

    rig.configure(&["Answer in BETA style.", "", "Cite sources."])
        .await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-TURN").await;
    let request = entries(&rig.request_messages("SECOND-TURN"));
    assert_eq!(request.len(), 1, "{request:#?}");
    let body = format!("{TEXT_PREAMBLE}\n\nAnswer in BETA style.\n\nCite sources.");
    assert!(
        carries_body(&request[0], &body),
        "the entry's body is the preamble and the spec's sections, byte for byte: {}",
        request[0]
    );
    let record = rig.record(CONFIGURED).await.expect("the agent's record");
    assert_eq!(
        record["content_sha256"],
        json!(meerkat_core::InstructionContentDigest::for_body(&body).to_string()),
        "the record's digest is that body's"
    );

    rig.configure(&[]).await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "THIRD-TURN").await;
    let request = entries(&rig.request_messages("THIRD-TURN"));
    assert!(
        request
            .last()
            .is_some_and(|entry| carries_body(entry, WITHDRAWN_BODY)),
        "a spec with no instructions left withdraws them, in the fixed words: {request:#?}"
    );
    rig.shutdown().await;
}

/// A model that takes a system instruction only ahead of the conversation:
/// the change is recorded, the step answers `applies_after_compaction`, the
/// agent's next run carries no entry, and the status read says why.
#[tokio::test(flavor = "multi_thread")]
async fn a_leading_prefix_only_model_answers_applies_after_compaction() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id(), vec![configured_spec(&[])]).await;
    rig.create_spec(created_spec_on("prefix", &["Keep ALPHA records."]))
        .await;
    rig.turn(CREATED, "FIRST-TURN").await;

    rig.update_spec(
        "update-one",
        created_spec_on("prefix", &["Keep ALPHA records."]),
        created_spec_on("prefix", &["Keep BETA records."]),
    )
    .await;
    assert_eq!(
        rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
            .await,
        InstructionOutcome::AppliesAfterCompaction
    );
    rig.recorded(CREATED).await;
    rig.turn(CREATED, "SECOND-TURN").await;
    assert!(
        entries(&rig.request_messages("SECOND-TURN")).is_empty(),
        "no entry is appended after conversation rows for such a model"
    );
    let status = rig.status(CREATED).await;
    assert_eq!(
        status["members"][0]["slots"][0]["state"],
        json!({"state": "pending", "reason": "leading_prefix_only_lowering"}),
        "the status read says why the change is pending: {status:#?}"
    );
    rig.shutdown().await;
}

mod governed {
    //! A host's native grant configuration that refuses everything: enough
    //! to make the runtime a governed one.

    use std::sync::Arc;

    use meerkat_authorization::clock::HostAuthorizationClock;
    use meerkat_authorization::grant_policy::{
        AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
    };
    use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
    use meerkat_authorization::policy::{LocalPolicyAllowance, LocalPolicyPurpose};
    use meerkat_authorization::publication::LocalAuthorizationPublication;
    use meerkat_authorization_contracts::evidence::EvidenceId;
    use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
    use meerkat_core::auth::{PrincipalKind, PrincipalRef};
    use meerkat_core::{OperationAuthorizationError, PreparedAuthorizationBinding};
    use meerkat_runtime::input_authority::NativeAdmissionError;
    use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;

    struct RefusingOwner;

    impl AdmittedWorkPolicyOwner for RefusingOwner {
        fn authorize_admitted_work(
            &self,
            _association: &InputAuthorityAssociation,
            _binding: &PreparedAuthorizationBinding,
            _purpose: LocalPolicyPurpose,
            _now_ms: u64,
        ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
            Err(OperationAuthorizationError::Unavailable)
        }
    }

    impl OperationPolicyOwner for RefusingOwner {
        fn authorize_operation(
            &self,
            _association: &InputAuthorityAssociation,
            _binding: &PreparedAuthorizationBinding,
            _purpose: LocalPolicyPurpose,
            _now_ms: u64,
        ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
            Err(OperationAuthorizationError::Unavailable)
        }
    }

    pub fn configuration() -> NativeGrantWorkConfiguration {
        let root = PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            "grant-owner",
            meerkat_core::TrustDomainId::new("instruction-append").expect("trust domain"),
        )
        .expect("service principal");
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root,
                    namespace: EvidenceId::new("instruction-append-grants").expect("namespace"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("host grant owner"),
        );
        let ingress: Arc<meerkat_runtime::meerkat_machine::NativeIngressCheck> =
            Arc::new(|_, _, _, _| -> Result<(), NativeAdmissionError> {
                Err(OperationAuthorizationError::Unavailable.into())
            });
        NativeGrantWorkConfiguration {
            grants,
            ingress,
            invocation_owner: Arc::new(RefusingOwner),
            operation_owner: Arc::new(RefusingOwner),
            retained_resume: None,
            kept_original: None,
            require_scoped_destination: true,
            configured_mcp_attachment: None,
        }
    }
}

/// A governed runtime takes no instruction record: the change of an agent's
/// instructions answers `applies_at_next_session` with the typed reason
/// `prepared_authorization_required`, nothing is recorded, and the agent's
/// row holds the change all the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_runtime_answers_prepared_authorization_required_and_keeps_the_change() {
    let temp = tempfile::tempdir().unwrap();
    let mob = fresh_mob_id();
    let mut rig = boot(temp.path(), &mob, vec![configured_spec(&[])]).await;
    rig.create(&["Keep ALPHA records."]).await;
    rig.turn(CREATED, "FIRST-TURN").await;
    rig.shutdown_for_reopen().await;

    let rig = boot_with(temp.path(), &mob, vec![configured_spec(&[])], |builder| {
        builder.native_grant_work_configuration(governed::configuration())
    })
    .await;
    rig.update(
        "update-one",
        &["Keep ALPHA records."],
        &["Keep BETA records."],
    )
    .await;
    assert_eq!(
        rig.after_update(&["Keep ALPHA records."], &["Keep BETA records."])
            .await,
        InstructionOutcome::AppliesAtNextSession {
            reason: InstructionNextSessionReason::PreparedAuthorizationRequired
        }
    );
    assert_eq!(rig.record(CREATED).await, None, "no record was written");
    let row = rig
        .mob()
        .identity_record(&meerkat_mob::AgentIdentity::from(CREATED))
        .await;
    assert!(
        format!("{row:?}").contains("Keep BETA records."),
        "the agent's row holds the change: {row:?}"
    );
    rig.shutdown().await;
}

/// What a fork of an agent whose instructions changed runs on, one route
/// each: the fork's first request, and whether it holds the agent's first
/// prompt text and its changed text.
struct ForkFacts {
    first_prompt_text: bool,
    changed_text: bool,
    entries_with_a_body: usize,
}

fn fork_facts(request: &[Value]) -> ForkFacts {
    let whole = Value::Array(request.to_vec()).to_string();
    ForkFacts {
        first_prompt_text: whole.contains("Answer in ALPHA style."),
        changed_text: whole.contains("Answer in BETA style."),
        entries_with_a_body: entries(request).len(),
    }
}

/// The source of the fork rows: the configured agent on the profile with
/// mob tools, built with one text, changed to another, and run once on the
/// change.
async fn edited_fork_source(state: &Path) -> Rig {
    let mut rig = boot(
        state,
        &fresh_mob_id(),
        vec![spec_on("forker", &["Answer in ALPHA style."])],
    )
    .await;
    rig.turn(CONFIGURED, "FIRST-TURN").await;
    rig.roster
        .upsert(spec_on("forker", &["Answer in BETA style."]));
    rig.roster_pass().await;
    rig.recorded(CONFIGURED).await;
    rig.turn(CONFIGURED, "SECOND-TURN").await;
    assert_eq!(
        entries(&rig.request_messages("SECOND-TURN")).len(),
        1,
        "the source itself runs on its changed instructions"
    );
    rig
}

/// `fork_off`, the agent tool: the child's first request, as it is today.
/// The child is another member, which the source's record does not select.
///
/// Pinned as it is, not as it should be: a fork does not carry the agent's
/// changed instructions in this release.
#[tokio::test(flavor = "multi_thread")]
async fn a_fork_off_child_of_an_edited_agent_starts_from_the_first_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = edited_fork_source(temp.path()).await;
    let sent = rig
        .rpc(
            "meta/send",
            json!({"identity": CONFIGURED, "content": format!("{FORK_OFF} now")}),
        )
        .await;
    assert!(sent["error"].is_null(), "{sent:#?}");
    assert!(
        rig.asked("FORK-TASK-MARKER").await,
        "the fork child runs its task"
    );
    let facts = fork_facts(&rig.request_messages("FORK-TASK-MARKER"));
    eprintln!(
        "MEASURED fork_off: first prompt text {}, changed text {}, entries with a body {}",
        facts.first_prompt_text, facts.changed_text, facts.entries_with_a_body
    );
    assert_eq!(
        (
            facts.first_prompt_text,
            facts.changed_text,
            facts.entries_with_a_body
        ),
        (true, false, 0),
        "the child starts from the agent's first prompt, without the changed text"
    );
    rig.shutdown().await;
}

/// The durable fork (`UnifiedRuntime::fork_member`, which `meta/fork_member`
/// and a console side chat run): the child's first request, as it is today.
/// The child reads the source's entry row as an inherited header that names
/// the record and its digest and holds no text: it is told an instruction
/// exists that it cannot read. Pinned as it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_durable_fork_of_an_edited_agent_starts_from_the_first_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = edited_fork_source(temp.path()).await;
    let forked = rig
        .runtime
        .fork_member(
            CONFIGURED,
            meerkat_mob::SpawnMemberSpec::new("forker", "durable-fork"),
            None,
        )
        .await;
    assert!(forked.is_ok(), "the durable fork commits: {forked:?}");
    let message = rig
        .rpc(
            "meta/send_message",
            json!({"member_id": "durable-fork", "message": "DURABLE-FORK-TURN"}),
        )
        .await;
    assert!(message["error"].is_null(), "{message:#?}");
    assert!(rig.asked("DURABLE-FORK-TURN").await, "the fork runs a turn");
    let request = rig.request_messages("DURABLE-FORK-TURN");
    let facts = fork_facts(&request);
    let inherited = request
        .iter()
        .filter_map(|message| message["content"].as_str())
        .filter(|content| content.starts_with("[meerkat-inherited-operator-instructions-v1]"))
        .count();
    eprintln!(
        "MEASURED durable fork: first prompt text {}, changed text {}, entries with a body {}, \
         inherited headers {inherited}",
        facts.first_prompt_text, facts.changed_text, facts.entries_with_a_body
    );
    assert_eq!(
        (
            facts.first_prompt_text,
            facts.changed_text,
            facts.entries_with_a_body,
            inherited
        ),
        (true, false, 0, 1),
        "the child reads the source's entry as a header without its text"
    );
    rig.shutdown().await;
}

/// The helper fork (`meta/fork_helper`): the helper's first request, as it
/// is today. A helper is a fresh one-shot member handed the source's
/// conversation as context: it holds neither the source's first prompt text
/// nor its changed text.
#[tokio::test(flavor = "multi_thread")]
async fn a_helper_fork_of_an_edited_agent_carries_neither_text() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = edited_fork_source(temp.path()).await;
    let helped = rig
        .rpc(
            "meta/fork_helper",
            json!({
                "source_member_id": CONFIGURED,
                "agent_identity": "helper-fork",
                "task": "HELPER-FORK-TASK: carry on",
                "result_label": "helper-result",
                "max_text_bytes": 4096,
            }),
        )
        .await;
    assert!(
        helped["error"].is_null(),
        "the helper fork runs: {helped:#?}"
    );
    assert!(
        rig.asked("HELPER-FORK-TASK").await,
        "the helper runs its task"
    );
    let facts = fork_facts(&rig.request_messages("HELPER-FORK-TASK"));
    eprintln!(
        "MEASURED helper fork: first prompt text {}, changed text {}, entries with a body {}",
        facts.first_prompt_text, facts.changed_text, facts.entries_with_a_body
    );
    assert_eq!(
        (
            facts.first_prompt_text,
            facts.changed_text,
            facts.entries_with_a_body
        ),
        (false, false, 0),
        "the helper holds none of the source's instructions"
    );
    rig.shutdown().await;
}

/// One function owns the comparison and the records: nothing else in Meta's
/// source names the reserved namespace, calls the comparison, or writes or
/// revokes an operator instruction, besides the host's own verbs (which
/// refuse the reserved namespaces).
#[test]
fn one_function_owns_the_instruction_records() {
    fn rust_sources(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                rust_sources(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    assert!(sources.len() > 100, "the crate's sources are read");

    const OWNER: &str = "identity_first/runtime/instruction_append.rs";
    // Each needle, and the files besides the owner that may hold it in code
    // (comment lines do not count), with how often.
    let rules: &[(&str, &[(&str, usize)])] = &[
        ("\"meerkat.identity.\"", &[]),
        ("\"meerkat.identity.instructions", &[]),
        (
            "INSTRUCTIONS_NAMESPACE",
            &[("identity_first/runtime.rs", 1)],
        ),
        (".append_changed_instructions(", &[]),
        (
            ".append_changed_instructions_on_registration(",
            &[
                ("identity_first/runtime.rs", 2),
                ("identity_first/runtime/created.rs", 1),
            ],
        ),
        (
            ".append_changed_instructions_at_first_door(",
            &[("identity_first/runtime/created.rs", 1)],
        ),
        (
            ".revoke_instruction_record_for_fresh_life(",
            &[("identity_first/runtime.rs", 1)],
        ),
        (
            ".set_operator_instruction(",
            &[("rpc/operator_methods.rs", 1)],
        ),
        (
            ".revoke_operator_instruction(",
            &[("rpc/operator_methods.rs", 1)],
        ),
    ];
    let mut violations = Vec::new();
    for (needle, allowed) in rules {
        for path in &sources {
            let relative = path
                .strip_prefix(&root)
                .expect("under the source root")
                .to_string_lossy()
                .replace('\\', "/");
            if relative == OWNER {
                continue;
            }
            let source = std::fs::read_to_string(path).expect("a source file");
            let count = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .filter(|line| line.contains(needle))
                .count();
            let expected = allowed
                .iter()
                .find(|(file, _)| *file == relative)
                .map_or(0, |(_, count)| *count);
            if count != expected {
                violations.push(format!(
                    "{relative}: `{needle}` occurs {count} times in code, expected {expected}"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "only {OWNER} keeps agents' instruction records: {violations:#?}"
    );
}
