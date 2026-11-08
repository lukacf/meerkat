//! A separate, narrowly scoped physical constituent. No decoder here grants
//! native finalization, zero effect for other resources, or G readmission.
use super::*;
use meerkat_toolkit::host_lifetime::control::SelectedHostArtifact;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct AbandonedPhysicalDisposition {
    artifact: SelectedHostArtifact,
}
impl AbandonedPhysicalDisposition {
    pub fn artifact(&self) -> &SelectedHostArtifact {
        &self.artifact
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryPin {
    entry: String,
    request_sha256: String,
    decision_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinedSnapshot {
    schema_version: u32,
    scope: String,
    controller: String,
    lifetime_sha256: String,
    owner_sha256: String,
    entries: Vec<EntryPin>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CensusOwner {
    schema_version: u32,
    scope: String,
    controller: String,
    lifetime_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryRequest {
    entry: String,
    request: serde_json::Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryDecision {
    entry: String,
    request_sha256: String,
    decision: String,
}
#[derive(Serialize)]
struct Disposition<'a> {
    schema_version: u32,
    scope: &'static str,
    owner: &'a str,
    lifetime_sha256: String,
    snapshot_sha256: String,
    member: &'a meerkat_core::MobMemberBinding,
    session: &'a str,
    peer: usize,
}

fn census(
    directory: &Path,
    identity: &str,
    lifetime: &[u8],
    raw: &[u8],
) -> Result<(), ConnectError> {
    let value: JoinedSnapshot = serde_json::from_slice(raw).map_err(|_| unavailable())?;
    if value.schema_version != 1
        || value.scope != "joined_shared_controller_zero_effect"
        || value.lifetime_sha256 != digest(lifetime)
        || !valid_digest(&value.controller)
    {
        return Err(unavailable());
    }
    let entries = directory.join(format!("shared-entries-{identity}"));
    let metadata = fs::symlink_metadata(&entries).map_err(|_| unavailable())?;
    if !metadata.is_dir()
        || mode(&metadata) & 0o077 != 0
        || entries.canonicalize().map_err(|_| unavailable())? != entries
    {
        return Err(unavailable());
    }
    let owner = lifetime::read(&entries.join("owner.json"))?;
    let header: CensusOwner = serde_json::from_slice(&owner).map_err(|_| unavailable())?;
    if digest(&owner) != value.owner_sha256
        || header.schema_version != 1
        || header.scope != "shared_controller_entry_census"
        || header.controller != value.controller
        || header.lifetime_sha256 != value.lifetime_sha256
    {
        return Err(unavailable());
    }
    let mut expected = BTreeSet::from(["owner.json".to_owned()]);
    let mut last = None;
    for pin in value.entries {
        if !valid_digest(&pin.entry) || last.as_ref().is_some_and(|prior| prior >= &pin.entry) {
            return Err(unavailable());
        }
        last = Some(pin.entry.clone());
        let request_name = format!("{}.request.json", pin.entry);
        let decision_name = format!("{}.decision.json", pin.entry);
        let request = lifetime::read(&entries.join(&request_name))?;
        let decision = lifetime::read(&entries.join(&decision_name))?;
        let row: EntryRequest = serde_json::from_slice(&request).map_err(|_| unavailable())?;
        let settled: EntryDecision =
            serde_json::from_slice(&decision).map_err(|_| unavailable())?;
        if digest(&request) != pin.request_sha256
            || digest(&decision) != pin.decision_sha256
            || row.entry != pin.entry
            || !row.request.is_object()
            || settled.entry != pin.entry
            || settled.request_sha256 != pin.request_sha256
            || settled.decision != "not_entered"
        {
            return Err(unavailable());
        }
        expected.insert(request_name);
        expected.insert(decision_name);
    }
    let actual = fs::read_dir(&entries)
        .map_err(|_| unavailable())?
        .map(|entry| {
            entry
                .map_err(|_| unavailable())?
                .file_name()
                .into_string()
                .map_err(|_| unavailable())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if actual != expected {
        return Err(unavailable());
    }
    Ok(())
}

impl SharedSetupOwner {
    /// Explicitly select physical abandonment after actual native peer close.
    /// The same retained close task joins all native attempts, entry settlement,
    /// Controller workers and the actual child. No cold PID inference or caller
    /// outcome can construct this reference. Entered/unknown work returns None.
    pub fn close_abandoned_for_recovery(
        self,
        context: Arc<SharedSetupCallContext>,
    ) -> impl Future<Output = Result<Option<AbandonedPhysicalDisposition>, ConnectError>> + Send + 'static
    {
        let entries = self.entries.clone();
        let selected = Arc::new(std::sync::OnceLock::new());
        let produced = selected.clone();
        let retained = entries.retain_resource_close(async move {
            let registration = if Arc::ptr_eq(&context.peer.channel, &self.channel)
                && Arc::ptr_eq(&context.peer.entries, &self.entries)
            {
                context.peer.attempt.closed_registration()
            } else {
                None
            };
            let directory = self.artifact_directory.clone();
            let identity = self.close_identity.clone();
            let expected_lifetime = self.lifetime_sha256.clone();
            let peer = context.peer.index;
            // Capture exact pre-spawn bytes before dropping the live owner.
            // Missing historical identity is unresolved, never reconstructed.
            let lifetime = identity
                .as_deref()
                .map(|id| lifetime::PhysicalLifetime::selected(&directory, id));
            self.close_owned().await?;
            let (Some(identity), Some(expected), Some(Ok((_, lifetime))), Some((member, session))) =
                (identity, expected_lifetime, lifetime, registration)
            else {
                return Ok(());
            };
            if digest(&lifetime) != expected {
                return Err(unavailable());
            }
            let path = directory.join(format!(
                "prepared/local-session/shared-abandoned-{identity}.json"
            ));
            let raw = match lifetime::read(&path) {
                Ok(raw) => raw,
                Err(_) => return Ok(()),
            };
            census(&directory, &identity, &lifetime, &raw)?;
            if lifetime::PhysicalLifetime::selected(&directory, &identity)?.1 != lifetime {
                return Err(unavailable());
            }
            let disposition = Disposition {
                schema_version: 1,
                scope: "joined_shared_resource_not_entered_only",
                owner: &identity,
                lifetime_sha256: digest(&lifetime),
                snapshot_sha256: digest(&raw),
                member: &member,
                session: &session,
                peer,
            };
            let bytes = serde_json::to_vec(&disposition).map_err(|_| unavailable())?;
            let artifact = SelectedHostArtifact {
                path: directory.join(format!("shared-abandoned-{}.json", digest(&bytes))),
                sha256: digest(&bytes),
            };
            lifetime::write_new(&artifact.path, &bytes)?;
            produced
                .set(AbandonedPhysicalDisposition { artifact })
                .map_err(|_| unavailable())?;
            Ok(())
        });
        async move {
            let retained = retained?;
            (&mut *retained.lock().await)
                .await
                .map_err(|_| unavailable())??;
            Ok(selected.get().cloned())
        }
    }
}
