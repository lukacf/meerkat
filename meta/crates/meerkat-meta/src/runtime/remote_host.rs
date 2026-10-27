//! Remote runtime-host lifecycle: endpoint identity, capability
//! discovery, health, durable pairing, and reconnect for Meta gateways
//! that other gateways treat as PLACEMENT INFRASTRUCTURE.
//!
//! # What a runtime host is here, and what it is not
//!
//! A runtime host is a process that can *run* work. It is never a mob
//! authority and never a project authority. Nothing in this module lets a
//! host mutate a controller's mob, roster, topology, or project state:
//! the only two operations that cross the wire
//! ([`super::cross_mob_control::ControlVerb::HostDescribe`] and
//! [`super::cross_mob_control::ControlVerb::HostHealth`]) are read-only
//! projections the host makes *about itself*, and the only durable
//! artifact ([`HostPairingRecord`]) lives on the CONTROLLER and is never
//! read or written by the host.
//!
//! That asymmetry is structural, not a convention:
//!
//! * The host serves facts and stores nothing about who asked.
//! * The controller stores the pairing and pins the host identity.
//! * A [`HostPairingRecord`] can therefore never become an authority the
//!   host can edit, because the host has no verb that writes one.
//!
//! # Placement is surveyed, not decided
//!
//! [`HostFacts::placement_labels`] carries the host's self-reported
//! labels through as an opaque projection (the same shape
//! `meerkat_contracts::RuntimeHostInfo::placement_labels` uses). This
//! module deliberately implements NO candidate selection, scoring, or
//! assignment. Placement authority for multi-host mobs already exists on
//! the meerkat side and is not Meta's to duplicate:
//! `meerkat_mob::store::MobHostAuthorityRecord` with its typed
//! write/delete permits, and the `RemoteHostBind*` / `RemoteHostRevoke*`
//! event family in `meerkat_mob::event::MobEventKind`. A Meta-side
//! chooser would either shadow that authority or compete with it.
//!
//! # Identity
//!
//! A host is identified by the Ed25519 public key its gateway signs
//! control responses with - the same key a
//! [`crate::contact_directory::ContactEntry`] pins - and by nothing else.
//! Self-reported strings (`host_label`, `advertised_control_address`) are
//! carried for display and are never trusted:
//!
//! * `meerkat_contracts::RuntimeHostInfo::host_id` is surface-minted and,
//!   under `RuntimeHostIdScope::Process`, is explicitly only stable for
//!   one process lifetime - useless as a durable pairing key.
//! * `meerkat_mob::store::MobHostAuthorityRecord::host_id` is a comms
//!   `PeerId`, a third vocabulary again.
//!
//! Keying the pairing on the signing key is what makes the pin
//! meaningful: it is the one field the control channel actually
//! authenticates.
//!
//! # Verification status
//!
//! Everything here is exercised across a PROCESS boundary at best (see
//! `tests/cross_mob_two_process.rs` for the existing idiom). No test in
//! this repository crosses a real HOST boundary, so health and reconnect
//! behaviour under genuine network partition is unverified.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use meerkat_contracts::{
    RuntimeHostCapabilities, RuntimeHostEndpointProjection, RuntimeHostFeatureFlags,
    RuntimeHostHealth, RuntimeHostHealthStatus,
};

use super::cross_mob_control::{
    ControlRequest, ControlResponse, DEFAULT_CONTROL_TIMEOUT, RemoteControlClient,
    push_signed_field, sha256_hex, verify_control_response,
};
use super::cross_mob_remote::{RemoteEndpoint, RemoteMobError};

/// Canonical controller-owned pairing file under a gateway's durable state
/// root. The gateway signing key and these endpoint-identity pins must share
/// one root so a restart cannot retain one identity while forgetting the
/// other.
pub const HOST_PAIRING_FILE_NAME: &str = "runtime-host-pairings.json";

/// Engine name Meta gateways report in [`HostFacts::engine`].
pub const META_HOST_ENGINE: &str = "meerkat-meta";

/// Stable `ControlResponse::Err` code answered when a gateway serves the
/// host plane but has no [`HostFactsProvider`] installed.
///
/// Distinct from an authorization refusal on purpose: the caller is
/// granted the verb, the peer simply is not configured to be a runtime
/// host. Retrying will not help; the peer operator has to install a
/// provider.
pub const HOST_PLANE_UNAVAILABLE_CODE: &str = "host_plane_unavailable";

/// Domain separation for the [`HostFacts`] digest. Versioned: any change
/// to the digested field set must bump this.
const HOST_FACTS_DIGEST_CONTEXT: &str = "meta-runtime-host-facts-v1";

/// Domain separation for the host-health digest. Distinct from
/// [`HOST_FACTS_DIGEST_CONTEXT`] so a signature over one can never be
/// replayed as a signature over the other.
const HOST_HEALTH_DIGEST_CONTEXT: &str = "meta-runtime-host-health-v1";

// ---------------------------------------------------------------------
// Host facts: the read-only projection a host serves about itself
// ---------------------------------------------------------------------

/// What a runtime host reports about itself.
///
/// Every field is the host's own claim. Only `gateway_pubkey_b64` is
/// checkable, and [`RemoteHostClient::describe`] checks it: the response
/// signature must verify against the pinned key AND the facts must name
/// that same key, so a host cannot answer for an identity it does not
/// hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFacts {
    /// Operator-facing name. UNVERIFIED - display only. Never use it as
    /// a pairing key or an authorization input.
    pub host_label: String,
    /// Standard-base64 Ed25519 public key of the serving gateway.
    pub gateway_pubkey_b64: String,
    /// Control address the host advertises for itself. May differ from
    /// the address a controller actually dialed (NAT, split horizon), so
    /// the pairing record pins the DIALED endpoint, not this one.
    pub advertised_control_address: String,
    /// Engine that produced these facts ([`META_HOST_ENGINE`] for a
    /// Meta gateway).
    pub engine: String,
    /// Engine version string.
    pub engine_version: String,
    /// Capability projection in meerkat's own vocabulary, so controller
    /// and host never drift into two capability dialects.
    pub capabilities: RuntimeHostCapabilities,
    /// Surface reachability projection. Reported, never owned.
    #[serde(default)]
    pub endpoints: RuntimeHostEndpointProjection,
    /// Opaque host-reported placement labels. Carried through; never
    /// interpreted here. See the module docs on why no selection exists.
    #[serde(default)]
    pub placement_labels: BTreeMap<String, String>,
}

impl HostFacts {
    /// Build the minimal honest fact set for a Meta gateway.
    ///
    /// `capabilities` should come from
    /// `meerkat::surface::build_runtime_host_capabilities`, whose
    /// `RuntimeHostSurfaceOptions::process` default reports every feature
    /// as absent - a host advertises a capability only after its owner
    /// explicitly sets the flag.
    pub fn new(
        host_label: impl Into<String>,
        gateway_pubkey_b64: impl Into<String>,
        advertised_control_address: impl Into<String>,
        capabilities: RuntimeHostCapabilities,
    ) -> Self {
        Self {
            host_label: host_label.into(),
            gateway_pubkey_b64: gateway_pubkey_b64.into(),
            advertised_control_address: advertised_control_address.into(),
            engine: META_HOST_ENGINE.to_string(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities,
            endpoints: RuntimeHostEndpointProjection::default(),
            placement_labels: BTreeMap::new(),
        }
    }

    /// Attach the surface reachability projection.
    #[must_use]
    pub fn with_endpoints(mut self, endpoints: RuntimeHostEndpointProjection) -> Self {
        self.endpoints = endpoints;
        self
    }

    /// Attach host-reported placement labels.
    #[must_use]
    pub fn with_placement_labels(mut self, labels: BTreeMap<String, String>) -> Self {
        self.placement_labels = labels;
        self
    }

    /// Host-reported placement labels.
    ///
    /// ADVISORY ONLY. Reading these never places anything: Meta has no
    /// placement authority, and the meerkat-side host binding
    /// (`MobHostAuthorityRecord`) is the thing that actually decides where
    /// a member materializes.
    pub fn placement_labels(&self) -> &BTreeMap<String, String> {
        &self.placement_labels
    }

    /// SHA-256 over a length-prefixed canonical encoding of every field.
    ///
    /// Length-prefixed for the same reason the request signing payload is
    /// (see `push_signed_field`): a placement label value containing a
    /// newline must not be able to forge a different fact set that
    /// digests identically. The `Member` response's newline-joined facts
    /// are deliberately NOT the precedent followed here - its fields are
    /// all constrained shapes; these carry operator-supplied maps.
    pub fn signing_digest_hex(&self) -> String {
        let Self {
            host_label,
            gateway_pubkey_b64,
            advertised_control_address,
            engine,
            engine_version,
            capabilities,
            endpoints,
            placement_labels,
        } = self;
        let mut payload = String::new();
        payload.push_str(HOST_FACTS_DIGEST_CONTEXT);
        payload.push('\n');
        push_signed_field(&mut payload, host_label);
        push_signed_field(&mut payload, gateway_pubkey_b64);
        push_signed_field(&mut payload, advertised_control_address);
        push_signed_field(&mut payload, engine);
        push_signed_field(&mut payload, engine_version);
        push_capabilities(&mut payload, capabilities);
        push_endpoints(&mut payload, endpoints);
        push_string_map(&mut payload, placement_labels);
        sha256_hex(payload.as_bytes())
    }
}

/// Push an optional string so `None` and `Some("")` can never collide.
fn push_optional(out: &mut String, value: Option<&str>) {
    match value {
        Some(text) => {
            push_signed_field(out, "s");
            push_signed_field(out, text);
        }
        None => push_signed_field(out, "n"),
    }
}

/// Push a string sequence, length first so concatenation cannot forge a
/// different split.
fn push_string_seq<'a>(out: &mut String, values: impl ExactSizeIterator<Item = &'a str>) {
    push_signed_field(out, &values.len().to_string());
    for value in values {
        push_signed_field(out, value);
    }
}

fn push_string_map(out: &mut String, map: &BTreeMap<String, String>) {
    push_signed_field(out, &map.len().to_string());
    for (key, value) in map {
        push_signed_field(out, key);
        push_signed_field(out, value);
    }
}

/// Digest the capability projection.
///
/// The destructuring is exhaustive with no `..` on purpose: a capability
/// flag meerkat adds must break this build rather than silently drop out
/// of the signed material and become MITM-malleable.
fn push_capabilities(out: &mut String, capabilities: &RuntimeHostCapabilities) {
    let RuntimeHostCapabilities {
        contract_version,
        features,
    } = capabilities;
    push_signed_field(out, &contract_version.to_string());
    let RuntimeHostFeatureFlags {
        runtime_backed_sessions,
        mobs,
        mcp_live,
        comms,
        blobs,
        session_events,
        session_streams,
        schedules,
        skills,
        event_replay,
        artifacts,
        approvals,
        external_members,
        secure_remote_rpc,
        multi_host_mobs,
        durable_jobs,
    } = features;
    for flag in [
        *runtime_backed_sessions,
        *mobs,
        *mcp_live,
        *comms,
        *blobs,
        *session_events,
        *session_streams,
        *schedules,
        *skills,
        *event_replay,
        *artifacts,
        *approvals,
        *external_members,
        *secure_remote_rpc,
        *multi_host_mobs,
        *durable_jobs,
    ] {
        push_signed_field(out, if flag { "1" } else { "0" });
    }
}

/// Digest the endpoint projection. Exhaustive for the same reason
/// [`push_capabilities`] is.
fn push_endpoints(out: &mut String, endpoints: &RuntimeHostEndpointProjection) {
    let RuntimeHostEndpointProjection {
        rpc_transport,
        rest_base_url,
        rpc_methods,
        rest_paths,
    } = endpoints;
    push_optional(out, rpc_transport.as_deref());
    push_optional(out, rest_base_url.as_deref());
    push_string_seq(out, rpc_methods.iter().map(String::as_str));
    push_string_seq(out, rest_paths.iter().map(String::as_str));
}

/// Stable digest token for a health status.
///
/// Written out rather than derived from serde so a rename on meerkat's
/// side cannot silently change what a signature covers, and exhaustive so
/// a new status breaks the build.
fn health_status_token(status: RuntimeHostHealthStatus) -> &'static str {
    match status {
        RuntimeHostHealthStatus::Ok => "ok",
        RuntimeHostHealthStatus::Degraded => "degraded",
        RuntimeHostHealthStatus::Unhealthy => "unhealthy",
    }
}

/// SHA-256 over a length-prefixed canonical encoding of a health report.
///
/// Check names are operator-supplied, so the same newline-injection
/// argument as [`HostFacts::signing_digest_hex`] applies.
pub fn host_health_digest_hex(health: &RuntimeHostHealth) -> String {
    let RuntimeHostHealth {
        contract_version,
        status,
        checks,
    } = health;
    let mut payload = String::new();
    payload.push_str(HOST_HEALTH_DIGEST_CONTEXT);
    payload.push('\n');
    push_signed_field(&mut payload, &contract_version.to_string());
    push_signed_field(&mut payload, health_status_token(*status));
    push_signed_field(&mut payload, &checks.len().to_string());
    for (name, check) in checks {
        push_signed_field(&mut payload, name);
        push_signed_field(&mut payload, health_status_token(*check));
    }
    sha256_hex(payload.as_bytes())
}

// ---------------------------------------------------------------------
// Serving side
// ---------------------------------------------------------------------

/// Host-side source of the two read-only projections.
///
/// A gateway that installs no provider answers
/// [`HOST_PLANE_UNAVAILABLE_CODE`] to both host verbs, so serving host
/// facts is opt-in and absent by default.
pub trait HostFactsProvider: Send + Sync + 'static {
    /// Identity, capability and placement-label projection.
    fn describe(&self) -> HostFacts;
    /// Current health projection.
    fn health(&self) -> RuntimeHostHealth;
}

/// A provider over a fixed fact set - the shape a gateway that computes
/// its facts once at startup wants.
#[derive(Debug, Clone)]
pub struct StaticHostFacts {
    facts: HostFacts,
    health: RuntimeHostHealth,
}

impl StaticHostFacts {
    /// Build a provider over already-computed projections.
    pub fn new(facts: HostFacts, health: RuntimeHostHealth) -> Self {
        Self { facts, health }
    }
}

impl HostFactsProvider for StaticHostFacts {
    fn describe(&self) -> HostFacts {
        self.facts.clone()
    }

    fn health(&self) -> RuntimeHostHealth {
        self.health.clone()
    }
}

// ---------------------------------------------------------------------
// Controller side: the authenticated client
// ---------------------------------------------------------------------

/// Errors from the runtime-host control client.
///
/// Deliberately not `RemoteMobError`: every variant of that type carries a
/// `mob_id`, and a runtime host has no mob. Filling that field with a host
/// identifier would be a type-level lie about what the failure is about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemoteHostError {
    /// Connect / write / read / timeout against the host control channel.
    Transport {
        endpoint: String,
        operation: &'static str,
        detail: String,
    },
    /// The request could not be encoded (programmer error).
    Encode { endpoint: String, message: String },
    /// The response could not be decoded, or was the wrong shape.
    Decode { endpoint: String, message: String },
    /// The answer could not be attributed to the pinned host key:
    /// unsigned, malformed signature, signed by a different key, or facts
    /// claiming an identity other than the signer.
    Unauthenticated { endpoint: String, reason: String },
    /// The host answered with an error response. Error responses are never
    /// signed, so this is never evidence of anything: `code` (for example
    /// `caller_not_granted` or `host_plane_unavailable`) and `message` are
    /// the host's words, kept as text for operators, and no caller branches
    /// on them.
    Rejected {
        endpoint: String,
        code: String,
        message: String,
    },
}

impl std::fmt::Display for RemoteHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport {
                endpoint,
                operation,
                detail,
            } => write!(
                f,
                "runtime host at {endpoint} is unreachable ({operation}): {detail}"
            ),
            Self::Encode { endpoint, message } => {
                write!(f, "failed to encode host request for {endpoint}: {message}")
            }
            Self::Decode { endpoint, message } => {
                write!(f, "undecodable host response from {endpoint}: {message}")
            }
            Self::Unauthenticated { endpoint, reason } => write!(
                f,
                "host response from {endpoint} could not be attributed to the pinned host key: \
                 {reason}"
            ),
            Self::Rejected {
                endpoint,
                code,
                message,
            } => write!(f, "runtime host at {endpoint} refused ({code}): {message}"),
        }
    }
}

impl std::error::Error for RemoteHostError {}

/// Map a transport-layer `RemoteMobError` onto the host vocabulary.
///
/// `RemoteControlClient::send_payload` takes PRE-SERIALIZED bytes, so it
/// produces exactly three shapes: `ControlChannelUnavailable` (connect /
/// write / read / timeout), `UnsupportedTransport` (a `uds://` endpoint
/// on a non-unix target, where `send_inner` cannot connect at all), and
/// `Decode`. `RemoteMobError::Encode` belongs to `send`, which serializes
/// before delegating; `RemoteHostClient::dispatch` serializes itself and
/// raises `RemoteHostError::Encode` directly, so the arm below is a
/// cheap non-path kept for shape completeness, not a live one.
///
/// The catch-all is therefore NOT unreachable - `Decode` lands in it on
/// every malformed answer - it is the arm that maps `Decode` onto
/// `Decode`. Anything a future `send_payload` adds degrades to `Decode`
/// too, which is fail-closed: the client treats it as an unusable answer.
fn host_transport_error(endpoint: &RemoteEndpoint, err: &RemoteMobError) -> RemoteHostError {
    let address = endpoint.comms_address();
    match err {
        RemoteMobError::ControlChannelUnavailable {
            mob_id, operation, ..
        } => RemoteHostError::Transport {
            endpoint: address,
            operation,
            // `cross_mob_control::io_error` builds this with an empty
            // `mob_id` and then `RemoteMobError::with_context` stashes
            // the underlying io message there (there is no mob to name),
            // so `detail` carries it for connect / write / read. The
            // `timeout` path in `send_payload` never calls
            // `with_context`, so `detail` is EMPTY there and `operation`
            // ("timeout") is the whole signal - do not read a blank
            // detail as "no error".
            detail: mob_id.clone(),
        },
        // Unreachable-by-construction, not undecodable: reporting this as
        // `Decode` would send an operator hunting a protocol mismatch
        // that does not exist.
        RemoteMobError::UnsupportedTransport { transport, .. } => RemoteHostError::Transport {
            endpoint: address,
            operation: "connect",
            detail: format!("unsupported control transport: {transport}"),
        },
        RemoteMobError::Encode { message, .. } => RemoteHostError::Encode {
            endpoint: address,
            message: message.clone(),
        },
        other => RemoteHostError::Decode {
            endpoint: address,
            message: other.to_string(),
        },
    }
}

/// Host facts that were authenticated against a pinned host key.
///
/// A typed permit, not a data holder: the fields are private and no
/// public constructor exists, so the only way to hold one is to have gone
/// through [`RemoteHostClient::describe`]. [`HostPairingStore::pair`]
/// takes one by reference, which makes "never pair with a host you did
/// not authenticate" a compile-time property instead of a doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedHostFacts {
    host_key_b64: String,
    dialed_endpoint: String,
    facts: HostFacts,
}

impl VerifiedHostFacts {
    /// Canonical standard-base64 host key that signed these facts.
    pub fn host_key_b64(&self) -> &str {
        &self.host_key_b64
    }

    /// The endpoint that was actually dialed and authenticated. This, not
    /// the host's self-reported `advertised_control_address`, is what a
    /// pairing pins.
    pub fn dialed_endpoint(&self) -> &str {
        &self.dialed_endpoint
    }

    /// The authenticated facts.
    pub fn facts(&self) -> &HostFacts {
        &self.facts
    }
}

/// Controller-side client for one paired (or candidate) runtime host.
///
/// The pinned host key is NOT optional, unlike
/// [`super::cross_mob_remote::RemoteMobProxy`]'s, which tolerates
/// unpinned contacts for backward compatibility. Host discovery has no
/// legacy to preserve, so trust-on-first-use is refused by the type: you
/// cannot construct a client without already knowing which key must have
/// signed the answer.
#[derive(Debug, Clone)]
pub struct RemoteHostClient {
    endpoint: RemoteEndpoint,
    host_key: [u8; 32],
    caller_keys: Option<std::sync::Arc<crate::auth::peer_keys::GatewayPeerKeys>>,
    audience: Option<String>,
    timeout: Duration,
}

impl RemoteHostClient {
    /// Build a client pinned to `host_key`.
    pub fn new(endpoint: RemoteEndpoint, host_key: [u8; 32]) -> Self {
        Self {
            endpoint,
            host_key,
            caller_keys: None,
            audience: None,
            timeout: DEFAULT_CONTROL_TIMEOUT,
        }
    }

    /// Build a client from a base64 host key.
    pub fn from_pubkey_b64(
        endpoint: RemoteEndpoint,
        host_key_b64: &str,
    ) -> Result<Self, RemoteHostError> {
        let key = crate::auth::peer_keys::decode_pubkey_b64(host_key_b64).map_err(|err| {
            RemoteHostError::Unauthenticated {
                endpoint: endpoint.comms_address(),
                reason: format!("pinned host key is unusable: {err}"),
            }
        })?;
        Ok(Self::new(endpoint, key))
    }

    /// Authenticate every request as this gateway, so a host that enforces
    /// scoped grants can attribute (and refuse) the call.
    #[must_use]
    pub fn with_caller_keys(
        mut self,
        keys: std::sync::Arc<crate::auth::peer_keys::GatewayPeerKeys>,
    ) -> Self {
        self.caller_keys = Some(keys);
        self
    }

    /// Bind outgoing signatures to the name this controller knows the host
    /// by, so a captured request cannot be replayed at another host where
    /// this gateway also holds a grant.
    #[must_use]
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    /// Override the per-request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The pinned host key, standard-base64.
    pub fn host_key_b64(&self) -> String {
        base64_standard(&self.host_key)
    }

    /// The host's control endpoint.
    pub fn endpoint(&self) -> &RemoteEndpoint {
        &self.endpoint
    }

    /// Capability + endpoint-identity discovery.
    ///
    /// Fails closed on three independent grounds: the response must be
    /// signed, the signature must verify against the pinned key over the
    /// exact request bytes, and the returned facts must name that same
    /// key. The third check is what stops a host from answering under a
    /// borrowed identity.
    pub async fn describe(&self) -> Result<VerifiedHostFacts, RemoteHostError> {
        let request = ControlRequest::HostDescribe {
            nonce: Some(mint_nonce()),
            caller: None,
        };
        let response = self.dispatch(request).await?;
        match response {
            ControlResponse::Host { facts, .. } => {
                let presented = crate::auth::peer_keys::decode_pubkey_b64(
                    &facts.gateway_pubkey_b64,
                )
                .map_err(|err| RemoteHostError::Unauthenticated {
                    endpoint: self.endpoint.comms_address(),
                    reason: format!("host facts carry an unusable gateway pubkey: {err}"),
                })?;
                if presented != self.host_key {
                    return Err(RemoteHostError::Unauthenticated {
                        endpoint: self.endpoint.comms_address(),
                        reason: "host facts claim a gateway identity other than the key that \
                                 signed the response"
                            .to_string(),
                    });
                }
                Ok(VerifiedHostFacts {
                    host_key_b64: self.host_key_b64(),
                    dialed_endpoint: self.endpoint.comms_address(),
                    facts,
                })
            }
            other => Err(self.unexpected(&other, "Host")),
        }
    }

    /// Health probe. The report is signature-verified exactly like
    /// [`Self::describe`]'s facts.
    pub async fn health(&self) -> Result<RuntimeHostHealth, RemoteHostError> {
        let request = ControlRequest::HostHealth {
            nonce: Some(mint_nonce()),
            caller: None,
        };
        let response = self.dispatch(request).await?;
        match response {
            ControlResponse::HostHealth { health, .. } => Ok(health),
            other => Err(self.unexpected(&other, "HostHealth")),
        }
    }

    /// Sign, send, and authenticate one host request/response exchange.
    async fn dispatch(
        &self,
        mut request: ControlRequest,
    ) -> Result<ControlResponse, RemoteHostError> {
        if let Some(keys) = self.caller_keys.as_ref() {
            super::cross_mob_control::sign_control_request_as_caller(
                keys,
                self.audience.as_deref(),
                &mut request,
            );
        }
        let payload = serde_json::to_vec(&request).map_err(|err| RemoteHostError::Encode {
            endpoint: self.endpoint.comms_address(),
            message: err.to_string(),
        })?;
        let response = RemoteControlClient::send_payload(&self.endpoint, &payload, self.timeout)
            .await
            .map_err(|err| host_transport_error(&self.endpoint, &err))?;
        if let Err(reason) = verify_control_response(&self.host_key, &payload, &response) {
            return Err(RemoteHostError::Unauthenticated {
                endpoint: self.endpoint.comms_address(),
                reason,
            });
        }
        Ok(response)
    }

    /// Classify a response that is not the expected success shape.
    fn unexpected(&self, response: &ControlResponse, expected: &str) -> RemoteHostError {
        let endpoint = self.endpoint.comms_address();
        match response {
            // Unsigned: one unverified class for every code.
            ControlResponse::Err { code, message } => RemoteHostError::Rejected {
                endpoint,
                code: code.clone(),
                message: message.clone(),
            },
            other => RemoteHostError::Decode {
                endpoint,
                message: format!("expected {expected} response, got {other:?}"),
            },
        }
    }
}

fn base64_standard(bytes: &[u8; 32]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Fresh per-request freshness nonce. It rides inside the request bytes,
/// and the response signature covers their digest, so a signed host
/// response can never be replayed for a later probe.
fn mint_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// ---------------------------------------------------------------------
// Durable pairing (controller-owned)
// ---------------------------------------------------------------------

/// One durably paired runtime host, as the controller records it.
///
/// The host key is the map key of the store's file, not a field here:
/// duplicating it would let the two disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPairingRecord {
    /// The endpoint that was authenticated at pairing time.
    pub endpoint: String,
    /// Host's self-reported label at pairing time. Display only.
    pub host_label: String,
    /// Unix seconds when this host was first paired.
    pub paired_at_unix_secs: u64,
    /// Monotonic count of pair/rebind commits for this host key.
    #[serde(default)]
    pub pairing_generation: u64,
    /// Control audience used when probing a grant-enforcing host. Persisted
    /// because reconnect after process restart must reproduce the exact
    /// authenticated request context that established the pairing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_audience: Option<String>,
    /// Last authenticated facts, cached so a controller can answer
    /// capability questions while the host is unreachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_facts: Option<HostFacts>,
    /// Last authenticated health status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_health_status: Option<RuntimeHostHealthStatus>,
    /// Unix seconds of the last authenticated answer of any kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_unix_secs: Option<u64>,
}

/// Errors from the pairing store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPairingError {
    /// A host key in the file (or in a call) is not a usable Ed25519 key.
    InvalidHostKey { reason: String },
    /// Two file rows canonicalize to the same host key.
    DuplicateHostKey { host_key_b64: String },
    /// A DIFFERENT key answered at an endpoint this store already pins.
    /// The fail-closed identity guard: pairing never silently re-points.
    EndpointIdentityChanged {
        endpoint: String,
        pinned_host_key_b64: String,
        presented_host_key_b64: String,
    },
    /// An authenticated observation for a known key arrived through a
    /// different endpoint without an explicit re-pair operation.
    EndpointChanged {
        host_key_b64: String,
        pinned_endpoint: String,
        observed_endpoint: String,
    },
    /// No pairing exists for this host key.
    UnknownHost { host_key_b64: String },
    /// Filesystem failure reading or writing the store.
    Io { path: String, reason: String },
    /// The store file exists but is not decodable.
    Decode { path: String, reason: String },
    /// The store could not be encoded (programmer error).
    Encode { reason: String },
}

impl std::fmt::Display for HostPairingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHostKey { reason } => write!(f, "unusable runtime host key: {reason}"),
            Self::DuplicateHostKey { host_key_b64 } => write!(
                f,
                "runtime host pairing file lists '{host_key_b64}' twice; refusing an ambiguous pin"
            ),
            Self::EndpointIdentityChanged {
                endpoint,
                pinned_host_key_b64,
                presented_host_key_b64,
            } => write!(
                f,
                "endpoint {endpoint} is pinned to host key '{pinned_host_key_b64}' but \
                 '{presented_host_key_b64}' answered there; forget the pairing explicitly if \
                 the host was legitimately re-keyed"
            ),
            Self::EndpointChanged {
                host_key_b64,
                pinned_endpoint,
                observed_endpoint,
            } => write!(
                f,
                "runtime host '{host_key_b64}' is pinned to endpoint {pinned_endpoint} but an observation arrived through {observed_endpoint}; re-pair explicitly to move it"
            ),
            Self::UnknownHost { host_key_b64 } => {
                write!(f, "no runtime host pairing for '{host_key_b64}'")
            }
            Self::Io { path, reason } => {
                write!(f, "runtime host pairing io error for {path}: {reason}")
            }
            Self::Decode { path, reason } => write!(
                f,
                "runtime host pairing file {path} is undecodable: {reason}; refusing to start \
                 with forgotten pins"
            ),
            Self::Encode { reason } => {
                write!(f, "failed to encode runtime host pairings: {reason}")
            }
        }
    }
}

impl std::error::Error for HostPairingError {}

#[derive(Debug, Clone, Default, Deserialize)]
struct HostPairingFile {
    #[serde(default)]
    hosts: BTreeMap<String, HostPairingRecord>,
}

/// Borrowed mirror of [`HostPairingFile`] used for writing, so persisting
/// never clones the whole table.
#[derive(Debug, Serialize)]
struct HostPairingFileRef<'a> {
    hosts: &'a BTreeMap<String, HostPairingRecord>,
}

/// Durable, controller-owned record of which runtime hosts this gateway
/// has paired with, keyed by the host's Ed25519 signing key.
///
/// Takes its path by injection rather than reaching into
/// [`crate::storage_layout`], so the file it writes is entirely the
/// caller's choice and this module owns no layout policy.
///
/// NOTHING on the host side reads or writes this. A runtime host cannot
/// see, edit, or invalidate a controller's pairing, which is how "hosts
/// are not an authority" is enforced structurally.
#[derive(Debug, Clone)]
pub struct HostPairingStore {
    path: PathBuf,
    hosts: BTreeMap<String, HostPairingRecord>,
}

impl HostPairingStore {
    /// Load the store at `path`.
    ///
    /// A MISSING file is an empty store. An UNREADABLE or UNDECODABLE file
    /// is an error, deliberately unlike `meta_gateway`'s
    /// `load_registry`, which defaults on parse failure: silently
    /// defaulting here would forget every pinned host key and make the
    /// next pairing accept any key at a known endpoint. Forgetting a pin
    /// must be an explicit operator act.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, HostPairingError> {
        let path = path.into();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    path,
                    hosts: BTreeMap::new(),
                });
            }
            Err(err) => {
                return Err(HostPairingError::Io {
                    path: path.display().to_string(),
                    reason: err.to_string(),
                });
            }
        };
        let file: HostPairingFile =
            serde_json::from_str(&text).map_err(|err| HostPairingError::Decode {
                path: path.display().to_string(),
                reason: err.to_string(),
            })?;
        let mut hosts = BTreeMap::new();
        let mut endpoints = BTreeMap::<String, String>::new();
        for (raw_key, record) in file.hosts {
            let key = canonical_host_key(&raw_key)?;
            // Two spellings of one key must not both survive: whichever
            // row won would be arbitrary, and the loser would look like a
            // forgotten pin.
            if hosts.contains_key(&key) {
                return Err(HostPairingError::DuplicateHostKey { host_key_b64: key });
            }
            if let Some(pinned_host_key_b64) = endpoints.get(&record.endpoint)
                && pinned_host_key_b64 != &key
            {
                return Err(HostPairingError::EndpointIdentityChanged {
                    endpoint: record.endpoint,
                    pinned_host_key_b64: pinned_host_key_b64.clone(),
                    presented_host_key_b64: key,
                });
            }
            endpoints.insert(record.endpoint.clone(), key.clone());
            hosts.insert(key, record);
        }
        Ok(Self { path, hosts })
    }

    /// The file this store persists to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every pairing, host key first, in deterministic key order.
    pub fn hosts(&self) -> impl Iterator<Item = (&str, &HostPairingRecord)> {
        self.hosts
            .iter()
            .map(|(key, record)| (key.as_str(), record))
    }

    /// The pairing for one host key, if any.
    pub fn get(&self, host_key_b64: &str) -> Result<Option<&HostPairingRecord>, HostPairingError> {
        let key = canonical_host_key(host_key_b64)?;
        Ok(self.hosts.get(&key))
    }

    /// Commit a pairing from facts that were authenticated against the
    /// host's pinned key.
    ///
    /// Refuses with [`HostPairingError::EndpointIdentityChanged`] when a
    /// DIFFERENT key answers at an endpoint this store already pins. Note
    /// what that guard is and is not: it catches a re-keyed or
    /// impersonated host at a known address. It does not catch a host that
    /// legitimately moves address, which is allowed and bumps the pairing
    /// generation, because identity - not address - is the pin.
    ///
    /// In memory only. Call [`Self::save`] to make it durable.
    pub fn pair(
        &mut self,
        verified: &VerifiedHostFacts,
        now_unix_secs: u64,
    ) -> Result<&HostPairingRecord, HostPairingError> {
        self.pair_with_audience(verified, None, now_unix_secs)
    }

    /// Commit a pairing and its authenticated control audience.
    pub fn pair_with_audience(
        &mut self,
        verified: &VerifiedHostFacts,
        control_audience: Option<String>,
        now_unix_secs: u64,
    ) -> Result<&HostPairingRecord, HostPairingError> {
        let key = canonical_host_key(verified.host_key_b64())?;
        let endpoint = verified.dialed_endpoint().to_string();
        let conflict = self
            .hosts
            .iter()
            .find(|(existing, record)| record.endpoint == endpoint && *existing != &key)
            .map(|(existing, _)| existing.clone());
        if let Some(pinned_host_key_b64) = conflict {
            return Err(HostPairingError::EndpointIdentityChanged {
                endpoint,
                pinned_host_key_b64,
                presented_host_key_b64: key,
            });
        }
        let record = self.hosts.entry(key).or_insert_with(|| HostPairingRecord {
            endpoint: String::new(),
            host_label: String::new(),
            paired_at_unix_secs: now_unix_secs,
            pairing_generation: 0,
            control_audience: None,
            last_facts: None,
            last_health_status: None,
            last_seen_unix_secs: None,
        });
        record.endpoint = endpoint;
        record.host_label = verified.facts().host_label.clone();
        record.pairing_generation = record.pairing_generation.saturating_add(1);
        record.control_audience = control_audience;
        record.last_facts = Some(verified.facts().clone());
        record.last_seen_unix_secs = Some(now_unix_secs);
        Ok(record)
    }

    /// Refresh authenticated facts and health for an existing pairing.
    ///
    /// This never changes endpoint, identity, audience, or pairing generation.
    /// Reconnect is an observation accelerator, not authority to re-pair.
    pub fn record_authenticated_observation(
        &mut self,
        verified: &VerifiedHostFacts,
        health: &RuntimeHostHealth,
        now_unix_secs: u64,
    ) -> Result<(), HostPairingError> {
        let key = canonical_host_key(verified.host_key_b64())?;
        let Some(record) = self.hosts.get_mut(&key) else {
            return Err(HostPairingError::UnknownHost { host_key_b64: key });
        };
        if record.endpoint != verified.dialed_endpoint() {
            return Err(HostPairingError::EndpointChanged {
                host_key_b64: key,
                pinned_endpoint: record.endpoint.clone(),
                observed_endpoint: verified.dialed_endpoint().to_string(),
            });
        }
        record.host_label = verified.facts().host_label.clone();
        record.last_facts = Some(verified.facts().clone());
        record.last_health_status = Some(health.status);
        record.last_seen_unix_secs = Some(now_unix_secs);
        Ok(())
    }

    /// Record an authenticated health observation against an existing
    /// pairing. Never creates one: an unknown host is an error, so a
    /// health probe can never become an implicit pairing.
    pub fn record_health(
        &mut self,
        host_key_b64: &str,
        health: &RuntimeHostHealth,
        now_unix_secs: u64,
    ) -> Result<(), HostPairingError> {
        let key = canonical_host_key(host_key_b64)?;
        let Some(record) = self.hosts.get_mut(&key) else {
            return Err(HostPairingError::UnknownHost { host_key_b64: key });
        };
        record.last_health_status = Some(health.status);
        record.last_seen_unix_secs = Some(now_unix_secs);
        Ok(())
    }

    /// Drop a pairing. Returns whether one was present. This is the only
    /// way a pinned key is ever released.
    pub fn forget(&mut self, host_key_b64: &str) -> Result<bool, HostPairingError> {
        let key = canonical_host_key(host_key_b64)?;
        Ok(self.hosts.remove(&key).is_some())
    }

    /// Persist the store: write a sibling temp file, flush it to the
    /// device, then rename over the target.
    ///
    /// Scope of that guarantee, stated exactly: `rename` makes the
    /// REPLACEMENT atomic, so a reader never observes a half-written
    /// file. It does NOT by itself make the replacing CONTENT durable,
    /// which is why the flush is here and not optional - a crash that
    /// landed the rename with the temp file's bytes still in page cache
    /// would leave a zero-length pairing file, and [`Self::load`] refuses
    /// an undecodable file outright by design. Atomic-but-empty would
    /// therefore be a permanent startup refusal rather than a recoverable
    /// state.
    ///
    /// On Unix the parent directory is flushed after rename as well, so a
    /// successful first pairing cannot be acknowledged before the new
    /// directory entry is durable. Other targets retain atomic replacement
    /// and file-data flush but have no portable directory-sync primitive.
    pub fn save(&self) -> Result<(), HostPairingError> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|err| HostPairingError::Io {
                path: parent.display().to_string(),
                reason: err.to_string(),
            })?;
        }
        let file = HostPairingFileRef { hosts: &self.hosts };
        let text = serde_json::to_string_pretty(&file).map_err(|err| HostPairingError::Encode {
            reason: err.to_string(),
        })?;
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        write_and_flush(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, &self.path).map_err(|err| HostPairingError::Io {
            path: self.path.display().to_string(),
            reason: err.to_string(),
        })?;
        sync_parent_dir(&self.path)
    }
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> Result<(), HostPairingError> {
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Ok(());
    };
    let directory = std::fs::File::open(parent).map_err(|err| HostPairingError::Io {
        path: parent.display().to_string(),
        reason: err.to_string(),
    })?;
    directory.sync_all().map_err(|err| HostPairingError::Io {
        path: parent.display().to_string(),
        reason: err.to_string(),
    })
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> Result<(), HostPairingError> {
    Ok(())
}

/// Write `bytes` to `path` and flush them to the device before returning.
///
/// See [`HostPairingStore::save`] for why the flush is load-bearing here
/// rather than belt-and-braces.
fn write_and_flush(path: &Path, bytes: &[u8]) -> Result<(), HostPairingError> {
    use std::io::Write as _;
    let io_error = |err: &std::io::Error| HostPairingError::Io {
        path: path.display().to_string(),
        reason: err.to_string(),
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|err| io_error(&err))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|err| io_error(&err))?;
    }
    file.write_all(bytes).map_err(|err| io_error(&err))?;
    file.sync_all().map_err(|err| io_error(&err))
}

/// Normalize a host key to canonical standard base64, rejecting anything
/// that is not a usable Ed25519 public key.
///
/// Canonicalizing means `ed25519:AAA...` and `AAA...` name the same host
/// and cannot both be pinned - a store with two spellings of one key
/// would let a forgotten pin hide behind the other spelling.
fn canonical_host_key(text: &str) -> Result<String, HostPairingError> {
    let key = crate::auth::peer_keys::decode_pubkey_b64(text).map_err(|err| {
        HostPairingError::InvalidHostKey {
            reason: err.to_string(),
        }
    })?;
    Ok(base64_standard(&key))
}

// ---------------------------------------------------------------------
// Reconnect: a pure state machine over probe outcomes
// ---------------------------------------------------------------------

/// Backoff schedule for re-probing a runtime host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostReconnectPolicy {
    first_retry: Duration,
    max_retry: Duration,
    multiplier: u32,
    healthy_probe_interval: Duration,
}

impl HostReconnectPolicy {
    /// Build a policy. `multiplier` below 1 is clamped to 1 (a zero
    /// multiplier would collapse every backoff to zero and turn a failing
    /// host into a hot loop), and `max_retry` below `first_retry` is
    /// clamped up to it.
    ///
    /// The multiplier clamp is NOT the only way a policy could have
    /// scheduled no delay at all, so do not read it as one: the schedule
    /// is applied at whole-second resolution by [`HostReconnectState`],
    /// which rounds any non-zero delay UP (see `ceil_secs`) precisely so
    /// a sub-second `first_retry` cannot truncate away. A `first_retry`
    /// of EXACTLY zero still means "retry immediately" and is honoured as
    /// written - that is a policy, not an accident.
    pub fn new(
        first_retry: Duration,
        max_retry: Duration,
        multiplier: u32,
        healthy_probe_interval: Duration,
    ) -> Self {
        Self {
            first_retry,
            max_retry: max_retry.max(first_retry),
            multiplier: multiplier.max(1),
            healthy_probe_interval,
        }
    }

    /// Delay before the `consecutive_failures`-th retry. Zero failures
    /// means "probe now".
    pub fn backoff_after(&self, consecutive_failures: u32) -> Duration {
        if consecutive_failures == 0 {
            return Duration::ZERO;
        }
        let mut delay = self.first_retry;
        for _ in 1..consecutive_failures {
            delay = delay.saturating_mul(self.multiplier);
            if delay >= self.max_retry {
                return self.max_retry;
            }
        }
        delay.min(self.max_retry)
    }

    /// How long a reachable host may go unprobed.
    pub fn healthy_probe_interval(&self) -> Duration {
        self.healthy_probe_interval
    }
}

impl Default for HostReconnectPolicy {
    /// 1s first retry, doubling to a 60s ceiling, and a 30s cadence while
    /// the host is answering.
    fn default() -> Self {
        Self::new(
            Duration::from_secs(1),
            Duration::from_mins(1),
            2,
            Duration::from_secs(30),
        )
    }
}

/// Whole seconds a [`Duration`] costs at the resolution [`probe_due`]
/// actually compares in.
///
/// [`HostReconnectState`] schedules in unix SECONDS, so a plain
/// `Duration::as_secs` would TRUNCATE: a policy written in milliseconds
/// would yield `retry_after_unix_secs == now`, making `probe_due`
/// permanently true - a hot probe loop wearing a configured backoff. Any
/// non-zero delay therefore rounds UP to one second, which is the only
/// conversion at this resolution that keeps "a backoff happened" true.
/// Exactly zero stays zero: that is an operator asking for no backoff,
/// not a rounding artefact.
///
/// [`probe_due`]: HostReconnectState::probe_due
fn ceil_secs(delay: Duration) -> u64 {
    delay
        .as_secs()
        .saturating_add(u64::from(delay.subsec_nanos() > 0))
}

/// What the last probe proved about a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostReachability {
    /// Never probed since this process started. Durable pairing state is
    /// NOT reachability: a paired host is `Unknown` until this process
    /// authenticates an answer from it.
    Unknown,
    /// An authenticated answer arrived.
    Reachable {
        observed_at_unix_secs: u64,
        status: RuntimeHostHealthStatus,
    },
    /// A probe failed. Carries the retry schedule position.
    Unreachable {
        consecutive_failures: u32,
        observed_at_unix_secs: u64,
        retry_after_unix_secs: u64,
    },
}

/// Reconnect state for one host. Pure: it observes outcomes and answers
/// "is a probe due", and performs no I/O, so its behaviour is testable
/// without a socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostReconnectState {
    policy: HostReconnectPolicy,
    reachability: HostReachability,
}

impl HostReconnectState {
    /// A never-probed host under `policy`.
    pub fn new(policy: HostReconnectPolicy) -> Self {
        Self {
            policy,
            reachability: HostReachability::Unknown,
        }
    }

    /// Current reachability.
    pub fn reachability(&self) -> &HostReachability {
        &self.reachability
    }

    /// Consecutive failed probes; zero whenever the last probe succeeded.
    pub fn consecutive_failures(&self) -> u32 {
        match &self.reachability {
            HostReachability::Unreachable {
                consecutive_failures,
                ..
            } => *consecutive_failures,
            HostReachability::Unknown | HostReachability::Reachable { .. } => 0,
        }
    }

    /// Record an authenticated answer. Resets the backoff.
    pub fn observe_reachable(&mut self, status: RuntimeHostHealthStatus, now_unix_secs: u64) {
        self.reachability = HostReachability::Reachable {
            observed_at_unix_secs: now_unix_secs,
            status,
        };
    }

    /// Record a failed probe.
    ///
    /// A DEGRADED-but-answering host is reachable, not failed: only an
    /// unauthenticated or undelivered answer belongs here. Feeding a
    /// degraded health report in as a failure would back a host off for
    /// telling the truth about itself.
    pub fn observe_unreachable(&mut self, now_unix_secs: u64) {
        let consecutive_failures = self.consecutive_failures().saturating_add(1);
        let delay = self.policy.backoff_after(consecutive_failures);
        self.reachability = HostReachability::Unreachable {
            consecutive_failures,
            observed_at_unix_secs: now_unix_secs,
            retry_after_unix_secs: now_unix_secs.saturating_add(ceil_secs(delay)),
        };
    }

    /// Whether a probe is due at `now_unix_secs`.
    pub fn probe_due(&self, now_unix_secs: u64) -> bool {
        match &self.reachability {
            HostReachability::Unknown => true,
            HostReachability::Reachable {
                observed_at_unix_secs,
                ..
            } => {
                now_unix_secs
                    >= observed_at_unix_secs
                        .saturating_add(ceil_secs(self.policy.healthy_probe_interval()))
            }
            HostReachability::Unreachable {
                retry_after_unix_secs,
                ..
            } => now_unix_secs >= *retry_after_unix_secs,
        }
    }
}

/// Wall-clock helper for callers that do not already carry a clock. Every
/// state transition above takes `now` explicitly so tests never need it.
pub fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

// ---------------------------------------------------------------------
// Production lifecycle controller
// ---------------------------------------------------------------------

/// Typed lifecycle failure. Placement refusals are deliberately errors, not
/// `None`: once a caller selected a remote host, no layer may reinterpret an
/// unavailable host as permission to materialize locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteHostLifecycleError {
    Pairing(HostPairingError),
    Remote(RemoteHostError),
    UnknownContact { contact: String },
    InProcessContact { contact: String },
    MissingPinnedIdentity { contact: String },
    UnknownHost { host_key_b64: String },
    NotAuthenticatedThisBoot { host_key_b64: String },
    Unreachable { host_key_b64: String },
    Unhealthy { host_key_b64: String },
    PlacementCapabilityMissing { host_key_b64: String },
}

impl std::fmt::Display for RemoteHostLifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pairing(error) => write!(f, "runtime-host pairing: {error}"),
            Self::Remote(error) => write!(f, "runtime-host probe: {error}"),
            Self::UnknownContact { contact } => {
                write!(f, "no remote contact named '{contact}' is configured")
            }
            Self::InProcessContact { contact } => write!(
                f,
                "contact '{contact}' is in-process and cannot be paired as remote placement infrastructure"
            ),
            Self::MissingPinnedIdentity { contact } => write!(
                f,
                "contact '{contact}' has no pinned Ed25519 identity; refusing remote-host pairing"
            ),
            Self::UnknownHost { host_key_b64 } => {
                write!(f, "runtime host '{host_key_b64}' is not durably paired")
            }
            Self::NotAuthenticatedThisBoot { host_key_b64 } => write!(
                f,
                "runtime host '{host_key_b64}' has not authenticated an endpoint response since this controller started"
            ),
            Self::Unreachable { host_key_b64 } => {
                write!(f, "runtime host '{host_key_b64}' is unreachable")
            }
            Self::Unhealthy { host_key_b64 } => {
                write!(f, "runtime host '{host_key_b64}' reports unhealthy")
            }
            Self::PlacementCapabilityMissing { host_key_b64 } => write!(
                f,
                "runtime host '{host_key_b64}' does not advertise multi-host placement"
            ),
        }
    }
}

impl std::error::Error for RemoteHostLifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pairing(error) => Some(error),
            Self::Remote(error) => Some(error),
            _ => None,
        }
    }
}

impl From<HostPairingError> for RemoteHostLifecycleError {
    fn from(value: HostPairingError) -> Self {
        Self::Pairing(value)
    }
}

impl From<RemoteHostError> for RemoteHostLifecycleError {
    fn from(value: RemoteHostError) -> Self {
        Self::Remote(value)
    }
}

/// Exact placement carrier derived from a durably paired, freshly
/// authenticated host key. It is a projection only: Meerkat's host binding
/// authority still decides whether this host is bound and may materialize a
/// member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHostPlacement {
    pub host: meerkat_contracts::WireHostRef,
    pub host_key_b64: String,
    pub pairing_generation: u64,
}

/// Result of one due reconnect probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHostProbeOutcome {
    pub host_key_b64: String,
    pub result: Result<RuntimeHostHealthStatus, RemoteHostLifecycleError>,
}

/// Runtime-owned controller for durable host identity pins and transient
/// reachability acceleration.
///
/// The pairing store is authority for endpoint identity only. Reachability is
/// intentionally reset to `Unknown` on every construction and must be rebuilt
/// from authenticated responses before [`Self::placement`] succeeds.
pub struct RemoteHostLifecycle {
    store: tokio::sync::Mutex<HostPairingStore>,
    reconnect: tokio::sync::Mutex<BTreeMap<String, HostReconnectState>>,
    policy: HostReconnectPolicy,
}

impl RemoteHostLifecycle {
    pub fn load(
        path: impl Into<PathBuf>,
        policy: HostReconnectPolicy,
    ) -> Result<Self, HostPairingError> {
        let store = HostPairingStore::load(path)?;
        let reconnect = store
            .hosts()
            .map(|(key, _)| (key.to_string(), HostReconnectState::new(policy)))
            .collect();
        Ok(Self {
            store: tokio::sync::Mutex::new(store),
            reconnect: tokio::sync::Mutex::new(reconnect),
            policy,
        })
    }

    pub async fn contains(&self, host_key_b64: &str) -> Result<bool, HostPairingError> {
        let canonical = canonical_host_key(host_key_b64)?;
        Ok(self.store.lock().await.get(&canonical)?.is_some())
    }

    pub async fn pair_contact(
        &self,
        contact: &crate::contact_directory::ContactEntry,
        caller_keys: Option<std::sync::Arc<crate::auth::peer_keys::GatewayPeerKeys>>,
        now_unix_secs: u64,
    ) -> Result<RuntimeHostPlacement, RemoteHostLifecycleError> {
        let host_key =
            contact
                .pubkey
                .ok_or_else(|| RemoteHostLifecycleError::MissingPinnedIdentity {
                    contact: contact.mob_id.clone(),
                })?;
        let endpoint = match &contact.transport {
            crate::contact_directory::MobTransport::Tcp(address) => {
                RemoteEndpoint::Tcp(address.clone())
            }
            crate::contact_directory::MobTransport::Uds(path) => RemoteEndpoint::Uds(path.clone()),
            crate::contact_directory::MobTransport::Inproc => {
                return Err(RemoteHostLifecycleError::InProcessContact {
                    contact: contact.mob_id.clone(),
                });
            }
        };
        let mut client =
            RemoteHostClient::new(endpoint, host_key).with_audience(contact.mob_id.clone());
        if let Some(keys) = caller_keys {
            client = client.with_caller_keys(keys);
        }
        let verified = client.describe().await?;
        let health = client.health().await?;

        // Clone-mutate-save-swap: a failed durable write cannot leave the
        // process believing it owns a pairing that restart would forget.
        let mut store = self.store.lock().await;
        let mut replacement = store.clone();
        replacement.pair_with_audience(&verified, Some(contact.mob_id.clone()), now_unix_secs)?;
        replacement.record_health(verified.host_key_b64(), &health, now_unix_secs)?;
        replacement.save()?;
        *store = replacement;
        drop(store);

        self.reconnect
            .lock()
            .await
            .entry(verified.host_key_b64().to_string())
            .or_insert_with(|| HostReconnectState::new(self.policy))
            .observe_reachable(health.status, now_unix_secs);
        self.placement(verified.host_key_b64()).await
    }

    pub async fn refresh(
        &self,
        host_key_b64: &str,
        caller_keys: Option<std::sync::Arc<crate::auth::peer_keys::GatewayPeerKeys>>,
        now_unix_secs: u64,
    ) -> Result<RuntimeHostHealthStatus, RemoteHostLifecycleError> {
        let canonical = canonical_host_key(host_key_b64)?;
        let record = self
            .store
            .lock()
            .await
            .get(&canonical)?
            .cloned()
            .ok_or_else(|| RemoteHostLifecycleError::UnknownHost {
                host_key_b64: canonical.clone(),
            })?;
        let endpoint = endpoint_from_address(&record.endpoint).map_err(|message| {
            RemoteHostLifecycleError::Remote(RemoteHostError::Decode {
                endpoint: record.endpoint.clone(),
                message,
            })
        })?;
        let mut client = RemoteHostClient::from_pubkey_b64(endpoint, &canonical)?;
        if let Some(audience) = record.control_audience.as_ref() {
            client = client.with_audience(audience.clone());
        }
        if let Some(keys) = caller_keys {
            client = client.with_caller_keys(keys);
        }
        let observation = async {
            let verified = client.describe().await?;
            let health = client.health().await?;
            Ok::<_, RemoteHostError>((verified, health))
        }
        .await;
        match observation {
            Ok((verified, health)) => {
                let mut store = self.store.lock().await;
                let mut replacement = store.clone();
                replacement.record_authenticated_observation(&verified, &health, now_unix_secs)?;
                replacement.save()?;
                *store = replacement;
                drop(store);
                self.reconnect
                    .lock()
                    .await
                    .entry(canonical)
                    .or_insert_with(|| HostReconnectState::new(self.policy))
                    .observe_reachable(health.status, now_unix_secs);
                Ok(health.status)
            }
            Err(error) => {
                self.reconnect
                    .lock()
                    .await
                    .entry(canonical)
                    .or_insert_with(|| HostReconnectState::new(self.policy))
                    .observe_unreachable(now_unix_secs);
                Err(error.into())
            }
        }
    }

    pub async fn probe_due(
        &self,
        caller_keys: Option<std::sync::Arc<crate::auth::peer_keys::GatewayPeerKeys>>,
        now_unix_secs: u64,
    ) -> Vec<RuntimeHostProbeOutcome> {
        let due: Vec<String> = self
            .reconnect
            .lock()
            .await
            .iter()
            .filter(|(_, state)| state.probe_due(now_unix_secs))
            .map(|(key, _)| key.clone())
            .collect();
        let mut outcomes = Vec::with_capacity(due.len());
        for host_key_b64 in due {
            let result = self
                .refresh(&host_key_b64, caller_keys.clone(), now_unix_secs)
                .await;
            outcomes.push(RuntimeHostProbeOutcome {
                host_key_b64,
                result,
            });
        }
        outcomes
    }

    pub async fn placement(
        &self,
        host_key_b64: &str,
    ) -> Result<RuntimeHostPlacement, RemoteHostLifecycleError> {
        let canonical = canonical_host_key(host_key_b64)?;
        let record = self
            .store
            .lock()
            .await
            .get(&canonical)?
            .cloned()
            .ok_or_else(|| RemoteHostLifecycleError::UnknownHost {
                host_key_b64: canonical.clone(),
            })?;
        let reachability = self
            .reconnect
            .lock()
            .await
            .get(&canonical)
            .map(|state| state.reachability().clone())
            .unwrap_or(HostReachability::Unknown);
        match reachability {
            HostReachability::Unknown => {
                return Err(RemoteHostLifecycleError::NotAuthenticatedThisBoot {
                    host_key_b64: canonical,
                });
            }
            HostReachability::Unreachable { .. } => {
                return Err(RemoteHostLifecycleError::Unreachable {
                    host_key_b64: canonical,
                });
            }
            HostReachability::Reachable {
                status: RuntimeHostHealthStatus::Unhealthy,
                ..
            } => {
                return Err(RemoteHostLifecycleError::Unhealthy {
                    host_key_b64: canonical,
                });
            }
            HostReachability::Reachable { .. } => {}
        }
        let facts = record.last_facts.as_ref().ok_or_else(|| {
            RemoteHostLifecycleError::NotAuthenticatedThisBoot {
                host_key_b64: canonical.clone(),
            }
        })?;
        if !facts.capabilities.features.multi_host_mobs {
            return Err(RemoteHostLifecycleError::PlacementCapabilityMissing {
                host_key_b64: canonical,
            });
        }
        let key = crate::auth::peer_keys::decode_pubkey_b64(&canonical).map_err(|error| {
            HostPairingError::InvalidHostKey {
                reason: error.to_string(),
            }
        })?;
        let host = meerkat_comms::PubKey::new(key).to_peer_id().to_string();
        Ok(RuntimeHostPlacement {
            host: meerkat_contracts::WireHostRef(host),
            host_key_b64: canonical,
            pairing_generation: record.pairing_generation,
        })
    }
}

fn endpoint_from_address(address: &str) -> Result<RemoteEndpoint, String> {
    if let Some(value) = address.strip_prefix("tcp://") {
        return Ok(RemoteEndpoint::Tcp(value.to_string()));
    }
    if let Some(value) = address.strip_prefix("uds://") {
        return Ok(RemoteEndpoint::Uds(value.to_string()));
    }
    Err(format!(
        "paired runtime-host endpoint '{address}' has no supported tcp:// or uds:// scheme"
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::auth::peer_keys::GatewayPeerKeys;
    use crate::runtime::cross_mob_control::{
        BoundControlListener, ControlHandler, ControlListenAddr,
    };
    use meerkat_contracts::ContractVersion;

    struct HostOnlyHandler {
        facts: HostFacts,
        health: RuntimeHostHealth,
    }

    impl ControlHandler for HostOnlyHandler {
        fn handle(
            &self,
            request: ControlRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>>
        {
            Box::pin(async move {
                match request {
                    ControlRequest::HostDescribe { .. } => ControlResponse::Host {
                        facts: self.facts.clone(),
                        sig_b64: None,
                    },
                    ControlRequest::HostHealth { .. } => ControlResponse::HostHealth {
                        health: self.health.clone(),
                        sig_b64: None,
                    },
                    _ => ControlResponse::Err {
                        code: "host_only".to_string(),
                        message: "test host serves only the read-only host plane".to_string(),
                    },
                }
            })
        }
    }

    fn flags() -> RuntimeHostFeatureFlags {
        RuntimeHostFeatureFlags {
            runtime_backed_sessions: true,
            mobs: true,
            mcp_live: false,
            comms: true,
            blobs: false,
            session_events: true,
            session_streams: true,
            schedules: false,
            skills: false,
            event_replay: false,
            artifacts: false,
            approvals: false,
            external_members: false,
            secure_remote_rpc: false,
            multi_host_mobs: false,
            durable_jobs: false,
        }
    }

    fn capabilities() -> RuntimeHostCapabilities {
        RuntimeHostCapabilities {
            contract_version: ContractVersion::CURRENT,
            features: flags(),
        }
    }

    fn facts_for(keys: &GatewayPeerKeys) -> HostFacts {
        HostFacts::new(
            "worker-host-a",
            keys.pubkey_b64(),
            "tcp://10.0.0.7:7801",
            capabilities(),
        )
    }

    fn verified_for(keys: &GatewayPeerKeys, endpoint: &str) -> VerifiedHostFacts {
        VerifiedHostFacts {
            host_key_b64: keys.pubkey_b64(),
            dialed_endpoint: endpoint.to_string(),
            facts: facts_for(keys),
        }
    }

    fn health(status: RuntimeHostHealthStatus) -> RuntimeHostHealth {
        RuntimeHostHealth {
            contract_version: ContractVersion::CURRENT,
            status,
            checks: BTreeMap::new(),
        }
    }

    /// POSITIVE CONTROL for every digest test below: two independently
    /// built copies of the same facts must digest identically, otherwise a
    /// "digest changed" assertion proves nothing about tampering.
    #[test]
    fn host_facts_digest_is_stable_for_equal_facts() {
        let keys = GatewayPeerKeys::ephemeral();
        assert_eq!(
            facts_for(&keys).signing_digest_hex(),
            facts_for(&keys).signing_digest_hex()
        );
    }

    #[test]
    fn host_facts_digest_changes_when_any_field_is_tampered() {
        let keys = GatewayPeerKeys::ephemeral();
        let baseline = facts_for(&keys).signing_digest_hex();

        let mut label = facts_for(&keys);
        label.host_label = "worker-host-b".to_string();
        assert_ne!(baseline, label.signing_digest_hex(), "host_label");

        let mut address = facts_for(&keys);
        address.advertised_control_address = "tcp://10.0.0.8:7801".to_string();
        assert_ne!(baseline, address.signing_digest_hex(), "advertised address");

        let mut capability = facts_for(&keys);
        capability.capabilities.features.multi_host_mobs = true;
        assert_ne!(baseline, capability.signing_digest_hex(), "capability flag");

        let mut endpoints = facts_for(&keys);
        endpoints.endpoints.rest_base_url = Some("http://10.0.0.7:8080".to_string());
        assert_ne!(baseline, endpoints.signing_digest_hex(), "endpoints");

        let mut labels = facts_for(&keys);
        labels
            .placement_labels
            .insert("zone".to_string(), "eu-west".to_string());
        assert_ne!(baseline, labels.signing_digest_hex(), "placement labels");
    }

    /// Length-prefixed framing must make newline injection inside an
    /// operator-supplied label unable to forge a different label set that
    /// digests the same.
    #[test]
    fn placement_labels_cannot_be_forged_by_newline_injection() {
        let keys = GatewayPeerKeys::ephemeral();

        let mut injected = facts_for(&keys);
        injected
            .placement_labels
            .insert("zone".to_string(), "eu-west\ntier\nprod".to_string());

        let mut split = facts_for(&keys);
        split
            .placement_labels
            .insert("zone".to_string(), "eu-west".to_string());
        split
            .placement_labels
            .insert("tier".to_string(), "prod".to_string());

        assert_ne!(
            injected.signing_digest_hex(),
            split.signing_digest_hex(),
            "a newline inside one label value must not digest as two labels"
        );
    }

    #[test]
    fn absent_endpoint_field_does_not_digest_as_empty_string() {
        let keys = GatewayPeerKeys::ephemeral();
        let mut absent = facts_for(&keys);
        absent.endpoints.rpc_transport = None;
        let mut empty = facts_for(&keys);
        empty.endpoints.rpc_transport = Some(String::new());
        assert_ne!(absent.signing_digest_hex(), empty.signing_digest_hex());
    }

    #[test]
    fn health_digest_separates_status_and_check_names() {
        let ok = health(RuntimeHostHealthStatus::Ok);
        assert_eq!(host_health_digest_hex(&ok), host_health_digest_hex(&ok));
        let degraded = health(RuntimeHostHealthStatus::Degraded);
        assert_ne!(
            host_health_digest_hex(&ok),
            host_health_digest_hex(&degraded)
        );

        let mut checked = health(RuntimeHostHealthStatus::Ok);
        checked
            .checks
            .insert("jobs".to_string(), RuntimeHostHealthStatus::Degraded);
        assert_ne!(
            host_health_digest_hex(&ok),
            host_health_digest_hex(&checked)
        );
    }

    /// A facts digest must never equal a health digest, even if both were
    /// somehow built over the same bytes: distinct domain contexts.
    #[test]
    fn facts_and_health_digests_are_domain_separated() {
        let keys = GatewayPeerKeys::ephemeral();
        assert_ne!(
            facts_for(&keys).signing_digest_hex(),
            host_health_digest_hex(&health(RuntimeHostHealthStatus::Ok))
        );
    }

    #[test]
    fn pairing_pins_the_host_key_and_refuses_a_new_key_at_a_pinned_endpoint() {
        let dir = std::env::temp_dir().join(format!("meta-pairing-{}", uuid::Uuid::new_v4()));
        let path = dir.join("hosts.json");
        let mut store = HostPairingStore::load(&path).expect("empty store loads");

        let host_a = GatewayPeerKeys::ephemeral();
        let verified = verified_for(&host_a, "tcp://10.0.0.7:7801");
        let record = store.pair(&verified, 1_000).expect("first pairing commits");
        assert_eq!(record.pairing_generation, 1);
        assert_eq!(record.endpoint, "tcp://10.0.0.7:7801");

        // POSITIVE CONTROL: the same key re-pairing at the same endpoint
        // is accepted, so the refusal below is about identity change and
        // not about pairing being broken outright.
        let again = store.pair(&verified, 1_100).expect("rebind commits");
        assert_eq!(again.pairing_generation, 2);

        let host_b = GatewayPeerKeys::ephemeral();
        let impostor = verified_for(&host_b, "tcp://10.0.0.7:7801");
        let refused = store.pair(&impostor, 1_200).expect_err("identity change");
        assert!(matches!(
            refused,
            HostPairingError::EndpointIdentityChanged { .. }
        ));
        assert_eq!(store.hosts().count(), 1, "impostor must not be recorded");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_paired_host_may_move_address() {
        let dir = std::env::temp_dir().join(format!("meta-pairing-{}", uuid::Uuid::new_v4()));
        let path = dir.join("hosts.json");
        let mut store = HostPairingStore::load(&path).expect("empty store loads");
        let host = GatewayPeerKeys::ephemeral();
        store
            .pair(&verified_for(&host, "tcp://10.0.0.7:7801"), 1)
            .expect("initial pairing");
        let moved = store
            .pair(&verified_for(&host, "tcp://10.0.0.9:7801"), 2)
            .expect("same identity at a new address");
        assert_eq!(moved.endpoint, "tcp://10.0.0.9:7801");
        assert_eq!(moved.pairing_generation, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pairings_round_trip_through_the_file_and_survive_key_spelling() {
        let dir = std::env::temp_dir().join(format!("meta-pairing-{}", uuid::Uuid::new_v4()));
        let path = dir.join("hosts.json");
        let host = GatewayPeerKeys::ephemeral();
        {
            let mut store = HostPairingStore::load(&path).expect("empty store loads");
            store
                .pair(&verified_for(&host, "tcp://10.0.0.7:7801"), 42)
                .expect("pairing");
            store
                .record_health(&host.pubkey_b64(), &health(RuntimeHostHealthStatus::Ok), 43)
                .expect("health observation");
            store.save().expect("persist");
        }
        let reloaded = HostPairingStore::load(&path).expect("reload");
        let prefixed = format!("ed25519:{}", host.pubkey_b64());
        let record = reloaded
            .get(&prefixed)
            .expect("canonical lookup")
            .expect("the ed25519:-prefixed spelling names the same host");
        assert_eq!(record.paired_at_unix_secs, 42);
        assert_eq!(record.last_seen_unix_secs, Some(43));
        assert_eq!(record.last_health_status, Some(RuntimeHostHealthStatus::Ok));
        assert!(record.last_facts.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pairing_restart_preserves_endpoint_identity_and_control_audience() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosts.json");
        let host = GatewayPeerKeys::ephemeral();
        let mut store = HostPairingStore::load(&path).expect("empty store");
        store
            .pair_with_audience(
                &verified_for(&host, "tcp://10.0.0.7:7801"),
                Some("worker-contact".to_string()),
                41,
            )
            .expect("pair");
        store.save().expect("save");

        let reloaded = HostPairingStore::load(&path).expect("reload");
        let record = reloaded
            .get(&host.pubkey_b64())
            .expect("valid key")
            .expect("pairing survives restart");
        assert_eq!(record.endpoint, "tcp://10.0.0.7:7801");
        assert_eq!(record.control_audience.as_deref(), Some("worker-contact"));
        assert_eq!(record.pairing_generation, 1);
    }

    #[test]
    fn reconnect_observation_cannot_move_a_durable_endpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = GatewayPeerKeys::ephemeral();
        let mut store = HostPairingStore::load(dir.path().join("hosts.json")).expect("store");
        store
            .pair_with_audience(
                &verified_for(&host, "tcp://10.0.0.7:7801"),
                Some("worker-contact".to_string()),
                41,
            )
            .expect("pair");
        let before = store
            .get(&host.pubkey_b64())
            .expect("valid key")
            .expect("record")
            .clone();

        let error = store
            .record_authenticated_observation(
                &verified_for(&host, "tcp://10.0.0.9:7801"),
                &health(RuntimeHostHealthStatus::Ok),
                99,
            )
            .expect_err("probe cannot re-pair");
        assert!(matches!(error, HostPairingError::EndpointChanged { .. }));
        assert_eq!(
            store
                .get(&host.pubkey_b64())
                .expect("valid key")
                .expect("record"),
            &before,
            "a refused probe must not partially mutate the pin"
        );
    }

    #[cfg(unix)]
    #[test]
    fn pairing_file_is_replaced_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosts.json");
        let tmp_path = dir.path().join("hosts.json.tmp");
        std::fs::write(&tmp_path, "{}").expect("seed stale temp target");
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o644))
            .expect("make positive-control stale temp permissive");
        let host = GatewayPeerKeys::ephemeral();
        let mut store = HostPairingStore::load(&path).expect("store");
        store
            .pair(&verified_for(&host, "tcp://10.0.0.7:7801"), 1)
            .expect("pair");
        store.save().expect("save");

        let mode = std::fs::metadata(&path)
            .expect("pairing metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "pairing file must be owner-only");
    }

    #[test]
    fn an_undecodable_pairing_file_is_refused_rather_than_forgotten() {
        let dir = std::env::temp_dir().join(format!("meta-pairing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("hosts.json");
        std::fs::write(&path, "{ not json").expect("write corrupt file");
        let err = HostPairingStore::load(&path).expect_err("corrupt store must not load empty");
        assert!(matches!(err, HostPairingError::Decode { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pairing_file_cannot_pin_two_identities_to_one_endpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosts.json");
        let host_a = GatewayPeerKeys::ephemeral();
        let host_b = GatewayPeerKeys::ephemeral();
        let mut first = HostPairingStore::load(dir.path().join("first.json")).expect("store");
        let record_a = first
            .pair(&verified_for(&host_a, "tcp://10.0.0.7:7801"), 1)
            .expect("pair a")
            .clone();
        let mut second = HostPairingStore::load(dir.path().join("second.json")).expect("store");
        let record_b = second
            .pair(&verified_for(&host_b, "tcp://10.0.0.7:7801"), 1)
            .expect("pair b in independent store")
            .clone();
        let hosts = BTreeMap::from([
            (host_a.pubkey_b64(), record_a),
            (host_b.pubkey_b64(), record_b),
        ]);
        std::fs::write(
            &path,
            serde_json::to_vec(&HostPairingFileRef { hosts: &hosts }).expect("encode fixture"),
        )
        .expect("write fixture");

        assert!(matches!(
            HostPairingStore::load(&path),
            Err(HostPairingError::EndpointIdentityChanged { .. })
        ));
    }

    #[test]
    fn health_observation_never_creates_a_pairing() {
        let dir = std::env::temp_dir().join(format!("meta-pairing-{}", uuid::Uuid::new_v4()));
        let mut store = HostPairingStore::load(dir.join("hosts.json")).expect("empty store");
        let host = GatewayPeerKeys::ephemeral();
        let err = store
            .record_health(&host.pubkey_b64(), &health(RuntimeHostHealthStatus::Ok), 1)
            .expect_err("unknown host");
        assert!(matches!(err, HostPairingError::UnknownHost { .. }));
        assert_eq!(store.hosts().count(), 0);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let policy = HostReconnectPolicy::new(
            Duration::from_secs(2),
            Duration::from_secs(16),
            2,
            Duration::from_secs(30),
        );
        assert_eq!(policy.backoff_after(0), Duration::ZERO);
        assert_eq!(policy.backoff_after(1), Duration::from_secs(2));
        assert_eq!(policy.backoff_after(2), Duration::from_secs(4));
        assert_eq!(policy.backoff_after(3), Duration::from_secs(8));
        assert_eq!(policy.backoff_after(4), Duration::from_secs(16));
        assert_eq!(policy.backoff_after(50), Duration::from_secs(16));
    }

    #[test]
    fn a_zero_multiplier_cannot_produce_a_hot_retry_loop() {
        let policy = HostReconnectPolicy::new(
            Duration::from_secs(5),
            Duration::from_secs(5),
            0,
            Duration::from_secs(30),
        );
        assert_eq!(policy.backoff_after(1), Duration::from_secs(5));
        assert_eq!(policy.backoff_after(9), Duration::from_secs(5));
    }

    #[test]
    fn reconnect_state_backs_off_then_resets_on_success() {
        let policy = HostReconnectPolicy::new(
            Duration::from_secs(2),
            Duration::from_secs(8),
            2,
            Duration::from_secs(30),
        );
        let mut state = HostReconnectState::new(policy);
        assert!(state.probe_due(0), "a never-probed host is due immediately");

        state.observe_unreachable(100);
        assert_eq!(state.consecutive_failures(), 1);
        assert!(!state.probe_due(101));
        assert!(state.probe_due(102));

        state.observe_unreachable(102);
        assert_eq!(state.consecutive_failures(), 2);
        assert!(!state.probe_due(105));
        assert!(state.probe_due(106));

        state.observe_reachable(RuntimeHostHealthStatus::Ok, 106);
        assert_eq!(state.consecutive_failures(), 0);
        assert!(!state.probe_due(120), "a reachable host waits the cadence");
        assert!(state.probe_due(136));
    }

    /// A backoff shorter than the scheduler's resolution must still cost
    /// a whole tick.
    ///
    /// Truncating it (`Duration::as_secs` on 500ms is 0) would set
    /// `retry_after_unix_secs == now` and make `probe_due` permanently
    /// true - a hot probe loop wearing a configured backoff, which is the
    /// exact failure the multiplier clamp is written to prevent on its
    /// own axis. POSITIVE CONTROL: the second half asserts the same
    /// rounding for the healthy cadence, so a `probe_due` that simply
    /// answered `false` for everything would fail here.
    #[test]
    fn a_sub_second_schedule_is_rounded_up_not_truncated_away() {
        let policy = HostReconnectPolicy::new(
            Duration::from_millis(500),
            Duration::from_millis(500),
            2,
            Duration::from_millis(500),
        );
        let mut state = HostReconnectState::new(policy);

        state.observe_unreachable(100);
        assert!(!state.probe_due(100), "a configured backoff must delay");
        assert!(state.probe_due(101));

        state.observe_reachable(RuntimeHostHealthStatus::Ok, 200);
        assert!(!state.probe_due(200), "a configured cadence must delay");
        assert!(state.probe_due(201));
    }

    /// A degraded host is still answering. Backing it off would punish it
    /// for reporting honestly, and would hide the degradation behind a
    /// reachability failure.
    #[test]
    fn a_degraded_but_answering_host_stays_reachable() {
        let mut state = HostReconnectState::new(HostReconnectPolicy::default());
        state.observe_reachable(RuntimeHostHealthStatus::Degraded, 10);
        assert_eq!(state.consecutive_failures(), 0);
        assert!(matches!(
            state.reachability(),
            HostReachability::Reachable {
                status: RuntimeHostHealthStatus::Degraded,
                ..
            }
        ));
    }

    /// Durable pairing is not reachability: reloading a store must not
    /// make a host look live.
    #[test]
    fn a_freshly_loaded_pairing_is_not_reachable() {
        let state = HostReconnectState::new(HostReconnectPolicy::default());
        assert_eq!(state.reachability(), &HostReachability::Unknown);
    }

    #[tokio::test]
    async fn lifecycle_restart_requires_fresh_authenticated_reconnect_before_placement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosts.json");
        let host = GatewayPeerKeys::ephemeral();
        let mut verified = verified_for(&host, "tcp://10.0.0.7:7801");
        verified.facts.capabilities.features.multi_host_mobs = true;
        let mut store = HostPairingStore::load(&path).expect("store");
        store
            .pair_with_audience(&verified, Some("worker-contact".to_string()), 1)
            .expect("pair");
        store
            .record_health(&host.pubkey_b64(), &health(RuntimeHostHealthStatus::Ok), 2)
            .expect("health");
        store.save().expect("save");

        let lifecycle =
            RemoteHostLifecycle::load(&path, HostReconnectPolicy::default()).expect("restart load");
        let error = lifecycle
            .placement(&host.pubkey_b64())
            .await
            .expect_err("durable cached health is not live authentication");
        assert!(matches!(
            error,
            RemoteHostLifecycleError::NotAuthenticatedThisBoot { .. }
        ));
    }

    #[tokio::test]
    async fn lifecycle_pairs_an_authenticated_endpoint_and_persists_it() {
        let host = GatewayPeerKeys::ephemeral();
        let mut facts = facts_for(&host);
        facts.capabilities.features.multi_host_mobs = true;
        let listener = BoundControlListener::bind(
            &ControlListenAddr::parse("tcp://127.0.0.1:0").expect("listen address"),
        )
        .await
        .expect("bind host");
        let advertised = listener.advertised_address().to_string();
        let dial = advertised
            .strip_prefix("tcp://")
            .expect("tcp address")
            .to_string();
        let signer = std::sync::Arc::new(std::sync::RwLock::new(Some(std::sync::Arc::new(
            host.clone(),
        ))));
        let handler: std::sync::Arc<dyn ControlHandler> = std::sync::Arc::new(HostOnlyHandler {
            facts,
            health: health(RuntimeHostHealthStatus::Ok),
        });
        let server = tokio::spawn(listener.serve(handler, signer));

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosts.json");
        let lifecycle =
            RemoteHostLifecycle::load(&path, HostReconnectPolicy::default()).expect("lifecycle");
        let contact = crate::contact_directory::ContactEntry {
            mob_id: "worker-contact".to_string(),
            transport: crate::contact_directory::MobTransport::Tcp(dial),
            pubkey: Some(host.pubkey_bytes()),
            require_signed_control: None,
        };
        let placement = lifecycle
            .pair_contact(&contact, None, 17)
            .await
            .expect("signed describe and health pair the endpoint");
        assert_eq!(
            placement.host.0,
            meerkat_comms::PubKey::new(host.pubkey_bytes())
                .to_peer_id()
                .to_string()
        );

        let reloaded = HostPairingStore::load(&path).expect("durable reload");
        let record = reloaded
            .get(&host.pubkey_b64())
            .expect("valid key")
            .expect("pairing committed");
        assert_eq!(record.endpoint, advertised);
        assert_eq!(record.control_audience.as_deref(), Some("worker-contact"));
        assert_eq!(record.last_health_status, Some(RuntimeHostHealthStatus::Ok));

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn lifecycle_placement_is_exact_and_fail_closed_by_health_and_capability() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosts.json");
        let host = GatewayPeerKeys::ephemeral();
        let mut verified = verified_for(&host, "tcp://10.0.0.7:7801");
        verified.facts.capabilities.features.multi_host_mobs = true;
        let mut store = HostPairingStore::load(&path).expect("store");
        store.pair(&verified, 1).expect("pair");
        store.save().expect("save");
        let lifecycle =
            RemoteHostLifecycle::load(&path, HostReconnectPolicy::default()).expect("lifecycle");
        let canonical = host.pubkey_b64();

        lifecycle
            .reconnect
            .lock()
            .await
            .get_mut(&canonical)
            .expect("reconnect row")
            .observe_reachable(RuntimeHostHealthStatus::Ok, 2);
        let placement = lifecycle
            .placement(&canonical)
            .await
            .expect("healthy capable host");
        let expected = meerkat_comms::PubKey::new(host.pubkey_bytes())
            .to_peer_id()
            .to_string();
        assert_eq!(placement.host.0, expected);
        assert_eq!(placement.host_key_b64, canonical);
        assert_eq!(placement.pairing_generation, 1);

        lifecycle
            .reconnect
            .lock()
            .await
            .get_mut(&host.pubkey_b64())
            .expect("reconnect row")
            .observe_reachable(RuntimeHostHealthStatus::Unhealthy, 3);
        assert!(matches!(
            lifecycle.placement(&host.pubkey_b64()).await,
            Err(RemoteHostLifecycleError::Unhealthy { .. })
        ));

        let mut store = lifecycle.store.lock().await;
        store
            .hosts
            .get_mut(&host.pubkey_b64())
            .expect("pairing row")
            .last_facts
            .as_mut()
            .expect("cached authenticated facts")
            .capabilities
            .features
            .multi_host_mobs = false;
        drop(store);
        lifecycle
            .reconnect
            .lock()
            .await
            .get_mut(&host.pubkey_b64())
            .expect("reconnect row")
            .observe_reachable(RuntimeHostHealthStatus::Ok, 4);
        assert!(matches!(
            lifecycle.placement(&host.pubkey_b64()).await,
            Err(RemoteHostLifecycleError::PlacementCapabilityMissing { .. })
        ));
    }

    #[tokio::test]
    async fn lifecycle_unreachable_is_a_typed_refusal_not_local_permission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosts.json");
        let host = GatewayPeerKeys::ephemeral();
        let mut verified = verified_for(&host, "tcp://10.0.0.7:7801");
        verified.facts.capabilities.features.multi_host_mobs = true;
        let mut store = HostPairingStore::load(&path).expect("store");
        store.pair(&verified, 1).expect("pair");
        store.save().expect("save");
        let lifecycle =
            RemoteHostLifecycle::load(&path, HostReconnectPolicy::default()).expect("lifecycle");
        lifecycle
            .reconnect
            .lock()
            .await
            .get_mut(&host.pubkey_b64())
            .expect("reconnect row")
            .observe_unreachable(2);

        assert!(matches!(
            lifecycle.placement(&host.pubkey_b64()).await,
            Err(RemoteHostLifecycleError::Unreachable { .. })
        ));
    }
}
