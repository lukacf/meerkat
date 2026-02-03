//! Project root detection utilities
//!
//! This module provides functions to detect the project root directory
//! for Meerkat agents, following a `.rkat`-only search strategy.

use std::path::{Path, PathBuf};

/// Find the project root directory.
///
/// Search order:
/// 1. Walk up from `start_dir` looking for `.rkat/`
///
/// # Arguments
/// * `start_dir` - The directory to start searching from (typically current working directory)
///
/// # Returns
/// The detected project root directory, or None if no `.rkat` ancestor exists.
pub fn find_project_root(start_dir: &Path) -> Option<PathBuf> {
    find_ancestor_with(start_dir, ".rkat")
}

/// Find an ancestor directory containing the specified marker directory.
///
/// Walks up the directory tree from `start` looking for a directory
/// that contains a subdirectory or file named `marker`.
fn find_ancestor_with(start: &Path, marker: &str) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(marker).exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Ensure `.rkat` directory exists in project root, creating if needed.
///
/// # Arguments
/// * `project_root` - The project root directory
///
/// # Returns
/// * `Ok(PathBuf)` - Path to the `.rkat` directory
/// * `Err` - If directory creation failed
pub fn ensure_rkat_dir(project_root: &Path) -> std::io::Result<PathBuf> {
    let rkat_dir = project_root.join(".rkat");
    if !rkat_dir.exists() {
        std::fs::create_dir_all(&rkat_dir)?;
    }
    Ok(rkat_dir)
}

/// Ensure `.rkat` directory exists in project root, creating if needed.
///
/// This async variant uses `tokio::fs` so it can be called from async request handlers
/// without blocking the runtime.
pub async fn ensure_rkat_dir_async(project_root: &Path) -> std::io::Result<PathBuf> {
    let rkat_dir = project_root.join(".rkat");
    tokio::fs::create_dir_all(&rkat_dir).await?;
    Ok(rkat_dir)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_find_project_root_with_rkat_dir() {
        let temp_dir = TempDir::new().unwrap();

        // Create a project structure with .rkat/
        let project_root = temp_dir.path().join("my_project");
        let rkat_dir = project_root.join(".rkat");
        let nested_dir = project_root.join("src/deeply/nested");

        fs::create_dir_all(&rkat_dir).unwrap();
        fs::create_dir_all(&nested_dir).unwrap();

        let result = find_project_root(&nested_dir);
        assert_eq!(result, Some(project_root));
    }

    #[test]
    fn test_find_project_root_without_rkat_returns_none() {
        let temp_dir = TempDir::new().unwrap();

        // Create a directory with no markers
        let orphan_dir = temp_dir.path().join("orphan/nested");
        fs::create_dir_all(&orphan_dir).unwrap();

        let result = find_project_root(&orphan_dir);
        assert_eq!(result, None);
    }

    #[test]
    fn test_rkat_dir_takes_precedence_over_git() {
        let temp_dir = TempDir::new().unwrap();

        // Create a structure where .rkat is at a different level than .git
        // git_root/
        //   .git/
        //   subproject/
        //     .rkat/
        //     src/
        let git_root = temp_dir.path().join("git_root");
        let git_dir = git_root.join(".git");
        let subproject = git_root.join("subproject");
        let rkat_dir = subproject.join(".rkat");
        let src_dir = subproject.join("src");

        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(&rkat_dir).unwrap();
        fs::create_dir_all(&src_dir).unwrap();

        // Starting from src/, should find .rkat in subproject/ (not .git in git_root/)
        let result = find_project_root(&src_dir);
        assert_eq!(result, Some(subproject));
    }

    #[test]
    fn test_ensure_rkat_dir_creates_directory() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        // Verify .rkat doesn't exist initially
        let rkat_path = project_root.join(".rkat");
        assert!(!rkat_path.exists());

        // Call ensure_rkat_dir
        let result = ensure_rkat_dir(project_root).unwrap();

        // Verify it was created
        assert!(rkat_path.exists());
        assert!(rkat_path.is_dir());
        assert_eq!(result, rkat_path);
    }

    #[test]
    fn test_ensure_rkat_dir_existing_directory() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        // Create .rkat directory manually
        let rkat_path = project_root.join(".rkat");
        fs::create_dir_all(&rkat_path).unwrap();

        // Add a file inside to verify we don't overwrite
        let test_file = rkat_path.join("config.toml");
        fs::write(&test_file, "test").unwrap();

        // Call ensure_rkat_dir
        let result = ensure_rkat_dir(project_root).unwrap();

        // Verify directory still exists and file is preserved
        assert!(rkat_path.exists());
        assert!(test_file.exists());
        assert_eq!(result, rkat_path);
    }

    #[test]
    fn test_find_ancestor_with_at_start_dir() {
        let temp_dir = TempDir::new().unwrap();

        // Create .rkat at the start directory itself
        let project_root = temp_dir.path().join("project");
        let rkat_dir = project_root.join(".rkat");
        fs::create_dir_all(&rkat_dir).unwrap();

        // Starting from project root should find it immediately
        let result = find_project_root(&project_root);
        assert_eq!(result, Some(project_root));
    }
}
