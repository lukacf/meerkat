//! Current generated grants composed with the actual native and resource owners.
//!
//! This module keeps no admission, identity, resource or grant registry. The
//! trusted owner interfaces below must read their existing canonical facts. A
//! decoded association, selected credential or successful constructor is never
//! accepted work or permission. All owner mutations participate in the grant
//! authority's publication; evaluation is local and performs no I/O.

use std::sync::Arc;

use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, LifetimeBound};
use meerkat_authorization_contracts::work_association::{
    GrantTarget, InputAuthorityAssociation, OriginalWorkRef, QualifiedIngressNamespace,
    WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    ToolAuthorizationTarget, ToolDelegateRole, WorkAuthorizationContext,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::tool_execution::{ResolvedToolExecutionPlan, ToolOperationTarget};

use crate::grants::{GrantRefusal, LineageResolutionRefusal, LocalGrantAuthority, ResolvedGrant};
use crate::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose, LocalWorkPolicy,
};
use crate::publication::{LocalPublicationGuard, PublicationError};

/// The one owner-produced operation target of a dispatched tool plan, if any
/// ([`ResolvedToolExecutionPlan::single_operation_target`]).
///
/// # Errors
/// A plan naming more than one owner target is ambiguous and unavailable.
pub fn single_tool_operation_target(
    plan: &ResolvedToolExecutionPlan,
) -> Result<Option<&ToolOperationTarget>, meerkat_core::OperationAuthorizationError> {
    plan.single_operation_target()
}

/// The typed target an operation's grant lineage is selected by: the single
/// MCP server a dispatched tool operates on, the exact checked route to that
/// server it was reached through, the exact registered tool bundle operation
/// it runs, the one peer a comms delivery reaches, the session's own peer
/// directory, the exact memory scope a memory operation reaches, the builtin
/// operation it is, the hosted-search helper route and capability of a
/// builtin web_search call's delegate, or `None` for an operation with no
/// typed target or a tool catalog control, which uses the basis's base
/// lineage. Raw operation meaning is untouched; this only says which
/// selected lineage applies.
///
/// It reads the whole binding, not only its facts: a delegate's model
/// operation selects the lineage of its own route and capability, while an
/// ordinary inference with equal facts keeps the base. The same function
/// selects the lineage of the work and of every original, so the decision
/// and every contributor's bound agree on the target.
///
/// # Errors
/// An ambiguous tool plan target, or a delegate whose own route has no key,
/// is unavailable; a delegate binding without its role's facts is
/// malformed.
pub fn operation_grant_target(
    binding: &PreparedAuthorizationBinding,
) -> Result<Option<GrantTarget>, meerkat_core::OperationAuthorizationError> {
    let facts = binding.facts();
    if let Some(delegate) = binding.delegate_attribution() {
        return delegated_grant_target(delegate.role(), &facts.operation).map(Some);
    }
    let AuthorizationOperation::Tool(tool) = &facts.operation else {
        return Ok(None);
    };
    let ToolAuthorizationTarget::Dispatcher(plan) = &tool.target else {
        return Ok(None);
    };
    match single_tool_operation_target(plan)? {
        None => Ok(None),
        Some(ToolOperationTarget::McpServerOperation { server, .. }) => {
            Ok(Some(GrantTarget::McpServer {
                server: server.clone(),
            }))
        }
        // The session's own tool catalog has no grant target of its own: it
        // is base work, so it uses the basis's base lineage (and is denied
        // when the basis has none).
        Some(ToolOperationTarget::CatalogControl { .. }) => Ok(None),
        // A registered tool bundle's operation is its own target: it uses
        // only the lineage selected for exactly that bundle and operation,
        // never the base (a catalog grant does not cover it) and never a
        // sibling operation's.
        Some(ToolOperationTarget::RegisteredToolBundleOperation {
            bundle, operation, ..
        }) => Ok(Some(GrantTarget::RegisteredToolBundleOperation {
            bundle: bundle.clone(),
            operation: operation.clone(),
        })),
        // An MCP operation reached through a host's checked route uses only
        // the lineage selected for exactly that route to that server: never
        // the server's ordinary lineage, which keeps refusing the identical
        // raw operation, and never the base.
        Some(ToolOperationTarget::CheckedMcpServerOperation { route, server, .. }) => {
            Ok(Some(GrantTarget::CheckedMcpRoute {
                route: route.clone(),
                server: server.clone(),
            }))
        }
        // A comms delivery uses only the lineage selected for exactly the
        // peer it reaches, whatever kind of delivery it is: never another
        // peer's, the peer directory's or the base.
        Some(ToolOperationTarget::PeerSend { peer, .. }) => {
            Ok(Some(GrantTarget::Peer { peer: *peer }))
        }
        // A read of the session's own peer directory uses only its own
        // lineage, which covers no delivery.
        Some(ToolOperationTarget::PeerDirectoryRead) => Ok(Some(GrantTarget::PeerDirectory)),
        // A memory operation uses only the lineage selected for exactly the
        // scope it reaches: never another scope's or the base.
        Some(ToolOperationTarget::MemoryOperation { scope, .. }) => Ok(Some(GrantTarget::Memory {
            scope: scope.clone(),
        })),
        // A builtin operation (the builtin web_search call) is its own
        // target: it uses only the lineage selected for exactly that
        // operation, never the base, the catalog or the controller lineage.
        Some(ToolOperationTarget::BuiltinOperation { operation }) => {
            Ok(Some(GrantTarget::BuiltinToolOperation {
                operation: *operation,
            }))
        }
        // Any other target kind selects nothing: unavailable.
        Some(_) => Err(meerkat_core::OperationAuthorizationError::Unavailable),
    }
}

/// The target of a builtin tool's delegated operation, from the delegate's
/// own facts: for the web_search helper, its own lowered route's key and
/// the one hosted search capability its request enables. Never the
/// controller's lineage, whatever route the helper runs on.
fn delegated_grant_target(
    role: ToolDelegateRole,
    operation: &AuthorizationOperation,
) -> Result<GrantTarget, meerkat_core::OperationAuthorizationError> {
    match role {
        ToolDelegateRole::HostedSearchInference => {
            let AuthorizationOperation::Model(model) = operation else {
                return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into());
            };
            let [
                capability @ (meerkat_core::ServerToolKind::WebSearch
                | meerkat_core::ServerToolKind::GoogleSearch),
            ] = model.hosted_capabilities.as_ref()
            else {
                return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into());
            };
            let route = model
                .controller_route_key()
                .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
            Ok(GrantTarget::DelegatedHostedSearch {
                route: route.as_str().to_owned(),
                capability: capability.clone(),
            })
        }
    }
}

/// The grant lineages native admission and retained resume resolve before any
/// operation exists. A single `GrantLineage` is the work's common authority and
/// is resolved as before. A targeted selection has no common grant lineage:
/// each target's lineage, and the optional base, are resolved only at use on
/// their own operations, so a revoked or expired one refuses only those
/// operations and never admission, resume or another target. Ingress
/// authentication and controller authority are checked by their own owners.
pub fn admission_grant_lineages(
    basis: &WorkAuthorityBasis,
) -> impl Iterator<Item = &[meerkat_authorization_contracts::work_association::GrantLineageRef]> {
    match basis {
        WorkAuthorityBasis::GrantLineage { lineage } => Some(lineage.as_slice()),
        WorkAuthorityBasis::TargetedGrantLineages { .. }
        | WorkAuthorityBasis::HostPolicy { .. }
        | WorkAuthorityBasis::ServiceMandate { .. } => None,
    }
    .into_iter()
}

/// Current owner bounds, not a capability, cached acceptance or wire DTO.
///
/// Only an installed trusted owner implementation supplies this value. Its
/// expiry covers the current caller/mandate/policy decision for the requested
/// purpose. The generated grant's independent expiry is conjoined afterwards.
pub struct WorkOwnerAllowance {
    pub restrictions: ExecutionRestrictions,
    pub expires_at_ms: u64,
}

impl std::fmt::Debug for WorkOwnerAllowance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkOwnerAllowance").finish_non_exhaustive()
    }
}

/// Existing native input/mandate owner, installed by the trusted embedding.
///
/// Resolve the complete association against the actual accepted input and exact
/// binding execution scope, run and context. Check the actual requester may
/// currently invoke this delegation for its executor and represented subject;
/// an agent holding the grant is insufficient. Do not infer the requester from
/// an agent owner, credential/account, InputOrigin or the association itself.
///
/// For Operation, HostPolicy and ServiceMandate must resolve the exact current
/// owner decision/occurrence here. Their presence in a claim is not a fallback
/// permission. GrantLineage is subsequently resolved by the generated owner.
/// For Controller, independently resolve the admitted controller mandate and
/// route; ordinary operation authority is not a prerequisite or substitute.
/// Missing ownership, unsupported scope and any failed conjunct must refuse.
///
/// Every relevant accepted mutation must use the same publication as the grant
/// authority. This synchronous method cannot perform I/O or hold a native lock
/// that is already held by its caller. Real runtime/schedule adapters must bind
/// this contract to their own admitted records; this trait creates none.
///
/// An `Unavailable` answer is final where it is asked: Core neither rebuilds
/// nor retries it. At a controller's model request it ends that run with no
/// retry, and nothing is sent. Core waits only for contention it can name: an
/// owner change in progress on the work's own publication, or a hold of that
/// publication's writer (`WorkAuthorization::owner_change_in_progress`). An
/// owner's contention on its own locks names nothing, so an owner must not
/// answer `Unavailable` for its own transient lock contention: it waits
/// briefly for its lock (never one its caller holds) or reads an immutable
/// snapshot that its writers replace. `Unavailable` is for an owner that
/// genuinely cannot answer now.
pub trait AdmittedWorkPolicyOwner: Send + Sync {
    /// Admit this exact authenticated UI submission for its current native
    /// member. Resolve the actual ingress actor and mandate independently of
    /// the original tool result or any earlier run. Unsupported owners refuse.
    fn authorize_tool_application(
        &self,
        _control: &meerkat_core::ToolApplicationControlRequest,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    /// Resolve the actual authenticated control/original or service mandate.
    /// The process receipt is not a claim-derived grant. Validate its exact
    /// requester, executor, source, destination and immutable request against
    /// the real owner. All relevant changes share this grant publication.
    fn authorize_context_control(
        &self,
        _control: &meerkat_core::service::SystemContextControlRequest,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    /// Resolve the actual authenticated MCP attachment control: its exact
    /// requester, actor, realm, destination and server descriptor against the
    /// real owner, for the one effect `binding` names. Unsupported owners
    /// fail closed.
    fn authorize_mcp_attachment(
        &self,
        _control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError>;

    /// The actual current mandate behind this admitted work, as bounded
    /// passive text for review and consent presentation. Read from the same
    /// owner records and publication as `authorize_admitted_work`; never an
    /// explanation reconstructed from identifiers or history. `Ok(None)`
    /// means explicitly not applicable. Fail closed: an owner that does not
    /// implement this, or requires a mandate it cannot read, is
    /// `Unavailable`.
    fn review_mandate(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    /// The record of the service original `original` that the composition's
    /// original source holds enrolled under `namespace`: the exact
    /// association it enrolled for a governed service call (work no runtime
    /// input carries, such as a scheduled host runnable's provider calls).
    /// This only fetches the record for the native host, which compares it
    /// with the claims a call presents; it decides nothing. Every operation
    /// is still decided by this owner's `authorize_admitted_work`, the grant
    /// owner and the operation owner.
    ///
    /// `Ok(None)` is no current enrollment (never enrolled, or revoked). A
    /// cancelled original is refused `Denied`; an owner that cannot say now
    /// is `Unavailable`. Synchronous and without I/O, under the same
    /// publication as `authorize_admitted_work`. The default is
    /// `Unavailable`: an owner that cannot read enrolled service originals
    /// serves no service call.
    fn enrolled_service_original(
        &self,
        _namespace: &QualifiedIngressNamespace,
        _original: &OriginalWorkRef,
    ) -> Result<Option<InputAuthorityAssociation>, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}

/// Existing resource, peer, account and sink policy composition.
///
/// Resolve the actual bound operation against its canonical target owners and
/// require every relevant owner to allow. Preserve correlated rule semantics
/// when mapping actual action/resource/processor/audience tuples. In particular,
/// endpoint, wire model, hosted capabilities and account access are independent
/// of controller selection and credential possession. A resource's metadata or
/// historical source observation cannot replace its access decision.
///
/// For Controller return only the actual model-route tuples. For Operation on
/// ControllerInference return every actual hosted-capability tuple; do not add
/// the separately authorized controller tuple or omit an enabled capability.
/// For ordinary operations return all their actual tuples.
///
/// Classify tools by their owner-produced target, never by name: the builtin
/// web_search call carries `ToolOperationTarget::BuiltinOperation`. Its
/// helper's model request is an Operation whose binding carries a
/// `ToolDelegateAttribution` naming that call; map it from the helper's own
/// route facts (identity, credential, backend, endpoint, wire model) and
/// return the inference on that route and one tuple per enabled hosted
/// capability, never the controller tuple. The attribution grants nothing;
/// an inference without it is an ordinary alternate-model operation.
///
/// The returned bounds and deadline are current owner facts, read under the
/// enclosing publication observation. This is a trusted extension seam for
/// application ABAC, not a generic policy language or an allow-by-default hook.
///
/// As for [`AdmittedWorkPolicyOwner`], an `Unavailable` answer is final where
/// it is asked: Core neither rebuilds nor retries it, and at a controller's
/// model request, where this owner is asked too, it ends that run with no
/// retry, and nothing is sent. An owner's contention on its own locks names
/// no owner change in progress, so an owner must not answer `Unavailable` for
/// it: it waits briefly for its lock (never one its caller holds) or reads an
/// immutable snapshot that its writers replace. `Unavailable` is for an owner
/// that genuinely cannot answer now.
pub trait OperationPolicyOwner: Send + Sync {
    /// Authorize the actual canonical tool/arguments or qualified resource
    /// bound for a fresh UI submission. Resolve source registration and target
    /// policy from their actual owners; the original invocation grants nothing.
    fn authorize_tool_application(
        &self,
        _control: &meerkat_core::ToolApplicationControlRequest,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    /// Independently resolve the actual source or destination bound here for
    /// the exact authenticated control. Called for Source(Hydrate) and for
    /// Publication, in one coherent native publication observation. Neither
    /// invocation permission nor content/source annotations replace these ACLs.
    fn authorize_context_control(
        &self,
        _control: &meerkat_core::service::SystemContextControlRequest,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    /// Resolve the exact server, destination or configuration entry of one
    /// MCP attachment effect for the authenticated control. Called for every
    /// operation of the effect (server hydrate and destination publication
    /// for a live attachment; configuration retain for a persisted one), in
    /// one coherent native publication observation.
    fn authorize_mcp_attachment(
        &self,
        _control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    /// Authorize this exact plain controller before native admission. The native
    /// ingress owner separately checks the current authenticated caller's right
    /// to invoke the mandate. No operation, run or transcript exists yet.
    ///
    /// Resolve current correlated account/endpoint/wire-model rules. This
    /// allowance must last for admitted work: explicit unrestricted lifetime,
    /// with removal protected by complete native controller custody and this
    /// grant authority's publication. A finite operation-cache deadline cannot
    /// stand in for that contract. Do not perform I/O or infer account authority
    /// from possession of credentials. Unsupported owners fail closed.
    fn authorize_controller_admission(
        &self,
        _association: &InputAuthorityAssociation,
        _facts: &meerkat_core::ControllerModelFacts,
        _now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError>;

    /// The action account this owner actually selected for the bound
    /// operation's sink, as bounded passive text for review and consent
    /// presentation. A credential or auth-binding reference is not the
    /// selected account. `Ok(None)` means explicitly not applicable. Fail
    /// closed: an owner that does not implement this, or requires an account
    /// it cannot read, is `Unavailable`.
    fn review_action_account(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}

/// Current correlated controller rules lasting for admitted work.
/// This is trusted owner data, not a serialized permission or entry permit.
pub struct ControllerAdmissionAllowance {
    pub operation_values: Vec<LocalOperationValues>,
    pub restrictions: ExecutionRestrictions,
}

impl std::fmt::Debug for ControllerAdmissionAllowance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControllerAdmissionAllowance")
            .finish_non_exhaustive()
    }
}

/// The exact grant publication's reserved writer, not policy authority.
///
/// The host must first hold complete native controller custody, then reserve
/// this writer, then lock its actual policy state. Validate the proposal under
/// those locks. On success replace state and publish before unlocking policy;
/// on refusal change nothing. Do not hold this guard across I/O or await.
/// A panic poisons publication; an unchanged rejected proposal does not.
pub struct ControllerPolicyChange<'a> {
    publication: LocalPublicationGuard<'a>,
}

impl ControllerPolicyChange<'_> {
    /// Publish only after successful replacement, while the policy mutex still
    /// hides the changed facts. Retain native custody until this guard drops.
    ///
    /// After a [poison](crate::publication::LocalAuthorizationPublication::poison)
    /// of the grant publication this publishes nothing: the replaced policy
    /// stays retained but unusable until its owners are reconstructed.
    pub fn publish(&mut self) {
        // Poisoned, nothing is published and every check stays unavailable.
        let _ = self.publication.publish();
    }
}

/// Conjunction using one explicitly selected actual generated grant authority.
/// References naming another authority refuse; claims never select a new owner.
pub struct GrantBackedWorkPolicy {
    grants: Arc<LocalGrantAuthority>,
    work_owner: Arc<dyn AdmittedWorkPolicyOwner>,
    operation_owner: Arc<dyn OperationPolicyOwner>,
}

impl std::fmt::Debug for GrantBackedWorkPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrantBackedWorkPolicy")
            .finish_non_exhaustive()
    }
}

impl GrantBackedWorkPolicy {
    /// One fresh host UI request, with no synthetic input or borrowed run.
    pub fn tool_application_authorization(
        self: &Arc<Self>,
        control: Arc<meerkat_core::ToolApplicationControlRequest>,
    ) -> WorkAuthorizationContext {
        WorkAuthorizationContext::new(
            Arc::new(ToolApplicationAuthorization {
                policy: Arc::clone(self),
                control,
            }),
            OperationExecutionScope::Domain,
        )
    }

    /// Non-input control composition over these same actual installed owners.
    /// No accepted input, run, grant or original requester is manufactured.
    pub fn context_control_authorization(
        self: &Arc<Self>,
        control: Arc<meerkat_core::service::SystemContextControlRequest>,
    ) -> WorkAuthorizationContext {
        WorkAuthorizationContext::new(
            Arc::new(ContextControlAuthorization {
                policy: Arc::clone(self),
                control,
            }),
            OperationExecutionScope::Domain,
        )
    }

    /// Non-input MCP attachment control over these same installed owners.
    /// Each prepared effect conjoins the work-owner mandate with the
    /// operation owner's allowance for every operation of that effect.
    pub fn mcp_attachment_authorization(
        self: &Arc<Self>,
        control: Arc<meerkat_core::mcp_attachment_control::McpAttachmentControlRequest>,
    ) -> WorkAuthorizationContext {
        WorkAuthorizationContext::new(
            Arc::new(McpAttachmentAuthorization {
                policy: Arc::clone(self),
                control,
            }),
            OperationExecutionScope::Domain,
        )
    }

    /// Native production factory. The actual admitted row supplies this sink;
    /// a logging-only exporter is not a substitute. Entry staging must succeed
    /// before a body runs, and observations join that row's next existing commit.
    pub fn audited_work_context(
        self: &Arc<Self>,
        associations: Arc<[InputAuthorityAssociation]>,
        execution_scope: OperationExecutionScope,
        audit_sink: Arc<dyn meerkat_authorization_contracts::audit::AuthorizationAuditSink>,
    ) -> Result<WorkAuthorizationContext, OperationRefused> {
        let policy: Arc<dyn LocalWorkPolicy> = self.clone();
        self.grants
            .audited_work_context(associations, execution_scope, policy, audit_sink)
    }

    /// Install trusted existing owner adapters, never adapters selected by a
    /// wire claim. Construction does not accept work or resolve any grant.
    pub fn new(
        grants: Arc<LocalGrantAuthority>,
        work_owner: Arc<dyn AdmittedWorkPolicyOwner>,
        operation_owner: Arc<dyn OperationPolicyOwner>,
    ) -> Self {
        Self {
            grants,
            work_owner,
            operation_owner,
        }
    }

    /// Rebuild a context from the native owner's complete retained contributors.
    /// The compiler uses this grant authority's own publication and clock;
    /// callers cannot substitute an unrelated invalidation source here.
    ///
    /// This validates batch shape only. Each preparation still resolves every
    /// original association and exact execution scope through `work_owner`.
    pub fn work_context(
        self: &Arc<Self>,
        associations: Arc<[InputAuthorityAssociation]>,
        execution_scope: OperationExecutionScope,
    ) -> Result<WorkAuthorizationContext, OperationRefused> {
        let policy: Arc<dyn LocalWorkPolicy> = self.clone();
        self.grants
            .work_context(associations, execution_scope, policy)
    }

    /// Validate current controller grant/account policy before acceptance.
    /// This never authenticates a caller, admits work or returns an entry permit.
    /// The native ingress owner performs its independent current invocation
    /// check and excludes a protected policy removal until acceptance commits.
    pub fn validate_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.grants.observe_controller_admission(|now_ms| {
            let candidate = association.candidate();
            if candidate.controller_model.as_ref() != Some(facts.selection()) {
                return Err(denied().into());
            }
            let grant = self
                .grants
                .resolve_controller_lineage_observed(
                    &candidate.controller_grant_lineage,
                    &candidate.logical_executor,
                    candidate.represented_subject.as_ref(),
                )
                .map_err(observed_grant_refusal)?;
            let allowance =
                self.operation_owner
                    .authorize_controller_admission(association, facts, now_ms)?;
            if allowance.operation_values.is_empty()
                || !matches!(
                    allowance.restrictions.lifetime.bound(),
                    LifetimeBound::Unrestricted
                )
                || !matches!(
                    candidate.controller_ceiling.lifetime.bound(),
                    LifetimeBound::Unrestricted
                )
            {
                return Err(denied().into());
            }
            let restrictions = allowance
                .restrictions
                .conjoin(&candidate.controller_ceiling)
                .conjoin(grant.restrictions());
            for values in &allowance.operation_values {
                restrictions
                    .check_bounds(values.at(now_ms))
                    .map_err(|_| denied())?;
            }
            Ok(())
        })
    }

    /// Reserve this policy's exact publication for a controller-preserving
    /// proposed change. The caller still owns all policy decisions and must
    /// follow `ControllerPolicyChange`'s native/policy lock and commit contract.
    pub fn reserve_controller_policy_change(
        &self,
    ) -> Result<ControllerPolicyChange<'_>, PublicationError> {
        Ok(ControllerPolicyChange {
            publication: self.grants.reserve_controller_policy_change()?,
        })
    }

    fn resolve_grant(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        controller: bool,
    ) -> Result<Option<ResolvedGrant>, meerkat_core::OperationAuthorizationError> {
        let candidate = association.candidate();
        let lineage: &[_] = if controller {
            &candidate.controller_grant_lineage
        } else {
            match &candidate.authority_basis {
                // Only reached after the trusted work owner resolved this
                // exact policy or occurrence. No implicit grant is synthesized.
                WorkAuthorityBasis::HostPolicy { .. }
                | WorkAuthorityBasis::ServiceMandate { .. } => {
                    return Ok(None);
                }
                // One lineage for every operation, as recorded.
                WorkAuthorityBasis::GrantLineage { lineage } => lineage,
                // The lineage selected for this operation's typed target
                // only; a target without one is denied for this operation.
                basis @ WorkAuthorityBasis::TargetedGrantLineages { .. } => {
                    let target = operation_grant_target(binding)?;
                    basis.lineage_for(target.as_ref()).ok_or_else(denied)?
                }
            }
        };
        let resolved = if controller {
            self.grants.resolve_controller_lineage_observed(
                lineage,
                &candidate.logical_executor,
                candidate.represented_subject.as_ref(),
            )
        } else {
            self.grants.resolve_lineage_observed(
                lineage,
                &candidate.logical_executor,
                candidate.represented_subject.as_ref(),
            )
        };
        resolved.map(Some).map_err(observed_grant_refusal)
    }
}

impl GrantBackedWorkPolicy {
    /// The generated grant owner this policy resolves grants in.
    pub fn grants(&self) -> &Arc<LocalGrantAuthority> {
        &self.grants
    }

    /// This policy's Operation answer for `binding`, conjoined with each of
    /// `further` grants (restrictions and expiry), with every actual
    /// operation tuple required within the conjunction at `now_ms`. For a
    /// trusted native composition that must also carry the ceilings of
    /// originals an association does not name itself; it adds no authority.
    ///
    /// # Errors
    /// Any refusal of the policy, an expired conjunction, or a tuple outside
    /// it is denied; unavailable owners are unavailable.
    pub fn evaluate_operation_within<'a>(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        further: impl IntoIterator<Item = &'a ResolvedGrant>,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let mut allowance =
            self.evaluate_for(association, binding, LocalPolicyPurpose::Operation, now_ms)?;
        for grant in further {
            allowance.restrictions = allowance.restrictions.conjoin(grant.restrictions());
            allowance.expires_at_ms = allowance.expires_at_ms.min(grant.expires_at_ms());
        }
        if allowance.operation_values.is_empty() || allowance.expires_at_ms <= now_ms {
            return Err(denied().into());
        }
        for values in &allowance.operation_values {
            allowance
                .restrictions
                .check_bounds(values.at(now_ms))
                .map_err(|_| denied())?;
        }
        Ok(allowance)
    }
}

impl LocalWorkPolicy for GrantBackedWorkPolicy {
    fn grant_backed(self: Arc<Self>) -> Option<Arc<GrantBackedWorkPolicy>> {
        Some(self)
    }

    fn review_context(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<
        meerkat_core::authorization::ReviewOperationContextResolution,
        meerkat_core::OperationAuthorizationError,
    > {
        use meerkat_core::authorization::{
            ReviewOperationContext, ReviewOperationContextResolution,
        };
        let action_account =
            self.operation_owner
                .review_action_account(association, binding, now_ms)?;
        let mandate = self
            .work_owner
            .review_mandate(association, binding, now_ms)?;
        if action_account.is_none() && mandate.is_none() {
            return Ok(ReviewOperationContextResolution::NotApplicable);
        }
        // Owner text outside the bound is not truncated or dropped: the
        // required material is unavailable.
        ReviewOperationContext::new(action_account, mandate)
            .map(ReviewOperationContextResolution::Material)
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)
    }

    fn evaluate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let controller_model = association.candidate().controller_model.as_ref();
        let purpose = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if crate::work::controller_route(facts, controller_model) =>
            {
                // The legacy single-allowance method cannot encode independent
                // model and hosted-capability bounds. The compiler invokes
                // evaluate_for twice for this exact same binding instead.
                if !facts.hosted_capabilities.is_empty() {
                    return Err(denied().into());
                }
                LocalPolicyPurpose::Controller
            }
            _ => LocalPolicyPurpose::Operation,
        };
        self.evaluate_for(association, binding, purpose, now_ms)
    }

    fn evaluate_for(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let controller = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if crate::work::controller_route(
                    facts,
                    association.candidate().controller_model.as_ref(),
                ) =>
            {
                Some(facts)
            }
            _ => None,
        };
        if purpose == LocalPolicyPurpose::Controller && controller.is_none()
            || purpose == LocalPolicyPurpose::Operation
                && controller.is_some_and(|facts| facts.hosted_capabilities.is_empty())
        {
            return Err(denied().into());
        }
        // Authentication and actual caller entitlement precede grant lookup.
        // Each role is resolved independently; one cannot stand in for another.
        let owner =
            self.work_owner
                .authorize_admitted_work(association, binding, purpose, now_ms)?;
        if purpose == LocalPolicyPurpose::Controller {
            let facts = controller.ok_or_else(denied)?;
            let selected = association
                .candidate()
                .controller_model
                .as_ref()
                .ok_or_else(denied)?;
            if !selected.matches_model_facts(facts) {
                return Err(denied().into());
            }
        }
        let grant = self.resolve_grant(
            association,
            binding,
            purpose == LocalPolicyPurpose::Controller,
        )?;
        let mut operation =
            self.operation_owner
                .authorize_operation(association, binding, purpose, now_ms)?;
        operation.restrictions = owner.restrictions.conjoin(&operation.restrictions);
        operation.expires_at_ms = owner.expires_at_ms.min(operation.expires_at_ms);
        if let Some(grant) = &grant {
            operation.restrictions = operation.restrictions.conjoin(grant.restrictions());
            operation.expires_at_ms = operation.expires_at_ms.min(grant.expires_at_ms());
        }
        if operation.operation_values.is_empty() || operation.expires_at_ms <= now_ms {
            return Err(denied().into());
        }
        // A direct policy caller also receives no allowance for known exclusion
        // or unresolved required facts. The work compiler separately conjoins
        // the purpose's retained admitted ceiling and every original input.
        for values in &operation.operation_values {
            // A connector approval is the work's own grant's record, listed
            // explicitly: a wildcard bound does not approve one, and a grant
            // that records none for the server is missing evidence, not a
            // verdict.
            if values.action.feature == crate::mcp_installation::MCP_CONNECTOR_FEATURE {
                let recorded = grant.as_ref().map(|grant| {
                    crate::mcp_installation::lineage_connector_bound(
                        grant.lineage_restrictions(),
                        grant.restrictions(),
                        &values.action,
                        &values.resource_domain,
                    )
                });
                match recorded {
                    Some(crate::mcp_installation::McpConnectorBound::Approved) => {}
                    Some(crate::mcp_installation::McpConnectorBound::Other) => {
                        return Err(denied().into());
                    }
                    Some(crate::mcp_installation::McpConnectorBound::Unrecorded) | None => {
                        return Err(meerkat_core::OperationAuthorizationError::Unavailable);
                    }
                }
            }
            operation
                .restrictions
                .check_bounds(values.at(now_ms))
                .map_err(|_| denied())?;
        }
        Ok(operation)
    }
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

/// A grant resolution read inside the caller's own observation of the grant
/// publication. An owner change published there makes that observation
/// stale too: a stale refusal it classifies and may rebuild once, never a
/// known infrastructure failure that would hide the change.
fn observed_grant_refusal(
    refusal: LineageResolutionRefusal,
) -> meerkat_core::OperationAuthorizationError {
    match refusal {
        LineageResolutionRefusal::Changed => {
            OperationRefused::new(OperationRefusalKind::ReprepareRequired).into()
        }
        LineageResolutionRefusal::Refused(refusal) => grant_refusal(refusal),
    }
}

fn grant_refusal(refusal: GrantRefusal) -> meerkat_core::OperationAuthorizationError {
    match refusal {
        GrantRefusal::Denied => denied().into(),
        // ControllerInUse belongs to administration and is preserved by revoke.
        // Current resolution cannot produce it; never invent policy denial if
        // a future resolver reports an unsupported operational condition.
        GrantRefusal::ControllerInUse | GrantRefusal::Duplicate | GrantRefusal::Unavailable => {
            meerkat_core::OperationAuthorizationError::Unavailable
        }
    }
}

#[cfg(test)]
mod tests;

/// One exact attachment control; each prepared effect covers all of its
/// operations under one observation.
struct McpAttachmentAuthorization {
    policy: Arc<GrantBackedWorkPolicy>,
    control: Arc<meerkat_core::mcp_attachment_control::McpAttachmentControlRequest>,
}

impl meerkat_core::authorization::WorkAuthorization for McpAttachmentAuthorization {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<
        Arc<dyn meerkat_core::PreparedOperationAuthorization>,
        meerkat_core::OperationAuthorizationError,
    > {
        self.prepare_observed(binding).result
    }

    fn prepare_observed(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> meerkat_core::authorization::ObservedAuthorizationResult<
        Arc<dyn meerkat_core::PreparedOperationAuthorization>,
    > {
        let Some(effect) = self.control.effect_of(binding) else {
            return meerkat_core::authorization::ObservedAuthorizationResult::unobserved(Err(
                denied().into(),
            ));
        };
        let operations = self
            .control
            .effect_operations(effect)
            .into_iter()
            .map(|operation| self.control.binding_for(operation))
            .collect::<Vec<_>>();
        self.policy
            .grants
            .compile_context_control(binding, |now_ms| {
                let mandate = self.policy.work_owner.authorize_mcp_attachment(
                    &self.control,
                    binding,
                    now_ms,
                )?;
                let mut allowances = Vec::with_capacity(operations.len());
                for operation in &operations {
                    let mut allowance = self.policy.operation_owner.authorize_mcp_attachment(
                        &self.control,
                        operation,
                        now_ms,
                    )?;
                    allowance.restrictions = allowance.restrictions.conjoin(&mandate.restrictions);
                    allowance.expires_at_ms = allowance.expires_at_ms.min(mandate.expires_at_ms);
                    allowances.push(allowance);
                }
                Ok(allowances)
            })
    }
}

struct ToolApplicationAuthorization {
    policy: Arc<GrantBackedWorkPolicy>,
    control: Arc<meerkat_core::ToolApplicationControlRequest>,
}

impl meerkat_core::authorization::WorkAuthorization for ToolApplicationAuthorization {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<
        Arc<dyn meerkat_core::PreparedOperationAuthorization>,
        meerkat_core::OperationAuthorizationError,
    > {
        self.prepare_observed(binding).result
    }

    fn prepare_observed(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> meerkat_core::authorization::ObservedAuthorizationResult<
        Arc<dyn meerkat_core::PreparedOperationAuthorization>,
    > {
        use meerkat_core::{
            ToolApplicationOperation,
            authorization::{ObservedAuthorizationResult, SourceAuthorizationUse},
        };
        if let Err(error) = self.control.revalidate() {
            return ObservedAuthorizationResult::unobserved(Err(error));
        }
        let facts = binding.facts();
        let exact_kind = match (&self.control.request().operation, &facts.operation) {
            (
                ToolApplicationOperation::CallTool { arguments, .. },
                AuthorizationOperation::Tool(tool),
            ) => serde_json::from_str::<serde_json::Value>(tool.arguments.get())
                .is_ok_and(|actual| &actual == arguments),
            (
                ToolApplicationOperation::ReadResource { .. },
                AuthorizationOperation::Source(source),
            ) => source.usage == SourceAuthorizationUse::Read,
            _ => false,
        };
        if !exact_kind
            || facts.execution_scope != OperationExecutionScope::Domain
            || facts.run_id.is_some()
            || facts.context_revision.is_some()
        {
            return ObservedAuthorizationResult::unobserved(Err(denied().into()));
        }
        self.policy
            .grants
            .compile_context_control(binding, |now_ms| {
                self.control.revalidate()?;
                let mandate = self.policy.work_owner.authorize_tool_application(
                    &self.control,
                    binding,
                    now_ms,
                )?;
                let mut allowance = self.policy.operation_owner.authorize_tool_application(
                    &self.control,
                    binding,
                    now_ms,
                )?;
                allowance.restrictions = allowance.restrictions.conjoin(&mandate.restrictions);
                allowance.expires_at_ms = allowance.expires_at_ms.min(mandate.expires_at_ms);
                Ok(vec![allowance])
            })
            .map(|inner| {
                Arc::new(ToolApplicationDecision {
                    inner,
                    control: Arc::clone(&self.control),
                }) as Arc<dyn meerkat_core::PreparedOperationAuthorization>
            })
    }
}

struct ToolApplicationDecision {
    inner: Arc<dyn meerkat_core::PreparedOperationAuthorization>,
    control: Arc<meerkat_core::ToolApplicationControlRequest>,
}

impl meerkat_core::PreparedOperationAuthorization for ToolApplicationDecision {
    fn policy_observation(
        &self,
    ) -> Option<meerkat_core::authorization::PolicyPublicationObservation> {
        self.inner.policy_observation()
    }
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        self.inner.review_tier()
    }
    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.control.revalidate()?;
        self.inner.check_current(binding)
    }
    fn observe(
        &self,
        binding: &PreparedAuthorizationBinding,
        observation: meerkat_core::authorization::OperationObservation,
    ) -> Result<(), meerkat_core::authorization::OperationObservationError> {
        self.inner.observe(binding, observation)
    }
}

/// One exact control; every prepared projection covers both source and audience.
struct ContextControlAuthorization {
    policy: Arc<GrantBackedWorkPolicy>,
    control: Arc<meerkat_core::service::SystemContextControlRequest>,
}

impl meerkat_core::authorization::WorkAuthorization for ContextControlAuthorization {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<
        Arc<dyn meerkat_core::PreparedOperationAuthorization>,
        meerkat_core::OperationAuthorizationError,
    > {
        self.prepare_observed(binding).result
    }

    fn prepare_observed(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> meerkat_core::authorization::ObservedAuthorizationResult<
        Arc<dyn meerkat_core::PreparedOperationAuthorization>,
    > {
        use meerkat_core::authorization::{
            OperationAuthorizationFacts, PublicationRecipient, SourceAuthorizationFacts,
            SourceAuthorizationUse,
        };
        let facts = binding.facts();
        if facts.operation_id != *self.control.operation_id()
            || facts.execution_scope != OperationExecutionScope::Domain
            || facts.run_id.is_some()
            || facts.context_revision.is_some()
            || !matches!(&facts.operation, AuthorizationOperation::Publication(publication)
                if matches!(&publication.recipient, PublicationRecipient::Destination(destination)
                    if destination.authority == self.control.facts().destination.authority
                    && destination.namespace == self.control.facts().destination.namespace
                    && destination.id == self.control.facts().destination.id)
                && publication.scope.is_none() && publication.live_channel.is_none()
                && publication.mode == meerkat_core::authorization::PublicationMode::Buffered)
        {
            return meerkat_core::authorization::ObservedAuthorizationResult::unobserved(Err(
                denied().into(),
            ));
        }
        let source = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: facts.operation_id.clone(),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: self.control.facts().source.clone(),
                usage: SourceAuthorizationUse::Hydrate,
            }),
        });
        self.policy
            .grants
            .compile_context_control(binding, |now_ms| {
                let mandate = self.policy.work_owner.authorize_context_control(
                    &self.control,
                    binding,
                    now_ms,
                )?;
                let mut allowances = Vec::with_capacity(2);
                for operation in [&source, binding] {
                    let mut allowance = self.policy.operation_owner.authorize_context_control(
                        &self.control,
                        operation,
                        now_ms,
                    )?;
                    allowance.restrictions = allowance.restrictions.conjoin(&mandate.restrictions);
                    allowance.expires_at_ms = allowance.expires_at_ms.min(mandate.expires_at_ms);
                    allowances.push(allowance);
                }
                Ok(allowances)
            })
    }
}
