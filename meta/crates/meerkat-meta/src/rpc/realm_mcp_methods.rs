//! `meta/realm_mcp/*`: host administration of the installation realm's own
//! MCP servers and the service secrets of their stdio servers, served by the
//! stdio gateway only.
//!
//! Each method is one call of the gateway's realm MCP installation
//! ([`GatewayRealmMcpInstallation`]), with no semantics of its own. A write
//! is admitted by the stdio connection's [`AdministrationEntry`], which the
//! write's owner runs after the document's locks and the credential owner's
//! exclusion, immediately before it writes: the SDK host on the gateway's
//! stdio is its operator while that connection is open. The Console never
//! serves these methods, and no agent tool, mob tool or callback reaches
//! them.
//!
//! A secret value appears only in the `set_secret` request. Its parameter
//! moves the value into [`McpServiceSecretValue`] as it is parsed; no
//! response, error or log line carries it (`set_secret` answers `{}`, and a
//! malformed `set_secret` is refused without quoting its request), and
//! `servers` lists definitions, which name the secret slots and hold no
//! value.
//!
//! Typed outcomes, by `error.data.kind`: `realm_mcp_refused` (`-32602`, with
//! `realm_mcp_refusal.kind`), nothing written; `access_denied` (`-32030`,
//! the entry's refusal), nothing written; `realm_mcp_unavailable`
//! (`-32004`): the document or the credential owner could not be held now,
//! or the launch's credential store did not open, nothing written;
//! `realm_mcp_outcome_unknown` (`-32603`): the write began and its result is
//! not known.

use std::sync::Arc;

use meerkat::{McpServiceSecretRemoval, McpServiceSecretUpdateError};
use meerkat_core::auth::McpServiceSecretValue;
use meerkat_core::config_runtime::ConfigRuntimeError;
use meerkat_core::{
    AdministrationEntry, AdministrationEntryRefusal, McpRealmPersistError, McpServerConfig,
};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::{CAPABILITY_UNAVAILABLE_CODE, JSONRPC_VERSION, JsonRpcError, JsonRpcResponse};
use crate::gateway_mcp_composition::{GatewayRealmMcpError, GatewayRealmMcpInstallation};

/// The realm's own servers and its document's generation.
pub(crate) const REALM_MCP_SERVERS: &str = "meta/realm_mcp/servers";
/// Add a reviewed server at the generation the review read.
pub(crate) const REALM_MCP_INSTALL: &str = "meta/realm_mcp/install";
/// Store a slot's value for a reviewed definition.
pub(crate) const REALM_MCP_SET_SECRET: &str = "meta/realm_mcp/set_secret";
/// Remove a slot's value.
pub(crate) const REALM_MCP_REMOVE_SECRET: &str = "meta/realm_mcp/remove_secret";

/// The family, one registry the stdio gateway derives its dispatch arm, its
/// read/write split and its advertised methods from.
pub(crate) const REALM_MCP_METHODS: &[&str] = &[
    REALM_MCP_SERVERS,
    REALM_MCP_INSTALL,
    REALM_MCP_SET_SECRET,
    REALM_MCP_REMOVE_SECRET,
];

/// Whether `method` belongs to the family.
#[must_use]
pub(crate) fn is_realm_mcp_method(method: &str) -> bool {
    REALM_MCP_METHODS.contains(&method)
}

/// Whether `method` writes the realm's document or its credential store: a
/// candidate launch refuses it.
#[must_use]
pub(crate) fn is_realm_mcp_mutating_method(method: &str) -> bool {
    matches!(
        method,
        REALM_MCP_INSTALL | REALM_MCP_SET_SECRET | REALM_MCP_REMOVE_SECRET
    )
}

/// The stdio gateway's realm MCP administration: its installation and the
/// entry of the stdio connection its host administers through.
#[derive(Clone)]
pub(crate) struct RealmMcpAdministration {
    installation: GatewayRealmMcpInstallation,
    entry: Arc<dyn AdministrationEntry>,
}

impl RealmMcpAdministration {
    pub(crate) fn new(
        installation: GatewayRealmMcpInstallation,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Self {
        Self {
            installation,
            entry,
        }
    }

    /// The entry a write of this administration runs.
    #[cfg(test)]
    pub(crate) fn entry(&self) -> &dyn AdministrationEntry {
        self.entry.as_ref()
    }
}

/// Serve `method` of the family: `None` for a method outside it.
pub(crate) async fn handle_realm_mcp_rpc(
    administration: &RealmMcpAdministration,
    method: &str,
    response_id: Value,
    params: &Value,
) -> Option<JsonRpcResponse> {
    let answer = match method {
        REALM_MCP_SERVERS => servers(administration, params).await,
        REALM_MCP_INSTALL => install(administration, params).await,
        REALM_MCP_SET_SECRET => set_secret(administration, params).await,
        REALM_MCP_REMOVE_SECRET => remove_secret(administration, params).await,
        _ => return None,
    };
    let (result, error) = match answer {
        Ok(result) => (Some(result), None),
        Err(error) => (None, Some(error)),
    };
    Some(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result,
        error,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServersParams {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallParams {
    server: McpServerConfig,
    expected_generation: u64,
}

/// `set_secret`'s request. It has no `Debug`: its value is never shown.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetSecretParams {
    server: McpServerConfig,
    env: String,
    value: SecretValueParam,
}

/// The value of a `set_secret` request, moved into
/// [`McpServiceSecretValue`] as it is parsed. No `Debug`, `Display`, `Clone`
/// or serialization.
struct SecretValueParam(McpServiceSecretValue);

impl<'de> Deserialize<'de> for SecretValueParam {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(McpServiceSecretValue::new(value)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveSecretParams {
    server: McpServerConfig,
    env: String,
    removal: RemovalParam,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RemovalParam {
    Current,
    Obsolete,
}

impl From<RemovalParam> for McpServiceSecretRemoval {
    fn from(removal: RemovalParam) -> Self {
        match removal {
            RemovalParam::Current => Self::Current,
            RemovalParam::Obsolete => Self::Obsolete,
        }
    }
}

/// `params` as `T`: an absent `params` is an empty object. The parse error
/// is quoted only where `quote` allows; a request that may carry a secret is
/// refused without its detail, which can quote a value.
fn parse<T: serde::de::DeserializeOwned>(
    params: &Value,
    method: &str,
    quote: bool,
) -> Result<T, JsonRpcError> {
    let empty = Value::Object(serde_json::Map::new());
    let params = if params.is_null() { &empty } else { params };
    T::deserialize(params).map_err(|error| JsonRpcError {
        code: -32602,
        message: if quote {
            format!("Invalid params for {method}: {error}")
        } else {
            format!("Invalid params for {method}: expected {{server, env, value}}")
        },
        data: None,
    })
}

/// A typed refusal: invalid params carrying
/// `{"kind": "realm_mcp_refused", "realm_mcp_refusal": refusal}`. Nothing
/// was written.
fn refused(refusal: Value, message: String) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message,
        data: Some(json!({"kind": "realm_mcp_refused", "realm_mcp_refusal": refusal})),
    }
}

/// The document or the credential owner could not be held now, or the
/// launch's credential store did not open. Nothing was written.
fn unavailable(message: String) -> JsonRpcError {
    JsonRpcError {
        code: CAPABILITY_UNAVAILABLE_CODE,
        message,
        data: Some(json!({"kind": "realm_mcp_unavailable"})),
    }
}

/// The write began and its result is not known: read `servers` again after
/// an install, and repeat the same secret update, which settles it.
fn outcome_unknown(message: String) -> JsonRpcError {
    JsonRpcError {
        code: -32603,
        message,
        data: Some(json!({"kind": "realm_mcp_outcome_unknown"})),
    }
}

/// The entry refused the caller at the write's entry. Nothing was written.
fn not_admitted(refusal: &AdministrationEntryRefusal) -> JsonRpcError {
    let kind = match refusal {
        AdministrationEntryRefusal::NotAuthenticated => "not_authenticated",
        AdministrationEntryRefusal::NotAdministrator => "not_administrator",
        _ => "unavailable",
    };
    JsonRpcError {
        code: crate::http_console::ACCESS_DENIED_RPC_CODE,
        message: format!("access denied: {refusal}"),
        data: Some(json!({"kind": "access_denied", "refusal": kind})),
    }
}

fn persist_error(error: McpRealmPersistError) -> JsonRpcError {
    let message = error.to_string();
    match error {
        McpRealmPersistError::ServerExists(name) => {
            refused(json!({"kind": "server_exists", "name": name}), message)
        }
        McpRealmPersistError::ServerNotFound(name) => {
            refused(json!({"kind": "server_not_found", "name": name}), message)
        }
        McpRealmPersistError::EnvReference(_)
        | McpRealmPersistError::Access(_)
        | McpRealmPersistError::Cwd(_) => refused(json!({"kind": "invalid_server"}), message),
        McpRealmPersistError::Config(ConfigRuntimeError::GenerationConflict {
            expected,
            current,
        }) => refused(
            json!({"kind": "generation_conflict", "expected": expected, "current": current}),
            message,
        ),
        McpRealmPersistError::Config(_) => unavailable(message),
        McpRealmPersistError::NotAdmitted(refusal) => not_admitted(&refusal),
        McpRealmPersistError::OutcomeUnknown(_) => outcome_unknown(message),
    }
}

fn installation_error(error: GatewayRealmMcpError) -> JsonRpcError {
    match error {
        GatewayRealmMcpError::NoDocument => {
            refused(json!({"kind": "no_document"}), error.to_string())
        }
        GatewayRealmMcpError::Persist(error) => persist_error(error),
        GatewayRealmMcpError::NotAdmitted(refusal) => not_admitted(&refusal),
        GatewayRealmMcpError::OutcomeUnknown(_) => outcome_unknown(error.to_string()),
    }
}

fn secret_error(error: McpServiceSecretUpdateError) -> JsonRpcError {
    let message = error.to_string();
    match error {
        McpServiceSecretUpdateError::Invalid(_) => {
            refused(json!({"kind": "invalid_secret_update"}), message)
        }
        McpServiceSecretUpdateError::DefinitionChanged(name) => {
            refused(json!({"kind": "definition_changed", "name": name}), message)
        }
        McpServiceSecretUpdateError::StillCurrent(name) => {
            refused(json!({"kind": "still_current", "name": name}), message)
        }
        McpServiceSecretUpdateError::ConfigurationUnavailable(_)
        | McpServiceSecretUpdateError::Ownership(_) => unavailable(message),
        McpServiceSecretUpdateError::NotAdmitted(refusal) => not_admitted(&refusal),
        McpServiceSecretUpdateError::OutcomeUnknown(_) => outcome_unknown(message),
    }
}

async fn servers(
    administration: &RealmMcpAdministration,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    let ServersParams {} = parse(params, REALM_MCP_SERVERS, true)?;
    let (servers, generation) = administration
        .installation
        .servers()
        .await
        .map_err(installation_error)?;
    Ok(json!({"servers": servers, "generation": generation}))
}

async fn install(
    administration: &RealmMcpAdministration,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    let InstallParams {
        server,
        expected_generation,
    } = parse(params, REALM_MCP_INSTALL, true)?;
    let generation = administration
        .installation
        .install(
            server,
            expected_generation,
            Arc::clone(&administration.entry),
        )
        .await
        .map_err(installation_error)?;
    Ok(json!({"generation": generation}))
}

async fn set_secret(
    administration: &RealmMcpAdministration,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    let SetSecretParams { server, env, value } = parse(params, REALM_MCP_SET_SECRET, false)?;
    administration
        .installation
        .set_secret(&server, &env, value.0, Arc::clone(&administration.entry))
        .await
        .map_err(secret_error)?;
    Ok(json!({}))
}

async fn remove_secret(
    administration: &RealmMcpAdministration,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    let RemoveSecretParams {
        server,
        env,
        removal,
    } = parse(params, REALM_MCP_REMOVE_SECRET, true)?;
    administration
        .installation
        .remove_secret(
            &server,
            &env,
            removal.into(),
            Arc::clone(&administration.entry),
        )
        .await
        .map_err(secret_error)?;
    Ok(json!({}))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// The family's registry and its read/write split.
    #[test]
    fn the_listing_reads_and_the_rest_write() {
        for method in REALM_MCP_METHODS {
            assert!(is_realm_mcp_method(method), "{method}");
        }
        assert!(!is_realm_mcp_method("meta/realm_mcp/remove"));
        assert!(!is_realm_mcp_mutating_method(REALM_MCP_SERVERS));
        for method in [
            REALM_MCP_INSTALL,
            REALM_MCP_SET_SECRET,
            REALM_MCP_REMOVE_SECRET,
        ] {
            assert!(is_realm_mcp_mutating_method(method), "{method}");
        }
    }

    /// A malformed `set_secret` is refused without quoting its request,
    /// whatever the fault.
    #[test]
    fn a_malformed_set_secret_quotes_nothing_of_its_request() {
        let secret = "realm-mcp-unit-secret";
        for params in [
            json!({"server": {"name": "s", "command": "sh"}, "env": "K", "value": secret, "extra": secret}),
            json!({"server": {"name": "s", "command": "sh"}, "env": secret}),
            json!({"server": secret, "env": "K", "value": secret}),
        ] {
            let Err(error) = parse::<SetSecretParams>(&params, REALM_MCP_SET_SECRET, false) else {
                panic!("refused: {params}");
            };
            assert_eq!(error.code, -32602);
            assert!(!error.message.contains(secret), "{}", error.message);
            assert_eq!(error.data, None);
        }
    }

    /// Each outcome answers its kind and code.
    #[test]
    fn each_outcome_answers_its_kind() {
        let conflict = persist_error(McpRealmPersistError::Config(
            ConfigRuntimeError::GenerationConflict {
                expected: 3,
                current: 4,
            },
        ));
        assert_eq!(conflict.code, -32602);
        assert_eq!(
            conflict.data,
            Some(json!({
                "kind": "realm_mcp_refused",
                "realm_mcp_refusal": {"kind": "generation_conflict", "expected": 3, "current": 4},
            }))
        );
        let busy = persist_error(McpRealmPersistError::Config(
            ConfigRuntimeError::LockTimeout,
        ));
        assert_eq!(busy.code, CAPABILITY_UNAVAILABLE_CODE);
        assert_eq!(busy.data, Some(json!({"kind": "realm_mcp_unavailable"})));
        let closed = secret_error(McpServiceSecretUpdateError::NotAdmitted(
            AdministrationEntryRefusal::NotAuthenticated,
        ));
        assert_eq!(closed.code, crate::http_console::ACCESS_DENIED_RPC_CODE);
        assert_eq!(
            closed.data,
            Some(json!({"kind": "access_denied", "refusal": "not_authenticated"}))
        );
        let unknown = secret_error(McpServiceSecretUpdateError::OutcomeUnknown(
            "the store write failed".to_string(),
        ));
        assert_eq!(unknown.code, -32603);
        assert_eq!(
            unknown.data,
            Some(json!({"kind": "realm_mcp_outcome_unknown"}))
        );
        let no_document = installation_error(GatewayRealmMcpError::NoDocument);
        assert_eq!(
            no_document.data,
            Some(json!({
                "kind": "realm_mcp_refused",
                "realm_mcp_refusal": {"kind": "no_document"},
            }))
        );
    }
}
