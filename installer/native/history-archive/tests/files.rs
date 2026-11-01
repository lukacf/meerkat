#![cfg(unix)]
use forge_history_archive::{ArchiveError, DIRECTORY, MAX_ARCHIVE_BYTES, save};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let name = format!(
            "forge-history-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn exact_json_bytes_and_private_file_survive_return_without_pending_name() {
    let root = Root::new();
    let value =
        json!({"instance":"abc","payload":"line\nquote\"nul\0","optional":null,"state":"failure"});
    let saved = save(&root.0, "abc", &value, 4096).unwrap();
    assert_eq!(
        saved.path.file_name().unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.json"
    );
    let bytes = fs::read(&saved.path).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        value
    );
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_eq!(saved.bytes, bytes.len());
    assert_eq!(saved.sha256, format!("{:x}", Sha256::digest(&bytes)));
    assert_eq!(
        fs::metadata(&saved.path).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert_eq!(
        fs::metadata(root.0.join(DIRECTORY))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(fs::read_dir(root.0.join(DIRECTORY)).unwrap().count(), 1);
}

#[test]
fn an_original_archive_cannot_be_replaced_by_another_snapshot() {
    let root = Root::new();
    let saved = save(&root.0, "same-original", &json!({"first":true}), 4096).unwrap();
    let original = fs::read(&saved.path).unwrap();
    assert!(matches!(
        save(&root.0, "same-original", &json!({"replacement":true}), 4096),
        Err(ArchiveError::Storage)
    ));
    assert_eq!(fs::read(&saved.path).unwrap(), original);
    assert_eq!(fs::read_dir(root.0.join(DIRECTORY)).unwrap().count(), 1);
}

#[test]
fn interrupted_or_oversized_archive_remains_pending_and_is_not_replayed() {
    let root = Root::new();
    let data = json!({"payload":"x".repeat(128)});
    assert!(save(&root.0, "limited", &data, 16).is_err());
    let paths = fs::read_dir(root.0.join(DIRECTORY))
        .unwrap()
        .map(|x| x.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].extension().unwrap(), "pending");
    assert!(fs::metadata(&paths[0]).unwrap().len() <= 16);
    let original = fs::read(&paths[0]).unwrap();
    assert!(save(&root.0, "limited", &data, 4096).is_err());
    assert_eq!(fs::read(&paths[0]).unwrap(), original);
}

#[test]
fn invalid_limits_or_identity_do_not_create_output() {
    let root = Root::new();
    for limit in [0, MAX_ARCHIVE_BYTES + 1] {
        assert!(matches!(
            save(&root.0, "original", &json!({}), limit),
            Err(ArchiveError::InvalidLimit)
        ));
    }
    for id in ["".to_owned(), "bad\nidentity".to_owned(), "x".repeat(257)] {
        assert!(matches!(
            save(&root.0, &id, &json!({}), 1024),
            Err(ArchiveError::InvalidInstance)
        ));
    }
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
}

#[test]
fn symlinks_and_insecure_directories_cannot_redirect_private_data() {
    let root = Root::new();
    let other = Root::new();
    let alias = root.0.join("alias");
    symlink(&other.0, &alias).unwrap();
    assert!(matches!(
        save(&alias, "original", &json!({}), 1024),
        Err(ArchiveError::InvalidRoot)
    ));
    symlink(&other.0, root.0.join(DIRECTORY)).unwrap();
    assert!(matches!(
        save(&root.0, "original", &json!({}), 1024),
        Err(ArchiveError::InvalidRoot)
    ));
    assert_eq!(fs::read_dir(&other.0).unwrap().count(), 0);
    fs::remove_file(root.0.join(DIRECTORY)).unwrap();
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        save(&root.0, "original", &json!({}), 1024),
        Err(ArchiveError::InvalidRoot)
    ));
}
