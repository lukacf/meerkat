//! Baseline runtime configuration and module bootstrapping.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

pub const MEERKAT_REPO_ENV: &str = "MEERKAT_REPO";
pub const REQUIRED_MEERKAT_SYMBOLS: &[&str] = &[
    "MobEventRouter",
    "send_message(id, msg)",
    "subscribe_agent_events(id)",
    "subscribe_all_agent_events()",
    "SpawnPolicy trait",
    "respawn(id, msg)",
    "AttributedEvent",
    "Roster::session_id(id)",
    "Roster::find_by_label(k, v)",
    "SessionBuildOptions.app_context",
    "SessionBuildOptions.additional_instructions",
    "CreateSessionRequest.labels",
    "RosterEntry.labels",
    "SpawnMemberSpec.resume_session_id",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineVerificationReport {
    pub repo_root: PathBuf,
    pub missing_symbols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineVerificationError {
    RepoNotConfigured { env_var: &'static str },
    RepoMissing(PathBuf),
    RepoUnreadable(PathBuf),
    MissingSymbols(BaselineVerificationReport),
}

impl std::fmt::Display for BaselineVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RepoNotConfigured { env_var } => write!(
                f,
                "Meerkat repository is not configured; pass an explicit repository path or set {env_var}"
            ),
            Self::RepoMissing(path) => write!(f, "repo missing: {}", path.display()),
            Self::RepoUnreadable(path) => write!(f, "repo unreadable: {}", path.display()),
            Self::MissingSymbols(report) => {
                write!(
                    f,
                    "missing symbols in {}: {}",
                    report.repo_root.display(),
                    report.missing_symbols.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for BaselineVerificationError {}

pub fn verify_meerkat_baseline_symbols(
    explicit_repo_root: Option<&Path>,
) -> Result<BaselineVerificationReport, BaselineVerificationError> {
    let configured_repo_root = std::env::var_os(MEERKAT_REPO_ENV);
    let repo_root = resolve_meerkat_repo_root(explicit_repo_root, configured_repo_root.as_deref())?;

    if !repo_root.exists() {
        return Err(BaselineVerificationError::RepoMissing(repo_root));
    }
    if !repo_root.is_dir() {
        return Err(BaselineVerificationError::RepoUnreadable(repo_root));
    }

    let mut missing: Vec<String> = REQUIRED_MEERKAT_SYMBOLS
        .iter()
        .map(std::string::ToString::to_string)
        .collect();

    scan_dir_for_symbols(&repo_root, &mut missing)
        .map_err(|_| BaselineVerificationError::RepoUnreadable(repo_root.clone()))?;

    let report = BaselineVerificationReport {
        repo_root,
        missing_symbols: missing,
    };

    if report.missing_symbols.is_empty() {
        Ok(report)
    } else {
        Err(BaselineVerificationError::MissingSymbols(report))
    }
}

fn resolve_meerkat_repo_root(
    explicit_repo_root: Option<&Path>,
    configured_repo_root: Option<&OsStr>,
) -> Result<PathBuf, BaselineVerificationError> {
    let configured_path = match explicit_repo_root {
        Some(path) if !path.as_os_str().is_empty() => path,
        Some(_) => {
            return Err(BaselineVerificationError::RepoNotConfigured {
                env_var: MEERKAT_REPO_ENV,
            });
        }
        None => configured_repo_root
            .map(Path::new)
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or(BaselineVerificationError::RepoNotConfigured {
                env_var: MEERKAT_REPO_ENV,
            })?,
    };

    Ok(configured_path.to_path_buf())
}

fn scan_dir_for_symbols(path: &Path, missing: &mut Vec<String>) -> std::io::Result<()> {
    if missing.is_empty() {
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if should_skip_dir(&entry_path) {
                continue;
            }
            scan_dir_for_symbols(&entry_path, missing)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if should_skip_file(&entry_path) {
            continue;
        }

        let content = match fs::read_to_string(&entry_path) {
            Ok(content) => content,
            Err(_) => continue,
        };

        missing.retain(|symbol| !contains_symbol(&content, symbol));
        if missing.is_empty() {
            return Ok(());
        }
    }

    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | "node_modules" | ".next" | ".turbo")
    )
}

fn should_skip_file(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "pdf" | "wasm" | "lock"
    )
}

fn contains_symbol(content: &str, symbol: &str) -> bool {
    if content.contains(symbol) {
        return true;
    }

    match symbol {
        "MobEventRouter" => content.contains("MobEventRouter"),
        "subscribe_agent_events(id)" => content.contains("subscribe_agent_events"),
        "subscribe_all_agent_events()" => content.contains("subscribe_all_agent_events"),
        "SpawnPolicy trait" => content.contains("trait SpawnPolicy"),
        "SessionBuildOptions.app_context" => {
            content.contains("SessionBuildOptions")
                && (content.contains("app_context") || content.contains(".app_context"))
        }
        "SessionBuildOptions.additional_instructions" => {
            content.contains("SessionBuildOptions")
                && (content.contains("additional_instructions")
                    || content.contains(".additional_instructions"))
        }
        "CreateSessionRequest.labels" => {
            content.contains("CreateSessionRequest")
                && (content.contains("labels") || content.contains(".labels"))
        }
        "RosterEntry.labels" => {
            content.contains("RosterEntry")
                && (content.contains("labels") || content.contains(".labels"))
        }
        "SpawnMemberSpec.resume_session_id" => {
            content.contains("SpawnMemberSpec")
                && (content.contains("resume_session_id") || content.contains(".resume_session_id"))
        }
        "Roster::session_id(id)" => {
            content.contains("Roster::session_id")
                || content.contains("fn session_id")
                || content.contains(".session_id(")
        }
        "Roster::find_by_label(k, v)" => {
            content.contains("Roster::find_by_label")
                || content.contains("fn find_by_label")
                || content.contains(".find_by_label(")
        }
        "send_message(id, msg)" => content.contains("send_message"),
        "respawn(id, msg)" => content.contains("respawn("),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_root_requires_explicit_path_or_environment_configuration() {
        assert_eq!(
            resolve_meerkat_repo_root(None, None),
            Err(BaselineVerificationError::RepoNotConfigured {
                env_var: MEERKAT_REPO_ENV,
            })
        );
        assert_eq!(
            resolve_meerkat_repo_root(Some(Path::new("")), Some(OsStr::new("/configured"))),
            Err(BaselineVerificationError::RepoNotConfigured {
                env_var: MEERKAT_REPO_ENV,
            })
        );
    }

    #[test]
    fn repo_root_prefers_an_explicit_path_and_otherwise_uses_environment_configuration() {
        assert_eq!(
            resolve_meerkat_repo_root(
                Some(Path::new("/explicit")),
                Some(OsStr::new("/configured"))
            ),
            Ok(PathBuf::from("/explicit"))
        );
        assert_eq!(
            resolve_meerkat_repo_root(None, Some(OsStr::new("/configured"))),
            Ok(PathBuf::from("/configured"))
        );
    }

    #[test]
    fn explicit_repo_root_is_verified_without_a_machine_specific_default()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = tempfile::tempdir()?;
        fs::write(
            repo.path().join("baseline-symbols.rs"),
            REQUIRED_MEERKAT_SYMBOLS.join("\n"),
        )?;

        let report = verify_meerkat_baseline_symbols(Some(repo.path()))?;
        assert_eq!(report.repo_root, repo.path());
        assert!(report.missing_symbols.is_empty());
        Ok(())
    }
}
