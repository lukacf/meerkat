//! The created identity set (0.9.11): every row of schema version 2 or later
//! of one mob (its store-owned identities, archived ones and the tombstones
//! of removed names), with each identity's session reference, as one
//! export. It carries the downgrade cycle (`docs/guides/upgrading.mdx`): a
//! host exports the set, purges the rows before a downgrade
//! (`MobHandle::purge_created_identities`, which needs the export's digest)
//! and imports the export after its roll-forward
//! (`MobHandle::import_created_identities`), which re-creates each record
//! and re-binds its session.
//!
//! The export holds each row as the store sealed it: its spec, revision,
//! presence, owned edges and provenance with its consent evidence. It is
//! complete without the host's decision store, and it holds no session
//! contents and no memory. A row this build cannot read is listed by its
//! identity and evidence digest only: no import restores it, and the purge
//! refuses while one remains.

use std::collections::BTreeMap;

use meerkat_core::SessionId;
use serde::{Deserialize, Serialize};

use super::created::{CreatedIdentityObservation, CreatedIdentityRecord};
use super::{IdentityIntentError, sha256_digest};
use crate::ids::{AgentIdentity, MobId};

/// The schema of a created identity set export
/// ([`CreatedIdentitySetExport::schema`]).
pub const CREATED_IDENTITY_SET_SCHEMA: &str = "meerkat.identity.created_set.v1";

/// Where an exported identity's session stands at the export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportedSessionState {
    /// The identity's member is bound to the session.
    Current,
    /// The identity has no member; the session is its archived session, the
    /// one its next seat revives.
    Archived,
}

/// An exported identity's session reference: the session id only, never its
/// contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ExportedSessionReference {
    pub session_id: SessionId,
    pub state: ExportedSessionState,
}

impl ExportedSessionReference {
    #[must_use]
    pub fn new(session_id: SessionId, state: ExportedSessionState) -> Self {
        Self { session_id, state }
    }
}

/// One identity of an export: its row as the store sealed it, and its
/// session reference when it has one (a tombstone has none, and neither has
/// an identity no member was ever seated for).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ExportedIdentity {
    pub record: CreatedIdentityRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<ExportedSessionReference>,
}

impl ExportedIdentity {
    #[must_use]
    pub fn new(record: CreatedIdentityRecord, session: Option<ExportedSessionReference>) -> Self {
        Self { record, session }
    }
}

/// A row of the set this build cannot read: listed by its identity and its
/// evidence digest, so the export's digest covers it. No import restores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct UnreadableIdentityRow {
    pub identity: AgentIdentity,
    pub evidence_digest: String,
}

/// The export of one mob's created identity set
/// ([`CREATED_IDENTITY_SET_SCHEMA`]). `export_digest` covers every row of
/// the set, by its identity and its seal (a readable row's authority digest,
/// an unreadable one's evidence digest), and nothing else: the purge
/// recomputes it over the rows its store holds and removes them only when
/// the two are equal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreatedIdentitySetExport {
    pub schema: String,
    pub mob_id: MobId,
    /// The readable rows, in identity order.
    pub identities: Vec<ExportedIdentity>,
    /// The rows this build cannot read, in identity order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<UnreadableIdentityRow>,
    pub export_digest: String,
}

impl CreatedIdentitySetExport {
    /// The export of `mob_id`'s rows, each readable one with its session
    /// reference, sealed with its digest.
    ///
    /// # Errors
    /// [`IdentityIntentError`] for a row of another mob, two rows of one
    /// identity, or a row that does not validate.
    pub fn sealed(
        mob_id: MobId,
        identities: Vec<ExportedIdentity>,
        unreadable: Vec<UnreadableIdentityRow>,
    ) -> Result<Self, IdentityIntentError> {
        let mut identities = identities;
        identities
            .sort_by(|left, right| left.record.agent_identity.cmp(&right.record.agent_identity));
        let mut unreadable = unreadable;
        unreadable.sort_by(|left, right| left.identity.cmp(&right.identity));
        let mut export = Self {
            schema: CREATED_IDENTITY_SET_SCHEMA.to_string(),
            mob_id,
            identities,
            unreadable,
            export_digest: String::new(),
        };
        export.export_digest = export.computed_digest()?;
        export.validate()?;
        Ok(export)
    }

    /// The digest of the rows the export holds.
    fn computed_digest(&self) -> Result<String, IdentityIntentError> {
        created_set_digest(
            &self.mob_id,
            self.identities
                .iter()
                .map(|exported| {
                    (
                        &exported.record.agent_identity,
                        exported.record.authority_digest.as_str(),
                    )
                })
                .chain(
                    self.unreadable
                        .iter()
                        .map(|row| (&row.identity, row.evidence_digest.as_str())),
                ),
        )
    }

    /// Check the export as an import reads it: its schema, each row (sealed,
    /// of this export's mob), one row per identity, and its digest.
    ///
    /// # Errors
    /// [`IdentityIntentError::InvalidCreatedIdentity`] for another schema, a
    /// row of another mob or a second row of an identity; the row's own
    /// error for a row that does not validate;
    /// [`IdentityIntentError::DigestMismatch`] for a digest that is not the
    /// rows'.
    pub fn validate(&self) -> Result<(), IdentityIntentError> {
        if self.schema != CREATED_IDENTITY_SET_SCHEMA {
            return Err(IdentityIntentError::InvalidCreatedIdentity(format!(
                "the export's schema '{}' is not {CREATED_IDENTITY_SET_SCHEMA}",
                self.schema
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for exported in &self.identities {
            let record = &exported.record;
            record.validate()?;
            if record.mob_id != self.mob_id {
                return Err(IdentityIntentError::InvalidCreatedIdentity(format!(
                    "the export of mob '{}' holds a row of mob '{}'",
                    self.mob_id, record.mob_id
                )));
            }
            if !seen.insert(record.agent_identity.clone()) {
                return Err(IdentityIntentError::InvalidCreatedIdentity(format!(
                    "the export holds two rows of '{}'",
                    record.agent_identity
                )));
            }
        }
        for row in &self.unreadable {
            super::validate_identity(&row.identity)?;
            super::validate_text("evidence_digest", &row.evidence_digest)?;
            if !seen.insert(row.identity.clone()) {
                return Err(IdentityIntentError::InvalidCreatedIdentity(format!(
                    "the export holds two rows of '{}'",
                    row.identity
                )));
            }
        }
        if self.computed_digest()? != self.export_digest {
            return Err(IdentityIntentError::DigestMismatch);
        }
        Ok(())
    }
}

/// The digest of a created identity set: SHA-256 over its domain, its mob
/// and each row's identity and seal, in identity order. The export seals
/// itself with it, and the purge recomputes it over the rows its store
/// holds.
pub(crate) fn created_set_digest<'a>(
    mob_id: &MobId,
    rows: impl IntoIterator<Item = (&'a AgentIdentity, &'a str)>,
) -> Result<String, IdentityIntentError> {
    #[derive(Serialize)]
    struct SetMaterial<'a> {
        domain: &'static str,
        mob_id: &'a MobId,
        rows: Vec<(&'a AgentIdentity, &'a str)>,
    }
    let ordered: BTreeMap<&AgentIdentity, &str> = rows.into_iter().collect();
    let bytes = serde_json::to_vec(&SetMaterial {
        domain: CREATED_IDENTITY_SET_SCHEMA,
        mob_id,
        rows: ordered.into_iter().collect(),
    })
    .map_err(|error| IdentityIntentError::Serialization(error.to_string()))?;
    Ok(sha256_digest(&bytes))
}

/// The digest of the set of rows a store reads (`observations`, by
/// identity): what the purge compares the export's digest with.
pub(crate) fn created_set_digest_of_rows(
    mob_id: &MobId,
    observations: &BTreeMap<AgentIdentity, CreatedIdentityObservation>,
) -> Result<String, IdentityIntentError> {
    created_set_digest(
        mob_id,
        observations.iter().map(|(identity, observation)| {
            (
                identity,
                match observation {
                    CreatedIdentityObservation::Created(record) => record.authority_digest.as_str(),
                    CreatedIdentityObservation::Unreadable {
                        evidence_digest, ..
                    } => evidence_digest.as_str(),
                },
            )
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{
        CreatedIdentityPresence, CreatedIdentityProvenance, CreatedIdentitySpec,
    };

    fn record(mob: &MobId, name: &str, presence: CreatedIdentityPresence) -> CreatedIdentityRecord {
        CreatedIdentityRecord::with_spec(
            mob.clone(),
            CreatedIdentitySpec::new(name, "worker"),
            1,
            presence,
            CreatedIdentityProvenance::new(format!("create-{name}"), 1),
        )
        .expect("a sealed row")
    }

    /// The export's digest is its rows' digest, whatever order they come in,
    /// and it changes with any row's seal.
    #[test]
    fn an_exports_digest_is_its_rows_digest_in_any_order() {
        let mob = MobId::from("mob");
        let alpha = record(&mob, "alpha", CreatedIdentityPresence::Present);
        let beta = record(&mob, "beta", CreatedIdentityPresence::Removed);
        let forward = CreatedIdentitySetExport::sealed(
            mob.clone(),
            vec![
                ExportedIdentity::new(alpha.clone(), None),
                ExportedIdentity::new(beta.clone(), None),
            ],
            Vec::new(),
        )
        .expect("an export");
        let backward = CreatedIdentitySetExport::sealed(
            mob.clone(),
            vec![
                ExportedIdentity::new(beta.clone(), None),
                ExportedIdentity::new(alpha.clone(), None),
            ],
            Vec::new(),
        )
        .expect("an export");
        assert_eq!(forward, backward);
        let rows = BTreeMap::from([
            (
                alpha.agent_identity.clone(),
                CreatedIdentityObservation::Created(alpha.clone()),
            ),
            (
                beta.agent_identity.clone(),
                CreatedIdentityObservation::Created(beta),
            ),
        ]);
        assert_eq!(
            created_set_digest_of_rows(&mob, &rows).expect("a digest"),
            forward.export_digest
        );
        let archived = record(
            &mob,
            "beta",
            CreatedIdentityPresence::Archived {
                archived_session: None,
            },
        );
        let changed = CreatedIdentitySetExport::sealed(
            mob,
            vec![
                ExportedIdentity::new(alpha, None),
                ExportedIdentity::new(archived, None),
            ],
            Vec::new(),
        )
        .expect("an export");
        assert_ne!(changed.export_digest, forward.export_digest);
    }

    /// An export whose digest is not its rows', or that holds a row of
    /// another mob or two rows of one identity, does not validate.
    #[test]
    fn an_incoherent_export_does_not_validate() {
        let mob = MobId::from("mob");
        let alpha = record(&mob, "alpha", CreatedIdentityPresence::Present);
        let mut export = CreatedIdentitySetExport::sealed(
            mob.clone(),
            vec![ExportedIdentity::new(alpha.clone(), None)],
            Vec::new(),
        )
        .expect("an export");
        export.export_digest = "sha256:0".to_string();
        assert_eq!(export.validate(), Err(IdentityIntentError::DigestMismatch));
        let other = record(
            &MobId::from("other"),
            "beta",
            CreatedIdentityPresence::Present,
        );
        assert!(matches!(
            CreatedIdentitySetExport::sealed(
                mob.clone(),
                vec![ExportedIdentity::new(other, None)],
                Vec::new(),
            ),
            Err(IdentityIntentError::InvalidCreatedIdentity(_))
        ));
        assert!(matches!(
            CreatedIdentitySetExport::sealed(
                mob,
                vec![ExportedIdentity::new(alpha.clone(), None)],
                vec![UnreadableIdentityRow {
                    identity: alpha.agent_identity,
                    evidence_digest: "sha256:1".to_string(),
                }],
            ),
            Err(IdentityIntentError::InvalidCreatedIdentity(_))
        ));
    }
}
