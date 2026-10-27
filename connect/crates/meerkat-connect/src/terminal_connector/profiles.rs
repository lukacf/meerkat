//! Terminal input for the native realm profile owner. No profile or secret store
//! lives here; retained profiles are only immutable review evidence.
use super::{review, review_request, ReviewOutcome};
use crate::{ConnectError, approval::{ApprovalPresenter, LocalApprovals}, terminal::{TerminalInput, review_text}};
use futures_util::FutureExt;
use meerkat::{ConnectorClientAuthProfile, ConnectorClientAuthSource, ConnectorClientSecretUpdate, ConnectorClientUpdateError, ConnectorOAuthTarget, RealmConnectorClients};
use meerkat_core::{ApprovalStatus, ConnectorClientAuthMethod, ConnectorClientConfig, ResolvedStorage, SessionId};
use serde_json::{Value, json};
use std::{future::Future, io::{self, Write}, pin::Pin, sync::Arc};
use tokio::sync::oneshot;

pub(super) enum Preparation {
    Current(Option<ConnectorClientAuthProfile>),
    Denied,
    CallerGone,
    Stopped,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ProfileError {
    #[error("{message}")]
    Refused { reason: &'static str, message: &'static str },
    #[error(transparent)]
    Terminal(#[from] ConnectError),
}

fn unavailable(message: &'static str) -> ProfileError {
    ProfileError::Refused { reason: "client_profile_unavailable", message }
}

impl ProfileError {
    pub(super) fn respond(self, caller: oneshot::Sender<Value>, output: &mut impl Write) -> Result<bool, ConnectError> {
        match self {
            Self::Refused { reason, message } => {
                // Only allowlisted static text reaches either surface. Native
                // backend errors and secret values are never serialized here.
                writeln!(output, "{message}").map_err(|e| ConnectError::native("Display client profile refusal", e))?;
                let _ = caller.send(json!({"state":"unavailable", "phase":"client_profile",
                    "reason":reason, "instruction":message, "grant_issued":false}));
                Ok(true)
            }
            Self::Terminal(error) => Err(error),
        }
    }
}

pub(super) enum Presentation {
    Current(Option<ConnectorClientAuthProfile>, oneshot::Sender<Value>),
    Finished(bool),
}

/// The exact conversion used by the connector presenter before any sign-in.
/// Ordinary refusal settles this tool request. Only an explicit stop exits the
/// conversation; losing this request is not authority to cancel its parent.
pub(super) fn presentation(
    result: Result<Preparation, ProfileError>, caller: oneshot::Sender<Value>, output: &mut impl Write,
) -> Result<Presentation, ConnectError> {
    Ok(match result {
        Ok(Preparation::Current(profile)) => Presentation::Current(profile, caller),
        Ok(Preparation::Denied) => {
            let _ = caller.send(json!({"state":"denied", "phase":"client_profile", "grant_issued":false}));
            Presentation::Finished(true)
        }
        Ok(Preparation::CallerGone) => Presentation::Finished(true),
        Ok(Preparation::Stopped) => Presentation::Finished(false),
        Err(error) => Presentation::Finished(error.respond(caller, output)?),
    })
}

#[derive(Clone, Copy)]
enum Stop { Requested, CallerGone }
impl Stop {
    fn preparation(self) -> Preparation {
        match self { Self::Requested => Preparation::Stopped, Self::CallerGone => Preparation::CallerGone }
    }
}

enum Save<T> {
    Unentered(Stop),
    Settled { result: T, stop: Option<Stop> },
}

/// Public clients need no named profile. A named profile must belong to the
/// exact issuer/client descriptor; a conflicting name is never silently used.
pub(super) async fn selected(
    source: &RealmConnectorClients, target: &ConnectorOAuthTarget,
) -> Result<Option<ConnectorClientAuthProfile>, ProfileError> {
    let Some(id) = target.client_auth_profile.as_deref() else { return Ok(None) };
    let profiles = source.current_profiles().await.map_err(|_| unavailable(
        "The native realm client profiles could not be read. No sign-in was started by this read.",
    ))?;
    let profile = profiles.into_iter().find(|profile| profile.id == id);
    if profile.as_ref().is_some_and(|profile| profile.issuer != target.issuer || profile.client != target.client) {
        return Err(unavailable("This native profile names a different issuer or client. Select the matching profile; it was not replaced."));
    }
    Ok(profile)
}

pub(super) fn binding(profile: Option<&ConnectorClientAuthProfile>) -> Option<String> {
    profile.map(ConnectorClientAuthProfile::binding)
}

pub(super) async fn recheck(
    source: &RealmConnectorClients, target: &ConnectorOAuthTarget,
    reviewed: Option<&ConnectorClientAuthProfile>,
) -> Result<(), ProfileError> {
    let current = selected(source, target).await?;
    if target.client_auth_profile.is_some() != reviewed.is_some()
        || binding(current.as_ref()) != binding(reviewed)
        || current.as_ref().map(|p| &p.client_secret) != reviewed.map(|p| &p.client_secret)
    {
        return Err(unavailable("The native client profile changed or disappeared after review. Nothing further was authorized; review its current definition before continuing."));
    }
    Ok(())
}

/// Projection for protected review only. Secret values are neither read nor
/// serialized, including when the native source points to an inherited secret.
pub(super) fn display(profile: Option<&ConnectorClientAuthProfile>) -> Value {
    profile.map_or(Value::Null, |profile| json!({
        "id":profile.id, "binding":profile.binding(), "issuer":profile.issuer,
        "client":profile.client, "sends_client_secret":profile.client_secret.is_some(),
        "resource_indicator":profile.resource_indicator,
        "authorize_params":profile.authorize_params, "protocol":profile.protocol,
        "registered_redirect_uris":profile.registered_redirect_uris,
    }))
}

// Reuse the native loader for validation and the canonical review binding. This
// transient document is never persisted; put_connector_client owns all writes.
fn reviewed_definition(
    realm: &meerkat_core::RealmId, id: &str, client: &ConnectorClientConfig,
) -> Result<ConnectorClientAuthProfile, ProfileError> {
    let mut config = meerkat_core::Config::default();
    config.realm.entry(realm.as_str().to_owned()).or_default()
        .connector_client.insert(id.to_owned(), client.clone());
    meerkat::connector_client_auth_profiles(&config, realm)
        .map_err(|_| unavailable("The supplied client settings are not a valid native profile. Nothing was saved."))?
        .into_iter().find(|profile| profile.id == id)
        .ok_or_else(|| unavailable("Native profile validation did not return the selected definition."))
}

pub(super) async fn prepare<F: Future<Output = io::Result<()>> + Send, W: Write>(
    storage: &ResolvedStorage, factory: &meerkat::AgentFactory,
    source: &RealmConnectorClients, target: &mut ConnectorOAuthTarget, session: &SessionId,
    approvals: &LocalApprovals, presenter: &mut ApprovalPresenter,
    input: &mut TerminalInput, output: &mut W, caller: &mut oneshot::Sender<Value>,
    check_selected: impl Fn() -> Result<(), &'static str>, mut interrupt: Pin<&mut F>,
) -> Result<Preparation, ProfileError> {
    if target.client_auth_profile.is_none() {
        let offered = source.current_profiles().await.map_err(|_| unavailable("The native realm client profiles could not be read."))?;
        check_selected().map_err(unavailable)?;
        if caller.is_closed() { return Ok(Preparation::CallerGone) }
        let ids = offered.iter().filter(|profile| profile.issuer == target.issuer && profile.client == target.client)
            .map(|profile| review_text(&profile.id)).collect::<Vec<_>>();
        writeln!(output, "Matching client profiles: {}", if ids.is_empty() { "none".into() } else { ids.join(", ") })
            .map_err(|error| ConnectError::native("Display native profile choices", error))?;
        let choice = tokio::select! { biased;
            signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for profile selection cancellation", e))?; return Ok(Preparation::Stopped) },
            _ = caller.closed() => return Ok(Preparation::CallerGone),
            choice = line(input, output, "Client profile [Enter=public default, or existing/new profile ID]: ") => choice?,
        };
        let Some(choice) = choice else { return Ok(Preparation::Stopped) };
        target.client_auth_profile = (!choice.is_empty()).then_some(choice);
    }
    let current = selected(source, target).await?;
    check_selected().map_err(unavailable)?;
    if caller.is_closed() { return Ok(Preparation::CallerGone) }
    let repair_realm = match current.as_ref().and_then(|p| p.client_secret.as_ref()) {
        Some(meerkat::ConnectorClientSecret::RealmStore { realm }) => {
            let persistence = factory.resolution_provider_auth_persistence()
                .map_err(|_| unavailable("The original credential persistence is unavailable."))?
                .ok_or_else(|| unavailable("The original credential persistence is unavailable."))?;
            // Native validates the exact row/binding. Keep only presence, never
            // display or send the returned secret and never treat read failure as absence.
            let present = meerkat::load_connector_client_secret(
                persistence.token_store().as_ref(), realm, &current.as_ref().expect("profile selected").binding(),
            ).await.map_err(|_| ProfileError::Refused {
                reason: "client_secret_status_unavailable",
                message: "The client secret could not be checked. Nothing was changed; retry only after the credential store is available.",
            })?.is_some();
            (!present).then(|| realm.clone())
        }
        _ => None,
    };
    check_selected().map_err(unavailable)?;
    if caller.is_closed() { return Ok(Preparation::CallerGone) }
    if repair_realm.is_none() && (current.is_some() || target.client_auth_profile.is_none()) {
        return Ok(Preparation::Current(current));
    }
    let id = target.client_auth_profile.as_deref().ok_or_else(|| unavailable("The selected profile has no name."))?;
    let save_realm = repair_realm.as_ref().unwrap_or(&storage.locator.realm);
    let client = if repair_realm.is_some() {
        let profile = current.as_ref().expect("missing secret belongs to selected profile");
        writeln!(output, "Client secret required for profile {}. Repair its unchanged definition in realm {} (selected setup realm {}). The secret stays with that defining realm. Enter /quit to stop.",
            review_text(id), review_text(save_realm.as_str()), review_text(storage.locator.realm.as_str()))
            .map_err(|e| ConnectError::native("Display missing client secret", e))?;
        ConnectorClientConfig {
            issuer: profile.issuer.clone(), client: profile.client.clone(),
            client_auth: ConnectorClientAuthMethod::ClientSecretPost,
            authorize_params: profile.authorize_params.clone(), resource_indicator: profile.resource_indicator,
            protocol: profile.protocol.clone(), registered_redirect_uris: profile.registered_redirect_uris.clone(),
        }
    } else {
        writeln!(output, "Set up client profile {} in realm {} for issuer {} and client {}.\nUse the integration publisher's client settings. These answers stay in the terminal. Enter /quit to stop.",
            review_text(id), review_text(save_realm.as_str()), review_text(&target.issuer), review_text(&target.client))
            .map_err(|error| ConnectError::native("Display client setup", error))?;
        tokio::select! {
            biased;
            signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for client setup cancellation", e))?; return Ok(Preparation::Stopped) },
            _ = caller.closed() => return Ok(Preparation::CallerGone),
            result = collect(target, input, output) => match result? { Some(client) => client, None => return Ok(Preparation::Stopped) },
        }
    };
    check_selected().map_err(unavailable)?;
    let expected = reviewed_definition(save_realm, id, &client)?;
    if repair_realm.is_some() { recheck(source, target, current.as_ref()).await?; }
    let request = review_request(session, "Save this connector client profile", json!({
        "phase":if repair_realm.is_some() { "connector_client_secret_repair" } else { "connector_client_configuration" },
        "realm":save_realm.as_str(), "selected_setup_realm":storage.locator.realm.as_str(),
        "profile":display(Some(&expected)),
        "notice":"Save this exact named definition in the displayed defining realm. The native setter replaces that name if another writer changes it before this save; this is not a conditional create. A client secret is entered separately with echo disabled and stored only by the native credential owner. This does not authorize sign-in or account access."
    }))?;
    let decision = review(
        approvals, presenter, input, output, request, caller, interrupt.as_mut(),
    ).await?;
    let decision = match decision {
        ReviewOutcome::Decided(decision) => decision,
        ReviewOutcome::CallerGone => return Ok(Preparation::CallerGone),
        ReviewOutcome::Stopped => return Ok(Preparation::Stopped),
    };
    if decision.status != ApprovalStatus::Approved { return Ok(Preparation::Denied) }
    check_selected().map_err(unavailable)?;
    if caller.is_closed() { return Ok(Preparation::CallerGone) }
    let secret = if client.client_auth == ConnectorClientAuthMethod::ClientSecretPost {
        tokio::select! {
            biased;
            signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for client secret cancellation", e))?; return Ok(Preparation::Stopped) },
            _ = caller.closed() => return Ok(Preparation::CallerGone),
            secret = input.read_connector_client_secret(output) => match secret.map_err(|e| ConnectError::native("Read hidden client secret", e))? {
                Some(secret) => secret, None => return Ok(Preparation::Stopped),
            },
        }
    } else { ConnectorClientSecretUpdate::Keep };
    check_selected().map_err(unavailable)?;
    // Observe later writers without claiming this read is a create-only CAS.
    // An observed different definition refuses before the native setter enters.
    if repair_realm.is_some() {
        recheck(source, target, current.as_ref()).await?;
    } else if let Some(current) = selected(source, target).await?
        && (current.binding() != expected.binding() || current.client_secret != expected.client_secret)
    { return Err(unavailable("Another client definition appeared during review. It was not replaced; review that current profile before continuing.")); }
    check_selected().map_err(unavailable)?;
    let persistence = factory.resolution_provider_auth_persistence()
        .map_err(|_| unavailable("The original native credential persistence is unavailable."))?
        .ok_or_else(|| unavailable("The original native credential persistence is unavailable."))?;
    let path = crate::realm::config_source(storage)?.config_doc_path(save_realm);
    let store = Arc::new(meerkat_core::FileConfigStore::new(path, meerkat_models::canonical()));
    let updating = meerkat::put_connector_client(store, &persistence, save_realm, id, client, secret);
    let cancelled = async {
        tokio::select! { biased;
            signal = interrupt.as_mut() => signal.map(|()| Stop::Requested),
            _ = caller.closed() => Ok(Stop::CallerGone),
            end = input.wait_for_end() => end.map(|()| Stop::Requested),
        }
    };
    let (result, stop) = match settle(updating, cancelled, output).await? {
        Save::Unentered(stop) => return Ok(stop.preparation()),
        Save::Settled { result, stop } => (result, stop),
    };
    if let Err(error) = result {
        let failure = update_error(error);
        if let Some(stop) = stop {
            writeln!(output, "{failure}").map_err(|e| ConnectError::native("Display client save settlement", e))?;
            return Ok(stop.preparation());
        }
        return Err(failure);
    }
    writeln!(output, "Client profile {} was saved in realm {}. No sign-in or account grant was performed by that save.",
        review_text(id), review_text(save_realm.as_str()))
        .map_err(|error| ConnectError::native("Display saved client profile", error))?;
    if let Some(stop) = stop { return Ok(stop.preparation()) }
    recheck(source, target, Some(&expected)).await?;
    check_selected().map_err(unavailable)?;
    if caller.is_closed() { return Ok(Preparation::CallerGone) }
    Ok(Preparation::Current(Some(expected)))
}

/// Before entry cancellation has no effect. After entry, preserve and await the
/// same native coordinated update even if the request or terminal disappears.
async fn settle<T>(
    update: impl Future<Output = T>, cancel: impl Future<Output = io::Result<Stop>>,
    output: &mut impl Write,
) -> Result<Save<T>, ConnectError> {
    tokio::pin!(cancel);
    if let Some(signal) = cancel.as_mut().now_or_never() {
        return Ok(Save::Unentered(signal.map_err(|e| ConnectError::native("Listen for profile save cancellation", e))?));
    }
    tokio::pin!(update);
    if let Some(result) = update.as_mut().now_or_never() {
        return Ok(Save::Settled { result, stop: None });
    }
    tokio::select! { biased;
        signal = cancel.as_mut() => {
            let notice = writeln!(output, "The client-profile save has already started and may complete. Waiting for its result before closing this request.")
                .and_then(|()| output.flush());
            // Even failed terminal output cannot drop an entered native write.
            let result = update.await;
            notice.map_err(|e| ConnectError::native("Display pending client save settlement", e))?;
            let stop = signal.map_err(|e| ConnectError::native("Listen for profile save cancellation", e))?;
            Ok(Save::Settled { result, stop: Some(stop) })
        },
        result = update.as_mut() => Ok(Save::Settled { result, stop: None }),
    }
}

fn update_error(error: ConnectorClientUpdateError) -> ProfileError {
    use ConnectorClientUpdateError::*;
    // Never print token-store/config backend strings, which may contain private
    // material. Preserve the native effect category and never retry implicitly.
    if matches!(&error, SecretRequired(_)) {
        return ProfileError::Refused { reason: "client_secret_required", message: "The exact client profile needs a secret. Use the protected client-profile repair prompt; do not put a secret in chat." };
    }
    unavailable(match error {
        Invalid { .. } | SecretRequired(_) | SecretNotUsed(_) | Unknown { .. } =>
            "Native client settings or secret were refused. No configuration was saved by this attempt.",
        Ownership(_) => "Native profile update ownership was unavailable. This update did not start.",
        ConfigFailedAfterSecretStored { .. } =>
            "The native secret was stored, but configuration settlement is unknown. Do not repeat automatically; inspect native profile state before another approved update.",
        SecretCleanupFailed { .. } =>
            "The native client configuration was saved, but unused-secret cleanup failed. Do not repeat automatically; native cleanup remains incomplete.",
        Config(_) | Store(_) | OutcomeUnknown(_) =>
            "The native client update did not establish a complete outcome. It may have written configuration or a secret. Do not repeat automatically; inspect native profile state before another approved update.",
    })
}

async fn line(input: &mut TerminalInput, output: &mut impl Write, prompt: &str) -> Result<Option<String>, ConnectError> {
    if !input.fresh_prompt().map_err(|e| ConnectError::native("Prepare client setting input", e))? { return Ok(None) }
    write!(output, "{prompt}").and_then(|()| output.flush()).map_err(|e| ConnectError::native("Display client setting", e))?;
    match input.recv().await {
        Some(Ok(value)) if value != "/quit" => Ok(Some(value)),
        Some(Err(error)) => Err(ConnectError::native("Read client setting", error)),
        _ => Ok(None),
    }
}

async fn collect(target: &ConnectorOAuthTarget, input: &mut TerminalInput, output: &mut impl Write) -> Result<Option<ConnectorClientConfig>, ProfileError> {
    let client_auth = loop {
        let Some(choice) = line(input, output, "Client authentication [public/secret]: ").await? else { return Ok(None) };
        match choice.as_str() {
            "public" => break ConnectorClientAuthMethod::None,
            "secret" => break ConnectorClientAuthMethod::ClientSecretPost,
            _ => writeln!(output, "Enter public or secret. Do not enter the secret here.").map_err(|e| ConnectError::native("Display client choices", e))?,
        }
    };
    let advanced = loop {
        let Some(choice) = line(input, output, "Advanced settings? [y/N]: ").await? else { return Ok(None) };
        match choice.as_str() {
            "" | "n" | "N" | "no" => break false,
            "y" | "Y" | "yes" => break true,
            _ => writeln!(output, "Enter y or n.").map_err(|e| ConnectError::native("Display advanced choices", e))?,
        }
    };
    let mut resource_indicator = true;
    let mut protocol = None;
    let mut authorize_params = Vec::new();
    if advanced {
        resource_indicator = loop {
            let Some(choice) = line(input, output, "Send the OAuth resource parameter? [yes/no, Enter=yes]: ").await? else { return Ok(None) };
            match choice.as_str() {
                "" | "yes" => break true,
                "no" => break false,
                _ => writeln!(output, "Enter yes or no.").map_err(|e| ConnectError::native("Display resource choices", e))?,
            }
        };
        let Some(choice) = line(input, output, "Provider protocol ID [Enter=default]: ").await? else { return Ok(None) };
        protocol = (!choice.is_empty()).then_some(choice);
        loop {
            let Some(parameter) = line(input, output, "Additional non-secret sign-in parameter NAME=VALUE [Enter=done]: ").await? else { return Ok(None) };
            if parameter.is_empty() { break }
            let Some((name, value)) = parameter.split_once('=') else {
                writeln!(output, "Use NAME=VALUE, or Enter to finish.").map_err(|e| ConnectError::native("Display parameter format", e))?;
                continue;
            };
            if authorize_params.len() == 32 { return Err(unavailable("Too many additional sign-in parameters. Nothing was saved.")) }
            authorize_params.push((name.to_owned(), value.to_owned()));
        }
    }
    Ok(Some(ConnectorClientConfig {
        issuer: target.issuer.clone(), client: target.client.clone(), client_auth,
        authorize_params, resource_indicator,
        protocol,
        // This terminal uses the native loopback callback, not a remote HTTPS callback.
        registered_redirect_uris: Vec::new(),
    }))
}

#[cfg(test)]
mod tests;
