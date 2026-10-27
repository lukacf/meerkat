//! Replacement-process composition under the application's existing update
//! transaction. The private launcher selects source custody; native owns actual
//! construction, declaration, convergence and settlement. No new desired owner.
use std::{
    collections::{BTreeMap, BTreeSet}, fs::{File, OpenOptions},
    io::{Read, Write}, os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf}, sync::{Arc, Mutex},
};
use nix::{errno::Errno, fcntl::{openat, AtFlags, OFlag}, sys::stat::Mode, unistd::{geteuid, linkat, unlinkat, UnlinkatFlags}};
use meerkat_core::{CompiledApplicationToolPolicy, PreparedToolCatalog, PreparedToolSurface, ToolDef, ToolIdentity};
use meerkat_mob::MemberRuntimeBuildIdentity;
use meerkat_mobkit::member_tool_policy::providers_from_canonical_payloads;
use meerkat_toolkit::host_lifetime::{control::{HostLifetimeError, SelectedHostArtifact}, policy::CompiledArtifactReference};
use toolkit_mobkit::{installed_host::{InstalledSurfaceAssociations, read_bundle_artifact}, private_host::ApplicationUpdateStartup};
use crate::{application_bootstrap::hash, application_setup::{read_application_input,
    compilation::update::{ApplicationUpdateSelection, ValidatedApplicationUpdate}}};

fn invalid() -> HostLifetimeError { HostLifetimeError::InvalidSelection }
fn pinned(pin: &SelectedHostArtifact) -> Result<Vec<u8>, HostLifetimeError> {
    let bytes = read_application_input(&pin.path).map_err(|_| invalid())?;
    if hash(&bytes) != pin.sha256 { return Err(invalid()); }
    Ok(bytes)
}

/// Opaque trusted-startup result. No serialization and no permission, finish or
/// publication claim. It retains exact bytes for the actual public init join.
pub struct ValidatedReplacementStartup {
    pub(crate) update: ValidatedApplicationUpdate,
    pub(crate) prior_policy: Vec<u8>,
    pub(crate) carrier: CompiledArtifactReference,
    pub(crate) carrier_bytes: Vec<u8>,
    pub(crate) associations: Arc<UpdateAssociations>,
}
impl ValidatedReplacementStartup {
    /// All read-only validation, including output custody and the original ->
    /// actual prior desired -> candidate revision fence, precedes owner Restore.
    /// The trusted factory is responsible for selecting the transaction's real
    /// prior artifact; a matching hash is not authentication of that assertion.
    pub fn validate(startup: &ApplicationUpdateStartup) -> Result<Self, HostLifetimeError> {
        let bytes = pinned(&startup.selection)?;
        let selected: ApplicationUpdateSelection = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let update = selected.validate().map_err(|_| invalid())?;
        let prior_policy = pinned(&startup.prior_tool_policy)?;
        let prior = CompiledApplicationToolPolicy::parse_canonical_json(&prior_policy).map_err(|_| invalid())?;
        if prior.provider_id != update.tool_policy().provider_id || prior.policy_id != update.tool_policy().policy_id
            || prior.members.iter().map(|r| &r.member_identity).collect::<BTreeSet<_>>()
                != update.tool_policy().members.iter().map(|r| &r.member_identity).collect::<BTreeSet<_>>()
        { return Err(invalid()); }
        // Acquisition validation binds this original reference to the pinned
        // compiled predecessor; prepared authority facts carry no artifacts.
        let original = read_bundle_artifact(&update.selection().predecessor,
            &update.original_setup().application_tool_policy)?;
        providers_from_canonical_payloads([original.as_slice(), prior_policy.as_slice(), update.tool_policy_bytes()])
            .map_err(|_| invalid())?;
        let directory = AssociationDirectory::open(&startup.association_directory)?;
        let associations = Arc::new(UpdateAssociations {
            directory, selection: startup.selection.clone(), prior: startup.prior_tool_policy.clone(),
            update_id: update.selection().update_id.clone(),
            helper: update.original_setup().helper.member.clone(),
            observed: Mutex::new(BTreeMap::new()),
        });
        Ok(Self { update, prior_policy, carrier: CompiledArtifactReference {
            name: "application-update-selection.json".into(), sha256: startup.selection.sha256.clone(),
        }, carrier_bytes: bytes, associations })
    }
    pub fn predecessor(&self) -> &meerkat_toolkit::host_lifetime::control::HostBootstrapSelection {
        &self.update.selection().predecessor
    }
}

/// An existing transaction-selected directory, held by descriptor. It never
/// supplies desired state, authority, retry decisions or completed members.
struct AssociationDirectory { file: File, path: PathBuf, device: u64, inode: u64 }
impl AssociationDirectory {
    fn open(path: &Path) -> Result<Self, HostLifetimeError> {
        if !path.is_absolute() || path.canonicalize().map_err(|_| invalid())? != path { return Err(invalid()); }
        let file = OpenOptions::new().read(true)
            .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(path).map_err(|_| invalid())?;
        let stat = file.metadata().map_err(|_| HostLifetimeError::Unavailable)?;
        if !stat.is_dir() || stat.mode() & 0o077 != 0 || stat.uid() != geteuid().as_raw() { return Err(invalid()); }
        let selected = Self { file, path: path.into(), device: stat.dev(), inode: stat.ino() };
        selected.current()?;
        Ok(selected)
    }
    fn current(&self) -> Result<(), HostLifetimeError> {
        let stat = std::fs::symlink_metadata(&self.path).map_err(|_| HostLifetimeError::Unavailable)?;
        if !stat.is_dir() || stat.dev() != self.device || stat.ino() != self.inode
            || stat.mode() & 0o077 != 0 || stat.uid() != geteuid().as_raw()
            || self.path.canonicalize().map_err(|_| invalid())? != self.path
        { return Err(invalid()); }
        Ok(())
    }
    fn retain(&self, kind: &str, bytes: &[u8]) -> Result<(), HostLifetimeError> {
        self.current()?;
        if bytes.len() > 256 * 1024 { return Err(invalid()); }
        let name = format!("{kind}-{}.json", hash(bytes));
        // This counter selects only temporary filesystem names, never a native
        // construction identity, request, retry key or desired-state revision.
        static NEXT_FILE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut staged = None;
        for _ in 0..64 {
            let sequence = NEXT_FILE.fetch_update(std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed, |value| value.checked_add(1))
                .map_err(|_| HostLifetimeError::Unavailable)?;
            let temporary = format!("uncommitted-observation-{}-{sequence}", std::process::id());
            let flags = OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
            // The descriptor and generated single component prevent any path
            // replacement from redirecting writes into another owner's file.
            match openat(&self.file, temporary.as_str(), flags, Mode::S_IRUSR | Mode::S_IWUSR) {
                Ok(fd) => { staged = Some((temporary, File::from(fd))); break; }
                Err(Errno::EEXIST) => (),
                Err(_) => return Err(HostLifetimeError::Unavailable),
            }
        }
        let (temporary, mut file) = staged.ok_or(HostLifetimeError::Unavailable)?;
        // A partial/failed staging file stays with the existing transaction for
        // inspection. It cannot poison the committed content-derived name.
        file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|_| HostLifetimeError::Unavailable)?;
        self.current()?;
        match linkat(&self.file, temporary.as_str(), &self.file, name.as_str(), AtFlags::empty()) {
            Ok(()) => (),
            Err(Errno::EEXIST) => self.exact(&name, bytes)?,
            Err(_) => return Err(HostLifetimeError::Unavailable),
        }
        // Remove only this invocation's fully written staging file, never a
        // previous observation. Failure remains uncertain and is not replay.
        unlinkat(&self.file, temporary.as_str(), UnlinkatFlags::NoRemoveDir)
            .map_err(|_| HostLifetimeError::Unavailable)?;
        self.file.sync_all().map_err(|_| HostLifetimeError::Unavailable)?;
        self.current()
    }
    fn exact(&self, name: &str, bytes: &[u8]) -> Result<(), HostLifetimeError> {
        let fd = openat(&self.file, name, OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC, Mode::empty())
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let file = File::from(fd);
        let stat = file.metadata().map_err(|_| HostLifetimeError::Unavailable)?;
        if !stat.is_file() || stat.nlink() != 1 || stat.mode() & 0o077 != 0 || stat.uid() != geteuid().as_raw() { return Err(invalid()); }
        let mut existing = Vec::new();
        file.take(256 * 1024 + 1).read_to_end(&mut existing).map_err(|_| HostLifetimeError::Unavailable)?;
        if existing != bytes { return Err(invalid()); }
        Ok(())
    }
}

pub(crate) struct UpdateAssociations {
    directory: AssociationDirectory,
    selection: SelectedHostArtifact,
    prior: SelectedHostArtifact,
    update_id: String,
    helper: meerkat_core::MobMemberBinding,
    // Current-process preparation observations only. Nothing is loaded from
    // prior records or promoted to finish. Native owns construction correlation.
    observed: Mutex<BTreeMap<String, Vec<Vec<ToolIdentity>>>>,
}
impl UpdateAssociations {
    fn identity(identity: &MemberRuntimeBuildIdentity) -> serde_json::Value {
        serde_json::json!({"realm": identity.realm_id(), "member": identity.member(), "session_document": identity.session_document()})
    }
    fn record(&self, kind: &str, identity: &serde_json::Value, identities: &[ToolIdentity]) -> Result<(), HostLifetimeError> {
        let bytes = toolkit_package::canonical_json(&serde_json::json!({
            "schema": 1, "observation": kind, "update_id": self.update_id,
            "selection": self.selection, "prior_tool_policy": self.prior,
            "identity": identity, "identities": identities,
        })).map_err(|_| invalid())?;
        self.directory.retain(kind, &bytes)
    }
}
fn full_identities(surface: &PreparedToolSurface) -> Result<Vec<ToolIdentity>, HostLifetimeError> {
    let mut identities = surface.tools.clone();
    if identities.iter().enumerate().any(|(i, row)| identities[..i].iter().any(|prior| prior.name == row.name))
        || surface.projected.iter().any(|row| !surface.tools.contains(row)) { return Err(invalid()); }
    if let PreparedToolCatalog::Complete(entries) = &surface.catalog {
        let mut names = BTreeSet::new();
        for entry in entries {
            if !names.insert(&entry.identity.name) { return Err(invalid()); }
            match identities.iter().find(|row| row.name == entry.identity.name) {
                Some(row) if row != &entry.identity => return Err(invalid()),
                Some(_) => (), None => identities.push(entry.identity.clone()),
            }
        }
    }
    Ok(identities)
}
impl UpdateAssociations {
    fn prepared_observation(&self, identity: serde_json::Value, member: &meerkat_core::MobMemberBinding,
        surface: &PreparedToolSurface) -> Result<(), HostLifetimeError> {
        let identities = full_identities(surface)?;
        if member == &self.helper && !identities.contains(
            &toolkit_mobkit::installed_host::acquisition::setup_report_definition().identity()) { return Err(invalid()); }
        let key = identity.to_string();
        let mut observed = self.observed.lock().map_err(|_| HostLifetimeError::Unavailable)?;
        self.record("prepared", &identity, &identities)?;
        let rows = observed.entry(key).or_default();
        if !rows.contains(&identities) { rows.push(identities); }
        Ok(())
    }
    fn visible_observation(&self, identity: serde_json::Value, surface: &[ToolDef]) -> Result<(), HostLifetimeError> {
        let visible: Vec<_> = surface.iter().map(ToolDef::identity).collect();
        let observed = self.observed.lock().map_err(|_| HostLifetimeError::Unavailable)?;
        let rows = observed.get(&identity.to_string()).ok_or_else(invalid)?;
        // One complete observed preparation must contain the actual visible
        // surface. Never union sibling rows or manufacture deferred inventory.
        if !rows.iter().any(|row| visible.iter().all(|tool| row.contains(tool))) { return Err(invalid()); }
        self.record("visible", &identity, &visible)
    }
}
impl InstalledSurfaceAssociations for UpdateAssociations {
    fn prepared(&self, identity: &MemberRuntimeBuildIdentity, surface: &PreparedToolSurface) -> Result<(), HostLifetimeError> {
        self.prepared_observation(Self::identity(identity), identity.member(), surface)
    }
    fn visible(&self, identity: &MemberRuntimeBuildIdentity, surface: &[ToolDef]) -> Result<(), HostLifetimeError> {
        self.visible_observation(Self::identity(identity), surface)
    }
}

impl ValidatedReplacementStartup {
    /// The actual replacement-process consumer. Restore first retains the same
    /// canonical decision participant; the core accepts only that owner's
    /// already-current policy and keeps these preparation associations separate.
    pub async fn open(self) -> Result<(
        meerkat_toolkit::host_lifetime::NativeHostLifetime,
        toolkit_mobkit::installed_host::InstalledToolkitHost,
        Option<crate::application_acquisition::PreparedGatewayAcquisition>,
    ), HostLifetimeError> {
        let selection = self.predecessor().clone();
        let (owner, installed, participant) = crate::application_acquisition::open_gateway_owner(
            selection.clone(), toolkit_mobkit::installed_host::NativeOwnerStartup::Restore).await?;
        let native = owner.selected_policy().native_host.as_ref().ok_or_else(invalid)?;
        let installed = installed.with_application_update(&self.prior_policy,
            self.update.tool_policy_bytes(), self.associations.clone())?;
        let (installed, acquisition) = crate::application_acquisition::prepare_gateway_acquisition_update(
            &selection, native, installed, &self, participant)?;
        Ok((owner, installed, acquisition))
    }
}

#[cfg(test)]
mod tests;
