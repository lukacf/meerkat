#![cfg(all(feature = "openai-live-e2e", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! Scenario 97: public GPT Live (`gpt-live-1`) client-context vertical.
//!
//! Twin of scenario 96 on the public OpenAI Live API: a plain OpenAI API key
//! from the environment is the configured realm binding, the host composes
//! [`ExperimentalGptLiveOpenAuthority::new_public`] with no operator, realm
//! admission, or Gate0 evidence, the RPC client selects the public
//! client-context profile, and the browser peer speaks the public `session.*`
//! event vocabulary on the `oai-events` data channel.

#[path = "support/gpt_live_e2e.rs"]
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat::experimental_gpt_live::{
    ExperimentalGptLiveOpenAuthority, ExperimentalGptLiveWebrtcTransport,
    GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID, GPT_LIVE_PUBLIC_MODEL,
    PublicGptLiveOpenAuthorityConfig,
};
use meerkat_core::{
    AuthBindingRef, AuthProfileConfig, BackendProfileConfig, BindingId, BindingOrigin,
    BindingPolicy, BlobStore, Config, ConfigRuntime, ConfigStore, CredentialSourceSpec,
    MemoryConfigStore, ProviderBindingConfig, RealmConfigSection, RealmId,
};
use meerkat_rpc::router::NotificationSink;
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::BufReader;
use tokio::time::{Duration, Instant, sleep};

use support::{
    BrowserPeer, BrowserPeerProtocol, ExplicitScenarioBindingAuthority, FixedConfigSource,
    JsonlRpcClient, execution_identity, wait_for_events, wait_for_spoken_output,
};

const REALM: &str = "scenario-97-gpt-live-public";
const BINDING: &str = "openai_api_key";
const API_KEY_ENV: &str = "OPENAI_API_KEY";
/// Deprecated private profile: the public authority must reject it.
const EXPERIMENTAL_CLIENT_PROFILE: &str = "openai.gpt-live-1-codex.client-context.v1";
const DEFAULT_EXECUTOR_MODEL: &str = "gpt-5.6-sol";
const OUTPUT_AVAILABLE: &str = "live/assistant_output_available";

fn executor_model() -> String {
    std::env::var("GPT_LIVE_E2E_EXECUTOR_MODEL")
        .unwrap_or_else(|_| DEFAULT_EXECUTOR_MODEL.to_string())
}

fn auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(REALM).expect("valid realm"),
        binding: BindingId::parse(BINDING).expect("valid binding"),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

/// The binding's credential source is the `OPENAI_API_KEY` environment
/// variable (the resolver also honors the `RKAT_` prefixed form). Missing
/// material is a hard failure: this scenario never skips silently.
fn require_api_key() -> Result<(), Box<dyn std::error::Error>> {
    let present = [format!("RKAT_{API_KEY_ENV}"), API_KEY_ENV.to_string()]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()));
    if present {
        return Ok(());
    }
    Err(format!(
        "{API_KEY_ENV} (or RKAT_{API_KEY_ENV}) is required: scenario 97 drives the public OpenAI Live API through an API-key realm binding and does not skip"
    )
    .into())
}

fn scenario_config() -> Config {
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
            source: CredentialSourceSpec::Env {
                env: API_KEY_ENV.to_string(),
                fallback: Vec::new(),
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
            default_model: Some(executor_model()),
            policy: BindingPolicy::default(),
            provider_default: false,
        },
    );
    let mut config = Config::default();
    config.realm.insert(REALM.to_string(), section);
    config.model_fallback.enabled = Some(false);
    config
}

fn is_user_input(event: &Value) -> bool {
    event["type"] == "session.input_transcript.delta"
}

fn is_assistant_output(event: &Value) -> bool {
    event["type"] == "session.output_transcript.delta"
        || event["type"] == "session.output_audio.delta"
}

fn is_client_delegation(event: &Value) -> bool {
    event["type"] == "session.delegation.created" && event["delegation"]["target"] == "client"
}

/// Acknowledge one published assistant output so the machine can admit the
/// next synthesized assistant turn.
async fn complete_playback(
    rpc: &mut JsonlRpcClient,
    channel_id: &Value,
    output: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(&output["channel_id"], channel_id);
    rpc.call(
        "live/playback_complete",
        json!({
            "channel_id": output["channel_id"],
            "output_id": output["output_id"],
        }),
        30,
    )
    .await?;
    Ok(())
}

/// Everything both public-Live scenarios share: the runtime-backed RPC host,
/// a turn-driven executor mob member, the session-bound public open
/// authority, one open channel and one browser peer answering its offer.
struct PublicLiveHarness {
    rpc: JsonlRpcClient,
    peer: BrowserPeer,
    channel_id: Value,
    session_id: meerkat_core::SessionId,
    mob_id: String,
    server_task: tokio::task::AbortHandle,
    _temp: tempfile::TempDir,
}

impl PublicLiveHarness {
    /// Close the current browser peer, open a second Live channel on the same
    /// session and answer its offer with a fresh peer. The runtime seeds the
    /// canonical dialogue into the new provider session at creation.
    async fn reopen(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let old_peer = std::mem::replace(
            &mut self.peer,
            BrowserPeer::start(BrowserPeerProtocol::Public).await?,
        );
        old_peer.close().await;
        let open = self
            .rpc
            .call(
                "live/open",
                json!({"session_id":self.session_id,"transport":"webrtc",
                    "execution_identity":execution_identity(GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID)}),
                60,
            )
            .await?;
        self.channel_id = open["channel_id"].clone();
        let offer = self.peer.call(json!({"type":"prepare"})).await?;
        assert_eq!(offer["protocol"], "public");
        let answer = self
            .rpc
            .call(
                open["transport"]["answer_method"]
                    .as_str()
                    .unwrap_or("live/webrtc/answer"),
                json!({"channel_id":self.channel_id,"token":open["transport"]["token"],
                    "offer_sdp":offer["offer_sdp"]}),
                90,
            )
            .await?;
        self.peer
            .call(json!({"type":"answer","answer_sdp":answer["answer_sdp"]}))
            .await?;
        Ok(())
    }
}

async fn open_public_live(
    temp_prefix: &str,
    operator_principal: &'static str,
) -> Result<PublicLiveHarness, Box<dyn std::error::Error>> {
    let temp = tempfile::Builder::new()
        .prefix(temp_prefix)
        .tempdir_in(support::test_tmp_root()?)?;
    let config = scenario_config();
    let binding = auth_binding();
    // No operator, realm admission, or provider auth persistence: the
    // configured API-key binding and the compiled `openai-live` feature are
    // the whole admission for the public path.
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
    let persistence = meerkat::PersistenceBundle::new(
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()) as Arc<dyn BlobStore>,
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

    let callback_rx = runtime.init_callback_channel();
    let mobs = meerkat_rpc::router::compose_rpc_mob_state(&runtime, &config_store, None);
    runtime.set_mob_state(Arc::clone(&mobs));
    let (client_stream, server_stream) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let mut rpc = JsonlRpcClient::new(client_stream);

    let projection = Arc::new(
        meerkat_rpc::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(&runtime)),
    );
    let live_host = Arc::new(meerkat_live::LiveAdapterHost::new(projection.clone()));
    let webrtc = Arc::new(meerkat_live::LiveWebrtcState::new(
        Arc::clone(&live_host),
        projection.clone(),
        projection.clone(),
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
    let mut server = RpcServer::new_with_skill_runtime_and_mob_state(
        BufReader::new(server_read),
        server_write,
        Arc::clone(&runtime),
        Arc::clone(&config_store),
        None,
        Arc::clone(&mobs),
        callback_rx,
    )
    .with_live_session_factory_opt(Some(live_factory))
    .with_live_webrtc(webrtc);

    let server_task = tokio::spawn(async move { server.run().await });
    rpc.call("initialize", json!({}), 60).await?;
    let mob_id = format!("gpt-live-public-e2e-{}", std::process::id());
    rpc.call(
        "mob/create",
        json!({"definition":{"id":mob_id,"profiles":{"executor":{
            "model":executor_model(),
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

    let public_transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
    let open_authority = Arc::new(ExperimentalGptLiveOpenAuthority::new_public(
        PublicGptLiveOpenAuthorityConfig {
            agent_factory: factory.clone(),
            config_source: Arc::new(FixedConfigSource(config)),
            binding_authority: Arc::new(ExplicitScenarioBindingAuthority {
                session_id: session_id.clone(),
                binding: binding.clone(),
                auth_lease: runtime.generated_auth_lease_handle(),
                mobs: Arc::clone(&mobs),
                principal_id: operator_principal,
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
            // Public Live voice names differ from the private protocol: an
            // unknown voice (for example "cove") is refused with HTTP 403
            // "Voice session access denied", not a validation error.
            voice: "marin".to_string(),
            session_instructions: None,
        },
    )?);
    // Rebuild the connection host with the session-bound public authority.
    drop(rpc);
    server_task.abort();
    let _ = server_task.await;
    let (client_stream, server_stream) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let mut rpc = JsonlRpcClient::new(client_stream);
    let callback_rx = runtime.init_callback_channel();
    let projection = Arc::new(
        meerkat_rpc::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(&runtime)),
    );
    let live_host = Arc::new(meerkat_live::LiveAdapterHost::new(projection.clone()));
    let webrtc = Arc::new(meerkat_live::LiveWebrtcState::new(
        live_host,
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
    let mut server = RpcServer::new_with_skill_runtime_and_mob_state(
        BufReader::new(server_read),
        server_write,
        Arc::clone(&runtime),
        Arc::clone(&config_store),
        None,
        Arc::clone(&mobs),
        callback_rx,
    )
    .with_live_session_factory_opt(Some(live_factory))
    .with_live_webrtc(webrtc)
    .with_live_webrtc_answer_transport(public_transport)
    .with_experimental_live_open_authority(open_authority);
    let server_task = tokio::spawn(async move { server.run().await });
    rpc.call("initialize", json!({}), 60).await?;

    let rejected = rpc
        .call_raw(
            "live/open",
            json!({"session_id":session_id,"transport":"webrtc",
                "execution_identity":execution_identity(EXPERIMENTAL_CLIENT_PROFILE)}),
            30,
        )
        .await?;
    assert!(
        !rejected["error"].is_null(),
        "the public authority must fail closed on the deprecated private profile"
    );
    let open = rpc
        .call(
            "live/open",
            json!({"session_id":session_id,"transport":"webrtc",
                "execution_identity":execution_identity(GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID)}),
            60,
        )
        .await?;
    let channel_id = open["channel_id"].clone();

    let mut peer = BrowserPeer::start(BrowserPeerProtocol::Public).await?;
    let offer = peer.call(json!({"type":"prepare"})).await?;
    assert_eq!(offer["protocol"], "public");
    // The answer step is where the host creates the provider session. The
    // broker sanitizes provider HTTP failures to `remote_unavailable` and
    // logs the status. A 403 "Voice session access denied" from
    // `POST /v1/live/sessions` means either a voice name the public API does
    // not offer or an organization without Live voice-session access.
    let answer = rpc
        .call(
            open["transport"]["answer_method"]
                .as_str()
                .unwrap_or("live/webrtc/answer"),
            json!({"channel_id":channel_id,"token":open["transport"]["token"],
                "offer_sdp":offer["offer_sdp"]}),
            90,
        )
        .await
        .map_err(|error| {
            format!(
                "{error}; the public Live session could not be created for the configured {API_KEY_ENV}: verify the configured voice is a public Live voice and the key's organization has OpenAI Live (gpt-live-1) voice-session access"
            )
        })?;
    peer.call(json!({"type":"answer","answer_sdp":answer["answer_sdp"]}))
        .await?;

    Ok(PublicLiveHarness {
        rpc,
        peer,
        channel_id,
        session_id,
        mob_id,
        server_task: server_task.abort_handle(),
        _temp: temp,
    })
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_scenario_97_gpt_live_public_client_context_vertical()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            "oai_rt_rs::live=debug,meerkat_openai::public_live=debug,meerkat::experimental_gpt_live=debug,meerkat::session_runtime=debug,meerkat_live=debug,meerkat_rpc=debug,meerkat_runtime::meerkat_machine::runtime_control=debug,meerkat_mob_mcp::live_delegation=debug,meerkat_mob::runtime::delegation=debug",
        )
        .with_test_writer()
        .try_init();
    require_api_key()?;
    let PublicLiveHarness {
        mut rpc,
        mut peer,
        channel_id,
        mob_id,
        server_task,
        ..
    } = open_public_live("gpt-live-public-e2e-", "scenario-97-operator").await?;

    // Phase A: greeting with a provider-native barge-in. The public API has
    // no turn identifiers, so the boundary is the first assistant output
    // delta after the fixture plays; the browser starts the interrupting
    // audio synchronously at that event.
    peer.call(json!({"type":"arm_barge_in","name":"greeting"}))
        .await?;
    let greeting_audio_baseline = peer.audio_evidence().await?;
    let before = peer.events().await?.len();
    peer.call(json!({"type":"play","name":"greeting"})).await?;
    let interrupted_output = match rpc.wait_for_notification(OUTPUT_AVAILABLE, 45).await {
        Ok(output) => output,
        Err(error) => {
            let events = peer.events().await?;
            return Err(format!(
                "greeting produced no admitted assistant output: {error}; browser events: {}",
                peer.event_summary(&events[before..])
            )
            .into());
        }
    };
    assert_eq!(interrupted_output["channel_id"], channel_id);
    let truncated = rpc
        .call(
            "live/truncate",
            json!({
                "channel_id": interrupted_output["channel_id"],
                "output_id": interrupted_output["output_id"],
                "audio_played_ms": 0,
            }),
            30,
        )
        .await?;
    assert_eq!(
        truncated["status"], "truncated",
        "barge-in must retire the interrupted playback owner before another assistant output"
    );
    let greeting = wait_for_events(&mut peer, 90, |events| {
        let turn = &events[before..];
        let Some(start) = turn.iter().position(is_assistant_output) else {
            return false;
        };
        let Some(user_after) = turn[start + 1..]
            .iter()
            .position(is_user_input)
            .map(|offset| start + 1 + offset)
        else {
            return false;
        };
        turn[user_after + 1..].iter().any(is_assistant_output)
    })
    .await?;
    let snapshot = peer.snapshot().await?;
    assert_eq!(
        snapshot["barge_in"]["failures"].as_u64(),
        Some(0),
        "browser failed to start the armed barge-in fixture"
    );
    let barge_start = snapshot["barge_in"]["starts"]
        .as_array()
        .and_then(|starts| starts.first())
        .ok_or("browser did not start the armed barge-in fixture")?;
    let event_count_at_start: usize = barge_start["event_count_at_start"]
        .as_u64()
        .ok_or("browser barge-in evidence has no event count")?
        .try_into()?;
    assert!(
        event_count_at_start > before && event_count_at_start <= greeting.len(),
        "barge-in must start inside the greeting exchange"
    );
    assert!(
        is_assistant_output(&greeting[event_count_at_start - 1]),
        "the browser must start barge-in audio synchronously at the exact assistant-output boundary"
    );
    let admitted_user_index = greeting[event_count_at_start..]
        .iter()
        .position(is_user_input)
        .map(|offset| event_count_at_start + offset)
        .ok_or("provider did not admit the user speech started by the armed barge-in audio")?;
    assert!(
        greeting[admitted_user_index + 1..]
            .iter()
            .any(is_assistant_output),
        "provider did not answer the barge-in speech"
    );
    wait_for_spoken_output(&mut peer, greeting_audio_baseline, 30).await?;
    let greeting_output = rpc.wait_for_notification(OUTPUT_AVAILABLE, 45).await?;
    complete_playback(&mut rpc, &channel_id, &greeting_output).await?;
    assert!(
        !greeting[before..].iter().any(is_client_delegation),
        "simple greeting must remain in the same live conversation; {}",
        peer.event_summary(&greeting[before..])
    );

    // Phase B: spoken request that the voice model delegates to the
    // channel-bound Meerkat executor. The public API emits an opaque client
    // delegation with no task text; Meerkat joins it to the user transcript,
    // runs the executor, and appends the result as commentary.
    let before = greeting.len();
    peer.call(json!({"type":"play","name":"delegation"}))
        .await?;
    let joined = wait_for_events(&mut peer, 120, |events| {
        events[before..].iter().any(is_client_delegation)
    })
    .await?;
    let delegation_index = joined[before..]
        .iter()
        .position(is_client_delegation)
        .map(|offset| before + offset)
        .expect("joined delegation");
    let provider_delegation_ref = joined[delegation_index]["delegation"]["id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
        .ok_or("client delegation has no provider id")?
        .to_string();
    assert!(
        joined[before..delegation_index].iter().any(is_user_input),
        "the client delegation must follow admitted user speech; {}",
        peer.event_summary(&joined[before..])
    );

    // The delegated worker is a durable fork that the runtime retires as soon
    // as it reaches realized terminality, so the live roster is not reliable
    // evidence. Canonical mob events are: `member_spawned` for a
    // `live-delegation-*` identity followed by its `member_retired`.
    let executor_deadline = Instant::now() + Duration::from_secs(300);
    let mut delegation_outputs = 0usize;
    let worker_identity = loop {
        // Keep acknowledging assistant outputs (spoken acknowledgement and
        // result readout) so the machine can admit each synthesized turn.
        while let Some(output) = rpc
            .poll_notification(OUTPUT_AVAILABLE, Duration::from_millis(500))
            .await?
        {
            complete_playback(&mut rpc, &channel_id, &output).await?;
            delegation_outputs += 1;
        }
        let events = rpc
            .call(
                "mob/events",
                json!({"mob_id":mob_id,"after_cursor":0,"limit":200,"strict":true}),
                30,
            )
            .await?;
        let lifecycle = delegated_worker_lifecycle(&events);
        if let (Some(identity), true) = (&lifecycle.spawned, lifecycle.retired) {
            break identity.clone();
        }
        if Instant::now() >= executor_deadline {
            let events = peer.events().await?;
            return Err(format!(
                "timed out waiting for the delegated executor to finish; spawned={:?} retired={}; {}",
                lifecycle.spawned,
                lifecycle.retired,
                peer.event_summary(&events[before..])
            )
            .into());
        }
        sleep(Duration::from_millis(500)).await;
    };
    let post_result_audio_baseline = peer.audio_evidence().await?;
    let readout = wait_for_events(&mut peer, 120, |events| {
        events[delegation_index + 1..]
            .iter()
            .any(|event| event["type"] == "session.output_transcript.delta")
    })
    .await?;
    wait_for_spoken_output(&mut peer, post_result_audio_baseline, 60).await?;
    while let Some(output) = rpc
        .poll_notification(OUTPUT_AVAILABLE, Duration::from_secs(5))
        .await?
    {
        complete_playback(&mut rpc, &channel_id, &output).await?;
        delegation_outputs += 1;
    }
    assert!(
        delegation_outputs >= 1,
        "the voice model must publish at least one assistant output after the delegation"
    );
    let commentary_acks = readout[delegation_index + 1..]
        .iter()
        .filter(|event| event["type"] == "session.commentary.appended")
        .count();
    let provider_delegation_ref_digest = format!(
        "sha256:{:x}",
        Sha256::digest(provider_delegation_ref.as_bytes())
    );

    let events = rpc
        .call(
            "mob/events",
            json!({"mob_id":mob_id,"after_cursor":0,"limit":200,"strict":true}),
            30,
        )
        .await?;
    assert!(
        events["events"].as_array().is_some_and(|events| {
            events.iter().any(|event| {
                event.pointer("/kind/type").and_then(Value::as_str) == Some("member_spawned")
                    && event
                        .pointer("/kind/agent_identity")
                        .and_then(Value::as_str)
                        .is_some_and(|identity| identity.starts_with("live-delegation-"))
            })
        }),
        "durable delegated executor spawn did not materialize in canonical mob events"
    );

    rpc.call("live/close", json!({"channel_id":channel_id}), 30)
        .await?;
    peer.close().await;
    drop(rpc);
    server_task.abort();
    println!(
        "GPT_LIVE_PUBLIC_E2E_OK delegation_ref_digest={provider_delegation_ref_digest} delegation_index={delegation_index} delegation_outputs={delegation_outputs} commentary_acks_seen_by_browser={commentary_acks} worker={}",
        worker_identity
    );
    Ok(())
}

/// Collect every transcript delta text after `start` in browser event order.
fn output_transcript_text(events: &[Value], start: usize) -> String {
    events[start..]
        .iter()
        .filter(|event| event["type"] == "session.output_transcript.delta")
        .filter_map(|event| event["delta"].as_str().or_else(|| event["text"].as_str()))
        .collect::<Vec<_>>()
        .join("")
}

/// Every string under `text` or `content` keys of the session history, so the
/// check does not depend on one message shape (user text, assistant blocks).
fn history_text(history: &Value) -> String {
    fn walk(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    if (key == "text" || key == "content") && inner.is_string() {
                        out.push(inner.as_str().unwrap_or_default().to_string());
                    } else {
                        walk(inner, out);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(&history["messages"], &mut out);
    out.join("\n")
}

/// Scenario 98: the public Live lifecycle facts that no provider event
/// establishes on its own, driven against the real API.
///
/// 1. `live/playback_complete` is a caller-confirmed snapshot cut: the
///    assistant text reaches canonical session history right after the call
///    returns, without any provider turn-final event and without a quiet
///    interval.
/// 2. Later output in the same interaction gets a fresh one-use playback
///    handle that settles the same way.
/// 3. `live/close` drains and returns a closed status; the channel is gone
///    afterwards.
/// 4. Reopening the same session seeds the canonical dialogue as native
///    startup input with roles intact: the voice model recalls a code word
///    told to it before the close.
#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_scenario_98_gpt_live_public_playback_settlement_and_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            "oai_rt_rs::live=debug,meerkat_openai::public_live=debug,meerkat::experimental_gpt_live=debug,meerkat::session_runtime=debug,meerkat_live=debug,meerkat_rpc=debug",
        )
        .with_test_writer()
        .try_init();
    require_api_key()?;
    let mut live = open_public_live("gpt-live-public-reopen-e2e-", "scenario-98-operator").await?;
    let session_id = live.session_id.clone();

    // Phase A: tell the model a code word and confirm playback of its reply.
    let before = live.peer.events().await?.len();
    let audio_baseline = live.peer.audio_evidence().await?;
    live.peer
        .call(json!({"type":"play","name":"remember"}))
        .await?;
    let first_output = live.rpc.wait_for_notification(OUTPUT_AVAILABLE, 60).await?;
    assert_eq!(first_output["channel_id"], live.channel_id);
    wait_for_spoken_output(&mut live.peer, audio_baseline, 45).await?;
    let confirmed_at = Instant::now();
    complete_playback(&mut live.rpc, &live.channel_id, &first_output).await?;
    // The snapshot cut commits without a provider final: the assistant text
    // must be in canonical history promptly, bounded well under the retired
    // 1.5 s quiet heuristic plus its 2.5 s readout grace.
    let settle_deadline = confirmed_at + Duration::from_secs(3);
    let history = loop {
        let history = live
            .rpc
            .call(
                "session/history",
                json!({"session_id":session_id,"offset":0,"limit":200}),
                30,
            )
            .await?;
        let text = history_text(&history);
        if history["messages"].as_array().is_some_and(|messages| {
            messages.iter().any(|message| {
                message["role"]
                    .as_str()
                    .is_some_and(|role| role.contains("assistant"))
            })
        }) && !text.trim().is_empty()
        {
            break history;
        }
        if Instant::now() >= settle_deadline {
            return Err(format!(
                "caller-confirmed playback did not settle into session history within 3 s; history: {}",
                serde_json::to_string(&history)?
            )
            .into());
        }
        sleep(Duration::from_millis(100)).await;
    };
    let settled_after = confirmed_at.elapsed();
    let events = live.peer.events().await?;
    assert!(
        events[before..].iter().any(is_user_input),
        "provider did not admit the code-word instruction; {}",
        live.peer.event_summary(&events[before..])
    );

    // Phase B: later output in the same interaction gets a fresh handle.
    let before_second = events.len();
    let audio_baseline = live.peer.audio_evidence().await?;
    live.peer
        .call(json!({"type":"play","name":"greeting"}))
        .await?;
    let second_output = live.rpc.wait_for_notification(OUTPUT_AVAILABLE, 60).await?;
    assert_eq!(second_output["channel_id"], live.channel_id);
    assert_ne!(
        second_output["output_id"], first_output["output_id"],
        "continuation after a settled snapshot cut must carry a fresh one-use playback handle"
    );
    wait_for_spoken_output(&mut live.peer, audio_baseline, 45).await?;
    complete_playback(&mut live.rpc, &live.channel_id, &second_output).await?;
    // Any further outputs (the model may split its reply) settle the same way.
    while let Some(output) = live
        .rpc
        .poll_notification(OUTPUT_AVAILABLE, Duration::from_secs(3))
        .await?
    {
        complete_playback(&mut live.rpc, &live.channel_id, &output).await?;
    }
    let events = live.peer.events().await?;
    assert!(
        events[before_second..].iter().any(is_assistant_output),
        "second exchange produced no assistant output; {}",
        live.peer.event_summary(&events[before_second..])
    );

    // Phase C: close drains provider observations and confirms closure.
    let status = live
        .rpc
        .call("live/status", json!({"channel_id":live.channel_id}), 30)
        .await?;
    assert_eq!(status["channel_id"], live.channel_id);
    let closed = live
        .rpc
        .call("live/close", json!({"channel_id":live.channel_id}), 60)
        .await?;
    assert_eq!(
        closed["status"], "closed",
        "live/close must report a confirmed closure"
    );
    let after_close = live
        .rpc
        .call_raw("live/status", json!({"channel_id":live.channel_id}), 30)
        .await?;
    assert!(
        !after_close["error"].is_null()
            || after_close["result"]["status"]
                .to_string()
                .to_lowercase()
                .contains("closed"),
        "a closed channel must not report as live: {after_close}"
    );
    let history_after_close = live
        .rpc
        .call(
            "session/history",
            json!({"session_id":session_id,"offset":0,"limit":200}),
            30,
        )
        .await?;
    let committed_before_reopen = history_text(&history_after_close);
    assert!(
        committed_before_reopen.contains(&history_text(&history)),
        "close must not lose transcript text committed by the earlier snapshot cut"
    );

    // Phase D: reopen the same session; the canonical dialogue is seeded as
    // native startup input, so the model can answer from it.
    live.reopen().await?;
    let before_recall = live.peer.events().await?.len();
    let audio_baseline = live.peer.audio_evidence().await?;
    live.peer
        .call(json!({"type":"play","name":"recall"}))
        .await?;
    let recall_output = live.rpc.wait_for_notification(OUTPUT_AVAILABLE, 60).await?;
    assert_eq!(recall_output["channel_id"], live.channel_id);
    let recalled = wait_for_events(&mut live.peer, 60, |events| {
        output_transcript_text(events, before_recall)
            .to_lowercase()
            .contains("tangerine")
    })
    .await
    .map_err(|error| {
        format!("reopened session did not recall the code word from the seeded dialogue: {error}")
    })?;
    wait_for_spoken_output(&mut live.peer, audio_baseline, 45).await?;
    complete_playback(&mut live.rpc, &live.channel_id, &recall_output).await?;
    while let Some(output) = live
        .rpc
        .poll_notification(OUTPUT_AVAILABLE, Duration::from_secs(3))
        .await?
    {
        complete_playback(&mut live.rpc, &live.channel_id, &output).await?;
    }
    let recall_text = output_transcript_text(&recalled, before_recall);
    let closed = live
        .rpc
        .call("live/close", json!({"channel_id":live.channel_id}), 60)
        .await?;
    assert_eq!(closed["status"], "closed");
    live.peer.close().await;
    live.server_task.abort();
    println!(
        "GPT_LIVE_PUBLIC_REOPEN_E2E_OK settled_after_ms={} recall_transcript={:?} mob={}",
        settled_after.as_millis(),
        recall_text.trim(),
        live.mob_id
    );
    Ok(())
}

#[derive(Debug, Default)]
struct DelegatedWorkerLifecycle {
    spawned: Option<String>,
    retired: bool,
}

/// Canonical mob-event evidence for the durable delegated worker: its spawn
/// and, once the bounded turn reached terminality, its retirement.
fn delegated_worker_lifecycle(events: &Value) -> DelegatedWorkerLifecycle {
    let mut lifecycle = DelegatedWorkerLifecycle::default();
    let Some(events) = events["events"].as_array() else {
        return lifecycle;
    };
    for event in events {
        let kind = event.pointer("/kind/type").and_then(Value::as_str);
        let identity = event
            .pointer("/kind/agent_identity")
            .and_then(Value::as_str)
            .filter(|identity| identity.starts_with("live-delegation-"));
        match (kind, identity) {
            (Some("member_spawned"), Some(identity)) => {
                lifecycle.spawned = Some(identity.to_string());
            }
            (Some("member_retired"), Some(identity))
                if lifecycle.spawned.as_deref() == Some(identity) =>
            {
                lifecycle.retired = true;
            }
            _ => {}
        }
    }
    lifecycle
}

#[cfg(test)]
mod config_tests {
    use super::{API_KEY_ENV, BINDING, REALM, scenario_config};
    use meerkat_core::CredentialSourceSpec;

    #[test]
    fn realm_binding_sources_the_api_key_from_the_environment() {
        let config = scenario_config();
        let section = config.realm.get(REALM).expect("scenario realm");
        let auth = section.auth.get(BINDING).expect("api key auth profile");
        assert_eq!(auth.auth_method, "api_key");
        assert_eq!(
            auth.source,
            CredentialSourceSpec::Env {
                env: API_KEY_ENV.to_string(),
                fallback: Vec::new(),
            }
        );
        let binding = section.binding.get(BINDING).expect("binding");
        assert_eq!(
            section.backend[&binding.backend_profile].backend_kind,
            "openai_api"
        );
        assert_eq!(section.default_binding.as_deref(), Some(BINDING));
    }
}
