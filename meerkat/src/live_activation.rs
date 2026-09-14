//! Trusted, native Live activation document resolution.
//!
//! This source is not reachable through ordinary Config mutation. A selection
//! remains declaration content until the host resolves the current executor
//! binding and the generated request owner activates its grant.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use meerkat_core::RealmId;
use meerkat_core::connection::RealmChain;
use meerkat_core::live_execution::activation::{
    LiveActivationDeclaration, LiveActivationDocument, LiveActivationEntry, LiveActivationId,
    LiveExecutorSelector, LiveProfileRevision,
};
use meerkat_core::live_execution::profile::LiveProfileId;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

const MAX_ACTIVATION_DOCUMENT_BYTES: u64 = 1024 * 1024;

/// Bootstrap supplies the already-resolved config document. No HOME, CWD, or
/// realm-directory inference is performed by the activation loader.
#[derive(Debug, Clone)]
pub struct LiveActivationDocumentLocator {
    path: PathBuf,
}

impl LiveActivationDocumentLocator {
    pub fn beside_config_document(
        config_document: &Path,
    ) -> Result<Self, LiveActivationSourceError> {
        if !config_document.is_absolute() {
            return Err(LiveActivationSourceError::InvalidLocator);
        }
        let parent = config_document
            .parent()
            .ok_or(LiveActivationSourceError::InvalidLocator)?;
        Ok(Self {
            path: parent.join("live-activations.toml"),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Host-only source. Every load opens the current documents again; it never
/// trusts a stale profile/config snapshot as a revocation check.
#[derive(Debug, Clone, Default)]
pub struct FileLiveExecutionGrantSource {
    documents: BTreeMap<RealmId, LiveActivationDocumentLocator>,
}

pub struct LiveActivationDocumentObservation<Member> {
    documents: BTreeMap<RealmId, LiveActivationDocument<Member>>,
    digest: [u8; 32],
}

impl<Member> LiveActivationDocumentObservation<Member> {
    pub fn documents(&self) -> &BTreeMap<RealmId, LiveActivationDocument<Member>> {
        &self.documents
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

impl FileLiveExecutionGrantSource {
    pub fn new(documents: BTreeMap<RealmId, LiveActivationDocumentLocator>) -> Self {
        Self { documents }
    }

    pub async fn load<Member: DeserializeOwned>(
        &self,
        chain: &RealmChain,
    ) -> Result<BTreeMap<RealmId, LiveActivationDocument<Member>>, LiveActivationSourceError> {
        Ok(self.observe(chain).await?.documents)
    }

    /// Bind every observed document, including absence, without claiming an
    /// atomic read across files or a lease against subsequent editor writes.
    pub async fn observe<Member: DeserializeOwned>(
        &self,
        chain: &RealmChain,
    ) -> Result<LiveActivationDocumentObservation<Member>, LiveActivationSourceError> {
        let mut documents = BTreeMap::new();
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-activation-observation.v1\0");
        for realm in chain.realms() {
            let Some(locator) = self.documents.get(realm) else {
                return Err(LiveActivationSourceError::MissingLocator {
                    realm: realm.clone(),
                });
            };
            digest.update((realm.as_str().len() as u64).to_be_bytes());
            digest.update(realm.as_str().as_bytes());
            let file = match tokio::fs::File::open(locator.path()).await {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    digest.update([0]);
                    documents.insert(realm.clone(), LiveActivationDocument::default());
                    continue;
                }
                Err(source) => {
                    return Err(LiveActivationSourceError::Read {
                        realm: realm.clone(),
                        source,
                    });
                }
            };
            let mut bytes = Vec::new();
            file.take(MAX_ACTIVATION_DOCUMENT_BYTES + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|source| LiveActivationSourceError::Read {
                    realm: realm.clone(),
                    source,
                })?;
            if bytes.len() as u64 > MAX_ACTIVATION_DOCUMENT_BYTES {
                return Err(LiveActivationSourceError::TooLarge {
                    realm: realm.clone(),
                });
            }
            let text = std::str::from_utf8(&bytes).map_err(|_| {
                LiveActivationSourceError::InvalidDocument {
                    realm: realm.clone(),
                }
            })?;
            let document =
                toml::from_str(text).map_err(|_| LiveActivationSourceError::InvalidDocument {
                    realm: realm.clone(),
                })?;
            digest.update([1]);
            digest.update(Sha256::digest(&bytes));
            documents.insert(realm.clone(), document);
        }
        Ok(LiveActivationDocumentObservation {
            documents,
            digest: digest.finalize().into(),
        })
    }
}

pub struct LiveActivationLookup<'a, Member> {
    pub profile_id: &'a LiveProfileId,
    pub profile_revision: LiveProfileRevision,
    pub requesting_realm: &'a RealmId,
    pub executor: &'a LiveExecutorSelector<Member>,
    pub now: DateTime<Utc>,
}

/// An exact declaration selection, NOT a sealed LiveExecutionGrant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveActivationSelection<Member> {
    Disabled,
    Selected {
        activation_id: LiveActivationId,
        declaration: Box<LiveActivationDeclaration<Member>>,
    },
}

pub fn select_live_activation<Member: Clone + PartialEq>(
    chain: &RealmChain,
    documents: &BTreeMap<RealmId, LiveActivationDocument<Member>>,
    lookup: &LiveActivationLookup<'_, Member>,
) -> Result<LiveActivationSelection<Member>, LiveActivationSourceError> {
    if chain.realms().first() != Some(lookup.requesting_realm) {
        return Err(LiveActivationSourceError::RequestingRealmMismatch);
    }
    let mut effective = BTreeMap::new();
    for realm in chain.realms().iter().rev() {
        let document =
            documents
                .get(realm)
                .ok_or_else(|| LiveActivationSourceError::MissingDocument {
                    realm: realm.clone(),
                })?;
        for (id, entry) in &document.activations {
            match entry {
                LiveActivationEntry::Inherit => {}
                LiveActivationEntry::Disable => {
                    effective.remove(id);
                }
                LiveActivationEntry::Set(declaration) => {
                    if &declaration.issuer_realm != realm {
                        return Err(LiveActivationSourceError::IssuerMismatch {
                            realm: realm.clone(),
                        });
                    }
                    effective.insert(id.clone(), declaration.clone());
                }
            }
        }
    }
    let mut selected = None;
    for (id, declaration) in effective {
        if &declaration.profile_id != lookup.profile_id || &declaration.executor != lookup.executor
        {
            continue;
        }
        if !declaration
            .requesting_realms
            .contains(lookup.requesting_realm)
        {
            continue;
        }
        if declaration.profile_revision != lookup.profile_revision {
            return Err(LiveActivationSourceError::ProfileRevisionMismatch);
        }
        if declaration
            .expires_at
            .is_some_and(|expiry| lookup.now >= expiry)
        {
            return Err(LiveActivationSourceError::Expired);
        }
        if selected.is_some() {
            return Err(LiveActivationSourceError::AmbiguousActivation);
        }
        selected = Some(LiveActivationSelection::Selected {
            activation_id: id,
            declaration,
        });
    }
    Ok(selected.unwrap_or(LiveActivationSelection::Disabled))
}

#[derive(Debug, thiserror::Error)]
pub enum LiveActivationSourceError {
    #[error("live activation locator requires an absolute resolved config-document path")]
    InvalidLocator,
    #[error("live activation document locator is missing for realm {realm}")]
    MissingLocator { realm: RealmId },
    #[error("live activation document was not loaded for realm {realm}")]
    MissingDocument { realm: RealmId },
    #[error("cannot read live activation document for realm {realm}: {source}")]
    Read {
        realm: RealmId,
        source: std::io::Error,
    },
    #[error("live activation document exceeds its byte bound for realm {realm}")]
    TooLarge { realm: RealmId },
    #[error("live activation document is invalid for realm {realm}")]
    InvalidDocument { realm: RealmId },
    #[error("live activation declares an issuer different from document owner {realm}")]
    IssuerMismatch { realm: RealmId },
    #[error("live activation requesting realm is not the resolved chain head")]
    RequestingRealmMismatch,
    #[error("live activation profile revision is no longer current")]
    ProfileRevisionMismatch,
    #[error("live activation has expired")]
    Expired,
    #[error("more than one live activation matches the exact requested target")]
    AmbiguousActivation,
}
