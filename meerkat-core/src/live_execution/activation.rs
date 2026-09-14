//! Trusted activation declarations, separate from writable model profiles.
//!
//! These serializable values are requests to the trusted issuer, not grants,
//! run authority, or effect-start permits. The member owner supplies its own
//! canonical selector type; core never duplicates or parses mob identity.

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU32, NonZeroU64};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::profile::LiveProfileId;
use super::request::LiveRequestEvidenceKind;
use crate::connection::RealmId;
use crate::{SessionId, ToolMutationClass, ToolNameSet};

/// Stable key in a trusted activation document, not an auth binding name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveActivationId(LiveProfileId);

impl LiveActivationId {
    pub fn parse(
        value: impl Into<String>,
    ) -> Result<Self, super::profile::LiveProfileDeclarationError> {
        LiveProfileId::parse(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// A definition revision is an exact digest, not a monotonically guessed
/// counter. It binds the entire selected profile, including voice guidance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveProfileRevision([u8; 32]);

impl LiveProfileRevision {
    pub fn of(
        definition: &super::profile::LiveProfileDefinition,
    ) -> Result<Self, serde_json::Error> {
        use sha2::{Digest, Sha256};
        let encoded = serde_json::to_vec(definition)?;
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-profile.v1\0");
        digest.update(encoded);
        Ok(Self(digest.finalize().into()))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveExecutorSelector<Member> {
    Session { session_id: SessionId },
    MobMember { member: Member },
}

/// Host-declared upper bound. Ordinary tool policy always intersects this
/// ceiling at the generated claim seam; this value cannot override it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveWorkPermission {
    pub allowed_mutations: BTreeSet<ToolMutationClass>,
    pub tools: LiveToolRestriction,
    pub limits: LiveWorkLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveToolRestriction {
    AllowListed { names: ToolNameSet },
    Unrestricted {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LiveWorkLimitsWire")]
pub struct LiveWorkLimits {
    max_requests: NonZeroU32,
    max_concurrent_requests: NonZeroU32,
    max_effects_per_request: NonZeroU32,
    max_tokens_per_request: NonZeroU64,
    max_duration_ms: NonZeroU64,
}

impl LiveWorkLimits {
    pub fn new(
        max_requests: NonZeroU32,
        max_concurrent_requests: NonZeroU32,
        max_effects_per_request: NonZeroU32,
        max_tokens_per_request: NonZeroU64,
        max_duration_ms: NonZeroU64,
    ) -> Result<Self, LiveActivationDeclarationError> {
        if max_concurrent_requests > max_requests {
            return Err(LiveActivationDeclarationError::ConcurrencyExceedsRequests);
        }
        Ok(Self {
            max_requests,
            max_concurrent_requests,
            max_effects_per_request,
            max_tokens_per_request,
            max_duration_ms,
        })
    }

    pub const fn max_requests(self) -> NonZeroU32 {
        self.max_requests
    }

    pub const fn max_concurrent_requests(self) -> NonZeroU32 {
        self.max_concurrent_requests
    }

    pub const fn max_effects_per_request(self) -> NonZeroU32 {
        self.max_effects_per_request
    }

    /// Observed normalized-token budget. Reaching it refuses subsequent model
    /// claims, not an already-completed answer. Missing usage advances no known
    /// counter, so this is not a hard ceiling on actual or billed tokens.
    pub const fn max_tokens_per_request(self) -> NonZeroU64 {
        self.max_tokens_per_request
    }

    pub const fn max_duration_ms(self) -> NonZeroU64 {
        self.max_duration_ms
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveWorkLimitsWire {
    max_requests: NonZeroU32,
    max_concurrent_requests: NonZeroU32,
    max_effects_per_request: NonZeroU32,
    max_tokens_per_request: NonZeroU64,
    max_duration_ms: NonZeroU64,
}

impl TryFrom<LiveWorkLimitsWire> for LiveWorkLimits {
    type Error = LiveActivationDeclarationError;

    fn try_from(value: LiveWorkLimitsWire) -> Result<Self, Self::Error> {
        Self::new(
            value.max_requests,
            value.max_concurrent_requests,
            value.max_effects_per_request,
            value.max_tokens_per_request,
            value.max_duration_ms,
        )
    }
}

/// Missing entries and explicit Inherit select the nearest defining owner.
/// Disable blocks inheritance. Set replaces the whole declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    content = "declaration",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LiveActivationEntry<Member> {
    Inherit,
    Disable,
    Set(Box<LiveActivationDeclaration<Member>>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveActivationDeclaration<Member> {
    pub issuer_realm: RealmId,
    pub profile_id: LiveProfileId,
    pub profile_revision: LiveProfileRevision,
    pub requesting_realms: BTreeSet<RealmId>,
    pub executor: LiveExecutorSelector<Member>,
    pub allowed_evidence: BTreeSet<LiveRequestEvidenceKind>,
    pub permission: LiveWorkPermission,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub generation: NonZeroU64,
    pub revoke_policy: LiveRevokePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveRevokePolicy {
    CancelPendingAndRequestRunningCancellation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "Member: Deserialize<'de>"))]
pub struct LiveActivationDocument<Member> {
    #[serde(default)]
    pub activations: BTreeMap<LiveActivationId, LiveActivationEntry<Member>>,
}

impl<Member> Default for LiveActivationDocument<Member> {
    fn default() -> Self {
        Self {
            activations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveActivationDeclarationError {
    #[error("live activation concurrency exceeds its total request bound")]
    ConcurrencyExceedsRequests,
}
