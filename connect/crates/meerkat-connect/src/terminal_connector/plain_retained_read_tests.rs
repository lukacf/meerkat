use super::bounded_read;
use std::os::unix::fs::{OpenOptionsExt, symlink};
use std::time::Duration;

#[tokio::test]
async fn regular_document_at_limit_is_read_exactly() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("document.json");
    let bytes = vec![b'x'; 1024 * 1024];
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(bounded_read(&path).await.unwrap(), bytes);
}

#[tokio::test]
async fn oversized_document_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("document.json");
    std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(bounded_read(&path).await.is_err());
}

#[tokio::test]
async fn symlink_to_regular_document_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target.json");
    let link = root.path().join("document.json");
    std::fs::write(&target, b"{}").unwrap();
    symlink(&target, &link).unwrap();
    assert!(bounded_read(&link).await.is_err());
}

#[tokio::test]
async fn fifo_without_writer_is_refused_without_waiting_for_a_writer() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("document.json");
    nix::unistd::mkfifo(
        &path,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), bounded_read(&path)).await;
    // Release a blocking open from a regressed implementation before asserting,
    // so the Tokio blocking pool can shut down even when this control fails.
    let release = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(&path)
        .unwrap();
    drop(release);
    assert!(
        result
            .expect("retained-file open waited for a FIFO writer")
            .is_err()
    );
}

#[tokio::test]
async fn directory_is_refused() {
    let root = tempfile::tempdir().unwrap();
    assert!(bounded_read(root.path()).await.is_err());
}
