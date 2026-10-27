//! Protected Forge host selection and exact application configuration custody.
//! The application owns configuration, revision, activation and delivery.

use std::{
    fmt,
    future::Future,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use cookie::SameSite;
use futures_util::FutureExt;
use meerkat_core::{
    AgentToolDispatcher, ToolDef, ToolError, ToolMutationClass, ToolResult,
    ops::ToolDispatchOutcome, types::ToolCallView,
};
use reqwest::{
    Client, Response, Url,
    header::{
        CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, HeaderValue, LOCATION, ORIGIN, SET_COOKIE,
    },
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::{ConnectError, terminal::TerminalInput};

#[path = "forge_configuration.rs"]
mod configuration;
#[path = "forge_descriptor.rs"]
mod descriptor;
#[path = "forge_stdio.rs"]
mod stdio;
pub use configuration::{
    FORGE_INSPECT, FORGE_PREPARE, FORGE_REAUTHENTICATE, FORGE_STATUS, FORGE_SUBMIT,
};
pub(crate) use configuration::{FORGE_TOOLS, HeldForgeAction};
pub(crate) use stdio::present as present_stdio;

pub const FORGE_SELECT_HOST: &str = "forge_select_host";
const LIMIT: usize = 256 * 1024;
const LOGIN_LIMIT: usize = 64 * 1024;
const SUPPORT_PATH: &str = "/console/forge/frontends";
const UNSUPPORTED: &str = "This application has not declared frontend management support.";

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error(
        "Enter a literal http://127.0.0.1:PORT Forge Console origin, without credentials, path, query or fragment"
    )]
    InvalidOrigin,
    #[error(
        "Forge Console password must contain 12-4096 UTF-8 characters; its whitespace is preserved"
    )]
    InvalidPassword,
    #[error("The Forge Console response was not the supported login or frontend-support contract")]
    UnexpectedResponse,
    #[error(
        "The Forge Console request could not be confirmed; no automatic sign-in retry is allowed"
    )]
    Unconfirmed,
    #[error("Forge Console sign-in was refused; no automatic password retry was attempted")]
    SignInRefused,
    #[error(
        "Forge Console sign-in expired or was refused; a new protected interaction is required"
    )]
    SessionRequired,
    #[error("Forge Console frontend support is temporarily unavailable; no activation is claimed")]
    Unavailable,
    #[error("This application has not declared frontend management support.")]
    DeclarationUnsupported,
    #[error("Forge reports changed application source; no frontend support identity was captured")]
    SourceChanged,
    #[error(
        "Forge refused this configuration command before owner admission; retain the input and inspect current choices"
    )]
    CommandRefused,
    #[error("Forge refused the exact proposal approval; no automatic submission retry is allowed")]
    ApprovalRefused,
}

/// Private UTF-8 text, never serialized or forwarded through conversation input.
pub struct ForgePassword(String);
impl fmt::Debug for ForgePassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ForgePassword([private])")
    }
}
impl ForgePassword {
    pub(crate) fn new(value: String) -> Result<Self, ForgeError> {
        if !(12..=4096).contains(&value.chars().count()) {
            return Err(ForgeError::InvalidPassword);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeHost {
    protocol: u8,
    host_id: String,
    application_id: String,
    source_digest: String,
}
impl ForgeHost {
    /// Compare only trusted constructor facts, before opening a private caller.
    /// This does not authenticate an HTTP response or confer app permission.
    pub(crate) fn matches_application_id(&self, application: &str) -> bool {
        self.valid() && self.application_id == application
    }

    fn valid(&self) -> bool {
        let hex = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        self.protocol == 1
            && self.host_id.strip_prefix("forge:").is_some_and(hex)
            && hex(&self.source_digest)
            && !self.application_id.is_empty()
            && self.application_id.len() <= 128
            && self
                .application_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "frontend_management", rename_all = "snake_case")]
pub enum ForgeSupport {
    Unsupported,
    /// The initial support check admits only the facade envelope. Concrete
    /// component inventory is read separately through its supported contract.
    Declared {
        host: ForgeHost,
    },
}

struct LoginForm {
    cookie_name: String,
    nonce: String,
}
#[derive(Clone)]
struct ForgeClient {
    origin: Url,
    literal_origin: String,
    http: Client,
    session: Option<HeaderValue>,
}
impl ForgeClient {
    fn new(value: &str) -> Result<Self, ForgeError> {
        // Validate the literal spelling before URL normalization. DNS, alternate
        // IPv4 spellings and paths cannot broaden the selected loopback target.
        let port = value
            .strip_prefix("http://127.0.0.1:")
            .ok_or(ForgeError::InvalidOrigin)?;
        let parsed = port.parse::<u16>().map_err(|_| ForgeError::InvalidOrigin)?;
        if parsed == 0 || parsed.to_string() != port {
            return Err(ForgeError::InvalidOrigin);
        }
        let origin = Url::parse(value).map_err(|_| ForgeError::InvalidOrigin)?;
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| ForgeError::Unconfirmed)?;
        Ok(Self {
            origin,
            literal_origin: value.into(),
            http,
            session: None,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, self.origin.join(path).unwrap())
            .header(HOST, self.literal_origin.strip_prefix("http://").unwrap())
    }

    async fn login_form(&self) -> Result<LoginForm, ForgeError> {
        let response = self
            .request(reqwest::Method::GET, "/login")
            .send()
            .await
            .map_err(|_| ForgeError::Unconfirmed)?;
        if response.status().as_u16() != 200
            || !content_type(&response, "text/html")
            || !no_store(&response)
        {
            return Err(ForgeError::UnexpectedResponse);
        }
        let cookies = cookies(&response)?;
        if cookies.len() != 1 {
            return Err(ForgeError::UnexpectedResponse);
        }
        let cookie = &cookies[0];
        let prefix = cookie
            .name()
            .strip_prefix("mobkit_console_")
            .and_then(|s| s.strip_suffix("_login"));
        if !prefix.is_some_and(|s| {
            s.len() == 16
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) || !private_cookie(cookie, 300)
            || !opaque(cookie.value())
        {
            return Err(ForgeError::UnexpectedResponse);
        }
        let form = LoginForm {
            cookie_name: cookie.name().into(),
            nonce: cookie.value().into(),
        };
        let body = bounded(response, LOGIN_LIMIT).await?;
        let html = std::str::from_utf8(&body).map_err(|_| ForgeError::UnexpectedResponse)?;
        // Read only the exact frozen owner's form fields, never execute HTML,
        // infer another action or follow a page-supplied destination.
        let nonce = format!(
            "<input type=\"hidden\" name=\"nonce\" value=\"{}\">",
            form.nonce
        );
        if html.matches("<form ").count() != 1
            || html.matches("</form>").count() != 1
            || html
                .matches("<form method=\"post\" action=\"/login\">")
                .count()
                != 1
            || html.matches("name=\"nonce\"").count() != 1
            || html.matches(&nonce).count() != 1
            || html.matches("name=\"password\"").count() != 1
            || !html.contains("type=\"password\" name=\"password\"")
        {
            return Err(ForgeError::UnexpectedResponse);
        }
        Ok(form)
    }

    async fn login(&mut self, form: LoginForm, password: ForgePassword) -> Result<(), ForgeError> {
        let mut cookie = HeaderValue::from_str(&format!("{}={}", form.cookie_name, form.nonce))
            .map_err(|_| ForgeError::UnexpectedResponse)?;
        cookie.set_sensitive(true);
        let body = url_form(&form.nonce, &password.0);
        let response = self
            .request(reqwest::Method::POST, "/login")
            .header(ORIGIN, &self.literal_origin)
            .header(COOKIE, cookie)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|_| ForgeError::Unconfirmed)?;
        if matches!(response.status().as_u16(), 401 | 403 | 429) {
            return Err(ForgeError::SignInRefused);
        }
        if response.status().as_u16() != 303
            || response.headers().get_all(LOCATION).iter().count() != 1
            || response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                != Some("/console")
            || !no_store(&response)
        {
            return Err(ForgeError::UnexpectedResponse);
        }
        let cookies = cookies(&response)?;
        let session_name = form.cookie_name.strip_suffix("_login").unwrap();
        if cookies.len() != 2 {
            return Err(ForgeError::UnexpectedResponse);
        }
        let session: Vec<_> = cookies
            .iter()
            .filter(|c| c.name() == session_name)
            .collect();
        let removed: Vec<_> = cookies
            .iter()
            .filter(|c| c.name() == form.cookie_name)
            .collect();
        if session.len() != 1
            || removed.len() != 1
            || !private_cookie(session[0], 8 * 60 * 60)
            || !opaque(session[0].value())
            || !removed[0].value_trimmed().is_empty()
            || removed[0].max_age().map(|d| d.whole_seconds()) != Some(0)
            || removed[0].path() != Some("/")
            || removed[0].domain().is_some()
        {
            return Err(ForgeError::UnexpectedResponse);
        }
        let mut header = HeaderValue::from_str(&format!("{}={}", session_name, session[0].value()))
            .map_err(|_| ForgeError::UnexpectedResponse)?;
        header.set_sensitive(true);
        // A complete bounded response precedes the in-memory session capture.
        bounded(response, LOGIN_LIMIT).await?;
        self.session = Some(header);
        Ok(())
    }

    async fn support(&self) -> Result<ForgeSupport, ForgeError> {
        let response = self
            .request(reqwest::Method::GET, SUPPORT_PATH)
            .header(
                COOKIE,
                self.session
                    .as_ref()
                    .ok_or(ForgeError::SessionRequired)?
                    .clone(),
            )
            .send()
            .await
            .map_err(|_| ForgeError::Unconfirmed)?;
        let status = response.status().as_u16();
        if !content_type(&response, "application/json")
            || (status != 401 && !no_store(&response))
            || response.headers().contains_key(SET_COOKIE)
        {
            return Err(ForgeError::UnexpectedResponse);
        }
        let bytes = bounded(response, LIMIT).await?;
        serde_json::from_slice::<UniqueJson>(&bytes).map_err(|_| ForgeError::UnexpectedResponse)?;
        if status == 200 {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Envelope {
                protocol: u8,
                host: ForgeHost,
                observation: serde_json::Map<String, serde_json::Value>,
            }
            let value: Envelope =
                serde_json::from_slice(&bytes).map_err(|_| ForgeError::UnexpectedResponse)?;
            if value.protocol != 1 || !value.host.valid() {
                return Err(ForgeError::UnexpectedResponse);
            }
            // Do not project arbitrary component fields or infer readiness.
            drop(value.observation);
            return Ok(ForgeSupport::Declared { host: value.host });
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Detail {
            detail: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LoginRequired {
            error: String,
        }
        if status == 401 {
            let value: LoginRequired =
                serde_json::from_slice(&bytes).map_err(|_| ForgeError::UnexpectedResponse)?;
            return if value.error == "Console login required." {
                Err(ForgeError::SessionRequired)
            } else {
                Err(ForgeError::UnexpectedResponse)
            };
        }
        let value: Detail =
            serde_json::from_slice(&bytes).map_err(|_| ForgeError::UnexpectedResponse)?;
        match (status, value.detail.as_str()) {
            (501, UNSUPPORTED) => Ok(ForgeSupport::Unsupported),
            (409, "The loaded application source changed. No frontend command was admitted.") => {
                Err(ForgeError::SourceChanged)
            }
            (503, "The application frontend owner is unavailable. No activation is claimed.") => {
                Err(ForgeError::Unavailable)
            }
            _ => Err(ForgeError::UnexpectedResponse),
        }
    }
}

fn opaque(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn private_cookie(value: &cookie::Cookie<'_>, age: i64) -> bool {
    value.secure() != Some(true)
        && value.http_only() == Some(true)
        && value.same_site() == Some(SameSite::Strict)
        && value.path() == Some("/")
        && value.domain().is_none()
        && value.max_age().map(|d| d.whole_seconds()) == Some(age)
}
fn cookies(response: &Response) -> Result<Vec<cookie::Cookie<'static>>, ForgeError> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .take(4)
        .map(|header| {
            if header.as_bytes().len() > 4096 {
                return Err(ForgeError::UnexpectedResponse);
            }
            cookie::Cookie::parse(
                header
                    .to_str()
                    .map_err(|_| ForgeError::UnexpectedResponse)?
                    .to_owned(),
            )
            .map_err(|_| ForgeError::UnexpectedResponse)
        })
        .collect()
}
fn no_store(response: &Response) -> bool {
    response.headers().get_all(CACHE_CONTROL).iter().count() == 1
        && response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            == Some("no-store")
}
fn content_type(response: &Response, expected: &str) -> bool {
    response.headers().get_all(CONTENT_TYPE).iter().count() == 1
        && response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.split(';').next() == Some(expected))
}
async fn bounded(mut response: Response, limit: usize) -> Result<Vec<u8>, ForgeError> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(ForgeError::UnexpectedResponse);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ForgeError::Unconfirmed)?
    {
        if body.len() + chunk.len() > limit {
            return Err(ForgeError::UnexpectedResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
fn url_form(nonce: &str, password: &str) -> String {
    // Url's standard form serializer preserves Unicode and every password byte.
    let mut url = Url::parse("http://127.0.0.1/").unwrap();
    url.query_pairs_mut()
        .append_pair("nonce", nonce)
        .append_pair("password", password);
    url.query().unwrap().to_owned()
}

// Validate decoded key uniqueness at every depth before typed envelope parsing.
pub(crate) struct UniqueJson;
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("ordinary JSON with unique keys")
            }
            fn visit_bool<E>(self, _: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_i64<E>(self, _: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_u64<E>(self, _: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_f64<E>(self, _: f64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_str<E>(self, _: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_unit<E>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<UniqueJson, A::Error> {
                while access.next_element::<UniqueJson>()?.is_some() {}
                Ok(UniqueJson)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut keys = std::collections::BTreeSet::new();
                while let Some(key) = access.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    access.next_value::<UniqueJson>()?;
                }
                Ok(UniqueJson)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Clone)]
struct Captured {
    client: ForgeClient,
    support: ForgeSupport,
}
enum State {
    Fresh { native_setup_claimed: bool },
    Pending(Arc<()>),
    Captured { _captured: Captured },
    Retired,
}
struct Binding {
    state: Mutex<State>,
    prompts: mpsc::Sender<ForgePrompt>,
    configuration: Mutex<configuration::ConfigurationState>,
    enabled: std::sync::atomic::AtomicBool,
    recipe: configuration::Recipe,
    continued_client: Mutex<Option<ForgeClient>>,
}

/// A separate host protocol sibling in the existing native tool composition.
#[derive(Clone)]
pub struct ForgeFrontend {
    binding: Arc<Binding>,
}
impl ForgeFrontend {
    /// Existing named Reading recipe, including its narrower configuration checks.
    pub fn new() -> (Self, ForgePresenter) {
        Self::with_recipe(configuration::Recipe::Reading)
    }

    /// Select the generic Forge contract from trusted application composition.
    /// `private_fields` must be the declarations for this exact loaded host;
    /// model arguments and saved change locators cannot select these values.
    /// The protected presenter still selects the origin and authenticates the
    /// human through the existing Console session. This creates no app owner.
    pub fn for_application(
        expected: ForgeHost,
        private_fields: std::collections::BTreeSet<String>,
    ) -> Result<(Self, ForgePresenter), ForgeError> {
        if !expected.valid() {
            return Err(ForgeError::UnexpectedResponse);
        }
        Ok(Self::with_recipe(configuration::Recipe::Application {
            expected,
            private_fields,
        }))
    }

    fn with_recipe(recipe: configuration::Recipe) -> (Self, ForgePresenter) {
        let (send, receive) = mpsc::channel(1);
        let binding = Arc::new(Binding {
            state: Mutex::new(State::Fresh {
                native_setup_claimed: false,
            }),
            prompts: send,
            configuration: Mutex::new(configuration::ConfigurationState::default()),
            enabled: std::sync::atomic::AtomicBool::new(true),
            recipe,
            continued_client: Mutex::new(None),
        });
        (
            Self {
                binding: binding.clone(),
            },
            ForgePresenter {
                prompts: receive,
                binding,
            },
        )
    }
    /// A tuple can be assembled from different constructors. Preserve the
    /// existing shared binding when handing its sole receiver to a setup.
    pub(crate) fn check_presenter(&self, presenter: &ForgePresenter) -> Result<(), ConnectError> {
        if !Arc::ptr_eq(&self.binding, &presenter.binding) {
            return Err(ConnectError::InvalidInput(
                "The Forge frontend and protected presenter belong to different bindings".into(),
            ));
        }
        Ok(())
    }

    /// Claim fresh custody once for a native setup. The same state mutex also
    /// guards selection, so a concurrent selection cannot cross this check.
    /// A failed or unused setup never returns this binding to unclaimed Fresh.
    pub(crate) fn claim_native_setup(
        &self,
        presenter: &ForgePresenter,
    ) -> Result<(), ConnectError> {
        self.check_presenter(presenter)?;
        let unavailable = || {
            ConnectError::InvalidInput(
                "Native Forge setup requires a fresh binding that has never been handed off".into(),
            )
        };
        let mut state = self.binding.state.lock().map_err(|_| unavailable())?;
        match &mut *state {
            State::Fresh {
                native_setup_claimed,
            } if !*native_setup_claimed => {
                *native_setup_claimed = true;
                Ok(())
            }
            _ => Err(unavailable()),
        }
    }

    pub fn dispatcher(&self) -> Arc<dyn AgentToolDispatcher> {
        Arc::new(self.clone())
    }
}
impl Binding {
    fn retire(&self, token: &Arc<()>) {
        if let Ok(mut state) = self.state.lock()
            && matches!(&*state, State::Pending(active) if Arc::ptr_eq(active, token))
        {
            *state = State::Retired;
        }
    }
}
struct Lease {
    binding: Arc<Binding>,
    token: Arc<()>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.binding.retire(&self.token);
    }
}

#[async_trait]
impl AgentToolDispatcher for ForgeFrontend {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        if !self.available() {
            return Vec::new().into();
        }
        let mut tools = vec![Arc::new(ToolDef::new(
            FORGE_SELECT_HOST,
            "Ask the protected setup controls in this conversation to select one local Forge application's Console and sign in once, then read its frontend-management support. Supply only {}. Never supply or request an origin, port, password, nonce or cookie in chat. The person selects the literal loopback Console origin and enters its password privately. Sign-in creates an in-memory host session; it is not PairedHost pairing or a read-only operation. Unsupported means that application has no frontend producer, with no host identity or routing inferred. Declared support is not configuration, permission to submit, activation or readiness. Use the offered concrete inspection tools next. A saved change selected privately is returned with a recovery-only locator; read its status without preparing or submitting again. Never repeat this selection automatically or switch host. Explicit same-host reauthentication is a separate protected tool after session expiry.",
            serde_json::json!({"type":"object","additionalProperties":false,"properties":{}}),
        ))];
        tools.extend(configuration::tools(&self.binding.recipe));
        tools.into()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if matches!(
            name,
            FORGE_SELECT_HOST | FORGE_REAUTHENTICATE | FORGE_PREPARE | FORGE_SUBMIT
        ) {
            ToolMutationClass::Mutating
        } else if matches!(name, FORGE_INSPECT | FORGE_STATUS) {
            ToolMutationClass::ReadOnly
        } else {
            ToolMutationClass::Unknown
        }
    }
    fn live_bridge_effect_kind(&self, name: &str) -> meerkat_core::LiveBridgeEffectKind {
        if name == FORGE_SUBMIT {
            meerkat_core::LiveBridgeEffectKind::ExternalIo
        } else {
            meerkat_core::LiveBridgeEffectKind::ToolDispatch
        }
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !self.available() {
            return Err(ToolError::access_denied(call.name));
        }
        if call.name != FORGE_SELECT_HOST {
            return self.dispatch_configuration(call).await;
        }
        self.select(call, None).await
    }
}

impl ForgeFrontend {
    async fn select(
        &self,
        call: ToolCallView<'_>,
        prior: Option<Captured>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.args.get().len() > 1024
            || !call
                .parse_args::<serde_json::Map<String, serde_json::Value>>()
                .is_ok_and(|v| v.is_empty())
        {
            return Err(ToolError::execution_failed(
                "Forge host selection takes only {}; host and password entry belong to the protected setup controls",
            ));
        }
        let token = Arc::new(());
        {
            let mut state = self
                .binding
                .state
                .lock()
                .map_err(|_| ToolError::access_denied(call.name))?;
            if !(matches!(*state, State::Fresh { .. }) && prior.is_none()
                || matches!(*state, State::Captured { .. }) && prior.is_some())
            {
                return Err(ToolError::access_denied(call.name));
            }
            *state = State::Pending(token.clone());
        }
        let _lease = Lease {
            binding: self.binding.clone(),
            token: token.clone(),
        };
        let (send, receive) = oneshot::channel();
        self.binding
            .prompts
            .try_send(ForgePrompt::Selection(ForgeSelection {
                binding: self.binding.clone(),
                token: token.clone(),
                response: Some(send),
                prior,
            }))
            .map_err(|_| {
                ToolError::execution_failed(
                    "Protected Forge sign-in is unavailable; do not retry automatically",
                )
            })?;
        let (selected, acknowledge) = receive.await.map_err(|_| ToolError::execution_failed("Protected Forge sign-in ended without a confirmed handoff; do not retry automatically"))?;
        let result = match &selected {
            Ok(captured) => {
                let mut observation = serde_json::to_value(&captured.support).map_err(|_| {
                    ToolError::execution_failed("Forge support could not be represented")
                })?;
                if let Some(recovery) = self.recovery_selection() {
                    observation["recovery"] = recovery;
                }
                let text = serde_json::to_string(&observation).map_err(|_| {
                    ToolError::execution_failed("Forge support could not be represented")
                })?;
                Ok(ToolResult::new(call.id.into(), text, false).into())
            }
            Err(error) => Err(ToolError::execution_failed(error.to_string())),
        };
        {
            let mut state = self
                .binding
                .state
                .lock()
                .map_err(|_| ToolError::access_denied(call.name))?;
            if !matches!(&*state, State::Pending(active) if Arc::ptr_eq(active, &token)) {
                return Err(ToolError::access_denied(call.name));
            }
            *state = match selected {
                Ok(captured) => State::Captured {
                    _captured: captured,
                },
                Err(_) => State::Retired,
            };
        }
        let _ = acknowledge.send(());
        result
    }
}

/// The sole protected receiver. No cookie, nonce, origin or password is serialized.
pub struct ForgePresenter {
    prompts: mpsc::Receiver<ForgePrompt>,
    binding: Arc<Binding>,
}
impl ForgePresenter {
    pub async fn next(&mut self) -> Option<ForgePrompt> {
        self.prompts.recv().await
    }
}
pub enum ForgePrompt {
    #[doc(hidden)]
    Selection(ForgeSelection),
    #[doc(hidden)]
    Operation(configuration::ForgeOperation),
}
#[doc(hidden)]
pub struct ForgeSelection {
    binding: Arc<Binding>,
    token: Arc<()>,
    response: Option<oneshot::Sender<(Result<Captured, ForgeError>, oneshot::Sender<()>)>>,
    prior: Option<Captured>,
}
impl ForgeSelection {
    async fn closed(&mut self) {
        if let Some(response) = &mut self.response {
            response.closed().await;
        }
        self.binding.retire(&self.token);
    }
    async fn handoff(&mut self, captured: Captured) -> Result<(), ConnectError> {
        self.reply(Ok(captured)).await
    }
    async fn terminal_reply<F: Future<Output = io::Result<()>>>(
        &mut self,
        selected: Result<Captured, ForgeError>,
        interrupted: std::pin::Pin<&mut F>,
    ) -> Result<bool, ConnectError> {
        // The caller acknowledgment may suspend. Keep the original terminal
        // interrupt arbitration through that handoff, for success and refusal.
        tokio::select! { biased;
            signal = interrupted => {
                signal.map_err(|e| ConnectError::native("interrupt Forge handoff", e))?;
                Ok(false)
            },
            result = self.reply(selected) => result.map(|()| true),
        }
    }
    async fn reply(&mut self, selected: Result<Captured, ForgeError>) -> Result<(), ConnectError> {
        let (send, receive) = oneshot::channel();
        self.response
            .take()
            .ok_or_else(ended)?
            .send((selected, send))
            .map_err(|_| ended())?;
        receive.await.map_err(|_| ended())
    }
}
impl Drop for ForgeSelection {
    fn drop(&mut self) {
        self.binding.retire(&self.token);
    }
}
fn ended() -> ConnectError {
    ConnectError::InvalidInput("The protected Forge interaction ended; no late binding or automatic sign-in retry is allowed".into())
}
fn protocol(error: ForgeError) -> ConnectError {
    ConnectError::native("check Forge Console", error)
}

/// Uses the existing exclusive terminal owner. The caller cancels and joins the
/// exact native input on false/error. A polled login POST is always joined.
pub async fn present_forge_selection(
    prompt: ForgePrompt,
    input: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<bool, ConnectError> {
    match prompt {
        ForgePrompt::Selection(prompt) => {
            present_selection(prompt, input, output, interrupted).await
        }
        ForgePrompt::Operation(prompt) => {
            configuration::present_operation(prompt, output, interrupted).await
        }
    }
}

async fn present_selection(
    mut prompt: ForgeSelection,
    input: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<bool, ConnectError> {
    input
        .require_terminal()
        .map_err(|e| ConnectError::native("check protected Forge input", e))?;
    tokio::pin!(interrupted);
    writeln!(output, "Select the running Forge application's local Console. This checks frontend support only. The realm keeps this setup conversation. /quit ends setup.")
        .map_err(|e| ConnectError::native("write Forge selection", e))?;
    let binding = prompt.binding.clone();
    let prior = prompt.prior.clone();
    let deferred = binding.recipe.deferred();
    let retained = if deferred {
        binding.continued_client.lock().map_err(|_| ended())?.take()
    } else {
        None
    };
    let mut client = if let Some(client) = retained {
        client
    } else {
        let selected = if deferred {
            prior
                .as_ref()
                .map(|p| p.client.literal_origin.clone())
                .or_else(|| binding.recipe.selected_origin().map(str::to_owned))
        } else {
            let selected = tokio::select! { biased;
                _ = prompt.closed() => return Err(ended()),
                selected = configuration::select_recovery(&binding, prior.as_ref(), input, output, &mut interrupted) => selected?,
            };
            let std::ops::ControlFlow::Continue(selected) = selected else {
                return Ok(false);
            };
            selected
        };
        if let Some(origin) = selected {
            ForgeClient::new(&origin).map_err(protocol)?
        } else {
            loop {
                write!(output, "Forge Console origin (http://127.0.0.1:PORT): ")
                    .and_then(|_| output.flush())
                    .map_err(|e| ConnectError::native("write Forge selection", e))?;
                let line = tokio::select! { biased;
                    _ = prompt.closed() => return Err(ended()),
                    signal = &mut interrupted => { signal.map_err(|e| ConnectError::native("interrupt Forge selection", e))?; return Ok(false); },
                    value = input.recv() => value.transpose().map_err(|e| ConnectError::native("read Forge origin", e))?,
                };
                input
                    .check_cleanup()
                    .map_err(|e| ConnectError::native("restore Forge input", e))?;
                match line.as_deref() {
                    None | Some("/quit") => return Ok(false),
                    Some(value) => match ForgeClient::new(value) {
                        Ok(client) => break client,
                        Err(error) => writeln!(output, "{error}")
                            .map_err(|e| ConnectError::native("write Forge selection", e))?,
                    },
                }
            }
        }
    };
    // A moved, still-valid Console session needs only a fresh descriptor read.
    // A 401 requests the existing same-origin login once, never an automatic
    // password retry. No cookie leaves this private client.
    let mut observed = if deferred && client.session.is_some() {
        Some(tokio::select! { biased;
            _ = prompt.closed() => return Err(ended()),
            signal = &mut interrupted => { signal.map_err(|e| ConnectError::native("interrupt Forge descriptor", e))?; return Ok(false); },
            result = client.descriptor() => result,
        })
    } else {
        None
    };
    if observed.is_none() || matches!(&observed, Some(Err(ForgeError::SessionRequired))) {
        match login_selection(
            &mut prompt,
            &mut client,
            input,
            output,
            interrupted.as_mut(),
        )
        .await?
        {
            LoginEnd::Ready => (),
            LoginEnd::Stopped => return Ok(false),
            LoginEnd::Refused(error) => {
                return refuse_selection(
                    &mut prompt,
                    client,
                    error,
                    false,
                    output,
                    interrupted.as_mut(),
                )
                .await;
            }
        }
        observed = None;
    }
    let support = if deferred {
        let observed = match observed {
            Some(result) => result,
            None => tokio::select! { biased;
                _ = prompt.closed() => return Err(ended()),
                signal = &mut interrupted => { signal.map_err(|e| ConnectError::native("interrupt Forge descriptor", e))?; return Ok(false); },
                result = client.descriptor() => result,
            },
        };
        let descriptor = match observed {
            Ok(descriptor) => descriptor,
            Err(error) => {
                return refuse_selection(
                    &mut prompt,
                    client,
                    error,
                    true,
                    output,
                    interrupted.as_mut(),
                )
                .await;
            }
        };
        let host = descriptor.host.clone();
        writeln!(
            output,
            "Application: {}\nHost identity: {}\nLoaded source: {}",
            crate::terminal::display_text(&host.application_id),
            host.host_id,
            host.source_digest
        )
        .map_err(|e| ConnectError::native("write Forge application identity", e))?;
        if !binding.bind_descriptor(&prompt.token, &client, descriptor)? {
            return refuse_selection(
                &mut prompt,
                client,
                ForgeError::SourceChanged,
                true,
                output,
                interrupted.as_mut(),
            )
            .await;
        }
        // Generic recovery is inspected only after authenticated declarations
        // exist. The selected origin cannot be changed by a saved locator.
        let recovery = tokio::select! { biased;
            _ = prompt.closed() => return Err(ended()),
            selected = configuration::select_recovery(&binding, prior.as_ref(), input, output, &mut interrupted) => selected?,
        };
        let std::ops::ControlFlow::Continue(recovery) = recovery else {
            return Ok(false);
        };
        if recovery
            .as_deref()
            .is_some_and(|origin| origin != client.literal_origin)
        {
            return Err(ended());
        }
        ForgeSupport::Declared { host }
    } else {
        let support = tokio::select! { biased;
            _ = prompt.closed() => return Err(ended()),
            signal = &mut interrupted => { signal.map_err(|e| ConnectError::native("interrupt Forge support check", e))?; return Ok(false); },
            result = client.support() => result,
        };
        match support {
            Ok(support) => support,
            Err(error) => {
                return refuse_selection(
                    &mut prompt,
                    client,
                    error,
                    false,
                    output,
                    interrupted.as_mut(),
                )
                .await;
            }
        }
    };
    if let Err(error) = configuration::selected(&binding, prompt.prior.as_ref(), &client, &support)
    {
        return refuse_selection(
            &mut prompt,
            client,
            error,
            false,
            output,
            interrupted.as_mut(),
        )
        .await;
    }
    let notice = match &support {
        ForgeSupport::Unsupported => {
            "Forge sign-in succeeded. This application has not declared frontend management support; no host identity, routing or readiness is inferred."
        }
        ForgeSupport::Declared { .. } => {
            "Forge reports frontend management support. Component configuration, activation and readiness have not been checked."
        }
    };
    writeln!(output, "{notice}")
        .and_then(|_| output.flush())
        .map_err(|e| ConnectError::native("write Forge support", e))?;
    if let ForgeSupport::Declared { host } = &support {
        writeln!(
            output,
            "Selected application: {}",
            crate::terminal::display_text(&host.application_id)
        )
        .map_err(|e| ConnectError::native("write Forge application", e))?;
    }
    if let Some(signal) = interrupted.as_mut().now_or_never() {
        signal.map_err(|e| ConnectError::native("interrupt Forge handoff", e))?;
        return Ok(false);
    }
    prompt
        .terminal_reply(Ok(Captured { client, support }), interrupted.as_mut())
        .await
}

async fn refuse_selection<F: Future<Output = io::Result<()>>>(
    prompt: &mut ForgeSelection,
    client: ForgeClient,
    error: ForgeError,
    descriptor_read: bool,
    output: &mut impl Write,
    mut interrupted: std::pin::Pin<&mut F>,
) -> Result<bool, ConnectError> {
    // Descriptor GET failures are safe local refusals, including old wrappers
    // and malformed data. Unsafe login/transport checks keep their fatal path.
    if !descriptor_read
        && !matches!(
            error,
            ForgeError::SignInRefused
                | ForgeError::SessionRequired
                | ForgeError::SourceChanged
                | ForgeError::Unavailable
                | ForgeError::DeclarationUnsupported
        )
    {
        return Err(protocol(error));
    }
    if prompt
        .response
        .as_ref()
        .is_none_or(oneshot::Sender::is_closed)
    {
        return Err(ended());
    }
    if let Some(signal) = interrupted.as_mut().now_or_never() {
        signal.map_err(|e| ConnectError::native("interrupt Forge refusal", e))?;
        return Ok(false);
    }
    // Retain only the actual private client for same-conversation continuation.
    // Failed selection grants no configuration authority and never retries.
    if prompt.binding.recipe.deferred() {
        let state = prompt.binding.state.lock().map_err(|_| ended())?;
        if !matches!(&*state, State::Pending(active) if Arc::ptr_eq(active, &prompt.token)) {
            return Err(ended());
        }
        *prompt
            .binding
            .continued_client
            .lock()
            .map_err(|_| ended())? = Some(client);
    }
    configuration::refuse_selection(&prompt.binding)?;
    writeln!(output, "Forge selection was refused: {error} The setup conversation can continue; no frontend command was sent.")
        .and_then(|_| output.flush()).map_err(|e| ConnectError::native("write Forge refusal", e))?;
    prompt
        .terminal_reply(Err(error), interrupted.as_mut())
        .await
}

enum LoginEnd {
    Ready,
    Stopped,
    Refused(ForgeError),
}

async fn login_selection<F: Future<Output = io::Result<()>>>(
    prompt: &mut ForgeSelection,
    client: &mut ForgeClient,
    input: &mut TerminalInput,
    output: &mut impl Write,
    mut interrupted: std::pin::Pin<&mut F>,
) -> Result<LoginEnd, ConnectError> {
    let form = tokio::select! {
        biased;
        _ = prompt.closed() => return Err(ended()),
        signal = &mut interrupted => { signal.map_err(|e| ConnectError::native("interrupt Forge login", e))?; return Ok(LoginEnd::Stopped); },
        result = client.login_form() => result.map_err(protocol)?,
    };
    writeln!(output, "Use this application's Console password. Sign-in is kept only in memory; no OS credential store or PairedHost pairing is used.")
        .map_err(|e| ConnectError::native("write Forge login", e))?;
    let read = tokio::select! {
        biased;
        _ = prompt.closed() => Err(ended()),
        signal = &mut interrupted => signal.map(|_| None).map_err(|e| ConnectError::native("interrupt Forge password", e)),
        result = input.read_forge_password(output) => result.map_err(|e| ConnectError::native("read Forge password", e)),
    };
    input
        .check_cleanup()
        .map_err(|e| ConnectError::native("restore Forge password input", e))?;
    let Some(password) = read? else {
        return Ok(LoginEnd::Stopped);
    };
    if prompt
        .response
        .as_ref()
        .is_none_or(oneshot::Sender::is_closed)
    {
        return Err(ended());
    }
    if let Some(signal) = interrupted.as_mut().now_or_never() {
        signal.map_err(|e| ConnectError::native("interrupt Forge login", e))?;
        return Ok(LoginEnd::Stopped);
    }
    let (result, cancelled) = {
        let login = client.login(form, password);
        tokio::pin!(login);
        let mut cancelled = None;
        let result = if let Some(result) = login.as_mut().now_or_never() {
            result
        } else {
            tokio::select! {
                biased;
                _ = prompt.closed() => { cancelled = Some(Err(ended())); login.as_mut().await },
                signal = &mut interrupted => {
                    prompt.binding.retire(&prompt.token);
                    let printed = writeln!(output, "Cancellation requested. Waiting for the Forge sign-in request to settle.")
                        .and_then(|_| output.flush()).map_err(|e| ConnectError::native("write Forge cancellation", e));
                    cancelled = Some(signal.map_err(|e| ConnectError::native("interrupt Forge login", e)).and(printed));
                    // Do not abandon a polled request that may have created a host session.
                    login.as_mut().await
                },
                result = login.as_mut() => result,
            }
        };
        (result, cancelled)
    };
    if let Some(cancelled) = cancelled {
        cancelled?;
        result.map_err(protocol)?;
        return Ok(LoginEnd::Stopped);
    }
    match result {
        Ok(()) => Ok(LoginEnd::Ready),
        Err(ForgeError::SignInRefused) => Ok(LoginEnd::Refused(ForgeError::SignInRefused)),
        Err(error) => Err(protocol(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_setup_presenter_must_share_the_existing_forge_binding() {
        let (reading, reading_presenter) = ForgeFrontend::new();
        let (application, application_presenter) = ForgeFrontend::for_application(
            ForgeHost {
                protocol: 1,
                host_id: format!("forge:{}", "a".repeat(64)),
                application_id: "native-application".into(),
                source_digest: "b".repeat(64),
            },
            ["PRIVATE_TOKEN".to_owned()].into(),
        )
        .unwrap();
        assert!(reading.check_presenter(&reading_presenter).is_ok());
        assert!(application.check_presenter(&application_presenter).is_ok());
        assert!(
            application
                .clone()
                .check_presenter(&application_presenter)
                .is_ok()
        );
        assert!(reading.check_presenter(&application_presenter).is_err());
        assert!(application.check_presenter(&reading_presenter).is_err());
        // Equal constructor facts still create different prompt owners.
        let (other, other_presenter) = ForgeFrontend::for_application(
            ForgeHost {
                protocol: 1,
                host_id: format!("forge:{}", "a".repeat(64)),
                application_id: "native-application".into(),
                source_digest: "b".repeat(64),
            },
            ["PRIVATE_TOKEN".to_owned()].into(),
        )
        .unwrap();
        assert!(other.check_presenter(&other_presenter).is_ok());
        assert!(application.check_presenter(&other_presenter).is_err());
        assert!(other.check_presenter(&application_presenter).is_err());
    }
    #[test]
    fn native_setup_claim_accepts_fresh_clones_once_and_refuses_crossed_pairs() {
        let application_pair = || {
            ForgeFrontend::for_application(
                ForgeHost {
                    protocol: 1,
                    host_id: format!("forge:{}", "a".repeat(64)),
                    application_id: "native-application".into(),
                    source_digest: "b".repeat(64),
                },
                ["PRIVATE_TOKEN".to_owned()].into(),
            )
            .unwrap()
        };
        let (frontend, presenter) = application_pair();
        let (other, other_presenter) = application_pair();
        // Identical trusted facts are still different owners. A crossed
        // rejection must not consume either fresh owner's real handoff.
        assert!(frontend.claim_native_setup(&other_presenter).is_err());
        assert!(other.claim_native_setup(&presenter).is_err());
        assert!(frontend.clone().claim_native_setup(&presenter).is_ok());
        assert!(other.claim_native_setup(&other_presenter).is_ok());
        // No selection has run. Even this unused, still-Fresh pair is spent.
        assert!(frontend.claim_native_setup(&presenter).is_err());
        assert!(other.clone().claim_native_setup(&other_presenter).is_err());
        assert!(matches!(
            *frontend.binding.state.lock().unwrap(),
            State::Fresh {
                native_setup_claimed: true
            }
        ));
        // Custody consumption does not change receiver identity.
        assert!(frontend.check_presenter(&presenter).is_ok());
    }

    #[test]
    fn native_setup_claim_serializes_competing_handoffs() {
        let (frontend, presenter) = ForgeFrontend::new();
        let start = std::sync::Barrier::new(3);
        let outcomes = std::thread::scope(|scope| {
            let left = scope.spawn(|| {
                start.wait();
                frontend.claim_native_setup(&presenter).is_ok()
            });
            let right = scope.spawn(|| {
                start.wait();
                frontend.clone().claim_native_setup(&presenter).is_ok()
            });
            start.wait();
            // Join both contenders before interpreting either result.
            (left.join(), right.join())
        });
        assert_ne!(outcomes.0.unwrap(), outcomes.1.unwrap());
        assert!(matches!(
            *frontend.binding.state.lock().unwrap(),
            State::Fresh {
                native_setup_claimed: true
            }
        ));
    }

    #[test]
    fn native_setup_claim_refuses_poisoned_selection_state() {
        let (frontend, presenter) = ForgeFrontend::new();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = frontend.binding.state.lock().unwrap();
            panic!("synthetic failure while holding the Forge selection state");
        }));
        assert!(poisoned.is_err() && frontend.binding.state.is_poisoned());
        assert!(frontend.claim_native_setup(&presenter).is_err());
        assert!(frontend.binding.state.is_poisoned());
    }

    #[tokio::test]
    async fn native_setup_claim_refuses_used_selection_without_reset() {
        for claimed_before_selection in [false, true] {
            for cancel in [false, true] {
                let (frontend, mut presenter) = ForgeFrontend::new();
                if claimed_before_selection {
                    frontend.claim_native_setup(&presenter).unwrap();
                }
                let dispatcher = frontend.dispatcher();
                let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
                let mut selection = Box::pin(dispatcher.dispatch(ToolCallView {
                    id: "forge-fresh-custody",
                    name: FORGE_SELECT_HOST,
                    args: &args,
                }));
                // Both ordinary direct Forge and the first native handoff
                // retain the existing selection/sole-presenter path.
                assert!(selection.as_mut().now_or_never().is_none());
                let ForgePrompt::Selection(mut prompt) = presenter
                    .next()
                    .now_or_never()
                    .flatten()
                    .expect("exact queued selection")
                else {
                    panic!("host selection reached another prompt kind")
                };
                assert!(frontend.claim_native_setup(&presenter).is_err());
                assert!(matches!(&*frontend.binding.state.lock().unwrap(),
                    State::Pending(token) if Arc::ptr_eq(token, &prompt.token)));
                if cancel {
                    drop(prompt);
                    assert!(matches!(selection.as_mut().now_or_never(), Some(Err(_))));
                    assert!(frontend.claim_native_setup(&presenter).is_err());
                    assert!(matches!(
                        *frontend.binding.state.lock().unwrap(),
                        State::Retired
                    ));
                } else {
                    // This synthetic captured value exercises the real local
                    // handoff and acknowledgement only. No request is made,
                    // and this is not authenticated or native-runtime evidence.
                    let mut client = ForgeClient::new("http://127.0.0.1:1").unwrap();
                    client.session = Some(HeaderValue::from_static("synthetic-prior-session"));
                    let mut handoff = Box::pin(prompt.handoff(Captured {
                        client,
                        support: ForgeSupport::Unsupported,
                    }));
                    assert!(handoff.as_mut().now_or_never().is_none());
                    assert!(matches!(selection.as_mut().now_or_never(), Some(Ok(_))));
                    assert!(matches!(handoff.as_mut().now_or_never(), Some(Ok(()))));
                    drop(handoff);
                    assert!(frontend.claim_native_setup(&presenter).is_err());
                    assert!(matches!(&*frontend.binding.state.lock().unwrap(),
                        State::Captured { _captured }
                            if _captured.client.literal_origin == "http://127.0.0.1:1"
                            && _captured.client.session.as_ref()
                                == Some(&HeaderValue::from_static("synthetic-prior-session"))));
                }
                // A refused handoff cannot reset the used pair into a fresh
                // selection, whether the first selection completed or retired.
                assert!(matches!(
                    dispatcher
                        .dispatch(ToolCallView {
                            id: "forge-fresh-reuse",
                            name: FORGE_SELECT_HOST,
                            args: &args,
                        })
                        .now_or_never(),
                    Some(Err(_))
                ));
            }
        }
    }

    #[test]
    fn origin_is_literal_loopback_and_password_is_not_normalized() {
        for bad in [
            "http://localhost:80",
            "http://127.1:80",
            "http://2130706433:80",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://127.0.0.1:080",
            "http://127.0.0.1:80/",
            "http://127.0.0.1:80?x=1",
            "http://user@127.0.0.1:80",
            "https://127.0.0.1:443",
            " http://127.0.0.1:80",
            "http://[::1]:80",
        ] {
            assert!(matches!(
                ForgeClient::new(bad),
                Err(ForgeError::InvalidOrigin)
            ));
        }
        assert!(ForgeClient::new("http://127.0.0.1:65535").is_ok());
        let client = ForgeClient::new("http://127.0.0.1:80").unwrap();
        assert_eq!(client.literal_origin, "http://127.0.0.1:80");
        let request = client
            .request(reqwest::Method::GET, "/login")
            .build()
            .unwrap();
        assert_eq!(request.headers()[HOST], "127.0.0.1:80");
        let value = "  fixture 密码 \t spaces  ";
        let password = ForgePassword::new(value.into()).unwrap();
        let form = url_form(NONCE_FOR_TEST, &password.0);
        let url = Url::parse(&format!("http://127.0.0.1/?{form}")).unwrap();
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "password")
                .unwrap()
                .1,
            value
        );
        assert!(ForgePassword::new("短".repeat(4096)).is_ok());
        assert!(ForgePassword::new("短".repeat(4097)).is_err());
        assert!(!format!("{password:?}").contains(value));
    }
    const NONCE_FOR_TEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    #[test]
    fn duplicate_support_fields_are_refused_even_when_escaped_or_nested() {
        for body in [
            r#"{"protocol":1,"protocol":1}"#,
            r#"{"protocol":1,"\u0070rotocol":1}"#,
            r#"{"observation":{"x":1,"x":2}}"#,
        ] {
            assert!(serde_json::from_str::<UniqueJson>(body).is_err());
        }
    }
}
