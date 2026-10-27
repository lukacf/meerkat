//! The existing identity runtime's real setup-helper preparation and input path.
//! WorkGraph admission precedes every call here. No frontend event is fabricated.

use super::*;
use crate::original_ingress::{
    PendingToolkitOriginal,
    helper_originals::{
        AdmittedHelperWork, HelperAdmissionSelection, HelperNativeBinding, HelperOperation,
        RetainedHelperSource,
    },
};
use async_trait::async_trait;
use meerkat_authorization_contracts::{
    constraints::ExecutionRestrictions, grant::GrantLineageRef, work_association::{NativeWorkTarget, WorkAuthorityBasis},
};
use meerkat_core::{ControllerModelClient, SessionId, ToolDispatchContext, types::ToolCallView};
use meerkat_mobkit::identity_first::{
    BoundGovernedInput, GovernedAdmission, GovernedBindContext, GovernedInputBinder,
    GovernedInputError, GovernedSettlement, IdentityRuntimeError, TurnTracking,
};
use meerkat_runtime::{Input, PromptInput};
use meerkat_workgraph::{CreateWorkItemRequest, WorkAdmissionKey, WorkGraphService};
use toolkit_package::ContentDigest;

mod controller;
mod entry;

/// An actual retained native controller/registration observation. This is not
/// a grant, source custody or proof that the setup helper was admitted.
pub(crate) struct PreparedHelperController {
    source: Weak<PrivateOriginals>,
    admitted: Arc<AdmittedHelperWork>,
    session_id: SessionId,
    target: NativeWorkTarget,
    controller: ControllerModelClient,
}

impl PreparedHelperController {
    pub(crate) fn controller(&self) -> &ControllerModelClient {
        &self.controller
    }
    pub(crate) fn admitted(&self) -> &Arc<AdmittedHelperWork> {
        &self.admitted
    }
    pub(crate) fn target(&self) -> &NativeWorkTarget {
        &self.target
    }
}

/// Helper-local admission observation. Exhausted reprepare proves only that
/// this exact invocation was not admitted; it starts no background replay.
pub(crate) enum SetupHelperSubmission {
    Native(PrivateOriginalSubmission),
    ReprepareRequired,
}

/// Kept by the existing source adapter when native cannot settle admission.
/// Its guard owns only source custody. Native remains the input/turn owner.
pub(crate) struct HeldSetupHelper {
    pub(super) source: RetainedHelperSource,
    first_input: OnceLock<cold_service_admission::RetainedBoundInput>,
    pub(super) first_custody: Mutex<Option<Arc<PendingHelperAdmission>>>,
    unresolved: Mutex<Vec<UnresolvedHelperAdmission>>,
    // Bound once from the canonical live setup before input admission. Never
    // reconstructed from source history or refreshed by human approval.
    operation_deadline_ms: OnceLock<u64>,
}

#[cfg(test)]
pub(crate) struct HelperHandoffObservation {
    pub session: SessionId,
    pub input: serde_json::Value,
    pub first_candidate: bool,
    pub native_admission: Option<GovernedAdmission>,
    pub source_acknowledged: Option<bool>,
}

impl HeldSetupHelper {
    pub(crate) fn original(&self) -> &Arc<RetainedToolkitOriginal> {
        &self.source.original
    }
    pub(crate) fn source_record(&self) -> &Arc<StoredRecord> {
        &self.source.source_record
    }

    pub(super) fn check_result_record(
        &self, saved: &toolkit_store::StoredRecord, session: &SessionId,
    ) -> Result<(), PrivateOriginalError> {
        // First-input bytes are retained even without optional recovery
        // enrollment. Native result lookup has already checked this session.
        self.source.check_result_record(saved,
            self.first_input.get().map(|input| (input, session))).map_err(helper_error)
    }

    #[cfg(test)]
    pub(crate) fn handoffs_for_test(&self) -> Vec<Option<HelperHandoffObservation>> {
        self.unresolved.lock().unwrap().iter().map(|retained| {
            let pending = retained.pending.as_ref()?;
            let publication = pending.publication.lock().unwrap();
            Some(HelperHandoffObservation {
                session: pending.session.clone(), input: serde_json::to_value(&pending.input).unwrap(),
                first_candidate: pending.first_candidate,
                native_admission: publication.native_admission.clone(),
                source_acknowledged: match &publication.source {
                    governed_submit::SourcePublication::Pending(_) => None,
                    governed_submit::SourcePublication::Settled { source_acknowledged, .. } =>
                        Some(*source_acknowledged),
                },
            })
        }).collect()
    }

    fn bind_setup_deadline(
        &self,
        snapshot: &meerkat_toolkit::setup::SetupSnapshot,
    ) -> Result<(), PrivateOriginalError> {
        let operation = self.source.policy_source().map_err(helper_error)?.operation;
        retain_setup_snapshot_deadline(&self.operation_deadline_ms, &operation, snapshot)
    }
}

fn retain_setup_snapshot_deadline(
    held: &OnceLock<u64>, operation: &HelperOperation,
    snapshot: &meerkat_toolkit::setup::SetupSnapshot,
) -> Result<(), PrivateOriginalError> {
    let input = canonical_json(snapshot.request().request())
        .map_err(|_| PrivateOriginalError::Conflict)?;
    if operation.acquisition != *snapshot.key()
        || operation.input_digest != ContentDigest::of_bytes(&input)
        || snapshot.state().terminal_revision != 0
    {
        return Err(PrivateOriginalError::Denied);
    }
    retain_exact_setup_deadline(held, snapshot.request().host().limits.deadline_ms)
}

// Expose only the production data-binding check to existing canonical-store
// controls. This cannot construct a helper or establish runtime custody.
#[cfg(test)]
pub(crate) fn bind_snapshot_deadline_fixture(
    held: &OnceLock<u64>, operation: &HelperOperation,
    snapshot: &meerkat_toolkit::setup::SetupSnapshot,
) -> Result<(), PrivateOriginalError> {
    retain_setup_snapshot_deadline(held, operation, snapshot)
}

// Idempotence preserves the original deadline; replay cannot extend it.
fn retain_exact_setup_deadline(
    held: &OnceLock<u64>, deadline_ms: u64,
) -> Result<(), PrivateOriginalError> {
    let retained = held.get_or_init(|| deadline_ms);
    if *retained != deadline_ms { return Err(PrivateOriginalError::Conflict); }
    Ok(())
}

impl PrivateOriginals {
    #[cfg(test)]
    pub(crate) fn canonical_store_for_policy_fixture(&self) -> &dyn ToolkitStore {
        self.store.as_ref()
    }

    /// Retain an actual canonical helper source for policy-component controls.
    /// This neither promotes input custody nor installs a permissive owner.
    #[cfg(test)]
    pub(crate) fn retain_helper_policy_fixture(&self, source: RetainedHelperSource) {
        self.retain_helper_policy_deadline_fixture(source, Some(u64::MAX));
    }

    #[cfg(test)]
    pub(crate) fn retain_helper_policy_deadline_fixture(
        &self, source: RetainedHelperSource, deadline_ms: Option<u64>,
    ) {
        source.policy_source().unwrap();
        let operation_deadline_ms = OnceLock::new();
        if let Some(deadline) = deadline_ms { operation_deadline_ms.set(deadline).unwrap(); }
        self.helper_sources.lock().unwrap().push(Arc::new(HeldSetupHelper {
            source, first_input: OnceLock::new(), first_custody: Mutex::new(None),
            unresolved: Mutex::new(Vec::new()), operation_deadline_ms,
        }));
    }

    /// Live operation limit of this exact held helper, not a historical grant.
    /// Missing live binding refuses helper effects; it is never synthesized.
    pub(crate) fn helper_operation_deadline(
        &self,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<Option<u64>, PrivateOriginalError> {
        let held = self.helper_sources.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut matches = held.iter().filter(|source| source.original().association() == association);
        let Some(source) = matches.next() else { return Ok(None); };
        if matches.next().is_some() { return Err(PrivateOriginalError::Conflict); }
        source.operation_deadline_ms.get().copied().map(Some)
            .ok_or(PrivateOriginalError::Unavailable)
    }

    /// Exact immutable source facts from a handle this source owner retained.
    /// The native input and grant owners still decide current custody/permission.
    /// No storage read or native lock is taken under the caller's publication.
    pub(crate) fn helper_policy_source(
        &self,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<Option<crate::original_ingress::helper_originals::HelperPolicySource>, PrivateOriginalError> {
        let held = self.helper_sources.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut matches = held.iter().filter(|source| source.original().association() == association);
        let Some(source) = matches.next() else { return Ok(None); };
        if matches.next().is_some() { return Err(PrivateOriginalError::Conflict); }
        source.source.policy_source().map(Some).map_err(helper_error)
    }

    /// Pure currentness check for the retained private owner. No callback or
    /// disclosure runs here; actual invocation and native binding remain the
    /// enrolled leaf's separate entry checks.
    pub(crate) fn validate_private_setup_owner(
        &self,
        realm: &str,
        source_owner: &str,
    ) -> Result<(), PrivateOriginalError> {
        if self.scope.realm != realm || self.scope.owner != source_owner {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        // Enrollment recovers the same retained owner; it does not disclose
        // policy or authorize an invocation. Ordinary setup issuance leaves
        // that owner current. Keep the separate native and source-entry checks.
        lifetime
            .validate_current_selection()
            .map_err(|error| match error {
                meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable => {
                    PrivateOriginalError::Unavailable
                }
                _ => PrivateOriginalError::Denied,
            })
    }

    /// Selected application's private settlement custody only. The caller is
    /// the installed acquisition owner using this same source/store scope;
    /// it has already validated the immutable canonical effect/input. This
    /// guard is not a native WorkGraph read grant or permission to retry.
    /// The callback moves an immutable private projection and performs no I/O.
    pub(crate) fn with_private_setup_recovery(
        &self,
        realm: &str,
        source_owner: &str,
        disclose: Box<dyn FnOnce() + Send + '_>,
    ) -> Result<(), PrivateOriginalError> {
        if self.scope.realm != realm || self.scope.owner != source_owner {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .with_current_setup(|| {
                disclose();
                Ok(())
            })
            .map_err(|error| match error {
                meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable => {
                    PrivateOriginalError::Unavailable
                }
                _ => PrivateOriginalError::Denied,
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn admit_setup_helper(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        service: &WorkGraphService,
        key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        operation: HelperOperation,
        selection: HelperAdmissionSelection,
    ) -> Result<Arc<AdmittedHelperWork>, PrivateOriginalError> {
        let original = self.acquisition_original(context).await?;
        self.require_helper_selection(&selection)?;
        self.owner
            .admit_fenced_helper(
                call,
                context,
                self.store.as_ref(),
                &self.scope,
                service,
                key,
                request,
                operation,
                original.setup_origin(),
                &original.original,
                &original.contributors,
                &original.own_originals,
                selection,
            )
            .await
            .map(Arc::new)
            .map_err(helper_error)
    }

    #[allow(clippy::too_many_arguments)]
    /// Current M1 replay disclosure for a live invocation only. This is not
    /// independent host-private recovery after originating work is cancelled.
    pub(crate) async fn observe_setup_helper_for_live_replay(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        service: &WorkGraphService,
        key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        operation: HelperOperation,
        selection: HelperAdmissionSelection,
    ) -> Result<Option<Arc<AdmittedHelperWork>>, PrivateOriginalError> {
        let original = self.acquisition_original(context).await?;
        self.require_helper_selection(&selection)?;
        self.owner
            .observe_fenced_helper(
                call,
                context,
                self.store.as_ref(),
                &self.scope,
                service,
                key,
                request,
                operation,
                original.setup_origin(),
                &original.original,
                &original.contributors,
                &original.own_originals,
                selection,
            )
            .await
            .map(|observed| observed.map(Arc::new))
            .map_err(helper_error)
    }

    fn require_helper_selection(
        &self,
        selection: &HelperAdmissionSelection,
    ) -> Result<(), PrivateOriginalError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let policy = lifetime.selected_policy();
        let native = policy
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if self.scope.realm != native.realm.to_string()
            || self.scope.owner != policy.deployment.to_string()
            || self.owner.ingress_domain() != &native.ingress_domain
            || selection.grant_caller != native.grant_administrator
            || selection.helper.member != selection.member.member
            || !native
                .members
                .iter()
                .any(|member| member == &selection.member)
        {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(())
    }

    /// The real native identity lease supplies its client and session. The
    /// binder returns the typed Bind branch before submitting any native input.
    /// This is called only after fenced work admission, so a cold helper is
    /// never created before acquisition coalescing has selected its one item.
    pub(crate) async fn prepare_setup_helper(
        self: &Arc<Self>,
        admitted: Arc<AdmittedHelperWork>,
        content: String,
        source_deadline: tokio::time::Instant,
    ) -> Result<PreparedHelperController, PrivateOriginalError> {
        // The native acceptance callback is independent of this tool's turn.
        // Wait before any Meta identity call, with no mutation/publication
        // guard held. Each observation performs the same strict contributor
        // check; the native binder and publication paths still recheck below.
        self.owner
            .wait_admitted_helper(&admitted, source_deadline)
            .await
            .map_err(helper_error)?;
        self.require_helper_selection(admitted.selection())?;
        if ContentDigest::of_bytes(content.as_bytes()) != admitted.operation().input_digest {
            return Err(PrivateOriginalError::Conflict);
        }
        let runtime = self
            .identity_runtime
            .get()
            .and_then(|runtime| runtime.upgrade())
            .ok_or(PrivateOriginalError::Unavailable)?;
        let alias = admitted.selection().member.member.member.clone();
        let identity = runtime
            .owned_identity_for_member_alias(&alias)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let binder = Arc::new(PrepareHelperBinder {
            source: Arc::downgrade(self),
            admitted,
            captured: Mutex::new(None),
        });
        let outcome = runtime
            .dispatch_governed_with_turn_ticket(
                &identity,
                Some(&alias),
                Input::Prompt(PromptInput::new(content, None)),
                binder.clone(),
            )
            .await;
        if !matches!(
            outcome,
            Err(IdentityRuntimeError::GovernedInput(
                GovernedInputError::Bind(_)
            ))
        ) {
            return Err(PrivateOriginalError::Unavailable);
        }
        let captured = binder
            .captured
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .take();
        captured.ok_or(PrivateOriginalError::Unavailable)
    }

    /// Recover only a previously retained source for this exact work and
    /// actual native controller/session. A missing source stays missing; this
    /// method issues no grant and cannot replace the saved destination.
    pub(crate) async fn recover_setup_helper_source(
        self: &Arc<Self>,
        prepared: &PreparedHelperController,
    ) -> Result<Option<Arc<HeldSetupHelper>>, PrivateOriginalError> {
        if !prepared
            .source
            .upgrade()
            .is_some_and(|source| Arc::ptr_eq(&source, self))
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.require_helper_selection(prepared.admitted.selection())?;
        let Some(binding) = self
            .owner
            .observe_helper_binding(self.store.as_ref(), &prepared.admitted)
            .await
            .map_err(helper_error)?
        else {
            return Ok(None);
        };
        if binding.target != prepared.target
            || &binding.controller_model != prepared.controller.selection()
        {
            return Err(PrivateOriginalError::Conflict);
        }
        self.retain_setup_helper_source(prepared, binding.controller_grant_lineage, binding.authority_basis)
            .await
            .map(Some)
    }

    /// The actual native child issuer supplies this lineage. This method never
    /// issues a grant or treats its configured parent as a runnable child.
    /// Resolve the issued path with the real owner, then retain the exact source.
    pub(crate) async fn retain_setup_helper_source(
        self: &Arc<Self>,
        prepared: &PreparedHelperController,
        issued_controller_lineage: Vec<GrantLineageRef>,
        operation_basis: WorkAuthorityBasis,
    ) -> Result<Arc<HeldSetupHelper>, PrivateOriginalError> {
        if !prepared
            .source
            .upgrade()
            .is_some_and(|source| Arc::ptr_eq(&source, self))
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .check_admitted_helper(&prepared.admitted)
            .map_err(helper_error)?;
        self.require_helper_selection(prepared.admitted.selection())?;
        let selected = &prepared.admitted.selection().helper;
        if issued_controller_lineage.len() != selected.controller_parent.len() + 1
            || !issued_controller_lineage.starts_with(&selected.controller_parent)
        {
            return Err(PrivateOriginalError::Denied);
        }
        let controller_ceiling = {
            let grants = self.owner.grants();
            let resolved = grants
                .resolve_controller_lineage(
                    &issued_controller_lineage,
                    &selected.logical_executor,
                    selected.represented_subject.as_ref(),
                )
                .map_err(|_| PrivateOriginalError::Denied)?;
            resolved
                .check_current()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            resolved.restrictions().clone()
        };
        let binding = HelperNativeBinding {
            ingress_actor: selected.ingress_actor.clone(),
            logical_executor: selected.logical_executor.clone(),
            represented_subject: selected.represented_subject.clone(),
            target: prepared.target.clone(),
            authority_basis: operation_basis,
            controller_grant_lineage: issued_controller_lineage,
            controller_model: prepared.controller.selection().clone(),
            controller_ceiling,
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
        };
        let source = self
            .owner
            .bind_admitted_helper(self.store.as_ref(), &prepared.admitted, binding)
            .await
            .map_err(helper_error)?;
        let mut held = self
            .helper_sources
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if let Some(prior) = held
            .iter()
            .find(|prior| prior.source.source_record.key == source.source_record.key)
        {
            if !prior.source.same_bound_source(&source).map_err(helper_error)? {
                return Err(PrivateOriginalError::Conflict);
            }
            return Ok(prior.clone());
        }
        let source = Arc::new(HeldSetupHelper {
            source,
            first_input: OnceLock::new(),
            first_custody: Mutex::new(None),
            unresolved: Mutex::new(Vec::new()),
            operation_deadline_ms: OnceLock::new(),
        });
        held.push(source.clone());
        Ok(source)
    }

    /// Export only exact source history for the existing preparation owner.
    /// Publication still requires its acquisition/effect claim and native
    /// operation admission; this is not a new permission-bearing token.
    pub(crate) async fn setup_helper_publication_source(
        self: &Arc<Self>,
        prepared: &PreparedHelperController,
        held: &Arc<HeldSetupHelper>,
        service: &WorkGraphService,
    ) -> Result<meerkat_toolkit::setup::NativeHelperSource, PrivateOriginalError> {
        if !prepared
            .source
            .upgrade()
            .is_some_and(|source| Arc::ptr_eq(&source, self))
            || !self
                .helper_sources
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .iter()
                .any(|entry| Arc::ptr_eq(entry, held))
            || held.source.original.association().candidate().target != prepared.target
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.require_helper_selection(prepared.admitted.selection())?;
        self.owner
            .helper_publication_source(
                self.store.as_ref(),
                service,
                &prepared.admitted,
                &held.source,
            )
            .await
            .map_err(helper_error)
    }

    /// Re-open exactly the contributor rows selected by the native invocation.
    /// The preparation owner uses their versions in its same-store conditional
    /// write. No catalog scan, source copy or new authority is constructed.
    pub(crate) async fn setup_helper_contributor_conditions(
        self: &Arc<Self>,
        admitted: &AdmittedHelperWork,
    ) -> Result<Vec<toolkit_store::RecordCondition>, PrivateOriginalError> {
        self.owner
            .check_admitted_helper(admitted)
            .map_err(helper_error)?;
        self.require_helper_selection(admitted.selection())?;
        let sources = {
            let retained = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            let mut sources = Vec::with_capacity(admitted.contributors().len());
            for original in admitted.contributors() {
                let mut matches = retained
                    .iter()
                    .filter(|held| Arc::ptr_eq(&held.original, original));
                let held = matches.next().ok_or(PrivateOriginalError::Unavailable)?;
                if matches.next().is_some() {
                    return Err(PrivateOriginalError::Conflict);
                }
                sources.push((held.reference.clone(), original.clone()));
            }
            sources
        };
        let mut conditions = Vec::with_capacity(sources.len());
        for (reference, original) in sources {
            let saved = self
                .store
                .get(&self.key(&reference)?)
                .await
                .map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let source = self.decode_record(&saved, &reference)?;
            if source.operation_cancelled {
                return Err(PrivateOriginalError::Denied);
            }
            if source.final_association.as_ref() != Some(original.association()) {
                return Err(PrivateOriginalError::Conflict);
            }
            conditions.push(toolkit_store::RecordCondition {
                key: saved.key,
                revision: saved.revision,
                content_digest: saved.content_digest,
                claim: None,
                dispatch_claim: None,
            });
        }
        self.owner
            .check_admitted_helper(admitted)
            .map_err(helper_error)?;
        Ok(conditions)
    }

    /// Submit the exact source under the same existing identity owner. Real
    /// native acceptance/dedup/uncertainty controls the custody settlement.
    pub(crate) async fn submit_setup_helper(
        self: &Arc<Self>,
        prepared: &PreparedHelperController,
        held: Arc<HeldSetupHelper>,
        snapshot: &meerkat_toolkit::setup::SetupSnapshot,
        content: String,
        source_deadline: tokio::time::Instant,
    ) -> Result<SetupHelperSubmission, PrivateOriginalError> {
        if tokio::time::Instant::now() >= source_deadline {
            return Err(PrivateOriginalError::Denied);
        }
        if !prepared
            .source
            .upgrade()
            .is_some_and(|source| Arc::ptr_eq(&source, self))
            || ContentDigest::of_bytes(content.as_bytes())
                != prepared.admitted.operation().input_digest
            || !held.source.matches_admission(&prepared.admitted)
            || held.source.original.association().candidate().target != prepared.target
            || !self
                .helper_sources
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .iter()
                .any(|entry| Arc::ptr_eq(entry, &held))
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .check_admitted_helper(&prepared.admitted)
            .map_err(helper_error)?;
        self.require_helper_selection(prepared.admitted.selection())?;
        held.bind_setup_deadline(snapshot)?;
        // Capture every actual contributing login before later identity/member
        // awaits. The same handles order final native entry against retirement.
        let entry_fence = entry::for_logins(self.helper_controller_logins(&prepared.admitted)?);
        let runtime = self
            .identity_runtime
            .get()
            .and_then(|runtime| runtime.upgrade())
            .ok_or(PrivateOriginalError::Unavailable)?;
        let alias = &prepared.admitted.selection().member.member.member;
        let identity = runtime
            .owned_identity_for_member_alias(alias)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let binder = Arc::new(SubmitHelperBinder {
            source: self.clone(),
            admitted: prepared.admitted.clone(),
            held,
            session_id: prepared.session_id.clone(),
            controller: prepared.controller.clone(),
            entry_fence,
            source_deadline,
            pending: Mutex::new(Weak::new()),
        });
        // Keep one raw input identity and event content. A definite policy
        // change may rebuild its current binding once, never its source/key.
        let input = Input::Prompt(PromptInput::new(content, None));
        let mut reprepared = false;
        let result = loop {
            // Meta may await the current member/controller before calling the
            // binder. Check here as well as inside that actual binder.
            if tokio::time::Instant::now() >= source_deadline {
                return Err(PrivateOriginalError::Denied);
            }
            let result = runtime
                .dispatch_governed_with_turn_ticket(
                    &identity,
                    Some(alias),
                    input.clone(),
                    binder.clone(),
                )
                .await;
            if result.as_ref().err().is_some_and(|error| {
                helper_admission_reprepare_required(error, &prepared.session_id)
            }) {
                if let Some(pending) = binder.pending.lock().map_err(|_| PrivateOriginalError::Unavailable)?.upgrade() {
                    pending.handoff.record_definite_refusal();
                }
                if reprepared {
                    return Ok(SetupHelperSubmission::ReprepareRequired);
                }
                reprepared = true;
                continue;
            }
            break result;
        };
        Ok(SetupHelperSubmission::Native(match result {
            Ok(outcome) => PrivateOriginalSubmission::Admitted {
                session_id: outcome.admission.session_id.to_string(),
                input_id: outcome.admission.input_id.to_string(),
                deduplicated: outcome.admission.deduplicated,
                turn_ticket: match outcome.ticketed.turn {
                    TurnTracking::Tracked(ticket) => Some(ticket.to_string()),
                    TurnTracking::Unavailable(_) => None,
                },
                source_settlement: if outcome.admission.deduplicated {
                    PrivateSourceSettlement::RetryWithdrawn
                } else if outcome.settlement.is_ok() {
                    PrivateSourceSettlement::FreshAcceptancePromoted
                } else {
                    PrivateSourceSettlement::PostAdmissionPublicationFailed
                },
            },
            Err(IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved {
                ..
            })) => PrivateOriginalSubmission::AdmissionUnresolved,
            Err(error) => {
                Self::native_submission_error_diagnostic("toolkit_helper_submission_error", &error);
                #[cfg(test)]
                aa1_helper_send_failure(&error);
                PrivateOriginalSubmission::NativeOutcomeUnavailable
            }
        }))
    }
}

// The selected Meta governed door propagates this native error only after
// NotAdmitted drops native retry custody; the source owner retains the exact candidate. Uncertain/admitted/missing receipts are
// AdmissionUnresolved instead. A bare runtime readiness error is not sufficient.
fn helper_admission_reprepare_required(
    error: &IdentityRuntimeError,
    expected_session: &SessionId,
) -> bool {
    matches!(error,
        IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(error))
        if matches!(error.as_ref(), meerkat_mob::MobError::MemberInputRefused {
            session_id,
            error: meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                reason: meerkat_runtime::traits::ControllerReadinessFailure::PolicyChanged,
            },
        } if session_id == expected_session))
}

#[cfg(test)]
mod reprepare_tests {
    use super::*;
    use meerkat_runtime::traits::ControllerReadinessFailure;

    #[test]
    fn reprepare_requires_exact_session_policy_changed_nonadmission() {
        let expected = SessionId::new();
        let readiness = |session_id, reason| {
            IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(Box::new(
                meerkat_mob::MobError::MemberInputRefused {
                    session_id,
                    error: meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                        reason,
                    },
                },
            )))
        };
        assert!(helper_admission_reprepare_required(
            &readiness(expected.clone(), ControllerReadinessFailure::PolicyChanged),
            &expected,
        ));
        assert!(!helper_admission_reprepare_required(
            &readiness(SessionId::new(), ControllerReadinessFailure::PolicyChanged),
            &expected,
        ));
        for reason in [
            ControllerReadinessFailure::PolicyUnavailable,
            ControllerReadinessFailure::ExecutorUnavailable,
            ControllerReadinessFailure::FactsUnavailable,
            ControllerReadinessFailure::Busy,
            ControllerReadinessFailure::AuthorityUnavailable,
            ControllerReadinessFailure::AuthorityChanged,
            ControllerReadinessFailure::UnsupportedScope,
            ControllerReadinessFailure::CredentialUnusable {
                disposition: meerkat_core::handles::CredentialUseDisposition::LeaseAbsent,
            },
            ControllerReadinessFailure::CredentialPreparationFailed {
                kind: meerkat_core::auth::AuthErrorKind::LeaseAbsent,
            },
            ControllerReadinessFailure::ReplacementNotEmpty,
        ] {
            assert!(!helper_admission_reprepare_required(
                &readiness(expected.clone(), reason),
                &expected,
            ));
        }
        for error in [
            GovernedInputError::AdmissionUnresolved {
                detail: "acceptance answer unavailable".into(),
            },
            GovernedInputError::Bind("binding unavailable".into()),
            GovernedInputError::Unsupported("no governed controller".into()),
            GovernedInputError::BindingChanged,
            GovernedInputError::Unbound,
        ] {
            assert!(!helper_admission_reprepare_required(
                &IdentityRuntimeError::GovernedInput(error),
                &expected,
            ));
        }
    }
}

struct PrepareHelperBinder {
    source: Weak<PrivateOriginals>,
    admitted: Arc<AdmittedHelperWork>,
    captured: Mutex<Option<PreparedHelperController>>,
}

#[async_trait]
impl GovernedInputBinder for PrepareHelperBinder {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        _input: Input,
    ) -> Result<BoundGovernedInput, String> {
        let source = self
            .source
            .upgrade()
            .ok_or("Toolkit helper source is unavailable")?;
        source
            .owner
            .check_admitted_helper(&self.admitted)
            .map_err(|_| "Toolkit helper source is unavailable")?;
        source
            .require_helper_selection(self.admitted.selection())
            .map_err(|_| "Toolkit helper selection is unavailable")?;
        let machine = source
            .machine()
            .map_err(|_| "Toolkit helper runtime is unavailable")?;
        let registration = machine
            .current_session_registration_witness(&context.session_id)
            .await
            .ok_or("Toolkit helper registration is unavailable")?;
        let member = &self.admitted.selection().member;
        let target = NativeWorkTarget {
            logical_owner: member.logical_owner.clone(),
            logical_runtime: EvidenceId::new(
                meerkat_runtime::LogicalRuntimeId::for_session(registration.session_id())
                    .to_string(),
            )
            .map_err(|_| "Toolkit helper runtime identity is unavailable")?,
            context: EvidenceId::new(registration.session_id().to_string())
                .map_err(|_| "Toolkit helper session is unavailable")?,
            context_generation: member.context_generation,
            audience: member.audience.clone(),
        };
        let mut captured = self
            .captured
            .lock()
            .map_err(|_| "Toolkit helper preparation is unavailable")?;
        if captured.is_some() {
            return Err("Toolkit helper preparation was already captured".into());
        }
        *captured = Some(PreparedHelperController {
            source: self.source.clone(),
            admitted: self.admitted.clone(),
            session_id: context.session_id.clone(),
            target,
            controller: context.controller.clone(),
        });
        Err("Toolkit helper controller preparation precedes input submission".into())
    }

    fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {
        // bind never returns input or settlement, so there is no custody here.
    }
}

struct SubmitHelperBinder {
    source: Arc<PrivateOriginals>,
    admitted: Arc<AdmittedHelperWork>,
    held: Arc<HeldSetupHelper>,
    session_id: SessionId,
    controller: ControllerModelClient,
    entry_fence: Option<Arc<dyn meerkat_mobkit::identity_first::GovernedEntryFence>>,
    source_deadline: tokio::time::Instant,
    // One invocation per binder. Only the actual Box keeps it alive; the
    // bounded definite-refusal retry can bind a new invocation afterwards.
    // Shared source-owner locks separately exclude competing Candidates.
    pending: Mutex<Weak<PendingHelperAdmission>>,
}

impl Drop for SubmitHelperBinder {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).upgrade() {
            pending.handoff.foreground_finished();
        }
    }
}

#[async_trait]
impl GovernedInputBinder for SubmitHelperBinder {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        if tokio::time::Instant::now() >= self.source_deadline {
            return Err("Toolkit helper submission deadline elapsed".into());
        }
        if context.session_id != self.session_id
            || context.controller.selection() != self.controller.selection()
        {
            #[cfg(test)]
            crate::aa1_failure("helper.bind.placement", "PlacementChanged");
            return Err("Toolkit helper native placement changed".into());
        }
        self.source
            .owner
            .check_admitted_helper(&self.admitted)
            .map_err(|_| {
                #[cfg(test)]
                crate::aa1_failure("helper.bind.source", "Unavailable");
                "Toolkit helper source is unavailable"
            })?;
        self.source
            .require_helper_selection(self.admitted.selection())
            .map_err(|_| {
                #[cfg(test)]
                crate::aa1_failure("helper.bind.selection", "Unavailable");
                "Toolkit helper selection is unavailable"
            })?;
        self.source.restore_promoted_custody(
            &self.held.source.source_record.key, &self.held.source.original,
        ).await.map_err(|_| "Toolkit historical helper custody is unavailable")?;
        let mut prepared = self
            .source
            .owner
            .prepare_authenticated_input(
                &self.held.source.original,
                &context.session_id,
                input,
                self.held.source.event_key.clone(),
                self.held.source.authentication.clone(),
                context.controller.clone(),
            )
            .map_err(|_error| {
                #[cfg(test)]
                crate::aa1_failure("helper.bind.input", aa1_helper_runtime_variant(&_error));
                "Toolkit helper input binding is unavailable"
            })?;
        let admitted = prepared.admitted().map_err(|_| "Toolkit admitted helper is unavailable")?;
        if admitted.is_none() {
            if let Some(exact) = cold_service_admission::RetainedBoundInput::retain_for_attempt(
                &self.held.first_input, prepared.input(),
            ).map_err(|_| "Toolkit first helper input changed")? {
                prepared = self.source.owner.prepare_authenticated_input(
                    &self.held.source.original, &context.session_id, exact,
                    self.held.source.event_key.clone(), self.held.source.authentication.clone(),
                    context.controller.clone(),
                ).map_err(|_| "Toolkit could not rebind the exact first helper input")?;
            }
            self.source.owner.persist_helper_custody_attempt(
                self.source.store.as_ref(), &self.admitted, &self.held.source,
                &context.session_id, prepared.input(),
            ).await.map_err(|_| "Toolkit could not persist the first helper attempt")?;
        }
        if tokio::time::Instant::now() >= self.source_deadline {
            return Err("Toolkit helper submission deadline elapsed".into());
        }
        self.source.owner.check_admitted_helper(&self.admitted)
            .map_err(|_| "Toolkit helper source is unavailable")?;
        self.source.require_helper_selection(self.admitted.selection())
            .map_err(|_| "Toolkit helper selection is unavailable")?;
        let (input, pending) = self.source.custody_recovery.enroll(|selected| {
            // Refuse overlap before staging: dropping a newly staged guard
            // must never withdraw the earlier invocation's Candidate.
            let mut invocation = self.pending.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
            let mut retained = self.held.first_custody.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
            let prior = retained.as_ref().filter(|_| selected);
            if let Some(invoked) = invocation.upgrade() {
                if !prior.is_some_and(|prior| Arc::ptr_eq(prior, &invoked)) {
                    return Err(PrivateOriginalError::Unavailable);
                }
            }
            let (input, pending, handoff) = match admitted {
                Some((input, pending)) => {
                    if invocation.upgrade().is_some() { return Err(PrivateOriginalError::Unavailable); }
                    (input, pending, custody_recovery::CustodyHandoff::new(false, &self.source))
                }
                None => match prior {
                    Some(prior) => {
                        if prior.session != context.session_id { return Err(PrivateOriginalError::Conflict); }
                        let ((input, pending), handoff) = prior.handoff.rebind(|| {
                            prior.publication.lock().map_err(|_| PrivateOriginalError::Unavailable)?
                                .source.rebind_candidate(prepared, &prior.input)
                        })?;
                        (input, pending, handoff)
                    }
                    None => {
                        let (input, pending) = prepared.stage().map_err(|_| PrivateOriginalError::Unavailable)?;
                        let handoff = custody_recovery::CustodyHandoff::new(selected && pending.is_first_candidate(), &self.source);
                        (input, pending, handoff)
                    }
                },
            };
            let enrolled = handoff.enrolled();
        pending.enroll_settlement(&handoff).map_err(|_| PrivateOriginalError::Unavailable)?;
            let pending = Arc::new(PendingHelperAdmission {
                handoff,
                held: Arc::downgrade(&self.held), admitted: self.admitted.clone(),
                session: context.session_id.clone(), input: input.clone(),
                first_candidate: pending.is_first_candidate(),
                publication: Mutex::new(HelperPublication {
                    native_admission: None,
                    source: governed_submit::SourcePublication::Pending(pending),
                }),
            });
            *invocation = Arc::downgrade(&pending);
            if enrolled { *retained = Some(pending.clone()); }
            Ok((input, pending))
        }).map_err(|_| "Toolkit helper recovery enrollment is unavailable")?;
        let settlement = HelperSettlement {
            pending, held: Arc::downgrade(&self.held),
        };
        Ok(BoundGovernedInput {
            input,
            settlement: Box::new(settlement),
            entry_fence: self.entry_fence.clone(),
        })
    }

    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, _detail: &str) {
        // Meta transfers this one Box only after its native operation has
        // stopped owning the decision. A reader/timer never manufactures it.
        let pending = self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).upgrade();
        if let Some(pending) = &pending { pending.handoff.release(); }
        self.held.unresolved.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(UnresolvedHelperAdmission { pending, settlement });
    }
}

/// Exact per-invocation input, kept with the actual guard. The helper's first
/// serialized Input cannot stand in for later admitted-only invocations.
pub(super) struct PendingHelperAdmission {
    pub(super) handoff: Arc<custody_recovery::CustodyHandoff>,
    pub(super) held: Weak<HeldSetupHelper>,
    pub(super) admitted: Arc<AdmittedHelperWork>,
    pub(super) session: SessionId,
    pub(super) input: Input,
    first_candidate: bool,
    pub(super) publication: Mutex<HelperPublication>,
}

pub(super) struct HelperPublication {
    // Only the actual matching fresh callback records this native receipt.
    pub(super) native_admission: Option<GovernedAdmission>,
    pub(super) source: governed_submit::SourcePublication,
}

/// Actual handoff custody. Even a missing typed pointer retains the native Box
/// as unavailable evidence; it cannot be dropped as proof of nonadmission.
struct UnresolvedHelperAdmission {
    pending: Option<Arc<PendingHelperAdmission>>,
    settlement: Box<dyn GovernedSettlement>,
}

struct HelperSettlement {
    pending: Arc<PendingHelperAdmission>,
    held: Weak<HeldSetupHelper>,
}

impl Drop for HelperSettlement {
    fn drop(&mut self) { self.pending.handoff.native_box_finished(); }
}
impl GovernedSettlement for HelperSettlement {
    fn accepted(self: Box<Self>, admission: &GovernedAdmission) -> Result<(), String> {
        if admission.deduplicated
            || admission.session_id != self.pending.session
            || admission.input_id != *self.pending.input.id()
        {
            self.pending.handoff.release();
            if let Some(held) = self.held.upgrade() {
                let pending = Some(self.pending.clone());
                held.unresolved.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(UnresolvedHelperAdmission { pending, settlement: self });
            }
            return Err("Toolkit helper source acknowledgement did not match".into());
        }
        use governed_submit::SourcePublication;
        let unavailable = || "Native helper was admitted but source publication failed".to_owned();
        self.pending.publication.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .native_admission = Some(admission.clone());
        let result = self.pending.handoff.accepted(|| {
            let mut publication = self.pending.publication.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let prior = std::mem::replace(&mut publication.source, SourcePublication::Settled {
                source_acknowledged: false, result: Err(unavailable()),
            });
            let result = match prior {
                SourcePublication::Pending(pending) => {
                    let result = pending.admit().map(|_| ()).map_err(|_| unavailable());
                    publication.source = SourcePublication::Settled {
                        source_acknowledged: result.is_ok(), result: result.clone(),
                    };
                    result
                }
                settled @ SourcePublication::Settled { .. } => {
                    publication.source = settled;
                    Err("Toolkit helper source settlement was already consumed".into())
                }
            };
            result
        });
        if result.is_err() {
            // Native acceptance is final even when this later source step
            // fails. Keep the same consumed Box and exact Input with its
            // existing owner; never call accepted again or dispatch a retry.
            self.pending.handoff.release();
            if let Some(held) = self.held.upgrade() {
                let pending = Some(self.pending.clone());
                held.unresolved.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(UnresolvedHelperAdmission { pending, settlement: self });
            }
        }
        result
    }
}

// Failure-only labels for the existing AA1 executor. Payloads and native
// error text stay private; the public result and settlement mapping are unchanged.
#[cfg(test)]
fn aa1_helper_send_failure(error: &IdentityRuntimeError) {
    let variant = match error {
        IdentityRuntimeError::GovernedInput(error) => match error {
            GovernedInputError::Unsupported(_) => "GovernedInputError::Unsupported",
            GovernedInputError::BindingChanged => "GovernedInputError::BindingChanged",
            GovernedInputError::Bind(_) => "GovernedInputError::Bind",
            GovernedInputError::Unbound => "GovernedInputError::Unbound",
            GovernedInputError::EntryRefused(_) => "GovernedInputError::EntryRefused",
            GovernedInputError::AdmissionUnresolved { .. } => "GovernedInputError::AdmissionUnresolved",
            GovernedInputError::Mob(error) => match error.as_ref() {
                meerkat_mob::MobError::MemberInputRefused { error, .. } => {
                    crate::aa1_failure("helper.send.member_input", aa1_helper_runtime_variant(error));
                    "MobError::MemberInputRefused"
                }
                meerkat_mob::MobError::GovernedInputUnbound { .. } => "MobError::GovernedInputUnbound",
                meerkat_mob::MobError::MemberSessionNotLive { .. } => "MobError::MemberSessionNotLive",
                meerkat_mob::MobError::MemberNotFound(_) => "MobError::MemberNotFound",
                _ => "MobError::Other",
            },
        },
        IdentityRuntimeError::UnknownIdentity(_) => "IdentityRuntimeError::UnknownIdentity",
        IdentityRuntimeError::NotAddressable(_) => "IdentityRuntimeError::NotAddressable",
        IdentityRuntimeError::CompletionUnavailable { .. } => "IdentityRuntimeError::CompletionUnavailable",
        IdentityRuntimeError::AdmissionFailed { .. } => "IdentityRuntimeError::AdmissionFailed",
        IdentityRuntimeError::ActorAdmissionTimeout { .. } => "IdentityRuntimeError::ActorAdmissionTimeout",
        IdentityRuntimeError::ActorLoopStalled { .. } => "IdentityRuntimeError::ActorLoopStalled",
        IdentityRuntimeError::ActorTerminated { .. } => "IdentityRuntimeError::ActorTerminated",
        IdentityRuntimeError::InvalidState { .. } => "IdentityRuntimeError::InvalidState",
        IdentityRuntimeError::StaleRuntimeAlias { .. } => "IdentityRuntimeError::StaleRuntimeAlias",
        _ => "IdentityRuntimeError::Other",
    };
    crate::aa1_failure("helper.send", variant);
}

#[cfg(test)]
fn aa1_helper_runtime_variant(error: &meerkat_runtime::RuntimeDriverError) -> &'static str {
    use meerkat_runtime::RuntimeDriverError;
    match error {
        RuntimeDriverError::InputRefused { refusal } => match refusal.kind() {
            meerkat_core::OperationRefusalKind::Denied => "RuntimeDriverError::InputRefused::Denied",
            meerkat_core::OperationRefusalKind::ReprepareRequired => "RuntimeDriverError::InputRefused::ReprepareRequired",
            meerkat_core::OperationRefusalKind::MalformedFacts => "RuntimeDriverError::InputRefused::MalformedFacts",
        },
        RuntimeDriverError::ControllerReadinessUnavailable { .. } => "RuntimeDriverError::ControllerReadinessUnavailable",
        RuntimeDriverError::ValidationFailed { .. } => "RuntimeDriverError::ValidationFailed",
        RuntimeDriverError::InputIdempotencyConflict { .. } => "RuntimeDriverError::InputIdempotencyConflict",
        RuntimeDriverError::NotReady { .. } => "RuntimeDriverError::NotReady",
        RuntimeDriverError::NotFound { .. } => "RuntimeDriverError::NotFound",
        RuntimeDriverError::Destroyed => "RuntimeDriverError::Destroyed",
        _ => "RuntimeDriverError::Other",
    }
}

fn helper_error(error: OriginalWriterError) -> PrivateOriginalError {
    match error {
        OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
        OriginalWriterError::OperationDenied
        | OriginalWriterError::OperationCancelled
        | OriginalWriterError::WrongOwner => {
            PrivateOriginalError::Denied
        }
        _ => PrivateOriginalError::Unavailable,
    }
}


#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn helper_review_lifetime_deadline_is_bound_once_without_replay_renewal() {
        let held = OnceLock::new();
        assert!(held.get().is_none());
        retain_exact_setup_deadline(&held, 180_000).unwrap();
        retain_exact_setup_deadline(&held, 180_000).unwrap();
        for changed in [179_999, 180_001, u64::MAX] {
            assert!(matches!(retain_exact_setup_deadline(&held, changed), Err(PrivateOriginalError::Conflict)));
            assert_eq!(held.get(), Some(&180_000));
        }
    }
}
