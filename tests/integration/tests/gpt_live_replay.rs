#![cfg(all(feature = "gpt-live-replay", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// The live gate's file is a crate root in its own target and declares its
// own copy of the harness support module, so this target loads those two
// files twice: once here and once inside `gpt_live_public_e2e`.
#![allow(clippy::duplicate_mod)]

//! Deterministic replays of recorded Turbo S gpt-live-1 runs (S104).
//!
//! Each test drives the same host graph as `gpt_live_public_e2e.rs` (RPC
//! server, mob, ExistingMember/DurableFork executor, the shared exact-receipt
//! live host with a concurrent bootstrap summary), with three substitutions:
//! - the provider is a [`Cassette`] serving a recorded, scrubbed provider
//!   stream (`tests/integration/fixtures/gpt_live_replay/`), gated causally
//!   on Meerkat's own client events and the test's steps, never on time;
//! - every executor/worker LLM call is a [`ScriptedLlm`] keyed by purpose,
//!   and each open's summary is a [`RecordedSummarizer`]: the recorded text,
//!   seeded or late exactly as the recorded open had it;
//! - there is no browser: the offer is a literal string, and the test steps
//!   the recording's markers (`play_at:*`, `disconnect:*`) itself.
//!
//! The replay records its own provider stream through the same journal hook
//! as a live run and must send exactly the recording's client events per
//! channel (type and deterministic `event_id`), in order.
//!
//! Re-capture: `scripts/gpt-live-recapture-replay-fixture S104` (see the
//! fixture README).

/// The live gate's own file, compiled into this keyless target so that a
/// normal lane runs the rows that test its judges (`config_tests` and the
/// other rows beside the scenarios): they need no provider and no key. The
/// scenarios themselves stay ignored (`lane:e2e-smoke`), as in their own
/// target, and still stop at their first line without a provider key.
#[path = "gpt_live_public_e2e.rs"]
mod gpt_live_public_e2e;
#[path = "support/gpt_live_replay.rs"]
mod replay;
#[path = "support/gpt_live_e2e.rs"]
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat::experimental_gpt_live::{
    ExperimentalGptLiveOpenAuthority, ExperimentalGptLiveWebrtcTransport,
    ExperimentalLiveOpenAuthorityProvider, ExperimentalLivePublicObservation,
    ExperimentalLivePublicObservationDeliveryError, ExperimentalLivePublicObservationPublisher,
    GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID, GPT_LIVE_PUBLIC_MODEL, LIVE_LATE_SUMMARY_PREFIX,
    LIVE_RUNTIME_WORK_PREFIX, PublicGptLiveOpenAuthorityConfig, PublicGptLivePlaybackPolicy,
    provider_recording,
};
use meerkat::session_runtime::live_summary::{
    LiveContextBootstrapMode, LiveContextSummarizer, LiveContextSummaryError,
    LiveContextSummaryPolicy, LiveContextSummarySnapshot,
};
use meerkat::surface::{
    ExperimentalGptLiveContextMirrorHost, ExperimentalLiveChannelPhaseStatus,
    LiveWebrtcBoundReadyBinder, ServiceMemberLiveHost, ServiceMemberLiveHostConfig,
};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_contracts::{LiveOpenTransport, WireLiveTransportBootstrap};
use meerkat_core::{
    AuthBindingRef, AuthProfileConfig, BackendProfileConfig, BindingId, BindingOrigin,
    BindingPolicy, BlobStore, Config, ConfigRuntime, ConfigStore, CredentialSourceSpec,
    MemoryConfigStore, Message, ProviderBindingConfig, RealmConfigSection, RealmId,
};
use meerkat_live::LiveChannelId;
use meerkat_mob_mcp::live_delegation::{
    LIVE_DELEGATION_SPEECH_TRANSCRIPT_NOTE, LiveDelegationExecutionPolicy,
    compose_experimental_live_delegation_coordinator_with_policy,
};
use meerkat_rpc::router::NotificationSink;
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_runtime::live_execution::{
    LiveDelegationWorkerOwnership, LiveDelegationWorkerTerminalKind,
};
use serde_json::{Value, json};
use tokio::io::BufReader;
use tokio::sync::watch;
use tokio::time::{Duration, timeout};

use replay::{Cassette, ClientKey, Fixture, RecordedSummary, SummaryGate, fixture_findings};
use support::evidence::{self, Journal};
use support::{
    ExplicitScenarioBindingAuthority, FixedConfigSource, JsonlRpcClient, execution_identity,
};

const REALM: &str = "scenario-replay-gpt-live-public";
const BINDING: &str = "openai_api_key";
/// The replay never reaches a real provider: the cassette ignores the key.
const FIXTURE_SECRET: &str = "replay-fixture-key-not-a-secret";
const EXECUTOR_MODEL: &str = "gpt-5.6-sol";
/// Any non-blank offer: the broker forwards it, the cassette ignores it.
const OFFER_SDP: &str = "v=0\r\nREPLAY_OFFER_SDP";
/// Safety bound on host-side convergence waits (custody, delegation
/// terminality). Ordering never rides it: the cassette and the scripted
/// LLM's gates order every step.
const CONVERGENCE_BOUND: Duration = Duration::from_secs(60);

const S106_FIXTURE: &str = include_str!("../fixtures/gpt_live_replay/s106.provider-stream.jsonl");
const S106_SEED_PROMPT: &str =
    "For the record: the sponsor's name is Marlow. Just acknowledge in one short sentence.";
const S106_TYPED_PROMPT: &str = "Typed while the voice call is down: the budget code is Kestrel. Reply with one short sentence.";
const S106_EXECUTOR_INSTRUCTIONS: &str = "You are the executor behind a voice assistant. Your current working directory is the \
     scratch workspace; do every file operation there with the shell tool. When asked for a \
     note of at least two hundred words, write at least two hundred words into the requested \
     file, then answer with the word count in one short sentence. When asked to add a sentence \
     to a file, append it, run `wc -w` on the file and answer with the new number in one short \
     sentence.";
const S104_FIXTURE: &str = include_str!("../fixtures/gpt_live_replay/s104.provider-stream.jsonl");
/// Every committed fixture, for the scrub check.
const FIXTURES: &[(&str, &str)] = &[("s104", S104_FIXTURE), ("s106", S106_FIXTURE)];
const S104_SEED_PROMPT: &str = "For the record: the team mascot is a heron named Bartleby. Just acknowledge in one short sentence.";
const S104_TYPED_PROMPT: &str = "Typed while the voice call is down: remember that the meeting room is called Osprey. Reply with one short sentence.";
const S104_EXECUTOR_INSTRUCTIONS: &str = "You are the executor behind a voice assistant. Your current working directory is the \
     scratch workspace; do every file operation there with the shell tool. When asked for an \
     ode in a file, write exactly the requested file with the requested content, then in your \
     spoken answer read it back line by line. Answer typed questions in one short sentence.";
/// The DurableFork merge's injected context names the post-close result.
const S104_MERGE_MARKER: &str = "which finished after the voice call ended";
const S104_RESULT_TOKEN: &str = "lantern";

// --- scripted LLM -----------------------------------------------------------

/// What an LLM call is for, read from its request: the live delegation's
/// worker turn, the source member's reply to a merged post-close result, or
/// the executor's next conversational turn (by ordinal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Purpose {
    /// The live delegation worker's turn of the session's n-th job.
    DelegatedJob(usize),
    MergeReply,
    Conversation(usize),
}

/// One scripted answer, optionally held until a gate opens.
#[derive(Clone)]
struct Script {
    text: String,
    gate: Option<watch::Receiver<bool>>,
}

/// Executor and worker LLM, keyed by purpose. Request bytes never select
/// an answer; only the purpose (and the conversational ordinal) does.
struct ScriptedLlm {
    scripts: BTreeMap<Purpose, Script>,
    conversations: AtomicUsize,
    jobs: AtomicUsize,
    calls: std::sync::Mutex<Vec<(Purpose, String)>>,
    /// Advanced on every call, so a test can await one by purpose.
    called: watch::Sender<usize>,
}

impl ScriptedLlm {
    fn purpose(&self, request: &LlmRequest) -> Purpose {
        // The newest input names what the turn is for: a merged post-close
        // result is injected context carrying the merge marker; a live
        // delegation's worker runs over the delegation execution context.
        let newest = request
            .messages
            .last()
            .and_then(|message| serde_json::to_string(message).ok())
            .unwrap_or_default();
        if newest.contains(S104_MERGE_MARKER) {
            return Purpose::MergeReply;
        }
        // A live delegation's worker (a fork, or the existing member under
        // its execution-context row) gets the voice request framed as a
        // speech transcript as its newest user row; a conversational turn
        // never does (an earlier job's row may sit in the history).
        let delegated = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .is_some_and(|text| text.contains(LIVE_DELEGATION_SPEECH_TRANSCRIPT_NOTE));
        if delegated {
            return Purpose::DelegatedJob(self.jobs.fetch_add(1, Ordering::SeqCst));
        }
        Purpose::Conversation(self.conversations.fetch_add(1, Ordering::SeqCst))
    }
}

impl ScriptedLlm {
    /// Wait until a call for `purpose` has been made (each call notifies).
    async fn wait_for_call(&self, purpose: Purpose) -> Result<(), Box<dyn std::error::Error>> {
        let mut called = self.called.subscribe();
        let seen = |calls: &std::sync::Mutex<Vec<(Purpose, String)>>| {
            calls
                .lock()
                .expect("calls")
                .iter()
                .any(|(made, _)| *made == purpose)
        };
        timeout(CONVERGENCE_BOUND, async {
            while !seen(&self.calls) {
                if called.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .map_err(|_| format!("the scripted LLM never received a {purpose:?} call"))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedLlm {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let purpose = self.purpose(request);
        let newest: String = request
            .messages
            .last()
            .and_then(|message| serde_json::to_string(message).ok())
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect();
        if std::env::var_os("REPLAY_DUMP_REQUESTS").is_some() {
            let roles: Vec<String> = request
                .messages
                .iter()
                .map(|message| {
                    serde_json::to_string(message)
                        .unwrap_or_default()
                        .chars()
                        .take(220)
                        .collect()
                })
                .collect();
            eprintln!("REPLAY_REQUEST purpose={purpose:?} messages={roles:#?}");
        }
        self.calls.lock().expect("calls").push((purpose, newest));
        self.called.send_modify(|count| *count += 1);
        let script = self.scripts.get(&purpose).cloned();
        let model = request.model.clone();
        Box::pin(
            futures::stream::once(async move {
                let Some(script) = script else {
                    return vec![Err(LlmError::InvalidRequest {
                        message: format!("replay script has no answer for {purpose:?}"),
                    })];
                };
                if let Some(mut gate) = script.gate {
                    let _ = gate.wait_for(|open| *open).await;
                }
                vec![
                    Ok(LlmEvent::TextDelta {
                        delta: script.text,
                        meta: None,
                    }),
                    Ok(LlmEvent::UsageUpdate {
                        usage: meerkat_core::TurnUsage::host_declared(
                            meerkat_core::Provider::OpenAI,
                            &model,
                            meerkat_core::Usage {
                                input_tokens: 1,
                                output_tokens: 1,
                                ..Default::default()
                            },
                        ),
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                    }),
                ]
            })
            .flat_map(futures::stream::iter),
        )
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

use futures::StreamExt as _;

/// Each open's summary as the recording had it. The live pre-open summary
/// wait is bounded, so whether an open was seeded with its summary or got it
/// late on the thinking lane is live timing. The replay follows the recorded
/// open instead, keyed on the tape, never on time:
/// - seeded: the recorded summary, ready at once, so this open seeds it;
/// - late: the recorded summary, held until this open's create request has
///   been served, so it cannot be ready within the pre-open bound and
///   follows on the thinking lane as recorded;
/// - absent (an open past the recording, or one that carried none): the
///   scenario's fallback text, ready at once.
///
/// The recorded text also keeps the late append's fragment count (and so
/// its `event_id`s) equal to the recording's.
struct RecordedSummarizer {
    gate: SummaryGate,
    fallback: String,
}

#[async_trait::async_trait]
impl LiveContextSummarizer for RecordedSummarizer {
    async fn summarize(
        &self,
        _snapshot: LiveContextSummarySnapshot<'_>,
    ) -> Result<String, LiveContextSummaryError> {
        let open = self.gate.opens_created();
        match self
            .gate
            .fixture()
            .recorded_summary(open, LIVE_LATE_SUMMARY_PREFIX)
        {
            RecordedSummary::Seeded(summary) => Ok(summary),
            RecordedSummary::Late(summary) => {
                self.gate
                    .created(open + 1)
                    .await
                    .map_err(LiveContextSummaryError::Producer)?;
                Ok(summary)
            }
            RecordedSummary::Absent => Ok(self.fallback.clone()),
        }
    }
}

/// Unmeasured playback, as the recorded runs: no actionable output
/// publication is ever accepted.
struct UnmeasuredPublisher;

#[async_trait::async_trait]
impl ExperimentalLivePublicObservationPublisher for UnmeasuredPublisher {
    async fn publish(
        &self,
        _observation: ExperimentalLivePublicObservation,
    ) -> Result<(), ExperimentalLivePublicObservationDeliveryError> {
        Err(ExperimentalLivePublicObservationDeliveryError::Rejected)
    }
}

// --- host graph -------------------------------------------------------------

fn auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(REALM).expect("valid realm"),
        binding: BindingId::parse(BINDING).expect("valid binding"),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

fn replay_config() -> Config {
    let mut section = RealmConfigSection {
        backend: BTreeMap::new(),
        auth: BTreeMap::new(),
        binding: BTreeMap::new(),
        default_binding: Some(BINDING.to_string()),
        parent: None,
        connector_client: BTreeMap::new(),
        copilot: None,
    };
    section.backend.insert(
        "openai_api".to_string(),
        BackendProfileConfig {
            provider: "openai".to_string(),
            backend_kind: "openai_api".to_string(),
            base_url: None,
            options: Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        BINDING.to_string(),
        AuthProfileConfig {
            provider: "openai".to_string(),
            auth_method: "api_key".to_string(),
            source: CredentialSourceSpec::InlineSecret {
                secret: FIXTURE_SECRET.to_string(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        BINDING.to_string(),
        ProviderBindingConfig {
            backend_profile: "openai_api".to_string(),
            auth_profile: BINDING.to_string(),
            credential_account: None,
            default_model: Some(EXECUTOR_MODEL.to_string()),
            policy: BindingPolicy::default(),
            provider_default: false,
        },
    );
    let mut config = Config::default();
    config.realm.insert(REALM.to_string(), section);
    config.model_fallback.enabled = Some(false);
    config
}

struct ReplayHost {
    _temp: tempfile::TempDir,
    rpc: JsonlRpcClient,
    _server: tokio::task::JoinHandle<()>,
    session_runtime: Arc<SessionRuntime>,
    mobs: Arc<meerkat_mob_mcp::MobMcpState>,
    mob_id: String,
    member_host: Arc<ServiceMemberLiveHost>,
    authority: Arc<ExperimentalGptLiveOpenAuthority>,
    transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    binder: Arc<dyn LiveWebrtcBoundReadyBinder>,
    session_id: meerkat_core::SessionId,
}

struct OpenChannel {
    id: LiveChannelId,
    pending_receipt: String,
    activation_receipt: String,
    /// The session's event stream, subscribed before the channel opened, so
    /// the committed close's `AgentEvent::LiveChannelClosed` cannot be missed.
    events: meerkat_core::EventStream,
}

struct HostOptions<'a> {
    base_url: &'a str,
    llm: Arc<ScriptedLlm>,
    policy: LiveDelegationExecutionPolicy,
    executor_instructions: &'a str,
    seed_prompt: Option<&'a str>,
    /// The summary of an open the recording carries none for.
    summary: &'a str,
    summary_gate: SummaryGate,
}

async fn open_replay_host(
    options: HostOptions<'_>,
) -> Result<ReplayHost, Box<dyn std::error::Error>> {
    let temp = tempfile::Builder::new()
        .prefix("gpt-live-replay-")
        .tempdir_in(support::test_tmp_root()?)?;
    let config = replay_config();
    let binding = auth_binding();
    let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
        .runtime_root(temp.path().join("runtime"))
        .project_root(temp.path().join("project"))
        .context_root(temp.path().join("project"))
        .builtins(true)
        .shell(true)
        .mob(true);
    tokio::fs::create_dir_all(temp.path().join("project")).await?;
    let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
        config.clone(),
        meerkat_models::canonical(),
    ));
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let persistence = meerkat::PersistenceBundle::new_with_subsystem_stores(
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()) as Arc<dyn BlobStore>,
        Arc::new(meerkat::DisabledScheduleStore),
        Arc::new(meerkat::MemoryWorkGraphStore::new()),
    )?;
    let runtime = Arc::new(SessionRuntime::new_with_config_store(
        factory.clone(),
        config.clone(),
        Arc::clone(&config_store),
        16,
        persistence,
        NotificationSink::noop(),
    )?);
    runtime.set_realm_context(
        Some(binding.realm.clone()),
        None,
        Some("memory".to_string()),
    );
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config-state.json"),
    )));
    // Every executor and worker turn is the scripted LLM.
    runtime.set_default_llm_client(Some(options.llm.clone() as Arc<dyn LlmClient>));

    let mobs = meerkat_rpc::router::compose_rpc_mob_state(&runtime, &config_store, None)?;
    runtime.set_mob_state(Arc::clone(&mobs));

    let public_transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
    let projection = Arc::new(
        meerkat_rpc::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(&runtime)),
    );
    let live_host = Arc::new(meerkat_live::LiveAdapterHost::new(projection.clone()));
    let webrtc = Arc::new(meerkat_live::LiveWebrtcState::new(
        live_host.clone(),
        projection.clone(),
        projection,
    ));
    let realm_source = Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
        temp.path().join("realm-state"),
        temp.path().join("global-config.toml"),
        meerkat_models::canonical(),
    ));
    let live_factory = meerkat_rpc::live_wiring::build_per_open_realtime_session_factory(
        &factory,
        Arc::clone(&config_store),
        realm_source,
        binding.realm.clone(),
    );
    let (client_stream, server_stream) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let mut rpc = JsonlRpcClient::new(client_stream);
    let callback_rx = runtime.init_callback_channel();
    let mut server = RpcServer::new_with_skill_runtime_and_mob_state(
        BufReader::new(server_read),
        server_write,
        Arc::clone(&runtime),
        Arc::clone(&config_store),
        None,
        Arc::clone(&mobs),
        callback_rx,
    )
    .with_live_session_factory_opt(Some(live_factory.clone()))
    .with_live_webrtc(webrtc.clone())
    .with_live_webrtc_answer_transport(public_transport.clone());
    let server = tokio::spawn(async move {
        let _ = server.run().await;
    });

    rpc.call("initialize", json!({}), 60).await?;
    let mob_id = format!("gpt-live-replay-{}", uuid::Uuid::new_v4().simple());
    rpc.call(
        "mob/create",
        json!({"definition":{"id":mob_id,"profiles":{"executor":{
            "model":EXECUTOR_MODEL,
            "runtime_mode":"turn_driven","external_addressable":true,
            "tools":{"builtins":true,"shell":true,"comms":true}
        }}}}),
        60,
    )
    .await?;
    rpc.call(
        "mob/spawn",
        json!({"mob_id":mob_id,"profile":"executor","agent_identity":"voice-executor",
            "runtime_mode":"turn_driven",
            "additional_instructions":[options.executor_instructions],
            "auth_binding":{"realm":REALM,"binding":BINDING}}),
        60,
    )
    .await?;
    let status = rpc
        .call(
            "mob/member_status",
            json!({"mob_id":mob_id,"agent_identity":"voice-executor"}),
            60,
        )
        .await?;
    let session_id = meerkat_core::SessionId::parse(
        status["current_session_id"]
            .as_str()
            .ok_or("spawned executor has no durable session")?,
    )?;
    if let Some(prompt) = options.seed_prompt {
        rpc.call(
            "turn/start",
            json!({"session_id":session_id,"prompt":prompt}),
            120,
        )
        .await?;
    }

    let authority =
        ExperimentalGptLiveOpenAuthority::new_public(PublicGptLiveOpenAuthorityConfig {
            agent_factory: factory.clone(),
            config_source: Arc::new(FixedConfigSource(config.clone())),
            binding_authority: Arc::new(ExplicitScenarioBindingAuthority {
                session_id: session_id.clone(),
                binding: binding.clone(),
                auth_lease: runtime.generated_auth_lease_handle(),
                mobs: Arc::clone(&mobs),
                principal_id: "replay-operator",
            }),
            execution_identity: meerkat_core::SessionLlmIdentity {
                model: GPT_LIVE_PUBLIC_MODEL.to_string(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: Some(binding.clone()),
            },
            realm: binding.realm.clone(),
            transport: Arc::clone(&public_transport),
            voice: "marin".to_string(),
            session_instructions: None,
            session_instructions_preface: None,
        })?
        .with_public_playback_policy(PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured)?
        .with_test_base_url(options.base_url);
    let authority = Arc::new(authority);

    let member_host = ServiceMemberLiveHost::new(ServiceMemberLiveHostConfig {
        service: runtime.inner().service.clone(),
        runtime_adapter: runtime.runtime_adapter(),
        host: live_host.clone(),
        ws_state: None,
        base_url: None,
        session_factory: live_factory,
        realm_id: runtime.realm_id(),
        instance_id: runtime.instance_id(),
        backend: runtime.backend(),
    })
    .with_webrtc_cleanup_state(webrtc)
    .with_context_summary_policy(
        LiveContextSummaryPolicy::new(
            Arc::new(RecordedSummarizer {
                gate: options.summary_gate,
                fallback: options.summary.to_owned(),
            }),
            4 * 1024 * 1024,
            16 * 1024,
            Duration::from_secs(60),
        )?
        .with_bootstrap_mode(LiveContextBootstrapMode::Concurrent),
    );
    let member_host = Arc::new(member_host);
    let coordinator = compose_experimental_live_delegation_coordinator_with_policy(
        runtime.runtime_adapter(),
        mobs.clone(),
        options.policy,
    );
    // As the RPC router does: the coordinator tells later opens about work
    // that outlived its channel.
    authority.bind_post_close_work_source(coordinator.clone());
    let context_host = ExperimentalGptLiveContextMirrorHost::new(
        runtime.runtime_adapter(),
        member_host.clone(),
        authority.clone(),
        coordinator,
    );
    runtime
        .runtime_adapter()
        .set_member_live_host(member_host.clone());
    mobs.set_member_live_host(member_host.clone());
    let binder = authority
        .bound_ready_binder_for(context_host, live_host, Arc::new(UnmeasuredPublisher))
        .ok_or("replay host has no complete WebRTC answer binder")?;
    Ok(ReplayHost {
        _temp: temp,
        rpc,
        _server: server,
        session_runtime: Arc::clone(&runtime),
        mobs,
        mob_id,
        member_host,
        authority,
        transport: public_transport,
        binder,
        session_id,
    })
}

impl ReplayHost {
    /// Open one channel without a browser, inside the journal's capture and
    /// recorder scopes (the same hooks a live run installs).
    async fn connect(
        &self,
        evidence: &Journal,
    ) -> Result<(u32, OpenChannel), Box<dyn std::error::Error>> {
        let channel = evidence.next_channel()?;
        let events = self
            .session_runtime
            .subscribe_session_events(&self.session_id)
            .await?;
        let open = async {
            let pending = self
                .member_host
                .open_with_execution_identity(
                    self.authority.as_ref(),
                    &self.session_id,
                    &serde_json::from_value(execution_identity(
                        GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
                    ))?,
                    None,
                    None,
                    Some(LiveOpenTransport::Webrtc),
                )
                .await?;
            let WireLiveTransportBootstrap::Webrtc { token, .. } = &pending.open().transport else {
                return Err::<(LiveChannelId, String, String), Box<dyn std::error::Error>>(
                    "replay requires the WebRTC bootstrap".into(),
                );
            };
            let readiness = self
                .member_host
                .register_experimental_live_playback_owner(
                    pending.channel_id(),
                    pending.pending_receipt(),
                )
                .await?;
            let answer = self
                .member_host
                .answer_experimental_live_webrtc_offer(
                    self.transport.clone(),
                    self.binder.clone(),
                    pending.channel_id().clone(),
                    pending.pending_receipt(),
                    readiness.readiness_receipt(),
                    token.clone(),
                    OFFER_SDP.to_string(),
                )
                .await?;
            answer.delivery_custody.delivered().await?;
            let custody = self
                .member_host
                .validate_experimental_live_channel_custody(
                    pending.channel_id(),
                    pending.pending_receipt(),
                )
                .await?;
            let Some(activation_receipt) = custody.phase().activation_receipt() else {
                return Err("the replayed answer did not activate the pending channel".into());
            };
            Ok((
                pending.channel_id().clone(),
                pending.pending_receipt().to_string(),
                activation_receipt.to_string(),
            ))
        };
        let (id, pending_receipt, activation_receipt) = evidence
            .provider_recording(channel)
            .scope(evidence.wire(channel).scope(open))
            .await?;
        evidence.require_attached(channel)?;
        evidence.channel(channel, evidence::ChannelAction::Connected)?;
        Ok((
            channel,
            OpenChannel {
                id,
                pending_receipt,
                activation_receipt,
                events,
            },
        ))
    }

    /// The channel was closed (by the provider at the recording's
    /// disconnect, or by the host): wait for the committed close's
    /// `AgentEvent::LiveChannelClosed` for this exact channel on the
    /// session's event stream, then read its custody once, which must be
    /// Closed. Any read failure fails the replay. The bound only reports a
    /// close that never committed.
    async fn closed(&self, open: &mut OpenChannel) -> Result<(), Box<dyn std::error::Error>> {
        use futures::StreamExt as _;
        let channel = open.id.as_str().to_owned();
        let committed = timeout(CONVERGENCE_BOUND, async {
            while let Some(envelope) = open.events.next().await {
                if let meerkat_core::AgentEvent::LiveChannelClosed { channel_id, .. } =
                    &envelope.payload
                    && channel_id == &channel
                {
                    return true;
                }
            }
            false
        })
        .await;
        match committed {
            Ok(true) => {}
            Ok(false) => {
                return Err("the session event stream ended before the channel closed".into());
            }
            Err(_) => return Err(format!("no LiveChannelClosed for channel {channel}").into()),
        }
        let custody = self
            .member_host
            .validate_experimental_live_channel_custody(&open.id, &open.pending_receipt)
            .await?;
        if custody.phase() != &ExperimentalLiveChannelPhaseStatus::Closed {
            return Err(format!(
                "custody after the committed close is {:?}, not Closed",
                custody.phase()
            )
            .into());
        }
        Ok(())
    }

    /// The host closes the channel, as the recorded run's harness did when
    /// the provider did not end it after the browser left (the recording
    /// shows the client's mute right after the disconnect step).
    async fn host_close(&self, open: &OpenChannel) -> Result<(), Box<dyn std::error::Error>> {
        self.member_host
            .close_experimental_live_active_channel(
                self.authority.as_ref(),
                &open.id,
                &open.activation_receipt,
            )
            .await?;
        Ok(())
    }

    /// The session is still the executor member's own text session: its
    /// model and provider, and the member's current session.
    async fn assert_executor_identity(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let session = self
            .rpc
            .call("session/read", json!({"session_id":self.session_id}), 30)
            .await?;
        assert_eq!(session["session_id"], json!(self.session_id));
        assert_eq!(session["model"], EXECUTOR_MODEL);
        assert_eq!(session["provider"], "openai");
        let member = self
            .rpc
            .call(
                "mob/member_status",
                json!({"mob_id":self.mob_id,"agent_identity":"voice-executor"}),
                30,
            )
            .await?;
        assert_eq!(member["current_session_id"], json!(self.session_id));
        Ok(())
    }

    /// The committed rows of the executor's session, by their typed fields.
    async fn history_rows(&mut self) -> Result<Vec<HistoryRow>, Box<dyn std::error::Error>> {
        let history = self.rpc.session_history(json!(self.session_id), 30).await?;
        Ok(history_rows(&history))
    }

    /// Every live delegation the session's machine recorded: who ran it,
    /// under which ownership, and its typed terminal.
    async fn delegations(&self) -> Result<Vec<DelegationRecord>, Box<dyn std::error::Error>> {
        let snapshots = self
            .session_runtime
            .runtime_adapter()
            .live_delegation_recovery_snapshots(&self.session_id)
            .await?;
        Ok(snapshots
            .iter()
            .map(|snapshot| DelegationRecord {
                worker_identity: snapshot.worker_identity().to_string(),
                ownership: snapshot.worker_ownership(),
                terminal: snapshot.terminal(),
            })
            .collect())
    }

    /// The titles of the WorkGraph items of the executor's mob, terminal
    /// ones included: voice delegations are scheduled through the WorkGraph.
    async fn work_item_titles(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let service = self
            .mobs
            .workgraph_service_for_mob(&meerkat_mob::MobId::from(self.mob_id.as_str()))?
            .ok_or("the mob state has no WorkGraph service")?;
        let items = service
            .list(meerkat::WorkItemFilter {
                include_terminal: true,
                ..Default::default()
            })
            .await?;
        Ok(items.into_iter().map(|item| item.title).collect())
    }
}

/// One live delegation as the session's machine recorded it.
#[derive(Debug, PartialEq, Eq)]
struct DelegationRecord {
    worker_identity: String,
    ownership: LiveDelegationWorkerOwnership,
    terminal: Option<LiveDelegationWorkerTerminalKind>,
}

/// One committed row of `session/history`, read by its typed wire fields.
#[derive(Debug)]
struct HistoryRow {
    role: String,
    /// `transcript_role` is `injected_context`: a row the runtime authored
    /// (a merged post-close result), not something the user said or typed.
    injected_context: bool,
    text: String,
}

impl HistoryRow {
    /// An assistant row, in either wire form (plain text or blocks).
    fn is_assistant(&self) -> bool {
        self.role == "assistant" || self.role == "block_assistant"
    }
}

fn history_rows(history: &Value) -> Vec<HistoryRow> {
    fn text_of(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    match inner {
                        Value::String(text) if key == "text" || key == "content" => {
                            out.push(text.clone());
                        }
                        _ => text_of(inner, out),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| text_of(item, out)),
            _ => {}
        }
    }
    history["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|message| {
            let mut text = Vec::new();
            text_of(message, &mut text);
            HistoryRow {
                role: message["role"].as_str().unwrap_or_default().to_owned(),
                injected_context: message["transcript_role"].as_str() == Some("injected_context"),
                text: text.join("\n"),
            }
        })
        .collect()
}

/// Lowercased words only: punctuation and casing differ between the
/// provider's transcript deltas and the committed rows.
fn words(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn client_events_by_channel(lines: &[provider_recording::Line]) -> BTreeMap<u32, Vec<ClientKey>> {
    let mut by_channel: BTreeMap<u32, Vec<ClientKey>> = BTreeMap::new();
    for line in lines {
        if let provider_recording::Entry::ClientEvent { event } = &line.entry {
            by_channel
                .entry(line.channel_ordinal)
                .or_default()
                .push(ClientKey::of(event));
        }
    }
    by_channel
}

/// The recorded executor reply a runtime-work append carried: its JSON row's
/// `text`, after the runtime-work prefix line.
fn runtime_work_reply(fixture: &Fixture, channel: u32) -> Option<String> {
    let content: String = fixture
        .client_contents(channel, "session.thinking.append")
        .concat();
    let row = content
        .strip_prefix(LIVE_RUNTIME_WORK_PREFIX)?
        .trim_start_matches('\n');
    // A post-close merge reply leads with its voice request's framing line
    // (`post_close_result_context`) before the row.
    let row = match row.split_once('\n') {
        Some((framing, rest)) if framing.starts_with("Finished voice request: \"") => rest,
        _ => row,
    };
    let value: Value = serde_json::from_str(row).ok()?;
    value["text"].as_str().map(str::to_owned)
}

// --- tests ------------------------------------------------------------------

/// One replay per process at a time: every live open reserves the whole
/// process realtime-projection budget, so concurrent replays in one test
/// binary (`cargo test` threads; nextest already isolates) would refuse
/// each other's opens.
static REPLAY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Committed fixtures carry no credential, SDP, host path or voice audio
/// (the same rules as `scripts/gpt-live-scrub-provider-stream check`).
/// The broker's duck-release burst gap is the browser peer's end hysteresis:
/// the gap the talk-over oracle uses to join voiced frames into one burst.
/// One value, never two tunables that drift apart.
#[test]
fn the_duck_release_burst_gap_is_the_peer_end_hysteresis() {
    const BROKER: &str = include_str!("../../../crates/meerkat-openai/src/public_live.rs");
    const PEER: &str = include_str!("../../live_smoke/browser/harness/gpt-live-peer-e2e.mjs");
    let value_after = |source: &str, marker: &str| -> u64 {
        let start = source
            .find(marker)
            .unwrap_or_else(|| panic!("`{marker}` is defined"))
            + marker.len();
        source[start..]
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit() || *c == '_')
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or_else(|_| panic!("`{marker}` has a numeric value"))
    };
    assert_eq!(
        value_after(BROKER, "const OUTPUT_BURST_GAP_MS: u64 ="),
        value_after(PEER, "end_hysteresis_ms:"),
        "the broker's burst gap must equal the peer's end hysteresis"
    );
}

#[test]
fn replay_fixtures_are_scrubbed() {
    for (name, text) in FIXTURES {
        let findings = fixture_findings(text);
        assert!(
            findings.is_empty(),
            "{name} fixture is not scrubbed: {findings:?}"
        );
    }
}

/// A recorded open's summary is read from the tape: seeded from the create
/// request's developer item, late from the thinking append (all of its
/// fragments) when the create request has none, absent otherwise.
#[test]
fn recorded_summary_follows_each_recorded_open() {
    let line = |seq: u64, channel: u32, entry: provider_recording::Entry| {
        serde_json::to_string(&provider_recording::Line {
            seq,
            channel_ordinal: channel,
            elapsed_ms: 0,
            entry,
        })
        .unwrap()
    };
    let create = |input: Value| provider_recording::Entry::CreateRequest {
        body: json!({"session": {"input": input}}),
    };
    let created = || provider_recording::Entry::CreateResponse {
        body: json!({"session": {"id": "s"}}),
    };
    let thinking = |id: &str, content: &str| provider_recording::Entry::ClientEvent {
        event: json!({"type": "session.thinking.append", "event_id": id, "content": content}),
    };
    let seeded_item = json!([{"type": "message", "role": "developer", "content": [{
        "type": "input_text",
        "text": "Factual summary of the background agent's context at voice-channel open (context data, not a new user request):\nThe venue is Lisbon."
    }]}]);
    let recent_only = json!([{"type": "message", "role": "user", "content": [{
        "type": "input_text", "text": "Typed while the voice call is down: the code is Kestrel."
    }]}]);
    let text = [
        line(1, 1, create(seeded_item)),
        line(2, 1, created()),
        line(3, 2, create(recent_only.clone())),
        line(4, 2, created()),
        line(
            5,
            2,
            thinking(
                "meerkat-thinking-1-0",
                &format!("{LIVE_LATE_SUMMARY_PREFIX}\nThe venue is Lis"),
            ),
        ),
        line(
            6,
            2,
            thinking("meerkat-thinking-1-1", "bon. The code is Kestrel."),
        ),
        line(
            7,
            2,
            thinking(
                "meerkat-thinking-2-0",
                "Earlier in this call, replayed after the summary",
            ),
        ),
        line(8, 3, create(recent_only)),
        line(9, 3, created()),
    ]
    .join("\n");
    let fixture = Fixture::parse(&text).unwrap();
    assert_eq!(
        fixture.recorded_summary(0, LIVE_LATE_SUMMARY_PREFIX),
        RecordedSummary::Seeded("The venue is Lisbon.".to_owned())
    );
    assert_eq!(
        fixture.recorded_summary(1, LIVE_LATE_SUMMARY_PREFIX),
        RecordedSummary::Late("The venue is Lisbon. The code is Kestrel.".to_owned()),
        "a late summary joins its fragments and leaves the causal replay out"
    );
    assert_eq!(
        fixture.recorded_summary(2, LIVE_LATE_SUMMARY_PREFIX),
        RecordedSummary::Absent
    );
    assert_eq!(
        fixture.recorded_summary(3, LIVE_LATE_SUMMARY_PREFIX),
        RecordedSummary::Absent,
        "an open past the recording"
    );
}

/// One delegated job of a recording, in creation order (the order the
/// scripted worker sees `Purpose::DelegatedJob(n)`).
#[derive(Debug, Clone)]
struct RecordedJob {
    channel: u32,
    /// The job's "Started voice request" narration on its channel.
    started: Option<ClientKey>,
    /// Its result reached this channel (a "Finished voice request"
    /// narration followed on it); otherwise the channel went down first and
    /// the result merged after it.
    delivered_on_channel: bool,
    /// The result text the channel received, when it was delivered there.
    result: Option<String>,
}

/// Every client delegation of the recording with where its narrations and
/// result landed, joined by the provider delegation id that the commentary
/// appends carry.
fn recorded_jobs(fixture: &Fixture) -> Vec<RecordedJob> {
    let mut jobs: Vec<(String, RecordedJob)> = Vec::new();
    for line in &fixture.lines {
        match &line.entry {
            provider_recording::Entry::ServerFrame { raw }
                if raw["type"] == "session.delegation.created"
                    && raw["delegation"]["target"] == "client" =>
            {
                if let Some(id) = raw["delegation"]["id"].as_str() {
                    jobs.push((
                        id.to_owned(),
                        RecordedJob {
                            channel: line.channel_ordinal,
                            started: None,
                            delivered_on_channel: false,
                            result: None,
                        },
                    ));
                }
            }
            provider_recording::Entry::ClientEvent { event }
                if event["type"] == "session.commentary.append" =>
            {
                let (Some(id), Some(content)) =
                    (event["delegation_id"].as_str(), event["content"].as_str())
                else {
                    continue;
                };
                let Some((_, job)) = jobs.iter_mut().find(|(job_id, _)| job_id == id) else {
                    continue;
                };
                if content.starts_with("Started voice request") {
                    job.started = Some(ClientKey::of(event));
                } else if content.starts_with("Finished voice request") {
                    job.delivered_on_channel = true;
                } else if job.delivered_on_channel && job.result.is_none() {
                    job.result = Some(content.to_owned());
                }
            }
            _ => {}
        }
    }
    jobs.into_iter().map(|(_, job)| job).collect()
}

/// The scenario-specific steps a recording cannot carry: typed turns the
/// recorded test made between channels, and a gate the recorded run's
/// timing set on the merge of a post-close result.
struct ReplayScript {
    typed_after_channel: BTreeMap<u32, &'static str>,
    merge_gate_on_channel_open: Option<(u32, watch::Sender<bool>)>,
    /// After this channel closed and released its held jobs, the next
    /// channel opens only once the scripted LLM received this call: the
    /// typed sign that those jobs reached the state the recorded run had at
    /// its reopen (S104: the merged job's reply turn has started).
    reopen_after_call: BTreeMap<u32, Purpose>,
}

/// The owner's append lanes around one replayed open. `at_first_step` is
/// read while the tape stands at the channel's first test step: every frame
/// the provider sent before the channel's first user turn has been served,
/// and no user turn has begun.
#[derive(Clone, Copy, Debug)]
struct OpenObservation {
    channel: u32,
    before_open: evidence::OwnerAppends,
    at_first_step: evidence::OwnerAppends,
}

/// Drive a recording: each channel opens, its markers are stepped in
/// recorded order (a disconnect followed by a recorded client mute is the
/// host's close), and each delegated job is released once its channel
/// carried its "Started" narration when its result was delivered there, or
/// once its channel closed when it was not. Returns the append lanes at
/// each open.
async fn drive_replay(
    llm: &ScriptedLlm,
    host: &mut ReplayHost,
    cassette: Arc<Cassette>,
    evidence: &Journal,
    jobs: &[RecordedJob],
    job_gates: Vec<watch::Sender<bool>>,
    script: ReplayScript,
) -> Result<Vec<OpenObservation>, Box<dyn std::error::Error>> {
    let mut gate_openers = Vec::new();
    let mut held_until_close: BTreeMap<u32, Vec<watch::Sender<bool>>> = BTreeMap::new();
    for (job, gate) in jobs.iter().zip(job_gates) {
        match (&job.started, job.delivered_on_channel) {
            (Some(started), true) => {
                let cassette = Arc::clone(&cassette);
                let started = started.clone();
                let channel = job.channel;
                gate_openers.push(tokio::spawn(async move {
                    if cassette.received(channel, &started).await.is_ok() {
                        gate.send_replace(true);
                    }
                }));
            }
            _ => held_until_close.entry(job.channel).or_default().push(gate),
        }
    }
    let mut merge_gate = script.merge_gate_on_channel_open;
    let mut opens = Vec::new();
    for tape in &cassette.fixture().channels {
        let before_open = evidence.owner_appends()?;
        let (channel, mut open) = host.connect(evidence).await?;
        if channel != tape.ordinal {
            return Err(format!("opened channel {channel}, the tape is {}", tape.ordinal).into());
        }
        if let Some((gated, gate)) = merge_gate.take() {
            if gated == channel {
                gate.send_replace(true);
            } else {
                merge_gate = Some((gated, gate));
            }
        }
        let markers = tape.markers();
        if let Some(first) = markers.first() {
            cassette.reached(channel, first).await?;
        }
        opens.push(OpenObservation {
            channel,
            before_open,
            at_first_step: evidence.owner_appends()?,
        });
        for step in markers {
            cassette.release(channel, &step).await?;
            if step.starts_with("disconnect") && tape.host_closes_after(&step) {
                cassette.at_host_close(channel).await?;
                host.host_close(&open).await?;
            }
        }
        cassette.ended(channel).await?;
        host.closed(&mut open).await?;
        // The recorded test typed during the closure, before the jobs the
        // close left running finished (their merge then waits behind it).
        if let Some(prompt) = script.typed_after_channel.get(&channel) {
            let typed = host
                .rpc
                .call_raw(
                    "turn/start",
                    json!({"session_id":host.session_id,"prompt":prompt}),
                    120,
                )
                .await?;
            if !typed["error"].is_null() {
                return Err(format!("the typed turn failed: {}", typed["error"]).into());
            }
        }
        for gate in held_until_close.remove(&channel).unwrap_or_default() {
            gate.send_replace(true);
        }
        if let Some(purpose) = script.reopen_after_call.get(&channel) {
            llm.wait_for_call(*purpose).await?;
        }
    }
    for opener in gate_openers {
        opener.abort();
    }
    Ok(opens)
}

/// The replay's own recorded client events, per channel, must equal the
/// fixture's (type and deterministic `event_id`, in order).
fn assert_recorded_client_events(
    evidence: &Journal,
    fixture: &Fixture,
    llm: &ScriptedLlm,
) -> Result<(), Box<dyn std::error::Error>> {
    let replayed = provider_recording::read(
        &evidence
            .path()
            .with_file_name(support::evidence::PROVIDER_STREAM_FILE),
    )?;
    // A channel that sent no client event has no entry on either side.
    let recorded: BTreeMap<u32, Vec<ClientKey>> = fixture
        .channels
        .iter()
        .filter(|tape| !tape.client_events.is_empty())
        .map(|tape| (tape.ordinal, tape.client_events.clone()))
        .collect();
    // A host close sends the mute, then `session.close`. When the recorded
    // run's transport was already gone, the mute's send failed and the close
    // was never attempted, so the recording ends at the mute; the replay's
    // transport is alive and sends both. That trailing close is the only
    // difference allowed.
    let mut replayed = client_events_by_channel(&replayed);
    for (ordinal, events) in &mut replayed {
        let recorded_ends_at_mute = recorded
            .get(ordinal)
            .and_then(|recorded| recorded.last())
            .is_some_and(|last| last.kind == "session.input_audio.mute");
        if recorded_ends_at_mute
            && events
                .last()
                .is_some_and(|last| last.kind == "session.close")
        {
            events.pop();
        }
    }
    assert_eq!(
        replayed,
        recorded,
        "the replay's client events (type, event_id) differ from the recording; LLM calls: {:?}",
        llm.calls.lock().expect("calls")
    );
    Ok(())
}

fn scripted_llm(scripts: BTreeMap<Purpose, Script>) -> Arc<ScriptedLlm> {
    Arc::new(ScriptedLlm {
        scripts,
        conversations: AtomicUsize::new(0),
        jobs: AtomicUsize::new(0),
        calls: std::sync::Mutex::new(Vec::new()),
        called: watch::channel(0).0,
    })
}

/// Job scripts from the recording: a delivered job answers with the result
/// its channel received; each waits on its own gate.
fn job_scripts(
    jobs: &[RecordedJob],
    undelivered_answer: &str,
) -> (BTreeMap<Purpose, Script>, Vec<watch::Sender<bool>>) {
    let mut scripts = BTreeMap::new();
    let mut gates = Vec::new();
    for (index, job) in jobs.iter().enumerate() {
        let (gate, gate_rx) = watch::channel(false);
        gates.push(gate);
        scripts.insert(
            Purpose::DelegatedJob(index),
            Script {
                text: job
                    .result
                    .clone()
                    .unwrap_or_else(|| undelivered_answer.to_owned()),
                gate: Some(gate_rx),
            },
        );
    }
    (scripts, gates)
}

fn conversation(text: &str) -> Script {
    Script {
        text: text.to_owned(),
        gate: None,
    }
}

/// One finished replay, with everything a row reads afterwards: the tape,
/// the replay's own journal, the scripted LLM, the host (its session, mob
/// and machine), and the append lanes at each open.
struct Replayed {
    fixture: Fixture,
    cassette: Arc<Cassette>,
    evidence: Journal,
    llm: Arc<ScriptedLlm>,
    host: ReplayHost,
    opens: Vec<OpenObservation>,
    /// The scripted answer of a job whose result its channel never carried
    /// (the channel went down first), and of the source member's reply to
    /// that result once it merged.
    post_close_result: String,
    _one_at_a_time: tokio::sync::MutexGuard<'static, ()>,
}

const S104_SEED_REPLY: &str = "Acknowledged: the team mascot is a heron named Bartleby.";
const S104_TYPED_REPLY: &str = "Remembered: the meeting room is called Osprey.";
const S106_SEED_REPLY: &str = "Acknowledged: the sponsor's name is Marlow.";
const S106_TYPED_REPLY: &str = "Acknowledged: the budget code is Kestrel.";

/// S104 replayed to its end: open with a concurrent bootstrap summary, a
/// voice job on a DurableFork worker, the channel dropped while it runs, a
/// typed turn during the closure, the reopen, and the job merged into the
/// source with its reply replayed on the reopened channel as quiet runtime
/// work.
async fn replay_s104() -> Result<Replayed, Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let one_at_a_time = REPLAY_LOCK.lock().await;
    let fixture = Fixture::parse(S104_FIXTURE)?;
    let merge_reply = runtime_work_reply(&fixture, 2)
        .ok_or("the S104 fixture carries no runtime-work append on channel 2")?;
    let jobs = recorded_jobs(&fixture);
    let cassette = Arc::new(Cassette::start(fixture.clone()).await?);
    let evidence = Journal::create_for("S104-replay", S104_RESULT_TOKEN.to_owned())?;

    let (mut scripts, job_gates) = job_scripts(&jobs, &merge_reply);
    scripts.insert(Purpose::Conversation(0), conversation(S104_SEED_REPLY));
    scripts.insert(Purpose::Conversation(1), conversation(S104_TYPED_REPLY));
    // The recorded run merged the post-close job only after channel 2 was up.
    let (merge_gate, merge_gate_rx) = watch::channel(false);
    scripts.insert(
        Purpose::MergeReply,
        Script {
            text: merge_reply.clone(),
            gate: Some(merge_gate_rx),
        },
    );
    let llm = scripted_llm(scripts);
    let mut host = open_replay_host(HostOptions {
        base_url: cassette.base_url(),
        llm: llm.clone(),
        policy: LiveDelegationExecutionPolicy::DurableFork,
        executor_instructions: S104_EXECUTOR_INSTRUCTIONS,
        seed_prompt: Some(S104_SEED_PROMPT),
        summary: "The team mascot is a heron named Bartleby.",
        summary_gate: cassette.summary_gate(),
    })
    .await?;
    let result = drive_replay(
        &llm,
        &mut host,
        Arc::clone(&cassette),
        &evidence,
        &jobs,
        job_gates,
        ReplayScript {
            typed_after_channel: BTreeMap::from([(1, S104_TYPED_PROMPT)]),
            merge_gate_on_channel_open: Some((2, merge_gate)),
            reopen_after_call: BTreeMap::from([(1, Purpose::MergeReply)]),
        },
    )
    .await;
    let finished = evidence.finish(match &result {
        Ok(_) => evidence::Outcome::Passed,
        Err(_) => evidence::Outcome::Failed,
    });
    cassette.diverged()?;
    let opens = result.map_err(|error| {
        format!(
            "{error}; scripted LLM calls so far: {:?}",
            llm.calls.lock().expect("calls")
        )
    })?;
    finished?;
    Ok(Replayed {
        fixture,
        cassette,
        evidence,
        llm,
        host,
        opens,
        post_close_result: merge_reply,
        _one_at_a_time: one_at_a_time,
    })
}

/// S104 replayed: the replay sends exactly the recorded client events, each
/// open seeds the recorded shape, and the merged job's reply reaches the
/// reopened channel as quiet runtime work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s104_replay_sends_the_recorded_client_events() -> Result<(), Box<dyn std::error::Error>> {
    let replayed = replay_s104().await?;
    let Replayed {
        fixture,
        cassette,
        evidence,
        llm,
        ..
    } = &replayed;

    // Each channel opened with the recorded seed shape (item roles of the
    // startup history; texts follow from the scripted LLM).
    let seed_roles = |body: &Value| -> Vec<String> {
        body["session"]["input"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["role"].as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let created: Vec<Vec<String>> = cassette.create_bodies().iter().map(seed_roles).collect();
    let recorded_seeds: Vec<Vec<String>> = fixture
        .channels
        .iter()
        .map(|tape| seed_roles(&tape.create_request))
        .collect();
    assert_eq!(
        created, recorded_seeds,
        "the replayed opens' seed shapes differ"
    );
    assert_recorded_client_events(evidence, fixture, llm)?;
    // The S104 contract on the reopened channel: the merged job's reply is
    // runtime work on the quiet lane, carrying the job's result.
    let runtime_work: Vec<String> = evidence
        .owned_thinking_appends(2)?
        .into_iter()
        .filter(|text| text.starts_with(LIVE_RUNTIME_WORK_PREFIX))
        .collect();
    assert!(
        runtime_work
            .iter()
            .any(|text| text.contains(S104_RESULT_TOKEN)),
        "the merged job's reply must reach channel 2 as runtime work: {runtime_work:?}"
    );
    Ok(())
}

/// S106 replayed to its end: three channels over one ExistingMember
/// executor, native exchanges, delegated jobs narrated Started, Finished and
/// their result on the delegation lane, a typed turn during the first
/// closure, and reopens seeded from the retained summary.
async fn replay_s106() -> Result<Replayed, Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let one_at_a_time = REPLAY_LOCK.lock().await;
    let fixture = Fixture::parse(S106_FIXTURE)?;
    let jobs = recorded_jobs(&fixture);
    let cassette = Arc::new(Cassette::start(fixture.clone()).await?);
    let evidence = Journal::create_for("S106-replay", "Saffron".to_owned())?;

    let (mut scripts, job_gates) = job_scripts(&jobs, "Done.");
    scripts.insert(Purpose::Conversation(0), conversation(S106_SEED_REPLY));
    scripts.insert(Purpose::Conversation(1), conversation(S106_TYPED_REPLY));
    let llm = scripted_llm(scripts);
    let mut host = open_replay_host(HostOptions {
        base_url: cassette.base_url(),
        llm: llm.clone(),
        policy: LiveDelegationExecutionPolicy::ExistingMember,
        executor_instructions: S106_EXECUTOR_INSTRUCTIONS,
        seed_prompt: Some(S106_SEED_PROMPT),
        summary: "The sponsor's name is Marlow. The project codename is Saffron. The launch venue is Lisbon. The budget code is Kestrel.",
        summary_gate: cassette.summary_gate(),
    })
    .await?;
    let result = drive_replay(
        &llm,
        &mut host,
        Arc::clone(&cassette),
        &evidence,
        &jobs,
        job_gates,
        ReplayScript {
            typed_after_channel: BTreeMap::from([(1, S106_TYPED_PROMPT)]),
            merge_gate_on_channel_open: None,
            reopen_after_call: BTreeMap::new(),
        },
    )
    .await;
    let finished = evidence.finish(match &result {
        Ok(_) => evidence::Outcome::Passed,
        Err(_) => evidence::Outcome::Failed,
    });
    cassette.diverged()?;
    let opens = result.map_err(|error| {
        format!(
            "{error}; scripted LLM calls so far: {:?}",
            llm.calls.lock().expect("calls")
        )
    })?;
    finished?;
    Ok(Replayed {
        fixture,
        cassette,
        evidence,
        llm,
        host,
        opens,
        post_close_result: "Done.".to_owned(),
        _one_at_a_time: one_at_a_time,
    })
}

/// S106 replayed: the replay sends exactly the recorded client events, and
/// each open carries its summary as the recorded open did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s106_replay_sends_the_recorded_client_events() -> Result<(), Box<dyn std::error::Error>> {
    let replayed = replay_s106().await?;
    let Replayed {
        fixture,
        cassette,
        evidence,
        llm,
        ..
    } = &replayed;

    // Each open carries the summary first, as its recorded open did: seeded
    // as the first startup item, or (not ready within the pre-open bound)
    // absent from the seed and delivered as the thinking append the client
    // event comparison below pins.
    for (index, body) in cassette.create_bodies().iter().enumerate() {
        let seeded = body["session"]["input"][0]["role"] == "developer";
        match fixture.recorded_summary(index, LIVE_LATE_SUMMARY_PREFIX) {
            RecordedSummary::Seeded(_) => assert!(
                seeded,
                "S106 open #{} is seeded with the summary first: {body}",
                index + 1
            ),
            RecordedSummary::Late(_) => assert!(
                !seeded,
                "S106 open #{} was recorded with a late summary and must not seed one: {body}",
                index + 1
            ),
            RecordedSummary::Absent => {
                panic!("S106 open #{} carries no recorded summary", index + 1)
            }
        }
    }
    assert_recorded_client_events(evidence, fixture, llm)?;
    Ok(())
}

// --- the scenarios' product checks, over the recordings -----------------------
//
// The rows below state checks of the live scenarios S104 and S106 that are
// product behaviour over a given provider stream. They read the product's
// own records (the journal's capture of what the owner sent, the session's
// committed rows, the delegation records of the session's machine, the
// WorkGraph), by typed fields where the product has them.

/// The startup input of a recorded open: its item count and how many of the
/// items are developer items (the seeded summary).
fn recorded_seed_shape(tape: &replay::ChannelTape) -> (usize, usize) {
    let items = tape.create_request["session"]["input"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    (
        items.len(),
        items
            .iter()
            .filter(|item| item["role"] == "developer")
            .count(),
    )
}

/// The framing line of the summary item in a create body: the first line of
/// the developer item's text, which says what the summary covers.
fn summary_framing(create_body: &Value) -> Option<String> {
    create_body["session"]["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["role"] == "developer")
        .filter_map(|item| item["content"][0]["text"].as_str())
        .find_map(|text| text.split_once('\n').map(|(framing, _)| framing.to_owned()))
}

/// The open rule of a session that opens with a summary (S104 C3, C13, C18
/// and C22; S106 C3, C12 to C15, C19 and C26), over one finished replay:
/// - no owned append of either lane before the channel's first user turn:
///   a seeded summary rides the startup input, a late one waits for the
///   first utterance;
/// - the startup input has the recorded shape and frames the history, and
///   its summary item is framed as recorded (generated for this open, or
///   retained from an earlier channel);
/// - a late summary is the channel's first owned thinking append, and a
///   reopened channel never receives a summary append at all;
/// - every instructions fragment the owner sent was acknowledged.
fn assert_open_rule(replayed: &Replayed) -> Result<(), Box<dyn std::error::Error>> {
    let Replayed {
        fixture,
        evidence,
        opens,
        ..
    } = replayed;
    assert_eq!(
        opens.len(),
        fixture.channels.len(),
        "every recorded channel opened"
    );
    for (index, (open, tape)) in opens.iter().zip(&fixture.channels).enumerate() {
        let channel = open.channel;
        assert_eq!(
            (
                open.at_first_step.instructions_attempts,
                open.at_first_step.thinking_attempts
            ),
            (
                open.before_open.instructions_attempts,
                open.before_open.thinking_attempts
            ),
            "channel {channel} used an append lane (instructions, thinking) before its first user turn"
        );
        let seed = evidence
            .session_input_seed(channel)?
            .ok_or_else(|| format!("no startup seed was captured for channel {channel}"))?;
        assert_eq!(
            (seed.input_items, seed.developer_items),
            recorded_seed_shape(tape),
            "channel {channel} seeds the recorded shape (items, developer items)"
        );
        assert!(
            seed.frames_history,
            "the startup instructions of channel {channel} frame the history"
        );
        let reopened = index > 0;
        let summary_appends = evidence
            .owned_thinking_appends(channel)?
            .iter()
            .filter(|append| append.starts_with(LIVE_LATE_SUMMARY_PREFIX))
            .count();
        match fixture.recorded_summary(index, LIVE_LATE_SUMMARY_PREFIX) {
            RecordedSummary::Seeded(_) => {
                assert_eq!(
                    seed.developer_items, 1,
                    "a seeded open carries exactly one summary item (channel {channel})"
                );
                assert_eq!(
                    summary_framing(&replayed.cassette.create_bodies()[index]),
                    summary_framing(&tape.create_request),
                    "the summary item of channel {channel} is framed as recorded (a summary generated for this open, or one retained from an earlier channel)"
                );
                assert_eq!(
                    summary_appends, 0,
                    "a seeded open receives no summary append (channel {channel})"
                );
            }
            RecordedSummary::Late(_) => {
                assert!(
                    !reopened,
                    "a reopen's summary rides its startup input, never the append lane (channel {channel})"
                );
                assert_eq!(seed.developer_items, 0);
                let first = evidence.first_owned_thinking_append(channel)?;
                assert!(
                    first
                        .as_deref()
                        .is_some_and(|text| text.starts_with(LIVE_LATE_SUMMARY_PREFIX)),
                    "a late summary is the first owned thinking append of channel {channel}: {first:?}"
                );
            }
            RecordedSummary::Absent => {
                panic!("the recorded open of channel {channel} carries no summary")
            }
        }
    }
    let owner = evidence.owner_appends()?;
    assert_eq!(
        owner.instructions_acknowledged, owner.instructions_attempts,
        "every instructions fragment the owner sent was acknowledged"
    );
    Ok(())
}

/// S104 replayed, the open rule: see [`assert_open_rule`]. The reopen seeds
/// the summary retained from the first channel (S104 C18).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s104_replay_opens_append_nothing_before_the_first_user_turn()
-> Result<(), Box<dyn std::error::Error>> {
    let replayed = replay_s104().await?;
    assert_open_rule(&replayed)?;
    let reopen = replayed
        .evidence
        .session_input_seed(2)?
        .ok_or("no startup seed was captured for the reopen")?;
    assert!(
        reopen.preceding_history_summary,
        "the reopen seeds the retained summary and the rows committed since: {reopen:?}"
    );
    Ok(())
}

/// S106 replayed, the open rule over two reopen cycles: see
/// [`assert_open_rule`].
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s106_replay_reopens_seed_their_summary_and_append_nothing()
-> Result<(), Box<dyn std::error::Error>> {
    assert_open_rule(&replay_s106().await?)
}

/// S104 replayed, the reopen's contract (S104 C16 and C17): every row
/// committed before the voice session was created rides its startup input;
/// a row committed after it reaches the channel through the owner, as
/// runtime work replayed once the user's turn has ended. The note typed
/// during the closure is the first kind. The job's result is exactly one of
/// the two, and here the one the recording took.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s104_replay_reopen_seeds_the_typed_row_and_carries_the_job_result_once()
-> Result<(), Box<dyn std::error::Error>> {
    let replayed = replay_s104().await?;
    let Replayed {
        fixture, evidence, ..
    } = &replayed;
    let seed_texts = evidence.session_input_texts(2)?;
    assert!(
        seed_texts.iter().any(|text| text == S104_TYPED_PROMPT),
        "the note typed during the closure rides the reopen's startup input verbatim: {seed_texts:?}"
    );
    let result_in_seed = seed_texts
        .iter()
        .any(|text| text.contains(S104_MERGE_MARKER));
    let result_after_creation = evidence
        .owned_thinking_appends(2)?
        .iter()
        .any(|text| text.starts_with(LIVE_RUNTIME_WORK_PREFIX));
    assert!(
        result_in_seed != result_after_creation,
        "the job result reaches the reopened channel exactly once (in its startup input: {result_in_seed}; replayed after its creation: {result_after_creation})"
    );
    let recorded_in_seed = fixture.channels[1]
        .create_request
        .to_string()
        .contains(S104_MERGE_MARKER);
    assert_eq!(
        result_in_seed, recorded_in_seed,
        "the job result takes the side of the contract the recording took"
    );
    Ok(())
}

/// S104 replayed, the session's own record of the handoff (S104 C1, C5 to
/// C7, C9, C10 and C20):
/// - the session is still the executor member's own;
/// - the note typed during the closure is committed as an ordinary user row
///   with its reply;
/// - the job that outlived its channel is merged into the source session: an
///   injected-context row (by its typed transcript role, not by any row that
///   happens to hold a word) that carries the fork worker's result, followed
///   by the source member's reply to it;
/// - every delegation ended Completed on a worker the delegation owned.
///
/// Both closes are part of the replay itself: it fails unless each channel's
/// close committed and left its custody Closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s104_replay_commits_the_typed_turn_and_merges_the_job_result_into_the_source()
-> Result<(), Box<dyn std::error::Error>> {
    let mut replayed = replay_s104().await?;
    replayed.host.assert_executor_identity().await?;
    let rows = replayed.host.history_rows().await?;
    let reply_after = |index: usize| {
        rows[index + 1..]
            .iter()
            .find(|row| row.is_assistant())
            .map(|row| row.text.as_str())
    };

    let typed = rows
        .iter()
        .position(|row| {
            row.role == "user" && !row.injected_context && row.text == S104_TYPED_PROMPT
        })
        .ok_or_else(|| format!("the typed turn's user row is not committed: {rows:#?}"))?;
    assert_eq!(
        reply_after(typed),
        Some(S104_TYPED_REPLY),
        "the typed turn's reply follows its row"
    );

    let merged: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row.role == "user" && row.injected_context && row.text.contains(S104_MERGE_MARKER)
        })
        .map(|(index, _)| index)
        .collect();
    let [merge] = merged.as_slice() else {
        return Err(format!(
            "the post-close result is merged into the source exactly once, as injected context: {rows:#?}"
        )
        .into());
    };
    let result_start: String = replayed.post_close_result.chars().take(120).collect();
    assert!(
        rows[*merge].text.contains(&result_start),
        "the merge row carries the fork worker's result: {:?}",
        rows[*merge].text
    );
    assert_eq!(
        reply_after(*merge),
        Some(replayed.post_close_result.as_str()),
        "the source member's reply to the merged result follows it"
    );
    assert!(
        typed < *merge,
        "the typed turn was committed during the closure, before the merge"
    );

    let delegations = replayed.host.delegations().await?;
    assert_eq!(
        delegations.len(),
        recorded_jobs(&replayed.fixture).len(),
        "one delegation record per recorded job: {delegations:?}"
    );
    for delegation in &delegations {
        assert_eq!(
            (delegation.ownership, delegation.terminal),
            (
                LiveDelegationWorkerOwnership::OwnedMember,
                Some(LiveDelegationWorkerTerminalKind::Completed)
            ),
            "a DurableFork job ends Completed on a worker the delegation owns: {delegation:?}"
        );
    }
    Ok(())
}

/// The first words of the executor-input row of an ExistingMember job. The
/// product gives that row no typed role of its own, so it is told apart by
/// the runtime's prefix.
const DELEGATION_EXECUTION_CONTEXT_START: &str = "Live delegation execution context:";

/// S106 replayed, what a reopened channel's model is given at its start:
/// - the note typed while the call was down rides the next open's startup
///   input verbatim, as a row of its own (the reopen contract, as in S104);
/// - the final channel's startup input is the summary the summarizer
///   produced for that open, and nothing else.
///
/// The live scenario's check that every planted fact reaches the final
/// channel (S106 C18) is not stated here on purpose. On the final channel
/// the facts ride the summary's wording, and a replay's summarizer returns
/// the recorded summary: a row over them would pass whatever the session
/// held. What the product owns is that the summary it was given is what the
/// channel starts with, and that a typed row is carried verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s106_replay_reopen_seeds_the_typed_row_and_the_final_channel_its_summary()
-> Result<(), Box<dyn std::error::Error>> {
    let replayed = replay_s106().await?;
    let reopen = replayed.evidence.session_input_texts(2)?;
    assert!(
        reopen.iter().any(|text| text == S106_TYPED_PROMPT),
        "the note typed during the first closure rides the reopen's startup input verbatim: {reopen:?}"
    );
    let RecordedSummary::Seeded(summary) = replayed
        .fixture
        .recorded_summary(2, LIVE_LATE_SUMMARY_PREFIX)
    else {
        return Err("the recording's third open is not seeded with a summary".into());
    };
    let last = replayed.evidence.session_input_texts(3)?;
    let [only] = last.as_slice() else {
        return Err(
            format!("the final channel starts with its summary item alone: {last:?}").into(),
        );
    };
    assert!(
        only.ends_with(&summary),
        "the final channel's summary item carries the summary produced for that open: {only:?}"
    );
    Ok(())
}

/// S106 replayed, the session's own record of the call (S106 C1, C8 to C10,
/// C22 and C24):
/// - the session is still the executor member's own;
/// - the committed user rows that are neither runtime-authored context nor
///   an executor input carry exactly the words of the two typed turns and
///   of everything the provider heard on the three channels, in order
///   (words, not rows: where a row is cut is not part of the contract);
/// - both typed turns have their replies;
/// - every delegation ran on the existing member and ended Completed, and
///   each was scheduled through the WorkGraph.
///
/// The closes and reopens are part of the replay itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s106_replay_commits_exactly_the_heard_words_and_schedules_through_the_workgraph()
-> Result<(), Box<dyn std::error::Error>> {
    let mut replayed = replay_s106().await?;
    replayed.host.assert_executor_identity().await?;
    let rows = replayed.host.history_rows().await?;

    let fixture = &replayed.fixture;
    let heard = [
        S106_SEED_PROMPT.to_owned(),
        fixture.heard_user_text(1),
        S106_TYPED_PROMPT.to_owned(),
        fixture.heard_user_text(2),
        fixture.heard_user_text(3),
    ]
    .join(" ");
    let committed: Vec<&str> = rows
        .iter()
        .filter(|row| {
            row.role == "user"
                && !row.injected_context
                && !row.text.starts_with(DELEGATION_EXECUTION_CONTEXT_START)
        })
        .map(|row| row.text.as_str())
        .collect();
    assert_eq!(
        words(&committed.join(" ")),
        words(&heard),
        "the committed user rows carry exactly the typed turns and the heard words, in order"
    );
    for (prompt, reply) in [
        (S106_SEED_PROMPT, S106_SEED_REPLY),
        (S106_TYPED_PROMPT, S106_TYPED_REPLY),
    ] {
        let typed = rows
            .iter()
            .position(|row| row.role == "user" && !row.injected_context && row.text == prompt)
            .ok_or_else(|| format!("the typed row {prompt:?} is not committed"))?;
        assert_eq!(
            rows[typed + 1..]
                .iter()
                .find(|row| row.is_assistant())
                .map(|row| row.text.as_str()),
            Some(reply),
            "the typed turn's reply follows its row"
        );
    }

    let jobs = recorded_jobs(fixture).len();
    let delegations = replayed.host.delegations().await?;
    assert_eq!(
        delegations.len(),
        jobs,
        "one delegation record per recorded job: {delegations:?}"
    );
    for delegation in &delegations {
        assert_eq!(
            delegation,
            &DelegationRecord {
                worker_identity: "voice-executor".to_owned(),
                ownership: LiveDelegationWorkerOwnership::ExistingMember,
                terminal: Some(LiveDelegationWorkerTerminalKind::Completed),
            },
            "an ExistingMember job runs on the channel's own member and ends Completed"
        );
    }
    let titles = replayed.host.work_item_titles().await?;
    assert_eq!(
        titles.len(),
        jobs,
        "each voice delegation is one WorkGraph item, not the serial fallback: {titles:?}"
    );
    Ok(())
}
