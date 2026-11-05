//! Whether a file is tracked by the git repository around it, read from the
//! repository's index file (no `git` process is started).
//!
//! The realm rules file (`<realm>/tui.toml`) lives inside the workspace. If
//! it is tracked it arrived with the repository, so only its deny rules
//! apply ([`super::rules::RealmTrust::Tracked`]). Callers treat every `Err`
//! as tracked: anything this reader does not understand fails closed.
//!
//! Supported: index versions 2–4, SHA-1 and SHA-256 object formats, linked
//! work trees (`.git` files) and sparse-index directory entries. A split
//! index (`link` extension) keeps entries in a second file and is reported
//! as an error.

use std::path::{Path, PathBuf};

/// Size of the fixed part of an index entry before the object id
/// (ctime, mtime, dev, ino, mode, uid, gid, size: ten 32-bit fields).
const STAT_BYTES: usize = 40;
/// Extended-flags bit of an entry's flags (index version 3 and later).
const EXTENDED_FLAG: u16 = 0x4000;

/// Whether `path` is tracked by the git repository that contains
/// `workspace`.
///
/// `Ok(false)` only when that is known: the file does not exist, lies
/// outside the workspace or the repository's work tree, the workspace is
/// not in a repository, or the repository has no index.
///
/// # Errors
///
/// A description of what could not be read or understood.
pub fn is_tracked(workspace: &Path, path: &Path) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let (Ok(workspace), Ok(path)) = (workspace.canonicalize(), path.canonicalize()) else {
        return Err("cannot resolve the rules file or workspace path".to_string());
    };
    if !path.starts_with(&workspace) {
        return Ok(false);
    }
    let Some((work_tree, dot_git)) = find_dot_git(&workspace) else {
        return Ok(false);
    };
    let Ok(relative) = path.strip_prefix(&work_tree) else {
        return Ok(false);
    };
    let relative = index_path(relative)?;
    let git_dir = git_dir(&work_tree, &dot_git)?;
    let index = match std::fs::read(git_dir.join("index")) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(format!("cannot read the git index: {err}")),
    };
    let hash_len = hash_len(&common_dir(&git_dir))?;
    index_contains(&index, hash_len, &relative)
}

/// The nearest ancestor of `start` (itself included) holding `.git`, and
/// that `.git` path.
fn find_dot_git(start: &Path) -> Option<(PathBuf, PathBuf)> {
    start.ancestors().find_map(|dir| {
        let dot_git = dir.join(".git");
        dot_git
            .symlink_metadata()
            .is_ok()
            .then(|| (dir.to_path_buf(), dot_git))
    })
}

/// The git directory of a work tree: `.git` itself, or the directory a
/// `.git` file points to (`gitdir: …`).
fn git_dir(work_tree: &Path, dot_git: &Path) -> Result<PathBuf, String> {
    if dot_git.is_dir() {
        return Ok(dot_git.to_path_buf());
    }
    let pointer =
        std::fs::read_to_string(dot_git).map_err(|err| format!("cannot read .git: {err}"))?;
    let target = pointer
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .ok_or_else(|| "unrecognized .git file".to_string())?;
    let target = PathBuf::from(target);
    Ok(if target.is_absolute() {
        target
    } else {
        work_tree.join(target)
    })
}

/// The directory holding the repository's shared `config` (`commondir` of
/// a linked work tree's git directory, else the git directory itself).
fn common_dir(git_dir: &Path) -> PathBuf {
    match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => {
            let common = PathBuf::from(common.trim());
            if common.is_absolute() {
                common
            } else {
                git_dir.join(common)
            }
        }
        Err(_) => git_dir.to_path_buf(),
    }
}

/// Object id length of the repository: 32 bytes with
/// `extensions.objectformat = sha256`, else 20.
fn hash_len(common_dir: &Path) -> Result<usize, String> {
    let config = match std::fs::read_to_string(common_dir.join("config")) {
        Ok(config) => config,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(20),
        Err(err) => return Err(format!("cannot read the git config: {err}")),
    };
    let sha256 = config.lines().any(|line| {
        line.split_once('=').is_some_and(|(key, value)| {
            key.trim().eq_ignore_ascii_case("objectformat")
                && value.trim().eq_ignore_ascii_case("sha256")
        })
    });
    Ok(if sha256 { 32 } else { 20 })
}

/// A relative path as the index spells it: `/`-separated UTF-8 bytes.
fn index_path(relative: &Path) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (i, component) in relative.components().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err("unexpected path component".to_string());
        };
        if i > 0 {
            out.push(b'/');
        }
        let name = name
            .to_str()
            .ok_or_else(|| "the rules file path is not UTF-8".to_string())?;
        out.extend_from_slice(name.as_bytes());
    }
    Ok(out)
}

/// A little cursor over the index bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| "truncated git index".to_string())?;
        let slice = self.bytes.get(self.at..end).unwrap_or_default();
        self.at = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([
            b.first().copied().unwrap_or_default(),
            b.get(1).copied().unwrap_or_default(),
        ]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        let mut word = [0u8; 4];
        word.copy_from_slice(b);
        Ok(u32::from_be_bytes(word))
    }

    /// Bytes up to (not including) the next NUL, consuming the NUL.
    fn until_nul(&mut self) -> Result<&'a [u8], String> {
        let rest = self.bytes.get(self.at..).unwrap_or_default();
        let len = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| "unterminated path in git index".to_string())?;
        let name = self.take(len)?;
        self.take(1)?;
        Ok(name)
    }

    /// Git's offset varint (index version 4 path prefix lengths).
    fn varint(&mut self) -> Result<usize, String> {
        let mut byte = self.take(1)?.first().copied().unwrap_or_default();
        let mut value = usize::from(byte & 0x7f);
        while byte & 0x80 != 0 {
            byte = self.take(1)?.first().copied().unwrap_or_default();
            value = value
                .checked_add(1)
                .and_then(|v| v.checked_mul(128))
                .and_then(|v| v.checked_add(usize::from(byte & 0x7f)))
                .ok_or_else(|| "bad varint in git index".to_string())?;
        }
        Ok(value)
    }
}

/// Whether the index lists `relative` (or a sparse directory holding it).
fn index_contains(index: &[u8], hash_len: usize, relative: &[u8]) -> Result<bool, String> {
    let mut reader = Reader {
        bytes: index,
        at: 0,
    };
    if reader.take(4)? != b"DIRC" {
        return Err("not a git index".to_string());
    }
    let version = reader.u32()?;
    if !(2..=4).contains(&version) {
        return Err(format!("unsupported git index version {version}"));
    }
    let count = reader.u32()?;
    let mut previous: Vec<u8> = Vec::new();
    let mut found = false;
    for _ in 0..count {
        let start = reader.at;
        reader.take(STAT_BYTES)?;
        reader.take(hash_len)?;
        let flags = reader.u16()?;
        if version >= 3 && flags & EXTENDED_FLAG != 0 {
            reader.u16()?;
        }
        let name = if version == 4 {
            let strip = reader.varint()?;
            let keep = previous
                .len()
                .checked_sub(strip)
                .ok_or_else(|| "bad path prefix in git index".to_string())?;
            let mut name = previous.get(..keep).unwrap_or_default().to_vec();
            name.extend_from_slice(reader.until_nul()?);
            name
        } else {
            let name = reader.until_nul()?.to_vec();
            // Entries are NUL-padded to a multiple of eight bytes.
            let len = reader.at - start;
            let padded = len.div_ceil(8) * 8;
            reader.take(padded - len)?;
            name
        };
        found |= name == relative || (name.last() == Some(&b'/') && relative.starts_with(&name));
        previous = name;
    }
    if has_extension(&reader, hash_len, b"link")? {
        return Err("split git indexes are not supported".to_string());
    }
    Ok(found)
}

/// Whether the extensions after the entries include `signature`.
fn has_extension(
    reader: &Reader<'_>,
    hash_len: usize,
    signature: &[u8; 4],
) -> Result<bool, String> {
    let end = reader.bytes.len().saturating_sub(hash_len);
    let mut at = reader.at;
    while at.saturating_add(8) <= end {
        let header = reader.bytes.get(at..at + 8).unwrap_or_default();
        let (name, size) = header.split_at(4);
        if name == signature {
            return Ok(true);
        }
        let mut word = [0u8; 4];
        word.copy_from_slice(size);
        let size = usize::try_from(u32::from_be_bytes(word))
            .map_err(|_| "bad extension size in git index".to_string())?;
        at = at
            .checked_add(8)
            .and_then(|a| a.checked_add(size))
            .ok_or_else(|| "bad extension size in git index".to_string())?;
    }
    Ok(false)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// An index of `version` listing `paths` (sorted), SHA-1 ids.
    fn index(version: u32, paths: &[&str], extension: Option<&[u8; 4]>) -> Vec<u8> {
        let mut out = b"DIRC".to_vec();
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&u32::try_from(paths.len()).unwrap().to_be_bytes());
        let mut previous = String::new();
        for path in paths {
            let start = out.len();
            out.extend_from_slice(&[0u8; STAT_BYTES]);
            out.extend_from_slice(&[0xab; 20]);
            let flags = u16::try_from(path.len().min(0xfff)).unwrap();
            out.extend_from_slice(&flags.to_be_bytes());
            if version == 4 {
                let common = previous
                    .bytes()
                    .zip(path.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                let strip = previous.len() - common;
                assert!(strip < 128, "test varints are one byte");
                out.push(u8::try_from(strip).unwrap());
                out.extend_from_slice(&path.as_bytes()[common..]);
                out.push(0);
            } else {
                out.extend_from_slice(path.as_bytes());
                out.push(0);
                while !(out.len() - start).is_multiple_of(8) {
                    out.push(0);
                }
            }
            previous = (*path).to_string();
        }
        if let Some(signature) = extension {
            out.extend_from_slice(signature);
            out.extend_from_slice(&4u32.to_be_bytes());
            out.extend_from_slice(&[0; 4]);
        }
        out.extend_from_slice(&[0; 20]);
        out
    }

    #[test]
    fn finds_paths_in_every_version() {
        let paths = [".rkat/tui/tui.toml", "src/lib.rs", "src/main.rs"];
        for version in [2, 3, 4] {
            let bytes = index(version, &paths, None);
            for path in paths {
                assert!(
                    index_contains(&bytes, 20, path.as_bytes()).unwrap(),
                    "v{version} {path}"
                );
            }
            assert!(!index_contains(&bytes, 20, b"src/other.rs").unwrap());
            assert!(!index_contains(&bytes, 20, b"src").unwrap());
        }
    }

    #[test]
    fn sparse_directories_cover_their_files() {
        let bytes = index(2, &[".rkat/", "src/lib.rs"], None);
        assert!(index_contains(&bytes, 20, b".rkat/tui/tui.toml").unwrap());
    }

    #[test]
    fn unknown_formats_fail_closed() {
        assert!(index_contains(b"JUNK", 20, b"x").is_err());
        assert!(index_contains(&index(2, &["a"], None)[..30], 20, b"a").is_err());
        assert!(index_contains(&index(2, &["a"], Some(b"link")), 20, b"a").is_err());
        assert!(index_contains(&index(2, &["a"], Some(b"TREE")), 20, b"a").unwrap());
        let mut v5 = index(2, &["a"], None);
        v5[7] = 5;
        assert!(index_contains(&v5, 20, b"a").is_err());
    }

    #[test]
    fn repository_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let rules = root.join(".rkat/tui/tui.toml");
        std::fs::create_dir_all(rules.parent().unwrap()).unwrap();
        std::fs::write(&rules, "[auto]\n").unwrap();
        // Not a repository.
        assert!(!is_tracked(&root, &rules).unwrap());
        // A repository without an index.
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(!is_tracked(&root, &rules).unwrap());
        // Untracked, then tracked.
        std::fs::write(root.join(".git/index"), index(2, &["src/lib.rs"], None)).unwrap();
        assert!(!is_tracked(&root, &rules).unwrap());
        std::fs::write(
            root.join(".git/index"),
            index(2, &[".rkat/tui/tui.toml"], None),
        )
        .unwrap();
        assert!(is_tracked(&root, &rules).unwrap());
        // A file that does not exist is never tracked.
        assert!(!is_tracked(&root, &root.join("missing.toml")).unwrap());
    }

    #[test]
    fn linked_work_trees_read_their_own_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main_git = root.join("main/.git");
        let wt_git = main_git.join("worktrees/wt");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        std::fs::write(main_git.join("config"), "[core]\n").unwrap();
        let wt = root.join("wt");
        std::fs::create_dir_all(wt.join("sub")).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        std::fs::write(wt.join("sub/tui.toml"), "").unwrap();
        std::fs::write(wt_git.join("index"), index(3, &["sub/tui.toml"], None)).unwrap();
        // The workspace is a subdirectory of the work tree.
        assert!(is_tracked(&wt.join("sub"), &wt.join("sub/tui.toml")).unwrap());
    }

    #[test]
    fn sha256_repositories_use_long_ids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config"),
            "[extensions]\n\tobjectformat = sha256\n",
        )
        .unwrap();
        assert_eq!(hash_len(dir.path()).unwrap(), 32);
        assert_eq!(hash_len(&dir.path().join("none")).unwrap(), 20);
    }
}
