//! Typed execution-placement metadata.
//!
//! Placement describes where shell/tool mechanics ran. It is observable
//! metadata, not semantic identity or authorization truth.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Stable, non-path placement identity facts.
///
/// Working directories and allowed roots are intentionally excluded. Paths are
/// execution metadata; callers must not use them as the semantic identity of a
/// session, run, mob member, or operation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPlacementIdentity {
    /// Stable runtime host identifier, when a trusted runtime owner supplied it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// Stable worktree identifier, when a trusted runtime owner supplied it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
}

/// Structured metadata for where a tool or shell command executed.
///
/// This type deliberately avoids policy enforcement. It records facts that were
/// already established by the owning shell/tool/runtime path.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPlacement {
    /// Stable runtime host identifier, when safely known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// Canonical working root used for execution, when safely known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_root: Option<PathBuf>,
    /// Canonical roots that bounded execution, when safely known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_roots: Vec<PathBuf>,
    /// Stable worktree identifier, when safely known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
}

impl ExecutionPlacement {
    /// Build placement metadata from already trusted execution facts.
    ///
    /// Paths must be absolute and must not contain lexical parent components.
    /// The owner that creates this metadata is responsible for supplying
    /// canonicalized paths when symlinks matter.
    pub fn new(
        host_id: Option<impl Into<String>>,
        working_root: Option<impl Into<PathBuf>>,
        allowed_roots: impl IntoIterator<Item = impl Into<PathBuf>>,
        worktree_id: Option<impl Into<String>>,
    ) -> Result<Self, PlacementError> {
        let host_id = validate_optional_id("host_id", host_id.map(Into::into))?;
        let worktree_id = validate_optional_id("worktree_id", worktree_id.map(Into::into))?;

        let working_root = working_root
            .map(Into::into)
            .map(|path| normalize_absolute_path("working_root", path))
            .transpose()?;

        let mut roots = Vec::new();
        for root in allowed_roots {
            let normalized = normalize_absolute_path("allowed_roots", root.into())?;
            if !roots.iter().any(|existing| existing == &normalized) {
                roots.push(normalized);
            }
        }
        roots.sort();

        if let Some(working_root) = working_root.as_ref()
            && !roots.is_empty()
            && !roots.iter().any(|root| working_root.starts_with(root))
        {
            return Err(PlacementError::WorkingRootOutsideAllowedRoots {
                working_root: working_root.clone(),
                allowed_roots: roots,
            });
        }

        Ok(Self {
            host_id,
            working_root,
            allowed_roots: roots,
            worktree_id,
        })
    }

    /// Return the non-path identity facts for this placement.
    pub fn identity(&self) -> ExecutionPlacementIdentity {
        ExecutionPlacementIdentity {
            host_id: self.host_id.clone(),
            worktree_id: self.worktree_id.clone(),
        }
    }
}

/// Errors raised while constructing placement metadata.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlacementError {
    /// A supplied id was empty or whitespace-only.
    #[error("{field} must not be empty")]
    EmptyId { field: &'static str },
    /// A supplied id contained characters outside the portable id alphabet.
    #[error("{field} contains unsupported characters: {value}")]
    InvalidId { field: &'static str, value: String },
    /// A placement path was relative.
    #[error("{field} must be absolute: {path}")]
    RelativePath { field: &'static str, path: PathBuf },
    /// A placement path contained a parent component.
    #[error("{field} must not contain parent components: {path}")]
    ParentPathComponent { field: &'static str, path: PathBuf },
    /// The working root was not bounded by any supplied allowed root.
    #[error("working_root {working_root} is outside allowed_roots")]
    WorkingRootOutsideAllowedRoots {
        working_root: PathBuf,
        allowed_roots: Vec<PathBuf>,
    },
}

fn validate_optional_id(
    field: &'static str,
    value: Option<String>,
) -> Result<Option<String>, PlacementError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(PlacementError::EmptyId { field });
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '@'))
    {
        return Err(PlacementError::InvalidId {
            field,
            value: value.clone(),
        });
    }
    Ok(Some(trimmed.to_string()))
}

fn normalize_absolute_path(
    field: &'static str,
    path: impl AsRef<Path>,
) -> Result<PathBuf, PlacementError> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return Err(PlacementError::RelativePath {
            field,
            path: path.to_path_buf(),
        });
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                return Err(PlacementError::ParentPathComponent {
                    field,
                    path: path.to_path_buf(),
                });
            }
        }
    }

    Ok(normalized)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn placement_normalizes_safe_absolute_paths() {
        let placement = ExecutionPlacement::new(
            Some("host-a"),
            Some("/tmp/project/./worktree"),
            ["/tmp/project", "/tmp/project"],
            Some("wt-main"),
        )
        .unwrap();

        assert_eq!(placement.host_id.as_deref(), Some("host-a"));
        assert_eq!(
            placement.working_root.as_deref(),
            Some(Path::new("/tmp/project/worktree"))
        );
        assert_eq!(placement.allowed_roots, vec![PathBuf::from("/tmp/project")]);
        assert_eq!(placement.worktree_id.as_deref(), Some("wt-main"));
    }

    #[test]
    fn placement_rejects_relative_paths() {
        let err = ExecutionPlacement::new(
            None::<String>,
            Some("relative/project"),
            ["/tmp/project"],
            None::<String>,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            PlacementError::RelativePath {
                field: "working_root",
                ..
            }
        ));
    }

    #[test]
    fn placement_rejects_parent_components() {
        let err = ExecutionPlacement::new(
            None::<String>,
            Some("/tmp/project/../other"),
            ["/tmp/project"],
            None::<String>,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            PlacementError::ParentPathComponent {
                field: "working_root",
                ..
            }
        ));
    }

    #[test]
    fn placement_rejects_unbounded_working_root() {
        let err = ExecutionPlacement::new(
            None::<String>,
            Some("/tmp/other"),
            ["/tmp/project"],
            None::<String>,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            PlacementError::WorkingRootOutsideAllowedRoots { .. }
        ));
    }

    #[test]
    fn placement_identity_excludes_paths() {
        let first = ExecutionPlacement::new(
            Some("host-a"),
            Some("/tmp/project-a"),
            ["/tmp"],
            Some("wt-main"),
        )
        .unwrap();
        let second = ExecutionPlacement::new(
            Some("host-a"),
            Some("/var/project-b"),
            ["/var"],
            Some("wt-main"),
        )
        .unwrap();

        assert_ne!(first.working_root, second.working_root);
        assert_eq!(first.identity(), second.identity());
    }

    #[test]
    fn placement_rejects_spoofed_ids() {
        let err = ExecutionPlacement::new(
            Some("host/a"),
            Some("/tmp/project"),
            ["/tmp"],
            None::<String>,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            PlacementError::InvalidId {
                field: "host_id",
                ..
            }
        ));
    }
}
