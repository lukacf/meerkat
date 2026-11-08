//! Admission checks on actual Linux process and installed filesystem objects.
//! The root launcher must additionally exclude hostile same-UID access before
//! exec and throughout helper execution. These checks do not create namespaces
//! or turn private descriptors into a process/filesystem isolation boundary.
use super::{
    mapping::{ExecutionMapping, InstallationMapping, ParsedMapping, ProviderStore},
    refused,
};
use crate::ConnectError;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) fn protect_process() -> Result<(), ConnectError> {
    // no_new_privs is inherited by subsequently created threads. Setting it on
    // one worker of an existing runtime would leave other threads unaffected.
    let tasks = fs::read_dir("/proc/self/task")
        .map_err(|_| refused())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| refused())?;
    if tasks.len() != 1 {
        return Err(refused());
    }
    let ids = nix::unistd::getresuid().map_err(|_| refused())?;
    if ids.real.is_root() || ids.effective != ids.real || ids.saved != ids.real {
        return Err(refused());
    }
    nix::sys::prctl::set_dumpable(false).map_err(|_| refused())?;
    nix::sys::prctl::set_no_new_privs().map_err(|_| refused())?;
    if nix::sys::prctl::get_dumpable().map_err(|_| refused())?
        || !nix::sys::prctl::get_no_new_privs().map_err(|_| refused())?
    {
        return Err(refused());
    }
    Ok(())
}

pub(super) fn verify_identity(expected: &ExecutionMapping) -> Result<(), ConnectError> {
    let uid = nix::unistd::getresuid().map_err(|_| refused())?;
    let gid = nix::unistd::getresgid().map_err(|_| refused())?;
    let mut groups: Vec<_> = nix::unistd::getgroups()
        .map_err(|_| refused())?
        .into_iter()
        .map(|id| id.as_raw())
        .collect();
    groups.sort_unstable();
    if expected.uid == 0
        || expected.gid == 0
        || expected.supplementary_gids.contains(&0)
        || [
            uid.real.as_raw(),
            uid.effective.as_raw(),
            uid.saved.as_raw(),
        ] != [expected.uid; 3]
        || [
            gid.real.as_raw(),
            gid.effective.as_raw(),
            gid.saved.as_raw(),
        ] != [expected.gid; 3]
        || groups != expected.supplementary_gids
    {
        return Err(refused());
    }
    let mut raw = String::new();
    fs::File::open("/proc/self/status")
        .map_err(|_| refused())?
        .take(65537)
        .read_to_string(&mut raw)
        .map_err(|_| refused())?;
    verify_status(&raw, expected)
}

fn verify_status(raw: &str, expected: &ExecutionMapping) -> Result<(), ConnectError> {
    if raw.len() > 65536 {
        return Err(refused());
    }
    let mut fields = BTreeMap::new();
    for line in raw.lines() {
        let Some((key, value)) = line.split_once(':') else {
            return Err(refused());
        };
        if fields.insert(key, value.trim()).is_some() {
            return Err(refused());
        }
    }
    for (field, identity) in [("Uid", expected.uid), ("Gid", expected.gid)] {
        let ids = fields
            .get(field)
            .ok_or_else(refused)?
            .split_whitespace()
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| refused())?;
        // Include the actual filesystem identity, not only real/effective/saved.
        if ids != [identity; 4] {
            return Err(refused());
        }
    }
    for field in ["CapInh", "CapPrm", "CapEff", "CapAmb"] {
        let value = fields.get(field).ok_or_else(refused)?;
        if value.len() != 16 || u64::from_str_radix(value, 16).map_err(|_| refused())? != 0 {
            return Err(refused());
        }
    }
    if fields.get("NoNewPrivs") != Some(&"1") || fields.get("Threads") != Some(&"1") {
        return Err(refused());
    }
    Ok(())
}

fn ancestors(path: &Path) -> Result<Vec<PathBuf>, ConnectError> {
    if !path.is_absolute() || path.canonicalize().map_err(|_| refused())? != path {
        return Err(refused());
    }
    let mut paths: Vec<_> = path.ancestors().map(Path::to_path_buf).collect();
    paths.reverse();
    Ok(paths)
}

fn require_root_path(path: &Path, file: bool) -> Result<(), ConnectError> {
    for part in ancestors(path)? {
        let info = fs::symlink_metadata(&part).map_err(|_| refused())?;
        if info.uid() != 0
            || info.mode() & 0o7022 != 0
            || if part == path && file {
                !info.is_file() || info.mode() & 0o111 == 0
            } else {
                !info.is_dir()
            }
        {
            return Err(refused());
        }
    }
    Ok(())
}

fn root_resource_metadata(info: &fs::Metadata) -> Result<(), ConnectError> {
    if info.uid() != 0
        || !info.file_type().is_symlink()
            && (info.mode() & 0o7022 != 0 || !(info.is_dir() || info.is_file()))
    {
        return Err(refused());
    }
    Ok(())
}

pub(super) fn verify_installation(
    mapping: &InstallationMapping,
) -> Result<crate::resources::InstalledResources, ConnectError> {
    if std::env::current_exe().map_err(|_| refused())? != mapping.executable {
        return Err(refused());
    }
    require_root_path(&mapping.executable, true)?;
    require_root_path(&mapping.resource_root, false)?;
    // Validate ownership of the entire closure. Content/mode verification
    // alone would accept a child-owned 0644 script that the child could rewrite.
    let mut directories = vec![mapping.resource_root.clone()];
    let mut entries = 0;
    while let Some(directory) = directories.pop() {
        for item in fs::read_dir(directory).map_err(|_| refused())? {
            let item = item.map_err(|_| refused())?;
            entries += 1;
            if entries > 100_000 {
                return Err(refused());
            }
            let info = fs::symlink_metadata(item.path()).map_err(|_| refused())?;
            root_resource_metadata(&info)?;
            if info.file_type().is_symlink() {
                let target = item.path().canonicalize().map_err(|_| refused())?;
                if !target.starts_with(&mapping.resource_root) {
                    return Err(refused());
                }
            } else {
                if info.is_dir() {
                    directories.push(item.path());
                }
            }
        }
    }
    crate::resources::InstalledResources::locate_protected(
        &mapping.executable,
        &mapping.resource_root,
        &mapping.resource_manifest_sha256,
    )
}

fn private_directory(path: &Path, uid: u32) -> Result<(), ConnectError> {
    for part in ancestors(path)? {
        let info = fs::symlink_metadata(&part).map_err(|_| refused())?;
        private_component(&info, uid, part == path)?;
    }
    Ok(())
}
fn private_component(info: &fs::Metadata, uid: u32, leaf: bool) -> Result<(), ConnectError> {
    if !info.is_dir()
        || ![0, uid].contains(&info.uid())
        || info.mode() & 0o7022 != 0
        || leaf && (info.uid() != uid || info.mode() & 0o777 != 0o700)
    {
        return Err(refused());
    }
    Ok(())
}

fn selected_provider_root(store: &ProviderStore) -> Result<PathBuf, ConnectError> {
    let root = match store {
        ProviderStore::File { root } => root,
        ProviderStore::NativeDefault {
            home,
            xdg_config_home,
            root,
        } => {
            if std::env::var_os("HOME").as_deref() != Some(home.as_os_str())
                || std::env::var_os("XDG_CONFIG_HOME").as_deref()
                    != xdg_config_home.as_ref().map(|path| path.as_os_str())
            {
                return Err(refused());
            }
            let backend = meerkat_auth_core::auth_store::TokenStoreBackend::default_auto()
                .map_err(|_| refused())?;
            match backend {
                meerkat_auth_core::auth_store::TokenStoreBackend::File { root: actual }
                    if actual == *root => {}
                _ => return Err(refused()),
            }
            root
        }
    };
    Ok(root.clone())
}

pub(super) fn verify_roots(mapping: &ParsedMapping) -> Result<PathBuf, ConnectError> {
    // The launcher precreates private roots, even before first credentials.
    // Native lazy directory creation is not used as a permission proof.
    private_directory(&mapping.realm.state_root, mapping.execution.uid)?;
    private_directory(&mapping.realm.user_config_root, mapping.execution.uid)?;
    let context = fs::symlink_metadata(&mapping.realm.context_root).map_err(|_| refused())?;
    if !context.is_dir()
        || mapping
            .realm
            .context_root
            .canonicalize()
            .map_err(|_| refused())?
            != mapping.realm.context_root
    {
        return Err(refused());
    }
    let root = selected_provider_root(&mapping.provider_store)?;
    private_directory(&root, mapping.execution.uid)?;
    Ok(root)
}

#[cfg(test)]
mod tests;
