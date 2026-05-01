//! Cross-process token-store operation lock.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

use super::{TokenKey, TokenStoreError};

pub struct TokenStoreOpLock {
    file: Option<File>,
}

impl Drop for TokenStoreOpLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = FileExt::unlock(&file);
            drop(file);
        }
    }
}

pub async fn lock(root: &Path, key: &TokenKey) -> Result<TokenStoreOpLock, TokenStoreError> {
    let lock_dir = root.join(".locks").join(key.realm.as_str());
    tokio::fs::create_dir_all(&lock_dir).await?;
    let lock_path = lock_dir.join(lock_file_name(key));
    let file = tokio::task::spawn_blocking(move || -> Result<File, TokenStoreError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        Ok(file)
    })
    .await
    .map_err(|e| TokenStoreError::Unavailable(format!("token-store lock task failed: {e}")))??;
    Ok(TokenStoreOpLock { file: Some(file) })
}

fn lock_file_name(key: &TokenKey) -> PathBuf {
    let stem = match &key.profile {
        Some(profile) => format!("{}@{}", key.binding.as_str(), profile.as_str()),
        None => key.binding.as_str().to_string(),
    };
    PathBuf::from(format!("{stem}.lock"))
}
