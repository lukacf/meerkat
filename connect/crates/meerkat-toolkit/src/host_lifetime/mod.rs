//! Toolkit activation/component lifetime. Native runtime work and grants have
//! their own owners; this module retains only selected deployment/component
//! close state and its physical settlement guards.

pub mod control;
pub(crate) mod fence;
pub mod policy;
mod request;

use control::*;
use fence::LocalFence;
use meerkat_authorization::publication::LocalAuthorizationPublication;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock as PhaseLock};
use tokio::sync::{Notify, OwnedRwLockReadGuard, RwLock};
use toolkit_catalog::{
    ComponentLifetimeInput, ComponentLifetimePhase, ToolkitComponentLifetimeMachineAuthority,
    ToolkitComponentLifetimeMachineMutator,
};
use toolkit_package::Id;
use uuid::Uuid;

type Result<T> = std::result::Result<T, HostLifetimeError>;

// Fixed local failure labels; no deployment, component or request data is emitted.
fn selection_refused(stage: &'static str, cause: &'static str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(),
        "{{\"event\":\"toolkit_host_selection_refused\",\"stage\":\"{stage}\",\"cause\":\"{cause}\"}}");
}
fn selection_owner_refused(stage: &'static str, error: HostLifetimeError) {
    selection_refused(stage, match error {
        HostLifetimeError::InvalidSelection => "InvalidSelection",
        HostLifetimeError::Unavailable => "Unavailable",
        HostLifetimeError::Denied => "Denied",
        HostLifetimeError::WrongOwner => "WrongOwner",
        HostLifetimeError::NotSettled => "NotSettled",
    });
}

struct ComponentOwner {
    // Callbacks and phase changes own entry write custody, including callback
    // panic poison. Pure observations read the generated machine instead.
    // This gate owns no phase facts or separate currentness stamp.
    entry: PhaseLock<()>,
    #[cfg(test)]
    entered_for_test: std::sync::OnceLock<std::sync::mpsc::SyncSender<()>>,
    // Only phase transitions take write guards. Read guards run no callback
    // and cannot mutate the generated machine. Callback poison lives in entry;
    // a panic during a phase mutation also poisons this lock.
    machine: PhaseLock<ToolkitComponentLifetimeMachineAuthority>,
    physical: Arc<RwLock<()>>,
}
impl ComponentOwner {
    fn stage(incarnation: Uuid, component: &str, digest: &str) -> Result<Self> {
        let mut machine = ToolkitComponentLifetimeMachineAuthority::new();
        machine
            .apply(ComponentLifetimeInput::Stage {
                incarnation: incarnation.to_string(),
                component: component.into(),
                activation_digest: digest.into(),
            })
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        Ok(Self {
            entry: PhaseLock::new(()),
            #[cfg(test)]
            entered_for_test: std::sync::OnceLock::new(),
            machine: PhaseLock::new(machine),
            physical: Arc::new(RwLock::new(())),
        })
    }

    // Read-only observations must retain the old fail-closed panic behavior
    // after a callback releases its phase guard but still owns entry.
    fn check_entry_poison(&self) -> Result<()> {
        if self.entry.is_poisoned() {
            Err(HostLifetimeError::Unavailable)
        } else {
            Ok(())
        }
    }

    fn open(
        &self,
        incarnation: Uuid,
        publication: Option<&LocalAuthorizationPublication>,
    ) -> Result<u64> {
        let _entry = self.entry.write().map_err(|_| HostLifetimeError::Unavailable)?;
        let _owner_poison = publication.map(LocalAuthorizationPublication::poison_on_unwind);
        let mut machine = self
            .machine
            .write()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let state = machine.state();
        if state.lifetime_phase == ComponentLifetimePhase::Open {
            return Ok(state.revision);
        }
        let expected_revision = state.revision;
        // Entry and phase first, then native publication. Setup may issue a
        // native grant while holding entry, so the reverse order would
        // deadlock with that existing synchronous owner contract.
        let _publication = publication
            .map(LocalAuthorizationPublication::begin_owner_change)
            .transpose()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        machine
            .apply(ComponentLifetimeInput::Open {
                incarnation: incarnation.to_string(),
                expected_revision,
            })
            .map_err(|_| HostLifetimeError::Denied)?;
        Ok(machine.state().revision)
    }

    fn retain(
        &self,
        incarnation: Uuid,
        expected_revision: Option<u64>,
    ) -> Result<(OwnedRwLockReadGuard<()>, u64)> {
        // Never wait for a phase writer while holding physical/file custody.
        // Pure observations share the generated machine's read guard; another
        // reader does not mean the installed selection has changed.
        let guard = self
            .physical
            .clone()
            .try_read_owned()
            .map_err(|_| {
                selection_refused("lifetime.component_retain.physical_try_read", "unavailable");
                HostLifetimeError::Unavailable
            })?;
        let machine = self
            .machine
            .try_read()
            .map_err(|error| {
                selection_refused("lifetime.component_retain.phase_try_lock", match error {
                    std::sync::TryLockError::WouldBlock => "would_block",
                    std::sync::TryLockError::Poisoned(_) => "poisoned",
                });
                HostLifetimeError::Unavailable
            })?;
        self.check_entry_poison()?;
        let state = machine.state();
        if state.incarnation != incarnation.to_string()
            || state.lifetime_phase != ComponentLifetimePhase::Open
            || expected_revision.is_some_and(|expected| expected != state.revision)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok((guard, state.revision))
    }

    /// Internal phase observation only. A finite machine read excludes actual
    /// phase writes without borrowing callback entry. No callback or await runs
    /// under this guard. Native callers must also validate the bound publication.
    fn observe_open(
        &self,
        incarnation: Uuid,
        expected_revision: Option<u64>,
    ) -> Result<std::sync::RwLockReadGuard<'_, ToolkitComponentLifetimeMachineAuthority>> {
        // Never wait under a native publication or file fence. Healthy setup
        // callbacks keep entry exclusive but do not mutate this phase machine.
        let machine = self.machine.try_read().map_err(|error| {
            selection_refused(
                "lifetime.component_observe.phase_try_read",
                match error {
                    std::sync::TryLockError::WouldBlock => "would_block",
                    std::sync::TryLockError::Poisoned(_) => "poisoned",
                },
            );
            HostLifetimeError::Unavailable
        })?;
        self.check_entry_poison()?;
        let state = machine.state();
        if state.incarnation != incarnation.to_string()
            || state.lifetime_phase != ComponentLifetimePhase::Open
            || expected_revision.is_some_and(|expected| expected != state.revision)
        {
            return Err(HostLifetimeError::Denied);
        }
        #[cfg(test)]
        if let Some(entered) = self.entered_for_test.get() {
            let _ = entered.try_send(());
        }
        Ok(machine)
    }

    /// The operation, not a reusable observation, owns the close boundary.
    /// Entry try-write is required because callers may already hold M/file fences.
    fn with_open<T>(
        &self,
        incarnation: Uuid,
        expected_revision: Option<u64>,
        publication: Option<&LocalAuthorizationPublication>,
        operation: impl FnOnce() -> T,
    ) -> Result<T> {
        let _entry = self.entry.try_write().map_err(|error| {
            selection_refused("lifetime.component_entry.entry_try_write", match error {
                std::sync::TryLockError::WouldBlock => "would_block",
                std::sync::TryLockError::Poisoned(_) => "poisoned",
            });
            HostLifetimeError::Unavailable
        })?;
        // This native sentinel never acquires the publication writer. It must
        // precede phase reads and callbacks, and unwind before entry releases.
        let _owner_poison = publication.map(LocalAuthorizationPublication::poison_on_unwind);
        // A test can observe real acquisition without contending on entry.
        #[cfg(test)]
        if let Some(entered) = self.entered_for_test.get() {
            let _ = entered.try_send(());
        }
        // Entry excludes every phase writer and blocking close observer.
        // Read-only retain may share this guard, without waiting for entry or
        // invoking a callback. No writer can wait on this read while owning
        // a publication/file fence needed by the caller.
        let machine = self
            .machine
            .read()
            .map_err(|_| {
                selection_refused("lifetime.component_entry.phase_lock", "poisoned");
                HostLifetimeError::Unavailable
            })?;
        let state = machine.state();
        if state.incarnation != incarnation.to_string()
            || state.lifetime_phase != ComponentLifetimePhase::Open
            || expected_revision.is_some_and(|expected| expected != state.revision)
        {
            return Err(HostLifetimeError::Denied);
        }
        // Every phase writer takes entry first. Retain can read this same
        // machine while the callback runs, but close and other callbacks wait
        // or refuse at entry. No cached phase or deferred operation is created.
        drop(machine);
        Ok(operation())
    }

    fn begin_close(
        &self,
        incarnation: Uuid,
        scope: HostCloseScope,
        reason: HostCloseReason,
        publication: Option<&LocalAuthorizationPublication>,
    ) -> Result<HostCloseObservation> {
        let _entry = self.entry.write().map_err(|_| HostLifetimeError::Unavailable)?;
        let _owner_poison = publication.map(LocalAuthorizationPublication::poison_on_unwind);
        let mut machine = self
            .machine
            .write()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let state = machine.state();
        if matches!(
            state.lifetime_phase,
            ComponentLifetimePhase::Staged | ComponentLifetimePhase::Open
        ) {
            let expected_revision = state.revision;
            // Never hold a publication writer while waiting for entry/phase.
            // The publication covers this exact transition and ends before
            // the caller acquires any other component's entry/phase.
            let _publication = publication
                .map(LocalAuthorizationPublication::begin_owner_change)
                .transpose()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            machine
                .apply(ComponentLifetimeInput::BeginClose {
                    incarnation: incarnation.to_string(),
                    expected_revision,
                    close_id: Uuid::new_v4().to_string(),
                    reason: format!("{reason:?}"),
                    quarantine: reason == HostCloseReason::Quarantine,
                })
                .map_err(|_| HostLifetimeError::Denied)?;
        }
        Self::close_observation(&machine, incarnation, scope)
    }

    fn close_observation(
        machine: &ToolkitComponentLifetimeMachineAuthority,
        incarnation: Uuid,
        scope: HostCloseScope,
    ) -> Result<HostCloseObservation> {
        let state = machine.state();
        if state.incarnation != incarnation.to_string() {
            return Err(HostLifetimeError::WrongOwner);
        }
        let (phase, failure) = match state.lifetime_phase {
            ComponentLifetimePhase::Closing => (HostCloseState::Closing, None),
            ComponentLifetimePhase::Closed | ComponentLifetimePhase::Quarantined => {
                (HostCloseState::Settled, None)
            }
            ComponentLifetimePhase::Uncertain => (
                HostCloseState::Uncertain,
                Some(HostCloseFailure::PhysicalSettlementUncertain),
            ),
            ComponentLifetimePhase::Failed => {
                (HostCloseState::Failed, Some(HostCloseFailure::OwnerFailed))
            }
            _ => return Err(HostLifetimeError::NotSettled),
        };
        Ok(HostCloseObservation {
            handle: HostCloseHandle {
                incarnation,
                close_id: Uuid::parse_str(&state.close_id)
                    .map_err(|_| HostLifetimeError::Unavailable)?,
                scope,
            },
            state: phase,
            failure,
        })
    }

    fn observe(
        &self,
        handle: &HostCloseHandle,
        publication: Option<&LocalAuthorizationPublication>,
    ) -> Result<HostCloseObservation> {
        let _entry = self.entry.write().map_err(|_| HostLifetimeError::Unavailable)?;
        let _owner_poison = publication.map(LocalAuthorizationPublication::poison_on_unwind);
        let machine = self
            .machine
            .read()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let observation =
            Self::close_observation(&machine, handle.incarnation, handle.scope.clone())?;
        if observation.handle != *handle {
            return Err(HostLifetimeError::WrongOwner);
        }
        Ok(observation)
    }

    async fn join(
        &self,
        handle: &HostCloseHandle,
        publication: Option<&LocalAuthorizationPublication>,
    ) -> Result<HostCloseObservation> {
        self.observe(handle, publication)?;
        // This is the actual operation guard, not a counter or task snapshot.
        // Every entered bounded physical callback retains its read guard through
        // execution, cancellation settlement and the result-release decision.
        let _settled = self.physical.clone().write_owned().await;
        let _entry = self.entry.write().map_err(|_| HostLifetimeError::Unavailable)?;
        let _owner_poison = publication.map(LocalAuthorizationPublication::poison_on_unwind);
        let mut machine = self
            .machine
            .write()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let observed = Self::close_observation(&machine, handle.incarnation, handle.scope.clone())?;
        if observed.handle != *handle {
            return Err(HostLifetimeError::WrongOwner);
        }
        if observed.state == HostCloseState::Closing {
            let expected_revision = machine.state().revision;
            machine
                .apply(ComponentLifetimeInput::ConfirmSettlement {
                    incarnation: handle.incarnation.to_string(),
                    expected_revision,
                    close_id: handle.close_id.to_string(),
                })
                .map_err(|_| HostLifetimeError::Unavailable)?;
        }
        Self::close_observation(&machine, handle.incarnation, handle.scope.clone())
    }
}

/// The actual process-local owner installed only by the private bootstrap.
/// The immutable selection is checked again under the existing deny.lock at
/// every entry and release; it cannot preserve permission after activation move.
pub struct NativeHostLifetime {
    selection: HostBootstrapSelection,
    incarnation: Uuid,
    policy: policy::CompiledLocalPolicy,
    policy_revision: toolkit_package::Revision,
    deny_fence: LocalFence,
    activation: Arc<ComponentOwner>,
    components: BTreeMap<Id, Arc<ComponentOwner>>,
    closed: Notify,
    native_publication: Option<LocalAuthorizationPublication>,
}

impl NativeHostLifetime {
    /// Bounded installer work under the existing deployment entry fence.
    /// This creates no runtime lifetime and does not relax current selection.
    /// The trusted application must hold its stopped-host deployment transaction;
    /// the callback must complete synchronously without returning a live owner.
    /// Native owner creation additionally enforces its own exclusive store lease.
    pub fn with_candidate_installation<T>(
        selection: &HostBootstrapSelection,
        operation: impl FnOnce(&policy::CompiledLocalPolicy) -> Result<T>,
    ) -> Result<T> {
        let fence = LocalFence::open(selection.deployment_root.join("shared/control/deny.lock"))?;
        let _exclusive = fence.exclusive()?;
        let (policy, _) = policy::validate_candidate_selection(selection)?;
        operation(&policy)
    }

    pub fn bootstrap(selection: HostBootstrapSelection) -> Result<Self> {
        let deny_fence =
            LocalFence::open(selection.deployment_root.join("shared/control/deny.lock"))?;
        let _selected = deny_fence.shared()?;
        let (policy, revision) = policy::validate_selection(&selection)?;
        let incarnation = Uuid::new_v4();
        let mut components = BTreeMap::new();
        for id in &policy.components {
            components.insert(
                id.clone(),
                Arc::new(ComponentOwner::stage(
                    incarnation,
                    id.as_str(),
                    &selection.activation.sha256,
                )?),
            );
        }
        let activation = Arc::new(ComponentOwner::stage(
            incarnation,
            "activation",
            &selection.activation.sha256,
        )?);
        activation.open(incarnation, None)?;
        Ok(Self {
            selection,
            incarnation,
            policy,
            policy_revision: toolkit_package::Revision::new(revision)
                .map_err(|_| HostLifetimeError::InvalidSelection)?,
            deny_fence,
            activation,
            components,
            closed: Notify::new(),
            native_publication: None,
        })
    }

    fn component(&self, id: &Id) -> Result<&Arc<ComponentOwner>> {
        self.components.get(id).ok_or(HostLifetimeError::Denied)
    }

    /// Fixed order: activation entry, then exact component entry. Both actual
    /// close writers need these exclusive gates. The bounded callback cannot reenter
    /// this owner, await, or detach work. Its result is produced while open.
    fn with_component_open<T>(
        &self,
        component: &Id,
        activation_revision: Option<u64>,
        component_revision: Option<u64>,
        operation: impl FnOnce() -> T,
    ) -> Result<T> {
        self.activation
            .with_open(self.incarnation, activation_revision, self.native_publication.as_ref(), || {
                self.component(component)?.with_open(
                    self.incarnation,
                    component_revision,
                    self.native_publication.as_ref(),
                    operation,
                )
            })?
    }

    fn check_selected(&self) -> Result<()> {
        let (policy, revision) = policy::validate_selection(&self.selection)?;
        if policy != self.policy || revision != self.policy_revision.as_str() {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    /// Wait for close admission, without manufacturing settlement. The private
    /// gateway then joins this same owner's actual close handle.
    pub async fn wait_closed(&self) {
        loop {
            let notified = self.closed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // Preserve the old blocking observation and callback-poison wake.
            // Both guards end before waiting for a later close notification.
            let closed = match self.activation.entry.write() {
                Ok(_entry) => {
                    let _owner_poison = self.native_publication.as_ref()
                        .map(LocalAuthorizationPublication::poison_on_unwind);
                    self.activation.machine.read()
                        .map(|m| m.state().lifetime_phase != ComponentLifetimePhase::Open)
                        .unwrap_or(true)
                },
                Err(_) => true,
            };
            if closed {
                return;
            }
            notified.await;
        }
    }

    /// Install the existing native grant owner's publication before this
    /// lifetime is shared. This is synchronization, not a new policy owner.
    /// Rebinding would leave previously prepared native work on another stamp.
    pub fn bind_native_authorization_publication(
        &mut self,
        publication: LocalAuthorizationPublication,
    ) -> Result<()> {
        self.validate_current_selection()?;
        if self.native_publication.is_some() {
            return Err(HostLifetimeError::Denied);
        }
        self.native_publication = Some(publication);
        Ok(())
    }

    /// Local phase facts for the native authorization observation. The caller
    /// must use the bound publication. The installed selection was validated
    /// before composition; replacement must close this actual owner first.
    /// No filesystem operation or independently maintained closed flag is used.
    pub fn validate_native_entry(&self, component: Option<&Id>) -> Result<()> {
        if self.native_publication.is_none() {
            return Err(HostLifetimeError::Unavailable);
        }
        // Observation has no callback/effect. Share phase readers in the same
        // activation-then-component order; actual phase writers still refuse it.
        // Callback mutual exclusion and native grant currentness have their own
        // existing entry and publication owners.
        let _activation = self.activation.observe_open(self.incarnation, None)?;
        if let Some(component) = component {
            let _component = self.component(component)?.observe_open(self.incarnation, None)?;
        }
        Ok(())
    }

    /// Copy the exact current policy for a pure installed source-facts read.
    /// No caller callback, authority mutation or deferred operation runs here.
    /// A caller reading another owner must reobserve this selection afterward
    /// and discard its result if selection or lifetime currentness has changed.
    pub fn current_setup_policy(&self) -> Result<policy::CompiledLocalPolicy> {
        let _fence = self.deny_fence.shared()?;
        self.check_selected()?;
        let _activation = self.activation.observe_open(self.incarnation, None)?;
        Ok(self.policy.clone())
    }

    /// Enter a native administration operation with this activation's actual
    /// close fence held through the synchronous authority check and reservation.
    /// The native writer already owns its mutation custody. The callback must
    /// not wait, await, reenter this owner, or perform filesystem observations;
    /// it may return the existing login reservation for native settlement.
    /// This is not the grant-publication observation in validate_native_entry.
    pub fn with_native_administration_entry<T>(&self, enter: impl FnOnce() -> T) -> Result<T> {
        self.activation.with_open(self.incarnation, None, self.native_publication.as_ref(), enter)
    }

    pub fn validate_current_selection(&self) -> Result<()> {
        let _fence = self.deny_fence.shared().map_err(|error| {
            selection_owner_refused("lifetime.current_selection.deny_fence", error);
            error
        })?;
        self.check_selected().map_err(|error| {
            selection_owner_refused("lifetime.current_selection.selected_files", error);
            error
        })?;
        let (_activation, _) = self.activation.retain(self.incarnation, None).map_err(|error| {
            selection_owner_refused("lifetime.current_selection.activation_retain", error);
            error
        })?;
        Ok(())
    }

    /// Retain the real selected-file reservation through one synchronous native
    /// consent decision, its store commit and final allowance check. Local phase
    /// observation finishes before the callback: the native eligibility check
    /// must not reenter a phase mutex already held by this adapter.
    ///
    /// The callback must not await, detach work or return deferred decision work.
    /// Release this reservation before human waiting or later async dispatch.
    /// Deployment replacement must still close/join this host before changing
    /// its selected files; this reservation is not an independent writer policy.
    pub fn with_current_consent<T>(&self, decide: impl FnOnce() -> T) -> Result<T> {
        let _fence = self.deny_fence.shared()?;
        self.check_selected()?;
        self.validate_native_entry(None)?;
        Ok(decide())
    }

    /// Execute a bounded synchronous operator setup mutation while this exact
    /// installed selection and activation remain open. The callback must not
    /// await, detach work or reenter this owner. The native store lease remains
    /// the cross-process owner exclusion; this retains the existing close fence.
    /// Native issuance may acquire its own publication writer. Phase changes
    /// acquire entry and phase before that writer, never the reverse order.
    pub fn with_current_setup<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let _fence = self.deny_fence.shared()?;
        self.check_selected()?;
        self.activation
            .with_open(self.incarnation, None, self.native_publication.as_ref(), operation)?
    }

    /// Application-selected destination facts only. The native registration
    /// supplies its actual session document and logical runtime later.
    pub fn destination_for_member(
        &self,
        realm: &meerkat_core::connection::RealmId,
        member: &meerkat_core::connection::MobMemberBinding,
    ) -> Result<policy::CompiledNativeMember> {
        self.validate_current_selection()?;
        let native = self
            .policy
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        if &native.realm != realm {
            return Err(HostLifetimeError::Denied);
        }
        native
            .members
            .iter()
            .find(|row| &row.member == member)
            .cloned()
            .ok_or(HostLifetimeError::Denied)
    }

    /// Atomically publish the already-constructed gateway while admission is
    /// still open. The callback may synchronously start the already-bound
    /// server and consume the reserved ready-queue permit; it must never await,
    /// perform model/tool work, or reenter this owner. No permit escapes.
    pub fn publish_gateway_ready(&self, publish: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        let _fence = self.deny_fence.shared()?;
        self.check_selected()?;
        self.activation.with_open(self.incarnation, None, self.native_publication.as_ref(), publish)?
    }

    pub fn selected_policy(&self) -> &policy::CompiledLocalPolicy {
        &self.policy
    }
}

#[async_trait::async_trait]
impl PrivateHostLifetimeControl for NativeHostLifetime {
    fn validate_current_selection(&self) -> Result<()> {
        NativeHostLifetime::validate_current_selection(self)
    }

    fn open_component(&self, component: &Id) -> Result<HostComponentObservation> {
        let _fence = self.deny_fence.shared()?;
        self.check_selected()?;
        let revision = self.activation.with_open(self.incarnation, None, self.native_publication.as_ref(), || {
            self.component(component)?
                .open(self.incarnation, self.native_publication.as_ref())
        })??;
        Ok(HostComponentObservation {
            incarnation: self.incarnation,
            component: component.clone(),
            revision,
            state: HostComponentState::Open,
        })
    }

    fn begin_close_component(
        &self,
        component: &Id,
        reason: HostCloseReason,
    ) -> Result<HostCloseObservation> {
        self.component(component)?.begin_close(
            self.incarnation,
            HostCloseScope::Component {
                component: component.clone(),
            },
            reason,
            self.native_publication.as_ref(),
        )
    }

    fn begin_close_activation(&self, reason: HostCloseReason) -> Result<HostCloseObservation> {
        let observation = self.activation.begin_close(
            self.incarnation,
            HostCloseScope::Activation,
            reason,
            self.native_publication.as_ref(),
        )?;
        // Admission is closed and its publication guard released before
        // waiting for any component. Every component publishes its own close.
        self.closed.notify_waiters();
        for (id, component) in &self.components {
            component.begin_close(
                self.incarnation,
                HostCloseScope::Component {
                    component: id.clone(),
                },
                reason,
                self.native_publication.as_ref(),
            )?;
        }
        Ok(observation)
    }

    fn observe_close(&self, handle: &HostCloseHandle) -> Result<HostCloseObservation> {
        if handle.incarnation != self.incarnation {
            return Err(HostLifetimeError::WrongOwner);
        }
        match &handle.scope {
            HostCloseScope::Activation => self.activation.observe(handle, self.native_publication.as_ref()),
            HostCloseScope::Component { component } => self.component(component)?
                .observe(handle, self.native_publication.as_ref()),
        }
    }

    async fn join_close(&self, handle: &HostCloseHandle) -> Result<HostCloseObservation> {
        if handle.incarnation != self.incarnation {
            return Err(HostLifetimeError::WrongOwner);
        }
        match &handle.scope {
            HostCloseScope::Activation => {
                self.activation.observe(handle, self.native_publication.as_ref())?;
                for (id, component) in &self.components {
                    let observation = {
                        let _entry = component.entry.write()
                            .map_err(|_| HostLifetimeError::Unavailable)?;
                        let _owner_poison = self.native_publication.as_ref()
                            .map(LocalAuthorizationPublication::poison_on_unwind);
                        let machine = component
                            .machine
                            .read()
                            .map_err(|_| HostLifetimeError::Unavailable)?;
                        ComponentOwner::close_observation(
                            &machine,
                            self.incarnation,
                            HostCloseScope::Component {
                                component: id.clone(),
                            },
                        )?
                    };
                    let result = component.join(&observation.handle, self.native_publication.as_ref()).await?;
                    if result.state != HostCloseState::Settled {
                        return Err(HostLifetimeError::NotSettled);
                    }
                }
                self.activation.join(handle, self.native_publication.as_ref()).await
            }
            HostCloseScope::Component { component } => {
                self.component(component)?.join(handle, self.native_publication.as_ref()).await
            }
        }
    }

    fn host_lost(&self) -> Result<HostCloseObservation> {
        self.begin_close_activation(HostCloseReason::HostLost)
    }
}

#[cfg(all(test, unix))]
mod tests;
