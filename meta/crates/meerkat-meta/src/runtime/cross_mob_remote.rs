//! Cross-mob remote-mob proxy: the client side of cross-process cross-mob
//! operations.
//!
//! `cross_mob.rs` dispatches wire/unwire/send operations as
//! `LocalOrRemote`: same-process peers registered via `register_peer_mob`
//! use their `MobHandle` directly; peers whose contact-directory entry
//! names a `tcp://host:port` or `uds:///path` transport are reached through
//! [`RemoteMobProxy`], which speaks the control protocol defined in
//! [`super::cross_mob_control`] (length-prefixed JSON frames, one
//! request/response pair per connection).
//!
//! [`RemoteMobProxy`] carries four operations against the peer gateway's
//! control listener:
//!
//! * [`RemoteMobProxy::wire_remote`] / [`RemoteMobProxy::unwire_remote`]
//!   install or remove a `TrustedPeerDescriptor` for one of OUR members on
//!   a member of the remote mob.
//! * [`RemoteMobProxy::inject_message`] performs an app-level external-turn
//!   injection into a remote member's session (`send_cross_mob`).
//! * [`RemoteMobProxy::lookup_member`] discovers a remote member's comms
//!   facts (peer id, comms name, transport pubkey, advertised address) so
//!   the local side can build a signed descriptor without caller-supplied
//!   bookkeeping.
//!
//! It also carries the keyed inject plane ([`super::keyed_inject`]):
//! [`RemoteMobProxy::inject_keyed`] and [`RemoteMobProxy::inject_status`].
//! Those never run unauthenticated, in either direction.
//!
//! The server side is [`super::cross_mob_control::MobHandleControlHandler`],
//! bound by `UnifiedRuntime::start_control_listener` (builder
//! `control_listen()` or the gateway `--control-listen` flag). Each request
//! opens a fresh connection; control RPC frequency is low (one message per
//! wire/unwire/inject call), so connection pooling is deliberately not
//! implemented.

use crate::contact_directory::{ContactEntry, MobTransport};

/// Errors raised by the remote-mob proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemoteMobError {
    /// The contact directory entry uses an unsupported transport.
    UnsupportedTransport { mob_id: String, transport: String },
    /// The cross-process control channel for this peer mob is not
    /// reachable: connect failed, write/read failed, or the peer timed
    /// out. The `operation` field reports which RPC stage hit the wall.
    ControlChannelUnavailable {
        mob_id: String,
        endpoint: String,
        operation: &'static str,
    },
    /// The peer answered with an error response. Error responses are never
    /// signed, so this is never evidence of anything: `code` (for example
    /// `unknown_member`, `member_not_granted` or `mob_error`) and `message`
    /// are the peer's words, kept as text for operators, and no caller
    /// branches on them.
    Rejected {
        mob_id: String,
        endpoint: String,
        code: String,
        message: String,
    },
    /// We failed to encode the request payload — usually a programmer
    /// error (un-serializable JSON value passed in).
    Encode { endpoint: String, message: String },
    /// We received a response we couldn't decode — peer is speaking a
    /// different protocol or sent corrupted data.
    Decode { endpoint: String, message: String },
    /// The contact directory pins this gateway's pubkey and requires
    /// signed control responses, but the response could not be
    /// authenticated (unsigned, malformed signature, or signed by a
    /// different key). Distinct from [`Self::Rejected`]: the peer did not
    /// refuse the request; its ANSWER could not be trusted.
    ControlResponseUnauthenticated {
        mob_id: String,
        endpoint: String,
        reason: String,
    },
}

impl RemoteMobError {
    /// Attach IO-error context to a `ControlChannelUnavailable` variant.
    /// Internal helper for the control-protocol module — keeps the public
    /// shape stable while letting us record the underlying message.
    pub(crate) fn with_context(self, context: String) -> Self {
        match self {
            Self::ControlChannelUnavailable {
                mob_id,
                endpoint,
                operation,
            } => Self::ControlChannelUnavailable {
                mob_id: if mob_id.is_empty() { context } else { mob_id },
                endpoint,
                operation,
            },
            other => other,
        }
    }
}

impl std::fmt::Display for RemoteMobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedTransport { mob_id, transport } => {
                write!(
                    f,
                    "remote mob '{mob_id}' uses unsupported transport: {transport}"
                )
            }
            Self::ControlChannelUnavailable {
                mob_id,
                endpoint,
                operation,
            } => {
                write!(
                    f,
                    "remote mob '{mob_id}' control channel ({endpoint}): operation '{operation}' \
                     failed — confirm the peer gateway is running with a control listener bound \
                     on this endpoint"
                )
            }
            Self::Rejected {
                mob_id,
                endpoint,
                code,
                message,
            } => {
                write!(
                    f,
                    "remote mob '{mob_id}' control channel ({endpoint}) rejected request \
                     [{code}]: {message}"
                )
            }
            Self::Encode { endpoint, message } => {
                write!(f, "control request encode failed for {endpoint}: {message}")
            }
            Self::Decode { endpoint, message } => {
                write!(
                    f,
                    "control response decode failed for {endpoint}: {message}"
                )
            }
            Self::ControlResponseUnauthenticated {
                mob_id,
                endpoint,
                reason,
            } => {
                write!(
                    f,
                    "remote mob '{mob_id}' control response ({endpoint}) failed \
                     authentication against the pinned gateway pubkey: {reason}. Upgrade the \
                     peer gateway to one that signs control responses (0.8.16+ with gateway \
                     keys installed), fix the pinned pubkey, or set \
                     require_signed_control = false on this contact to accept unsigned \
                     responses"
                )
            }
        }
    }
}

impl std::error::Error for RemoteMobError {}

/// Endpoint address for the peer mob's control channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEndpoint {
    /// TCP `host:port`.
    Tcp(String),
    /// Unix-domain-socket path.
    Uds(String),
}

impl RemoteEndpoint {
    /// Return the comms-style transport scheme (`tcp` or `uds`).
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Tcp(_) => "tcp",
            Self::Uds(_) => "uds",
        }
    }

    /// Return a comms-style address for this endpoint
    /// (`tcp://host:port` or `uds:///path`).
    pub fn comms_address(&self) -> String {
        match self {
            Self::Tcp(addr) => format!("tcp://{addr}"),
            Self::Uds(path) => format!("uds://{path}"),
        }
    }

    /// Bare endpoint string (without scheme) — `host:port` or `/path`.
    pub fn raw(&self) -> &str {
        match self {
            Self::Tcp(s) | Self::Uds(s) => s.as_str(),
        }
    }
}

/// Comms facts for a remote member, as returned by the peer gateway's
/// `LookupMember` control operation.
///
/// `pubkey_b64` and `advertised_address` are `None` when the peer gateway
/// could not resolve them (member without a comms runtime, or a control
/// handler running without session-service access). Remote wiring fails
/// closed on either absence: a descriptor without the member's own
/// transport key or a dialable socket address can never deliver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMemberInfo {
    pub peer_id: String,
    pub comms_name: String,
    /// The member's transport pubkey (base64, optional `ed25519:` prefix).
    pub pubkey_b64: Option<String>,
    /// The member's dialable envelope-listener address (`tcp://host:port`,
    /// `uds:///path`) or `inproc://name` for members without a socket
    /// transport.
    pub advertised_address: Option<String>,
}

/// A proxy for a mob that lives in a different process.
///
/// Every method opens a control connection to the peer gateway's
/// TCP/UDS control listener, sends one framed request, and awaits the
/// framed response. [`RemoteMobError::ControlChannelUnavailable`] means
/// the peer gateway is unreachable or has no control listener bound at
/// the configured endpoint.
#[derive(Debug, Clone)]
pub struct RemoteMobProxy {
    mob_id: String,
    endpoint: RemoteEndpoint,
    /// The peer GATEWAY's pinned Ed25519 pubkey from the contact
    /// directory, used to authenticate control responses.
    pinned_pubkey: Option<[u8; 32]>,
    /// Strict-when-pinned: a pinned pubkey requires signed responses
    /// unless the contact explicitly opted out
    /// (`require_signed_control = false`).
    require_signed_responses: bool,
    /// THIS gateway's keypair, used to authenticate US to the peer. The
    /// peer files grants under this pubkey. `None` on gateways that never
    /// installed a keypair: their requests go out unauthenticated and a
    /// grant-enforcing peer refuses them typed.
    caller_keys: Option<std::sync::Arc<crate::auth::peer_keys::GatewayPeerKeys>>,
}

impl RemoteMobProxy {
    /// Build a proxy from a contact-directory entry. Returns `None` for
    /// `Inproc` entries — those are served by an `Arc<MobHandle>` via
    /// `LocalOrRemote::Local` instead.
    ///
    /// Sends unauthenticated requests. Prefer
    /// [`Self::from_entry_with_caller`] anywhere the local gateway
    /// keypair is reachable, so peers that enforce scoped grants can
    /// attribute the call.
    pub fn from_entry(entry: &ContactEntry) -> Result<Option<Self>, RemoteMobError> {
        Self::from_entry_with_caller(entry, None)
    }

    /// Build a proxy that authenticates every request as `caller_keys`.
    ///
    /// The signature is minted over the request's semantic fields (see
    /// `control_request_signing_payload`) and bound to `entry.mob_id` as
    /// the audience, so a captured request cannot be replayed at another
    /// gateway where this same gateway also holds a grant.
    pub fn from_entry_with_caller(
        entry: &ContactEntry,
        caller_keys: Option<std::sync::Arc<crate::auth::peer_keys::GatewayPeerKeys>>,
    ) -> Result<Option<Self>, RemoteMobError> {
        let endpoint = match &entry.transport {
            MobTransport::Inproc => return Ok(None),
            MobTransport::Tcp(addr) => RemoteEndpoint::Tcp(addr.clone()),
            MobTransport::Uds(path) => RemoteEndpoint::Uds(path.clone()),
        };
        let require_signed_responses =
            entry.pubkey.is_some() && entry.require_signed_control.unwrap_or(true);
        Ok(Some(Self {
            mob_id: entry.mob_id.clone(),
            endpoint,
            pinned_pubkey: entry.pubkey,
            require_signed_responses,
            caller_keys,
        }))
    }

    /// The peer mob's identifier.
    pub fn mob_id(&self) -> &str {
        &self.mob_id
    }

    /// The peer mob's control endpoint.
    pub fn endpoint(&self) -> &RemoteEndpoint {
        &self.endpoint
    }

    /// Fresh per-request nonce. It rides inside the request bytes, and the
    /// response signature covers the digest of those bytes, so a signed
    /// response can never be replayed for a later request.
    fn mint_nonce() -> Option<String> {
        Some(uuid::Uuid::new_v4().simple().to_string())
    }

    /// A peer `Err` response. It is unsigned, so its code is kept as text
    /// and never classified: one unverified class for every code.
    fn rejected(&self, code: String, message: String) -> RemoteMobError {
        RemoteMobError::Rejected {
            mob_id: self.mob_id.clone(),
            endpoint: self.endpoint.comms_address(),
            code,
            message,
        }
    }

    /// Serialize, send, and (when this contact pins a gateway pubkey and
    /// requires it) authenticate one control request/response exchange.
    ///
    /// Takes the request by value because it stamps this gateway's caller
    /// credential onto it first: the signature covers the request as it
    /// goes on the wire, so it cannot be minted before the final shape is
    /// known.
    async fn dispatch(
        &self,
        mut request: super::cross_mob_control::ControlRequest,
    ) -> Result<super::cross_mob_control::ControlResponse, RemoteMobError> {
        if let Some(keys) = self.caller_keys.as_ref() {
            super::cross_mob_control::sign_control_request_as_caller(
                keys,
                Some(self.mob_id.as_str()),
                &mut request,
            );
        }
        let payload = serde_json::to_vec(&request).map_err(|err| RemoteMobError::Encode {
            endpoint: self.endpoint.comms_address(),
            message: err.to_string(),
        })?;
        let response = super::cross_mob_control::RemoteControlClient::send_payload(
            &self.endpoint,
            &payload,
            super::cross_mob_control::DEFAULT_CONTROL_TIMEOUT,
        )
        .await
        .map_err(|err| self.attach_mob_id(err))?;
        if self.require_signed_responses
            && let Some(pinned) = self.pinned_pubkey.as_ref()
            && let Err(reason) =
                super::cross_mob_control::verify_control_response(pinned, &payload, &response)
        {
            return Err(RemoteMobError::ControlResponseUnauthenticated {
                mob_id: self.mob_id.clone(),
                endpoint: self.endpoint.comms_address(),
                reason,
            });
        }
        Ok(response)
    }

    /// Wire a peer on the remote side.
    ///
    /// Sends a `Wire` control request to the peer gateway over its
    /// configured TCP/UDS endpoint. The remote side calls
    /// `MobHandle::wire(local_member, PeerTarget::External(spec))` and
    /// responds with `Ok` or `Err`.
    pub async fn wire_remote(
        &self,
        remote_member: &str,
        local_peer_spec_address: &str,
        local_comms_name: &str,
        local_peer_id: &str,
        local_pubkey_b64: Option<String>,
    ) -> Result<(), RemoteMobError> {
        let request = super::cross_mob_control::ControlRequest::Wire {
            remote_member: remote_member.to_string(),
            local_peer_spec_address: local_peer_spec_address.to_string(),
            local_comms_name: local_comms_name.to_string(),
            local_peer_id: local_peer_id.to_string(),
            local_pubkey_b64,
            nonce: Self::mint_nonce(),
            caller: None,
        };
        self.dispatch_no_payload(request, "wire").await
    }

    /// Unwire a peer on the remote side. Symmetric with [`Self::wire_remote`].
    pub async fn unwire_remote(
        &self,
        remote_member: &str,
        local_peer_spec_address: &str,
        local_comms_name: &str,
        local_peer_id: &str,
        local_pubkey_b64: Option<String>,
    ) -> Result<(), RemoteMobError> {
        let request = super::cross_mob_control::ControlRequest::Unwire {
            remote_member: remote_member.to_string(),
            local_peer_spec_address: local_peer_spec_address.to_string(),
            local_comms_name: local_comms_name.to_string(),
            local_peer_id: local_peer_id.to_string(),
            local_pubkey_b64,
            nonce: Self::mint_nonce(),
            caller: None,
        };
        self.dispatch_no_payload(request, "unwire").await
    }

    /// Inject an external-turn message into a remote member's session.
    ///
    /// Returns the bridge session id that accepted the injection, so the
    /// caller can correlate downstream events. Mirrors the local
    /// `MobHandle::send` shape used by `send_cross_mob`.
    pub async fn inject_message(
        &self,
        remote_member: &str,
        content_json: serde_json::Value,
    ) -> Result<String, RemoteMobError> {
        let request = super::cross_mob_control::ControlRequest::Inject {
            remote_member: remote_member.to_string(),
            content: content_json,
            nonce: Self::mint_nonce(),
            caller: None,
        };
        let response = self.dispatch(request).await?;
        match response {
            super::cross_mob_control::ControlResponse::Injected { session_id, .. } => {
                Ok(session_id)
            }
            super::cross_mob_control::ControlResponse::Err { code, message } => {
                Err(self.rejected(code, message))
            }
            other => Err(RemoteMobError::Decode {
                endpoint: self.endpoint.comms_address(),
                message: format!("expected Injected response, got {other:?}"),
            }),
        }
    }

    async fn dispatch_no_payload(
        &self,
        request: super::cross_mob_control::ControlRequest,
        operation: &'static str,
    ) -> Result<(), RemoteMobError> {
        let response = self.dispatch(request).await?;
        match response {
            super::cross_mob_control::ControlResponse::Ok { .. } => Ok(()),
            super::cross_mob_control::ControlResponse::Err { code, message } => {
                Err(self.rejected(code, message))
            }
            other => Err(RemoteMobError::Decode {
                endpoint: self.endpoint.comms_address(),
                message: format!("expected Ok for {operation}, got {other:?}"),
            }),
        }
    }

    /// Look up a remote member's comms facts via the control channel.
    /// Used during `wire_cross_mob` to discover what `TrustedPeerDescriptor`
    /// to build for the local-side wire without caller-supplied bookkeeping.
    pub async fn lookup_member(
        &self,
        remote_member: &str,
    ) -> Result<RemoteMemberInfo, RemoteMobError> {
        let request = super::cross_mob_control::ControlRequest::LookupMember {
            remote_member: remote_member.to_string(),
            nonce: Self::mint_nonce(),
            caller: None,
        };
        let response = self.dispatch(request).await?;
        match response {
            super::cross_mob_control::ControlResponse::Member {
                peer_id,
                comms_name,
                pubkey_b64,
                advertised_address,
                sig_b64: _,
            } => Ok(RemoteMemberInfo {
                peer_id,
                comms_name,
                pubkey_b64,
                advertised_address,
            }),
            super::cross_mob_control::ControlResponse::Err { code, message } => {
                Err(self.rejected(code, message))
            }
            other => Err(RemoteMobError::Decode {
                endpoint: self.endpoint.comms_address(),
                message: format!("expected Member response, got {other:?}"),
            }),
        }
    }

    /// Whether this proxy can make authenticated keyed inject calls: the contact
    /// pins the peer's gateway key and does not opt out of signed control
    /// responses, and this gateway holds its own signing key.
    pub fn keyed_inject_ready(&self) -> Result<(), super::keyed_inject::KeyedInjectCallError> {
        use super::keyed_inject::KeyedInjectCallError;
        if self.pinned_pubkey.is_none() {
            return Err(KeyedInjectCallError::NotAuthenticatable(format!(
                "the contact for '{}' pins no gateway key",
                self.mob_id
            )));
        }
        if !self.require_signed_responses {
            return Err(KeyedInjectCallError::NotAuthenticatable(format!(
                "the contact for '{}' opts out of signed control responses",
                self.mob_id
            )));
        }
        if self.caller_keys.is_none() {
            return Err(KeyedInjectCallError::NotAuthenticatable(
                "this gateway holds no signing key".to_string(),
            ));
        }
        Ok(())
    }

    /// Deliver one keyed host continuation to `delivery.member` on the peer.
    ///
    /// Unlike the member-plane calls, this never runs unauthenticated: see
    /// [`Self::keyed_inject_ready`]. The answer must verify against the pinned
    /// key whatever the contact's `require_signed_control` says. An
    /// unsigned, unverifiable or `Err` answer is a
    /// [`super::keyed_inject::KeyedInjectCallError`], which is never evidence
    /// about the delivery.
    pub async fn inject_keyed(
        &self,
        delivery: super::keyed_inject::KeyedInject<'_>,
        issued_at_ms: u64,
    ) -> Result<
        super::keyed_inject::KeyedInjectCallOutcome<super::keyed_inject::NativeReceipt>,
        super::keyed_inject::KeyedInjectCallError,
    > {
        use super::cross_mob_control::ControlResponse;
        use super::keyed_inject::{KeyedInjectCallError, KeyedInjectCallOutcome};
        let request = super::cross_mob_control::ControlRequest::InjectKeyed {
            remote_member: delivery.member.to_string(),
            delivery_key: delivery.delivery_key.to_string(),
            content_digest: delivery.content_digest.to_string(),
            summary: delivery.summary.to_string(),
            body: delivery.body.to_string(),
            issued_at_ms,
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            caller: None,
        };
        match self.dispatch_keyed_inject(request).await? {
            ControlResponse::InjectReceipt { receipt, .. } => {
                Ok(KeyedInjectCallOutcome::Answered(receipt))
            }
            ControlResponse::InjectRefused {
                code,
                message,
                cause,
                ..
            } => Ok(KeyedInjectCallOutcome::Refused {
                code,
                message,
                cause,
            }),
            ControlResponse::Err { code, message } => {
                Err(KeyedInjectCallError::Rejected { code, message })
            }
            other => Err(KeyedInjectCallError::Unexpected(format!(
                "expected a keyed inject receipt, got {other:?}"
            ))),
        }
    }

    /// Read a keyed delivery's native status for `member` on the peer.
    /// Authenticated exactly like [`Self::inject_keyed`].
    pub async fn inject_status(
        &self,
        member: &str,
        delivery_key: &str,
        with_input: bool,
        issued_at_ms: u64,
    ) -> Result<
        super::keyed_inject::KeyedInjectCallOutcome<super::keyed_inject::KeyedInjectStatusAnswer>,
        super::keyed_inject::KeyedInjectCallError,
    > {
        use super::cross_mob_control::ControlResponse;
        use super::keyed_inject::{
            KeyedInjectCallError, KeyedInjectCallOutcome, KeyedInjectStatusAnswer,
        };
        let request = super::cross_mob_control::ControlRequest::InjectStatus {
            remote_member: member.to_string(),
            delivery_key: delivery_key.to_string(),
            with_input,
            issued_at_ms,
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            caller: None,
        };
        match self.dispatch_keyed_inject(request).await? {
            ControlResponse::InjectStatus { status, input, .. } => {
                Ok(KeyedInjectCallOutcome::Answered(KeyedInjectStatusAnswer {
                    status,
                    input,
                }))
            }
            ControlResponse::InjectRefused {
                code,
                message,
                cause,
                ..
            } => Ok(KeyedInjectCallOutcome::Refused {
                code,
                message,
                cause,
            }),
            ControlResponse::Err { code, message } => {
                Err(KeyedInjectCallError::Rejected { code, message })
            }
            other => Err(KeyedInjectCallError::Unexpected(format!(
                "expected a keyed inject status, got {other:?}"
            ))),
        }
    }

    /// Sign, send and authenticate one keyed inject exchange. The response must
    /// verify against the pinned key; `Err` answers, which are never signed,
    /// pass through for the caller to report as rejections.
    async fn dispatch_keyed_inject(
        &self,
        mut request: super::cross_mob_control::ControlRequest,
    ) -> Result<super::cross_mob_control::ControlResponse, super::keyed_inject::KeyedInjectCallError>
    {
        use super::keyed_inject::KeyedInjectCallError;
        self.keyed_inject_ready()?;
        let (Some(pinned), Some(keys)) = (self.pinned_pubkey.as_ref(), self.caller_keys.as_ref())
        else {
            return Err(KeyedInjectCallError::NotAuthenticatable(
                "no pinned key or no signing key".to_string(),
            ));
        };
        super::cross_mob_control::sign_control_request_as_caller(
            keys,
            Some(self.mob_id.as_str()),
            &mut request,
        );
        let payload = serde_json::to_vec(&request)
            .map_err(|error| KeyedInjectCallError::Unexpected(error.to_string()))?;
        if payload.len() > super::cross_mob_control::MAX_CONTROL_PAYLOAD as usize {
            return Err(KeyedInjectCallError::Oversize {
                bytes: payload.len(),
            });
        }
        let response = super::cross_mob_control::RemoteControlClient::send_payload(
            &self.endpoint,
            &payload,
            super::cross_mob_control::DEFAULT_CONTROL_TIMEOUT,
        )
        .await
        .map_err(|error| KeyedInjectCallError::Transport(self.attach_mob_id(error).to_string()))?;
        super::cross_mob_control::verify_control_response(pinned, &payload, &response)
            .map_err(KeyedInjectCallError::Unauthenticated)?;
        Ok(response)
    }

    fn attach_mob_id(&self, err: RemoteMobError) -> RemoteMobError {
        match err {
            RemoteMobError::ControlChannelUnavailable {
                mob_id,
                endpoint,
                operation,
            } if mob_id.is_empty() || mob_id != self.mob_id => {
                RemoteMobError::ControlChannelUnavailable {
                    mob_id: self.mob_id.clone(),
                    endpoint,
                    operation,
                }
            }
            other => other,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn from_entry_inproc_returns_none() {
        let entry = ContactEntry {
            mob_id: "demo".to_string(),
            transport: MobTransport::Inproc,
            pubkey: None,
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry(&entry).expect("inproc is supported");
        assert!(proxy.is_none());
    }

    #[test]
    fn from_entry_tcp_round_trip() {
        let entry = ContactEntry {
            mob_id: "remote".to_string(),
            transport: MobTransport::Tcp("127.0.0.1:9001".to_string()),
            pubkey: None,
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry(&entry)
            .expect("tcp is supported")
            .expect("tcp returns Some(proxy)");
        assert_eq!(proxy.mob_id(), "remote");
        assert_eq!(proxy.endpoint().scheme(), "tcp");
        assert_eq!(proxy.endpoint().raw(), "127.0.0.1:9001");
        assert_eq!(proxy.endpoint().comms_address(), "tcp://127.0.0.1:9001");
    }

    #[test]
    fn from_entry_uds_round_trip() {
        let entry = ContactEntry {
            mob_id: "remote-uds".to_string(),
            transport: MobTransport::Uds("/tmp/cross-mob.sock".to_string()),
            pubkey: None,
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry(&entry)
            .expect("uds is supported")
            .expect("uds returns Some(proxy)");
        assert_eq!(proxy.endpoint().scheme(), "uds");
        assert_eq!(proxy.endpoint().raw(), "/tmp/cross-mob.sock");
        assert_eq!(
            proxy.endpoint().comms_address(),
            "uds:///tmp/cross-mob.sock"
        );
    }

    /// When no listener is bound at the configured endpoint, `wire_remote`
    /// surfaces `ControlChannelUnavailable` with the mob id attached.
    /// (End-to-end happy path lives in `tests/cross_mob_tcp.rs` once
    /// the unified runtime spins up its own control listener.)
    #[tokio::test]
    async fn wire_remote_returns_unavailable_when_no_listener() {
        let entry = ContactEntry {
            mob_id: "remote".to_string(),
            transport: MobTransport::Tcp("127.0.0.1:1".to_string()),
            pubkey: None,
            require_signed_control: None,
        };
        let proxy = RemoteMobProxy::from_entry(&entry)
            .expect("tcp ok")
            .expect("some");
        let err = proxy
            .wire_remote(
                "alice",
                "tcp://127.0.0.1:9000",
                "demo/role/alice",
                "00000000-0000-4000-8000-000000000001",
                None,
            )
            .await
            .expect_err("no listener");
        assert!(
            matches!(err, RemoteMobError::ControlChannelUnavailable { ref mob_id, .. } if mob_id == "remote"),
            "got {err:?}"
        );
    }
}
