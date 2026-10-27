//! Installed resource closure verification before any bundled code executes.
//!
//! The product assembly embeds one manifest digest in the binary. Neither the
//! model, working directory nor a runtime environment variable selects code.
//! Product-owned installation permissions remain the mutation boundary; these
//! checks do not provide OS isolation against concurrent whole-tree replacement.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufReader, Read},
    path::{Component, Path, PathBuf},
    process::Stdio,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{ConnectError, setup::PreparedSetupConnection};

/// Reserve private invocation artifacts without starting the service preparer.
/// Both setup entry paths use the same realm-scoped product directory rules.
#[cfg(unix)]
pub fn reserve_setup_artifacts(
    storage: &meerkat_core::ResolvedStorage,
) -> Result<PathBuf, ConnectError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let realm = storage.locator.realm.as_str();
    if !relative(realm) || Path::new(realm).components().count() != 1 {
        return Err(invalid());
    }
    // Native resolution has already selected this root. Toolkit artifacts
    // stay outside the realm directory, before its first native reservation.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&storage.locator.state_root)
        .map_err(|e| ConnectError::native("create selected state root", e))?;
    let product = storage.locator.state_root.join(".meerkat-connect");
    let parent = product.join(realm);
    for path in [&product, &parent] {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(path).map_err(|_| invalid())?;
                if !metadata.is_dir() || mode(&metadata) & 0o077 != 0 {
                    return Err(invalid());
                }
            }
            Err(error) => {
                return Err(ConnectError::native(
                    "create private setup artifacts",
                    error,
                ));
            }
        }
    }
    let directory = tempfile::Builder::new()
        .prefix("setup-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(&parent)
        .map_err(|e| ConnectError::native("reserve setup artifact directory", e))?
        .keep();
    Ok(directory)
}

#[cfg(not(unix))]
pub fn reserve_setup_artifacts(
    _storage: &meerkat_core::ResolvedStorage,
) -> Result<PathBuf, ConnectError> {
    Err(invalid())
}

#[cfg(all(unix, feature = "console"))]
pub(crate) mod installation_readback;
#[cfg(unix)]
mod readback_process;
#[cfg(unix)]
mod local_round;
#[cfg(unix)]
pub use local_round::{LocalRoundGuard, LocalRoundOwner, OwnedLocalRoundGuard};
#[cfg(all(unix, feature = "console"))]
pub use local_round::LocalRoundAttempts;

#[cfg(not(unix))]
impl InstalledResources {
    pub(crate) async fn readback(
        self, _storage: &meerkat_core::ResolvedStorage, _request: &serde_json::Value,
    ) -> Result<serde_json::Value, ConnectError> {
        Err(invalid())
    }
}

const MANIFEST_LIMIT: u64 = 16 * 1024 * 1024;
const ENTRY_LIMIT: usize = 50_000;
const CONTENT_LIMIT: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    kind: String,
    target: String,
    prepare: String,
    python: String,
    node: String,
    npm_cli: String,
    #[serde(default, deserialize_with = "npm_resources_present")]
    npm_acquisition: Option<NpmResources>,
    starter: String,
    archive: String,
    entries: Vec<Entry>,
    sources: Vec<Source>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NpmResources {
    node: String,
    npm_cli: String,
    npm_archive: String,
}

fn npm_resources_present<'de, D>(deserializer: D) -> Result<Option<NpmResources>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Omission preserves older products; explicit null is not a resource group.
    NpmResources::deserialize(deserializer).map(Some)
}

#[cfg(all(test, unix))]
mod tests {

    use super::*;
    use serde_json::{Value, json};
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Fixture {
        directory: tempfile::TempDir,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("fixture");
            let root = directory.path().join("libexec/meerkat-connect");
            fs::create_dir_all(&root).expect("resource root");
            for name in [
                "prepare.py",
                "runtime/python/bin/python3.11",
                "runtime/node/bin/node",
                "runtime/node/lib/node_modules/npm/bin/npm-cli.js",
                "starter/start.py",
                "archives/library.tar.gz",
            ] {
                let path = root.join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, format!("owned fixture bytes: {name}")).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            for path in inventory(&root).unwrap() {
                if root.join(&path).is_dir() {
                    fs::set_permissions(root.join(path), fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
            }
            fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
            Self { directory, root }
        }

        fn manifest(&self) -> Value {
            let entries: Vec<Value> = inventory(&self.root).unwrap().into_iter().map(|path| {
                let full = self.root.join(&path);
                let metadata = fs::symlink_metadata(&full).unwrap();
                if metadata.is_symlink() {
                    json!({"path":path,"kind":"symlink","target":fs::read_link(full).unwrap(),"mode":mode(&metadata)})
                } else if metadata.is_dir() {
                    json!({"path":path,"kind":"directory","mode":mode(&metadata)})
                } else {
                    json!({"path":path,"kind":"file","sha256":hash_file(&full,metadata.len()).unwrap(),"size":metadata.len(),"mode":mode(&metadata)})
                }
            }).collect();
            json!({"schema_version":1,"kind":"meerkat_connect_resources","target":platform_target().unwrap(),
                "prepare":"prepare.py","python":"runtime/python/bin/python3.11","node":"runtime/node/bin/node",
                "npm_cli":"runtime/node/lib/node_modules/npm/bin/npm-cli.js","starter":"starter/start.py",
                "archive":"archives/library.tar.gz","entries":entries,"sources":[]})
        }

        fn pin(&self, manifest: Value) -> String {
            let bytes = serde_json::to_vec(&manifest).unwrap();
            let path = self.root.join("resources.json");
            fs::write(&path, &bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
            digest(&bytes)
        }

        fn verify(&self, pin: &str) -> Result<InstalledResources, ConnectError> {
            InstalledResources::verify(self.root.clone(), pin)
        }
    }

    #[test]
    fn installed_closure_is_bound_to_executable_layout_and_exact_manifest() {
        let fixture = Fixture::new();
        let pin = fixture.pin(fixture.manifest());
        let executable = fixture.directory.path().join("bin/meerkat-connect");
        fs::create_dir(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "test binary placeholder").unwrap();
        assert!(InstalledResources::from_executable(&executable, &pin).is_ok());
        assert!(InstalledResources::from_executable(&executable, &"0".repeat(64)).is_err());
        let wrong = fixture.directory.path().join("meerkat-connect");
        fs::write(&wrong, "outside bin").unwrap();
        assert!(InstalledResources::from_executable(&wrong, &pin).is_err());
        fs::write(
            fixture.root.join("runtime/python/bin/python3.11"),
            "changed",
        )
        .unwrap();
        assert!(fixture.verify(&pin).is_err());
    }

    #[test]
    fn optional_npm_group_requires_complete_exact_regular_executable_resources() {
        let fixture = Fixture::new();
        assert!(fixture.verify(&fixture.pin(fixture.manifest())).is_ok());
        let group = json!({
            "node": "runtime/acquisition-node/bin/node",
            "npm_cli": "runtime/acquisition-node/lib/node_modules/npm/bin/npm-cli.js",
            "npm_archive": "runtime/npm-acquisition.tgz",
        });
        for path in group.as_object().unwrap().values() {
            let full = fixture.root.join(path.as_str().unwrap());
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, "inert separately selected resource").unwrap();
            fs::set_permissions(&full, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut manifest = fixture.manifest();
        manifest["npm_acquisition"] = group.clone();
        let pin = fixture.pin(manifest.clone());
        assert!(fixture.verify(&pin).is_ok());
        for malformed in [Value::Null, json!({"node": group["node"]}),
            json!({"node": group["node"], "npm_cli": group["npm_cli"],
                "npm_archive": group["npm_archive"], "fallback": true}),
            json!({"node": "runtime/node/bin/node", "npm_cli": group["npm_cli"],
                "npm_archive": group["npm_archive"]})]
        {
            let mut bad = manifest.clone();
            bad["npm_acquisition"] = malformed;
            assert!(fixture.verify(&fixture.pin(bad)).is_err());
        }
        let node = fixture.root.join(group["node"].as_str().unwrap());
        fs::set_permissions(&node, fs::Permissions::from_mode(0o644)).unwrap();
        let mut no_exec = fixture.manifest();
        no_exec["npm_acquisition"] = group.clone();
        assert!(fixture.verify(&fixture.pin(no_exec)).is_err());
        fs::remove_file(&node).unwrap();
        symlink(fixture.root.join("runtime/node/bin/node"), &node).unwrap();
        let mut linked = fixture.manifest();
        linked["npm_acquisition"] = group.clone();
        assert!(fixture.verify(&fixture.pin(linked)).is_err());
        fs::remove_file(&node).unwrap();
        let mut missing = fixture.manifest();
        missing["npm_acquisition"] = group;
        assert!(fixture.verify(&fixture.pin(missing)).is_err());
    }

    #[test]
    fn undeclared_children_and_permission_changes_refuse_before_execution() {
        let fixture = Fixture::new();
        let pin = fixture.pin(fixture.manifest());
        fs::write(
            fixture.root.join("runtime/python/surprise.py"),
            "undeclared",
        )
        .unwrap();
        assert!(fixture.verify(&pin).is_err());
        fs::remove_file(fixture.root.join("runtime/python/surprise.py")).unwrap();
        fs::set_permissions(
            fixture.root.join("prepare.py"),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(fixture.verify(&pin).is_err());
        let writable_pin = fixture.pin(fixture.manifest());
        assert!(
            fixture.verify(&writable_pin).is_err(),
            "a pinned writable executable is still unsupported"
        );
        fs::set_permissions(
            fixture.root.join("prepare.py"),
            fs::Permissions::from_mode(0o4755),
        )
        .unwrap();
        assert!(
            fixture.verify(&pin).is_err(),
            "special permission bits must refuse before interpreter execution"
        );
    }

    #[test]
    fn internal_symlinks_work_but_escape_and_cycles_refuse() {
        let fixture = Fixture::new();
        let link = fixture.root.join("runtime/python/bin/python");
        symlink("python3.11", &link).unwrap();
        let pin = fixture.pin(fixture.manifest());
        assert!(fixture.verify(&pin).is_ok());
        fs::remove_file(&link).unwrap();
        let outside = fixture.directory.path().join("outside");
        fs::write(outside, "unselected").unwrap();
        symlink("../../../../../outside", &link).unwrap();
        let pin = fixture.pin(fixture.manifest());
        assert!(fixture.verify(&pin).is_err());
        fs::remove_file(&link).unwrap();
        symlink("python", &link).unwrap();
        let pin = fixture.pin(fixture.manifest());
        assert!(fixture.verify(&pin).is_err());
    }

    #[test]
    fn malformed_pinned_contract_is_not_authority_to_execute() {
        let fixture = Fixture::new();
        for change in 0..5 {
            let mut manifest = fixture.manifest();
            match change {
                0 => manifest["target"] = json!("unselected-target"),
                1 => manifest["python"] = json!("/usr/bin/python3"),
                2 => manifest["unexpected"] = json!(true),
                3 => manifest["entries"][0]["path"] = json!("../escape"),
                4 => manifest["entries"].as_array_mut().unwrap().reverse(),
                _ => unreachable!(),
            }
            let pin = fixture.pin(manifest);
            assert!(fixture.verify(&pin).is_err(), "malformed contract {change}");
        }
    }

    #[tokio::test]
    async fn preparation_resolves_owned_artifacts_without_rewriting_aliased_native_root() {
        let fixture = Fixture::new();
        // An owned executable models only the preparer's canonical-parent
        // precondition. It refuses the old lexical handoff before emitting any
        // usable connection, without requiring Python or a product archive.
        fs::write(fixture.root.join("runtime/python/bin/python3.11"), br##"#!/bin/sh
set -eu
[ "$#" -eq 5 ]
[ "$4" = "--destination" ]
parent=${5%/*}
canonical=$(CDPATH= cd "$parent" && pwd -P)
[ "$parent" = "$canonical" ]
[ ! -e "$5" ]
printf '%s\n' '{"mcpServers":{"toolkit-setup":{"command":"/usr/bin/env","args":["-i","/bin/true"]}}}'
"##).unwrap();
        let pin = fixture.pin(fixture.manifest());
        let resources = fixture.verify(&pin).unwrap();
        let physical = fixture.directory.path().join("physical-state");
        fs::create_dir(&physical).unwrap();
        let alias = fixture.directory.path().join("selected-state-alias");
        symlink(&physical, &alias).unwrap();
        let storage = meerkat_core::StorageLayout::resolve_with_global_candidate(
            meerkat_core::StorageLayoutInputs {
                invocation_context: fixture.directory.path().to_owned(),
                explicit_state_root: Some(alias.clone()),
                ..Default::default()
            },
            &meerkat_core::RealmConfig {
                selection: meerkat_core::RealmSelection::Explicit {
                    realm_id: "alias-test".into(),
                },
                ..Default::default()
            },
            &fixture.directory.path().join("unused-global"),
        )
        .unwrap();
        assert_ne!(alias, fs::canonicalize(&alias).unwrap());
        let prepared = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            resources.prepare(&storage, std::future::pending::<std::io::Result<()>>()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(storage.locator.state_root, alias);
        assert_eq!(storage.layout.state_root(), storage.locator.state_root);
        assert_eq!(
            prepared.artifact_directory,
            fs::canonicalize(&prepared.artifact_directory).unwrap()
        );
        assert!(
            prepared
                .artifact_directory
                .starts_with(fs::canonicalize(physical).unwrap())
        );
        assert_eq!(
            mode(&fs::metadata(&prepared.artifact_directory).unwrap()) & 0o777,
            0o700
        );
        assert!(prepared.artifact_directory.join("prepare.stdout").is_file());
        assert!(
            fs::read(prepared.artifact_directory.join("prepare.stderr"))
                .unwrap()
                .is_empty()
        );
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    name: String,
    url: String,
    sha256: String,
    size: u64,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Entry {
    File {
        path: String,
        sha256: String,
        size: u64,
        mode: u32,
    },
    Symlink {
        path: String,
        target: String,
        mode: u32,
    },
    Directory {
        path: String,
        mode: u32,
    },
}

impl Entry {
    fn path(&self) -> &str {
        match self {
            Self::File { path, .. } | Self::Symlink { path, .. } | Self::Directory { path, .. } => {
                path
            }
        }
    }

    fn mode(&self) -> u32 {
        match self {
            Self::File { mode, .. } | Self::Symlink { mode, .. } | Self::Directory { mode, .. } => {
                *mode
            }
        }
    }
}

pub struct InstalledResources {
    root: PathBuf,
    python: PathBuf,
    prepare: PathBuf,
}

/// Prepared artifact inputs. This is not an installation or native grant.
pub struct PreparedResources {
    pub connection: PreparedSetupConnection,
    pub artifact_directory: PathBuf,
}

fn invalid() -> ConnectError {
    ConnectError::InvalidInput("Installed Connect resources are unavailable or changed; reinstall the complete product. No setup command was started.".into())
}

fn relative(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', '\0'])
        && !value.starts_with('/')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(unix)]
fn mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode(_: &fs::Metadata) -> u32 {
    0
}

fn platform_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-arm64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        _ => None,
    }
}

fn hash_file(path: &Path, size: u64) -> Result<String, ConnectError> {
    let mut input = BufReader::new(fs::File::open(path).map_err(|_| invalid())?).take(size + 1);
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|_| invalid())?;
        if read == 0 {
            break;
        }
        count += read as u64;
        hash.update(&buffer[..read]);
    }
    if count != size {
        return Err(invalid());
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn inventory(root: &Path) -> Result<BTreeSet<String>, ConnectError> {
    let mut result = BTreeSet::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for item in fs::read_dir(directory).map_err(|_| invalid())? {
            let item = item.map_err(|_| invalid())?;
            let path = item.path();
            let name = path
                .strip_prefix(root)
                .map_err(|_| invalid())?
                .to_str()
                .ok_or_else(invalid)?
                .to_owned();
            if name == "resources.json" {
                continue;
            }
            if !relative(&name) || !result.insert(name) || result.len() > ENTRY_LIMIT {
                return Err(invalid());
            }
            if item.file_type().map_err(|_| invalid())?.is_dir() {
                directories.push(path);
            }
        }
    }
    Ok(result)
}

impl InstalledResources {
    pub fn locate() -> Result<Self, ConnectError> {
        let pin = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256").ok_or_else(invalid)?;
        let executable = std::env::current_exe().map_err(|_| invalid())?;
        Self::from_executable(&executable, pin)
    }

    fn from_executable(executable: &Path, pin: &str) -> Result<Self, ConnectError> {
        if !valid_digest(pin) {
            return Err(invalid());
        }
        let executable = executable.canonicalize().map_err(|_| invalid())?;
        let bin = executable.parent().ok_or_else(invalid)?;
        if bin.file_name().and_then(|name| name.to_str()) != Some("bin") {
            return Err(invalid());
        }
        let prefix = bin.parent().ok_or_else(invalid)?;
        let root = prefix.join("libexec/meerkat-connect");
        Self::verify(root, pin)
    }

    fn verify(root: PathBuf, pin: &str) -> Result<Self, ConnectError> {
        let metadata = fs::symlink_metadata(&root).map_err(|_| invalid())?;
        if !metadata.is_dir() || mode(&metadata) & 0o7022 != 0 {
            return Err(invalid());
        }
        let root = root.canonicalize().map_err(|_| invalid())?;
        let manifest_path = root.join("resources.json");
        let metadata = fs::symlink_metadata(&manifest_path).map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.len() > MANIFEST_LIMIT || mode(&metadata) & 0o7022 != 0 {
            return Err(invalid());
        }
        let mut raw = Vec::new();
        fs::File::open(manifest_path)
            .map_err(|_| invalid())?
            .take(MANIFEST_LIMIT + 1)
            .read_to_end(&mut raw)
            .map_err(|_| invalid())?;
        if raw.len() as u64 > MANIFEST_LIMIT || digest(&raw) != pin {
            return Err(invalid());
        }
        let manifest: Manifest = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        if manifest.schema_version != 1
            || manifest.kind != "meerkat_connect_resources"
            || Some(manifest.target.as_str()) != platform_target()
            || manifest.prepare != "prepare.py"
            || manifest.python != "runtime/python/bin/python3.11"
            || manifest.node != "runtime/node/bin/node"
            || manifest.npm_cli != "runtime/node/lib/node_modules/npm/bin/npm-cli.js"
            || manifest.starter != "starter/start.py"
            || !relative(&manifest.archive)
            || !manifest.archive.starts_with("archives/")
            || manifest.entries.is_empty()
            || manifest.entries.len() > ENTRY_LIMIT
            || manifest.sources.len() > 32
        {
            return Err(invalid());
        }
        for source in &manifest.sources {
            if source.name.is_empty()
                || source.name.len() > 256
                || source.url.len() > 4096
                || !source.url.starts_with("https://")
                || !valid_digest(&source.sha256)
                || source.size == 0
                || source.size > CONTENT_LIMIT
            {
                return Err(invalid());
            }
        }
        let mut expected = BTreeMap::new();
        let mut previous: Option<&str> = None;
        let mut total = 0_u64;
        for entry in &manifest.entries {
            let name = entry.path();
            if !relative(name)
                || name == "resources.json"
                || previous.is_some_and(|old| old >= name)
                || entry.mode() > 0o777
                || expected.insert(name.to_owned(), entry).is_some()
            {
                return Err(invalid());
            }
            previous = Some(name);
            if let Entry::File { sha256, size, .. } = entry {
                total = total.checked_add(*size).ok_or_else(invalid)?;
                if !valid_digest(sha256) || total > CONTENT_LIMIT {
                    return Err(invalid());
                }
            }
        }
        if inventory(&root)? != expected.keys().cloned().collect() {
            return Err(invalid());
        }
        for (name, entry) in &expected {
            for ancestor in Path::new(name)
                .ancestors()
                .skip(1)
                .filter(|path| !path.as_os_str().is_empty())
            {
                if !matches!(
                    expected.get(ancestor.to_str().ok_or_else(invalid)?),
                    Some(Entry::Directory { .. })
                ) {
                    return Err(invalid());
                }
            }
            let path = root.join(name);
            let metadata = fs::symlink_metadata(&path).map_err(|_| invalid())?;
            if mode(&metadata) != entry.mode() {
                return Err(invalid());
            }
            match entry {
                Entry::File { sha256, size, .. } => {
                    if !metadata.is_file()
                        || metadata.len() != *size
                        || mode(&metadata) & 0o022 != 0
                        || hash_file(&path, *size)? != *sha256
                    {
                        return Err(invalid());
                    }
                }
                Entry::Directory { .. } => {
                    if !metadata.is_dir() || mode(&metadata) & 0o022 != 0 {
                        return Err(invalid());
                    }
                }
                Entry::Symlink { target, .. } => {
                    if !metadata.is_symlink()
                        || target.is_empty()
                        || Path::new(target).is_absolute()
                        || fs::read_link(&path).map_err(|_| invalid())? != Path::new(target)
                        || !path
                            .canonicalize()
                            .map_err(|_| invalid())?
                            .starts_with(&root)
                    {
                        return Err(invalid());
                    }
                }
            }
        }
        if let Some(npm) = &manifest.npm_acquisition {
            if npm.node != "runtime/acquisition-node/bin/node"
                || npm.npm_cli != "runtime/acquisition-node/lib/node_modules/npm/bin/npm-cli.js"
                || npm.npm_archive != "runtime/npm-acquisition.tgz"
            {
                return Err(invalid());
            }
            for name in [&npm.node, &npm.npm_cli, &npm.npm_archive] {
                if !matches!(expected.get(name), Some(Entry::File { .. })) {
                    return Err(invalid());
                }
            }
            if mode(&fs::metadata(root.join(&npm.node)).map_err(|_| invalid())?) & 0o111 == 0 {
                return Err(invalid());
            }
        }
        for name in [
            &manifest.prepare,
            &manifest.python,
            &manifest.node,
            &manifest.npm_cli,
            &manifest.starter,
            &manifest.archive,
        ] {
            if !expected.contains_key(name) || !root.join(name).is_file() {
                return Err(invalid());
            }
        }
        for name in [&manifest.python, &manifest.node] {
            if mode(&fs::metadata(root.join(name)).map_err(|_| invalid())?) & 0o111 == 0 {
                return Err(invalid());
            }
        }
        Ok(Self {
            python: root.join(manifest.python),
            prepare: root.join(manifest.prepare),
            root,
        })
    }

    /// Run the selected resource preparer once. It owns the bootstrap pipeline
    /// and its descendant settlement. A cancelled or failed attempt is retained;
    /// this method never starts a successor or reconstructs installation state.
    #[cfg(unix)]
    pub async fn prepare(
        self,
        storage: &meerkat_core::ResolvedStorage,
        cancelled: impl std::future::Future<Output = std::io::Result<()>>,
    ) -> Result<PreparedResources, ConnectError> {
        self.prepare_once(storage, cancelled).await
    }

    #[cfg(unix)]
    async fn prepare_once(
        &self,
        storage: &meerkat_core::ResolvedStorage,
        cancelled: impl std::future::Future<Output = std::io::Result<()>>,
    ) -> Result<PreparedResources, ConnectError> {
        use nix::{
            sys::signal::{Signal, kill},
            unistd::Pid,
        };
        use std::os::unix::fs::OpenOptionsExt;

        let directory = reserve_setup_artifacts(storage)?;
        // The selected native state root may use a filesystem alias (for
        // example /var on macOS). The trusted preparer requires an existing,
        // canonical parent for its fresh destination. Resolve only our newly
        // reserved artifacts; the native locator and privacy checks stay intact.
        let directory = fs::canonicalize(&directory)
            .map_err(|e| ConnectError::native("resolve reserved preparation artifacts", e))?;
        let failure = |reason: &str| {
            ConnectError::InvalidInput(format!(
                "Connect preparation {reason}. Files are retained at {}. No automatic retry was started.",
                directory.display(),
            ))
        };
        let stdout_path = directory.join("prepare.stdout");
        let stdout = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&stdout_path)
            .map_err(|_| failure("could not retain output"))?;
        let stderr = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("prepare.stderr"))
            .map_err(|_| failure("could not retain diagnostics"))?;
        let mut command = tokio::process::Command::new(&self.python);
        command
            .args(["-I", "-B"])
            .arg(&self.prepare)
            .arg("--destination")
            .arg(directory.join("prepared"))
            .arg("--realm-state-root").arg(&storage.locator.state_root)
            .arg("--realm").arg(storage.locator.realm.as_str())
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| failure("could not start"))?;
        tokio::pin!(cancelled);
        let mut interrupted = false;
        let status = tokio::select! {
            status = child.wait() => status,
            signal = &mut cancelled => {
                interrupted = true;
                // Signal only our unreaped direct child. Its accepted starter
                // handles SIGTERM and owns joining bootstrap descendants.
                let signal_error = child.id().and_then(|id| i32::try_from(id).ok())
                    .and_then(|pid| kill(Pid::from_raw(pid), Signal::SIGTERM).err())
                    .filter(|error| *error != nix::errno::Errno::ESRCH);
                let status = child.wait().await;
                if signal_error.is_some() { return Err(failure("could not deliver cancellation; direct child was joined")); }
                if signal.is_err() { return Err(failure("could not observe cancellation; child was joined")); }
                status
            }
        }.map_err(|_| failure("outcome is unconfirmed"))?;
        if interrupted {
            return Err(failure(
                "was cancelled; any completed or partial files remain",
            ));
        }
        if !status.success() {
            return Err(failure("did not complete"));
        }
        let metadata =
            fs::symlink_metadata(&stdout_path).map_err(|_| failure("result is unavailable"))?;
        if !metadata.is_file() || metadata.len() > 256 * 1024 {
            return Err(failure("result is invalid"));
        }
        let mut raw = Vec::new();
        fs::File::open(stdout_path)
            .map_err(|_| failure("result is unavailable"))?
            .take(256 * 1024 + 1)
            .read_to_end(&mut raw)
            .map_err(|_| failure("result is unavailable"))?;
        let connection = PreparedSetupConnection::from_preparer_output(&raw)
            .map_err(|_| failure("result is invalid"))?;
        Ok(PreparedResources {
            connection,
            artifact_directory: directory,
        })
    }

    #[cfg(not(unix))]
    pub async fn prepare(
        self,
        _storage: &meerkat_core::ResolvedStorage,
        _cancelled: impl std::future::Future<Output = std::io::Result<()>>,
    ) -> Result<PreparedResources, ConnectError> {
        Err(invalid())
    }
}
