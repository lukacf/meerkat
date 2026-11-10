//! The file a write of a configuration document creates.
//!
//! A config document (`config.toml`, `mcp.toml`) can hold credential
//! material: an auth profile's inline secret, a command source's
//! environment, an MCP server's literal arguments, environment and headers.
//! On Unix the engine therefore writes such a document, and the generation
//! sidecar beside one, for its owner, whatever the process umask:
//!
//! - 0600 in an ordinary directory;
//! - 0640 in a directory with the setgid bit, which says the directory is
//!   shared through its group on purpose: the directory gives the file its
//!   group, and that group can read it. It is never group-writable and
//!   never readable by others;
//! - never a permission the document lacked: a rewrite keeps only the bits
//!   of that mode the existing document has, so a document its owner made
//!   0400 stays 0400.
//!
//! The mode is given in the `open` that creates the write's temporary file,
//! so the file never exists with a wider one and holds no byte before it has
//! it; the rename then publishes that file. No directory's mode is changed.
//! Other platforms create the file as they create any file.

use std::path::Path;

/// Create `temporary`, which must not exist, as the file a rename will
/// publish as `document`, with the mode of the module's rule (Unix).
pub(crate) async fn create_replacement(
    temporary: &Path,
    document: &Path,
) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    let mode = unix::replacement_mode(document).await;
    #[cfg(unix)]
    options.mode(mode);
    let file = options.open(temporary).await?;
    // The umask may have taken the group's read bit from the open's mode:
    // the mode of a shared directory's document is exact whatever the umask,
    // as the admission fence's is. The file is still empty.
    #[cfg(unix)]
    if mode & unix::GROUP_READ != 0 {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .await?;
    }
    Ok(file)
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    /// A document only its owner reads and writes.
    const OWNER_ONLY: u32 = 0o600;
    /// The group's read bit, which a shared directory's document also has.
    pub(super) const GROUP_READ: u32 = 0o040;
    /// The setgid bit of a directory's mode.
    const SETGID: u32 = 0o2000;

    /// The mode of the file that replaces `document`: owner-only, with the
    /// group's read bit when the document's directory is setgid, and
    /// without any bit of those an existing document lacks.
    pub(super) async fn replacement_mode(document: &Path) -> u32 {
        let directory = match document.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let shared = tokio::fs::metadata(directory)
            .await
            .is_ok_and(|directory| directory.permissions().mode() & SETGID != 0);
        let mode = if shared {
            OWNER_ONLY | GROUP_READ
        } else {
            OWNER_ONLY
        };
        match tokio::fs::metadata(document).await {
            Ok(existing) => existing.permissions().mode() & mode,
            // No document to replace (or none that can be read): a new one.
            Err(_) => mode,
        }
    }
}
