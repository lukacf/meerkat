//! Durable selection before child launch. Identity only, never a process-exit,
//! native registration, operation outcome, or resource-reuse proof.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PhysicalLifetime {
    pub(super) schema_version: u32,
    pub(super) owner: String,
    realm: String,
    state_root: PathBuf,
    pub(super) artifact_directory: PathBuf,
    resources_sha256: String,
    command: String,
    arguments: Vec<String>,
    peers: usize,
    restored: Option<String>,
    pub(super) preparer: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) fence: Option<super::physical::Fence>,
}

pub(super) fn read(path: &Path) -> Result<Vec<u8>, ConnectError> {
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
    file.take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut raw)
        .map_err(|_| unavailable())?;
    if raw.len() as u64 > MANIFEST_LIMIT {
        return Err(unavailable());
    }
    Ok(raw)
}

pub(super) fn write_new(path: &Path, raw: &[u8]) -> Result<(), ConnectError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| unavailable())?;
    file.write_all(raw)
        .and_then(|_| file.sync_all())
        .map_err(|_| unavailable())?;
    fs::File::open(path.parent().ok_or_else(unavailable)?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| unavailable())
}

impl PhysicalLifetime {
    pub(super) fn retain(
        directory: &Path,
        resources: &Path,
        realm: &meerkat_core::RealmLocator,
        owner: &str,
        command: &str,
        arguments: &[String],
        peers: usize,
        restored: Option<&str>,
    ) -> Result<(PathBuf, String), ConnectError> {
        if !valid_digest(owner)
            || !(1..=8).contains(&peers)
            || restored.is_some_and(|value| !valid_digest(value))
            || directory.canonicalize().map_err(|_| unavailable())? != directory
        {
            return Err(unavailable());
        }
        // Every public preparer authenticates and retains this exact descriptor.
        // Product-private preparation files are not a universal prerequisite.
        let mut preparer = BTreeMap::new();
        let name = "prepare.stdout";
        preparer.insert(name.into(), digest(&read(&directory.join(name))?));
        // The already selected installed resource manifest can be public mode;
        // it is validated by InstalledResources before this launch.
        let manifest = resources.join("resources.json");
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(&manifest)
            .map_err(|_| unavailable())?;
        let metadata = file.metadata().map_err(|_| unavailable())?;
        if !metadata.is_file() || metadata.len() > MANIFEST_LIMIT || mode(&metadata) & 0o7022 != 0 {
            return Err(unavailable());
        }
        let mut resource_raw = Vec::new();
        file.take(MANIFEST_LIMIT + 1)
            .read_to_end(&mut resource_raw)
            .map_err(|_| unavailable())?;
        if resource_raw.len() as u64 > MANIFEST_LIMIT {
            return Err(unavailable());
        }
        let fence = super::physical::create(directory, owner)?;
        let value = Self {
            schema_version: 2,
            owner: owner.into(),
            realm: realm.realm.as_str().into(),
            state_root: realm.state_root.clone(),
            artifact_directory: directory.into(),
            resources_sha256: digest(&resource_raw),
            command: command.into(),
            arguments: arguments.into(),
            peers,
            restored: restored.map(str::to_owned),
            preparer,
            fence: Some(fence),
        };
        let raw = serde_json::to_vec(&value).map_err(|_| unavailable())?;
        let path = directory.join(format!("shared-lifetime-{owner}.json"));
        // Never replace a previous lifetime or classify a failed spawn as a
        // joined process. Partial/new records remain retained on every error.
        write_new(&path, &raw)?;
        if read(&path)? != raw {
            return Err(unavailable());
        }
        Ok((path, digest(&raw)))
    }

    #[cfg(feature = "console")]
    pub(super) fn selected(directory: &Path, owner: &str) -> Result<(Self, Vec<u8>), ConnectError> {
        if !valid_digest(owner) {
            return Err(unavailable());
        }
        let raw = read(&directory.join(format!("shared-lifetime-{owner}.json")))?;
        let value: Self = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
        if !matches!(value.schema_version, 1 | 2)
            || (value.schema_version == 2) != value.fence.is_some()
            || value.owner != owner
            || value.artifact_directory != directory
            || serde_json::to_vec(&value).map_err(|_| unavailable())? != raw
        {
            return Err(unavailable());
        }
        for (name, expected) in &value.preparer {
            if !["prepare.stdout"].contains(&name.as_str())
                || digest(&read(&directory.join(name))?) != *expected
            {
                return Err(unavailable());
            }
        }
        if value.preparer.len() != 1 {
            return Err(unavailable());
        }
        Ok((value, raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn physical_lifetime_selection_is_durable_before_launch_and_never_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let directory = root.join("invocation");
        fs::create_dir_all(directory.join("prepared")).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        {
            let name = "prepare.stdout";
            let path = directory.join(name);
            fs::write(&path, name.as_bytes()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let resources = root.join("resources");
        fs::create_dir(&resources).unwrap();
        fs::write(resources.join("resources.json"), b"selected resources").unwrap();
        let realm = meerkat_core::RealmLocator {
            state_root: root.clone(),
            realm: meerkat_core::mob_realm_id("lifetime-control").unwrap(),
        };
        let identity = "a".repeat(64);
        let (path, expected) = PhysicalLifetime::retain(
            &directory,
            &resources,
            &realm,
            &identity,
            "/usr/bin/env",
            &["selected-launch".into()],
            2,
            None,
        )
        .unwrap();
        let before = read(&path).unwrap();
        assert_eq!(digest(&before), expected);
        assert_eq!(mode(&fs::metadata(&path).unwrap()), 0o600);
        let selected: PhysicalLifetime = serde_json::from_slice(&before).unwrap();
        assert_eq!(selected.owner, identity);
        assert_eq!(selected.peers, 2);
        assert_eq!(selected.arguments, vec!["selected-launch".to_owned()]);
        assert_eq!(selected.preparer.len(), 1);
        assert!(!directory.join("prepared/product-mcp.json").exists());
        assert!(!directory.join("prepared/product-prepared.json").exists());
        assert!(
            PhysicalLifetime::retain(
                &directory,
                &resources,
                &realm,
                &identity,
                "/usr/bin/env",
                &["different".into()],
                1,
                None
            )
            .is_err()
        );
        assert_eq!(read(&path).unwrap(), before);
        // This test calls the actual durable selection writer, not a process
        // stub. No process was launched, so no cleanup/effect proof exists.
        assert_eq!(
            fs::read_dir(&directory)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("shared-abandoned-"))
                .count(),
            0
        );
    }
}
