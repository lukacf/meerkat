//! One-use orderly closure of this exact local resource. Python owns the empty
//! round observation; Core owns native process closure and current registration.
use super::*;
use meerkat_core::MobMemberBinding;
use serde::{Deserialize, Serialize};

const CLOSED: &str = "local-round-closed.json";
const CLAIMED: &str = "local-round-claimed.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseObservation {
    schema_version: u32,
    round: serde_json::Value,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClosedRound {
    schema_version: u32,
    resources_sha256: String,
    request_sha256: String,
    handoff_sha256: String,
    helper: MobMemberBinding,
    session: String,
    observation: CloseObservation,
}

#[derive(Default)]
pub(super) struct Continuity {
    // Filled only by the exact installed, gracefully closed native attempt.
    joined: Option<(MobMemberBinding, String)>,
    pending: Option<(ClosedRound, Vec<u8>)>,
    claimed: Option<Vec<u8>>,
}

fn sync_directory(path: &Path) -> Result<(), ConnectError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable())
}

fn absent(path: &Path) -> Result<bool, ConnectError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Ok(false),
        Err(_) => Err(unavailable()),
    }
}

impl LocalRoundOwner {
    /// This constructor is called only after authentication of the completed
    /// schema4 bundle. Nothing is consumed before native selects its session.
    pub(crate) fn restore_empty(
        resources: InstalledResources,
        setup: &crate::application_setup::PreparedApplicationSetup,
    ) -> Result<Self, ConnectError> {
        let storage = setup.realm.resolve().map_err(|_| unavailable())?;
        let selected_resources = setup.resources.as_ref().ok_or_else(unavailable)?;
        let artifacts = &selected_resources.artifact_directory;
        let directory = artifacts.join("prepared");
        let descriptor = exact_file(&directory.join("product-mcp.json"))?;
        let preparation = exact_file(&directory.join("product-prepared.json"))?;
        let mut owner = Self {
            resources,
            artifacts: artifacts.clone(),
            request: serde_json::json!({
                "prepared": directory,
                "realm_state_root": storage.locator.state_root,
                "realm": storage.locator.realm.as_str(),
                "descriptor_sha256": digest(&descriptor),
                "preparation_sha256": digest(&preparation),
            }),
            handoff_sha256: selected_resources.handoff.sha256.clone(),
            poisoned: false,
            process: None,
            continuity: Continuity::default(),
            #[cfg(test)]
            test_location: None,
        };
        owner
            .matches_preparation(selected_resources)
            .map_err(|_| unavailable())?;
        owner.select_closed(
            setup
                .helper_projection()
                .map_err(|_| unavailable())?
                .member(),
        )?;
        Ok(owner)
    }

    fn resources_digest(&self) -> Result<String, ConnectError> {
        // The installed manifest is already verified using the binary's pin.
        let path = self.resources.root.join("resources.json");
        let size = fs::metadata(&path).map_err(|_| unavailable())?.len();
        hash_file(&path, size)
    }

    fn request_digest(&self) -> Result<String, ConnectError> {
        let mut request = self.request.clone();
        request
            .as_object_mut()
            .ok_or_else(unavailable)?
            .remove("expected_round");
        Ok(digest(
            &serde_json::to_vec(&request).map_err(|_| unavailable())?,
        ))
    }

    fn observation_current(&self, observation: &CloseObservation) -> Result<(), ConnectError> {
        // Only compare the owner's opaque round observation with its retained
        // bytes. Do not interpret checkpoints, emptiness or request history.
        let session = exact_file(&self.artifacts.join("prepared/local-session/session.json"))?;
        let session_sha256 = digest(&session);
        if observation.schema_version != 1
            || observation
                .round
                .get("session_sha256")
                .and_then(serde_json::Value::as_str)
                != Some(session_sha256.as_str())
        {
            return Err(unavailable());
        }
        Ok(())
    }

    pub(super) fn select_closed(&mut self, helper: &MobMemberBinding) -> Result<(), ConnectError> {
        self.check_resources()?;
        if !absent(&self.artifacts.join(CLAIMED))? {
            return Err(unavailable());
        }
        let raw = exact_file(&self.artifacts.join(CLOSED))?;
        let closed: ClosedRound = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
        if closed.schema_version != 1
            || &closed.helper != helper
            || closed.session.is_empty()
            || closed.resources_sha256 != self.resources_digest()?
            || closed.request_sha256 != self.request_digest()?
            || closed.handoff_sha256 != self.handoff_sha256
        {
            return Err(unavailable());
        }
        self.observation_current(&closed.observation)?;
        self.continuity.pending = Some((closed, raw));
        Ok(())
    }

    pub(super) fn is_restored(&self) -> bool {
        self.continuity.pending.is_some()
    }

    pub(super) fn before_attempt(
        &mut self,
        helper: &MobMemberBinding,
        session: &str,
    ) -> Result<(), ConnectError> {
        self.continuity.joined = None;
        let Some((closed, raw)) = &self.continuity.pending else {
            return Ok(());
        };
        if &closed.helper != helper || closed.session != session {
            return Err(unavailable());
        }
        self.observation_current(&closed.observation)?;
        if exact_file(&self.artifacts.join(CLOSED))? != *raw {
            return Err(unavailable());
        }
        // create-new hard linking is the atomic one-claim operation. A crash at
        // any subsequent boundary leaves CLAIMED and cannot reopen this round.
        let claimed = self.artifacts.join(CLAIMED);
        fs::hard_link(self.artifacts.join(CLOSED), &claimed).map_err(|_| unavailable())?;
        self.continuity.claimed = Some(raw.clone());
        self.poisoned = true;
        sync_directory(&self.artifacts)?;
        if exact_file(&claimed)? != *raw || exact_file(&self.artifacts.join(CLOSED))? != *raw {
            return Err(unavailable());
        }
        fs::remove_file(self.artifacts.join(CLOSED)).map_err(|_| unavailable())?;
        sync_directory(&self.artifacts)?;
        self.request
            .as_object_mut()
            .ok_or_else(unavailable)?
            .insert("expected_round".into(), closed.observation.round.clone());
        self.continuity.pending = None;
        self.poisoned = false;
        Ok(())
    }

    pub(super) fn observed_graceful_close(&mut self, helper: MobMemberBinding, session: String) {
        if let Some(request) = self.request.as_object_mut() {
            request.remove("expected_round");
        }
        self.continuity.joined = Some((helper, session));
    }

    /// Called only after the factory is fenced and all native begin/finish and
    /// physical round children have joined. No observation alone reaches here.
    pub(crate) fn persist_empty_close(&mut self) -> Result<(), ConnectError> {
        if self.poisoned || self.has_pending_round() {
            return Ok(());
        }
        let Some((helper, session)) = &self.continuity.joined else {
            return Ok(());
        };
        let path = self
            .artifacts
            .join("prepared/local-session/empty-close.json");
        if absent(&path)? {
            return Ok(());
        }
        self.check_resources()?;
        let observation: CloseObservation =
            serde_json::from_slice(&exact_file(&path)?).map_err(|_| unavailable())?;
        self.observation_current(&observation)?;
        let closed = ClosedRound {
            schema_version: 1,
            resources_sha256: self.resources_digest()?,
            request_sha256: self.request_digest()?,
            handoff_sha256: self.handoff_sha256.clone(),
            helper: helper.clone(),
            session: session.clone(),
            observation,
        };
        // A prior claim belongs to this exact owner, not whichever record is
        // present now. Never overwrite another claimant or a published receipt.
        if !absent(&self.artifacts.join(CLOSED))? {
            return Err(unavailable());
        }
        if let Some(raw) = &self.continuity.claimed {
            if exact_file(&self.artifacts.join(CLAIMED))? != *raw {
                return Err(unavailable());
            }
            fs::remove_file(self.artifacts.join(CLAIMED)).map_err(|_| unavailable())?;
            sync_directory(&self.artifacts)?;
        } else if !absent(&self.artifacts.join(CLAIMED))? {
            return Err(unavailable());
        }
        retain(
            &self.artifacts.join(CLOSED),
            &serde_json::to_vec(&closed).map_err(|_| unavailable())?,
        )?;
        sync_directory(&self.artifacts)?;
        self.continuity.joined = None;
        self.continuity.claimed = None;
        Ok(())
    }
}
