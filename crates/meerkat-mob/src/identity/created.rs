//! Store-owned identity entries: the row an identity keeps in Core's identity
//! store when that store is the identity's one durable owner (its custody),
//! the declared spec the row holds, how the identity entered the store and
//! under what consent, and the receipts of the changes that write it.
//!
//! An identity has one durable owner. A roster-owned identity's desired
//! record is its host's roster, and the host declares it again at each
//! apply; a store-owned identity's desired record is its row here (schema
//! version 2, in the identity store's intents table beside the host-adopted
//! identity intents of version 1). Custody decides only where the record
//! lives, which recovery reads it at a start, which writer persists a change
//! of it, and the name reservations (a removal's tombstone, an unreadable
//! row): never the identity's profile, labels, tools, grants or wiring,
//! which come from its profile and the host's build as for any member.
//!
//! The row's format is its gate: every type here refuses a field it does
//! not know, and the row's authority digest seals all of it, so a row a
//! later build wrote reads unreadable for its identity alone
//! ([`CreatedIdentityObservation::Unreadable`]), never as something else.

use std::collections::BTreeMap;

use meerkat_core::approval::decision::ConsentProvenance;
use meerkat_core::{ContentInput, SessionId};
use serde::{Deserialize, Serialize};

use super::{IdentityIntentError, sha256_digest, validate_identity, validate_text};
use crate::ids::{AgentIdentity, MobId, ProfileName};

/// The schema version of a store-owned identity's row in the identity
/// store's intents table. A version 1 row there is a host-adopted identity
/// intent ([`super::IdentityIntentRecord`]).
pub const CREATED_IDENTITY_SCHEMA_VERSION: u32 = 2;

/// The schema version of an identity change's receipt
/// ([`IdentityChangeReceipt`]).
pub const IDENTITY_CHANGE_RECEIPT_SCHEMA_VERSION: u32 = 1;

/// The most identities one change commits: one store transaction, all or
/// nothing. A larger create is refused before anything is written.
pub const IDENTITY_CHANGE_BATCH_MAX: usize = 256;

/// The canonical digest of a JSON value: SHA-256 over its JSON with object
/// keys sorted, as `sha256:<hex>`. Equal values have equal digests, whatever
/// the order their maps were built in.
fn canonical_json_digest(value: &serde_json::Value) -> Result<String, IdentityIntentError> {
    fn write(value: &serde_json::Value, out: &mut String) -> Result<(), IdentityIntentError> {
        let scalar = |value: &serde_json::Value| {
            serde_json::to_string(value)
                .map_err(|error| IdentityIntentError::Serialization(error.to_string()))
        };
        match value {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by_key(|(key, _)| *key);
                out.push('{');
                for (index, (key, item)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(
                        &serde_json::to_string(key).map_err(|error| {
                            IdentityIntentError::Serialization(error.to_string())
                        })?,
                    );
                    out.push(':');
                    write(item, out)?;
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write(item, out)?;
                }
                out.push(']');
            }
            other => out.push_str(&scalar(other)?),
        }
        Ok(())
    }
    let mut canonical = String::new();
    write(value, &mut canonical)?;
    Ok(sha256_digest(canonical.as_bytes()))
}

/// Whether an identity accepts a host's send, or only its dispatch. Recorded
/// with the spec; the host's delivery surface reads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityAddressability {
    #[default]
    Addressable,
    InternalOnly,
}

/// The declared spec of an identity: what a host's roster row declares for a
/// roster-owned identity, field for field, and what a store-owned identity's
/// row holds. Its profile is bound directly: the identity's tools, model and
/// skills come from that profile and the host's build at each build, and
/// nothing else is bound.
///
/// Its JSON is the roster row's: the same fields under the same names, each
/// one written (an absent value as `null`), so [`Self::spec_digest`] equals
/// the digest a host computes over its roster row of the same spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreatedIdentitySpec {
    pub identity: AgentIdentity,
    pub profile: ProfileName,
    #[serde(default)]
    pub addressability: IdentityAddressability,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub context: Option<serde_json::Value>,
    #[serde(default)]
    pub additional_instructions: Vec<String>,
    /// The identity's first turn: a kickoff is owed exactly when it is set
    /// (or when the profile runs autonomously, as for any fresh member).
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
    #[serde(default)]
    pub runtime_mode_override: Option<crate::MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<crate::MobBackendKind>,
    #[serde(default)]
    pub binding: Option<meerkat_contracts::WireRuntimeBinding>,
    /// The bound member host the identity lives on, when it is placed.
    #[serde(default)]
    pub placement: Option<meerkat_contracts::WireHostRef>,
}

impl CreatedIdentitySpec {
    /// The spec of `identity`, bound to `profile`, with nothing else set.
    #[must_use]
    pub fn new(identity: impl Into<AgentIdentity>, profile: impl Into<ProfileName>) -> Self {
        Self {
            identity: identity.into(),
            profile: profile.into(),
            addressability: IdentityAddressability::default(),
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }
    }

    #[must_use]
    pub fn with_addressability(mut self, addressability: IdentityAddressability) -> Self {
        self.addressability = addressability;
        self
    }

    #[must_use]
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    #[must_use]
    pub fn with_labels(mut self, labels: BTreeMap<String, String>) -> Self {
        self.labels = labels;
        self
    }

    #[must_use]
    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }

    #[must_use]
    pub fn with_additional_instructions(mut self, instructions: Vec<String>) -> Self {
        self.additional_instructions = instructions;
        self
    }

    #[must_use]
    pub fn with_initial_message(mut self, message: ContentInput) -> Self {
        self.initial_message = Some(message);
        self
    }

    #[must_use]
    pub fn with_runtime_mode_override(mut self, runtime_mode: crate::MobRuntimeMode) -> Self {
        self.runtime_mode_override = Some(runtime_mode);
        self
    }

    #[must_use]
    pub fn with_backend(mut self, backend: crate::MobBackendKind) -> Self {
        self.backend = Some(backend);
        self
    }

    #[must_use]
    pub fn with_binding(mut self, binding: meerkat_contracts::WireRuntimeBinding) -> Self {
        self.binding = Some(binding);
        self
    }

    #[must_use]
    pub fn with_placement(mut self, placement: meerkat_contracts::WireHostRef) -> Self {
        self.placement = Some(placement);
        self
    }

    /// The spec's canonical digest: SHA-256 over its JSON with object keys
    /// sorted, as `sha256:<hex>`. A change's expected spec digest is checked
    /// against it, and it equals the digest of the roster row of the same
    /// spec.
    pub fn spec_digest(&self) -> Result<String, IdentityIntentError> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        canonical_json_digest(&value)
    }

    /// Refuse a spec whose identity or profile is not canonical text, or
    /// whose display name is empty.
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_identity(&self.identity)?;
        validate_text("profile", self.profile.as_str())?;
        if let Some(display_name) = &self.display_name {
            validate_text("display_name", display_name)?;
        }
        for key in self.labels.keys() {
            validate_text("label_key", key)?;
        }
        Ok(())
    }

    /// Whether a member of this spec is placed or bound to a host or an
    /// external process: such an identity is hosted on its first use, never
    /// seated Dormant here.
    #[must_use]
    pub fn is_hosted(&self) -> bool {
        self.placement.is_some()
            || matches!(
                self.binding,
                Some(meerkat_contracts::WireRuntimeBinding::External { .. })
            )
    }
}

/// How an identity is prepared by its host before its first use: decided by
/// the host's policy for the identity's profile, whatever its custody.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityPreparationMode {
    /// No preparation: the identity is seated at once.
    #[default]
    None,
    /// The host prepares its records and settles: until then the identity
    /// has no member, and deliveries to it are refused, retryable.
    Required,
    /// The host prepares a held, unpublished construction of the identity's
    /// member and settles by activating it: the identity is never seated
    /// Dormant.
    Constructed,
}

/// The presence a store-owned identity's row records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "presence", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum CreatedIdentityPresence {
    /// The identity is present: its member is seated, owed its seat, hosted
    /// on its first use or awaiting its held creation.
    Present,
    /// The identity's host preparation is pending: it has no member.
    /// `attempt` is its current construction attempt (0 before the first),
    /// and `constructed` says the preparation activates a held construction
    /// rather than seating it.
    PendingPreparation {
        attempt: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        constructed: bool,
    },
    /// The identity was removed: the row is its name's tombstone, and the
    /// name is never an identity or a member again.
    Removed,
    /// The identity is archived: it has no member, keeps its name and its
    /// record, and its unarchive seats it again on `archived_session`, the
    /// session its member was bound to.
    Archived {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archived_session: Option<SessionId>,
    },
}

/// Who asked for a change: recorded, never authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum IdentityChangeRequester {
    /// A member identity, attributed from its session, never from arguments.
    Agent { identity: AgentIdentity },
    /// The host, on one of its surfaces (the host plane, or `console`).
    Host { surface: String },
    /// An authenticated human principal.
    Human { principal: String },
}

impl IdentityChangeRequester {
    fn validate(&self) -> Result<(), IdentityIntentError> {
        match self {
            Self::Agent { identity } => validate_identity(identity),
            Self::Host { surface } => validate_text("requester_surface", surface),
            Self::Human { principal } => validate_text("requester_principal", principal),
        }
    }
}

/// The outcome of a change rule that commits a change without a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityPolicyOutcome {
    Allow,
    Notify,
}

/// The consent a change committed under, sealed in the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum IdentityChangeConsent {
    /// A durable decision's consent, from its execution permit
    /// ([`ConsentProvenance::Decided`]).
    Decided { provenance: ConsentProvenance },
    /// The change rule's outcome: the change needed no decision.
    Policy { outcome: IdentityPolicyOutcome },
    /// Consent decided outside Meerkat, accepted only on a host operator's
    /// import ([`ConsentProvenance::Imported`]).
    Imported { provenance: ConsentProvenance },
}

impl IdentityChangeConsent {
    fn validate(&self) -> Result<(), IdentityIntentError> {
        match self {
            Self::Decided {
                provenance: ConsentProvenance::Decided { .. },
            }
            | Self::Imported {
                provenance: ConsentProvenance::Imported { .. },
            }
            | Self::Policy { .. } => Ok(()),
            Self::Decided { .. } | Self::Imported { .. } => {
                Err(IdentityIntentError::InvalidCreatedIdentity(
                    "the consent's form and its provenance's kind differ".to_string(),
                ))
            }
        }
    }
}

/// Evidence a trusted host ingress attested about the source of a request:
/// recorded in provenance, never authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IdentitySourceEvidence {
    /// The host adapter that attests it.
    pub attestor: String,
    /// What or whom the source is about.
    pub subject: String,
    /// The source event's reference.
    pub source_ref: String,
    /// When the source was observed, in UNIX milliseconds.
    pub observed_at_ms: u64,
}

impl IdentitySourceEvidence {
    #[must_use]
    pub fn new(
        attestor: impl Into<String>,
        subject: impl Into<String>,
        source_ref: impl Into<String>,
        observed_at_ms: u64,
    ) -> Self {
        Self {
            attestor: attestor.into(),
            subject: subject.into(),
            source_ref: source_ref.into(),
            observed_at_ms,
        }
    }
}

/// Where an adopted identity came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityAdoptedFrom {
    /// A roster-owned identity.
    Roster,
    /// A worker member another member spawned.
    Worker,
}

/// How an identity's row entered the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum IdentityEntered {
    /// A create committed it.
    Created,
    /// An adoption moved an existing member into store custody.
    Adopted {
        from: IdentityAdoptedFrom,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        former_spawner: Option<AgentIdentity>,
    },
    /// A host operator's import restored it.
    Imported,
    /// The removal of a roster-owned identity wrote it: the store's first
    /// row of the name, its tombstone.
    RosterRemoval,
}

/// How, when and at whose request an identity's row entered the store, and
/// under what consent. Written once, by the operation that wrote the row
/// first; recorded and shown, never read for a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreatedIdentityProvenance {
    pub entered: IdentityEntered,
    /// The id of the operation that wrote the row first.
    pub operation_id: String,
    /// When that operation committed, in UNIX milliseconds.
    pub at_ms: u64,
    pub requester: IdentityChangeRequester,
    /// Who drafted the request, when it was not the requester: recorded,
    /// never authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_evidence: Option<IdentitySourceEvidence>,
    pub consent: IdentityChangeConsent,
}

impl CreatedIdentityProvenance {
    /// The provenance of a create the host plane committed under an allow
    /// rule, as operation `operation_id` at `at_ms`.
    #[must_use]
    pub fn new(operation_id: impl Into<String>, at_ms: u64) -> Self {
        Self {
            entered: IdentityEntered::Created,
            operation_id: operation_id.into(),
            at_ms,
            requester: IdentityChangeRequester::Host {
                surface: "host".to_string(),
            },
            proposer: None,
            source_evidence: None,
            consent: IdentityChangeConsent::Policy {
                outcome: IdentityPolicyOutcome::Allow,
            },
        }
    }

    /// The provenance of an operation that wrote a row first.
    #[must_use]
    pub fn entered(
        entered: IdentityEntered,
        operation_id: impl Into<String>,
        at_ms: u64,
        requester: IdentityChangeRequester,
        consent: IdentityChangeConsent,
    ) -> Self {
        Self {
            entered,
            operation_id: operation_id.into(),
            at_ms,
            requester,
            proposer: None,
            source_evidence: None,
            consent,
        }
    }

    #[must_use]
    pub fn with_proposer(mut self, proposer: impl Into<String>) -> Self {
        self.proposer = Some(proposer.into());
        self
    }

    #[must_use]
    pub fn with_source_evidence(mut self, evidence: IdentitySourceEvidence) -> Self {
        self.source_evidence = Some(evidence);
        self
    }

    fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("operation_id", &self.operation_id)?;
        self.requester.validate()?;
        self.consent.validate()
    }
}

/// An approved profile change of a store-owned identity whose restamp has not
/// committed yet: the role its member moves from. The row keeps it until the
/// restamp commits, so a restart before it carries the move out again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PendingRoleMigration {
    pub from: ProfileName,
}

impl PendingRoleMigration {
    #[must_use]
    pub fn new(from: impl Into<ProfileName>) -> Self {
        Self { from: from.into() }
    }
}

/// An edge a change of this identity declared: the peer and the rule that
/// admitted it. The identity owns it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OwnedIdentityEdge {
    pub peer: AgentIdentity,
    pub rule: String,
}

impl OwnedIdentityEdge {
    #[must_use]
    pub fn new(peer: impl Into<AgentIdentity>, rule: impl Into<String>) -> Self {
        Self {
            peer: peer.into(),
            rule: rule.into(),
        }
    }
}

/// The removal that tombstoned an identity: its operation, time, requester
/// and consent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IdentityRemovalRecord {
    pub operation_id: String,
    pub at_ms: u64,
    pub requester: IdentityChangeRequester,
    pub consent: IdentityChangeConsent,
}

impl IdentityRemovalRecord {
    #[must_use]
    pub fn new(
        operation_id: impl Into<String>,
        at_ms: u64,
        requester: IdentityChangeRequester,
        consent: IdentityChangeConsent,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            at_ms,
            requester,
            consent,
        }
    }

    fn validate(&self) -> Result<(), IdentityIntentError> {
        validate_text("removal_operation_id", &self.operation_id)?;
        self.requester.validate()?;
        self.consent.validate()
    }
}

/// A store-owned identity's desired record: its row in the identity store's
/// intents table (schema version 2), beside the host-adopted identity intents
/// (version 1). The identity's custody is the store; its spec, revision,
/// presence and provenance are sealed in its authority digest. A row this
/// build cannot read (another schema version, a field it does not know, a
/// broken seal) reads unreadable for its identity alone
/// ([`CreatedIdentityObservation::Unreadable`]).
///
/// The row is complete with this release: spec, revision, presence, a
/// pending role migration, owned edges, provenance and a removal record. A
/// later change of its shape takes a new schema version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreatedIdentityRecord {
    pub schema_version: u32,
    /// The mob the row belongs to, sealed with it.
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    /// The spec's revision, from 1; each committed update adds one.
    pub revision: u64,
    pub presence: CreatedIdentityPresence,
    /// The identity's declared spec. Absent only on the tombstone a
    /// roster-owned identity's removal wrote, and on a row written before
    /// the row carried its spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<CreatedIdentitySpec>,
    pub provenance: CreatedIdentityProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_role_migration: Option<PendingRoleMigration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owned_edges: Vec<OwnedIdentityEdge>,
    /// The removal that tombstoned the identity, once one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removal: Option<IdentityRemovalRecord>,
    /// Digest of the complete store-sealed row, itself excluded.
    pub authority_digest: String,
}

impl CreatedIdentityRecord {
    /// A sealed record of `agent_identity`'s row in `mob_id`, with no spec.
    pub fn sealed(
        mob_id: MobId,
        agent_identity: AgentIdentity,
        revision: u64,
        presence: CreatedIdentityPresence,
        provenance: CreatedIdentityProvenance,
    ) -> Result<Self, IdentityIntentError> {
        Self {
            schema_version: CREATED_IDENTITY_SCHEMA_VERSION,
            mob_id,
            agent_identity,
            revision,
            presence,
            spec: None,
            provenance,
            pending_role_migration: None,
            owned_edges: Vec::new(),
            removal: None,
            authority_digest: String::new(),
        }
        .resealed()
    }

    /// A sealed record of a store-owned identity's row for `spec`, at
    /// `revision`.
    pub fn with_spec(
        mob_id: MobId,
        spec: CreatedIdentitySpec,
        revision: u64,
        presence: CreatedIdentityPresence,
        provenance: CreatedIdentityProvenance,
    ) -> Result<Self, IdentityIntentError> {
        Self {
            schema_version: CREATED_IDENTITY_SCHEMA_VERSION,
            mob_id,
            agent_identity: spec.identity.clone(),
            revision,
            presence,
            spec: Some(spec),
            provenance,
            pending_role_migration: None,
            owned_edges: Vec::new(),
            removal: None,
            authority_digest: String::new(),
        }
        .resealed()
    }

    /// The row with its pending role migration set (or cleared), sealed
    /// again.
    pub fn with_pending_role_migration(
        mut self,
        pending: Option<PendingRoleMigration>,
    ) -> Result<Self, IdentityIntentError> {
        self.pending_role_migration = pending;
        self.resealed()
    }

    /// The row with its removal record set, sealed again.
    pub fn with_removal(
        mut self,
        removal: IdentityRemovalRecord,
    ) -> Result<Self, IdentityIntentError> {
        self.removal = Some(removal);
        self.resealed()
    }

    /// The row with its owned edges set, sealed again.
    pub fn with_owned_edges(
        mut self,
        mut edges: Vec<OwnedIdentityEdge>,
    ) -> Result<Self, IdentityIntentError> {
        edges.sort();
        edges.dedup();
        self.owned_edges = edges;
        self.resealed()
    }

    /// The row after an update: `spec` at the next revision, sealed again.
    pub(crate) fn updated(
        mut self,
        spec: CreatedIdentitySpec,
        pending_role_migration: Option<PendingRoleMigration>,
    ) -> Result<Self, IdentityIntentError> {
        self.revision =
            self.revision
                .checked_add(1)
                .ok_or(IdentityIntentError::CounterExhausted {
                    counter: "created identity revision",
                })?;
        self.spec = Some(spec);
        self.pending_role_migration = pending_role_migration;
        self.resealed()
    }

    /// The row with `presence`, sealed again.
    pub(crate) fn with_presence(
        mut self,
        presence: CreatedIdentityPresence,
    ) -> Result<Self, IdentityIntentError> {
        self.presence = presence;
        self.resealed()
    }

    /// Seal the row's current content: compute its authority digest and
    /// validate it.
    pub(crate) fn resealed(mut self) -> Result<Self, IdentityIntentError> {
        self.authority_digest = String::new();
        self.authority_digest = self.canonical_authority_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != CREATED_IDENTITY_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "created_identity",
                version: self.schema_version,
            });
        }
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_identity(&self.agent_identity)?;
        if self.revision == 0 {
            return Err(IdentityIntentError::ZeroIntentRevision);
        }
        self.provenance.validate()?;
        if let Some(spec) = &self.spec {
            spec.validate()?;
            if spec.identity != self.agent_identity {
                return Err(IdentityIntentError::InvalidCreatedIdentity(
                    "the row's spec names another identity than the row".to_string(),
                ));
            }
        }
        if let Some(removal) = &self.removal {
            removal.validate()?;
        }
        match (&self.presence, &self.removal) {
            (CreatedIdentityPresence::Removed, _) | (_, None) => {}
            (_, Some(_)) => {
                return Err(IdentityIntentError::InvalidCreatedIdentity(
                    "only a removed identity's row records a removal".to_string(),
                ));
            }
        }
        if self.spec.is_none()
            && self.provenance.entered == IdentityEntered::RosterRemoval
            && self.presence != CreatedIdentityPresence::Removed
        {
            return Err(IdentityIntentError::InvalidCreatedIdentity(
                "a roster removal's row is a tombstone".to_string(),
            ));
        }
        if self.canonical_authority_digest()? != self.authority_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }

    /// The digest of the row's spec, when the row carries one.
    pub fn spec_digest(&self) -> Result<Option<String>, IdentityIntentError> {
        self.spec
            .as_ref()
            .map(CreatedIdentitySpec::spec_digest)
            .transpose()
    }

    /// The digest of the row's sealed content: everything but the digest
    /// itself, under its own domain.
    pub fn canonical_authority_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct AuthorityMaterial<'a> {
            domain: &'static str,
            schema_version: u32,
            mob_id: &'a MobId,
            agent_identity: &'a AgentIdentity,
            revision: u64,
            presence: &'a CreatedIdentityPresence,
            #[serde(skip_serializing_if = "Option::is_none")]
            spec: Option<&'a CreatedIdentitySpec>,
            provenance: &'a CreatedIdentityProvenance,
            #[serde(skip_serializing_if = "Option::is_none")]
            pending_role_migration: Option<&'a PendingRoleMigration>,
            #[serde(skip_serializing_if = "<[OwnedIdentityEdge]>::is_empty")]
            owned_edges: &'a [OwnedIdentityEdge],
            #[serde(skip_serializing_if = "Option::is_none")]
            removal: Option<&'a IdentityRemovalRecord>,
        }
        let bytes = serde_json::to_vec(&AuthorityMaterial {
            domain: "meerkat.identity.created_authority.v2",
            schema_version: self.schema_version,
            mob_id: &self.mob_id,
            agent_identity: &self.agent_identity,
            revision: self.revision,
            presence: &self.presence,
            spec: self.spec.as_ref(),
            provenance: &self.provenance,
            pending_role_migration: self.pending_role_migration.as_ref(),
            owned_edges: &self.owned_edges,
            removal: self.removal.as_ref(),
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
}

/// How one row of the identity store's intents table reads to the created
/// identity scan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CreatedIdentityObservation {
    /// A store-owned identity's row this build reads and validates.
    Created(CreatedIdentityRecord),
    /// A row of schema version 2 or later that this build cannot read: the
    /// identity reads unreadable, alone, and nothing acts on it.
    Unreadable {
        evidence_digest: String,
        detail: String,
    },
}

/// Classify one stored row of the identity store's intents table for the
/// created identity scan. `None` for a host-adopted intent's row (schema
/// version 1) and for a row whose schema version cannot be read: the intent
/// readers report those ([`super::IdentityStoredObservation`]).
#[must_use]
pub fn classify_created_identity_row(bytes: &[u8]) -> Option<CreatedIdentityObservation> {
    let version = serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()?
        .get("schema_version")?
        .as_u64()?;
    if version < u64::from(CREATED_IDENTITY_SCHEMA_VERSION) {
        return None;
    }
    let unreadable = |detail: String| CreatedIdentityObservation::Unreadable {
        evidence_digest: sha256_digest(bytes),
        detail,
    };
    Some(
        match serde_json::from_slice::<CreatedIdentityRecord>(bytes) {
            Ok(record) => match record.validate() {
                Ok(()) => CreatedIdentityObservation::Created(record),
                Err(error) => unreadable(error.to_string()),
            },
            Err(error) => unreadable(format!(
                "created identity row (schema version {version}) does not match this build's \
                 schema: {error}"
            )),
        },
    )
}

/// The kind of an identity change ([`IdentityChangeReceipt::change`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityChangeKind {
    Create,
    Update,
    Archive,
    Unarchive,
    Remove,
    /// A host's settle of an identity's pending preparation.
    SettlePreparation,
    /// A roster-owned identity's change: the partial declared apply of the
    /// identity its host's roster writer wrote.
    RosterApply,
}

/// What a change did to one identity at its commit
/// ([`IdentityChangeReceiptEntry::outcome`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityChangeEntryOutcome {
    /// The change committed; the identity is seated or owed its seat.
    Committed,
    /// The identity waits for its host's preparation.
    PendingPreparation,
    /// The identity is hosted on its first use.
    HostedOnFirstUse,
    /// The identity awaits its held creation.
    AwaitingHeldCreation,
    /// The identity's member retires; its removal ends at the retirement's
    /// end.
    Retiring,
    /// The identity was removed: its name is tombstoned.
    Removed,
    /// The identity's member retires; its archive ends at the retirement's
    /// end.
    Archiving,
    /// The identity is archived.
    Archived,
    /// The identity is listed again, or unarchived.
    Restored,
}

/// One identity of a change's receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IdentityChangeReceiptEntry {
    pub identity: AgentIdentity,
    /// The identity's spec digest after the change, when the store holds
    /// its spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_digest: Option<String>,
    /// The identity's spec revision after the change, for a store-owned
    /// identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub outcome: IdentityChangeEntryOutcome,
}

impl IdentityChangeReceiptEntry {
    #[must_use]
    pub fn new(identity: AgentIdentity, outcome: IdentityChangeEntryOutcome) -> Self {
        Self {
            identity,
            spec_digest: None,
            revision: None,
            outcome,
        }
    }

    #[must_use]
    pub fn with_spec(mut self, spec_digest: String, revision: Option<u64>) -> Self {
        self.spec_digest = Some(spec_digest);
        self.revision = revision;
        self
    }
}

/// The receipt of one identity change: one per operation, keyed by its id in
/// its mob, inserted once, in the same store transaction as the change. A
/// retry with the same id and request digest replays it; another digest
/// under the same id is refused, and nothing changes. It records who asked
/// and the consent the change committed under, for every change (an
/// identity's row keeps only the provenance of the operation that wrote it
/// first, and its removal's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IdentityChangeReceipt {
    pub schema_version: u32,
    pub mob_id: MobId,
    pub operation_id: String,
    pub request_digest: String,
    pub change: IdentityChangeKind,
    /// Who asked for the change: recorded, never authority.
    pub requester: IdentityChangeRequester,
    /// The consent the change committed under.
    pub consent: IdentityChangeConsent,
    pub identities: Vec<IdentityChangeReceiptEntry>,
    /// When the change committed, in UNIX milliseconds.
    pub committed_at_ms: u64,
    /// Digest of the complete receipt, itself excluded.
    pub authority_digest: String,
}

impl IdentityChangeReceipt {
    /// A sealed receipt of a change the host plane committed under an allow
    /// rule ([`Self::with_attribution`] records another requester and
    /// consent).
    pub fn sealed(
        mob_id: MobId,
        operation_id: impl Into<String>,
        request_digest: impl Into<String>,
        change: IdentityChangeKind,
        identities: Vec<IdentityChangeReceiptEntry>,
        committed_at_ms: u64,
    ) -> Result<Self, IdentityIntentError> {
        Self {
            schema_version: IDENTITY_CHANGE_RECEIPT_SCHEMA_VERSION,
            mob_id,
            operation_id: operation_id.into(),
            request_digest: request_digest.into(),
            change,
            requester: IdentityChangeRequester::Host {
                surface: "host".to_string(),
            },
            consent: IdentityChangeConsent::Policy {
                outcome: IdentityPolicyOutcome::Allow,
            },
            identities,
            committed_at_ms,
            authority_digest: String::new(),
        }
        .resealed()
    }

    /// The receipt with its requester and consent, sealed again.
    pub fn with_attribution(
        mut self,
        requester: IdentityChangeRequester,
        consent: IdentityChangeConsent,
    ) -> Result<Self, IdentityIntentError> {
        self.requester = requester;
        self.consent = consent;
        self.resealed()
    }

    fn resealed(mut self) -> Result<Self, IdentityIntentError> {
        self.authority_digest = String::new();
        self.authority_digest = self.canonical_authority_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema_version != IDENTITY_CHANGE_RECEIPT_SCHEMA_VERSION {
            return Err(IdentityIntentError::UnsupportedSchemaVersion {
                record: "identity_change_receipt",
                version: self.schema_version,
            });
        }
        validate_text("mob_id", self.mob_id.as_str())?;
        validate_text("operation_id", &self.operation_id)?;
        validate_text("request_digest", &self.request_digest)?;
        self.requester.validate()?;
        self.consent.validate()?;
        if self.identities.is_empty() || self.identities.len() > IDENTITY_CHANGE_BATCH_MAX {
            return Err(IdentityIntentError::InvalidOperationReceipt);
        }
        for entry in &self.identities {
            validate_identity(&entry.identity)?;
        }
        if self.canonical_authority_digest()? != self.authority_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }

    /// The identity whose name keys the receipt in a store that keys
    /// receipts by one identity: the change's first.
    #[must_use]
    pub fn key_identity(&self) -> &AgentIdentity {
        &self.identities[0].identity
    }

    pub fn canonical_authority_digest(&self) -> Result<String, IdentityIntentError> {
        #[derive(Serialize)]
        struct AuthorityMaterial<'a> {
            domain: &'static str,
            schema_version: u32,
            mob_id: &'a MobId,
            operation_id: &'a str,
            request_digest: &'a str,
            change: IdentityChangeKind,
            requester: &'a IdentityChangeRequester,
            consent: &'a IdentityChangeConsent,
            identities: &'a [IdentityChangeReceiptEntry],
            committed_at_ms: u64,
        }
        let bytes = serde_json::to_vec(&AuthorityMaterial {
            domain: "meerkat.identity.change_receipt.v1",
            schema_version: self.schema_version,
            mob_id: &self.mob_id,
            operation_id: &self.operation_id,
            request_digest: &self.request_digest,
            change: self.change,
            requester: &self.requester,
            consent: &self.consent,
            identities: &self.identities,
            committed_at_ms: self.committed_at_ms,
        })
        .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
        Ok(sha256_digest(&bytes))
    }
}

/// A retry's answer from a receipt store
/// ([`IdentityChangeReceiptObservation`]): no receipt, the same operation
/// with the same digest, or the same id under another digest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityChangeReceiptObservation {
    Absent,
    Same(IdentityChangeReceipt),
    Other(IdentityChangeReceipt),
}

impl IdentityChangeReceiptObservation {
    /// How `stored` (the receipt under an operation's id, if any) answers a
    /// request of `request_digest`.
    #[must_use]
    pub fn of(stored: Option<IdentityChangeReceipt>, request_digest: &str) -> Self {
        match stored {
            None => Self::Absent,
            Some(receipt) if receipt.request_digest == request_digest => Self::Same(receipt),
            Some(receipt) => Self::Other(receipt),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spec with every field set but the backend and the binding.
    fn alpha_spec() -> CreatedIdentitySpec {
        CreatedIdentitySpec::new("alpha", "worker")
            .with_addressability(IdentityAddressability::InternalOnly)
            .with_display_name("Alpha")
            .with_labels(BTreeMap::from([("team".to_string(), "blue".to_string())]))
            .with_context(serde_json::json!({"desk": "north"}))
            .with_additional_instructions(vec!["Be brief.".to_string()])
            .with_initial_message(ContentInput::from("Hello."))
            .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven)
            .with_placement(meerkat_contracts::WireHostRef("host-peer".to_string()))
    }

    /// A present store-owned row with every optional part set.
    fn alpha_row() -> CreatedIdentityRecord {
        let provenance = CreatedIdentityProvenance::entered(
            IdentityEntered::Created,
            "create-alpha",
            1_700_000_000_000,
            IdentityChangeRequester::Agent {
                identity: AgentIdentity::from("lead"),
            },
            IdentityChangeConsent::Policy {
                outcome: IdentityPolicyOutcome::Notify,
            },
        )
        .with_proposer("drafting-session")
        .with_source_evidence(IdentitySourceEvidence::new(
            "host-adapter",
            "ticket-7",
            "event-9",
            1_699_999_999_000,
        ));
        CreatedIdentityRecord::with_spec(
            MobId::from("mob"),
            alpha_spec(),
            3,
            CreatedIdentityPresence::Present,
            provenance,
        )
        .and_then(|row| row.with_pending_role_migration(Some(PendingRoleMigration::new("analyst"))))
        .and_then(|row| row.with_owned_edges(vec![OwnedIdentityEdge::new("beta", "team-peers")]))
        .expect("a sealed row")
    }

    /// The tombstone a roster-owned identity's removal writes: the store's
    /// first row of the name, with no spec.
    fn gamma_tombstone() -> CreatedIdentityRecord {
        let requester = IdentityChangeRequester::Host {
            surface: "console".to_string(),
        };
        let consent = IdentityChangeConsent::Policy {
            outcome: IdentityPolicyOutcome::Allow,
        };
        CreatedIdentityRecord::sealed(
            MobId::from("mob"),
            AgentIdentity::from("gamma"),
            1,
            CreatedIdentityPresence::Removed,
            CreatedIdentityProvenance::entered(
                IdentityEntered::RosterRemoval,
                "remove-gamma",
                5,
                requester.clone(),
                consent.clone(),
            ),
        )
        .and_then(|row| {
            row.with_removal(IdentityRemovalRecord::new(
                "remove-gamma",
                5,
                requester,
                consent,
            ))
        })
        .expect("a sealed tombstone")
    }

    /// The row's stored form is pinned: its fields, their names and forms,
    /// and the authority digest over them. The first release that writes
    /// rows of schema version 2 freezes this shape; a later change takes a
    /// new schema version.
    #[test]
    fn the_v2_row_schema_is_pinned() {
        let row = alpha_row();
        assert_eq!(
            serde_json::to_value(&row).expect("encode the row"),
            serde_json::json!({
                "schema_version": 2,
                "mob_id": "mob",
                "agent_identity": "alpha",
                "revision": 3,
                "presence": {"presence": "present"},
                "spec": {
                    "identity": "alpha",
                    "profile": "worker",
                    "addressability": "internal_only",
                    "display_name": "Alpha",
                    "labels": {"team": "blue"},
                    "context": {"desk": "north"},
                    "additional_instructions": ["Be brief."],
                    "initial_message": "Hello.",
                    "runtime_mode_override": "turn_driven",
                    "backend": null,
                    "binding": null,
                    "placement": "host-peer"
                },
                "provenance": {
                    "entered": {"kind": "created"},
                    "operation_id": "create-alpha",
                    "at_ms": 1_700_000_000_000_u64,
                    "requester": {"kind": "agent", "identity": "lead"},
                    "proposer": "drafting-session",
                    "source_evidence": {
                        "attestor": "host-adapter",
                        "subject": "ticket-7",
                        "source_ref": "event-9",
                        "observed_at_ms": 1_699_999_999_000_u64
                    },
                    "consent": {"form": "policy", "outcome": "notify"}
                },
                "pending_role_migration": {"from": "analyst"},
                "owned_edges": [{"peer": "beta", "rule": "team-peers"}],
                "authority_digest":
                    "sha256:0c67b480ecd87a386407d5d02ea364d0e2cf96e0da127b3374032ed5ec1204c9"
            })
        );
        assert_eq!(
            row.spec_digest().expect("the spec digest"),
            Some(
                "sha256:2027f0834a536aa6d978ec3cee0bde3cf254937c0f69ce30b89673ce85b64479"
                    .to_string()
            ),
            "the spec digest is the canonical digest of the roster row of the same spec"
        );

        let tombstone = gamma_tombstone();
        assert_eq!(
            serde_json::to_value(&tombstone).expect("encode the tombstone"),
            serde_json::json!({
                "schema_version": 2,
                "mob_id": "mob",
                "agent_identity": "gamma",
                "revision": 1,
                "presence": {"presence": "removed"},
                "provenance": {
                    "entered": {"kind": "roster_removal"},
                    "operation_id": "remove-gamma",
                    "at_ms": 5,
                    "requester": {"kind": "host", "surface": "console"},
                    "consent": {"form": "policy", "outcome": "allow"}
                },
                "removal": {
                    "operation_id": "remove-gamma",
                    "at_ms": 5,
                    "requester": {"kind": "host", "surface": "console"},
                    "consent": {"form": "policy", "outcome": "allow"}
                },
                "authority_digest":
                    "sha256:61295b263e58b28231f97de905d1b7da929ce3f0f415edf4b11d1292229703e5"
            })
        );

        // The two other presences' forms.
        let session =
            SessionId::parse("00000000-0000-4000-8000-000000000001").expect("a session id");
        assert_eq!(
            serde_json::to_value(CreatedIdentityPresence::Archived {
                archived_session: Some(session),
            })
            .expect("encode the presence"),
            serde_json::json!({
                "presence": "archived",
                "archived_session": "00000000-0000-4000-8000-000000000001"
            })
        );
        assert_eq!(
            serde_json::to_value(CreatedIdentityPresence::PendingPreparation {
                attempt: 2,
                constructed: true,
            })
            .expect("encode the presence"),
            serde_json::json!({"presence": "pending_preparation", "attempt": 2, "constructed": true})
        );
    }

    /// A row with a field this build does not know, or whose seal does not
    /// match its content, reads unreadable for its identity alone; a row of
    /// this build's shape reads back as written.
    #[test]
    fn a_v2_row_this_build_cannot_read_is_unreadable() {
        let row = alpha_row();
        let bytes = serde_json::to_vec(&row).expect("encode the row");
        assert_eq!(
            classify_created_identity_row(&bytes),
            Some(CreatedIdentityObservation::Created(row.clone()))
        );

        let mut later = serde_json::to_value(&row).expect("encode the row");
        later["spec"]["a_later_field"] = serde_json::json!(true);
        assert!(matches!(
            classify_created_identity_row(&serde_json::to_vec(&later).expect("encode")),
            Some(CreatedIdentityObservation::Unreadable { .. })
        ));

        let mut forged = serde_json::to_value(&row).expect("encode the row");
        forged["spec"]["profile"] = serde_json::json!("admin");
        assert!(matches!(
            classify_created_identity_row(&serde_json::to_vec(&forged).expect("encode")),
            Some(CreatedIdentityObservation::Unreadable { .. })
        ));
    }

    /// A receipt is replayed only under the same request digest.
    #[test]
    fn a_receipt_answers_the_same_digest_replayed_and_another_reused() {
        let receipt = IdentityChangeReceipt::sealed(
            MobId::from("mob"),
            "create-alpha",
            "sha256:request",
            IdentityChangeKind::Create,
            vec![IdentityChangeReceiptEntry::new(
                AgentIdentity::from("alpha"),
                IdentityChangeEntryOutcome::Committed,
            )],
            9,
        )
        .expect("a sealed receipt");
        assert_eq!(
            IdentityChangeReceiptObservation::of(Some(receipt.clone()), "sha256:request"),
            IdentityChangeReceiptObservation::Same(receipt.clone())
        );
        assert_eq!(
            IdentityChangeReceiptObservation::of(Some(receipt.clone()), "sha256:other"),
            IdentityChangeReceiptObservation::Other(receipt)
        );
        assert_eq!(
            IdentityChangeReceiptObservation::of(None, "sha256:request"),
            IdentityChangeReceiptObservation::Absent
        );
    }
}
