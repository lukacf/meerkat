//! Prospective physical exclusion and complete nonentry evidence only.
//!
//! This never settles native inputs, resurrects consent, or admits a successor.
//! Entered calls (including read routes) remain unresolved here even if every
//! tracked process is gone: a descendant may have escaped its parent's lease.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Node {
    device: u64,
    inode: u64,
}
impl Node {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            device: meta.dev(),
            inode: meta.ino(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fence {
    root: Node,
    directory: Node,
    lease: Node,
}

/// An identity retained in the canonical resource association, not authority.
/// Decoding it cannot establish native/issuer settlement or resource admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalSelection {
    artifact_directory: PathBuf,
    owner: String,
    lifetime_sha256: String,
    fence: Fence,
}

struct Directory {
    path: PathBuf,
    file: fs::File,
    node: Node,
}
fn private(meta: &fs::Metadata, directory: bool) -> Result<(), ConnectError> {
    if meta.uid() != nix::unistd::geteuid().as_raw()
        || mode(meta) != if directory { 0o700 } else { 0o600 }
        || if directory {
            !meta.is_dir()
        } else {
            !meta.is_file() || meta.nlink() != 1
        }
    {
        return Err(unavailable());
    }
    Ok(())
}
fn name(value: &str) -> Result<&str, ConnectError> {
    if value.is_empty() || value == "." || value == ".." || value.contains('/') {
        return Err(unavailable());
    }
    if value.contains('\0') {
        return Err(unavailable());
    }
    Ok(value)
}
impl Directory {
    fn open(path: &Path) -> Result<Self, ConnectError> {
        if path.canonicalize().map_err(|_| unavailable())? != path {
            return Err(unavailable());
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| unavailable())?;
        let meta = file.metadata().map_err(|_| unavailable())?;
        private(&meta, true)?;
        let value = Self {
            path: path.into(),
            file,
            node: Node::of(&meta),
        };
        value.current()?;
        Ok(value)
    }
    fn current(&self) -> Result<(), ConnectError> {
        let meta = fs::symlink_metadata(&self.path).map_err(|_| unavailable())?;
        private(&meta, true)?;
        if Node::of(&meta) != self.node
            || self.path.canonicalize().map_err(|_| unavailable())? != self.path
        {
            return Err(unavailable());
        }
        Ok(())
    }
    fn file(&self, key: &str, create: bool) -> Result<fs::File, ConnectError> {
        let key = name(key)?;
        // The retained directory descriptor, not a second pathname lookup,
        // selects every record. Ownership/mode/link checks follow openat.
        use nix::{
            fcntl::{OFlag, openat},
            sys::stat::Mode,
        };
        let flags = OFlag::O_NOFOLLOW
            | OFlag::O_NONBLOCK
            | OFlag::O_CLOEXEC
            | if create {
                OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL
            } else {
                OFlag::O_RDONLY
            };
        let fd = openat(&self.file, key, flags, Mode::from_bits_truncate(0o600))
            .map_err(|_| unavailable())?;
        let file = fs::File::from(fd);
        private(&file.metadata().map_err(|_| unavailable())?, false)?;
        self.current()?;
        Ok(file)
    }
    fn read(&self, key: &str) -> Result<Vec<u8>, ConnectError> {
        let file = self.file(key, false)?;
        if file.metadata().map_err(|_| unavailable())?.len() > MANIFEST_LIMIT {
            return Err(unavailable());
        }
        let mut raw = Vec::new();
        file.take(MANIFEST_LIMIT + 1)
            .read_to_end(&mut raw)
            .map_err(|_| unavailable())?;
        if raw.len() as u64 > MANIFEST_LIMIT {
            return Err(unavailable());
        }
        self.current()?;
        Ok(raw)
    }
    fn value(&self, key: &str) -> Result<Value, ConnectError> {
        serde_json::from_slice(&self.read(key)?).map_err(|_| unavailable())
    }
    fn write(&self, key: &str, raw: &[u8]) -> Result<(), ConnectError> {
        if raw.len() as u64 > MANIFEST_LIMIT {
            return Err(unavailable());
        }
        let mut file = self.file(key, true)?;
        file.write_all(raw)
            .and_then(|_| file.sync_all())
            .map_err(|_| unavailable())?;
        self.file.sync_all().map_err(|_| unavailable())?;
        if self.read(key)? != raw {
            return Err(unavailable());
        }
        Ok(())
    }
    fn exact(&self, key: &str, value: &Value) -> Result<(), ConnectError> {
        let raw = serde_json::to_vec(value).map_err(|_| unavailable())?;
        match self.file(key, false) {
            Ok(_) => {
                if self.read(key)? != raw {
                    return Err(unavailable());
                }
                self.file(key, false)?
                    .sync_all()
                    .map_err(|_| unavailable())?;
                self.file.sync_all().map_err(|_| unavailable())?;
                Ok(())
            }
            Err(_) => self.write(key, &raw),
        }
    }
    fn exists(&self, key: &str) -> Result<bool, ConnectError> {
        use nix::{errno::Errno, fcntl::AtFlags, sys::stat::fstatat};
        match fstatat(&self.file, name(key)?, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(Errno::ENOENT) => Ok(false),
            Err(_) => Err(unavailable()),
        }
    }
    fn names(&self) -> Result<BTreeSet<String>, ConnectError> {
        use nix::{dir::Dir, fcntl::OFlag, sys::stat::Mode};
        // An independent directory descriptor opened relative to the retained
        // one has its own iteration position and never follows a replacement.
        let mut dir = Dir::openat(
            &self.file,
            ".",
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| unavailable())?;
        let mut names = BTreeSet::new();
        for entry in dir.iter() {
            let entry = entry.map_err(|_| unavailable())?;
            let key = entry.file_name().to_str().map_err(|_| unavailable())?;
            if key != "." && key != ".." {
                names.insert(key.to_owned());
            }
            if names.len() > 8192 {
                return Err(unavailable());
            }
        }
        self.current()?;
        Ok(names)
    }
}

/// Immutable bytes selected once for both validation and returned pins.
struct Snapshot {
    files: BTreeMap<String, Vec<u8>>,
}
impl Snapshot {
    fn capture(directory: &Directory) -> Result<Self, ConnectError> {
        let names = directory.names()?;
        let mut files = BTreeMap::new();
        let mut total = 0usize;
        for key in names {
            let raw = directory.read(&key)?;
            total = total.checked_add(raw.len()).ok_or_else(unavailable)?;
            if total > MANIFEST_LIMIT as usize {
                return Err(unavailable());
            }
            files.insert(key, raw);
        }
        let value = Self { files };
        value.recheck(directory)?;
        Ok(value)
    }
    fn read(&self, key: &str) -> Result<Vec<u8>, ConnectError> {
        self.files.get(key).cloned().ok_or_else(unavailable)
    }
    fn value(&self, key: &str) -> Result<Value, ConnectError> {
        serde_json::from_slice(&self.read(key)?).map_err(|_| unavailable())
    }
    fn names(&self) -> BTreeSet<String> {
        self.files.keys().cloned().collect()
    }
    fn recheck(&self, directory: &Directory) -> Result<(), ConnectError> {
        if directory.names()? != self.names() {
            return Err(unavailable());
        }
        for (key, raw) in &self.files {
            if directory.read(key)? != *raw {
                return Err(unavailable());
            }
        }
        if directory.names()? != self.names() {
            return Err(unavailable());
        }
        directory.current()
    }
}

pub(super) fn create(directory: &Path, owner: &str) -> Result<Fence, ConnectError> {
    let root = Directory::open(directory)?;
    let path = directory.join(format!("shared-physical-{owner}"));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(|_| unavailable())?;
    root.file.sync_all().map_err(|_| unavailable())?;
    let selected = Directory::open(&path)?;
    selected.write("lease", b"")?;
    Ok(Fence {
        root: root.node.clone(),
        directory: selected.node.clone(),
        lease: Node::of(
            &selected
                .file("lease", false)?
                .metadata()
                .map_err(|_| unavailable())?,
        ),
    })
}

pub(super) struct PhysicalLease {
    selection: PhysicalSelection,
    root: Directory,
    directory: Directory,
    _lease: fs::File,
}
impl PhysicalLease {
    pub(super) fn join(path: &Path, expected: &str) -> Result<Arc<Self>, ConnectError> {
        let raw = lifetime::read(path)?;
        let value: lifetime::PhysicalLifetime =
            serde_json::from_slice(&raw).map_err(|_| unavailable())?;
        if digest(&raw) != expected
            || value.schema_version != 2
            || path
                != value
                    .artifact_directory
                    .join(format!("shared-lifetime-{}.json", value.owner))
        {
            return Err(unavailable());
        }
        let selection = PhysicalSelection {
            artifact_directory: value.artifact_directory,
            owner: value.owner,
            lifetime_sha256: expected.into(),
            fence: value.fence.ok_or_else(unavailable)?,
        };
        let (root, directory, lease) = selection.open()?;
        lease.try_lock_shared().map_err(|_| unavailable())?;
        if directory.exists("sealed.json")? {
            return Err(unavailable());
        }
        let value = Arc::new(Self {
            selection,
            root,
            directory,
            _lease: lease,
        });
        value.current()?;
        Ok(value)
    }
    fn current(&self) -> Result<(), ConnectError> {
        self.selection
            .current(&self.root, &self.directory, &self._lease)?;
        if self.directory.exists("sealed.json")? {
            return Err(unavailable());
        }
        Ok(())
    }
    #[cfg(any(test, feature = "console"))]
    pub(super) fn selection(&self) -> PhysicalSelection {
        self.selection.clone()
    }
    #[cfg(any(test, feature = "console"))]
    pub(super) fn member(
        &self,
        peer: usize,
        member: &meerkat_core::MobMemberBinding,
        session: &str,
    ) -> Result<(), ConnectError> {
        self.current()?;
        self.directory.exact(
            &format!("member-{peer}.json"),
            &json!({"peer":peer,"member":member,"session":session,
            "lifetime_sha256":self.selection.lifetime_sha256}),
        )
    }
    pub(super) fn intent(&self, peer: usize, request: &Value) -> Result<Value, ConnectError> {
        self.current()?;
        let member_name = format!("member-{peer}.json");
        let member_sha256 = if self.directory.exists(&member_name)? {
            let member = self.directory.value(&member_name)?;
            if member["session"] != request["native_request"]["session_id"] {
                return Err(unavailable());
            }
            Some(digest(&self.directory.read(&member_name)?))
        } else {
            // The public non-Console shared composer has no attempt-registration
            // callback. Preserve its ordinary authorized dispatch, but record
            // the missing provenance before send. Later registration cannot
            // remove this lifetime's cold-refusal evidence or invent an owner.
            self.directory.exact(
                &format!("unresolved-member-{peer}.json"),
                &json!({
                "peer":peer,"scope":"member_provenance_unavailable",
                "lifetime_sha256":self.selection.lifetime_sha256}),
            )?;
            None
        };
        let id = digest(uuid::Uuid::new_v4().as_bytes());
        let raw = serde_json::to_vec(&json!({"id":id,"peer":peer,"request":request,
            "member_sha256":member_sha256,"lifetime_sha256":self.selection.lifetime_sha256}))
        .map_err(|_| unavailable())?;
        self.directory
            .write(&format!("operation-{id}.json"), &raw)?;
        Ok(json!({"id":id,"sha256":digest(&raw)}))
    }
    pub(super) fn observed(&self, entry: &str, decision: &str) -> Result<(), ConnectError> {
        self.current()?;
        let raw = self
            .directory
            .read(&format!("census-{entry}.decision.json"))?;
        let value: Value = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
        if value["entry"] != entry || value["decision"] != decision {
            return Err(unavailable());
        }
        self.directory.exact(
            &format!("observed-{entry}.json"),
            &json!({"entry":entry,"decision_sha256":digest(&raw)}),
        )
    }
}

/// Exclusive physical custody plus a complete nonentry census. Not a success
/// disposition for entered work, no native terminality or effect permission.
#[must_use = "retain exclusive physical custody while composing other owner evidence"]
pub struct ColdPhysicalNoEntry {
    selection: PhysicalSelection,
    _lease: fs::File,
    census: Value,
}
impl ColdPhysicalNoEntry {
    pub fn selection(&self) -> &PhysicalSelection {
        &self.selection
    }
    pub fn census(&self) -> &Value {
        &self.census
    }
    /// Recheck the same held exclusion and immutable census. No new lease,
    /// admission reopening, native observation or replacement is performed.
    pub fn recheck(&self) -> Result<(), ConnectError> {
        let root = Directory::open(&self.selection.artifact_directory)?;
        let directory = Directory::open(
            &root
                .path
                .join(format!("shared-physical-{}", self.selection.owner)),
        )?;
        self.selection.current(&root, &directory, &self._lease)?;
        let current = self.selection.no_entry(&directory, || {})?;
        if current != self.census {
            return Err(unavailable());
        }
        self.selection.current(&root, &directory, &self._lease)
    }
}
impl PhysicalSelection {
    fn open(&self) -> Result<(Directory, Directory, fs::File), ConnectError> {
        if !valid_digest(&self.owner) || !valid_digest(&self.lifetime_sha256) {
            return Err(unavailable());
        }
        let root = Directory::open(&self.artifact_directory)?;
        let directory =
            Directory::open(&root.path.join(format!("shared-physical-{}", self.owner)))?;
        let lease = directory.file("lease", false)?;
        self.current(&root, &directory, &lease)?;
        Ok((root, directory, lease))
    }
    fn current(
        &self,
        root: &Directory,
        directory: &Directory,
        lease: &fs::File,
    ) -> Result<(), ConnectError> {
        root.current()?;
        directory.current()?;
        let lease_meta = lease.metadata().map_err(|_| unavailable())?;
        private(&lease_meta, false)?;
        if root.node != self.fence.root
            || directory.node != self.fence.directory
            || Node::of(&lease_meta) != self.fence.lease
            || Node::of(
                &directory
                    .file("lease", false)?
                    .metadata()
                    .map_err(|_| unavailable())?,
            ) != self.fence.lease
            || lease_meta.len() != 0
        {
            return Err(unavailable());
        }
        let raw = root.read(&format!("shared-lifetime-{}.json", self.owner))?;
        if digest(&raw) != self.lifetime_sha256 {
            return Err(unavailable());
        }
        let value: lifetime::PhysicalLifetime =
            serde_json::from_slice(&raw).map_err(|_| unavailable())?;
        if value.schema_version != 2
            || value.owner != self.owner
            || value.artifact_directory != root.path
            || value.fence.as_ref() != Some(&self.fence)
            || value.preparer.len() != 1
            || !value.preparer.contains_key("prepare.stdout")
        {
            return Err(unavailable());
        }
        for (name, hash) in value.preparer {
            if digest(&root.read(&name)?) != hash {
                return Err(unavailable());
            }
        }
        Ok(())
    }
    /// Does not wait, signal processes, query native state, or launch work.
    /// A failed census leaves admission sealed; repetition observes the same
    /// immutable files and never reopens the old physical lifetime.
    pub fn seal_no_entry(&self) -> Result<ColdPhysicalNoEntry, ConnectError> {
        let (root, directory, lease) = self.open()?;
        lease.try_lock().map_err(|_| unavailable())?;
        self.current(&root, &directory, &lease)?;
        directory.exact(
            "sealed.json",
            &json!({"schema_version":1,"scope":"closed_physical_admission",
            "lifetime_sha256":self.lifetime_sha256}),
        )?;
        let census = self.no_entry(&directory, || {})?;
        self.current(&root, &directory, &lease)?;
        Ok(ColdPhysicalNoEntry {
            selection: self.clone(),
            _lease: lease,
            census,
        })
    }
    fn no_entry(
        &self,
        dir: &Directory,
        after_capture: impl FnOnce(),
    ) -> Result<Value, ConnectError> {
        let snapshot = Snapshot::capture(dir)?;
        let census = Directory::open(
            &self
                .artifact_directory
                .join(format!("shared-entries-{}", self.owner)),
        )?;
        let census_snapshot = Snapshot::capture(&census)?;
        // Private deterministic test seam only; production supplies an empty
        // closure. No host callback can influence this disk-only reader.
        after_capture();
        let mut expected = BTreeSet::from(
            [
                "lease",
                "sealed.json",
                "startup-begin.json",
                "startup.json",
                "census-owner.json",
                "census-directory.json",
            ]
            .map(str::to_owned),
        );
        let start = snapshot.value("startup-begin.json")?;
        let ready = snapshot.value("startup.json")?;
        let owner = snapshot.value("census-owner.json")?;
        if start
            != json!({"schema_version":1,"scope":"fresh_scoped_workspace_initialization",
                "lifetime_sha256":self.lifetime_sha256})
            || ready["startup_sha256"] != digest(&snapshot.read("startup-begin.json")?)
            || ready["lifetime_sha256"] != self.lifetime_sha256
            || ready["scope"] != "shared_physical_ready"
            || ready.as_object().is_none_or(|v| v.len() != 5)
            || owner.as_object().is_none_or(|v| v.len() != 4)
            || owner["controller"]
                .as_str()
                .is_none_or(|v| !valid_digest(v))
            || ready["controller"] != owner["controller"]
            || owner["lifetime_sha256"] != self.lifetime_sha256
            || owner["scope"] != "shared_controller_entry_census"
            || owner["schema_version"] != 1
        {
            return Err(unavailable());
        }
        let initial = ready["peers"]
            .as_u64()
            .filter(|n| (1..=8).contains(n))
            .ok_or_else(unavailable)?;
        // The initial roster belongs to the exact pre-spawn selection. Later
        // peer renewal may extend it, but census edits cannot shrink its base.
        let lifetime_raw = Directory::open(&self.artifact_directory)?
            .read(&format!("shared-lifetime-{}.json", self.owner))?;
        if digest(&lifetime_raw) != self.lifetime_sha256 {
            return Err(unavailable());
        }
        let selected: Value = serde_json::from_slice(&lifetime_raw).map_err(|_| unavailable())?;
        if selected["peers"].as_u64() != Some(initial) {
            return Err(unavailable());
        }
        let actual = snapshot.names();
        let count = actual
            .iter()
            .filter(|key| key.starts_with("peer-") && key.ends_with(".json"))
            .count() as u64;
        if !(initial..=8).contains(&count) {
            return Err(unavailable());
        }
        for peer in 0..count {
            let key = format!("peer-{peer}.json");
            let row = snapshot.value(&key)?;
            if row["peer"] != peer
                || row["lifetime_sha256"] != self.lifetime_sha256
                || row.as_object().is_none_or(|v| v.len() != 5)
                || row["path"].as_str().is_none()
                || row["device"].as_u64().is_none()
                || row["inode"].as_u64().is_none()
            {
                return Err(unavailable());
            }
            expected.insert(key);
        }
        let mut operations = Vec::new();
        let mut members = Vec::new();
        for key in &actual {
            if let Some(id) = key
                .strip_prefix("operation-")
                .and_then(|s| s.strip_suffix(".json"))
            {
                if !valid_digest(id) {
                    return Err(unavailable());
                }
                let raw = snapshot.read(key)?;
                let op: Value = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
                let peer = op["peer"]
                    .as_u64()
                    .filter(|v| *v < count)
                    .ok_or_else(unavailable)?;
                let member_key = format!("member-{peer}.json");
                let member_raw = snapshot.read(&member_key)?;
                let member: Value =
                    serde_json::from_slice(&member_raw).map_err(|_| unavailable())?;
                let accepted_key = format!("accepted-{id}.json");
                let accepted = snapshot.value(&accepted_key)?;
                let token = accepted["entry"]
                    .as_str()
                    .filter(|v| valid_digest(v))
                    .ok_or_else(unavailable)?;
                let req_key = format!("census-{token}.request.json");
                let decision_key = format!("census-{token}.decision.json");
                let observed_key = format!("observed-{token}.json");
                let req_raw = snapshot.read(&req_key)?;
                let mut req: Value = serde_json::from_slice(&req_raw).map_err(|_| unavailable())?;
                let intent = req["request"]
                    .as_object_mut()
                    .ok_or_else(unavailable)?
                    .remove("physical_intent")
                    .ok_or_else(unavailable)?;
                let decision = snapshot.value(&decision_key)?;
                let observed = snapshot.value(&observed_key)?;
                if op["id"] != id
                    || op["lifetime_sha256"] != self.lifetime_sha256
                    || op["member_sha256"] != digest(&member_raw)
                    || member["peer"] != peer
                    || member["session"] != op["request"]["native_request"]["session_id"]
                    || op["request"] != req["request"]
                    || req["entry"] != token
                    || intent != json!({"id":id,"sha256":digest(&raw)})
                    || accepted
                        != json!({"id":id,"entry":token,"operation_sha256":digest(&raw),"request_sha256":digest(&req_raw)})
                    || decision
                        != json!({"entry":token,"request_sha256":digest(&req_raw),"decision":"not_entered"})
                    || observed
                        != json!({"entry":token,"decision_sha256":digest(&snapshot.read(&decision_key)?)})
                {
                    return Err(unavailable());
                }
                for file in [
                    key.clone(),
                    accepted_key,
                    member_key,
                    req_key,
                    decision_key,
                    observed_key,
                ] {
                    expected.insert(file);
                }
                operations.push(
                    json!({"id":id,"entry":token,"operation_sha256":digest(&raw),
                    "request":op["request"].clone()}),
                );
            } else if key.starts_with("member-") && key.ends_with(".json") {
                let row = snapshot.value(key)?;
                let peer = row["peer"]
                    .as_u64()
                    .filter(|v| *v < count)
                    .ok_or_else(unavailable)?;
                if *key != format!("member-{peer}.json")
                    || row["session"].as_str().is_none()
                    || row.as_object().is_none_or(|v| v.len() != 4)
                    || serde_json::from_value::<meerkat_core::MobMemberBinding>(
                        row["member"].clone(),
                    )
                    .is_err()
                    || row["lifetime_sha256"] != self.lifetime_sha256
                {
                    return Err(unavailable());
                }
                members.push(row);
                expected.insert(key.clone());
            } else if key.starts_with("bridge-") && key.ends_with(".json") {
                let row = snapshot.value(key)?;
                let peer = row["peer"]
                    .as_u64()
                    .filter(|v| *v < count)
                    .ok_or_else(unavailable)?;
                let bridge_id = key
                    .strip_prefix("bridge-")
                    .and_then(|v| v.strip_suffix(".json"))
                    .ok_or_else(unavailable)?;
                if bridge_id.len() != 32
                    || !bridge_id
                        .bytes()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                    || row.as_object().is_none_or(|v| v.len() != 3)
                    || row["lifetime_sha256"] != self.lifetime_sha256
                    || row["peer_sha256"] != digest(&snapshot.read(&format!("peer-{peer}.json"))?)
                {
                    return Err(unavailable());
                }
                expected.insert(key.clone());
            }
        }
        if actual != expected {
            return Err(unavailable());
        }
        let census_node: Node = serde_json::from_value(snapshot.value("census-directory.json")?)
            .map_err(|_| unavailable())?;
        if census.node != census_node {
            return Err(unavailable());
        }
        let mut census_names = BTreeSet::new();
        for key in &actual {
            if let Some(name) = key.strip_prefix("census-") {
                if name == "directory.json" {
                    continue;
                }
                if census_snapshot.read(name)? != snapshot.read(key)? {
                    return Err(unavailable());
                }
                census_names.insert(name.to_owned());
            }
        }
        if census_snapshot.names() != census_names {
            return Err(unavailable());
        }
        census_snapshot.recheck(&census)?;
        snapshot.recheck(dir)?;
        // Pin every retained file; no missing/orphan row can disappear from a
        // partial successful report. Actual process exclusion is held separately.
        let mut files = serde_json::Map::new();
        let mut total = 0usize;
        for key in actual {
            let raw = snapshot.read(&key)?;
            total = total.checked_add(raw.len()).ok_or_else(unavailable)?;
            if total > MANIFEST_LIMIT as usize {
                return Err(unavailable());
            }
            files.insert(key, json!(digest(&raw)));
        }
        Ok(
            json!({"scope":"cold_shared_physical_no_entry_only","lifetime_sha256":self.lifetime_sha256,
            "files":files,"members":members,"operations":operations}),
        )
    }
}

#[cfg(test)]
mod tests;
