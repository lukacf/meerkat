//! Add the first direct model target through the selected realm's native owner.
//! Only nonsecret named connection maps, the reviewed first default binding,
//! and an explicitly reviewed model default when needed are patched. No
//! effective configuration or inherited section is copied into the document.

use super::*;
use meerkat_core::{
    Config, ConfigDelta, ConfigRuntime, EffectiveConfigReader, Provider, RealmConfigSource,
};
use meerkat_core::connection::RealmConnectionSet;
use serde_json::Value;

pub(super) struct TargetReview {
    target: ResolvedConnectionTarget,
    request: CreateSessionModelResolutionRequest,
    model: String,
    provider: Provider,
}

struct PreparedTarget {
    review: TargetReview,
    own_document: Option<Value>,
    generation: u64,
    delta: ConfigDelta,
    previous_default_model: Option<String>,
}

fn invalid(message: &str) -> ConnectError {
    ConnectError::InvalidInput(message.to_owned())
}

fn direct_backend(provider: Provider) -> Result<&'static str, ConnectError> {
    match provider {
        Provider::OpenAI => Ok("openai_api"),
        Provider::Anthropic => Ok("anthropic_api"),
        Provider::Gemini => Ok("google_genai"),
        _ => Err(invalid("First-target setup currently supports direct OpenAI, Anthropic and Google API keys. This selected model needs an existing native target.")),
    }
}

async fn own_document(startup: &SessionStartup) -> Result<Option<Value>, ConnectError> {
    crate::realm::config_source(&startup.storage)?
        .raw_config_for_realm(&startup.storage.locator.realm)
        .await
        .map_err(|error| ConnectError::native("Read first-target document presence", error))?
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| ConnectError::native("Retain first-target document presence", error))
}

fn check_absence(config: &Config, own: Option<&Value>, realm: &meerkat_core::RealmId, name: &str)
    -> Result<(), ConnectError>
{
    // Raw presence, not a default-filled Config, decides whether this scalar
    // was already supplied. Even an explicit empty/default value is preserved.
    if own.and_then(|value| value.get("realm")).and_then(|value| value.get(realm.as_str()))
        .is_some_and(|section| section.get("default_binding").is_some())
    {
        return Err(invalid("This realm already declares a default binding. First-target setup will not replace it; review its native configuration before trying again."));
    }
    let chain = RealmChain::resolve(config, realm)
        .map_err(|error| ConnectError::native("Resolve first-target inheritance", error))?;
    for owner in chain.realms() {
        if let Some(section) = config.realm.get(owner.as_str()) {
            if !section.binding.is_empty() {
                return Err(invalid("A native model binding appeared while first-target setup was being prepared. Restart setup to review the existing target."));
            }
            if owner == realm && (section.backend.contains_key(name) || section.auth.contains_key(name)) {
                return Err(invalid("That connection name is already used by a backend or auth profile in this realm. No profile was replaced."));
            }
            if owner == realm && section.default_binding.is_some() {
                return Err(invalid("This realm already has an effective default binding. First-target setup will not replace it."));
            }
        }
    }
    Ok(())
}

fn check_preserved_realm_fields(current: &Config, preview: &Config, realm: &meerkat_core::RealmId)
    -> Result<(), ConnectError>
{
    let Some(prior) = current.realm.get(realm.as_str()) else { return Ok(()); };
    let after = preview.realm.get(realm.as_str())
        .ok_or_else(|| invalid("The native preview lost the selected realm"))?;
    let prior = serde_json::to_value(prior)
        .map_err(|error| ConnectError::native("Retain existing realm fields for review", error))?;
    let after = serde_json::to_value(after)
        .map_err(|error| ConnectError::native("Check native realm preview preservation", error))?;
    let fields = prior.as_object().ok_or_else(|| invalid("Native realm fields are not an object"))?;
    for (field, value) in fields {
        if field == "default_binding" { continue; }
        if matches!(field.as_str(), "backend" | "auth" | "binding") {
            if let Some(entries) = value.as_object() {
                for (name, prior) in entries {
                    if after.get(field).and_then(|map| map.get(name)) != Some(prior) {
                        return Err(invalid("Creating this connection would hide existing settings for this realm from an ancestor document. No configuration was saved. Add the connection manually to the native realm configuration while preserving those settings, then rerun Connect."));
                    }
                }
            }
        } else if after.get(field) != Some(value) {
            return Err(invalid("Creating this connection would change an unreviewed realm setting. No configuration was saved. Add the connection manually to the native realm configuration while preserving that setting, then rerun Connect."));
        }
    }
    Ok(())
}

impl TargetReview {
    fn verify(&self, config: &Config, realm: &meerkat_core::RealmId) -> Result<(), ConnectError> {
        let target = resolve_explicit_auth_binding_target(config, &self.target.auth_binding)
            .map_err(|error| ConnectError::native("Resolve reviewed first-target identity", error))?;
        let owner = meerkat_core::connection::resolve_write_owner(
            config, realm, &self.target.auth_binding.binding,
        ).map_err(|error| ConnectError::provider_setup("Resolve first-target credential write owner", error))?;
        if target != self.target || owner != *realm || target.auth_binding.realm != *realm {
            return Err(invalid("The first model target or its credential owner changed after review. Setup cannot continue with that reviewed target."));
        }
        validate_api_key_target(config, &target)?;
        let selected = resolve_create_session_model(config, self.request.clone())
            .map_err(|error| ConnectError::native("Resolve reviewed first-target model", error))?;
        let ordinary = resolve_create_session_model(config, CreateSessionModelResolutionRequest::default())
            .map_err(|error| ConnectError::native("Resolve ordinary model after first-target setup", error))?;
        if selected.model != self.model || selected.provider != self.provider
            || ordinary.model != self.model || ordinary.provider != self.provider
        {
            return Err(invalid("Native model resolution no longer matches the reviewed first target. The requested model was not replaced."));
        }
        // Session model resolution and realm-scoped credential selection are
        // separate native operations. Verify ordinary selection with no model,
        // provider or auth-binding override, including actual candidate order.
        let candidates = resolve_auth_binding_candidates_for_provider(
            config, ordinary.provider, ordinary.auth_binding.as_ref(), Some(realm), true,
        ).map_err(|error| ConnectError::native("Resolve ordinary first-target candidates", error))?;
        if candidates.first() != Some(&self.target) {
            return Err(invalid("The reviewed first target is not the native realm's ordinary credential choice. Setup cannot continue with that selection."));
        }
        Ok(())
    }

    pub(super) async fn refresh(&self, startup: &mut SessionStartup) -> Result<(), ConnectError> {
        let config = crate::realm::read_config(&startup.storage).await?;
        self.verify(&config, &startup.storage.locator.realm)?;
        // Preserve the exact startup, factory, stage and native auth lease. The
        // first controller later consumes this freshly read builder config.
        startup.config = config;
        Ok(())
    }
}

async fn prepare(
    startup: &SessionStartup,
    runtime: &ConfigRuntime,
    original_request: CreateSessionModelResolutionRequest,
    name: BindingId,
    expected_model: &str,
    expected_provider: Provider,
) -> Result<PreparedTarget, ConnectError> {
    let source = crate::realm::config_source(&startup.storage)?;
    let snapshot = runtime.get().await
        .map_err(|error| ConnectError::native("Read first-target native generation", error))?;
    let own = own_document(startup).await?;
    let current = crate::realm::read_config(&startup.storage).await?;
    check_absence(&current, own.as_ref(), &startup.storage.locator.realm, name.as_str())?;
    // Snapshot and raw presence are separate native observations, not an atomic
    // reservation. Check snapshot keys as well, and retain the original CAS.
    if snapshot.config.realm.get(startup.storage.locator.realm.as_str()).is_some_and(|section|
        !section.binding.is_empty() || section.backend.contains_key(name.as_str())
            || section.auth.contains_key(name.as_str()))
    {
        return Err(invalid("The own-realm snapshot already contains a binding or the requested profile name. No existing profile was replaced."));
    }
    let selected = resolve_create_session_model(&current, original_request.clone())
        .map_err(|error| ConnectError::native("Resolve first-target requested model", error))?;
    if selected.model != expected_model || selected.provider != expected_provider {
        return Err(invalid("The native model selection changed while setup was awaiting input. No target was saved; review the model again."));
    }
    let ordinary = resolve_create_session_model(&current, CreateSessionModelResolutionRequest::default())
        .map_err(|error| ConnectError::native("Resolve prior ordinary realm model", error))?;
    let previous_default_model = (ordinary.model != selected.model).then_some(ordinary.model);
    let backend = direct_backend(selected.provider)?;
    let mut patch = serde_json::json!({"realm": {
        (startup.storage.locator.realm.as_str()): {
            "default_binding": name.as_str(),
            "backend": {(name.as_str()): {"provider": selected.provider.as_str(), "backend_kind": backend}},
            "auth": {(name.as_str()): {"provider": selected.provider.as_str(), "auth_method": "api_key", "source": {"kind": "managed_store"}}},
            "binding": {(name.as_str()): {"backend_profile": name.as_str(), "auth_profile": name.as_str(), "default_model": selected.model.clone()}},
        }
    }});
    if previous_default_model.is_some() {
        // This is a separately reviewed first-target default change, not
        // automatic persistence of an ordinary invocation's --model flag.
        patch["agent"] = serde_json::json!({"model": selected.model.clone()});
    }
    let delta = ConfigDelta(patch);
    let preview = runtime.patch_preview(&delta).await
        .map_err(|error| ConnectError::native("Preview first native model target", error))?;
    let mut effective = EffectiveConfigReader::new(source)
        .effective_config_over_head(&startup.storage.locator.realm, preview).await
        .map_err(|error| ConnectError::native("Compose first-target native preview", error))?;
    effective.apply_env_overrides()
        .map_err(|error| ConnectError::native("Apply native first-target environment", error))?;
    effective.validate(meerkat_models::canonical())
        .map_err(|error| ConnectError::native("Validate first-target configuration", error))?;
    check_preserved_realm_fields(&current, &effective, &startup.storage.locator.realm)?;
    let section = effective.realm.get(startup.storage.locator.realm.as_str())
        .ok_or_else(|| invalid("The native preview did not retain the selected realm"))?;
    RealmConnectionSet::from_config(startup.storage.locator.realm.as_str(), section)
        .map_err(|error| ConnectError::native("Validate first-target connection graph", error))?;
    let reference = AuthBindingRef { realm: startup.storage.locator.realm.clone(),
        binding: name, profile: None, origin: BindingOrigin::Configured };
    let target = resolve_explicit_auth_binding_target(&effective, &reference)
        .map_err(|error| ConnectError::native("Resolve first-target native preview", error))?;
    let request = CreateSessionModelResolutionRequest {
        auth_binding: Some(target.auth_binding.clone()), ..original_request
    };
    let review = TargetReview { target, request,
        model: selected.model, provider: selected.provider };
    review.verify(&effective, &startup.storage.locator.realm)?;
    if own_document(startup).await? != own {
        return Err(invalid("The native document changed while the first target was being prepared. Review setup again; nothing was saved by this operation."));
    }
    Ok(PreparedTarget { review, own_document: own, generation: snapshot.generation, delta,
        previous_default_model })
}

async fn save<I: Future<Output = io::Result<()>>, W: Write>(
    startup: &mut SessionStartup, runtime: &ConfigRuntime, prepared: PreparedTarget,
    output: &mut W, mut interrupt: Pin<&mut I>,
) -> Result<Option<TargetReview>, ConnectError> {
    if own_document(startup).await? != prepared.own_document {
        return Err(invalid("The native document changed after review. Nothing was saved by this operation; review setup again."));
    }
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        signal.map_err(|error| ConnectError::native("Listen for first-target cancellation", error))?;
        return Ok(None);
    }
    let write = runtime.patch(prepared.delta, Some(prepared.generation));
    tokio::pin!(write);
    let (saved, interrupted) = if let Some(saved) = write.as_mut().now_or_never() {
        (saved, None)
    } else {
        tokio::select! { biased;
            signal = interrupt.as_mut() => {
                let notice = writeln!(output, "Cancellation requested. Waiting for native model configuration to settle; the target may be saved.")
                    .and_then(|()| output.flush())
                    .map_err(|error| ConnectError::native("Write first-target cancellation", error));
                let signal = signal.map_err(|error| ConnectError::native("Listen for first-target cancellation", error));
                (write.await, Some(match signal { Ok(()) => notice, Err(error) => Err(combined(error, notice)) }))
            }
            saved = write.as_mut() => (saved, None),
        }
    };
    let outcome = match saved {
        Ok(_) => prepared.review.refresh(startup).await.and_then(|()| {
            writeln!(output, "The reviewed native model target is saved. It remains saved if authentication or setup stops.")
                .and_then(|()| output.flush())
                .map_err(|error| ConnectError::native("Write first-target storage result", error))
        }),
        Err(error @ meerkat_core::ConfigRuntimeError::GenerationConflict { .. }) => {
            let notice = writeln!(output, "The native configuration generation changed after review. This operation wrote no configuration and requested no credential. Review setup again.")
                .and_then(|()| output.flush())
                .map_err(|error| ConnectError::native("Write first-target generation conflict", error));
            Err(combined(ConnectError::native("Save reviewed model target; generation changed before write", error), notice))
        }
        Err(error) => {
            // Native config replacement precedes generation publication. Even
            // a sidecar error can follow a saved target. Reconcile but never
            // retry, roll back, collect a secret or open a controller here.
            let reconciled = crate::realm::read_config(&startup.storage).await;
            let present = reconciled.as_ref().is_ok_and(|config|
                prepared.review.verify(config, &startup.storage.locator.realm).is_ok());
            let notice = writeln!(output, "Saving the native model target failed and may be partial. Reviewed target currently confirmed: {}. No credential was requested and no retry was started.", if present { "yes" } else { "no" })
                .and_then(|()| output.flush())
                .map_err(|error| ConnectError::native("Write first-target partial outcome", error));
            let primary = ConnectError::native("Save reviewed model target; outcome may be partial", error);
            let primary = match reconciled { Ok(_) => primary, Err(error) => combined(primary, Err(error)) };
            Err(combined(primary, notice))
        }
    };
    let interrupted = match interrupted {
        Some(value) => Some(value),
        None => interrupt.as_mut().now_or_never().map(|signal|
            signal.map_err(|error| ConnectError::native("Listen for first-target cancellation", error))),
    };
    match (outcome, interrupted) {
        (Err(error), Some(cleanup)) => Err(combined(error, cleanup)),
        (Err(error), None) => Err(error),
        (Ok(()), Some(stopped)) => { stopped?; Ok(None) }
        (Ok(()), None) => Ok(Some(prepared.review)),
    }
}

pub(super) async fn present<I: Future<Output = io::Result<()>>, W: Write>(
    startup: &mut SessionStartup, original_request: CreateSessionModelResolutionRequest,
    input: &mut TerminalInput, output: &mut W, mut interrupt: Pin<&mut I>,
) -> Result<Option<CreateSessionModelResolutionRequest>, ConnectError> {
    input.require_terminal()
        .map_err(|error| ConnectError::native("Require protected first-target terminal", error))?;
    let selected = resolve_create_session_model(&startup.config, original_request.clone())
        .map_err(|error| ConnectError::native("Resolve first-target model", error))?;
    direct_backend(selected.provider)?;
    writeln!(output, "No native model target is configured for this realm or its parents. Create a managed API-key target for provider: {}; model: {}; owner realm: {}.",
        selected.provider.as_str(), review_text(&selected.model), review_text(startup.storage.locator.realm.as_str()))
        .map_err(|error| ConnectError::native("Write first-target introduction", error))?;
    let default_name = format!("{}_api_key", selected.provider.as_str());
    let name_prompt = format!("Connection name [{default_name}] (Enter to accept, or /quit): ");
    let name = loop {
        let Some(name) = line(input, output, &name_prompt, interrupt.as_mut()).await? else { return Ok(None); };
        let name = if name.trim().is_empty() { default_name.as_str() } else { name.trim() };
        match BindingId::parse(name) {
            Ok(name) => break name,
            Err(_) => writeln!(output, "Choose another connection name, or /quit.")
                .map_err(|error| ConnectError::native("Write first-target name guidance", error))?,
        }
    };
    let runtime = crate::realm::own_config_runtime(&startup.storage)?;
    let prepared = prepare(startup, &runtime, original_request, name, &selected.model, selected.provider).await?;
    let target = &prepared.review.target;
    writeln!(output, "Create connection '{}': provider {}; model {}; owner realm {}. Use it as this realm's first default connection. The next step lets you reuse an existing key for this exact connection or explicitly store or replace it. Saved configuration remains if setup stops.",
        review_text(target.auth_binding.binding.as_str()), prepared.review.provider.as_str(),
        review_text(&prepared.review.model), review_text(target.auth_binding.realm.as_str()))
        .map_err(|error| ConnectError::native("Write first-target review", error))?;
    if let Some(previous) = &prepared.previous_default_model {
        writeln!(output, "Change this realm's default model from '{}' to '{}'. Future ordinary runs in this realm will use the new model.",
            review_text(previous), review_text(&prepared.review.model))
            .map_err(|error| ConnectError::native("Write first-target model-default review", error))?;
    } else {
        writeln!(output, "Keep this realm's current default model '{}'; no model override is added.", review_text(&prepared.review.model))
            .map_err(|error| ConnectError::native("Write retained model-default review", error))?;
    }
    writeln!(output, "{}", if startup.storage.locator.realm.as_str() == "global" {
        "This writes the global realm configuration. Its defaults are available to realms that inherit global."
    } else {
        "Only the selected realm's own configuration is written. Parent and global documents are unchanged."
    }).map_err(|error| ConnectError::native("Write first-target document scope", error))?;
    loop {
        let Some(answer) = line(input, output, "Type 'create' to save this model target, or /quit: ", interrupt.as_mut()).await? else { return Ok(None); };
        if answer.trim() == "create" { break; }
    }
    let Some(review) = save(startup, &runtime, prepared, output, interrupt.as_mut()).await? else { return Ok(None); };
    let observation = observe(startup, &review.target).await?;
    let prompt = if observation.admits_open() {
        "Type 'enter-key' to store or replace this owner's API key, 'use' for native open, or /quit: "
    } else {
        "Type 'enter-key' to store or replace this owner's API key, or /quit: "
    };
    loop {
        let Some(answer) = line(input, output, prompt, interrupt.as_mut()).await? else { return Ok(None); };
        match answer.trim() {
            "use" if observation.admits_open() => { review.refresh(startup).await?; return Ok(Some(review.request)); }
            "enter-key" => return if api_key_login_reviewed(startup, &review.target, Some(&review), input, output, interrupt.as_mut()).await? {
                Ok(Some(review.request))
            } else { Ok(None) },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
