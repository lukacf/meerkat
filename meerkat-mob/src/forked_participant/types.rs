//! Source-owned forked-participant capability vocabulary (issue #159, phase 2).
//!
//! A forked participant capability is a detached, scoped, expiring
//! participation grant: the runtime that owns a source member forks that
//! member's conversation at an exact complete boundary and hands out an
//! immutable reference to the resulting branch. A temporary coordination mob
//! can later attach that reference, run bounded work inside the participant's
//! own execution context, and release it.
//!
//! Three hard rules shape every type here.
//!
//! - The reference is bearer authority. [`ForkedParticipantCapabilityId`] is
//!   secret-quality 256-bit OS entropy, its `Debug` is redacted, it has no
//!   `Display`, its correlation handle is a digest rather than a token prefix,
//!   and every parse (including serde) fails closed on a malformed token.
//! - The reference is immutable. Its fields are private with read-only
//!   accessors and one source-owner constructor, so a holder cannot widen a
//!   grant by mutating a struct field.
//! - The reference carries no live state and no credentials. It binds
//!   identities, a route, exact provenance, scope, expiry, and reuse policy.
//!   Inherited tool/auth/realm/filesystem policy stays in the source-owned fork
//!   session metadata, which the source runtime already owns.
//!
//! Lifecycle legality is not decided here. Every mutation drives the canonical
//! `ForkedParticipantLifecycleMachine` and interprets its typed effects.

use super::service::ForkedParticipantError;
use crate::ids::AgentIdentity;
use crate::machines::mob_machine::HostId;
use chrono::{DateTime, Utc};
use meerkat_core::SessionId;
use meerkat_core::connection::RealmId;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use thiserror::Error;

/// Project one immutable capability reference onto its V6 wire form.
///
/// Public because the wire form is the shape a host compares an incoming
/// materialize attachment against; callers that hold a domain reference need
/// the same projection the serving arms use, not a hand-rolled copy of it.
pub fn bridge_ref(
    reference: &ForkedParticipantRef,
) -> meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantRef {
    use meerkat_contracts::wire::supervisor_bridge as bridge;
    bridge::BridgeForkedParticipantRef {
        capability_id: reference.capability_id.expose_bearer_token().to_string(),
        source_identity: reference.source_identity.as_str().to_string(),
        fork_session_id: reference.fork_session_id.to_string(),
        owner_route: match &reference.owner_route {
            ForkedParticipantOwnerRoute::Local { realm_id } => {
                bridge::BridgeForkedParticipantOwnerRoute::Local {
                    realm_id: realm_id.as_str().to_string(),
                }
            }
            ForkedParticipantOwnerRoute::Host { realm_id, host_id } => {
                bridge::BridgeForkedParticipantOwnerRoute::Host {
                    realm_id: realm_id.as_str().to_string(),
                    host_id: host_id.as_str().to_string(),
                }
            }
        },
        source_session_id: reference.provenance.source_session_id.to_string(),
        prefix_message_count: reference.provenance.prefix_message_count as u64,
        prefix_digest: reference.provenance.prefix_digest.clone(),
        scope: match reference.scope {
            ForkedParticipantOperationScope::Invoke => bridge::BridgeForkedParticipantScope::Invoke,
            ForkedParticipantOperationScope::Observe => {
                bridge::BridgeForkedParticipantScope::Observe
            }
            ForkedParticipantOperationScope::InvokeAndObserve => {
                bridge::BridgeForkedParticipantScope::InvokeAndObserve
            }
        },
        reuse: match reference.reuse {
            ForkedParticipantReusePolicy::OneShot => bridge::BridgeForkedParticipantReuse::OneShot,
            ForkedParticipantReusePolicy::BoundedReuse { max_uses } => {
                bridge::BridgeForkedParticipantReuse::BoundedReuse { max_uses }
            }
        },
        expires_at: reference.expires_at,
        revocation_id: reference.revocation_id.as_str().to_string(),
        cleanup_id: reference.cleanup_id.as_str().to_string(),
    }
}

/// Parse one presented wire reference back into its validated domain form.
///
/// Every field runs its own validator, so a tampered wire capability either
/// fails to parse here or fails the owner's full-reference comparison later.
pub fn domain_ref(
    wire: &meerkat_contracts::wire::supervisor_bridge::BridgeForkedParticipantRef,
) -> Result<ForkedParticipantRef, ForkedParticipantError> {
    use meerkat_contracts::wire::supervisor_bridge as bridge;
    let capability_id = ForkedParticipantCapabilityId::parse_bearer_token(&wire.capability_id)
        .map_err(|_| ForkedParticipantError::CapabilityRejected {
            detail: "capability bearer is malformed".to_string(),
        })?;
    let source_identity = AgentIdentity::from(wire.source_identity.clone());
    let fork_session_id = SessionId::parse(&wire.fork_session_id).map_err(|_| {
        ForkedParticipantError::CapabilityRejected {
            detail: "fork session identity is malformed".to_string(),
        }
    })?;
    let (realm_id, host_id) = match &wire.owner_route {
        bridge::BridgeForkedParticipantOwnerRoute::Host { realm_id, host_id } => (
            RealmId::parse(realm_id).map_err(|_| ForkedParticipantError::CapabilityRejected {
                detail: "owner realm is malformed".to_string(),
            })?,
            HostId::from(host_id.clone()),
        ),
        bridge::BridgeForkedParticipantOwnerRoute::Local { .. } => {
            return Err(ForkedParticipantError::CapabilityRejected {
                detail: "remote capability reference must name a host route".to_string(),
            });
        }
    };
    let source_session_id = SessionId::parse(&wire.source_session_id).map_err(|_| {
        ForkedParticipantError::CapabilityRejected {
            detail: "source session identity is malformed".to_string(),
        }
    })?;
    let prefix_message_count = usize::try_from(wire.prefix_message_count).map_err(|_| {
        ForkedParticipantError::CapabilityRejected {
            detail: "prefix count is out of range".to_string(),
        }
    })?;
    let request_id =
        ForkedParticipantRequestId::new(wire.revocation_id.strip_prefix("fpr:").ok_or_else(
            || ForkedParticipantError::CapabilityRejected {
                detail: "capability revocation handle is malformed".to_string(),
            },
        )?)
        .map_err(|_| ForkedParticipantError::CapabilityRejected {
            detail: "capability revocation handle is malformed".to_string(),
        })?;
    if ForkedParticipantRevocationId::for_request(&request_id).as_str() != wire.revocation_id
        || ForkedParticipantCleanupId::for_request(&request_id).as_str() != wire.cleanup_id
    {
        return Err(ForkedParticipantError::CapabilityRejected {
            detail: "capability control handles do not match".to_string(),
        });
    }
    Ok(ForkedParticipantRef::new_source_owned(
        capability_id,
        source_identity,
        fork_session_id,
        ForkedParticipantOwnerRoute::Host { realm_id, host_id },
        ForkedParticipantProvenance {
            source_session_id,
            prefix_message_count,
            prefix_digest: wire.prefix_digest.clone(),
        },
        match wire.scope {
            bridge::BridgeForkedParticipantScope::Invoke => ForkedParticipantOperationScope::Invoke,
            bridge::BridgeForkedParticipantScope::Observe => {
                ForkedParticipantOperationScope::Observe
            }
            bridge::BridgeForkedParticipantScope::InvokeAndObserve => {
                ForkedParticipantOperationScope::InvokeAndObserve
            }
        },
        wire.expires_at,
        match wire.reuse {
            bridge::BridgeForkedParticipantReuse::OneShot => ForkedParticipantReusePolicy::OneShot,
            bridge::BridgeForkedParticipantReuse::BoundedReuse { max_uses } => {
                ForkedParticipantReusePolicy::BoundedReuse { max_uses }
            }
        },
        ForkedParticipantRevocationId::for_request(&request_id),
        ForkedParticipantCleanupId::for_request(&request_id),
    ))
}

/// Hard cap on a capability's bounded reuse budget.
///
/// The lifecycle machine tracks every granted attachment id in a per-record set
/// so a replayed attachment can never consume a second use. That set is bounded
/// by the reuse budget, so the budget itself must be bounded before machine
/// admission — otherwise a caller-supplied `max_uses` would let an attacker
/// grow source-owned durable state without limit.
pub const MAX_FORKED_PARTICIPANT_USES: u32 = 64;

/// Hard cap on a capability's time-to-live.
pub const MAX_FORKED_PARTICIPANT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Maximum accepted length of a caller-supplied attachment identity.
pub const MAX_FORKED_PARTICIPANT_ATTACHMENT_ID_LEN: usize = 256;

/// Maximum accepted length of a caller-supplied request identity.
pub const MAX_FORKED_PARTICIPANT_REQUEST_ID_LEN: usize = 256;

/// Number of hex characters in a capability bearer token (256 bits).
pub const FORKED_PARTICIPANT_BEARER_TOKEN_LEN: usize = 64;

/// Version of the canonical request-fingerprint shape.
///
/// The fingerprint is a persisted contract: the lifecycle machine compares it
/// to decide replay versus conflict, so its serialized shape is explicitly
/// versioned. Changing any field, discriminant, or ordering requires bumping
/// this constant.
pub const FORKED_PARTICIPANT_FINGERPRINT_VERSION: u32 = 1;

/// Rejection of a malformed typed identity.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ForkedParticipantIdentityError {
    /// A capability bearer token was not exactly 64 lowercase hex characters.
    #[error(
        "capability bearer token must be exactly {FORKED_PARTICIPANT_BEARER_TOKEN_LEN} lowercase hex characters"
    )]
    MalformedBearerToken,

    /// A caller-supplied identity was empty (or whitespace only).
    #[error("{kind} identity must not be empty")]
    Empty {
        /// Which identity kind was rejected.
        kind: &'static str,
    },

    /// A caller-supplied identity exceeded its bound.
    #[error("{kind} identity must not exceed {max} bytes")]
    TooLong {
        /// Which identity kind was rejected.
        kind: &'static str,
        /// The bound that was exceeded.
        max: usize,
    },

    /// A caller-supplied identity carried control characters.
    #[error("{kind} identity must not contain control characters")]
    ControlCharacter {
        /// Which identity kind was rejected.
        kind: &'static str,
    },
}

/// Failure to compute a canonical request fingerprint.
#[derive(Debug, Error)]
#[error("failed to serialize the canonical forked-participant request fingerprint")]
pub struct ForkedParticipantFingerprintError {
    #[source]
    source: serde_json::Error,
}

fn validate_identity(
    kind: &'static str,
    raw: &str,
    max: usize,
) -> Result<String, ForkedParticipantIdentityError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ForkedParticipantIdentityError::Empty { kind });
    }
    if trimmed.len() > max {
        return Err(ForkedParticipantIdentityError::TooLong { kind, max });
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ForkedParticipantIdentityError::ControlCharacter { kind });
    }
    Ok(trimmed.to_owned())
}

/// Unguessable bearer identity of one forked-participant capability.
///
/// This is 256 bits of OS entropy, not a UUID: a UUIDv4 carries 122 bits and
/// advertises uniqueness, not secrecy. `Debug` is redacted and there is no
/// `Display`, so the value cannot reach a log line by accident. Parsing —
/// including deserialization — fails closed on anything that is not exactly
/// [`FORKED_PARTICIPANT_BEARER_TOKEN_LEN`] lowercase hex characters, so a
/// malformed capability id can never exist as a typed value.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ForkedParticipantCapabilityId(String);

impl ForkedParticipantCapabilityId {
    /// Mint a fresh capability id from secret-quality OS entropy.
    pub fn mint() -> Result<Self, meerkat_core::secret_entropy::SecretEntropyError> {
        Ok(Self(
            meerkat_core::secret_entropy::secret_entropy_hex::<32>()?,
        ))
    }

    /// Parse a bearer token presented by a holder.
    ///
    /// Named `parse_bearer_token` rather than `new`/`from_str` so a reader can
    /// see that the caller is handling bearer authority.
    pub fn parse_bearer_token(token: &str) -> Result<Self, ForkedParticipantIdentityError> {
        if token.len() != FORKED_PARTICIPANT_BEARER_TOKEN_LEN
            || !token
                .chars()
                .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
        {
            return Err(ForkedParticipantIdentityError::MalformedBearerToken);
        }
        Ok(Self(token.to_owned()))
    }

    /// Borrow the raw bearer token.
    ///
    /// Deliberately explicit: call sites that persist or transmit the token are
    /// visible in review, and nothing recovers the token by formatting.
    pub fn expose_bearer_token(&self) -> &str {
        &self.0
    }

    /// Non-secret correlation handle for logs and error messages.
    ///
    /// A digest prefix, never a prefix of the bearer itself: leaking the first
    /// characters of a bearer token narrows the search space for the rest.
    pub fn correlation_hint(&self) -> String {
        let digest = format!("{:x}", Sha256::digest(self.0.as_bytes()));
        format!("fpc:{}", &digest[..12])
    }
}

impl std::fmt::Debug for ForkedParticipantCapabilityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ForkedParticipantCapabilityId")
            .field(&"[REDACTED]")
            .finish()
    }
}

impl<'de> Deserialize<'de> for ForkedParticipantCapabilityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse_bearer_token(&raw).map_err(serde::de::Error::custom)
    }
}

macro_rules! validated_identity {
    ($name:ident, $kind:expr, $max:expr, $doc:literal) => {
        #[doc = $doc]
        ///
        /// Construction and deserialization both validate: the value is
        /// trimmed, non-empty, bounded, and free of control characters, so an
        /// invalid typed value cannot exist.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validate and wrap a caller-supplied identity.
            pub fn new(raw: impl AsRef<str>) -> Result<Self, ForkedParticipantIdentityError> {
                validate_identity($kind, raw.as_ref(), $max).map(Self)
            }

            /// Borrow the validated identity.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let raw = String::deserialize(deserializer)?;
                Self::new(raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

validated_identity!(
    ForkedParticipantRequestId,
    "request",
    MAX_FORKED_PARTICIPANT_REQUEST_ID_LEN,
    "Caller-stable idempotency identity of one fork request."
);
validated_identity!(
    ForkedParticipantAttachmentId,
    "attachment",
    MAX_FORKED_PARTICIPANT_ATTACHMENT_ID_LEN,
    "Stable identity of one attachment of a capability to a temporary mob."
);

/// Non-secret revocation handle bound into the capability reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ForkedParticipantRevocationId(String);

/// Non-secret cleanup handle bound into the capability reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ForkedParticipantCleanupId(String);

impl ForkedParticipantRevocationId {
    /// Derive the revocation handle for one request.
    pub fn for_request(request_id: &ForkedParticipantRequestId) -> Self {
        Self(format!("fpr:{}", request_id.as_str()))
    }

    /// Borrow the raw handle.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ForkedParticipantCleanupId {
    /// Derive the cleanup handle for one request.
    pub fn for_request(request_id: &ForkedParticipantRequestId) -> Self {
        Self(format!("fpk:{}", request_id.as_str()))
    }

    /// Borrow the raw handle.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Operations a holder may perform through an attached capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkedParticipantOperationScope {
    /// Send bounded work into the forked participant.
    Invoke,
    /// Observe the forked participant's output only.
    Observe,
    /// Both invoke and observe.
    InvokeAndObserve,
}

impl ForkedParticipantOperationScope {
    /// Fixed fingerprint discriminant.
    fn fingerprint_discriminant(self) -> &'static str {
        match self {
            Self::Invoke => "invoke",
            Self::Observe => "observe",
            Self::InvokeAndObserve => "invoke_and_observe",
        }
    }
}

/// How many times one capability may be attached over its lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ForkedParticipantReusePolicy {
    /// Exactly one attachment; the capability exhausts on its release.
    OneShot,
    /// A bounded number of sequential attachments.
    BoundedReuse {
        /// Attachment budget, capped by [`MAX_FORKED_PARTICIPANT_USES`].
        max_uses: u32,
    },
}

impl ForkedParticipantReusePolicy {
    /// Reuse budget as the machine's `max_uses`.
    pub fn max_uses(self) -> u32 {
        match self {
            Self::OneShot => 1,
            Self::BoundedReuse { max_uses } => max_uses,
        }
    }

    /// Fixed fingerprint discriminant.
    fn fingerprint_discriminant(self) -> &'static str {
        match self {
            Self::OneShot => "one_shot",
            Self::BoundedReuse { .. } => "bounded_reuse",
        }
    }
}

/// Typed route to the runtime that owns the source member and the fork.
///
/// The route is part of the immutable reference: a holder cannot re-point a
/// capability at a different realm or host, and every operation verifies it
/// against the owning service's own route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ForkedParticipantOwnerRoute {
    /// The source member runs in this runtime, inside the named realm.
    Local {
        /// Realm that owns the source member and the fork.
        realm_id: RealmId,
    },
    /// The source member runs on a bound member host inside the named realm.
    Host {
        /// Realm that owns the source member and the fork.
        realm_id: RealmId,
        /// Existing typed host identity of the owning runtime host.
        host_id: HostId,
    },
}

impl ForkedParticipantOwnerRoute {
    /// Realm that owns the source member and the fork.
    pub fn realm_id(&self) -> &RealmId {
        match self {
            Self::Local { realm_id } | Self::Host { realm_id, .. } => realm_id,
        }
    }

    /// Fixed fingerprint discriminant.
    fn fingerprint_discriminant(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local",
            Self::Host { .. } => "host",
        }
    }

    /// Host identity, when the route names one.
    fn fingerprint_host(&self) -> Option<&str> {
        match self {
            Self::Local { .. } => None,
            Self::Host { host_id, .. } => Some(host_id.as_str()),
        }
    }
}

/// Exact selected-prefix provenance of one fork.
///
/// This is what makes the branch auditable without copying it: the source
/// session plus the exact prefix (message count and content digest) selected
/// for the child.
///
/// It deliberately does NOT carry a source head revision. Issue #159 asks for
/// the exact source revision OR a provenance digest, and only the digest is
/// observable on every path that can produce a capability: a crash-retry
/// discovers an already-durable child and can prove its content digest, but it
/// cannot prove what the source head was when the child was first taken.
/// Publishing a fabricated or substituted head revision would be false
/// provenance, so the field is absent rather than approximated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedParticipantProvenance {
    /// Session the fork was taken from.
    pub source_session_id: SessionId,
    /// Number of source messages selected into the child.
    pub prefix_message_count: usize,
    /// Content digest of the selected prefix.
    pub prefix_digest: String,
}

/// Immutable, authenticated capability reference handed to a holder.
///
/// Fields are private: a holder holds a value it can read but cannot widen.
/// The only in-crate mint is [`ForkedParticipantRef::new_source_owned`], and
/// deserialization runs every field's own validation, so a tampered wire
/// capability either fails to parse or fails the store's full-reference
/// comparison.
///
/// `Debug` is safe to print: the only secret field redacts itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedParticipantRef {
    capability_id: ForkedParticipantCapabilityId,
    source_identity: AgentIdentity,
    fork_session_id: SessionId,
    owner_route: ForkedParticipantOwnerRoute,
    provenance: ForkedParticipantProvenance,
    scope: ForkedParticipantOperationScope,
    expires_at: DateTime<Utc>,
    reuse: ForkedParticipantReusePolicy,
    revocation_id: ForkedParticipantRevocationId,
    cleanup_id: ForkedParticipantCleanupId,
}

impl ForkedParticipantRef {
    /// Mint one capability reference. Source-owner path only.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_source_owned(
        capability_id: ForkedParticipantCapabilityId,
        source_identity: AgentIdentity,
        fork_session_id: SessionId,
        owner_route: ForkedParticipantOwnerRoute,
        provenance: ForkedParticipantProvenance,
        scope: ForkedParticipantOperationScope,
        expires_at: DateTime<Utc>,
        reuse: ForkedParticipantReusePolicy,
        revocation_id: ForkedParticipantRevocationId,
        cleanup_id: ForkedParticipantCleanupId,
    ) -> Self {
        Self {
            capability_id,
            source_identity,
            fork_session_id,
            owner_route,
            provenance,
            scope,
            expires_at,
            reuse,
            revocation_id,
            cleanup_id,
        }
    }

    /// Bearer identity of the capability.
    pub fn capability_id(&self) -> &ForkedParticipantCapabilityId {
        &self.capability_id
    }

    /// Stable identity of the source member whose conversation was forked.
    pub fn source_identity(&self) -> &AgentIdentity {
        &self.source_identity
    }

    /// Durable identity of the forked branch session.
    pub fn fork_session_id(&self) -> &SessionId {
        &self.fork_session_id
    }

    /// Typed route to the owning runtime.
    pub fn owner_route(&self) -> &ForkedParticipantOwnerRoute {
        &self.owner_route
    }

    /// Exact selected-prefix provenance.
    pub fn provenance(&self) -> &ForkedParticipantProvenance {
        &self.provenance
    }

    /// Operations the holder may perform.
    pub fn scope(&self) -> ForkedParticipantOperationScope {
        self.scope
    }

    /// Absolute expiry instant. The machine never reads a clock; the source
    /// owner samples time and feeds the resulting observation in.
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// One-shot or bounded reuse.
    pub fn reuse(&self) -> ForkedParticipantReusePolicy {
        self.reuse
    }

    /// Non-secret revocation handle.
    pub fn revocation_id(&self) -> &ForkedParticipantRevocationId {
        &self.revocation_id
    }

    /// Non-secret cleanup handle.
    pub fn cleanup_id(&self) -> &ForkedParticipantCleanupId {
        &self.cleanup_id
    }
}

/// Mechanical evidence that one materialized residency is the host-side
/// realization of exactly one capability attachment.
///
/// This is deliberately NOT lifecycle truth. Attachment lifecycle is owned by
/// the source-owner service and its canonical
/// `ForkedParticipantLifecycleMachine`; this value only lets a host row route
/// its own teardown back to the exact attachment it admitted. It carries the
/// FULL immutable reference rather than a bearer id so a recovered row can be
/// validated (route, fork session, provenance shape) without consulting the
/// capability record, and so `load_exact`'s full-reference comparison — not a
/// bare id lookup — is what a later release presents.
///
/// `Debug` is safe to print: the reference's only secret field redacts itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedParticipantAttachmentAssociation {
    /// Full immutable capability reference the residency was admitted under.
    pub capability: ForkedParticipantRef,
    /// Stable attachment identity admitted for this residency.
    pub attachment_id: ForkedParticipantAttachmentId,
}

impl ForkedParticipantAttachmentAssociation {
    /// Bind one admitted attachment to the residency it materialized.
    pub fn new(
        capability: ForkedParticipantRef,
        attachment_id: ForkedParticipantAttachmentId,
    ) -> Self {
        Self {
            capability,
            attachment_id,
        }
    }

    /// Stable, non-secret key for one association.
    ///
    /// The capability half is a digest correlation hint rather than the bearer
    /// token itself: the key lands in durable host-record map keys and in
    /// operator-visible diagnostics, and neither may leak bearer material.
    pub fn association_key(&self) -> String {
        format!(
            "{}|{}",
            self.capability.capability_id().correlation_hint(),
            self.attachment_id.as_str()
        )
    }
}

/// A source-owner request to create one forked participant capability.
///
/// There is deliberately no tool-access-policy override. Issue #159 requires
/// that tool, auth, realm, and filesystem boundaries remain those of the source
/// execution context, so the branch INHERITS the source's effective policy and
/// the capability layer never supplies a replacement that could broaden it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedParticipantRequest {
    /// Caller-stable idempotency identity. The same exact request may be
    /// retried; a different request may never take a bound identity.
    pub request_id: ForkedParticipantRequestId,
    /// Source member whose conversation is forked.
    pub source_identity: AgentIdentity,
    /// Source session to fork.
    pub source_session_id: SessionId,
    /// Typed route to the owning runtime.
    pub owner_route: ForkedParticipantOwnerRoute,
    /// Complete-boundary prefix length; `None` selects the whole transcript.
    pub prefix_message_count: Option<usize>,
    /// Operations the holder may perform.
    pub scope: ForkedParticipantOperationScope,
    /// One-shot or bounded reuse.
    pub reuse: ForkedParticipantReusePolicy,
    /// Requested time-to-live, validated positive and capped.
    pub ttl: Duration,
}

/// Canonical, explicitly-versioned serialized fingerprint shape.
///
/// Fixed field order, fixed discriminant strings, and integer TTL components:
/// nothing here depends on `Debug`, on enum ordering, or on any renderer that
/// is free to change. Adding, removing, or reordering a field requires bumping
/// [`FORKED_PARTICIPANT_FINGERPRINT_VERSION`].
#[derive(Serialize)]
struct CanonicalFingerprint<'a> {
    fingerprint_version: u32,
    request_id: &'a str,
    source_identity: &'a str,
    source_session_id: String,
    owner_route_kind: &'static str,
    owner_route_realm: &'a str,
    owner_route_host: Option<&'a str>,
    prefix_message_count: Option<u64>,
    scope: &'static str,
    reuse_kind: &'static str,
    reuse_max_uses: u32,
    ttl_seconds: u64,
    ttl_subsec_nanos: u32,
}

impl ForkedParticipantRequest {
    /// Canonical fingerprint of the immutable request shape.
    ///
    /// The lifecycle machine decides identity conflicts by comparing this
    /// value, so it is a persisted contract rather than a debugging
    /// convenience. It covers every field whose change would make the
    /// capability a materially different grant, and a serialization failure is
    /// returned typed rather than defaulted or panicked.
    pub fn fingerprint(&self) -> Result<String, ForkedParticipantFingerprintError> {
        Ok(format!(
            "fpf1:sha256:{:x}",
            Sha256::digest(self.canonical_fingerprint_bytes()?)
        ))
    }

    /// Canonical fingerprint pre-image, exposed for byte-level pin tests.
    pub fn canonical_fingerprint_bytes(
        &self,
    ) -> Result<Vec<u8>, ForkedParticipantFingerprintError> {
        let shape = CanonicalFingerprint {
            fingerprint_version: FORKED_PARTICIPANT_FINGERPRINT_VERSION,
            request_id: self.request_id.as_str(),
            source_identity: self.source_identity.as_str(),
            source_session_id: self.source_session_id.to_string(),
            owner_route_kind: self.owner_route.fingerprint_discriminant(),
            owner_route_realm: self.owner_route.realm_id().as_str(),
            owner_route_host: self.owner_route.fingerprint_host(),
            prefix_message_count: self.prefix_message_count.map(|count| count as u64),
            scope: self.scope.fingerprint_discriminant(),
            reuse_kind: self.reuse.fingerprint_discriminant(),
            reuse_max_uses: self.reuse.max_uses(),
            ttl_seconds: self.ttl.as_secs(),
            ttl_subsec_nanos: self.ttl.subsec_nanos(),
        };
        serde_json::to_vec(&shape).map_err(|source| ForkedParticipantFingerprintError { source })
    }
}

/// Outcome of reserving one capability identity before the fork is taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedParticipantReservation {
    /// Bearer identity minted for the record.
    pub capability_id: ForkedParticipantCapabilityId,
    /// Request the reservation is bound to.
    pub request_id: ForkedParticipantRequestId,
    /// Fingerprint the machine compares on replay.
    pub request_fingerprint: String,
    /// Child session identity reserved before the fork, so a crashed create can
    /// be retried without ever producing a second child.
    pub planned_child_session_id: SessionId,
    /// Whether an existing reservation was replayed rather than created.
    pub replayed: bool,
}

/// Outcome of an attach admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedParticipantGrant {
    /// Attachment the grant was issued to.
    pub attachment_id: ForkedParticipantAttachmentId,
    /// 1-based index of the use this grant consumed.
    pub use_index: u64,
    /// Attachments still available after this grant.
    pub remaining_uses: u64,
    /// True when this was an exact replay of an existing grant (no use was
    /// consumed).
    pub replayed: bool,
    /// Operations the holder may perform.
    pub scope: ForkedParticipantOperationScope,
    /// Forked branch session the holder may drive.
    pub fork_session_id: SessionId,
}

/// Outcome of releasing an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkedParticipantReleaseOutcome {
    /// The capability returned to the detached, usable state.
    Reusable,
    /// The release spent the reuse budget; the capability is exhausted.
    Exhausted,
    /// The release completed a pending revocation.
    Revoked,
    /// The release completed a pending expiry.
    Expired,
    /// The release was an exact replay of an earlier release.
    Replayed,
}

/// Outcome of a revocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkedParticipantRevocationOutcome {
    /// The capability terminalized to revoked while detached.
    Revoked {
        /// Whether the revocation accrued durable cleanup debt.
        cleanup_pending: bool,
    },
    /// The capability holds an attachment; revocation waits for its release.
    PendingAttachedRelease,
    /// An earlier revocation converged; nothing changed.
    Converged,
}

/// Typed cleanup debt retained on a record whose archive failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedParticipantCleanupDebt {
    /// Fork session that must still be archived.
    pub fork_session_id: SessionId,
    /// Number of failed archive attempts.
    pub attempts: u32,
    /// Typed detail of the last failure.
    pub last_error: String,
    /// When the last attempt failed.
    pub observed_at: DateTime<Utc>,
}

/// Non-secret identity of ONE cleanup attempt on ONE record.
///
/// Minted per record attempt, never per service: two concurrent sweeps issued
/// by the same service must be able to fence each other, so a service-wide
/// identity cannot be the authority. It is a uniqueness token, not a bearer
/// secret — it names an attempt, it does not authorize one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ForkedParticipantCleanupAttemptId(String);

impl ForkedParticipantCleanupAttemptId {
    /// Mint a fresh attempt identity.
    pub fn mint() -> Result<Self, meerkat_core::secret_entropy::SecretEntropyError> {
        Ok(Self(format!(
            "fpa:{}",
            meerkat_core::secret_entropy::secret_entropy_hex::<16>()?
        )))
    }

    /// Borrow the raw attempt identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ForkedParticipantCleanupAttemptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Mechanical, crash-recoverable exclusive claim on one record's cleanup.
///
/// This is not lifecycle authority: terminal cleanup completion stays
/// machine-owned. The claim exists only so two concurrent sweepers cannot both
/// archive the same fork, and so a sweeper that dies mid-archive does not park
/// the record forever — a stale claim is reclaimable after
/// [`FORKED_PARTICIPANT_CLEANUP_CLAIM_TTL`].
///
/// The claim names an ATTEMPT, so a sweeper whose claim was taken over cannot
/// publish a late outcome: its attempt id no longer matches the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedParticipantCleanupClaim {
    /// Attempt currently holding the claim.
    pub attempt_id: ForkedParticipantCleanupAttemptId,
    /// When the claim was taken.
    pub claimed_at: DateTime<Utc>,
}

/// Proof that one attempt holds the cleanup claim on one record.
///
/// A lease is required to publish any cleanup outcome. Publishing re-reads the
/// record and re-checks the attempt id, so a lease whose claim was taken over
/// in the meantime publishes nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedParticipantCleanupLease {
    capability_id: ForkedParticipantCapabilityId,
    attempt_id: ForkedParticipantCleanupAttemptId,
    claimed_at: DateTime<Utc>,
    claim_revision: u64,
}

impl ForkedParticipantCleanupLease {
    /// Mint a lease for a claim this owner just committed.
    pub(crate) fn new_owned(
        capability_id: ForkedParticipantCapabilityId,
        attempt_id: ForkedParticipantCleanupAttemptId,
        claimed_at: DateTime<Utc>,
        claim_revision: u64,
    ) -> Self {
        Self {
            capability_id,
            attempt_id,
            claimed_at,
            claim_revision,
        }
    }

    /// Record the lease fences.
    pub fn capability_id(&self) -> &ForkedParticipantCapabilityId {
        &self.capability_id
    }

    /// Attempt the lease proves.
    pub fn attempt_id(&self) -> &ForkedParticipantCleanupAttemptId {
        &self.attempt_id
    }

    /// When the claim was taken.
    pub fn claimed_at(&self) -> DateTime<Utc> {
        self.claimed_at
    }

    /// Record revision the claim was committed at.
    pub fn claim_revision(&self) -> u64 {
        self.claim_revision
    }
}

/// Outcome of trying to take the mechanical cleanup claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkedParticipantCleanupClaimOutcome {
    /// The claim is held by this attempt.
    Claimed(ForkedParticipantCleanupLease),
    /// Another live attempt currently holds the claim.
    ClaimedElsewhere,
    /// The record no longer carries machine-owned cleanup debt, so there is
    /// nothing to claim. A completed or non-pending record is never claimable.
    NotPending,
}

/// Outcome of publishing a cleanup result under a lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkedParticipantCleanupPublish<T> {
    /// The lease still held the claim; the outcome was published.
    Published(T),
    /// The claim was taken over before the outcome could be published, so
    /// nothing was written. A superseded attempt never records false debt and
    /// never completes a cleanup another attempt owns.
    ClaimLost,
}

/// How long a cleanup claim is honored before another sweeper may reclaim it.
pub const FORKED_PARTICIPANT_CLEANUP_CLAIM_TTL: Duration = Duration::from_secs(300);

/// What a durable capability record says about one fork child session.
///
/// Returned by the containment lookup, so a surface that can address sessions
/// by id can decide "is this session capability-protected" from OWNER truth
/// rather than from anything the caller supplied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedParticipantForkProtection {
    /// Non-secret correlation handle of the owning capability. Safe to log.
    pub capability_hint: String,
    /// Immutable route of the owning capability.
    pub owner_route: ForkedParticipantOwnerRoute,
    /// The activated immutable reference.
    ///
    /// `None` means the record is still in its reserved/planned window: the
    /// session is protected, but no reference exists yet, so NOTHING can
    /// authenticate against it and every request naming the session must be
    /// refused rather than served.
    pub capability: Option<ForkedParticipantRef>,
}

/// Which terminal a capability is waiting on while an attachment is held.
///
/// Both terminals are *recorded* facts on the lifecycle machine — the machine
/// accepted the terminal and parked it because an attachment is active. This
/// enum is a typed read of that parked phase, never an inference from text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ForkedParticipantPendingTerminal {
    /// Expiry was observed while attached.
    Expiry,
    /// Revocation was requested while attached.
    Revocation,
}

impl ForkedParticipantPendingTerminal {
    /// Fixed, non-secret label for structured diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Expiry => "expiry",
            Self::Revocation => "revocation",
        }
    }
}

/// One capability whose recorded terminal is blocked behind a live attachment.
///
/// This is what lets an owner converge autonomously after the coordinator that
/// took the attachment disappears: it carries the FULL immutable reference and
/// the exact active attachment id, which together are the only thing a host may
/// correlate its own durable residency rows against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedParticipantPendingAttachment {
    /// Full immutable capability reference.
    pub capability: ForkedParticipantRef,
    /// The exact attachment the machine still holds active.
    pub attachment_id: ForkedParticipantAttachmentId,
    /// Terminal the capability is parked on.
    pub terminal: ForkedParticipantPendingTerminal,
}

impl ForkedParticipantPendingAttachment {
    /// The association a host row must carry to own this attachment.
    #[must_use]
    pub fn association(&self) -> ForkedParticipantAttachmentAssociation {
        ForkedParticipantAttachmentAssociation::new(
            self.capability.clone(),
            self.attachment_id.clone(),
        )
    }
}

/// Report of one pending-attached enumeration.
///
/// A record this owner cannot read into a typed pending entry is REPORTED, not
/// dropped: silently skipping it would let one corrupt row hide a capability
/// that will never converge. One unreadable record likewise never aborts the
/// enumeration — the remaining pending terminals must still be servable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkedParticipantPendingAttachmentReport {
    /// Capabilities parked on a terminal behind a live attachment.
    pub pending: Vec<ForkedParticipantPendingAttachment>,
    /// Records whose parked phase could not be read into a typed entry.
    pub unreadable: Vec<(ForkedParticipantCapabilityId, String)>,
}

/// One entry in a sweep report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedParticipantSweepEntry {
    /// Capability the entry describes.
    pub capability_id: ForkedParticipantCapabilityId,
    /// Fork session the entry describes, when the record has one.
    pub fork_session_id: Option<SessionId>,
}

/// Report of one cleanup sweep.
///
/// A sweep never aborts on the first failure: a record whose archive fails
/// keeps its typed debt, a record another sweeper holds is skipped, a record
/// whose compare-and-swap loses is reported, and the sweep continues with the
/// remaining records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkedParticipantCleanupReport {
    /// Records whose fork session was archived and whose cleanup completed.
    pub completed: Vec<ForkedParticipantSweepEntry>,
    /// Records whose archive failed; their typed debt was persisted.
    pub retained: Vec<(ForkedParticipantSweepEntry, ForkedParticipantCleanupDebt)>,
    /// Records another sweeper currently holds.
    pub claimed_elsewhere: Vec<ForkedParticipantSweepEntry>,
    /// Records this sweep could not durably advance (typed detail retained).
    pub failed: Vec<(ForkedParticipantSweepEntry, String)>,
}

/// Report of one expiry sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkedParticipantExpirySweepReport {
    /// Records terminalized to expired while detached.
    pub expired: Vec<ForkedParticipantSweepEntry>,
    /// Records whose expiry is recorded but blocked behind an attachment.
    pub expiry_pending_attached: Vec<ForkedParticipantSweepEntry>,
    /// Records this sweep could not durably advance (typed detail retained).
    pub failed: Vec<(ForkedParticipantSweepEntry, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_session(byte: u8) -> SessionId {
        SessionId::from_uuid(uuid::Uuid::from_bytes([byte; 16]))
    }

    fn request() -> ForkedParticipantRequest {
        ForkedParticipantRequest {
            request_id: ForkedParticipantRequestId::new("req-1").expect("request id"),
            source_identity: AgentIdentity::from("researcher"),
            source_session_id: fixed_session(0x11),
            owner_route: ForkedParticipantOwnerRoute::Local {
                realm_id: RealmId::parse("global").expect("realm"),
            },
            prefix_message_count: Some(4),
            scope: ForkedParticipantOperationScope::InvokeAndObserve,
            reuse: ForkedParticipantReusePolicy::OneShot,
            ttl: Duration::from_millis(600_500),
        }
    }

    #[test]
    fn capability_id_debug_is_redacted_and_correlation_hint_is_a_digest() {
        let id = ForkedParticipantCapabilityId::mint().expect("mint");
        let rendered = format!("{id:?}");
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
        assert!(!rendered.contains(id.expose_bearer_token()));

        let hint = id.correlation_hint();
        assert!(hint.starts_with("fpc:"));
        assert!(
            !id.expose_bearer_token().starts_with(&hint[4..]),
            "the correlation hint must be a digest, never a bearer prefix"
        );
        assert_eq!(hint.len(), 4 + 12);
    }

    #[test]
    fn capability_bearer_parsing_fails_closed() {
        let valid = ForkedParticipantCapabilityId::mint().expect("mint");
        assert_eq!(
            ForkedParticipantCapabilityId::parse_bearer_token(valid.expose_bearer_token())
                .expect("valid token parses"),
            valid
        );

        for malformed in [
            "",
            "short",
            &"a".repeat(63),
            &"a".repeat(65),
            &"A".repeat(64),
            &"g".repeat(64),
            &format!("{} ", "a".repeat(63)),
        ] {
            assert_eq!(
                ForkedParticipantCapabilityId::parse_bearer_token(malformed),
                Err(ForkedParticipantIdentityError::MalformedBearerToken),
                "`{malformed}` must not parse as a bearer token"
            );
            assert!(
                serde_json::from_value::<ForkedParticipantCapabilityId>(serde_json::json!(
                    malformed
                ))
                .is_err(),
                "`{malformed}` must not deserialize"
            );
        }
    }

    #[test]
    fn caller_identities_are_validated_at_construction_and_deserialization() {
        assert_eq!(
            ForkedParticipantRequestId::new("  spaced  ")
                .expect("trimmed")
                .as_str(),
            "spaced"
        );
        assert!(matches!(
            ForkedParticipantRequestId::new("   "),
            Err(ForkedParticipantIdentityError::Empty { .. })
        ));
        assert!(matches!(
            ForkedParticipantRequestId::new("a".repeat(MAX_FORKED_PARTICIPANT_REQUEST_ID_LEN + 1)),
            Err(ForkedParticipantIdentityError::TooLong { .. })
        ));
        assert!(matches!(
            ForkedParticipantAttachmentId::new("bad\nid"),
            Err(ForkedParticipantIdentityError::ControlCharacter { .. })
        ));
        assert!(
            serde_json::from_value::<ForkedParticipantAttachmentId>(serde_json::json!(
                "bad\u{7}id"
            ))
            .is_err(),
            "a control character must not survive deserialization"
        );
        assert!(
            serde_json::from_value::<ForkedParticipantRequestId>(serde_json::json!("")).is_err()
        );
    }

    #[test]
    fn canonical_fingerprint_bytes_are_pinned() {
        let bytes = request()
            .canonical_fingerprint_bytes()
            .expect("fingerprint bytes");
        let rendered = String::from_utf8(bytes).expect("utf8");
        assert_eq!(
            rendered,
            concat!(
                r#"{"fingerprint_version":1,"#,
                r#""request_id":"req-1","#,
                r#""source_identity":"researcher","#,
                r#""source_session_id":"11111111-1111-1111-1111-111111111111","#,
                r#""owner_route_kind":"local","#,
                r#""owner_route_realm":"global","#,
                r#""owner_route_host":null,"#,
                r#""prefix_message_count":4,"#,
                r#""scope":"invoke_and_observe","#,
                r#""reuse_kind":"one_shot","#,
                r#""reuse_max_uses":1,"#,
                r#""ttl_seconds":600,"#,
                r#""ttl_subsec_nanos":500000000}"#
            ),
            "the canonical fingerprint pre-image is a persisted contract"
        );
    }

    #[test]
    fn fingerprint_digest_is_pinned() {
        assert_eq!(
            request().fingerprint().expect("fingerprint"),
            "fpf1:sha256:b20eb9ed731eef54c5eb2c23b2c0b8eb68362050c3753a77d09f21267a8d9195"
        );
    }

    #[test]
    fn fingerprint_uses_fixed_discriminants_not_debug_renderings() {
        let base = request();
        let rendered =
            String::from_utf8(base.canonical_fingerprint_bytes().expect("bytes")).expect("utf8");

        // The pre-image must carry the fixed discriminants. If the fingerprint
        // were derived from `Debug`, these Rust-shaped renderings would appear
        // instead, and any unrelated Debug edit would silently rewrite the
        // persisted contract.
        for debug_shaped in [
            "Local {",
            "InvokeAndObserve",
            "OneShot",
            "RealmId(",
            "Duration",
        ] {
            assert!(
                !rendered.contains(debug_shaped),
                "fingerprint pre-image must not contain the Debug rendering `{debug_shaped}`"
            );
        }
        assert!(rendered.contains(r#""owner_route_kind":"local""#));
        assert!(rendered.contains(r#""scope":"invoke_and_observe""#));
        assert!(rendered.contains(r#""reuse_kind":"one_shot""#));
    }

    #[test]
    fn fingerprint_covers_every_material_field() {
        let base = request();
        let baseline = base.fingerprint().expect("fingerprint");
        assert_eq!(baseline, base.clone().fingerprint().expect("stable"));

        let mut scope_changed = base.clone();
        scope_changed.scope = ForkedParticipantOperationScope::Observe;
        assert_ne!(baseline, scope_changed.fingerprint().expect("fingerprint"));

        let mut reuse_changed = base.clone();
        reuse_changed.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 3 };
        assert_ne!(baseline, reuse_changed.fingerprint().expect("fingerprint"));

        // A bounded-reuse budget of 1 is still a different grant than one-shot.
        let mut bounded_one = base.clone();
        bounded_one.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 1 };
        assert_ne!(baseline, bounded_one.fingerprint().expect("fingerprint"));

        let mut ttl_changed = base.clone();
        ttl_changed.ttl = Duration::from_millis(600_501);
        assert_ne!(
            baseline,
            ttl_changed.fingerprint().expect("fingerprint"),
            "sub-second TTL differences must change the fingerprint"
        );

        let mut prefix_changed = base.clone();
        prefix_changed.prefix_message_count = None;
        assert_ne!(baseline, prefix_changed.fingerprint().expect("fingerprint"));

        let mut route_changed = base.clone();
        route_changed.owner_route = ForkedParticipantOwnerRoute::Host {
            realm_id: RealmId::parse("global").expect("realm"),
            host_id: HostId::from("host-a"),
        };
        assert_ne!(baseline, route_changed.fingerprint().expect("fingerprint"));

        let mut identity_changed = base;
        identity_changed.source_identity = AgentIdentity::from("other");
        assert_ne!(
            baseline,
            identity_changed.fingerprint().expect("fingerprint")
        );
    }
}
