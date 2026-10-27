//! Local, inert artifact preparation for Linux and macOS.
//!
//! The caller owns filesystem and effect authority. These handles establish
//! checked bytes, never permission to install or execute them. Prepared files
//! are private, read-only materializations, not a sandbox against their owner.

use crate::{
    ArtifactDependencies, ComponentSource, ContentDigest, Entrypoint, Id, ValidatedManifest,
    canonical_json,
};
use rustix::fs::{self, Mode, OFlags, RenameFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

mod zip32;

const VERSION: u16 = 1;
const DIRECTORY_MODE: u32 = 0o500;
const FILE_MODE: u32 = 0o400;
const EXECUTABLE_MODE: u32 = 0o500;
const RECEIPT_LIMIT: u64 = 16 * 1024 * 1024;

type Result<T> = std::result::Result<T, PreparationError>;

#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    #[error("artifact I/O: {0}")]
    Io(#[from] io::Error),
    #[error("artifact archive: {0}")]
    Archive(#[from] zip::result::ZipError),
    #[error("artifact evidence: {0}")]
    Evidence(#[from] serde_json::Error),
    #[error("invalid artifact: {0}")]
    Invalid(&'static str),
    #[error("unsupported artifact: {0}")]
    Unsupported(&'static str),
    #[error("artifact exceeds limit: {0}")]
    Limit(&'static str),
    #[error("artifact bytes do not match the selected digest")]
    DigestMismatch,
    #[error("selected lock requires ecosystem dependency realization")]
    DependencyRealizationRequired,
    #[error("artifact was published at {locator}; final durability acknowledgment failed")]
    PublishedButUnconfirmed { locator: String },
}

/// A strict ZIP32 profile, or one file placed at the selected entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalArtifactLayout {
    SingleFile { executable: bool },
    Zip,
}

/// Limits bound actual bytes, including metadata, not only archive declarations.
#[derive(Debug, Clone)]
pub struct PreparationLimits {
    pub maximum_raw_bytes: u64,
    pub maximum_file_bytes: u64,
    pub maximum_expanded_bytes: u64,
    pub maximum_dependency_bytes: u64,
    pub maximum_entries: usize,
    pub maximum_path_bytes: usize,
    pub maximum_archive_metadata_bytes: u64,
}

impl Default for PreparationLimits {
    fn default() -> Self {
        Self {
            maximum_raw_bytes: 256 * 1024 * 1024,
            maximum_file_bytes: 64 * 1024 * 1024,
            maximum_expanded_bytes: 512 * 1024 * 1024,
            maximum_dependency_bytes: 512 * 1024 * 1024,
            maximum_entries: 10_000,
            maximum_path_bytes: 240,
            maximum_archive_metadata_bytes: 4 * 1024 * 1024,
        }
    }
}

impl PreparationLimits {
    fn validate(&self) -> Result<()> {
        if self.maximum_entries == 0
            || self.maximum_entries > 65_534
            || self.maximum_path_bytes == 0
            || self.maximum_path_bytes > 4096
            || self.maximum_raw_bytes == u64::MAX
            || self.maximum_file_bytes == u64::MAX
            || self.maximum_expanded_bytes == u64::MAX
            || self.maximum_dependency_bytes == u64::MAX
        {
            return Err(PreparationError::Invalid("invalid preparation limits"));
        }
        Ok(())
    }
}

pub struct LocalArtifactInputs {
    pub artifact: File,
    pub pinned_dependencies: BTreeMap<Id, File>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEvidence {
    size: u64,
    digest: ContentDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum InventoryValue {
    Directory,
    File {
        size: u64,
        digest: ContentDigest,
        executable: bool,
    },
}

type Inventory = BTreeMap<String, InventoryValue>;

/// A bounded observation of ZIP members, not a prepared or executable artifact.
#[derive(Debug, Clone, Serialize)]
pub struct InspectedZipEntry {
    pub path: String,
    pub byte_length: u64,
    pub directory: bool,
}

/// The selected bytes and sorted inventory of an entirely validated ZIP32 file.
/// Every member passes the same profile, length and CRC checks as preparation.
#[derive(Debug)]
pub struct InspectedZipSelection {
    /// Number of explicit ZIP members. Inventory also contains implied parents.
    pub archive_entry_count: usize,
    pub inventory: Vec<InspectedZipEntry>,
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Inspect an immutable byte snapshot without writing or executing any member.
/// Paths use the archive's canonical relative spelling, without a `./` prefix.
pub fn inspect_zip_selection(
    raw: &[u8],
    paths: &BTreeSet<String>,
    limits: &PreparationLimits,
    maximum_selected_file_bytes: u64,
    maximum_selected_bytes: u64,
) -> Result<InspectedZipSelection> {
    limits.validate()?;
    if paths.len() > limits.maximum_entries {
        return Err(PreparationError::Limit("selected ZIP entries"));
    }
    for path in paths {
        portable_path(path, limits.maximum_path_bytes)?;
    }
    let mut files = paths
        .iter()
        .map(|path| (path.clone(), Vec::new()))
        .collect();
    let (inventory, archive_entry_count) = zip32::scan_zip(
        &mut io::Cursor::new(raw),
        None,
        limits,
        &mut files,
        maximum_selected_file_bytes,
        maximum_selected_bytes,
    )?;
    Ok(InspectedZipSelection {
        archive_entry_count,
        inventory: inventory
            .into_iter()
            .map(|(path, entry)| match entry {
                InventoryValue::Directory => InspectedZipEntry {
                    path,
                    byte_length: 0,
                    directory: true,
                },
                InventoryValue::File { size, .. } => InspectedZipEntry {
                    path,
                    byte_length: size,
                    directory: false,
                },
            })
            .collect(),
        files,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceBody {
    version: u16,
    manifest_digest: ContentDigest,
    component: Id,
    layout: LocalArtifactLayout,
    raw: FileEvidence,
    dependencies: BTreeMap<Id, FileEvidence>,
    inventory: Inventory,
    locator: String,
}

/// Inert evidence. Even self-consistent JSON cannot construct a verified handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedArtifactEvidence {
    body: EvidenceBody,
    digest: ContentDigest,
}

impl PreparedArtifactEvidence {
    pub fn digest(&self) -> &ContentDigest {
        &self.digest
    }
    pub fn manifest_digest(&self) -> &ContentDigest {
        &self.body.manifest_digest
    }
    pub fn component(&self) -> &Id {
        &self.body.component
    }
    pub fn locator(&self) -> &str {
        &self.body.locator
    }
    pub fn layout(&self) -> &LocalArtifactLayout {
        &self.body.layout
    }
    fn new(body: EvidenceBody) -> Result<Self> {
        let digest = ContentDigest::of_bytes(&canonical_json(&body)?);
        Ok(Self { body, digest })
    }
}

/// The returned handle owns only byte-verification facts, not activation state.
pub struct PreparedLocalArtifact {
    tree: File,
    evidence: PreparedArtifactEvidence,
    entrypoint: Entrypoint,
}

impl PreparedLocalArtifact {
    pub fn evidence(&self) -> &PreparedArtifactEvidence {
        &self.evidence
    }
    pub fn entrypoint(&self) -> &Entrypoint {
        &self.entrypoint
    }

    /// Read an inventoried file with a fresh content/type/mode check. No ambient
    /// pathname or writable descriptor is exposed to the caller.
    pub fn read(&self, relative: &str, maximum_bytes: u64) -> Result<Vec<u8>> {
        let Some(InventoryValue::File {
            size,
            digest,
            executable,
        }) = self.evidence.body.inventory.get(relative)
        else {
            return Err(PreparationError::Invalid("file is not inventoried"));
        };
        if *size > maximum_bytes {
            return Err(PreparationError::Limit("read"));
        }
        let mut file = open_relative(&self.tree, relative)?;
        check_regular(&file, Some(file_mode(*executable)))?;
        let mut bytes = Vec::new();
        let observed = hash_copy(&mut file, &mut bytes, *size)?;
        if observed.size != *size || &observed.digest != digest {
            return Err(PreparationError::DigestMismatch);
        }
        Ok(bytes)
    }
}

pub struct LocalArtifactPreparer {
    root: File,
}

#[derive(Clone, Copy)]
enum Point {
    Frozen,
    Materialized,
    BeforePublish,
    AfterPublish,
}

impl LocalArtifactPreparer {
    /// The supplied root is a dedicated directory for one preparation attempt,
    /// recorded by the host's effect owner before this operation. `incomplete`
    /// and `prepared` are fixed child locations, known before any write. Distinct
    /// attempts require distinct roots. Other writers with the same filesystem
    /// authority are outside this library's isolation.
    pub fn new(root: cap_std::fs::Dir) -> Self {
        Self {
            root: root.into_std_file(),
        }
    }

    pub fn prepare(
        &self,
        manifest: &ValidatedManifest,
        component: &Id,
        layout: LocalArtifactLayout,
        inputs: LocalArtifactInputs,
        limits: &PreparationLimits,
    ) -> Result<PreparedLocalArtifact> {
        self.prepare_with(manifest, component, layout, inputs, limits, &mut |_| Ok(()))
    }

    fn prepare_with(
        &self,
        manifest: &ValidatedManifest,
        component: &Id,
        layout: LocalArtifactLayout,
        mut inputs: LocalArtifactInputs,
        limits: &PreparationLimits,
        hook: &mut dyn FnMut(Point) -> Result<()>,
    ) -> Result<PreparedLocalArtifact> {
        limits.validate()?;
        let selection = selection(manifest, component, limits)?;
        if inputs
            .pinned_dependencies
            .keys()
            .ne(selection.dependencies.keys())
        {
            return Err(PreparationError::Invalid(
                "dependency input set differs from selection",
            ));
        }
        let incomplete = "incomplete";
        let locator = "prepared".to_string();
        let directory = create_directory(&self.root, incomplete)?;
        let raw = freeze(
            &directory,
            "payload",
            &mut inputs.artifact,
            selection.digest,
            limits.maximum_raw_bytes,
        )?;
        let dependencies_dir = create_directory(&directory, "dependencies")?;
        let mut remaining = limits.maximum_dependency_bytes;
        let mut dependencies = BTreeMap::new();
        for (id, pin) in &selection.dependencies {
            let mut source = inputs
                .pinned_dependencies
                .remove(id)
                .ok_or(PreparationError::Invalid("missing dependency"))?;
            let evidence = freeze(
                &dependencies_dir,
                &dependency_name(id),
                &mut source,
                pin,
                remaining.min(limits.maximum_raw_bytes),
            )?;
            remaining -= evidence.size;
            dependencies.insert(id.clone(), evidence);
        }
        hook(Point::Frozen)?;
        let tree = create_directory(&directory, "tree")?;
        let mut frozen = open_file(&directory, "payload")?;
        let inventory = materialize(
            &mut frozen,
            &layout,
            selection.entrypoint,
            Some(&tree),
            limits,
        )?;
        hook(Point::Materialized)?;
        seal_tree(&tree, &inventory)?;
        seal_directory(&dependencies_dir)?;
        let evidence = PreparedArtifactEvidence::new(EvidenceBody {
            version: VERSION,
            manifest_digest: manifest.digest().clone(),
            component: component.clone(),
            layout,
            raw,
            dependencies,
            inventory,
            locator: locator.clone(),
        })?;
        let bytes = canonical_json(&evidence)?;
        if bytes.len() as u64 > RECEIPT_LIMIT {
            return Err(PreparationError::Limit("evidence"));
        }
        let mut receipt = create_file(&directory, "evidence.json")?;
        receipt.write_all(&bytes)?;
        seal_file(&receipt, false)?;
        seal_directory(&directory)?;
        // Verify the exact complete on-disk image before making it discoverable.
        let prepared = verify_directory(&directory, manifest, &evidence, limits)?;
        hook(Point::BeforePublish)?;
        fs::renameat_with(
            &self.root,
            incomplete,
            &self.root,
            locator.as_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)?;
        if hook(Point::AfterPublish).is_err() || sync_directory(&self.root).is_err() {
            return Err(PreparationError::PublishedButUnconfirmed { locator });
        }
        Ok(prepared)
    }

    /// Recovery re-derives the expected tree from pinned raw bytes, then checks
    /// the full stored tree. Edited evidence and an edited inventory cannot
    /// certify changed content, even if their hashes are internally consistent.
    /// The expected component/layout must come from the original host intent,
    /// independently of stored evidence; evidence cannot select its interpretation.
    pub fn reopen_verified(
        &self,
        manifest: &ValidatedManifest,
        component: &Id,
        layout: &LocalArtifactLayout,
        evidence: &PreparedArtifactEvidence,
        limits: &PreparationLimits,
    ) -> Result<PreparedLocalArtifact> {
        limits.validate()?;
        if evidence.component() != component || evidence.layout() != layout {
            return Err(PreparationError::Invalid(
                "evidence differs from original preparation intent",
            ));
        }
        validate_locator(evidence.locator())?;
        let directory = open_directory(&self.root, evidence.locator())?;
        verify_directory(&directory, manifest, evidence, limits)
    }
}

struct Selection<'a> {
    digest: &'a ContentDigest,
    entrypoint: &'a Entrypoint,
    dependencies: BTreeMap<Id, ContentDigest>,
}

fn selection<'a>(
    manifest: &'a ValidatedManifest,
    id: &Id,
    limits: &PreparationLimits,
) -> Result<Selection<'a>> {
    let component = manifest
        .manifest()
        .components
        .iter()
        .find(|c| &c.id == id)
        .ok_or(PreparationError::Invalid("component is not selected"))?;
    let ComponentSource::Artifact {
        content_digest,
        entrypoint,
        dependencies,
        ..
    } = &component.source
    else {
        return Err(PreparationError::Unsupported(
            "component is not a local artifact",
        ));
    };
    if entrypoint.python_console_script.is_some() {
        return Err(PreparationError::Unsupported(
            "console script selection requires the selected Python installer",
        ));
    }
    portable_path(&entrypoint.executable, limits.maximum_path_bytes)?;
    let dependencies = match dependencies {
        ArtifactDependencies::SelfContained => BTreeMap::new(),
        ArtifactDependencies::PinnedClosure { artifacts } => {
            if artifacts.len() > limits.maximum_entries {
                return Err(PreparationError::Limit("dependencies"));
            }
            artifacts.clone()
        }
        ArtifactDependencies::Locked { .. } => {
            return Err(PreparationError::DependencyRealizationRequired);
        }
    };
    Ok(Selection {
        digest: content_digest,
        entrypoint,
        dependencies,
    })
}

fn verify_directory(
    directory: &File,
    manifest: &ValidatedManifest,
    evidence: &PreparedArtifactEvidence,
    limits: &PreparationLimits,
) -> Result<PreparedLocalArtifact> {
    // Evidence is inert caller data. Bound it before cloning/canonicalizing it.
    if evidence.body.inventory.len() > limits.maximum_entries
        || evidence.body.dependencies.len() > limits.maximum_entries
        || evidence
            .body
            .inventory
            .keys()
            .any(|path| path.len() > limits.maximum_path_bytes)
        || evidence
            .body
            .inventory
            .keys()
            .try_fold(0_u64, |sum, path| sum.checked_add(path.len() as u64 + 256))
            .is_none_or(|size| size > RECEIPT_LIMIT)
    {
        return Err(PreparationError::Limit("evidence"));
    }
    if evidence.body.version != VERSION
        || evidence.manifest_digest() != manifest.digest()
        || PreparedArtifactEvidence::new(evidence.body.clone())?.digest != evidence.digest
    {
        return Err(PreparationError::Invalid("evidence binding mismatch"));
    }
    validate_locator(evidence.locator())?;
    let selected = selection(manifest, evidence.component(), limits)?;
    check_directory(directory, DIRECTORY_MODE)?;
    check_names(
        directory,
        &["payload", "tree", "dependencies", "evidence.json"],
    )?;
    let mut receipt = open_file(directory, "evidence.json")?;
    check_regular(&receipt, Some(FILE_MODE))?;
    let mut bytes = Vec::new();
    hash_copy(&mut receipt, &mut bytes, RECEIPT_LIMIT)?;
    // The caller's receipt is an expected observation, never replaced by disk JSON.
    if canonical_json(evidence)? != bytes {
        return Err(PreparationError::Invalid("stored evidence differs"));
    }
    let mut raw = open_file(directory, "payload")?;
    check_regular(&raw, Some(FILE_MODE))?;
    let observed = hash_copy(&mut raw, &mut io::sink(), limits.maximum_raw_bytes)?;
    if &observed.digest != selected.digest || observed != evidence.body.raw {
        return Err(PreparationError::DigestMismatch);
    }
    let dependencies = open_directory(directory, "dependencies")?;
    check_directory(&dependencies, DIRECTORY_MODE)?;
    if evidence
        .body
        .dependencies
        .keys()
        .ne(selected.dependencies.keys())
    {
        return Err(PreparationError::Invalid("dependency evidence set differs"));
    }
    let expected_names = selected
        .dependencies
        .keys()
        .map(dependency_name)
        .collect::<Vec<_>>();
    check_names(
        &dependencies,
        &expected_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    )?;
    let mut remaining = limits.maximum_dependency_bytes;
    for (id, digest) in selected.dependencies {
        let mut file = open_file(&dependencies, &dependency_name(&id))?;
        check_regular(&file, Some(FILE_MODE))?;
        let observed = hash_copy(
            &mut file,
            &mut io::sink(),
            remaining.min(limits.maximum_raw_bytes),
        )?;
        if observed.digest != digest || evidence.body.dependencies.get(&id) != Some(&observed) {
            return Err(PreparationError::DigestMismatch);
        }
        remaining -= observed.size;
    }
    raw.seek(SeekFrom::Start(0))?;
    let derived = materialize(
        &mut raw,
        evidence.layout(),
        selected.entrypoint,
        None,
        limits,
    )?;
    if derived != evidence.body.inventory {
        return Err(PreparationError::Invalid(
            "inventory is not derived from selected bytes",
        ));
    }
    let tree = open_directory(directory, "tree")?;
    verify_tree(&tree, &derived, limits)?;
    Ok(PreparedLocalArtifact {
        tree,
        evidence: evidence.clone(),
        entrypoint: selected.entrypoint.clone(),
    })
}

fn validate_locator(value: &str) -> Result<()> {
    if value != "prepared" {
        return Err(PreparationError::Invalid("invalid prepared locator"));
    }
    Ok(())
}

fn dependency_name(id: &Id) -> String {
    ContentDigest::of_bytes(id.as_str().as_bytes()).to_string()
}
fn file_mode(executable: bool) -> u32 {
    if executable {
        EXECUTABLE_MODE
    } else {
        FILE_MODE
    }
}

fn check_regular(file: &File, mode: Option<u32>) -> Result<()> {
    let meta = file.metadata()?;
    if !meta.is_file()
        || (mode.is_some() && meta.nlink() != 1)
        || mode.is_some_and(|mode| meta.mode() & 0o7777 != mode)
    {
        return Err(PreparationError::Invalid(
            "expected private regular file with exact mode",
        ));
    }
    Ok(())
}
fn check_directory(file: &File, mode: u32) -> Result<()> {
    let meta = file.metadata()?;
    if !meta.is_dir() || meta.mode() & 0o7777 != mode {
        return Err(PreparationError::Invalid(
            "expected directory with exact mode",
        ));
    }
    Ok(())
}
fn open_directory(parent: &File, name: &str) -> Result<File> {
    let file = File::from(
        fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?,
    );
    Ok(file)
}
fn open_file(parent: &File, name: &str) -> Result<File> {
    let file = File::from(
        fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?,
    );
    check_regular(&file, None)?;
    Ok(file)
}
fn create_directory(parent: &File, name: &str) -> Result<File> {
    fs::mkdirat(parent, name, Mode::from_raw_mode(0o700)).map_err(io::Error::from)?;
    let file = open_directory(parent, name)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    Ok(file)
}
fn create_file(parent: &File, name: &str) -> Result<File> {
    let file = File::from(
        fs::openat(
            parent,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?,
    );
    check_regular(&file, None)?;
    Ok(file)
}
fn seal_file(file: &File, executable: bool) -> Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(file_mode(executable)))?;
    file.sync_all()?;
    Ok(())
}
/// Make a directory's entries durable. On Linux, cap-std opens ambient
/// directories with `O_PATH`, and `fsync` on such a descriptor fails, so the
/// directory is reopened for reading through that handle. Other platforms
/// sync the handle itself, as before.
fn sync_directory(directory: &File) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        File::from(fs::openat(
            directory,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?)
        .sync_all()
    }
    #[cfg(not(target_os = "linux"))]
    {
        directory.sync_all()
    }
}
fn seal_directory(file: &File) -> Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(DIRECTORY_MODE))?;
    file.sync_all()?;
    Ok(())
}
fn open_relative(root: &File, path: &str) -> Result<File> {
    let parts = portable_path(path, 4096)?;
    let mut parent = root.try_clone()?;
    check_directory(&parent, DIRECTORY_MODE)?;
    for part in &parts[..parts.len() - 1] {
        parent = open_directory(&parent, part)?;
        check_directory(&parent, DIRECTORY_MODE)?;
    }
    open_file(&parent, parts[parts.len() - 1])
}
fn materialization_parent(root: &File, path: &str) -> Result<(File, String)> {
    let parts = portable_path(path, 4096)?;
    let mut parent = root.try_clone()?;
    for part in &parts[..parts.len() - 1] {
        parent = open_directory(&parent, part)?;
    }
    Ok((parent, parts[parts.len() - 1].to_string()))
}
fn hash_copy(
    reader: &mut impl Read,
    writer: &mut impl Write,
    maximum: u64,
) -> Result<FileEvidence> {
    let mut hasher = Sha256::new();
    let mut count = 0_u64;
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let size = reader.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        count = count
            .checked_add(size as u64)
            .ok_or(PreparationError::Limit("bytes"))?;
        if count > maximum {
            return Err(PreparationError::Limit("bytes"));
        }
        writer.write_all(&buffer[..size])?;
        hasher.update(&buffer[..size]);
    }
    let digest = ContentDigest::parse(hex::encode(hasher.finalize()))
        .map_err(|_| PreparationError::Invalid("invalid generated digest"))?;
    Ok(FileEvidence {
        size: count,
        digest,
    })
}
fn freeze(
    directory: &File,
    name: &str,
    source: &mut File,
    expected: &ContentDigest,
    limit: u64,
) -> Result<FileEvidence> {
    check_regular(source, None)?;
    source.seek(SeekFrom::Start(0))?;
    let mut target = create_file(directory, name)?;
    let observed = hash_copy(source, &mut target, limit)?;
    if &observed.digest != expected {
        return Err(PreparationError::DigestMismatch);
    }
    seal_file(&target, false)?;
    Ok(observed)
}

// ASCII-only is an explicit v1 interoperability profile, not lossy Unicode
// normalization. Unsupported names fail before any archive member is written.
fn portable_path(path: &str, maximum: usize) -> Result<Vec<&str>> {
    if path.is_empty() || path.len() > maximum {
        return Err(PreparationError::Limit("path length"));
    }
    if !path.is_ascii() {
        return Err(PreparationError::Unsupported("non-ASCII artifact path"));
    }
    let parts = path.split('/').collect::<Vec<_>>();
    for part in &parts {
        if part.is_empty()
            || *part == "."
            || *part == ".."
            || (part.ends_with('.') || part.ends_with(' '))
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-+@ ()".contains(&b))
        {
            return Err(PreparationError::Invalid("non-portable artifact path"));
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(PreparationError::Invalid("reserved artifact path"));
        }
    }
    Ok(parts)
}

fn add_path(
    inventory: &mut Inventory,
    aliases: &mut BTreeMap<String, String>,
    path: &str,
    value: InventoryValue,
    limits: &PreparationLimits,
) -> Result<()> {
    let parts = portable_path(path, limits.maximum_path_bytes)?;
    for index in 1..=parts.len() {
        let current = parts[..index].join("/");
        let alias = current.to_ascii_lowercase();
        if aliases.get(&alias).is_some_and(|prior| prior != &current) {
            return Err(PreparationError::Invalid("case-alias artifact paths"));
        }
        aliases.insert(alias, current.clone());
        let desired = if index == parts.len() {
            value.clone()
        } else {
            InventoryValue::Directory
        };
        match inventory.get(&current) {
            Some(InventoryValue::Directory) if desired == InventoryValue::Directory => {}
            Some(_) => {
                return Err(PreparationError::Invalid(
                    "duplicate or conflicting artifact paths",
                ));
            }
            None => {
                inventory.insert(current, desired);
            }
        }
        if inventory.len() > limits.maximum_entries {
            return Err(PreparationError::Limit("inventory entries"));
        }
    }
    Ok(())
}

fn materialize(
    raw: &mut File,
    layout: &LocalArtifactLayout,
    entrypoint: &Entrypoint,
    tree: Option<&File>,
    limits: &PreparationLimits,
) -> Result<Inventory> {
    raw.seek(SeekFrom::Start(0))?;
    let inventory = match layout {
        LocalArtifactLayout::SingleFile { executable } => {
            let mut inventory = Inventory::new();
            let observed = hash_copy(
                raw,
                &mut io::sink(),
                limits.maximum_file_bytes.min(limits.maximum_expanded_bytes),
            )?;
            add_path(
                &mut inventory,
                &mut BTreeMap::new(),
                &entrypoint.executable,
                InventoryValue::File {
                    size: observed.size,
                    digest: observed.digest,
                    executable: *executable,
                },
                limits,
            )?;
            if let Some(tree) = tree {
                create_tree_directories(tree, &inventory)?;
                let (parent, name) = materialization_parent(tree, &entrypoint.executable)?;
                let mut target = create_file(&parent, &name)?;
                raw.seek(SeekFrom::Start(0))?;
                hash_copy(
                    raw,
                    &mut target,
                    limits.maximum_file_bytes.min(limits.maximum_expanded_bytes),
                )?;
                seal_file(&target, *executable)?;
            }
            inventory
        }
        LocalArtifactLayout::Zip => zip32::materialize_zip(raw, tree, limits)?,
    };
    if !matches!(
        inventory.get(&entrypoint.executable),
        Some(InventoryValue::File { .. })
    ) {
        return Err(PreparationError::Invalid(
            "entrypoint is not a regular inventoried file",
        ));
    }
    Ok(inventory)
}

fn create_tree_directories(tree: &File, inventory: &Inventory) -> Result<()> {
    // Lexical order visits every parent before its child.
    for (path, value) in inventory {
        if *value == InventoryValue::Directory {
            let (parent, name) = materialization_parent(tree, path)?;
            create_directory(&parent, &name)?;
        }
    }
    Ok(())
}
fn seal_tree(tree: &File, inventory: &Inventory) -> Result<()> {
    for (path, value) in inventory.iter().rev() {
        if *value == InventoryValue::Directory {
            let (parent, name) = materialization_parent(tree, path)?;
            seal_directory(&open_directory(&parent, &name)?)?;
        }
    }
    seal_directory(tree)
}
fn names(directory: &File, maximum: usize) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for entry in fs::Dir::read_from(directory).map_err(io::Error::from)? {
        let entry = entry.map_err(io::Error::from)?;
        let name = entry
            .file_name()
            .to_str()
            .map_err(|_| PreparationError::Invalid("non-UTF8 stored path"))?;
        if name == "." || name == ".." {
            continue;
        }
        names.insert(name.to_string());
        if names.len() > maximum {
            return Err(PreparationError::Limit("directory entries"));
        }
    }
    Ok(names)
}
fn check_names(directory: &File, expected: &[&str]) -> Result<()> {
    let expected = expected
        .iter()
        .map(|name| (*name).to_string())
        .collect::<BTreeSet<_>>();
    if names(directory, expected.len())? != expected {
        return Err(PreparationError::Invalid(
            "unexpected or missing stored entry",
        ));
    }
    Ok(())
}
fn verify_tree(tree: &File, expected: &Inventory, limits: &PreparationLimits) -> Result<()> {
    check_directory(tree, DIRECTORY_MODE)?;
    let mut dirs = vec![(String::new(), tree.try_clone()?)];
    let mut found = BTreeSet::new();
    let mut remaining = limits.maximum_expanded_bytes;
    while let Some((prefix, dir)) = dirs.pop() {
        for name in names(&dir, limits.maximum_entries)? {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let value = expected
                .get(&path)
                .ok_or(PreparationError::Invalid("unexpected stored tree entry"))?;
            found.insert(path.clone());
            match value {
                InventoryValue::Directory => {
                    let child = open_directory(&dir, &name)?;
                    check_directory(&child, DIRECTORY_MODE)?;
                    dirs.push((path, child));
                }
                InventoryValue::File {
                    size,
                    digest,
                    executable,
                } => {
                    let mut file = open_file(&dir, &name)?;
                    check_regular(&file, Some(file_mode(*executable)))?;
                    let observed = hash_copy(
                        &mut file,
                        &mut io::sink(),
                        remaining.min(limits.maximum_file_bytes),
                    )?;
                    if observed.size != *size || &observed.digest != digest {
                        return Err(PreparationError::DigestMismatch);
                    }
                    remaining -= observed.size;
                }
            }
        }
    }
    if found.len() != expected.len() {
        return Err(PreparationError::Invalid("missing stored tree entry"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
