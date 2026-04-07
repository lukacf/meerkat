use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, bail};
use chrono::Utc;
use futures_util::StreamExt;
use meerkat_client::{LlmClientAdapter, OpenAiClient};
use meerkat_comms::agent::wrap_with_comms;
use meerkat_comms::{
    CommsConfig, CommsRuntime, Keypair, PubKey, ResolvedCommsConfig, TrustedPeer,
    TrustedPeers,
};
use meerkat_core::agent::CommsRuntime as _;
use meerkat_core::comms::CommsCommand;
use meerkat_core::error::ToolError;
use meerkat_core::event::AgentEvent;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::EventEnvelope;
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, SessionService,
};
use meerkat_core::types::{ContentInput, SessionId, ToolCallView, ToolResult};
use meerkat_core::{AgentBuilder, AgentLlmClient, AgentToolDispatcher, BindOutcome, BudgetLimits};
use meerkat_runtime::comms_drain::spawn_comms_drain_with_observer;
use meerkat_runtime::SessionServiceRuntimeExt;
use meerkat_runtime::input::{Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput};
use meerkat_runtime::RuntimeSessionAdapter;
use meerkat_session::EphemeralSessionService;
use meerkat_session::ephemeral::SessionAgentBuilder;
use meerkat_tools::{BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore, ToolMode, ToolPolicyLayer};
use serde_json::{Value, json};
use tokio::sync::mpsc;

#[path = "../../runtime_support.rs"]
mod runtime_support;

use runtime_support::{
    ProbeNoopStore, ProbeSessionAgent, ProbeSessionRuntimeExecutor,
};

const PHASE0_HOST_SECRET: [u8; 32] = [7; 32];

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;
    let peer_pubkey = PubKey::from_peer_id(&args.peer_id).context("invalid ESP peer id")?;
    let trusted_peers = TrustedPeers {
        peers: vec![TrustedPeer {
            name: args.peer_name.clone(),
            pubkey: peer_pubkey,
            addr: args.peer_addr.clone(),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    };
    let mut comms_runtime = CommsRuntime::new_with_state(
        host_comms_config(&args.listen_addr),
        Keypair::from_secret(PHASE0_HOST_SECRET),
        trusted_peers,
    )
    .await
    .context("failed to build host comms runtime")?;
    comms_runtime
        .start_listeners()
        .await
        .with_context(|| format!("failed to bind host listener on {}", args.listen_addr))?;
    let comms_runtime = Arc::new(comms_runtime);
    let inbound_epoch = Arc::new(AtomicUsize::new(1));

    let service = Arc::new(EphemeralSessionService::new(
        HostProbeAgentBuilder {
            peer_name: args.peer_name.clone(),
            scenario: args.scenario.clone(),
            exchanges: args.exchanges,
            model: args.model.clone(),
            stream: args.stream,
            openai_api_key: args.openai_api_key.clone(),
            openai_base_url: args.openai_base_url.clone(),
            comms_runtime: Arc::clone(&comms_runtime),
            inbound_epoch: Arc::clone(&inbound_epoch),
        },
        1,
    ));
    let runtime_adapter = Arc::new(RuntimeSessionAdapter::ephemeral());

    let session_id = create_keep_alive_session(&service, &args.model).await?;
    println!(
        "MKT:HOST_MEERKAT:SESSION_CREATED session_id={}",
        json_quote(&session_id.to_string())
    );
    runtime_adapter
        .ensure_session_with_executor(
            session_id.clone(),
            Box::new(ProbeSessionRuntimeExecutor::new(
                Arc::clone(&service),
                session_id.clone(),
            )),
        )
        .await;
    println!(
        "MKT:HOST_MEERKAT:EXECUTOR_READY session_id={}",
        json_quote(&session_id.to_string())
    );

    let session_comms = service
        .comms_runtime(&session_id)
        .await
        .context("host session missing comms runtime")?;

    let (transcript_tx, mut transcript_rx) = mpsc::unbounded_channel::<TranscriptEvent>();
    let observer_tx = transcript_tx.clone();
    let peer_name = args.peer_name.clone();
    let inbound_epoch_for_observer = Arc::clone(&inbound_epoch);
    let _drain = spawn_comms_drain_with_observer(
        Arc::clone(&runtime_adapter),
        session_id.clone(),
        session_comms,
        None,
        Some(Arc::new(move |ci| {
            if let Some(text) = inbound_text(ci) {
                inbound_epoch_for_observer.fetch_add(1, Ordering::SeqCst);
                let _ = observer_tx.send(TranscriptEvent::Incoming {
                    from: peer_name.clone(),
                    text,
                });
            }
        })),
    );

    let mut events = service
        .subscribe_session_events(&session_id)
        .await
        .context("failed to subscribe to session events")?;
    let event_tx = transcript_tx.clone();
    tokio::spawn(async move {
        while let Some(envelope) = events.next().await {
            match &envelope.payload {
                AgentEvent::ToolCallRequested { name, args, .. } if name == "send" => {
                    if let Some(text) = args
                        .get("body")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                    {
                        println!(
                            "MKT:HOST_MEERKAT:SEND_REQUESTED text={}",
                            json_quote(&text)
                        );
                    }
                }
                AgentEvent::ToolExecutionCompleted {
                    name,
                    result,
                    is_error,
                    ..
                } if name == "send" => {
                    println!(
                        "MKT:HOST_MEERKAT:SEND_COMPLETED is_error={} result={}",
                        is_error,
                        json_quote(result)
                    );
                    if !*is_error {
                        if let Some(text) = outgoing_send_text(&envelope) {
                            let _ = event_tx.send(TranscriptEvent::Outgoing {
                                to: "esp32-probe".to_string(),
                                text,
                            });
                        }
                    }
                }
                AgentEvent::ToolResultReceived {
                    name, is_error, ..
                } if name == "send" => {
                    println!(
                        "MKT:HOST_MEERKAT:SEND_RESULT_RECEIVED is_error={}",
                        is_error
                    );
                }
                AgentEvent::InteractionComplete { interaction_id, result } => {
                    println!(
                        "MKT:HOST_MEERKAT:INTERACTION_COMPLETE id={} result={}",
                        json_quote(&interaction_id.0.to_string()),
                        json_quote(result)
                    );
                }
                AgentEvent::InteractionFailed { interaction_id, error } => {
                    println!(
                        "MKT:HOST_MEERKAT:INTERACTION_FAILED id={} error={}",
                        json_quote(&interaction_id.0.to_string()),
                        json_quote(error)
                    );
                }
                _ => {}
            }
        }
    });

    let _ = runtime_adapter
        .accept_input(
            &session_id,
            make_prompt_input(&kickoff_prompt(&args.peer_name, &args.scenario)),
        )
        .await
        .context("failed to inject kickoff input")?;
    println!(
        "MKT:HOST_MEERKAT:KICKOFF_ACCEPTED session_id={}",
        json_quote(&session_id.to_string())
    );

    println!(
        "MKT:HOST_MEERKAT:READY peer_name={} listen_addr={} exchanges={}",
        json_quote(&args.peer_name),
        json_quote(&args.listen_addr),
        args.exchanges
    );

    let mut transcript: Vec<String> = Vec::new();
    let mut exchange_count = 0usize;
    let mut stop_sent = false;
    let mut stop_deadline = None;

    loop {
        let event = if let Some(deadline) = stop_deadline {
            match tokio::time::timeout_at(deadline, transcript_rx.recv()).await {
                Ok(event) => event,
                Err(_) => break,
            }
        } else {
            transcript_rx.recv().await
        };

        let Some(event) = event else {
            break;
        };

        let mut inbound_after_stop = false;
        match event {
            TranscriptEvent::Incoming { from, text } => {
                exchange_count += 1;
                let line = format!("{from}: {text}");
                transcript.push(line.clone());
                inbound_after_stop = stop_sent;
                println!(
                    "MKT:HOST_MEERKAT:TRANSCRIPT direction={} count={} text={}",
                    json_quote("in"),
                    exchange_count,
                    json_quote(&line)
                );
            }
            TranscriptEvent::Outgoing { to, text } => {
                exchange_count += 1;
                let line = format!("host-> {to}: {text}");
                transcript.push(line.clone());
                println!(
                    "MKT:HOST_MEERKAT:TRANSCRIPT direction={} count={} text={}",
                    json_quote("out"),
                    exchange_count,
                    json_quote(&line)
                );
            }
        }

        if !stop_sent && exchange_count >= args.exchanges {
            stop_sent = true;
            stop_deadline =
                Some(tokio::time::Instant::now() + std::time::Duration::from_secs(8));
            let _ = runtime_adapter
                .accept_input(
                    &session_id,
                    make_prompt_input(&stop_prompt(&args.peer_name, &args.scenario)),
                )
                .await
                .context("failed to inject stop prompt")?;
            println!(
                "MKT:HOST_MEERKAT:STOP_PROMPT count={} peer={}",
                exchange_count,
                json_quote(&args.peer_name)
            );
        } else if stop_sent && inbound_after_stop {
            break;
        }
    }

    comms_runtime
        .send(CommsCommand::PeerMessage {
            to: meerkat_core::comms::PeerName::new(args.peer_name.clone())
                .expect("valid peer name"),
            body: "DISMISS".to_string(),
            blocks: None,
        })
        .await
        .context("failed to send DISMISS to ESP peer")?;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let _ = service.archive(&session_id).await;
    runtime_adapter.unregister_session(&session_id).await;
    println!(
        "MKT:HOST_MEERKAT:PASS exchanges={} transcript_lines={}",
        exchange_count,
        transcript.len()
    );
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    std::process::exit(0);
}

#[derive(Clone)]
struct HostProbeAgentBuilder {
    peer_name: String,
    scenario: String,
    exchanges: usize,
    model: String,
    stream: bool,
    openai_api_key: String,
    openai_base_url: String,
    comms_runtime: Arc<CommsRuntime>,
    inbound_epoch: Arc<AtomicUsize>,
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait::async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait::async_trait
)]
impl SessionAgentBuilder for HostProbeAgentBuilder {
    type Agent = ProbeSessionAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, meerkat_core::service::SessionError> {
        let task_store = Arc::new(MemoryTaskStore::new());
        let inner = CompositeDispatcher::new(
            task_store,
            &BuiltinToolConfig {
                policy: ToolPolicyLayer::new().with_mode(ToolMode::DenyAll),
                ..Default::default()
            },
            None,
            None,
            None,
            Some("phase0-host-runtime".to_string()),
            true,
        )
        .map_err(|error| {
            meerkat_core::service::SessionError::Agent(
                meerkat_core::error::AgentError::ConfigError(error.to_string()),
            )
        })?;

        let tools: Arc<dyn AgentToolDispatcher> = Arc::new(HostSendGuardDispatcher::new(
            wrap_with_comms(Arc::new(inner), Arc::clone(&self.comms_runtime)),
            Arc::clone(&self.inbound_epoch),
            Arc::new(AtomicUsize::new(0)),
        ));
        let llm: Arc<dyn AgentLlmClient> = Arc::new(
            LlmClientAdapter::new(
                Arc::new(OpenAiClient::new_with_base_url(
                    self.openai_api_key.clone(),
                    self.openai_base_url.clone(),
                )),
                self.model.clone(),
            )
            .with_provider_params(Some(json!({
                "reasoning_effort": "low",
                "stream": self.stream
            }))),
        );
        let store: Arc<dyn meerkat_core::AgentSessionStore> = Arc::new(ProbeNoopStore);

        let comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> =
            self.comms_runtime.clone();

        let agent = AgentBuilder::new()
            .model(req.model.clone())
            .system_prompt(
                req.system_prompt
                    .clone()
                    .unwrap_or_else(|| {
                        host_system_prompt(&self.peer_name, &self.scenario, self.exchanges)
                    }),
            )
            .budget(BudgetLimits::default())
            .max_tokens_per_turn(req.max_tokens.unwrap_or(256))
            .max_turns(1)
            .with_comms_runtime(comms_runtime)
            .build(llm, tools, store)
            .await;

        Ok(ProbeSessionAgent::new(agent))
    }
}

enum TranscriptEvent {
    Incoming { from: String, text: String },
    Outgoing { to: String, text: String },
}

struct PendingPrompt {
    step: usize,
    prompt: String,
    retries: usize,
}

struct HostSendGuardDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    inbound_epoch: Arc<AtomicUsize>,
    sent_epoch: Arc<AtomicUsize>,
}

impl HostSendGuardDispatcher {
    fn new(
        inner: Arc<dyn AgentToolDispatcher>,
        inbound_epoch: Arc<AtomicUsize>,
        sent_epoch: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            inner,
            inbound_epoch,
            sent_epoch,
        }
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for HostSendGuardDispatcher {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        self.inner.tools()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == "send" {
            let epoch = self.inbound_epoch.load(Ordering::SeqCst);
            let previous = self.sent_epoch.swap(epoch, Ordering::SeqCst);
            if epoch == previous {
                println!("MKT:HOST_MEERKAT:SEND_SUPPRESSED epoch={epoch}");
                return Ok(ToolDispatchOutcome::from(ToolResult::new(
                    call.id.to_string(),
                    "{\"kind\":\"peer_message\",\"status\":\"sent\"}".to_string(),
                    false,
                )));
            }
            let outcome = self.inner.dispatch(call).await;
            if outcome.is_err() {
                self.sent_epoch.store(previous, Ordering::SeqCst);
            }
            return outcome;
        }
        self.inner.dispatch(call).await
    }

    async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn bind_wait_interrupt(
        self: Arc<Self>,
        rx: meerkat_core::wait_interrupt::WaitInterruptReceiver,
    ) -> Result<BindOutcome, meerkat_core::wait_interrupt::WaitInterruptBindError>
    {
        let outcome = Arc::clone(&self.inner).bind_wait_interrupt(rx)?;
        let bound = outcome.was_bound();
        let inner = outcome.into_dispatcher();
        let wrapped: Arc<dyn AgentToolDispatcher> = Arc::new(HostSendGuardDispatcher::new(
            inner,
            Arc::clone(&self.inbound_epoch),
            Arc::clone(&self.sent_epoch),
        ));
        Ok(if bound {
            BindOutcome::Bound(wrapped)
        } else {
            BindOutcome::Skipped(wrapped)
        })
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        owner_session_id: SessionId,
    ) -> Result<BindOutcome, meerkat_core::agent::OpsLifecycleBindError> {
        let outcome = Arc::clone(&self.inner).bind_ops_lifecycle(registry, owner_session_id)?;
        let bound = outcome.was_bound();
        let inner = outcome.into_dispatcher();
        let wrapped: Arc<dyn AgentToolDispatcher> = Arc::new(HostSendGuardDispatcher::new(
            inner,
            Arc::clone(&self.inbound_epoch),
            Arc::clone(&self.sent_epoch),
        ));
        Ok(if bound {
            BindOutcome::Bound(wrapped)
        } else {
            BindOutcome::Skipped(wrapped)
        })
    }
}

fn inbound_text(ci: meerkat_core::interaction::ClassifiedInboxInteraction) -> Option<String> {
    match &ci.interaction.content {
        meerkat_core::interaction::InteractionContent::Message { body, .. } => Some(body.clone()),
        _ => None,
    }
}

fn outgoing_send_text(envelope: &EventEnvelope<AgentEvent>) -> Option<String> {
    match &envelope.payload {
        AgentEvent::ToolCallRequested { name, args, .. } if name == "send" => args
            .get("body")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    }
}

async fn create_keep_alive_session(
    service: &Arc<EphemeralSessionService<HostProbeAgentBuilder>>,
    model: &str,
) -> anyhow::Result<SessionId> {
    let result = service
        .create_session(CreateSessionRequest {
            model: model.to_string(),
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: None,
            max_tokens: Some(256),
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                keep_alive: true,
                comms_name: Some("phase0-host".to_string()),
                ..Default::default()
            }),
            labels: None,
        })
        .await
        .context("failed to create host session")?;
    Ok(result.session_id)
}

fn make_prompt_input(text: &str) -> Input {
    Input::Prompt(PromptInput {
        header: InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: text.to_string(),
        blocks: None,
        turn_metadata: None,
    })
}

fn host_comms_config(listen_addr: &str) -> ResolvedCommsConfig {
    ResolvedCommsConfig {
        enabled: true,
        name: "phase0-host".to_string(),
        inproc_namespace: None,
        identity_dir: PathBuf::from("/tmp/meerkat-phase0-host-identity"),
        trusted_peers_path: PathBuf::from("/tmp/meerkat-phase0-host-trusted.json"),
        listen_uds: None,
        listen_tcp: Some(listen_addr.parse().expect("valid listen addr")),
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        comms_config: CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: true,
        allow_external_unauthenticated: false,
    }
}

fn kickoff_prompt(peer_name: &str, scenario: &str) -> String {
    match scenario {
        "micropython" => format!(
            "Start the conversation with {peer_name}. \
             In this interaction, call the send tool exactly once with this exact body and nothing else:\n{}\n\
             After that single send call, stop immediately. \
             Do not answer with plain assistant text.",
            micropython_step_message(1)
        ),
        _ => format!(
            "Start the conversation with {peer_name}. Collaboratively write the greatest LLM joke ever. \
             Use the send tool exactly once to send a short opening line to your peer. \
             Do not explain the tool use and do not answer with plain assistant text."
        ),
    }
}

fn stop_prompt(peer_name: &str, scenario: &str) -> String {
    match scenario {
        "micropython" => format!(
            "Stop now. Use the send tool exactly once with this exact body and nothing else:\nSTOP - micropython validation is complete. Thanks, {peer_name}.\n\
             Do not continue afterward and do not answer with plain assistant text."
        ),
        _ => format!(
            "Stop now. Use the send tool exactly once to send a short closing line to {peer_name}. \
             Include the word STOP so the peer knows the conversation is ending. \
             Do not continue afterward and do not answer with plain assistant text."
        ),
    }
}

fn host_system_prompt(peer_name: &str, scenario: &str, exchanges: usize) -> String {
    match scenario {
        "micropython" => format!(
            "You are the host Meerkat coordinating validation with {peer_name}. \
             The micropython scenario is fully scripted. \
             Count how many incoming transcript messages from {peer_name} you have already observed in this session. \
             Let N be that inbound message count. \
             If the latest peer message contains STOP or DISMISS, call the send tool exactly once with the exact body `STOP - micropython validation is complete. Thanks, {peer_name}.` and then stop. \
             Otherwise, if N is less than {exchanges}, call the send tool exactly once with the exact scheduled body for step N+1. \
             After you receive the first inbound message from {peer_name}, the next scheduled body is step 2; after the second inbound message, the next scheduled body is step 3; and so on. \
             Use the scheduled body verbatim. Do not paraphrase it, shorten it, expand it, or swap in a different snippet. \
             Never ask for a generic 'another short safe snippet'. Never preview future steps. Never send twice in one interaction. \
             After the single send call, stop immediately for this interaction. Do not answer with plain assistant text.\n\n\
             Scheduled bodies:\n{}",
            micropython_schedule_block(exchanges)
        ),
        _ => format!(
            "You are the host half of a two-agent comedy duo talking to {peer_name}. \
             Each turn, read the latest peer message and continue the collaborative joke. \
             You must respond by calling the send tool exactly once per turn with kind=peer_message and to={peer_name}. \
             Do not finish a turn with plain assistant text and do not skip the send call. \
             If you are unsure, still use send with one short witty line. \
             Keep each line short, witty, and specific. \
             If the latest peer message contains STOP or DISMISS, send one short farewell via send and then stop sending further messages."
        ),
    }
}

fn micropython_step_message(step: usize) -> String {
    match step {
        1 => "Please run micropython_exec exactly once with this short snippet:\nprint('hi')\nresult = 1\nReply with the printed output and the result.".to_string(),
        2 => "Run micropython_exec once with: print('ok'); result = 2. Reply with output and result.".to_string(),
        3 => "Run micropython_exec once with: print(3*3); result = 'nine'. Reply with output and result.".to_string(),
        4 => "Run micropython_exec once with: print('ESP32'); result = 4. Reply with output and result.".to_string(),
        5 => "Run micropython_exec once with: print(sum([1,2])); result = 5. Reply with output and result.".to_string(),
        6 => "Run micropython_exec once with: print('pong'); result = 6. Reply with output and result.".to_string(),
        7 => "Run micropython_exec once with: print(7); result = 7. Reply with output and result.".to_string(),
        8 => "Run micropython_exec once with: print('x'); result = 8. Reply with output and result.".to_string(),
        9 => "Run micropython_exec once with: print(2**5); result = '32'. Reply with output and result.".to_string(),
        10 => "Run micropython_exec once with: print('a'); result = 9. Reply with output and result.".to_string(),
        11 => "Run micropython_exec once with: print(10-3); result = 10. Reply with output and result.".to_string(),
        12 => "Run micropython_exec once with: print('bee'); result = 11. Reply with output and result.".to_string(),
        13 => "Run micropython_exec once with: print(len('cat')); result = 12. Reply with output and result.".to_string(),
        14 => "Run micropython_exec once with: print('z'); result = 13. Reply with output and result.".to_string(),
        15 => "Run micropython_exec once with: print(14); result = 14. Reply with output and result.".to_string(),
        16 => "Run micropython_exec once with: print('emu'); result = 15. Reply with output and result.".to_string(),
        17 => "Run micropython_exec once with: print(16//2); result = 16. Reply with output and result.".to_string(),
        18 => "Run micropython_exec once with: print('go'); result = 17. Reply with output and result.".to_string(),
        19 => "Run micropython_exec once with: print(18%5); result = 18. Reply with output and result.".to_string(),
        20 => "Run micropython_exec once with: print('last'); result = 19. Reply with output and result.".to_string(),
        21 => "Run micropython_exec once with: print(20); result = 20. Reply with output and result.".to_string(),
        other => format!(
            "Run micropython_exec once with: print({other}); result = {other}. Reply with output and result."
        ),
    }
}

fn micropython_schedule_block(exchanges: usize) -> String {
    let mut lines = Vec::with_capacity(exchanges);
    for step in 1..=exchanges {
        lines.push(format!("{step}. {}", micropython_step_message(step)));
    }
    lines.join("\n")
}

struct Args {
    peer_name: String,
    peer_id: String,
    peer_addr: String,
    listen_addr: String,
    exchanges: usize,
    scenario: String,
    model: String,
    stream: bool,
    openai_api_key: String,
    openai_base_url: String,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut peer_name = None;
        let mut peer_id = None;
        let mut peer_addr = None;
        let mut listen_addr = None;
        let mut exchanges = 15_usize;
        let mut scenario =
            std::env::var("ESP32_SCENARIO").unwrap_or_else(|_| "joke".to_string());
        let mut model =
            std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-5.4-mini".to_string());
        let stream = parse_env_bool("OPENAI_STREAM_HOST") || parse_env_bool("OPENAI_STREAM");
        let openai_api_key = std::env::var("OPENAI_API_KEY").context("missing OPENAI_API_KEY")?;
        let openai_base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".to_string());

        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--peer-name" => peer_name = iter.next(),
                "--peer-id" => peer_id = iter.next(),
                "--peer-addr" => peer_addr = iter.next(),
                "--listen-addr" => listen_addr = iter.next(),
                "--exchanges" => {
                    let value = iter.next().context("missing value for --exchanges")?;
                    exchanges = value.parse().context("invalid --exchanges value")?;
                }
                "--scenario" => scenario = iter.next().context("missing value for --scenario")?,
                "--model" => model = iter.next().context("missing value for --model")?,
                other => bail!("unknown argument: {other}"),
            }
        }

        Ok(Self {
            peer_name: peer_name.context("missing --peer-name")?,
            peer_id: peer_id.context("missing --peer-id")?,
            peer_addr: peer_addr.context("missing --peer-addr")?,
            listen_addr: listen_addr.context("missing --listen-addr")?,
            exchanges,
            scenario,
            model,
            stream,
            openai_api_key,
            openai_base_url,
        })
    }
}

fn parse_env_bool(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

fn json_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<encoding-error>\"".to_string())
}
