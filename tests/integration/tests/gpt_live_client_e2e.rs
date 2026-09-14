#![cfg(all(feature = "experimental-gpt-live-e2e", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "support/gpt_live_e2e.rs"]
mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use meerkat::experimental_gpt_live::{
    ExperimentalGptLiveOpenAuthority, ExperimentalGptLiveOpenAuthorityConfig,
    ExperimentalGptLiveWebrtcTransport,
};
use meerkat_core::handles::LeaseKey;
use meerkat_core::{
    AuthBindingRef, BackendProfileConfig, BindingId, BindingOrigin, BindingPolicy, BlobStore,
    Config, ConfigRuntime, ConfigStore, CredentialSourceSpec, MemoryConfigStore,
    ProviderBindingConfig, RealmConfigSection, RealmId,
};
use meerkat_providers::auth_store::{
    FileTokenStore, InMemoryCoordinator, PersistedAuthMode, PersistedTokens,
    ProviderAuthPersistence, TokenKey, TokenStore,
};
use meerkat_rpc::router::NotificationSink;
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::BufReader;
use tokio::time::{Duration, Instant, sleep, timeout};

use support::{
    BrowserPeer, BrowserPeerProtocol, ExplicitScenarioBindingAuthority, FixedConfigSource,
    JsonlRpcClient, delegated_executor_diagnostic, execution_identity, wait_for_events,
    wait_for_spoken_output,
};

const REALM: &str = "scenario-96-gpt-live-client";
const BINDING: &str = "openai_oauth";
const CLIENT_PROFILE: &str = "openai.gpt-live-1-codex.client-context.v1";
const FUNCTION_BRIDGE_PROFILE: &str = "openai.gpt-live-1-codex.function-bridge.v1";
const MIN_TTL_SECS: i64 = 5 * 60;

fn auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(REALM).expect("valid realm"),
        binding: BindingId::parse(BINDING).expect("valid binding"),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

fn required_tokens() -> Result<PersistedTokens, Box<dyn std::error::Error>> {
    let raw = std::env::var("MEERKAT_E2E_AUTH_OPENAI_OAUTH_TOKENS_JSON")
        .map_err(|_| "MEERKAT_E2E_AUTH_OPENAI_OAUTH_TOKENS_JSON is required")?;
    let content = if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path)?
    } else if Path::new(&raw).exists() {
        std::fs::read_to_string(&raw)?
    } else {
        raw
    };
    let tokens: PersistedTokens = serde_json::from_str(&content)
        .map_err(|error| format!("invalid OpenAI OAuth token bundle: {error}"))?;
    let now: i64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?;
    prepare_remote_tokens(tokens, now)
}

fn prepare_remote_tokens(
    mut tokens: PersistedTokens,
    now_epoch_secs: i64,
) -> Result<PersistedTokens, Box<dyn std::error::Error>> {
    if tokens.auth_mode != PersistedAuthMode::ChatgptOauth
        || tokens
            .primary_secret
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err("scenario 96 requires a complete chatgpt_oauth access-token bundle".into());
    }
    if tokens
        .account_id
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        let id_token = tokens
            .id_token
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or("scenario 96 OAuth bundle has neither account_id nor an id_token")?;
        let claims = meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
            .map_err(|_| "scenario 96 OAuth id_token payload is invalid")?;
        tokens.account_id = claims
            .raw
            .get("https://api.openai.com/auth")
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .and_then(Value::as_str)
            .or(claims.chatgpt_account_id.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
    }
    if tokens.account_id.is_none() {
        return Err("scenario 96 OAuth bundle has no supported ChatGPT account-id claim".into());
    }
    let expires_at = tokens
        .expires_at
        .as_ref()
        .ok_or("scenario 96 OAuth bundle has no expires_at")?
        .timestamp();
    if expires_at <= now_epoch_secs + MIN_TTL_SECS {
        return Err("scenario 96 OAuth access token is expired or within five minutes of expiry; rotate the encrypted secret locally".into());
    }
    // The source bundle is immutable. The remote action receives no refresh
    // authority because it cannot write a rotated refresh token back.
    tokens.refresh_token = None;
    Ok(tokens)
}

fn scenario_config() -> Config {
    let executor_model =
        std::env::var("GPT_LIVE_E2E_EXECUTOR_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".to_string());
    let mut section = RealmConfigSection {
        backend: BTreeMap::new(),
        auth: BTreeMap::new(),
        binding: BTreeMap::new(),
        default_binding: Some(BINDING.to_string()),
        parent: None,
    };
    section.backend.insert(
        "chatgpt_backend".to_string(),
        BackendProfileConfig {
            provider: "openai".to_string(),
            backend_kind: "chatgpt_backend".to_string(),
            base_url: None,
            options: Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        BINDING.to_string(),
        meerkat_core::AuthProfileConfig {
            provider: "openai".to_string(),
            auth_method: "managed_chatgpt_oauth".to_string(),
            source: CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        BINDING.to_string(),
        ProviderBindingConfig {
            backend_profile: "chatgpt_backend".to_string(),
            auth_profile: BINDING.to_string(),
            credential_account: None,
            default_model: Some(executor_model),
            policy: BindingPolicy::default(),
            provider_default: false,
        },
    );
    let mut config = Config::default();
    config.realm.insert(REALM.to_string(), section);
    config.model_fallback.enabled = Some(false);
    config
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_scenario_96_gpt_live_client_context_vertical() -> Result<(), Box<dyn std::error::Error>>
{
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            "oai_rt_rs::experimental::gpt_live=debug,meerkat_openai::gpt_live=debug,meerkat::experimental_gpt_live=warn,meerkat_runtime::meerkat_machine::runtime_control=debug,meerkat_mob_mcp::live_delegation=debug,meerkat_mob::runtime::delegation=debug",
        )
        .with_test_writer()
        .try_init();
    let tokens = required_tokens()?;
    let temp = tempfile::Builder::new()
        .prefix("gpt-live-client-e2e-")
        .tempdir_in(support::test_tmp_root()?)?;
    let config = scenario_config();
    let binding = auth_binding();
    let token_store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(
        temp.path().join("xdg/meerkat/credentials"),
    ));
    let operator = meerkat::ExperimentalLiveOperatorConfig::gpt_live_client_context();
    let factory_identity = operator.factory().clone();
    let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
        .runtime_root(temp.path().join("runtime"))
        .project_root(temp.path().join("project"))
        .context_root(temp.path().join("project"))
        .builtins(true)
        .shell(true)
        .mob(true)
        .with_provider_auth_persistence(ProviderAuthPersistence::new(
            Arc::clone(&token_store),
            Arc::new(InMemoryCoordinator::new()),
        ))
        .with_experimental_live_admission(operator, [binding.realm.clone()]);
    let live_factory_owner = factory.clone();
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
    let token_key = TokenKey::from_auth_binding(&binding);
    let transition = runtime.auth_lease_handle().acquire_lease(
        &LeaseKey::from_auth_binding(&binding),
        meerkat_core::persisted_token_expires_at_epoch_secs(&tokens),
    )?;
    let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
        &token_key,
        &tokens,
        &transition,
    )?;
    token_store.save(&token_key, &marked).await?;
    drop(tokens);

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
    let mob_id = format!("gpt-live-client-e2e-{}", std::process::id());
    rpc.call(
        "mob/create",
        json!({"definition":{"id":mob_id,"profiles":{"executor":{
            "model":std::env::var("GPT_LIVE_E2E_EXECUTOR_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".to_string()),
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

    let experimental_transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
    let open_authority = Arc::new(ExperimentalGptLiveOpenAuthority::new(
        ExperimentalGptLiveOpenAuthorityConfig {
            agent_factory: factory.clone(),
            config_source: Arc::new(FixedConfigSource(config)),
            binding_authority: Arc::new(ExplicitScenarioBindingAuthority {
                session_id: session_id.clone(),
                binding: binding.clone(),
                auth_lease: runtime.generated_auth_lease_handle(),
                mobs: Arc::clone(&mobs),
                principal_id: "scenario-96-operator",
            }),
            execution_identity: meerkat_core::SessionLlmIdentity {
                model: "gpt-live-1-codex".to_string(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: Some(binding.clone()),
            },
            realm: binding.realm.clone(),
            factory_identity,
            transport: Arc::clone(&experimental_transport),
            voice: "cove".to_string(),
        },
    )?);
    // Rebuild the connection host with the exact authenticated authority. The
    // authority is a public production constructor; no Gate0 test witness is
    // minted by this fixture.
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
        &live_factory_owner,
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
    .with_live_webrtc_answer_transport(experimental_transport)
    .with_experimental_live_open_authority(open_authority);
    let server_task = tokio::spawn(async move { server.run().await });
    rpc.call("initialize", json!({}), 60).await?;

    let rejected = rpc
        .call_raw(
            "live/open",
            json!({"session_id":session_id,"transport":"webrtc",
                "execution_identity":execution_identity(FUNCTION_BRIDGE_PROFILE)}),
            30,
        )
        .await?;
    assert!(
        !rejected["error"].is_null(),
        "FunctionBridge must fail closed"
    );
    let open = rpc
        .call(
            "live/open",
            json!({"session_id":session_id,"transport":"webrtc",
                "execution_identity":execution_identity(CLIENT_PROFILE)}),
            60,
        )
        .await?;

    let mut peer = BrowserPeer::start(BrowserPeerProtocol::Experimental).await?;
    let offer = peer.call(json!({"type":"prepare"})).await?;
    let answer = rpc
        .call(
            open["transport"]["answer_method"]
                .as_str()
                .unwrap_or("live/webrtc/answer"),
            json!({"channel_id":open["channel_id"],"token":open["transport"]["token"],
                "offer_sdp":offer["offer_sdp"]}),
            90,
        )
        .await?;
    peer.call(json!({"type":"answer","answer_sdp":answer["answer_sdp"]}))
        .await?;

    peer.call(json!({"type":"arm_barge_in","name":"greeting"}))
        .await?;
    let greeting_audio_baseline = peer.audio_evidence().await?;
    let before = peer.events().await?.len();
    peer.call(json!({"type":"play","name":"greeting"})).await?;
    let interrupted_output = rpc
        .wait_for_notification("live/assistant_output_available", 30)
        .await?;
    assert_eq!(interrupted_output["channel_id"], open["channel_id"]);
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
        turn.iter()
            .filter(|event| event["type"] == "turn.done" && event["turn"]["role"] == "user")
            .count()
            >= 2
            && turn
                .iter()
                .filter(|event| {
                    event["type"] == "turn.done" && event["turn"]["role"] == "assistant"
                })
                .count()
                >= 2
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
    let interrupted_assistant = &barge_start["assistant_turn_id"];
    let event_count_at_start: usize = barge_start["event_count_at_start"]
        .as_u64()
        .ok_or("browser barge-in evidence has no event count")?
        .try_into()?;
    let interrupted_start_index = greeting
        .iter()
        .position(|event| {
            event["type"] == "turn.created"
                && event["turn"]["role"] == "assistant"
                && &event["turn"]["id"] == interrupted_assistant
        })
        .ok_or("armed barge-in has no exact assistant start")?;
    let interrupted_done_index = greeting
        .iter()
        .position(|event| {
            event["type"] == "turn.done"
                && event["turn"]["role"] == "assistant"
                && &event["turn"]["id"] == interrupted_assistant
        })
        .ok_or("armed barge-in has no exact assistant terminal")?;
    assert_eq!(
        event_count_at_start,
        interrupted_start_index + 1,
        "the browser must start barge-in audio synchronously at the exact assistant-start boundary"
    );
    assert!(
        interrupted_done_index >= event_count_at_start,
        "barge-in audio must start before the interrupted assistant terminalizes"
    );
    assert!(
        greeting
            .iter()
            .enumerate()
            .any(|(index, event)| index >= event_count_at_start
                && event["type"] == "turn.created"
                && event["turn"]["role"] == "user"),
        "provider did not admit the user turn started by the armed barge-in audio"
    );
    wait_for_spoken_output(&mut peer, greeting_audio_baseline, 30).await?;
    let greeting_output = rpc
        .wait_for_notification("live/assistant_output_available", 30)
        .await?;
    assert_eq!(greeting_output["channel_id"], open["channel_id"]);
    rpc.call(
        "live/playback_complete",
        json!({
            "channel_id": greeting_output["channel_id"],
            "output_id": greeting_output["output_id"],
        }),
        30,
    )
    .await?;
    assert!(
        !greeting[before..]
            .iter()
            .any(|event| event["type"] == "delegation.created"),
        "simple greeting must remain in the same live conversation; {}",
        peer.event_summary(&greeting[before..])
    );

    let before = greeting.len();
    let delegation_audio_baseline = peer.audio_evidence().await?;
    peer.call(json!({"type":"play","name":"delegation"}))
        .await?;
    let joined = wait_for_events(&mut peer, 120, |events| {
        let events = &events[before..];
        let Some(delegation) = events
            .iter()
            .find(|event| event["type"] == "delegation.created")
        else {
            return false;
        };
        let turn_id = &delegation["item"]["user_bidi_turn_id"];
        events.iter().any(|event| {
            event["type"] == "turn.done"
                && event["turn"]["role"] == "user"
                && &event["turn"]["id"] == turn_id
        })
    })
    .await?;
    let delegation = joined[before..]
        .iter()
        .find(|event| event["type"] == "delegation.created")
        .expect("joined delegation");
    assert_eq!(delegation["item"]["target"], "client");
    let provider_delegation_ref = delegation["item"]["id"]
        .as_str()
        .ok_or("joined delegation has no provider item id")?
        .to_string();
    let joined_user_turn_id = delegation["item"]["user_bidi_turn_id"]
        .as_str()
        .ok_or("joined delegation has no provider user turn id")?
        .to_string();
    let acknowledgement_output = rpc
        .wait_for_notification("live/assistant_output_available", 30)
        .await?;
    assert_eq!(acknowledgement_output["channel_id"], open["channel_id"]);
    wait_for_spoken_output(&mut peer, delegation_audio_baseline, 30).await?;
    rpc.call(
        "live/playback_complete",
        json!({
            "channel_id": acknowledgement_output["channel_id"],
            "output_id": acknowledgement_output["output_id"],
        }),
        30,
    )
    .await?;
    let append_deadline = Instant::now() + Duration::from_secs(240);
    let acked = loop {
        let events = peer.events().await?;
        if events[before..].iter().any(|event| {
            event["type"] == "delegation.context.appended"
                && event["delegation_item_id"].as_str() == Some(provider_delegation_ref.as_str())
        }) {
            break events;
        }
        let now = Instant::now();
        if now >= append_deadline {
            let executor_diagnostic = timeout(
                Duration::from_secs(5),
                delegated_executor_diagnostic(&mut rpc, &mob_id),
            )
            .await
            .map_or_else(
                |_| "diagnostic=timed_out".to_string(),
                |diagnostic| diagnostic.to_string(),
            );
            return Err(format!(
                "timed out waiting for delegation context append; {executor_diagnostic}"
            )
            .into());
        }
        sleep(Duration::from_millis(250)).await;
    };
    let ack_index = acked
        .iter()
        .position(|event| {
            event["type"] == "delegation.context.appended"
                && event["delegation_item_id"].as_str() == Some(provider_delegation_ref.as_str())
        })
        .expect("exact context append ack");
    let joined_turn_done_index = acked
        .iter()
        .position(|event| {
            event["type"] == "turn.done"
                && event["turn"]["role"] == "user"
                && event["turn"]["id"].as_str() == Some(joined_user_turn_id.as_str())
        })
        .expect("exact joined user turn.done");
    assert!(
        joined_turn_done_index < ack_index,
        "exact joined user turn.done at index {joined_turn_done_index} must precede the exact delegation.context.appended at index {ack_index}"
    );
    let provider_delegation_ref_digest = format!(
        "sha256:{:x}",
        Sha256::digest(provider_delegation_ref.as_bytes())
    );
    assert_eq!(provider_delegation_ref_digest.len(), "sha256:".len() + 64);
    let post_append_audio_baseline = peer.audio_evidence().await?;
    wait_for_events(&mut peer, 90, |events| {
        events[ack_index + 1..]
            .iter()
            .any(|event| event["type"] == "turn.done" && event["turn"]["role"] == "assistant")
    })
    .await?;
    wait_for_spoken_output(&mut peer, post_append_audio_baseline, 30).await?;
    let delegated_output = rpc
        .wait_for_notification("live/assistant_output_available", 30)
        .await?;
    assert_eq!(delegated_output["channel_id"], open["channel_id"]);
    rpc.call(
        "live/playback_complete",
        json!({
            "channel_id": delegated_output["channel_id"],
            "output_id": delegated_output["output_id"],
        }),
        30,
    )
    .await?;

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

    rpc.call("live/close", json!({"channel_id":open["channel_id"]}), 30)
        .await?;
    peer.close().await;
    drop(rpc);
    server_task.abort();
    println!(
        "GPT_LIVE_CLIENT_E2E_OK delegation_ref_digest={provider_delegation_ref_digest} joined_turn_done_index={joined_turn_done_index} context_appended_index={ack_index}"
    );
    Ok(())
}

#[cfg(test)]
mod token_tests {
    use base64::Engine;

    use super::{PersistedTokens, prepare_remote_tokens};

    fn unsigned_jwt(payload: serde_json::Value) -> String {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = engine.encode(br#"{"alg":"none"}"#);
        let payload = engine.encode(serde_json::to_vec(&payload).expect("encode JWT payload"));
        format!("{header}.{payload}.fixture-signature")
    }

    fn canonical_tokens(payload: serde_json::Value) -> PersistedTokens {
        serde_json::from_value(serde_json::json!({
            "auth_mode": "chatgpt_oauth",
            "primary_secret": "access-fixture",
            "refresh_token": "must-not-reach-remote",
            "id_token": unsigned_jwt(payload),
            "expires_at": 2_000,
            "scopes": [],
            "metadata": {}
        }))
        .expect("canonical token fixture")
    }

    #[test]
    fn remote_bundle_lifts_nested_account_id_and_strips_refresh_authority() {
        let tokens = canonical_tokens(serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct-nested"}
        }));
        let prepared = prepare_remote_tokens(tokens, 1_000).expect("prepare nested claim");
        assert_eq!(prepared.account_id.as_deref(), Some("acct-nested"));
        assert_eq!(prepared.refresh_token, None);
    }

    #[test]
    fn remote_bundle_lifts_top_level_account_id() {
        let tokens = canonical_tokens(serde_json::json!({
            "chatgpt_account_id": "acct-top-level"
        }));
        let prepared = prepare_remote_tokens(tokens, 1_000).expect("prepare top-level claim");
        assert_eq!(prepared.account_id.as_deref(), Some("acct-top-level"));
    }

    #[test]
    fn remote_bundle_fails_closed_without_supported_account_id() {
        let tokens = canonical_tokens(serde_json::json!({"sub": "user-only"}));
        let error = prepare_remote_tokens(tokens, 1_000).expect_err("missing account claim");
        assert!(
            error
                .to_string()
                .contains("no supported ChatGPT account-id claim"),
            "unexpected credential-maintenance error: {error}"
        );
    }
}
