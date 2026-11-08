//! Immutable native work association data, never accepted admission or permission.
//!
//! The generated input owner binds this complete value to its actual InputId.
//! Decoding and canonical encoding cannot establish authentication, currentness,
//! source custody, grant issuance or replay acceptance. The transport must impose
//! its own byte limit before decoding; the bounds here also apply to local data.

use std::{fmt, sync::Arc};

use meerkat_core::auth::PrincipalRef;
use meerkat_core::connection::RealmId;
use serde::{Deserialize, Serialize};

use crate::constraints::{AudienceRef, ExecutionRestrictions, ResourceDomain};
use crate::evidence::{EvidenceId, HistoricalEvidenceRef};
use crate::protocol::ContractRequirements;

pub const MAX_ASSOCIATION_BYTES: usize = 32 * 1024;
pub const MAX_ASSOCIATION_REFERENCES: usize = 64;

/// Compatibility path for the single canonical issued-grant reference.
/// This re-export does not create a second reference or an authority owner.
pub use crate::grant::GrantLineageRef;

/// Logical destination and its audience. A current physical incarnation is
/// deliberately absent: recovery cannot rewrite original work provenance.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeWorkTarget {
    pub logical_owner: PrincipalRef,
    pub logical_runtime: EvidenceId,
    pub context: EvidenceId,
    pub context_generation: u64,
    pub audience: AudienceRef,
}

/// A complete source-owned original work reference, not a native InputId.
/// Native contributing InputIds are selected separately by the input owner.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalWorkRef {
    pub authority: PrincipalRef,
    pub work: EvidenceId,
}

/// Requester and target are taken from the same immutable association when a
/// durable key is constructed. They are not duplicated in this namespace.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualifiedIngressNamespace {
    pub realm: RealmId,
    pub ingress: ResourceDomain,
    pub occurrence_scope: EvidenceId,
}

/// Which canonical owner must resolve authority for this original work.
///
/// Every variant is a historical claim, not a permission. Native admission must
/// bind the variant and its exact references to the authenticated ingress and
/// actual input, policy or schedule/connector owner. A caller cannot choose
/// `HostPolicy` or `ServiceMandate` to bypass grant checks. Every later use needs
/// current authority, restrictions and deadlines from that same owner.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkAuthorityBasis {
    GrantLineage {
        lineage: Vec<GrantLineageRef>,
    },
    HostPolicy {
        policy: HistoricalEvidenceRef,
    },
    /// The actual schedule/connector retains the scoped mandate, binds each
    /// occurrence and checks its current authority. No service registry or
    /// authenticated mandate is constructed by decoding this value.
    ServiceMandate {
        mandate: HistoricalEvidenceRef,
        commissioning_actor: PrincipalRef,
        occurrence: EvidenceId,
    },
    /// Exact grant lineages selected per typed operation target. Each value is
    /// one complete root-to-leaf lineage resolved on its own, at use, for
    /// operations on that target. `base`, when selected, is the optional
    /// lineage of operations without a typed target (for example a catalog
    /// read); it is never admission authority and never a fallback for a
    /// target. Lineages of different targets are alternatives: they are never
    /// concatenated, combined or intersected. A target without a selected
    /// lineage has none, whatever any other target or `base` holds. Selecting
    /// nothing at all is controller-only work: no operation has a lineage.
    TargetedGrantLineages {
        base: Option<Vec<GrantLineageRef>>,
        targets: Vec<TargetGrantLineage>,
    },
}

/// The typed target an operation's grant lineage is selected by.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum GrantTarget {
    /// One registered MCP server, by its configured name.
    McpServer { server: String },
    /// One operation of the Rust tool bundle a trusted host registered
    /// under the stable name `bundle`: the target of the owner-produced
    /// `RegisteredToolBundleOperation`. Each operation is its own target, so
    /// its lineage expires and is revoked on its own.
    RegisteredToolBundleOperation { bundle: String, operation: String },
    /// The raw operations of the registered MCP server `server` reached
    /// through the checked route a trusted host registered under the stable
    /// name `route`: the target of the owner-produced
    /// `CheckedMcpServerOperation`. Its lineage never covers the server's
    /// ordinary operations, and the server's lineage never covers it.
    CheckedMcpRoute { route: String, server: String },
    /// Deliveries to exactly one peer, by its canonical identity (the comms
    /// routing and trust key, never a display name): the target of the
    /// owner-produced `PeerSend`. Each peer is its own target: one peer's
    /// lineage never covers another peer, the peer directory or the base,
    /// and the base never covers a delivery. It covers only the comms
    /// tools' deliveries (`send_message`, `reply_to_peer`, `send_request`
    /// and `send_response`): other tools that reach agents, such as
    /// `delegate`, `council`, `mob_spawn_member` and a mob member's calls up
    /// to its mob's operator, carry no peer target and stay on the base
    /// lineage, which a peer's lineage neither widens nor narrows. A
    /// `send_response` delivers only to the requester of the request it
    /// answers; the runtime refuses any other addressee before transport.
    /// Holds when every wrapper between the root and the owner
    /// forwards plan resolution and resolved dispatch with its context; a
    /// governed comms plan without the owner's target is refused. Not an
    /// egress fence for mob members (see the changelog).
    Peer { peer: meerkat_core::comms::PeerId },
    /// Reads of the session's own trusted-peer directory: the target of the
    /// owner-produced `PeerDirectoryRead`. Its lineage covers no delivery.
    PeerDirectory,
    /// Operations on exactly one memory scope, as its memory owner names
    /// it: the target of the owner-produced `MemoryOperation`. Each scope is
    /// its own target; the base never covers a memory operation. Holds on
    /// the same forwarding as `Peer`.
    Memory {
        scope: meerkat_core::memory::MemoryScopeRef,
    },
    /// One builtin tool operation: the target of the owner-produced
    /// `BuiltinOperation` (the builtin web_search call). Its lineage covers
    /// only that operation, never the base, the catalog or the controller.
    BuiltinToolOperation {
        operation: meerkat_core::BuiltinToolOperation,
    },
    /// The hosted-search helper of a governed builtin web_search call, on
    /// the helper's own lowered model route `route` (its canonical
    /// `ControllerRouteKey`) with the hosted search `capability` its request
    /// enables. Selected only for a model operation attributed as that
    /// call's delegate. Another route or capability, the controller's
    /// lineage and an ordinary inference never use it.
    DelegatedHostedSearch {
        route: String,
        capability: meerkat_core::ServerToolKind,
    },
}

/// One target's selected exact lineage.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetGrantLineage {
    pub target: GrantTarget,
    pub lineage: Vec<GrantLineageRef>,
}

fn validate_lineage(lineage: &[GrantLineageRef]) -> Result<(), AssociationError> {
    if lineage.is_empty() || lineage.len() > MAX_ASSOCIATION_REFERENCES {
        return Err(AssociationError::Shape);
    }
    for grant in lineage {
        grant.validate()?;
    }
    Ok(())
}

impl WorkAuthorityBasis {
    fn validate(&self) -> Result<(), AssociationError> {
        match self {
            Self::GrantLineage { lineage } => validate_lineage(lineage)?,
            Self::TargetedGrantLineages { base, targets } => {
                if let Some(base) = base {
                    validate_lineage(base)?;
                }
                // Selecting nothing is valid: controller-only work holds no
                // operation grant, so every operation is refused while the
                // separately admitted controller lineage still applies.
                if targets.len() > MAX_ASSOCIATION_REFERENCES {
                    return Err(AssociationError::Shape);
                }
                // One canonical form: targets strictly ordered, so each target
                // occurs once and equal selections have equal bytes.
                if targets
                    .windows(2)
                    .any(|pair| pair[0].target >= pair[1].target)
                {
                    return Err(AssociationError::Shape);
                }
                for selected in targets {
                    let named = match &selected.target {
                        GrantTarget::McpServer { server } => !server.is_empty(),
                        GrantTarget::RegisteredToolBundleOperation { bundle, operation } => {
                            !bundle.is_empty() && !operation.is_empty()
                        }
                        GrantTarget::CheckedMcpRoute { route, server } => {
                            !route.is_empty() && !server.is_empty()
                        }
                        // A peer is named by its typed identity; the peer
                        // directory is the session's own.
                        GrantTarget::Peer { .. } | GrantTarget::PeerDirectory => true,
                        GrantTarget::Memory { scope } => scope.is_named(),
                        GrantTarget::BuiltinToolOperation { .. } => true,
                        GrantTarget::DelegatedHostedSearch { route, capability } => {
                            meerkat_core::ControllerRouteKey::parse(route).is_some()
                                && matches!(
                                    capability,
                                    meerkat_core::ServerToolKind::WebSearch
                                        | meerkat_core::ServerToolKind::GoogleSearch
                                )
                        }
                    };
                    if !named {
                        return Err(AssociationError::Shape);
                    }
                    validate_lineage(&selected.lineage)?;
                }
            }
            Self::HostPolicy { policy } => qualified(&policy.resource.domain.authority)?,
            Self::ServiceMandate {
                mandate,
                commissioning_actor,
                ..
            } => {
                qualified(&mandate.resource.domain.authority)?;
                qualified(commissioning_actor)?;
            }
        }
        Ok(())
    }

    /// The exact grant lineage this basis selects for an operation on
    /// `target` (`None` for an operation without a typed target). A single
    /// `GrantLineage` is the lineage of every operation. `None` when the
    /// basis is not a grant lineage, or selected none for that target (or no
    /// optional base): a missing target never falls back to the base.
    pub fn lineage_for(&self, target: Option<&GrantTarget>) -> Option<&[GrantLineageRef]> {
        match (self, target) {
            (Self::GrantLineage { lineage }, _) => Some(lineage),
            (Self::TargetedGrantLineages { base, .. }, None) => base.as_deref(),
            (Self::TargetedGrantLineages { targets, .. }, Some(target)) => targets
                .iter()
                .find(|selected| &selected.target == target)
                .map(|selected| selected.lineage.as_slice()),
            (Self::HostPolicy { .. } | Self::ServiceMandate { .. }, _) => None,
        }
    }

    /// Every grant lineage this basis holds, each complete and separate.
    pub fn grant_lineages(&self) -> impl Iterator<Item = &[GrantLineageRef]> {
        let (first, targets): (Option<&[GrantLineageRef]>, &[TargetGrantLineage]) = match self {
            Self::GrantLineage { lineage } => (Some(lineage), &[]),
            Self::TargetedGrantLineages { base, targets } => (base.as_deref(), targets),
            Self::HostPolicy { .. } | Self::ServiceMandate { .. } => (None, &[]),
        };
        first
            .into_iter()
            .chain(targets.iter().map(|selected| selected.lineage.as_slice()))
    }
}

/// All fields are claims. The real verified ingress and joined native admission
/// must bind them before any acknowledgment, hydration or protected processing.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputAuthorityAssociationCandidate {
    pub requester: PrincipalRef,
    pub ingress_actor: PrincipalRef,
    /// Principal represented by the logical executor for this work. This is a
    /// claim the native owner must authenticate against the actual requester
    /// and scoped mandate. An explicit null never selects a credential or agent
    /// owner. The wire field must be present even when its value is null.
    #[serde(deserialize_with = "Option::deserialize")]
    pub represented_subject: Option<PrincipalRef>,
    pub original_authentication: HistoricalEvidenceRef,
    pub logical_executor: PrincipalRef,
    pub target: NativeWorkTarget,
    pub original_work: OriginalWorkRef,
    pub root_event: HistoricalEvidenceRef,
    pub contributing_work: Vec<OriginalWorkRef>,
    pub authority_basis: WorkAuthorityBasis,
    /// Historical controller entitlement, independent of operation grants.
    /// The native owner verifies the complete root-to-leaf path at admission.
    /// An empty path never supplies governed controller authority.
    pub controller_grant_lineage: Vec<GrantLineageRef>,
    /// Exact admitted controller route, projected from the actual selected
    /// client. This is historical identity data, never a runnable client or
    /// permission; governed native admission requires a verified selection.
    /// The wire field must be present even when its candidate value is null.
    #[serde(deserialize_with = "Option::deserialize")]
    pub controller_model: Option<meerkat_core::ControllerModelSelection>,
    /// Independently admitted controller ceiling. This does not grant a model
    /// route; the actual current policy owner must still allow that operation.
    pub controller_ceiling: ExecutionRestrictions,
    pub admitted_ceiling: ExecutionRestrictions,
    /// Actual source observations retained for audit. This is not a complete
    /// dependency inventory, a permission, or a restriction on derived content.
    pub source_observations: Vec<HistoricalEvidenceRef>,
    pub ingress_namespace: QualifiedIngressNamespace,
    pub contract: ContractRequirements,
}

/// Bounded immutable association data. This type intentionally carries no
/// "accepted" status and can be constructed by an untrusted caller. The actual
/// generated source-bound preparation, not this wrapper, confers admission.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct InputAuthorityAssociation(Arc<InputAuthorityAssociationCandidate>);

impl InputAuthorityAssociation {
    /// Bound an immutable candidate without accepting it as native work.
    ///
    /// # Errors
    /// Refuses unsupported contracts, unqualified identities or excessive data.
    pub fn new(candidate: InputAuthorityAssociationCandidate) -> Result<Self, AssociationError> {
        for principal in [
            &candidate.requester,
            &candidate.ingress_actor,
            &candidate.logical_executor,
            &candidate.target.logical_owner,
            &candidate.original_work.authority,
            &candidate.ingress_namespace.ingress.authority,
        ] {
            qualified(principal)?;
        }
        if let Some(subject) = &candidate.represented_subject {
            qualified(subject)?;
        }
        match &candidate.target.audience {
            AudienceRef::Principal { principal } => qualified(principal)?,
            AudienceRef::Destination { authority, .. } => qualified(authority)?,
        }
        if candidate.contract != ContractRequirements::local_governed_v1()
            || candidate.contributing_work.len() > MAX_ASSOCIATION_REFERENCES
            || candidate.controller_grant_lineage.len() > MAX_ASSOCIATION_REFERENCES
            || candidate.source_observations.len() > MAX_ASSOCIATION_REFERENCES
        {
            return Err(AssociationError::Shape);
        }
        for work in &candidate.contributing_work {
            qualified(&work.authority)?;
        }
        candidate.authority_basis.validate()?;
        for grant in &candidate.controller_grant_lineage {
            grant.validate()?;
        }
        for evidence in [&candidate.original_authentication, &candidate.root_event]
            .into_iter()
            .chain(candidate.source_observations.iter())
        {
            qualified(&evidence.resource.domain.authority)?;
        }
        // The order of contributor, grant and source references is retained.
        // It is not silently sorted or deduplicated during admission/recovery.
        let association = Self(Arc::new(candidate));
        association.canonical_bytes()?;
        Ok(association)
    }

    #[must_use]
    pub fn candidate(&self) -> &InputAuthorityAssociationCandidate {
        &self.0
    }

    /// Equality of the authority participants that may share one native turn.
    /// Authentication/event references and restriction ceilings may differ;
    /// every original association must still be retained and checked at use.
    #[must_use]
    pub fn batch_compatible_with(&self, other: &Self) -> bool {
        self.0.requester == other.0.requester
            && self.0.represented_subject == other.0.represented_subject
            && self.0.logical_executor == other.0.logical_executor
            && self.0.ingress_namespace.realm == other.0.ingress_namespace.realm
            && self.0.target == other.0.target
            && self.0.controller_model == other.0.controller_model
    }

    /// Exact opaque equality key for generated native batch selection. This is
    /// a lossless encoding of the same typed relation, never an access token.
    ///
    /// # Errors
    /// Refuses an unsupported encoding or excessive data.
    pub fn batch_identity_bytes(&self) -> Result<Vec<u8>, AssociationError> {
        canonical(
            b"meerkat.input-authority-batch.v1\0",
            &(
                &self.0.requester,
                &self.0.represented_subject,
                &self.0.logical_executor,
                &self.0.ingress_namespace.realm,
                &self.0.target,
                &self.0.controller_model,
            ),
            MAX_ASSOCIATION_BYTES,
        )
    }

    /// Exact protected binding bytes, not a permission or currentness token.
    /// The V1 codec has explicit tags/lengths and UTF-8 byte-sorted object keys.
    /// Integer width, sequence order and all named fields are part of its meaning.
    ///
    /// # Errors
    /// Refuses excessive size or data outside the declared V1 encoding.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AssociationError> {
        canonical(
            b"meerkat.input-authority-association.v1\0",
            &self.0,
            MAX_ASSOCIATION_BYTES,
        )
    }

    /// This association's [`AssociationDigest`]: SHA-256 over
    /// [`Self::canonical_bytes`], under [`ASSOCIATION_DIGEST_VERSION`]. It is
    /// the one digest function a governed submission's receipt names its
    /// association by; hosts call it and never compute their own.
    ///
    /// # Errors
    /// As [`Self::canonical_bytes`].
    pub fn digest(&self) -> Result<AssociationDigest, AssociationError> {
        use sha2::Digest as _;
        let bytes = self.canonical_bytes()?;
        Ok(AssociationDigest {
            sha256: sha2::Sha256::digest(&bytes).into(),
        })
    }

    /// Complete qualified native index data. Hashing it may choose an index
    /// bucket, but actual replay must compare every field and the full exact
    /// association/content/semantic binding separately. A key is not read access.
    #[must_use]
    pub fn qualified_key(&self, event_key: EvidenceId) -> QualifiedInputKey {
        QualifiedInputKey {
            namespace: self.0.ingress_namespace.clone(),
            requester: self.0.requester.clone(),
            target: self.0.target.clone(),
            event_key,
        }
    }
}

impl<'de> Deserialize<'de> for InputAuthorityAssociation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(InputAuthorityAssociationCandidate::deserialize(
            deserializer,
        )?)
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualifiedInputKey {
    pub namespace: QualifiedIngressNamespace,
    pub requester: PrincipalRef,
    pub target: NativeWorkTarget,
    pub event_key: EvidenceId,
}

impl QualifiedInputKey {
    /// Encode exact protected index data without resolving or authorizing it.
    ///
    /// # Errors
    /// Refuses excessive size or data outside the declared V1 encoding.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AssociationError> {
        canonical(
            b"meerkat.qualified-input-key.v1\0",
            self,
            MAX_ASSOCIATION_BYTES,
        )
    }
}

/// The version of [`AssociationDigest`] this build writes and reads.
pub const ASSOCIATION_DIGEST_VERSION: u8 = 1;

/// The digest a governed submission names its authority association by:
/// SHA-256 over the association's canonical V1 bytes
/// ([`InputAuthorityAssociation::canonical_bytes`]), under
/// [`ASSOCIATION_DIGEST_VERSION`] ([`InputAuthorityAssociation::digest`]).
///
/// It identifies an association. It is no permission and no evidence that
/// anything was admitted. Its text form is `v1:` and 64 lowercase hex
/// digits. A reader refuses any other version or shape: a digest of an
/// unknown version is never read as another association, nor as none.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssociationDigest {
    sha256: [u8; 32],
}

/// Why a text is no [`AssociationDigest`] this build reads.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AssociationDigestError {
    /// A digest of a version this build does not know.
    #[error("unknown association digest version {0:?}")]
    UnknownVersion(String),
    /// Not a digest.
    #[error("malformed association digest")]
    Malformed,
}

impl AssociationDigest {
    /// The version this digest was computed under.
    #[must_use]
    pub fn version(&self) -> u8 {
        ASSOCIATION_DIGEST_VERSION
    }

    /// Read a digest's text form, refusing any version but
    /// [`ASSOCIATION_DIGEST_VERSION`].
    ///
    /// # Errors
    /// [`AssociationDigestError::UnknownVersion`] for another version,
    /// [`AssociationDigestError::Malformed`] for anything else.
    pub fn parse(text: &str) -> Result<Self, AssociationDigestError> {
        let (version, hex) = text
            .split_once(':')
            .ok_or(AssociationDigestError::Malformed)?;
        if version != "v1" {
            return Err(AssociationDigestError::UnknownVersion(version.to_string()));
        }
        if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(AssociationDigestError::Malformed);
        }
        let mut sha256 = [0u8; 32];
        for (byte, pair) in sha256.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            let pair = std::str::from_utf8(pair).map_err(|_| AssociationDigestError::Malformed)?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| AssociationDigestError::Malformed)?;
        }
        Ok(Self { sha256 })
    }
}

impl fmt::Display for AssociationDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("v1:")?;
        for byte in self.sha256 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for AssociationDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssociationDigest({self})")
    }
}

impl Serialize for AssociationDigest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AssociationDigest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssociationError {
    #[error("native authority association has an unsupported data shape")]
    Shape,
    #[error("native authority association requires qualified identities")]
    Unqualified,
    #[error("native authority association exceeds the supported bound")]
    Limit,
    #[error("native authority association cannot be canonically encoded")]
    Encoding,
}

impl From<crate::grant::GrantReferenceError> for AssociationError {
    fn from(error: crate::grant::GrantReferenceError) -> Self {
        match error {
            crate::grant::GrantReferenceError::Unqualified => Self::Unqualified,
            crate::grant::GrantReferenceError::Shape
            | crate::grant::GrantReferenceError::Incarnation => Self::Shape,
        }
    }
}

fn qualified(principal: &PrincipalRef) -> Result<(), AssociationError> {
    principal
        .validate_qualified()
        .map_err(|_| AssociationError::Unqualified)
}

// A capped serialization bounds the intermediate value before it is allocated.
// This encoder makes no pre-decoding allocation guarantee for a caller's input.
// It supports exactly the non-floating data used by these V1 contracts. Object
// traversal is explicitly sorted even if another crate enables preserve_order.
fn canonical<T: Serialize>(
    domain: &[u8],
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, AssociationError> {
    fn append(out: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<(), AssociationError> {
        if bytes.len() > limit.saturating_sub(out.len()) {
            return Err(AssociationError::Limit);
        }
        out.extend_from_slice(bytes);
        Ok(())
    }
    fn length(out: &mut Vec<u8>, size: usize, limit: usize) -> Result<(), AssociationError> {
        append(
            out,
            &u32::try_from(size)
                .map_err(|_| AssociationError::Limit)?
                .to_be_bytes(),
            limit,
        )
    }
    fn encode(
        out: &mut Vec<u8>,
        value: &serde_json::Value,
        limit: usize,
    ) -> Result<(), AssociationError> {
        use serde_json::Value;
        match value {
            Value::Null => append(out, &[0], limit),
            Value::Bool(false) => append(out, &[1], limit),
            Value::Bool(true) => append(out, &[2], limit),
            Value::Number(number) => {
                if let Some(value) = number.as_u64() {
                    append(out, &[3], limit)?;
                    append(out, &value.to_be_bytes(), limit)
                } else if let Some(value) = number.as_i64() {
                    append(out, &[4], limit)?;
                    append(out, &value.to_be_bytes(), limit)
                } else {
                    Err(AssociationError::Encoding)
                }
            }
            Value::String(value) => {
                append(out, &[5], limit)?;
                length(out, value.len(), limit)?;
                append(out, value.as_bytes(), limit)
            }
            Value::Array(values) => {
                append(out, &[6], limit)?;
                length(out, values.len(), limit)?;
                for value in values {
                    encode(out, value, limit)?;
                }
                Ok(())
            }
            Value::Object(values) => {
                append(out, &[7], limit)?;
                length(out, values.len(), limit)?;
                let mut entries: Vec<_> = values.iter().collect();
                entries.sort_unstable_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
                for (key, value) in entries {
                    length(out, key.len(), limit)?;
                    append(out, key.as_bytes(), limit)?;
                    encode(out, value, limit)?;
                }
                Ok(())
            }
        }
    }
    struct CappedJson {
        bytes: Vec<u8>,
        limit: usize,
        exceeded: bool,
    }
    impl std::io::Write for CappedJson {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                self.exceeded = true;
                return Err(std::io::Error::other("bounded association encoding"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut json = CappedJson {
        bytes: Vec::new(),
        // JSON escaping takes at most six bytes per byte of supported text.
        limit: limit.checked_mul(6).ok_or(AssociationError::Limit)?,
        exceeded: false,
    };
    if serde_json::to_writer(&mut json, value).is_err() {
        return Err(if json.exceeded {
            AssociationError::Limit
        } else {
            AssociationError::Encoding
        });
    }
    let value: serde_json::Value =
        serde_json::from_slice(&json.bytes).map_err(|_| AssociationError::Encoding)?;
    let mut bytes = Vec::new();
    append(&mut bytes, domain, limit)?;
    encode(&mut bytes, &value, limit)?;
    Ok(bytes)
}

macro_rules! protected_debug {
    ($($ty:ty),+ $(,)?) => {$(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), "([protected])"))
            }
        }
    )+};
}
protected_debug!(
    NativeWorkTarget,
    OriginalWorkRef,
    QualifiedIngressNamespace,
    WorkAuthorityBasis,
    GrantTarget,
    TargetGrantLineage,
    InputAuthorityAssociationCandidate,
    InputAuthorityAssociation,
    QualifiedInputKey
);

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::evidence::EvidenceDigest;
    use crate::resource::ResourceRef;
    use meerkat_core::auth::{PrincipalKind, TrustDomainId};

    fn id(value: &str) -> EvidenceId {
        EvidenceId::new(value).expect("fixture id")
    }
    fn principal(value: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            value,
            TrustDomainId::new("fixture-domain").expect("domain"),
        )
        .expect("principal")
    }
    fn evidence(value: &str) -> HistoricalEvidenceRef {
        HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: ResourceDomain {
                    authority: principal("source"),
                    namespace: "receipts".into(),
                },
                resource_id: value.into(),
            },
            revision: id("revision"),
            digest: EvidenceDigest::from_array([7; 32]),
        }
    }
    fn controller_selection(model: &str) -> meerkat_core::ControllerModelSelection {
        meerkat_core::ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: model.into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(serde_json::json!({"realm":"realm", "account":"controller"}))
                .expect("credential identity"),
            "profile".into(),
            "fixture".into(),
        )
    }

    fn candidate() -> InputAuthorityAssociationCandidate {
        let original_work = OriginalWorkRef {
            authority: principal("ingress"),
            work: id("work"),
        };
        InputAuthorityAssociationCandidate {
            requester: principal("requester"),
            ingress_actor: principal("actor"),
            represented_subject: None,
            original_authentication: evidence("authentication"),
            logical_executor: principal("executor"),
            target: NativeWorkTarget {
                logical_owner: principal("owner"),
                logical_runtime: id("runtime"),
                context: id("context"),
                context_generation: 0,
                audience: AudienceRef::Principal {
                    principal: principal("requester"),
                },
            },
            original_work: original_work.clone(),
            root_event: evidence("event"),
            contributing_work: vec![original_work],
            authority_basis: WorkAuthorityBasis::GrantLineage {
                lineage: vec![GrantLineageRef {
                    root_authority: principal("grant-authority"),
                    authority_namespace: id("grants"),
                    authority_generation: 1,
                    authority_incarnation: crate::grant::GrantAuthorityIncarnation::from_uuid(
                        uuid::Uuid::from_u128(0x00000000_0000_4000_8000_000000000001),
                    )
                    .expect("fixture incarnation data"),
                    grant_id: id("grant"),
                    issued_revision: 1,
                }],
            },
            controller_grant_lineage: Vec::new(),
            controller_model: None,
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: vec![evidence("source-observation")],
            ingress_namespace: QualifiedIngressNamespace {
                realm: RealmId::parse("realm").expect("realm"),
                ingress: ResourceDomain {
                    authority: principal("ingress"),
                    namespace: "prompts".into(),
                },
                occurrence_scope: id("occurrence"),
            },
            contract: ContractRequirements::local_governed_v1(),
        }
    }

    fn fixture_lineage_mut(
        candidate: &mut InputAuthorityAssociationCandidate,
    ) -> &mut [GrantLineageRef] {
        let lineage = match &mut candidate.authority_basis {
            WorkAuthorityBasis::GrantLineage { lineage } => Some(lineage.as_mut_slice()),
            _ => None,
        };
        lineage.expect("fixture contains grant lineage")
    }

    #[test]
    fn association_retains_the_canonical_grant_incarnation_without_decode_default() {
        let value = InputAuthorityAssociation::new(candidate()).expect("association");
        let wire = serde_json::to_value(&value).expect("wire");
        let decoded: InputAuthorityAssociation =
            serde_json::from_value(wire.clone()).expect("decode");
        assert_eq!(decoded, value);
        let mut old = wire.clone();
        old["authority_basis"]["lineage"][0]
            .as_object_mut()
            .expect("grant object")
            .remove("authority_incarnation");
        assert!(serde_json::from_value::<InputAuthorityAssociation>(old).is_err());
        let mut null = wire;
        null["authority_basis"]["lineage"][0]["authority_incarnation"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<InputAuthorityAssociation>(null).is_err());
    }

    #[test]
    fn historical_source_observations_have_no_legacy_envelope_alias() {
        let mut wire = serde_json::to_value(candidate()).expect("candidate wire");
        let fields = wire.as_object_mut().expect("object");
        let observations = fields.remove("source_observations").expect("observations");
        fields.insert("source_envelopes".into(), observations);
        assert!(serde_json::from_value::<InputAuthorityAssociation>(wire).is_err());
    }

    #[test]
    fn complete_association_roundtrips_and_diagnostics_are_redacted() {
        let value = InputAuthorityAssociation::new(candidate()).expect("candidate");
        let wire = serde_json::to_vec(&value).expect("wire");
        let decoded: InputAuthorityAssociation = serde_json::from_slice(&wire).expect("decode");
        assert_eq!(value, decoded);
        assert_eq!(value.canonical_bytes(), decoded.canonical_bytes());
        assert_eq!(
            format!("{value:?}"),
            "InputAuthorityAssociation([protected])"
        );
        assert_eq!(
            format!("{:?}", value.candidate()),
            "InputAuthorityAssociationCandidate([protected])"
        );
    }

    #[test]
    fn cloned_association_shares_immutable_data_and_preserves_exact_encoding() {
        let mut original = candidate();
        original.represented_subject = Some(principal("represented"));
        original.controller_grant_lineage = fixture_lineage_mut(&mut candidate()).to_vec();
        original.controller_model = Some(controller_selection("controller"));
        let association = InputAuthorityAssociation::new(original.clone()).expect("association");
        let cloned = association.clone();
        assert!(std::ptr::eq(association.candidate(), cloned.candidate()));
        assert_eq!(association.candidate(), &original);
        assert_eq!(cloned, association);
        let wire = serde_json::to_vec(&original).expect("candidate wire");
        assert_eq!(
            serde_json::to_vec(&association).expect("association wire"),
            wire
        );
        assert_eq!(serde_json::to_vec(&cloned).expect("clone wire"), wire);
        let bytes = association.canonical_bytes().expect("canonical bytes");
        assert_eq!(cloned.canonical_bytes().expect("clone bytes"), bytes);
        let decoded: InputAuthorityAssociation = serde_json::from_slice(&wire).expect("decode");
        assert_eq!(decoded, association);
        assert!(!std::ptr::eq(association.candidate(), decoded.candidate()));
        assert_eq!(decoded.canonical_bytes().expect("decoded bytes"), bytes);

        let mut independent = cloned.candidate().clone();
        independent.requester = principal("other-requester");
        independent
            .source_observations
            .push(evidence("other-source"));
        let changed = InputAuthorityAssociation::new(independent).expect("changed candidate");
        assert!(!std::ptr::eq(association.candidate(), changed.candidate()));
        assert_ne!(changed, association);
        assert_ne!(changed.canonical_bytes().expect("changed bytes"), bytes);
        assert_eq!(association.candidate(), &original);
        assert_eq!(cloned.candidate(), &original);
        assert_eq!(
            association.canonical_bytes().expect("retained bytes"),
            bytes
        );
        assert_eq!(
            serde_json::to_vec(&association).expect("retained wire"),
            wire
        );
    }

    #[test]
    fn represented_subject_is_explicit_and_does_not_replace_actual_participants() {
        let direct = InputAuthorityAssociation::new(candidate()).expect("direct candidate");
        let mut represented = candidate();
        represented.represented_subject = Some(principal("represented-user"));
        let represented = InputAuthorityAssociation::new(represented).expect("represented claim");
        assert_eq!(
            represented.candidate().requester,
            direct.candidate().requester
        );
        assert_eq!(
            represented.candidate().ingress_actor,
            direct.candidate().ingress_actor
        );
        assert_eq!(
            represented.candidate().logical_executor,
            direct.candidate().logical_executor
        );
        assert!(direct.candidate().represented_subject.is_none());
        assert_ne!(represented.canonical_bytes(), direct.canonical_bytes());
        let roundtrip: InputAuthorityAssociation =
            serde_json::from_slice(&serde_json::to_vec(&represented).expect("encode"))
                .expect("decode");
        assert_eq!(roundtrip, represented);
    }

    #[test]
    fn every_retained_field_is_part_of_exact_binding() {
        let first = candidate();
        let bytes = InputAuthorityAssociation::new(first.clone())
            .expect("candidate")
            .canonical_bytes()
            .expect("encoding");
        for mutation in 0..17 {
            let mut changed = first.clone();
            match mutation {
                0 => changed.requester = principal("other-requester"),
                1 => changed.ingress_actor = principal("other-actor"),
                2 => changed.original_authentication = evidence("other-authentication"),
                3 => changed.logical_executor = principal("other-executor"),
                4 => changed.target.context_generation += 1,
                5 => changed.original_work.work = id("other-work"),
                6 => changed.root_event = evidence("other-root"),
                7 => changed.contributing_work.push(OriginalWorkRef {
                    authority: principal("another"),
                    work: id("another"),
                }),
                8 => {
                    fixture_lineage_mut(&mut changed)[0].issued_revision += 1;
                }
                9 => {
                    changed.admitted_ceiling.actions =
                        crate::constraints::ExactRestriction::exact([]);
                }
                10 => changed.source_observations.push(evidence("other-source")),
                11 => changed.ingress_namespace.occurrence_scope = id("other-occurrence"),
                12 => {
                    fixture_lineage_mut(&mut changed)[0].authority_generation += 1;
                }
                13 => changed.represented_subject = Some(principal("represented")),
                14 => {
                    changed.controller_grant_lineage =
                        fixture_lineage_mut(&mut candidate()).to_vec();
                }
                15 => {
                    changed.controller_ceiling.actions =
                        crate::constraints::ExactRestriction::exact([]);
                }
                _ => changed.controller_model = Some(controller_selection("controller")),
            }
            let changed = InputAuthorityAssociation::new(changed).expect("changed data");
            assert_ne!(
                bytes,
                changed.canonical_bytes().expect("encoding"),
                "field {mutation}"
            );
        }
    }

    #[test]
    fn controller_route_is_retained_and_batches_cannot_substitute_another_route() {
        let mut first = candidate();
        first.controller_model = Some(controller_selection("controller-a"));
        let first = InputAuthorityAssociation::new(first).expect("controller association");
        let mut other = first.candidate().clone();
        other.controller_model = Some(controller_selection("controller-b"));
        let other = InputAuthorityAssociation::new(other).expect("other controller");
        assert!(!first.batch_compatible_with(&other));
        assert_ne!(first.batch_identity_bytes(), other.batch_identity_bytes());
        assert_ne!(first.canonical_bytes(), other.canonical_bytes());
        let wire = serde_json::to_vec(&first).expect("wire");
        assert_eq!(
            serde_json::from_slice::<InputAuthorityAssociation>(&wire).expect("retained route"),
            first
        );
    }

    #[test]
    fn key_cannot_alias_realm_requester_target_generation_or_ingress() {
        let first = InputAuthorityAssociation::new(candidate()).expect("candidate");
        let key = first.qualified_key(id("same-key"));
        for mutation in 0..6 {
            let mut changed = first.candidate().clone();
            match mutation {
                0 => changed.ingress_namespace.realm = RealmId::parse("other").expect("realm"),
                1 => changed.requester = principal("other"),
                2 => changed.target.logical_runtime = id("other"),
                3 => changed.target.context_generation += 1,
                4 => changed.ingress_namespace.ingress.authority = principal("other"),
                _ => changed.ingress_namespace.occurrence_scope = id("other"),
            }
            let other = InputAuthorityAssociation::new(changed)
                .expect("changed")
                .qualified_key(id("same-key"));
            assert_ne!(key, other);
            assert_ne!(key.canonical_bytes(), other.canonical_bytes());
        }
        assert_ne!(key.canonical_bytes(), first.canonical_bytes());
    }

    #[test]
    fn downgrade_missing_and_unknown_top_level_fields_refuse() {
        let value = InputAuthorityAssociation::new(candidate()).expect("candidate");
        let wire = serde_json::to_value(&value).expect("wire");
        for field in wire.as_object().expect("object").keys() {
            let mut changed = wire.clone();
            changed.as_object_mut().expect("object").remove(field);
            assert!(
                serde_json::from_value::<InputAuthorityAssociation>(changed).is_err(),
                "{field}"
            );
        }
        let mut changed = wire;
        changed["default_to_service"] = serde_json::json!(true);
        assert!(serde_json::from_value::<InputAuthorityAssociation>(changed).is_err());
        let mut changed = candidate();
        changed.contract.profile = crate::protocol::EnforcementProfile::TrustedEmbedded;
        assert_eq!(
            InputAuthorityAssociation::new(changed),
            Err(AssociationError::Shape)
        );
        let mut changed = candidate();
        changed.requester =
            PrincipalRef::new(PrincipalKind::ServiceAccount, "legacy").expect("legacy");
        assert_eq!(
            InputAuthorityAssociation::new(changed),
            Err(AssociationError::Unqualified)
        );
    }

    #[test]
    fn canonical_encoding_is_framed_ordered_and_bounded() {
        let left = serde_json::json!({"a":["ab","c"],"b":true});
        let right = serde_json::json!({"b":true,"a":["ab","c"]});
        assert_eq!(
            canonical(b"test\0", &left, 256),
            canonical(b"test\0", &right, 256)
        );
        let different = serde_json::json!({"a":["a","bc"],"b":true});
        assert_ne!(
            canonical(b"test\0", &left, 256),
            canonical(b"test\0", &different, 256)
        );
        assert_eq!(
            canonical(b"test\0", &"x".repeat(100_000), 32),
            Err(AssociationError::Limit)
        );
        assert_eq!(
            canonical(b"test\0", &1.5_f64, 256),
            Err(AssociationError::Encoding)
        );
        let mut changed = candidate();
        changed.source_observations = vec![evidence("source"); MAX_ASSOCIATION_REFERENCES + 1];
        assert_eq!(
            InputAuthorityAssociation::new(changed),
            Err(AssociationError::Shape)
        );
    }

    #[test]
    fn policy_and_service_paths_are_explicit_historical_claims() {
        let grant = InputAuthorityAssociation::new(candidate()).expect("grant claim");
        let mut policy = candidate();
        policy.authority_basis = WorkAuthorityBasis::HostPolicy {
            policy: evidence("host-policy-version"),
        };
        let policy = InputAuthorityAssociation::new(policy).expect("policy claim");
        let mut service = candidate();
        service.authority_basis = WorkAuthorityBasis::ServiceMandate {
            mandate: evidence("connector-mandate-version"),
            commissioning_actor: principal("commissioner"),
            occurrence: id("scheduled-occurrence"),
        };
        let service = InputAuthorityAssociation::new(service).expect("service claim");
        for value in [&grant, &policy, &service] {
            let bytes = serde_json::to_vec(value).expect("wire claim");
            let decoded: InputAuthorityAssociation =
                serde_json::from_slice(&bytes).expect("strict claim roundtrip");
            assert_eq!(*value, decoded);
            assert_eq!(
                format!("{:?}", value.candidate().authority_basis),
                "WorkAuthorityBasis([protected])"
            );
        }
        assert_ne!(grant.canonical_bytes(), policy.canonical_bytes());
        assert_ne!(grant.canonical_bytes(), service.canonical_bytes());
        assert_ne!(policy.canonical_bytes(), service.canonical_bytes());
        // No resolver, policy owner or authenticated transport participated:
        // successful data construction therefore cannot be native admission.
    }

    #[test]
    fn every_service_mandate_component_is_retained_in_exact_binding() {
        let mut source = candidate();
        source.authority_basis = WorkAuthorityBasis::ServiceMandate {
            mandate: evidence("mandate"),
            commissioning_actor: principal("commissioner"),
            occurrence: id("occurrence"),
        };
        let original = InputAuthorityAssociation::new(source.clone())
            .expect("source")
            .canonical_bytes()
            .expect("source bytes");
        for change in 0..6 {
            let mut mandate = evidence("mandate");
            let mut commissioning_actor = principal("commissioner");
            let mut occurrence = id("occurrence");
            match change {
                0 => mandate.resource.domain.authority = principal("another-issuer"),
                1 => mandate.resource.resource_id = "another-mandate".into(),
                2 => mandate.revision = id("new-version"),
                3 => mandate.digest = EvidenceDigest::from_array([8; 32]),
                4 => commissioning_actor = principal("another-commissioner"),
                _ => occurrence = id("another-occurrence"),
            }
            let mut changed = source.clone();
            changed.authority_basis = WorkAuthorityBasis::ServiceMandate {
                mandate,
                commissioning_actor,
                occurrence,
            };
            assert_ne!(
                original,
                InputAuthorityAssociation::new(changed)
                    .expect("changed data")
                    .canonical_bytes()
                    .expect("changed bytes"),
                "service field {change}"
            );
        }
    }

    #[test]
    fn empty_or_unqualified_authority_paths_cannot_become_valid_association_data() {
        let legacy = PrincipalRef::new(PrincipalKind::ServiceAccount, "legacy")
            .expect("unqualified principal claim");
        let mut policy = evidence("policy");
        policy.resource.domain.authority = legacy.clone();
        for basis in [
            WorkAuthorityBasis::HostPolicy { policy },
            WorkAuthorityBasis::ServiceMandate {
                mandate: evidence("mandate"),
                commissioning_actor: legacy,
                occurrence: id("occurrence"),
            },
        ] {
            let mut value = candidate();
            value.authority_basis = basis;
            assert_eq!(
                InputAuthorityAssociation::new(value),
                Err(AssociationError::Unqualified)
            );
        }
        let mut empty = candidate();
        empty.authority_basis = WorkAuthorityBasis::GrantLineage { lineage: vec![] };
        assert_eq!(
            InputAuthorityAssociation::new(empty),
            Err(AssociationError::Shape)
        );
        let mut absent_generation = candidate();
        fixture_lineage_mut(&mut absent_generation)[0].authority_generation = 0;
        assert_eq!(
            InputAuthorityAssociation::new(absent_generation),
            Err(AssociationError::Shape)
        );
    }

    #[test]
    fn association_requires_the_exact_local_profile_and_baseline() {
        let valid = candidate();
        let wire = serde_json::to_value(&valid).expect("candidate wire");
        for profile in [
            "governed_buffered_v1",
            "trusted_embedded",
            "local_governed_v2",
        ] {
            let mut changed = wire.clone();
            changed["contract"]["profile"] = serde_json::json!(profile);
            assert!(serde_json::from_value::<InputAuthorityAssociation>(changed).is_err());
        }
        for field in ["semantics", "obligations"] {
            let mut changed = wire.clone();
            changed["contract"][field] = serde_json::json!([]);
            assert!(serde_json::from_value::<InputAuthorityAssociation>(changed).is_err());
        }
        let mut future = valid;
        future.contract.version.minor = 1;
        assert_eq!(
            InputAuthorityAssociation::new(future),
            Err(AssociationError::Shape)
        );
    }

    #[test]
    fn authority_basis_cannot_mix_branches_or_ignore_an_authority_field() {
        let mut value = candidate();
        value.authority_basis = WorkAuthorityBasis::ServiceMandate {
            mandate: evidence("mandate"),
            commissioning_actor: principal("commissioner"),
            occurrence: id("occurrence"),
        };
        let wire = serde_json::to_value(value).expect("wire");
        assert!(serde_json::from_value::<InputAuthorityAssociation>(wire.clone()).is_ok());
        for field in ["kind", "mandate", "commissioning_actor", "occurrence"] {
            let mut changed = wire.clone();
            changed["authority_basis"]
                .as_object_mut()
                .expect("basis object")
                .remove(field);
            assert!(serde_json::from_value::<InputAuthorityAssociation>(changed).is_err());
        }
        for extra in ["lineage", "policy", "current", "allow"] {
            let mut changed = wire.clone();
            changed["authority_basis"][extra] = serde_json::Value::Null;
            assert!(serde_json::from_value::<InputAuthorityAssociation>(changed).is_err());
        }
    }

    fn server(name: &str) -> GrantTarget {
        GrantTarget::McpServer {
            server: name.into(),
        }
    }

    fn targeted(
        base: Option<Vec<GrantLineageRef>>,
        targets: Vec<(GrantTarget, Vec<GrantLineageRef>)>,
    ) -> WorkAuthorityBasis {
        WorkAuthorityBasis::TargetedGrantLineages {
            base,
            targets: targets
                .into_iter()
                .map(|(target, lineage)| TargetGrantLineage { target, lineage })
                .collect(),
        }
    }

    #[test]
    fn targeted_lineages_select_each_target_separately_and_round_trip() {
        let mut value = candidate();
        let chain = fixture_lineage_mut(&mut value).to_vec();
        let mut other = chain.clone();
        other[0].grant_id = id("other-grant");
        value.authority_basis = targeted(
            Some(chain.clone()),
            vec![
                (server("alpha"), chain.clone()),
                (server("beta"), other.clone()),
            ],
        );
        let association = InputAuthorityAssociation::new(value).expect("association");
        let basis = &association.candidate().authority_basis;
        assert_eq!(basis.lineage_for(None), Some(chain.as_slice()));
        assert_eq!(
            basis.lineage_for(Some(&server("alpha"))),
            Some(chain.as_slice())
        );
        assert_eq!(
            basis.lineage_for(Some(&server("beta"))),
            Some(other.as_slice())
        );
        assert_eq!(basis.lineage_for(Some(&server("gamma"))), None);
        assert_eq!(basis.grant_lineages().count(), 3);

        let wire = serde_json::to_value(&association).expect("wire");
        let decoded: InputAuthorityAssociation =
            serde_json::from_value(wire.clone()).expect("decode");
        assert_eq!(decoded, association);
        let mut unknown = wire;
        unknown["authority_basis"]["targets"][0]["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<InputAuthorityAssociation>(unknown).is_err());

        // A single lineage still selects itself for every target.
        let legacy = candidate();
        let legacy_chain = legacy
            .authority_basis
            .lineage_for(None)
            .expect("lineage")
            .to_vec();
        assert_eq!(
            legacy.authority_basis.lineage_for(Some(&server("alpha"))),
            Some(legacy_chain.as_slice())
        );
    }

    #[test]
    fn targeted_lineages_have_one_canonical_valid_shape() {
        let mut value = candidate();
        let chain = fixture_lineage_mut(&mut value).to_vec();
        for basis in [
            targeted(Some(Vec::new()), vec![(server("alpha"), chain.clone())]),
            targeted(Some(chain.clone()), vec![(server("alpha"), Vec::new())]),
            targeted(Some(chain.clone()), vec![(server(""), chain.clone())]),
            targeted(
                Some(chain.clone()),
                vec![
                    (server("beta"), chain.clone()),
                    (server("alpha"), chain.clone()),
                ],
            ),
            targeted(
                Some(chain.clone()),
                vec![
                    (server("alpha"), chain.clone()),
                    (server("alpha"), chain.clone()),
                ],
            ),
        ] {
            let mut invalid = value.clone();
            invalid.authority_basis = basis;
            assert_eq!(
                InputAuthorityAssociation::new(invalid),
                Err(AssociationError::Shape)
            );
        }
        let mut base_only = value.clone();
        base_only.authority_basis = targeted(Some(chain.clone()), Vec::new());
        assert!(InputAuthorityAssociation::new(base_only).is_ok());
        // No optional base: a target-only selection is complete on its own,
        // and an operation without a target has no lineage.
        let mut target_only = value;
        target_only.authority_basis = targeted(None, vec![(server("alpha"), chain)]);
        let target_only = InputAuthorityAssociation::new(target_only).expect("target only");
        assert_eq!(
            target_only.candidate().authority_basis.lineage_for(None),
            None
        );
        // Selecting nothing is controller-only work: no operation lineage.
        let mut controller_only = candidate();
        controller_only.authority_basis = targeted(None, Vec::new());
        let controller_only =
            InputAuthorityAssociation::new(controller_only).expect("controller only");
        let basis = &controller_only.candidate().authority_basis;
        assert_eq!(basis.lineage_for(None), None);
        assert_eq!(basis.lineage_for(Some(&server("alpha"))), None);
        assert_eq!(basis.grant_lineages().count(), 0);
    }

    fn bundle_operation(bundle: &str, operation: &str) -> GrantTarget {
        GrantTarget::RegisteredToolBundleOperation {
            bundle: bundle.into(),
            operation: operation.into(),
        }
    }

    /// A registered bundle's operation is a target of its own: each exact
    /// (bundle, operation) selects only its own lineage, never the base, a
    /// sibling operation or a same-named server, and both names are required.
    #[test]
    fn bundle_operation_targets_select_only_their_own_lineage() {
        let mut value = candidate();
        let chain = fixture_lineage_mut(&mut value).to_vec();
        let mut read = chain.clone();
        read[0].grant_id = id("mail-read");
        value.authority_basis = targeted(
            Some(chain.clone()),
            vec![
                (server("mail"), chain.clone()),
                (bundle_operation("mail", "read_mail"), read.clone()),
            ],
        );
        let association = InputAuthorityAssociation::new(value.clone()).expect("association");
        let basis = &association.candidate().authority_basis;
        assert_eq!(
            basis.lineage_for(Some(&bundle_operation("mail", "read_mail"))),
            Some(read.as_slice())
        );
        assert_eq!(
            basis.lineage_for(Some(&bundle_operation("mail", "send_draft"))),
            None,
            "a sibling operation has no lineage"
        );
        assert_eq!(
            basis.lineage_for(Some(&bundle_operation("other", "read_mail"))),
            None,
            "another bundle has no lineage"
        );
        let wire = serde_json::to_value(&association).expect("wire");
        assert_eq!(
            wire["authority_basis"]["targets"][1]["target"],
            serde_json::json!({
                "kind": "registered_tool_bundle_operation",
                "bundle": "mail",
                "operation": "read_mail",
            })
        );
        let decoded: InputAuthorityAssociation = serde_json::from_value(wire).expect("decode");
        assert_eq!(decoded, association);

        for target in [
            bundle_operation("", "read_mail"),
            bundle_operation("mail", ""),
        ] {
            let mut invalid = value.clone();
            invalid.authority_basis = targeted(None, vec![(target, chain.clone())]);
            assert_eq!(
                InputAuthorityAssociation::new(invalid),
                Err(AssociationError::Shape)
            );
        }
        // One canonical order across target kinds.
        let mut unordered = value;
        unordered.authority_basis = targeted(
            None,
            vec![
                (bundle_operation("mail", "read_mail"), read),
                (server("mail"), chain),
            ],
        );
        assert_eq!(
            InputAuthorityAssociation::new(unordered),
            Err(AssociationError::Shape)
        );
    }

    fn checked_route(route: &str, server: &str) -> GrantTarget {
        GrantTarget::CheckedMcpRoute {
            route: route.into(),
            server: server.into(),
        }
    }

    /// A checked route to a server's raw operations is a target of its own:
    /// it selects only its own lineage, never the server's ordinary lineage
    /// or another route's, the server's ordinary operations never select
    /// it, and both names are required.
    #[test]
    fn checked_route_targets_select_only_their_own_lineage() {
        let mut value = candidate();
        let chain = fixture_lineage_mut(&mut value).to_vec();
        let mut checked = chain.clone();
        checked[0].grant_id = id("setup-checked");
        value.authority_basis = targeted(
            None,
            vec![
                (server("setup"), chain.clone()),
                (checked_route("checked", "setup"), checked.clone()),
            ],
        );
        let association = InputAuthorityAssociation::new(value.clone()).expect("association");
        let basis = &association.candidate().authority_basis;
        assert_eq!(
            basis.lineage_for(Some(&checked_route("checked", "setup"))),
            Some(checked.as_slice())
        );
        assert_eq!(
            basis.lineage_for(Some(&server("setup"))),
            Some(chain.as_slice()),
            "the server's ordinary operations keep their own lineage"
        );
        assert_eq!(
            basis.lineage_for(Some(&checked_route("other", "setup"))),
            None,
            "another route has no lineage"
        );
        assert_eq!(
            basis.lineage_for(Some(&checked_route("checked", "other"))),
            None,
            "the route to another server has no lineage"
        );
        let wire = serde_json::to_value(&association).expect("wire");
        assert_eq!(
            wire["authority_basis"]["targets"][1]["target"],
            serde_json::json!({
                "kind": "checked_mcp_route",
                "route": "checked",
                "server": "setup",
            })
        );
        let decoded: InputAuthorityAssociation = serde_json::from_value(wire).expect("decode");
        assert_eq!(decoded, association);

        for target in [checked_route("", "setup"), checked_route("checked", "")] {
            let mut invalid = value.clone();
            invalid.authority_basis = targeted(None, vec![(target, chain.clone())]);
            assert_eq!(
                InputAuthorityAssociation::new(invalid),
                Err(AssociationError::Shape)
            );
        }
        // One canonical order across target kinds.
        let mut unordered = value;
        unordered.authority_basis = targeted(
            None,
            vec![
                (checked_route("checked", "setup"), checked),
                (server("setup"), chain),
            ],
        );
        assert_eq!(
            InputAuthorityAssociation::new(unordered),
            Err(AssociationError::Shape)
        );
    }

    fn builtin_web_search() -> GrantTarget {
        GrantTarget::BuiltinToolOperation {
            operation: meerkat_core::BuiltinToolOperation::WebSearch,
        }
    }

    fn delegated(route: &str, capability: meerkat_core::ServerToolKind) -> GrantTarget {
        GrantTarget::DelegatedHostedSearch {
            route: route.into(),
            capability,
        }
    }

    fn route_key(model: &str) -> String {
        meerkat_core::ControllerModelFacts::new(
            controller_selection(model),
            "https://search.invalid/v1/responses".into(),
            "search-wire-model".into(),
        )
        .route_key()
        .as_str()
        .to_owned()
    }

    /// The builtin web_search operation and a delegated hosted search are
    /// targets of their own, ordered after every earlier target kind, each
    /// with one wire shape. A delegated search names a canonical route key
    /// and a hosted search capability (web search or Google Search
    /// grounding); other route text or another capability is a malformed
    /// shape. An association without them keeps its exact bytes.
    #[test]
    fn delegated_hosted_search_targets_validate_route_and_capability() {
        use meerkat_core::ServerToolKind;
        let mut value = candidate();
        let chain = fixture_lineage_mut(&mut value).to_vec();
        let mut existing = value.clone();
        existing.authority_basis = targeted(
            Some(chain.clone()),
            vec![
                (server("mail"), chain.clone()),
                (bundle_operation("mail", "read_mail"), chain.clone()),
                (checked_route("checked", "mail"), chain.clone()),
            ],
        );
        let existing = InputAuthorityAssociation::new(existing).expect("earlier target kinds");
        let decoded: InputAuthorityAssociation =
            serde_json::from_value(serde_json::to_value(&existing).expect("wire")).expect("decode");
        assert_eq!(
            decoded.canonical_bytes().expect("bytes"),
            existing.canonical_bytes().expect("bytes"),
            "an existing association decodes and re-encodes byte-identically"
        );

        let route = route_key("search-model");
        let other = route_key("other-search-model");
        assert_ne!(route, other);
        let mut searching = value.clone();
        searching.authority_basis = targeted(
            None,
            vec![
                (checked_route("checked", "mail"), chain.clone()),
                (builtin_web_search(), chain.clone()),
                (delegated(&route, ServerToolKind::WebSearch), chain.clone()),
                (
                    delegated(&route, ServerToolKind::GoogleSearch),
                    chain.clone(),
                ),
            ],
        );
        let association = InputAuthorityAssociation::new(searching).expect("both new target kinds");
        let basis = &association.candidate().authority_basis;
        assert_eq!(
            basis.lineage_for(Some(&builtin_web_search())),
            Some(chain.as_slice())
        );
        assert_eq!(
            basis.lineage_for(Some(&delegated(&route, ServerToolKind::WebSearch))),
            Some(chain.as_slice())
        );
        assert_eq!(
            basis.lineage_for(Some(&delegated(&other, ServerToolKind::WebSearch))),
            None,
            "another route has no lineage"
        );
        assert_eq!(basis.lineage_for(None), None, "never the base");
        let wire = serde_json::to_value(&association).expect("wire");
        assert_eq!(
            wire["authority_basis"]["targets"][1]["target"],
            serde_json::json!({"kind": "builtin_tool_operation", "operation": "web_search"})
        );
        assert_eq!(
            wire["authority_basis"]["targets"][2]["target"],
            serde_json::json!({
                "kind": "delegated_hosted_search",
                "route": route,
                "capability": {"kind": "web_search"},
            })
        );
        assert_eq!(
            wire["authority_basis"]["targets"][3]["target"]["capability"],
            serde_json::json!({"kind": "google_search"})
        );
        let decoded: InputAuthorityAssociation =
            serde_json::from_value(wire.clone()).expect("decode");
        assert_eq!(decoded, association);
        assert_eq!(
            decoded.canonical_bytes().expect("bytes"),
            association.canonical_bytes().expect("bytes")
        );
        assert!(checked_route("checked", "mail") < builtin_web_search());
        assert!(builtin_web_search() < delegated(&route, ServerToolKind::WebSearch));

        for target in [
            delegated("", ServerToolKind::WebSearch),
            delegated("model-route/v1/not-a-digest", ServerToolKind::WebSearch),
            delegated(&route.to_uppercase(), ServerToolKind::WebSearch),
            delegated(
                &route,
                ServerToolKind::ProviderNative {
                    name: "web_search".into(),
                },
            ),
        ] {
            let mut invalid = value.clone();
            invalid.authority_basis = targeted(None, vec![(target, chain.clone())]);
            assert_eq!(
                InputAuthorityAssociation::new(invalid),
                Err(AssociationError::Shape)
            );
        }
        let mut unknown = wire.clone();
        unknown["authority_basis"]["targets"][2]["target"]["account"] = serde_json::json!("x");
        assert!(serde_json::from_value::<InputAuthorityAssociation>(unknown).is_err());
        let mut unknown_operation = wire;
        unknown_operation["authority_basis"]["targets"][1]["target"]["operation"] =
            serde_json::json!("shell");
        assert!(serde_json::from_value::<InputAuthorityAssociation>(unknown_operation).is_err());
    }

    fn peer(value: u128) -> GrantTarget {
        GrantTarget::Peer {
            peer: meerkat_core::comms::PeerId::from_uuid(uuid::Uuid::from_u128(value)),
        }
    }

    fn host_memory(owner: &str, scope: &str) -> GrantTarget {
        GrantTarget::Memory {
            scope: meerkat_core::memory::MemoryScopeRef::Host {
                owner: owner.into(),
                scope: scope.into(),
            },
        }
    }

    /// Each peer, the peer directory and each memory scope is a target of
    /// its own: it selects only its own lineage, never another peer's or
    /// scope's, and never the base. The wire shape is pinned, a host memory
    /// scope needs both of its names, and the order stays canonical across
    /// target kinds.
    #[test]
    fn peer_directory_and_memory_targets_select_only_their_own_lineage() {
        let mut value = candidate();
        let chain = fixture_lineage_mut(&mut value).to_vec();
        let lineage = |name: &str| {
            let mut lineage = chain.clone();
            lineage[0].grant_id = id(name);
            lineage
        };
        let session = meerkat_core::SessionId::from_uuid(uuid::Uuid::from_u128(9));
        let session_memory = GrantTarget::Memory {
            scope: meerkat_core::memory::MemoryScopeRef::Session {
                session_id: session.clone(),
            },
        };
        value.authority_basis = targeted(
            Some(chain.clone()),
            vec![
                (server("alpha"), chain.clone()),
                (peer(1), lineage("peer-one")),
                (GrantTarget::PeerDirectory, lineage("peer-directory")),
                (session_memory.clone(), lineage("session-memory")),
                (
                    host_memory("agent_memory", "identity/alice"),
                    lineage("alice"),
                ),
            ],
        );
        let association = InputAuthorityAssociation::new(value.clone()).expect("association");
        let basis = &association.candidate().authority_basis;
        assert_eq!(
            basis.lineage_for(Some(&peer(1))),
            Some(lineage("peer-one").as_slice())
        );
        assert_eq!(
            basis.lineage_for(Some(&peer(2))),
            None,
            "another peer has no lineage, and the base never stands in"
        );
        assert_eq!(
            basis.lineage_for(Some(&GrantTarget::PeerDirectory)),
            Some(lineage("peer-directory").as_slice())
        );
        assert_eq!(
            basis.lineage_for(Some(&host_memory("agent_memory", "identity/alice"))),
            Some(lineage("alice").as_slice())
        );
        assert_eq!(
            basis.lineage_for(Some(&host_memory("agent_memory", "identity/bob"))),
            None,
            "another scope of the same owner has no lineage"
        );
        assert_eq!(
            basis.lineage_for(Some(&host_memory("other_memory", "identity/alice"))),
            None,
            "the same key of another owner has no lineage"
        );
        assert_eq!(
            basis.lineage_for(Some(&session_memory)),
            Some(lineage("session-memory").as_slice())
        );

        let wire = serde_json::to_value(&association).expect("wire");
        let targets = &wire["authority_basis"]["targets"];
        assert_eq!(
            targets[1]["target"],
            serde_json::json!({
                "kind": "peer",
                "peer": "00000000-0000-0000-0000-000000000001",
            })
        );
        assert_eq!(
            targets[2]["target"],
            serde_json::json!({"kind": "peer_directory"})
        );
        assert_eq!(
            targets[3]["target"],
            serde_json::json!({
                "kind": "memory",
                "scope": {"kind": "session", "session_id": session.to_string()},
            })
        );
        assert_eq!(
            targets[4]["target"],
            serde_json::json!({
                "kind": "memory",
                "scope": {"kind": "host", "owner": "agent_memory", "scope": "identity/alice"},
            })
        );
        let decoded: InputAuthorityAssociation = serde_json::from_value(wire).expect("decode");
        assert_eq!(decoded, association);

        for target in [
            host_memory("", "identity/alice"),
            host_memory("agent_memory", ""),
        ] {
            let mut invalid = value.clone();
            invalid.authority_basis = targeted(None, vec![(target, chain.clone())]);
            assert_eq!(
                InputAuthorityAssociation::new(invalid),
                Err(AssociationError::Shape)
            );
        }
        // One canonical order across target kinds.
        let mut unordered = value;
        unordered.authority_basis = targeted(
            None,
            vec![(peer(1), chain.clone()), (server("alpha"), chain)],
        );
        assert_eq!(
            InputAuthorityAssociation::new(unordered),
            Err(AssociationError::Shape)
        );
    }

    /// The association digest is a function of the canonical bytes: the
    /// same association gives the same digest, any field change gives
    /// another, its text form round-trips, and a reader refuses an unknown
    /// version or shape instead of reading it as some association or none.
    #[test]
    fn the_association_digest_is_versioned_and_follows_every_field() {
        let digest = |candidate: InputAuthorityAssociationCandidate| {
            InputAuthorityAssociation::new(candidate)
                .expect("association")
                .digest()
                .expect("digest")
        };
        let base = digest(candidate());
        assert_eq!(
            base,
            digest(candidate()),
            "the same association, the same digest"
        );
        assert_eq!(base.version(), ASSOCIATION_DIGEST_VERSION);
        let changes: &[(&str, fn(&mut InputAuthorityAssociationCandidate))] = &[
            ("requester", |c| {
                c.requester = principal("another-requester");
            }),
            ("ingress actor", |c| {
                c.ingress_actor = principal("another-actor");
            }),
            ("represented subject", |c| {
                c.represented_subject = Some(principal("represented"));
            }),
            ("original authentication", |c| {
                c.original_authentication = evidence("another-authentication");
            }),
            ("logical executor", |c| {
                c.logical_executor = principal("another-executor");
            }),
            ("target context", |c| {
                c.target.context = id("another-context");
            }),
            ("target generation", |c| c.target.context_generation = 1),
            ("original work", |c| {
                c.original_work.work = id("another-work");
            }),
            ("root event", |c| c.root_event = evidence("another-event")),
            ("contributing work", |c| c.contributing_work.clear()),
            ("authority basis", |c| {
                fixture_lineage_mut(c)[0].issued_revision = 2;
            }),
            ("controller model", |c| {
                c.controller_model = Some(controller_selection("controller"));
            }),
            ("source observations", |c| c.source_observations.clear()),
            ("occurrence scope", |c| {
                c.ingress_namespace.occurrence_scope = id("another-occurrence");
            }),
        ];
        for &(field, change) in changes {
            let mut changed = candidate();
            change(&mut changed);
            assert_ne!(
                digest(changed),
                base,
                "changing the {field} changes the digest"
            );
        }

        let text = base.to_string();
        assert!(text.starts_with("v1:") && text.len() == 3 + 64, "{text}");
        assert_eq!(AssociationDigest::parse(&text), Ok(base));
        let wire = serde_json::to_value(base).expect("wire");
        assert_eq!(wire, serde_json::Value::String(text.clone()));
        assert_eq!(
            serde_json::from_value::<AssociationDigest>(wire).expect("decode"),
            base
        );
        assert_eq!(
            AssociationDigest::parse(&text.replacen("v1:", "v2:", 1)),
            Err(AssociationDigestError::UnknownVersion("v2".into())),
            "an unknown version is refused, never read as another association"
        );
        for malformed in [
            "",
            "v1",
            "v1:",
            &text[..text.len() - 1],
            &text.to_uppercase().replacen("V1:", "v1:", 1),
        ] {
            assert_eq!(
                AssociationDigest::parse(malformed),
                Err(AssociationDigestError::Malformed),
                "{malformed:?}"
            );
        }
        assert!(serde_json::from_str::<AssociationDigest>("\"v9:00\"").is_err());
    }
}
