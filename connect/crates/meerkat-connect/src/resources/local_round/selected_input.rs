//! Bounded placement through the actual preparation owner, before native add.
//! No path from model text or a reconstructed preparation can enter this code.
use super::{LocalRoundOwner, unavailable};
use crate::{
    ConnectError,
    protected_parent::selected_input::{SelectedInput, StagedSelectedInput, digest, is_digest},
};
use meerkat_core::{McpServerConfig, mcp_config::McpTransportConfig};
use nix::{
    fcntl::{OFlag, open, openat},
    sys::stat::{Mode, mkdirat},
};
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

const CONFIG_LIMIT: usize = 1024 * 1024;

impl LocalRoundOwner {
    /// Runs on the original custody's retained blocking task. The task returns
    /// this exact owner even on failure or panic, and close joins that task.
    pub(super) fn stage_selected_input(
        &mut self,
        server: &McpServerConfig,
        input: &SelectedInput,
        cancelled: &impl Fn() -> bool,
    ) -> Result<StagedSelectedInput, ConnectError> {
        let result = self.copy_selected_input(server, input, cancelled);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn copy_selected_input(
        &self,
        server: &McpServerConfig,
        input: &SelectedInput,
        cancelled: &impl Fn() -> bool,
    ) -> Result<StagedSelectedInput, ConnectError> {
        let live = || {
            if cancelled() || input.retired() {
                Err(unavailable())
            } else {
                Ok(())
            }
        };
        live()?;
        if self.poisoned || self.has_pending_round() {
            return Err(unavailable());
        }
        self.check_resources()?;
        let artifacts = Directory::absolute(&self.artifacts)?;
        let prepared = artifacts.child("prepared")?;
        let descriptor = prepared.read("product-mcp.json", CONFIG_LIMIT)?;
        let preparation = prepared.read("product-prepared.json", CONFIG_LIMIT)?;
        if self.request["prepared"].as_str() != prepared.path.to_str()
            || self.request["descriptor_sha256"].as_str() != Some(digest(&descriptor).as_str())
            || self.request["preparation_sha256"].as_str() != Some(digest(&preparation).as_str())
        {
            return Err(unavailable());
        }
        let original = crate::setup::PreparedSetupConnection::from_preparer_output(&descriptor)?;
        if original.server().name != server.name || original.server().transport != server.transport
        {
            return Err(unavailable());
        }
        let McpTransportConfig::Stdio(transport) = &server.transport else {
            return Err(unavailable());
        };
        let config = prepared.child("config")?;
        let config_path = config.path.join("mcp.json");
        let config_hash = argument(&transport.args, "--configuration-sha256")?;
        if argument(&transport.args, "--configuration")?
            != config_path.to_str().ok_or_else(unavailable)?
            || !is_digest(config_hash)
        {
            return Err(unavailable());
        }
        let config_raw = config.read("mcp.json", CONFIG_LIMIT)?;
        let prepared_value: serde_json::Value =
            serde_json::from_slice(&preparation).map_err(|_| unavailable())?;
        if digest(&config_raw) != config_hash
            || prepared_value["mcp_sha256"].as_str() != Some(config_hash)
        {
            return Err(unavailable());
        }
        let backend: serde_json::Value =
            serde_json::from_slice(&config_raw).map_err(|_| unavailable())?;
        let args: Vec<String> = backend["mcpServers"]["toolkit-setup"]["args"]
            .as_array()
            .ok_or_else(unavailable)?
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(unavailable))
            .collect::<Result<_, _>>()?;
        let profile_path = config.path.join("selected-install.json");
        let profile_hash = argument(&args, "--selected-install-profile-sha256")?;
        if argument(&args, "--selected-install-profile")?
            != profile_path.to_str().ok_or_else(unavailable)?
            || !is_digest(profile_hash)
        {
            return Err(unavailable());
        }
        let profile_raw = config.read("selected-install.json", CONFIG_LIMIT)?;
        if digest(&profile_raw) != profile_hash {
            return Err(unavailable());
        }
        let profile: serde_json::Value =
            serde_json::from_slice(&profile_raw).map_err(|_| unavailable())?;
        let staging = prepared.child("staging")?;
        if profile["schema_version"] != 1
            || profile["scope"] != "standing_local_software_installation_only"
            || profile["staging_root"].as_str() != staging.path.to_str()
            || !profile["validators"]["python"].is_object()
        {
            return Err(unavailable());
        }
        live()?;
        let staged_id = format!("input-{}", uuid::Uuid::new_v4().simple());
        let target = staging.create(&staged_id)?;
        let bundle = input.bundle();
        let closure = if bundle
            .members
            .iter()
            .any(|m| m.name.starts_with("closure/"))
        {
            Some(target.create("closure")?)
        } else {
            None
        };
        let mut copied = Vec::with_capacity(bundle.members.len());
        for member in &bundle.members {
            live()?;
            let (directory, leaf) = match member.name.strip_prefix("closure/") {
                Some(leaf) => (closure.as_ref().ok_or_else(unavailable)?, leaf),
                None => (&target, member.name.as_str()),
            };
            copied.push((
                directory,
                directory.write(leaf, bundle.bytes(member), &live)?,
            ));
        }
        if let Some(closure) = &closure {
            closure.current()?;
            closure.file.sync_all().map_err(|_| unavailable())?;
        }
        target.file.sync_all().map_err(|_| unavailable())?;
        staging.file.sync_all().map_err(|_| unavailable())?;
        // Reobserve the exact original path/descriptor chain and pinned config
        // after copy. Refusal leaves partial files private and unprojected.
        for directory in [&artifacts, &prepared, &config, &staging, &target] {
            directory.current()?;
        }
        if prepared.read("product-mcp.json", CONFIG_LIMIT)? != descriptor
            || prepared.read("product-prepared.json", CONFIG_LIMIT)? != preparation
            || config.read("mcp.json", CONFIG_LIMIT)? != config_raw
            || config.read("selected-install.json", CONFIG_LIMIT)? != profile_raw
        {
            return Err(unavailable());
        }
        self.check_resources()?;
        // Keep every original file open through the completion pass. A same-byte
        // replacement of an earlier member must not become a staged input.
        verify_members(&mut copied, &live)?;
        live()?;
        bundle.observation(staged_id)
    }
}

fn argument<'a>(args: &'a [String], key: &str) -> Result<&'a str, ConnectError> {
    let rows: Vec<_> = args
        .iter()
        .enumerate()
        .filter(|(_, s)| s.as_str() == key)
        .collect();
    if rows.len() != 1 {
        return Err(unavailable());
    }
    args.get(rows[0].0 + 1)
        .map(String::as_str)
        .ok_or_else(unavailable)
}

struct Directory {
    path: PathBuf,
    file: File,
    identity: (u64, u64),
}
fn identity(meta: &Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}
fn private(meta: &Metadata, directory: bool) -> Result<(), ConnectError> {
    let expected_mode = if directory { 0o700 } else { 0o600 };
    let wrong_kind = if directory {
        !meta.is_dir()
    } else {
        !meta.is_file() || meta.nlink() != 1
    };
    if meta.uid() != nix::unistd::getuid().as_raw()
        || meta.mode() & 0o7777 != expected_mode
        || wrong_kind
    {
        return Err(unavailable());
    }
    Ok(())
}
fn leaf(value: &str) -> Result<&str, ConnectError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\0')
    {
        return Err(unavailable());
    }
    Ok(value)
}
impl Directory {
    fn absolute(path: &Path) -> Result<Self, ConnectError> {
        let spelling = path.to_str().ok_or_else(unavailable)?;
        if !spelling.starts_with('/')
            || spelling[1..]
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(unavailable());
        }
        let flags = OFlag::O_RDONLY
            | OFlag::O_DIRECTORY
            | OFlag::O_NOFOLLOW
            | OFlag::O_CLOEXEC
            | OFlag::O_NONBLOCK;
        let mut file = File::from(open("/", flags, Mode::empty()).map_err(|_| unavailable())?);
        for part in path.components() {
            match part {
                Component::RootDir => (),
                Component::Normal(name) => {
                    file = File::from(
                        openat(&file, name, flags, Mode::empty()).map_err(|_| unavailable())?,
                    )
                }
                _ => return Err(unavailable()),
            }
        }
        let meta = file.metadata().map_err(|_| unavailable())?;
        private(&meta, true)?;
        Ok(Self {
            path: path.into(),
            file,
            identity: identity(&meta),
        })
    }
    fn current(&self) -> Result<(), ConnectError> {
        if Self::absolute(&self.path)?.identity != self.identity {
            return Err(unavailable());
        }
        Ok(())
    }
    fn child(&self, name: &str) -> Result<Self, ConnectError> {
        self.current()?;
        let file = File::from(
            openat(
                &self.file,
                leaf(name)?,
                OFlag::O_RDONLY
                    | OFlag::O_DIRECTORY
                    | OFlag::O_CLOEXEC
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_NONBLOCK,
                Mode::empty(),
            )
            .map_err(|_| unavailable())?,
        );
        let meta = file.metadata().map_err(|_| unavailable())?;
        private(&meta, true)?;
        let value = Self {
            path: self.path.join(name),
            file,
            identity: identity(&meta),
        };
        value.current()?;
        Ok(value)
    }
    fn create(&self, name: &str) -> Result<Self, ConnectError> {
        self.current()?;
        mkdirat(&self.file, leaf(name)?, Mode::from_bits_truncate(0o700))
            .map_err(|_| unavailable())?;
        self.file.sync_all().map_err(|_| unavailable())?;
        self.child(name)
    }
    fn read(&self, name: &str, max: usize) -> Result<Vec<u8>, ConnectError> {
        self.current()?;
        let mut file = File::from(
            openat(
                &self.file,
                leaf(name)?,
                OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
                Mode::empty(),
            )
            .map_err(|_| unavailable())?,
        );
        let meta = file.metadata().map_err(|_| unavailable())?;
        private(&meta, false)?;
        if meta.len() > max as u64 {
            return Err(unavailable());
        }
        let bytes = bounded_read(&mut file, max, &|| Ok(()))?;
        let after = file.metadata().map_err(|_| unavailable())?;
        private(&after, false)?;
        if identity(&after) != identity(&meta) || after.len() != meta.len() {
            return Err(unavailable());
        }
        if bytes.len() as u64 != meta.len() {
            return Err(unavailable());
        }
        self.current()?;
        Ok(bytes)
    }
    fn write(
        &self,
        name: &str,
        bytes: &[u8],
        live: &impl Fn() -> Result<(), ConnectError>,
    ) -> Result<WrittenFile, ConnectError> {
        self.current()?;
        live()?;
        let mut file = File::from(
            openat(
                &self.file,
                leaf(name)?,
                OFlag::O_RDWR
                    | OFlag::O_CLOEXEC
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_NONBLOCK
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|_| unavailable())?,
        );
        let meta = file.metadata().map_err(|_| unavailable())?;
        private(&meta, false)?;
        let node = identity(&meta);
        for chunk in bytes.chunks(65536) {
            live()?;
            let mut remaining = chunk;
            let mut interruptions = 0;
            while !remaining.is_empty() {
                live()?;
                match file.write(remaining) {
                    Ok(0) => return Err(unavailable()),
                    Ok(n) => remaining = &remaining[n..],
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted && interruptions < 8 => {
                        interruptions += 1
                    }
                    Err(_) => return Err(unavailable()),
                }
            }
        }
        file.sync_all().map_err(|_| unavailable())?;
        let mut written = WrittenFile {
            name: name.to_owned(),
            file,
            identity: node,
            size: bytes.len(),
            sha256: digest(bytes),
        };
        written.verify(self, live)?;
        Ok(written)
    }
}

/// The original open file prevents inode reuse while all members are copied.
struct WrittenFile {
    name: String,
    file: File,
    identity: (u64, u64),
    size: usize,
    sha256: String,
}
impl WrittenFile {
    fn verify(
        &mut self,
        directory: &Directory,
        live: &impl Fn() -> Result<(), ConnectError>,
    ) -> Result<(), ConnectError> {
        live()?;
        directory.current()?;
        verify_file(&mut self.file, self.identity, self.size, &self.sha256, live)?;
        let open_named = || {
            openat(
                &directory.file,
                leaf(&self.name)?,
                OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
                Mode::empty(),
            )
            .map(File::from)
            .map_err(|_| unavailable())
        };
        let mut named = open_named()?;
        verify_file(&mut named, self.identity, self.size, &self.sha256, live)?;
        // Check the directory entry again after reading it, while both original
        // and observed descriptors still prevent reuse of their identities.
        let current = open_named()?.metadata().map_err(|_| unavailable())?;
        private(&current, false)?;
        if identity(&current) != self.identity || current.len() != self.size as u64 {
            return Err(unavailable());
        }
        directory.current()?;
        live()
    }
}

fn verify_file(
    file: &mut File,
    node: (u64, u64),
    size: usize,
    sha256: &str,
    live: &impl Fn() -> Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    let before = file.metadata().map_err(|_| unavailable())?;
    private(&before, false)?;
    if identity(&before) != node || before.len() != size as u64 {
        return Err(unavailable());
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| unavailable())?;
    let bytes = bounded_read(file, size, live)?;
    let after = file.metadata().map_err(|_| unavailable())?;
    private(&after, false)?;
    if identity(&after) != node
        || after.len() != size as u64
        || bytes.len() != size
        || digest(&bytes) != sha256
    {
        return Err(unavailable());
    }
    live()
}

fn verify_members(
    copied: &mut [(&Directory, WrittenFile)],
    live: &impl Fn() -> Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    for (directory, member) in copied {
        member.verify(directory, live)?;
    }
    Ok(())
}

// Bounded payload, bounded interruptions, and a live fence between each read.
fn bounded_read(
    reader: &mut impl Read,
    max: usize,
    live: &impl Fn() -> Result<(), ConnectError>,
) -> Result<Vec<u8>, ConnectError> {
    let mut result = Vec::new();
    let mut buffer = [0; 65536];
    let mut interruptions = 0;
    loop {
        live()?;
        let capacity = buffer.len().min(max + 1 - result.len());
        match reader.read(&mut buffer[..capacity]) {
            Ok(0) => return Ok(result),
            Ok(n) => {
                result.extend_from_slice(&buffer[..n]);
                if result.len() > max {
                    return Err(unavailable());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted && interruptions < 8 => {
                interruptions += 1
            }
            Err(_) => return Err(unavailable()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn private_copy_is_exclusive_nofollow_and_detects_replaced_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = Directory::absolute(&temp.path().canonicalize().unwrap()).unwrap();
        let child = directory
            .create("input-0123456789abcdef0123456789abcdef")
            .unwrap();
        child.write("artifact.whl", b"inert", &|| Ok(())).unwrap();
        assert_eq!(child.read("artifact.whl", 5).unwrap(), b"inert");
        assert!(
            child
                .write("artifact.whl", b"replacement", &|| Ok(()))
                .is_err()
        );
        symlink("artifact.whl", child.path.join("link")).unwrap();
        assert!(child.read("link", 5).is_err());
        assert!(child.write("link", b"other", &|| Ok(())).is_err());
        std::fs::hard_link(child.path.join("artifact.whl"), child.path.join("hard")).unwrap();
        assert!(child.read("hard", 5).is_err());
        std::fs::rename(&child.path, temp.path().join("retired")).unwrap();
        std::fs::create_dir(&child.path).unwrap();
        std::fs::set_permissions(&child.path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(child.current().is_err());
    }
    fn replace_same_bytes(directory: &Directory, name: &str, bytes: &[u8]) {
        std::fs::rename(
            directory.path.join(name),
            directory.path.join("original-held"),
        )
        .unwrap();
        std::fs::write(directory.path.join(name), bytes).unwrap();
        std::fs::set_permissions(
            directory.path.join(name),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }

    #[test]
    fn completion_rejects_same_bytes_on_a_replaced_leaf() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = Directory::absolute(&temp.path().canonicalize().unwrap()).unwrap();
        let mut member = directory
            .write("artifact.whl", b"inert", &|| Ok(()))
            .unwrap();
        replace_same_bytes(&directory, "artifact.whl", b"inert");
        assert_eq!(directory.read("artifact.whl", 5).unwrap(), b"inert");
        assert_ne!(
            identity(&std::fs::metadata(directory.path.join("artifact.whl")).unwrap()),
            member.identity
        );
        assert_eq!(member.file.metadata().unwrap().nlink(), 1);
        assert!(member.verify(&directory, &|| Ok(())).is_err());
    }

    #[test]
    fn completion_rechecks_earlier_member_replaced_during_later_copy() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = Directory::absolute(&temp.path().canonicalize().unwrap()).unwrap();
        let first = directory
            .write("artifact.whl", b"inert", &|| Ok(()))
            .unwrap();
        let replaced = std::cell::Cell::new(false);
        let mut later = directory
            .write("package.json", b"{}", &|| {
                if !replaced.replace(true) {
                    replace_same_bytes(&directory, "artifact.whl", b"inert");
                }
                Ok(())
            })
            .unwrap();
        assert!(replaced.get());
        later.verify(&directory, &|| Ok(())).unwrap();
        let mut copied = vec![(&directory, first), (&directory, later)];
        assert!(verify_members(&mut copied, &|| Ok(())).is_err());
    }

    #[test]
    fn copy_read_checks_limit_interruption_budget_and_retirement() {
        assert!(bounded_read(&mut b"four".as_slice(), 3, &|| Ok(())).is_err());
        assert!(bounded_read(&mut b"four".as_slice(), 4, &|| Err(unavailable())).is_err());
        struct Interrupted(usize);
        impl Read for Interrupted {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                self.0 += 1;
                Err(std::io::ErrorKind::Interrupted.into())
            }
        }
        let mut reader = Interrupted(0);
        assert!(bounded_read(&mut reader, 4, &|| Ok(())).is_err());
        assert_eq!(reader.0, 9);
    }
}
