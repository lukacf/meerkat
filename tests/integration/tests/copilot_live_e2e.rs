#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//!
//! Real-account GitHub Copilot coverage for shared auth, tools, multimodal
//! input, and runtime model/provider switching.
//!
//! Run after `rkat auth login copilot` with `make e2e-copilot-live`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::{
    AgentBuildConfig, AgentFactory, AgentToolDispatcher, Config, PersistenceBundle, ToolDef,
    ToolError, ToolResult,
};
use meerkat_copilot::{
    CopilotModelAccess, CopilotModelCapabilities, CopilotModelSnapshot, CopilotProviderRoute,
    CopilotResolvedAuth, CopilotRuntime, GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID,
};
use meerkat_core::auth::{PersistedAuthMode, ProviderAuthPersistence, TokenKey};
use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
use meerkat_core::service::SessionHistoryQuery;
use meerkat_core::{
    AuthBindingRef, AuthCredentialIdentity, BindingId, BindingOrigin, ConfigRuntime, ContentBlock,
    ContentInput, CredentialAccountId, CredentialAccountRef, ImageData, MemoryConfigStore, Message,
    Provider, RealmConnectionSet, RealmId, ToolCallView, ToolDispatchOutcome,
};
use meerkat_integration_tests::e2e_lanes::strict_prereqs_enabled;
use meerkat_providers::auth_store::TokenStoreBackend;
use meerkat_providers::{ProviderRuntimeCatalog, ResolverEnvironment};
use meerkat_rpc::handlers::turn::TurnOverrides;
use meerkat_rpc::router::NotificationSink;
use meerkat_rpc::session_runtime::SessionRuntime;
use serde::Deserialize;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;

const TOOL_NONCE: &str = "meerkat-copilot-live-tool";
const DEFAULT_MAX_TOKENS: u32 = 512;
const IMAGE_MARKER: &str = "RKAT7319";
const MARKER_PNG: &str = concat!(
    "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",
    "Z87GeynbDQAAAABJRU5ErkJggg==",
);

#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteModel {
    route: CopilotProviderRoute,
    provider: Provider,
    model: String,
    capabilities: CopilotModelCapabilities,
    registry_image_input: bool,
    registry_max_output_tokens: Option<u32>,
}

impl RouteModel {
    fn supports_tools(&self) -> bool {
        self.capabilities.supports.tool_calls == Some(true)
    }

    fn supports_vision(&self) -> bool {
        self.registry_image_input && self.capabilities.supports.vision == Some(true)
    }

    fn max_tokens(&self) -> u32 {
        self.capabilities
            .limits
            .max_output_tokens
            .into_iter()
            .chain(self.registry_max_output_tokens)
            .min()
            .unwrap_or(DEFAULT_MAX_TOKENS)
            .min(DEFAULT_MAX_TOKENS)
    }
}

#[derive(Debug, Deserialize)]
struct ProbeArgs {
    nonce: String,
}

struct RecordingProbeDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    calls: Mutex<Vec<String>>,
}

impl RecordingProbeDispatcher {
    fn new() -> Self {
        Self {
            tools: vec![Arc::new(ToolDef::new(
                "live_probe",
                "Record the exact nonce supplied by the caller.",
                json!({
                    "type": "object",
                    "properties": {
                        "nonce": { "type": "string" }
                    },
                    "required": ["nonce"],
                    "additionalProperties": false
                }),
            ))]
            .into(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("probe call lock").clone()
    }
}

#[async_trait]
impl AgentToolDispatcher for RecordingProbeDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let args: ProbeArgs = call
            .parse_args()
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        self.calls
            .lock()
            .expect("probe call lock")
            .push(args.nonce.clone());
        Ok(ToolResult::new(
            call.id.to_string(),
            json!({"recorded": args.nonce}).to_string(),
            false,
        )
        .into())
    }
}

fn skip_or_fail(reason: impl AsRef<str>) {
    let reason = reason.as_ref();
    if strict_prereqs_enabled() {
        panic!("Copilot live suite prerequisite failed: {reason}");
    }
    eprintln!("SKIP copilot-live: {reason}");
}

fn account_identity() -> AuthCredentialIdentity {
    AuthCredentialIdentity::Account(CredentialAccountRef {
        realm: RealmId::global(),
        account: CredentialAccountId::parse(GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID)
            .expect("canonical Copilot account id"),
    })
}

fn auth_binding(route: CopilotProviderRoute) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::global(),
        binding: BindingId::parse(route.binding_id()).expect("canonical Copilot binding id"),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

fn validate_copilot_routes(config: &Config) -> Result<(), String> {
    let expected_account = CredentialAccountId::parse(GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID)
        .expect("canonical Copilot account id");
    let expected_identity = account_identity();
    let section = config
        .realm
        .get("global")
        .ok_or_else(|| "global realm config is missing".to_string())?;
    let realm = RealmConnectionSet::from_config("global", section)
        .map_err(|error| format!("global Copilot route config is invalid: {error}"))?;
    let mut canonical_options = None;
    for route in CopilotProviderRoute::ALL {
        let route_id = route.binding_id();
        let backend_config = section
            .backend
            .get(route_id)
            .ok_or_else(|| format!("reserved backend profile {route_id} is missing"))?;
        if backend_config.provider != route.provider().as_str()
            || backend_config.backend_kind != route.backend_kind()
            || backend_config.base_url.is_some()
            || backend_config.server.is_some()
        {
            return Err(format!(
                "{route_id} is not the canonical {} Copilot backend",
                route.provider().as_str()
            ));
        }
        if let Some(expected) = canonical_options {
            if &backend_config.options != expected {
                return Err("reserved Copilot routes have different backend options".to_string());
            }
        } else {
            canonical_options = Some(&backend_config.options);
        }

        let auth_config = section
            .auth
            .get(route_id)
            .ok_or_else(|| format!("reserved auth profile {route_id} is missing"))?;
        if auth_config.provider != route.provider().as_str()
            || auth_config.auth_method != route.auth_method()
            || !matches!(
                auth_config.source,
                meerkat_core::CredentialSourceSpec::ManagedStore
            )
            || !auth_config.constraints.allow_interactive_login
        {
            return Err(format!(
                "{route_id} is not the canonical {} Copilot auth profile",
                route.provider().as_str()
            ));
        }

        let binding_config = section
            .binding
            .get(route_id)
            .ok_or_else(|| format!("reserved binding {route_id} is missing"))?;
        if binding_config.backend_profile != route_id
            || binding_config.auth_profile != route_id
            || binding_config.credential_account.as_ref() != Some(&expected_account)
        {
            return Err(format!(
                "{route_id} does not bind its matching profiles to account {expected_account}"
            ));
        }

        let binding_ref = auth_binding(route);
        let (binding, backend, auth) = realm
            .lookup_auth_binding(&binding_ref)
            .map_err(|error| format!("{} is missing or invalid: {error}", route.binding_id()))?;
        let identity = binding.credential_identity(&binding_ref);
        if identity != expected_identity {
            return Err(format!(
                "{} resolves credential identity {identity}, expected {expected_identity}",
                route.binding_id()
            ));
        }
        ProviderRuntimeCatalog::validate_binding_with_credential_identity(
            &binding_ref,
            identity,
            backend,
            auth,
            &binding.policy,
        )
        .map_err(|error| format!("{} failed provider validation: {error}", route.binding_id()))?;
    }
    Ok(())
}

async fn resolve_account(
    config: &Config,
    persistence: ProviderAuthPersistence,
) -> Result<CopilotResolvedAuth, String> {
    let binding_ref = auth_binding(CopilotProviderRoute::OpenAi);
    let realm = RealmConnectionSet::from_config(
        "global",
        config
            .realm
            .get("global")
            .expect("global Copilot route config"),
    )
    .expect("valid global Copilot route config");
    let (binding, backend, auth) = realm
        .lookup_auth_binding(&binding_ref)
        .expect("configured Copilot OpenAI binding");
    let validated = ProviderRuntimeCatalog::validate_binding_with_credential_identity(
        &binding_ref,
        binding.credential_identity(&binding_ref),
        backend,
        auth,
        &binding.policy,
    )
    .expect("valid Copilot provider binding");
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let environment = ResolverEnvironment::testing()
        .with_provider_auth_persistence(persistence)
        .with_auth_lease_handle(machine.generated_auth_lease_handle());
    CopilotRuntime::new()
        .resolve(&validated, &environment)
        .await
        .map_err(|error| error.to_string())
}

fn route_models(config: &Config, snapshot: &CopilotModelSnapshot) -> Vec<RouteModel> {
    let registry = config
        .model_registry(meerkat_models::canonical())
        .expect("effective model registry");
    let mut models = Vec::new();
    for route in CopilotProviderRoute::ALL {
        let provider = route.provider();
        for model in snapshot.available_model_ids(provider) {
            let Some(offering) = snapshot.model(model) else {
                continue;
            };
            let Some(profile) = registry.profile_witness_for_provider(provider, model) else {
                continue;
            };
            if offering.model_picker_enabled == Some(false)
                || offering.capabilities.supports.streaming == Some(false)
                || offering.capabilities.limits.max_output_tokens == Some(0)
                || profile.max_output_tokens() == Some(0)
            {
                continue;
            }
            models.push(RouteModel {
                route,
                provider,
                model: model.to_string(),
                capabilities: offering.capabilities.clone(),
                registry_image_input: profile.profile().image_input,
                registry_max_output_tokens: profile.max_output_tokens(),
            });
        }
    }
    models
}

fn turn_overrides(current: &RouteModel, target: &RouteModel) -> Option<TurnOverrides> {
    if current == target {
        return None;
    }
    Some(TurnOverrides {
        model: Some(target.model.clone()),
        provider: Some(target.provider.as_str().to_string()),
        auth_binding: Some(TurnMetadataOverride::Set(auth_binding(target.route))),
        ..Default::default()
    })
}

async fn run_turn(
    runtime: &Arc<SessionRuntime>,
    session_id: &meerkat_core::SessionId,
    prompt: ContentInput,
    current: &RouteModel,
    target: &RouteModel,
) -> meerkat_core::RunResult {
    let (event_tx, _event_rx) = mpsc::channel(4096);
    timeout(
        Duration::from_secs(180),
        runtime.start_turn_via_runtime(
            session_id,
            prompt,
            Vec::new(),
            event_tx,
            None,
            None,
            None,
            turn_overrides(current, target),
        ),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "Copilot turn timed out for {}:{}",
            target.provider.as_str(),
            target.model
        )
    })
    .unwrap_or_else(|error| {
        panic!(
            "Copilot turn failed for {}:{}: {error:?}",
            target.provider.as_str(),
            target.model
        )
    })
}

async fn assert_live_identity(
    runtime: &SessionRuntime,
    session_id: &meerkat_core::SessionId,
    expected: &RouteModel,
) {
    let identity = runtime
        .live_llm_identity_for_session(session_id)
        .await
        .expect("read live session LLM identity");
    assert_eq!(identity.provider, expected.provider);
    assert_eq!(identity.model, expected.model);
    assert_eq!(
        identity
            .auth_binding
            .as_ref()
            .map(|binding| binding.binding.as_str()),
        Some(expected.route.binding_id())
    );
}

fn mark_provider_covered(covered: &mut Vec<Provider>, provider: Provider) {
    if !covered.contains(&provider) {
        covered.push(provider);
    }
}

fn normalized_marker(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_uppercase)
        .collect()
}

async fn stage_session(
    runtime: &Arc<SessionRuntime>,
    target: &RouteModel,
    max_tokens: u32,
    external_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> meerkat_core::SessionId {
    let mut build = AgentBuildConfig::new(target.model.clone());
    build.provider = Some(target.provider);
    build.auth_binding = Some(auth_binding(target.route));
    build.external_tools = external_tools;
    build.max_tokens = Some(max_tokens);
    runtime
        .create_session(build, None, None, Vec::new())
        .await
        .expect("stage Copilot live session")
}

async fn maybe_test_same_provider_switch(
    runtime: &Arc<SessionRuntime>,
    session_id: &meerkat_core::SessionId,
    current: &RouteModel,
    candidates: &[RouteModel],
) -> Option<RouteModel> {
    let target = candidates
        .iter()
        .find(|candidate| {
            candidate.provider == current.provider && candidate.model != current.model
        })?
        .clone();
    let result = run_turn(
        runtime,
        session_id,
        "Reply with only: switched".into(),
        current,
        &target,
    )
    .await;
    assert!(
        !result.text.trim().is_empty(),
        "same-provider model switch returned empty output"
    );
    assert_live_identity(runtime, session_id, &target).await;
    Some(target)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:e2e-live; requires `rkat auth login copilot`"]
async fn copilot_live_account_routes_tools_images_and_switches() {
    let persistence = TokenStoreBackend::default_auto()
        .expect("resolve default credential store")
        .open_with_refresh_authority()
        .expect("open default credential store");
    let token_key = TokenKey::from_credential_identity(&account_identity());
    let Some(tokens) = persistence
        .token_store()
        .load(&token_key)
        .await
        .expect("read Copilot account credential")
    else {
        skip_or_fail("no global GitHub Copilot credential; run `rkat auth login copilot`");
        return;
    };
    assert_eq!(
        tokens.auth_mode,
        PersistedAuthMode::GithubCopilotOauth,
        "shared Copilot account contains the wrong credential kind"
    );
    assert!(
        tokens
            .primary_secret
            .as_deref()
            .is_some_and(|secret| !secret.is_empty()),
        "shared Copilot account credential is empty"
    );

    let mut config = Config::load().await.expect("load effective Meerkat config");
    if let Err(error) = validate_copilot_routes(&config) {
        skip_or_fail(format!(
            "canonical Copilot routes are unavailable ({error}); rerun `rkat auth login copilot`"
        ));
        return;
    }
    config.model_fallback.enabled = false;

    let resolved = timeout(
        Duration::from_secs(90),
        resolve_account(&config, persistence.clone()),
    )
    .await
    .expect("Copilot token exchange/model discovery timed out")
    .expect("Copilot token exchange/model discovery failed");
    let snapshot = resolved
        .model_snapshot()
        .expect("Copilot authentication succeeded but model discovery returned no snapshot");
    let candidates = route_models(&config, &snapshot);
    for route in CopilotProviderRoute::ALL {
        let discovered = snapshot
            .available_model_ids(route.provider())
            .collect::<Vec<_>>();
        let usable = candidates
            .iter()
            .filter(|candidate| candidate.route == route)
            .map(|candidate| candidate.model.as_str())
            .collect::<Vec<_>>();
        eprintln!(
            "copilot-live {} discovered={discovered:?} catalog_usable={usable:?}",
            route.provider().as_str()
        );
    }
    if candidates.is_empty() {
        skip_or_fail(
            "the account exposes no streaming model that is also present in the effective Meerkat model registry",
        );
        return;
    }

    let registry = config
        .model_registry(meerkat_models::canonical())
        .expect("effective model registry");
    if let Some((route, unavailable)) = CopilotProviderRoute::ALL.iter().find_map(|route| {
        registry
            .entries_for_provider(route.provider())
            .find(|entry| {
                snapshot
                    .model(&entry.id)
                    .and_then(|model| model.route_for(route.provider()))
                    .is_none()
            })
            .map(|entry| (*route, entry.id.clone()))
    }) {
        assert_eq!(
            resolved.route_for(route.provider(), &unavailable),
            CopilotModelAccess::Unavailable,
            "account offering must reject a catalog model absent from the discovered route"
        );
    }

    let temp = TempDir::new().expect("Copilot live temp directory");
    let factory = AgentFactory::new(temp.path().join("factory-store"))
        .runtime_root(temp.path().join("runtime"))
        .with_provider_auth_persistence(persistence);
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let bundle = PersistenceBundle::new(
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        blob_store,
    );
    let runtime = Arc::new(SessionRuntime::new(
        factory,
        config.clone(),
        8,
        bundle,
        NotificationSink::noop(),
    ));
    runtime.set_realm_context(
        Some(RealmId::global()),
        Some("copilot-live-e2e".to_string()),
        None,
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        config_store,
        temp.path().join("config-state.json"),
    )));

    let switch_initial = candidates[0].clone();
    let switch_max_tokens = candidates
        .iter()
        .map(RouteModel::max_tokens)
        .min()
        .expect("non-empty Copilot route candidates");
    let switch_session = stage_session(&runtime, &switch_initial, switch_max_tokens, None).await;
    let first_result = run_turn(
        &runtime,
        &switch_session,
        "Reply with only: ready".into(),
        &switch_initial,
        &switch_initial,
    )
    .await;
    assert!(
        !first_result.text.trim().is_empty(),
        "initial Copilot turn returned empty output"
    );
    assert_live_identity(&runtime, &switch_session, &switch_initial).await;

    let mut current = switch_initial.clone();
    let mut covered_providers = vec![switch_initial.provider];
    let mut same_provider_switch_tested = false;
    let mut cross_provider_switch_tested = false;

    if let Some(switched) =
        maybe_test_same_provider_switch(&runtime, &switch_session, &current, &candidates).await
    {
        current = switched;
        same_provider_switch_tested = true;
    }

    for route in CopilotProviderRoute::ALL {
        let provider = route.provider();
        if covered_providers.contains(&provider) {
            continue;
        }
        let Some(target) = candidates
            .iter()
            .find(|candidate| candidate.route == route)
            .cloned()
        else {
            eprintln!(
                "SKIP copilot-live {} route: no catalog-usable account model",
                provider.as_str()
            );
            continue;
        };
        let previous = current.clone();
        let result = run_turn(
            &runtime,
            &switch_session,
            format!("Reply with only: {}", provider.as_str()).into(),
            &previous,
            &target,
        )
        .await;
        assert!(
            !result.text.trim().is_empty(),
            "Copilot {} route returned empty output",
            provider.as_str()
        );
        assert_live_identity(&runtime, &switch_session, &target).await;
        cross_provider_switch_tested |= previous.provider != target.provider;
        mark_provider_covered(&mut covered_providers, target.provider);
        current = target;

        if !same_provider_switch_tested
            && let Some(switched) =
                maybe_test_same_provider_switch(&runtime, &switch_session, &current, &candidates)
                    .await
        {
            current = switched;
            same_provider_switch_tested = true;
        }
    }

    let available_provider_count = CopilotProviderRoute::ALL
        .iter()
        .filter(|route| {
            candidates
                .iter()
                .any(|candidate| candidate.route == **route)
        })
        .count();
    if available_provider_count > 1 {
        assert!(
            cross_provider_switch_tested,
            "multiple Copilot provider routes were available but no cross-provider switch ran"
        );
    } else {
        eprintln!("SKIP copilot-live cross-provider switch: only one provider route is usable");
    }

    let has_same_provider_pair = candidates.iter().any(|candidate| {
        candidates
            .iter()
            .any(|other| candidate.provider == other.provider && candidate.model != other.model)
    });
    if has_same_provider_pair {
        assert!(
            same_provider_switch_tested,
            "a same-provider model pair was available but no model switch ran"
        );
    } else {
        eprintln!("SKIP copilot-live same-provider switch: no provider exposes two usable models");
    }

    if let Some(tool_target) = candidates
        .iter()
        .find(|candidate| candidate.supports_tools())
        .cloned()
    {
        let recorder = Arc::new(RecordingProbeDispatcher::new());
        let external_tools: Arc<dyn AgentToolDispatcher> = recorder.clone();
        let tool_session = stage_session(
            &runtime,
            &tool_target,
            tool_target.max_tokens(),
            Some(external_tools),
        )
        .await;
        let result = run_turn(
            &runtime,
            &tool_session,
            format!(
                "Call live_probe exactly once with nonce \"{TOOL_NONCE}\". \
                 After the tool succeeds, reply with only: recorded"
            )
            .into(),
            &tool_target,
            &tool_target,
        )
        .await;
        assert!(
            !result.text.trim().is_empty(),
            "tool-capable Copilot route returned empty output"
        );
        assert!(
            result.tool_calls > 0,
            "Copilot model did not emit a tool call"
        );
        assert_eq!(
            recorder.calls(),
            vec![TOOL_NONCE.to_string()],
            "tool assertion must come from the dispatcher, not model narration"
        );
        assert_live_identity(&runtime, &tool_session, &tool_target).await;
    } else {
        eprintln!(
            "SKIP copilot-live tool call: no catalog-usable route advertises tool_calls=true"
        );
    }

    if let Some(vision_target) = candidates
        .iter()
        .find(|candidate| candidate.supports_vision())
        .cloned()
    {
        let image_session =
            stage_session(&runtime, &vision_target, vision_target.max_tokens(), None).await;
        let result = run_turn(
            &runtime,
            &image_session,
            ContentInput::Blocks(vec![
                ContentBlock::Text {
                    text: "Read the exact uppercase letters and digits printed in the image. \
                           Reply with only that marker."
                        .to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: MARKER_PNG.to_string(),
                    },
                },
            ]),
            &vision_target,
            &vision_target,
        )
        .await;
        assert!(
            !result.text.trim().is_empty(),
            "vision-capable Copilot route returned empty output"
        );
        assert!(
            normalized_marker(&result.text).contains(IMAGE_MARKER),
            "vision response did not identify the undisclosed marker; response={:?}",
            result.text
        );
        assert_live_identity(&runtime, &image_session, &vision_target).await;

        let history = runtime
            .session_service()
            .read_history(&image_session, SessionHistoryQuery::default())
            .await
            .expect("read authoritative Copilot image session history");
        assert!(
            history.messages.iter().any(|message| {
                matches!(
                    message,
                    Message::User(user)
                        if user.content.iter().any(|block| {
                            matches!(
                                block,
                                ContentBlock::Image { media_type, .. }
                                    if media_type == "image/png"
                            )
                        })
                )
            }),
            "successful image turn did not commit an image block to canonical history"
        );
    } else {
        eprintln!(
            "SKIP copilot-live image input: no route has both registry and account vision support"
        );
    }

    eprintln!(
        "copilot-live PASS providers={covered_providers:?} final={}:{}",
        current.provider.as_str(),
        current.model
    );
}
