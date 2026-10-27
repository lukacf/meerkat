//! The explicit authority transition for a persistent mob definition.
//!
//! Initial creation is exclusively Meerkat's `MobCreated` event at definition
//! epoch 1. Ordinary resume never updates authority: it reads and verifies the
//! latest canonical definition from Meerkat's event log, with the spec store
//! serving only as a checked projection.
//!
//! This ceremony is the sole sanctioned update path. An explicit operator
//! declaration compare-and-swaps the observed canonical definition epoch through
//! [`meerkat_mob::MobStorage::update_definition`]. Meerkat atomically appends the
//! successor `MobDefinitionUpdated` authority event and converges its spec
//! projection. Meta does not perform a projection-only resume update, append
//! raw events, rewrite creation provenance, or emit `MobCompleted`.
//!
//! Deliberately two-phase. [`propose_spec_update`] only READS, and reports what
//! an update would change; [`commit_spec_update`] writes. A single call that
//! silently accepted whatever it found would be indistinguishable from not
//! having the pin at all, and the point is that config evolution becomes a
//! DECLARED transition rather than refused drift.
//!
//! The compare-and-swap is not decoration. Between proposing and committing,
//! another process can run its own ceremony; the revision makes a stale
//! declaration fail closed instead of overwriting a definition the operator
//! never saw. The write routes only through [`meerkat_mob::MobStorage`], so
//! MobMachine authority, the canonical event, and the spec projection remain one
//! upstream-owned transaction.

use meerkat_mob::store::MobStoreError;
use meerkat_mob::{
    MobDefinition, MobDefinitionProjectionHealth, MobDefinitionProjectionMismatchKind, MobError,
    MobStorage,
};

/// Why a spec-update ceremony could not proceed.
///
/// Separated from the composition-provenance errors on purpose: those say "this
/// storage is not the one you think it is", which is never operator-clearable
/// by declaration. These are all states an operator can act on.
#[derive(Debug)]
#[non_exhaustive]
pub enum SpecUpdateError {
    /// The canonical definition could not be read.
    ReadFailed {
        mob_id: String,
        source: MobStoreError,
    },
    /// The event-log authority has no definition for this mob.
    NothingPinned { mob_id: String },
    /// The canonical definition was read, and it AGREES with the supplied definition.
    /// Committing would be a no-op; the caller asked to move a pin that is not
    /// blocking anything, which usually means the divergence was elsewhere.
    AlreadyMatching { mob_id: String, revision: u64 },
    /// The revision moved between proposing and committing. Fail closed: the
    /// operator declared agreement with a definition that is no longer current.
    RevisionMoved {
        mob_id: String,
        proposed_at: u64,
        found: u64,
    },
    /// The canonical definition and its spec projection disagree.
    DefinitionProjectionDisagreement {
        mob_id: String,
        authority_epoch: u64,
        projection_revision: u64,
        kind: MobDefinitionProjectionMismatchKind,
    },
    /// This Meta build does not understand the health state returned by Meerkat.
    UnrecognizedDefinitionHealth { mob_id: String },
    /// Meerkat rejected the authoritative definition transition.
    UpdateFailed {
        mob_id: String,
        source: Box<MobError>,
    },
    /// The declaration names one mob while the definition it carries names
    /// another. Both are operator-supplied, so neither is trusted over the other.
    MobIdMismatch {
        declared_for: String,
        definition_names: String,
    },
}

impl std::fmt::Display for SpecUpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadFailed { mob_id, source } => write!(
                f,
                "the canonical definition for mob {mob_id} could not be read: {source}"
            ),
            Self::NothingPinned { mob_id } => write!(
                f,
                "no canonical definition exists for mob {mob_id}, so nothing is pinned and there \
                 is nothing to declare; creating the mob records the definition normally"
            ),
            Self::AlreadyMatching { mob_id, revision } => write!(
                f,
                "the canonical definition for mob {mob_id} at revision {revision} already matches the \
                 supplied definition, so a declaration would change nothing; if a boot was \
                 refused, the divergence is not in the mob definition"
            ),
            Self::RevisionMoved {
                mob_id,
                proposed_at,
                found,
            } => write!(
                f,
                "the canonical definition for mob {mob_id} moved from revision {proposed_at} to \
                 {found} while the update was being declared, so the declaration refers to a \
                 definition that is no longer current; re-read and declare again"
            ),
            Self::DefinitionProjectionDisagreement {
                mob_id,
                authority_epoch,
                projection_revision,
                kind,
            } => write!(
                f,
                "mob {mob_id} canonical definition and spec projection disagree ({kind}): \
                 authority epoch {authority_epoch}, projection revision {projection_revision}"
            ),
            Self::UnrecognizedDefinitionHealth { mob_id } => write!(
                f,
                "mob {mob_id} returned a definition health state this Meta build cannot judge"
            ),
            Self::UpdateFailed { mob_id, source } => write!(
                f,
                "declaring the updated definition for mob {mob_id} failed: {source}"
            ),
            Self::MobIdMismatch {
                declared_for,
                definition_names,
            } => write!(
                f,
                "the declaration is for mob {declared_for} but the definition it carries names \
                 {definition_names}; refusing rather than moving a pin the operator did not name"
            ),
        }
    }
}

impl std::error::Error for SpecUpdateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadFailed { source, .. } => Some(source),
            Self::UpdateFailed { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

/// A read-only account of what declaring this definition would change.
///
/// Holds the revision it was observed at, which [`commit_spec_update`] uses as
/// its compare-and-swap precondition. Not `Clone`: one observation authorizes
/// one declaration.
#[derive(Debug)]
pub struct SpecUpdateProposal {
    mob_id: meerkat_mob::MobId,
    observed_revision: u64,
    declared: MobDefinition,
    diverged_fields: Vec<String>,
}

impl SpecUpdateProposal {
    /// Which definition fields differ from the canonical authority.
    #[must_use]
    pub fn diverged_fields(&self) -> &[String] {
        &self.diverged_fields
    }

    /// The revision this proposal is valid against.
    #[must_use]
    pub fn observed_revision(&self) -> u64 {
        self.observed_revision
    }

    /// The mob whose pin would move.
    #[must_use]
    pub fn mob_id(&self) -> &str {
        self.mob_id.as_str()
    }
}

/// Evidence that a definition epoch advanced, for the operator's log.
#[derive(Debug, Clone)]
pub struct SpecUpdateReceipt {
    /// The mob whose definition advanced.
    pub mob_id: String,
    /// The revision the divergence was observed at.
    pub previous_revision: u64,
    /// The revision the declared definition now holds.
    pub committed_revision: u64,
    /// The spec projection revision committed with the canonical definition.
    pub projection_revision: u64,
    /// The cursor of the canonical definition event.
    pub event_cursor: u64,
    /// The dotted definition paths that differed when the declaration was made.
    pub declared_fields: Vec<String>,
}

#[derive(Debug)]
struct CanonicalDefinitionWitness {
    definition: MobDefinition,
    revision: u64,
}

fn normalize_definition(definition: &MobDefinition) -> MobDefinition {
    let mut normalized = definition.clone();
    crate::mob_handle_runtime::auto_mark_declared_resume_overrides(&mut normalized);
    normalized
}

fn authority_epoch(
    mob_id: &str,
    health: Option<MobDefinitionProjectionHealth>,
    released_projection_ahead_expected_revision: Option<u64>,
) -> Result<u64, SpecUpdateError> {
    match health {
        Some(
            MobDefinitionProjectionHealth::Healthy {
                authority_epoch, ..
            }
            | MobDefinitionProjectionHealth::ProjectionMissing { authority_epoch }
            | MobDefinitionProjectionHealth::ProjectionStale {
                authority_epoch, ..
            },
        ) => Ok(authority_epoch),
        Some(MobDefinitionProjectionHealth::Diverged {
            authority_epoch: 1,
            projection_revision: 2,
            kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
        }) if released_projection_ahead_expected_revision == Some(1) => Ok(1),
        Some(MobDefinitionProjectionHealth::Diverged {
            authority_epoch,
            projection_revision,
            kind,
        }) => Err(SpecUpdateError::DefinitionProjectionDisagreement {
            mob_id: mob_id.to_string(),
            authority_epoch,
            projection_revision,
            kind,
        }),
        None => Err(SpecUpdateError::NothingPinned {
            mob_id: mob_id.to_string(),
        }),
        _ => Err(SpecUpdateError::UnrecognizedDefinitionHealth {
            mob_id: mob_id.to_string(),
        }),
    }
}

async fn observe_canonical_definition(
    storage: &MobStorage,
    mob_id: &str,
    released_projection_ahead_expected_revision: Option<u64>,
) -> Result<CanonicalDefinitionWitness, SpecUpdateError> {
    let read_health = || async {
        storage
            .definition_projection_health()
            .await
            .map_err(|source| SpecUpdateError::ReadFailed {
                mob_id: mob_id.to_string(),
                source,
            })
    };
    let before = authority_epoch(
        mob_id,
        read_health().await?,
        released_projection_ahead_expected_revision,
    )?;
    let definition = storage
        .created_definition()
        .await
        .map_err(|source| SpecUpdateError::ReadFailed {
            mob_id: mob_id.to_string(),
            source,
        })?
        .filter(|definition| definition.id.as_str() == mob_id)
        .ok_or_else(|| SpecUpdateError::NothingPinned {
            mob_id: mob_id.to_string(),
        })?;
    let after = authority_epoch(
        mob_id,
        read_health().await?,
        released_projection_ahead_expected_revision,
    )?;
    if before != after {
        return Err(SpecUpdateError::RevisionMoved {
            mob_id: mob_id.to_string(),
            proposed_at: before,
            found: after,
        });
    }
    Ok(CanonicalDefinitionWitness {
        definition,
        revision: after,
    })
}

fn map_update_error(mob_id: &str, error: MobError) -> SpecUpdateError {
    match error {
        MobError::SpecRevisionConflict {
            expected, actual, ..
        } => SpecUpdateError::RevisionMoved {
            mob_id: mob_id.to_string(),
            proposed_at: expected.unwrap_or(actual),
            found: actual,
        },
        MobError::MobDefinitionProjectionMismatch {
            authority_epoch,
            projection_revision,
            kind,
            ..
        } => SpecUpdateError::DefinitionProjectionDisagreement {
            mob_id: mob_id.to_string(),
            authority_epoch,
            projection_revision,
            kind,
        },
        source => SpecUpdateError::UpdateFailed {
            mob_id: mob_id.to_string(),
            source: Box::new(source),
        },
    }
}

/// One-shot declaration: the operator already knows the revision they observed.
///
/// This is the shape the downstream app's activation machinery wants - the activation
/// payload names the diverged fields and the expected revision, the deploy
/// invokes this once, and the receipt goes into the activation evidence. The
/// two-phase [`propose_spec_update`] / [`commit_spec_update`] pair remains for
/// interactive use, where the operator needs to SEE the divergence first.
///
/// `expected_revision` is a precondition, not a hint. If the canonical definition is
/// at any other revision this refuses, because the operator declared agreement
/// with a definition they have not seen. That is the whole reason the revision is in
/// the payload rather than being read here and trusted.
///
/// `mob_id` is checked against `declared.id` rather than being trusted: a
/// payload naming one mob while carrying another mob's definition would
/// otherwise move the wrong pin, and both values are operator-supplied.
///
/// # Errors
///
/// See [`SpecUpdateError`].
pub async fn declare_spec_update(
    storage: &MobStorage,
    mob_id: &str,
    declared: &MobDefinition,
    expected_revision: u64,
) -> Result<SpecUpdateReceipt, SpecUpdateError> {
    if declared.id.as_str() != mob_id {
        return Err(SpecUpdateError::MobIdMismatch {
            declared_for: mob_id.to_string(),
            definition_names: declared.id.as_str().to_string(),
        });
    }
    let declared = normalize_definition(declared);
    // Meta admits only the exact projection-only residue produced by the
    // released epoch-1 updater. Meerkat remains responsible for atomically
    // verifying projection content and storage capability before repair.
    let witness = observe_canonical_definition(storage, mob_id, Some(expected_revision)).await?;
    let exact_replay = expected_revision
        .checked_add(1)
        .is_some_and(|next| next == witness.revision && witness.definition == declared);
    if witness.revision != expected_revision && !exact_replay {
        return Err(SpecUpdateError::RevisionMoved {
            mob_id: mob_id.to_string(),
            proposed_at: expected_revision,
            found: witness.revision,
        });
    }
    let diverged_fields =
        crate::mob_composition_manifest::diverged_definition_fields(&witness.definition, &declared);
    if diverged_fields.is_empty() && !exact_replay {
        return Err(SpecUpdateError::AlreadyMatching {
            mob_id: mob_id.to_string(),
            revision: witness.revision,
        });
    }
    let committed = storage
        .update_definition(expected_revision, declared)
        .await
        .map_err(|error| map_update_error(mob_id, error))?;
    Ok(SpecUpdateReceipt {
        mob_id: mob_id.to_string(),
        previous_revision: expected_revision,
        committed_revision: committed.epoch,
        projection_revision: committed.projection_revision,
        event_cursor: committed.event_cursor,
        declared_fields: diverged_fields,
    })
}

/// Read the canonical definition and report what declaring `supplied` would change.
///
/// Reads only. `Err(AlreadyMatching)` when the pin is not the thing blocking the
/// boot - worth distinguishing, because an operator reaching for this door after
/// a refusal deserves to be told the refusal came from somewhere else.
///
/// # Errors
///
/// See [`SpecUpdateError`].
pub async fn propose_spec_update(
    storage: &MobStorage,
    supplied: &MobDefinition,
) -> Result<SpecUpdateProposal, SpecUpdateError> {
    let supplied = normalize_definition(supplied);
    let mob_id = supplied.id.clone();
    let witness = observe_canonical_definition(storage, mob_id.as_str(), None).await?;
    let diverged_fields =
        crate::mob_composition_manifest::diverged_definition_fields(&witness.definition, &supplied);
    if diverged_fields.is_empty() {
        return Err(SpecUpdateError::AlreadyMatching {
            mob_id: mob_id.as_str().to_string(),
            revision: witness.revision,
        });
    }
    Ok(SpecUpdateProposal {
        mob_id,
        observed_revision: witness.revision,
        declared: supplied,
        diverged_fields,
    })
}

/// Declare that the persisted spec now matches the proposed definition.
///
/// Compare-and-swapped on the revision the proposal was read at, so a spec that
/// moved underneath fails closed rather than being overwritten.
///
/// The commit routes only through Meerkat's typed definition-epoch seam. The
/// event-log authority, canonical event, and spec projection therefore advance
/// under upstream ownership; Meta does not rewrite the immutable creation
/// manifest or mutate the projection independently.
///
/// # Errors
///
/// See [`SpecUpdateError`].
pub async fn commit_spec_update(
    storage: &MobStorage,
    proposal: SpecUpdateProposal,
) -> Result<SpecUpdateReceipt, SpecUpdateError> {
    let mob_id_text = proposal.mob_id.as_str().to_string();
    let committed = storage
        .update_definition(proposal.observed_revision, proposal.declared)
        .await
        .map_err(|error| map_update_error(&mob_id_text, error))?;
    Ok(SpecUpdateReceipt {
        mob_id: mob_id_text,
        previous_revision: proposal.observed_revision,
        committed_revision: committed.epoch,
        projection_revision: committed.projection_revision,
        event_cursor: committed.event_cursor,
        declared_fields: proposal.diverged_fields,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn definition(mob_id: &str, security_model: &str) -> MobDefinition {
        MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.general]
model = "gpt-5.5"

[profiles.security]
model = "{security_model}"
"#
        ))
        .expect("definition parses")
    }

    /// The downstream app's note 2, and the reason it matters: "profiles diverged" on a
    /// one-field model pin is the message that makes an operator declare through
    /// without reading. The path has to name the field that actually moved.
    ///
    /// Mutation check for whoever changes this: make `collect_diverged_paths`
    /// stop descending at the top level and this fails with `profiles`, which is
    /// exactly the old behaviour.
    #[test]
    fn a_single_changed_model_pin_names_the_field_not_the_whole_profiles_table() {
        let before = definition("ceremony-paths", "gpt-5.5");
        let after = definition("ceremony-paths", "gpt-5.6");

        let fields = crate::mob_composition_manifest::diverged_definition_fields(&before, &after);

        assert_eq!(
            fields.len(),
            1,
            "one changed pin must report one path, got {fields:?}"
        );
        let path = &fields[0];
        assert!(
            path.starts_with("profiles.security"),
            "the path must name the profile that changed: {fields:?}"
        );
        assert_ne!(
            path, "profiles",
            "reporting the whole profiles table is the message that gets declared through unread"
        );
        assert!(
            path.ends_with("model") || path.contains("model"),
            "the path must reach the field that moved, not stop at the profile: {fields:?}"
        );
    }

    /// An identical definition is not a divergence, and the walk must agree with
    /// equality. A walk that reported paths for equal inputs would make every
    /// boot look like drift.
    #[test]
    fn an_identical_definition_diverges_in_no_field() {
        let a = definition("ceremony-same", "gpt-5.5");
        let b = definition("ceremony-same", "gpt-5.5");
        assert!(
            crate::mob_composition_manifest::diverged_definition_fields(&a, &b).is_empty(),
            "equal definitions must report no diverged fields"
        );
    }

    /// A declaration whose payload names one mob while carrying another mob's
    /// definition must refuse. Both halves are operator-supplied, so trusting
    /// either would move a pin the operator did not name.
    #[tokio::test]
    async fn a_declaration_naming_a_different_mob_than_its_definition_refuses() {
        let temp = tempfile::tempdir().expect("temp dir");
        let db = temp.path().join("mob.sqlite");
        let storage = MobStorage::persistent(&db).expect("open storage");
        let declared = definition("ceremony-real", "gpt-5.6");

        let error = declare_spec_update(&storage, "ceremony-other", &declared, 1)
            .await
            .expect_err("a mismatched declaration must refuse");

        match error {
            SpecUpdateError::MobIdMismatch {
                declared_for,
                definition_names,
            } => {
                assert_eq!(declared_for, "ceremony-other");
                assert_eq!(definition_names, "ceremony-real");
            }
            other => panic!("expected MobIdMismatch, got {other:?}"),
        }
    }

    /// Nothing pinned is a distinct, actionable state - not an error to be
    /// laundered into "declare anyway". An operator reaching for the door on a
    /// store with no spec is being told the refusal came from somewhere else.
    #[tokio::test]
    async fn declaring_against_a_store_with_no_pinned_spec_says_so() {
        let temp = tempfile::tempdir().expect("temp dir");
        let db = temp.path().join("mob.sqlite");
        // Open once so the schema exists but no spec is written.
        let storage = MobStorage::persistent(&db).expect("stores open");
        let declared = definition("ceremony-empty", "gpt-5.6");

        let error = declare_spec_update(&storage, "ceremony-empty", &declared, 1)
            .await
            .expect_err("no pinned spec must not be silently declarable");

        match error {
            SpecUpdateError::NothingPinned { mob_id } => {
                assert_eq!(mob_id, "ceremony-empty");
            }
            other => panic!("expected NothingPinned, got {other:?}"),
        }
    }
}
