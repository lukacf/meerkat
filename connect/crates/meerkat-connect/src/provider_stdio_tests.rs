//! Actual protected prompts over an isolated native config and credential owner.
//! No provider client, model session, browser, or network is constructed here.
use super::*;
use crate::stdio::Exchange;
use meerkat_auth_core::auth_store::{
    FileLockCoordinator, FileTokenStore, PersistedTokens, ProviderAuthPersistence, TokenKey,
    TokenStore, TokenStoreError,
};
use meerkat_core::auth::token_store::TokenStoreObservation;
use meerkat_core::{ConfigDelta, ConfigStore, FileConfigStore, ResolvedStorage};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Semaphore;

const KEY: &str = "synthetic-private-prompt-key";

// The native transaction still owns serialization and lifecycle publication.
// This only holds the real file-store call so the presenter must retain it.
struct HeldTokens {
    file: Arc<FileTokenStore>,
    held: bool,
    entered: Semaphore,
    release: Semaphore,
    saves: AtomicUsize,
    settled: AtomicUsize,
    clears: AtomicUsize,
}

#[async_trait::async_trait]
impl TokenStore for HeldTokens {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.file.load(key).await
    }
    async fn save(
        &self,
        fence: &meerkat_core::AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        if self.held {
            self.release.acquire().await.unwrap().forget();
        }
        let result = self.file.save(fence, key, tokens).await;
        self.settled.fetch_add(1, Ordering::SeqCst);
        result
    }
    async fn clear(
        &self,
        fence: &meerkat_core::AdmissionGuard,
        key: &TokenKey,
    ) -> Result<(), TokenStoreError> {
        self.clears.fetch_add(1, Ordering::SeqCst);
        self.file.clear(fence, key).await
    }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        self.file.list().await
    }
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<TokenStoreObservation, TokenStoreError> {
        self.file.observe_status(key).await
    }
    fn backend_name(&self) -> &'static str {
        "private-prompt-native-file"
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    startup: SessionStartup,
    config: Arc<FileConfigStore>,
    tokens: Arc<HeldTokens>,
    target: ResolvedConnectionTarget,
    request: CreateSessionModelResolutionRequest,
}

impl Fixture {
    async fn open(held: bool, explicit_model: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions {
            context_root: root.path().to_owned(),
            state_root: Some(root.path().join("state")),
            user_config_root: Some(root.path().join("home")),
        }
        .resolve("work")
        .unwrap();
        let path = crate::realm::config_source(&storage)
            .unwrap()
            .config_doc_path(&storage.locator.realm);
        let config = Arc::new(FileConfigStore::new(path, meerkat_models::canonical()));
        config.patch(ConfigDelta(json!({
            "agent":{"model":""}, "models":{"openai":"gpt-6-astra"},
            "realm":{"work":{
                "default_binding":"primary",
                "backend":{
                    "primary":{"provider":"openai","backend_kind":"openai_api"},
                    "other":{"provider":"openai","backend_kind":"openai_api"}
                },
                "auth":{
                    "primary":{"provider":"openai","auth_method":"api_key","source":{"kind":"managed_store"}},
                    "other":{"provider":"openai","auth_method":"api_key","source":{"kind":"managed_store"}}
                },
                "binding":{"primary":{"backend_profile":"primary","auth_profile":"primary"}}
            }}
        }))).await.unwrap();
        let tokens = Arc::new(HeldTokens {
            file: Arc::new(FileTokenStore::new(root.path().join("credentials"))),
            held,
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            saves: AtomicUsize::new(0),
            settled: AtomicUsize::new(0),
            clears: AtomicUsize::new(0),
        });
        let persistence = ProviderAuthPersistence::new(
            tokens.clone(),
            // The coordinator's credential root (its lock directory's parent)
            // is the file store's, so the fence it takes covers the store.
            Arc::new(FileLockCoordinator::new(
                root.path().join("credentials").join("refresh-locks"),
            )),
        );
        let mut startup = SessionStartup::open(storage).await.unwrap();
        startup.factory = startup
            .factory
            .clone()
            .with_provider_auth_persistence(persistence);
        let reference = configured_choices(&startup)
            .unwrap()
            .into_iter()
            .find(|r| r.realm.as_str() == "work" && r.binding.as_str() == "primary")
            .unwrap();
        let target = resolve_explicit_auth_binding_target(&startup.config, &reference).unwrap();
        let request = CreateSessionModelResolutionRequest {
            model: explicit_model.then(|| "gpt-6-astra".into()),
            auth_binding: Some(reference),
            ..Default::default()
        };
        assert!(
            !ready_for_open(&mut startup, &request).await.unwrap(),
            "isolated native store starts missing"
        );
        Self {
            _root: root,
            startup,
            config,
            tokens,
            target,
            request,
        }
    }
    async fn assert_writes(&self, count: usize) {
        assert_eq!(self.tokens.saves.load(Ordering::SeqCst), count);
        assert_eq!(self.tokens.settled.load(Ordering::SeqCst), count);
        assert_eq!(self.tokens.clears.load(Ordering::SeqCst), 0);
        let saved = self
            .tokens
            .file
            .load(&TokenKey::from_credential_identity(
                &self.target.credential_identity,
            ))
            .await
            .unwrap();
        assert_eq!(
            saved
                .as_ref()
                .and_then(|tokens| tokens.primary_secret.as_deref()),
            (count == 1).then_some(KEY)
        );
    }
}

async fn prompt(prompts: &mut mpsc::Receiver<Exchange>, kind: &str) -> Exchange {
    let exchange = tokio::time::timeout(Duration::from_secs(10), prompts.recv())
        .await
        .expect("actual presenter must reach its next prompt")
        .expect("presenter ended before its next prompt");
    assert_eq!(exchange.prompt["kind"], kind);
    assert!(
        !exchange.prompt.to_string().contains(KEY),
        "secret is never projected to a prompt"
    );
    exchange
}

async fn save_entered(tokens: &HeldTokens) {
    tokio::time::timeout(Duration::from_secs(10), tokens.entered.acquire())
        .await
        .expect("present must enter the selected native file save")
        .unwrap()
        .forget();
}

fn answer(exchange: Exchange, answer: PromptAnswer) {
    assert!(
        exchange.response.send(Ok(answer)).is_ok(),
        "the exact prompt still owns its response"
    );
}

fn select(exchange: Exchange) {
    let choices = exchange.prompt["choices"].as_array().unwrap();
    let selected = choices
        .iter()
        .find(|v| v["target"]["realm"] == "work" && v["target"]["binding"] == "primary")
        .unwrap();
    assert_eq!(selected["target"]["model"], "gpt-6-astra");
    let choice_id = selected["choice_id"].as_str().unwrap().to_owned();
    answer(exchange, PromptAnswer::ProviderTarget { choice_id });
}

fn enter_key(exchange: Exchange) {
    assert!(
        exchange.prompt["actions"]
            .as_array()
            .unwrap()
            .contains(&json!("enter_key"))
    );
    answer(
        exchange,
        PromptAnswer::ProviderAction {
            action: ProviderAction::EnterKey,
        },
    );
}

// Catch both branches, retire the real bridge and release any held file save
// on panic, then join both branches before propagating the original panic.
// No spawned task or timeout may abandon an entered native write here.
async fn drive<F, S>(
    fixture: &mut Fixture,
    script: F,
) -> Result<Option<CreateSessionModelResolutionRequest>, ConnectError>
where
    F: FnOnce(mpsc::Receiver<Exchange>, watch::Sender<bool>, Arc<AtomicBool>) -> S,
    S: Future<Output = ()>,
{
    let (bridge, prompts, retire) = PromptBridge::channel();
    let completed = Arc::new(AtomicBool::new(false));
    let finished = completed.clone();
    let tokens = fixture.tokens.clone();
    let operation_tokens = tokens.clone();
    let operation_retire = retire.clone();
    let operation = async {
        let result = std::panic::AssertUnwindSafe(present(
            &mut fixture.startup,
            fixture.request.clone(),
            bridge,
        ))
        .catch_unwind()
        .await;
        if result.is_err() {
            operation_retire.send_replace(true);
            operation_tokens.release.add_permits(1);
        }
        finished.store(true, Ordering::SeqCst);
        result
    };
    let actor = async {
        let result = std::panic::AssertUnwindSafe(script(prompts, retire.clone(), completed))
            .catch_unwind()
            .await;
        if result.is_err() {
            retire.send_replace(true);
        }
        tokens.release.add_permits(1);
        result
    };
    let (result, actor) = tokio::join!(operation, actor);
    let result = match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    if let Err(panic) = actor {
        std::panic::resume_unwind(panic);
    }
    result
}

async fn changed_target(
    config: &FileConfigStore,
    storage: &ResolvedStorage,
    original: &ResolvedConnectionTarget,
    delta: Value,
) {
    config.patch(ConfigDelta(delta)).await.unwrap();
    let current = crate::realm::read_config(storage).await.unwrap();
    let target = resolve_explicit_auth_binding_target(&current, &original.auth_binding).unwrap();
    assert_ne!(
        &target, original,
        "the mutation must change the native target, not just its document"
    );
}

fn refused(result: Result<Option<CreateSessionModelResolutionRequest>, ConnectError>) {
    let error = result
        .expect_err("stale prompt must not admit a provider request")
        .to_string();
    assert!(
        error.contains("reviewed native provider target or model changed"),
        "{error}"
    );
    assert!(!error.contains(KEY));
}

#[tokio::test]
async fn private_provider_target_prompt_refuses_changed_native_target_profile_source_and_backend() {
    for delta in [
        json!({"realm":{"work":{"binding":{"primary":{"backend_profile":"other"}}}}}),
        json!({"realm":{"work":{"binding":{"primary":{"auth_profile":"other"}}}}}),
        json!({"realm":{"work":{"auth":{"primary":{"source":{"kind":"env","env":"PRIVATE_PROMPT_ABSENT_KEY"}}}}}}),
        json!({"realm":{"work":{"backend":{"primary":{"base_url":"https://changed.invalid/v1"}}}}}),
    ] {
        let mut fixture = Fixture::open(false, true).await;
        let config = fixture.config.clone();
        let storage = fixture.startup.storage.clone();
        let target = fixture.target.clone();
        let result = drive(
            &mut fixture,
            |mut prompts, _retire, _completed| async move {
                let held = prompt(&mut prompts, "provider_target").await;
                changed_target(&config, &storage, &target, delta).await;
                select(held);
            },
        )
        .await;
        refused(result);
        fixture.assert_writes(0).await;
        assert!(
            fixture.startup.persistence.is_none(),
            "refusal before key save opens no native bundle"
        );
    }
}

#[tokio::test]
async fn private_provider_action_prompt_refuses_changed_native_model_and_defaults() {
    for delta in [
        json!({"agent":{"model":"gpt-5.4"}}),
        json!({"models":{"openai":"gpt-5.4"}}),
        json!({"realm":{"work":{"binding":{"primary":{"default_model":"gpt-5.4"}}}}}),
    ] {
        let mut fixture = Fixture::open(false, false).await;
        let config = fixture.config.clone();
        let storage = fixture.startup.storage.clone();
        let request = fixture.request.clone();
        let result = drive(
            &mut fixture,
            |mut prompts, _retire, _completed| async move {
                select(prompt(&mut prompts, "provider_target").await);
                let held = prompt(&mut prompts, "provider_action").await;
                config.patch(ConfigDelta(delta)).await.unwrap();
                let current = crate::realm::read_config(&storage).await.unwrap();
                assert_eq!(
                    resolve_create_session_model(&current, request)
                        .unwrap()
                        .model,
                    "gpt-5.4"
                );
                enter_key(held);
            },
        )
        .await;
        refused(result);
        fixture.assert_writes(0).await;
    }
}

#[tokio::test]
async fn private_provider_key_prompt_rechecks_before_native_save() {
    let mut fixture = Fixture::open(false, true).await;
    let config = fixture.config.clone();
    let storage = fixture.startup.storage.clone();
    let target = fixture.target.clone();
    let result = drive(
        &mut fixture,
        |mut prompts, _retire, _completed| async move {
            select(prompt(&mut prompts, "provider_target").await);
            enter_key(prompt(&mut prompts, "provider_action").await);
            let held = prompt(&mut prompts, "provider_api_key").await;
            changed_target(
                &config,
                &storage,
                &target,
                json!({"realm":{"work":{"binding":{"primary":{"auth_profile":"other"}}}}}),
            )
            .await;
            answer(held, PromptAnswer::ProviderApiKey { key: KEY.into() });
        },
    )
    .await;
    refused(result);
    fixture.assert_writes(0).await;
    assert!(fixture.startup.persistence.is_none());
}

#[tokio::test]
async fn private_provider_entered_save_settles_once_then_refuses_changed_target() {
    let mut fixture = Fixture::open(true, true).await;
    let config = fixture.config.clone();
    let storage = fixture.startup.storage.clone();
    let target = fixture.target.clone();
    let tokens = fixture.tokens.clone();
    let before = fixture
        .startup
        .provider_auth_runtime_authority()
        .await
        .unwrap();
    let result = drive(&mut fixture, |mut prompts, _retire, completed| async move {
        select(prompt(&mut prompts, "provider_target").await);
        enter_key(prompt(&mut prompts, "provider_action").await);
        answer(
            prompt(&mut prompts, "provider_api_key").await,
            PromptAnswer::ProviderApiKey { key: KEY.into() },
        );
        save_entered(&tokens).await;
        changed_target(
            &config,
            &storage,
            &target,
            json!({"realm":{"work":{"binding":{"primary":{"auth_profile":"other"}}}}}),
        )
        .await;
        assert!(!completed.load(Ordering::SeqCst));
        assert_eq!(tokens.settled.load(Ordering::SeqCst), 0);
        tokens.release.add_permits(1);
    })
    .await;
    refused(result);
    fixture.assert_writes(1).await;
    let after = fixture
        .startup
        .provider_auth_runtime_authority()
        .await
        .unwrap();
    assert!(
        before
            .generated_auth_lease_handle()
            .same_owner(&after.generated_auth_lease_handle())
    );
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&before.oauth_flow_authority()),
        Arc::as_ptr(&after.oauth_flow_authority())
    ));
}

#[tokio::test]
async fn private_provider_unchanged_and_unrelated_config_edits_preserve_success() {
    for unrelated in [false, true] {
        let mut fixture = Fixture::open(false, true).await;
        let config = fixture.config.clone();
        let result = drive(
            &mut fixture,
            |mut prompts, _retire, _completed| async move {
                let held = prompt(&mut prompts, "provider_target").await;
                if unrelated {
                    config
                        .patch(ConfigDelta(json!({"max_tokens":2301})))
                        .await
                        .unwrap();
                }
                select(held);
                let held = prompt(&mut prompts, "provider_action").await;
                if unrelated {
                    config
                        .patch(ConfigDelta(json!({"max_tokens":2302})))
                        .await
                        .unwrap();
                }
                enter_key(held);
                let held = prompt(&mut prompts, "provider_api_key").await;
                if unrelated {
                    config
                        .patch(ConfigDelta(json!({"max_tokens":2303})))
                        .await
                        .unwrap();
                }
                answer(held, PromptAnswer::ProviderApiKey { key: KEY.into() });
            },
        )
        .await
        .unwrap()
        .expect("unchanged reviewed identity still admits the selected request");
        assert_eq!(
            result.auth_binding,
            Some(fixture.target.auth_binding.clone())
        );
        assert_eq!(result.model.as_deref(), Some("gpt-6-astra"));
        fixture.assert_writes(1).await;
        if unrelated {
            assert_eq!(fixture.startup.config.max_tokens, Some(2303));
        }
    }
}

#[tokio::test]
async fn private_provider_use_rechecks_after_action_wait() {
    for changed in [false, true] {
        let mut fixture = Fixture::open(false, false).await;
        let config = fixture.config.clone();
        let file = fixture.tokens.file.clone();
        let key = TokenKey::from_credential_identity(&fixture.target.credential_identity);
        let credentials = fixture._root.path().join("credentials");
        let result = drive(
            &mut fixture,
            |mut prompts, _retire, _completed| async move {
                let held = prompt(&mut prompts, "provider_target").await;
                // A real native file-store credential arrives after the initial
                // Missing observation. This makes the actual Use branch reachable.
                // A direct save outside any coordinator takes the credential
                // fence of the key's realm under the store's root itself.
                let fence = meerkat_core::AdmissionGuard::try_acquire(vec![
                    meerkat_core::AdmissionFence::credentials(&credentials, key.realm()).unwrap(),
                ])
                .unwrap();
                file.save(&fence, &key, &PersistedTokens::api_key(KEY))
                    .await
                    .unwrap();
                drop(fence);
                select(held);
                let held = prompt(&mut prompts, "provider_action").await;
                assert!(
                    held.prompt["actions"]
                        .as_array()
                        .unwrap()
                        .contains(&json!("use"))
                );
                if changed {
                    config
                        .patch(ConfigDelta(json!({"models":{"openai":"gpt-5.4"}})))
                        .await
                        .unwrap();
                }
                answer(
                    held,
                    PromptAnswer::ProviderAction {
                        action: ProviderAction::Use,
                    },
                );
            },
        )
        .await;
        if changed {
            refused(result);
        } else {
            assert_eq!(
                result.unwrap().unwrap().auth_binding,
                Some(fixture.target.auth_binding.clone())
            );
        }
        assert_eq!(
            fixture.tokens.saves.load(Ordering::SeqCst),
            0,
            "Use does not save again"
        );
        assert_eq!(
            fixture
                .tokens
                .file
                .load(&TokenKey::from_credential_identity(
                    &fixture.target.credential_identity
                ))
                .await
                .unwrap()
                .unwrap()
                .primary_secret
                .as_deref(),
            Some(KEY)
        );
    }
}

#[tokio::test]
async fn private_provider_cancel_or_retirement_at_each_prompt_never_saves() {
    for retirement in [false, true] {
        for stop_at in ["provider_target", "provider_action", "provider_api_key"] {
            let mut fixture = Fixture::open(false, true).await;
            let result = drive(&mut fixture, |mut prompts, retire, _completed| async move {
                for kind in ["provider_target", "provider_action", "provider_api_key"] {
                    let held = prompt(&mut prompts, kind).await;
                    if kind == stop_at {
                        if retirement {
                            retire.send_replace(true);
                            drop(held);
                        } else {
                            answer(held, PromptAnswer::Cancel);
                        }
                        return;
                    }
                    if kind == "provider_target" {
                        select(held);
                    } else {
                        enter_key(held);
                    }
                }
            })
            .await;
            assert!(result.unwrap().is_none());
            fixture.assert_writes(0).await;
        }
    }
}

#[tokio::test]
async fn private_provider_retirement_joins_entered_native_save_without_continuation() {
    let mut fixture = Fixture::open(true, true).await;
    let tokens = fixture.tokens.clone();
    let result = drive(&mut fixture, |mut prompts, retire, completed| async move {
        select(prompt(&mut prompts, "provider_target").await);
        enter_key(prompt(&mut prompts, "provider_action").await);
        answer(
            prompt(&mut prompts, "provider_api_key").await,
            PromptAnswer::ProviderApiKey { key: KEY.into() },
        );
        save_entered(&tokens).await;
        retire.send_replace(true);
        // Let the real cancellation branch poll while the exact native save
        // remains held. Completion before release would abandon its custody.
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        assert!(!completed.load(Ordering::SeqCst));
        assert_eq!(tokens.settled.load(Ordering::SeqCst), 0);
        tokens.release.add_permits(1);
    })
    .await;
    assert!(result.unwrap().is_none());
    fixture.assert_writes(1).await;
}

#[tokio::test]
async fn private_provider_targets_are_the_routes_their_sessions_run_on() {
    // A GitHub Copilot account configured by the explicit tables a token
    // login writes, or once by a `[copilot]` section. Its token variable
    // stays unset, so the prompt lists the configured targets.
    let source = json!({"kind":"env","env":"CONNECT_COPILOT_SECTION_TOKEN"});
    let mut tables = json!({"backend":{},"auth":{},"binding":{}});
    for provider in ["openai", "anthropic", "gemini"] {
        let route = format!("copilot_{provider}");
        tables["backend"][&route] = json!({"provider":provider,"backend_kind":"copilot"});
        tables["auth"][&route] = json!({"provider":provider,
            "auth_method":"github_copilot_token","source":source});
        tables["binding"][&route] = json!({"backend_profile":route,"auth_profile":route,
            "credential_account":"github_copilot"});
    }
    let route = |binding: &str| AuthBindingRef {
        realm: meerkat_core::RealmId::parse("work").unwrap(),
        binding: BindingId::parse(binding).unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    };
    let request = |binding: &str| CreateSessionModelResolutionRequest {
        model: Some("claude-opus-5-5".into()),
        auth_binding: Some(route(binding)),
        ..Default::default()
    };
    for section in [tables, json!({"copilot":{"source":source}})] {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions {
            context_root: root.path().to_owned(),
            state_root: Some(root.path().join("state")),
            user_config_root: Some(root.path().join("home")),
        }
        .resolve("work")
        .unwrap();
        let path = crate::realm::config_source(&storage)
            .unwrap()
            .config_doc_path(&storage.locator.realm);
        FileConfigStore::new(path, meerkat_models::canonical())
            .patch(ConfigDelta(json!({"realm":{"work":section}})))
            .await
            .unwrap();
        let mut startup =
            SessionStartup::open_with_provider_store(storage, root.path().join("credentials"))
                .await
                .unwrap();
        let (bridge, mut prompts, _retire) = PromptBridge::channel();
        let actor = async {
            let exchange = prompt(&mut prompts, "provider_target").await;
            let listed: Vec<String> = exchange.prompt["choices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|choice| choice["target"]["binding"].as_str().unwrap().to_owned())
                .collect();
            answer(exchange, PromptAnswer::Cancel);
            listed
        };
        let (result, listed) = tokio::join!(
            present(&mut startup, request("copilot_anthropic"), bridge),
            actor
        );
        assert!(result.unwrap().is_none());
        // Core's account affinity runs the account's route of the model's
        // family. A listed route's session runs on that route itself.
        for binding in &listed {
            let native =
                resolve_create_session_model(&startup.config, request(binding.as_str())).unwrap();
            assert_eq!(
                native.auth_binding,
                Some(route(binding.as_str())),
                "listed {binding} runs on another route"
            );
        }
        assert_eq!(listed, ["copilot_anthropic"]);
    }
}
