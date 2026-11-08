//! Physical shared-resource closure only. Native workflow admission and approval
//! settlement belong to the caller; this record cannot replace either owner.
use super::*;
use meerkat_core::MobMemberBinding;
use meerkat_toolkit::host_lifetime::control::SelectedHostArtifact;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClosedResource {
    schema_version: u32,
    realm: String,
    state_root: PathBuf,
    artifact_directory: PathBuf,
    resources_sha256: String,
    descriptor_sha256: String,
    preparation_sha256: String,
    handoff_sha256: String,
    member: MobMemberBinding,
    session: String,
    peer: usize,
    snapshot_sha256: String,
    session_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClosedSnapshot {
    schema_version: u32,
    owner: String,
    round: serde_json::Value,
    controller: serde_json::Value,
}

fn current_snapshot(raw: &[u8], session: &[u8], identity: &str) -> Result<(), ConnectError> {
    let value: ClosedSnapshot = serde_json::from_slice(raw).map_err(|_| unavailable())?;
    if value.schema_version != 1
        || value.owner != identity
        || value.round["session_sha256"] != digest(session)
        || value.controller["scope"] != "joined_shared_controller_readback_only"
    {
        return Err(unavailable());
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, ConnectError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| unavailable())?;
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file() || metadata.len() > MANIFEST_LIMIT || mode(&metadata) & 0o077 != 0 {
        return Err(unavailable());
    }
    let mut raw = Vec::new();
    Read::take(file, MANIFEST_LIMIT + 1)
        .read_to_end(&mut raw)
        .map_err(|_| unavailable())?;
    if raw.len() as u64 > MANIFEST_LIMIT {
        return Err(unavailable());
    }
    Ok(raw)
}

fn sync(path: &Path) -> Result<(), ConnectError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable())
}

fn write_new(path: &Path, raw: &[u8]) -> Result<(), ConnectError> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| unavailable())?;
    file.write_all(raw)
        .and_then(|_| file.sync_all())
        .map_err(|_| unavailable())?;
    sync(path.parent().ok_or_else(unavailable)?)
}

/// Descriptor, preparation and handoff bytes, plus the resources digest.
type RetainedPreparation = (Vec<u8>, Vec<u8>, Vec<u8>, String);

fn retained_preparation(directory: &Path) -> Result<RetainedPreparation, ConnectError> {
    let descriptor = read(&directory.join("prepared/product-mcp.json"))?;
    let preparation = read(&directory.join("prepared/product-prepared.json"))?;
    let handoff = read(&directory.join("prepare.stdout"))?;
    let product: serde_json::Value =
        serde_json::from_slice(&preparation).map_err(|_| unavailable())?;
    let resources = product["resources_sha256"]
        .as_str()
        .filter(|value| valid_digest(value))
        .ok_or_else(unavailable)?
        .to_owned();
    if product["local_setup"]["descriptor_sha256"] != digest(&descriptor)
        || serde_json::from_slice::<serde_json::Value>(&descriptor).map_err(|_| unavailable())?
            != serde_json::from_slice::<serde_json::Value>(&handoff).map_err(|_| unavailable())?
    {
        return Err(unavailable());
    }
    Ok((descriptor, preparation, handoff, resources))
}

impl SharedSetupOwner {
    /// Close this exact resource, retaining its cleanup even if the observer
    /// disappears. Only a joined selected native peer and an owner-produced
    /// settled Controller snapshot can produce a physical close selection.
    /// None preserves unresolved work; it never authorizes a replacement.
    pub(crate) fn close_for_restore(
        self,
        context: Arc<SharedSetupCallContext>,
    ) -> impl Future<Output = Result<Option<SelectedHostArtifact>, ConnectError>> + Send + 'static
    {
        let entries = self.entries.clone();
        let selected = Arc::new(std::sync::OnceLock::new());
        let produced = selected.clone();
        let retained = entries.retain_resource_close(async move {
            let registration = if Arc::ptr_eq(&context.peer.channel, &self.channel)
                && Arc::ptr_eq(&context.peer.entries, &self.entries)
            {
                context.peer.attempt.closed_registration()
            } else {
                None
            };
            let directory = self.artifact_directory.clone();
            let realm = self.realm.clone();
            let close_identity = self.close_identity.clone();
            let peer = context.peer.index;
            self.close_owned().await?;
            let Some(close_identity) = close_identity else {
                return Ok(());
            };
            let Some((member, session)) = registration else {
                return Ok(());
            };
            let snapshot_path = directory.join("prepared/local-session/shared-close.json");
            match fs::symlink_metadata(&snapshot_path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(_) => return Err(unavailable()),
                Ok(_) => {}
            }
            let snapshot = read(&snapshot_path)?;
            let session_raw = read(&directory.join("prepared/local-session/session.json"))?;
            current_snapshot(&snapshot, &session_raw, &close_identity)?;
            let (descriptor, preparation, handoff, resources_sha256) =
                retained_preparation(&directory)?;
            let snapshot_sha256 = digest(&snapshot);
            let path = directory.join(format!("shared-resource-closed-{snapshot_sha256}.json"));
            let closed = ClosedResource {
                schema_version: 1,
                realm: realm.realm.as_str().into(),
                state_root: realm.state_root,
                artifact_directory: directory,
                resources_sha256,
                descriptor_sha256: digest(&descriptor),
                preparation_sha256: digest(&preparation),
                handoff_sha256: digest(&handoff),
                member,
                session,
                peer,
                snapshot_sha256,
                session_sha256: digest(&session_raw),
            };
            let raw = serde_json::to_vec(&closed).map_err(|_| unavailable())?;
            write_new(&path, &raw)?;
            produced
                .set(SelectedHostArtifact {
                    path,
                    sha256: digest(&raw),
                })
                .map_err(|_| unavailable())?;
            Ok(())
        });
        async move {
            let retained = retained?;
            (&mut *retained.lock().await)
                .await
                .map_err(|_| unavailable())??;
            Ok(selected.get().cloned())
        }
    }
}

impl InstalledResources {
    /// Consume an exactly selected physical close record for one installed G
    /// peer. The caller must already have current canonical admission plus
    /// independent native-idle and approval-owner proof. Prior P facts are
    /// retained readback only; no prior effect, entry or validation is replayed.
    pub(crate) async fn restore_shared_setup(
        self,
        storage: &meerkat_core::ResolvedStorage,
        resources: &crate::application_setup::PreparedApplicationResources,
        selected: &SelectedHostArtifact,
        member: &MobMemberBinding,
        session: &str,
        cancelled: impl Future<Output = io::Result<()>>,
    ) -> Result<PreparedSharedSetup, ConnectError> {
        let directory = &resources.artifact_directory;
        let raw = read(&selected.path)?;
        let closed: ClosedResource = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
        let (descriptor, preparation, handoff, resource_sha) = retained_preparation(directory)?;
        let manifest = self.root.join("resources.json");
        let size = fs::metadata(&manifest).map_err(|_| unavailable())?.len();
        let root = storage
            .locator
            .state_root
            .canonicalize()
            .map_err(|_| unavailable())?;
        let old_root = closed
            .state_root
            .canonicalize()
            .map_err(|_| unavailable())?;
        if closed.schema_version != 1
            || digest(&raw) != selected.sha256
            || closed.artifact_directory != *directory
            || directory.canonicalize().map_err(|_| unavailable())? != *directory
            || directory.parent()
                != Some(
                    root.join(".meerkat-connect")
                        .join(storage.locator.realm.as_str())
                        .as_path(),
                )
            || closed.realm != storage.locator.realm.as_str()
            || root != old_root
            || &closed.member != member
            || closed.session != session
            || session.is_empty()
            || closed.peer >= 8
            || selected.path
                != directory.join(format!(
                    "shared-resource-closed-{}.json",
                    closed.snapshot_sha256
                ))
            || closed.resources_sha256 != resource_sha
            || resource_sha != hash_file(&manifest, size)?
            || closed.descriptor_sha256 != digest(&descriptor)
            || closed.preparation_sha256 != digest(&preparation)
            || closed.handoff_sha256 != digest(&handoff)
            || resources.handoff.sha256 != digest(&handoff)
            || resources.handoff.path != directory.join("prepare.stdout")
            || closed.snapshot_sha256
                != digest(&read(
                    &directory.join("prepared/local-session/shared-close.json"),
                )?)
            || closed.session_sha256
                != digest(&read(
                    &directory.join("prepared/local-session/session.json"),
                )?)
        {
            return Err(unavailable());
        }
        let connection = PreparedSetupConnection::from_preparer_output(&handoff)?;
        // Atomic create-new claim. A failed or interrupted launch retains this
        // exact claim and all old requests; it cannot select another attempt.
        let claimed = selected.path.with_extension("claimed");
        fs::hard_link(&selected.path, &claimed).map_err(|_| unavailable())?;
        sync(directory)?;
        if read(&claimed)? != raw || read(&selected.path)? != raw {
            return Err(unavailable());
        }
        self.launch_shared_setup(
            storage,
            PreparedResources {
                connection,
                artifact_directory: directory.clone(),
            },
            1,
            Some(&closed.snapshot_sha256),
            cancelled,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_snapshot_requires_this_joined_invocation_and_exact_session() {
        let session = br#"{"workspace":"original"}"#;
        let identity = "a".repeat(64);
        let snapshot = serde_json::json!({"schema_version":1, "owner":identity,
            "round":{"session_sha256":digest(session)},
            "controller":{"scope":"joined_shared_controller_readback_only"}});
        let raw = serde_json::to_vec(&snapshot).unwrap();
        assert!(current_snapshot(&raw, session, &identity).is_ok());
        assert!(
            current_snapshot(&raw, session, &"b".repeat(64)).is_err(),
            "an old close file cannot certify the current physical owner"
        );
        assert!(current_snapshot(&raw, b"changed session", &identity).is_err());
        let mut changed = snapshot;
        changed["native_idle"] = true.into();
        assert!(
            current_snapshot(&serde_json::to_vec(&changed).unwrap(), session, &identity).is_err(),
            "physical owner output cannot add native admission truth"
        );
    }
}
