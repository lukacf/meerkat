#![cfg(all(feature = "gpt-live-replay", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! Deterministic replays of recorded Turbo S gpt-live-1 runs (S104).
//!
//! Each test drives the same host graph as `gpt_live_public_e2e.rs` (RPC
//! server, mob, ExistingMember/DurableFork executor, the shared exact-receipt
//! live host with a concurrent bootstrap summary), with three substitutions:
//! - the provider is a [`Cassette`] serving a recorded, scrubbed provider
//!   stream (`tests/integration/fixtures/gpt_live_replay/`), gated causally
//!   on Meerkat's own client events and the test's steps, never on time;
//! - every executor/worker LLM call is a [`ScriptedLlm`] keyed by purpose,
//!   and the summary is a fixed [`ScriptedSummarizer`];
//! - there is no browser: the offer is a literal string, and the test steps
//!   the recording's markers (`play_at:*`, `disconnect:*`) itself.
//!
//! The replay records its own provider stream through the same journal hook
//! as a live run and must send exactly the recording's client events per
//! channel (type and deterministic `event_id`), in order.
//!
//! Re-capture: `scripts/gpt-live-recapture-replay-fixture S104` (see the
//! fixture README).

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
    GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID, GPT_LIVE_PUBLIC_MODEL, LIVE_RUNTIME_WORK_PREFIX,
    PublicGptLiveOpenAuthorityConfig, PublicGptLivePlaybackPolicy, provider_recording,
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
use serde_json::{Value, json};
use tokio::io::BufReader;
use tokio::sync::watch;
use tokio::time::{Duration, Instant, sleep};

use replay::{Cassette, ClientKey, Fixture, fixture_findings};
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
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
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

/// The bootstrap/reopen summary: fixed content, ready at once (the recorded
/// runs had their summary before each open, so neither carried a late one).
struct ScriptedSummarizer(String);

#[async_trait::async_trait]
impl LiveContextSummarizer for ScriptedSummarizer {
    async fn summarize(
        &self,
        _snapshot: LiveContextSummarySnapshot<'_>,
    ) -> Result<String, LiveContextSummaryError> {
        Ok(self.0.clone())
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
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    member_host: Arc<ServiceMemberLiveHost>,
    authority: Arc<ExperimentalGptLiveOpenAuthority>,
    transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    binder: Arc<dyn LiveWebrtcBoundReadyBinder>,
    session_id: meerkat_core::SessionId,
}

struct OpenChannel {
    id: LiveChannelId,
    pending_receipt: String,
}

struct HostOptions<'a> {
    base_url: &'a str,
    llm: Arc<ScriptedLlm>,
    policy: LiveDelegationExecutionPolicy,
    executor_instructions: &'a str,
    seed_prompt: Option<&'a str>,
    summary: &'a str,
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
    );
    let runtime = Arc::new(SessionRuntime::new_with_config_store(
        factory.clone(),
        config.clone(),
        Arc::clone(&config_store),
        16,
        persistence,
        NotificationSink::noop(),
    ));
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

    let mobs = meerkat_rpc::router::compose_rpc_mob_state(&runtime, &config_store, None);
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
            Arc::new(ScriptedSummarizer(options.summary.to_owned())),
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
        runtime: runtime.runtime_adapter(),
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
                return Err::<OpenChannel, Box<dyn std::error::Error>>(
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
            if custody.phase().activation_receipt().is_none() {
                return Err("the replayed answer did not activate the pending channel".into());
            }
            Ok(OpenChannel {
                id: pending.channel_id().clone(),
                pending_receipt: pending.pending_receipt().to_string(),
            })
        };
        let opened = evidence
            .provider_recording(channel)
            .scope(evidence.wire(channel).scope(open))
            .await?;
        evidence.require_attached(channel)?;
        evidence.channel(channel, evidence::ChannelAction::Connected)?;
        Ok((channel, opened))
    }

    /// The provider closed the channel (the recording's disconnect): wait
    /// for the exact channel's custody to converge to Closed.
    async fn closed(&self, open: &OpenChannel) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + CONVERGENCE_BOUND;
        loop {
            let custody = self
                .member_host
                .validate_experimental_live_channel_custody(&open.id, &open.pending_receipt)
                .await?;
            if custody.phase() == &ExperimentalLiveChannelPhaseStatus::Closed {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "channel custody did not converge to Closed after the provider closed: {:?}",
                    custody.phase()
                )
                .into());
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait until the session's live delegation worker has started (its
    /// recovery snapshot left `StartAuthorized`): the state the recorded run
    /// had reached when its test stepped the next browser action.
    async fn delegation_admitted(&self) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + CONVERGENCE_BOUND;
        loop {
            let snapshots = self
                .runtime
                .live_delegation_recovery_snapshots(&self.session_id)
                .await?;
            use meerkat_runtime::live_execution::LiveDelegationRecoveryPhase;
            if snapshots
                .iter()
                .any(|snapshot| snapshot.phase() != LiveDelegationRecoveryPhase::StartAuthorized)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("the voice job's delegation never gained durable custody".into());
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait for the session's next live delegation to reach terminality.
    async fn delegation_terminal(&self) -> Result<(), Box<dyn std::error::Error>> {
        use meerkat_runtime::live_execution::LiveDelegationWorkerTerminalKind;
        let deadline = Instant::now() + CONVERGENCE_BOUND;
        loop {
            let snapshots = self
                .runtime
                .live_delegation_recovery_snapshots(&self.session_id)
                .await?;
            if let Some(snapshot) = snapshots
                .iter()
                .find(|snapshot| snapshot.terminal().is_some())
            {
                if snapshot.terminal() != Some(LiveDelegationWorkerTerminalKind::Completed) {
                    return Err(format!(
                        "the delegated turn did not complete: {:?}",
                        snapshot.terminal()
                    )
                    .into());
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("the delegated turn never reached terminality".into());
            }
            sleep(Duration::from_millis(20)).await;
        }
    }
}

/// Client events per channel from a provider-stream recording.
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

/// S104 replayed: open with a concurrent bootstrap summary, a voice job
/// delegated to a DurableFork worker, the channel dropped while it runs, a
/// typed turn during the closure, the job merged into the source after the
/// reopen, and the merge's reply replayed on the reopened channel as quiet
/// runtime work after the user's question.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s104_replay_sends_the_recorded_client_events() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let _replay = REPLAY_LOCK.lock().await;
    let fixture = Fixture::parse(S104_FIXTURE)?;
    let merge_reply = runtime_work_reply(&fixture, 2)
        .ok_or("the S104 fixture carries no runtime-work append on channel 2")?;
    let cassette = Cassette::start(fixture.clone()).await?;
    let evidence = Journal::create_for("S104-replay", S104_RESULT_TOKEN.to_owned())?;

    // The recorded interleaving: the job finished only after channel 1 was
    // down, and its merge reply committed only after channel 2 was up.
    let (job_gate, job_gate_rx) = watch::channel(false);
    let (merge_gate, merge_gate_rx) = watch::channel(false);
    let llm = Arc::new(ScriptedLlm {
        scripts: BTreeMap::from([
            (
                Purpose::Conversation(0),
                Script {
                    text: "Acknowledged: the team mascot is a heron named Bartleby.".into(),
                    gate: None,
                },
            ),
            (
                Purpose::Conversation(1),
                Script {
                    text: "Remembered: the meeting room is called Osprey.".into(),
                    gate: None,
                },
            ),
            (
                Purpose::DelegatedJob(0),
                Script {
                    text: format!("Wrote coffee.md. {merge_reply}"),
                    gate: Some(job_gate_rx),
                },
            ),
            (
                Purpose::MergeReply,
                Script {
                    text: merge_reply.clone(),
                    gate: Some(merge_gate_rx),
                },
            ),
        ]),
        conversations: AtomicUsize::new(0),
        jobs: AtomicUsize::new(0),
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let mut host = open_replay_host(HostOptions {
        base_url: cassette.base_url(),
        llm: llm.clone(),
        policy: LiveDelegationExecutionPolicy::DurableFork,
        executor_instructions: S104_EXECUTOR_INSTRUCTIONS,
        seed_prompt: Some(S104_SEED_PROMPT),
        summary: "The team mascot is a heron named Bartleby.",
    })
    .await?;

    let result = async {
        // Channel 1: the job is asked for, then the call drops.
        let (channel1, open1) = host.connect(&evidence).await?;
        cassette.release(channel1, "play_at:handoff_job").await?;
        host.delegation_admitted().await?;
        cassette.release(channel1, "disconnect:graceful").await?;
        cassette.ended(channel1).await?;
        host.closed(&open1).await?;

        // Typed turn during the closure, then the job completes.
        let typed = host
            .rpc
            .call_raw(
                "turn/start",
                json!({"session_id":host.session_id,"prompt":S104_TYPED_PROMPT}),
                120,
            )
            .await?;
        if !typed["error"].is_null() {
            return Err(format!("the typed turn failed: {}", typed["error"]).into());
        }
        job_gate.send_replace(true);
        host.delegation_terminal().await?;

        // Channel 2: reopen, the merge reply commits, the user asks.
        let (channel2, open2) = host.connect(&evidence).await?;
        merge_gate.send_replace(true);
        cassette.release(channel2, "play_at:handoff_back").await?;
        cassette.release(channel2, "disconnect:graceful").await?;
        cassette.ended(channel2).await?;
        host.closed(&open2).await?;
        Ok::<_, Box<dyn std::error::Error>>((channel1, channel2))
    }
    .await;
    let finished = evidence.finish(match &result {
        Ok(_) => evidence::Outcome::Passed,
        Err(_) => evidence::Outcome::Failed,
    });
    cassette.diverged()?;
    let (_, channel2) = result.map_err(|error| {
        format!(
            "{error}; scripted LLM calls so far: {:?}",
            llm.calls.lock().expect("calls")
        )
    })?;
    finished?;

    // Each channel opened with the recorded seed shape (item count and
    // roles of the startup history; texts follow from the scripted LLM).
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

    // The replay sent exactly the recorded client events, per channel.
    let replayed = provider_recording::read(
        &evidence
            .path()
            .with_file_name(support::evidence::PROVIDER_STREAM_FILE),
    )?;
    let recorded: BTreeMap<u32, Vec<ClientKey>> = fixture
        .channels
        .iter()
        .map(|tape| (tape.ordinal, tape.client_events.clone()))
        .collect();
    assert_eq!(
        client_events_by_channel(&replayed),
        recorded,
        "the replay's client events (type, event_id) differ from the recording; LLM calls: {:?}",
        llm.calls.lock().expect("calls")
    );

    // The S104 contract on the reopened channel: the merged job's reply is
    // runtime work on the quiet lane, carrying the job's result.
    let runtime_work: Vec<String> = evidence
        .owned_thinking_appends(channel2)?
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

/// The recorded delegation-lane commentary results of `channel`, in order:
/// the third commentary of each delegation is its result text.
fn delegation_results(fixture: &Fixture, channel: u32) -> Vec<String> {
    fixture
        .client_contents(channel, "session.commentary.append")
        .into_iter()
        .filter(|text| {
            !text.starts_with("Started voice request")
                && !text.starts_with("Finished voice request")
        })
        .collect()
}

/// S106 replayed: three channels over one ExistingMember executor. Native
/// exchanges, a delegated job per channel (e3, e6, e9) narrated Started,
/// Finished and its result on the delegation lane, a typed turn during the
/// first closure, and reopens seeded from the retained summary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s106_replay_sends_the_recorded_client_events() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let _replay = REPLAY_LOCK.lock().await;
    let fixture = Fixture::parse(S106_FIXTURE)?;
    let cassette = Cassette::start(fixture.clone()).await?;
    let evidence = Journal::create_for("S106-replay", "Saffron".to_owned())?;

    // One delegated job per channel, each released once the channel carried
    // its "Started" narration (the recorded run's job ran for seconds).
    let mut gates = Vec::new();
    let mut scripts = BTreeMap::from([
        (
            Purpose::Conversation(0),
            Script {
                text: "Acknowledged: the sponsor's name is Marlow.".into(),
                gate: None,
            },
        ),
        (
            Purpose::Conversation(1),
            Script {
                text: "Acknowledged: the budget code is Kestrel.".into(),
                gate: None,
            },
        ),
    ]);
    for (job, channel) in [(0_usize, 1_u32), (1, 2), (2, 3)] {
        let result = delegation_results(&fixture, channel)
            .into_iter()
            .next()
            .ok_or_else(|| {
                format!("the S106 fixture has no delegation result on channel {channel}")
            })?;
        let (gate, gate_rx) = watch::channel(false);
        gates.push(gate);
        scripts.insert(
            Purpose::DelegatedJob(job),
            Script {
                text: result,
                gate: Some(gate_rx),
            },
        );
    }
    let llm = Arc::new(ScriptedLlm {
        scripts,
        conversations: AtomicUsize::new(0),
        jobs: AtomicUsize::new(0),
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let mut host = open_replay_host(HostOptions {
        base_url: cassette.base_url(),
        llm: llm.clone(),
        policy: LiveDelegationExecutionPolicy::ExistingMember,
        executor_instructions: S106_EXECUTOR_INSTRUCTIONS,
        seed_prompt: Some(S106_SEED_PROMPT),
        summary: "The sponsor's name is Marlow. The project codename is Saffron. The launch venue is Lisbon. The budget code is Kestrel.",
    })
    .await?;
    let started = ClientKey {
        kind: "session.commentary.append".into(),
        event_id: Some("meerkat-append-1".into()),
    };

    let result = async {
        // Channel 1: e1, e2 native; e3 delegated; e4 native; drop.
        let (channel1, open1) = host.connect(&evidence).await?;
        for step in ["play_at:haul_e1", "play_at:haul_e2", "play_at:haul_e3"] {
            cassette.release(channel1, step).await?;
        }
        cassette.received(channel1, &started).await?;
        gates[0].send_replace(true);
        cassette.release(channel1, "play_at:haul_e4").await?;
        cassette.release(channel1, "disconnect:graceful").await?;
        cassette.ended(channel1).await?;
        host.closed(&open1).await?;

        // Typed turn during the first closure.
        let typed = host
            .rpc
            .call_raw(
                "turn/start",
                json!({"session_id":host.session_id,"prompt":S106_TYPED_PROMPT}),
                120,
            )
            .await?;
        if !typed["error"].is_null() {
            return Err(format!("the typed turn failed: {}", typed["error"]).into());
        }

        // Channel 2: e5 native; e6 delegated; drop.
        let (channel2, open2) = host.connect(&evidence).await?;
        for step in ["play_at:haul_e5", "play_at:haul_e6"] {
            cassette.release(channel2, step).await?;
        }
        cassette.received(channel2, &started).await?;
        gates[1].send_replace(true);
        cassette.release(channel2, "disconnect:graceful").await?;
        cassette.ended(channel2).await?;
        host.closed(&open2).await?;

        // Channel 3: e7, e8 native; e9 delegated; e10 native; drop.
        let (channel3, open3) = host.connect(&evidence).await?;
        for step in ["play_at:haul_e7", "play_at:haul_e8", "play_at:haul_e9"] {
            cassette.release(channel3, step).await?;
        }
        cassette.received(channel3, &started).await?;
        gates[2].send_replace(true);
        cassette.release(channel3, "play_at:haul_e10").await?;
        cassette.release(channel3, "disconnect:graceful").await?;
        cassette.ended(channel3).await?;
        host.closed(&open3).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    let finished = evidence.finish(match &result {
        Ok(_) => evidence::Outcome::Passed,
        Err(_) => evidence::Outcome::Failed,
    });
    cassette.diverged()?;
    result.map_err(|error| {
        format!(
            "{error}; scripted LLM calls so far: {:?}",
            llm.calls.lock().expect("calls")
        )
    })?;
    finished?;

    // Every reopen is seeded with the summary first (a developer item).
    for body in cassette.create_bodies() {
        assert_eq!(
            body["session"]["input"][0]["role"], "developer",
            "each S106 open is seeded with the summary first: {body}"
        );
    }
    let replayed = provider_recording::read(
        &evidence
            .path()
            .with_file_name(support::evidence::PROVIDER_STREAM_FILE),
    )?;
    let recorded: BTreeMap<u32, Vec<ClientKey>> = fixture
        .channels
        .iter()
        .map(|tape| (tape.ordinal, tape.client_events.clone()))
        .collect();
    assert_eq!(
        client_events_by_channel(&replayed),
        recorded,
        "the replay's client events (type, event_id) differ from the recording; LLM calls: {:?}",
        llm.calls.lock().expect("calls")
    );
    let _ = &mut host;
    Ok(())
}
