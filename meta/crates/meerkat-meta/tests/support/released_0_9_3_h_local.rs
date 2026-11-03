//! Staging for the released Meta 0.9.3 H-local state directories under
//! `tests/fixtures/released_0_9_3_h_local/` (see tests/fixtures/README.md).
//!
//! Every reader works on bytes: the committed files are hashed and copied,
//! never opened as databases. A test stages one pristine copy per use:
//! `state/` is copied out, the files the mint script stored gzip-compressed
//! (MANIFEST.json `packed`) are inflated and checked against their raw
//! sha256, and the empty directories git cannot keep (MANIFEST.json
//! `empty_dirs`) are recreated. The staged tree then equals MANIFEST.json
//! `files.state` byte for byte. The H-app captures stage the same way, through
//! [`assert_capture_is_the_released_bytes`] and [`stage_capture`].

#![allow(dead_code, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The captures. See the mint scripts beside them for the plans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Three clean boots; the roster drops `agent:hl-18` at the last one.
    /// The 0.9.3 gateway starts on the final state again.
    Bootable,
    /// The roster drops `agent:hl-18` at boot 2, which then runs the
    /// lifecycle verbs. The 0.9.3 gateway cannot start the final state.
    Stuck,
    /// As `Bootable`, then compact_member wedges the last boot and the
    /// gateway is killed.
    Killed,
    /// As `Bootable`, plus two identities compacted six times, past the 0.9.3
    /// retention of four transcript rewrites, so both committed graphs are
    /// re-anchored: `agent:hl-16` stays bound to its session, and
    /// `agent:hl-17`'s session outlives a delete_identity as a runtime-only
    /// body nothing is bound to. Minted by its own driver.
    Reanchored,
    /// A plan of its own over three identities: `agent:hl-02` is deleted and
    /// materialized again, and `agent:hl-03` is materialized for the first
    /// time, neither taking a turn before the clean shutdown. Each is bound
    /// to a one-message session that continuity.sqlite3 holds only as its
    /// materialization snapshot, with no head. Minted by its own driver.
    Rematerialized,
    /// A plan of its own over one identity, `agent:hl-01`, and a member the
    /// host makes itself with `meta/ensure_member` (the stdio gateway's raw
    /// mob path) and an application label, `hl-direct`, that takes a turn at
    /// each of two boots. 0.9.3
    /// keeps no continuity record of `hl-direct`, and only runtime.sqlite
    /// holds its session. Minted by its own driver.
    DirectMember,
}

impl Variant {
    pub const ALL: [Variant; 6] = [
        Variant::Bootable,
        Variant::Stuck,
        Variant::Killed,
        Variant::Reanchored,
        Variant::Rematerialized,
        Variant::DirectMember,
    ];

    /// The captures of the 18-identity H-local plan; the others follow plans
    /// of their own.
    pub const H_LOCAL_PLAN: [Variant; 4] = [
        Variant::Bootable,
        Variant::Stuck,
        Variant::Killed,
        Variant::Reanchored,
    ];

    pub fn dir_name(self) -> &'static str {
        match self {
            Variant::Bootable => "bootable",
            Variant::Stuck => "stuck",
            Variant::Killed => "killed",
            Variant::Reanchored => "reanchored",
            Variant::Rematerialized => "rematerialized",
            Variant::DirectMember => "direct-member",
        }
    }

    /// The mint script beside the variants that wrote this one; the
    /// variant's MANIFEST pins its sha256.
    pub fn driver_file(self) -> &'static str {
        match self {
            Variant::Bootable | Variant::Stuck | Variant::Killed => {
                "mint_released_0_9_3_h_local.py"
            }
            Variant::Reanchored => "mint_released_0_9_3_h_local_reanchored.py",
            Variant::Rematerialized => "mint_released_0_9_3_h_local_rematerialized.py",
            Variant::DirectMember => "mint_released_0_9_3_h_local_direct_member.py",
        }
    }

    pub fn dir(self) -> PathBuf {
        fixture_root().join(self.dir_name())
    }

    pub fn manifest(self) -> Value {
        read_manifest(&self.dir())
    }
}

/// The MANIFEST.json of the capture under `root`.
pub fn read_manifest(root: &Path) -> Value {
    let text =
        std::fs::read_to_string(root.join("MANIFEST.json")).expect("read the variant manifest");
    serde_json::from_str(&text).expect("the variant manifest is JSON")
}

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/released_0_9_3_h_local")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn files_under(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read a fixture directory") {
        let path = entry.expect("fixture directory entry").path();
        if path.is_dir() {
            files_under(root, &path, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .expect("fixture file under its root")
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
            );
        }
    }
}

/// The committed variant is exactly the capture: every file listed in its
/// `checksums.sha256` holds the listed digest, and `state/` holds nothing
/// else.
pub fn assert_variant_is_the_released_capture(variant: Variant) {
    assert_capture_is_the_released_bytes(&variant.dir(), variant.dir_name());
}

/// [`assert_variant_is_the_released_capture`] for the capture under `root`,
/// named `name` in failures.
pub fn assert_capture_is_the_released_bytes(root: &Path, name: &str) {
    let listing =
        std::fs::read_to_string(root.join("checksums.sha256")).expect("read the variant checksums");
    let mut pinned_state = Vec::new();
    for line in listing.lines() {
        let (digest, file) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("malformed checksum line {line:?}"));
        let bytes = std::fs::read(root.join(file))
            .unwrap_or_else(|error| panic!("read fixture file {file}: {error}"));
        assert_eq!(
            sha256_hex(&bytes),
            digest,
            "{name}/{file} no longer holds the released bytes; restore it from git and never \
             open fixture files in place"
        );
        if file.starts_with("state/") {
            pinned_state.push(file.to_string());
        }
    }
    let mut present = Vec::new();
    files_under(root, &root.join("state"), &mut present);
    pinned_state.sort();
    present.sort();
    assert_eq!(
        present, pinned_state,
        "{name}/state holds files outside its checksums"
    );
}

fn copy_dir_recursive(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create a staged directory");
    for entry in std::fs::read_dir(from).expect("read a fixture directory") {
        let entry = entry.expect("fixture directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("fixture entry type").is_dir() {
            copy_dir_recursive(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy a fixture file");
            make_owner_writable(&target);
        }
    }
}

/// A staged copy is the consumer's to write. `fs::copy` keeps the source
/// mode, so a read-only checkout (a sandboxed build, say) would otherwise
/// stage databases SQLite cannot open for writing.
fn make_owner_writable(path: &Path) {
    let mut permissions = std::fs::metadata(path)
        .expect("staged file metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    // Outside unix the read-only flag is the owner's write bit.
    #[cfg(not(unix))]
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions).expect("make a staged file writable");
}

/// The raw digests the mint script recorded for the final state, by path
/// relative to `state/`.
pub fn manifest_state_digests(variant: Variant) -> BTreeMap<String, String> {
    state_digests_in(&variant.manifest())
}

fn state_digests_in(manifest: &Value) -> BTreeMap<String, String> {
    manifest["files"]["state"]
        .as_array()
        .expect("files.state is an array")
        .iter()
        .map(|row| {
            (
                row["path"].as_str().expect("file path").to_string(),
                row["sha256"].as_str().expect("file sha256").to_string(),
            )
        })
        .collect()
}

/// Every file under a staged state directory with its sha256, by path
/// relative to it.
pub fn staged_digests(state: &Path) -> BTreeMap<String, String> {
    let mut files = Vec::new();
    files_under(state, state, &mut files);
    files
        .into_iter()
        .map(|rel| {
            let bytes = std::fs::read(state.join(&rel)).expect("read a staged file");
            (rel, sha256_hex(&bytes))
        })
        .collect()
}

/// Stage one pristine, owner-writable copy of the variant's final state into
/// `<temp>/state` and return its path. Panics when the committed bytes or the
/// staged tree differ from what the release wrote.
pub fn stage_variant(variant: Variant, temp: &Path) -> PathBuf {
    stage_capture(&variant.dir(), variant.dir_name(), temp)
}

/// [`stage_variant`] for the capture under `root`, named `name` in failures.
pub fn stage_capture(root: &Path, name: &str, temp: &Path) -> PathBuf {
    assert_capture_is_the_released_bytes(root, name);
    let manifest = read_manifest(root);
    let state = temp.join("state");
    copy_dir_recursive(&root.join("state"), &state);
    for row in manifest["packed"].as_array().expect("packed is an array") {
        let path = row["path"].as_str().expect("packed path");
        let stored_as = row["stored_as"].as_str().expect("packed stored_as");
        let stored = std::fs::read(state.join(stored_as)).expect("read a packed file");
        let mut raw = Vec::new();
        GzDecoder::new(stored.as_slice())
            .read_to_end(&mut raw)
            .expect("inflate a packed file");
        assert_eq!(
            sha256_hex(&raw),
            row["sha256"].as_str().expect("packed sha256"),
            "{stored_as} does not inflate to the bytes the release wrote"
        );
        std::fs::write(state.join(path), &raw).expect("write an inflated file");
        std::fs::remove_file(state.join(stored_as)).expect("drop the packed copy");
    }
    for dir in manifest["empty_dirs"]
        .as_array()
        .expect("empty_dirs is an array")
    {
        std::fs::create_dir_all(state.join(dir.as_str().expect("empty dir path")))
            .expect("recreate an empty directory");
    }
    assert_eq!(
        staged_digests(&state),
        state_digests_in(&manifest),
        "the staged {name} state differs from the minted final state"
    );
    state
}
