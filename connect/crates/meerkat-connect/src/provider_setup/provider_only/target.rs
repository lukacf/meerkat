//! Native choice projection and narrow first-target CAS; no browser-authored config.
use super::*;
use meerkat_core::connection::RealmConnectionSet;
use meerkat_core::provider_matrix::{
    AnthropicBackendKind, GoogleBackendKind, OpenAiBackendKind, SelfHostedBackendKind,
};
use meerkat_core::{
    Config, ConfigDelta, ConfigRuntime, EffectiveConfigReader, Provider, RealmConfigSource,
};
use meerkat_llm_core::provider_runtime::binding::NormalizedAuthMethod;

pub(super) struct Choice {
    pub id: String,
    pub model: String,
    pub provider: Provider,
    pub target: Option<ResolvedConnectionTarget>,
    pub backend: String,
    pub auth: String,
    pub enabled: bool,
    pub observation: Value,
}
fn pairs(provider: Provider) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    macro_rules! add {
        ($type:ty) => {
            for backend in <$type>::ALL {
                for auth in backend.supported_auth_methods() {
                    pairs.push((backend.as_str().to_owned(), auth.as_str().to_owned()));
                }
            }
        };
    }
    match provider {
        Provider::OpenAI => add!(OpenAiBackendKind),
        Provider::Anthropic => add!(AnthropicBackendKind),
        Provider::Gemini => add!(GoogleBackendKind),
        Provider::SelfHosted => add!(SelfHostedBackendKind),
        Provider::Other => {}
    }
    pairs
}
fn onboardable(provider: Provider, auth: &str) -> bool {
    NormalizedAuthMethod::parse_for_provider(provider, auth)
        .and_then(NormalizedAuthMethod::persisted_auth_mode)
        .is_some_and(|mode| {
            mode == PersistedAuthMode::ApiKey
                || OAuthProviderIdentity::INTERACTIVE
                    .iter()
                    .any(|identity| identity.auth_mode() == mode)
        })
}
impl Choice {
    pub fn show(&self) -> Value {
        json!({"choice_id":self.id,"model":self.model,"provider":self.provider,
            "backend":self.backend,"auth":self.auth,"configured":self.target.is_some(),
            "selectable":self.enabled,"observation":self.observation,
            "live_entitlement":"unobserved"})
    }
    pub fn request(
        &self,
        binding: meerkat_core::AuthBindingRef,
    ) -> CreateSessionModelResolutionRequest {
        CreateSessionModelResolutionRequest {
            model: Some(self.model.clone()),
            provider: Some(self.provider),
            auth_binding: Some(binding),
        }
    }
}
pub(super) async fn catalog(startup: &mut SessionStartup) -> Result<Vec<Choice>, ConnectError> {
    startup.config = crate::realm::read_config(&startup.storage).await?;
    // Build the native config-backed response as well as its typed registry.
    // Neither path contacts a provider or proves model/account access.
    meerkat::surface::build_models_catalog_response(&startup.config)
        .map_err(|e| ConnectError::native("Read native model catalog", e))?;
    let registry = startup
        .config
        .model_registry(meerkat_models::canonical())
        .map_err(|e| ConnectError::native("Read native model registry", e))?;
    let models: Vec<_> = registry
        .provider_defaults()
        .flat_map(|(provider, _)| {
            registry
                .entries_for_provider(provider)
                .map(move |entry| (provider, entry.id.clone()))
        })
        .collect();
    let configured = configured_choices(startup)?;
    let mut choices = Vec::new();
    for reference in &configured {
        let target = resolve_explicit_auth_binding_target(&startup.config, reference)
            .map_err(|e| ConnectError::native("Resolve configured provider target", e))?;
        let observation = observe(startup, &target).await?;
        for (provider, model) in &models {
            if *provider != target.backend.provider {
                continue;
            }
            let request = CreateSessionModelResolutionRequest {
                model: Some(model.clone()),
                provider: Some(*provider),
                auth_binding: Some(reference.clone()),
            };
            if resolve_create_session_model(&startup.config, request).is_err() {
                continue;
            }
            choices.push(Choice {
                id: uuid::Uuid::new_v4().to_string(),
                model: model.clone(),
                provider: *provider,
                backend: target.backend.backend_kind.clone(),
                auth: target.auth_profile.auth_method.clone(),
                enabled: true,
                observation: json!({"credential":observation.label()}),
                target: Some(target.clone()),
            });
        }
    }
    // New construction is deliberately first-target only. Existing configuration
    // is selected, never replaced or supplemented by a browser patch.
    if configured.is_empty() {
        for (provider, model) in models {
            let runtime = startup
                .factory
                .provider_runtime_registry()
                .get(provider)
                .is_some();
            for (backend, auth) in pairs(provider) {
                let supported = onboardable(provider, &auth) && provider != Provider::SelfHosted;
                choices.push(Choice {
                    id: uuid::Uuid::new_v4().to_string(),
                    model: model.clone(),
                    provider,
                    target: None,
                    backend,
                    auth,
                    enabled: runtime && supported,
                    observation: json!({"compiled_runtime":runtime,"interactive_setup":supported,
                        "credential":"not_configured"}),
                });
            }
        }
    }
    if choices.len() > 4096 {
        return Err(invalid(
            "Native provider catalog exceeds this presentation bound",
        ));
    }
    // Backend/auth support alone does not prove that a first target can be
    // constructed without operator endpoints or additional native fields.
    // Exercise the same native preview used for admission, once per offered
    // backend/auth pair. This writes neither config nor credentials.
    let mut previews = std::collections::BTreeMap::new();
    for choice in &mut choices {
        if choice.target.is_some() || !choice.enabled {
            continue;
        }
        let key = (
            choice.provider.as_str().to_owned(),
            choice.backend.clone(),
            choice.auth.clone(),
        );
        let available = match previews.get(&key) {
            Some(available) => *available,
            None => {
                let available = prepare(startup, choice).await.is_ok();
                previews.insert(key, available);
                available
            }
        };
        choice.enabled = available;
        choice.observation["native_construction_preview"] = json!(if available {
            "available"
        } else {
            "unavailable"
        });
    }
    Ok(choices)
}

pub(super) struct Prepared {
    runtime: ConfigRuntime,
    generation: u64,
    own: Option<Value>,
    delta: ConfigDelta,
    pub target: ResolvedConnectionTarget,
    pub request: CreateSessionModelResolutionRequest,
    pub review: Value,
}
async fn own(startup: &SessionStartup) -> Result<Option<Value>, ConnectError> {
    crate::realm::config_source(&startup.storage)?
        .raw_config_for_realm(&startup.storage.locator.realm)
        .await
        .map_err(|e| ConnectError::native("Read own provider document", e))?
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| ConnectError::native("Read provider document fields", e))
}
fn first(
    config: &Config,
    own: Option<&Value>,
    realm: &meerkat_core::RealmId,
) -> Result<(), ConnectError> {
    if own
        .and_then(|v| v.get("realm"))
        .and_then(|v| v.get(realm.as_str()))
        .is_some_and(|v| {
            v.get("default_binding")
                .is_some_and(|value| !value.is_null())
        })
    {
        return Err(invalid(
            "Existing native default is not replaceable by first-target setup",
        ));
    }
    let chain = RealmChain::resolve(config, realm)
        .map_err(|e| ConnectError::native("Resolve provider inheritance", e))?;
    for owner in chain.realms() {
        if realm_connection_set(config, owner)?
            .is_some_and(|set| !set.bindings.is_empty() || set.default_binding.is_some())
        {
            return Err(invalid("Native targets changed; reopen provider choices"));
        }
    }
    Ok(())
}
pub(super) async fn prepare(
    startup: &mut SessionStartup,
    choice: &Choice,
) -> Result<Prepared, ConnectError> {
    if !choice.enabled || choice.target.is_some() {
        return Err(invalid("Not an offered first target"));
    }
    startup.config = crate::realm::read_config(&startup.storage).await?;
    let runtime = crate::realm::own_config_runtime(&startup.storage)?;
    let snapshot = runtime
        .get()
        .await
        .map_err(|e| ConnectError::native("Read native config generation", e))?;
    let original = own(startup).await?;
    let realm = &startup.storage.locator.realm;
    first(&startup.config, original.as_ref(), realm)?;
    first(&snapshot.config, original.as_ref(), realm)?;
    let prefix = format!("connect_{}", uuid::Uuid::new_v4().simple());
    let account = format!("{prefix}_account");
    let mode = NormalizedAuthMethod::parse_for_provider(choice.provider, &choice.auth)
        .and_then(NormalizedAuthMethod::persisted_auth_mode)
        .ok_or_else(|| invalid("Native auth mode unavailable"))?;
    let copilot = mode == PersistedAuthMode::GithubCopilotOauth;
    let registry = startup
        .config
        .model_registry(meerkat_models::canonical())
        .map_err(|e| ConnectError::native("Read first-target model registry", e))?;
    let mut section = json!({"backend":{},"auth":{},"binding":{}});
    let mut selected_name = None;
    for provider in Provider::ALL_CONCRETE {
        if !copilot && *provider != choice.provider {
            continue;
        }
        if startup
            .factory
            .provider_runtime_registry()
            .get(*provider)
            .is_none()
        {
            continue;
        }
        let pair = if *provider == choice.provider {
            Some((choice.backend.clone(), choice.auth.clone()))
        } else {
            pairs(*provider).into_iter().find(|(_, auth)| {
                NormalizedAuthMethod::parse_for_provider(*provider, auth)
                    .and_then(NormalizedAuthMethod::persisted_auth_mode)
                    == Some(PersistedAuthMode::GithubCopilotOauth)
            })
        };
        let Some((backend, auth)) = pair else {
            continue;
        };
        let model = if *provider == choice.provider {
            choice.model.clone()
        } else {
            match registry.provider_defaults().find(|(p, _)| p == provider) {
                Some((_, model)) => model.to_owned(),
                None => continue,
            }
        };
        let name = format!("{prefix}_{}", provider.as_str());
        section["backend"][&name] = json!({"provider":provider,"backend_kind":backend});
        section["auth"][&name] = json!({"provider":provider,"auth_method":auth,
            "source":{"kind":"managed_store"},"interactive":true});
        section["binding"][&name] =
            json!({"backend_profile":name,"auth_profile":name,"default_model":model});
        if copilot {
            section["binding"][&name]["credential_account"] = json!(account);
        }
        if *provider == choice.provider {
            selected_name = Some(name);
        }
    }
    let name = selected_name.ok_or_else(|| invalid("Selected native runtime is unavailable"))?;
    section["default_binding"] = json!(name);
    let delta = ConfigDelta(json!({"realm":{(realm.as_str()):section}}));
    let preview = runtime
        .patch_preview(&delta)
        .await
        .map_err(|e| ConnectError::native("Preview native provider target", e))?;
    let mut effective = EffectiveConfigReader::new(crate::realm::config_source(&startup.storage)?)
        .effective_config_over_head(realm, preview)
        .await
        .map_err(|e| ConnectError::native("Compose provider preview", e))?;
    effective
        .apply_env_overrides()
        .map_err(|e| ConnectError::native("Resolve provider environment", e))?;
    effective
        .validate(meerkat_models::canonical())
        .map_err(|e| ConnectError::native("Validate provider preview", e))?;
    if let Some(prior) = startup.config.realm.get(realm.as_str()) {
        let before = serde_json::to_value(prior)
            .map_err(|e| ConnectError::native("Retain provider realm fields", e))?;
        let after = serde_json::to_value(effective.realm.get(realm.as_str()))
            .map_err(|e| ConnectError::native("Read provider preview fields", e))?;
        for (field, value) in before
            .as_object()
            .ok_or_else(|| invalid("Invalid native realm fields"))?
        {
            if field == "default_binding" {
                continue;
            }
            if matches!(field.as_str(), "backend" | "auth" | "binding") {
                for (name, old) in value
                    .as_object()
                    .ok_or_else(|| invalid("Invalid native realm map"))?
                {
                    if after.get(field).and_then(|m| m.get(name)) != Some(old) {
                        return Err(invalid("Provider preview hides existing realm settings"));
                    }
                }
            } else if after.get(field) != Some(value) {
                return Err(invalid("Provider preview changes unreviewed realm fields"));
            }
        }
    }

    let section = effective
        .realm
        .get(realm.as_str())
        .ok_or_else(|| invalid("Preview lost own realm"))?;
    RealmConnectionSet::from_config(realm.as_str(), section)
        .map_err(|e| ConnectError::native("Validate provider account routes", e))?;
    let reference = meerkat_core::AuthBindingRef {
        realm: realm.clone(),
        binding: BindingId::parse(name)
            .map_err(|e| ConnectError::native("Read native binding name", e))?,
        profile: None,
        origin: BindingOrigin::Configured,
    };
    let target = resolve_explicit_auth_binding_target(&effective, &reference)
        .map_err(|e| ConnectError::native("Resolve provider preview target", e))?;
    if let Some(identity) = oauth_identity(&target) {
        validate_oauth_login_connection_target(&target, identity)
            .map_err(|e| ConnectError::provider_setup("Validate OAuth target", e))?;
    } else {
        validate_api_key_target(&effective, &target)?;
    }
    let request = choice.request(reference);
    resolve_create_session_model(&effective, request.clone())
        .map_err(|e| ConnectError::native("Resolve provider preview model", e))?;
    if own(startup).await? != original {
        return Err(invalid("Provider configuration changed during preview"));
    }
    let review = json!({"kind":"target_review","selection":choice.show(),
        "new_default_binding":target.auth_binding,"routes":delta.0["realm"][realm.as_str()]["binding"],
        "credential_account":if copilot { Some(account) } else { None },"agent_model_changed":false});
    Ok(Prepared {
        runtime,
        generation: snapshot.generation,
        own: original,
        delta,
        target,
        request,
        review,
    })
}
impl Prepared {
    pub async fn commit(
        self,
        startup: &mut SessionStartup,
        bridge: &Bridge,
    ) -> Result<
        (
            ResolvedConnectionTarget,
            CreateSessionModelResolutionRequest,
        ),
        ConnectError,
    > {
        if bridge.retired() {
            return Err(invalid(
                "Provider interaction retired before write admission",
            ));
        }
        if own(startup).await? != self.own {
            return Err(invalid("Provider document changed after review"));
        }
        // The reviewed eligibility included inherited targets. Reobserve the
        // actual effective chain after human confirmation, not only this own
        // document. This snapshot is not a cross-realm atomic transaction;
        // only the own document generation is guarded by the admitted CAS.
        let effective = crate::realm::read_config(&startup.storage).await?;
        first(
            &effective,
            self.own.as_ref(),
            &startup.storage.locator.realm,
        )?;
        if bridge.retired() {
            return Err(invalid(
                "Provider interaction retired before write admission",
            ));
        }
        // Confirm admits one CAS operation. Cancellation after admission joins
        // it, including its native lock wait; it is not post-lock reauthorization.
        let result = self.runtime.patch(self.delta, Some(self.generation)).await;
        let config = crate::realm::read_config(&startup.storage).await?;
        startup.config = config;
        result.map_err(|e| {
            ConnectError::native("Native provider write failed; target may be saved", e)
        })?;
        let current =
            resolve_explicit_auth_binding_target(&startup.config, &self.target.auth_binding)
                .map_err(|e| ConnectError::native("Read saved provider target", e))?;
        if current != self.target {
            return Err(invalid(
                "Saved provider target differs from reviewed target",
            ));
        }
        bridge
            .notice(json!({"kind":"target_saved","auth_binding":current.auth_binding}))
            .await?;
        Ok((current, self.request))
    }
}
