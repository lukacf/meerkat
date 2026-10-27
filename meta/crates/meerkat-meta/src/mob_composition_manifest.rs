//! Meta-owned composition provenance for a persistent mob storage path.
//!
//! Mob storage is event-sourced: a non-empty event log already carries the
//! `MobCreated` definition. Authoritative Meta launches bind resume to a
//! storage-minted definition snapshot through
//! [`meerkat_mob::MobBuilder::for_resume_verified`]; the supplied host definition
//! remains a preflight comparison, never an alternate runtime authority. A bare
//! unverified resume would silently boot the definition recorded in storage even
//! after the operator edited their config. The boot would look healthy: right
//! member count, right phases, wrong composition.
//!
//! This module records what composition the storage path was created for and
//! whether that creator spoke for the durable composition. On resume, the
//! supplied definition is compared with the event log's current canonical
//! definition. The manifest remains provenance, not mutable semantic authority.
//!
//! A mob storage kept in a host store realm (see [`crate::host_store`]) keeps
//! its manifest there too, as the realm's `composition/<storage key>`
//! document, in the same format and judged by the same rules; its refusals
//! name the document by a `host-store:<provider>:composition/<storage key>`
//! location where a file manifest names its path.
//!
//! Comparison is structural rather than digest-based, deliberately. A digest
//! can only report "different", while the recorded definition can name which
//! fields diverged - and a digest over serialized bytes would mismatch
//! spuriously if serialization ordering ever changed, turning "fail loud on
//! divergence" into "fail loud on upgrade".

use std::path::{Path, PathBuf};

use meerkat_mob::MobDefinition;
use serde::{Deserialize, Serialize};

/// Current manifest schema version.
pub const MANIFEST_VERSION: u32 = 1;

/// File-name suffix of a composition manifest: [`manifest_path`] appends it
/// to the mob storage file name.
pub const MANIFEST_FILE_SUFFIX: &str = ".composition.json";

/// Provenance record written beside a persistent mob storage path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobCompositionManifest {
    /// Schema version of this file.
    pub manifest_version: u32,
    /// Version of the release that created the storage path.
    ///
    /// Also read under its predecessor (pre-0.9) spelling,
    /// [`crate::storage_predecessor::PREDECESSOR_MANIFEST_CREATOR_FIELD`]: the
    /// predecessor wrote this same version-1 schema with only this field
    /// named differently, beside every persistent mob storage it created.
    /// Written only as `created_by_meta`; predecessor adoption renames the
    /// field on disk. A manifest that carries both spellings is malformed:
    /// nothing here picks one.
    #[serde(alias = "created_by_mobkit")]
    pub created_by_meta: String,
    /// The composition the storage path was created for.
    pub definition: MobDefinition,
    /// Whether the launch that created this storage spoke for the durable
    /// composition.
    ///
    /// Defaulted rather than version-gated: manifests written before this field
    /// existed were all written by authoritative launches, because a
    /// non-authoritative launch did not record at all, so `Authoritative` is
    /// the accurate reading of an absent field rather than a fallback.
    #[serde(default)]
    pub created_by_authority: CompositionAuthority,
    /// Whether this manifest was written with the released synthesized profile
    /// normalization. Absent on old manifests; new writers always serialize
    /// `false` so their own development version cannot imply legacy provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_synthesized_profile_normalization: Option<bool>,
}

/// Where the manifest lives for a given mob storage path: beside it, with the
/// storage file name preserved so two mobs on one directory cannot collide.
pub fn manifest_path(mob_storage_path: &Path) -> PathBuf {
    let mut file_name = mob_storage_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mob".to_string());
    file_name.push_str(MANIFEST_FILE_SUFFIX);
    mob_storage_path.with_file_name(file_name)
}

/// Fail-closed composition provenance failure. Every variant is an init-time
/// refusal raised *before* the mob actuates.
#[derive(Debug)]
pub enum MobCompositionProvenanceError {
    /// Non-empty storage with no provenance record beside it.
    Missing { manifest: PathBuf, storage: PathBuf },
    /// The provenance record exists but could not be read.
    Unreadable { manifest: PathBuf, message: String },
    /// The provenance record was read but is not a manifest.
    Malformed { manifest: PathBuf, message: String },
    /// The provenance record is a manifest of a schema this build cannot judge.
    UnsupportedVersion {
        manifest: PathBuf,
        found: u32,
        supported: u32,
    },
    /// The supplied composition differs from the current canonical definition
    /// in the storage.
    Divergent {
        manifest: PathBuf,
        fields: Vec<String>,
    },
    /// The provenance record could not be written at create time.
    NotRecorded { manifest: PathBuf, message: String },
    /// The storage was created by a launch that did not speak for the durable
    /// composition, so no authoritative composition can take effect on it.
    CreatedByRehearsal { manifest: PathBuf, storage: PathBuf },
    /// A candidate (non-authoritative) launch supplied a definition that
    /// differs from the stored one it would boot. A resume cannot apply the
    /// supplied definition, so booting would run the stored composition while
    /// presenting the supplied one. Refused unless the launch acknowledges
    /// the stored definition ([`CandidateDefinition::Stored`]).
    CandidateDivergent { fields: Vec<String> },
    /// A storage arrived already holding events with nothing declared about
    /// what it is.
    ///
    /// `MobBootstrapSpec::new` takes arbitrary storage, so an undeclared
    /// non-empty log could be a durable database an external embedder opened
    /// directly. Resuming it would skip composition verification entirely, and
    /// no default claim can be safely inferred here: the constructor cannot
    /// know what the caller handed it.
    UnprovenStorage,
}

impl std::fmt::Display for MobCompositionProvenanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { manifest, storage } => write!(
                f,
                "the mob storage at {} already holds events but has no composition \
                 provenance at {}, so this build cannot tell whether resuming it would \
                 boot the composition you supplied or an older one (refusing before the \
                 mob actuates; if the storage is known-good and matches your current \
                 config, remove the storage to recreate it, or restore the manifest \
                 written beside it)",
                storage.display(),
                manifest.display()
            ),
            Self::Unreadable { manifest, message } => write!(
                f,
                "failed to read the mob composition provenance at {}: {} (resuming \
                 blind could boot a stale composition that looks healthy; fix the file \
                 permissions or restore the file)",
                manifest.display(),
                message
            ),
            Self::Malformed { manifest, message } => write!(
                f,
                "the mob composition provenance at {} is not a readable manifest: {} \
                 (resuming blind could boot a stale composition that looks healthy; \
                 restore the file from a backup, or inspect and repair it by hand; \
                 removing the mob storage instead recreates it empty and discards the \
                 mob's recorded events)",
                manifest.display(),
                message
            ),
            Self::UnsupportedVersion {
                manifest,
                found,
                supported,
            } => write!(
                f,
                "the mob composition provenance at {} is schema version {found}, but \
                 this build understands version {supported} and cannot judge whether \
                 the stored composition matches the one supplied (upgrade the gateway \
                 to a build that understands version {found}, or remove the mob storage \
                 to recreate it)",
                manifest.display()
            ),
            Self::Divergent { manifest, fields } => write!(
                f,
                "the supplied mob definition diverges from the canonical definition in this \
                 storage, in: {} (booting would silently run the event-log definition rather \
                 than the supplied composition; revert the change or advance it through the \
                 declared definition-update ceremony; storage provenance is recorded at {})",
                fields.join(", "),
                manifest.display()
            ),
            Self::UnprovenStorage => write!(
                f,
                "a mob storage supplied to bootstrap already holds events but nothing \
                 was declared about what that storage is, so this build cannot verify \
                 that resuming it would boot the composition supplied rather than an \
                 older one; if it is durable, compose it through \
                 mob_composition_manifest::persistent_mob_storage and pass the returned \
                 provenance to MobBootstrapSpec::with_mob_storage_provenance, and if it \
                 is in-process only, declare that with \
                 MobBootstrapSpec::with_declared_ephemeral_mob_storage"
            ),
            Self::NotRecorded { manifest, message } => write!(
                f,
                "failed to record mob composition provenance at {}: {} (without it the \
                 next restart cannot prove the stored composition matches your config, \
                 so refusing now rather than leaving an unjudgeable storage path behind)",
                manifest.display(),
                message
            ),
            Self::CandidateDivergent { fields } => write!(
                f,
                "this candidate launch supplied a mob definition that differs from the one \
                 stored in its mob storage, in: {}. A candidate resume cannot apply the \
                 supplied definition, so booting would silently run the stored composition \
                 (for example without a tool deny the supplied config adds) while presenting \
                 yours. Make the candidate config match the stored definition, or run the \
                 candidate on its own storage path, or set \
                 runtime_options.mob_composition.candidate_definition = \"stored\" to boot \
                 the stored definition knowingly (logged and reported as degraded health)",
                fields.join(", ")
            ),
            Self::CreatedByRehearsal { manifest, storage } => write!(
                f,
                "the mob storage at {} was created by a launch that declared it does \
                 not speak for the durable composition (a candidate or certification \
                 pass), recorded at {}, so the composition you supplied can never take \
                 effect on it: a resume cannot apply a new definition, and the event \
                 log's MobCreated definition is the rehearsal one. Create the durable \
                 store from an authoritative launch and run the candidate against a \
                 separate rehearsal path",
                storage.display(),
                manifest.display()
            ),
        }
    }
}

impl std::error::Error for MobCompositionProvenanceError {}

impl MobCompositionProvenanceError {
    /// The refusal as structured data, for surfaces that report it to a client
    /// (the `data` of the gateway's `meta/init` composition provenance
    /// refusal). The message stays the human remedy; this carries what a
    /// client acts on, such as the diverged fields.
    pub fn refusal(&self) -> CompositionProvenanceRefusal {
        let mut refusal = CompositionProvenanceRefusal::of(self.refusal_kind());
        match self {
            Self::Missing { manifest, storage }
            | Self::CreatedByRehearsal { manifest, storage } => {
                refusal.manifest = Some(manifest.display().to_string());
                refusal.storage = Some(storage.display().to_string());
            }
            Self::Unreadable { manifest, .. }
            | Self::Malformed { manifest, .. }
            | Self::NotRecorded { manifest, .. } => {
                refusal.manifest = Some(manifest.display().to_string());
            }
            Self::UnsupportedVersion {
                manifest,
                found,
                supported,
            } => {
                refusal.manifest = Some(manifest.display().to_string());
                refusal.found_version = Some(*found);
                refusal.supported_version = Some(*supported);
            }
            Self::Divergent { manifest, fields } => {
                refusal.manifest = Some(manifest.display().to_string());
                refusal.fields.clone_from(fields);
            }
            Self::CandidateDivergent { fields } => refusal.fields.clone_from(fields),
            Self::UnprovenStorage => {}
        }
        refusal
    }

    /// The variant as a stable kind.
    pub const fn refusal_kind(&self) -> CompositionProvenanceRefusalKind {
        match self {
            Self::Missing { .. } => CompositionProvenanceRefusalKind::Missing,
            Self::Unreadable { .. } => CompositionProvenanceRefusalKind::Unreadable,
            Self::Malformed { .. } => CompositionProvenanceRefusalKind::Malformed,
            Self::UnsupportedVersion { .. } => CompositionProvenanceRefusalKind::UnsupportedVersion,
            Self::Divergent { .. } => CompositionProvenanceRefusalKind::Divergent,
            Self::NotRecorded { .. } => CompositionProvenanceRefusalKind::NotRecorded,
            Self::CreatedByRehearsal { .. } => CompositionProvenanceRefusalKind::CreatedByRehearsal,
            Self::CandidateDivergent { .. } => CompositionProvenanceRefusalKind::CandidateDivergent,
            Self::UnprovenStorage => CompositionProvenanceRefusalKind::UnprovenStorage,
        }
    }
}

/// Which composition provenance refusal fired. One kind per
/// [`MobCompositionProvenanceError`] variant; on the wire, its snake_case
/// name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompositionProvenanceRefusalKind {
    Missing,
    Unreadable,
    Malformed,
    UnsupportedVersion,
    Divergent,
    NotRecorded,
    CreatedByRehearsal,
    CandidateDivergent,
    UnprovenStorage,
}

/// A composition provenance refusal as structured data: its kind, plus the
/// facts the variant carries. Absent facts are omitted on the wire, except
/// `fields`, which is always present (empty unless the kind is `divergent` or
/// `candidate_divergent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CompositionProvenanceRefusal {
    pub kind: CompositionProvenanceRefusalKind,
    /// Dotted paths of the diverged definition fields
    /// (`profiles.lead.tools.deny`).
    #[serde(default)]
    pub fields: Vec<String>,
    /// The composition manifest path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// The mob storage path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    /// The manifest schema version found (`unsupported_version`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found_version: Option<u32>,
    /// The newest manifest schema version this build reads
    /// (`unsupported_version`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_version: Option<u32>,
}

impl CompositionProvenanceRefusal {
    const fn of(kind: CompositionProvenanceRefusalKind) -> Self {
        Self {
            kind,
            fields: Vec::new(),
            manifest: None,
            storage: None,
            found_version: None,
            supported_version: None,
        }
    }
}

/// Record the composition a fresh persistent storage path was created for.
///
/// Crate-internal, and called on the create branch ONLY. Exposing it would let
/// a caller stamp a new composition onto a non-empty store, which makes
/// [`verify_before_resume`] pass while `MobBuilder::for_resume` still boots the
/// definition the event log actually holds - a certified lie.
///
/// Called on the create branch only. Overwrites any stale record, since an
/// empty event log means no prior mob survives on this path.
pub(crate) fn record_on_create(
    mob_storage_path: &Path,
    definition: &MobDefinition,
    created_by_authority: CompositionAuthority,
) -> Result<(), MobCompositionProvenanceError> {
    let manifest = manifest_path(mob_storage_path);
    let bytes = manifest_bytes(&manifest, definition, created_by_authority)?;
    if let Some(parent) = manifest.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            MobCompositionProvenanceError::NotRecorded {
                manifest: manifest.clone(),
                message: err.to_string(),
            }
        })?;
    }
    std::fs::write(&manifest, bytes).map_err(|err| MobCompositionProvenanceError::NotRecorded {
        manifest: manifest.clone(),
        message: err.to_string(),
    })
}

/// The manifest a create records, as its location keeps it.
fn manifest_bytes(
    manifest: &Path,
    definition: &MobDefinition,
    created_by_authority: CompositionAuthority,
) -> Result<Vec<u8>, MobCompositionProvenanceError> {
    let record = MobCompositionManifest {
        manifest_version: MANIFEST_VERSION,
        created_by_meta: env!("CARGO_PKG_VERSION").to_string(),
        definition: definition.clone(),
        created_by_authority,
        legacy_synthesized_profile_normalization: Some(false),
    };
    serde_json::to_vec_pretty(&record).map_err(|err| MobCompositionProvenanceError::NotRecorded {
        manifest: manifest.to_path_buf(),
        message: err.to_string(),
    })
}

/// Provenance result for one exact released-profile compatibility check.
pub(crate) enum LegacyDefinitionResume {
    /// A released-version manifest proves the legacy representation.
    Recorded,
}

/// Verify the exact profile representation synthesized by MobKit 0.8.9–0.8.28.
///
/// This is deliberately narrower than ordinary definition comparison:
/// compatibility is available only for epoch 1 and only when a manifest proves
/// an old writer. A missing manifest cannot distinguish an old store from
/// recovery surgery on a current one, so it remains fail-closed. New stores
/// cannot use this path to turn an explicit operator unmask edit into a silent
/// no-op.
pub(crate) fn verify_legacy_synthesized_definition_before_resume(
    mob_storage_path: &Path,
    authority_epoch: u64,
    authoritative: &MobDefinition,
    supplied: &MobDefinition,
    legacy_supplied: &MobDefinition,
) -> Result<LegacyDefinitionResume, MobCompositionProvenanceError> {
    let manifest = manifest_path(mob_storage_path);
    if authority_epoch != 1 {
        return verify_authoritative_definition(&manifest, authoritative, supplied)
            .map(|()| LegacyDefinitionResume::Recorded);
    }
    let bytes = read_manifest_file(&manifest)?;
    judge_legacy_definition(
        manifest,
        mob_storage_path.to_path_buf(),
        bytes.as_deref(),
        authoritative,
        supplied,
        legacy_supplied,
    )
}

/// The manifest file at `manifest`, `None` when there is none.
fn read_manifest_file(manifest: &Path) -> Result<Option<Vec<u8>>, MobCompositionProvenanceError> {
    match std::fs::read(manifest) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(MobCompositionProvenanceError::Unreadable {
            manifest: manifest.to_path_buf(),
            message: err.to_string(),
        }),
    }
}

/// A manifest's bytes as a manifest of the schema this build judges: its
/// version first, so a future schema is `UnsupportedVersion` rather than
/// `Malformed`.
fn parse_manifest(
    manifest: &Path,
    bytes: &[u8],
) -> Result<MobCompositionManifest, MobCompositionProvenanceError> {
    let version = serde_json::from_slice::<ManifestVersionProbe>(bytes)
        .map(|probe| probe.manifest_version)
        .map_err(|err| MobCompositionProvenanceError::Malformed {
            manifest: manifest.to_path_buf(),
            message: err.to_string(),
        })?;
    if version != MANIFEST_VERSION {
        return Err(MobCompositionProvenanceError::UnsupportedVersion {
            manifest: manifest.to_path_buf(),
            found: version,
            supported: MANIFEST_VERSION,
        });
    }
    serde_json::from_slice::<MobCompositionManifest>(bytes).map_err(|err| {
        MobCompositionProvenanceError::Malformed {
            manifest: manifest.to_path_buf(),
            message: err.to_string(),
        }
    })
}

/// [`verify_legacy_synthesized_definition_before_resume`] over the epoch-1
/// storage's manifest `bytes` (`None`: there is none).
fn judge_legacy_definition(
    manifest: PathBuf,
    storage: PathBuf,
    bytes: Option<&[u8]>,
    authoritative: &MobDefinition,
    supplied: &MobDefinition,
    legacy_supplied: &MobDefinition,
) -> Result<LegacyDefinitionResume, MobCompositionProvenanceError> {
    let Some(bytes) = bytes else {
        return verify_authoritative_definition(&manifest, authoritative, supplied)
            .map(|()| LegacyDefinitionResume::Recorded);
    };
    let record = parse_manifest(&manifest, bytes)?;
    if !record.created_by_authority.speaks_for_composition() {
        return Err(MobCompositionProvenanceError::CreatedByRehearsal { manifest, storage });
    }
    let released_writer = match record.legacy_synthesized_profile_normalization {
        Some(value) => value,
        None => is_synthesizing_release(&record.created_by_meta),
    };
    if !released_writer {
        return verify_authoritative_definition(&manifest, authoritative, supplied)
            .map(|()| LegacyDefinitionResume::Recorded);
    }
    verify_authoritative_definition(&manifest, authoritative, legacy_supplied)?;
    Ok(LegacyDefinitionResume::Recorded)
}

fn is_synthesizing_release(version: &str) -> bool {
    let mut parts = version.split('.');
    let Some(major) = parts.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    let Some(minor) = parts.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    let Some(patch) = parts.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    parts.next().is_none() && major == 0 && minor == 8 && (9..29).contains(&patch)
}

/// Verify a non-empty storage path was created for the supplied composition.
///
/// Called on the resume branch, before the builder is constructed, so every
/// refusal lands before the mob can actuate.
pub fn verify_before_resume(
    mob_storage_path: &Path,
    authoritative: &MobDefinition,
    supplied: &MobDefinition,
) -> Result<(), MobCompositionProvenanceError> {
    let manifest = manifest_path(mob_storage_path);
    let bytes = read_manifest_file(&manifest)?;
    judge_resume_manifest(
        manifest,
        mob_storage_path.to_path_buf(),
        bytes.as_deref(),
        authoritative,
        supplied,
    )
}

/// [`verify_before_resume`] over the storage's manifest `bytes` (`None`:
/// there is none).
fn judge_resume_manifest(
    manifest: PathBuf,
    storage: PathBuf,
    bytes: Option<&[u8]>,
    authoritative: &MobDefinition,
    supplied: &MobDefinition,
) -> Result<(), MobCompositionProvenanceError> {
    let Some(bytes) = bytes else {
        verify_authoritative_definition(&manifest, authoritative, supplied)?;
        return Err(MobCompositionProvenanceError::Missing { manifest, storage });
    };
    // Read the version before the whole manifest: a future schema must produce
    // UnsupportedVersion, not Malformed, or the operator is told to restore a
    // file that is in fact newer than this build.
    let record = parse_manifest(&manifest, bytes)?;
    // Before any field comparison. A rehearsal-created store cannot host an
    // authoritative composition at all, so reporting which fields differ would
    // name a fixable-looking cause for something no edit to the definition can
    // fix.
    if !record.created_by_authority.speaks_for_composition() {
        return Err(MobCompositionProvenanceError::CreatedByRehearsal { manifest, storage });
    }
    verify_authoritative_definition(&manifest, authoritative, supplied)
}

/// Where a mob storage's composition manifest lives: beside its file, or in
/// the host store realm that keeps the storage. Crate-internal, derived from
/// the storage's provenance ([`MobStorageProvenance::manifest_location`]), so
/// a storage is always judged against its own manifest.
#[derive(Debug, Clone)]
pub(crate) enum ManifestLocation {
    /// The file beside the persistent mob storage at this path.
    File(PathBuf),
    /// The realm's `composition/<storage key>` document.
    HostStore {
        documents: crate::host_store_documents::MetaDocuments,
        storage_key: String,
    },
}

impl ManifestLocation {
    fn document_key(storage_key: &str) -> String {
        crate::host_store_documents::composition_key(storage_key)
    }

    /// The manifest's location as refusals name it: the file's path, or
    /// `host-store:<provider>:composition/<storage key>`.
    pub(crate) fn manifest_label(&self) -> PathBuf {
        match self {
            Self::File(path) => manifest_path(path),
            Self::HostStore {
                documents,
                storage_key,
            } => documents.label(&Self::document_key(storage_key)),
        }
    }

    /// The mob storage's location as refusals name it.
    fn storage_label(&self) -> PathBuf {
        match self {
            Self::File(path) => path.clone(),
            Self::HostStore {
                documents,
                storage_key,
            } => documents.label(storage_key),
        }
    }

    /// The realm document's bytes, `None` when there is none.
    async fn read_document(
        &self,
        documents: &crate::host_store_documents::MetaDocuments,
        storage_key: &str,
    ) -> Result<Option<Vec<u8>>, MobCompositionProvenanceError> {
        documents
            .read(&Self::document_key(storage_key))
            .await
            .map(|bytes| bytes.map(std::sync::Arc::unwrap_or_clone))
            .map_err(|error| MobCompositionProvenanceError::Unreadable {
                manifest: self.manifest_label(),
                message: error.to_string(),
            })
    }

    /// [`record_on_create`] at this location.
    pub(crate) async fn record_on_create(
        &self,
        definition: &MobDefinition,
        created_by_authority: CompositionAuthority,
    ) -> Result<(), MobCompositionProvenanceError> {
        match self {
            Self::File(path) => record_on_create(path, definition, created_by_authority),
            Self::HostStore {
                documents,
                storage_key,
            } => {
                let manifest = self.manifest_label();
                let bytes = manifest_bytes(&manifest, definition, created_by_authority)?;
                documents
                    .write(&Self::document_key(storage_key), Some(bytes))
                    .await
                    .map_err(|error| MobCompositionProvenanceError::NotRecorded {
                        manifest,
                        message: error.to_string(),
                    })
            }
        }
    }

    /// [`verify_before_resume`] at this location.
    pub(crate) async fn verify_before_resume(
        &self,
        authoritative: &MobDefinition,
        supplied: &MobDefinition,
    ) -> Result<(), MobCompositionProvenanceError> {
        match self {
            Self::File(path) => verify_before_resume(path, authoritative, supplied),
            Self::HostStore {
                documents,
                storage_key,
            } => {
                let bytes = self.read_document(documents, storage_key).await?;
                judge_resume_manifest(
                    self.manifest_label(),
                    self.storage_label(),
                    bytes.as_deref(),
                    authoritative,
                    supplied,
                )
            }
        }
    }

    /// [`verify_legacy_synthesized_definition_before_resume`] at this
    /// location.
    pub(crate) async fn verify_legacy_synthesized_definition_before_resume(
        &self,
        authority_epoch: u64,
        authoritative: &MobDefinition,
        supplied: &MobDefinition,
        legacy_supplied: &MobDefinition,
    ) -> Result<LegacyDefinitionResume, MobCompositionProvenanceError> {
        match self {
            Self::File(path) => verify_legacy_synthesized_definition_before_resume(
                path,
                authority_epoch,
                authoritative,
                supplied,
                legacy_supplied,
            ),
            Self::HostStore {
                documents,
                storage_key,
            } => {
                if authority_epoch != 1 {
                    return verify_authoritative_definition(
                        &self.manifest_label(),
                        authoritative,
                        supplied,
                    )
                    .map(|()| LegacyDefinitionResume::Recorded);
                }
                let bytes = self.read_document(documents, storage_key).await?;
                judge_legacy_definition(
                    self.manifest_label(),
                    self.storage_label(),
                    bytes.as_deref(),
                    authoritative,
                    supplied,
                    legacy_supplied,
                )
            }
        }
    }
}

/// Version-only view, so an unreadable *body* and an unjudgeable *schema* stay
/// distinguishable.
#[derive(Deserialize)]
struct ManifestVersionProbe {
    manifest_version: u32,
}

fn verify_authoritative_definition(
    manifest: &Path,
    authoritative: &MobDefinition,
    supplied: &MobDefinition,
) -> Result<(), MobCompositionProvenanceError> {
    let fields = diverged_definition_fields(authoritative, supplied);
    if fields.is_empty() {
        Ok(())
    } else {
        Err(MobCompositionProvenanceError::Divergent {
            manifest: manifest.to_path_buf(),
            fields,
        })
    }
}

/// Which definition fields differ, as dotted PATHS rather than top-level keys.
///
/// Reported at the deepest point where both sides are still objects, so a single
/// changed model pin reads `profiles.security.model` instead of `profiles`.
/// That granularity is the difference between an operator reading the diff and
/// declaring through it unread - a downstream app named this directly, and they are the
/// party who would be clicking past it on every activation.
///
/// Stops descending when either side is a non-object (a list, a scalar, or
/// absent) and reports the path itself: below that point "which element changed"
/// needs a keying rule this does not have, and inventing one would name fields
/// that do not exist.
///
/// Falls back to a whole-definition marker if either side will not serialize, so
/// a serialization failure cannot be read as "no divergence".
pub(crate) fn diverged_definition_fields(
    recorded: &MobDefinition,
    supplied: &MobDefinition,
) -> Vec<String> {
    let (Ok(recorded_value), Ok(supplied_value)) = (
        serde_json::to_value(recorded),
        serde_json::to_value(supplied),
    ) else {
        return if recorded == supplied {
            Vec::new()
        } else {
            vec!["<whole definition>".to_string()]
        };
    };
    let mut paths = Vec::new();
    collect_diverged_paths("", &recorded_value, &supplied_value, &mut paths);
    if paths.is_empty() && recorded_value != supplied_value {
        // Equality disagreed with the walk: report something rather than
        // claiming agreement.
        return vec!["<whole definition>".to_string()];
    }
    paths.sort_unstable();
    paths.dedup();
    paths
}

fn collect_diverged_paths(
    prefix: &str,
    recorded: &serde_json::Value,
    supplied: &serde_json::Value,
    out: &mut Vec<String>,
) {
    if recorded == supplied {
        return;
    }
    match (recorded.as_object(), supplied.as_object()) {
        (Some(recorded_map), Some(supplied_map)) => {
            let mut keys: Vec<&String> = recorded_map.keys().chain(supplied_map.keys()).collect();
            keys.sort_unstable();
            keys.dedup();
            for key in keys {
                let recorded_child = recorded_map.get(key);
                let supplied_child = supplied_map.get(key);
                if recorded_child == supplied_child {
                    continue;
                }
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match (recorded_child, supplied_child) {
                    (Some(left), Some(right)) => {
                        collect_diverged_paths(&path, left, right, out);
                    }
                    // Present on one side only: the path IS the difference.
                    _ => out.push(path),
                }
            }
        }
        // Not both objects, so this is as deep as a path can honestly go.
        _ => out.push(if prefix.is_empty() {
            "<whole definition>".to_string()
        } else {
            prefix.to_string()
        }),
    }
}

/// Whether a launch speaks for the durable composition of its mob storage.
///
/// The composition pin assumes one state directory sees one composition. That
/// assumption held for every launch shape I tested and fails for a normal one:
/// a candidate-then-promote pipeline boots a deliberately RESTRICTED composition
/// first, against the same state directory, to certify it. On a fresh store that
/// candidate boot CREATED the pin from the candidate-phase composition, and the
/// promoted boot - carrying the real composition - was then refused on every
/// field the two modes differ in. Measured in production: 929 supervisor
/// respawns of the refused boot before rollback.
///
/// The door cannot reach that state. The refusal happens at BOOT, before any
/// operator ceremony can run, and the composition the operator would declare
/// never existed in the store to begin with.
///
/// So a launch may say it does not speak for the durable composition. Such a
/// launch is not VERIFIED against the pin: it is not claiming to be the durable
/// composition, and a deliberately-restricted certification pass is exactly
/// that claim's opposite.
///
/// It still records provenance when it creates a store, tagged with the
/// authority that created it. Skipping the record instead would leave a store
/// whose composition nobody has spoken for, and the next authoritative resume
/// would then adopt ITS OWN definition as the record while
/// `MobBuilder::for_resume` boots the definition the event log actually holds.
/// That is the certified lie [`record_on_create`] warns about, reached by
/// omission rather than by a bad write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionAuthority {
    /// This launch speaks for the durable composition: it creates the pin when
    /// absent and is verified against it when present. The default, so a launch
    /// that says nothing keeps the protection - the inverse default would mean
    /// forgetting a flag silently loses the pin.
    #[default]
    Authoritative,
    /// This launch does NOT speak for the durable composition - a candidate,
    /// probe, or certification pass whose composition is intentionally not the
    /// one that should be pinned.
    ///
    /// Exempt from the PIN: it never writes or claims the composition pin, and
    /// the pin never refuses it. It is not exempt from presenting what it
    /// runs: a candidate resume boots the stored definition, so a supplied
    /// definition that differs refuses
    /// ([`MobCompositionProvenanceError::CandidateDivergent`]) unless the
    /// launch acknowledges the stored definition
    /// ([`CandidateDefinition::Stored`]).
    ///
    /// Not exempt from RECORDING. A store this launch created is tagged as
    /// rehearsal-created, and an authoritative resume of it is refused
    /// ([`MobCompositionProvenanceError::CreatedByRehearsal`]) because a resume
    /// structurally cannot apply the promoted composition.
    NonAuthoritative,
}

/// Which definition a candidate (non-authoritative) resume may boot when the
/// supplied definition differs from the stored one. A resume always boots the
/// stored definition; this only decides whether a difference refuses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDefinition {
    /// The supplied definition must match the stored one; any difference
    /// refuses with [`MobCompositionProvenanceError::CandidateDivergent`].
    #[default]
    RequireMatch,
    /// Boot the stored definition despite a difference, knowingly: the
    /// diverged fields are logged at WARN and reported as degraded health.
    Stored,
}

/// Judge a candidate resume from `fields`, the [`diverged_definition_fields`]
/// of the stored definition the resume will boot against the supplied one
/// (empty when they match, after the released-representation allowance). A
/// difference refuses under [`CandidateDefinition::RequireMatch`] and is
/// returned for reporting under [`CandidateDefinition::Stored`].
///
/// The comparison is structural over the serialized definition: maps compare
/// by key, lists by value in order, so a reordered list (a `tools.deny`
/// written in another order, say) counts as a difference, as it does for the
/// authoritative pin.
pub fn verify_candidate_resume(
    fields: Vec<String>,
    policy: CandidateDefinition,
) -> Result<Vec<String>, MobCompositionProvenanceError> {
    if fields.is_empty() || policy == CandidateDefinition::Stored {
        Ok(fields)
    } else {
        Err(MobCompositionProvenanceError::CandidateDivergent { fields })
    }
}

impl CompositionAuthority {
    /// Whether this launch participates in composition pin semantics at all.
    #[must_use]
    pub fn speaks_for_composition(self) -> bool {
        matches!(self, Self::Authoritative)
    }
}

/// What a composed mob storage is, as declared by whoever composed it.
///
/// `MobStorage` does not report its own durability, so this cannot be derived
/// and has to be declared. The default is "unspecified" rather than an
/// ephemeral claim deliberately: `MobBootstrapSpec::new` accepts arbitrary
/// storage, so claiming ephemerality would assert a fact the constructor
/// cannot know, and an external embedder passing durable storage would
/// silently inherit that claim.
///
/// Opaque on purpose. The persistent form is only constructible inside this
/// crate, by [`persistent_mob_storage`], which returns it paired with the
/// storage it describes. A public `Persistent { path }` would let a caller
/// pair storage A with storage B's manifest path and have bootstrap verify the
/// wrong composition, which is exactly the guarantee the paired constructor
/// exists to provide.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MobStorageProvenance(Provenance);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Provenance {
    /// Nothing was declared. Safe while the event log is empty (a create needs
    /// no provenance), and fails closed the moment a non-empty log would be
    /// resumed unverified.
    #[default]
    Unspecified,
    /// In-memory by declaration: nothing survives the process, so there is no
    /// cross-restart composition to judge. A pre-seeded in-memory storage may
    /// resume without composition verification, because the composition it
    /// would be checked against was built in this same process.
    DeclaredEphemeral,
    /// Persistent at this path: provenance is recorded on create and verified
    /// before any resume.
    Persistent { path: PathBuf },
    /// Persistent in a host store realm, as the storage named `storage_key`:
    /// provenance is recorded on create and verified before any resume, in
    /// the realm's own documents.
    HostStore {
        storage_key: String,
        documents: crate::host_store_documents::MetaDocuments,
    },
}

impl MobStorageProvenance {
    /// Declare that the storage lives only for this process.
    ///
    /// This is a claim the caller is making about storage this crate cannot
    /// inspect. It is the deliberate escape for in-process callers and for
    /// launches that would rather keep an editable definition than durable mob
    /// state; it is never a default.
    pub fn declared_ephemeral() -> Self {
        Self(Provenance::DeclaredEphemeral)
    }

    /// Crate-internal: only [`persistent_mob_storage`] may say a storage is
    /// persistent, and only about the path it just opened.
    fn persistent(path: PathBuf) -> Self {
        Self(Provenance::Persistent { path })
    }

    /// The path when this storage is persistent in a file. A storage kept in
    /// a host store realm is persistent without a path
    /// ([`Self::is_persistent`]).
    pub fn persistent_path(&self) -> Option<&Path> {
        match &self.0 {
            Provenance::Unspecified
            | Provenance::DeclaredEphemeral
            | Provenance::HostStore { .. } => None,
            Provenance::Persistent { path } => Some(path),
        }
    }

    /// Whether this storage outlives the process: persistent at a path, or
    /// kept in a host store realm.
    pub fn is_persistent(&self) -> bool {
        matches!(
            self.0,
            Provenance::Persistent { .. } | Provenance::HostStore { .. }
        )
    }

    /// Whether this storage is kept in a host store realm.
    pub(crate) fn is_host_store(&self) -> bool {
        matches!(self.0, Provenance::HostStore { .. })
    }

    /// Where this storage's composition manifest lives; `None` for a storage
    /// that does not outlive the process.
    pub(crate) fn manifest_location(&self) -> Option<ManifestLocation> {
        match &self.0 {
            Provenance::Unspecified | Provenance::DeclaredEphemeral => None,
            Provenance::Persistent { path } => Some(ManifestLocation::File(path.clone())),
            Provenance::HostStore {
                storage_key,
                documents,
            } => Some(ManifestLocation::HostStore {
                documents: documents.clone(),
                storage_key: storage_key.clone(),
            }),
        }
    }

    /// Whether the caller said what the storage is: persistent at a path, or
    /// in-memory by declaration.
    pub(crate) fn is_declared(&self) -> bool {
        !matches!(self.0, Provenance::Unspecified)
    }

    /// Whether a non-empty event log may be resumed under this declaration.
    ///
    /// Only the unspecified form is refused: an undeclared storage could be
    /// durable, and resuming it would skip composition verification entirely.
    pub fn permits_unverified_resume(&self) -> bool {
        !matches!(self.0, Provenance::Unspecified)
    }
}

/// The mob storage of a host store realm's one root mob
/// ([`crate::host_store::ROOT_MOB_STORAGE_KEY`]) paired with its provenance:
/// Core's storage over the realm, whose identity-member transaction commits
/// every member spawn and wiring change as one batch over the mob and
/// identity scopes, and the realm's documents for its composition manifest.
/// The paired counterpart of [`persistent_mob_storage`]; the provenance is
/// constructible nowhere else.
pub(crate) async fn host_store_mob_storage(
    realm: &crate::host_store::HostStoreRealm,
) -> Result<(meerkat_mob::MobStorage, MobStorageProvenance), meerkat_mob::store::MobStoreError> {
    let storage_key = crate::host_store::ROOT_MOB_STORAGE_KEY;
    let stores = realm.mob_host().open(storage_key).await?;
    Ok((
        stores.storage(),
        MobStorageProvenance(Provenance::HostStore {
            storage_key: storage_key.to_string(),
            documents: realm.documents().clone(),
        }),
    ))
}

/// The one way Meta composes persistent mob storage.
///
/// Returns the storage paired with its provenance, so a persistent mob storage
/// cannot be composed anywhere in Meta without the record that makes it
/// judgeable on the next restart. Three launch paths previously composed mob
/// storage independently and drifted; pairing the two facts in one return
/// value is what keeps them from drifting again.
pub fn persistent_mob_storage(
    path: PathBuf,
) -> Result<(meerkat_mob::MobStorage, MobStorageProvenance), meerkat_mob::MobError> {
    let storage = meerkat_mob::MobStorage::persistent(&path)?;
    Ok((storage, MobStorageProvenance::persistent(path)))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wire(error: &MobCompositionProvenanceError) -> serde_json::Value {
        serde_json::to_value(error.refusal()).expect("refusal serializes")
    }

    /// The `meta/init` refusal data is a wire contract the SDKs read: one
    /// snake_case kind per variant, the variant's facts, and `fields` always
    /// present.
    #[test]
    fn every_refusal_has_a_stable_kind_and_its_facts_on_the_wire() {
        let manifest = PathBuf::from("/state/mob.sqlite3.composition.json");
        let storage = PathBuf::from("/state/mob.sqlite3");
        let cases = [
            (
                MobCompositionProvenanceError::Missing {
                    manifest: manifest.clone(),
                    storage: storage.clone(),
                },
                json!({
                    "kind": "missing",
                    "fields": [],
                    "manifest": "/state/mob.sqlite3.composition.json",
                    "storage": "/state/mob.sqlite3"
                }),
            ),
            (
                MobCompositionProvenanceError::Unreadable {
                    manifest: manifest.clone(),
                    message: "permission denied".to_string(),
                },
                json!({
                    "kind": "unreadable",
                    "fields": [],
                    "manifest": "/state/mob.sqlite3.composition.json"
                }),
            ),
            (
                MobCompositionProvenanceError::Malformed {
                    manifest: manifest.clone(),
                    message: "expected value".to_string(),
                },
                json!({
                    "kind": "malformed",
                    "fields": [],
                    "manifest": "/state/mob.sqlite3.composition.json"
                }),
            ),
            (
                MobCompositionProvenanceError::UnsupportedVersion {
                    manifest: manifest.clone(),
                    found: 9,
                    supported: 1,
                },
                json!({
                    "kind": "unsupported_version",
                    "fields": [],
                    "manifest": "/state/mob.sqlite3.composition.json",
                    "found_version": 9,
                    "supported_version": 1
                }),
            ),
            (
                MobCompositionProvenanceError::Divergent {
                    manifest: manifest.clone(),
                    fields: vec!["profiles.lead.model".to_string()],
                },
                json!({
                    "kind": "divergent",
                    "fields": ["profiles.lead.model"],
                    "manifest": "/state/mob.sqlite3.composition.json"
                }),
            ),
            (
                MobCompositionProvenanceError::NotRecorded {
                    manifest: manifest.clone(),
                    message: "read-only file system".to_string(),
                },
                json!({
                    "kind": "not_recorded",
                    "fields": [],
                    "manifest": "/state/mob.sqlite3.composition.json"
                }),
            ),
            (
                MobCompositionProvenanceError::CreatedByRehearsal { manifest, storage },
                json!({
                    "kind": "created_by_rehearsal",
                    "fields": [],
                    "manifest": "/state/mob.sqlite3.composition.json",
                    "storage": "/state/mob.sqlite3"
                }),
            ),
            (
                MobCompositionProvenanceError::CandidateDivergent {
                    fields: vec![
                        "profiles.lead.tools.deny".to_string(),
                        "profiles.worker".to_string(),
                    ],
                },
                json!({
                    "kind": "candidate_divergent",
                    "fields": ["profiles.lead.tools.deny", "profiles.worker"]
                }),
            ),
            (
                MobCompositionProvenanceError::UnprovenStorage,
                json!({ "kind": "unproven_storage", "fields": [] }),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(wire(&error), expected, "{error:?}");
            let round_trip: CompositionProvenanceRefusal =
                serde_json::from_value(expected).expect("the wire form reads back");
            assert_eq!(round_trip, error.refusal());
        }
    }

    /// The gateway finds the refusal behind a bootstrap failure, also when
    /// the startup rollback that followed it failed too, and only there.
    #[test]
    fn a_bootstrap_failure_exposes_only_a_composition_refusal() {
        use crate::mob_handle_runtime::MobRuntimeError;
        use crate::unified_runtime::UnifiedRuntimeBootstrapError;

        let refused = UnifiedRuntimeBootstrapError::Mob(MobRuntimeError::CompositionProvenance(
            MobCompositionProvenanceError::CandidateDivergent {
                fields: vec!["profiles.lead.tools.deny".to_string()],
            },
        ));
        assert_eq!(
            refused
                .composition_provenance()
                .map(MobCompositionProvenanceError::refusal_kind),
            Some(CompositionProvenanceRefusalKind::CandidateDivergent)
        );

        let rolled_back = UnifiedRuntimeBootstrapError::ModuleStartupRollbackFailed {
            startup_error: Box::new(refused),
            rollback_error: MobRuntimeError::InvalidInput("rollback failed"),
        };
        assert_eq!(
            rolled_back
                .composition_provenance()
                .map(MobCompositionProvenanceError::refusal_kind),
            Some(CompositionProvenanceRefusalKind::CandidateDivergent)
        );

        for other in [
            UnifiedRuntimeBootstrapError::Mob(MobRuntimeError::InvalidConfig("bad".to_string())),
            UnifiedRuntimeBootstrapError::PreSpawnHook("hook".to_string()),
            UnifiedRuntimeBootstrapError::ModuleStartupThreadPanicked,
        ] {
            assert!(other.composition_provenance().is_none(), "{other}");
        }
    }

    fn definition(model: &str) -> MobDefinition {
        MobDefinition::from_toml(&format!(
            "[mob]\nid = \"predecessor-manifest\"\n\n[profiles.lead]\nmodel = \"{model}\"\n"
        ))
        .expect("test definition")
    }

    /// A version-1 manifest as the predecessor (pre-0.9) release wrote it:
    /// its creator field spelled `created_by_mobkit` and no normalization
    /// flag, exactly the shape of a MobKit 0.8.26 store.
    fn predecessor_manifest(definition: &MobDefinition, creator: &str) -> serde_json::Value {
        json!({
            "manifest_version": 1,
            "created_by_mobkit": creator,
            "definition": definition,
            "created_by_authority": "authoritative",
        })
    }

    fn write_manifest_beside(storage: &Path, manifest: &serde_json::Value) {
        std::fs::write(
            manifest_path(storage),
            serde_json::to_vec_pretty(manifest).expect("manifest serializes"),
        )
        .expect("write manifest");
    }

    /// The predecessor spelling of the creator field reads as the creator
    /// version, and a manifest is only ever written with the current one.
    #[test]
    fn a_predecessor_manifest_reads_its_creator_and_writes_the_current_spelling() {
        let record: MobCompositionManifest =
            serde_json::from_value(predecessor_manifest(&definition("gpt-5.5"), "0.8.26"))
                .expect("a predecessor manifest decodes");
        assert_eq!(record.created_by_meta, "0.8.26");
        assert_eq!(record.legacy_synthesized_profile_normalization, None);

        let written = serde_json::to_value(&record).expect("manifest serializes");
        assert_eq!(written["created_by_meta"], json!("0.8.26"));
        assert!(written.get("created_by_mobkit").is_none(), "{written}");
    }

    /// A store a predecessor release created resumes against its own
    /// manifest: no malformed refusal, and the recorded creator version
    /// still selects the released-representation allowance (a 0.8.26
    /// writer is a synthesizing release, a 0.9 writer is not).
    #[test]
    fn a_predecessor_manifest_is_judged_like_a_current_one() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = temp.path().join("event_log.sqlite3");
        let recorded = definition("gpt-5.5");
        write_manifest_beside(&storage, &predecessor_manifest(&recorded, "0.8.26"));
        verify_before_resume(&storage, &recorded, &recorded)
            .expect("a predecessor manifest verifies an unchanged definition");

        // The supplied definition differs, and only its released (legacy)
        // representation matches the stored one.
        let supplied = definition("gpt-5.4");
        assert!(matches!(
            verify_before_resume(&storage, &recorded, &supplied),
            Err(MobCompositionProvenanceError::Divergent { .. })
        ));
        assert!(matches!(
            verify_legacy_synthesized_definition_before_resume(
                &storage, 1, &recorded, &supplied, &recorded,
            ),
            Ok(LegacyDefinitionResume::Recorded)
        ));

        // The same file written by a 0.9 release earns no allowance.
        write_manifest_beside(
            &storage,
            &json!({
                "manifest_version": 1,
                "created_by_meta": "0.9.1",
                "definition": recorded,
                "created_by_authority": "authoritative",
            }),
        );
        assert!(matches!(
            verify_legacy_synthesized_definition_before_resume(
                &storage, 1, &recorded, &supplied, &recorded,
            ),
            Err(MobCompositionProvenanceError::Divergent { .. })
        ));
    }

    /// Both spellings at once is not a manifest this build can judge:
    /// nothing picks one.
    #[test]
    fn a_manifest_carrying_both_creator_spellings_is_malformed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = temp.path().join("event_log.sqlite3");
        let recorded = definition("gpt-5.5");
        let mut manifest = predecessor_manifest(&recorded, "0.8.26");
        manifest["created_by_meta"] = json!("0.9.1");
        write_manifest_beside(&storage, &manifest);
        let refusal = verify_before_resume(&storage, &recorded, &recorded);
        assert!(
            matches!(
                &refusal,
                Err(MobCompositionProvenanceError::Malformed { message, .. })
                    if message.contains("duplicate field")
            ),
            "{refusal:?}"
        );
    }
}
