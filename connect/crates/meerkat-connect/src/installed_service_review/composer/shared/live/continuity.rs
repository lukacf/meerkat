//! Exact workflow-resource provenance. Canonical admission selects these bytes;
//! native receipts and the physical/approval owners supply their own evidence.
//! A saved association cannot admit an input, replay approval or reopen work.
use super::*;
use crate::application_setup::{PreparedApplicationResources, read_regular_artifact};
use meerkat_core::lifecycle::InputId;
use meerkat_runtime::{
    completion::CompletionClass,
    input_state::InputTerminalOutcome,
    terminal_status::{
        InputTerminalReceiptObservation, InteractionSelector, TerminalWitnessSource,
    },
};
use meerkat_toolkit::host_lifetime::control::SelectedHostArtifact;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

#[cfg(test)]
mod canonical_g_capture;
pub(super) mod cold;
#[cfg(test)]
mod current_g_receiptful_capture;
#[cfg(test)]
mod tests;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceAssociation {
    schema_version: u32,
    // The original held Prepared admission, before selecting these bytes.
    // Later Active phase remains canonical slot truth, never this document's.
    admission: MemberClassAdmission,
    session: SessionId,
    #[serde(serialize_with = "serialize_input_ids")]
    baseline: HashSet<InputId>,
    resources: PreparedApplicationResources,
    approval_source: PathBuf,
    // Prospective physical identity only; legacy associations remain readable
    // by the unchanged successful-close path and cannot prove cold nonentry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    physical: Option<crate::resources::PhysicalSelection>,
    // Passive scope only. Missing legacy/ordinary history stays Unknown; this
    // never embeds the outer artifact that will hash this document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    consent: Option<crate::application_consent::ConsentResourceProvenance>,
    // Exact selected projection only. Never reconstruct a launch from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    projected_peer: Option<McpServerConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    abandoned_predecessor: Option<SelectedHostArtifact>,
    predecessor: Option<SelectedHostArtifact>,
    closed: Option<ClosedObservations>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClosedObservations {
    physical: SelectedHostArtifact,
    approvals: SelectedHostArtifact,
    #[serde(serialize_with = "serialize_input_ids")]
    inputs: HashSet<InputId>,
    // Exact retired transport selection, only for removing its old projection.
    // The restored physical owner supplies its own new descriptor.
    peer: McpServerConfig,
}

pub(super) struct RetainedAssociation {
    selection: SelectedHostArtifact,
    pub(super) document: ResourceAssociation,
}

pub(super) struct RestoreInputs {
    presenters: crate::application_gateway::LocalSetupApprovals,
    policy: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
    tasks: InstalledServiceTaskSource,
    realm: crate::application_setup::ApplicationSetupRealm,
}

struct SettledResolver(std::sync::Weak<LiveSharedSetup>);
#[async_trait::async_trait]
impl toolkit_mobkit::private_originals::SettledSharedPreparationResolver for SettledResolver {
    async fn resolve(
        &self,
        member: &meerkat_toolkit::setup::SharedPreparationMember,
        selection: &meerkat_toolkit::setup::SharedPreparationSelection,
    ) -> Result<
        Arc<dyn toolkit_mobkit::private_originals::SharedPreparationResource>,
        toolkit_mobkit::private_originals::PrivateOriginalError,
    > {
        use toolkit_mobkit::private_originals::PrivateOriginalError;
        let owner = self.0.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
        let entry = owner
            .snapshot()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .into_iter()
            .find(|entry| &entry.admitted.instance().member.member == member.helper())
            .ok_or(PrivateOriginalError::Unavailable)?;
        let before = owner
            .current(&entry)
            .map_err(|_| PrivateOriginalError::Denied)?;
        let association = entry
            .custody
            .association
            .get()
            .ok_or(PrivateOriginalError::Unavailable)?;
        association
            .check_open(
                &before,
                &owner.realm,
                &entry.custody.prepared.artifact_directory,
            )
            .map_err(|_| PrivateOriginalError::Conflict)?;
        owner
            .check_consent_provenance(&association.document)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if association.document.predecessor.is_none() {
            return Err(PrivateOriginalError::Unavailable);
        }
        let session = active_session(&before).map_err(|_| PrivateOriginalError::Denied)?;
        let resource = entry
            .context
            .attempt_peer()
            .settled_preparation(member, selection, &session.to_string())
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if owner
            .current(&entry)
            .map_err(|_| PrivateOriginalError::Denied)?
            != before
            || !owner
                .snapshot()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .iter()
                .any(|current| Arc::ptr_eq(current, &entry))
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(resource)
    }
}

/// The existing resource owner retains the complete claim, canonical write and
/// publication task. Losing a Meta callback never discards entered custody.
pub(super) struct RecoveryTask {
    task: Option<tokio::task::JoinHandle<Result<(), HostLifetimeError>>>,
    result: Option<Result<(), HostLifetimeError>>,
}
impl RecoveryTask {
    async fn join(&mut self) -> Result<(), HostLifetimeError> {
        if let Some(result) = self.result {
            return result;
        }
        let result = match self.task.as_mut().ok_or_else(unavailable)?.await {
            Ok(result) => result,
            Err(_) => Err(HostLifetimeError::NotSettled),
        };
        self.result = Some(result);
        self.task.take();
        result
    }
}

fn consent_error(error: crate::application_consent::ConsentOwnerError) -> HostLifetimeError {
    use crate::application_consent::ConsentOwnerError;
    match error {
        ConsentOwnerError::WrongApplication | ConsentOwnerError::WrongClaim => {
            HostLifetimeError::WrongOwner
        }
        ConsentOwnerError::MissingConsent => HostLifetimeError::NotSettled,
        _ => HostLifetimeError::Unavailable,
    }
}

fn unavailable() -> HostLifetimeError {
    HostLifetimeError::Unavailable
}
fn invalid() -> HostLifetimeError {
    HostLifetimeError::InvalidSelection
}
fn digest(raw: &[u8]) -> String {
    crate::application_bootstrap::hash(raw)
}
fn read(path: &Path) -> Result<Vec<u8>, HostLifetimeError> {
    read_regular_artifact(path).map_err(|_| unavailable())
}
fn read_selected(selected: &SelectedHostArtifact) -> Result<Vec<u8>, HostLifetimeError> {
    let bytes = read(&selected.path)?;
    if digest(&bytes) != selected.sha256 {
        return Err(invalid());
    }
    Ok(bytes)
}

// InputId has equality and hashing, not semantic ordering. Sort its UUID bytes
// only for deterministic persisted arrays and native receipt traversal.
fn ordered_input_ids(inputs: &HashSet<InputId>) -> Vec<&InputId> {
    let mut ordered: Vec<_> = inputs.iter().collect();
    ordered.sort_unstable_by_key(|input| input.0);
    ordered
}

fn serialize_input_ids<S: serde::Serializer>(
    inputs: &HashSet<InputId>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    ordered_input_ids(inputs).serialize(serializer)
}

fn derived_inputs_settled(settled: &HashSet<InputId>, derived: &HashMap<InputId, InputId>) -> bool {
    for input in derived.keys() {
        let mut visited = HashSet::new();
        let mut current = input;
        while !settled.contains(current) {
            if !visited.insert(current) {
                return false;
            }
            let Some(next) = derived.get(current) else {
                return false;
            };
            current = next;
        }
    }
    true
}

pub(super) fn prepared_resources(
    directory: &Path,
    realm: &RealmLocator,
) -> Result<PreparedApplicationResources, HostLifetimeError> {
    let root = realm.state_root.canonicalize().map_err(|_| unavailable())?;
    if directory.canonicalize().map_err(|_| unavailable())? != directory
        || directory.parent()
            != Some(
                root.join(".meerkat-connect")
                    .join(realm.realm.as_str())
                    .as_path(),
            )
    {
        return Err(invalid());
    }
    let path = directory.join("prepare.stdout");
    let raw = read(&path)?;
    // The producer already selected this artifact directory. Parsing its
    // retained handoff checks exact native configuration, not local history.
    crate::setup::PreparedSetupConnection::from_preparer_output(&raw).map_err(|_| invalid())?;
    Ok(PreparedApplicationResources {
        artifact_directory: directory.to_owned(),
        handoff: SelectedHostArtifact {
            path,
            sha256: digest(&raw),
        },
    })
}

impl ResourceAssociation {
    fn matches(
        &self,
        current: &MemberClassAdmission,
        realm: &RealmLocator,
    ) -> Result<(), HostLifetimeError> {
        if self.schema_version != 1
            || self.admission.workflow_resource.is_some()
            || self.admission.workflow.is_none()
            || self.admission.route.is_none()
            || self.admission.class.purpose != CompiledMemberClassPurpose::SharedWorkflow
            || !matches!(&self.admission.phase, MemberClassAdmissionPhase::Prepared { context, context_generation }
                if context.as_str() == self.session.to_string()
                    && self.admission.destination.as_ref().is_some_and(|destination|
                        destination.context == *context && destination.context_generation == *context_generation))
        {
            return Err(invalid());
        }
        if self
            .consent
            .as_ref()
            .is_some_and(|consent| consent.session() != &self.session)
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let mut identity = current.clone();
        identity.phase = self.admission.phase.clone();
        identity.workflow_resource = None;
        if identity != self.admission || !self.approval_source.is_absolute() {
            return Err(HostLifetimeError::WrongOwner);
        }
        let resources = prepared_resources(&self.resources.artifact_directory, realm)?;
        if resources.handoff != self.resources.handoff {
            return Err(invalid());
        }
        Ok(())
    }

    fn retain(self) -> Result<RetainedAssociation, HostLifetimeError> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let raw = toolkit_package::canonical_json(&self).map_err(|_| invalid())?;
        let sha256 = digest(&raw);
        let path = self
            .resources
            .artifact_directory
            .join(format!("workflow-resource-{sha256}.json"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| unavailable())?;
        file.write_all(&raw)
            .and_then(|_| file.sync_all())
            .map_err(|_| unavailable())?;
        std::fs::File::open(&self.resources.artifact_directory)
            .and_then(|file| file.sync_all())
            .map_err(|_| unavailable())?;
        Ok(RetainedAssociation {
            selection: SelectedHostArtifact { path, sha256 },
            document: self,
        })
    }
}

impl RetainedAssociation {
    pub(super) fn check_open(
        &self,
        current: &MemberClassAdmission,
        realm: &RealmLocator,
        directory: &Path,
    ) -> Result<(), HostLifetimeError> {
        self.document.matches(current, realm)?;
        if current.workflow_resource.as_ref() != Some(&self.selection)
            || self.document.closed.is_some()
            || self.document.resources.artifact_directory != directory
            || self.selection.path
                != directory.join(format!("workflow-resource-{}.json", self.selection.sha256))
        {
            return Err(invalid());
        }
        read_selected(&self.selection)?;
        Ok(())
    }

    fn selected(
        current: &MemberClassAdmission,
        realm: &RealmLocator,
    ) -> Result<Self, HostLifetimeError> {
        let selection = current
            .workflow_resource
            .clone()
            .ok_or(HostLifetimeError::NotSettled)?;
        let document: ResourceAssociation =
            serde_json::from_slice(&read_selected(&selection)?).map_err(|_| invalid())?;
        document.matches(current, realm)?;
        if selection.path
            != document
                .resources
                .artifact_directory
                .join(format!("workflow-resource-{}.json", selection.sha256))
        {
            return Err(invalid());
        }
        Ok(Self {
            selection,
            document,
        })
    }

    fn bind_consent(
        current: &MemberClassAdmission,
        realm: &RealmLocator,
        owner: &crate::application_consent::ApplicationConsentOwner,
        machine: &meerkat_runtime::MeerkatMachine,
        claim: &meerkat_core::session_hosting::HostingClaim,
    ) -> Result<crate::application_consent::ConsentAssociation, HostLifetimeError> {
        let selected = Self::selected(current, realm)?;
        selected
            .document
            .consent
            .as_ref()
            .ok_or(HostLifetimeError::NotSettled)?
            .bind_claim(
                owner,
                machine,
                &selected.document.session,
                selected.selection,
                claim,
            )
            .map_err(consent_error)
    }

    fn closed(
        current: &MemberClassAdmission,
        realm: &RealmLocator,
    ) -> Result<Self, HostLifetimeError> {
        let Self {
            selection,
            document,
        } = Self::selected(current, realm)?;
        let closed = document
            .closed
            .as_ref()
            .ok_or(HostLifetimeError::NotSettled)?;
        if active_session(current)? != document.session
            || document.predecessor.is_none()
            || selection.path
                != document
                    .resources
                    .artifact_directory
                    .join(format!("workflow-resource-{}.json", selection.sha256))
            || closed.approvals.path != document.approval_source
            || !document.baseline.is_subset(&closed.inputs)
        {
            return Err(invalid());
        }
        read_selected(&closed.physical)?;
        read_selected(&closed.approvals)?;
        Ok(Self {
            selection,
            document,
        })
    }
}

impl LiveSharedSetup {
    pub(crate) fn bind_restore_source(
        self: &Arc<Self>,
        source: &Arc<toolkit_mobkit::private_originals::PrivateOriginals>,
    ) -> Result<(), HostLifetimeError> {
        if let Some(prior) = self.restore_source.get() {
            return if prior
                .upgrade()
                .is_some_and(|prior| Arc::ptr_eq(&prior, source))
            {
                Ok(())
            } else {
                Err(HostLifetimeError::WrongOwner)
            };
        }
        source
            .bind_settled_shared_preparations(Arc::new(SettledResolver(Arc::downgrade(self))))
            .map_err(|_| HostLifetimeError::Denied)?;
        self.restore_source
            .set(Arc::downgrade(source))
            .map_err(|_| HostLifetimeError::Denied)
    }

    pub(crate) fn configure_restore(
        &self,
        presenters: crate::application_gateway::LocalSetupApprovals,
        policy: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
        tasks: InstalledServiceTaskSource,
        realm: crate::application_setup::ApplicationSetupRealm,
    ) -> Result<(), HostLifetimeError> {
        presenters.check_open().map_err(|_| unavailable())?;
        if realm.resolve().map_err(|_| unavailable())?.locator != self.realm {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.restore_inputs
            .set(RestoreInputs {
                presenters,
                policy,
                tasks,
                realm,
            })
            .map_err(|_| HostLifetimeError::Denied)
    }

    pub(super) async fn restore_entry(
        self: &Arc<Self>,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
        profile: &str,
    ) -> Result<(), HostLifetimeError> {
        if self.closed.load(Ordering::Acquire) || self.restore_inputs.get().is_none() {
            return Err(HostLifetimeError::NotSettled);
        }
        let admitted = Arc::new(
            self.policy
                .restore_workflow_member_class_for_identity(identity)?
                .ok_or(HostLifetimeError::Denied)?,
        );
        let current = self.policy.current_member_class(&admitted)?;
        if current.class.profile != profile {
            return Err(HostLifetimeError::Denied);
        }
        if current.workflow_resource.as_ref().is_some_and(|artifact| {
            self.abandoned
                .get()
                .and_then(Option::as_ref)
                .is_some_and(|selection| selection.selects(artifact))
        }) {
            return self
                .restore_abandoned(admitted, current, identity.clone())
                .await;
        }
        let recovery = {
            let mut recoveries = self.recoveries.lock().map_err(|_| unavailable())?;
            if self.closed.load(Ordering::Acquire) {
                return Err(HostLifetimeError::Denied);
            }
            if let Some(recovery) = recoveries.get(admitted.admission_id()) {
                recovery.clone()
            } else {
                if recoveries.len() >= self.policy.member_class_registration_capacity()? {
                    return Err(HostLifetimeError::Denied);
                }
                let live = self.clone();
                let identity = identity.clone();
                let id = admitted.admission_id().clone();
                let recovery = Arc::new(tokio::sync::Mutex::new(RecoveryTask {
                    task: Some(tokio::spawn(async move {
                        live.restore_one(admitted, current, identity).await
                    })),
                    result: None,
                }));
                recoveries.insert(id, recovery.clone());
                recovery
            }
        };
        recovery.lock().await.join().await
    }

    pub(super) async fn join_recoveries(&self) -> Result<(), HostLifetimeError> {
        let recoveries: Vec<_> = self
            .recoveries
            .lock()
            .map_err(|_| unavailable())?
            .values()
            .cloned()
            .collect();
        let mut failure = None;
        for recovery in recoveries {
            if let Err(error) = recovery.lock().await.join().await {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    async fn restore_one(
        self: Arc<Self>,
        admitted: Arc<AdmittedMemberClass>,
        expected: MemberClassAdmission,
        identity: meerkat_mobkit::identity_first::AgentIdentity,
    ) -> Result<(), HostLifetimeError> {
        let inputs = self.restore_inputs.get().ok_or_else(unavailable)?;
        let retained = RetainedAssociation::closed(&expected, &self.realm)?;
        let document = &retained.document;
        self.check_consent_provenance(document)?;
        let closed = document
            .closed
            .as_ref()
            .ok_or(HostLifetimeError::NotSettled)?;
        if self
            .closed_inputs(&document.session, &document.baseline)
            .await?
            .as_ref()
            != Some(&closed.inputs)
            || self.policy.current_member_class(&admitted)? != expected
            || self.closed.load(Ordering::Acquire)
        {
            return Err(HostLifetimeError::NotSettled);
        }
        let source = self
            .restore_source
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(unavailable)?;
        // Restore a restriction before native registration can capture G.
        // This neither promotes the saved input nor supplies a controller.
        self.check_consent_provenance(document)?;
        source
            .restore_workflow_class_source(admitted.clone(), &identity)
            .await?;
        let storage = inputs.realm.resolve().map_err(|_| unavailable())?;
        let resources =
            crate::resources::InstalledResources::locate().map_err(|_| unavailable())?;
        // Register before checking the close flag so notification cannot be
        // lost between selecting this task and entering physical preparation.
        let cancelled = self.closing.notified();
        tokio::pin!(cancelled);
        cancelled.as_mut().enable();
        let prepared = resources
            .restore_shared_setup(
                &storage,
                &document.resources,
                &closed.physical,
                &expected.instance.member.member,
                &document.session.to_string(),
                async {
                    if !self.closed.load(Ordering::Acquire) {
                        cancelled.await;
                    }
                    Ok(())
                },
            )
            .await
            .map_err(|_| HostLifetimeError::NotSettled)?;
        let mut new_approvals = None;
        let publication = async {
            if self.closed.load(Ordering::Acquire)
                || self.policy.current_member_class(&admitted)? != expected
            {
                return Err(HostLifetimeError::Denied);
            }
            // The retired source is evidence of its old owner's disposition.
            // This new presenter can approve only new, current-authorized work;
            // it never loads or replays an earlier approval decision.
            read_selected(&closed.approvals)?;
            let approvals = inputs
                .presenters
                .create_shared(inputs.tasks.clone(), inputs.policy.clone())
                .map_err(|_| unavailable())?;
            new_approvals = Some(approvals.clone());
            approvals
                .bind_session(&document.session)
                .map_err(|_| unavailable())?;
            self.check_consent_provenance(document)?;
            let mut open = document.clone();
            open.approval_source = approvals
                .source_for_session(&document.session)
                .map_err(|_| unavailable())?
                .to_owned();
            open.physical = prepared.owner.physical_selection();
            open.projected_peer = Some(
                prepared
                    .connections
                    .first()
                    .ok_or_else(unavailable)?
                    .connection
                    .server()
                    .clone(),
            );
            open.predecessor = Some(retained.selection.clone());
            open.closed = None;
            let open = open.retain()?;
            let observation = self
                .policy
                .observe_member_class(&expected.instance.member.member)
                .await?;
            observation.authorization?;
            if observation.admission != expected || self.closed.load(Ordering::Acquire) {
                approvals.retire();
                return Err(HostLifetimeError::Denied);
            }
            self.check_consent_provenance(&open.document)?;
            let saved = self
                .policy
                .retain_workflow_member_resource(
                    &admitted,
                    &expected,
                    &observation.checkpoint,
                    open.selection.clone(),
                )
                .await;
            if let Err(error) = saved {
                approvals.retire();
                return Err(error);
            }
            Ok((approvals, open))
        }
        .await;
        match publication {
            Ok((approvals, open)) => {
                match self.retain_selected(
                    admitted,
                    prepared,
                    approvals.clone(),
                    Some(document.session.clone()),
                    Some(closed.peer.clone()),
                    Some(open),
                ) {
                    Ok(()) => Ok(()),
                    Err((owner, error)) => {
                        approvals.retire();
                        owner
                            .close()
                            .await
                            .map_err(|_| HostLifetimeError::NotSettled)?;
                        Err(error)
                    }
                }
            }
            Err(error) => {
                if let Some(approvals) = new_approvals {
                    approvals.retire();
                }
                prepared
                    .owner
                    .close()
                    .await
                    .map_err(|_| HostLifetimeError::NotSettled)?;
                Err(error)
            }
        }
    }

    pub(super) fn check_consent_provenance(
        &self,
        document: &ResourceAssociation,
    ) -> Result<(), HostLifetimeError> {
        let selected = self.consent_owner.get().ok_or_else(unavailable)?;
        match (&document.consent, selected) {
            (None, None) => Ok(()), // Explicit ordinary history, never a linked recovery scope.
            (Some(provenance), Some(owner)) => {
                let native = self.native_context()?;
                provenance
                    .check_current(owner, native.machine()?, &document.session)
                    .map_err(consent_error)
            }
            _ => Err(HostLifetimeError::NotSettled), // No legacy or issuer upgrade.
        }
    }

    /// Retain the supplied service and its canonical native owner as one pair
    /// before any member construction. No store or replacement service is
    /// inferred or reopened from a resolved filesystem path.
    pub(crate) fn capture_native(
        &self,
        spec: &meerkat_mobkit::mob_handle_runtime::MobBootstrapSpec,
    ) -> Result<(), HostLifetimeError> {
        let service = spec.session_service.clone();
        let machine = service
            .acquire_runtime_adapter(spec.runtime_adapter.clone())
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        if spec
            .runtime_adapter
            .as_ref()
            .is_some_and(|selected| !Arc::ptr_eq(selected, &machine))
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let mut native = self.native.lock().map_err(|_| unavailable())?;
        if self.consent_owner.get().is_none() {
            return Err(HostLifetimeError::Unavailable);
        }
        if self.closed.load(Ordering::Acquire) || !matches!(&*native, NativeCapture::Uncaptured) {
            return Err(HostLifetimeError::Denied);
        }
        *native = NativeCapture::Captured(Arc::new(NativeContext { service, machine }));
        Ok(())
    }

    /// Only the real held Add finalizer calls this: its exact route is Prepared
    /// and its initial message is withheld. Persist before that finalizer can
    /// publish G or the commissioner can submit workflow planning/context.
    pub(crate) async fn retain_held_baseline(
        &self,
        admitted: &Arc<AdmittedMemberClass>,
    ) -> Result<(), HostLifetimeError> {
        let entry = self
            .snapshot()?
            .into_iter()
            .find(|entry| Arc::ptr_eq(&entry.admitted, admitted))
            .ok_or(HostLifetimeError::WrongOwner)?;
        let current = self.current(&entry)?;
        let MemberClassAdmissionPhase::Prepared {
            context,
            context_generation,
        } = &current.phase
        else {
            return Err(HostLifetimeError::Denied);
        };
        if current.workflow_resource.is_some()
            || entry.custody.association.get().is_some()
            || current.route.is_none()
            || current.destination.as_ref().is_none_or(|selected| {
                selected.context != *context || selected.context_generation != *context_generation
            })
        {
            return Err(HostLifetimeError::Denied);
        }
        let session = SessionId::parse(context.as_str()).map_err(|_| invalid())?;
        let native = self.native_context()?;
        let machine = native.machine()?;
        let registration = machine
            .current_session_registration_witness(&session)
            .await
            .ok_or(HostLifetimeError::Denied)?;
        let observed = machine
            .session_input_states(&session)
            .await
            .map_err(|_| unavailable())?;
        if observed.source != TerminalWitnessSource::LiveRuntime
            || registration.has_ended()
            || machine
                .current_session_registration_witness(&session)
                .await
                .as_ref()
                != Some(&registration)
        {
            return Err(HostLifetimeError::Denied);
        }
        let count = observed.report.len();
        let baseline: HashSet<_> = observed
            .report
            .into_iter()
            .map(|row| row.state.input_id)
            .collect();
        if baseline.len() != count {
            return Err(invalid());
        }
        let source = entry
            .approvals
            .source_for_session(&session)
            .map_err(|_| unavailable())?
            .to_owned();
        let observation = self
            .policy
            .observe_member_class(&current.instance.member.member)
            .await?;
        observation.authorization?;
        if observation.admission != current
            || self.current(&entry)? != current
            || registration.has_ended()
        {
            return Err(HostLifetimeError::Denied);
        }
        let consent = self
            .consent_owner
            .get()
            .ok_or_else(unavailable)?
            .as_ref()
            .map(|owner| owner.resource_provenance(machine, &session))
            .transpose()
            .map_err(consent_error)?;
        let association = ResourceAssociation {
            schema_version: 1,
            admission: current.clone(),
            session,
            baseline,
            resources: entry.custody.prepared.clone(),
            approval_source: source,
            physical: entry.context.physical_selection(),
            consent,
            projected_peer: Some(entry.resource.server().clone()),
            abandoned_predecessor: None,
            predecessor: None,
            closed: None,
        }
        .retain()?;
        self.check_consent_provenance(&association.document)?;
        self.policy
            .retain_workflow_member_resource(
                admitted,
                &current,
                &observation.checkpoint,
                association.selection.clone(),
            )
            .await?;
        // The accepted native Add owns this complete finalizer future through
        // abort/join. A caller losing its observer cannot cancel this transfer.
        entry
            .custody
            .association
            .set(association)
            .map_err(|_| HostLifetimeError::Denied)
    }

    /// The retained shared-close task calls this after native/source/guard
    /// joins and this exact physical close. Missing or unsettled observations
    /// leave the open association intact; they never select replacement work.
    pub(super) async fn retain_closed_association(
        &self,
        entry: &LiveEntry,
        physical: SelectedHostArtifact,
    ) -> Result<(), HostLifetimeError> {
        let Some(retained) = entry.custody.association.get() else {
            return Ok(());
        };
        read_selected(&retained.selection)
            .map_err(|error| close_refusal("association.selection_read", error))?;
        let document = &retained.document;
        let observation = self
            .policy
            .observe_member_class(&entry.admitted.instance().member.member)
            .await
            .map_err(|error| close_refusal("association.member_observation", error))?;
        observation
            .authorization
            .map_err(|error| close_refusal("association.member_authorization", error))?;
        let current = &observation.admission;
        if active_session(current)
            .map_err(|error| close_refusal("association.active_session", error))?
            != document.session
            || current.workflow_resource.as_ref() != Some(&retained.selection)
            || document.closed.is_some()
        {
            return Err(close_refusal(
                "association.current_selection",
                HostLifetimeError::Denied,
            ));
        }
        document
            .matches(current, &self.realm)
            .map_err(|error| close_refusal("association.identity", error))?;
        let Some(approval) = entry
            .approvals
            .closed_observation(&document.session)
            .map_err(|_| close_refusal("association.approval_observation", unavailable()))?
        else {
            return Ok(());
        };
        let (source, session) = approval.source();
        if source != document.approval_source || session != &document.session {
            return Err(close_refusal(
                "association.approval_source",
                HostLifetimeError::WrongOwner,
            ));
        }
        let Some(inputs) = self.closed_inputs(session, &document.baseline).await? else {
            return Ok(());
        };
        let approvals = SelectedHostArtifact {
            path: source.to_owned(),
            sha256: digest(
                &read(source).map_err(|error| close_refusal("association.approval_read", error))?,
            ),
        };
        read_selected(&physical)
            .map_err(|error| close_refusal("association.physical_read", error))?;
        let mut closed = document.clone();
        closed.predecessor = Some(retained.selection.clone());
        closed.closed = Some(ClosedObservations {
            physical,
            approvals,
            inputs,
            peer: entry.resource.server().clone(),
        });
        let closed = closed
            .retain()
            .map_err(|error| close_refusal("association.artifact_write", error))?;
        self.policy
            .retain_workflow_member_resource(
                &entry.admitted,
                current,
                &observation.checkpoint,
                closed.selection,
            )
            .await
            .map_err(|error| close_refusal("association.conditional_publish", error))?;
        Ok(())
    }

    /// Caller has already joined native shutdown and its local guard owner.
    /// Census every durable row, including the original baseline, so a prior
    /// unfinished input can never be hidden by the workflow's start boundary.
    async fn closed_inputs(
        &self,
        session: &SessionId,
        baseline: &HashSet<InputId>,
    ) -> Result<Option<HashSet<InputId>>, HostLifetimeError> {
        let native = self
            .native_context()
            .map_err(|error| close_refusal("inputs.native_owner", error))?;
        let machine = native
            .machine()
            .map_err(|error| close_refusal("inputs.native_owner", error))?;
        if machine
            .current_session_registration_witness(session)
            .await
            .is_some()
        {
            return Err(close_refusal(
                "inputs.registration_before",
                HostLifetimeError::NotSettled,
            ));
        }
        let observed = machine
            .session_input_states(session)
            .await
            .map_err(|_| close_refusal("inputs.census_read", unavailable()))?;
        if observed.source != TerminalWitnessSource::DurableStore {
            return Err(close_refusal("inputs.census_source", unavailable()));
        }
        let count = observed.report.len();
        let ids: HashSet<_> = observed
            .report
            .into_iter()
            .map(|row| row.state.input_id)
            .collect();
        if ids.len() != count || !baseline.is_subset(&ids) {
            return Err(close_refusal("inputs.census_identity", invalid()));
        }
        let mut settled = HashSet::new();
        let mut derived = HashMap::new();
        for input in ordered_input_ids(&ids) {
            let observed = machine
                .input_terminal_receipt_observation(
                    session,
                    InteractionSelector::InputId(input.clone()),
                )
                .await
                .map_err(|_| close_refusal("inputs.receipt_read", unavailable()))?
                .ok_or_else(|| close_refusal("inputs.receipt_missing", invalid()))?;
            if observed.source != TerminalWitnessSource::DurableStore
                || observed.report.input_id() != input
            {
                return Err(close_refusal("inputs.receipt_identity", invalid()));
            }
            match observed.report {
                InputTerminalReceiptObservation::Finalized(receipt)
                    if receipt.terminal() == &InputTerminalOutcome::Consumed
                        && matches!(
                            receipt.class(),
                            CompletionClass::Completed | CompletionClass::CompletedWithoutResult
                        )
                        && receipt
                            .recipient_input_ids()
                            .iter()
                            .all(|recipient| ids.contains(recipient)) =>
                {
                    settled.insert(input.clone());
                }
                InputTerminalReceiptObservation::TerminalWithoutReceipt {
                    terminal:
                        InputTerminalOutcome::Superseded {
                            superseded_by: next,
                        }
                        | InputTerminalOutcome::Coalesced { aggregate_id: next },
                    ..
                } if ids.contains(&next) && &next != input => {
                    derived.insert(input.clone(), next);
                }
                // Requeued/suspended, receipt-less consumed, callback-pending,
                // interrupted and unknown outcomes remain unresolved.
                _ => return Ok(None),
            }
        }
        if !derived_inputs_settled(&settled, &derived) {
            return Ok(None);
        }
        if machine
            .current_session_registration_witness(session)
            .await
            .is_some()
        {
            return Err(close_refusal(
                "inputs.registration_after",
                HostLifetimeError::NotSettled,
            ));
        }
        Ok(Some(ids))
    }
}
