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
//!   binds it to specific verbs, a member scope and the mobs of this
//!   gateway it reaches (the default mob unless the grant lists mobs; the
//!   request's signed audience names its mob). Anything outside the grant
//!   is refused before dispatch with a typed [`ControlAuthzDenial`], and a
//!   mob outside the grant exactly as a mob this gateway does not have.
//!
//! An admitted request is served on the mob its audience names. A listener
//! of a host with more mobs than its default mob is given the host's mobs
//! (a crate-private router over the host's mob set): the host verbs answer
//! the host's own facts and load no mob, and a request that names a member
//! of another mob is dispatched to a handler over that mob's own entry.
//!
//! A request made with the serving gateway's own key (the envelope names
//! it and the signature verifies) is refused before either, whatever its
//! verb, with [`HOST_OWN_KEY_CONTROL_CODE`]: a gateway's own control
//! listener is not a way to connect two mobs it hosts.
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
        self.serve_all(ControlServing {
            handler,
            signer,
            authorizer,
            mobs: None,
        })
        .await;
    }

    /// [`Self::serve_with_authorizer`] on a host that serves other mobs
    /// beside its default mob: `handler` serves the default mob and the
    /// host verbs, and a request whose signed audience names another mob
    /// the caller's grant reaches is served on that mob's own entry, which
    /// `mobs` resolves.
    pub(crate) async fn serve_on_host(
        self,
        handler: std::sync::Arc<dyn ControlHandler>,
        signer: ControlSignerSlot,
        authorizer: std::sync::Arc<ControlAuthorizer>,
        mobs: std::sync::Arc<dyn ControlHostedMobRouter>,
    ) {
        self.serve_all(ControlServing {
            handler,
            signer,
            authorizer,
            mobs: Some(mobs),
        })
        .await;
    }

    async fn serve_all(self, serving: ControlServing) {
        match self {
            Self::Tcp { listener, .. } => serve_tcp(listener, serving).await,
            #[cfg(unix)]
            Self::Uds { listener, .. } => serve_uds(listener, serving).await,
        }
    }
}

/// What a control listener serves each connection from.
#[derive(Clone)]
struct ControlServing {
    /// Serves the host's default mob, and the host verbs for every mob.
    handler: std::sync::Arc<dyn ControlHandler>,
    signer: ControlSignerSlot,
    authorizer: std::sync::Arc<ControlAuthorizer>,
    /// The host's other mobs. `None`: a listener of one mob.
    mobs: Option<std::sync::Arc<dyn ControlHostedMobRouter>>,
}

/// Maximum control payload size. Control messages are tiny (a few hundred
/// bytes typical, ~1 KiB max for an injected text turn) — we cap well below
/// that so a misbehaving peer can't tie up reads.
pub(crate) const MAX_CONTROL_PAYLOAD: u32 = 64 * 1024;

/// Default request timeout. Control RPC should be fast — local mob
/// dispatch on the remote side is sub-millisecond. We give it 5s to absorb
/// scheduler pauses, network latency, and lazy-spawn warm-up.
pub const DEFAULT_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a request waits for the first read of the record of the
/// members its mob keeps apart from other callers, while that read is on
/// its first attempt (a request can be the one that loaded its mob). It is
/// the control channel's own bound, well inside the time a default caller
/// waits for an answer ([`DEFAULT_CONTROL_TIMEOUT`]): the answer of a read
/// that is still owed after it reaches such a caller.
pub(crate) const MEMBERS_APART_READ_WAIT: Duration = Duration::from_secs(2);

// Twice the wait fits inside the time a default caller waits.
const _: () = assert!(
    MEMBERS_APART_READ_WAIT.as_millis() * 2 <= DEFAULT_CONTROL_TIMEOUT.as_millis(),
    "a request's wait for its mob's first read must end well inside a default caller's wait"
);

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
    /// holds a grant. It also names the mob of the serving gateway the
    /// request is for, which a grant's mobs are matched against (see
    /// [`ControlMobScope`]). Enforced only when the serving authorizer was
    /// built with an expected audience (see
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
    /// The caller holds a grant for this verb, but not on this member of a
    /// mob the grant reaches. (On a listener that was told no audience, and
    /// so knows no mob id, a grant that lists mob ids reaches no member.)
    MemberNotGranted {
        label: String,
        verb: ControlVerb,
        member: String,
    },
    /// The request was minted for a mob this gateway does not answer the
    /// caller for: its audience is no mob of this gateway, or it is a mob
    /// the caller's grant does not reach. The two are one refusal, value
    /// for value, so a caller learns nothing about a mob outside its grant.
    /// `expected` names the gateway's default mob when the caller's grant
    /// reaches the default mob, and is empty otherwise.
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
            // A caller whose grant does not reach the default mob is told
            // nothing but the id it sent: `expected` is empty for it.
            Self::AudienceMismatch {
                label,
                expected,
                presented,
            } if expected.is_empty() => write!(
                f,
                "caller '{label}' presented a request minted for '{presented}', which this \
                 gateway does not answer for"
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

/// The code a control listener refuses a request with when the request was
/// made with this gateway's own key.
///
/// A gateway never serves its own control requests: the loopback through
/// its own listener would be a second way to connect two mobs it hosts.
/// The rule has no verb list, so the read-only verbs are refused too. This
/// is not one of the [`CONTROL_AUTHZ_DENIAL_CODES`]: no grant admits such a
/// request, so a wider grant is not the remedy.
pub const HOST_OWN_KEY_CONTROL_CODE: &str = "host_own_key";

/// What a control listener answers a request made with `own_keys`, the key
/// this gateway itself signs with: a request whose caller envelope names
/// that key and whose signature verifies against it. `None` for every
/// other request, and for a gateway that has no key installed.
///
/// Only a signature that verifies makes a request this gateway's own. A
/// claim of the key without one falls through to the authorizer and gets
/// the answer any unverified claim gets, so on a listener that enforces
/// grants a caller without a grant cannot use this refusal to test whether
/// a key is this gateway's. The signature is checked for every key an
/// envelope names, this gateway's or not, so the work done before the
/// answer does not depend on whose key was named either.
fn host_own_key_refusal(
    request: &ControlRequest,
    own_keys: Option<&crate::auth::peer_keys::GatewayPeerKeys>,
) -> Option<ControlResponse> {
    let own_keys = own_keys?;
    let caller = request.caller()?;
    let claimed = crate::auth::peer_keys::decode_pubkey_b64(&caller.pubkey_b64).ok()?;
    let verified = verify_caller_signature(&claimed, caller, request).is_ok();
    if !verified || claimed != own_keys.pubkey_bytes() {
        return None;
    }
    Some(ControlResponse::Err {
        code: HOST_OWN_KEY_CONTROL_CODE.to_string(),
        message: format!(
            "this gateway does not serve a control request made with its own key, so \
             '{verb}' is refused: its own control listener is not a way to connect two \
             mobs it hosts, and no control grant changes that",
            verb = request.verb()
        ),
    })
}

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

/// Which of the host's mobs a grant reaches.
///
/// A request names its mob with the signed audience (see [`ControlCaller`]).
/// A grant that names no mob reaches the host's default mob only (the mob
/// the launch names), which is what every grant written before grants could
/// list mobs means.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ControlMobScope {
    /// The grant names no mob: it reaches the host's default mob only.
    #[default]
    DefaultMob,
    /// Every mob of the host, present and future.
    All,
    /// Only these mobs. The default mob is named by its id like any other;
    /// an id that is no mob of the host reaches nothing.
    Mobs(BTreeSet<meerkat_mob::MobId>),
}

impl ControlMobScope {
    /// Build a mob scope from mob ids. `*` anywhere in the list widens to
    /// [`Self::All`]. An empty list reaches no mob; use
    /// [`Self::DefaultMob`] for a grant that names none.
    pub fn mobs<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = BTreeSet::new();
        for id in ids {
            let id = id.as_ref();
            if id == "*" {
                return Self::All;
            }
            set.insert(meerkat_mob::MobId::from(id));
        }
        Self::Mobs(set)
    }

    /// Whether this scope reaches the mob a request is for.
    fn reaches(&self, mob: &AddressedMob<'_>) -> bool {
        match (self, mob) {
            (Self::All, _) => true,
            (Self::DefaultMob, AddressedMob::Default(_) | AddressedMob::UnnamedDefault) => true,
            (Self::DefaultMob, AddressedMob::Other(_)) => false,
            (Self::Mobs(mobs), AddressedMob::Default(id) | AddressedMob::Other(id)) => {
                mobs.contains(*id)
            }
            // A listener that was told no audience knows no id for its one
            // mob, so no listed id can be shown to name it.
            (Self::Mobs(_), AddressedMob::UnnamedDefault) => false,
        }
    }
}

/// The mob one request is for, as the audience check resolved it. Resolved
/// once per request, before the grant is consulted.
enum AddressedMob<'a> {
    /// The host's default mob, named by the audience the authorizer was
    /// built for.
    Default(&'a str),
    /// Another mob of the host, named by the request's signed audience.
    Other(&'a str),
    /// The one mob of a listener that was told no audience: the audience
    /// of a request is not checked there and never selects a mob.
    UnnamedDefault,
}

/// "Is this id a mob of this host", asked at most once per request by the
/// audience check, and only about an id the caller's grant reaches.
///
/// The owner of the host's mob set answers it from the set's own records,
/// without loading a mob. The authorizer keeps no list of mobs: it knows
/// the default mob (the audience it was built for) and asks for every
/// other id. This is the form for a host that can answer at once; a
/// listener whose mob set answers asynchronously puts the same question
/// between [`ControlAuthorizer::begin`] and [`PendingAudience::finish`].
pub(crate) trait ControlHostedMobs {
    /// Whether `mob_id` is a mob of this host. Asked only about an id
    /// other than the default mob's, which the authorizer already knows.
    fn hosts_mob(&self, mob_id: &meerkat_mob::MobId) -> bool;
}

/// A host with no mob beside its default mob.
struct NoOtherMob;

impl ControlHostedMobs for NoOtherMob {
    fn hosts_mob(&self, _mob_id: &meerkat_mob::MobId) -> bool {
        false
    }
}

/// The mob a request the grant policy admitted is for. It is the only
/// thing a listener routes a request on: never a second reading of the
/// request's envelope, whose audience an open listener and a listener
/// that was told no audience do not check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdmittedMob {
    /// The host's default mob: named by the audience the authorizer was
    /// built for, or the one mob of a listener that checks no audience.
    Default,
    /// Another mob of the host, which the signed audience named, the
    /// caller's grant reaches and the host said it serves.
    Other(AdmittedOtherMob),
}

/// A mob of the host other than its default mob, admitted for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdmittedOtherMob {
    mob_id: meerkat_mob::MobId,
    /// What the request is answered when the host no longer serves the
    /// mob by the time the request is routed: the refusal it would have
    /// got had the mob been gone a moment earlier.
    not_served: ControlAuthzDenial,
    /// Whether the caller's grant reaches the host's default mob too. A
    /// caller whose grant does not is told nothing about that mob, its id
    /// included: not in a refusal, and not in the host's facts.
    names_the_default_mob: bool,
}

impl AdmittedOtherMob {
    /// The id of the admitted mob.
    pub(crate) fn mob_id(&self) -> &meerkat_mob::MobId {
        &self.mob_id
    }

    /// The refusal for a mob the host does not serve.
    pub(crate) fn not_served(&self) -> &ControlAuthzDenial {
        &self.not_served
    }
}

/// What the grant policy admitted for one request: the caller, and the mob
/// the request is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ControlAdmission {
    /// The admitted caller; `None` on a listener that enforces no grants.
    pub(crate) caller: Option<AuthorizedCaller>,
    /// The mob the request is served on.
    pub(crate) mob: AdmittedMob,
}

/// Where the grant policy stands for one request after everything it can
/// decide alone.
pub(crate) enum AuthorizeStep<'a> {
    /// Admitted: no question for the host.
    Admitted(ControlAdmission),
    /// The signed audience names a mob other than the default mob, and the
    /// caller's grant reaches it: whether the host serves that mob is the
    /// one question left.
    AskHost(PendingAudience<'a>),
}

/// A request whose admission waits for the host's answer about one mob.
pub(crate) struct PendingAudience<'a> {
    grant: &'a ControlGrant,
    pubkey: [u8; 32],
    request: &'a ControlRequest,
    mob: AdmittedOtherMob,
}

impl PendingAudience<'_> {
    /// The mob to ask the host about.
    pub(crate) fn mob_id(&self) -> &meerkat_mob::MobId {
        self.mob.mob_id()
    }

    /// Finish the check with the host's answer. A mob the host does not
    /// serve is refused exactly as a mob the grant does not reach; for a
    /// mob it serves, the grant's verbs and members decide.
    pub(crate) fn finish(self, hosted: bool) -> Result<ControlAdmission, ControlAuthzDenial> {
        if !hosted {
            return Err(self.mob.not_served);
        }
        self.grant.permits(
            self.request.verb(),
            &AddressedMob::Other(self.mob.mob_id.as_str()),
            self.request.remote_member(),
        )?;
        Ok(ControlAdmission {
            caller: Some(AuthorizedCaller {
                label: self.grant.label().to_string(),
                pubkey: self.pubkey,
            }),
            mob: AdmittedMob::Other(self.mob),
        })
    }
}

/// What a [`ControlHostedMobRouter`] answers, when it has asked the host's
/// mob set.
pub(crate) type HostedMobAnswer<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// The mobs a control listener serves beside the host's default mob.
///
/// The host's mob set owns which mobs those are and whether one is loaded;
/// the listener holds none of it and asks per request. A listener without
/// one serves the default mob alone.
pub(crate) trait ControlHostedMobRouter: Send + Sync + 'static {
    /// Whether `mob_id` is a mob this host serves, answered from the mob
    /// set's own records: it loads no mob. Asked by the audience check,
    /// only about an id the caller's grant reaches.
    fn serves<'a>(&'a self, mob_id: &'a meerkat_mob::MobId) -> HostedMobAnswer<'a, bool>;

    /// The handler over the entry of the mob `mob_id`, for one request of
    /// `verb`: its handle, its identity runtime, its session service, and
    /// for a keyed inject verb the mob's own receiver. This is the lookup
    /// that loads a mob the set knows and has not loaded; it is asked only
    /// for an admitted request that names a member. `None` when the host
    /// does not serve the mob any more.
    fn handler_for<'a>(
        &'a self,
        mob_id: &'a meerkat_mob::MobId,
        verb: ControlVerb,
    ) -> HostedMobAnswer<'a, Option<std::sync::Arc<dyn ControlHandler>>>;
}

/// What one caller identity may do on this gateway's control channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlGrant {
    label: String,
    verbs: BTreeSet<ControlVerb>,
    members: ControlMemberScope,
    mobs: ControlMobScope,
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
            mobs: ControlMobScope::DefaultMob,
        }
    }

    /// The same grant over `mobs` instead of the host's default mob only.
    #[must_use]
    pub fn with_mobs(mut self, mobs: ControlMobScope) -> Self {
        self.mobs = mobs;
        self
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

    /// Mobs of the host this caller may reach.
    pub fn mobs(&self) -> &ControlMobScope {
        &self.mobs
    }

    /// Check one request against this grant.
    ///
    /// ORDER IS LOAD-BEARING: the verb check runs first and unconditionally,
    /// so the host-plane exemption below is only ever reachable for a verb
    /// the operator named explicitly. Hoisting the exemption above the verb
    /// check would turn it into a bypass.
    fn permits(
        &self,
        verb: ControlVerb,
        mob: &AddressedMob<'_>,
        member: &str,
    ) -> Result<(), ControlAuthzDenial> {
        if !self.verbs.contains(&verb) {
            return Err(ControlAuthzDenial::VerbNotGranted {
                label: self.label.clone(),
                verb,
            });
        }
        // Host-plane verbs address the serving gateway itself, so there is
        // no member to scope. (Their audience was held against this grant's
        // mobs like any other request's before this is reached.) Requiring
        // `members = ["*"]`
        // to reach them would be worse than exempting them: an operator
        // granting a read-only host projection would have to widen the SAME
        // grant's `inject` to every member to do it.
        if verb.is_host_plane() {
            return Ok(());
        }
        // A member is a member of one mob. The audience check has already
        // refused a mob this grant does not reach; what is left for the
        // mob scope here is a listener that was told no audience, where a
        // grant that lists mob ids reaches no member.
        if !self.mobs.reaches(mob) || !self.members.contains(member) {
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
    /// mobs = ["mob-second"]      # omit for the default mob; ["*"] for every mob
    /// ```
    ///
    /// Returns `Ok(None)` when the section is ABSENT, which is what keeps
    /// this backward compatible: a config that never mentions grants must
    /// stay on [`ControlAuthorizer::open`] rather than silently becoming a
    /// deny-all table. A present-but-empty section is `Ok(Some(empty))` -
    /// an operator who wrote the section and listed nobody meant deny-all.
    ///
    /// Keys inside a caller entry are CLOSED: anything other than
    /// `pubkey`, `verbs`, `members`, `mobs` is an error rather than an
    /// ignored key. See the check in the loop for why silence there widens.
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
                if !matches!(key.as_str(), "pubkey" | "verbs" | "members" | "mobs") {
                    return Err(ControlGrantConfigError::InvalidField {
                        label: label.clone(),
                        detail: format!(
                            "unknown key '{key}'; a control grant accepts only pubkey, verbs, \
                             members, and mobs (did you mean 'members' or 'mobs'?)"
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
            // An absent `mobs` is the default mob, which is what every
            // grant written before grants could list mobs means. A `mobs`
            // the operator wrote must name something: an empty list or an
            // empty id is refused, never read as the default mob (wider
            // than what was written for a grant meant for another mob) and
            // never as every mob.
            let mobs = match entry.get("mobs") {
                None => ControlMobScope::DefaultMob,
                Some(value) => {
                    let items =
                        value
                            .as_array()
                            .ok_or_else(|| ControlGrantConfigError::InvalidField {
                                label: label.clone(),
                                detail: "'mobs' must be an array of mob ids".to_string(),
                            })?;
                    if items.is_empty() {
                        return Err(ControlGrantConfigError::InvalidField {
                            label: label.clone(),
                            detail: "'mobs' is empty; omit it for the default mob, or list the \
                                     mob ids (or \"*\") the caller may reach"
                                .to_string(),
                        });
                    }
                    let mut ids = Vec::with_capacity(items.len());
                    for item in items {
                        let id =
                            item.as_str()
                                .ok_or_else(|| ControlGrantConfigError::InvalidField {
                                    label: label.clone(),
                                    detail: "'mobs' entries must be strings".to_string(),
                                })?;
                        if id.is_empty() {
                            return Err(ControlGrantConfigError::InvalidField {
                                label: label.clone(),
                                detail: "'mobs' entries must not be empty".to_string(),
                            });
                        }
                        // A mob id is matched exactly, so an id with
                        // whitespace around it would load and reach no mob.
                        if id.trim() != id {
                            return Err(ControlGrantConfigError::InvalidField {
                                label: label.clone(),
                                detail: format!(
                                    "'mobs' entry {id:?} begins or ends with whitespace; a mob \
                                     id is matched exactly"
                                ),
                            });
                        }
                        ids.push(id);
                    }
                    ControlMobScope::mobs(ids)
                }
            };
            labels_by_pubkey.insert(pubkey, label.clone());
            out.insert(
                pubkey,
                ControlGrant::new(label.clone(), verbs, members).with_mobs(mobs),
            );
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
    /// never a fallback to [`Self::open`]. The authorizer is told no
    /// audience, so it knows no mob id: a grant that lists mob ids reaches
    /// nothing here (see [`Self::with_grants_for_audience`]).
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
    ///
    /// `audience` is the gateway's default mob (the mob its launch names):
    /// a grant without `mobs` reaches that mob only, and a grant that lists
    /// mobs reaches it only by this id. An authorizer built without an
    /// audience ([`Self::with_grants`]) knows no mob id, so a grant that
    /// lists mob ids matches nothing there.
    pub fn with_grants_for_audience(table: ControlGrantTable, audience: impl Into<String>) -> Self {
        Self::Grants {
            table,
            expected_audience: Some(audience.into()),
        }
    }

    /// Parse a `[control_grants]` section into an authorizer: absent
    /// section yields [`Self::Open`], present section yields
    /// [`Self::Grants`].
    ///
    /// The authorizer this builds was told no audience, so it knows no mob
    /// id and a grant that lists mob ids could match nothing. Such a grant
    /// is a configuration error here, not a grant that silently reaches no
    /// one: a host that lists mobs parses the table with
    /// [`ControlGrantTable::from_toml`] and builds the authorizer with
    /// [`Self::with_grants_for_audience`]. A grant without `mobs`, or with
    /// `mobs = ["*"]`, loads.
    pub fn from_toml(text: &str) -> Result<Self, ControlGrantConfigError> {
        let Some(table) = ControlGrantTable::from_toml(text)? else {
            return Ok(Self::Open);
        };
        if let Some(grant) = table
            .grants
            .values()
            .find(|grant| matches!(grant.mobs, ControlMobScope::Mobs(_)))
        {
            return Err(ControlGrantConfigError::InvalidField {
                label: grant.label.clone(),
                detail: "'mobs' lists mob ids, which only an authorizer that was told its \
                         default mob can match; build it with \
                         ControlAuthorizer::with_grants_for_audience, or grant every mob \
                         with [\"*\"]"
                    .to_string(),
            });
        }
        Ok(Self::with_grants(table))
    }

    /// Whether this authorizer refuses anything at all.
    pub fn is_enforcing(&self) -> bool {
        matches!(self, Self::Grants { .. })
    }

    /// Authorize one request. `Ok(None)` means the listener does not
    /// enforce grants; `Ok(Some(caller))` names the admitted caller.
    ///
    /// This is the form of a listener that serves one mob: a request for
    /// any other mob is refused as one minted for another gateway.
    pub fn authorize(
        &self,
        request: &ControlRequest,
    ) -> Result<Option<AuthorizedCaller>, ControlAuthzDenial> {
        self.authorize_on_host(request, &NoOtherMob)
    }

    /// [`Self::authorize`] on a host whose mobs `hosted` answers for at
    /// once.
    pub(crate) fn authorize_on_host(
        &self,
        request: &ControlRequest,
        hosted: &dyn ControlHostedMobs,
    ) -> Result<Option<AuthorizedCaller>, ControlAuthzDenial> {
        self.admit_on_host(request, hosted)
            .map(|admission| admission.caller)
    }

    /// [`Self::authorize_on_host`], with the mob the request was admitted
    /// for: [`Self::begin`], the question, and [`PendingAudience::finish`]
    /// in one call.
    pub(crate) fn admit_on_host(
        &self,
        request: &ControlRequest,
        hosted: &dyn ControlHostedMobs,
    ) -> Result<ControlAdmission, ControlAuthzDenial> {
        match self.begin(request)? {
            AuthorizeStep::Admitted(admission) => Ok(admission),
            AuthorizeStep::AskHost(pending) => {
                let hosted = hosted.hosts_mob(pending.mob_id());
                pending.finish(hosted)
            }
        }
    }

    /// Everything the grant policy decides without the host: the caller's
    /// credential, its grant, and the mob its signed audience names.
    ///
    /// The audience must be a mob the caller's grant reaches and the host
    /// serves. The default mob is the audience this authorizer was built
    /// for. Any other id is first held against the grant's mobs: an id the
    /// grant does not reach is refused here, and the host is never asked
    /// about it. For an id the grant reaches the answer is
    /// [`AuthorizeStep::AskHost`], and a mob the host does not serve gets
    /// the same refusal, value for value. So nothing a caller is answered
    /// tells a mob outside its grant from an id no mob has, the default
    /// mob included.
    ///
    /// A listener that enforces no grants, and an authorizer that was told
    /// no audience, check no audience: every request they admit is the
    /// default mob's, whatever its envelope names.
    pub(crate) fn begin<'a>(
        &'a self,
        request: &'a ControlRequest,
    ) -> Result<AuthorizeStep<'a>, ControlAuthzDenial> {
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
            return Ok(AuthorizeStep::Admitted(ControlAdmission {
                caller: None,
                mob: AdmittedMob::Default,
            }));
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
        let mob = match expected_audience.as_deref() {
            Some(default_mob) => {
                let presented = caller.audience.as_deref().unwrap_or("");
                // The one refusal for an audience that is no mob of the
                // host in the grant's reach. It names the default mob only
                // to a caller whose grant reaches the default mob.
                let mismatch = || ControlAuthzDenial::AudienceMismatch {
                    label: grant.label().to_string(),
                    expected: if grant.mobs.reaches(&AddressedMob::Default(default_mob)) {
                        default_mob.to_string()
                    } else {
                        String::new()
                    },
                    presented: presented.to_string(),
                };
                let addressed = if presented == default_mob {
                    AddressedMob::Default(default_mob)
                } else {
                    AddressedMob::Other(presented)
                };
                // ORDER IS LOAD-BEARING: the grant's reach is read before
                // the host is asked, so a mob outside the grant is never a
                // question for the host and never answered differently
                // from an id no mob has.
                if presented.is_empty() || !grant.mobs.reaches(&addressed) {
                    return Err(mismatch());
                }
                if let AddressedMob::Other(other) = addressed {
                    let names_the_default_mob =
                        grant.mobs.reaches(&AddressedMob::Default(default_mob));
                    return Ok(AuthorizeStep::AskHost(PendingAudience {
                        grant,
                        pubkey,
                        request,
                        mob: AdmittedOtherMob {
                            mob_id: meerkat_mob::MobId::from(other),
                            not_served: mismatch(),
                            names_the_default_mob,
                        },
                    }));
                }
                addressed
            }
            None => AddressedMob::UnnamedDefault,
        };
        grant.permits(verb, &mob, request.remote_member())?;
        Ok(AuthorizeStep::Admitted(ControlAdmission {
            caller: Some(AuthorizedCaller {
                label: grant.label().to_string(),
                pubkey,
            }),
            mob: AdmittedMob::Default,
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
    /// Which members the member verbs answer as a name no member has.
    members_apart: MembersApart,
}

/// Which members of a mob a peer gateway's control request does not reach.
///
/// A member that is kept apart from other callers (its roster entry carries
/// the label that keeps it apart, or the runtime's record of its members
/// says so) is answered, by every verb that names a member, exactly as a
/// name no member has: nothing is read of it, nothing is wired to it and
/// nothing reaches its session, whatever the caller's grant lists. The
/// roster entry's own label is read for every handler. A runtime attaches
/// its record too; while that record still owes its first read it cannot
/// tell, and every name is answered as a name no member has until the read
/// has succeeded. A handler built without the record has the roster
/// entries' labels alone.
#[derive(Clone, Default)]
pub(crate) struct MembersApart {
    record: Option<crate::private_owners::PrivateOwners>,
}

impl MembersApart {
    /// Over a runtime's record of who its members are kept apart for.
    pub(crate) fn of_the_runtime(record: crate::private_owners::PrivateOwners) -> Self {
        Self {
            record: Some(record),
        }
    }

    /// Whether the roster member `member` of `handle`'s mob is answered as
    /// a name no member has. A member the record cannot tell yet (it has
    /// not read the durable records of the member's mob, which is never so
    /// for the launch's own mob) is answered so too.
    async fn holds(
        &self,
        handle: &meerkat_mob::MobHandle,
        member: &meerkat_mob::AgentIdentity,
    ) -> bool {
        if let Some(record) = &self.record
            && (record.first_read_pending()
                || record
                    .owner_of(handle.mob_id().as_str(), member.as_str())
                    .is_some()
                || matches!(
                    record.person_for_a_reader(handle.mob_id().as_str(), member.as_str()),
                    crate::mob_announcements::MemberPerson::NotKnown
                ))
        {
            return true;
        }
        matches!(
            handle.get_member(member).await,
            Ok(Some(entry)) if crate::access::private_owner(&entry.labels).is_some()
        )
    }
}

/// What a verb that reads a member answers a name the mob's roster does not
/// hold.
fn no_such_member(remote_member: &str, mob_id: &str) -> (String, String) {
    (
        "unknown_member".to_string(),
        format!("member '{remote_member}' not in mob '{mob_id}'"),
    )
}

/// What Core answers a command for a roster id no member has.
fn member_not_found(member: &meerkat_mob::AgentIdentity) -> String {
    meerkat_mob::MobError::MemberNotFound(member.clone()).to_string()
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
            members_apart: MembersApart::default(),
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
            members_apart: MembersApart::default(),
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
            members_apart: MembersApart::default(),
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

    /// Answer the members `apart` holds as names no member has (see
    /// [`MembersApart`]). A runtime gives its listener's handler its own
    /// record.
    #[must_use]
    pub(crate) fn with_members_apart(mut self, apart: MembersApart) -> Self {
        self.members_apart = apart;
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
        let members_apart = self.members_apart.clone();
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
                        &members_apart,
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
                        &members_apart,
                    )
                    .await
                }
                ControlRequest::Inject {
                    remote_member,
                    content,
                    nonce: _,
                    caller: _,
                } => {
                    handle_inject(
                        &handle,
                        &remote_member,
                        content,
                        identity_runtime.as_ref(),
                        &members_apart,
                    )
                    .await
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
                        &members_apart,
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
        // An archived identity created at runtime has no member until its
        // unarchive: not active, as any identity Core holds no member of.
        crate::identity_first::IdentityRuntimeError::InvalidState { .. }
        | crate::identity_first::IdentityRuntimeError::IdentityArchived { .. } => {
            "identity_not_active"
        }
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
        return Err(generated_alias_without_identity_runtime(&member_alias));
    }
    operation().await
}

/// What a member verb answers a generated alias on a handler that has no
/// identity runtime to ask whose alias it is.
fn generated_alias_without_identity_runtime(member_alias: &str) -> (String, String) {
    (
        "identity_authority_unavailable".to_string(),
        format!("generated member alias '{member_alias}' requires the owning IdentityRuntime"),
    )
}

/// The answer for `remote_member` when the member it names is kept apart
/// from other callers; `None` when it is not, and the verb goes on.
///
/// Every member verb asks this once, before its identity runtime or its
/// roster is consulted for the member, so nothing of the member decides the
/// answer: not whether it is registered, not whether it has a member now,
/// not which of its incarnations is current. The answer is the one this
/// handler gives a name no member has IN THE SAME SPELLING:
/// - a generated alias (`rt:<name>:<n>`, whatever the number) answers as
///   the alias of an identity the identity runtime does not hold, or, on a
///   handler without an identity runtime, as any generated alias does;
/// - a plain name answers what `plain_name` builds from the name as it is
///   spelled and the roster id it stands for: the verb's own answer for a
///   name its roster does not hold.
///
/// Kept apart is: what [`MembersApart::holds`] says of the roster member
/// the name stands for, or, with an identity runtime, that the registered
/// row of the identity the name stands for carries the label (an identity
/// that has no member yet has no roster entry to read).
///
/// A name in the reserved alias space that is no generated alias names no
/// member and no identity: the verb refuses the spelling itself.
async fn answer_for_a_member_kept_apart(
    apart: &MembersApart,
    handle: &meerkat_mob::MobHandle,
    remote_member: &str,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    plain_name: impl FnOnce(&str, &meerkat_mob::AgentIdentity) -> (String, String),
) -> Option<(String, String)> {
    let alias = crate::member_comms_id::runtime_alias_str(remote_member).into_owned();
    let generated =
        crate::identity_first::IdentityRuntime::identity_for_generated_member_alias(&alias);
    let (roster_member, identity) = match &generated {
        Some(identity) => (
            crate::member_comms_id::mob_member_id(identity.as_str()),
            Some(identity.clone()),
        ),
        None if crate::member_comms_id::is_reserved_generated_alias(&alias) => return None,
        None => (
            crate::member_comms_id::roster_member_id_for_supplied_id(&alias),
            crate::identity_first::AgentIdentity::parse(&alias).ok(),
        ),
    };
    let registered_apart = match (identity_runtime, &identity) {
        (Some(runtime), Some(identity)) => {
            runtime
                .registered_spec(identity)
                .await
                .is_ok_and(|registered| {
                    crate::access::private_owner(&registered.spec.labels).is_some()
                })
        }
        _ => false,
    };
    if !registered_apart && !apart.holds(handle, &roster_member).await {
        return None;
    }
    Some(match generated {
        Some(identity) if identity_runtime.is_some() => control_identity_error(
            crate::identity_first::IdentityRuntimeError::UnknownIdentity(identity),
        ),
        Some(_) => generated_alias_without_identity_runtime(&alias),
        None => plain_name(&alias, &roster_member),
    })
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
    apart: &MembersApart,
) -> ControlResponse {
    let spec = match build_remote_wire_descriptor(&params) {
        Ok(spec) => spec,
        Err((code, message)) => return ControlResponse::Err { code, message },
    };
    // A member kept apart is no member for a peer: the answer is the one
    // the same spelling of a name no member has gets, and nothing is wired.
    if let Some((code, message)) = answer_for_a_member_kept_apart(
        apart,
        handle,
        params.remote_member,
        identity_runtime,
        |_, roster_member| ("mob_error".to_string(), member_not_found(roster_member)),
    )
    .await
    {
        return ControlResponse::Err { code, message };
    }
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
    apart: &MembersApart,
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
    // A durable identity is reached through its identity door, whatever its
    // residency: its admission guards (addressability, a pending first-boot
    // row, an owed held completion, the registered row) and its delivery
    // preparation apply, and Core wakes it when it is dormant. The door takes
    // the identity's own lifecycle lock and checks a generated alias itself.
    let member_alias = crate::member_comms_id::runtime_alias_str(remote_member).into_owned();
    // A member kept apart is no member for a peer: neither its identity door
    // nor its raw door is tried, and the answer is the one the same spelling
    // of a name no member has gets.
    if let Some((code, message)) = answer_for_a_member_kept_apart(
        apart,
        handle,
        remote_member,
        identity_runtime,
        |_, roster_member| {
            (
                "unknown_member".to_string(),
                member_not_found(roster_member),
            )
        },
    )
    .await
    {
        return ControlResponse::Err { code, message };
    }
    // A name the identity runtime refuses (Core holds it for an identity the
    // runtime has not registered, or its entry could not be read) is refused
    // typed, never sent through the raw member door.
    let identity = match identity_runtime {
        Some(runtime) => match runtime.identity_for_member_mutation(&member_alias).await {
            Ok(identity) => identity,
            Err(error) => {
                let (code, message) = control_identity_error(error);
                return ControlResponse::Err { code, message };
            }
        },
        None => None,
    };
    if let Some(runtime) = identity_runtime
        && let Some(identity) = identity
    {
        return match runtime
            .send_through_identity_door(&identity, &member_alias, &content_input)
            .await
        {
            Ok(session_id) => ControlResponse::Injected {
                session_id,
                sig_b64: None,
            },
            Err(error) => {
                let (code, message) = control_identity_error(error);
                ControlResponse::Err { code, message }
            }
        };
    }
    let operation_handle = handle.clone();
    let operation_member = member_alias;
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
    apart: &MembersApart,
) -> ControlResponse {
    // A member kept apart is no member for a peer: nothing of its roster
    // entry is read out, and the answer is the one the same spelling of a
    // name no member has gets.
    if let Some((code, message)) = answer_for_a_member_kept_apart(
        apart,
        handle,
        remote_member,
        identity_runtime,
        |alias, _| no_such_member(alias, handle.mob_id().as_str()),
    )
    .await
    {
        return ControlResponse::Err { code, message };
    }
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

/// The comms identity a peer-info read reports for `member` when its roster
/// row projects none: its peer id and transport key (`ed25519:<base64>`), as
/// MobMachine records them for its current incarnation.
///
/// A roster row projects a member's comms identity only while a comms
/// runtime of it is live, so a member Core holds unmaterialized (a seat no
/// delivery has woken yet, or a released member) projects none. Its seat
/// recorded the endpoint its build loads, which a live peer trusts at once,
/// and that endpoint is the member's. A read: it wakes nothing. `None` when
/// MobMachine holds no usable endpoint either: a Broken member, or a member
/// on network comms, whose endpoint its first wake registers.
pub(crate) async fn recorded_member_comms_identity(
    handle: &meerkat_mob::MobHandle,
    member: &meerkat_mob::ids::AgentIdentity,
) -> Result<Option<(String, String)>, meerkat_mob::MobError> {
    Ok(handle.member_peer_endpoint(member).await?.map(|endpoint| {
        (
            endpoint.descriptor.peer_id.to_string(),
            meerkat_comms::PubKey::new(endpoint.descriptor.pubkey).to_pubkey_string(),
        )
    }))
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
            return Err(no_such_member(remote_member, &mob_id));
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
    serve_tcp(
        listener,
        ControlServing {
            handler,
            signer,
            authorizer,
            mobs: None,
        },
    )
    .await;
}

async fn serve_tcp(listener: TcpListener, serving: ControlServing) {
    loop {
        let (stream, _peer_addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(error = %err, "control listener accept failed; exiting");
                return;
            }
        };
        tokio::spawn(serve_one_tcp(stream, serving.clone()));
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
    serve_uds(
        listener,
        ControlServing {
            handler,
            signer,
            authorizer,
            mobs: None,
        },
    )
    .await;
}

#[cfg(unix)]
async fn serve_uds(listener: UnixListener, serving: ControlServing) {
    loop {
        let (stream, _peer_addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(error = %err, "uds control listener accept failed; exiting");
                return;
            }
        };
        tokio::spawn(serve_one_uds(stream, serving.clone()));
    }
}

async fn serve_one_tcp(stream: TcpStream, serving: ControlServing) {
    let mut s = ControlStream::Tcp(stream);
    serve_one(&mut s, serving).await;
}

#[cfg(unix)]
async fn serve_one_uds(stream: UnixStream, serving: ControlServing) {
    let mut s = ControlStream::Uds(stream);
    serve_one(&mut s, serving).await;
}

/// Write the grant policy's refusal of one request.
async fn refuse(stream: &mut ControlStream, verb: ControlVerb, denial: &ControlAuthzDenial) {
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
}

async fn serve_one(stream: &mut ControlStream, serving: ControlServing) {
    let ControlServing {
        handler,
        signer,
        authorizer,
        mobs,
    } = serving;
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
    // A request made with the gateway's own key first, before the grant
    // table is read: no grant admits it. The key is read from the signer
    // slot per request, where it lives; hosts install it after the
    // listener exists. Only a verified signature reaches the refusal and
    // its warning; a mere claim of the key goes on to the authorizer.
    let own_keys = signer
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(response) = host_own_key_refusal(&request, own_keys.as_deref()) {
        tracing::warn!(
            code = HOST_OWN_KEY_CONTROL_CODE,
            verb = %verb,
            "cross-mob control request refused: it was made with this gateway's own key"
        );
        let response_payload = serde_json::to_vec(&response).unwrap_or_default();
        let _ = stream.write_frame(&response_payload).await;
        return;
    }
    // The grant policy decides alone up to one question: whether the host
    // serves the mob the signed audience names. It is asked only about a
    // mob the caller's grant reaches, and answered from the mob set's own
    // records, so authorization loads no mob. A listener of one mob has no
    // other mob.
    let admission = {
        let step = match authorizer.begin(&request) {
            Ok(step) => step,
            Err(denial) => {
                refuse(stream, verb, &denial).await;
                return;
            }
        };
        let admitted = match step {
            AuthorizeStep::Admitted(admission) => Ok(admission),
            AuthorizeStep::AskHost(pending) => {
                let hosted = match &mobs {
                    Some(mobs) => mobs.serves(pending.mob_id()).await,
                    None => false,
                };
                pending.finish(hosted)
            }
        };
        match admitted {
            Ok(admission) => admission,
            Err(denial) => {
                refuse(stream, verb, &denial).await;
                return;
            }
        }
    };
    if let Some(caller) = &admission.caller {
        tracing::debug!(
            caller = %caller.label,
            verb = %verb,
            "cross-mob control request admitted by grant"
        );
    }
    // Route on the mob the grant policy admitted the request for, and on
    // nothing else: the envelope is not read again.
    let handler = match &admission.mob {
        AdmittedMob::Default => handler,
        // The host verbs answer the host's own facts: they name no member,
        // resolve no mob and load none.
        AdmittedMob::Other(_) if verb.is_host_plane() => handler,
        // A verb that names a member is served on its mob's own entry. This
        // is the lookup that loads a mob the set has not loaded.
        AdmittedMob::Other(other) => {
            let of_the_mob = match &mobs {
                Some(mobs) => mobs.handler_for(other.mob_id(), verb).await,
                None => None,
            };
            match of_the_mob {
                Some(handler) => handler,
                // The host stopped serving the mob since it was asked: the
                // answer is the one a moment earlier would have got.
                None => {
                    refuse(stream, verb, other.not_served()).await;
                    return;
                }
            }
        }
    };
    let mut response = handler.handle_admitted(request, admission.caller).await;
    // The host's facts name the host by its default mob's id. A caller
    // whose grant does not reach that mob is told nothing about it: for
    // that caller the host goes by the mob the request was signed for.
    if let AdmittedMob::Other(other) = &admission.mob
        && !other.names_the_default_mob
        && let ControlResponse::Host { facts, .. } = &mut response
    {
        facts.host_label = other.mob_id().to_string();
    }
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
    /// generation 0: its current runtime alias is `rt:worker:0`. Its bridge,
    /// returned with it, admits a delivery to that member on its session, as
    /// the identity door's delivery does.
    async fn identity_runtime_with_current_alias() -> Result<
        (Arc<IdentityRuntime>, Arc<CoreReadsBridge>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let bridge = Arc::new(CoreReadsBridge::with_deliveries());
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
        Ok((runtime, bridge))
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
        let (runtime, _) = identity_runtime_with_current_alias().await?;
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
    /// healthy member. An inject reaches a registered identity through its
    /// identity door, which delivers to the incarnation the alias names.
    #[tokio::test]
    async fn control_requests_reach_the_member_a_current_runtime_alias_names()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (identity_runtime, deliveries) = identity_runtime_with_current_alias().await?;
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

        let lookup = handle_lookup_member(
            &handle,
            None,
            "rt:worker:0",
            Some(&identity_runtime),
            &MembersApart::default(),
        )
        .await;
        let injected = handle_inject(
            &handle,
            "rt:worker:0",
            serde_json::json!("hello"),
            Some(&identity_runtime),
            &MembersApart::default(),
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
            &MembersApart::default(),
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
        assert_eq!(
            deliveries.delivered(),
            vec![crate::identity_first::AgentRuntimeId::parse("rt:worker:0")?],
            "the inject was delivered to the incarnation the current alias names"
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

    /// Each stored input image as its id, its state and the reason of its
    /// last transition, for a failure message.
    fn input_image_summary(images: &[serde_json::Value]) -> Vec<String> {
        images
            .iter()
            .map(|image| {
                let last_reason = image["history"]
                    .as_array()
                    .and_then(|history| history.last())
                    .map_or(serde_json::Value::Null, |entry| entry["reason"].clone());
                format!(
                    "{} {} ({last_reason})",
                    image["input_id"], image["current_state"]
                )
            })
            .collect()
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
        // The worker is an autonomous member: its spawn admits a kickoff
        // prompt and returns before that prompt's run ends, and the run then
        // moves the kickoff input through its states on its own. Settle it
        // before the baseline is read, so the stored inputs change below only
        // if the candidate's handler writes one.
        handle
            .wait_for_members_kickoff_complete(
                std::slice::from_ref(&worker),
                // A settle bound only: the assertions below check state, not elapsed time.
                Some(std::time::Duration::from_secs(30)),
            )
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
        let row_after = handle.get_member(&worker).await?;
        let inputs_after = member_input_images(&machine, &session_id).await?;
        assert_eq!(
            (
                refused,
                row_after == row_before,
                inputs_after == inputs_before,
            ),
            (
                vec![CANDIDATE_LAUNCH_CONTROL_CODE.to_string(); 3],
                true,
                true
            ),
            "a candidate refuses the peer's wire, unwire and inject, and the member's wiring \
             and stored inputs are unchanged\nstored inputs before: {:?}\nafter: {:?}",
            input_image_summary(&inputs_before),
            input_image_summary(&inputs_after),
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

    // -- the mob a request is for (the signed audience) and a grant's mobs --

    /// Stand-in for the owner of the host's mob set: the mobs the host has
    /// beside its default mob, and how often it was asked.
    #[derive(Default)]
    struct HostedMobs {
        others: std::sync::Mutex<std::collections::BTreeSet<String>>,
        asked: AtomicUsize,
    }

    impl HostedMobs {
        fn with(others: &[&str]) -> Self {
            let hosted = Self::default();
            for mob in others {
                hosted.create(mob);
            }
            hosted
        }

        fn create(&self, mob: &str) {
            self.others.lock().unwrap().insert(mob.to_string());
        }

        fn destroy(&self, mob: &str) {
            self.others.lock().unwrap().remove(mob);
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
    }

    impl ControlHostedMobs for HostedMobs {
        fn hosts_mob(&self, mob_id: &meerkat_mob::MobId) -> bool {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.others.lock().unwrap().contains(mob_id.as_str())
        }
    }

    const DEFAULT_MOB: &str = "mob-default";
    const SECOND_MOB: &str = "mob-second";

    fn authorizer_for_default_mob(
        keys: &crate::auth::peer_keys::GatewayPeerKeys,
        verbs: &[ControlVerb],
        mobs: Option<ControlMobScope>,
    ) -> ControlAuthorizer {
        let mut grant =
            ControlGrant::new("peer-mob", verbs.iter().copied(), ControlMemberScope::All);
        if let Some(mobs) = mobs {
            grant = grant.with_mobs(mobs);
        }
        let mut table = ControlGrantTable::new();
        table.insert(keys.pubkey_bytes(), grant);
        ControlAuthorizer::with_grants_for_audience(table, DEFAULT_MOB)
    }

    /// A grant written before grants could list mobs keeps meaning the
    /// default mob: it admits there, and a request for a second mob of the
    /// same host is refused as one for a mob the host does not have.
    #[test]
    fn grant_without_mobs_refuses_a_second_mob() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer = authorizer_for_default_mob(&keys, ControlVerb::all(), None);
        let hosted = HostedMobs::with(&[SECOND_MOB]);

        authorizer
            .authorize_on_host(
                &signed_as(&keys, Some(DEFAULT_MOB), inject_request("worker")),
                &hosted,
            )
            .expect("the default mob is what a grant without mobs reaches");
        let denial = authorizer
            .authorize_on_host(
                &signed_as(&keys, Some(SECOND_MOB), inject_request("worker")),
                &hosted,
            )
            .expect_err("a grant without mobs reaches no second mob");
        assert_eq!(
            denial,
            ControlAuthzDenial::AudienceMismatch {
                label: "peer-mob".to_string(),
                expected: DEFAULT_MOB.to_string(),
                presented: SECOND_MOB.to_string(),
            }
        );
    }

    fn one_request_of_each_verb(member: &str) -> Vec<ControlRequest> {
        let wire = |wire: bool| {
            let (remote_member, local_peer_spec_address, local_comms_name, local_peer_id) = (
                member.to_string(),
                "tcp://127.0.0.1:9001".to_string(),
                "far/role/alice".to_string(),
                "00000000-0000-4000-8000-000000000001".to_string(),
            );
            if wire {
                ControlRequest::Wire {
                    remote_member,
                    local_peer_spec_address,
                    local_comms_name,
                    local_peer_id,
                    local_pubkey_b64: None,
                    nonce: Some("n".to_string()),
                    caller: None,
                }
            } else {
                ControlRequest::Unwire {
                    remote_member,
                    local_peer_spec_address,
                    local_comms_name,
                    local_peer_id,
                    local_pubkey_b64: None,
                    nonce: Some("n".to_string()),
                    caller: None,
                }
            }
        };
        let requests = vec![
            wire(true),
            wire(false),
            inject_request(member),
            lookup_request(member),
            ControlRequest::HostDescribe {
                nonce: Some("n".to_string()),
                caller: None,
            },
            ControlRequest::HostHealth {
                nonce: Some("n".to_string()),
                caller: None,
            },
            ControlRequest::InjectKeyed {
                remote_member: member.to_string(),
                delivery_key: "batch-1".to_string(),
                content_digest: "digest".to_string(),
                summary: "1 announcement".to_string(),
                body: "a body".to_string(),
                issued_at_ms: 1,
                nonce: "n".to_string(),
                caller: None,
            },
            ControlRequest::InjectStatus {
                remote_member: member.to_string(),
                delivery_key: "batch-1".to_string(),
                with_input: false,
                issued_at_ms: 1,
                nonce: "n".to_string(),
                caller: None,
            },
        ];
        assert_eq!(
            requests.len(),
            ControlVerb::all().len(),
            "one request of each control verb"
        );
        requests
    }

    /// A mob the caller's grant does not reach is refused exactly as a mob
    /// the host does not have: the same refusal, value for value, whether
    /// the host has the mob or not, for every verb, the host verbs too. The
    /// host is not asked about such a mob.
    #[test]
    fn a_mob_the_grant_does_not_reach_is_refused_as_a_mob_the_host_does_not_have() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let has_the_mob = HostedMobs::with(&[SECOND_MOB]);
        let has_no_such_mob = HostedMobs::default();
        for mobs in [
            None,
            Some(ControlMobScope::mobs([DEFAULT_MOB])),
            Some(ControlMobScope::mobs(["mob-third"])),
            Some(ControlMobScope::mobs(Vec::<&str>::new())),
        ] {
            let authorizer = authorizer_for_default_mob(&keys, ControlVerb::all(), mobs.clone());
            for request in one_request_of_each_verb("worker") {
                let verb = request.verb();
                let request = signed_as(&keys, Some(SECOND_MOB), request);
                let where_hosted = authorizer
                    .authorize_on_host(&request, &has_the_mob)
                    .expect_err("the grant does not reach the mob");
                let where_absent = authorizer
                    .authorize_on_host(&request, &has_no_such_mob)
                    .expect_err("the host has no such mob");
                assert_eq!(
                    where_hosted, where_absent,
                    "{verb} under {mobs:?}: one refusal, whether the host has the mob or not"
                );
                assert_eq!(where_hosted.to_string(), where_absent.to_string());
                assert_eq!(where_hosted.code(), "audience_mismatch", "{verb}");
            }
        }
        assert_eq!(
            (has_the_mob.asked(), has_no_such_mob.asked()),
            (0, 0),
            "a mob the grant does not reach is no question for the host"
        );
    }

    /// The default mob is no exception: a grant that lists other mobs only
    /// does not reach it, and a request for it is refused as a request for
    /// an id no mob has. The refusal names the default mob only to a caller
    /// whose grant reaches the default mob, where it is the text every
    /// earlier grant has always got.
    #[test]
    fn a_refusal_names_the_default_mob_only_to_a_grant_that_reaches_it() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        let refusal = |mobs: Option<ControlMobScope>, audience: &str| {
            authorizer_for_default_mob(&keys, ControlVerb::all(), mobs)
                .authorize_on_host(
                    &signed_as(&keys, Some(audience), lookup_request("worker")),
                    &hosted,
                )
                .expect_err("no mob of the host in the grant's reach")
        };

        // A grant that reaches the default mob: today's refusal, to the byte.
        for mobs in [
            None,
            Some(ControlMobScope::All),
            Some(ControlMobScope::mobs([DEFAULT_MOB, SECOND_MOB])),
        ] {
            let denial = refusal(mobs.clone(), "mob-elsewhere");
            assert_eq!(denial.code(), "audience_mismatch");
            assert_eq!(
                denial.to_string(),
                "caller 'peer-mob' presented a request minted for 'mob-elsewhere', but this \
                 gateway answers for 'mob-default'",
                "under {mobs:?}"
            );
        }

        // A grant that lists another mob only does not reach the default
        // mob: nothing in a refusal names it, and its own id is refused as
        // any other id outside the grant.
        let second_only = Some(ControlMobScope::mobs([SECOND_MOB]));
        let for_the_default = refusal(second_only.clone(), DEFAULT_MOB);
        let for_no_mob = refusal(second_only, "mob-elsewhere");
        for denial in [&for_the_default, &for_no_mob] {
            assert_eq!(denial.code(), "audience_mismatch");
            assert!(
                matches!(denial, ControlAuthzDenial::AudienceMismatch { expected, .. }
                    if expected.is_empty()),
                "the refusal carries no mob the caller did not name: {denial:?}"
            );
        }
        assert_eq!(
            for_no_mob.to_string(),
            "caller 'peer-mob' presented a request minted for 'mob-elsewhere', which this \
             gateway does not answer for"
        );
        assert!(
            !for_no_mob.to_string().contains(DEFAULT_MOB),
            "the default mob is not named to a grant that does not reach it"
        );
        assert_eq!(
            for_the_default.to_string().replace(DEFAULT_MOB, "<id>"),
            for_no_mob.to_string().replace("mob-elsewhere", "<id>"),
            "the default mob's id is refused as an id no mob has"
        );
        assert_eq!(
            hosted.asked(),
            1,
            "only the grant for every mob reaches an id the host has to be asked about"
        );
    }

    /// A grant that lists the second mob, or every mob, admits a request
    /// whose audience names it.
    #[test]
    fn grant_with_mobs_admits_it() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        for mobs in [
            ControlMobScope::mobs([SECOND_MOB]),
            ControlMobScope::mobs([DEFAULT_MOB, SECOND_MOB]),
            ControlMobScope::All,
        ] {
            let authorizer =
                authorizer_for_default_mob(&keys, ControlVerb::all(), Some(mobs.clone()));
            let admitted = authorizer
                .authorize_on_host(
                    &signed_as(&keys, Some(SECOND_MOB), inject_request("worker")),
                    &hosted,
                )
                .unwrap_or_else(|denial| panic!("{mobs:?} must admit the second mob: {denial}"))
                .expect("enforcing mode names the caller");
            assert_eq!(admitted.label, "peer-mob");
        }
    }

    /// The audience check asks whether the id is a mob of the host: an id
    /// the host does not have is the existing `audience_mismatch`, whatever
    /// the grant lists, and a mob the host has is not.
    #[test]
    fn audience_of_an_unknown_mob_is_audience_mismatch() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer =
            authorizer_for_default_mob(&keys, ControlVerb::all(), Some(ControlMobScope::All));
        let hosted = HostedMobs::with(&[SECOND_MOB]);

        // Positive control: the audience check passes for a mob the host has.
        authorizer
            .authorize_on_host(
                &signed_as(&keys, Some(SECOND_MOB), lookup_request("worker")),
                &hosted,
            )
            .expect("a second mob of the host is an audience of the host");

        let denial = authorizer
            .authorize_on_host(
                &signed_as(&keys, Some("mob-elsewhere"), lookup_request("worker")),
                &hosted,
            )
            .expect_err("a mob the host does not have is not its audience");
        assert_eq!(
            denial,
            ControlAuthzDenial::AudienceMismatch {
                label: "peer-mob".to_string(),
                expected: DEFAULT_MOB.to_string(),
                presented: "mob-elsewhere".to_string(),
            }
        );
        let missing = authorizer
            .authorize_on_host(&signed_as(&keys, None, lookup_request("worker")), &hosted)
            .expect_err("a request without an audience names no mob of the host");
        assert_eq!(missing.code(), "audience_mismatch");
        let empty = authorizer
            .authorize_on_host(
                &signed_as(&keys, Some(""), lookup_request("worker")),
                &hosted,
            )
            .expect_err("an empty audience names no mob of the host");
        assert_eq!(empty.code(), "audience_mismatch");
        assert_eq!(
            hosted.asked(),
            2,
            "only a non-empty id other than the default mob's is a question for the host"
        );
    }

    /// The audience names a mob by its exact id. An id that extends the
    /// default mob's, is a prefix of it, or differs from it by case is
    /// another id: not the default mob, and no mob of a host that has no
    /// such mob.
    #[test]
    fn audience_is_matched_exactly() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer =
            authorizer_for_default_mob(&keys, ControlVerb::all(), Some(ControlMobScope::All));
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        for audience in [
            "mob-default-2",
            "mob-defaul",
            "MOB-DEFAULT",
            "Mob-Default",
            " mob-default",
            "mob-default ",
        ] {
            let request = signed_as(&keys, Some(audience), inject_request("worker"));
            for denial in [
                authorizer.authorize(&request),
                authorizer.authorize_on_host(&request, &hosted),
            ] {
                assert_eq!(
                    denial
                        .expect_err("another id than the default mob's")
                        .code(),
                    "audience_mismatch",
                    "audience {audience:?}"
                );
            }
        }
        // Positive control: the exact id is the default mob.
        authorizer
            .authorize(&signed_as(
                &keys,
                Some(DEFAULT_MOB),
                inject_request("worker"),
            ))
            .expect("the default mob by its exact id");
    }

    /// A grant's mobs are matched by their exact id too: a listed id that
    /// is a prefix of the request's mob, extends it, or differs by case
    /// does not reach it, and a mob the grant does not reach is refused as
    /// a mob the host does not have.
    #[test]
    fn grant_mobs_are_matched_exactly() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        let near_misses = authorizer_for_default_mob(
            &keys,
            ControlVerb::all(),
            Some(ControlMobScope::mobs([
                "mob-sec",
                "mob-second-2",
                "MOB-SECOND",
                "mob-def",
                "mob-default-2",
                "MOB-DEFAULT",
            ])),
        );
        for audience in [DEFAULT_MOB, SECOND_MOB] {
            let denial = near_misses
                .authorize_on_host(
                    &signed_as(&keys, Some(audience), inject_request("worker")),
                    &hosted,
                )
                .expect_err("no listed id is this mob's id");
            assert_eq!(denial.code(), "audience_mismatch", "mob {audience}");
        }
        assert_eq!(
            hosted.asked(),
            0,
            "no listed id is the request's mob, so the host is not asked"
        );
        // Positive control: the exact ids reach both mobs.
        let exact = authorizer_for_default_mob(
            &keys,
            ControlVerb::all(),
            Some(ControlMobScope::mobs([DEFAULT_MOB, SECOND_MOB])),
        );
        for audience in [DEFAULT_MOB, SECOND_MOB] {
            exact
                .authorize_on_host(
                    &signed_as(&keys, Some(audience), inject_request("worker")),
                    &hosted,
                )
                .expect("the listed id");
        }
    }

    /// The host verbs name no mob, and still need an audience this host
    /// answers for: a host verb minted for another gateway is not served
    /// here.
    #[test]
    fn host_verbs_need_an_audience_of_the_host() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        let authorizer = authorizer_for_default_mob(
            &keys,
            &[ControlVerb::HostDescribe, ControlVerb::HostHealth],
            Some(ControlMobScope::All),
        );
        let describe = |audience: Option<&str>| {
            signed_as(
                &keys,
                audience,
                ControlRequest::HostDescribe {
                    nonce: Some("n".to_string()),
                    caller: None,
                },
            )
        };
        for audience in [Some("mob-elsewhere"), Some(""), None] {
            for denial in [
                authorizer.authorize(&describe(audience)),
                authorizer.authorize_on_host(&describe(audience), &hosted),
            ] {
                assert_eq!(
                    denial.expect_err("not an audience of this host").code(),
                    "audience_mismatch",
                    "audience {audience:?}"
                );
            }
        }
        // Positive control: the same verb for an audience of the host.
        authorizer
            .authorize(&describe(Some(DEFAULT_MOB)))
            .expect("the default mob is an audience of the host");
    }

    /// The authorizer keeps no list of the host's mobs: it asks per
    /// request, so a mob created after the authorizer was built is an
    /// audience and a destroyed one no longer is.
    #[test]
    fn audience_check_asks_the_host_on_every_request() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer =
            authorizer_for_default_mob(&keys, ControlVerb::all(), Some(ControlMobScope::All));
        let hosted = HostedMobs::default();
        let request = signed_as(&keys, Some(SECOND_MOB), lookup_request("worker"));

        let before = authorizer
            .authorize_on_host(&request, &hosted)
            .expect_err("the host does not have the mob yet");
        assert_eq!(before.code(), "audience_mismatch");

        hosted.create(SECOND_MOB);
        authorizer
            .authorize_on_host(&request, &hosted)
            .expect("a mob created after the authorizer was built is an audience");

        hosted.destroy(SECOND_MOB);
        let after = authorizer
            .authorize_on_host(&request, &hosted)
            .expect_err("a destroyed mob is no audience");
        assert_eq!(after.code(), "audience_mismatch");
        assert_eq!(hosted.asked(), 3, "one question per request");
    }

    /// Only a caller that holds a grant, with a verified signature, makes
    /// the authorizer ask about a mob, and never about the default mob.
    #[test]
    fn caller_without_a_grant_never_reaches_the_mob_lookup() {
        let granted = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let stranger = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let authorizer =
            authorizer_for_default_mob(&granted, ControlVerb::all(), Some(ControlMobScope::All));
        let hosted = HostedMobs::with(&[SECOND_MOB]);

        let unknown = authorizer
            .authorize_on_host(
                &signed_as(&stranger, Some(SECOND_MOB), lookup_request("worker")),
                &hosted,
            )
            .expect_err("a stranger holds no grant");
        assert_eq!(unknown.code(), "caller_not_granted");

        let unsigned = authorizer
            .authorize_on_host(&lookup_request("worker"), &hosted)
            .expect_err("no credential");
        assert_eq!(unsigned.code(), "unauthenticated_caller");

        let mut moved = signed_as(&granted, Some(SECOND_MOB), lookup_request("worker"));
        let caller = moved.caller().cloned().expect("signed");
        moved = lookup_request("someone-else");
        moved.set_caller(Some(caller));
        let forged = authorizer
            .authorize_on_host(&moved, &hosted)
            .expect_err("a moved signature does not verify");
        assert_eq!(forged.code(), "invalid_caller_signature");

        authorizer
            .authorize_on_host(
                &signed_as(&granted, Some(DEFAULT_MOB), lookup_request("worker")),
                &hosted,
            )
            .expect("the default mob is admitted");
        assert_eq!(
            hosted.asked(),
            0,
            "none of these requests is a reason to ask about a mob"
        );
    }

    /// A grant that lists mobs names the default mob by its id like any
    /// other: listing only the second mob does not reach the default mob,
    /// and a mob outside the list is refused as a mob the host does not
    /// have.
    #[test]
    fn grant_that_lists_mobs_reaches_the_default_mob_only_by_its_id() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        let for_default = signed_as(&keys, Some(DEFAULT_MOB), inject_request("worker"));

        let second_only = authorizer_for_default_mob(
            &keys,
            ControlVerb::all(),
            Some(ControlMobScope::mobs([SECOND_MOB])),
        );
        let denial = second_only
            .authorize_on_host(&for_default, &hosted)
            .expect_err("the default mob is not in the list");
        assert_eq!(denial.code(), "audience_mismatch");
        // The public entry point, on a host with no second mob, agrees.
        assert_eq!(
            second_only
                .authorize(&for_default)
                .expect_err("the default mob is not in the list")
                .code(),
            "audience_mismatch"
        );

        let by_id = authorizer_for_default_mob(
            &keys,
            ControlVerb::all(),
            Some(ControlMobScope::mobs([DEFAULT_MOB])),
        );
        by_id
            .authorize_on_host(&for_default, &hosted)
            .expect("the default mob named by its id");
        let other = by_id
            .authorize_on_host(
                &signed_as(&keys, Some(SECOND_MOB), inject_request("worker")),
                &hosted,
            )
            .expect_err("the second mob is not in the list");
        assert_eq!(other.code(), "audience_mismatch");

        let none = authorizer_for_default_mob(
            &keys,
            ControlVerb::all(),
            Some(ControlMobScope::mobs(Vec::<&str>::new())),
        );
        assert_eq!(
            none.authorize_on_host(&for_default, &hosted)
                .expect_err("an empty list reaches no mob")
                .code(),
            "audience_mismatch"
        );
        assert_eq!(
            hosted.asked(),
            0,
            "a mob outside the list is no question for the host"
        );
    }

    /// An authorizer bound to an empty audience (a launch whose mob has no
    /// id) admits no granted request: an empty audience names no mob, so a
    /// request signed for it is refused as one signed for an id that is no
    /// mob of the host, under a grant that reaches every mob.
    #[test]
    fn an_authorizer_bound_to_an_empty_audience_admits_no_granted_request() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let mut table = ControlGrantTable::new();
        table.insert(
            keys.pubkey_bytes(),
            ControlGrant::new(
                "peer-mob",
                ControlVerb::all().iter().copied(),
                ControlMemberScope::All,
            )
            .with_mobs(ControlMobScope::All),
        );
        let authorizer = ControlAuthorizer::with_grants_for_audience(table, "");
        let hosted = HostedMobs::with(&[]);
        for audience in ["", DEFAULT_MOB] {
            let denial = authorizer
                .authorize_on_host(
                    &signed_as(&keys, Some(audience), lookup_request("alice")),
                    &hosted,
                )
                .expect_err("no audience is a mob of a host whose default mob has no id");
            assert_eq!(denial.code(), "audience_mismatch", "audience '{audience}'");
        }
    }

    /// The host verbs name no member, and they still need an audience the
    /// grant reaches: a grant that names the verb admits it for a mob in
    /// its reach, whatever members it lists, and refuses it for a mob
    /// outside as for a mob the host does not have. The audience comes
    /// first, then the verb.
    #[test]
    fn host_verbs_need_an_audience_the_grant_reaches() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        let describe = |audience: &str| {
            signed_as(
                &keys,
                Some(audience),
                ControlRequest::HostDescribe {
                    nonce: Some("n".to_string()),
                    caller: None,
                },
            )
        };
        let health = |audience: &str| {
            signed_as(
                &keys,
                Some(audience),
                ControlRequest::HostHealth {
                    nonce: Some("n".to_string()),
                    caller: None,
                },
            )
        };

        let second_only = authorizer_for_default_mob(
            &keys,
            &[ControlVerb::HostDescribe],
            Some(ControlMobScope::mobs([SECOND_MOB])),
        );
        second_only
            .authorize_on_host(&describe(SECOND_MOB), &hosted)
            .expect("a mob in the grant's reach is an audience for a host verb");
        assert_eq!(
            second_only
                .authorize_on_host(&describe(DEFAULT_MOB), &hosted)
                .expect_err("the default mob is outside the grant")
                .code(),
            "audience_mismatch"
        );
        assert_eq!(
            second_only
                .authorize_on_host(&health(SECOND_MOB), &hosted)
                .expect_err("host_health is not named")
                .code(),
            "verb_not_granted"
        );
        assert_eq!(
            second_only
                .authorize_on_host(&health(DEFAULT_MOB), &hosted)
                .expect_err("outside the grant, whatever the verb")
                .code(),
            "audience_mismatch",
            "a verb the grant lacks is not told for a mob outside the grant"
        );

        // A grant without mobs: the default mob is its one audience.
        let default_only = authorizer_for_default_mob(&keys, &[ControlVerb::HostDescribe], None);
        default_only
            .authorize_on_host(&describe(DEFAULT_MOB), &hosted)
            .expect("the default mob");
        assert_eq!(
            default_only
                .authorize_on_host(&describe(SECOND_MOB), &hosted)
                .expect_err("a grant without mobs reaches no second mob")
                .code(),
            "audience_mismatch"
        );

        // The members of a grant never scope a host verb.
        let mut table = ControlGrantTable::new();
        table.insert(
            keys.pubkey_bytes(),
            ControlGrant::new(
                "peer-mob",
                [ControlVerb::HostDescribe],
                ControlMemberScope::members(["alice"]),
            )
            .with_mobs(ControlMobScope::All),
        );
        ControlAuthorizer::with_grants_for_audience(table, DEFAULT_MOB)
            .authorize_on_host(&describe(SECOND_MOB), &hosted)
            .expect("the host plane is not scoped by member");
    }

    /// The keyed inject verbs name a member of one mob: a grant without
    /// mobs serves them for the default mob and refuses them for a second
    /// mob as for a mob the host does not have.
    #[test]
    fn keyed_inject_verbs_are_scoped_by_mob() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        let status = |audience: &str| {
            signed_as(
                &keys,
                Some(audience),
                ControlRequest::InjectStatus {
                    remote_member: "worker".into(),
                    delivery_key: "batch-1".into(),
                    with_input: false,
                    issued_at_ms: 1,
                    nonce: "n".into(),
                    caller: None,
                },
            )
        };
        let default_only = authorizer_for_default_mob(&keys, &[ControlVerb::InjectStatus], None);
        default_only
            .authorize_on_host(&status(DEFAULT_MOB), &hosted)
            .expect("the default mob");
        assert_eq!(
            default_only
                .authorize_on_host(&status(SECOND_MOB), &hosted)
                .expect_err("a grant without mobs reaches no second mob")
                .code(),
            "audience_mismatch"
        );
        let both = authorizer_for_default_mob(
            &keys,
            &[ControlVerb::InjectStatus],
            Some(ControlMobScope::All),
        );
        both.authorize_on_host(&status(SECOND_MOB), &hosted)
            .expect("every mob");
        // A mob in reach, a member outside the grant: the member refusal.
        let mut table = ControlGrantTable::new();
        table.insert(
            keys.pubkey_bytes(),
            ControlGrant::new(
                "peer-mob",
                [ControlVerb::InjectStatus],
                ControlMemberScope::members(["alice"]),
            )
            .with_mobs(ControlMobScope::All),
        );
        assert_eq!(
            ControlAuthorizer::with_grants_for_audience(table, DEFAULT_MOB)
                .authorize_on_host(&status(SECOND_MOB), &hosted)
                .expect_err("worker is outside the grant's members")
                .code(),
            "member_not_granted",
            "the member refusal says: the mob is in reach, the member is not"
        );
    }

    /// An authorizer that was told no audience knows no mob id: every
    /// request is its one mob's. A grant without mobs and a grant for every
    /// mob admit as before; a grant that lists mob ids matches nothing, and
    /// the unchecked audience of the request never selects a mob.
    #[test]
    fn authorizer_without_an_audience_matches_no_listed_mob() {
        let keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let hosted = HostedMobs::with(&[SECOND_MOB]);
        let authorizer_with = |mobs: Option<ControlMobScope>| {
            let mut grant = ControlGrant::new(
                "peer-mob",
                ControlVerb::all().iter().copied(),
                ControlMemberScope::All,
            );
            if let Some(mobs) = mobs {
                grant = grant.with_mobs(mobs);
            }
            let mut table = ControlGrantTable::new();
            table.insert(keys.pubkey_bytes(), grant);
            ControlAuthorizer::with_grants(table)
        };
        for audience in [None, Some(DEFAULT_MOB), Some(SECOND_MOB)] {
            let request = signed_as(&keys, audience, inject_request("worker"));
            authorizer_with(None)
                .authorize_on_host(&request, &hosted)
                .expect("a grant without mobs is the listener's one mob's");
            authorizer_with(Some(ControlMobScope::All))
                .authorize_on_host(&request, &hosted)
                .expect("every mob");
            let denial = authorizer_with(Some(ControlMobScope::mobs([DEFAULT_MOB, SECOND_MOB])))
                .authorize_on_host(&request, &hosted)
                .expect_err("no mob id is known, so no listed id matches");
            assert_eq!(denial.code(), "member_not_granted");
        }
        assert_eq!(
            hosted.asked(),
            0,
            "an unchecked audience is never resolved to a mob"
        );
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

    /// A `[control_grants]` entry written before grants could list mobs
    /// parses to the table it always parsed to: a grant of the default mob.
    #[test]
    fn grant_toml_without_mobs_parses_to_the_same_table() {
        let parsed = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["inject", "lookup_member"]
            members = ["bob"]
            "#
        ))
        .expect("parse")
        .expect("section is present");
        let mut built = ControlGrantTable::new();
        built.insert(
            [42u8; 32],
            ControlGrant::new(
                "ops-mob",
                [ControlVerb::Inject, ControlVerb::LookupMember],
                ControlMemberScope::members(["bob"]),
            ),
        );
        assert_eq!(parsed, built);
        assert_eq!(
            parsed.get(&[42u8; 32]).expect("filed").mobs(),
            &ControlMobScope::DefaultMob
        );
    }

    /// `mobs` lists the mobs of the host a grant reaches, `*` every mob.
    #[test]
    fn grant_toml_parses_the_mobs_scope() {
        let grant_with = |mobs: &str| {
            ControlGrantTable::from_toml(&format!(
                r#"
                [control_grants.ops-mob]
                pubkey = "{TEST_PUBKEY_B64}"
                verbs = ["inject"]
                mobs = {mobs}
                "#
            ))
            .unwrap_or_else(|error| panic!("mobs = {mobs} must parse: {error}"))
            .expect("section is present")
            .get(&[42u8; 32])
            .expect("filed")
            .clone()
        };
        assert_eq!(
            grant_with(r#"["mob-second", "mob-default"]"#).mobs(),
            &ControlMobScope::Mobs(
                [
                    meerkat_mob::MobId::from("mob-default"),
                    meerkat_mob::MobId::from("mob-second"),
                ]
                .into_iter()
                .collect()
            )
        );
        assert_eq!(grant_with(r#"["*"]"#).mobs(), &ControlMobScope::All);
        assert_eq!(
            grant_with(r#"["mob-second", "*"]"#).mobs(),
            &ControlMobScope::All
        );
        // The member scope is untouched by the mob scope.
        assert_eq!(
            grant_with(r#"["mob-second"]"#).members(),
            &ControlMemberScope::All
        );
    }

    /// A `mobs` an operator wrote and that names nothing is a configuration
    /// error, never the default mob and never every mob.
    #[test]
    fn grant_toml_refuses_an_empty_or_malformed_mobs_list() {
        let detail_of = |mobs: &str| {
            let error = ControlGrantTable::from_toml(&format!(
                r#"
                [control_grants.ops-mob]
                pubkey = "{TEST_PUBKEY_B64}"
                verbs = ["inject"]
                mobs = {mobs}
                "#
            ))
            .expect_err("must fail closed");
            match error {
                ControlGrantConfigError::InvalidField { label, detail } => {
                    assert_eq!(label, "ops-mob");
                    detail
                }
                other => panic!("expected an invalid field, got {other:?}"),
            }
        };
        assert!(
            detail_of("[]").contains("'mobs' is empty"),
            "an empty list is refused as empty"
        );
        assert!(
            detail_of(r#""mob-second""#).contains("'mobs' must be an array"),
            "a bare string is not a list"
        );
        assert!(
            detail_of("[1]").contains("'mobs' entries must be strings"),
            "entries are mob ids"
        );
        assert!(
            detail_of(r#"[""]"#).contains("'mobs' entries must not be empty"),
            "an empty id names no mob"
        );
        // An id with whitespace around it would load and match no mob.
        for padded in [
            r#"[" "]"#,
            r#"["mob-second "]"#,
            r#"[" mob-second"]"#,
            r#"["mob-second\n"]"#,
            r#"["mob-default", "\tmob-second"]"#,
        ] {
            assert!(
                detail_of(padded).contains("whitespace"),
                "mobs = {padded} is refused for its whitespace"
            );
        }

        // The singular spelling is an unknown key, as for `member`.
        let typo = ControlGrantTable::from_toml(&format!(
            r#"
            [control_grants.ops-mob]
            pubkey = "{TEST_PUBKEY_B64}"
            verbs = ["inject"]
            mob = ["mob-second"]
            "#
        ))
        .expect_err("a singular 'mob' typo must not silently grant the default mob");
        assert!(matches!(typo, ControlGrantConfigError::InvalidField { .. }));
    }

    /// `ControlAuthorizer::from_toml` builds an authorizer that was told no
    /// audience, where a listed mob id can match nothing. It refuses such a
    /// grant when the configuration is read, instead of loading a grant that
    /// reaches no one; a grant without mobs or for every mob loads.
    #[test]
    fn authorizer_from_toml_refuses_a_grant_that_lists_mob_ids() {
        let section = |mobs: &str| {
            format!(
                r#"
                [control_grants.ops-mob]
                pubkey = "{TEST_PUBKEY_B64}"
                verbs = ["inject"]
                {mobs}
                "#
            )
        };
        let error = ControlAuthorizer::from_toml(&section(r#"mobs = ["mob-second"]"#))
            .expect_err("a listed mob id cannot be matched without an audience");
        match error {
            ControlGrantConfigError::InvalidField { label, detail } => {
                assert_eq!(label, "ops-mob");
                assert!(
                    detail.contains("with_grants_for_audience"),
                    "names the way out: {detail}"
                );
            }
            other => panic!("expected an invalid field, got {other:?}"),
        }
        for loads in ["", r#"mobs = ["*"]"#, r#"mobs = ["mob-second", "*"]"#] {
            let authorizer = ControlAuthorizer::from_toml(&section(loads))
                .unwrap_or_else(|error| panic!("{loads:?} must load: {error}"));
            assert!(authorizer.is_enforcing());
        }
        // The table itself still parses: a host that binds an audience
        // reads it through `ControlGrantTable::from_toml`.
        ControlGrantTable::from_toml(&section(r#"mobs = ["mob-second"]"#))
            .expect("the table parses")
            .expect("section is present");
    }

    // -- the gateway's own key --

    /// Counts what reaches the handler.
    struct CountingEcho {
        dispatched: Arc<AtomicUsize>,
    }

    impl ControlHandler for CountingEcho {
        fn handle(
            &self,
            request: ControlRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>>
        {
            static ECHO: EchoHandler = EchoHandler;
            self.dispatched.fetch_add(1, Ordering::SeqCst);
            ECHO.handle(request)
        }
    }

    /// Serve `authorizer` over TCP for a gateway whose key is `own_keys`,
    /// counting what reaches the handler.
    async fn serve_counted(
        authorizer: ControlAuthorizer,
        own_keys: &crate::auth::peer_keys::GatewayPeerKeys,
    ) -> (
        RemoteEndpoint,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let dispatched = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let endpoint = RemoteEndpoint::Tcp(listener.local_addr().expect("addr").to_string());
        let handler: Arc<dyn ControlHandler> = Arc::new(CountingEcho {
            dispatched: Arc::clone(&dispatched),
        });
        let server = tokio::spawn(serve_tcp_control_with_authorizer(
            listener,
            handler,
            signer_with(own_keys.clone()),
            Arc::new(authorizer),
        ));
        (endpoint, dispatched, server)
    }

    /// One control exchange, as the listener answers it.
    async fn exchange(endpoint: &RemoteEndpoint, request: &ControlRequest) -> ControlResponse {
        RemoteControlClient::send(endpoint, request, DEFAULT_CONTROL_TIMEOUT)
            .await
            .expect("control rpc")
    }

    fn refusal_of(response: &ControlResponse) -> Option<(&str, &str)> {
        match response {
            ControlResponse::Err { code, message } => Some((code.as_str(), message.as_str())),
            _ => None,
        }
    }

    /// `request` with its caller key respelled and its signature untouched.
    /// The signed text does not cover the spelling of the key, so the
    /// signature still verifies.
    fn respelled(mut request: ControlRequest, spell: impl Fn(&str) -> String) -> ControlRequest {
        let caller = request.caller().cloned().expect("signed");
        request.set_caller(Some(ControlCaller {
            pubkey_b64: spell(&caller.pubkey_b64),
            ..caller
        }));
        request
    }

    /// `request` signed by `signer` and naming `claimed` as its caller: a
    /// claim whose signature does not verify.
    fn claiming(
        claimed: &crate::auth::peer_keys::GatewayPeerKeys,
        signer: &crate::auth::peer_keys::GatewayPeerKeys,
        request: ControlRequest,
    ) -> ControlRequest {
        respelled(signed_as(signer, Some(DEFAULT_MOB), request), |_| {
            claimed.pubkey_b64()
        })
    }

    fn full_grant() -> ControlGrant {
        ControlGrant::new(
            "everything",
            ControlVerb::all().iter().copied(),
            ControlMemberScope::All,
        )
        .with_mobs(ControlMobScope::All)
    }

    fn status_request(member: &str) -> ControlRequest {
        ControlRequest::InjectStatus {
            remote_member: member.to_string(),
            delivery_key: "batch-1".to_string(),
            with_input: false,
            issued_at_ms: 1,
            nonce: "n".to_string(),
            caller: None,
        }
    }

    fn describe_request() -> ControlRequest {
        ControlRequest::HostDescribe {
            nonce: Some("n".to_string()),
            caller: None,
        }
    }

    /// A request made with this gateway's own key is refused before the
    /// grant table is read, whatever the table says about that key: no
    /// grant, a grant without the verb, a grant for everything. Every verb
    /// is refused, the read-only ones too, the handler never runs, and
    /// another gateway is served by the same listener.
    #[tokio::test]
    async fn request_from_the_gateways_own_key_is_refused_before_the_grant_table() {
        let own_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let peer_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let own_grants = [
            None,
            Some(ControlGrant::new(
                "health-only",
                [ControlVerb::HostHealth],
                ControlMemberScope::All,
            )),
            Some(full_grant()),
        ];
        for own_grant in own_grants {
            let filed = own_grant
                .as_ref()
                .map_or("no grant", ControlGrant::label)
                .to_string();
            let mut table = ControlGrantTable::new();
            if let Some(grant) = own_grant {
                table.insert(own_keys.pubkey_bytes(), grant);
            }
            table.insert(peer_keys.pubkey_bytes(), full_grant());
            let (endpoint, dispatched, server) = serve_counted(
                ControlAuthorizer::with_grants_for_audience(table, DEFAULT_MOB),
                &own_keys,
            )
            .await;

            for request in [
                lookup_request("alice"),
                inject_request("alice"),
                describe_request(),
                status_request("alice"),
            ] {
                let verb = request.verb();
                let answer =
                    exchange(&endpoint, &signed_as(&own_keys, Some(DEFAULT_MOB), request)).await;
                assert_eq!(
                    refusal_of(&answer).map(|(code, _)| code),
                    Some(HOST_OWN_KEY_CONTROL_CODE),
                    "own key filed with {filed}, verb {verb}: {answer:?}"
                );
            }
            assert_eq!(
                dispatched.load(Ordering::SeqCst),
                0,
                "a refused request never reaches the handler"
            );

            // Positive control: another gateway with a grant is served.
            let served = exchange(
                &endpoint,
                &signed_as(&peer_keys, Some(DEFAULT_MOB), lookup_request("alice")),
            )
            .await;
            assert!(
                matches!(served, ControlResponse::Member { .. }),
                "got {served:?}"
            );
            assert_eq!(dispatched.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }

    /// A listener that enforces no grants refuses a request made with this
    /// gateway's own key too, ahead of every other answer it has; every
    /// other caller is served as before.
    #[tokio::test]
    async fn open_listener_refuses_the_gateways_own_key() {
        let own_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let peer_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let (endpoint, dispatched, server) =
            serve_counted(ControlAuthorizer::open(), &own_keys).await;

        for request in [lookup_request("alice"), status_request("alice")] {
            let verb = request.verb();
            let answer = exchange(&endpoint, &signed_as(&own_keys, None, request)).await;
            assert_eq!(
                refusal_of(&answer).map(|(code, _)| code),
                Some(HOST_OWN_KEY_CONTROL_CODE),
                "verb {verb}: {answer:?}"
            );
        }
        assert_eq!(dispatched.load(Ordering::SeqCst), 0);

        // Another gateway's keyed verb gets the open listener's own answer.
        let keyed = exchange(
            &endpoint,
            &signed_as(&peer_keys, None, status_request("alice")),
        )
        .await;
        assert_eq!(
            refusal_of(&keyed).map(|(code, _)| code),
            Some("grants_required")
        );
        let peer = exchange(
            &endpoint,
            &signed_as(&peer_keys, None, lookup_request("alice")),
        )
        .await;
        assert!(
            matches!(peer, ControlResponse::Member { .. }),
            "got {peer:?}"
        );
        let anonymous = exchange(&endpoint, &lookup_request("alice")).await;
        assert!(
            matches!(anonymous, ControlResponse::Member { .. }),
            "got {anonymous:?}"
        );
        assert_eq!(dispatched.load(Ordering::SeqCst), 2);
        server.abort();
    }

    /// A caller without a signature that verifies cannot tell whether the
    /// key it names is this gateway's: it gets the answer any such claim
    /// gets, on a listener with grants and on one without, and the
    /// listener's own-key refusal (and its warning) is not reached.
    #[tokio::test]
    async fn unverified_claim_of_the_gateways_key_is_answered_like_any_other() {
        let own_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let stranger_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let attacker_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let forged_own = claiming(&own_keys, &attacker_keys, lookup_request("alice"));
        let forged_stranger = claiming(&stranger_keys, &attacker_keys, lookup_request("alice"));
        let unsigned = |keys: &crate::auth::peer_keys::GatewayPeerKeys| {
            let mut request = lookup_request("alice");
            request.set_caller(Some(ControlCaller {
                pubkey_b64: keys.pubkey_b64(),
                sig_b64: String::new(),
                audience: Some(DEFAULT_MOB.to_string()),
            }));
            request
        };
        // What an answer says once the key the caller itself sent is taken
        // out of it.
        let shape = |response: &ControlResponse, keys: &crate::auth::peer_keys::GatewayPeerKeys| {
            let (code, message) = refusal_of(response).expect("a refusal");
            (
                code.to_string(),
                message.replace(&keys.pubkey_b64(), "<key>"),
            )
        };

        // A listener with grants, where the gateway's own key even holds one.
        let mut table = ControlGrantTable::new();
        table.insert(own_keys.pubkey_bytes(), full_grant());
        let (endpoint, dispatched, server) = serve_counted(
            ControlAuthorizer::with_grants_for_audience(table, DEFAULT_MOB),
            &own_keys,
        )
        .await;
        let own_answer = exchange(&endpoint, &forged_own).await;
        let stranger_answer = exchange(&endpoint, &forged_stranger).await;
        assert_eq!(
            shape(&own_answer, &own_keys),
            shape(&stranger_answer, &stranger_keys)
        );
        assert_eq!(
            refusal_of(&own_answer).map(|(code, _)| code),
            Some("invalid_caller_signature")
        );
        let own_unsigned = exchange(&endpoint, &unsigned(&own_keys)).await;
        let stranger_unsigned = exchange(&endpoint, &unsigned(&stranger_keys)).await;
        assert_eq!(
            shape(&own_unsigned, &own_keys),
            shape(&stranger_unsigned, &stranger_keys)
        );
        assert_eq!(
            refusal_of(&own_unsigned).map(|(code, _)| code),
            Some("invalid_caller_credential")
        );
        assert_eq!(dispatched.load(Ordering::SeqCst), 0);
        server.abort();

        // A listener without grants verifies no signature for anyone: both
        // claims are served.
        let (endpoint, dispatched, server) =
            serve_counted(ControlAuthorizer::open(), &own_keys).await;
        for request in [&forged_own, &forged_stranger, &unsigned(&own_keys)] {
            let answer = exchange(&endpoint, request).await;
            assert!(
                matches!(answer, ControlResponse::Member { .. }),
                "got {answer:?}"
            );
        }
        assert_eq!(dispatched.load(Ordering::SeqCst), 3);
        server.abort();

        // The function the listener calls, whose answer is the only thing
        // that writes the own-key warning.
        assert!(host_own_key_refusal(&forged_own, Some(&own_keys)).is_none());
        assert!(host_own_key_refusal(&unsigned(&own_keys), Some(&own_keys)).is_none());
    }

    /// The refusal is not a grant refusal: no grant admits the gateway's
    /// own key, so a caller must never read it as "ask for a wider grant".
    /// The eight denial codes stay eight: the array's length is in its type.
    #[test]
    fn host_own_key_is_not_an_authorization_denial() {
        assert!(!ControlAuthzDenial::is_denial_code(
            HOST_OWN_KEY_CONTROL_CODE
        ));
        let eight: [&str; 8] = CONTROL_AUTHZ_DENIAL_CODES;
        assert!(!eight.contains(&HOST_OWN_KEY_CONTROL_CODE));
        // POSITIVE CONTROL: the classifier does recognise a real denial.
        assert!(ControlAuthzDenial::is_denial_code("member_not_granted"));
    }

    /// The exact function the listener calls. It refuses a request whose
    /// signature verifies against this gateway's key, however the key is
    /// spelled in the envelope and whatever the verb, and nothing else. A
    /// gateway without a key has no caller to recognise as itself.
    #[test]
    fn own_key_refusal_needs_the_gateways_key_and_a_signature_that_verifies() {
        let own_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let other_keys = crate::auth::peer_keys::GatewayPeerKeys::ephemeral();
        let own = signed_as(&own_keys, Some(DEFAULT_MOB), lookup_request("alice"));
        let other = signed_as(&other_keys, Some(DEFAULT_MOB), lookup_request("alice"));

        assert!(host_own_key_refusal(&own, None).is_none());
        assert!(host_own_key_refusal(&other, Some(&own_keys)).is_none());
        assert!(host_own_key_refusal(&lookup_request("alice"), Some(&own_keys)).is_none());

        for request in [
            lookup_request("alice"),
            inject_request("alice"),
            describe_request(),
            status_request("alice"),
        ] {
            let verb = request.verb();
            let signed = signed_as(&own_keys, Some(DEFAULT_MOB), request);
            match host_own_key_refusal(&signed, Some(&own_keys)) {
                Some(ControlResponse::Err { code, message }) => {
                    assert_eq!(code, HOST_OWN_KEY_CONTROL_CODE);
                    assert!(message.contains(verb.as_str()), "names the verb: {message}");
                }
                other => panic!("expected the own-key refusal for {verb}, got {other:?}"),
            }
        }

        // Every spelling the authorizer would read as the gateway's key is
        // refused: the prefixed one, and one padded with whitespace.
        let prefixed = respelled(own.clone(), |key| format!("ed25519:{key}"));
        assert!(host_own_key_refusal(&prefixed, Some(&own_keys)).is_some());
        let padded = respelled(own.clone(), |key| format!("  ed25519:{key}\n"));
        assert!(host_own_key_refusal(&padded, Some(&own_keys)).is_some());

        // A claim of the gateway's key whose signature does not verify is
        // not the gateway: the authorizer answers it like any other claim.
        let forged = claiming(&own_keys, &other_keys, lookup_request("alice"));
        assert!(host_own_key_refusal(&forged, Some(&own_keys)).is_none());
        let moved = {
            let caller = own.caller().cloned().expect("signed");
            let mut request = lookup_request("someone-else");
            request.set_caller(Some(caller));
            request
        };
        assert!(
            host_own_key_refusal(&moved, Some(&own_keys)).is_none(),
            "a signature moved to another request does not verify"
        );

        let undecodable = respelled(own, |_| "not-base64!!".to_string());
        assert!(
            host_own_key_refusal(&undecodable, Some(&own_keys)).is_none(),
            "an unusable credential is the authorizer's to refuse"
        );
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
