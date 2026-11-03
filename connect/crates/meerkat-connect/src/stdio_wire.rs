//! Closed private-pipe wire values. No value here grants native authority.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

pub(super) const MAX_FRAME: usize = 64 * 1024 * 1024;
pub(super) const MAX_SAFE: u64 = 9_007_199_254_740_991;
pub(super) fn opaque(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
pub(super) fn request_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|v| {
        (1..=64).contains(&v.len())
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    })
}
pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(u64::MAX, |d| {
            u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
        })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Response {
    pub jsonrpc: String,
    pub id: String,
    pub result: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResponseError {
    pub jsonrpc: String,
    pub id: String,
    pub error: ErrorBody,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ErrorBody {
    pub code: i64,
    pub message: String,
}
#[derive(Clone)]
pub(super) struct Initialize {
    pub protocol_version: u8,
    pub application: Application,
    pub viewer: Viewer,
    pub realm: Realm,
    pub model: Option<String>,
    pub auth_binding: Option<Auth>,
    pub adapter: Adapter,
    pub paired_host: Option<PairedHost>,
    pub services: Option<ServiceResources>,
    pub connector_auth: Option<ConnectorProfiles>,
    pub forge: Option<ForgeApplication>,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Adapter {
    ForgeReading,
    ForgeApplication,
    PairedHost,
    ForgeServices,
    Services,
}
impl Adapter {
    pub(super) fn is_services(self) -> bool {
        matches!(self, Self::ForgeServices | Self::Services)
    }
}
/// Exact nonsecret selection from the existing trusted application owner.
/// Field declarations are policy inputs, never inferred from model or inventory.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ForgeApplication {
    pub host: crate::forge::ForgeHost,
    pub private_fields: std::collections::BTreeSet<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServiceResources {
    pub resource_manifest_sha256: String,
    pub preparation: ServicePermission,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ServicePermission {
    Permitted,
}

// Trusted private host configuration only. No Debug or Serialize projection:
// profile secrets are consumed into the native facade, never retained in state.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectorProfiles {
    pub profiles: Vec<ConnectorProfile>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectorProfile {
    id: String,
    issuer: String,
    client: String,
    #[serde(default, deserialize_with = "present")]
    client_secret: Option<String>,
    authorize_params: Vec<(String, String)>,
    resource_indicator: bool,
    #[serde(default, deserialize_with = "present")]
    protocol: Option<String>,
    registered_redirect_uris: Vec<String>,
}
impl ConnectorProfiles {
    pub fn into_native(self) -> Vec<meerkat::ConnectorClientAuthProfile> {
        self.profiles
            .into_iter()
            .map(|p| meerkat::ConnectorClientAuthProfile {
                id: p.id,
                issuer: p.issuer,
                client: p.client,
                // The trusted private wire carries inline configuration only.
                client_secret: p.client_secret.map(meerkat::ConnectorClientSecret::Inline),
                authorize_params: p.authorize_params,
                resource_indicator: p.resource_indicator,
                protocol: p.protocol,
                registered_redirect_uris: p.registered_redirect_uris,
            })
            .collect()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConnectorTarget {
    slot_id: String,
    issuer: String,
    client: String,
    resource: String,
    scopes: std::collections::BTreeSet<String>,
    strategy_id: String,
    account_selection: meerkat_auth_core::connector_oauth::AccountSelection,
    #[serde(default, deserialize_with = "present")]
    client_auth_profile: Option<String>,
}
impl ConnectorTarget {
    pub fn into_native(
        self,
        realm: meerkat_core::RealmId,
    ) -> Result<meerkat::ConnectorOAuthTarget, &'static str> {
        let account = meerkat_core::CredentialAccountId::parse(&self.slot_id)
            .map_err(|_| "invalid_binding")?;
        Ok(meerkat::ConnectorOAuthTarget {
            slot: meerkat_core::CredentialAccountRef { realm, account },
            issuer: self.issuer,
            client: self.client,
            resource: self.resource,
            scopes: self.scopes,
            strategy_id: self.strategy_id,
            account: self.account_selection,
            client_auth_profile: self.client_auth_profile,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectorStart {
    pub scope: String,
    pub job_id: String,
    pub connector: ConnectorTarget,
    pub redirect_uri: String,
    pub operations:
        std::collections::BTreeMap<String, meerkat_core::mcp_config::McpConnectorOperation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectorComplete {
    pub scope: String,
    pub state: String,
    pub code: String,
    #[serde(default, deserialize_with = "present")]
    pub host_attempt_handle: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectorCancel {
    pub scope: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PairedHost {
    pub targets: Vec<crate::paired_host_stdio::Target>,
    #[serde(default, deserialize_with = "present")]
    pub credential: Option<PairedHostCredential>,
}

// Only the trusted host writes this private initialization field. It is not a
// viewer identity, a browser answer or a credential storage instruction.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PairedHostCredential {
    pub header: String,
    pub value: String,
    pub expires_at_ms: u64,
}

// Keep the predecessor schema closed, including its unknown-field refusals.
// V2 is selected explicitly before decoding its distinct closed field set.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitializeV1 {
    protocol_version: u8,
    application: Application,
    viewer: Viewer,
    realm: Realm,
    #[serde(default, deserialize_with = "present")]
    model: Option<String>,
    #[serde(default, deserialize_with = "present")]
    auth_binding: Option<Auth>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitializeV2 {
    protocol_version: u8,
    adapter: Adapter,
    application: Application,
    viewer: Viewer,
    realm: Realm,
    #[serde(default, deserialize_with = "present")]
    model: Option<String>,
    #[serde(default, deserialize_with = "present")]
    auth_binding: Option<Auth>,
    #[serde(default, deserialize_with = "present")]
    paired_host: Option<PairedHost>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitializeV3 {
    protocol_version: u8,
    adapter: Adapter,
    application: Application,
    viewer: Viewer,
    realm: Realm,
    #[serde(default, deserialize_with = "present")]
    model: Option<String>,
    #[serde(default, deserialize_with = "present")]
    auth_binding: Option<Auth>,
    services: ServiceResources,
    #[serde(default, deserialize_with = "present")]
    connector_auth: Option<ConnectorProfiles>,
}
// A distinct closed mode: previous frontend/service schemas do not acquire
// generic application policy inputs or change their existing interpretation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitializeV4 {
    protocol_version: u8,
    adapter: Adapter,
    application: Application,
    viewer: Viewer,
    realm: Realm,
    #[serde(default, deserialize_with = "present")]
    model: Option<String>,
    #[serde(default, deserialize_with = "present")]
    auth_binding: Option<Auth>,
    forge: ForgeApplication,
}

impl<'de> Deserialize<'de> for Initialize {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if value["protocol_version"] == 4 {
            let v: InitializeV4 =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self {
                protocol_version: v.protocol_version,
                adapter: v.adapter,
                application: v.application,
                viewer: v.viewer,
                realm: v.realm,
                model: v.model,
                auth_binding: v.auth_binding,
                paired_host: None,
                services: None,
                connector_auth: None,
                forge: Some(v.forge),
            })
        } else if value["protocol_version"] == 3 {
            let v: InitializeV3 =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self {
                protocol_version: v.protocol_version,
                adapter: v.adapter,
                application: v.application,
                viewer: v.viewer,
                realm: v.realm,
                model: v.model,
                auth_binding: v.auth_binding,
                paired_host: None,
                services: Some(v.services),
                connector_auth: v.connector_auth,
                forge: None,
            })
        } else if value["protocol_version"] == 2 {
            let v: InitializeV2 =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self {
                protocol_version: v.protocol_version,
                adapter: v.adapter,
                application: v.application,
                viewer: v.viewer,
                realm: v.realm,
                model: v.model,
                auth_binding: v.auth_binding,
                paired_host: v.paired_host,
                services: None,
                connector_auth: None,
                forge: None,
            })
        } else {
            let v: InitializeV1 =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self {
                protocol_version: v.protocol_version,
                adapter: Adapter::ForgeReading,
                application: v.application,
                viewer: v.viewer,
                realm: v.realm,
                model: v.model,
                auth_binding: v.auth_binding,
                paired_host: None,
                services: None,
                connector_auth: None,
                forge: None,
            })
        }
    }
}
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Application {
    pub application_id: String,
    pub instance_id: String,
    pub origin: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Viewer {
    pub principal_id: String,
    pub login_scope_id: String,
    pub expires_at_ms: u64,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Realm {
    pub realm_id: String,
    pub state_root: String,
    pub context_root: String,
    pub user_config_root: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Auth {
    pub realm: String,
    pub binding: String,
    #[serde(default, deserialize_with = "present")]
    pub profile: Option<String>,
}
impl Initialize {
    pub fn valid(&self) -> bool {
        let port = self
            .application
            .origin
            .strip_prefix("http://127.0.0.1:")
            .and_then(|v| {
                v.parse::<u16>()
                    .ok()
                    .filter(|n| *n > 0 && n.to_string() == v)
            });
        let canonical = |origin: &str| {
            crate::paired_host::PairedHostClient::new(origin)
                .is_ok_and(|client| client.selected_origin() == origin)
        };
        let targets_valid = self.paired_host.as_ref().is_none_or(|paired_host| {
            let mut ids = std::collections::BTreeSet::new();
            let mut origins = std::collections::BTreeSet::new();
            (1..=8).contains(&paired_host.targets.len())
                && paired_host.targets.iter().all(|target| {
                    opaque(&target.target_id)
                        && ids.insert(&target.target_id)
                        && origins.insert(&target.origin)
                        && canonical(&target.origin)
                })
        });
        let composition_valid = match self.adapter {
            Adapter::ForgeApplication => {
                self.protocol_version == 4
                    && port.is_some()
                    && self.paired_host.is_none()
                    && self.services.is_none()
                    && self.connector_auth.is_none()
                    && self.forge.as_ref().is_some_and(|selection| {
                        selection
                            .host
                            .matches_application_id(&self.application.application_id)
                    })
            }
            Adapter::ForgeReading => {
                self.protocol_version <= 2
                    && port.is_some()
                    && self
                        .paired_host
                        .as_ref()
                        .is_none_or(|h| h.credential.is_none())
            }
            Adapter::PairedHost => {
                self.protocol_version == 2
                    && self.model.is_some()
                    && self.auth_binding.as_ref().is_some_and(|auth| {
                        opaque(&auth.realm)
                            && opaque(&auth.binding)
                            && auth.profile.as_ref().is_none_or(|profile| opaque(profile))
                    })
                    && self
                        .viewer
                        .principal_id
                        .strip_prefix("paired-device:")
                        .is_some_and(opaque)
                    && self.paired_host.as_ref().is_some_and(|paired_host| {
                        paired_host.targets.len() == 1
                            && paired_host.targets[0].origin == self.application.origin
                            && paired_host.credential.as_ref().is_some_and(|credential| {
                                credential.expires_at_ms == self.viewer.expires_at_ms
                                    && crate::paired_host::PairedHostClient::for_presenter(
                                        &self.application.origin,
                                        &credential.header,
                                        &credential.value,
                                    )
                                    .is_ok()
                            })
                    })
            }
            Adapter::ForgeServices | Adapter::Services => {
                self.protocol_version == 3
                    && (if self.adapter == Adapter::ForgeServices {
                        port.is_some()
                    } else {
                        canonical(&self.application.origin)
                    })
                    && self.paired_host.is_none()
                    && self.services.as_ref().is_some_and(|services| {
                        matches!(services.preparation, ServicePermission::Permitted)
                            && services.resource_manifest_sha256.len() == 64
                            && services
                                .resource_manifest_sha256
                                .bytes()
                                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                    })
            }
        };
        matches!(self.protocol_version, 1..=4)
            && targets_valid
            && composition_valid
            && opaque(&self.application.application_id)
            && opaque(&self.application.instance_id)
            && opaque(&self.viewer.login_scope_id)
            && self.viewer.expires_at_ms > now()
            && self.viewer.expires_at_ms <= MAX_SAFE
            && meerkat_core::ApprovalPrincipalId::new(&self.viewer.principal_id).is_ok()
            && meerkat_core::runtime_bootstrap::validate_explicit_realm_id(&self.realm.realm_id)
                .is_ok()
            && [
                &self.realm.state_root,
                &self.realm.context_root,
                &self.realm.user_config_root,
            ]
            .iter()
            .all(|p| !p.contains('\0') && Path::new(p).is_absolute())
            && self.model.as_ref().is_none_or(|m| opaque(m))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct History {
    pub scope: String,
    #[serde(default, deserialize_with = "present")]
    pub offset: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Send {
    pub scope: String,
    pub text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cancel {
    pub scope: String,
    pub send_request_id: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub(super) enum Target {
    Scope(TargetScope),
    Initialize(TargetInitialize),
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetScope {
    pub scope: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetInitialize {
    pub initialize_request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Retire {
    pub target: Target,
    pub retirement_id: String,
    pub reason: RetireReason,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RetireReason {
    Logout,
    LoginExpired,
    RealmChanged,
    ApplicationStopping,
    BridgeLost,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Close {
    pub target: Target,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AnswerEnvelope {
    pub answer: Answer,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Answer {
    ForgeRecovery { choice: Choice },
    ForgeOrigin { origin: String },
    ForgePassword { password: String },
    Approval { decision: Decision },
    PairedHostTarget { target_id: String },
    PairedHostPairingMethod { method: PairingMethod },
    PairedHostPairingCode { code: String },
    PairedHostNotice { dismiss: bool },
    ProviderTarget { choice_id: String },
    ProviderAction { action: ProviderAction },
    ProviderApiKey { key: String },
    Cancel,
}
#[derive(Clone, Copy, serde::Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderAction {
    Use,
    EnterKey,
    SignIn,
    Back,
}
#[derive(Clone, Copy, serde::Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PairingMethod {
    Request,
    Terminal,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Choice {
    Existing { choice_id: String },
    More,
    New,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Decision {
    Approve,
    Deny,
}
pub(super) fn valid_answer(prompt: &Value, answer: &Answer) -> bool {
    match (prompt["kind"].as_str(), answer) {
        (_, Answer::Cancel) => true,
        (Some("forge_origin"), Answer::ForgeOrigin { origin }) => {
            prompt["permitted_origin"].as_str() == Some(origin)
        }
        (Some("forge_password"), Answer::ForgePassword { password }) => {
            (12..=4096).contains(&password.chars().count())
        }
        (Some("approval"), Answer::Approval { .. }) => true,
        (Some("provider_target"), Answer::ProviderTarget { choice_id }) => {
            prompt["choices"].as_array().is_some_and(|choices| {
                choices
                    .iter()
                    .any(|choice| choice["choice_id"].as_str() == Some(choice_id))
            })
        }
        (Some("provider_action"), Answer::ProviderAction { action }) => {
            serde_json::to_value(action).ok().is_some_and(|action| {
                prompt["actions"]
                    .as_array()
                    .is_some_and(|actions| actions.contains(&action))
            })
        }
        (Some("provider_api_key"), Answer::ProviderApiKey { key }) => {
            !key.trim().is_empty() && key.len() <= 8192 && !key.chars().any(char::is_control)
        }
        (Some("paired_host_target"), Answer::PairedHostTarget { target_id }) => {
            prompt["choices"].as_array().is_some_and(|choices| {
                choices
                    .iter()
                    .any(|choice| choice["target_id"].as_str() == Some(target_id))
            })
        }
        (Some("paired_host_pairing_method"), Answer::PairedHostPairingMethod { .. }) => true,
        (Some("paired_host_pairing_code"), Answer::PairedHostPairingCode { code }) => {
            match prompt["method"].as_str() {
                Some("request") => crate::paired_host::PairingCode::new(code.clone()).is_ok(),
                Some("terminal") => {
                    crate::paired_host::TerminalPairingCode::new(code.clone()).is_ok()
                }
                _ => false,
            }
        }
        (Some("paired_host_notice"), Answer::PairedHostNotice { dismiss }) => *dismiss,
        (
            Some("forge_recovery"),
            Answer::ForgeRecovery {
                choice: Choice::New,
            },
        ) => true,
        (
            Some("forge_recovery"),
            Answer::ForgeRecovery {
                choice: Choice::More,
            },
        ) => prompt["more"] == true,
        (
            Some("forge_recovery"),
            Answer::ForgeRecovery {
                choice: Choice::Existing { choice_id },
            },
        ) => prompt["choices"]
            .as_array()
            .is_some_and(|v| v.iter().any(|c| c["choice_id"].as_str() == Some(choice_id))),
        _ => false,
    }
}
pub(super) fn error(id: Value, code: i64, message: &str, data: Option<&str>) -> Value {
    let mut error = json!({"code":code,"message":message});
    if let Some(data) = data {
        error["data"] = json!({"code":data});
    }
    json!({"jsonrpc":"2.0","id":id,"error":error})
}
pub(super) fn failed(id: &str, code: &str) -> Value {
    error(json!(id), -32010, "Connect request was refused", Some(code))
}
pub(super) fn result(id: &str, value: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":value})
}
