#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//!
//! Real-account GitHub Copilot coverage for shared auth, tools, multimodal
//! input, and runtime model/provider switching.
//!
//! Run after `rkat auth login copilot` with `make e2e-copilot-live`.
//! `copilot_live_operator_token_routes` checks the operator-token sign-in
//! (`auth_method = "github_copilot_token"`) on its own in-memory routes and
//! runs only when `COPILOT_LIVE_USER_TOKEN` holds a GitHub token with Copilot
//! access.

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
    same_account_model_id,
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

/// Resolve the account through the OpenAI route of `config`'s `global`
/// realm, in `environment` with a fresh AuthMachine lease.
async fn resolve_account(
    config: &Config,
    environment: ResolverEnvironment,
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
    let environment = environment.with_auth_lease_handle(machine.generated_auth_lease_handle());
    CopilotRuntime::new()
        .resolve(&validated, &environment)
        .await
        .map_err(|error| error.to_string())
}

/// The RPC session runtime the live rows run turns through: in-memory
/// session, runtime and blob stores, `config` as its config, and the
/// `global` realm.
fn live_session_runtime(
    temp: &TempDir,
    config: Config,
    persistence: ProviderAuthPersistence,
) -> Arc<SessionRuntime> {
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
    )
    .expect("construct runtime authority");
    let runtime = Arc::new(
        SessionRuntime::new(factory, config.clone(), 8, bundle, NotificationSink::noop())
            .expect("compose the RPC session runtime"),
    );
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
    runtime
}

/// The variable holding the GitHub token of the operator-token row.
const OPERATOR_TOKEN_VAR: &str = "COPILOT_LIVE_USER_TOKEN";

/// The three Copilot routes of the shared account with `auth_method =
/// "github_copilot_token"`, reading the token from [`OPERATOR_TOKEN_VAR`]. An
/// `integration_id` replaces the client integration id on every route.
fn operator_token_routes(integration_id: Option<&str>) -> meerkat_core::RealmConfigSection {
    let mut backend = serde_json::Map::new();
    let mut auth = serde_json::Map::new();
    let mut binding = serde_json::Map::new();
    for route in CopilotProviderRoute::ALL {
        let id = route.binding_id();
        let provider = route.provider().as_str();
        let mut backend_profile = json!({"provider": provider, "backend_kind": "copilot"});
        if let Some(integration_id) = integration_id {
            backend_profile["options"] = json!({"integration_id": integration_id});
        }
        backend.insert(id.to_string(), backend_profile);
        auth.insert(
            id.to_string(),
            json!({
                "provider": provider,
                "auth_method": "github_copilot_token",
                "source": {"kind": "env", "env": OPERATOR_TOKEN_VAR},
            }),
        );
        binding.insert(
            id.to_string(),
            json!({
                "backend_profile": id,
                "auth_profile": id,
                "credential_account": GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID,
            }),
        );
    }
    serde_json::from_value(json!({"backend": backend, "auth": auth, "binding": binding}))
        .expect("operator-token Copilot routes")
}

/// The id under which the account serves the registry model `model` to
/// `provider`'s family: `model` itself when the account lists it, or else
/// the only id it serves that names the same model. `None` when it serves
/// none, or more than one and none exactly.
fn account_id_for<'a>(
    snapshot: &'a CopilotModelSnapshot,
    provider: Provider,
    model: &str,
) -> Option<&'a str> {
    if snapshot.model(model).is_some() {
        return snapshot
            .available_model_ids(provider)
            .find(|listed| *listed == model);
    }
    let mut same = snapshot
        .available_model_ids(provider)
        .filter(|listed| same_account_model_id(listed, model));
    match (same.next(), same.next()) {
        (Some(listed), None) => Some(listed),
        _ => None,
    }
}

fn route_models(config: &Config, snapshot: &CopilotModelSnapshot) -> Vec<RouteModel> {
    let registry = config
        .model_registry(meerkat_models::canonical())
        .expect("effective model registry");
    let mut models = Vec::new();
    for route in CopilotProviderRoute::ALL {
        let provider = route.provider();
        for entry in registry.entries_for_provider(provider) {
            // The account's entry for the registry model: the one listed
            // under the registry id, or else the only one whose id names the
            // same model (the account lists `claude-opus-5.5` for the
            // catalog's `claude-opus-5-5`). The session runs the registry id.
            let model = entry.id.as_str();
            let Some(account_id) = account_id_for(snapshot, provider, model) else {
                continue;
            };
            let Some(offering) = snapshot.model(account_id) else {
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
    config.model_fallback.enabled = Some(false);

    let resolved = timeout(
        Duration::from_secs(90),
        resolve_account(
            &config,
            ResolverEnvironment::testing().with_provider_auth_persistence(persistence.clone()),
        ),
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
    // A catalog model the account serves under no spelling of its id is
    // unavailable on its route. One the account lists under another spelling
    // (a dot where the catalog writes a hyphen) is served under its catalog
    // id, as every candidate is.
    if let Some((route, unavailable)) = CopilotProviderRoute::ALL.iter().find_map(|route| {
        registry
            .entries_for_provider(route.provider())
            .find(|entry| {
                !snapshot
                    .available_model_ids(route.provider())
                    .any(|listed| same_account_model_id(listed, &entry.id))
            })
            .map(|entry| (*route, entry.id.clone()))
    }) {
        assert_eq!(
            resolved.route_for(route.provider(), &unavailable),
            CopilotModelAccess::Unavailable,
            "account offering must reject a catalog model the account serves under no spelling"
        );
    }
    for candidate in &candidates {
        assert!(
            matches!(
                resolved.route_for(candidate.provider, &candidate.model),
                CopilotModelAccess::Available { .. }
            ),
            "the account serves catalog model {} on its {} route",
            candidate.model,
            candidate.provider.as_str()
        );
    }

    let temp = TempDir::new().expect("Copilot live temp directory");
    let runtime = live_session_runtime(&temp, config, persistence);

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

/// Live probe P1 of the operator-token sign-in: one turn on each Copilot
/// route with a GitHub token and no device sign-in. The routes are the
/// documented token routes, in memory, reading `COPILOT_LIVE_USER_TOKEN`
/// (for example a machine user's fine-grained token with Copilot Requests on
/// a Business seat); the user's own config and credential store are not
/// used. `COPILOT_LIVE_INTEGRATION_ID`, when set, replaces the client
/// integration id on all three routes, to compare client identities. Run by
/// hand with the repository's Cargo wrapper (`repo-cargo`):
///
/// ```text
/// COPILOT_LIVE_USER_TOKEN=<token> repo-cargo test -p meerkat-integration-tests \
///     --features integration-real-tests --test copilot_live_e2e \
///     copilot_live_operator_token_routes -- --ignored --nocapture
/// ```
///
/// Without the variable the row prints a skip and passes, also under
/// `make e2e-copilot-live`, whose device rows do not need a token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:e2e-live; set COPILOT_LIVE_USER_TOKEN to a GitHub token with Copilot access"]
async fn copilot_live_operator_token_routes() {
    let token_set = std::env::var(OPERATOR_TOKEN_VAR).is_ok_and(|token| !token.trim().is_empty());
    if !token_set {
        eprintln!("SKIP copilot-live operator token: {OPERATOR_TOKEN_VAR} is not set");
        return;
    }
    let integration_id = std::env::var("COPILOT_LIVE_INTEGRATION_ID").ok();
    let mut config = Config::default();
    config.realm.insert(
        "global".to_string(),
        operator_token_routes(integration_id.as_deref()),
    );
    config.model_fallback.enabled = Some(false);
    let persistence = ProviderAuthPersistence::new(
        Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
        Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
    );

    let resolved = timeout(
        Duration::from_secs(90),
        resolve_account(
            &config,
            ResolverEnvironment::with_process_env()
                .with_provider_auth_persistence(persistence.clone()),
        ),
    )
    .await
    .expect("Copilot identity lookup/model discovery timed out")
    .expect("GitHub and Copilot accept the operator token");
    let identity = resolved
        .concrete_authorizer()
        .account_identity()
        .expect("the operator token resolved to a Copilot identity");
    eprintln!(
        "copilot-live operator token login={:?} plan={:?} api_base={} installation={} integration_id={integration_id:?}",
        identity.login, identity.plan, identity.api_base, identity.installation
    );
    let snapshot = resolved
        .model_snapshot()
        .expect("the operator token resolved but listed no models");
    let candidates = route_models(&config, &snapshot);
    if candidates.is_empty() {
        skip_or_fail(
            "the account exposes no streaming model that is also present in the effective Meerkat model registry",
        );
        return;
    }

    let temp = TempDir::new().expect("Copilot live temp directory");
    let runtime = live_session_runtime(&temp, config, persistence);
    let mut covered_providers = Vec::new();
    for route in CopilotProviderRoute::ALL {
        let Some(target) = candidates
            .iter()
            .find(|candidate| candidate.route == route)
            .cloned()
        else {
            eprintln!(
                "SKIP copilot-live operator token {} route: no catalog-usable account model",
                route.provider().as_str()
            );
            continue;
        };
        let session = stage_session(&runtime, &target, target.max_tokens(), None).await;
        let result = run_turn(
            &runtime,
            &session,
            "Reply with only: ready".into(),
            &target,
            &target,
        )
        .await;
        assert!(
            !result.text.trim().is_empty(),
            "Copilot {} route returned empty output with the operator token",
            route.provider().as_str()
        );
        assert_live_identity(&runtime, &session, &target).await;
        mark_provider_covered(&mut covered_providers, target.provider);
    }
    eprintln!("copilot-live operator token PASS providers={covered_providers:?}");
}

// ---------------------------------------------------------------------------
// OpenAI-family continuity across services (live differential)
// ---------------------------------------------------------------------------
//
// Encrypted reasoning, item ids and response ids are replayed only to the
// service the running process received them from. These rows send real
// requests to the public OpenAI API and to the GitHub Copilot `/responses`
// route with the clients a session uses, and print only sanitized shapes:
// row names, block kinds, whether a block still carries provider metadata,
// and the class of a refusal. No account, token, model list or response
// body is printed or written.
//
// A cold load needs another process, so the rows run as phases of this one
// test, chosen by `MEERKAT_CONTINUITY_LIVE_PHASE`, and hand their transcripts
// over as files in `MEERKAT_CONTINUITY_LIVE_DIR` (rows a session store would
// hold; delete the directory afterwards):
//
// - `mint`: one tool step on the public API, sent on to Copilot inside the
//   tool loop in the same process. Writes `api.json`.
// - `resume`: a new process. The stored tool loop resumes on the public API
//   and on Copilot, a further tool turn makes the request mixed, then the
//   session moves between the two services in both directions. Writes
//   `copilot.json` and, when the public API offers hosted web search to the
//   credential, `search.json`.
// - `resume-copilot`: a new process. The stored Copilot turn resumes on
//   Copilot (mixed request), and the stored search step on both services.
//
// Each phase stops at the first 401 or 429.

mod continuity_live {
    use std::path::PathBuf;
    use std::sync::Arc;

    use futures::StreamExt;
    use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest, OpenAiClient};
    use meerkat_core::lifecycle::run_primitive::{
        OpaqueProviderBody, OpenAiProviderTag, ProviderTag, ToolChoice,
    };
    use meerkat_core::{
        AssistantBlock, BlockAssistantMessage, Message, SessionLlmIdentity, StopReason, UserMessage,
    };
    use serde_json::json;

    /// Models tried in this order: the first the Copilot account serves on
    /// its `/responses` route is used on both services.
    pub(super) const MODEL_PREFERENCE: &[&str] = &["gpt-5.4-mini", "gpt-5.4", "gpt-5.5"];
    const MAX_TOKENS: u32 = 2048;

    pub(super) struct Probe {
        pub(super) model: String,
        pub(super) api: Arc<dyn LlmClient>,
        pub(super) copilot: Arc<dyn LlmClient>,
        pub(super) requests: std::cell::Cell<u32>,
    }

    pub(super) fn handover(name: &str) -> PathBuf {
        let dir = std::env::var("MEERKAT_CONTINUITY_LIVE_DIR")
            .expect("MEERKAT_CONTINUITY_LIVE_DIR names the handover directory");
        PathBuf::from(dir).join(name)
    }

    pub(super) fn store(name: &str, transcript: &[Message]) {
        let rows = serde_json::to_vec(transcript).expect("rows");
        std::fs::write(handover(name), rows).expect("write the handover rows");
    }

    pub(super) fn load(name: &str) -> Option<Vec<Message>> {
        let rows = std::fs::read(handover(name)).ok()?;
        Some(serde_json::from_slice(&rows).expect("the handover rows decode"))
    }

    fn weather_tool() -> Arc<meerkat_core::ToolDef> {
        Arc::new(meerkat_core::ToolDef::new(
            "get_weather",
            "Current weather of a city.",
            json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
                "additionalProperties": false
            }),
        ))
    }

    pub(super) fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text.to_string()))
    }

    /// The tool results answering every call of `step`.
    pub(super) fn tool_outputs(step: &Message) -> Message {
        let Message::BlockAssistant(assistant) = step else {
            panic!("a tool step is an assistant message");
        };
        let results = assistant
            .blocks
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::ToolUse { id, .. } => Some(meerkat_core::ToolResult::new(
                    id.clone(),
                    "4 C, light rain".to_string(),
                    false,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!results.is_empty(), "the step called the tool");
        Message::tool_results(results)
    }

    /// A transcript as kinds only: for each assistant step its block kinds,
    /// with `+meta` where a block carries provider metadata.
    pub(super) fn shape(messages: &[Message]) -> String {
        messages
            .iter()
            .map(|message| match message {
                Message::User(_) => "user".to_string(),
                Message::ToolResults { results, .. } => format!("tool_results({})", results.len()),
                Message::BlockAssistant(assistant) => {
                    let blocks = assistant
                        .blocks
                        .iter()
                        .map(|block| {
                            let (kind, meta) = match block {
                                AssistantBlock::Text { text, meta } if text.is_empty() => {
                                    ("empty_text", meta.is_some())
                                }
                                AssistantBlock::Text { meta, .. } => ("text", meta.is_some()),
                                AssistantBlock::Reasoning { meta, .. } => {
                                    ("reasoning", meta.is_some())
                                }
                                AssistantBlock::ToolUse { meta, .. } => {
                                    ("tool_use", meta.is_some())
                                }
                                AssistantBlock::ServerToolContent { meta, .. } => {
                                    ("hosted_tool", meta.is_some())
                                }
                                _ => ("other", false),
                            };
                            if meta {
                                format!("{kind}+meta")
                            } else {
                                kind.to_string()
                            }
                        })
                        .collect::<Vec<_>>();
                    format!("assistant[{}]", blocks.join(","))
                }
                _ => "system".to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The class of a refusal, without the provider's text beyond its code.
    fn refusal_class(error: &LlmError) -> String {
        let text = error.to_string();
        let class = match error {
            LlmError::InvalidRequest { .. } => "invalid_request",
            LlmError::AuthenticationFailed { .. } => "authentication_failed",
            LlmError::RateLimited { .. } => "rate_limited",
            _ => "other",
        };
        let code = [
            "invalid_request_body",
            "invalid_encrypted_content",
            "invalid_value",
        ]
        .into_iter()
        .find(|code| text.contains(code))
        .unwrap_or("-");
        let empty_message = text.contains("\"message\":\"\"");
        format!("{class} code={code} empty_message={empty_message}")
    }

    pub(super) enum Outcome {
        Accepted(Box<Message>),
        Refused(String),
    }

    impl Probe {
        /// One request: `history` to `client`. Prints the row and what the
        /// client replays, and returns the committed step or the refusal's
        /// class.
        pub(super) async fn send(
            &self,
            row: &str,
            service: &str,
            client: &Arc<dyn LlmClient>,
            history: &[Message],
            tool_choice: ToolChoice,
            provider_params: Option<ProviderTag>,
        ) -> Outcome {
            let replayed = client
                .project_replay_messages(history)
                .map(|projected| shape(&projected))
                .unwrap_or_else(|error| format!("projection refused: {error}"));
            let mut request = LlmRequest::new(&self.model, history.to_vec());
            request.max_tokens = MAX_TOKENS;
            request.tools = vec![weather_tool()];
            request.tool_choice = tool_choice;
            request.provider_params = provider_params;
            self.requests.set(self.requests.get() + 1);
            let mut stream = client.stream(&request);
            let mut blocks = None;
            let mut stop_reason = StopReason::EndTurn;
            let mut refusal = None;
            while let Some(event) = stream.next().await {
                match event {
                    Ok(LlmEvent::AssistantOutput { blocks: output, .. }) => blocks = Some(output),
                    Ok(LlmEvent::Done {
                        outcome:
                            meerkat_client::LlmDoneOutcome::Success {
                                stop_reason: stop, ..
                            },
                    }) => stop_reason = stop,
                    Ok(LlmEvent::Done {
                        outcome: meerkat_client::LlmDoneOutcome::Error { error },
                    })
                    | Err(error) => {
                        refusal = Some(error);
                        break;
                    }
                    Ok(_) => {}
                }
            }
            drop(stream);
            let outcome = match (refusal, blocks) {
                (Some(error), _) => Outcome::Refused(refusal_class(&error)),
                (None, Some(blocks)) => Outcome::Accepted(Box::new(Message::BlockAssistant(
                    BlockAssistantMessage::new(blocks, stop_reason),
                ))),
                (None, None) => Outcome::Refused("no committed output".to_string()),
            };
            let result = match &outcome {
                Outcome::Accepted(step) => {
                    format!("accepted committed={}", shape(std::slice::from_ref(step)))
                }
                Outcome::Refused(class) => format!("refused {class}"),
            };
            eprintln!(
                "RI-LIVE row={row} service={service} request#{} replayed=[{replayed}] -> {result}",
                self.requests.get()
            );
            if let Outcome::Refused(class) = &outcome {
                assert!(
                    !class.starts_with("authentication_failed")
                        && !class.starts_with("rate_limited"),
                    "STOP: {service} answered {class}; no further live request is sent"
                );
            }
            outcome
        }

        pub(super) async fn accepted(
            &self,
            row: &str,
            service: &str,
            client: &Arc<dyn LlmClient>,
            history: &[Message],
            tool_choice: ToolChoice,
        ) -> Message {
            match self
                .send(row, service, client, history, tool_choice, None)
                .await
            {
                Outcome::Accepted(step) => *step,
                Outcome::Refused(class) => panic!("{row}: {service} refused the request: {class}"),
            }
        }
    }

    pub(super) fn hosted_web_search() -> ProviderTag {
        ProviderTag::OpenAi(OpenAiProviderTag {
            web_search: Some(OpaqueProviderBody::from_value(
                &json!({"type": "web_search"}),
            )),
            ..Default::default()
        })
    }

    pub(super) fn public_api_client() -> Arc<dyn LlmClient> {
        let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY is set");
        Arc::new(OpenAiClient::new(key))
    }

    pub(super) fn copilot_identity(model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(super::auth_binding(super::CopilotProviderRoute::OpenAi)),
        }
    }
}

/// Live differential for OpenAI-family continuity across the public OpenAI
/// API and the GitHub Copilot `/responses` route; see the notes above
/// `continuity_live`. Run by hand, one phase per process:
///
/// ```text
/// MEERKAT_CONTINUITY_LIVE_PHASE=mint MEERKAT_CONTINUITY_LIVE_DIR=<dir> \
/// COPILOT_LIVE_USER_TOKEN=<token> OPENAI_API_KEY=<key> repo-cargo test \
///     -p meerkat-integration-tests --features integration-real-tests \
///     --test copilot_live_e2e copilot_live_openai_continuity_across_services \
///     -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:e2e-live; set MEERKAT_CONTINUITY_LIVE_PHASE, MEERKAT_CONTINUITY_LIVE_DIR, COPILOT_LIVE_USER_TOKEN and OPENAI_API_KEY"]
async fn copilot_live_openai_continuity_across_services() {
    use continuity_live::{Outcome, Probe, load, store, tool_outputs, user};
    use meerkat_core::lifecycle::run_primitive::ToolChoice;

    let Ok(phase) = std::env::var("MEERKAT_CONTINUITY_LIVE_PHASE") else {
        eprintln!("SKIP continuity-live: MEERKAT_CONTINUITY_LIVE_PHASE is not set");
        return;
    };
    let mut config = Config::default();
    config
        .realm
        .insert("global".to_string(), operator_token_routes(None));
    config.model_fallback.enabled = Some(false);
    let persistence = ProviderAuthPersistence::new(
        Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
        Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
    );
    let resolved = timeout(
        Duration::from_secs(90),
        resolve_account(
            &config,
            ResolverEnvironment::with_process_env()
                .with_provider_auth_persistence(persistence.clone()),
        ),
    )
    .await
    .expect("Copilot identity lookup/model discovery timed out")
    .expect("GitHub and Copilot accept the operator token");
    let snapshot = resolved
        .model_snapshot()
        .expect("the operator token resolved but listed no models");
    let candidates = route_models(&config, &snapshot);
    let model = continuity_live::MODEL_PREFERENCE
        .iter()
        .find(|model| {
            candidates.iter().any(|candidate| {
                candidate.route == CopilotProviderRoute::OpenAi
                    && candidate.model == **model
                    && account_id_for(&snapshot, Provider::OpenAI, model)
                        .and_then(|id| snapshot.model(id))
                        .is_some_and(|offering| {
                            offering.endpoints().any(|endpoint| {
                                endpoint == meerkat_copilot::CopilotEndpoint::Responses
                            })
                        })
            })
        })
        .expect("the account serves none of the probe models on its /responses route")
        .to_string();

    let temp = TempDir::new().expect("continuity live temp directory");
    let factory = AgentFactory::new(temp.path().join("factory-store"))
        .runtime_root(temp.path().join("runtime"))
        .with_provider_auth_persistence(persistence);
    let copilot = factory
        .build_llm_client_for_identity(&config, &continuity_live::copilot_identity(&model))
        .await
        .expect("the factory builds the Copilot route client");
    let probe = Probe {
        model: model.clone(),
        api: continuity_live::public_api_client(),
        copilot,
        requests: std::cell::Cell::new(0),
    };
    eprintln!("RI-LIVE phase={phase} model={model}");
    let question = "Use the get_weather tool for Oslo, then answer in one short sentence.";

    match phase.as_str() {
        "mint" => {
            let mut transcript = vec![user(question)];
            let step = probe
                .accepted(
                    "mint-tool-step",
                    "api",
                    &probe.api,
                    &transcript,
                    ToolChoice::Required,
                )
                .await;
            transcript.push(tool_outputs(&step));
            transcript.insert(1, step);
            store("api.json", &transcript);
            // The session moves to Copilot inside the tool loop, same process.
            probe
                .send(
                    "same-process-api-to-copilot-in-tool-loop",
                    "copilot",
                    &probe.copilot,
                    &transcript,
                    ToolChoice::Auto,
                    None,
                )
                .await;
        }
        "resume" => {
            let stored = load("api.json").expect("phase mint wrote api.json");
            // A new process resumes the stored tool loop on both services.
            let answer = probe
                .accepted(
                    "cold-resume-in-tool-loop",
                    "api",
                    &probe.api,
                    &stored,
                    ToolChoice::Auto,
                )
                .await;
            probe
                .accepted(
                    "cold-resume-in-tool-loop",
                    "copilot",
                    &probe.copilot,
                    &stored,
                    ToolChoice::Auto,
                )
                .await;
            // A further tool turn: stored steps dropped, new steps admitted.
            let mut transcript = stored;
            transcript.push(answer);
            transcript.push(user("Do the same for Bergen."));
            let step = probe
                .accepted(
                    "mixed-new-tool-step",
                    "api",
                    &probe.api,
                    &transcript,
                    ToolChoice::Required,
                )
                .await;
            transcript.push(step.clone());
            transcript.push(tool_outputs(&step));
            let answer = probe
                .accepted(
                    "mixed-inside-new-tool-loop",
                    "api",
                    &probe.api,
                    &transcript,
                    ToolChoice::Auto,
                )
                .await;
            transcript.push(answer);
            transcript.push(user("Which of the two is colder?"));
            probe
                .accepted(
                    "mixed-further-turn",
                    "api",
                    &probe.api,
                    &transcript,
                    ToolChoice::Auto,
                )
                .await;
            // The session moves to Copilot: the public API's steps of this
            // process are another service's.
            probe
                .accepted(
                    "same-process-api-to-copilot",
                    "copilot",
                    &probe.copilot,
                    &transcript,
                    ToolChoice::Auto,
                )
                .await;

            // A turn on Copilot, then the session moves to the public API.
            let mut transcript = vec![user(question)];
            let step = probe
                .accepted(
                    "mint-tool-step",
                    "copilot",
                    &probe.copilot,
                    &transcript,
                    ToolChoice::Required,
                )
                .await;
            transcript.push(step.clone());
            transcript.push(tool_outputs(&step));
            let answer = probe
                .accepted(
                    "same-service-inside-tool-loop",
                    "copilot",
                    &probe.copilot,
                    &transcript,
                    ToolChoice::Auto,
                )
                .await;
            transcript.push(answer);
            store("copilot.json", &transcript);
            transcript.push(user("Which city was that?"));
            probe
                .accepted(
                    "same-process-copilot-to-api",
                    "api",
                    &probe.api,
                    &transcript,
                    ToolChoice::Auto,
                )
                .await;

            // A hosted web search step, when the public API offers the tool.
            let search = vec![user(
                "Search the web: what is the capital of Norway? One sentence.",
            )];
            match probe
                .send(
                    "mint-hosted-search",
                    "api",
                    &probe.api,
                    &search,
                    ToolChoice::Auto,
                    Some(continuity_live::hosted_web_search()),
                )
                .await
            {
                Outcome::Accepted(step) => {
                    let mut transcript = search;
                    transcript.push(*step);
                    store("search.json", &transcript);
                }
                Outcome::Refused(class) => {
                    eprintln!(
                        "RI-LIVE hosted web search is not offered to this credential: {class}"
                    );
                }
            }
        }
        "resume-copilot" => {
            let stored = load("copilot.json").expect("phase resume wrote copilot.json");
            let mut transcript = stored;
            transcript.push(user("Do the same for Bergen."));
            let step = probe
                .accepted(
                    "cold-mixed-new-tool-step",
                    "copilot",
                    &probe.copilot,
                    &transcript,
                    ToolChoice::Required,
                )
                .await;
            transcript.push(step.clone());
            transcript.push(tool_outputs(&step));
            let answer = probe
                .accepted(
                    "cold-mixed-inside-new-tool-loop",
                    "copilot",
                    &probe.copilot,
                    &transcript,
                    ToolChoice::Auto,
                )
                .await;
            transcript.push(answer);
            transcript.push(user("Which of the two is colder?"));
            probe
                .accepted(
                    "cold-mixed-further-turn",
                    "copilot",
                    &probe.copilot,
                    &transcript,
                    ToolChoice::Auto,
                )
                .await;
            if let Some(mut search) = load("search.json") {
                search.push(user("And of Sweden? One sentence, no search."));
                probe
                    .accepted(
                        "cold-dropped-hosted-search",
                        "api",
                        &probe.api,
                        &search,
                        ToolChoice::Auto,
                    )
                    .await;
                probe
                    .accepted(
                        "cold-dropped-hosted-search",
                        "copilot",
                        &probe.copilot,
                        &search,
                        ToolChoice::Auto,
                    )
                    .await;
            } else {
                eprintln!("RI-LIVE no stored hosted search step: rows skipped");
            }
        }
        other => panic!("unknown MEERKAT_CONTINUITY_LIVE_PHASE {other}"),
    }
    eprintln!(
        "RI-LIVE phase={phase} done requests={}",
        probe.requests.get()
    );
}
