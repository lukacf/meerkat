//! Private presentation of the same native provider operations as the terminal.
//! The host adapter owns bootstrap admission. This module owns no host policy,
//! configuration, token store, OAuth state, callback transport or agent session.
use super::*;
use crate::stdio::{PromptAnswer, PromptBridge, ProviderAction};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

#[derive(Serialize)]
struct ShownTarget {
    model: String,
    provider: String,
    realm: String,
    binding: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
}
struct Choice {
    id: String,
    request: CreateSessionModelResolutionRequest,
    target: ResolvedConnectionTarget,
    shown: ShownTarget,
    observation: Observation,
}
fn stopped() -> ConnectError {
    ConnectError::InvalidInput("The protected provider interaction ended".into())
}

/// Resolve configured choices through native owners before collecting a secret.
/// This returns only the selected native request, never a constructed runtime.
/// The caller must retain and join this future before initialization completes.
pub(crate) async fn present(
    startup: &mut SessionStartup,
    original: CreateSessionModelResolutionRequest,
    bridge: PromptBridge,
) -> Result<Option<CreateSessionModelResolutionRequest>, ConnectError> {
    if bridge.retired() {
        return Ok(None);
    }
    if ready_for_open(startup, &original).await? {
        return Ok((!bridge.retired()).then_some(original));
    }
    let mut choices = Vec::new();
    for reference in configured_choices(startup)? {
        let request = CreateSessionModelResolutionRequest {
            model: original.model.clone(),
            auth_binding: Some(reference.clone()),
            ..Default::default()
        };
        let selected = match resolve_create_session_model(&startup.config, request.clone()) {
            Ok(selected) => selected,
            Err(CreateSessionModelResolutionError::ProviderModelMismatch(_)) => continue,
            Err(error) => return Err(ConnectError::native("Resolve configured model target", error)),
        };
        let target = resolve_explicit_auth_binding_target(&startup.config, &reference)
            .map_err(|error| ConnectError::native("Resolve configured credential owner", error))?;
        let observation = observe(startup, &target).await?;
        choices.push(Choice {
            id: format!("target:{}", choices.len() + 1),
            request,
            shown: ShownTarget {
                model: selected.model,
                provider: selected.provider.as_str().into(),
                realm: target.auth_binding.realm.as_str().into(),
                binding: target.auth_binding.binding.as_str().into(),
                profile: target.auth_binding.profile.as_ref().map(|p| p.as_str().into()),
            },
            target,
            observation,
        });
    }
    if choices.is_empty() {
        return Err(ConnectError::InvalidInput(
            "No existing native provider target can be presented for this model".into(),
        ));
    }
    loop {
        let answer = bridge.ask(json!({"kind":"provider_target","choices":choices.iter().map(|choice| {
            json!({"choice_id":choice.id,"target":choice.shown,"status":choice.observation.label()})
        }).collect::<Vec<_>>()})).await;
        let id = match answer {
            Ok(PromptAnswer::ProviderTarget { choice_id }) => choice_id,
            Ok(PromptAnswer::Cancel) => return Ok(None),
            Err(_) if bridge.retired() => return Ok(None),
            _ => return Err(stopped()),
        };
        let choice = choices.iter().find(|choice| choice.id == id).ok_or_else(stopped)?;
        // Recheck the full native target against what was actually presented.
        let target = resolve_explicit_auth_binding_target(&startup.config, &choice.target.auth_binding)
            .map_err(|error| ConnectError::native("Validate selected credential owner", error))?;
        if target != choice.target {
            return Err(stopped());
        }
        let observation = observe(startup, &target).await?;
        let mut actions = vec![ProviderAction::Back];
        if observation.admits_open() { actions.push(ProviderAction::Use); }
        if managed_api_key(&target) {
            // Fail before displaying key input if native write ownership or its
            // selected backend is unavailable. A missing key is not a store fault.
            validate_api_key_target(&startup.config, &target)?;
            credential_store(startup)?;
            actions.push(ProviderAction::EnterKey);
        } else if oauth_identity(&target).is_some_and(|identity| {
            identity.supports_browser_flow() || identity == OAuthProviderIdentity::GitHubCopilot
        }) {
            actions.push(ProviderAction::SignIn);
        }
        let answer = bridge.ask(json!({"kind":"provider_action","target":choice.shown,"actions":actions})).await;
        let action = match answer {
            Ok(PromptAnswer::ProviderAction { action }) if actions.contains(&action) => action,
            Ok(PromptAnswer::Cancel) => return Ok(None),
            Err(_) if bridge.retired() => return Ok(None),
            _ => return Err(stopped()),
        };
        match action {
            ProviderAction::Back => continue,
            ProviderAction::Use => return Ok(Some(choice.request.clone())),
            ProviderAction::EnterKey => {
                validate_api_key_target(&startup.config, &target)?;
                let persistence = credential_store(startup)?;
                let answer = bridge.ask(json!({"kind":"provider_api_key","target":choice.shown,"max_bytes":8192})).await;
                let key = match answer {
                    Ok(PromptAnswer::ProviderApiKey { key }) => key,
                    Ok(PromptAnswer::Cancel) => return Ok(None),
                    Err(_) if bridge.retired() => return Ok(None),
                    _ => return Err(stopped()),
                };
                if key.trim().is_empty() || key.len() > 8192 || key.chars().any(char::is_control) {
                    return Err(stopped());
                }
                let interrupt = async { bridge.closed().await; Ok(()) };
                tokio::pin!(interrupt);
                let stored = save_api_key(startup, &target, persistence,
                    meerkat_core::auth::token_store::PersistedTokens::api_key(key),
                    &mut io::sink(), interrupt.as_mut()).await?;
                return Ok(stored.then(|| choice.request.clone()));
            }
            ProviderAction::SignIn => {
                let identity = oauth_identity(&target).ok_or_else(stopped)?;
                let complete = sign_in(startup, &target, identity,
                    serde_json::to_value(&choice.shown).map_err(|_| stopped())?, &bridge).await?;
                return Ok(complete.then(|| choice.request.clone()));
            }
        }
    }
}

fn credential_store(startup: &SessionStartup) -> Result<meerkat_core::auth::token_store::ProviderAuthPersistence, ConnectError> {
    startup.factory.resolution_provider_auth_persistence()
        .map_err(|error| ConnectError::provider_setup("Open native provider credentials", error))?
        .ok_or_else(|| ConnectError::InvalidInput("Native managed credential storage is unavailable".into()))
}

async fn sign_in(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    identity: OAuthProviderIdentity,
    shown: Value,
    bridge: &PromptBridge,
) -> Result<bool, ConnectError> {
    let (notices, mut received) = mpsc::channel(1);
    let mut present = move |notice| notices.try_send(notice)
        .map_err(|_| io::Error::other("Private provider presentation is unavailable"));
    let (cancel, mut cancelled) = watch::channel(false);
    let interrupt = async {
        tokio::select! {biased;_=bridge.closed()=>{},_=async {
            while !*cancelled.borrow_and_update() {
                if cancelled.changed().await.is_err() { break; }
            }
        }=>{}}
        Ok(())
    };
    tokio::pin!(interrupt);
    let mut sink = io::sink();
    let operation = async {
        let mut presentation = SignInPresentation::new(&mut present);
        let result = if identity == OAuthProviderIdentity::GitHubCopilot {
            device_login_presented(startup, target, &mut sink, interrupt.as_mut(), Some(&mut presentation)).await
        } else {
            browser_login_presented(startup, target, identity, &mut sink, interrupt.as_mut(), Some(&mut presentation)).await
        };
        let disposition = presentation.disposition(&result);
        (result, disposition)
    };
    tokio::pin!(operation);
    if bridge.retired() { return Ok(false); }
    // A synchronous first poll establishes custody before cancellation can
    // select another branch. Once entered, always join this exact operation.
    if let Some((result, _)) = operation.as_mut().now_or_never() { return result; }
    let notice = tokio::select! {biased;
        _=bridge.closed()=>{cancel.send_replace(true);return operation.await.0.map(|_|false)},
        (result, _)=operation.as_mut()=>return result,
        notice=received.recv()=>notice,
    };
    let prompt = match notice {
        Some(SignInNotice::Browser { authorization_url }) => json!({"kind":"provider_browser","target":shown,"authorization_url":authorization_url}),
        Some(SignInNotice::Device { verification_uri, user_code }) => json!({"kind":"provider_device","target":shown,"verification_uri":verification_uri,"user_code":user_code}),
        None => {
            cancel.send_replace(true);
            return Err(combined(stopped(), operation.await.0.map(|_| ())));
        }
    };
    let mut notice = match bridge.provider_notice(prompt).await {
        Ok(notice) => notice,
        Err(error) => {
            cancel.send_replace(true);
            return Err(combined(error, operation.await.0.map(|_| ())));
        }
    };
    tokio::select! {biased;
        _=notice.cancelled()=>{
            cancel.send_replace(true);
            operation.await.0.map(|_|false)
        },
        (result, disposition)=operation.as_mut()=>{
            let closure=notice.finish(disposition).await;
            match result { Ok(value)=>{closure?;Ok(value)},Err(error)=>Err(combined(error,closure)) }
        },
    }
}
