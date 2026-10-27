//! The first-boot manifest of a migrated Meta state directory.
//!
//! Meta 0.9.3 kept each identity's continuity in its own store: the session
//! the identity resumes, or that its continuity was deleted. Meta 0.9.4 reads
//! it from Core's mob instead, so `storage-migrate-native` has to leave Core's
//! mob saying the same thing. It cannot write Core's member bindings, so it
//! leaves this manifest: one row for each identity whose 0.9.3 continuity
//! Core's mob does not carry yet, for the first boot to carry out.
//!
//! - [`FirstBootRow::Resume`]: the identity continues on `session`, which
//!   Core's session store holds and Core's mob may not have bound to it yet.
//!   Carried out by resuming the identity onto that session.
//! - [`FirstBootRow::Fresh`]: the identity's next life starts fresh (its
//!   0.9.3 continuity was deleted, or its life was reset while the
//!   directory was stopped), although Core's last binding of it still names
//!   `retired_session`, retired or live. Carried out the way a reset treats
//!   the identity, keeping its liveness: Core replaces a live member by its
//!   successor on a fresh session, and gives a dormant identity a fresh
//!   session it retires at once. The old session stays in Core, bound to
//!   nothing.
//!
//! Every roster compile carries out the pending rows of the identities it
//! lists, before anything wakes them; a row whose identity the roster does
//! not list waits for a compile that lists it, unless Core's history already
//! shows it done, which every roster compile settles too. Each carried-out
//! row leaves the file ([`FirstBootManifest::replace_rows`]), and the file
//! goes once its last row is done. Whether a row is done is read from Core's
//! member history ([`FirstBootRow::state_in`]), so a boot that stops
//! part-way finishes on the next one. A row Core's history contradicts
//! refuses that identity's wake, typed, and stays in the file.
//!
//! # Format (version 1)
//!
//! One JSON file, `<state_dir>/storage-migrate-native.first-boot.json`:
//!
//! ```json
//! {"format":"meerkat-meta.first-boot-manifest","version":1,
//!  "rows_sha256":"<64 lowercase hex digits>","rows":[
//!   {"kind":"resume","identity":"<identity>","session":"<session id>"},
//!   {"kind":"fresh","identity":"<identity>","retired_session":"<session id>"}]}
//! ```
//!
//! - `rows_sha256` is the SHA-256 of the compact JSON of `rows`: no
//!   whitespace, each row's keys in the order above, strings escaped as
//!   `serde_json` writes them.
//! - Rows are sorted by identity (byte order), each identity at most once.
//! - Session ids are canonical: lowercase, hyphenated UUIDs.
//! - The reader refuses, typed: another format, an unknown version, a digest
//!   mismatch, unsorted or repeated identities, unknown or repeated keys, and
//!   any value of the wrong shape.
//!
//! The writers here are the only ones: `storage-migrate-native` writes the
//! manifest through [`FirstBootManifest::write_new`], and `meta_gateway
//! identity-reset` adds or replaces one identity's row through
//! [`FirstBootManifest::upsert_row`], so the format lives in this module
//! alone. Every writer assumes the caller holds the state directory
//! exclusively (the realm lock), as the migration, the reset and the gateway
//! do.
//!
//! A realm kept in a host store (see [`crate::host_store`]) keeps the same
//! document as its `first-boot` record, written by the import of a
//! continuity export into the realm and carried out the same way; there the
//! realm's fence is the exclusivity.

use std::collections::BTreeSet;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::types::AgentIdentity;

/// The manifest's file name inside the migrated state directory.
pub const FIRST_BOOT_MANIFEST_FILE: &str = "storage-migrate-native.first-boot.json";
/// The manifest's `format` value.
pub const FIRST_BOOT_MANIFEST_FORMAT: &str = "meerkat-meta.first-boot-manifest";
/// The manifest format version this build reads and writes.
pub const FIRST_BOOT_MANIFEST_VERSION: u64 = 1;

/// What the first boot does for one migrated identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstBootRow {
    /// The identity continues on `session`, which Core's session store
    /// holds and Core's mob may not have bound to it yet.
    Resume {
        identity: AgentIdentity,
        session: SessionId,
    },
    /// The identity's next life starts fresh (its 0.9.3 continuity was
    /// deleted, or its life was reset while the directory was stopped),
    /// although Core's last binding of it still names `retired_session`,
    /// retired or live.
    Fresh {
        identity: AgentIdentity,
        retired_session: SessionId,
    },
}

/// The kind of a [`FirstBootRow`], as the manifest and the status name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstBootRowKind {
    Resume,
    Fresh,
}

impl FirstBootRowKind {
    /// The kind as the manifest spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::Fresh => "fresh",
        }
    }
}

impl fmt::Display for FirstBootRowKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a [`FirstBootRow`] stands in Core's member history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstBootRowState {
    /// Core's history shows the row carried out; nothing is left to do.
    Done,
    /// The row is still to carry out: Core holds no binding of a `Resume`
    /// row's identity, or Core's last binding of a `Fresh` row's identity is
    /// still its retired session.
    ToDo,
    /// Core's history contradicts the row: Core's mob store is not the one
    /// the migration read. Nothing may act on the identity.
    Contradicted {
        /// The session Core's last binding of the identity names, if any.
        core_session: Option<SessionId>,
        detail: String,
    },
}

impl FirstBootRow {
    /// The identity the row is for.
    pub fn identity(&self) -> &AgentIdentity {
        match self {
            Self::Resume { identity, .. } | Self::Fresh { identity, .. } => identity,
        }
    }

    /// The row's kind.
    pub fn kind(&self) -> FirstBootRowKind {
        match self {
            Self::Resume { .. } => FirstBootRowKind::Resume,
            Self::Fresh { .. } => FirstBootRowKind::Fresh,
        }
    }

    /// The session the row names: the one to resume, or the retired one.
    pub fn session(&self) -> &SessionId {
        match self {
            Self::Resume { session, .. } => session,
            Self::Fresh {
                retired_session, ..
            } => retired_session,
        }
    }

    /// Where the row stands in Core's member history, read from the
    /// identity's last binding since the latest mob reset:
    /// - `Resume`: done when that binding names the row's session; to do
    ///   when Core holds no binding; contradicted when it names another
    ///   session.
    /// - `Fresh`: done when that binding names any session but the retired
    ///   one (the fresh life started), or when it is the retired session's
    ///   binding of a retired member no identity-first spawn made for the
    ///   identity (a helper or a fork child the host made on the mob under
    ///   its member id, which `storage-migrate-native` before 0.9.5 wrote a
    ///   row for): the identity's wake resumes only a member the identity
    ///   runtime spawned, so its next life starts fresh without the row. To
    ///   do while it is the retired session's binding otherwise, retired or
    ///   live (a live member gets its successor); contradicted when Core
    ///   holds no binding, or when the binding has no session.
    ///
    /// A carried-out row leaves the manifest, so a row is read again only
    /// when a boot stopped between carrying it out and rewriting the file;
    /// nothing else acts on the identity in between.
    pub fn state_in(&self, history: &meerkat_mob::MemberBindingHistory) -> FirstBootRowState {
        let core_identity =
            crate::member_comms_id::roster_member_id_for_identity(self.identity().as_str());
        let last = history.last_binding(&core_identity);
        let core_session = last.and_then(|binding| binding.session_id().cloned());
        let contradicted = |detail: String| FirstBootRowState::Contradicted {
            core_session: core_session.clone(),
            detail,
        };
        match (self, last) {
            (Self::Resume { .. }, None) => FirstBootRowState::ToDo,
            (Self::Resume { session, .. }, Some(binding))
                if binding.session_id() == Some(session) =>
            {
                FirstBootRowState::Done
            }
            (Self::Resume { .. }, Some(binding)) => match binding.session_id() {
                Some(other) => contradicted(format!(
                    "Core's last binding of the identity names session {other}"
                )),
                None => {
                    contradicted("Core's last binding of the identity has no session".to_string())
                }
            },
            (Self::Fresh { .. }, None) => {
                contradicted("Core holds no binding of the identity".to_string())
            }
            (
                Self::Fresh {
                    retired_session, ..
                },
                Some(binding),
            ) => match binding.session_id() {
                Some(session)
                    if session == retired_session
                        && !binding.is_live()
                        && !super::runtime::binding_spawned_for_identity(
                            binding,
                            self.identity(),
                        ) =>
                {
                    FirstBootRowState::Done
                }
                Some(session) if session == retired_session => FirstBootRowState::ToDo,
                Some(_) => FirstBootRowState::Done,
                None => {
                    contradicted("Core's last binding of the identity has no session".to_string())
                }
            },
        }
    }
}

/// A read or written first-boot manifest: its rows, sorted by identity, and
/// their digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstBootManifest {
    rows: Vec<FirstBootRow>,
    rows_sha256: String,
}

/// Where a runtime's first-boot manifest lives.
#[derive(Debug, Clone)]
pub(crate) enum FirstBootLocation {
    /// The file in this state directory ([`FIRST_BOOT_MANIFEST_FILE`]).
    StateDir(PathBuf),
    /// The `first-boot` document of a host store realm.
    HostStore(crate::host_store_documents::MetaDocuments),
}

/// What [`FirstBootManifest::remove_if_unchanged`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstBootManifestRemoval {
    /// The manifest was the expected one and is gone.
    Removed,
    /// There was no manifest.
    Absent,
}

impl FirstBootManifest {
    /// The manifest's path in `state_dir`.
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join(FIRST_BOOT_MANIFEST_FILE)
    }

    /// A manifest of `rows`, sorted by identity. Refuses an identity that
    /// appears in more than one row.
    pub fn new(mut rows: Vec<FirstBootRow>) -> Result<Self, FirstBootManifestRefusal> {
        rows.sort_by(|left, right| left.identity().cmp(right.identity()));
        if let Some(pair) = rows
            .windows(2)
            .find(|pair| pair[0].identity() == pair[1].identity())
        {
            return Err(FirstBootManifestRefusal::RepeatedIdentity {
                identity: pair[0].identity().to_string(),
            });
        }
        let rows_sha256 = rows_digest(&rows)?;
        Ok(Self { rows, rows_sha256 })
    }

    /// The rows, sorted by identity.
    pub fn rows(&self) -> &[FirstBootRow] {
        &self.rows
    }

    /// The rows, sorted by identity.
    pub fn into_rows(self) -> Vec<FirstBootRow> {
        self.rows
    }

    /// The lowercase hex SHA-256 of the rows' compact JSON.
    pub fn rows_sha256(&self) -> &str {
        &self.rows_sha256
    }

    /// Read the manifest in `state_dir`: `None` when there is none. A file
    /// that is there but not a valid version 1 manifest is refused, typed.
    pub fn read(state_dir: &Path) -> Result<Option<Self>, FirstBootManifestError> {
        let path = Self::path(state_dir);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(FirstBootManifestError::new(
                    path,
                    FirstBootManifestRefusal::Io {
                        operation: "read",
                        source,
                    },
                ));
            }
        };
        Self::parse(&bytes)
            .map(Some)
            .map_err(|refusal| FirstBootManifestError::new(path, refusal))
    }

    /// Write a new manifest of `rows` into `state_dir`: a temporary file in
    /// the state directory, synced, then linked into place. Refuses if a
    /// manifest is already there (`AlreadyExists`); nothing is replaced.
    /// Returns the manifest written, with the digest to remove it by.
    pub fn write_new(
        state_dir: &Path,
        rows: Vec<FirstBootRow>,
    ) -> Result<Self, FirstBootManifestError> {
        let path = Self::path(state_dir);
        let manifest = Self::new(rows)
            .map_err(|refusal| FirstBootManifestError::new(path.clone(), refusal))?;
        let temporary = manifest
            .write_temporary(state_dir)
            .map_err(|refusal| FirstBootManifestError::new(path.clone(), refusal))?;
        if let Err(error) = temporary.persist_noclobber(&path) {
            let refusal = if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                FirstBootManifestRefusal::AlreadyExists
            } else {
                FirstBootManifestRefusal::Io {
                    operation: "link the manifest into place",
                    source: error.error,
                }
            };
            return Err(FirstBootManifestError::new(path, refusal));
        }
        sync_dir(state_dir).map_err(|source| {
            FirstBootManifestError::new(
                path,
                FirstBootManifestRefusal::Io {
                    operation: "sync the state directory",
                    source,
                },
            )
        })?;
        Ok(manifest)
    }

    /// Replace the manifest whose rows hash to `rows_sha256` with one of
    /// `remaining` alone, atomically (a synced temporary file renamed over
    /// it), or remove it when nothing remains. Refuses (`Changed`) when the
    /// manifest on disk is missing or another one; nothing is written then.
    /// Returns the manifest now on disk, `None` once it is removed.
    pub fn replace_rows(
        state_dir: &Path,
        rows_sha256: &str,
        remaining: Vec<FirstBootRow>,
    ) -> Result<Option<Self>, FirstBootManifestError> {
        let path = Self::path(state_dir);
        let current = Self::read(state_dir)?;
        let found = current.as_ref().map(|current| current.rows_sha256.clone());
        if found.as_deref() != Some(rows_sha256) {
            return Err(FirstBootManifestError::new(
                path,
                FirstBootManifestRefusal::Changed {
                    expected: rows_sha256.to_string(),
                    found,
                },
            ));
        }
        if remaining.is_empty() {
            remove_and_sync(state_dir, &path)?;
            return Ok(None);
        }
        let manifest = Self::new(remaining)
            .map_err(|refusal| FirstBootManifestError::new(path.clone(), refusal))?;
        let temporary = manifest
            .write_temporary(state_dir)
            .map_err(|refusal| FirstBootManifestError::new(path.clone(), refusal))?;
        if let Err(error) = temporary.persist(&path) {
            return Err(FirstBootManifestError::new(
                path,
                FirstBootManifestRefusal::Io {
                    operation: "rename the manifest into place",
                    source: error.error,
                },
            ));
        }
        sync_dir(state_dir).map_err(|source| {
            FirstBootManifestError::new(
                path,
                FirstBootManifestRefusal::Io {
                    operation: "sync the state directory",
                    source,
                },
            )
        })?;
        Ok(Some(manifest))
    }

    /// Add `row` to the manifest in `state_dir`, or replace the row of the
    /// same identity, keeping the rows sorted by identity and their digest
    /// current: a new manifest when there is none ([`Self::write_new`]),
    /// otherwise the one just read, rewritten atomically against its digest
    /// ([`Self::replace_rows`]). Returns the manifest now on disk. The caller
    /// holds the state directory exclusively, as every writer here assumes.
    pub fn upsert_row(state_dir: &Path, row: FirstBootRow) -> Result<Self, FirstBootManifestError> {
        let Some(current) = Self::read(state_dir)? else {
            return Self::write_new(state_dir, vec![row]);
        };
        let mut rows: Vec<FirstBootRow> = current
            .rows
            .into_iter()
            .filter(|existing| existing.identity() != row.identity())
            .collect();
        rows.push(row);
        // Unreachable: `rows` holds the new row, and `replace_rows` removes
        // the file only for an empty row set.
        Self::replace_rows(state_dir, &current.rows_sha256, rows)?.ok_or_else(|| {
            FirstBootManifestError::new(
                Self::path(state_dir),
                FirstBootManifestRefusal::Changed {
                    expected: current.rows_sha256.clone(),
                    found: None,
                },
            )
        })
    }

    /// Remove the manifest in `state_dir` only when its rows hash to
    /// `rows_sha256`: a manifest with other rows (`Changed`), or one the
    /// reader refuses, is kept, typed.
    pub fn remove_if_unchanged(
        state_dir: &Path,
        rows_sha256: &str,
    ) -> Result<FirstBootManifestRemoval, FirstBootManifestError> {
        let path = Self::path(state_dir);
        let Some(current) = Self::read(state_dir)? else {
            return Ok(FirstBootManifestRemoval::Absent);
        };
        if current.rows_sha256 != rows_sha256 {
            return Err(FirstBootManifestError::new(
                path,
                FirstBootManifestRefusal::Changed {
                    expected: rows_sha256.to_string(),
                    found: Some(current.rows_sha256),
                },
            ));
        }
        remove_and_sync(state_dir, &path)?;
        Ok(FirstBootManifestRemoval::Removed)
    }

    /// The manifest's location in a host store realm, as errors name it.
    pub(crate) fn host_store_label(
        documents: &crate::host_store_documents::MetaDocuments,
    ) -> PathBuf {
        documents.label(crate::host_store_documents::FIRST_BOOT_KEY)
    }

    /// [`Self::read`] for a host store realm's `first-boot` document.
    pub(crate) async fn read_host_store(
        documents: &crate::host_store_documents::MetaDocuments,
    ) -> Result<Option<Self>, FirstBootManifestError> {
        let label = Self::host_store_label(documents);
        let bytes = documents
            .read(crate::host_store_documents::FIRST_BOOT_KEY)
            .await
            .map_err(|error| host_store_error(&label, &error))?;
        bytes
            .map(|bytes| {
                Self::parse(&bytes)
                    .map_err(|refusal| FirstBootManifestError::new(label.clone(), refusal))
            })
            .transpose()
    }

    /// [`Self::write_new`] for a host store realm's `first-boot` document:
    /// one batch, refused (`AlreadyExists`) when the realm holds a manifest.
    pub(crate) async fn write_new_host_store(
        documents: &crate::host_store_documents::MetaDocuments,
        rows: Vec<FirstBootRow>,
    ) -> Result<Self, FirstBootManifestError> {
        let label = Self::host_store_label(documents);
        let manifest = Self::new(rows)
            .map_err(|refusal| FirstBootManifestError::new(label.clone(), refusal))?;
        let document = manifest
            .document()
            .map_err(|refusal| FirstBootManifestError::new(label.clone(), refusal))?;
        let written = documents
            .update(crate::host_store_documents::FIRST_BOOT_KEY, |current| {
                Ok(match current {
                    Some(_) => (crate::host_store_documents::DocumentChange::Keep, false),
                    None => (
                        crate::host_store_documents::DocumentChange::Replace(document),
                        true,
                    ),
                })
            })
            .await
            .map_err(|error| host_store_error(&label, &error))?;
        if !written {
            return Err(FirstBootManifestError::new(
                label,
                FirstBootManifestRefusal::AlreadyExists,
            ));
        }
        Ok(manifest)
    }

    /// [`Self::replace_rows`] for a host store realm's `first-boot`
    /// document, decided under the document's lock: one batch, refused
    /// (`Changed`) when the realm holds no manifest or another one.
    pub(crate) async fn replace_host_store_rows(
        documents: &crate::host_store_documents::MetaDocuments,
        rows_sha256: &str,
        remaining: Vec<FirstBootRow>,
    ) -> Result<Option<Self>, FirstBootManifestError> {
        use crate::host_store_documents::DocumentChange;

        let label = Self::host_store_label(documents);
        let replacement = if remaining.is_empty() {
            None
        } else {
            let manifest = Self::new(remaining)
                .map_err(|refusal| FirstBootManifestError::new(label.clone(), refusal))?;
            let document = manifest
                .document()
                .map_err(|refusal| FirstBootManifestError::new(label.clone(), refusal))?;
            Some((manifest, document))
        };
        let outcome = documents
            .update(crate::host_store_documents::FIRST_BOOT_KEY, |current| {
                let found = match current.map(Self::parse).transpose() {
                    Ok(found) => found.map(|current| current.rows_sha256),
                    Err(refusal) => return Ok((DocumentChange::Keep, Err(refusal))),
                };
                if found.as_deref() != Some(rows_sha256) {
                    let refusal = FirstBootManifestRefusal::Changed {
                        expected: rows_sha256.to_string(),
                        found,
                    };
                    return Ok((DocumentChange::Keep, Err(refusal)));
                }
                Ok(match replacement {
                    Some((manifest, document)) => {
                        (DocumentChange::Replace(document), Ok(Some(manifest)))
                    }
                    None => (DocumentChange::Remove, Ok(None)),
                })
            })
            .await
            .map_err(|error| host_store_error(&label, &error))?;
        outcome.map_err(|refusal| FirstBootManifestError::new(label, refusal))
    }

    /// The manifest document as the file holds it.
    fn document(&self) -> Result<Vec<u8>, FirstBootManifestRefusal> {
        let document = WireManifest {
            format: FIRST_BOOT_MANIFEST_FORMAT,
            version: FIRST_BOOT_MANIFEST_VERSION,
            rows_sha256: &self.rows_sha256,
            rows: wire_rows(&self.rows),
        };
        let mut bytes = serde_json::to_vec_pretty(&document).map_err(|error| {
            FirstBootManifestRefusal::Encode {
                detail: error.to_string(),
            }
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// The document in a synced temporary file in `state_dir`, removed
    /// again if it is dropped before it is persisted.
    fn write_temporary(
        &self,
        state_dir: &Path,
    ) -> Result<tempfile::NamedTempFile, FirstBootManifestRefusal> {
        let document = self.document()?;
        let io = |operation: &'static str| {
            move |source| FirstBootManifestRefusal::Io { operation, source }
        };
        let mut temporary = tempfile::Builder::new()
            .prefix(".storage-migrate-native.first-boot.")
            .suffix(".tmp")
            .tempfile_in(state_dir)
            .map_err(io("create a temporary file"))?;
        temporary
            .write_all(&document)
            .map_err(io("write the temporary file"))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(io("sync the temporary file"))?;
        Ok(temporary)
    }

    fn parse(bytes: &[u8]) -> Result<Self, FirstBootManifestRefusal> {
        use FirstBootManifestLocation::Manifest;
        use FirstBootManifestRefusal as Refusal;

        let value: Value = serde_json::from_slice(bytes).map_err(|error| Refusal::NotJson {
            detail: error.to_string(),
        })?;
        if let Some(key) = first_repeated_key(bytes) {
            return Err(Refusal::RepeatedKey { key });
        }
        let Value::Object(top) = value else {
            return Err(Refusal::WrongType {
                at: Manifest,
                key: None,
                expected: "an object",
            });
        };
        match top.get("format") {
            Some(Value::String(format)) if format == FIRST_BOOT_MANIFEST_FORMAT => {}
            other => {
                return Err(Refusal::Format {
                    found: other.map(Value::to_string),
                });
            }
        }
        match top.get("version") {
            Some(version) if version.as_u64() == Some(FIRST_BOOT_MANIFEST_VERSION) => {}
            other => {
                return Err(Refusal::Version {
                    found: other.map(Value::to_string),
                });
            }
        }
        if let Some(key) = top
            .keys()
            .find(|key| !matches!(key.as_str(), "format" | "version" | "rows_sha256" | "rows"))
        {
            return Err(Refusal::UnknownKey {
                at: Manifest,
                key: key.clone(),
            });
        }
        let recorded = required_str(&top, Manifest, "rows_sha256")?;
        if !is_lowercase_sha256(recorded) {
            return Err(Refusal::InvalidDigest {
                found: recorded.to_string(),
            });
        }
        let raw_rows = match top.get("rows") {
            Some(Value::Array(rows)) => rows,
            Some(_) => {
                return Err(Refusal::WrongType {
                    at: Manifest,
                    key: Some("rows"),
                    expected: "an array",
                });
            }
            None => {
                return Err(Refusal::MissingKey {
                    at: Manifest,
                    key: "rows",
                });
            }
        };
        let rows = raw_rows
            .iter()
            .enumerate()
            .map(|(index, raw)| parse_row(index, raw))
            .collect::<Result<Vec<_>, _>>()?;
        for (index, pair) in rows.windows(2).enumerate() {
            let (previous, identity) = (pair[0].identity(), pair[1].identity());
            if identity == previous {
                return Err(Refusal::RepeatedIdentity {
                    identity: identity.to_string(),
                });
            }
            if identity < previous {
                return Err(Refusal::Unsorted {
                    row: index + 1,
                    identity: identity.to_string(),
                    previous: previous.to_string(),
                });
            }
        }
        let computed = rows_digest(&rows)?;
        if computed != recorded {
            return Err(Refusal::DigestMismatch {
                recorded: recorded.to_string(),
                computed,
            });
        }
        Ok(Self {
            rows,
            rows_sha256: computed,
        })
    }
}

fn parse_row(index: usize, raw: &Value) -> Result<FirstBootRow, FirstBootManifestRefusal> {
    use FirstBootManifestRefusal as Refusal;

    let at = FirstBootManifestLocation::Row(index);
    let Value::Object(fields) = raw else {
        return Err(Refusal::WrongType {
            at,
            key: None,
            expected: "an object",
        });
    };
    let (kind, session_key) = match required_str(fields, at, "kind")? {
        "resume" => (FirstBootRowKind::Resume, "session"),
        "fresh" => (FirstBootRowKind::Fresh, "retired_session"),
        other => {
            return Err(Refusal::UnknownKind {
                row: index,
                kind: other.to_string(),
            });
        }
    };
    if let Some(key) = fields
        .keys()
        .find(|key| !matches!(key.as_str(), "kind" | "identity") && key.as_str() != session_key)
    {
        return Err(Refusal::UnknownKey {
            at,
            key: key.clone(),
        });
    }
    let identity =
        AgentIdentity::parse(required_str(fields, at, "identity")?).map_err(|error| {
            Refusal::InvalidIdentity {
                row: index,
                detail: error.to_string(),
            }
        })?;
    let raw_session = required_str(fields, at, session_key)?;
    let session = SessionId::parse(raw_session)
        .ok()
        .filter(|session| session.to_string() == raw_session)
        .ok_or_else(|| Refusal::NonCanonicalSession {
            row: index,
            key: session_key,
            value: raw_session.to_string(),
        })?;
    Ok(match kind {
        FirstBootRowKind::Resume => FirstBootRow::Resume { identity, session },
        FirstBootRowKind::Fresh => FirstBootRow::Fresh {
            identity,
            retired_session: session,
        },
    })
}

fn required_str<'a>(
    fields: &'a serde_json::Map<String, Value>,
    at: FirstBootManifestLocation,
    key: &'static str,
) -> Result<&'a str, FirstBootManifestRefusal> {
    match fields.get(key) {
        Some(Value::String(value)) => Ok(value),
        Some(_) => Err(FirstBootManifestRefusal::WrongType {
            at,
            key: Some(key),
            expected: "a string",
        }),
        None => Err(FirstBootManifestRefusal::MissingKey { at, key }),
    }
}

fn is_lowercase_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// One row in the manifest's spelling, keys in the format's order.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireRow<'a> {
    Resume {
        identity: &'a str,
        session: String,
    },
    Fresh {
        identity: &'a str,
        retired_session: String,
    },
}

#[derive(Serialize)]
struct WireManifest<'a> {
    format: &'static str,
    version: u64,
    rows_sha256: &'a str,
    rows: Vec<WireRow<'a>>,
}

fn wire_rows(rows: &[FirstBootRow]) -> Vec<WireRow<'_>> {
    rows.iter()
        .map(|row| match row {
            FirstBootRow::Resume { identity, session } => WireRow::Resume {
                identity: identity.as_str(),
                session: session.to_string(),
            },
            FirstBootRow::Fresh {
                identity,
                retired_session,
            } => WireRow::Fresh {
                identity: identity.as_str(),
                retired_session: retired_session.to_string(),
            },
        })
        .collect()
}

/// The lowercase hex SHA-256 of the rows' compact JSON.
fn rows_digest(rows: &[FirstBootRow]) -> Result<String, FirstBootManifestRefusal> {
    let compact =
        serde_json::to_vec(&wire_rows(rows)).map_err(|error| FirstBootManifestRefusal::Encode {
            detail: error.to_string(),
        })?;
    Ok(lowercase_hex(&Sha256::digest(compact)))
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    hex
}

/// A host store realm's refusal or failure of a manifest read or write.
fn host_store_error(
    label: &Path,
    error: &crate::host_store_documents::MetaDocumentError,
) -> FirstBootManifestError {
    FirstBootManifestError::new(
        label.to_path_buf(),
        FirstBootManifestRefusal::HostStore {
            detail: error.to_string(),
        },
    )
}

fn remove_and_sync(state_dir: &Path, path: &Path) -> Result<(), FirstBootManifestError> {
    let io = |operation: &'static str| {
        move |source| {
            FirstBootManifestError::new(
                path.to_path_buf(),
                FirstBootManifestRefusal::Io { operation, source },
            )
        }
    };
    std::fs::remove_file(path).map_err(io("remove the manifest"))?;
    sync_dir(state_dir).map_err(io("sync the state directory"))
}

/// Sync `dir`'s entries, so a link, rename or removal in it survives a
/// crash. Unix only: elsewhere a rename's durability rests on the
/// filesystem's own ordering.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(dir).and_then(|handle| handle.sync_all())
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// The first key an object in `bytes` repeats, in document order. `bytes`
/// is already known to be JSON; a plain parse keeps only one of the values.
fn first_repeated_key(bytes: &[u8]) -> Option<String> {
    serde_json::from_slice::<RepeatedKeyScan>(bytes)
        .ok()
        .and_then(|scan| scan.0)
}

/// One JSON value, walked only for an object that repeats a key.
struct RepeatedKeyScan(Option<String>);

impl<'de> Deserialize<'de> for RepeatedKeyScan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer
            .deserialize_any(RepeatedKeyVisitor)
            .map(RepeatedKeyScan)
    }
}

struct RepeatedKeyVisitor;

impl<'de> serde::de::Visitor<'de> for RepeatedKeyVisitor {
    type Value = Option<String>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(None)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(None)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(None)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(None)
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(None)
    }

    fn visit_seq<A>(self, mut items: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut first = None;
        while let Some(RepeatedKeyScan(found)) = items.next_element()? {
            first = first.or(found);
        }
        Ok(first)
    }

    fn visit_map<A>(self, mut entries: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut seen = BTreeSet::new();
        let mut first = None;
        while let Some(key) = entries.next_key::<String>()? {
            let repeated = !seen.insert(key.clone());
            let RepeatedKeyScan(nested) = entries.next_value()?;
            if first.is_none() {
                first = if repeated { Some(key) } else { nested };
            }
        }
        Ok(first)
    }
}

/// Where in the manifest a refusal points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstBootManifestLocation {
    /// The manifest's top-level object.
    Manifest,
    /// The row at this index of `rows`, counted from 0.
    Row(usize),
}

impl fmt::Display for FirstBootManifestLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest => f.write_str("the manifest"),
            Self::Row(index) => write!(f, "row {index}"),
        }
    }
}

/// Why a first-boot manifest was refused, typed. Nothing was changed.
#[derive(Debug)]
#[non_exhaustive]
pub enum FirstBootManifestRefusal {
    /// Reading, writing, syncing or removing a file failed.
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    /// The file is not JSON.
    NotJson { detail: String },
    /// An object repeats a key.
    RepeatedKey { key: String },
    /// A value is not of the type the format defines.
    WrongType {
        at: FirstBootManifestLocation,
        key: Option<&'static str>,
        expected: &'static str,
    },
    /// The `format` is missing or another one.
    Format { found: Option<String> },
    /// The `version` is missing or one this build does not know.
    Version { found: Option<String> },
    /// A key the format does not define.
    UnknownKey {
        at: FirstBootManifestLocation,
        key: String,
    },
    /// A key the format requires is missing.
    MissingKey {
        at: FirstBootManifestLocation,
        key: &'static str,
    },
    /// A row's `kind` is neither `resume` nor `fresh`.
    UnknownKind { row: usize, kind: String },
    /// A row's identity is not a valid agent identity.
    InvalidIdentity { row: usize, detail: String },
    /// A row's session id is not a canonical session id.
    NonCanonicalSession {
        row: usize,
        key: &'static str,
        value: String,
    },
    /// The rows are not sorted by identity.
    Unsorted {
        row: usize,
        identity: String,
        previous: String,
    },
    /// An identity appears in more than one row.
    RepeatedIdentity { identity: String },
    /// `rows_sha256` is not 64 lowercase hex digits.
    InvalidDigest { found: String },
    /// `rows_sha256` is not the digest of the rows.
    DigestMismatch { recorded: String, computed: String },
    /// The rows could not be encoded.
    Encode { detail: String },
    /// A manifest is already in place; `write_new` replaces none.
    AlreadyExists,
    /// The manifest on disk is missing (`found: None`) or holds other rows
    /// than the caller's digest names.
    Changed {
        expected: String,
        found: Option<String>,
    },
    /// The host store realm that keeps the manifest refused or failed the
    /// read or write.
    HostStore { detail: String },
}

impl fmt::Display for FirstBootManifestRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "could not {operation}: {source}"),
            Self::NotJson { detail } => write!(f, "not JSON: {detail}"),
            Self::RepeatedKey { key } => write!(f, "an object repeats the key \"{key}\""),
            Self::WrongType {
                at,
                key: Some(key),
                expected,
            } => write!(f, "{at}: \"{key}\" must be {expected}"),
            Self::WrongType {
                at,
                key: None,
                expected,
            } => write!(f, "{at} must be {expected}"),
            Self::Format { found: Some(found) } => write!(
                f,
                "the format is {found}, not \"{FIRST_BOOT_MANIFEST_FORMAT}\""
            ),
            Self::Format { found: None } => write!(f, "the format is missing"),
            Self::Version { found: Some(found) } => write!(
                f,
                "version {found} is not one this build reads (it reads \
                 {FIRST_BOOT_MANIFEST_VERSION})"
            ),
            Self::Version { found: None } => write!(f, "the version is missing"),
            Self::UnknownKey { at, key } => write!(f, "{at} has an unknown key \"{key}\""),
            Self::MissingKey { at, key } => write!(f, "{at} has no \"{key}\""),
            Self::UnknownKind { row, kind } => {
                write!(f, "row {row} has an unknown kind \"{kind}\"")
            }
            Self::InvalidIdentity { row, detail } => {
                write!(f, "row {row} names an invalid identity: {detail}")
            }
            Self::NonCanonicalSession { row, key, value } => write!(
                f,
                "row {row}: \"{key}\" is not a canonical session id: \"{value}\""
            ),
            Self::Unsorted {
                row,
                identity,
                previous,
            } => write!(
                f,
                "the rows are not sorted by identity: row {row} ({identity}) follows \
                 {previous}"
            ),
            Self::RepeatedIdentity { identity } => {
                write!(f, "identity {identity} appears in more than one row")
            }
            Self::InvalidDigest { found } => {
                write!(f, "rows_sha256 \"{found}\" is not 64 lowercase hex digits")
            }
            Self::DigestMismatch { recorded, computed } => write!(
                f,
                "rows_sha256 is {recorded}, but the rows hash to {computed}"
            ),
            Self::Encode { detail } => write!(f, "the rows could not be encoded: {detail}"),
            Self::AlreadyExists => f.write_str("a manifest is already in place"),
            Self::Changed {
                expected,
                found: Some(found),
            } => write!(
                f,
                "the manifest holds other rows (rows_sha256 {found}, expected {expected})"
            ),
            Self::Changed {
                expected,
                found: None,
            } => write!(f, "the manifest with rows_sha256 {expected} is gone"),
            Self::HostStore { detail } => write!(f, "the host store refused: {detail}"),
        }
    }
}

/// A refused first-boot manifest read, write or removal: the file and why.
#[derive(Debug)]
pub struct FirstBootManifestError {
    path: PathBuf,
    refusal: FirstBootManifestRefusal,
}

impl FirstBootManifestError {
    fn new(path: PathBuf, refusal: FirstBootManifestRefusal) -> Self {
        Self { path, refusal }
    }

    /// The manifest's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Why it was refused.
    pub fn refusal(&self) -> &FirstBootManifestRefusal {
        &self.refusal
    }
}

impl fmt::Display for FirstBootManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "first-boot manifest {}: {}",
            self.path.display(),
            self.refusal
        )
    }
}

impl std::error::Error for FirstBootManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.refusal {
            FirstBootManifestRefusal::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const SESSION_A: &str = "0192f0a0-0000-7000-8000-000000000001";
    const SESSION_B: &str = "0192f0a0-0000-7000-8000-000000000002";
    /// SHA-256 of the compact rows of [`known_rows`]: pins the digest's
    /// key order and spelling, which `storage-migrate-native` relies on.
    const KNOWN_ROWS_SHA256: &str =
        "1d2719447464120c9e9ab1186a0365b795479c7ae5f544be0ef0ff784d89062e";

    fn identity(name: &str) -> AgentIdentity {
        AgentIdentity::parse(name).expect("identity")
    }

    fn session(raw: &str) -> SessionId {
        SessionId::parse(raw).expect("session id")
    }

    /// The rows out of order: the writer sorts them.
    fn known_rows() -> Vec<FirstBootRow> {
        vec![
            FirstBootRow::Resume {
                identity: identity("agent-b"),
                session: session(SESSION_B),
            },
            FirstBootRow::Fresh {
                identity: identity("agent-a"),
                retired_session: session(SESSION_A),
            },
        ]
    }

    fn known_document(rows: &str, digest: &str) -> String {
        format!(
            r#"{{"format":"meerkat-meta.first-boot-manifest","version":1,"rows_sha256":"{digest}","rows":{rows}}}"#
        )
    }

    const KNOWN_ROWS_JSON: &str = r#"[{"kind":"fresh","identity":"agent-a","retired_session":"0192f0a0-0000-7000-8000-000000000001"},{"kind":"resume","identity":"agent-b","session":"0192f0a0-0000-7000-8000-000000000002"}]"#;

    fn refusal_of(document: &str) -> FirstBootManifestRefusal {
        FirstBootManifest::parse(document.as_bytes()).expect_err("the reader refuses")
    }

    #[test]
    fn the_writer_sorts_the_rows_and_pins_the_digest() {
        let dir = tempfile::tempdir().expect("state dir");
        let written = FirstBootManifest::write_new(dir.path(), known_rows()).expect("write");
        assert_eq!(written.rows_sha256(), KNOWN_ROWS_SHA256);
        assert_eq!(
            written
                .rows()
                .iter()
                .map(|row| row.identity().as_str())
                .collect::<Vec<_>>(),
            ["agent-a", "agent-b"]
        );
        let read = FirstBootManifest::read(dir.path())
            .expect("read")
            .expect("the manifest is there");
        assert_eq!(read, written);
        let parsed: Value = serde_json::from_slice(
            &std::fs::read(FirstBootManifest::path(dir.path())).expect("file"),
        )
        .expect("json");
        assert_eq!(parsed["format"], FIRST_BOOT_MANIFEST_FORMAT);
        assert_eq!(parsed["version"], FIRST_BOOT_MANIFEST_VERSION);
        assert_eq!(
            serde_json::to_string(&parsed["rows"]).expect("rows"),
            serde_json::to_string(
                &serde_json::from_str::<Value>(KNOWN_ROWS_JSON).expect("known rows")
            )
            .expect("known rows")
        );
        let stray = std::fs::read_dir(dir.path())
            .expect("list")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != FIRST_BOOT_MANIFEST_FILE)
            .count();
        assert_eq!(stray, 0, "no temporary file is left behind");
    }

    #[test]
    fn no_file_reads_as_no_manifest() {
        let dir = tempfile::tempdir().expect("state dir");
        assert_eq!(FirstBootManifest::read(dir.path()).expect("read"), None);
    }

    #[test]
    fn the_writer_never_replaces_a_manifest() {
        let dir = tempfile::tempdir().expect("state dir");
        FirstBootManifest::write_new(dir.path(), known_rows()).expect("first write");
        let before = std::fs::read(FirstBootManifest::path(dir.path())).expect("file");
        let error = FirstBootManifest::write_new(
            dir.path(),
            vec![FirstBootRow::Resume {
                identity: identity("agent-c"),
                session: session(SESSION_A),
            }],
        )
        .expect_err("a second manifest is refused");
        assert!(matches!(
            error.refusal(),
            FirstBootManifestRefusal::AlreadyExists
        ));
        assert_eq!(
            std::fs::read(FirstBootManifest::path(dir.path())).expect("file"),
            before
        );
    }

    #[test]
    fn the_writer_refuses_an_identity_twice() {
        let dir = tempfile::tempdir().expect("state dir");
        let error = FirstBootManifest::write_new(
            dir.path(),
            vec![
                FirstBootRow::Resume {
                    identity: identity("agent-a"),
                    session: session(SESSION_A),
                },
                FirstBootRow::Fresh {
                    identity: identity("agent-a"),
                    retired_session: session(SESSION_B),
                },
            ],
        )
        .expect_err("repeated identity");
        assert!(matches!(
            error.refusal(),
            FirstBootManifestRefusal::RepeatedIdentity { identity } if identity == "agent-a"
        ));
        assert!(!FirstBootManifest::path(dir.path()).exists());
    }

    #[test]
    fn the_known_document_reads() {
        let manifest =
            FirstBootManifest::parse(known_document(KNOWN_ROWS_JSON, KNOWN_ROWS_SHA256).as_bytes())
                .expect("valid");
        assert_eq!(manifest.rows_sha256(), KNOWN_ROWS_SHA256);
        assert_eq!(manifest.rows().len(), 2);
    }

    #[test]
    fn the_reader_refuses_another_format_or_version() {
        let valid = known_document(KNOWN_ROWS_JSON, KNOWN_ROWS_SHA256);
        let other_format = valid.replace(
            "meerkat-meta.first-boot-manifest",
            "meerkat-meta.first-boot-manifest-x",
        );
        assert!(matches!(
            refusal_of(&other_format),
            FirstBootManifestRefusal::Format { found: Some(_) }
        ));
        let no_format = valid.replace(r#""format":"meerkat-meta.first-boot-manifest","#, "");
        assert!(matches!(
            refusal_of(&no_format),
            FirstBootManifestRefusal::Format { found: None }
        ));
        for version in ["2", "0", "\"1\"", "1.0", "null"] {
            let document = valid.replace(r#""version":1"#, &format!(r#""version":{version}"#));
            assert!(
                matches!(
                    refusal_of(&document),
                    FirstBootManifestRefusal::Version { found: Some(_) }
                ),
                "version {version}"
            );
        }
    }

    #[test]
    fn the_reader_refuses_unknown_and_repeated_keys() {
        let valid = known_document(KNOWN_ROWS_JSON, KNOWN_ROWS_SHA256);
        let top = valid.replacen('{', r#"{"note":"x","#, 1);
        assert!(matches!(
            refusal_of(&top),
            FirstBootManifestRefusal::UnknownKey {
                at: FirstBootManifestLocation::Manifest,
                key,
            } if key == "note"
        ));
        let row = valid.replacen(r#"{"kind":"resume""#, r#"{"kind":"resume","note":"x""#, 1);
        assert!(matches!(
            refusal_of(&row),
            FirstBootManifestRefusal::UnknownKey {
                at: FirstBootManifestLocation::Row(1),
                key,
            } if key == "note"
        ));
        // A Resume row with a Fresh row's key.
        let crossed = valid.replacen(
            r#""session":"0192f0a0-0000-7000-8000-000000000002""#,
            r#""retired_session":"0192f0a0-0000-7000-8000-000000000002""#,
            1,
        );
        assert!(matches!(
            refusal_of(&crossed),
            FirstBootManifestRefusal::UnknownKey {
                at: FirstBootManifestLocation::Row(1),
                key,
            } if key == "retired_session"
        ));
        let repeated_top = valid.replacen('{', r#"{"version":1,"#, 1);
        assert!(matches!(
            refusal_of(&repeated_top),
            FirstBootManifestRefusal::RepeatedKey { key } if key == "version"
        ));
        let repeated_row = valid.replacen(
            r#"{"kind":"fresh","identity":"agent-a","#,
            r#"{"kind":"fresh","identity":"agent-z","identity":"agent-a","#,
            1,
        );
        assert!(matches!(
            refusal_of(&repeated_row),
            FirstBootManifestRefusal::RepeatedKey { key } if key == "identity"
        ));
    }

    #[test]
    fn the_reader_refuses_unsorted_or_repeated_identities() {
        let swapped = r#"[{"kind":"resume","identity":"agent-b","session":"0192f0a0-0000-7000-8000-000000000002"},{"kind":"fresh","identity":"agent-a","retired_session":"0192f0a0-0000-7000-8000-000000000001"}]"#;
        assert!(matches!(
            refusal_of(&known_document(swapped, KNOWN_ROWS_SHA256)),
            FirstBootManifestRefusal::Unsorted { row: 1, identity, previous }
                if identity == "agent-a" && previous == "agent-b"
        ));
        let repeated = r#"[{"kind":"fresh","identity":"agent-a","retired_session":"0192f0a0-0000-7000-8000-000000000001"},{"kind":"resume","identity":"agent-a","session":"0192f0a0-0000-7000-8000-000000000002"}]"#;
        assert!(matches!(
            refusal_of(&known_document(repeated, KNOWN_ROWS_SHA256)),
            FirstBootManifestRefusal::RepeatedIdentity { identity } if identity == "agent-a"
        ));
    }

    #[test]
    fn the_reader_refuses_a_digest_mismatch() {
        let edited = KNOWN_ROWS_JSON.replace(
            "0192f0a0-0000-7000-8000-000000000002",
            "0192f0a0-0000-7000-8000-000000000003",
        );
        assert!(matches!(
            refusal_of(&known_document(&edited, KNOWN_ROWS_SHA256)),
            FirstBootManifestRefusal::DigestMismatch { recorded, .. }
                if recorded == KNOWN_ROWS_SHA256
        ));
        assert!(matches!(
            refusal_of(&known_document(
                KNOWN_ROWS_JSON,
                &KNOWN_ROWS_SHA256.to_uppercase()
            )),
            FirstBootManifestRefusal::InvalidDigest { .. }
        ));
    }

    #[test]
    fn the_reader_refuses_malformed_rows() {
        let upper = KNOWN_ROWS_JSON.replace(
            "0192f0a0-0000-7000-8000-000000000001",
            "0192F0A0-0000-7000-8000-000000000001",
        );
        assert!(matches!(
            refusal_of(&known_document(&upper, KNOWN_ROWS_SHA256)),
            FirstBootManifestRefusal::NonCanonicalSession {
                row: 0,
                key: "retired_session",
                ..
            }
        ));
        let kind = KNOWN_ROWS_JSON.replace(r#""kind":"fresh""#, r#""kind":"retire""#);
        assert!(matches!(
            refusal_of(&known_document(&kind, KNOWN_ROWS_SHA256)),
            FirstBootManifestRefusal::UnknownKind { row: 0, kind } if kind == "retire"
        ));
        let spaced = KNOWN_ROWS_JSON.replace("agent-a", "agent a");
        assert!(matches!(
            refusal_of(&known_document(&spaced, KNOWN_ROWS_SHA256)),
            FirstBootManifestRefusal::InvalidIdentity { row: 0, .. }
        ));
        let no_identity = KNOWN_ROWS_JSON.replace(r#""identity":"agent-a","#, "");
        assert!(matches!(
            refusal_of(&known_document(&no_identity, KNOWN_ROWS_SHA256)),
            FirstBootManifestRefusal::MissingKey {
                at: FirstBootManifestLocation::Row(0),
                key: "identity",
            }
        ));
        assert!(matches!(
            refusal_of(&known_document("{}", KNOWN_ROWS_SHA256)),
            FirstBootManifestRefusal::WrongType {
                key: Some("rows"),
                ..
            }
        ));
        assert!(matches!(
            refusal_of("{\"format\":"),
            FirstBootManifestRefusal::NotJson { .. }
        ));
        assert!(matches!(
            refusal_of("[]"),
            FirstBootManifestRefusal::WrongType {
                at: FirstBootManifestLocation::Manifest,
                key: None,
                ..
            }
        ));
    }

    #[test]
    fn removal_deletes_only_the_expected_manifest() {
        let dir = tempfile::tempdir().expect("state dir");
        assert_eq!(
            FirstBootManifest::remove_if_unchanged(dir.path(), KNOWN_ROWS_SHA256).expect("absent"),
            FirstBootManifestRemoval::Absent
        );
        FirstBootManifest::write_new(dir.path(), known_rows()).expect("write");
        let other = "0".repeat(64);
        let error =
            FirstBootManifest::remove_if_unchanged(dir.path(), &other).expect_err("another digest");
        assert!(matches!(
            error.refusal(),
            FirstBootManifestRefusal::Changed { found: Some(found), .. }
                if found == KNOWN_ROWS_SHA256
        ));
        assert!(FirstBootManifest::path(dir.path()).exists());
        assert_eq!(
            FirstBootManifest::remove_if_unchanged(dir.path(), KNOWN_ROWS_SHA256).expect("remove"),
            FirstBootManifestRemoval::Removed
        );
        assert!(!FirstBootManifest::path(dir.path()).exists());
    }

    #[test]
    fn carried_out_rows_leave_the_file_and_the_last_removes_it() {
        let dir = tempfile::tempdir().expect("state dir");
        let written = FirstBootManifest::write_new(dir.path(), known_rows()).expect("write");
        let remaining = vec![written.rows()[1].clone()];
        let rewritten =
            FirstBootManifest::replace_rows(dir.path(), written.rows_sha256(), remaining.clone())
                .expect("rewrite")
                .expect("a row remains");
        assert_eq!(rewritten.rows(), remaining.as_slice());
        assert_eq!(
            FirstBootManifest::read(dir.path()).expect("read"),
            Some(rewritten.clone())
        );
        // A stale digest replaces nothing.
        let error = FirstBootManifest::replace_rows(dir.path(), written.rows_sha256(), Vec::new())
            .expect_err("stale digest");
        assert!(matches!(
            error.refusal(),
            FirstBootManifestRefusal::Changed { .. }
        ));
        assert!(FirstBootManifest::path(dir.path()).exists());
        assert_eq!(
            FirstBootManifest::replace_rows(dir.path(), rewritten.rows_sha256(), Vec::new())
                .expect("last row"),
            None
        );
        assert!(!FirstBootManifest::path(dir.path()).exists());
        let error =
            FirstBootManifest::replace_rows(dir.path(), rewritten.rows_sha256(), Vec::new())
                .expect_err("gone");
        assert!(matches!(
            error.refusal(),
            FirstBootManifestRefusal::Changed { found: None, .. }
        ));
    }

    #[test]
    fn upsert_adds_a_row_or_replaces_the_identitys_row() {
        let dir = tempfile::tempdir().expect("state dir");
        let fresh_a = FirstBootRow::Fresh {
            identity: identity("agent-a"),
            retired_session: session(SESSION_A),
        };
        let resume_b = FirstBootRow::Resume {
            identity: identity("agent-b"),
            session: session(SESSION_B),
        };
        // No manifest: a new one with the row.
        let written =
            FirstBootManifest::upsert_row(dir.path(), resume_b.clone()).expect("a new manifest");
        assert_eq!(written.rows(), [resume_b.clone()].as_slice());
        // Another identity's row joins, sorted by identity.
        let joined =
            FirstBootManifest::upsert_row(dir.path(), fresh_a.clone()).expect("another row");
        assert_eq!(joined.rows(), [fresh_a, resume_b.clone()].as_slice());
        // The identity's own row is replaced; the other row stays.
        let fresh_a_again = FirstBootRow::Fresh {
            identity: identity("agent-a"),
            retired_session: session(SESSION_B),
        };
        let replaced = FirstBootManifest::upsert_row(dir.path(), fresh_a_again.clone())
            .expect("the same identity's row");
        assert_eq!(replaced.rows(), [fresh_a_again, resume_b].as_slice());
        assert_eq!(
            FirstBootManifest::read(dir.path()).expect("read"),
            Some(replaced)
        );
    }

    fn binding(
        name: &str,
        session_id: Option<&str>,
        state: meerkat_mob::MemberBindingState,
    ) -> meerkat_mob::MemberBinding {
        meerkat_mob::MemberBinding::for_test(
            crate::member_comms_id::roster_member_id_for_identity(name),
            meerkat_mob::Generation::new(2),
            session_id.map(session),
            state,
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                continuity_key: name.to_string(),
            },
            meerkat_mob::MemberIdentityManagement::CommandDriven,
        )
    }

    fn history(last: Option<meerkat_mob::MemberBinding>) -> meerkat_mob::MemberBindingHistory {
        meerkat_mob::MemberBindingHistory::for_test(last.clone(), last, [], 1)
    }

    #[test]
    fn a_resume_row_reads_its_state_from_the_last_binding() {
        use meerkat_mob::MemberBindingState::{Live, Retired};
        let row = FirstBootRow::Resume {
            identity: identity("agent-a"),
            session: session(SESSION_A),
        };
        assert_eq!(row.state_in(&history(None)), FirstBootRowState::ToDo);
        for state in [Live, Retired] {
            assert_eq!(
                row.state_in(&history(Some(binding("agent-a", Some(SESSION_A), state)))),
                FirstBootRowState::Done
            );
        }
        assert!(matches!(
            row.state_in(&history(Some(binding("agent-a", Some(SESSION_B), Live)))),
            FirstBootRowState::Contradicted { core_session: Some(other), .. }
                if other == session(SESSION_B)
        ));
        // Another identity's binding says nothing about this row.
        assert_eq!(
            row.state_in(&history(Some(binding("agent-b", Some(SESSION_B), Live)))),
            FirstBootRowState::ToDo
        );
    }

    /// A `Fresh` row over the retired binding of a member no identity-first
    /// spawn made for the identity (a helper or a fork child the host made
    /// on the mob, which `storage-migrate-native` before 0.9.5 wrote a row
    /// for) is done: the identity's wake would start fresh anyway. The same
    /// member while live, and a retired member the identity runtime spawned
    /// (its marker or its `agent_identity` label), keep the row to do.
    #[test]
    fn a_fresh_row_over_a_retired_member_no_identity_first_spawn_made_is_done() {
        use meerkat_mob::MemberBindingState::{Live, Retired};
        let row = FirstBootRow::Fresh {
            identity: identity("helper-a"),
            retired_session: session(SESSION_A),
        };
        let raw = |state| {
            meerkat_mob::MemberBinding::for_test(
                crate::member_comms_id::roster_member_id_for_identity("helper-a"),
                meerkat_mob::Generation::new(2),
                Some(session(SESSION_A)),
                state,
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::SpawnContinuityIntent::Ephemeral,
                meerkat_mob::MemberIdentityManagement::CommandDriven,
            )
        };
        assert_eq!(
            row.state_in(&history(Some(raw(Retired)))),
            FirstBootRowState::Done
        );
        assert_eq!(
            row.state_in(&history(Some(raw(Live)))),
            FirstBootRowState::ToDo
        );
        let labeled = raw(Retired).with_labels(std::collections::BTreeMap::from([(
            "agent_identity".to_string(),
            "helper-a".to_string(),
        )]));
        assert_eq!(
            row.state_in(&history(Some(labeled))),
            FirstBootRowState::ToDo
        );
        assert_eq!(
            row.state_in(&history(Some(binding(
                "helper-a",
                Some(SESSION_A),
                Retired
            )))),
            FirstBootRowState::ToDo
        );
    }

    #[test]
    fn a_fresh_row_reads_its_state_from_the_last_binding() {
        use meerkat_mob::MemberBindingState::{Live, Retired};
        let row = FirstBootRow::Fresh {
            identity: identity("agent-a"),
            retired_session: session(SESSION_A),
        };
        // The retired session's binding: retired (a deleted identity) or
        // live (a reset of a live identity), the fresh life is still to do.
        for state in [Live, Retired] {
            assert_eq!(
                row.state_in(&history(Some(binding("agent-a", Some(SESSION_A), state)))),
                FirstBootRowState::ToDo
            );
        }
        for state in [Live, Retired] {
            assert_eq!(
                row.state_in(&history(Some(binding("agent-a", Some(SESSION_B), state)))),
                FirstBootRowState::Done
            );
        }
        assert!(matches!(
            row.state_in(&history(None)),
            FirstBootRowState::Contradicted {
                core_session: None,
                ..
            }
        ));
        assert!(matches!(
            row.state_in(&history(Some(binding("agent-a", None, Live)))),
            FirstBootRowState::Contradicted {
                core_session: None,
                ..
            }
        ));
    }
}
