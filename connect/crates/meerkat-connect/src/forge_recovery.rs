//! Nonsecret locators in existing realm setup artifacts, never restored grants.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

fn owned(path: &Path, directory: bool) -> Result<(), ForgeError> {
    let m = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if m.uid() != process_uid()?
        || m.mode() & 0o077 != 0
        || m.file_type().is_symlink()
        || if directory { !m.is_dir() } else { !m.is_file() }
    {
        return Err(invalid());
    }
    Ok(())
}
fn process_uid() -> Result<u32, ForgeError> {
    // A newly created anonymous file has this process's effective filesystem
    // owner. No extra OS API feature or persistent identity file is needed.
    tempfile::tempfile()
        .and_then(|f| f.metadata())
        .map(|m| m.uid())
        .map_err(|_| invalid())
}
fn read(path: &Path) -> Result<RecoveryReference, ForgeError> {
    owned(path, false)?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| invalid())?;
    let m = file.metadata().map_err(|_| invalid())?;
    if !m.is_file() || m.uid() != process_uid()? || m.mode() & 0o077 != 0 || m.len() > LIMIT as u64
    {
        return Err(invalid());
    }
    let mut bytes = String::new();
    std::io::Read::take(&mut file, (LIMIT + 1) as u64)
        .read_to_string(&mut bytes)
        .map_err(|_| invalid())?;
    let r: RecoveryReference = parse(&bytes)?;
    if !r.host.valid() || !id(&r.change_id) || ForgeClient::new(&r.origin).is_err() {
        return Err(invalid());
    }
    validate_reference_shape(&r)?;
    Ok(r)
}
pub(super) struct Discovery {
    directories: fs::ReadDir,
    files: Option<fs::ReadDir>,
    exhausted: bool,
}
pub(super) struct Page {
    pub references: Vec<RecoveryReference>,
    pub skipped: usize,
    pub more: bool,
}
pub(super) fn discover(directory: &Path) -> Result<Discovery, ForgeError> {
    if !directory.is_absolute() {
        return Err(invalid());
    }
    owned(directory, true)?;
    let parent = directory.parent().ok_or_else(invalid)?;
    owned(parent, true)?;
    Ok(Discovery {
        directories: fs::read_dir(parent).map_err(|_| invalid())?,
        files: None,
        exhausted: false,
    })
}
impl Discovery {
    pub(super) fn page(&mut self) -> Page {
        let mut page = Page {
            references: Vec::new(),
            skipped: 0,
            more: false,
        };
        // Work is bounded per presented page, never per realm lifetime. No
        // persisted index or status is inferred from traversal order.
        for _ in 0..128 {
            if let Some(files) = &mut self.files {
                match files.next() {
                    None => self.files = None,
                    Some(Err(_)) => page.skipped += 1,
                    Some(Ok(file)) => {
                        let name = file.file_name();
                        if name
                            .to_str()
                            .is_some_and(|s| s.starts_with("forge-change-") && s.ends_with(".json"))
                        {
                            match read(&file.path()) {
                                Ok(r) => page.references.push(r),
                                Err(_) => page.skipped += 1,
                            }
                        }
                    }
                }
            } else {
                match self.directories.next() {
                    None => {
                        self.exhausted = true;
                        break;
                    }
                    Some(Err(_)) => page.skipped += 1,
                    Some(Ok(entry)) => {
                        if !entry
                            .file_name()
                            .to_str()
                            .is_some_and(|s| s.starts_with("setup-"))
                        {
                            continue;
                        }
                        if owned(&entry.path(), true).is_err() {
                            page.skipped += 1;
                            continue;
                        }
                        match fs::read_dir(entry.path()) {
                            Ok(files) => self.files = Some(files),
                            Err(_) => page.skipped += 1,
                        }
                    }
                }
            }
        }
        page.more = !self.exhausted;
        page
    }
}
pub(super) fn save(directory: &Path, reference: &RecoveryReference) -> Result<(), ForgeError> {
    owned(directory, true)?;
    let bytes = serde_json::to_vec(reference).map_err(|_| invalid())?;
    if bytes.len() > LIMIT {
        return Err(invalid());
    }
    let name = format!(
        "forge-change-{:x}.json",
        Sha256::digest(reference.change_id.as_bytes())
    );
    let path = directory.join(name);
    if path.exists() {
        let old = read(&path)?;
        return if serde_json::to_vec(&old).map_err(|_| invalid())? == bytes {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(|_| invalid())?;
    temp.write_all(&bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| invalid())?;
    temp.persist_noclobber(&path).map_err(|_| invalid())?;
    let parent = fs::File::open(directory).map_err(|_| invalid())?;
    // The reference must be durable before the caller can approve a submit.
    parent.sync_all().map_err(|_| invalid())?;
    Ok(())
}
