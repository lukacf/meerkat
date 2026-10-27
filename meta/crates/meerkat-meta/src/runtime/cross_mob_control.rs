//! Cross-mob control protocol: the control-plane RPC that crosses
//! processes.
//!
//! * `ControlRequest` / `ControlResponse` - the on-the-wire types.
//! * [`serve_tcp_control`] / [`serve_uds_control`] - accept connections on
//!   a gateway's control listener, read framed requests, dispatch them
//!   against a local mob via a [`ControlHandler`], and write back framed
//!   responses. `UnifiedRuntime::start_control_listener` binds the
//!   listener and spawns the serve task.
//! * [`RemoteControlClient`] - opens a connection per request, sends a
//!   single frame, reads the response. Used by
//!   [`super::cross_mob_remote::RemoteMobProxy`].
//!
//! # Wire shape
//!
//! Each frame is `[u32 BE length][JSON UTF-8 payload]`. JSON is preferred
//! over CBOR for control because it stays human-debuggable in pcap traces
//! and the volume is tiny (one message per `wire`/`unwire`/`inject` call,
//! not per agent turn). `meerkat-comms`'s `TransportCodec` is reserved for
//! agent envelopes.
//!
//! # Trust
//!
//! Descriptor trust is delegated to the contact directory: when a peer
//! meta calls `RemoteMobProxy::wire_remote`, it includes the *peer's*
//! expected `peer_pubkey_b64` in the request. The remote gateway feeds
//! that pubkey into the resulting `TrustedPeerDescriptor`, and
//! `meerkat-comms` rejects envelope traffic that doesn't match it. So the
//! artifacts the control channel produces (peer descriptors) are
//! signature-checked at every subsequent comms ingress regardless of what
//! the control channel itself does.
//!
//! That delegation covers the ENVELOPE plane only. It says nothing about
//! who may drive the control plane itself: an unauthorized
//! [`ControlRequest::Inject`] reaches a member's session without ever
//! minting a descriptor. Two independent authenticities therefore live
//! here:
//!
//! * **Server authenticity** ([`verify_control_response`]) - the serving
//!   gateway signs its responses, and a caller that pins the peer's pubkey
//!   refuses answers it cannot attribute.
//! * **Caller authorization** ([`ControlAuthorizer`]) - the calling
//!   gateway signs its requests with its own keypair, and the serving
//!   gateway matches that identity against a [`ControlGrantTable`] that
//!   binds it to specific verbs and a member scope. Anything outside the
//!   grant is refused before dispatch with a typed
//!   [`ControlAuthzDenial`].
//!
//! [`ControlAuthorizer::open`] remains available only for an explicit
//! compatibility choice. Configured runtime listeners default to an empty
//! grant table and therefore refuse every request until an operator installs a
//! scoped grant. Callers stamp their signature unconditionally, which is why
//! the field is additive per variant rather than a wrapper frame - see
//! [`ControlCaller`].
//!
//! # Keyed inject plane
//!
//! `inject_keyed` and `inject_status` carry a keyed host continuation to a
//! durable identity hosted by this gateway and read its native status back
//! (see [`super::keyed_inject`]). Unlike `inject`, a keyed delivery can be
//! reconciled: an identical replay returns the original receipt. They are
//! grant-only: `verbs = ["*"]` never covers them, an open listener refuses
//! them, and both their requests and every answer, refusals included, are
//! signed.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

use super::cross_mob_remote::{RemoteEndpoint, RemoteMobError};

/// Address a gateway binds its cross-mob control listener on.
///
/// Accepts the same spelling the contact directory uses for remote peers
/// (`tcp://host:port`, `uds:///path`), parsed through the same
/// [`crate::contact_directory`] transport parser so the two surfaces never
/// drift. `inproc` is rejected: a control listener only makes sense across
/// processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlListenAddr {
    /// TCP `host:port`. Port 0 binds an ephemeral port; the bound port is
    /// reported by [`BoundControlListener::advertised_address`].
    Tcp(String),
    /// Unix-domain-socket path.
    Uds(String),
}

impl ControlListenAddr {
    /// Parse `tcp://host:port` or `uds:///path`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match crate::contact_directory::parse_transport(s) {
            Some(crate::contact_directory::MobTransport::Tcp(addr)) => Ok(Self::Tcp(addr)),
            Some(crate::contact_directory::MobTransport::Uds(path)) => Ok(Self::Uds(path)),
            Some(crate::contact_directory::MobTransport::Inproc) => Err(
                "control listener requires a cross-process address (tcp://host:port or \
                 uds:///path); 'inproc' has no listener"
                    .to_string(),
            ),
            None => Err(format!(
                "invalid control listen address '{s}': expected tcp://host:port or uds:///path"
            )),
        }
    }
}

impl std::fmt::Display for ControlListenAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp(addr) => write!(f, "tcp://{addr}"),
            Self::Uds(path) => write!(f, "uds://{path}"),
        }
    }
}

/// A control listener that has been bound but not yet served.
///
/// Binding is split from serving so callers learn the concrete local
/// address (the real port for `tcp://host:0`) before the accept loop
/// starts, and can surface it to tests and peer configuration.
pub enum BoundControlListener {
    Tcp {
        listener: TcpListener,
        advertised: String,
    },
    #[cfg(unix)]
    Uds {
        listener: UnixListener,
        advertised: String,
    },
}

impl BoundControlListener {
    /// Bind `addr`. For TCP the advertised address carries the kernel-
    /// assigned port when the caller bound port 0. For UDS the advertised
    /// address is `uds://{path}`.
    pub async fn bind(addr: &ControlListenAddr) -> Result<Self, std::io::Error> {
        match addr {
            ControlListenAddr::Tcp(spec) => {
                let listener = TcpListener::bind(spec).await?;
                let local = listener.local_addr()?;
                Ok(Self::Tcp {
                    listener,
                    advertised: format!("tcp://{local}"),
                })
            }
            #[cfg(unix)]
            ControlListenAddr::Uds(path) => {
                let listener = UnixListener::bind(std::path::Path::new(path))?;
                Ok(Self::Uds {
                    listener,
                    advertised: format!("uds://{path}"),
                })
            }
            #[cfg(not(unix))]
            ControlListenAddr::Uds(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "unix domain sockets are not supported on this platform",
            )),
        }
    }

    /// The dialable address of this listener (`tcp://ip:port` with the
    /// real bound port, or `uds:///path`).
    pub fn advertised_address(&self) -> &str {
        match self {
            Self::Tcp { advertised, .. } => advertised,
            #[cfg(unix)]
            Self::Uds { advertised, .. } => advertised,
        }
    }

    /// Serve control requests until the owning task is aborted. Responses
    /// are signed with the gateway key when `signer` holds one (re-read
    /// per request, so late-installed keys take effect).
    ///
    /// No caller authorization: every decodable request is dispatched. Use
    /// [`Self::serve_with_authorizer`] to bind callers to scoped grants.
    pub async fn serve(
        self,
        handler: std::sync::Arc<dyn ControlHandler>,
        signer: ControlSignerSlot,
    ) {
        self.serve_with_authorizer(
            handler,
            signer,
            std::sync::Arc::new(ControlAuthorizer::open()),
        )
        .await;
    }

    /// Serve control requests, refusing anything `authorizer` does not
    /// admit before it reaches the handler.
    pub async fn serve_with_authorizer(
        self,
        handler: std::sync::Arc<dyn ControlHandler>,
        signer: ControlSignerSlot,
        authorizer: std::sync::Arc<ControlAuthorizer>,
    ) {
        match self {
            Self::Tcp { listener, .. } => {
                serve_tcp_control_with_authorizer(listener, handler, signer, authorizer).await;
            }
            #[cfg(unix)]
            Self::Uds { listener, .. } => {
                serve_uds_control_with_authorizer(listener, handler, signer, authorizer).await;
            }
        }
    }
}

/// Maximum control payload size. Control messages are tiny (a few hundred
/// bytes typical, ~1 KiB max for an injected text turn) — we cap well below
/// that so a misbehaving peer can't tie up reads.
pub(crate) const MAX_CONTROL_PAYLOAD: u32 = 64 * 1024;

/// Default request timeout. Control RPC should be fast — local mob
/// dispatch on the remote side is sub-millisecond. We give it 5s to absorb
/// scheduler pauses, network latency, and lazy-spawn warm-up.
pub const DEFAULT_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

/// Cross-mob control request. One variant per control operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ControlRequest {
    /// Wire a local member to a peer mob's member.
    Wire {
        /// Member identity in the *receiving* (remote) mob.
        remote_member: String,
        /// Peer descriptor of the *calling* gateway's local member.
        local_peer_spec_address: String,
        local_comms_name: String,
        local_peer_id: String,
        /// Ed25519 transport pubkey of the calling MEMBER. When present,
        /// the receiving gateway builds a signed `TrustedPeerDescriptor`
        /// so meerkat-comms can verify envelope signatures.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local_pubkey_b64: Option<String>,
        /// Client-minted freshness nonce. It rides inside the request
        /// bytes, so the response signature (which covers the request
        /// digest) cannot be replayed for a later request. Old servers
        /// ignore it; old clients omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
        /// Calling gateway's authentication - see [`ControlCaller`].
        /// `None` from callers that hold no gateway keypair and from
        /// gateways that predate caller authorization.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ControlCaller>,
    },
    /// Unwire a previously-wired peer.
    Unwire {
        remote_member: String,
        local_peer_spec_address: String,
        local_comms_name: String,
        local_peer_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local_pubkey_b64: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ControlCaller>,
    },
    /// Inject an external-turn message into a remote member's session.
    /// Used by `send_cross_mob` for app-level injection.
    Inject {
        remote_member: String,
        /// JSON-encoded `meerkat_core::ContentInput`.
        content: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ControlCaller>,
    },
    /// Look up a member's comms info on the remote side. Used during
    /// `wire_cross_mob` to discover the remote member's peer_id and
    /// derived comms_name without requiring caller-supplied bookkeeping.
    LookupMember {
        remote_member: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ControlCaller>,
    },
    /// Ask the serving gateway to describe ITSELF as a runtime host:
    /// endpoint identity, capabilities, placement labels. Read-only, and
    /// it names no member. See [`super::remote_host`].
    HostDescribe {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ControlCaller>,
    },
    /// Ask the serving gateway for its own runtime-host health
    /// projection. Read-only, names no member.
    HostHealth {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ControlCaller>,
    },
    /// Deliver one keyed host continuation to a durable identity this
    /// gateway hosts. The delivery key, under the caller's scope, becomes
    /// the continuation key, so an identical replay returns the original
    /// receipt. See [`super::keyed_inject`].
    InjectKeyed {
        /// The member identity, in its public form.
        remote_member: String,
        delivery_key: String,
        /// The caller's digest of `body`.
        content_digest: String,
        summary: String,
        body: String,
        /// The caller's clock when it signed this request, in milliseconds
        /// since the Unix epoch. The serving gateway refuses a request
        /// outside its freshness window.
        issued_at_ms: u64,
        /// Fresh per request; the signed answer covers it.
        nonce: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ControlCaller>,
    },
    /// Read the native status of a keyed delivery for a durable identity
    /// this gateway hosts.
    InjectStatus {
        remote_member: String,
        delivery_key: String,
        /// Also report the admitted input's own runtime state.
        #[serde(default)]
        with_input: bool,
        issued_at_ms: u64,
        nonce: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<ControlCaller>,
    },
}

impl ControlRequest {
    /// Which control verb this request carries. The unit of grant
    /// authorization together with [`Self::remote_member`].
    pub fn verb(&self) -> ControlVerb {
        match self {
            Self::Wire { .. } => ControlVerb::Wire,
            Self::Unwire { .. } => ControlVerb::Unwire,
            Self::Inject { .. } => ControlVerb::Inject,
            Self::LookupMember { .. } => ControlVerb::LookupMember,
            Self::HostDescribe { .. } => ControlVerb::HostDescribe,
            Self::HostHealth { .. } => ControlVerb::HostHealth,
            Self::InjectKeyed { .. } => ControlVerb::InjectKeyed,
            Self::InjectStatus { .. } => ControlVerb::InjectStatus,
        }
    }

    /// The member of the RECEIVING mob this request reaches. Every
    /// MEMBER-plane verb names exactly one, which is what makes a member
    /// scope expressible; host-plane verbs address the gateway itself and
    /// answer [`HOST_PLANE_MEMBER`]. Keyed inject verbs name their member.
    pub fn remote_member(&self) -> &str {
        match self {
            Self::Wire { remote_member, .. }
            | Self::Unwire { remote_member, .. }
            | Self::Inject { remote_member, .. }
            | Self::LookupMember { remote_member, .. }
            | Self::InjectKeyed { remote_member, .. }
            | Self::InjectStatus { remote_member, .. } => remote_member,
            Self::HostDescribe { .. } | Self::HostHealth { .. } => HOST_PLANE_MEMBER,
        }
    }

    /// Borrow the caller authentication, if the caller stamped one.
    ///
    /// UNVERIFIED CLAIM. A [`ControlHandler`] that reads this is holding
    /// whatever the sender wrote: on a listener in [`ControlAuthorizer::Open`]
    /// mode nothing checked the signature at all, and even under
    /// [`ControlAuthorizer::Grants`] the verification happened at the
    /// listener seam, not here. Never make an authorization decision from
    /// this accessor - express the policy as a [`ControlGrant`] and let
    /// `serve_*_with_authorizer` refuse before dispatch. Logging and
    /// attribution are the intended uses.
    pub fn caller(&self) -> Option<&ControlCaller> {
        match self {
            Self::Wire { caller, .. }
            | Self::Unwire { caller, .. }
            | Self::Inject { caller, .. }
            | Self::LookupMember { caller, .. }
            | Self::HostDescribe { caller, .. }
            | Self::HostHealth { caller, .. }
            | Self::InjectKeyed { caller, .. }
            | Self::InjectStatus { caller, .. } => caller.as_ref(),
        }
    }

    fn caller_mut(&mut self) -> Option<&mut ControlCaller> {
        match self {
            Self::Wire { caller, .. }
            | Self::Unwire { caller, .. }
            | Self::Inject { caller, .. }
            | Self::LookupMember { caller, .. }
            | Self::HostDescribe { caller, .. }
            | Self::HostHealth { caller, .. }
            | Self::InjectKeyed { caller, .. }
            | Self::InjectStatus { caller, .. } => caller.as_mut(),
        }
    }

    /// Replace the caller authentication. Used by
    /// [`sign_control_request_as_caller`]; tests use it to strip or forge
    /// credentials.
    pub fn set_caller(&mut self, value: Option<ControlCaller>) {
        match self {
            Self::Wire { caller, .. }
            | Self::Unwire { caller, .. }
            | Self::Inject { caller, .. }
            | Self::LookupMember { caller, .. }
            | Self::HostDescribe { caller, .. }
            | Self::HostHealth { caller, .. }
            | Self::InjectKeyed { caller, .. }
            | Self::InjectStatus { caller, .. } => *caller = value,
        }
    }
}

/// Cross-mob control response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "result", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ControlResponse {
    /// Operation succeeded (no payload).
    Ok {
        /// Gateway signature over this response bound to the exact request
        /// bytes - see [`control_response_signing_payload`]. Absent when
        /// the serving gateway has no signing keys installed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig_b64: Option<String>,
    },
    /// Inject succeeded - the far side admitted the dispatch and returns
    /// the bridge session id that accepted it, so the caller can correlate
    /// downstream events.
    ///
    /// This is NOT a durability receipt. It classifies as dispatch
    /// admission only: the receiving runtime accepted the turn into the
    /// named session, and whether that turn commits durably is the
    /// receiving runtime's business. Coarse transport ACKs (this response
    /// included) must never be inferred as durable=true. Callers that need
    /// durable admission over remote paths reconcile by idempotent
    /// re-submit with WorkRef dedup - the protocol deliberately has no
    /// resend verb.
    Injected {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig_b64: Option<String>,
    },
    /// LookupMember succeeded - return the remote member's comms facts so
    /// the caller can build a signed `TrustedPeerDescriptor` pointing at
    /// it.
    Member {
        peer_id: String,
        comms_name: String,
        /// The member's transport pubkey as the roster carries it (base64,
        /// optional `ed25519:` prefix). `None` when the member has no
        /// comms runtime. Descriptors for this member MUST use this key:
        /// the peer-id/pubkey consistency check rejects any other (the
        /// gateway key included).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pubkey_b64: Option<String>,
        /// The member's dialable envelope-listener address as its live
        /// comms runtime advertises it (`tcp://host:port` with the real
        /// bound port, `uds:///path`, or `inproc://name` for members
        /// without a socket transport). `None` when the serving gateway's
        /// control handler has no session-service access.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        advertised_address: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig_b64: Option<String>,
    },
    /// `HostDescribe` succeeded - the serving gateway's own read-only
    /// runtime-host projection.
    ///
    /// TRUSTED MATERIAL, unlike `Err`: a controller pins a host identity
    /// and pairs durably off these facts, so the signature covers a
    /// length-prefixed digest of every field (see
    /// [`super::remote_host::HostFacts::signing_digest_hex`]) rather than
    /// a newline-joined string. Placement labels are operator-supplied,
    /// so newline injection into the signed material has to be
    /// impossible, not merely unlikely.
    Host {
        facts: super::remote_host::HostFacts,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig_b64: Option<String>,
    },
    /// `HostHealth` succeeded - the serving gateway's own health
    /// projection, in meerkat's health vocabulary.
    HostHealth {
        health: meerkat_contracts::RuntimeHostHealth,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig_b64: Option<String>,
    },
    /// `InjectKeyed` reached the member's continuation owner, which
    /// committed the delivery (or had committed it already: an identical
    /// replay returns the original receipt). TRUSTED MATERIAL: the caller
    /// settles a delivery from it only when the signature verifies.
    InjectReceipt {
        receipt: super::keyed_inject::NativeReceipt,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig_b64: Option<String>,
    },
    /// `InjectStatus` answered: the continuation owner's own status of the
    /// delivery's key, and the admitted input's state when asked for.
    InjectStatus {
        status: super::keyed_inject::NativeStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig_b64: Option<String>,
    },
    /// A keyed inject verb was refused by the serving side. Unlike `Err`, this
    /// is signed: a caller may settle a delivery from some refusals (see
    /// [`super::keyed_inject::KeyedInjectRefusalCode`]), so they have to be
    /// attributable.
    InjectRefused {
        code: String,
        message: String,
        /// The typed cause of a code that carries one, in its wire spelling
        /// (see [`super::keyed_inject::KeyedInjectRefusalCause`]). Signed with
        /// the code and the message; a caller reads it from here, never from
        /// the message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig_b64: Option<String>,
    },
    /// Operation failed. `code` is a stable short string for machine
    /// dispatch (`unknown_member`, `mob_error`, `decode`, ...); `message`
    /// is human-readable.
    Err { code: String, message: String },
}

/// Domain-separation context for control-response signatures. Versioned so
/// a future payload change cannot be confused with this one.
pub const CONTROL_SIG_CONTEXT: &str = "meta-cross-mob-control-v1";

/// Live signing authority for control responses: the gateway keypair,
/// late-bound because hosts install keys after the runtime (and possibly
/// after the listener) exists. `None` serves unsigned responses, which
/// clients with a pinned peer pubkey reject.
pub type ControlSignerSlot = std::sync::Arc<
    std::sync::RwLock<Option<std::sync::Arc<crate::auth::peer_keys::GatewayPeerKeys>>>,
>;

/// Live read-only host-facts projection. Like [`ControlSignerSlot`], this is
/// late-bound so a listener started before gateway identity installation can
/// begin serving authenticated host discovery without being rebuilt.
pub type HostFactsProviderSlot = std::sync::Arc<
    std::sync::RwLock<Option<std::sync::Arc<dyn super::remote_host::HostFactsProvider>>>,
>;

/// A signer slot that never signs (tests, deployments without keys).
pub fn unsigned_control_signer() -> ControlSignerSlot {
    std::sync::Arc::new(std::sync::RwLock::new(None))
}

/// Empty late-bound host-facts slot.
pub fn empty_host_facts_provider() -> HostFactsProviderSlot {
    std::sync::Arc::new(std::sync::RwLock::new(None))
}

fn control_request_digest_hex(request_bytes: &[u8]) -> String {
    sha256_hex(request_bytes)
}

/// Lowercase hex SHA-256. Shared with [`super::remote_host`] so the
/// control plane has exactly one digest implementation rather than two
/// that can drift.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The deterministic byte string a control-response signature covers.
///
/// Built from the semantic response fields plus the SHA-256 of the exact
/// request bytes the server read off the wire - never from re-serialized
/// JSON, so there is no canonicalization to get wrong. Binding the request
/// digest means a signature is only valid for the one request (including
/// its client-minted nonce) that elicited it: a MITM can neither
/// substitute member facts in a `Member` response nor replay a previous
/// response for a fresh request. `Err` responses are not trusted material
/// and are never signed.
pub fn control_response_signing_payload(
    request_bytes: &[u8],
    response: &ControlResponse,
) -> Option<String> {
    let facts = match response {
        ControlResponse::Ok { .. } => "ok".to_string(),
        ControlResponse::Injected { session_id, .. } => format!("injected\n{session_id}"),
        ControlResponse::Member {
            peer_id,
            comms_name,
            pubkey_b64,
            advertised_address,
            ..
        } => format!(
            "member\n{peer_id}\n{comms_name}\n{}\n{}",
            pubkey_b64.as_deref().unwrap_or(""),
            advertised_address.as_deref().unwrap_or("")
        ),
        // Host facts and health enter as fixed-width hex digests over a
        // length-prefixed canonical encoding, so an operator-supplied
        // label containing a newline cannot forge a different projection
        // that signs identically. Never inline these fields here.
        ControlResponse::Host { facts, .. } => {
            format!("host\n{}", facts.signing_digest_hex())
        }
        ControlResponse::HostHealth { health, .. } => format!(
            "host_health\n{}",
            super::remote_host::host_health_digest_hex(health)
        ),
        // Keyed inject answers enter length-prefixed field by field, so no
        // value (a refusal message, a reason) can pass for another answer.
        ControlResponse::InjectReceipt { receipt, .. } => format!(
            "inject_receipt\n{}",
            super::keyed_inject::receipt_facts(receipt)
        ),
        ControlResponse::InjectStatus { status, input, .. } => format!(
            "inject_status\n{}",
            super::keyed_inject::status_facts(status, input.as_ref())
        ),
        ControlResponse::InjectRefused {
            code,
            message,
            cause,
            ..
        } => format!(
            "inject_refused\n{}",
            super::keyed_inject::refusal_facts(code, cause.as_deref(), message)
        ),
        ControlResponse::Err { .. } => return None,
    };
    Some(format!(
        "{CONTROL_SIG_CONTEXT}\n{}\n{facts}",
        control_request_digest_hex(request_bytes)
    ))
}

/// Sign `response` in place with the gateway key. No-op for `Err`.
fn sign_control_response(
    signer: &crate::auth::peer_keys::GatewayPeerKeys,
    request_bytes: &[u8],
    response: &mut ControlResponse,
) {
    use ed25519_dalek::Signer;
    let Some(payload) = control_response_signing_payload(request_bytes, response) else {
        return;
    };
    let signature = signer.signing_key().sign(payload.as_bytes());
    let encoded = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
    match response {
        ControlResponse::Ok { sig_b64 }
        | ControlResponse::Injected { sig_b64, .. }
        | ControlResponse::Member { sig_b64, .. }
        | ControlResponse::Host { sig_b64, .. }
        | ControlResponse::HostHealth { sig_b64, .. }
        | ControlResponse::InjectReceipt { sig_b64, .. }
        | ControlResponse::InjectStatus { sig_b64, .. }
        | ControlResponse::InjectRefused { sig_b64, .. } => *sig_b64 = Some(encoded),
        ControlResponse::Err { .. } => {}
    }
}

/// Verify a control response against the pinned gateway pubkey and the
/// exact request bytes that were sent. `Err` responses pass unverified:
/// they are failure reports, not trusted material, and rejecting them
/// would only change WHICH error the caller sees.
pub fn verify_control_response(
    pinned_pubkey: &[u8; 32],
    request_bytes: &[u8],
    response: &ControlResponse,
) -> Result<(), String> {
    let Some(payload) = control_response_signing_payload(request_bytes, response) else {
        return Ok(());
    };
    let sig_b64 = match response {
        ControlResponse::Ok { sig_b64 }
        | ControlResponse::Injected { sig_b64, .. }
        | ControlResponse::Member { sig_b64, .. }
        | ControlResponse::Host { sig_b64, .. }
        | ControlResponse::HostHealth { sig_b64, .. }
        | ControlResponse::InjectReceipt { sig_b64, .. }
        | ControlResponse::InjectStatus { sig_b64, .. }
        | ControlResponse::InjectRefused { sig_b64, .. } => sig_b64.as_deref(),
        ControlResponse::Err { .. } => None,
    };
    let Some(sig_b64) = sig_b64 else {
        return Err(
            "response is unsigned; the peer gateway has no signing keys installed or predates \
             signed control responses"
                .to_string(),
        );
    };
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(sig_b64)
        .map_err(|err| format!("signature is not valid base64: {err}"))?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "signature must be 64 bytes".to_string())?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(pinned_pubkey)
        .map_err(|err| format!("pinned pubkey is not a valid Ed25519 key: {err}"))?;
    verifying_key
        .verify_strict(payload.as_bytes(), &signature)
        .map_err(|_| "signature does not verify against the pinned gateway pubkey".to_string())
}

// ---------------------------------------------------------------------
// Caller authorization: scoped grants on the control channel
// ---------------------------------------------------------------------

/// Authentication the CALLING gateway stamps onto every control request it
/// sends when it holds a keypair.
///
/// Carried as an optional field on each [`ControlRequest`] variant rather
/// than as a wrapper frame, deliberately: serde ignores unknown fields, so
/// a gateway that predates caller authorization still decodes and serves
/// these requests unchanged, and a gateway that enforces grants sees
/// `None` from an old caller and refuses it typed. A wrapper variant would
/// instead have made every pre-0.8.22 gateway answer `decode` to every
/// new caller, which is a fleet-wide outage rather than a rollout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlCaller {
    /// The calling gateway's Ed25519 pubkey - base64, optional `ed25519:`
    /// prefix, the same spelling `meta/peer_pubkey` reports and the
    /// contact directory pins. This is the identity a grant is keyed by.
    /// The CLAIM is worthless on its own; it is believed only once
    /// `sig_b64` verifies against it.
    pub pubkey_b64: String,
    /// Signature over [`control_request_signing_payload`] for this exact
    /// request, made with the key `pubkey_b64` names.
    pub sig_b64: String,
    /// Which peer this request was minted for - the contact-directory mob
    /// id the caller dialed. Signed material, so a captured request cannot
    /// be replayed against a DIFFERENT gateway where the same caller also
    /// holds a grant. Enforced only when the serving authorizer was built
    /// with an expected audience (see
    /// [`ControlAuthorizer::with_grants_for_audience`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
}

/// One control operation, as a grantable unit.
///
/// Three planes live in this enum:
///
/// * The MEMBER plane ([`Self::member_plane`]) - every verb that names a
///   member of the receiving mob and reaches it.
/// * The HOST plane ([`Self::is_host_plane`]) - read-only projections a
///   gateway makes about ITSELF as a runtime host (see
///   [`super::remote_host`]). These name no member, mutate nothing, and
///   are never covered by `verbs = ["*"]`.
/// * The KEYED INJECT plane ([`Self::is_keyed_inject_plane`]) - keyed,
///   reconcilable delivery to a durable identity this gateway hosts (see
///   [`super::keyed_inject`]). These name their member, so member scope
///   applies, but they are never covered by `verbs = ["*"]` and an open
///   listener refuses them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ControlVerb {
    Wire,
    Unwire,
    Inject,
    LookupMember,
    /// Runtime-host identity, capability and placement-label projection.
    HostDescribe,
    /// Runtime-host health projection.
    HostHealth,
    /// Deliver one keyed host continuation to a hosted identity.
    InjectKeyed,
    /// Read a keyed delivery's native status for a hosted identity.
    InjectStatus,
}

impl ControlVerb {
    /// Stable config/wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wire => "wire",
            Self::Unwire => "unwire",
            Self::Inject => "inject",
            Self::LookupMember => "lookup_member",
            Self::HostDescribe => "host_describe",
            Self::HostHealth => "host_health",
            Self::InjectKeyed => "inject_keyed",
            Self::InjectStatus => "inject_status",
        }
    }

    /// Parse the config spelling. `None` for anything else - grant config
    /// fails closed on an unknown verb rather than silently granting less
    /// (or more) than the operator wrote.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "wire" => Some(Self::Wire),
            "unwire" => Some(Self::Unwire),
            "inject" => Some(Self::Inject),
            "lookup_member" => Some(Self::LookupMember),
            "host_describe" => Some(Self::HostDescribe),
            "host_health" => Some(Self::HostHealth),
            "inject_keyed" => Some(Self::InjectKeyed),
            "inject_status" => Some(Self::InjectStatus),
            _ => None,
        }
    }

    /// Every verb this gateway knows, for tests and for exhaustive
    /// tables. NOT what `verbs = ["*"]` expands to - see
    /// [`Self::member_plane`]. A slice, so a later verb does not change
    /// its type.
    pub fn all() -> &'static [Self] {
        &[
            Self::Wire,
            Self::Unwire,
            Self::Inject,
            Self::LookupMember,
            Self::HostDescribe,
            Self::HostHealth,
            Self::InjectKeyed,
            Self::InjectStatus,
        ]
    }

    /// What `verbs = ["*"]` expands to: the member-plane verbs, and only
    /// those.
    ///
    /// Host-plane and keyed-inject-plane verbs are deliberately EXCLUDED. Every
    /// `*` grant already deployed was written when this enum held four
    /// verbs, and widening those grants by adding variants here would be a
    /// silent privilege change made by an upgrade rather than by an
    /// operator. A gateway that wants to serve them to a caller names them
    /// explicitly.
    pub fn member_plane() -> [Self; 4] {
        [Self::Wire, Self::Unwire, Self::Inject, Self::LookupMember]
    }

    /// Whether this verb addresses the HOST rather than a member.
    /// Host-plane verbs carry [`HOST_PLANE_MEMBER`] as their member and
    /// are exempt from member scoping (there is no member to scope).
    pub fn is_host_plane(&self) -> bool {
        match self {
            Self::HostDescribe | Self::HostHealth => true,
            Self::Wire
            | Self::Unwire
            | Self::Inject
            | Self::LookupMember
            | Self::InjectKeyed
            | Self::InjectStatus => false,
        }
    }

    /// Whether this verb carries keyed delivery. Only a grant that names
    /// it admits it; a listener that enforces no grants refuses it.
    pub fn is_keyed_inject_plane(&self) -> bool {
        match self {
            Self::InjectKeyed | Self::InjectStatus => true,
            Self::Wire
            | Self::Unwire
            | Self::Inject
            | Self::LookupMember
            | Self::HostDescribe
            | Self::HostHealth => false,
        }
    }
}

/// The member a host-plane request names: none.
///
/// Host-plane verbs address the serving gateway itself, so
/// [`ControlRequest::remote_member`] has nothing to return for them. The
/// authorization path never consults it either - [`ControlGrant`] exempts
/// host-plane verbs from member scoping AFTER the verb check - so this is
/// an attribution/logging value, not an authorization input.
pub const HOST_PLANE_MEMBER: &str = "";

impl std::fmt::Display for ControlVerb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Domain-separation context for caller (request) signatures. Distinct
/// from [`CONTROL_SIG_CONTEXT`] so a response signature can never be
/// replayed as a request signature or vice versa. Versioned: a payload
/// shape change must bump this.
pub const CONTROL_CALLER_SIG_CONTEXT: &str = "meta-cross-mob-control-caller-v1";

/// Length-prefix one field into the signing payload.
///
/// Length-prefixed rather than newline-delimited so a value that itself
/// contains newlines cannot forge extra fields - without this, a crafted
/// `remote_member` could make one verb's payload byte-identical to
/// another's and let a narrow grant be spent on a wider request.
pub(crate) fn push_signed_field(out: &mut String, value: &str) {
    use std::fmt::Write;
    let _ = writeln!(out, "{}:{value}", value.len());
}

/// SHA-256 of an injected content payload, as it appears in the signing
/// payload. Digested rather than inlined because the content is arbitrary
/// JSON of unbounded shape.
///
/// The `unwrap_or_default` below is unreachable, not a swallow: serializing
/// an already-parsed `serde_json::Value` cannot fail (no non-finite floats,
/// no non-string keys). It matters that it stays unreachable - a shape that
/// COULD fail would make every failing content digest to the same empty
/// input, and one signature would then cover any of them.
fn control_content_digest_hex(content: &serde_json::Value) -> String {
    control_request_digest_hex(&serde_json::to_vec(content).unwrap_or_default())
}

/// The deterministic byte string a caller signature covers.
///
/// Derived from the request's semantic fields rather than from the raw
/// frame bytes, because the signature itself rides inside those bytes and
/// cannot cover itself. Both sides compute it from a decoded
/// [`ControlRequest`], so there is no canonical-JSON problem for the
/// scalar fields; the one non-scalar field (`Inject.content`) enters as a
/// SHA-256 of its serialization, which round-trips stably because both
/// sides serialize a `serde_json::Value` (a request whose raw bytes do not
/// round-trip - duplicate object keys, say - simply fails to verify, which
/// is fail-closed).
pub fn control_request_signing_payload(request: &ControlRequest) -> String {
    let mut payload = String::new();
    payload.push_str(CONTROL_CALLER_SIG_CONTEXT);
    payload.push('\n');
    push_signed_field(&mut payload, request.verb().as_str());
    // INVARIANT: every semantically meaningful field of every variant must
    // be pushed here. The match destructures exhaustively (no `..`) on
    // purpose - a field added to a variant must break this build rather
    // than silently become MITM-malleable, since the signature covers this
    // derived payload and not the raw bytes.
    match request {
        ControlRequest::Wire {
            remote_member,
            local_peer_spec_address,
            local_comms_name,
            local_peer_id,
            local_pubkey_b64,
            nonce,
            // The credential envelope is handled below: `sig_b64` cannot
            // cover itself and `pubkey_b64` IS the verifying key, so the
            // only envelope field that needs signing is the audience.
            caller: _caller,
        }
        | ControlRequest::Unwire {
            remote_member,
            local_peer_spec_address,
            local_comms_name,
            local_peer_id,
            local_pubkey_b64,
            nonce,
            caller: _caller,
        } => {
            push_signed_field(&mut payload, remote_member);
            push_signed_field(&mut payload, local_peer_spec_address);
            push_signed_field(&mut payload, local_comms_name);
            push_signed_field(&mut payload, local_peer_id);
            push_signed_field(&mut payload, local_pubkey_b64.as_deref().unwrap_or(""));
            push_signed_field(&mut payload, nonce.as_deref().unwrap_or(""));
        }
        ControlRequest::Inject {
            remote_member,
            content,
            nonce,
            caller: _caller,
        } => {
            push_signed_field(&mut payload, remote_member);
            push_signed_field(&mut payload, &control_content_digest_hex(content));
            push_signed_field(&mut payload, nonce.as_deref().unwrap_or(""));
        }
        ControlRequest::LookupMember {
            remote_member,
            nonce,
            caller: _caller,
        } => {
            push_signed_field(&mut payload, remote_member);
            push_signed_field(&mut payload, nonce.as_deref().unwrap_or(""));
        }
        // Host-plane requests carry no member and no arguments: the verb
        // (already pushed above, and distinct per variant) plus the
        // nonce plus the audience is the whole semantic content.
        ControlRequest::HostDescribe {
            nonce,
            caller: _caller,
        }
        | ControlRequest::HostHealth {
            nonce,
            caller: _caller,
        } => {
            push_signed_field(&mut payload, nonce.as_deref().unwrap_or(""));
        }
        // The body enters as its own SHA-256 beside the claimed content
        // digest, so the signature binds the exact bytes the serving side
        // checks against that digest.
        ControlRequest::InjectKeyed {
            remote_member,
            delivery_key,
            content_digest,
            summary,
            body,
            issued_at_ms,
            nonce,
            caller: _caller,
        } => {
            push_signed_field(&mut payload, remote_member);
            push_signed_field(&mut payload, delivery_key);
            push_signed_field(&mut payload, content_digest);
            push_signed_field(&mut payload, summary);
            push_signed_field(&mut payload, &sha256_hex(body.as_bytes()));
            push_signed_field(&mut payload, &issued_at_ms.to_string());
            push_signed_field(&mut payload, nonce);
        }
        ControlRequest::InjectStatus {
            remote_member,
            delivery_key,
            with_input,
            issued_at_ms,
            nonce,
            caller: _caller,
        } => {
            push_signed_field(&mut payload, remote_member);
            push_signed_field(&mut payload, delivery_key);
            push_signed_field(&mut payload, if *with_input { "with_input" } else { "" });
            push_signed_field(&mut payload, &issued_at_ms.to_string());
            push_signed_field(&mut payload, nonce);
        }
    }
    push_signed_field(
        &mut payload,
        request
            .caller()
            .and_then(|caller| caller.audience.as_deref())
            .unwrap_or(""),
    );
    payload
}

/// Stamp this gateway's caller authentication onto `request`.
///
/// `audience` is the contact-directory mob id being dialed; it binds the
/// signature to one peer. Overwrites any existing caller envelope - the
/// signature is minted over the request as it will go on the wire.
pub fn sign_control_request_as_caller(
    keys: &crate::auth::peer_keys::GatewayPeerKeys,
    audience: Option<&str>,
    request: &mut ControlRequest,
) {
    use ed25519_dalek::Signer;
    request.set_caller(Some(ControlCaller {
        pubkey_b64: keys.pubkey_b64(),
        sig_b64: String::new(),
        audience: audience.map(str::to_string),
    }));
    let payload = control_request_signing_payload(request);
    let signature = keys.signing_key().sign(payload.as_bytes());
    let encoded = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
    if let Some(caller) = request.caller_mut() {
        caller.sig_b64 = encoded;
    }
}

/// Why a control request was refused before dispatch.
///
/// Typed so the serving side cannot accidentally answer a policy refusal
/// with a handler-shaped error (and so the codes below stay stable for
/// machine dispatch on the calling side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAuthzDenial {
    /// The request carried no caller credential at all, but this listener
    /// enforces grants.
    UnauthenticatedCaller { verb: ControlVerb },
    /// The credential was present but structurally unusable (pubkey or
    /// signature not decodable).
    InvalidCallerCredential { reason: String },
    /// The signature did not verify against the claimed pubkey, so the
    /// claimed identity is not established.
    InvalidCallerSignature { pubkey_b64: String },
    /// The caller authenticated successfully but holds no grant here.
    CallerNotGranted { pubkey_b64: String },
    /// The caller holds a grant, but not for this verb.
    VerbNotGranted { label: String, verb: ControlVerb },
    /// The caller holds a grant for this verb, but not on this member.
    MemberNotGranted {
        label: String,
        verb: ControlVerb,
        member: String,
    },
    /// The request was minted for a different gateway.
    AudienceMismatch {
        label: String,
        expected: String,
        presented: String,
    },
    /// The verb is admitted only under a grant that names it, and this
    /// listener enforces no grants.
    GrantsRequired { verb: ControlVerb },
}

/// Every stable denial code this module can answer with. Callers use it to
/// classify a peer's `Err` response as an authorization refusal rather
/// than string-matching one code at a time.
pub const CONTROL_AUTHZ_DENIAL_CODES: [&str; 8] = [
    "unauthenticated_caller",
    "invalid_caller_credential",
    "invalid_caller_signature",
    "caller_not_granted",
    "verb_not_granted",
    "member_not_granted",
    "audience_mismatch",
    "grants_required",
];

impl ControlAuthzDenial {
    /// Stable short code, carried in [`ControlResponse::Err`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnauthenticatedCaller { .. } => "unauthenticated_caller",
            Self::InvalidCallerCredential { .. } => "invalid_caller_credential",
            Self::InvalidCallerSignature { .. } => "invalid_caller_signature",
            Self::CallerNotGranted { .. } => "caller_not_granted",
            Self::VerbNotGranted { .. } => "verb_not_granted",
            Self::MemberNotGranted { .. } => "member_not_granted",
            Self::AudienceMismatch { .. } => "audience_mismatch",
            Self::GrantsRequired { .. } => "grants_required",
        }
    }

    /// Whether a peer-reported error code names an authorization refusal.
    pub fn is_denial_code(code: &str) -> bool {
        CONTROL_AUTHZ_DENIAL_CODES.contains(&code)
    }
}

impl std::fmt::Display for ControlAuthzDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnauthenticatedCaller { verb } => write!(
                f,
                "control verb '{verb}' requires a signed caller credential on this listener; \
                 the calling gateway must install a keypair (set_gateway_peer_keys) and be \
                 listed in this gateway's control grant table"
            ),
            Self::InvalidCallerCredential { reason } => {
                write!(f, "caller credential is unusable: {reason}")
            }
            Self::InvalidCallerSignature { pubkey_b64 } => write!(
                f,
                "caller signature does not verify against the presented pubkey '{pubkey_b64}'"
            ),
            Self::CallerNotGranted { pubkey_b64 } => write!(
                f,
                "caller '{pubkey_b64}' holds no control grant on this gateway"
            ),
            Self::VerbNotGranted { label, verb } if verb.is_host_plane() => write!(
                f,
                "caller '{label}' is not granted host-plane control verb '{verb}' on this \
                 gateway; host-plane verbs are never covered by verbs = [\"*\"] and must be \
                 named explicitly"
            ),
            Self::VerbNotGranted { label, verb } if verb.is_keyed_inject_plane() => write!(
                f,
                "caller '{label}' is not granted keyed inject control verb '{verb}' on this \
                 gateway; keyed inject verbs are never covered by verbs = [\"*\"] and must be \
                 named explicitly"
            ),
            Self::VerbNotGranted { label, verb } => write!(
                f,
                "caller '{label}' is not granted control verb '{verb}' on this gateway"
            ),
            Self::MemberNotGranted {
                label,
                verb,
                member,
            } => write!(
                f,
                "caller '{label}' is not granted control verb '{verb}' on member '{member}'"
            ),
            Self::AudienceMismatch {
                label,
                expected,
                presented,
            } => write!(
                f,
                "caller '{label}' presented a request minted for '{presented}', but this \
                 gateway answers for '{expected}'"
            ),
            Self::GrantsRequired { verb } => write!(
                f,
                "control verb '{verb}' is served only under a control grant that names it, and \
                 this listener enforces no grants"
            ),
        }
    }
}

impl std::error::Error for ControlAuthzDenial {}

/// Which members of the local mob a grant reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMemberScope {
    /// Every member, present and future.
    All,
    /// Only these member aliases, held in the PUBLIC alias space and
    /// ALREADY normalized. Build with [`Self::members`] rather than this
    /// variant directly: [`Self::contains`] normalizes the request side
    /// exactly once, so an entry stashed here in the comms-safe roster
    /// spelling matches nothing (fail-closed, but silently).
    Members(BTreeSet<String>),
}

impl ControlMemberScope {
    /// Build a member scope from public aliases. `*` anywhere in the list
    /// widens to [`Self::All`].
    pub fn members<I, S>(aliases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = BTreeSet::new();
        for alias in aliases {
            let alias = alias.as_ref();
            if alias == "*" {
                return Self::All;
            }
            set.insert(normalize_member_alias(alias));
        }
        Self::Members(set)
    }

    /// Whether this scope reaches `member`.
    ///
    /// Both sides are normalized through `runtime_alias_str` first, for
    /// the same reason `is_reserved_generated_alias` decodes before it
    /// checks: raw control surfaces may present either the public alias
    /// (`rt:worker:0`) or its comms-safe roster encoding, and a scope that
    /// only matched one spelling would be bypassable with the other. This
    /// is the same normalization `handle_wire` / `handle_inject` /
    /// `handle_lookup_member` apply, so the scope decides on exactly the
    /// member the handler will reach.
    ///
    /// Normalize EXACTLY ONCE per side. `runtime_alias_str` is not
    /// idempotent (`mk--mk--foo` -> `mk--foo` -> `foo`), so re-normalizing
    /// the stored side here would let a grant written for one public alias
    /// admit a different member - a fail-open, not a cleanup. The stored
    /// side is normalized on the way in, by [`Self::members`].
    pub fn contains(&self, member: &str) -> bool {
        match self {
            Self::All => true,
            Self::Members(members) => members.contains(&normalize_member_alias(member)),
        }
    }
}

fn normalize_member_alias(member: &str) -> String {
    crate::member_comms_id::runtime_alias_str(member).into_owned()
}

/// What one caller identity may do on this gateway's control channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlGrant {
    label: String,
    verbs: BTreeSet<ControlVerb>,
    members: ControlMemberScope,
}

impl ControlGrant {
    /// Build a grant. `label` is the operator-facing name of the caller
    /// (its mob id, typically) and is only used for logs and refusal
    /// messages - the authoritative identity is the pubkey this grant is
    /// filed under in the [`ControlGrantTable`].
    pub fn new<I>(label: impl Into<String>, verbs: I, members: ControlMemberScope) -> Self
    where
        I: IntoIterator<Item = ControlVerb>,
    {
        Self {
            label: label.into(),
            verbs: verbs.into_iter().collect(),
            members,
        }
    }

    /// Operator-facing caller name.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Verbs this caller may use.
    pub fn verbs(&self) -> &BTreeSet<ControlVerb> {
        &self.verbs
    }

    /// Members this caller may reach.
    pub fn members(&self) -> &ControlMemberScope {
        &self.members
    }

    /// Check one request against this grant.
    ///
    /// ORDER IS LOAD-BEARING: the verb check runs first and unconditionally,
    /// so the host-plane exemption below is only ever reachable for a verb
    /// the operator named explicitly. Hoisting the exemption above the verb
    /// check would turn it into a bypass.
    fn permits(&self, verb: ControlVerb, member: &str) -> Result<(), ControlAuthzDenial> {
        if !self.verbs.contains(&verb) {
            return Err(ControlAuthzDenial::VerbNotGranted {
                label: self.label.clone(),
                verb,
            });
        }
        // Host-plane verbs address the serving gateway itself, so there is
        // no member to scope. Requiring `members = ["*"]` to reach them
        // would be worse than exempting them: an operator granting a
        // read-only host projection would have to widen the SAME grant's
        // `inject` to every member to do it.
        if verb.is_host_plane() {
            return Ok(());
        }
        if !self.members.contains(member) {
            return Err(ControlAuthzDenial::MemberNotGranted {
                label: self.label.clone(),
                verb,
                member: member.to_string(),
            });
        }
        Ok(())
    }
}

/// Caller pubkey -> grant. Keyed by the 32-byte Ed25519 pubkey because
/// that is the only part of a caller's claim this gateway can verify; the
/// operator-facing label lives inside the grant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControlGrantTable {
    grants: BTreeMap<[u8; 32], ControlGrant>,
}

impl ControlGrantTable {
    /// An empty table. NOTE: an empty table grants NOTHING - it refuses
    /// every caller. Emptiness never means "no policy configured"; that
    /// state is [`ControlAuthorizer::open`], which must be named
    /// explicitly.
    pub fn new() -> Self {
        Self::default()
    }

    /// File a grant under a caller pubkey, replacing any previous one.
    pub fn insert(&mut self, caller_pubkey: [u8; 32], grant: ControlGrant) -> Option<ControlGrant> {
        self.grants.insert(caller_pubkey, grant)
    }

    /// The grant for a caller pubkey, if any.
    pub fn get(&self, caller_pubkey: &[u8; 32]) -> Option<&ControlGrant> {
        self.grants.get(caller_pubkey)
    }

    /// Number of filed callers.
    pub fn len(&self) -> usize {
        self.grants.len()
    }

    /// Whether no caller is filed (i.e. everything is refused).
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    /// Parse a `[control_grants]` section:
    ///
    /// ```toml
    /// [control_grants.ops-mob]
    /// pubkey = "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio="
    /// verbs = ["lookup_member", "wire", "unwire"]
    /// members = ["bob"]          # omit, or ["*"], for every member
    /// ```
    ///
    /// Returns `Ok(None)` when the section is ABSENT, which is what keeps
    /// this backward compatible: a config that never mentions grants must
    /// stay on [`ControlAuthorizer::open`] rather than silently becoming a
    /// deny-all table. A present-but-empty section is `Ok(Some(empty))` -
    /// an operator who wrote the section and listed nobody meant deny-all.
    ///
    /// Keys inside a caller entry are CLOSED: anything other than
    /// `pubkey`, `verbs`, `members` is an error rather than an ignored
    /// key. See the check in the loop for why silence there widens.
    pub fn from_toml(text: &str) -> Result<Option<Self>, ControlGrantConfigError> {
        let table: toml::Value =
            toml::from_str(text).map_err(|err| ControlGrantConfigError::Parse(err.to_string()))?;
        let Some(section) = table.get("control_grants") else {
            return Ok(None);
        };
        let section = section.as_table().ok_or_else(|| {
            ControlGrantConfigError::Parse(
                "control_grants must be a table of caller labels, e.g. [control_grants.ops-mob]"
                    .to_string(),
            )
        })?;
        let mut out = Self::new();
        let mut labels_by_pubkey: BTreeMap<[u8; 32], String> = BTreeMap::new();
        for (label, value) in section {
            let entry = value
                .as_table()
                .ok_or_else(|| ControlGrantConfigError::InvalidField {
                    label: label.clone(),
                    detail: "each caller must be a table with pubkey/verbs/members".to_string(),
                })?;
            // Unknown keys fail closed. Ignoring them WIDENS the grant on a
            // typo: `member = ["bob"]` (singular) leaves `members` absent,
            // and an absent `members` means ControlMemberScope::All - the
            // operator wrote one member and silently got every member,
            // present and future. The same silence would drop a narrowing
            // key a newer config carries but this binary cannot enforce.
            // Notes belong in `#` comments, which never reach this map.
            for (key, _) in entry {
                if !matches!(key.as_str(), "pubkey" | "verbs" | "members") {
                    return Err(ControlGrantConfigError::InvalidField {
                        label: label.clone(),
                        detail: format!(
                            "unknown key '{key}'; a control grant accepts only pubkey, verbs, \
                             and members (did you mean 'members'?)"
                        ),
                    });
                }
            }
            let pubkey_text = entry
                .get("pubkey")
                .and_then(|value| value.as_str())
                .ok_or_else(|| ControlGrantConfigError::InvalidField {
                    label: label.clone(),
                    detail: "missing 'pubkey' (base64 Ed25519, optional ed25519: prefix)"
                        .to_string(),
                })?;
            let pubkey = crate::auth::peer_keys::decode_pubkey_b64(pubkey_text).map_err(|err| {
                ControlGrantConfigError::InvalidPubkey {
                    label: label.clone(),
                    reason: err.to_string(),
                }
            })?;
            if let Some(other) = labels_by_pubkey.get(&pubkey) {
                // Two labels on one key is an ambiguous policy: whichever
                // grant won would be arbitrary. Refuse rather than pick.
                return Err(ControlGrantConfigError::DuplicatePubkey {
                    label: label.clone(),
                    other: other.clone(),
                });
            }
            let verb_values = entry
                .get("verbs")
                .and_then(|value| value.as_array())
                .ok_or_else(|| ControlGrantConfigError::InvalidField {
                    label: label.clone(),
                    detail: "missing 'verbs' array; a caller with no verbs reaches nothing"
                        .to_string(),
                })?;
            let mut verbs = BTreeSet::new();
            for verb_value in verb_values {
                let verb_text =
                    verb_value
                        .as_str()
                        .ok_or_else(|| ControlGrantConfigError::InvalidField {
                            label: label.clone(),
                            detail: "'verbs' entries must be strings".to_string(),
                        })?;
                // `*` expands to the MEMBER plane only. Every `*` grant
                // already written by an operator meant "the four verbs
                // that reach my members"; letting a new enum variant
                // widen it would be a privilege change made by an
                // upgrade instead of by the operator.
                if verb_text == "*" {
                    verbs.extend(ControlVerb::member_plane());
                    continue;
                }
                let verb = ControlVerb::parse(verb_text).ok_or_else(|| {
                    ControlGrantConfigError::UnknownVerb {
                        label: label.clone(),
                        verb: verb_text.to_string(),
                    }
                })?;
                verbs.insert(verb);
            }
            if verbs.is_empty() {
                return Err(ControlGrantConfigError::InvalidField {
                    label: label.clone(),
                    detail: "'verbs' is empty; remove the caller instead of granting nothing"
                        .to_string(),
                });
            }
            let members = match entry.get("members") {
                None => ControlMemberScope::All,
                Some(value) => {
                    let items =
                        value
                            .as_array()
                            .ok_or_else(|| ControlGrantConfigError::InvalidField {
                                label: label.clone(),
                                detail: "'members' must be an array of member aliases".to_string(),
                            })?;
                    let mut aliases = Vec::with_capacity(items.len());
                    for item in items {
                        let alias =
                            item.as_str()
                                .ok_or_else(|| ControlGrantConfigError::InvalidField {
                                    label: label.clone(),
                                    detail: "'members' entries must be strings".to_string(),
                                })?;
                        aliases.push(alias.to_string());
                    }
                    ControlMemberScope::members(aliases)
                }
            };
            labels_by_pubkey.insert(pubkey, label.clone());
            out.insert(pubkey, ControlGrant::new(label.clone(), verbs, members));
        }
        Ok(Some(out))
    }
}

/// Errors parsing a `[control_grants]` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlGrantConfigError {
    Parse(String),
    InvalidPubkey { label: String, reason: String },
    DuplicatePubkey { label: String, other: String },
    UnknownVerb { label: String, verb: String },
    InvalidField { label: String, detail: String },
}

impl std::fmt::Display for ControlGrantConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(reason) => write!(f, "control grant parse error: {reason}"),
            Self::InvalidPubkey { label, reason } => {
                write!(f, "control grant '{label}' has an invalid pubkey: {reason}")
            }
            Self::DuplicatePubkey { label, other } => write!(
                f,
                "control grants '{label}' and '{other}' name the same caller pubkey; one caller \
                 must have exactly one grant"
            ),
            Self::UnknownVerb { label, verb } => write!(
                f,
                "control grant '{label}' names unknown verb '{verb}'; expected one of \
                 wire, unwire, inject, lookup_member (or '*')"
            ),
            Self::InvalidField { label, detail } => {
                write!(f, "control grant '{label}': {detail}")
            }
        }
    }
}

impl std::error::Error for ControlGrantConfigError {}

/// A caller whose credential verified and whose grant admitted the
/// request. Returned for logging and attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedCaller {
    pub label: String,
    pub pubkey: [u8; 32],
}

/// Caller-authorization policy for a control listener.
///
/// Two modes, and only two, so "no policy" can never be spelled the same
/// way as "empty policy":
///
/// * [`Self::Open`] - authorize nothing. Every decodable request is
///   dispatched, exactly as before caller authorization existed, except a
///   keyed-inject-plane verb, which only a grant naming it admits and which is
///   therefore refused here. Any credential a caller stamps is left
///   UNVERIFIED here on purpose: there is nothing to authorize, and
///   verifying would turn a future payload revision into a cross-version
///   outage for deployments that never opted in.
/// * [`Self::Grants`] - enforce. A request must carry a credential that
///   verifies, name a caller filed in the table, and fall inside that
///   caller's verbs and member scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAuthorizer {
    Open,
    Grants {
        table: ControlGrantTable,
        /// When set, requests must have been minted for this audience
        /// (the mob id peers use for this gateway in their contact
        /// directories).
        expected_audience: Option<String>,
    },
}

impl ControlAuthorizer {
    /// No caller authorization - the pre-grant behaviour.
    pub fn open() -> Self {
        Self::Open
    }

    /// Enforce `table`. An EMPTY table refuses every caller; emptiness is
    /// never a fallback to [`Self::open`].
    pub fn with_grants(table: ControlGrantTable) -> Self {
        Self::Grants {
            table,
            expected_audience: None,
        }
    }

    /// Enforce `table` and additionally require that requests were minted
    /// for `audience` - the mob id peers name this gateway by. Use when
    /// one caller holds grants on several gateways and a captured request
    /// must not be replayable across them.
    pub fn with_grants_for_audience(table: ControlGrantTable, audience: impl Into<String>) -> Self {
        Self::Grants {
            table,
            expected_audience: Some(audience.into()),
        }
    }

    /// Parse a `[control_grants]` section into an authorizer: absent
    /// section yields [`Self::Open`], present section yields
    /// [`Self::Grants`].
    pub fn from_toml(text: &str) -> Result<Self, ControlGrantConfigError> {
        Ok(match ControlGrantTable::from_toml(text)? {
            Some(table) => Self::with_grants(table),
            None => Self::Open,
        })
    }

    /// Whether this authorizer refuses anything at all.
    pub fn is_enforcing(&self) -> bool {
        matches!(self, Self::Grants { .. })
    }

    /// Authorize one request. `Ok(None)` means the listener does not
    /// enforce grants; `Ok(Some(caller))` names the admitted caller.
    pub fn authorize(
        &self,
        request: &ControlRequest,
    ) -> Result<Option<AuthorizedCaller>, ControlAuthzDenial> {
        let verb = request.verb();
        let Self::Grants {
            table,
            expected_audience,
        } = self
        else {
            // Open authorizes nothing, so it cannot admit a verb that is
            // served only under a grant naming it.
            if verb.is_keyed_inject_plane() {
                return Err(ControlAuthzDenial::GrantsRequired { verb });
            }
            return Ok(None);
        };
        let Some(caller) = request.caller() else {
            return Err(ControlAuthzDenial::UnauthenticatedCaller { verb });
        };
        let pubkey =
            crate::auth::peer_keys::decode_pubkey_b64(&caller.pubkey_b64).map_err(|err| {
                ControlAuthzDenial::InvalidCallerCredential {
                    reason: format!("caller pubkey: {err}"),
                }
            })?;
        // Verify the signature BEFORE consulting the table, so an
        // unauthenticated prober cannot use the difference between
        // "not granted" and "bad signature" to enumerate which pubkeys
        // hold grants here.
        verify_caller_signature(&pubkey, caller, request)?;
        let Some(grant) = table.get(&pubkey) else {
            return Err(ControlAuthzDenial::CallerNotGranted {
                pubkey_b64: caller.pubkey_b64.clone(),
            });
        };
        if let Some(expected) = expected_audience.as_deref() {
            let presented = caller.audience.as_deref().unwrap_or("");
            if presented != expected {
                return Err(ControlAuthzDenial::AudienceMismatch {
                    label: grant.label().to_string(),
                    expected: expected.to_string(),
                    presented: presented.to_string(),
                });
            }
        }
        grant.permits(verb, request.remote_member())?;
        Ok(Some(AuthorizedCaller {
            label: grant.label().to_string(),
            pubkey,
        }))
    }
}

fn verify_caller_signature(
    pubkey: &[u8; 32],
    caller: &ControlCaller,
    request: &ControlRequest,
) -> Result<(), ControlAuthzDenial> {
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&caller.sig_b64)
        .map_err(|err| ControlAuthzDenial::InvalidCallerCredential {
            reason: format!("caller signature is not valid base64: {err}"),
        })?;
    let sig_bytes: [u8; 64] =
        sig_bytes
            .try_into()
            .map_err(|_| ControlAuthzDenial::InvalidCallerCredential {
                reason: "caller signature must be 64 bytes".to_string(),
            })?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(pubkey).map_err(|err| {
        ControlAuthzDenial::InvalidCallerCredential {
            reason: format!("caller pubkey is not a valid Ed25519 key: {err}"),
        }
    })?;
    let payload = control_request_signing_payload(request);
    verifying_key
        .verify_strict(payload.as_bytes(), &signature)
        .map_err(|_| ControlAuthzDenial::InvalidCallerSignature {
            pubkey_b64: caller.pubkey_b64.clone(),
        })
}

/// Open a control connection to a remote endpoint.
///
/// TCP and UDS share the same length-prefixed JSON framing, so we wrap
/// the underlying `TcpStream` / `UnixStream` in a small enum and write
/// codec-agnostic helpers below.
enum ControlStream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Uds(UnixStream),
}

impl ControlStream {
    async fn write_frame(&mut self, payload: &[u8]) -> Result<(), std::io::Error> {
        let len = u32::try_from(payload.len()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "payload too large")
        })?;
        let header = len.to_be_bytes();
        match self {
            Self::Tcp(s) => {
                s.write_all(&header).await?;
                s.write_all(payload).await?;
                s.flush().await
            }
            #[cfg(unix)]
            Self::Uds(s) => {
                s.write_all(&header).await?;
                s.write_all(payload).await?;
                s.flush().await
            }
        }
    }

    async fn read_frame(&mut self) -> Result<Vec<u8>, std::io::Error> {
        let mut header = [0u8; 4];
        match self {
            Self::Tcp(s) => s.read_exact(&mut header).await?,
            #[cfg(unix)]
            Self::Uds(s) => s.read_exact(&mut header).await?,
        };
        let len = u32::from_be_bytes(header);
        if len > MAX_CONTROL_PAYLOAD {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("frame too large: {len} bytes"),
            ));
        }
        let mut buf = vec![0u8; len as usize];
        match self {
            Self::Tcp(s) => s.read_exact(&mut buf).await?,
            #[cfg(unix)]
            Self::Uds(s) => s.read_exact(&mut buf).await?,
        };
        Ok(buf)
    }
}

/// Client side of the cross-mob control protocol.
pub struct RemoteControlClient;

impl RemoteControlClient {
    /// Send `request` to `endpoint`, await one response, and return it.
    ///
    /// Opens a fresh connection per request - control RPC frequency is
    /// low (one message per wire/unwire/inject call) and lazy-reconnect
    /// keeps the implementation simple. Connection pooling can come later
    /// if profiling ever shows it matters.
    pub async fn send(
        endpoint: &RemoteEndpoint,
        request: &ControlRequest,
        timeout: Duration,
    ) -> Result<ControlResponse, RemoteMobError> {
        let payload =
            serde_json::to_vec(request).map_err(|err| encode_error(endpoint, err.to_string()))?;
        Self::send_payload(endpoint, &payload, timeout).await
    }

    /// Send pre-serialized request bytes. Callers that verify signed
    /// responses use this form so the exact bytes on the wire (the input
    /// to the response's request digest) are in their hands.
    pub async fn send_payload(
        endpoint: &RemoteEndpoint,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<ControlResponse, RemoteMobError> {
        tokio::time::timeout(timeout, Self::send_inner(endpoint, payload))
            .await
            .map_err(|_| RemoteMobError::ControlChannelUnavailable {
                mob_id: String::new(),
                endpoint: endpoint.comms_address(),
                operation: "timeout",
            })?
    }

    async fn send_inner(
        endpoint: &RemoteEndpoint,
        payload: &[u8],
    ) -> Result<ControlResponse, RemoteMobError> {
        let mut stream = match endpoint {
            RemoteEndpoint::Tcp(addr) => ControlStream::Tcp(
                TcpStream::connect(addr)
                    .await
                    .map_err(|err| io_error("connect", endpoint, err))?,
            ),
            #[cfg(unix)]
            RemoteEndpoint::Uds(path) => ControlStream::Uds(
                UnixStream::connect(std::path::Path::new(path))
                    .await
                    .map_err(|err| io_error("connect", endpoint, err))?,
            ),
            #[cfg(not(unix))]
            RemoteEndpoint::Uds(_) => {
                return Err(RemoteMobError::UnsupportedTransport {
                    mob_id: String::new(),
                    transport: endpoint.comms_address(),
                });
            }
        };
        stream
            .write_frame(payload)
            .await
            .map_err(|err| io_error("write", endpoint, err))?;
        let response_payload = stream
            .read_frame()
            .await
            .map_err(|err| io_error("read", endpoint, err))?;
        serde_json::from_slice::<ControlResponse>(&response_payload)
            .map_err(|err| decode_error(endpoint, err.to_string()))
    }
}

fn io_error(stage: &'static str, endpoint: &RemoteEndpoint, err: std::io::Error) -> RemoteMobError {
    RemoteMobError::ControlChannelUnavailable {
        mob_id: String::new(),
        endpoint: endpoint.comms_address(),
        operation: match stage {
            "connect" => "connect",
            "write" => "write",
            "read" => "read",
            _ => "io",
        },
    }
    .with_context(err.to_string())
}

fn encode_error(endpoint: &RemoteEndpoint, message: String) -> RemoteMobError {
    RemoteMobError::Encode {
        endpoint: endpoint.comms_address(),
        message,
    }
}

fn decode_error(endpoint: &RemoteEndpoint, message: String) -> RemoteMobError {
    RemoteMobError::Decode {
        endpoint: endpoint.comms_address(),
        message,
    }
}

/// Asynchronously dispatch a single decoded `ControlRequest` against a
/// local mob handle, returning the response shape.
///
/// Boxed-future trait so we can store this in a `dyn` field without
/// requiring `async-trait` and so the listener doesn't need a generic
/// type parameter for the handler.
pub trait ControlHandler: Send + Sync + 'static {
    fn handle(
        &self,
        request: ControlRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>>;

    /// [`Self::handle`] for a request the listener's grant policy admitted,
    /// with the caller it admitted (`None` when the listener enforces no
    /// grants). The listener dispatches through this. Keyed inject verbs
    /// are scoped to their caller; the default ignores it.
    fn handle_admitted(
        &self,
        request: ControlRequest,
        caller: Option<AuthorizedCaller>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>> {
        let _ = caller;
        self.handle(request)
    }
}

/// Real `ControlHandler` that dispatches requests against a local
/// `MobHandle`. Constructed by `UnifiedRuntime::start_control_listener`
/// when a control listener is configured (builder `control_listen()` or
/// the meta_gateway `--control-listen` flag).
pub struct MobHandleControlHandler {
    handle: meerkat_mob::MobHandle,
    identity_authority: ControlIdentityAuthority,
    /// Session-service access for member-fact resolution: `LookupMember`
    /// reaches through it to the member's live comms runtime for the
    /// advertised envelope-listener address (the roster does not persist
    /// transport addresses). `None` degrades LookupMember to roster facts
    /// only, which remote wire callers reject fail-closed.
    session_service: Option<std::sync::Arc<dyn meerkat_mob::MobSessionService>>,
    /// Live source of the read-only runtime-host projections. An empty slot
    /// answers [`super::remote_host::HOST_PLANE_UNAVAILABLE_CODE`] to both host
    /// verbs. UnifiedRuntime populates it only when a gateway signing identity
    /// is present, so facts and endpoint identity are always served under an
    /// authenticated host key while both bootstrap orders remain supported.
    host_facts: HostFactsProviderSlot,
    /// Late-bound serving side of the keyed inject plane. An empty slot
    /// answers both verbs with a signed `keyed_inject_unavailable` refusal.
    keyed_inject: super::keyed_inject::KeyedInjectSlot,
    /// Whether the handler serves a candidate launch, which rewires no
    /// member and admits no member input: `Wire`, `Unwire` and `Inject`
    /// are refused with [`CANDIDATE_LAUNCH_CONTROL_CODE`].
    candidate_launch: bool,
}

/// How the handler resolves the durable identity authority for generated
/// member aliases. `Shared` re-reads a host-owned slot on every request:
/// gateways attach identity-first AFTER the base runtime (and thus after
/// the control listener) exists, so capturing `identity_runtime()` at
/// listener start would permanently capture `None`.
enum ControlIdentityAuthority {
    None,
    /// Supplied by the host with the handler: the owning public view.
    Fixed(Box<crate::identity_first::PublicIdentityRuntime>),
    Shared(
        std::sync::Arc<
            std::sync::RwLock<Option<std::sync::Arc<crate::identity_first::IdentityRuntime>>>,
        >,
    ),
}

impl ControlIdentityAuthority {
    fn current(&self) -> Option<std::sync::Arc<crate::identity_first::IdentityRuntime>> {
        match self {
            Self::None => None,
            Self::Fixed(runtime) => Some(std::sync::Arc::clone(runtime.runtime())),
            Self::Shared(slot) => slot
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        }
    }
}

impl MobHandleControlHandler {
    /// Construct a classic member-plane handler. Reserved generated aliases
    /// fail closed because no durable identity authority was supplied.
    pub fn new(handle: meerkat_mob::MobHandle) -> Self {
        Self {
            handle,
            identity_authority: ControlIdentityAuthority::None,
            session_service: None,
            host_facts: empty_host_facts_provider(),
            keyed_inject: super::keyed_inject::pending_keyed_inject_slot(),
            candidate_launch: false,
        }
    }

    /// Construct an identity-aware handler that pins every generated-alias
    /// operation to its current durable generation.
    pub fn with_identity_runtime(
        handle: meerkat_mob::MobHandle,
        identity_runtime: crate::identity_first::PublicIdentityRuntime,
    ) -> Self {
        Self {
            handle,
            identity_authority: ControlIdentityAuthority::Fixed(Box::new(identity_runtime)),
            session_service: None,
            host_facts: empty_host_facts_provider(),
            keyed_inject: super::keyed_inject::pending_keyed_inject_slot(),
            candidate_launch: false,
        }
    }

    /// Construct a handler whose identity authority is re-read from a
    /// host-owned slot on every request, so identity-first attachment that
    /// happens after the listener starts is still honored.
    pub fn with_shared_identity_authority(
        handle: meerkat_mob::MobHandle,
        identity_slot: std::sync::Arc<
            std::sync::RwLock<Option<std::sync::Arc<crate::identity_first::IdentityRuntime>>>,
        >,
    ) -> Self {
        Self {
            handle,
            identity_authority: ControlIdentityAuthority::Shared(identity_slot),
            session_service: None,
            host_facts: empty_host_facts_provider(),
            keyed_inject: super::keyed_inject::pending_keyed_inject_slot(),
            candidate_launch: false,
        }
    }

    /// Attach the session service that owns member comms runtimes, so
    /// `LookupMember` can report each member's advertised envelope-listener
    /// address alongside its roster facts.
    #[must_use]
    pub fn with_session_service(
        mut self,
        session_service: std::sync::Arc<dyn meerkat_mob::MobSessionService>,
    ) -> Self {
        self.session_service = Some(session_service);
        self
    }

    /// Serve the read-only runtime-host projections from `provider`.
    ///
    /// Until a host installs one, `HostDescribe` / `HostHealth` are
    /// refused typed. The provider is a pure projection source: nothing
    /// it returns can mutate this gateway's mob, and nothing a caller
    /// sends through these verbs reaches the mob at all.
    #[must_use]
    pub fn with_host_facts(
        self,
        provider: std::sync::Arc<dyn super::remote_host::HostFactsProvider>,
    ) -> Self {
        *self
            .host_facts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(provider);
        self
    }

    /// Share a host-owned facts slot that may be populated after this handler
    /// starts serving. Every host-plane request re-reads it.
    #[must_use]
    pub fn with_host_facts_slot(mut self, slot: HostFactsProviderSlot) -> Self {
        self.host_facts = slot;
        self
    }

    /// Serve the keyed inject plane from a host-owned slot that the
    /// gateway's mob announcement composition may populate after this
    /// handler starts serving. Every keyed inject request re-reads it.
    #[must_use]
    pub fn with_keyed_inject_slot(mut self, slot: super::keyed_inject::KeyedInjectSlot) -> Self {
        self.keyed_inject = slot;
        self
    }

    /// Serve a candidate launch (`candidate_launch` set): `Wire`, `Unwire`
    /// and `Inject` are refused with [`CANDIDATE_LAUNCH_CONTROL_CODE`]
    /// before they reach the mob, since a candidate rewires no member and
    /// admits no member input. The read verbs and the keyed inject plane are
    /// served as on any launch.
    #[must_use]
    pub(crate) fn with_candidate_launch(mut self, candidate_launch: bool) -> Self {
        self.candidate_launch = candidate_launch;
        self
    }
}

/// The code a candidate launch's control listener refuses a member-changing
/// verb with: the `data.kind` its RPC surfaces refuse member changes with.
pub(crate) const CANDIDATE_LAUNCH_CONTROL_CODE: &str = "candidate_launch";

/// What a candidate launch's control listener answers `verb` with when the
/// verb changes a member: `Wire` and `Unwire` rewire it, and `Inject`
/// admits input to it. `None` for the verbs a candidate serves.
fn candidate_launch_control_refusal(verb: ControlVerb) -> Option<ControlResponse> {
    match verb {
        ControlVerb::Wire | ControlVerb::Unwire | ControlVerb::Inject => {
            Some(ControlResponse::Err {
                code: CANDIDATE_LAUNCH_CONTROL_CODE.to_string(),
                message: format!(
                    "this gateway is a candidate launch: it rewires no member and admits no \
                     member input, so '{verb}' is refused; send it to the launch that speaks \
                     for the composition"
                ),
            })
        }
        ControlVerb::LookupMember
        | ControlVerb::HostDescribe
        | ControlVerb::HostHealth
        | ControlVerb::InjectKeyed
        | ControlVerb::InjectStatus => None,
    }
}

/// The typed refusal a gateway without a host-facts provider answers.
fn host_plane_unavailable(verb: ControlVerb) -> ControlResponse {
    ControlResponse::Err {
        code: super::remote_host::HOST_PLANE_UNAVAILABLE_CODE.to_string(),
        message: format!(
            "this gateway serves no runtime-host facts, so '{verb}' has nothing to answer; \
             install one with MobHandleControlHandler::with_host_facts"
        ),
    }
}

/// Answer `HostDescribe`.
///
/// Split out of the dispatch match so the absent-provider path is
/// exercisable without constructing a `MobHandle`: the test calls the
/// same function the handler does, rather than a restatement of it.
fn host_describe_response(
    provider: Option<&dyn super::remote_host::HostFactsProvider>,
) -> ControlResponse {
    match provider {
        Some(provider) => ControlResponse::Host {
            facts: provider.describe(),
            sig_b64: None,
        },
        None => host_plane_unavailable(ControlVerb::HostDescribe),
    }
}

/// Answer `HostHealth`. See [`host_describe_response`].
fn host_health_response(
    provider: Option<&dyn super::remote_host::HostFactsProvider>,
) -> ControlResponse {
    match provider {
        Some(provider) => ControlResponse::HostHealth {
            health: provider.health(),
            sig_b64: None,
        },
        None => host_plane_unavailable(ControlVerb::HostHealth),
    }
}

fn current_host_facts(
    slot: &HostFactsProviderSlot,
) -> Option<std::sync::Arc<dyn super::remote_host::HostFactsProvider>> {
    slot.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

impl ControlHandler for MobHandleControlHandler {
    fn handle(
        &self,
        request: ControlRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>> {
        let handle = self.handle.clone();
        let identity_runtime = self.identity_authority.current();
        let session_service = self.session_service.clone();
        let host_facts = self.host_facts.clone();
        let keyed_inject = self.keyed_inject.clone();
        let candidate_launch = self.candidate_launch;
        Box::pin(async move {
            if candidate_launch
                && let Some(refusal) = candidate_launch_control_refusal(request.verb())
            {
                return refusal;
            }
            // `nonce` and `caller` are consumed at the listener seam
            // (response signing binds the nonce via the request digest;
            // `serve_one` authorizes the caller before dispatch), so the
            // handler never re-derives authority from them.
            match request {
                ControlRequest::Wire {
                    remote_member,
                    local_peer_spec_address,
                    local_comms_name,
                    local_peer_id,
                    local_pubkey_b64,
                    nonce: _,
                    caller: _,
                } => {
                    handle_wire(
                        &handle,
                        WireControlParams {
                            remote_member: &remote_member,
                            local_peer_spec_address: &local_peer_spec_address,
                            local_comms_name: &local_comms_name,
                            local_peer_id: &local_peer_id,
                            local_pubkey_b64: local_pubkey_b64.as_deref(),
                            wire: true,
                        },
                        identity_runtime.as_ref(),
                    )
                    .await
                }
                ControlRequest::Unwire {
                    remote_member,
                    local_peer_spec_address,
                    local_comms_name,
                    local_peer_id,
                    local_pubkey_b64,
                    nonce: _,
                    caller: _,
                } => {
                    handle_wire(
                        &handle,
                        WireControlParams {
                            remote_member: &remote_member,
                            local_peer_spec_address: &local_peer_spec_address,
                            local_comms_name: &local_comms_name,
                            local_peer_id: &local_peer_id,
                            local_pubkey_b64: local_pubkey_b64.as_deref(),
                            wire: false,
                        },
                        identity_runtime.as_ref(),
                    )
                    .await
                }
                ControlRequest::Inject {
                    remote_member,
                    content,
                    nonce: _,
                    caller: _,
                } => {
                    handle_inject(&handle, &remote_member, content, identity_runtime.as_ref()).await
                }
                ControlRequest::LookupMember {
                    remote_member,
                    nonce: _,
                    caller: _,
                } => {
                    handle_lookup_member(
                        &handle,
                        session_service.as_ref(),
                        &remote_member,
                        identity_runtime.as_ref(),
                    )
                    .await
                }
                // The host plane never touches `handle`: these two arms
                // read a projection and return it. A runtime host that
                // could reach the mob from here would be the competing
                // authority the design forbids.
                ControlRequest::HostDescribe {
                    nonce: _,
                    caller: _,
                } => {
                    let provider = current_host_facts(&host_facts);
                    host_describe_response(provider.as_deref())
                }
                ControlRequest::HostHealth {
                    nonce: _,
                    caller: _,
                } => {
                    let provider = current_host_facts(&host_facts);
                    host_health_response(provider.as_deref())
                }
                // The keyed inject plane never touches `handle` either: the
                // gateway's mob announcement composition reaches its own
                // continuation owner.
                // Reached only without an admitting caller: a keyed inject
                // verb is answered through `handle_admitted`, scoped to it.
                request @ (ControlRequest::InjectKeyed { .. }
                | ControlRequest::InjectStatus { .. }) => {
                    super::keyed_inject::answer_keyed_inject_request(&keyed_inject, request, None)
                        .await
                }
            }
        })
    }

    fn handle_admitted(
        &self,
        request: ControlRequest,
        caller: Option<AuthorizedCaller>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>> {
        if !request.verb().is_keyed_inject_plane() {
            return self.handle(request);
        }
        let keyed_inject = self.keyed_inject.clone();
        Box::pin(async move {
            super::keyed_inject::answer_keyed_inject_request(
                &keyed_inject,
                request,
                caller.as_ref(),
            )
            .await
        })
    }
}

fn control_identity_error(error: crate::identity_first::IdentityRuntimeError) -> (String, String) {
    let code = match error {
        crate::identity_first::IdentityRuntimeError::StaleRuntimeAlias { .. } => {
            "stale_runtime_alias"
        }
        crate::identity_first::IdentityRuntimeError::UnknownIdentity(_) => "unknown_identity",
        crate::identity_first::IdentityRuntimeError::InvalidState { .. } => "identity_not_active",
        _ => "identity_error",
    };
    (code.to_string(), error.to_string())
}

const TRACKED_CONTROL_ERROR_PREFIX: &str = "meta-control-operation:";

fn tracked_control_error(error: crate::identity_first::IdentityRuntimeError) -> (String, String) {
    if let crate::identity_first::IdentityRuntimeError::Internal(message) = &error
        && let Some(encoded) = message.strip_prefix(TRACKED_CONTROL_ERROR_PREFIX)
        && let Ok((code, message)) = serde_json::from_str::<(String, String)>(encoded)
    {
        return (code, message);
    }
    control_identity_error(error)
}

async fn run_control_member_operation<T, F, Fut>(
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    member_alias: &str,
    operation: F,
) -> Result<T, (String, String)>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, (String, String)>> + Send + 'static,
{
    let member_alias = crate::member_comms_id::runtime_alias_str(member_alias).into_owned();
    if let Some(runtime) = identity_runtime {
        let target = runtime
            .member_alias_lifecycle_target(&member_alias)
            .await
            .map_err(control_identity_error)?;
        if let Some(target) = target {
            return crate::identity_first::IdentityRuntime::run_member_alias_targets_operation_tracked(
                vec![target],
                move || async move {
                    operation().await.map_err(|error| {
                        format!(
                            "{TRACKED_CONTROL_ERROR_PREFIX}{}",
                            serde_json::to_string(&error)
                                .unwrap_or_else(|_| "[\"identity_error\",\"unserializable control error\"]".to_string())
                        )
                    })
                },
            )
            .await
            .map_err(tracked_control_error);
        }
    }
    if crate::member_comms_id::is_reserved_generated_alias(&member_alias) {
        return Err((
            "identity_authority_unavailable".to_string(),
            format!("generated member alias '{member_alias}' requires the owning IdentityRuntime"),
        ));
    }
    operation().await
}

struct WireControlParams<'a> {
    remote_member: &'a str,
    local_peer_spec_address: &'a str,
    local_comms_name: &'a str,
    local_peer_id: &'a str,
    local_pubkey_b64: Option<&'a str>,
    wire: bool,
}

/// Build the `TrustedPeerDescriptor` a remote Wire/Unwire request installs
/// on this side.
///
/// Requests arriving over the remote control channel must carry a peer
/// address that is dialable from THIS process and the calling MEMBER's own
/// transport pubkey. `inproc://` fails closed: it used to slip through the
/// relaxed unsigned branch and install a descriptor that could never
/// deliver an envelope across processes. A missing pubkey also fails
/// closed: meerkat-comms keys its trust store by pubkey, so an unsigned
/// row would admit any sender at ingress. The gateway key is not accepted
/// either - `unsigned_with_pubkey` enforces peer_id == UUIDv5(pubkey), and
/// only the member's transport key satisfies that.
fn build_remote_wire_descriptor(
    params: &WireControlParams<'_>,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, (String, String)> {
    if params.local_peer_spec_address.starts_with("inproc://") {
        return Err((
            "inproc_address_rejected".to_string(),
            format!(
                "peer address '{}' is inproc:// and unreachable from another process; the \
                 calling gateway must advertise a tcp:// or uds:// member comms address",
                params.local_peer_spec_address
            ),
        ));
    }
    let Some(pubkey_b64) = params.local_pubkey_b64.filter(|value| !value.is_empty()) else {
        return Err((
            "missing_pubkey".to_string(),
            "remote wire requests must carry the calling member's Ed25519 transport pubkey; \
             an unsigned descriptor would admit any sender at comms ingress"
                .to_string(),
        ));
    };
    let pubkey = crate::auth::peer_keys::decode_pubkey_b64(pubkey_b64)
        .map_err(|err| ("decode".to_string(), format!("local_pubkey_b64: {err}")))?;
    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
        params.local_comms_name,
        params.local_peer_id,
        pubkey,
        params.local_peer_spec_address,
    )
    .map_err(|err| ("peer_spec".to_string(), err))
}

async fn handle_wire(
    handle: &meerkat_mob::MobHandle,
    params: WireControlParams<'_>,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
) -> ControlResponse {
    let spec = match build_remote_wire_descriptor(&params) {
        Ok(spec) => spec,
        Err((code, message)) => return ControlResponse::Err { code, message },
    };
    // The control plane speaks the public alias space; the local roster
    // holds comms-safe encoded ids (meerkat 0.7 MemberCommsName), so encode
    // at this boundary.
    let operation_handle = handle.clone();
    let operation_member =
        crate::member_comms_id::runtime_alias_str(params.remote_member).into_owned();
    let wire = params.wire;
    let result =
        run_control_member_operation(identity_runtime, params.remote_member, move || async move {
            // `run_control_member_operation` gates this closure: a generated
            // alias here is the identity's current incarnation, held under its
            // lifecycle lock, so it names the durable roster row.
            let mid = crate::member_comms_id::roster_member_id_for_supplied_id(&operation_member);
            let result = if wire {
                operation_handle
                    .wire(mid, meerkat_mob::PeerTarget::External(spec))
                    .await
            } else {
                operation_handle
                    .unwire(mid, meerkat_mob::PeerTarget::External(spec))
                    .await
            };
            result.map_err(|error| ("mob_error".to_string(), error.to_string()))
        })
        .await;
    match result {
        Ok(()) => ControlResponse::Ok { sig_b64: None },
        Err((code, message)) => ControlResponse::Err { code, message },
    }
}

async fn handle_inject(
    handle: &meerkat_mob::MobHandle,
    remote_member: &str,
    content: serde_json::Value,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
) -> ControlResponse {
    let content_input: meerkat_core::ContentInput = match serde_json::from_value(content) {
        Ok(c) => c,
        Err(err) => {
            return ControlResponse::Err {
                code: "decode".to_string(),
                message: format!("content: {err}"),
            };
        }
    };
    let operation_handle = handle.clone();
    let operation_member = crate::member_comms_id::runtime_alias_str(remote_member).into_owned();
    match run_control_member_operation(identity_runtime, remote_member, move || async move {
        // Gated by `run_control_member_operation`, as in `handle_wire`.
        let mid = crate::member_comms_id::roster_member_id_for_supplied_id(&operation_member);
        operation_handle
            .member(&mid)
            .await
            .map_err(|error| ("unknown_member".to_string(), error.to_string()))?
            .send(content_input, meerkat_core::types::HandlingMode::Queue)
            .await
            .map_err(|error| ("mob_error".to_string(), error.to_string()))?;
        operation_handle
            .resolve_bridge_session_id(&mid)
            .await
            .map(|session_id| session_id.to_string())
            .ok_or_else(|| {
                (
                    "no_session".to_string(),
                    format!("member '{operation_member}' has no bound bridge session"),
                )
            })
    })
    .await
    {
        Ok(session_id) => ControlResponse::Injected {
            session_id,
            sig_b64: None,
        },
        Err((code, message)) => ControlResponse::Err { code, message },
    }
}

async fn handle_lookup_member(
    handle: &meerkat_mob::MobHandle,
    session_service: Option<&std::sync::Arc<dyn meerkat_mob::MobSessionService>>,
    remote_member: &str,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
) -> ControlResponse {
    let operation_handle = handle.clone();
    let operation_service = session_service.cloned();
    let operation_member = crate::member_comms_id::runtime_alias_str(remote_member).into_owned();
    match run_control_member_operation(identity_runtime, remote_member, move || async move {
        handle_lookup_member_raw(
            &operation_handle,
            operation_service.as_ref(),
            &operation_member,
        )
        .await
    })
    .await
    {
        Ok(response) => response,
        Err((code, message)) => ControlResponse::Err { code, message },
    }
}

/// Where a member's runtime lives, classified once per operation from one
/// `MobHandle::member_endpoint_status` snapshot. Placement decides, never the
/// presence of a local comms runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemberPlane {
    /// The member's runtime lives in this process (or has none yet).
    Local,
    /// The member is placed on a remote host and is reachable only at this
    /// durable host-acknowledged endpoint.
    Placed(meerkat_core::comms::TrustedPeerDescriptor),
}

impl MemberPlane {
    pub(crate) fn is_placed(&self) -> bool {
        matches!(self, Self::Placed(_))
    }
}

/// Why a member's plane could not be classified.
#[derive(Debug)]
pub(crate) enum MemberPlaneFault {
    /// A placed member without a usable endpoint (Broken, or none
    /// registered). Never downgraded to a local `inproc` address.
    PlacedUnavailable { reason: String },
    /// The endpoint query itself failed.
    Mob(meerkat_mob::MobError),
}

impl std::fmt::Display for MemberPlaneFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlacedUnavailable { reason } => {
                write!(f, "placed member has no usable endpoint: {reason}")
            }
            Self::Mob(error) => write!(f, "{error}"),
        }
    }
}

/// Classify where `member`'s runtime lives. An absent member classifies as
/// `Local`; callers report its absence through their own member lookup.
pub(crate) async fn member_plane(
    handle: &meerkat_mob::MobHandle,
    member: &meerkat_mob::ids::AgentIdentity,
) -> Result<MemberPlane, MemberPlaneFault> {
    match handle
        .member_endpoint_status(member)
        .await
        .map_err(MemberPlaneFault::Mob)?
    {
        Some(meerkat_mob::MobMemberEndpointStatus::Host(descriptor)) => {
            Ok(MemberPlane::Placed(descriptor))
        }
        Some(meerkat_mob::MobMemberEndpointStatus::HostUnavailable { reason }) => {
            Err(MemberPlaneFault::PlacedUnavailable { reason })
        }
        Some(
            meerkat_mob::MobMemberEndpointStatus::Local(_)
            | meerkat_mob::MobMemberEndpointStatus::LocalUnavailable { .. },
        )
        | None => Ok(MemberPlane::Local),
        Some(other) => Err(MemberPlaneFault::PlacedUnavailable {
            reason: format!("unrecognized endpoint status {other:?}"),
        }),
    }
}

/// Where a member can be dialed across processes, in the same resolution
/// order as every other cross-mob decision (placement first):
///
/// * a placed member answers with its durable host-acknowledged address;
/// * a placed member without a usable endpoint is a typed fault, never a
///   local `inproc` address;
/// * a local member answers with its live runtime's advertised address
///   (`tcp://host:port`, `uds:///path`, or `inproc://name`, which remote
///   callers reject), or `None` without a live runtime (its registered
///   address may be stale).
pub(crate) async fn member_dialable_address(
    handle: &meerkat_mob::MobHandle,
    session_service: Option<&std::sync::Arc<dyn meerkat_mob::MobSessionService>>,
    member: &meerkat_mob::ids::AgentIdentity,
) -> Result<Option<String>, MemberPlaneFault> {
    if let MemberPlane::Placed(descriptor) = member_plane(handle, member).await? {
        return Ok(Some(descriptor.address.to_string()));
    }
    if let Some(service) = session_service
        && let Some(session_id) = handle.resolve_bridge_session_id(member).await
        && let Some(comms) = service.comms_runtime(&session_id).await
    {
        return Ok(comms.advertised_address());
    }
    Ok(None)
}

/// The address a peer-info read reports for `member`: a placed member's
/// durable host address, otherwise the in-process `inproc://{comms_name}`
/// route a same-process caller installs.
pub(crate) async fn member_peer_info_address(
    handle: &meerkat_mob::MobHandle,
    member: &meerkat_mob::ids::AgentIdentity,
    comms_name: &str,
) -> Result<String, MemberPlaneFault> {
    Ok(peer_info_address_for(
        &member_plane(handle, member).await?,
        comms_name,
    ))
}

/// Pure peer-info address choice: a placed member's durable host address,
/// otherwise the in-process route.
pub(crate) fn peer_info_address_for(plane: &MemberPlane, comms_name: &str) -> String {
    match plane {
        MemberPlane::Placed(descriptor) => descriptor.address.to_string(),
        MemberPlane::Local => format!("inproc://{comms_name}"),
    }
}

/// The typed LookupMember error for a plane fault.
pub(crate) fn lookup_address_fault(fault: MemberPlaneFault) -> (String, String) {
    match fault {
        MemberPlaneFault::PlacedUnavailable { .. } => {
            ("placed_member_unavailable".to_string(), fault.to_string())
        }
        MemberPlaneFault::Mob(error) => ("mob_error".to_string(), error.to_string()),
    }
}

/// Look a member up by `remote_member` with no identity gate of its own.
///
/// `remote_member` must already have passed `run_control_member_operation`
/// (as `handle_lookup_member` does): a generated alias that reaches here is
/// the identity's current incarnation, so it names the durable roster row.
pub(crate) async fn handle_lookup_member_raw(
    handle: &meerkat_mob::MobHandle,
    session_service: Option<&std::sync::Arc<dyn meerkat_mob::MobSessionService>>,
    remote_member: &str,
) -> Result<ControlResponse, (String, String)> {
    let mid = crate::member_comms_id::roster_member_id_for_supplied_id(remote_member);
    let mob_id = handle.mob_id().to_string();
    let entry = match handle.get_member(&mid).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            return Err((
                "unknown_member".to_string(),
                format!("member '{remote_member}' not in mob '{mob_id}'"),
            ));
        }
        // A faulted lookup is a mob error, not an unknown member.
        Err(err) => {
            return Err(("mob_error".to_string(), err.to_string()));
        }
    };
    let peer_id = match entry.peer_id() {
        Some(p) => p.to_string(),
        None => {
            return Err((
                "no_comms".to_string(),
                format!("member '{remote_member}' has no comms runtime"),
            ));
        }
    };
    // The comms name derives from the roster id (the comms-safe encoding),
    // not the public alias. Build it through meerkat_core::MemberCommsName::new,
    // the fail-closed typed owner meerkat-mob routes all such names through, so
    // a slug-invalid component is rejected with a clear error rather than
    // minting a descriptor that silently fails to match at comms ingress.
    let comms_name = match meerkat_core::MemberCommsName::new(
        mob_id.as_str(),
        entry.role.as_str(),
        mid.as_str(),
    ) {
        Ok(name) => name.to_string(),
        Err(err) => {
            return Err((
                "invalid_comms_name".to_string(),
                format!(
                    "member '{remote_member}' in mob '{mob_id}' has an invalid comms name component: {err}"
                ),
            ));
        }
    };
    let pubkey_b64 = entry.transport_public_key().map(str::to_string);
    // The member's dialable envelope-listener address: its live local comms
    // runtime, or a placed member's host-acknowledged durable endpoint.
    let advertised_address = member_dialable_address(handle, session_service, &mid)
        .await
        .map_err(lookup_address_fault)?;
    Ok(ControlResponse::Member {
        peer_id,
        comms_name,
        pubkey_b64,
        advertised_address,
        sig_b64: None,
    })
}

/// Run a control listener on `tcp_listener` until shutdown.
///
/// Each accepted connection is read-frame, dispatched to the handler,
/// response-written, then closed. The listener accepts indefinitely; cancel
/// via `tokio::select!` against your shutdown signal.
pub async fn serve_tcp_control(
    listener: TcpListener,
    handler: std::sync::Arc<dyn ControlHandler>,
    signer: ControlSignerSlot,
) {
    serve_tcp_control_with_authorizer(
        listener,
        handler,
        signer,
        std::sync::Arc::new(ControlAuthorizer::open()),
    )
    .await;
}

/// Same as [`serve_tcp_control`], but refusing requests `authorizer` does
/// not admit before they reach the handler.
pub async fn serve_tcp_control_with_authorizer(
    listener: TcpListener,
    handler: std::sync::Arc<dyn ControlHandler>,
    signer: ControlSignerSlot,
    authorizer: std::sync::Arc<ControlAuthorizer>,
) {
    loop {
        let (stream, _peer_addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(error = %err, "control listener accept failed; exiting");
                return;
            }
        };
        let handler = handler.clone();
        let signer = std::sync::Arc::clone(&signer);
        let authorizer = std::sync::Arc::clone(&authorizer);
        tokio::spawn(serve_one_tcp(stream, handler, signer, authorizer));
    }
}

/// Same as `serve_tcp_control` but for Unix-domain sockets.
#[cfg(unix)]
pub async fn serve_uds_control(
    listener: UnixListener,
    handler: std::sync::Arc<dyn ControlHandler>,
    signer: ControlSignerSlot,
) {
    serve_uds_control_with_authorizer(
        listener,
        handler,
        signer,
        std::sync::Arc::new(ControlAuthorizer::open()),
    )
    .await;
}

/// Same as [`serve_uds_control`], but grant-enforcing.
#[cfg(unix)]
pub async fn serve_uds_control_with_authorizer(
    listener: UnixListener,
    handler: std::sync::Arc<dyn ControlHandler>,
    signer: ControlSignerSlot,
    authorizer: std::sync::Arc<ControlAuthorizer>,
) {
    loop {
        let (stream, _peer_addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(error = %err, "uds control listener accept failed; exiting");
                return;
            }
        };
        let handler = handler.clone();
        let signer = std::sync::Arc::clone(&signer);
        let authorizer = std::sync::Arc::clone(&authorizer);
        tokio::spawn(serve_one_uds(stream, handler, signer, authorizer));
    }
}

async fn serve_one_tcp(
    stream: TcpStream,
    handler: std::sync::Arc<dyn ControlHandler>,
    signer: ControlSignerSlot,
    authorizer: std::sync::Arc<ControlAuthorizer>,
) {
    let mut s = ControlStream::Tcp(stream);
    serve_one(&mut s, handler, signer, authorizer).await;
}

#[cfg(unix)]
async fn serve_one_uds(
    stream: UnixStream,
    handler: std::sync::Arc<dyn ControlHandler>,
    signer: ControlSignerSlot,
    authorizer: std::sync::Arc<ControlAuthorizer>,
) {
    let mut s = ControlStream::Uds(stream);
    serve_one(&mut s, handler, signer, authorizer).await;
}

async fn serve_one(
    stream: &mut ControlStream,
    handler: std::sync::Arc<dyn ControlHandler>,
    signer: ControlSignerSlot,
    authorizer: std::sync::Arc<ControlAuthorizer>,
) {
    let payload = match stream.read_frame().await {
        Ok(buf) => buf,
        Err(err) => {
            tracing::debug!(error = %err, "control listener: read failed");
            return;
        }
    };
    let request = match serde_json::from_slice::<ControlRequest>(&payload) {
        Ok(req) => req,
        Err(err) => {
            let response = ControlResponse::Err {
                code: "decode".to_string(),
                message: err.to_string(),
            };
            let response_payload = serde_json::to_vec(&response).unwrap_or_default();
            let _ = stream.write_frame(&response_payload).await;
            return;
        }
    };
    // Authorize BEFORE dispatch, not inside the handler: an ungranted
    // caller must not learn whether the named member exists, and the check
    // has to cover every ControlHandler implementation rather than only
    // the one that happens to be mounted.
    let verb = request.verb();
    let admitted = match authorizer.authorize(&request) {
        Ok(None) => None,
        Ok(Some(caller)) => {
            tracing::debug!(
                caller = %caller.label,
                verb = %verb,
                "cross-mob control request admitted by grant"
            );
            Some(caller)
        }
        Err(denial) => {
            tracing::warn!(
                code = denial.code(),
                verb = %verb,
                reason = %denial,
                "cross-mob control request refused by grant policy"
            );
            let response = ControlResponse::Err {
                code: denial.code().to_string(),
                message: denial.to_string(),
            };
            let response_payload = serde_json::to_vec(&response).unwrap_or_default();
            let _ = stream.write_frame(&response_payload).await;
            return;
        }
    };
    let mut response = handler.handle_admitted(request, admitted).await;
    // Bind the response to this gateway's pinned identity and to the exact
    // request bytes. Re-read per request: hosts install keys after the
    // listener exists.
    let keys = signer
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(keys) = keys {
        sign_control_response(&keys, &payload, &mut response);
    }
    let response_payload = serde_json::to_vec(&response).unwrap_or_default();
    let _ = stream.write_frame(&response_payload).await;
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::identity_first::core_test_bridge::{CoreReadsBridge, register_live};
    use crate::identity_first::{
        AgentAddressability, AgentIdentity, DurabilityPolicy, DurableAgentSpec, IdentityRuntime,
        IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct EchoHandler;

    /// An identity runtime over a Core that holds `worker`'s member live at
    /// generation 0: its current runtime alias is `rt:worker:0`.
    async fn identity_runtime_with_current_alias()
    -> Result<Arc<IdentityRuntime>, Box<dyn std::error::Error + Send + Sync>> {
        let bridge = Arc::new(CoreReadsBridge::default());
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "cross-mob-control-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        }));
        register_live(
            &runtime,
            &bridge.core,
            DurableAgentSpec {
                identity: AgentIdentity::parse("worker")?,
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        Ok(runtime)
    }

    impl ControlHandler for EchoHandler {
        fn handle(
            &self,
            request: ControlRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>>
        {
            Box::pin(async move {
                match request {
                    ControlRequest::Wire { .. } | ControlRequest::Unwire { .. } => {
                        ControlResponse::Ok { sig_b64: None }
                    }
                    ControlRequest::Inject { remote_member, .. } => ControlResponse::Injected {
                        session_id: format!("session-for-{remote_member}"),
                        sig_b64: None,
                    },
                    ControlRequest::LookupMember { remote_member, .. } => ControlResponse::Member {
                        peer_id: format!("peer-id-for-{remote_member}"),
                        comms_name: format!("mob/role/{remote_member}"),
                        pubkey_b64: None,
                        advertised_address: None,
                        sig_b64: None,
                    },
                    ControlRequest::HostDescribe { .. } => ControlResponse::Host {
                        facts: echo_host_facts(),
                        sig_b64: None,
                    },
                    ControlRequest::HostHealth { .. } => ControlResponse::HostHealth {
                        health: echo_host_health(),
                        sig_b64: None,
                    },
                    request @ (ControlRequest::InjectKeyed { .. }
                    | ControlRequest::InjectStatus { .. }) => {
                        crate::runtime::keyed_inject::answer_keyed_inject_request(
                            &crate::runtime::keyed_inject::pending_keyed_inject_slot(),
                            request,
                            None,
                        )
                        .await
                    }
                }
            })
        }
    }

    fn echo_host_capabilities() -> meerkat_contracts::RuntimeHostCapabilities {
        meerkat_contracts::RuntimeHostCapabilities {
            contract_version: meerkat_contracts::ContractVersion::CURRENT,
            features: meerkat_contracts::RuntimeHostFeatureFlags {
                runtime_backed_sessions: true,
                mobs: true,
                mcp_live: false,
                comms: true,
                blobs: false,
                session_events: true,
                session_streams: false,
                schedules: false,
                skills: false,
                event_replay: false,
                artifacts: false,
                approvals: false,
                external_members: false,
                secure_remote_rpc: false,
                multi_host_mobs: false,
                durable_jobs: false,
            },
        }
    }

    fn echo_host_facts() -> super::super::remote_host::HostFacts {
        super::super::remote_host::HostFacts::new(
            "echo-host",
            TEST_PUBKEY_B64,
            "tcp://127.0.0.1:7801",
            echo_host_capabilities(),
        )
    }

    fn echo_host_health() -> meerkat_contracts::RuntimeHostHealth {
        meerkat_contracts::RuntimeHostHealth {
            contract_version: meerkat_contracts::ContractVersion::CURRENT,
            status: meerkat_contracts::RuntimeHostHealthStatus::Ok,
            checks: BTreeMap::new(),
        }
    }

    /// Base64 of the 32-byte pubkey `[42u8; 32]` (same constant the
    /// contact-directory tests use).
    const TEST_PUBKEY_B64: &str = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=";

    fn wire_params<'a>(
        address: &'a str,
        peer_id: &'a str,
        pubkey_b64: Option<&'a str>,
    ) -> WireControlParams<'a> {
        WireControlParams {
            remote_member: "bob",
            local_peer_spec_address: address,
            local_comms_name: "mob-a/worker/alice",
            local_peer_id: peer_id,
            local_pubkey_b64: pubkey_b64,
            wire: true,
        }
    }

    /// Regression: the remote Wire handler used to route inproc:// through
    /// the relaxed unsigned branch, silently installing a descriptor that
    /// could never deliver across processes.
    #[test]
    fn remote_wire_descriptor_rejects_inproc_addresses() {
        let params = wire_params(
            "inproc://mob-a/worker/alice",
            "00000000-0000-4000-8000-000000000001",
            Some(TEST_PUBKEY_B64),
        );
        let (code, _) = build_remote_wire_descriptor(&params).expect_err("inproc must fail");
        assert_eq!(code, "inproc_address_rejected");
    }

    #[test]
    fn remote_wire_descriptor_requires_pubkey() {
        for pubkey in [None, Some("")] {
            let params = wire_params(
                "tcp://127.0.0.1:9001",
                "00000000-0000-4000-8000-000000000001",
                pubkey,
            );
            let (code, _) =
                build_remote_wire_descriptor(&params).expect_err("missing pubkey must fail");
            assert_eq!(code, "missing_pubkey");
        }
    }

    #[test]
    fn remote_wire_descriptor_builds_signed_descriptor() {
        let derived_peer_id =
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&[42u8; 32]).to_string();
        let params = wire_params(
            "tcp://127.0.0.1:9001",
            &derived_peer_id,
            Some(TEST_PUBKEY_B64),
        );
        let spec = build_remote_wire_descriptor(&params).expect("descriptor");
        assert_eq!(spec.pubkey, [42u8; 32]);
        assert_eq!(spec.address.endpoint(), "127.0.0.1:9001");
    }

    /// The peer-id/pubkey consistency check is what forces the MEMBER
    /// transport key onto this path: a mismatched key (e.g. the gateway
    /// key) must be rejected, not installed.
    #[test]
    fn remote_wire_descriptor_rejects_mismatched_pubkey() {
        let params = wire_params(
            "tcp://127.0.0.1:9001",
            "00000000-0000-4000-8000-000000000001",
            Some(TEST_PUBKEY_B64),
        );
        let (code, _) =
            build_remote_wire_descriptor(&params).expect_err("mismatched pubkey must fail");
        assert_eq!(code, "peer_spec");
    }

    fn signer_with(keys: crate::auth::peer_keys::GatewayPeerKeys) -> ControlSignerSlot {
        Arc::new(std::sync::RwLock::new(Some(Arc::new(keys))))
    }

    /// A signed response verifies against the pinned key and the exact
    /// request bytes - and against nothing else: different request bytes
    /// (a replay for a fresh nonce) and a different pinned key both fail.
    #[tokio::test]
    async fn signed_responses_verify_against_pinned_gateway_key() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let pinned = keys.pubkey_bytes();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(serve_tcp_control(listener, handler, signer_with(keys)));

        let request = ControlRequest::LookupMember {
            remote_member: "alice".to_string(),
            nonce: Some("nonce-1".to_string()),
            caller: None,
        };
        let payload = serde_json::to_vec(&request).expect("encode");
        let endpoint = RemoteEndpoint::Tcp(addr.to_string());
        let response =
            RemoteControlClient::send_payload(&endpoint, &payload, DEFAULT_CONTROL_TIMEOUT)
                .await
                .expect("control rpc");
        verify_control_response(&pinned, &payload, &response).expect("signature verifies");

        let replayed_request = ControlRequest::LookupMember {
            remote_member: "alice".to_string(),
            nonce: Some("nonce-2".to_string()),
            caller: None,
        };
        let replayed_payload = serde_json::to_vec(&replayed_request).expect("encode");
        assert!(
            verify_control_response(&pinned, &replayed_payload, &response).is_err(),
            "a response must not verify for different request bytes (replay)"
        );
        let other_key = crate::auth::peer_keys::GatewayPeerKeys::ephemeral().pubkey_bytes();
        assert!(
            verify_control_response(&other_key, &payload, &response).is_err(),
            "a response must not verify against a different pinned key"
        );
        server.abort();
    }

    #[test]
    fn unsigned_response_fails_verification_when_pinned() {
        let pinned = crate::auth::peer_keys::GatewayPeerKeys::ephemeral().pubkey_bytes();
        let response = ControlResponse::Ok { sig_b64: None };
        let err =
            verify_control_response(&pinned, b"{}", &response).expect_err("unsigned must fail");
        assert!(err.contains("unsigned"), "got: {err}");
    }

    /// `Err` responses are failure reports, not trusted material: they
    /// carry no signature and pass verification unchanged (rejecting them
    /// would only swap WHICH error the caller sees).
    #[test]
    fn error_responses_skip_verification() {
        let pinned = crate::auth::peer_keys::GatewayPeerKeys::ephemeral().pubkey_bytes();
        let response = ControlResponse::Err {
            code: "unknown_member".to_string(),
            message: "nope".to_string(),
        };
        verify_control_response(&pinned, b"{}", &response).expect("Err passes through");
    }

    /// Strict-when-pinned at the proxy: a contact that pins a gateway
    /// pubkey rejects unsigned responses typed; the explicit
    /// `require_signed_control = false` opt-out accepts them.
    #[tokio::test]
    async fn pinned_contact_requires_signed_control_responses() {
        use crate::contact_directory::{ContactEntry, MobTransport};
        use crate::runtime::cross_mob_remote::RemoteMobProxy;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(serve_tcp_control(
            listener,
            handler,
            unsigned_control_signer(),
        ));
        let pinned = crate::auth::peer_keys::GatewayPeerKeys::ephemeral().pubkey_bytes();

        let strict_entry = ContactEntry {
            mob_id: "remote".to_string(),
            transport: MobTransport::Tcp(addr.to_string()),
            pubkey: Some(pinned),
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry(&strict_entry)
            .expect("tcp ok")
            .expect("some");
        let err = proxy
            .lookup_member("alice")
            .await
            .expect_err("unsigned response must fail a pinned contact");
        assert!(
            matches!(err, RemoteMobError::ControlResponseUnauthenticated { .. }),
            "got {err:?}"
        );

        let opt_out_entry = ContactEntry {
            require_signed_control: Some(false),
            ..strict_entry
        };
        let proxy = RemoteMobProxy::from_entry(&opt_out_entry)
            .expect("tcp ok")
            .expect("some");
        proxy
            .lookup_member("alice")
            .await
            .expect("explicit opt-out accepts unsigned responses");
        server.abort();
    }

    /// End-to-end signed exchange through the proxy: a pinned contact
    /// talking to a gateway that signs with the matching key succeeds.
    #[tokio::test]
    async fn pinned_contact_accepts_signed_responses() {
        use crate::contact_directory::{ContactEntry, MobTransport};
        use crate::runtime::cross_mob_remote::RemoteMobProxy;

        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let pinned = keys.pubkey_bytes();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(serve_tcp_control(listener, handler, signer_with(keys)));

        let entry = ContactEntry {
            mob_id: "remote".to_string(),
            transport: MobTransport::Tcp(addr.to_string()),
            pubkey: Some(pinned),
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry(&entry)
            .expect("tcp ok")
            .expect("some");
        let info = proxy
            .lookup_member("alice")
            .await
            .expect("signed lookup verifies");
        assert_eq!(info.peer_id, "peer-id-for-alice");
        proxy
            .wire_remote(
                "alice",
                "tcp://127.0.0.1:9001",
                "demo/role/alice",
                "00000000-0000-4000-8000-000000000001",
                None,
            )
            .await
            .expect("signed wire ack verifies");
        server.abort();
    }

    #[test]
    fn control_listen_addr_parses_contact_directory_spellings() {
        assert_eq!(
            ControlListenAddr::parse("tcp://127.0.0.1:9001"),
            Ok(ControlListenAddr::Tcp("127.0.0.1:9001".to_string())),
        );
        assert_eq!(
            ControlListenAddr::parse("uds:///var/run/mob.sock"),
            Ok(ControlListenAddr::Uds("/var/run/mob.sock".to_string())),
        );
        assert!(ControlListenAddr::parse("inproc").is_err());
        assert!(ControlListenAddr::parse("ftp://nope").is_err());
        assert!(ControlListenAddr::parse("127.0.0.1:9001").is_err());
    }

    #[test]
    fn control_listen_addr_display_round_trips() {
        for spec in ["tcp://127.0.0.1:9001", "uds:///var/run/mob.sock"] {
            let addr = ControlListenAddr::parse(spec).expect("parse");
            assert_eq!(addr.to_string(), spec);
        }
    }

    /// `tcp://host:0` binds an ephemeral port and the bound listener reports
    /// the real dialable address; a control request round-trips through it.
    #[tokio::test]
    async fn bound_listener_reports_real_port_and_serves() {
        let addr = ControlListenAddr::parse("tcp://127.0.0.1:0").expect("parse");
        let bound = BoundControlListener::bind(&addr).await.expect("bind");
        let advertised = bound.advertised_address().to_string();
        let dial = advertised.strip_prefix("tcp://").expect("tcp scheme");
        assert!(
            !dial.ends_with(":0"),
            "advertised address must carry the kernel-assigned port: {advertised}"
        );
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(bound.serve(handler, unsigned_control_signer()));

        let endpoint = RemoteEndpoint::Tcp(dial.to_string());
        let request = ControlRequest::LookupMember {
            remote_member: "alice".to_string(),
            nonce: None,
            caller: None,
        };
        let response = RemoteControlClient::send(&endpoint, &request, DEFAULT_CONTROL_TIMEOUT)
            .await
            .expect("control rpc");
        assert!(matches!(response, ControlResponse::Member { .. }));
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bound_uds_listener_serves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("control.sock");
        let addr = ControlListenAddr::Uds(path.display().to_string());
        let bound = BoundControlListener::bind(&addr).await.expect("bind");
        assert_eq!(
            bound.advertised_address(),
            format!("uds://{}", path.display())
        );
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(bound.serve(handler, unsigned_control_signer()));

        let endpoint = RemoteEndpoint::Uds(path.display().to_string());
        let request = ControlRequest::Inject {
            remote_member: "bob".to_string(),
            content: serde_json::json!({"text": "hi"}),
            nonce: None,
            caller: None,
        };
        let response = RemoteControlClient::send(&endpoint, &request, DEFAULT_CONTROL_TIMEOUT)
            .await
            .expect("control rpc");
        assert_eq!(
            response,
            ControlResponse::Injected {
                session_id: "session-for-bob".to_string(),
                sig_b64: None,
            },
        );
        server.abort();
    }

    #[tokio::test]
    async fn tcp_round_trip_inject() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(serve_tcp_control(
            listener,
            handler,
            unsigned_control_signer(),
        ));

        let endpoint = RemoteEndpoint::Tcp(addr.to_string());
        let request = ControlRequest::Inject {
            remote_member: "alice".to_string(),
            content: serde_json::json!({"text": "hello"}),
            nonce: None,
            caller: None,
        };
        let response = RemoteControlClient::send(&endpoint, &request, DEFAULT_CONTROL_TIMEOUT)
            .await
            .expect("control rpc");
        assert_eq!(
            response,
            ControlResponse::Injected {
                session_id: "session-for-alice".to_string(),
                sig_b64: None,
            },
        );
        server.abort();
    }

    #[tokio::test]
    async fn tcp_round_trip_wire() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(serve_tcp_control(
            listener,
            handler,
            unsigned_control_signer(),
        ));

        let endpoint = RemoteEndpoint::Tcp(addr.to_string());
        let request = ControlRequest::Wire {
            remote_member: "bob".to_string(),
            local_peer_spec_address: "tcp://127.0.0.1:9001".to_string(),
            local_comms_name: "demo/role/alice".to_string(),
            local_peer_id: "00000000-0000-4000-8000-000000000001".to_string(),
            local_pubkey_b64: None,
            nonce: None,
            caller: None,
        };
        let response = RemoteControlClient::send(&endpoint, &request, DEFAULT_CONTROL_TIMEOUT)
            .await
            .expect("control rpc");
        assert_eq!(response, ControlResponse::Ok { sig_b64: None });
        server.abort();
    }

    #[tokio::test]
    async fn malformed_request_returns_decode_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let _server = tokio::spawn(serve_tcp_control(
            listener,
            handler,
            unsigned_control_signer(),
        ));

        // Send raw garbage bytes as a "control request" — server should
        // respond with a `decode` error instead of dropping the connection
        // silently.
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(&u32::to_be_bytes(5))
            .await
            .expect("write header");
        stream.write_all(b"hello").await.expect("write payload");
        stream.flush().await.expect("flush");

        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await.expect("read header");
        let len = u32::from_be_bytes(header) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await.expect("read payload");
        let response: ControlResponse = serde_json::from_slice(&buf).expect("decode response");
        match response {
            ControlResponse::Err { code, .. } => assert_eq!(code, "decode"),
            other => panic!("expected decode error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn identity_control_rejects_stale_and_encoded_aliases_before_mutation()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = identity_runtime_with_current_alias().await?;
        let calls = Arc::new(AtomicUsize::new(0));

        for stale_alias in [
            "rt:worker:1".to_string(),
            crate::member_comms_id::mob_member_id_str("rt:worker:1").into_owned(),
        ] {
            let operation_calls = Arc::clone(&calls);
            let error =
                run_control_member_operation(Some(&runtime), &stale_alias, move || async move {
                    operation_calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, (String, String)>(())
                })
                .await
                .expect_err("stale generation must fail before the lower plane");
            assert_eq!(error.0, "stale_runtime_alias");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let operation_calls = Arc::clone(&calls);
        run_control_member_operation(Some(&runtime), "rt:worker:0", move || async move {
            operation_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, (String, String)>(())
        })
        .await
        .expect("current generation is admitted");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        Ok(())
    }

    #[tokio::test]
    async fn generated_control_alias_without_authority_fails_closed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let operation_calls = Arc::clone(&calls);
        let encoded = crate::member_comms_id::mob_member_id_str("rt:worker:0").into_owned();
        let error = run_control_member_operation(None, &encoded, move || async move {
            operation_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, (String, String)>(())
        })
        .await
        .expect_err("generated aliases require identity authority");

        assert_eq!(error.0, "identity_authority_unavailable");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// The control plane follows the member-id alias rule: a request naming a
    /// member by its CURRENT runtime alias runs under that identity's
    /// lifecycle lock and acts on the durable roster row the alias names.
    /// Before, the operation encoded the alias itself, which no row has, so
    /// lookup and inject answered unknown_member and wire failed for a
    /// healthy member.
    #[tokio::test]
    async fn control_requests_reach_the_member_a_current_runtime_alias_names()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let identity_runtime = identity_runtime_with_current_alias().await?;
        let dir = tempfile::tempdir()?;
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"control-current-alias\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             external_addressable = true\n\n[profiles.worker.tools]\ncomms = true\n",
        )?;
        let spec = crate::mob_handle_runtime::MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
        )?
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime = crate::mob_handle_runtime::MobRuntime::bootstrap(spec).await?;
        let handle = runtime.handle();
        handle
            .ensure_member(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::AgentIdentity::from("worker"),
            ))
            .await?;

        let lookup =
            handle_lookup_member(&handle, None, "rt:worker:0", Some(&identity_runtime)).await;
        let injected = handle_inject(
            &handle,
            "rt:worker:0",
            serde_json::json!("hello"),
            Some(&identity_runtime),
        )
        .await;
        let remote_peer_id =
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&[42_u8; 32]).to_string();
        let wired = handle_wire(
            &handle,
            WireControlParams {
                remote_member: "rt:worker:0",
                local_peer_spec_address: "tcp://127.0.0.1:9001",
                local_comms_name: "remote-mob/worker/peer",
                local_peer_id: &remote_peer_id,
                local_pubkey_b64: Some("KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio="),
                wire: true,
            },
            Some(&identity_runtime),
        )
        .await;
        assert_eq!(
            (
                matches!(&lookup, ControlResponse::Member { comms_name, .. } if comms_name.ends_with("/worker")),
                matches!(&injected, ControlResponse::Injected { .. }),
                matches!(&wired, ControlResponse::Ok { .. }),
            ),
            (true, true, true),
            "lookup: {lookup:?}\ninject: {injected:?}\nwire: {wired:?}"
        );
        let _ = handle.shutdown().await;
        Ok(())
    }

    /// The stored inputs of `session_id`'s runtime, each as its wire image.
    async fn member_input_images(
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let mut images = Vec::new();
        for state in machine.session_input_states(session_id).await?.report {
            images.push(serde_json::to_value(&state)?);
        }
        Ok(images)
    }

    /// A candidate launch's control listener refuses a peer's `Wire`,
    /// `Unwire` and `Inject` with `candidate_launch` before they reach the
    /// mob: the member's roster row, which holds its wiring, and its stored
    /// inputs are unchanged. An authoritative launch's handler serves the
    /// same requests.
    #[tokio::test]
    async fn a_candidate_launch_control_handler_refuses_wire_unwire_and_inject()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"control-candidate\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             external_addressable = true\n\n[profiles.worker.tools]\ncomms = true\n",
        )?;
        let spec = crate::mob_handle_runtime::MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
        )?
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime = crate::mob_handle_runtime::MobRuntime::bootstrap(spec).await?;
        let handle = runtime.handle();
        let worker = meerkat_mob::AgentIdentity::from("worker");
        handle
            .ensure_member(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                worker.clone(),
            ))
            .await?;
        let session_id = handle
            .resolve_bridge_session_id(&worker)
            .await
            .ok_or("the worker has a session")?;
        let machine = runtime
            .session_service()
            .ok_or("the mob has a session service")?
            .acquire_runtime_adapter(None)?
            .ok_or("the session service has a runtime")?;
        let remote_peer_id =
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&[42_u8; 32]).to_string();
        let wiring = |wire: bool| {
            let remote_member = "worker".to_string();
            let local_peer_spec_address = "tcp://127.0.0.1:9001".to_string();
            let local_comms_name = "remote-mob/worker/peer".to_string();
            let local_peer_id = remote_peer_id.clone();
            let local_pubkey_b64 = Some("KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=".to_string());
            if wire {
                ControlRequest::Wire {
                    remote_member,
                    local_peer_spec_address,
                    local_comms_name,
                    local_peer_id,
                    local_pubkey_b64,
                    nonce: None,
                    caller: None,
                }
            } else {
                ControlRequest::Unwire {
                    remote_member,
                    local_peer_spec_address,
                    local_comms_name,
                    local_peer_id,
                    local_pubkey_b64,
                    nonce: None,
                    caller: None,
                }
            }
        };
        let inject = || ControlRequest::Inject {
            remote_member: "worker".to_string(),
            content: serde_json::json!("hello"),
            nonce: None,
            caller: None,
        };
        let row_before = handle.get_member(&worker).await?;
        let inputs_before = member_input_images(&machine, &session_id).await?;

        let candidate = MobHandleControlHandler::new(handle.clone()).with_candidate_launch(true);
        let mut refused = Vec::new();
        for request in [wiring(true), wiring(false), inject()] {
            refused.push(match candidate.handle(request).await {
                ControlResponse::Err { code, .. } => code,
                other => format!("served: {other:?}"),
            });
        }
        assert_eq!(
            (
                refused,
                handle.get_member(&worker).await? == row_before,
                member_input_images(&machine, &session_id).await? == inputs_before,
            ),
            (
                vec![CANDIDATE_LAUNCH_CONTROL_CODE.to_string(); 3],
                true,
                true
            ),
            "a candidate refuses the peer's wire, unwire and inject, and the member's wiring \
             and stored inputs are unchanged"
        );

        let authoritative =
            MobHandleControlHandler::new(handle.clone()).with_candidate_launch(false);
        let wired = authoritative.handle(wiring(true)).await;
        let row_wired = handle.get_member(&worker).await?;
        let injected = authoritative.handle(inject()).await;
        let unwired = authoritative.handle(wiring(false)).await;
        assert_eq!(
            (
                matches!(&wired, ControlResponse::Ok { .. }),
                row_wired != row_before,
                matches!(&injected, ControlResponse::Injected { .. }),
                matches!(&unwired, ControlResponse::Ok { .. }),
            ),
            (true, true, true, true),
            "the authoritative launch serves them: wire {wired:?}, inject {injected:?}, \
             unwire {unwired:?}"
        );
        let _ = handle.shutdown().await;
        Ok(())
    }

    // -- caller authorization (scoped grants) --

    fn lookup_request(member: &str) -> ControlRequest {
        ControlRequest::LookupMember {
            remote_member: member.to_string(),
            nonce: Some("nonce".to_string()),
            caller: None,
        }
    }

    fn inject_request(member: &str) -> ControlRequest {
        ControlRequest::Inject {
            remote_member: member.to_string(),
            content: serde_json::json!({"text": "hi"}),
            nonce: Some("nonce".to_string()),
            caller: None,
        }
    }

    fn signed_as(
        keys: &crate::auth::peer_keys::GatewayPeerKeys,
        audience: Option<&str>,
        mut request: ControlRequest,
    ) -> ControlRequest {
        sign_control_request_as_caller(keys, audience, &mut request);
        request
    }

    fn table_for(
        keys: &crate::auth::peer_keys::GatewayPeerKeys,
        verbs: &[ControlVerb],
        members: ControlMemberScope,
    ) -> ControlGrantTable {
        let mut table = ControlGrantTable::new();
        table.insert(
            keys.pubkey_bytes(),
            ControlGrant::new("peer-mob", verbs.iter().copied(), members),
        );
        table
    }

    #[test]
    fn control_verb_config_spellings_round_trip() {
        for verb in ControlVerb::all().iter().copied() {
            assert_eq!(ControlVerb::parse(verb.as_str()), Some(verb));
        }
        assert_eq!(ControlVerb::parse("delete_everything"), None);
    }

    /// Back-compat: a listener with no grant table dispatches exactly what
    /// it dispatched before caller authorization existed, credential or
    /// not.
    #[test]
    fn open_authorizer_admits_unauthenticated_requests() {
        let authorizer = ControlAuthorizer::open();
        assert!(!authorizer.is_enforcing());
        assert_eq!(
            authorizer.authorize(&inject_request("bob")),
            Ok(None),
            "open mode authorizes nothing and refuses nothing"
        );
    }

    /// Regression guard for the classic fail-open bug in this shape of
    /// code: an EMPTY grant table must deny everyone, never fall back to
    /// open. `ControlAuthorizer::open()` is the only permissive mode and
    /// it has to be named explicitly.
    #[test]
    fn empty_grant_table_denies_every_caller() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = ControlAuthorizer::with_grants(ControlGrantTable::new());
        assert!(authorizer.is_enforcing());
        let denial = authorizer
            .authorize(&signed_as(&keys, None, lookup_request("bob")))
            .expect_err("an empty table grants nothing");
        assert_eq!(denial.code(), "caller_not_granted");
    }

    #[test]
    fn unauthenticated_request_is_refused_when_grants_are_enforced() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = ControlAuthorizer::with_grants(table_for(
            &keys,
            ControlVerb::all(),
            ControlMemberScope::All,
        ));
        let denial = authorizer
            .authorize(&inject_request("bob"))
            .expect_err("no credential must be refused");
        assert_eq!(denial.code(), "unauthenticated_caller");
    }

    #[test]
    fn granted_caller_is_admitted() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = ControlAuthorizer::with_grants(table_for(
            &keys,
            &[ControlVerb::Inject],
            ControlMemberScope::members(["bob"]),
        ));
        let admitted = authorizer
            .authorize(&signed_as(&keys, None, inject_request("bob")))
            .expect("granted verb and member")
            .expect("enforcing mode names the caller");
        assert_eq!(admitted.label, "peer-mob");
        assert_eq!(admitted.pubkey, keys.pubkey_bytes());
    }

    /// The scope is a REFUSAL surface, not a hint: the same authenticated
    /// caller that may inject into bob may not inject into carol, and may
    /// not wire at all.
    #[test]
    fn verb_and_member_outside_the_grant_are_refused() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = ControlAuthorizer::with_grants(table_for(
            &keys,
            &[ControlVerb::Inject],
            ControlMemberScope::members(["bob"]),
        ));

        let verb_denial = authorizer
            .authorize(&signed_as(&keys, None, lookup_request("bob")))
            .expect_err("lookup_member is not granted");
        assert_eq!(verb_denial.code(), "verb_not_granted");

        let member_denial = authorizer
            .authorize(&signed_as(&keys, None, inject_request("carol")))
            .expect_err("carol is outside the member scope");
        assert_eq!(member_denial.code(), "member_not_granted");
    }

    /// A caller that is not in the table at all is refused even with a
    /// perfectly valid signature.
    #[test]
    fn unknown_caller_is_refused() {
        let granted = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let stranger = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = ControlAuthorizer::with_grants(table_for(
            &granted,
            ControlVerb::all(),
            ControlMemberScope::All,
        ));
        let denial = authorizer
            .authorize(&signed_as(&stranger, None, inject_request("bob")))
            .expect_err("a stranger holds no grant");
        assert_eq!(denial.code(), "caller_not_granted");
    }

    /// Member scope must compare in one alias space. Raw control surfaces
    /// may present either the public alias or its comms-safe roster
    /// encoding; a scope that matched only one spelling would be
    /// bypassable with the other (the reason
    /// `is_reserved_generated_alias` decodes first, too).
    #[test]
    fn member_scope_matches_the_encoded_roster_alias() {
        let scope = ControlMemberScope::members(["rt:worker:0"]);
        let encoded = crate::member_comms_id::mob_member_id_str("rt:worker:0").into_owned();
        assert_ne!(encoded, "rt:worker:0", "fixture must exercise the encoding");
        assert!(scope.contains("rt:worker:0"));
        assert!(scope.contains(&encoded));
        assert!(!scope.contains("rt:worker:1"));

        let encoded_scope = ControlMemberScope::members([encoded.as_str()]);
        assert!(encoded_scope.contains("rt:worker:0"));
    }

    #[test]
    fn wildcard_member_entry_widens_to_all() {
        assert_eq!(
            ControlMemberScope::members(["bob", "*"]),
            ControlMemberScope::All
        );
    }

    /// The signature covers the semantic fields, so mutating any of them
    /// after signing invalidates it. This is what stops a MITM from
    /// re-pointing a granted caller's request at another member.
    #[test]
    fn tampering_with_the_request_breaks_the_caller_signature() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = ControlAuthorizer::with_grants(table_for(
            &keys,
            ControlVerb::all(),
            ControlMemberScope::All,
        ));
        let mut request = signed_as(&keys, None, inject_request("bob"));
        let caller = request.caller().cloned().expect("signed");
        request = inject_request("carol");
        request.set_caller(Some(caller));
        let denial = authorizer
            .authorize(&request)
            .expect_err("a moved signature must not verify");
        assert_eq!(denial.code(), "invalid_caller_signature");
    }

    /// A caller cannot claim a pubkey it does not hold: the signature is
    /// checked against the presented key before the table is consulted.
    #[test]
    fn claimed_pubkey_without_the_matching_key_is_refused() {
        let victim = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let attacker = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = ControlAuthorizer::with_grants(table_for(
            &victim,
            ControlVerb::all(),
            ControlMemberScope::All,
        ));
        let mut request = signed_as(&attacker, None, inject_request("bob"));
        if let Some(caller) = request.caller().cloned() {
            request.set_caller(Some(ControlCaller {
                pubkey_b64: victim.pubkey_b64(),
                ..caller
            }));
        }
        let denial = authorizer
            .authorize(&request)
            .expect_err("claiming the granted pubkey must not admit the attacker");
        assert_eq!(denial.code(), "invalid_caller_signature");
    }

    /// The audience is signed material, so a request minted for one
    /// gateway cannot be replayed at another where the same caller also
    /// holds a grant.
    #[test]
    fn audience_binding_refuses_a_request_minted_for_another_gateway() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = ControlAuthorizer::with_grants_for_audience(
            table_for(&keys, ControlVerb::all(), ControlMemberScope::All),
            "mob-b",
        );
        authorizer
            .authorize(&signed_as(&keys, Some("mob-b"), inject_request("bob")))
            .expect("matching audience is admitted");
        let denial = authorizer
            .authorize(&signed_as(&keys, Some("mob-c"), inject_request("bob")))
            .expect_err("a request minted for mob-c must not spend here");
        assert_eq!(denial.code(), "audience_mismatch");
        let missing = authorizer
            .authorize(&signed_as(&keys, None, inject_request("bob")))
            .expect_err("an audience-less request must not spend on a bound listener");
        assert_eq!(missing.code(), "audience_mismatch");
    }

    /// The signing payload is derived from a decoded request on both
    /// sides, so it has to survive the JSON round trip the wire imposes -
    /// including the one non-scalar field (`Inject.content`).
    #[test]
    fn inject_signing_payload_survives_the_json_round_trip() {
        let content = serde_json::json!({
            "text": "hi",
            "nested": {
                "b": 1.5,
                "a": [1, 2, {"z": true, "n": null}],
                "unicode": "naïve\nwith newline\tand tab",
            },
            "big": 9_007_199_254_740_993_u64,
        });
        let request = ControlRequest::Inject {
            remote_member: "bob".to_string(),
            content,
            nonce: Some("nonce".to_string()),
            caller: None,
        };
        let bytes = serde_json::to_vec(&request).expect("encode");
        let decoded: ControlRequest = serde_json::from_slice(&bytes).expect("decode");
        assert_eq!(
            control_request_signing_payload(&request),
            control_request_signing_payload(&decoded),
            "a signature minted by the caller must verify against the decoded request"
        );
    }

    /// Fields are length-prefixed, so a value containing the delimiter
    /// cannot forge a different field layout and let a narrow grant be
    /// spent on a wider request.
    #[test]
    fn signing_payload_is_not_field_injectable() {
        let split = ControlRequest::LookupMember {
            remote_member: "a".to_string(),
            nonce: Some("b".to_string()),
            caller: None,
        };
        let merged = ControlRequest::LookupMember {
            remote_member: "a\n1:b".to_string(),
            nonce: None,
            caller: None,
        };
        assert_ne!(
            control_request_signing_payload(&split),
            control_request_signing_payload(&merged)
        );
    }

    /// Request and response signatures live in separate domains, so
    /// neither can be replayed as the other.
    #[test]
    fn caller_and_response_signature_contexts_are_distinct() {
        assert_ne!(CONTROL_CALLER_SIG_CONTEXT, CONTROL_SIG_CONTEXT);
        assert!(
            control_request_signing_payload(&lookup_request("bob"))
                .starts_with(CONTROL_CALLER_SIG_CONTEXT)
        );
    }

    #[test]
    fn grant_toml_parses_verbs_and_member_scope() {
        let table = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["inject", "lookup_member"]
            members = ["bob"]
            "#
        ))
        .expect("parse")
        .expect("section is present");
        let grant = table.get(&[42u8; 32]).expect("filed under the pubkey");
        assert_eq!(grant.label(), "ops-mob");
        assert_eq!(
            grant.verbs(),
            &[ControlVerb::Inject, ControlVerb::LookupMember]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
        assert!(grant.members().contains("bob"));
        assert!(!grant.members().contains("carol"));
    }

    /// Absent section means "no policy configured", which must stay open -
    /// not become a deny-all table that would break every deployment that
    /// never opted in.
    #[test]
    fn absent_grant_section_leaves_the_listener_open() {
        assert_eq!(
            ControlGrantTable::from_toml("[mobs]\nremote = \"inproc\"\n").expect("parse"),
            None
        );
        assert!(matches!(
            ControlAuthorizer::from_toml("[mobs]\nremote = \"inproc\"\n").expect("parse"),
            ControlAuthorizer::Open
        ));
    }

    #[test]
    fn grant_toml_omitting_members_grants_every_member() {
        let table = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["*"]
            "#
        ))
        .expect("parse")
        .expect("section is present");
        let grant = table.get(&[42u8; 32]).expect("filed");
        assert_eq!(grant.verbs().len(), ControlVerb::member_plane().len());
        assert_eq!(grant.members(), &ControlMemberScope::All);
    }

    /// `*` must keep meaning exactly what it meant when it was written.
    ///
    /// Counting alone would keep passing while meaning something else, so
    /// this asserts the SET: every member-plane verb present (the positive
    /// control - a broken parser that granted nothing would fail here) and
    /// every host-plane verb absent.
    #[test]
    fn star_verbs_never_widen_to_the_host_plane() {
        let table = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["*"]
            "#
        ))
        .expect("parse")
        .expect("section is present");
        let grant = table.get(&[42u8; 32]).expect("filed");
        for verb in ControlVerb::member_plane() {
            assert!(
                grant.verbs().contains(&verb),
                "'*' must still grant member-plane verb {verb}"
            );
        }
        for verb in ControlVerb::all() {
            if verb.is_host_plane() {
                assert!(
                    !grant.verbs().contains(verb),
                    "'*' must not grant host-plane verb {verb}"
                );
            }
        }
    }

    /// Keyed inject verbs are reached only through a grant that names them: `*`
    /// never covers them, an open listener refuses them, and a named grant
    /// still scopes them to its members.
    #[test]
    fn keyed_inject_verbs_are_grant_only_and_member_scoped() {
        let table = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["*"]
            "#
        ))
        .expect("parse")
        .expect("section is present");
        let grant = table.get(&[42u8; 32]).expect("filed");
        let keyed = ControlVerb::all()
            .iter()
            .copied()
            .filter(ControlVerb::is_keyed_inject_plane)
            .collect::<Vec<_>>();
        assert_eq!(
            keyed,
            [ControlVerb::InjectKeyed, ControlVerb::InjectStatus],
            "the positive control: both keyed inject verbs exist"
        );
        for verb in &keyed {
            assert!(
                !grant.verbs().contains(verb),
                "'*' must not grant keyed inject verb {verb}"
            );
        }
        let status = ControlRequest::InjectStatus {
            remote_member: "bob".into(),
            delivery_key: "batch-1".into(),
            with_input: false,
            issued_at_ms: 1,
            nonce: "n".into(),
            caller: None,
        };
        assert_eq!(
            ControlAuthorizer::open().authorize(&status),
            Err(ControlAuthzDenial::GrantsRequired {
                verb: ControlVerb::InjectStatus
            })
        );
        assert_eq!(
            ControlAuthorizer::open().authorize(&inject_request("bob")),
            Ok(None),
            "an open listener still admits the member plane"
        );
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let named = ControlAuthorizer::with_grants(table_for(
            &keys,
            &[ControlVerb::InjectStatus],
            ControlMemberScope::members(["alice"]),
        ));
        let mut for_alice = status.clone();
        if let ControlRequest::InjectStatus { remote_member, .. } = &mut for_alice {
            *remote_member = "alice".into();
        }
        sign_control_request_as_caller(&keys, None, &mut for_alice);
        assert!(
            named.authorize(&for_alice).is_ok(),
            "the named member is reached"
        );
        let mut for_bob = status;
        sign_control_request_as_caller(&keys, None, &mut for_bob);
        assert!(matches!(
            named.authorize(&for_bob),
            Err(ControlAuthzDenial::MemberNotGranted { .. })
        ));
    }

    /// A host-plane verb is reachable only when it is named, and its
    /// member scope is irrelevant because it names no member.
    #[test]
    fn host_plane_verbs_are_named_explicitly_and_ignore_member_scope() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let mut table = ControlGrantTable::new();
        table.insert(
            keys.pubkey_bytes(),
            ControlGrant::new(
                "placement-controller",
                [ControlVerb::HostDescribe],
                // A narrow member scope that names no real member: the
                // host-plane grant must not depend on widening it.
                ControlMemberScope::members(["nobody"]),
            ),
        );
        let authorizer = ControlAuthorizer::with_grants(table);

        let mut describe = ControlRequest::HostDescribe {
            nonce: Some("n1".to_string()),
            caller: None,
        };
        sign_control_request_as_caller(&keys, None, &mut describe);
        let admitted = authorizer
            .authorize(&describe)
            .expect("named host verb is admitted");
        assert_eq!(
            admitted.map(|caller| caller.label),
            Some("placement-controller".to_string())
        );

        // Negative: the same grant reaches no member-plane verb, and the
        // un-named host verb is refused too.
        let mut health = ControlRequest::HostHealth {
            nonce: Some("n2".to_string()),
            caller: None,
        };
        sign_control_request_as_caller(&keys, None, &mut health);
        assert!(matches!(
            authorizer.authorize(&health),
            Err(ControlAuthzDenial::VerbNotGranted {
                verb: ControlVerb::HostHealth,
                ..
            })
        ));

        let mut inject = ControlRequest::Inject {
            remote_member: "nobody".to_string(),
            content: serde_json::json!({"text": "hi"}),
            nonce: Some("n3".to_string()),
            caller: None,
        };
        sign_control_request_as_caller(&keys, None, &mut inject);
        assert!(matches!(
            authorizer.authorize(&inject),
            Err(ControlAuthzDenial::VerbNotGranted { .. })
        ));
    }

    /// A tampered host projection must fail signature verification.
    ///
    /// The positive control is in the same test: the untampered response
    /// verifies. Without it, a `verify_control_response` that returned
    /// `Ok(())` for every Host response (the fail-open shape, reached by
    /// forgetting the signing-payload arm) would still look green on a
    /// negative-only assertion.
    #[test]
    fn signed_host_facts_do_not_verify_after_tampering() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let request = ControlRequest::HostDescribe {
            nonce: Some("probe-1".to_string()),
            caller: None,
        };
        let request_bytes = serde_json::to_vec(&request).expect("encode request");

        let mut response = ControlResponse::Host {
            facts: echo_host_facts(),
            sig_b64: None,
        };
        sign_control_response(&keys, &request_bytes, &mut response);
        assert!(
            verify_control_response(&keys.pubkey_bytes(), &request_bytes, &response).is_ok(),
            "positive control: the untampered signed response must verify"
        );

        let ControlResponse::Host { sig_b64, .. } = &response else {
            panic!("expected a Host response");
        };
        let signature = sig_b64.clone();

        let mut tampered_facts = echo_host_facts();
        tampered_facts
            .placement_labels
            .insert("zone".to_string(), "attacker".to_string());
        let tampered = ControlResponse::Host {
            facts: tampered_facts,
            sig_b64: signature.clone(),
        };
        assert!(
            verify_control_response(&keys.pubkey_bytes(), &request_bytes, &tampered).is_err(),
            "a tampered placement label must invalidate the host signature"
        );

        let unsigned = ControlResponse::Host {
            facts: echo_host_facts(),
            sig_b64: None,
        };
        assert!(
            verify_control_response(&keys.pubkey_bytes(), &request_bytes, &unsigned).is_err(),
            "an unsigned host response must never verify"
        );

        // A signature minted for the describe request must not carry over
        // to a health response for the same request bytes.
        let crossed = ControlResponse::HostHealth {
            health: echo_host_health(),
            sig_b64: signature,
        };
        assert!(
            verify_control_response(&keys.pubkey_bytes(), &request_bytes, &crossed).is_err(),
            "a facts signature must not verify as a health signature"
        );
    }

    /// A gateway that installs no provider must refuse the host plane
    /// typed rather than answering an empty projection.
    ///
    /// This calls the exact functions the dispatch arms call. The
    /// positive control is the `Some` half: without it, a
    /// `host_describe_response` that refused unconditionally would look
    /// identical from the `None` assertion alone.
    #[test]
    fn host_plane_is_refused_without_a_facts_provider() {
        let refused = host_describe_response(None);
        match &refused {
            ControlResponse::Err { code, .. } => {
                assert_eq!(code, super::super::remote_host::HOST_PLANE_UNAVAILABLE_CODE);
            }
            other => panic!("expected a typed refusal, got {other:?}"),
        }
        match host_health_response(None) {
            ControlResponse::Err { code, .. } => {
                assert_eq!(code, super::super::remote_host::HOST_PLANE_UNAVAILABLE_CODE);
            }
            other => panic!("expected a typed refusal, got {other:?}"),
        }

        let provider =
            super::super::remote_host::StaticHostFacts::new(echo_host_facts(), echo_host_health());
        assert!(matches!(
            host_describe_response(Some(&provider)),
            ControlResponse::Host { .. }
        ));
        assert!(matches!(
            host_health_response(Some(&provider)),
            ControlResponse::HostHealth { .. }
        ));
    }

    /// "Unavailable" must never be classified as an authorization
    /// refusal: the remedies are opposite (install a provider vs. widen a
    /// grant), and a caller that conflated them would retry forever.
    #[test]
    fn host_plane_unavailable_is_not_an_authorization_denial() {
        assert!(!ControlAuthzDenial::is_denial_code(
            super::super::remote_host::HOST_PLANE_UNAVAILABLE_CODE
        ));
        // POSITIVE CONTROL: the classifier does recognise a real denial.
        assert!(ControlAuthzDenial::is_denial_code("caller_not_granted"));
    }

    #[test]
    fn grant_toml_fails_closed_on_bad_policy() {
        let unknown_verb = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["inject", "drop_database"]
            "#
        ))
        .expect_err("unknown verb must fail closed");
        assert!(matches!(
            unknown_verb,
            ControlGrantConfigError::UnknownVerb { .. }
        ));

        let empty_verbs = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = []
            "#
        ))
        .expect_err("an empty verb list is a typo, not a policy");
        assert!(matches!(
            empty_verbs,
            ControlGrantConfigError::InvalidField { .. }
        ));

        let missing_verbs = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            "#
        ))
        .expect_err("verbs is mandatory");
        assert!(matches!(
            missing_verbs,
            ControlGrantConfigError::InvalidField { .. }
        ));

        let duplicate = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["inject"]

            [control_grants.other-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["wire"]
            "#
        ))
        .expect_err("one caller must have exactly one grant");
        assert!(matches!(
            duplicate,
            ControlGrantConfigError::DuplicatePubkey { .. }
        ));

        let bad_key = ControlGrantTable::from_toml(
            r#"
            [control_grants.ops-mob]
            pubkey = "not-base64!!"
            verbs = ["inject"]
            "#,
        )
        .expect_err("an undecodable pubkey must fail closed");
        assert!(matches!(
            bad_key,
            ControlGrantConfigError::InvalidPubkey { .. }
        ));
    }

    /// An ignored key WIDENS this policy, so keys are closed.
    ///
    /// `member` (singular) leaves `members` absent, and an absent `members`
    /// means every member - so tolerating the typo would silently turn a
    /// one-member grant into a whole-mob grant. Same for any narrowing key
    /// a newer config carries that this binary cannot enforce.
    #[test]
    fn grant_toml_rejects_unknown_keys_rather_than_widening() {
        let typo = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["inject"]
            member = ["bob"]
            "#
        ))
        .expect_err("a singular 'member' typo must not silently grant every member");
        assert!(matches!(typo, ControlGrantConfigError::InvalidField { .. }));

        // Positive control: the same policy spelled correctly loads, and
        // scopes to exactly the one member.
        let table = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["inject"]
            members = ["bob"]
            "#
        ))
        .expect("parse")
        .expect("section is present");
        let grant = table.get(&[42u8; 32]).expect("filed");
        assert!(grant.members().contains("bob"));
        assert!(!grant.members().contains("carol"));
    }

    /// End to end over a real socket: the grant-enforcing listener refuses
    /// an out-of-scope verb before the handler ever sees it, and the
    /// calling proxy surfaces the refusal as its own typed error rather
    /// than a generic rejection.
    #[tokio::test]
    async fn grant_enforcing_listener_refuses_ungranted_verbs_over_tcp() {
        use crate::contact_directory::{ContactEntry, MobTransport};
        use crate::runtime::cross_mob_remote::{RemoteMobError, RemoteMobProxy};

        let caller_keys = Arc::new(crate::auth::peer_keys::GatewayPeerKeys::ephemeral());
        let authorizer = Arc::new(ControlAuthorizer::with_grants(table_for(
            &caller_keys,
            &[ControlVerb::LookupMember],
            ControlMemberScope::members(["alice"]),
        )));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(serve_tcp_control_with_authorizer(
            listener,
            handler,
            unsigned_control_signer(),
            authorizer,
        ));

        let entry = ContactEntry {
            mob_id: "remote".to_string(),
            transport: MobTransport::Tcp(addr.to_string()),
            pubkey: None,
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry_with_caller(&entry, Some(Arc::clone(&caller_keys)))
            .expect("tcp ok")
            .expect("some");

        proxy
            .lookup_member("alice")
            .await
            .expect("granted verb on a granted member is admitted");

        let member_denied = proxy
            .lookup_member("bob")
            .await
            .expect_err("bob is outside the member scope");
        assert!(
            matches!(
                member_denied,
                RemoteMobError::Rejected { ref code, .. }
                    if code == "member_not_granted"
            ),
            "got {member_denied:?}"
        );

        let verb_denied = proxy
            .inject_message("alice", serde_json::json!({"text": "hi"}))
            .await
            .expect_err("inject is outside the verb scope");
        assert!(
            matches!(
                verb_denied,
                RemoteMobError::Rejected { ref code, .. }
                    if code == "verb_not_granted"
            ),
            "got {verb_denied:?}"
        );

        // A gateway with no keypair cannot authenticate at all.
        let anonymous = RemoteMobProxy::from_entry(&entry)
            .expect("tcp ok")
            .expect("some");
        let anonymous_denied = anonymous
            .lookup_member("alice")
            .await
            .expect_err("an unauthenticated caller must be refused");
        assert!(
            matches!(
                anonymous_denied,
                RemoteMobError::Rejected { ref code, .. }
                    if code == "unauthenticated_caller"
            ),
            "got {anonymous_denied:?}"
        );
        server.abort();
    }

    /// Back-compat over a real socket: an open listener serves an
    /// authenticated caller unchanged (the credential is inert), which is
    /// what lets callers sign unconditionally during a rollout.
    #[tokio::test]
    async fn open_listener_serves_authenticated_callers_unchanged() {
        use crate::contact_directory::{ContactEntry, MobTransport};
        use crate::runtime::cross_mob_remote::RemoteMobProxy;

        let caller_keys = Arc::new(crate::auth::peer_keys::GatewayPeerKeys::ephemeral());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(serve_tcp_control(
            listener,
            handler,
            unsigned_control_signer(),
        ));

        let entry = ContactEntry {
            mob_id: "remote".to_string(),
            transport: MobTransport::Tcp(addr.to_string()),
            pubkey: None,
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry_with_caller(&entry, Some(caller_keys))
            .expect("tcp ok")
            .expect("some");
        let info = proxy
            .lookup_member("alice")
            .await
            .expect("an open listener ignores the credential");
        assert_eq!(info.peer_id, "peer-id-for-alice");
        server.abort();
    }

    /// A signed caller credential rides inside the request bytes, so the
    /// server's response signature (which digests those bytes) still
    /// verifies for the caller that produced them - the two authenticities
    /// compose rather than collide.
    #[tokio::test]
    async fn signed_caller_and_signed_response_compose() {
        use crate::contact_directory::{ContactEntry, MobTransport};
        use crate::runtime::cross_mob_remote::RemoteMobProxy;

        let caller_keys = Arc::new(crate::auth::peer_keys::GatewayPeerKeys::ephemeral());
        let server_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let pinned = server_keys.pubkey_bytes();
        let authorizer = Arc::new(ControlAuthorizer::with_grants(table_for(
            &caller_keys,
            ControlVerb::all(),
            ControlMemberScope::All,
        )));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler: Arc<dyn ControlHandler> = Arc::new(EchoHandler);
        let server = tokio::spawn(serve_tcp_control_with_authorizer(
            listener,
            handler,
            signer_with(server_keys),
            authorizer,
        ));

        let entry = ContactEntry {
            mob_id: "remote".to_string(),
            transport: MobTransport::Tcp(addr.to_string()),
            pubkey: Some(pinned),
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry_with_caller(&entry, Some(caller_keys))
            .expect("tcp ok")
            .expect("some");
        let info = proxy
            .lookup_member("alice")
            .await
            .expect("authorized request, authenticated answer");
        assert_eq!(info.peer_id, "peer-id-for-alice");
        server.abort();
    }
}
