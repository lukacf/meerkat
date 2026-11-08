//! Durable evidence issued only by this exact retired presenter owner. This
//! preserves ordinary invocation history; it is not scoped consent withdrawal,
//! a native input receipt, effect evidence or permission to restore a session.
use super::*;
use meerkat_toolkit::host_lifetime::control::SelectedHostArtifact;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    scope: Scope,
    association: SelectedHostArtifact,
    session: SessionId,
    source: SelectedHostArtifact,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Scope {
    RetiredLocalPresenterDispatchesJoined,
}
#[derive(Clone)]
pub(crate) struct ClosedLocalApprovalsArtifact {
    artifact: SelectedHostArtifact,
    record: Record,
}
fn digest(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn read(selected: &SelectedHostArtifact) -> Result<Vec<u8>, ConnectError> {
    let bytes = crate::retained_artifact::read_regular_artifact(&selected.path)
        .map_err(ConnectError::InvalidInput)?;
    if digest(&bytes) != selected.sha256 {
        return Err(invalid("Local presenter source selection changed"));
    }
    Ok(bytes)
}
fn path(association: &SelectedHostArtifact) -> Result<PathBuf, ConnectError> {
    if association.sha256.len() != 64
        || !association
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("Invalid local presenter association digest"));
    }
    Ok(association
        .path
        .parent()
        .ok_or_else(|| invalid("Missing association directory"))?
        .join(format!(
            "local-presenter-closed-{}.json",
            association.sha256
        )))
}
impl ClosedLocalApprovals {
    pub(crate) fn retain_for_association(
        &self,
        association: &SelectedHostArtifact,
    ) -> Result<ClosedLocalApprovalsArtifact, ConnectError> {
        let check = || {
            let state = self.shared.state()?;
            if state.session.as_ref() != Some(&self.session)
                || !state.closed
                || state.unresolved_history
                || state.service_dispatches != 0
                || state
                    .attempt
                    .as_ref()
                    .is_some_and(|a| !a.settled_for_close())
            {
                return Err(invalid(
                    "The selected presenter is not closed with joined dispatches",
                ));
            }
            Ok(())
        };
        check()?;
        read(association)?;
        let source_raw = crate::retained_artifact::read_regular_artifact(&self.shared.store_path)
            .map_err(ConnectError::InvalidInput)?;
        let record = Record {
            schema_version: 1,
            scope: Scope::RetiredLocalPresenterDispatchesJoined,
            association: association.clone(),
            session: self.session.clone(),
            source: SelectedHostArtifact {
                path: self.shared.store_path.clone(),
                sha256: digest(&source_raw),
            },
        };
        let raw = toolkit_package::canonical_json(&record)
            .map_err(|e| ConnectError::native("encode local presenter disposition", e))?;
        check()?;
        let target = path(association)?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)
        {
            Ok(mut file) => file
                .write_all(&raw)
                .and_then(|_| file.sync_all())
                .map_err(|e| ConnectError::native("persist local presenter disposition", e))?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if crate::retained_artifact::read_regular_artifact(&target)
                    .map_err(ConnectError::InvalidInput)?
                    != raw
                {
                    return Err(invalid("Conflicting local presenter disposition"));
                }
            }
            Err(error) => {
                return Err(ConnectError::native(
                    "create local presenter disposition",
                    error,
                ));
            }
        }
        // Repeat BOTH barriers on exact-existing bytes too. A previous full
        // write with failed file sync is not made durable by directory sync.
        let mut durable = fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(&target)
            .map_err(|e| {
                ConnectError::native("open retained presenter disposition for durability", e)
            })?;
        let node = durable
            .metadata()
            .map_err(|e| ConnectError::native("inspect presenter disposition", e))?;
        if !node.is_file() {
            return Err(invalid("Presenter disposition is not a regular file"));
        }
        let same_node = || -> Result<(), ConnectError> {
            let current = fs::symlink_metadata(&target)
                .map_err(|e| ConnectError::native("recheck presenter disposition", e))?;
            if !current.is_file() || current.dev() != node.dev() || current.ino() != node.ino() {
                return Err(invalid("Presenter disposition identity changed"));
            }
            Ok(())
        };
        let mut actual = Vec::new();
        (&mut durable)
            .take(256 * 1024 + 1)
            .read_to_end(&mut actual)
            .map_err(|e| ConnectError::native("read presenter disposition for durability", e))?;
        if actual != raw {
            return Err(invalid(
                "Presenter disposition bytes changed before durability",
            ));
        }
        same_node()?;
        durable
            .sync_all()
            .map_err(|e| ConnectError::native("join presenter disposition file durability", e))?;
        fs::File::open(
            target
                .parent()
                .ok_or_else(|| invalid("Missing disposition parent"))?,
        )
        .and_then(|f| f.sync_all())
        .map_err(|e| {
            ConnectError::native("join local presenter disposition directory durability", e)
        })?;
        same_node()?;
        let observed = ClosedLocalApprovalsArtifact::read_for_association(
            association,
            &self.shared.store_path,
            &self.session,
        )?;
        if observed.artifact.sha256 != digest(&raw) {
            return Err(invalid("Local presenter disposition changed"));
        }
        check()?;
        Ok(observed)
    }
}
impl ClosedLocalApprovalsArtifact {
    pub(crate) fn read_for_association(
        association: &SelectedHostArtifact,
        source: &Path,
        session: &SessionId,
    ) -> Result<Self, ConnectError> {
        read(association)?;
        let target = path(association)?;
        let raw = crate::retained_artifact::read_regular_artifact(&target)
            .map_err(ConnectError::InvalidInput)?;
        let record: Record = serde_json::from_slice(&raw)
            .map_err(|e| ConnectError::native("read retired local presenter disposition", e))?;
        if record.schema_version != 1
            || record.scope != Scope::RetiredLocalPresenterDispatchesJoined
            || record.association != *association
            || record.source.path != source
            || &record.session != session
            || toolkit_package::canonical_json(&record)
                .map_err(|e| ConnectError::native("verify local presenter disposition", e))?
                != raw
        {
            return Err(invalid(
                "Local presenter disposition does not cover the selected association",
            ));
        }
        read(&record.source)?;
        Ok(Self {
            artifact: SelectedHostArtifact {
                path: target,
                sha256: digest(&raw),
            },
            record,
        })
    }
    pub(crate) fn artifact(&self) -> &SelectedHostArtifact {
        &self.artifact
    }
    pub(crate) fn check(&self) -> Result<(), ConnectError> {
        let current = Self::read_for_association(
            &self.record.association,
            &self.record.source.path,
            &self.record.session,
        )?;
        if current.artifact != self.artifact {
            return Err(invalid("Retired local presenter evidence changed"));
        }
        Ok(())
    }
}
