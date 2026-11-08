//! Live construction participants for canonically admitted shared helpers.
//! These entries retain actual resources, not a second membership authority.

use super::*;
use async_trait::async_trait;
use meerkat_core::mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal};
use meerkat_core::{McpServerConfig, ToolCallView, ToolDispatchContext, ToolMutationClass};
use meerkat_mcp::{
    McpAttemptCancellation, McpAttemptGuard, McpAttemptGuardFactory, McpAttemptGuardRequest,
    McpCallContext, McpCallContextError, McpCallContextProvider, McpCallTarget, McpResourceTarget,
};
use meerkat_mob::{MobError, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec};
use meerkat_runtime::meerkat_machine::{
    ConfiguredMcpAttachmentProvider, RuntimeSessionRegistrationWitness, SessionAgentBinding,
};
use meerkat_toolkit::host_lifetime::{
    control::HostLifetimeError, policy::CompiledMemberClassPurpose,
};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use toolkit_mobkit::installed_host::{
    dynamic_members::{AdmittedMemberClass, MemberClassAdmission, MemberClassAdmissionPhase},
    tool_policy::PreparedInstalledToolPolicy,
};
mod continuity;

pub(crate) struct LiveSharedSetup {
    realm: RealmLocator,
    policy: PreparedInstalledToolPolicy,
    definition: meerkat_mob::MobDefinition,
    entries: Mutex<Vec<Arc<LiveEntry>>>,
    retired: Mutex<Vec<McpServerConfig>>,
    closed: AtomicBool,
    delegate: OnceLock<Arc<dyn McpCallContextProvider>>,
    profiles: std::collections::BTreeSet<String>,
    native: Mutex<NativeCapture>,
    consent_owner: OnceLock<Option<Arc<crate::application_consent::ApplicationConsentOwner>>>,
    abandoned: OnceLock<Option<crate::application_consent::AbandonedWorkflowRecovery>>,
    cold_recoveries:
        Mutex<std::collections::BTreeMap<toolkit_package::Id, Arc<continuity::cold::ColdRecovery>>>,
    close: tokio::sync::Mutex<Option<tokio::task::JoinHandle<Result<(), HostLifetimeError>>>>,
    close_result: OnceLock<Result<(), HostLifetimeError>>,
    restore_inputs: OnceLock<continuity::RestoreInputs>,
    restore_source: OnceLock<std::sync::Weak<toolkit_mobkit::private_originals::PrivateOriginals>>,
    recoveries: Mutex<
        std::collections::BTreeMap<
            toolkit_package::Id,
            Arc<tokio::sync::Mutex<continuity::RecoveryTask>>,
        >,
    >,
    closing: tokio::sync::Notify,
}
// One capture retains the supplied erased facade and the canonical native
// owner it selected. Service decorators may have distinct Arc identities.
enum NativeCapture {
    Uncaptured,
    Captured(Arc<NativeContext>),
    Retired,
}
struct NativeContext {
    service: Arc<dyn meerkat_mob::MobSessionService>,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
}
impl NativeContext {
    fn machine(&self) -> Result<&Arc<meerkat_runtime::MeerkatMachine>, HostLifetimeError> {
        let current = self
            .service
            .acquire_runtime_adapter(Some(self.machine.clone()))
            .map_err(|_| HostLifetimeError::Unavailable)?
            .ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&current, &self.machine) {
            return Err(HostLifetimeError::WrongOwner);
        }
        Ok(&self.machine)
    }
}
// Constructed before the retained close future exists, not in its disposable
// observer or first poll. Dropping an unpolled task must retire this cycle.
// The host has reached its native-close observation before this task starts.
// Retire even when the resource observation fails or unwinds, without changing
// its result or the coherent flag. Actual native owners retain any incomplete
// shutdown; releasing this extra reference is not evidence of native cleanup.
// This releases only the service/factory cycle. It neither removes retained
// physical owners/tasks nor proves their disposition. NativeContext owns no
// HostingClaim; future claim custody needs its own joined/publication boundary
// and must not be added to this unconditional service-retirement guard.
struct RetireNativeContext(Arc<LiveSharedSetup>);
impl Drop for RetireNativeContext {
    fn drop(&mut self) {
        let prior = {
            let mut state = self
                .0
                .native
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *state, NativeCapture::Retired)
        };
        // Service/factory callbacks can retain this owner. Drop outside the
        // capture lock, and leave a terminal slot that cannot be rebound.
        drop(prior);
    }
}
struct LiveEntry {
    admitted: Arc<AdmittedMemberClass>,
    resource: Arc<SetupResourceSelection>,
    context: Arc<SharedSetupCallContext>,
    approvals: Arc<LocalApprovals>,
    resumed: Option<SessionId>,
    predecessor: Option<McpServerConfig>,
    custody: Arc<LiveCustody>,
}
type LiveCloseTask = tokio::task::JoinHandle<
    Result<Option<meerkat_toolkit::host_lifetime::control::SelectedHostArtifact>, ConnectError>,
>;
struct LiveCustody {
    prepared: crate::application_setup::PreparedApplicationResources,
    association: OnceLock<continuity::RetainedAssociation>,
    resources: Arc<crate::service_install::SharedPreparationResources>,
    owner: Mutex<Option<crate::resources::SharedSetupOwner>>,
    renewal: tokio::sync::Mutex<Option<tokio::task::JoinHandle<Result<(), HostLifetimeError>>>>,
    renewal_failed: AtomicBool,
    close: tokio::sync::Mutex<Option<LiveCloseTask>>,
    closed_resource:
        OnceLock<Option<meerkat_toolkit::host_lifetime::control::SelectedHostArtifact>>,
    close_failed: AtomicBool,
}
impl LiveSharedSetup {
    pub(crate) fn new(
        realm: RealmLocator,
        policy: PreparedInstalledToolPolicy,
        definition: meerkat_mob::MobDefinition,
        classes: &[meerkat_toolkit::host_lifetime::policy::CompiledMemberClass],
    ) -> Arc<Self> {
        Arc::new(Self {
            realm,
            policy,
            definition,
            entries: Mutex::new(Vec::new()),
            retired: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
            delegate: OnceLock::new(),
            profiles: classes
                .iter()
                .filter(|class| class.purpose == CompiledMemberClassPurpose::SharedWorkflow)
                .map(|class| class.profile.clone())
                .collect(),
            native: Mutex::new(NativeCapture::Uncaptured),
            consent_owner: OnceLock::new(),
            abandoned: OnceLock::new(),
            cold_recoveries: Mutex::new(Default::default()),
            close: tokio::sync::Mutex::new(None),
            close_result: OnceLock::new(),
            restore_inputs: OnceLock::new(),
            restore_source: OnceLock::new(),
            recoveries: Mutex::new(Default::default()),
            closing: tokio::sync::Notify::new(),
        })
    }
    // Only the successful prepared/installed review carrier supplies this
    // exact issuer. Freeze ordinary None too; later capture cannot upgrade it.
    pub(crate) fn bind_consent_owner(
        &self,
        owner: Option<Arc<crate::application_consent::ApplicationConsentOwner>>,
    ) -> Result<(), HostLifetimeError> {
        if let Some(owner) = &owner {
            owner.check().map_err(|_| HostLifetimeError::Unavailable)?;
        }
        let native = self
            .native
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if self.closed.load(Ordering::Acquire) || !matches!(&*native, NativeCapture::Uncaptured) {
            return Err(HostLifetimeError::Denied);
        }
        self.consent_owner
            .set(owner)
            .map_err(|_| HostLifetimeError::Denied)
    }

    fn native_context(&self) -> Result<Arc<NativeContext>, HostLifetimeError> {
        match &*self
            .native
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
        {
            NativeCapture::Captured(context) => Ok(context.clone()),
            NativeCapture::Uncaptured => Err(HostLifetimeError::Unavailable),
            NativeCapture::Retired => Err(HostLifetimeError::Denied),
        }
    }
    fn current(&self, entry: &LiveEntry) -> Result<MemberClassAdmission, HostLifetimeError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(HostLifetimeError::Denied);
        }
        let current = self.policy.current_member_class(&entry.admitted)?;
        if current.class.purpose != CompiledMemberClassPurpose::SharedWorkflow
            || current.workflow.is_none()
            || !entry.context.validates_resource(
                &entry
                    .resource
                    .helper_projection()
                    .map_err(|_| HostLifetimeError::Unavailable)?,
            )
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(current)
    }
    fn snapshot(&self) -> Result<Vec<Arc<LiveEntry>>, HostLifetimeError> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .map_err(|_| HostLifetimeError::Unavailable)
    }
    fn stale(&self, server: &McpServerConfig) -> Result<bool, HostLifetimeError> {
        Ok(self
            .retired
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .contains(server))
    }
    pub(crate) fn customizer(
        self: &Arc<Self>,
        inner: Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>,
    ) -> Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer> {
        Arc::new(LiveCustomizer {
            live: self.clone(),
            inner,
        })
    }

    async fn prepare_rebuild(
        self: &Arc<Self>,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
        profile: &str,
    ) -> Result<(), HostLifetimeError> {
        let member =
            meerkat_mobkit::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let Some(entry) = self
            .snapshot()?
            .into_iter()
            .find(|entry| entry.admitted.instance().member.member.member == member.as_str())
        else {
            return if self.profiles.contains(profile) {
                self.restore_entry(identity, profile).await
            } else {
                Ok(())
            };
        };
        let current = self.current(&entry)?;
        if current.class.profile != profile {
            return Err(HostLifetimeError::Denied);
        }
        if current.phase == MemberClassAdmissionPhase::Reserved {
            return Ok(());
        }
        let session = active_session(&current)?;
        entry
            .approvals
            .check_session(&session)
            .map_err(|_| HostLifetimeError::Denied)?;
        let custody = entry.custody.clone();
        let mut renewal = custody.renewal.lock().await;
        // Another callback may have completed publication while this one
        // waited. Select the current slot within this exact retained owner.
        let entry = self
            .snapshot()?
            .into_iter()
            .find(|selected| {
                Arc::ptr_eq(&selected.custody, &custody)
                    && Arc::ptr_eq(&selected.admitted, &entry.admitted)
            })
            .ok_or(HostLifetimeError::WrongOwner)?;
        if self.current(&entry)? != current {
            return Err(HostLifetimeError::Denied);
        }
        if entry.custody.renewal_failed.load(Ordering::Acquire) {
            return Err(HostLifetimeError::NotSettled);
        }
        if renewal.is_none()
            && entry.resumed.as_ref() == Some(&session)
            && entry.context.attempt_unbound()
        {
            // Reuse only the already-retained inert preparation. This issues
            // no new owner command and never resets a native registration.
            return Ok(());
        }
        if renewal.is_none() {
            if !entry
                .context
                .attempt_renewable(&current.instance.member.member, &session.to_string())
            {
                // Refusing before a physical request is sent does not poison
                // the same owner for a later actual native retirement.
                return Err(HostLifetimeError::NotSettled);
            }
            let live = self.clone();
            let retained = entry.clone();
            // The actual owner retains both the returned peer and publication.
            // Dropping Meta's callback future does not drop physical custody.
            *renewal = Some(tokio::spawn(async move {
                live.renew_entry(retained, current, session).await
            }));
        }
        let result = match renewal.as_mut().expect("retained renewal").await {
            Ok(result) => result,
            Err(_) => Err(HostLifetimeError::Unavailable),
        };
        renewal.take();
        if result.is_err() {
            entry.custody.renewal_failed.store(true, Ordering::Release);
        }
        result
    }

    async fn renew_entry(
        self: Arc<Self>,
        entry: Arc<LiveEntry>,
        expected: MemberClassAdmission,
        session: SessionId,
    ) -> Result<(), HostLifetimeError> {
        if self.current(&entry)? != expected {
            return Err(HostLifetimeError::Denied);
        }
        {
            let owner = entry
                .custody
                .owner
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            if !owner
                .as_ref()
                .is_some_and(|owner| owner.selects_peer(&entry.context.peer, &self.realm))
            {
                return Err(HostLifetimeError::WrongOwner);
            }
        }
        let replacement = entry
            .context
            .peer
            .renew(&expected.instance.member.member, &session.to_string())
            .await
            .map_err(|_| HostLifetimeError::NotSettled)?;
        let cleanup = replacement.peer.clone();
        let published = self.publish_replacement(&entry, &expected, session, replacement);
        if published.is_err() {
            // This remains inside the retained task even when Meta has dropped
            // its callback. A refused preparation cannot leave a new listener.
            cleanup
                .join_retirement()
                .await
                .map_err(|_| HostLifetimeError::NotSettled)?;
        }
        published
    }

    fn publish_replacement(
        &self,
        entry: &Arc<LiveEntry>,
        expected: &MemberClassAdmission,
        session: SessionId,
        replacement: crate::resources::PreparedSharedConnection,
    ) -> Result<(), HostLifetimeError> {
        let helper = replacement
            .connection
            .for_mobkit_helper_in_place(
                expected.instance.member.member.clone(),
                crate::setup::SetupHelperTools::CheckedInstallation,
            )
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        let context = SharedSetupCallContext::new(
            replacement.peer,
            &helper,
            self.delegate
                .get()
                .cloned()
                .ok_or(HostLifetimeError::Unavailable)?,
        )
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
        let resource = Arc::new(SetupResourceSelection::from_workflow(
            self.policy.clone(),
            entry.admitted.clone(),
            helper,
            context.clone(),
        )?);
        if &self.current(entry)? != expected {
            return Err(HostLifetimeError::Denied);
        }
        entry
            .approvals
            .check_session(&session)
            .map_err(|_| HostLifetimeError::Denied)?;
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let slot = entries
            .iter_mut()
            .find(|current| Arc::ptr_eq(current, entry))
            .ok_or(HostLifetimeError::WrongOwner)?;
        let mut retired = self
            .retired
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(HostLifetimeError::Denied);
        }
        retired.push(entry.resource.server().clone());
        *slot = Arc::new(LiveEntry {
            admitted: entry.admitted.clone(),
            resource,
            context,
            approvals: entry.approvals.clone(),
            resumed: Some(session),
            predecessor: Some(entry.resource.server().clone()),
            custody: entry.custody.clone(),
        });
        Ok(())
    }
    pub(crate) fn check_reservation(
        &self,
        admitted: &AdmittedMemberClass,
    ) -> Result<MemberClassAdmission, HostLifetimeError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(HostLifetimeError::Denied);
        }
        let current = self.policy.current_member_class(admitted)?;
        if current.class.purpose != CompiledMemberClassPurpose::SharedWorkflow
            || current.workflow.is_none()
            || current.phase != MemberClassAdmissionPhase::Reserved
            || !self.profiles.contains(&current.class.profile)
            || self
                .definition
                .resolve_inline_profile(&meerkat_mob::ProfileName::from(
                    current.class.profile.clone(),
                ))
                .is_none()
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(current)
    }
    /// Exact real peer descriptor retained by this resource participant. This
    /// is available only while the same canonical admission remains current.
    pub(crate) fn workflow_server(
        &self,
        admitted: &Arc<AdmittedMemberClass>,
    ) -> Result<meerkat_core::McpServerConfig, HostLifetimeError> {
        let entry = self
            .snapshot()?
            .into_iter()
            .find(|entry| Arc::ptr_eq(&entry.admitted, admitted))
            .ok_or(HostLifetimeError::Denied)?;
        self.current(&entry)?;
        Ok(entry.resource.server().clone())
    }
    pub(crate) fn require(&self, admitted: &AdmittedMemberClass) -> Result<(), HostLifetimeError> {
        let current = self.policy.current_member_class(admitted)?;
        let entries = self.snapshot()?;
        let entry = entries
            .iter()
            .find(|entry| entry.admitted.admission_id() == admitted.admission_id())
            .ok_or(HostLifetimeError::Unavailable)?;
        if self.current(entry)? != current {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }
    /// Failure returns the physical owner unchanged so the caller can join it.
    /// After success it stays here until native shutdown and this owner's join.
    #[allow(
        clippy::result_large_err,
        reason = "the refused owner returns by value so the caller can join it"
    )]
    pub(crate) fn retain(
        &self,
        admitted: Arc<AdmittedMemberClass>,
        prepared: crate::resources::PreparedSharedSetup,
        approvals: Arc<LocalApprovals>,
    ) -> Result<(), (crate::resources::SharedSetupOwner, HostLifetimeError)> {
        self.retain_selected(admitted, prepared, approvals, None, None, None)
    }

    #[allow(
        clippy::result_large_err,
        reason = "the refused owner returns by value so the caller can join it"
    )]
    fn retain_selected(
        &self,
        admitted: Arc<AdmittedMemberClass>,
        prepared: crate::resources::PreparedSharedSetup,
        approvals: Arc<LocalApprovals>,
        resumed: Option<SessionId>,
        predecessor: Option<McpServerConfig>,
        association: Option<continuity::RetainedAssociation>,
    ) -> Result<(), (crate::resources::SharedSetupOwner, HostLifetimeError)> {
        let crate::resources::PreparedSharedSetup {
            mut connections,
            owner,
            artifact_directory,
        } = prepared;
        let checked = (|| {
            if self.closed.load(Ordering::Acquire) || connections.len() != 1 {
                return Err(HostLifetimeError::Denied);
            }
            let current = match (&resumed, &association) {
                (None, None) => self.check_reservation(&admitted)?,
                (Some(session), Some(association)) => {
                    let current = self.policy.current_member_class(&admitted)?;
                    if active_session(&current)? != *session {
                        return Err(HostLifetimeError::Denied);
                    }
                    association.check_open(&current, &self.realm, &artifact_directory)?;
                    self.check_consent_provenance(&association.document)?;
                    approvals
                        .check_session(session)
                        .map_err(|_| HostLifetimeError::Denied)?;
                    current
                }
                _ => return Err(HostLifetimeError::Denied),
            };
            let prepared = continuity::prepared_resources(&artifact_directory, &self.realm)?;
            let delegate = self
                .delegate
                .get()
                .cloned()
                .ok_or(HostLifetimeError::Unavailable)?;
            let peer = connections.pop().ok_or(HostLifetimeError::Denied)?;
            if !owner.selects_peer(&peer.peer, &self.realm) {
                return Err(HostLifetimeError::WrongOwner);
            }
            let helper = peer
                .connection
                .for_mobkit_helper_in_place(
                    current.instance.member.member.clone(),
                    crate::setup::SetupHelperTools::CheckedInstallation,
                )
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
            let context = SharedSetupCallContext::new(peer.peer, &helper, delegate)
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
            let resource = Arc::new(SetupResourceSelection::from_workflow(
                self.policy.clone(),
                admitted.clone(),
                helper,
                context.clone(),
            )?);
            let entries = self
                .entries
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            let capacity = self.policy.member_class_registration_capacity()?;
            if self.closed.load(Ordering::Acquire)
                || entries.len() >= capacity
                || entries.iter().any(|entry| {
                    entry.admitted.admission_id() == admitted.admission_id()
                        || entry.admitted.instance().member.member
                            == admitted.instance().member.member
                        || entry.resource.server() == resource.server()
                })
            {
                return Err(HostLifetimeError::Denied);
            }
            // Recheck the canonical reservation after creating inert transport
            // participants and before making them visible to native creation.
            let after = self.policy.current_member_class(&admitted)?;
            if after != current {
                return Err(HostLifetimeError::Denied);
            }
            Ok((entries, resource, context, prepared))
        })();
        match checked {
            Ok((mut entries, resource, context, prepared)) => {
                let retained = OnceLock::new();
                if let Some(association) = association {
                    let _ = retained.set(association);
                }
                if let Some(predecessor) = &predecessor {
                    match self.retired.lock() {
                        Ok(mut retired) => retired.push(predecessor.clone()),
                        Err(_) => return Err((owner, HostLifetimeError::Unavailable)),
                    }
                }
                entries.push(Arc::new(LiveEntry {
                    admitted,
                    resource,
                    context,
                    approvals,
                    resumed,
                    predecessor,
                    custody: Arc::new(LiveCustody {
                        prepared,
                        association: retained,
                        resources: Arc::default(),
                        owner: Mutex::new(Some(owner)),
                        renewal: tokio::sync::Mutex::new(None),
                        renewal_failed: AtomicBool::new(false),
                        close: tokio::sync::Mutex::new(None),
                        closed_resource: OnceLock::new(),
                        close_failed: AtomicBool::new(false),
                    }),
                }));
                Ok(())
            }
            Err(error) => Err((owner, error)),
        }
    }
    pub(crate) fn selected_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        destination: &meerkat_mob::MemberDestinationSelection,
    ) -> Result<(), MobError> {
        for entry in self.snapshot().map_err(wiring)? {
            if &entry.admitted.instance().member.member != identity.member() {
                continue;
            }
            self.current(&entry).map_err(wiring)?;
            if entry
                .resumed
                .as_ref()
                .is_some_and(|session| identity.session_document().as_str() != session.to_string())
            {
                return Err(wiring(HostLifetimeError::Denied));
            }
            entry.resource.selected_destination(identity, destination)?;
            self.current(&entry).map_err(wiring)?;
        }
        Ok(())
    }
    pub(crate) fn provider(
        self: &Arc<Self>,
        delegate: Arc<dyn McpCallContextProvider>,
    ) -> Result<Arc<dyn McpCallContextProvider>, HostLifetimeError> {
        self.delegate
            .set(delegate.clone())
            .map_err(|_| HostLifetimeError::Denied)?;
        Ok(Arc::new(LiveCallContext {
            live: self.clone(),
            delegate,
        }))
    }
    pub(crate) fn configured(
        self: &Arc<Self>,
        installed: Option<Arc<dyn ConfiguredMcpAttachmentProvider>>,
    ) -> Arc<dyn ConfiguredMcpAttachmentProvider> {
        Arc::new(LiveConfigured {
            live: self.clone(),
            installed,
        })
    }
    pub(crate) fn composer(
        self: &Arc<Self>,
        delegate: Arc<dyn McpAdapterComposer>,
        tasks: InstalledServiceTaskSource,
        policy: Arc<super::super::super::account_request::ServicePolicySource>,
    ) -> Arc<dyn McpAdapterComposer> {
        Arc::new(LiveComposer {
            live: self.clone(),
            delegate,
            tasks,
            policy,
        })
    }
    pub(crate) fn guards(
        self: &Arc<Self>,
        delegate: Arc<dyn McpAttemptGuardFactory>,
    ) -> Arc<dyn McpAttemptGuardFactory> {
        Arc::new(LiveGuards {
            live: self.clone(),
            delegate,
        })
    }
    pub(crate) fn begin_close(&self) {
        self.closed.store(true, Ordering::Release);
        self.closing.notify_waiters();
        if let Ok(entries) = self.snapshot() {
            for entry in entries {
                entry.approvals.retire();
            }
        }
    }
    /// Only the caller retaining this exact resource can invoke this after an
    /// actual native refusal/retirement has settled. It is not a phase-based
    /// test for native absence. Unknown and Created outcomes never use it.
    pub(crate) async fn release_settled_refusal(
        &self,
        admitted: &Arc<AdmittedMemberClass>,
    ) -> Result<(), HostLifetimeError> {
        let entry = self
            .snapshot()?
            .into_iter()
            .find(|entry| Arc::ptr_eq(&entry.admitted, admitted))
            .ok_or(HostLifetimeError::WrongOwner)?;
        entry.custody.join_close(None).await?;
        // Remove only after physical cleanup joined. Failed or interrupted
        // observations keep the exact entry and close task for later drain.
        self.entries
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .retain(|current| !Arc::ptr_eq(current, &entry));
        Ok(())
    }
    /// Called only after the native runtime and its shared guard installation
    /// have joined. Each physical close is retained before its first await.
    pub(crate) async fn join_after_native_close(
        self: &Arc<Self>,
        coherent: bool,
    ) -> Result<(), HostLifetimeError> {
        self.begin_close();
        let mut close = self.close.lock().await;
        if let Some(result) = self.close_result.get() {
            return *result;
        }
        if close.is_none() {
            *close = Some(tokio::spawn(self.close_task(coherent)));
        }
        let result = match close.as_mut().expect("retained shared close").await {
            Ok(result) => result,
            Err(_) => Err(close_refusal(
                "close_task_join",
                HostLifetimeError::NotSettled,
            )),
        };
        let _ = self.close_result.set(result);
        close.take();
        result
    }

    fn close_task(
        self: &Arc<Self>,
        coherent: bool,
    ) -> impl std::future::Future<Output = Result<(), HostLifetimeError>> + Send + 'static {
        let retire = RetireNativeContext(self.clone());
        async move {
            // Move the whole guard into the future, including its unpolled
            // state. This guard owns only extra native-context references.
            let retained = retire;
            retained.0.close_entries(coherent).await
        }
    }

    async fn close_entries(&self, coherent: bool) -> Result<(), HostLifetimeError> {
        let mut failure = self
            .join_recoveries()
            .await
            .map_err(|error| close_refusal("recovery_join", error))
            .err();
        if let Err(error) = self.join_cold_recoveries().await {
            failure = Some(close_refusal("cold_recovery_join", error));
        }
        for entry in self
            .snapshot()
            .map_err(|error| close_refusal("entry_snapshot", error))?
        {
            // Preserve this independently settled presenter constituent even
            // if native/physical closure cannot publish a successful aggregate.
            if let Err(error) = self.retain_closed_presenter(&entry) {
                failure = Some(close_refusal("local_presenter_disposition", error));
            }
            match entry.custody.join_close(Some(entry.context.clone())).await {
                Ok(Some(physical)) if coherent => {
                    if let Err(error) = self.retain_closed_association(&entry, physical).await {
                        failure = Some(error);
                    }
                }
                Ok(_) => (),
                Err(error) => failure = Some(close_refusal("controller_close", error)),
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
impl LiveCustody {
    async fn join_close(
        &self,
        context: Option<Arc<SharedSetupCallContext>>,
    ) -> Result<
        Option<meerkat_toolkit::host_lifetime::control::SelectedHostArtifact>,
        HostLifetimeError,
    > {
        // Construction callbacks may have been cancelled after renewal began.
        // Join retained construction before closing its physical Controller.
        let mut renewal = self.renewal.lock().await;
        if let Some(task) = renewal.as_mut() {
            if !matches!(task.await, Ok(Ok(()))) {
                self.renewal_failed.store(true, Ordering::Release);
            }
            renewal.take();
        }
        let mut close = self.close.lock().await;
        if close.is_none() {
            let owner = self
                .owner
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?
                .take();
            if let Some(owner) = owner {
                *close = Some(tokio::spawn(async move {
                    match context {
                        Some(context) => owner.close_for_restore(context).await,
                        None => owner.close().await.map(|_| None),
                    }
                }));
            }
        }
        if let Some(task) = close.as_mut() {
            match task.await {
                Ok(Ok(selected)) => {
                    let _ = self.closed_resource.set(selected);
                }
                _ => self.close_failed.store(true, Ordering::Release),
            }
            close.take();
        }
        if self.close_failed.load(Ordering::Acquire) {
            Err(HostLifetimeError::NotSettled)
        } else if self.renewal_failed.load(Ordering::Acquire) {
            Ok(None)
        } else {
            Ok(self.closed_resource.get().cloned().flatten())
        }
    }
}
// Fixed failure boundaries only. Diagnostics cannot alter settlement or expose
// owner errors, identifiers, paths or retained values.
fn close_refusal(stage: &'static str, error: HostLifetimeError) -> HostLifetimeError {
    use std::io::Write as _;
    let _ = writeln!(
        std::io::stderr().lock(),
        "{{\"event\":\"toolkit_shared_close_refused\",\"stage\":\"{stage}\"}}"
    );
    error
}
fn wiring(error: HostLifetimeError) -> MobError {
    MobError::WiringError(error.to_string())
}
fn active_session(current: &MemberClassAdmission) -> Result<SessionId, HostLifetimeError> {
    let MemberClassAdmissionPhase::Active {
        context,
        context_generation,
    } = &current.phase
    else {
        return Err(HostLifetimeError::Denied);
    };
    if !current.destination.as_ref().is_some_and(|destination| {
        &destination.context == context && destination.context_generation == *context_generation
    }) {
        return Err(HostLifetimeError::Denied);
    }
    SessionId::parse(context.as_str()).map_err(|_| HostLifetimeError::Denied)
}

struct LiveCustomizer {
    live: Arc<LiveSharedSetup>,
    inner: Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>,
}
#[async_trait]
impl meerkat_mobkit::identity_first::AgentCustomizer for LiveCustomizer {
    async fn customize_build(
        &self,
        context: &meerkat_mobkit::identity_first::AgentBuildContext,
        spec: &meerkat_mobkit::identity_first::DurableAgentSpec,
        draft: &mut meerkat_mobkit::identity_first::AgentBuildDraft,
    ) -> Result<(), meerkat_mobkit::identity_first::CustomizerError> {
        self.inner.customize_build(context, spec, draft).await?;
        if context.identity != spec.identity {
            return Err(
                meerkat_mobkit::identity_first::CustomizerError::BuildFailed(
                    "shared helper identity changed".into(),
                ),
            );
        }
        self.live
            .prepare_rebuild(&context.identity, spec.profile.as_str())
            .await
            .map_err(|_| {
                meerkat_mobkit::identity_first::CustomizerError::BuildFailed(
                    "shared helper resource renewal refused".into(),
                )
            })
    }
    async fn after_create(
        &self,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
        session: &SessionId,
        context: &meerkat_mobkit::SessionCreatedContext,
    ) -> Result<(), meerkat_mobkit::identity_first::CustomizerError> {
        self.inner.after_create(identity, session, context).await
    }
}
impl SpawnMemberCustomizer for LiveSharedSetup {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        for entry in self.snapshot().map_err(wiring)? {
            let member = &entry.admitted.instance().member.member;
            if ctx.mob_id.as_str() != member.mob_id || spec.identity.as_str() != member.member {
                continue;
            }
            let current = self.current(&entry).map_err(wiring)?;
            let launch_matches = match (&spec.launch_mode, &entry.resumed) {
                (meerkat_mob::launch::MemberLaunchMode::Fresh, None) => {
                    current.phase == MemberClassAdmissionPhase::Reserved
                }
                (
                    meerkat_mob::launch::MemberLaunchMode::Resume {
                        bridge_session_id,
                        resume_from_role,
                    },
                    Some(session),
                ) => {
                    resume_from_role.is_none()
                        && bridge_session_id == session
                        && active_session(&current).is_ok_and(|selected| &selected == session)
                        && entry.approvals.check_session(session).is_ok()
                }
                _ => false,
            };
            if !launch_matches
                || ctx.requested_profile.as_str() != current.class.profile
                || spec.role_name.as_str() != current.class.profile
            {
                return Err(wiring(HostLifetimeError::Denied));
            }
            let base = self
                .definition
                .resolve_inline_profile(&spec.role_name)
                .ok_or_else(|| wiring(HostLifetimeError::Denied))?;
            let supplied =
                replacement_profile(spec.override_profile.as_ref(), entry.predecessor.as_ref())?;
            let profile = profile_with_peer(base, supplied.as_ref(), entry.resource.server())?;
            self.current(&entry).map_err(wiring)?;
            spec.override_profile = Some(profile);
            return Ok(());
        }
        // A compiled shared profile is not an instruction to construct an
        // unselected helper. Only the retained canonical reservation above can
        // supply its real peer before native creation.
        if self.profiles.contains(spec.role_name.as_str())
            || self.profiles.contains(ctx.requested_profile.as_str())
        {
            return Err(wiring(HostLifetimeError::Denied));
        }
        Ok(())
    }
}

fn replacement_profile(
    supplied: Option<&meerkat_mob::Profile>,
    predecessor: Option<&McpServerConfig>,
) -> Result<Option<meerkat_mob::Profile>, MobError> {
    let Some(supplied) = supplied else {
        return Ok(None);
    };
    let mut supplied = supplied.clone();
    if let Some(predecessor) = predecessor {
        let selected: Vec<_> = supplied
            .tools
            .mcp_servers
            .iter()
            .filter(|server| server.name == crate::setup::SERVER_NAME)
            .collect();
        if !selected.is_empty() && (selected.len() != 1 || selected[0] != predecessor) {
            return Err(wiring(HostLifetimeError::Denied));
        }
        supplied
            .tools
            .mcp_servers
            .retain(|server| server != predecessor);
    }
    Ok(Some(supplied))
}

fn profile_with_peer(
    base: &meerkat_mob::Profile,
    supplied: Option<&meerkat_mob::Profile>,
    peer: &McpServerConfig,
) -> Result<meerkat_mob::Profile, MobError> {
    let has_setup = |profile: &meerkat_mob::Profile| {
        profile
            .tools
            .mcp
            .iter()
            .any(|name| name == crate::setup::SERVER_NAME)
            || profile
                .tools
                .mcp_servers
                .iter()
                .any(|server| server.name == crate::setup::SERVER_NAME)
    };
    if has_setup(base) || supplied.is_some_and(has_setup) || peer.name != crate::setup::SERVER_NAME
    {
        return Err(wiring(HostLifetimeError::Denied));
    }
    let mut selected = supplied.unwrap_or(base).clone();
    selected.tools.mcp_servers.push(peer.clone());
    Ok(selected)
}
struct LiveConfigured {
    live: Arc<LiveSharedSetup>,
    installed: Option<Arc<dyn ConfiguredMcpAttachmentProvider>>,
}
impl ConfiguredMcpAttachmentProvider for LiveConfigured {
    fn configured(
        &self,
        witness: &RuntimeSessionRegistrationWitness,
        agent: SessionAgentBinding,
    ) -> Option<Arc<dyn ConfiguredMcpAttachmentAuthority>> {
        let selections = match self.live.snapshot() {
            // Retain exact failed/retired selections too: their authority will
            // refuse. Filtering them could delegate a known stale descriptor
            // to a different configured owner.
            Ok(entries) => entries
                .into_iter()
                .map(|entry| entry.resource.clone())
                .collect(),
            Err(_) => return Some(Arc::new(UnavailableAttachment)),
        };
        SetupResourceProvider::multiple(selections, self.installed.clone())
            .configured(witness, agent)
    }
    fn agent_selection(
        &self,
        agent: SessionAgentBinding,
    ) -> Option<Arc<dyn ConfiguredMcpAttachmentAuthority>> {
        self.installed
            .as_ref()
            .and_then(|owner| owner.agent_selection(agent))
    }
}
struct UnavailableAttachment;
impl ConfiguredMcpAttachmentAuthority for UnavailableAttachment {
    fn authorize_configured(&self, _: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        Err(McpAttachmentRefusal::Unavailable)
    }
}
struct LiveComposer {
    live: Arc<LiveSharedSetup>,
    delegate: Arc<dyn McpAdapterComposer>,
    tasks: InstalledServiceTaskSource,
    policy: Arc<super::super::super::account_request::ServicePolicySource>,
}
impl McpAdapterComposer for LiveComposer {
    fn compose(
        &self,
        adapter: &Arc<McpRouterAdapter>,
        session: &SessionId,
        member: Option<&MobMemberBinding>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
        for entry in self
            .live
            .snapshot()
            .map_err(|_| shared_composition_refused())?
        {
            if !selected_member(&entry.admitted.instance().member.member, member)? {
                continue;
            }
            self.live
                .current(&entry)
                .map_err(|_| shared_composition_refused())?;
            match &entry.resumed {
                Some(selected) if selected == session => entry.approvals.check_session(session),
                Some(_) => return Err(shared_composition_refused()),
                None => entry.approvals.bind_session(session),
            }
            .map_err(|_| shared_composition_refused())?;
            let checked =
                crate::service_install::CheckedInstalls::with_retained_shared_software_review(
                    adapter.clone(),
                    entry.approvals.clone(),
                    entry.context.clone(),
                    entry.custody.resources.clone(),
                );
            checked
                .bind_realm(&self.live.realm)
                .map_err(|_| shared_composition_refused())?;
            return super::super::super::account_request::compose_shared_public_requests(
                checked,
                self.live.realm.clone(),
                session.clone(),
                entry.approvals.clone(),
                self.tasks.clone(),
                self.policy.clone(),
            )
            .map_err(|_| shared_composition_refused());
        }
        self.delegate.compose(adapter, session, member)
    }
}
struct LiveCallContext {
    live: Arc<LiveSharedSetup>,
    delegate: Arc<dyn McpCallContextProvider>,
}
impl LiveCallContext {
    fn selected(
        &self,
        config: &McpServerConfig,
    ) -> Result<Option<Arc<LiveEntry>>, McpCallContextError> {
        let entries = self
            .live
            .snapshot()
            .map_err(|_| McpCallContextError::Unavailable)?;
        // Publication holds entries while recording the retired descriptor,
        // so observing the successor cannot miss its predecessor's refusal.
        if self
            .live
            .stale(config)
            .map_err(|_| McpCallContextError::Unavailable)?
        {
            return Err(McpCallContextError::Denied);
        }
        let mut exact = entries
            .into_iter()
            .filter(|entry| entry.resource.server() == config);
        let Some(entry) = exact.next() else {
            return Ok(None);
        };
        if exact.next().is_some() {
            return Err(McpCallContextError::Denied);
        }
        self.live.current(&entry).map_err(|error| match error {
            HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                McpCallContextError::Unavailable
            }
            _ => McpCallContextError::Denied,
        })?;
        Ok(Some(entry))
    }
}
#[async_trait]
impl McpCallContextProvider for LiveCallContext {
    async fn prepare(
        &self,
        target: McpCallTarget<'_>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        match self.selected(target.config).inspect_err(|error| {
            let kind = match error {
                McpCallContextError::Denied => "denied",
                McpCallContextError::Unavailable => "unavailable",
            };
            eprintln!("{{\"event\":\"toolkit_shared_dispatch_refused\",\"stage\":\"context.live_selection\",\"kind\":\"{kind}\"}}");
        })? {
            Some(entry) => entry.context.prepare(target, call, context).await,
            None => self.delegate.prepare(target, call, context).await,
        }
    }
    async fn prepare_resource(
        &self,
        target: McpResourceTarget<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        match self.selected(target.config)? {
            Some(_) => Err(McpCallContextError::Denied),
            None => self.delegate.prepare_resource(target, context).await,
        }
    }
    fn resource_authorization_target(
        &self,
        target: McpResourceTarget<'_>,
    ) -> Option<meerkat_core::authorization::OwnerQualifiedTarget> {
        match self.selected(target.config) {
            Ok(None) => self.delegate.resource_authorization_target(target),
            _ => None,
        }
    }
    fn tool_mutation_class(&self, config: &McpServerConfig, operation: &str) -> ToolMutationClass {
        self.delegate.tool_mutation_class(config, operation)
    }
}
struct LiveGuards {
    live: Arc<LiveSharedSetup>,
    delegate: Arc<dyn McpAttemptGuardFactory>,
}
#[async_trait]
impl McpAttemptGuardFactory for LiveGuards {
    async fn begin(
        &self,
        request: McpAttemptGuardRequest,
        cancel: McpAttemptCancellation,
    ) -> Result<Option<Box<dyn McpAttemptGuard>>, McpAttachmentRefusal> {
        let entries = self
            .live
            .snapshot()
            .map_err(|_| McpAttachmentRefusal::Unavailable)?;
        if self
            .live
            .stale(&request.server)
            .map_err(|_| McpAttachmentRefusal::Unavailable)?
        {
            return Err(McpAttachmentRefusal::NotCurrent);
        }
        for entry in entries {
            if entry.resource.server() != &request.server {
                continue;
            }
            self.live.current(&entry).map_err(|error| match error {
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    McpAttachmentRefusal::Unavailable
                }
                _ => McpAttachmentRefusal::NotCurrent,
            })?;
            return entry.context.begin_attempt(request, cancel).await;
        }
        self.delegate.begin(request, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn installed_customizer_refuses_shared_build_without_resource_custody() {
        use meerkat_core::{
            CompiledApplicationToolPolicy, CompiledMemberToolGrants,
            CompiledPolicySourceProvenance, PolicyDigest, PolicyId, PolicyProviderId,
            PolicyRevision,
        };
        use meerkat_mobkit::identity_first::{
            AgentBuildContext, AgentBuildDraft, AgentCustomizer, CustomizerError, DurableAgentSpec,
        };
        struct Callback;
        #[async_trait]
        impl AgentCustomizer for Callback {
            async fn customize_build(
                &self,
                _: &AgentBuildContext,
                _: &DurableAgentSpec,
                draft: &mut AgentBuildDraft,
            ) -> Result<(), CustomizerError> {
                draft.additional_instructions.push("actual callback".into());
                Ok(())
            }
        }
        let bytes = CompiledApplicationToolPolicy::new(
            PolicyProviderId::new("application").unwrap(),
            PolicyId::new("selected").unwrap(),
            PolicyRevision(1),
            CompiledPolicySourceProvenance {
                source_id: "selected".into(),
                source_digest: PolicyDigest::from_canonical_bytes(b"selected"),
            },
            vec![CompiledMemberToolGrants {
                member_identity: "ordinary".into(),
                grants: vec![],
            }],
        )
        .unwrap()
        .canonical_json()
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let live = LiveSharedSetup::new(
            RealmLocator {
                state_root: directory.path().to_owned(),
                realm: meerkat_core::mob_realm_id("application").unwrap(),
            },
            PreparedInstalledToolPolicy::from_canonical(&bytes).unwrap(),
            serde_json::from_value(
                serde_json::json!({"id":"application","profiles":{"shared":{"model":"fixture"}}}),
            )
            .unwrap(),
            &[
                meerkat_toolkit::host_lifetime::policy::CompiledMemberClass {
                    id: toolkit_package::Id::new("shared").unwrap(),
                    purpose: CompiledMemberClassPurpose::SharedWorkflow,
                    profile: "shared".into(),
                    factory: toolkit_package::Id::new("host:shared").unwrap(),
                    callback_account: None,
                    callbacks: vec![],
                    grants: vec![],
                },
            ],
        );
        let customizer = live.customizer(Arc::new(Callback));
        let context: AgentBuildContext = serde_json::from_value(serde_json::json!({
            "identity":"lead", "active_peers":[], "managed_edges":[]}))
        .unwrap();
        for (profile, accepted) in [("shared", false), ("ordinary", true)] {
            let spec: DurableAgentSpec =
                serde_json::from_value(serde_json::json!({"identity":"lead","profile":profile}))
                    .unwrap();
            let mut draft: AgentBuildDraft = serde_json::from_value(serde_json::json!({})).unwrap();
            assert_eq!(
                customizer
                    .customize_build(&context, &spec, &mut draft)
                    .await
                    .is_ok(),
                accepted
            );
            assert_eq!(draft.additional_instructions, ["actual callback"]);
            assert!(
                live.snapshot().unwrap().is_empty(),
                "callback cannot manufacture physical or approval custody"
            );
        }
        live.join_after_native_close(false).await.unwrap();
    }
    fn profile() -> meerkat_mob::Profile {
        serde_json::from_value(serde_json::json!({
            "model":"fixture-model", "provider":"anthropic",
            "skills":["retained-skill"], "peer_description":"selected helper",
            "tools":{"builtins":true,"comms":true,"read_only":true,
                "deny":["unselected_write"],"mcp":["reference"],
                "mcp_servers":[{"name":"other","command":"/retained/other","args":[]}],
                "rust_bundles":["retained-bundle"]}
        }))
        .unwrap()
    }
    fn peer() -> McpServerConfig {
        // Inert descriptor transformation control, not a physical resource or
        // native attachment witness. The online consumer must prove that join.
        McpServerConfig::stdio(
            crate::setup::SERVER_NAME,
            "/retained/python",
            vec![
                "-I".into(),
                "/retained/local_session.py".into(),
                "--shared-peer".into(),
                "/retained/peer-0".into(),
            ],
            Default::default(),
        )
    }
    #[test]
    fn fresh_profile_adds_only_exact_peer_and_preserves_prior_override() {
        let base = profile();
        let peer = peer();
        for supplied in [
            None,
            Some({
                let mut value = base.clone();
                value.tools.mcp.push("host-overlay".into());
                value
            }),
        ] {
            let mut expected = supplied.as_ref().unwrap_or(&base).clone();
            expected.tools.mcp_servers.push(peer.clone());
            assert_eq!(
                profile_with_peer(&base, supplied.as_ref(), &peer).unwrap(),
                expected
            );
        }
        assert_eq!(base, profile());
    }
    #[test]
    fn fresh_profile_refuses_named_saved_or_replaced_setup_before_mutation() {
        let base = profile();
        let peer = peer();
        for direct in [false, true] {
            let mut saved = base.clone();
            if direct {
                saved.tools.mcp_servers.push(peer.clone());
            } else {
                saved.tools.mcp.push(crate::setup::SERVER_NAME.into());
            }
            assert!(profile_with_peer(&saved, None, &peer).is_err());
            assert!(
                profile_with_peer(&saved, Some(&base), &peer).is_err(),
                "an override must not hide a conflicting configured base"
            );
            assert!(profile_with_peer(&base, Some(&saved), &peer).is_err());
        }
        let mut wrong = peer.clone();
        wrong.name = "other".into();
        assert!(profile_with_peer(&base, None, &wrong).is_err());
    }

    #[test]
    fn resumed_profile_replaces_only_exact_predecessor_and_preserves_other_tools() {
        let base = profile();
        let predecessor = peer();
        let mut successor = predecessor.clone();
        if let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) =
            &mut successor.transport
        {
            *launch.args.last_mut().unwrap() = "/retained/peer-1".into();
        }
        let mut saved = base.clone();
        saved.tools.mcp_servers.push(predecessor.clone());
        let selected = replacement_profile(Some(&saved), Some(&predecessor))
            .unwrap()
            .unwrap();
        assert_eq!(selected, base);
        let renewed = profile_with_peer(&base, Some(&selected), &successor).unwrap();
        assert_eq!(renewed.tools.mcp_servers.last(), Some(&successor));
        assert_eq!(saved.tools.mcp_servers.last(), Some(&predecessor));

        let mut wrong = saved.clone();
        *wrong.tools.mcp_servers.last_mut().unwrap() = successor.clone();
        assert!(replacement_profile(Some(&wrong), Some(&predecessor)).is_err());
        saved.tools.mcp_servers.push(predecessor.clone());
        assert!(replacement_profile(Some(&saved), Some(&predecessor)).is_err());
        let mut named = base.clone();
        named.tools.mcp.push(crate::setup::SERVER_NAME.into());
        let selected = replacement_profile(Some(&named), Some(&predecessor))
            .unwrap()
            .unwrap();
        assert!(profile_with_peer(&base, Some(&selected), &successor).is_err());
    }
}
