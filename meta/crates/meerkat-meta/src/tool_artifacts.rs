//! Installed-artifact attribution for SDK callback tools.
//!
//! A host that installs extension artifacts (packages, plugin manifests)
//! names them to the gateway at its trusted bootstrap. A callback tool may
//! then name the artifact its handler came from; the reference is verified
//! against that set when the build assembles its callback tools, before any
//! of them is published, and a verified reference is carried on the tool's
//! provenance. The reference is attribution only: it never grants or refuses
//! a call, and a tool's name never selects or implies an artifact. It is not
//! proof of the executable bytes or the semantics of the handler that runs.
//! The installed set is trusted host configuration from the host's own
//! bootstrap; the field itself authenticates nothing. Only callback tools
//! are checked: native and MCP tools carry no artifact requirement.

use std::collections::BTreeMap;
use std::fmt;

use meerkat_core::ToolArtifactRef;
pub use meerkat_core::ToolArtifactRefusal;

/// The artifacts a host installed: artifact id to the digest it installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledArtifacts(BTreeMap<String, String>);

/// Why an installed-artifact set is not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstalledArtifactsError {
    /// The same artifact id is listed with two different digests.
    ConflictingDigest { artifact_id: String },
}

impl fmt::Display for InstalledArtifactsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConflictingDigest { artifact_id } => write!(
                f,
                "installed artifact `{artifact_id}` is listed with two different digests"
            ),
        }
    }
}

impl std::error::Error for InstalledArtifactsError {}

impl InstalledArtifacts {
    pub fn new(
        artifacts: impl IntoIterator<Item = ToolArtifactRef>,
    ) -> Result<Self, InstalledArtifactsError> {
        let mut installed = BTreeMap::new();
        for artifact in artifacts {
            match installed.get(artifact.artifact_id()) {
                Some(digest) if digest != artifact.digest() => {
                    return Err(InstalledArtifactsError::ConflictingDigest {
                        artifact_id: artifact.artifact_id().to_owned(),
                    });
                }
                Some(_) => {}
                None => {
                    installed.insert(
                        artifact.artifact_id().to_owned(),
                        artifact.digest().to_owned(),
                    );
                }
            }
        }
        Ok(Self(installed))
    }

    fn verify(&self, tool: &str, artifact: &ToolArtifactRef) -> Result<(), ToolArtifactRefusal> {
        match self.0.get(artifact.artifact_id()) {
            None => Err(ToolArtifactRefusal::UnknownArtifact {
                tool: tool.to_owned(),
                artifact_id: artifact.artifact_id().to_owned(),
            }),
            Some(installed) if installed != artifact.digest() => {
                Err(ToolArtifactRefusal::DigestMismatch {
                    tool: tool.to_owned(),
                    artifact_id: artifact.artifact_id().to_owned(),
                    declared: artifact.digest().to_owned(),
                    installed: installed.clone(),
                })
            }
            Some(_) => Ok(()),
        }
    }
}

/// Verify every callback tool of one build against the host's installed set,
/// before any of them is published: the whole set is checked first, so a
/// refused build publishes nothing. Two tools with one name are refused, so
/// a name never carries two different artifacts. Without a set and without
/// references, callback tools stay unassociated, as before.
pub fn verify_callback_artifacts<'a>(
    installed: Option<&InstalledArtifacts>,
    tools: impl IntoIterator<Item = (&'a str, Option<&'a ToolArtifactRef>)>,
) -> Result<(), ToolArtifactRefusal> {
    let tools: Vec<_> = tools.into_iter().collect();
    // Duplicates matter once artifacts are in play: a host that selected
    // installed artifacts, or a duplicated name that carries a reference.
    // A build with neither keeps its earlier behaviour.
    let mut seen: std::collections::BTreeMap<&str, bool> = std::collections::BTreeMap::new();
    for (tool, artifact) in &tools {
        let has_artifact = artifact.is_some();
        if let Some(previous) = seen.insert(*tool, has_artifact)
            && (installed.is_some() || previous || has_artifact)
        {
            return Err(ToolArtifactRefusal::DuplicateTool {
                tool: (*tool).to_owned(),
            });
        }
    }
    for (tool, artifact) in tools {
        match (installed, artifact) {
            (None, None) => {}
            (None, Some(artifact)) => {
                return Err(ToolArtifactRefusal::NoInstalledArtifacts {
                    tool: tool.to_owned(),
                    artifact_id: artifact.artifact_id().to_owned(),
                });
            }
            (Some(_), None) => {
                return Err(ToolArtifactRefusal::Unassociated {
                    tool: tool.to_owned(),
                });
            }
            (Some(installed), Some(artifact)) => installed.verify(tool, artifact)?,
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn artifact(id: &str, digest: &str) -> ToolArtifactRef {
        ToolArtifactRef::new(id, digest).unwrap()
    }

    fn installed() -> InstalledArtifacts {
        InstalledArtifacts::new([artifact("ext.weather", "sha256:aa")]).unwrap()
    }

    #[test]
    fn no_set_and_no_reference_stays_unassociated() {
        assert_eq!(verify_callback_artifacts(None, [("plain", None)]), Ok(()));
    }

    #[test]
    fn a_matching_reference_verifies() {
        let reference = artifact("ext.weather", "sha256:aa");
        assert_eq!(
            verify_callback_artifacts(Some(&installed()), [("forecast", Some(&reference))]),
            Ok(())
        );
    }

    #[test]
    fn unknown_mismatched_unassociated_and_unverifiable_references_are_refused() {
        let unknown = artifact("ext.other", "sha256:aa");
        assert_eq!(
            verify_callback_artifacts(Some(&installed()), [("forecast", Some(&unknown))]),
            Err(ToolArtifactRefusal::UnknownArtifact {
                tool: "forecast".into(),
                artifact_id: "ext.other".into(),
            })
        );
        let mismatched = artifact("ext.weather", "sha256:bb");
        assert_eq!(
            verify_callback_artifacts(Some(&installed()), [("forecast", Some(&mismatched))]),
            Err(ToolArtifactRefusal::DigestMismatch {
                tool: "forecast".into(),
                artifact_id: "ext.weather".into(),
                declared: "sha256:bb".into(),
                installed: "sha256:aa".into(),
            })
        );
        assert_eq!(
            verify_callback_artifacts(Some(&installed()), [("plain", None)]),
            Err(ToolArtifactRefusal::Unassociated {
                tool: "plain".into()
            })
        );
        let reference = artifact("ext.weather", "sha256:aa");
        assert_eq!(
            verify_callback_artifacts(None, [("forecast", Some(&reference))]),
            Err(ToolArtifactRefusal::NoInstalledArtifacts {
                tool: "forecast".into(),
                artifact_id: "ext.weather".into(),
            })
        );
    }

    #[test]
    fn two_callback_tools_with_one_name_are_refused() {
        let reference = artifact("ext.weather", "sha256:aa");
        assert_eq!(
            verify_callback_artifacts(
                Some(&installed()),
                [("forecast", Some(&reference)), ("forecast", None)]
            ),
            Err(ToolArtifactRefusal::DuplicateTool {
                tool: "forecast".into()
            })
        );
        assert_eq!(
            verify_callback_artifacts(None, [("plain", Some(&reference)), ("plain", None)]),
            Err(ToolArtifactRefusal::DuplicateTool {
                tool: "plain".into()
            })
        );
        // Without artifacts in play a repeated name keeps its earlier
        // behaviour.
        assert_eq!(
            verify_callback_artifacts(None, [("plain", None), ("plain", None)]),
            Ok(())
        );
    }

    #[test]
    fn the_identity_runtime_reports_the_refusal_typed() {
        let refusal = ToolArtifactRefusal::Unassociated {
            tool: "plain".into(),
        };
        let error =
            crate::identity_first::IdentityRuntimeError::ToolArtifactRefused(Box::new(refusal));
        let data = error.structured_data().expect("typed error data");
        assert_eq!(data["kind"], ToolArtifactRefusal::KIND);
        assert_eq!(data["reason"], "unassociated");
        assert_eq!(data["tool"], "plain");
    }

    #[test]
    fn a_set_listing_one_id_with_two_digests_is_refused() {
        assert_eq!(
            InstalledArtifacts::new([artifact("ext", "d1"), artifact("ext", "d2")]),
            Err(InstalledArtifactsError::ConflictingDigest {
                artifact_id: "ext".into()
            })
        );
        assert!(InstalledArtifacts::new([artifact("ext", "d1"), artifact("ext", "d1")]).is_ok());
    }
}
