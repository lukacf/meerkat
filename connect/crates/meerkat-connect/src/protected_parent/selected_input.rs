//! One original sealed input, associated with one authenticated service slot.
//! Bytes and parsed metadata confer no install, grant, or runtime authority.
use super::{mapping, refused, transport};
use crate::ConnectError;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ops::Range,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

const HEADER_MAX: usize = 16384;
const PAYLOAD_MAX: usize = 16777216;
pub(super) const MAX_BYTES: usize = 12 + HEADER_MAX + PAYLOAD_MAX;

#[derive(Debug)]
pub(super) struct Identity {
    pub(super) service_id: String,
    pub(super) bundle_sha256: String,
}

pub(crate) struct SelectedInput {
    // Never duplicated, reopened, exported, or replaced by its digest.
    _fd: OwnedFd,
    bundle: Bundle,
    phase: AtomicU8, // 0 Unstaged, 1 Staging, 2 Staged, 3 Failed
    retired: AtomicBool,
}

pub(crate) struct Bundle {
    bytes: Vec<u8>,
    pub(crate) service_id: String,
    pub(crate) component: String,
    pub(crate) sha256: String,
    pub(crate) members: Vec<Member>,
}
pub(crate) struct Member {
    pub(crate) name: String,
    pub(crate) sha256: String,
    range: Range<usize>,
}

/// Observation of an actual completed copy, retained by its preparation owner.
/// This value is not a constructor for an input, owner, installation or Ready.
#[derive(Clone, Serialize)]
pub(crate) struct StagedSelectedInput {
    pub(crate) service_id: String,
    pub(crate) staged_id: String,
    pub(crate) profile: &'static str,
    pub(crate) component: String,
    pub(crate) package_sha256: String,
    pub(crate) selection_sha256: String,
    pub(crate) bundle_sha256: String,
}

impl SelectedInput {
    pub(super) fn adopt(
        fd: OwnedFd,
        others: &[BorrowedFd<'_>],
        identity: &Identity,
    ) -> Result<Arc<Self>, ConnectError> {
        transport::protect(fd.as_fd(), false).map_err(|_| refused())?;
        let mut descriptors = vec![fd.as_fd()];
        descriptors.extend_from_slice(others);
        transport::require_distinct(&descriptors).map_err(|_| refused())?;
        let bytes =
            transport::read_original_sealed(fd.as_fd(), MAX_BYTES).map_err(|_| refused())?;
        let bundle = Bundle::parse(bytes, identity)?;
        Ok(Arc::new(Self {
            _fd: fd,
            bundle,
            phase: AtomicU8::new(0),
            retired: AtomicBool::new(false),
        }))
    }
    pub(crate) fn service_id(&self) -> &str {
        &self.bundle.service_id
    }
    pub(crate) fn begin(&self) -> Result<(), ConnectError> {
        if self.retired.load(Ordering::Acquire)
            || self
                .phase
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(refused());
        }
        Ok(())
    }
    pub(crate) fn retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }
    pub(crate) fn retire(&self) {
        self.retired.store(true, Ordering::Release);
    }
    pub(crate) fn finish(&self, success: bool) -> Result<(), ConnectError> {
        let success = success && !self.retired();
        self.phase
            .compare_exchange(
                1,
                if success { 2 } else { 3 },
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| refused())?;
        if success { Ok(()) } else { Err(refused()) }
    }
    #[cfg(test)]
    pub(crate) fn failed_for_test(&self) -> bool {
        self.phase.load(Ordering::Acquire) == 3
    }

    pub(crate) fn bundle(&self) -> &Bundle {
        &self.bundle
    }
}

impl Bundle {
    fn parse(bytes: Vec<u8>, identity: &Identity) -> Result<Self, ConnectError> {
        if bytes.len() < 12
            || bytes.len() > MAX_BYTES
            || &bytes[..8] != b"MCSPKG01"
            || digest(&bytes) != identity.bundle_sha256
        {
            return Err(refused());
        }
        let length = u32::from_be_bytes(bytes[8..12].try_into().map_err(|_| refused())?) as usize;
        let payload = 12usize
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(refused)?;
        if length == 0 || length > HEADER_MAX || bytes.len() - payload > PAYLOAD_MAX {
            return Err(refused());
        }
        let value = mapping::bounded_json(&bytes[12..payload], HEADER_MAX, 8, 2048)
            .map_err(|_| refused())?;
        let root = fields(
            &value,
            &["schema", "service_id", "profile", "component", "members"],
        )?;
        if root["schema"] != "meerkat.connect.selected-input.v1" || root["profile"] != "python" {
            return Err(refused());
        }
        let service_id = id(&root["service_id"])?;
        if service_id != identity.service_id {
            return Err(refused());
        }
        let component = id(&root["component"])?;
        let rows = root["members"]
            .as_array()
            .filter(|v| (3..=64).contains(&v.len()))
            .ok_or_else(refused)?;
        let mut members = Vec::new();
        let mut names = BTreeSet::new();
        let mut previous: Option<&str> = None;
        let mut offset = payload;
        for value in rows {
            let row = fields(value, &["name", "size", "sha256"])?;
            let name = row["name"].as_str().ok_or_else(refused)?;
            if !member_name(name) || previous.is_some_and(|p| p >= name) {
                return Err(refused());
            }
            previous = Some(name);
            names.insert(name);
            let size = row["size"]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(refused)?;
            let end = offset
                .checked_add(size)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(refused)?;
            let sha256 = row["sha256"]
                .as_str()
                .filter(|s| is_digest(s))
                .ok_or_else(refused)?;
            if digest(&bytes[offset..end]) != sha256 {
                return Err(refused());
            }
            members.push(Member {
                name: name.into(),
                sha256: sha256.into(),
                range: offset..end,
            });
            offset = end;
        }
        if offset != bytes.len()
            || !["package.json", "activation.json", "artifact.whl"]
                .iter()
                .all(|n| names.contains(n))
        {
            return Err(refused());
        }
        Ok(Self {
            bytes,
            service_id,
            component,
            sha256: identity.bundle_sha256.clone(),
            members,
        })
    }
    pub(crate) fn bytes(&self, member: &Member) -> &[u8] {
        &self.bytes[member.range.clone()]
    }
    pub(crate) fn observation(
        &self,
        staged_id: String,
    ) -> Result<StagedSelectedInput, ConnectError> {
        let hash = |name| {
            self.members
                .iter()
                .find(|m| m.name == name)
                .map(|m| m.sha256.clone())
                .ok_or_else(refused)
        };
        Ok(StagedSelectedInput {
            service_id: self.service_id.clone(),
            staged_id,
            profile: "python",
            component: self.component.clone(),
            package_sha256: hash("package.json")?,
            selection_sha256: hash("activation.json")?,
            bundle_sha256: self.sha256.clone(),
        })
    }
}

fn fields<'a>(
    value: &'a serde_json::Value,
    expected: &[&str],
) -> Result<&'a serde_json::Map<String, serde_json::Value>, ConnectError> {
    value
        .as_object()
        .filter(|v| v.len() == expected.len() && v.keys().all(|k| expected.contains(&k.as_str())))
        .ok_or_else(refused)
}
fn id(value: &serde_json::Value) -> Result<String, ConnectError> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(refused)
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn member_name(value: &str) -> bool {
    if matches!(value, "package.json" | "activation.json" | "artifact.whl") {
        return true;
    }
    value.strip_prefix("closure/").is_some_and(|leaf| {
        !leaf.is_empty()
            && leaf.len() <= 128
            && leaf.as_bytes()[0].is_ascii_alphanumeric()
            && leaf
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    })
}

#[cfg(test)]
mod tests;

// Pure copy/retirement fixtures never pass or replace production root admission.
#[cfg(test)]
pub(crate) fn inert_test_input(service_id: &str) -> Arc<SelectedInput> {
    let (bytes, identity) = tests::fixture(service_id);
    Arc::new(SelectedInput {
        _fd: tempfile::tempfile().unwrap().into(),
        bundle: Bundle::parse(bytes, &identity).unwrap(),
        phase: AtomicU8::new(0),
        retired: AtomicBool::new(false),
    })
}
