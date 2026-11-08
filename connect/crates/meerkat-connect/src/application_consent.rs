//! Explicit prepared-application consent persistence. This owner issues no
//! grant, authenticates no person and admits no recovered session. Ordinary
//! per-invocation setup reviews keep their existing owners and artifacts.
//!
//! The opt-in gateway carrier retains this owner before factories and builds
//! its BoundOperationReview from `approvals()`. Meta installs that same
//! review's consent settlement when an actual human route was selected.

use meerkat_core::{
    ApprovalId, ApprovalRecord, ApprovalService, PrincipalRef, RealmId,
    approval::{ApprovalStore, ApprovalStoreError},
};
use meerkat_store::FileApprovalStore;
use meerkat_toolkit::host_lifetime::control::SelectedHostArtifact;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use toolkit_package::Id;

mod cold_selection;
mod disposition;
mod operation_review;
mod provenance;
pub use cold_selection::AbandonedWorkflowRecovery;
pub use disposition::{ConsentAssociation, RecoveryJoinStatus, ScopedConsentDisposition};
pub use operation_review::ApplicationOperationReview;
pub(crate) use operation_review::RetainedApplicationOperationReview;
pub use provenance::ConsentResourceProvenance;
#[cfg(test)]
pub(crate) mod tests;

const MAX_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024;

/// Identity selected by the actual prepared application, not by a model,
/// request, saved approval or human actor string. It creates no authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentApplicationIdentity {
    application: PrincipalRef,
    deployment: Id,
    realm: RealmId,
}
impl ConsentApplicationIdentity {
    pub fn from_preparation(
        owner: &toolkit_mobkit::installed_host::preparation::PreparedInstallationOwner,
    ) -> Result<Self, ConsentOwnerError> {
        let selected = owner.selected();
        let identity = Self {
            application: selected.native_host.grant_administrator.clone(),
            deployment: selected.deployment.clone(),
            realm: selected.native_host.realm.clone(),
        };
        identity.validate()?;
        Ok(identity)
    }
    /// Reopen an explicitly selected issuer against the actual installed native
    /// owner. This is the member realm, not Meta's factory storage realm.
    pub fn from_installed(
        owner: &meerkat_toolkit::host_lifetime::NativeHostLifetime,
    ) -> Result<Self, ConsentOwnerError> {
        owner
            .validate_current_selection()
            .map_err(|_| ConsentOwnerError::InvalidSelection)?;
        Self::from_installed_selection(owner)
    }
    // Used only inside ToolkitGatewayHost's existing current-selection fence;
    // reentering the native lifetime from that callback would invert its gates.
    fn from_installed_selection(
        owner: &meerkat_toolkit::host_lifetime::NativeHostLifetime,
    ) -> Result<Self, ConsentOwnerError> {
        let selected = owner.selected_policy();
        let native = selected
            .native_host
            .as_ref()
            .ok_or(ConsentOwnerError::WrongApplication)?;
        let identity = Self {
            application: native.grant_administrator.clone(),
            deployment: selected.deployment.clone(),
            realm: native.realm.clone(),
        };
        identity.validate()?;
        Ok(identity)
    }
    fn validate(&self) -> Result<(), ConsentOwnerError> {
        self.application
            .validate_qualified()
            .map_err(|_| ConsentOwnerError::WrongApplication)
    }
    pub fn application(&self) -> &PrincipalRef {
        &self.application
    }
    pub fn deployment(&self) -> &Id {
        &self.deployment
    }
    pub fn realm(&self) -> &RealmId {
        &self.realm
    }
}

/// Persist this exact selection with the trusted application configuration.
/// Decoding it does not open an owner. Reopen also requires the independently
/// selected prepared-application identity. Missing history never creates an
/// empty replacement issuer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentIssuerSelection {
    manifest: SelectedHostArtifact,
}
impl ConsentIssuerSelection {
    pub fn artifact(&self) -> &SelectedHostArtifact {
        &self.manifest
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    identity: ConsentApplicationIdentity,
    issuer: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConsentOwnerError {
    #[error("the consent application does not match the selected owner")]
    WrongApplication,
    #[error("the exact selected consent issuer or artifact is unavailable")]
    InvalidSelection,
    #[error("the consent issuer is already retained by another owner")]
    OwnerHeld,
    #[error("the native hosting claim does not cover this consent association")]
    WrongClaim,
    #[error("the native claim has no available owner-loss reservation")]
    ReservationUnavailable,
    #[error("the association has no scoped consent history")]
    MissingConsent,
    #[error("spent consent cannot establish abandoned zero-effect recovery: {0}")]
    Spent(ApprovalId),
    #[error("the scoped consent has no confirmed abandoned disposition: {0}")]
    NotAbandoned(ApprovalId),
    #[error(transparent)]
    Approval(#[from] meerkat_core::ApprovalError),
    #[error(transparent)]
    Store(#[from] ApprovalStoreError),
    #[error("consent artifact I/O failed")]
    Io(#[from] std::io::Error),
}

/// An exclusive issuing owner. ApprovalService clones retain the same backing
/// lease, so dropping this wrapper cannot permit a second owner while a review
/// or consent candidate still retains the first service.
pub struct ApplicationConsentOwner {
    selection: ConsentIssuerSelection,
    manifest: Manifest,
    directory: PathBuf,
    service: ApprovalService,
    store: Arc<dyn ApprovalStore>,
}
impl ApplicationConsentOwner {
    /// Explicit opt-in creation. The parent is already selected private host
    /// storage; this creates a fresh issuer, never a replacement for a selection
    /// that failed to reopen. The returned selection must be durably retained
    /// by the application before this owner is used by a factory.
    pub fn create(
        parent: &Path,
        identity: ConsentApplicationIdentity,
    ) -> Result<Arc<Self>, ConsentOwnerError> {
        identity.validate()?;
        private_directory(parent)?;
        let directory = parent.join(format!("consent-issuer-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        File::open(parent)?.sync_all()?;
        let manifest = Manifest {
            schema_version: 1,
            identity,
            issuer: uuid::Uuid::new_v4().to_string(),
        };
        let lease = create_file(&directory.join("issuer.lock"), b"")?;
        lock_issuer(&lease)?;
        let store: Arc<dyn ApprovalStore> = Arc::new(LeasedStore {
            inner: FileApprovalStore::open(directory.join("decisions.json"))?,
            _lease: Arc::new(lease),
        });
        let service = ApprovalService::with_store(store.clone())?;
        let raw = encode(&manifest)?;
        let path = directory.join("issuer.json");
        create_file(&path, &raw)?;
        File::open(&directory)?.sync_all()?;
        let selection = ConsentIssuerSelection {
            manifest: SelectedHostArtifact {
                path,
                sha256: hash(&raw),
            },
        };
        Ok(Arc::new(Self {
            selection,
            manifest,
            directory,
            service,
            store,
        }))
    }

    pub fn reopen(
        selection: ConsentIssuerSelection,
        expected: &ConsentApplicationIdentity,
    ) -> Result<Arc<Self>, ConsentOwnerError> {
        Self::reopen_with_store(selection, expected, |store| store)
    }

    fn reopen_with_store(
        selection: ConsentIssuerSelection,
        expected: &ConsentApplicationIdentity,
        wrap: impl FnOnce(Arc<dyn ApprovalStore>) -> Arc<dyn ApprovalStore>,
    ) -> Result<Arc<Self>, ConsentOwnerError> {
        expected.validate()?;
        let directory = selection
            .manifest
            .path
            .parent()
            .ok_or(ConsentOwnerError::InvalidSelection)?
            .to_owned();
        private_directory(&directory)?;
        if selection
            .manifest
            .path
            .file_name()
            .is_none_or(|name| name != "issuer.json")
        {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        let manifest: Manifest = decode(&read_artifact(&selection.manifest)?)?;
        if manifest.schema_version != 1 || uuid::Uuid::parse_str(&manifest.issuer).is_err() {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        if &manifest.identity != expected {
            return Err(ConsentOwnerError::WrongApplication);
        }
        let lease = open_private_file(&directory.join("issuer.lock"), true)?;
        lock_issuer(&lease)?;
        // FileApprovalStore::open can create missing files. Reopen explicitly
        // requires this issuer's existing history before using that API.
        open_private_file(&directory.join("decisions.json"), false)?;
        let store: Arc<dyn ApprovalStore> = wrap(Arc::new(LeasedStore {
            inner: FileApprovalStore::open(directory.join("decisions.json"))?,
            _lease: Arc::new(lease),
        }));
        let service = ApprovalService::with_store(store.clone())?;
        Ok(Arc::new(Self {
            selection,
            manifest,
            directory,
            service,
            store,
        }))
    }

    pub fn selection(&self) -> &ConsentIssuerSelection {
        &self.selection
    }
    pub fn identity(&self) -> &ConsentApplicationIdentity {
        &self.manifest.identity
    }
    /// This clone shares the actual issuer, store and lease. It is intended
    /// for the selected review and authenticated event adapter, not a new store.
    pub fn approvals(&self) -> ApprovalService {
        self.service.clone()
    }
    pub(crate) fn check(&self) -> Result<(), ConsentOwnerError> {
        private_directory(&self.directory)?;
        if decode::<Manifest>(&read_artifact(&self.selection.manifest)?)? != self.manifest {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        open_private_file(&self.directory.join("decisions.json"), false)?;
        Ok(())
    }
}

struct LeasedStore {
    inner: FileApprovalStore,
    _lease: Arc<File>,
}
impl ApprovalStore for LeasedStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        self.inner.load_all()
    }
    fn get(&self, id: &ApprovalId) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
        self.inner.get(id)
    }
    fn put_if_current(
        &self,
        prior: Option<&ApprovalRecord>,
        row: &ApprovalRecord,
    ) -> Result<(), ApprovalStoreError> {
        self.inner.put_if_current(prior, row)
    }
    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }
}

fn private_directory(path: &Path) -> Result<(), ConsentOwnerError> {
    let metadata = fs::symlink_metadata(path)?;
    if !path.is_absolute()
        || fs::canonicalize(path)? != path
        || !metadata.is_dir()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(ConsentOwnerError::InvalidSelection);
    }
    Ok(())
}
fn lock_issuer(file: &File) -> Result<(), ConsentOwnerError> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(ConsentOwnerError::OwnerHeld),
        Err(std::fs::TryLockError::Error(error)) => Err(ConsentOwnerError::Io(error)),
    }
}
fn open_private_file(path: &Path, write: bool) -> Result<File, ConsentOwnerError> {
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(ConsentOwnerError::InvalidSelection);
    }
    Ok(file)
}
fn create_file(path: &Path, bytes: &[u8]) -> Result<File, ConsentOwnerError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(file)
}
fn read_artifact(artifact: &SelectedHostArtifact) -> Result<Vec<u8>, ConsentOwnerError> {
    let file = open_private_file(&artifact.path, false)?;
    let mut raw = Vec::new();
    file.take(MAX_ARTIFACT_BYTES + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_ARTIFACT_BYTES || hash(&raw) != artifact.sha256 {
        return Err(ConsentOwnerError::InvalidSelection);
    }
    Ok(raw)
}
fn hash(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, ConsentOwnerError> {
    toolkit_package::canonical_json(value).map_err(|_| ConsentOwnerError::InvalidSelection)
}
fn decode<T: serde::de::DeserializeOwned>(raw: &[u8]) -> Result<T, ConsentOwnerError> {
    serde_json::from_slice(raw).map_err(|_| ConsentOwnerError::InvalidSelection)
}

#[cfg(test)]
pub(crate) mod fixture_review_floor;
