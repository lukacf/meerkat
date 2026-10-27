//! Standalone generic selection over the existing authenticated Forge owner.
//! Declarations are compatibility inputs; they never restore selection authority.
use super::*;
use std::{collections::BTreeSet, sync::OnceLock};

const PATH: &str = "/console/forge/frontends/descriptor";

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Descriptor {
    pub(super) host: ForgeHost,
    pub(super) private_fields: BTreeSet<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Selected {
    pub(super) origin: String,
    pub(super) descriptor: Descriptor,
}

pub(super) fn decode(status: u16, bytes: &[u8]) -> Result<Descriptor, ForgeError> {
    if bytes.len() > LIMIT { return Err(ForgeError::UnexpectedResponse); }
    serde_json::from_slice::<UniqueJson>(bytes).map_err(|_| ForgeError::UnexpectedResponse)?;
    if status == 200 {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Envelope { protocol: u8, host: ForgeHost, private_fields: Vec<String> }
        let value: Envelope = serde_json::from_slice(bytes).map_err(|_| ForgeError::UnexpectedResponse)?;
        let count = value.private_fields.len();
        let fields: BTreeSet<_> = value.private_fields.into_iter().collect();
        if value.protocol != 1 || !value.host.valid() || fields.len() != count {
            return Err(ForgeError::UnexpectedResponse);
        }
        return Ok(Descriptor { host:value.host, private_fields:fields });
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Detail { detail: String }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Login { error: String }
    if status == 401 {
        let value: Login = serde_json::from_slice(bytes).map_err(|_| ForgeError::UnexpectedResponse)?;
        return if value.error == "Console login required." { Err(ForgeError::SessionRequired) }
            else { Err(ForgeError::UnexpectedResponse) };
    }
    let value: Detail = serde_json::from_slice(bytes).map_err(|_| ForgeError::UnexpectedResponse)?;
    Err(match (status, value.detail.as_str()) {
        (501, UNSUPPORTED) => ForgeError::DeclarationUnsupported,
        (409, "The loaded application source changed. No frontend command was admitted.") => ForgeError::SourceChanged,
        (503, "The application frontend owner is unavailable. No activation is claimed.") => ForgeError::Unavailable,
        _ => ForgeError::UnexpectedResponse,
    })
}
impl ForgeClient {
    pub(super) async fn descriptor(&self) -> Result<Descriptor, ForgeError> {
        let response = self.request(reqwest::Method::GET, PATH)
            .header(COOKIE, self.session.as_ref().ok_or(ForgeError::SessionRequired)?.clone())
            .send().await.map_err(|_| ForgeError::Unconfirmed)?;
        let status = response.status().as_u16();
        if !content_type(&response, "application/json") || (status != 401 && !no_store(&response))
            || response.headers().contains_key(SET_COOKIE) {
            return Err(ForgeError::UnexpectedResponse);
        }
        decode(status, &bounded(response, LIMIT).await?)
    }
}

impl ForgeFrontend {
    /// Standalone generic mode. The agent requests the existing protected host
    /// selection; only its authenticated descriptor can supply declarations.
    /// This creates a fresh pair, without selecting an origin or logging in.
    pub fn deferred_application() -> (Self, ForgePresenter) {
        Self::with_recipe(configuration::Recipe::DeferredApplication(OnceLock::new()))
    }

    /// After native close and all presenter/effect joins, consume the unique
    /// old binding and move only its private client plus declaration expectation
    /// into a fresh pair, retaining only denial/status history. No selection,
    /// prepared proposal, proof or runtime is restored.
    /// Sharing the old dispatcher/presenter or an unsettled request refuses.
    pub fn continue_application_after_close(self) -> Result<(Self, ForgePresenter), ConnectError> {
        let unavailable = || ConnectError::InvalidInput(
            "Previous Forge owners remain or selection has not settled; no fresh Forge composition was created.".into());
        let old = Arc::try_unwrap(self.binding).map_err(|_| unavailable())?;
        let configuration::Recipe::DeferredApplication(selected) = old.recipe else { return Err(unavailable()); };
        let state = old.state.into_inner().map_err(|_| unavailable())?;
        let config = old.configuration.into_inner().map_err(|_| unavailable())?;
        if matches!(state, State::Pending(_)) { return Err(unavailable()); }
        let recovery = config.into_continuation()?;
        let retained = old.continued_client.into_inner().map_err(|_| unavailable())?;
        let client = match state {
            State::Captured { _captured } => Some(_captured.client),
            _ => retained,
        };
        let expectation = selected.into_inner();
        if let (Some(client), Some(expected)) = (&client, &expectation)
            && client.literal_origin != expected.origin { return Err(unavailable()); }
        let pair = Self::with_recipe(configuration::Recipe::DeferredApplication(
            expectation.map_or_else(OnceLock::new, OnceLock::from),
        ));
        *pair.0.binding.continued_client.lock().map_err(|_| unavailable())? = client;
        *pair.0.binding.configuration.lock().map_err(|_| unavailable())? = recovery;
        Ok(pair)
    }
}

impl Binding {
    pub(super) fn bind_descriptor(&self, token: &Arc<()>, client: &ForgeClient, descriptor: Descriptor)
        -> Result<bool, ConnectError>
    {
        let state = self.state.lock().map_err(|_| ended())?;
        if !matches!(&*state, State::Pending(active) if Arc::ptr_eq(active, token)) {
            return Err(ended());
        }
        let configuration::Recipe::DeferredApplication(selected) = &self.recipe else {
            return Err(ended());
        };
        let candidate = Selected { origin:client.literal_origin.clone(), descriptor };
        if let Some(expected) = selected.get() {
            if expected != &candidate { return Ok(false); }
        } else {
            selected.set(candidate).map_err(|_| ended())?;
        }
        Ok(true)
    }
}

impl Binding {
    /// Re-read declarations through the same private client immediately before
    /// using configuration. This never updates the confirmed expectation.
    pub(super) async fn current_descriptor(&self, client: &ForgeClient) -> Result<(), ForgeError> {
        let configuration::Recipe::DeferredApplication(selected) = &self.recipe else { return Ok(()); };
        let expected = selected.get().ok_or(ForgeError::Unavailable)?;
        let descriptor = client.descriptor().await?;
        if client.literal_origin != expected.origin || descriptor != expected.descriptor {
            return Err(ForgeError::SourceChanged);
        }
        Ok(())
    }
}
