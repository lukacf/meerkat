//! The stock host's current invocation policy for its scoped originals.
//!
//! In the local-operator posture stock rkat has one local operator per
//! process: the requester its CLI ingress authenticated. Its policy decides
//! only who may invoke what, now: the operator, through the stock ingress
//! actor, in this realm, invoking an executor for itself (no represented
//! subject) at one of the operator's own stock session destinations, under a
//! grant-lineage mandate. In the host-authenticated posture
//! ([`super::caller`]) the requester is instead a caller the trusted host
//! authenticated, admitted only by the host's exact-target decision for the
//! session it invokes; the operator stays the session's canonical owner and
//! is no requester. It keeps no registry and no store. Every other conjunct
//! stays with its owner and is never widened here: the native grant owner
//! resolves the lineage, the operation owner answers resources, accounts and
//! connectors, and the realm's MCP installation ceiling is enforced by the
//! connection.
//!
//! A refusal is a verdict ([`OperationRefusalKind::Denied`]): during an
//! active turn the native loop returns it to the model as that operation's
//! typed result, and siblings continue. When the composition cannot be built
//! (no restored grant owner or stock grant selection) the host has no owners
//! and opens the realm governed without them, where admission is unavailable
//! before any model runs; that is never a verdict.
//!
//! The same object is the stock operation owner. It maps each operation to
//! the true fact tuples of the stock owner that actually supplies it, in the
//! stock vocabulary (operator-owned domains, executor as the logical-agent
//! processor, the work's own destination as recipient):
//! - the admitted controller route (`infer`), and compaction on it;
//! - the native tool catalog pair as catalog reads (loading grants no
//!   invocation);
//! - an MCP operation by its owner-produced registration and raw operation
//!   (the installation's access ceiling stays the connection's conjunct);
//! - an MCP operation a trusted host's checked route reached, by the same
//!   registration and raw operation (the route's own selection replaces the
//!   ceiling at the connection, and the work's grant must be the one
//!   selected for exactly that route to that server);
//! - a registered tool bundle's operation by its owner-produced target: the
//!   trusted host's stable bundle name and the operation, which must be the
//!   call's own. The bundle's registration revalidates its live witness at
//!   dispatch, so a plan only runs on the registration that resolved it;
//!   the work's grant must select exactly that bundle operation;
//! - semantic memory of the work's own session.
//!
//! A refusal by one of these rules (another session's memory, a controller
//! request that is not the admitted route) is a Denied verdict.
//!
//! Families with no stock mapping yet are unavailable
//! ([`OperationAuthorizationError::Unavailable`]), never a fabricated
//! verdict and never a family allowance: that operation alone receives typed
//! local feedback and the turn continues. They are, explicitly:
//! - builtin tools and host tools that are not a registered bundle's
//!   operation (a dispatcher plan with no owner-produced operation target),
//!   the builtin web_search operation (its owner-produced
//!   [`ToolOperationTarget::BuiltinOperation`] target), and provider-hosted
//!   tool targets;
//! - comms deliveries, the trusted-peer directory read and memory
//!   operations. Their plans carry typed targets (`PeerSend`,
//!   `PeerDirectoryRead`, `MemoryOperation`), but stock issues no grant for
//!   those targets, so under a per-target selection the grant owner denies
//!   them before this owner is asked; an older single worker grant reaches
//!   this owner, which leaves them unmapped;
//! - blobs, runtime inputs, transcript ranges and external sources;
//! - publications;
//! - hosted capabilities, alternate models and live model channels,
//!   including the web_search helper's own inference as that call's
//!   attributed delegate.
//!
//! Their bounded mappings are follow-up work. Stock issues no grant for the
//! builtin web_search operation or its delegated hosted search: under a
//! per-target stock selection both have no lineage and are refused by the
//! grant owner (Denied) before this owner is asked; under a single stock
//! grant this owner leaves them unmapped. This is not stock coverage of the
//! builtin web_search; an embedding host's own owners map those facts.
//!
//! Every mapped operation is R1, which reads no review material. For a
//! composition that requires review, the mandate is the checked invocation
//! itself with the bounds and effective expiry that the work's own grant
//! lineage for the operation, resolved now, and the work's admitted ceiling
//! impose, and the action account is explicitly not applicable for the
//! catalog read and the work's own session memory and unavailable for every
//! other operation (MCP included) until its owner supplies it.

use std::sync::Arc;

use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, ControllerAdmissionAllowance, OperationPolicyOwner,
    WorkOwnerAllowance, operation_grant_target,
};
use meerkat_authorization::grants::{GrantRefusal, LineageResolutionRefusal, LocalGrantAuthority};
use meerkat_authorization::mcp_installation::mcp_connector_action;
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization_contracts::constraints::{
    ActionRef, ExecutionRestrictions, ProcessorRef, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::HistoricalEvidenceRef;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse, OperationReviewTier,
    PreparedAuthorizationBinding, SourceAuthorizationFacts, SourceAuthorizationTarget,
    SourceAuthorizationUse, ToolAuthorizationFacts, ToolAuthorizationTarget,
};
use meerkat_core::connection::RealmId;
use meerkat_core::mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal};
use meerkat_core::mcp_attachment_control::{McpAttachmentControlRequest, McpAttachmentEffect};
use meerkat_core::{ControllerModelFacts, OperationAccount, SessionId, ToolOperationTarget};
use meerkat_core::{
    OperationAuthorizationError, OperationRefusalKind, OperationRefused, PrincipalRef,
};
use meerkat_runtime::input_authority::{NativeAdmissionError, NativeIngressContext};
use meerkat_runtime::meerkat_machine::{ScopedMandateOwner, review_bounds_text};

use super::StockPrincipals;
use super::caller::{StockCallerPosture, StockCallerRequest, StockLoginLifetime};
use super::destination::StockSessionDestination;

/// How long one policy decision stands before the native owner asks again.
/// It bounds only this decision; the grant's own expiry is conjoined.
const DECISION_WINDOW_MS: u64 = 60_000;

/// One concrete policy: both invocation faces from the same `Arc`
/// ([`meerkat_runtime::meerkat_machine::ScopedInvocation::from_policy`]) and
/// the composition's operation owner.
pub struct StockInvocationPolicy {
    operator: PrincipalRef,
    ingress_actor: PrincipalRef,
    executor: PrincipalRef,
    realm: RealmId,
    /// Who may be the requester: the operator, or the host's authenticated
    /// callers ([`super::caller`]).
    posture: StockCallerPosture,
    /// The realm's own MCP installations: a live attachment is only ever one
    /// of them.
    installations: Arc<dyn ConfiguredMcpAttachmentAuthority>,
    /// The restored grant owner of this composition. The native owner
    /// resolves every lineage against it; the policy never widens it, and
    /// reads in it only the bounds its review mandate states.
    grants: Arc<LocalGrantAuthority>,
    /// The realm owner's review rules: they only raise the review tier of
    /// an operation these rules already allow.
    review: super::review::StockReviewRules,
}

impl std::fmt::Debug for StockInvocationPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StockInvocationPolicy")
            .finish_non_exhaustive()
    }
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

/// Whether `requester` may be the requester of work in `session`, a session
/// owned by `operator`, invoking `executor`, in `posture`: in the
/// local-operator posture only the operator; in the host-authenticated
/// posture only a caller the host admits for exactly that session, and never
/// the operator. The one requester rule of the native invocation policy and
/// of the stock binding of a caller's input, which asks it before the
/// session's binding changes ([`super::StockGovernance::bind_caller_prompt`]).
pub(super) fn admit_stock_requester(
    posture: &StockCallerPosture,
    operator: &PrincipalRef,
    executor: &PrincipalRef,
    requester: &PrincipalRef,
    authentication: Option<&HistoricalEvidenceRef>,
    session: &SessionId,
) -> Result<StockLoginLifetime, OperationAuthorizationError> {
    match posture {
        StockCallerPosture::LocalOperator if requester == operator => {
            Ok(StockLoginLifetime::UntilRetired)
        }
        StockCallerPosture::LocalOperator => Err(denied().into()),
        StockCallerPosture::HostAuthenticated(_) if requester == operator => Err(denied().into()),
        StockCallerPosture::HostAuthenticated(access) => {
            access.authorize_caller(&StockCallerRequest {
                caller: requester,
                authentication,
                executor,
                owner: operator,
                session,
            })
        }
    }
}

impl StockInvocationPolicy {
    /// The policy of the stock host of `realm`, whose ingress authenticates
    /// `principals.operator` through `principals.ingress_actor` and whose
    /// agent executor is `principals.executor`: the local-operator posture.
    pub fn new(
        grants: Arc<LocalGrantAuthority>,
        principals: &StockPrincipals,
        realm: RealmId,
        installations: Arc<dyn ConfiguredMcpAttachmentAuthority>,
    ) -> Arc<Self> {
        Self::with_caller_posture(
            grants,
            principals,
            realm,
            installations,
            StockCallerPosture::LocalOperator,
        )
    }

    /// The policy of a stock composition of `realm` that admits requesters
    /// in `posture`; `principals.operator` stays the canonical owner of
    /// every session it serves.
    pub fn with_caller_posture(
        grants: Arc<LocalGrantAuthority>,
        principals: &StockPrincipals,
        realm: RealmId,
        installations: Arc<dyn ConfiguredMcpAttachmentAuthority>,
        posture: StockCallerPosture,
    ) -> Arc<Self> {
        Self::with_review_rules(
            grants,
            principals,
            realm,
            installations,
            posture,
            super::review::StockReviewRules::default(),
        )
    }

    /// [`Self::with_caller_posture`] whose operations require the review
    /// `review` selects: an operation these rules allow needs the
    /// strictest tier a matching rule names, and R1 otherwise. The rules
    /// allow nothing these rules refuse or leave unmapped.
    pub fn with_review_rules(
        grants: Arc<LocalGrantAuthority>,
        principals: &StockPrincipals,
        realm: RealmId,
        installations: Arc<dyn ConfiguredMcpAttachmentAuthority>,
        posture: StockCallerPosture,
        review: super::review::StockReviewRules,
    ) -> Arc<Self> {
        Arc::new(Self {
            operator: principals.operator.clone(),
            ingress_actor: principals.ingress_actor.clone(),
            executor: principals.executor.clone(),
            realm,
            posture,
            installations,
            grants,
            review,
        })
    }

    /// Whether `requester` may be the requester of work in `session` (a
    /// session of the operator), by the one stock requester rule
    /// ([`admit_stock_requester`]).
    fn admit_requester(
        &self,
        requester: &PrincipalRef,
        authentication: Option<&HistoricalEvidenceRef>,
        session: &SessionId,
    ) -> Result<StockLoginLifetime, OperationAuthorizationError> {
        admit_stock_requester(
            &self.posture,
            &self.operator,
            &self.executor,
            requester,
            authentication,
            session,
        )
    }

    /// Whether the association is an admitted requester's invocation at one
    /// of the operator's stock session destinations, under a mandate the
    /// grant owner resolves. Every claim is compared; none is inferred.
    fn admit(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<StockLoginLifetime, OperationAuthorizationError> {
        let claims = association.candidate();
        let mandate = match &claims.authority_basis {
            WorkAuthorityBasis::GrantLineage { lineage } => !lineage.is_empty(),
            // Validated as a non-empty selection when the association was built.
            WorkAuthorityBasis::TargetedGrantLineages { .. } => true,
            WorkAuthorityBasis::HostPolicy { .. } | WorkAuthorityBasis::ServiceMandate { .. } => {
                false
            }
        };
        let session = StockSessionDestination::stock_session(&self.operator, &claims.target);
        match session {
            Some(session)
                if mandate
                    && claims.ingress_actor == self.ingress_actor
                    && claims.logical_executor == self.executor
                    && claims.ingress_namespace.realm == self.realm
                    && claims.represented_subject.is_none() =>
            {
                self.admit_requester(
                    &claims.requester,
                    Some(&claims.original_authentication),
                    &session,
                )
            }
            _ => Err(denied().into()),
        }
    }

    /// One decision for a requester admitted in `login`: it stands for this
    /// policy's decision window, never past the login's own deadline.
    fn allowance(
        now_ms: u64,
        login: StockLoginLifetime,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: Self::decision_expiry(now_ms, login)?,
        })
    }

    /// When a decision taken at `now_ms` for a requester admitted in `login`
    /// expires: the decision window bounded by the login's deadline. A login
    /// past its deadline admits nothing, even before the host retires it.
    fn decision_expiry(
        now_ms: u64,
        login: StockLoginLifetime,
    ) -> Result<u64, OperationAuthorizationError> {
        if !login.is_current_at(now_ms) {
            return Err(denied().into());
        }
        Ok(login.bound(now_ms.saturating_add(DECISION_WINDOW_MS)))
    }
}

impl ScopedMandateOwner for StockInvocationPolicy {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        // The ingress authenticated exactly the association's requester.
        if ingress.requester() != &association.candidate().requester
            || ingress.ingress_actor() != &self.ingress_actor
            || ingress.realm() != &self.realm
        {
            return Err(NativeAdmissionError::Refused(denied()));
        }
        self.admit(association)
            .map(|_| ())
            .map_err(NativeAdmissionError::from)
    }
}

impl AdmittedWorkPolicyOwner for StockInvocationPolicy {
    /// An attachment control of an admitted requester, through the stock
    /// ingress, in this realm, for one of the operator's own session
    /// destinations, of a server that is one of the realm's own
    /// installations. The machine separately joins the destination with the
    /// session's current binding; the connection enforces the installation's
    /// ceiling.
    fn authorize_mcp_attachment(
        &self,
        control: &McpAttachmentControlRequest,
        _binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        let facts = control.facts();
        if facts.actor != self.ingress_actor
            || facts.realm != self.realm
            || facts.destination.authority != self.operator
            || &*facts.destination.namespace != "session"
            || facts.destination.id.as_ref() != control.session_id().to_string()
        {
            return Err(denied().into());
        }
        let login = self.admit_requester(&facts.requester, None, control.session_id())?;
        self.installations
            .authorize_configured(control.server())
            .map_err(|refusal| match refusal {
                McpAttachmentRefusal::Denied | McpAttachmentRefusal::Mismatch => {
                    OperationAuthorizationError::from(denied())
                }
                McpAttachmentRefusal::Unavailable
                | McpAttachmentRefusal::NotCurrent
                | McpAttachmentRefusal::ReviewRequired => OperationAuthorizationError::Unavailable,
            })?;
        Self::allowance(now_ms, login)
    }

    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        let login = self.admit(association)?;
        Self::allowance(now_ms, login)
    }

    /// The stock mandate is the checked invocation itself: the admitted
    /// requester (the operator, or the host's authenticated caller), through
    /// the stock ingress, in this realm, invoking the realm's executor at one
    /// of the operator's own session destinations, under a grant-lineage
    /// mandate that the grant owner resolves at every operation. Stock holds
    /// no stored human or service mandate, so work the stock rule does not
    /// admit (a host-policy or service-mandate basis among it) is refused as
    /// at authorization. The text names this policy's own principals and the
    /// admitted requester, each in its serialized form (kind, id and
    /// qualification); it never repeats a lineage reference, which
    /// identifies a grant and is not consent.
    ///
    /// It states the bounds that mandate imposes on this operation now:
    /// the lineage the work's basis selects for the binding's target,
    /// resolved in the grant owner for the work's executor and represented
    /// subject exactly as the native owner resolves it, conjoined with the
    /// ceiling admitted with the work, and their effective expiry, the
    /// lineage's earliest. This policy's decision window bounds only how
    /// long one decision stands and is not stated as a grant lifetime. A
    /// revoked, expired or otherwise not current lineage, and a targeted
    /// selection with none for this target, are refused as at
    /// authorization; an owner that cannot say now is unavailable. An owner
    /// change published while the grant is read inside the review's own
    /// observation is a stale refusal (`ReprepareRequired`).
    fn review_mandate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<Option<String>, OperationAuthorizationError> {
        self.admit(association)?;
        let claims = association.candidate();
        let lineage = match &claims.authority_basis {
            WorkAuthorityBasis::GrantLineage { lineage } => lineage.as_slice(),
            basis @ WorkAuthorityBasis::TargetedGrantLineages { .. } => {
                let target = operation_grant_target(binding)?;
                basis.lineage_for(target.as_ref()).ok_or_else(denied)?
            }
            // Not admitted.
            WorkAuthorityBasis::HostPolicy { .. } | WorkAuthorityBasis::ServiceMandate { .. } => {
                return Err(denied().into());
            }
        };
        let grant = self
            .grants
            .resolve_lineage_observed(
                lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(|refusal| match refusal {
                // Read inside the review's own observation of the grant
                // publication, which is stale too: a stale refusal it
                // classifies, never an unavailable review context.
                LineageResolutionRefusal::Changed => {
                    OperationRefused::new(OperationRefusalKind::ReprepareRequired).into()
                }
                LineageResolutionRefusal::Refused(GrantRefusal::Denied) => {
                    OperationAuthorizationError::from(denied())
                }
                LineageResolutionRefusal::Refused(
                    GrantRefusal::ControllerInUse
                    | GrantRefusal::Duplicate
                    | GrantRefusal::Unavailable,
                ) => OperationAuthorizationError::Unavailable,
            })?;
        let bounds = review_bounds_text(
            &grant.restrictions().conjoin(&claims.admitted_ceiling),
            grant.expires_at_ms(),
            now_ms,
        )?;
        let principal = |principal: &PrincipalRef| {
            serde_json::to_string(principal).map_err(|_| OperationAuthorizationError::Unavailable)
        };
        Ok(Some(match &self.posture {
            StockCallerPosture::LocalOperator => format!(
                "The local operator {} invoked the realm agent {} through the stock ingress {} \
                 in realm {}, for one of the operator's own sessions, under a grant-lineage \
                 mandate. With the work's admitted ceiling it bounds this operation to: {bounds}",
                principal(&self.operator)?,
                principal(&self.executor)?,
                principal(&self.ingress_actor)?,
                self.realm.as_str(),
            ),
            StockCallerPosture::HostAuthenticated(_) => format!(
                "The caller {} authenticated by the trusted host invoked the realm agent {} \
                 through the ingress {} in realm {}, for a session owned by {}, as the host's \
                 access decision for that session allows, under a grant-lineage mandate. With \
                 the work's admitted ceiling it bounds this operation to: {bounds}",
                principal(&claims.requester)?,
                principal(&self.executor)?,
                principal(&self.ingress_actor)?,
                self.realm.as_str(),
                principal(&self.operator)?,
            ),
        }))
    }
}

/// The stock vocabulary's feature of every action this owner declares.
const STOCK_FEATURE: &str = "stock";
/// The stock vocabulary's feature of MCP raw operations.
const MCP_FEATURE: &str = "mcp";
/// The stock vocabulary's feature of registered tool bundle operations.
pub(super) const TOOL_BUNDLE_FEATURE: &str = "tool_bundle";

/// The operator-owned domain of the registered tool bundle `bundle`.
pub(super) fn tool_bundle_namespace(bundle: &str) -> String {
    format!("tool-bundle/{bundle}")
}

impl StockInvocationPolicy {
    /// One true fact tuple: `action` on the operator-owned `namespace`,
    /// processed by the realm's executor (the logical-agent processor),
    /// for the work's own destination.
    fn tuple(
        &self,
        association: &InputAuthorityAssociation,
        feature: &str,
        action: &str,
        namespace: String,
    ) -> LocalOperationValues {
        LocalOperationValues {
            action: ActionRef {
                feature: feature.to_string(),
                action: action.to_string(),
            },
            resource_domain: ResourceDomain {
                authority: self.operator.clone(),
                namespace,
            },
            processor: ProcessorRef::Principal {
                principal: self.executor.clone(),
            },
            audience: association.candidate().target.audience.clone(),
        }
    }

    /// The admitted controller's exact route, and nothing more: the native
    /// host already requires the association's controller to be the bound
    /// pin; this owner requires the request to be that same route. The
    /// selection matcher covers the logical model, provider, self-hosted
    /// server, auth binding, credential and backend identity. The endpoint
    /// and wire model are bound by the route key the operation values name
    /// ([`meerkat_core::ControllerRouteKey`]), which the controller grant
    /// must hold.
    fn controller_route(
        association: &InputAuthorityAssociation,
        facts: &ModelAuthorizationFacts,
    ) -> bool {
        association
            .candidate()
            .controller_model
            .as_ref()
            .is_some_and(|selection| selection.matches_model_facts(facts))
            && facts.hosted_capabilities.is_empty()
            && facts.live_channel.is_none()
    }

    fn model(
        &self,
        association: &InputAuthorityAssociation,
        facts: &ModelAuthorizationFacts,
        purpose: LocalPolicyPurpose,
    ) -> Result<Vec<LocalOperationValues>, OperationAuthorizationError> {
        let action = match (purpose, facts.usage) {
            // Continuation, and compaction of the same admitted controller
            // context: controller housekeeping on the retained route, the
            // same action and route as continuation. The Compaction label
            // alone grants nothing; the route must be the admitted one.
            (
                LocalPolicyPurpose::Controller,
                ModelAuthorizationUse::ControllerInference | ModelAuthorizationUse::Compaction,
            ) if Self::controller_route(association, facts) => "infer",
            // Native compaction presented under the Operation purpose (until
            // it is presented as controller housekeeping): the same route.
            (LocalPolicyPurpose::Operation, ModelAuthorizationUse::Compaction)
                if Self::controller_route(association, facts) =>
            {
                "compact"
            }
            // A controller request that is not the admitted route.
            (LocalPolicyPurpose::Controller, _) => return Err(denied().into()),
            // Hosted capabilities (the Operation conjunct of a controller
            // request), alternate models (a web_search helper's delegated
            // inference among them) and live channels: unmapped.
            (LocalPolicyPurpose::Operation, _) => {
                return Err(OperationAuthorizationError::Unavailable);
            }
        };
        // The request's own actual lowered route, as admission named it.
        let route = facts
            .controller_route_key()
            .ok_or_else(|| OperationAuthorizationError::from(denied()))?;
        Ok(vec![self.tuple(
            association,
            STOCK_FEATURE,
            action,
            route.as_str().to_string(),
        )])
    }

    fn tool(
        &self,
        association: &InputAuthorityAssociation,
        facts: &ToolAuthorizationFacts,
    ) -> Result<Vec<LocalOperationValues>, OperationAuthorizationError> {
        let ToolAuthorizationTarget::Dispatcher(plan) = &facts.target else {
            // A provider-hosted tool target: unmapped.
            return Err(OperationAuthorizationError::Unavailable);
        };
        // Classification comes only from the owner-produced target, never
        // from the tool name: any dispatcher can declare a catalog name.
        match meerkat_authorization::grant_policy::single_tool_operation_target(plan)? {
            // A read of the session's own tool catalog: it exposes metadata
            // and grants no later invocation.
            Some(ToolOperationTarget::CatalogControl { .. }) => Ok(vec![self.tuple(
                association,
                STOCK_FEATURE,
                "catalog-read",
                "tool-catalog".to_string(),
            )]),
            // The raw operation, and the connector binding of the actual
            // connection it runs on: the work's grant must cover both, so a
            // grant that approved another binding of the same server name
            // admits none of its operations. Reached through a host's
            // checked route, the operation is the same fact on the same
            // connection; the grant that must cover it is the one selected
            // for exactly that route ([`operation_grant_target`]).
            Some(
                ToolOperationTarget::McpServerOperation {
                    server,
                    raw_operation,
                    connector,
                    ..
                }
                | ToolOperationTarget::CheckedMcpServerOperation {
                    server,
                    raw_operation,
                    connector,
                    ..
                },
            ) => {
                let connector = mcp_connector_action(server, connector);
                Ok(vec![
                    self.tuple(
                        association,
                        MCP_FEATURE,
                        raw_operation,
                        format!("mcp-server/{server}"),
                    ),
                    self.tuple(
                        association,
                        &connector.feature,
                        &connector.action,
                        format!("mcp-server/{server}"),
                    ),
                ])
            }
            // One operation of a bundle the trusted host registered under
            // its stable name, attributed by that registration's own live
            // witness (revalidated by it at dispatch). The operation must be
            // the call's own; the work's grant must select exactly it.
            Some(ToolOperationTarget::RegisteredToolBundleOperation {
                bundle, operation, ..
            }) if facts.name == operation.as_str() => Ok(vec![self.tuple(
                association,
                TOOL_BUNDLE_FEATURE,
                operation,
                tool_bundle_namespace(bundle),
            )]),
            // No owner-produced target (a builtin tool, or a host tool
            // named like a catalog tool or a bundle operation), a bundle
            // target for another operation, the builtin web_search
            // operation's own target, or another target kind: unmapped. An
            // ambiguous one is refused above.
            _ => Err(OperationAuthorizationError::Unavailable),
        }
    }

    fn source(
        &self,
        association: &InputAuthorityAssociation,
        target: &SourceAuthorizationTarget,
    ) -> Result<Vec<LocalOperationValues>, OperationAuthorizationError> {
        let own_session = SessionId::parse(association.candidate().target.context.as_str()).ok();
        match target {
            SourceAuthorizationTarget::Memory(scope)
                if own_session.as_ref() == Some(scope.session_id()) =>
            {
                Ok(vec![self.tuple(
                    association,
                    STOCK_FEATURE,
                    "memory-read",
                    "session-memory".to_string(),
                )])
            }
            // Another session's memory: refused by the own-session rule.
            SourceAuthorizationTarget::Memory(_) => Err(denied().into()),
            // Blobs, runtime inputs, transcripts and external sources:
            // unmapped.
            SourceAuthorizationTarget::Blob(_)
            | SourceAuthorizationTarget::RuntimeInput { .. }
            | SourceAuthorizationTarget::Transcript { .. }
            | SourceAuthorizationTarget::External(_) => {
                Err(OperationAuthorizationError::Unavailable)
            }
        }
    }
}

/// The stock policy's conjunct for a configured MCP server: attaching one
/// of the operator's own realm installations to the realm executor's
/// session. The stock rule admits only a narrowed installation, one whose
/// host access ceiling is present and valid; an unceilinged or malformed
/// descriptor is refused. The installation itself, the per-agent grant and
/// the connection's ceiling stay their own owners' conjuncts.
impl ConfiguredMcpAttachmentAuthority for StockInvocationPolicy {
    fn authorize_configured(
        &self,
        server: &meerkat_core::McpServerConfig,
    ) -> Result<(), McpAttachmentRefusal> {
        if server.name.is_empty() || server.access.is_none() || server.validate_access().is_err() {
            return Err(McpAttachmentRefusal::Denied);
        }
        Ok(())
    }
}

impl StockInvocationPolicy {
    /// Whether `target` is exactly the server this control hydrates when it
    /// is applied.
    fn is_attached_server(
        control: &McpAttachmentControlRequest,
        target: &meerkat_core::authorization::OwnerQualifiedTarget,
    ) -> bool {
        control
            .effect_operations(McpAttachmentEffect::Apply)
            .iter()
            .any(|operation| {
                matches!(operation, AuthorizationOperation::Source(SourceAuthorizationFacts {
                    target: SourceAuthorizationTarget::External(expected),
                    usage: SourceAuthorizationUse::Hydrate,
                }) if expected.authority == target.authority
                    && expected.namespace == target.namespace
                    && expected.id == target.id)
            })
    }
}

impl OperationPolicyOwner for StockInvocationPolicy {
    /// The operation face of an MCP attachment: the same admitted decision
    /// as the work-owner face ([`AdmittedWorkPolicyOwner::authorize_mcp_attachment`]),
    /// never a second authority. Its values name the attached installation
    /// processed by the realm's executor for the session's own stock
    /// destination audience.
    fn authorize_mcp_attachment(
        &self,
        control: &McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        let admitted =
            AdmittedWorkPolicyOwner::authorize_mcp_attachment(self, control, binding, now_ms)?;
        let audience =
            StockSessionDestination::for_session(&self.operator, control.session_id(), 1)
                .map_err(|_| OperationAuthorizationError::from(denied()))?
                .target()
                .audience
                .clone();
        // Only the two operations of applying the attachment are mapped:
        // hydrating exactly this control's server, and publishing it to the
        // control's destination. Configuration retention (Persist) and any
        // other operation are unmapped.
        let (action, namespace) = match &binding.facts().operation {
            AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(target),
                usage: SourceAuthorizationUse::Hydrate,
            }) if Self::is_attached_server(control, target) => (
                ActionRef {
                    feature: MCP_FEATURE.to_string(),
                    action: "attach".to_string(),
                },
                format!("mcp-server/{}", control.server().name),
            ),
            AuthorizationOperation::Publication(_)
                if control.effect_of(binding) == Some(McpAttachmentEffect::Apply) =>
            {
                (
                    ActionRef {
                        feature: MCP_FEATURE.to_string(),
                        action: "publish".to_string(),
                    },
                    format!("session/{}", control.session_id()),
                )
            }
            _ => return Err(OperationAuthorizationError::Unavailable),
        };
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action,
                resource_domain: ResourceDomain {
                    authority: self.operator.clone(),
                    namespace,
                },
                processor: ProcessorRef::Principal {
                    principal: self.executor.clone(),
                },
                audience,
            }],
            restrictions: admitted.restrictions,
            expires_at_ms: admitted.expires_at_ms,
            review_tier: OperationReviewTier::R1,
        })
    }

    /// The admitted controller route at native admission: exactly the
    /// association's pinned selection, named by the key of the selected
    /// client's actual lowered route.
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &ControllerModelFacts,
        _now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, OperationAuthorizationError> {
        self.admit(association)?;
        if association.candidate().controller_model.as_ref() != Some(facts.selection()) {
            return Err(denied().into());
        }
        Ok(ControllerAdmissionAllowance {
            operation_values: vec![self.tuple(
                association,
                STOCK_FEATURE,
                "infer",
                facts.route_key().as_str().to_string(),
            )],
            restrictions: ExecutionRestrictions::unrestricted(),
        })
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        let login = self.admit(association)?;
        let operation_values = match (&binding.facts().operation, purpose) {
            (AuthorizationOperation::Model(facts), purpose) => {
                self.model(association, facts, purpose)?
            }
            (AuthorizationOperation::Tool(facts), LocalPolicyPurpose::Operation) => {
                self.tool(association, facts)?
            }
            (AuthorizationOperation::Source(facts), LocalPolicyPurpose::Operation) => {
                self.source(association, &facts.target)?
            }
            // Publications and any other pairing: unmapped.
            _ => return Err(OperationAuthorizationError::Unavailable),
        };
        // Only an operation the rules above allow reaches the owner's
        // review rules, which can only raise its tier.
        let review_tier = match &binding.facts().operation {
            AuthorizationOperation::Tool(ToolAuthorizationFacts {
                target: ToolAuthorizationTarget::Dispatcher(plan),
                ..
            }) => self
                .review
                .tier_for(meerkat_authorization::grant_policy::single_tool_operation_target(plan)?),
            _ => OperationReviewTier::R1,
        };
        Ok(LocalPolicyAllowance {
            operation_values,
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: Self::decision_expiry(now_ms, login)?,
            review_tier,
        })
    }

    /// The action account the operation's own owner states it acts with, for
    /// review and consent presentation, read after native authorization and
    /// only for reviewed operations. Stock selects none for a read of the
    /// session's own tool catalog or of the work's own session memory:
    /// explicitly not applicable there. An MCP or registered bundle
    /// operation states its account with its exact owner target: the
    /// account its connection's connector binding or OAuth selection names,
    /// or the owner configuration's or registration's declaration that it
    /// acts with none (not applicable). A target whose owner states neither
    /// is unavailable: a missing connector binding (`NoConnector`) is a grant
    /// lineage fact, never an account's absence, and the controller's
    /// credential owner is not the operation's. Tools without an owner
    /// target and every unmapped family are unavailable as well; no blanket
    /// answer stands in for their owners. Work and sources the stock rules
    /// refuse are refused here too.
    fn review_action_account(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<Option<String>, OperationAuthorizationError> {
        self.admit(association)?;
        match &binding.facts().operation {
            AuthorizationOperation::Tool(facts) => {
                // Only an operation the stock rules map has review material.
                self.tool(association, facts)?;
                let ToolAuthorizationTarget::Dispatcher(plan) = &facts.target else {
                    return Err(OperationAuthorizationError::Unavailable);
                };
                match meerkat_authorization::grant_policy::single_tool_operation_target(plan)? {
                    Some(ToolOperationTarget::CatalogControl { .. }) => Ok(None),
                    Some(target) => match target.account() {
                        Some(OperationAccount::Selected(account)) => Ok(Some(account.to_string())),
                        Some(OperationAccount::DeclaredNone) => Ok(None),
                        _ => Err(OperationAuthorizationError::Unavailable),
                    },
                    None => Err(OperationAuthorizationError::Unavailable),
                }
            }
            // Exactly the mapped own-session memory read: the same rule
            // refuses another session's memory and leaves every other
            // source unmapped.
            AuthorizationOperation::Source(facts) => {
                self.source(association, &facts.target).map(|_| None)
            }
            _ => Err(OperationAuthorizationError::Unavailable),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_authorization_contracts::constraints::{
        AudienceRef, ExactRestriction, LifetimeRestriction, ResourceDomain,
    };
    use meerkat_authorization_contracts::evidence::{
        EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
    };
    use meerkat_authorization_contracts::grant::{GrantAuthorityIncarnation, GrantLineageRef};
    use meerkat_authorization_contracts::grant_mutation::{
        ControllerCustodyRefusal, ControllerGrantMutationCustody,
    };
    use meerkat_authorization_contracts::protocol::ContractRequirements;
    use meerkat_authorization_contracts::resource::ResourceRef;
    use meerkat_authorization_contracts::work_association::{
        GrantTarget, InputAuthorityAssociationCandidate, OriginalWorkRef,
        QualifiedIngressNamespace, TargetGrantLineage,
    };
    use meerkat_core::authorization::{
        AuthorizationOperation, OperationAuthorizationFacts, SourceAuthorizationFacts,
        SourceAuthorizationTarget, SourceAuthorizationUse,
    };
    use meerkat_core::exact_operation::OperationExecutionScope;
    use meerkat_core::mcp_attachment_control::McpAttachmentFacts;
    use meerkat_core::memory::MemorySearchScope;
    use meerkat_core::tool_catalog::{TOOL_CATALOG_LOAD_NAME, TOOL_CATALOG_SEARCH_NAME};
    use meerkat_core::{PrincipalKind, SessionId, TrustDomainId};
    use meerkat_runtime::Input;
    use meerkat_runtime::input::PromptInput;

    fn principal(kind: PrincipalKind, id: &str) -> PrincipalRef {
        PrincipalRef::in_domain(kind, id, TrustDomainId::new("stock-test").expect("domain"))
            .expect("qualified")
    }
    fn operator() -> PrincipalRef {
        principal(PrincipalKind::Human, "operator")
    }
    fn ingress() -> PrincipalRef {
        principal(PrincipalKind::RuntimeHost, "rkat-cli")
    }
    fn realm() -> RealmId {
        RealmId::parse("stock-realm").expect("realm")
    }
    fn id(value: &str) -> EvidenceId {
        EvidenceId::new(value).expect("id")
    }
    fn evidence(value: &str) -> HistoricalEvidenceRef {
        HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: ResourceDomain {
                    authority: ingress(),
                    namespace: "observations".into(),
                },
                resource_id: value.into(),
            },
            revision: id("observation-1"),
            digest: EvidenceDigest::from_array([7; 32]),
        }
    }
    fn lineage() -> GrantLineageRef {
        GrantLineageRef {
            root_authority: principal(PrincipalKind::ServiceAccount, "grant-owner"),
            authority_namespace: id("stock-grants"),
            authority_generation: 1,
            authority_incarnation: GrantAuthorityIncarnation::from_uuid(uuid::Uuid::new_v4())
                .expect("v4"),
            grant_id: id("root"),
            issued_revision: 1,
        }
    }

    fn executor() -> PrincipalRef {
        principal(PrincipalKind::PersonalAgent, "executor")
    }

    /// The realm's installations: exactly the listed descriptors.
    struct Installed(Vec<meerkat_core::McpServerConfig>);
    impl ConfiguredMcpAttachmentAuthority for Installed {
        fn authorize_configured(
            &self,
            server: &meerkat_core::McpServerConfig,
        ) -> Result<(), McpAttachmentRefusal> {
            if self.0.contains(server) {
                Ok(())
            } else {
                Err(McpAttachmentRefusal::Unavailable)
            }
        }
    }

    fn installed_server() -> meerkat_core::McpServerConfig {
        meerkat_core::McpServerConfig::stdio(
            "installed",
            "/bin/true",
            Vec::new(),
            std::collections::HashMap::new(),
        )
    }

    fn grant_owner() -> Arc<LocalGrantAuthority> {
        Arc::new(
            LocalGrantAuthority::new(
                meerkat_authorization::grants::LocalGrantConfiguration {
                    root: operator(),
                    namespace: id("stock-grants"),
                    generation: 1,
                },
                meerkat_authorization::publication::LocalAuthorizationPublication::new(),
                Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
            )
            .expect("grant owner"),
        )
    }

    fn principals() -> StockPrincipals {
        StockPrincipals {
            operator: operator(),
            ingress_actor: ingress(),
            executor: executor(),
        }
    }

    fn policy() -> Arc<StockInvocationPolicy> {
        policy_over(grant_owner())
    }

    fn policy_over(grants: Arc<LocalGrantAuthority>) -> Arc<StockInvocationPolicy> {
        StockInvocationPolicy::new(
            grants,
            &principals(),
            realm(),
            Arc::new(Installed(vec![installed_server()])),
        )
    }

    /// A root grant of the stock operator to the realm's executor, for
    /// itself, issued in `grants`.
    fn issue(
        grants: &LocalGrantAuthority,
        grant: &str,
        restrictions: ExecutionRestrictions,
    ) -> GrantLineageRef {
        grants
            .issue_root(&operator(), id(grant), executor(), None, restrictions)
            .expect("an issued stock grant")
    }

    /// The operator's own work at `session` under the issued `lineage`.
    fn granted(session: &SessionId, lineage: &GrantLineageRef) -> InputAuthorityAssociation {
        association(session, |c| {
            c.authority_basis = WorkAuthorityBasis::GrantLineage {
                lineage: vec![lineage.clone()],
            };
        })
    }

    /// No controller work holds any grant.
    struct NoController;
    impl ControllerGrantMutationCustody for NoController {
        fn with_unreferenced_controller_grant<T, E>(
            &mut self,
            _: &GrantLineageRef,
            mutate: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
            Ok(mutate())
        }
    }

    /// The stock policy, except that it states no review mandate for the
    /// input of one original work: that input's review material is not
    /// applicable (the operation's account is not applicable either).
    struct SilentFor(Arc<StockInvocationPolicy>, EvidenceId);
    impl AdmittedWorkPolicyOwner for SilentFor {
        fn authorize_admitted_work(
            &self,
            association: &InputAuthorityAssociation,
            binding: &PreparedAuthorizationBinding,
            purpose: LocalPolicyPurpose,
            now_ms: u64,
        ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
            self.0
                .authorize_admitted_work(association, binding, purpose, now_ms)
        }

        fn review_mandate(
            &self,
            association: &InputAuthorityAssociation,
            binding: &PreparedAuthorizationBinding,
            now_ms: u64,
        ) -> Result<Option<String>, OperationAuthorizationError> {
            if association.candidate().original_work.work == self.1 {
                return Ok(None);
            }
            self.0.review_mandate(association, binding, now_ms)
        }
    }

    /// A value as review text states it: in its serialized form.
    fn json<T: serde::Serialize + ?Sized>(value: &T) -> String {
        serde_json::to_string(value).expect("serializable")
    }

    /// The stock mandate of this policy's principals, with `bounds`.
    fn stock_mandate(bounds: &str) -> String {
        format!(
            "The local operator {} invoked the realm agent {} through the stock ingress {} in \
             realm stock-realm, for one of the operator's own sessions, under a grant-lineage \
             mandate. With the work's admitted ceiling it bounds this operation to: {bounds}",
            json(&operator()),
            json(&executor()),
            json(&ingress()),
        )
    }

    fn association(
        session: &SessionId,
        change: impl FnOnce(&mut InputAuthorityAssociationCandidate),
    ) -> InputAuthorityAssociation {
        let target = StockSessionDestination::for_session(&operator(), session, 1)
            .unwrap()
            .target()
            .clone();
        let mut candidate = InputAuthorityAssociationCandidate {
            requester: operator(),
            ingress_actor: ingress(),
            represented_subject: None,
            original_authentication: evidence("authentication"),
            logical_executor: executor(),
            target,
            original_work: OriginalWorkRef {
                authority: ingress(),
                work: id("work-1"),
            },
            root_event: evidence("event"),
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::GrantLineage {
                lineage: vec![lineage()],
            },
            controller_grant_lineage: vec![lineage()],
            controller_model: None,
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: QualifiedIngressNamespace {
                realm: realm(),
                ingress: ResourceDomain {
                    authority: ingress(),
                    namespace: "cli-prompts".into(),
                },
                occurrence_scope: id("occurrence-1"),
            },
            contract: ContractRequirements::local_governed_v1(),
        };
        change(&mut candidate);
        InputAuthorityAssociation::new(candidate).expect("bounded association")
    }

    fn ingress_context(requester: PrincipalRef) -> NativeIngressContext {
        let input = Input::Prompt(PromptInput::new("hello", None));
        NativeIngressContext::from_trusted_ingress(
            &input,
            requester,
            ingress(),
            realm(),
            evidence("authentication"),
        )
        .expect("trusted ingress observation")
    }

    fn operation_with(
        _session: &SessionId,
        operation: AuthorizationOperation,
    ) -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId(uuid::Uuid::nil()),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation,
        })
    }

    /// The work's own session memory read.
    fn operation(session: &SessionId) -> PreparedAuthorizationBinding {
        operation_with(
            session,
            AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                    session.clone(),
                )),
                usage: SourceAuthorizationUse::Read,
            }),
        )
    }

    fn is_denied_verdict(error: &OperationAuthorizationError) -> bool {
        matches!(error, OperationAuthorizationError::Refused(refusal)
            if refusal.kind() == OperationRefusalKind::Denied)
    }

    #[test]
    fn the_operators_own_invocation_is_admitted_and_its_work_allowed() {
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |_| {});
        policy
            .authorize_invocation(&ingress_context(operator()), &own)
            .expect("the operator invokes its own session");
        let allowance = policy
            .authorize_admitted_work(&own, &operation(&session), LocalPolicyPurpose::Operation, 5)
            .expect("its admitted work is allowed now");
        assert_eq!(allowance.expires_at_ms, 5 + DECISION_WINDOW_MS);
    }

    fn controller_selection() -> meerkat_core::ControllerModelSelection {
        meerkat_core::ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: "controller-model".into(),
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(serde_json::json!({"realm": "governed", "account": "operator"}))
                .expect("credential owner"),
            "profile".into(),
            "anthropic".into(),
        )
    }

    fn controller_request(endpoint: &str, wire_model: &str) -> ModelAuthorizationFacts {
        let selection = controller_selection();
        ModelAuthorizationFacts {
            identity: Arc::new(meerkat_core::SessionLlmIdentity {
                model: selection.model().into(),
                provider: selection.provider(),
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            }),
            wire_model: Arc::from(wire_model),
            hosted_capabilities: Arc::from([]),
            backend_profile_id: Some(Arc::from(selection.backend_profile_id())),
            backend_kind: Arc::from(selection.backend_kind()),
            endpoint: Arc::from(endpoint),
            credential: Some(selection.credential().clone()),
            usage: ModelAuthorizationUse::ControllerInference,
            live_channel: None,
        }
    }

    /// Admission and every controller request name the actual lowered route
    /// by its key: a grant for that key covers them, and a request whose
    /// endpoint or wire model differs names another route.
    #[test]
    fn controller_admission_and_requests_name_the_actual_route() {
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |c| {
            c.controller_model = Some(controller_selection());
        });
        let facts = ControllerModelFacts::new(
            controller_selection(),
            Arc::from("https://models.invalid/v1/messages"),
            Arc::from("controller-wire-model"),
        );
        let admitted = policy
            .authorize_controller_admission(&own, &facts, 0)
            .expect("the admitted route");
        let route = facts.route_key();
        assert_eq!(
            admitted.operation_values[0].resource_domain.namespace,
            route.as_str()
        );
        let namespace_of = |request: ModelAuthorizationFacts| {
            policy
                .authorize_operation(
                    &own,
                    &operation_with(&session, AuthorizationOperation::Model(request)),
                    LocalPolicyPurpose::Controller,
                    0,
                )
                .map(|allowance| {
                    allowance.operation_values[0]
                        .resource_domain
                        .namespace
                        .clone()
                })
        };
        assert_eq!(
            namespace_of(controller_request(
                "https://models.invalid/v1/messages",
                "controller-wire-model"
            ))
            .expect("the same route"),
            route.as_str()
        );
        for (endpoint, wire_model) in [
            ("https://other.invalid/v1/messages", "controller-wire-model"),
            ("https://models.invalid/v1/messages", "other-wire-model"),
        ] {
            assert_ne!(
                namespace_of(controller_request(endpoint, wire_model)).expect("a route"),
                route.as_str(),
                "{endpoint} {wire_model}"
            );
        }
    }

    #[test]
    fn another_requester_is_refused_at_admission_as_a_verdict() {
        let session = SessionId::new();
        let policy = policy();
        let stranger = principal(PrincipalKind::Human, "stranger");
        let claimed = association(&session, |c| c.requester = stranger.clone());
        let refused = policy
            .authorize_invocation(&ingress_context(stranger), &claimed)
            .expect_err("another requester is refused");
        assert!(matches!(refused, NativeAdmissionError::Refused(_)));
        // The operator's ingress cannot carry another requester's claim either.
        assert!(matches!(
            policy.authorize_invocation(&ingress_context(operator()), &claimed),
            Err(NativeAdmissionError::Refused(_))
        ));
    }

    #[test]
    fn a_refused_operation_is_classified_as_a_local_verdict_not_unavailability() {
        // Error classification only (no model, run or turn here): a policy
        // refusal is a Denied verdict, which the native owner keeps as the
        // operation's own typed result, never as readiness.
        let session = SessionId::new();
        let policy = policy();
        for (label, change) in [
            (
                "a represented subject",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.represented_subject = Some(principal(PrincipalKind::Human, "subject"));
                }) as Box<dyn FnOnce(&mut InputAuthorityAssociationCandidate)>,
            ),
            (
                "another executor",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.logical_executor = principal(PrincipalKind::PersonalAgent, "other-agent");
                }),
            ),
            (
                "another session's owner",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.target.logical_owner = principal(PrincipalKind::Human, "stranger");
                }),
            ),
            (
                "a host-policy mandate",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.authority_basis = WorkAuthorityBasis::HostPolicy {
                        policy: evidence("policy"),
                    };
                }),
            ),
            (
                "another realm",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.ingress_namespace.realm = RealmId::parse("other-realm").expect("realm");
                }),
            ),
        ] {
            let refused = policy
                .authorize_admitted_work(
                    &association(&session, change),
                    &operation(&session),
                    LocalPolicyPurpose::Operation,
                    5,
                )
                .expect_err(label);
            assert!(is_denied_verdict(&refused), "{label} is a Denied verdict");
            assert!(
                matches!(
                    NativeAdmissionError::from(refused),
                    NativeAdmissionError::Refused(_)
                ),
                "{label} stays a verdict, never readiness"
            );
        }
    }

    fn tool_binding(
        _session: &SessionId,
        name: &str,
        target: Option<ToolOperationTarget>,
    ) -> PreparedAuthorizationBinding {
        let entry = meerkat_core::ToolCatalogEntry::session_inline(
            Arc::new(meerkat_core::ToolDef {
                name: name.into(),
                description: String::new(),
                input_schema: serde_json::json!({"type": "object"}),
                provenance: None,
                audience: Default::default(),
            }),
            true,
        );
        let mut witness = meerkat_core::ToolExecutionOwnerWitness::new(
            "execution-owner",
            name,
            meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&entry),
        )
        .expect("owner witness");
        if let Some(target) = target {
            witness = witness.with_operation_target(target);
        }
        let plan = meerkat_core::ToolExecutionContract::default()
            .resolve_default(deadlines())
            .expect("plan")
            .with_owner_witness(witness)
            .expect("witness");
        plan_binding(name, plan)
    }

    fn deadlines() -> meerkat_core::ToolDeadlineChain {
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_secs(600),
        )])
        .expect("deadline chain")
    }

    fn plan_binding(
        name: &str,
        plan: meerkat_core::ResolvedToolExecutionPlan,
    ) -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId(uuid::Uuid::nil()),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
                call_id: Arc::from("call-1"),
                name: name.into(),
                arguments: serde_json::value::RawValue::from_string("{}".to_string())
                    .expect("raw arguments")
                    .into(),
                target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
            }),
        })
    }

    /// A binding for a plan resolved by the real catalog control dispatcher.
    fn catalog_binding(name: &str) -> PreparedAuthorizationBinding {
        plan_binding(name, catalog_plan(name))
    }

    fn catalog_plan(name: &str) -> meerkat_core::ResolvedToolExecutionPlan {
        let control = meerkat_tools::CatalogControlDispatcher::new(
            Arc::new(meerkat_tools::EmptyToolDispatcher),
            Arc::new(meerkat_tools::CatalogControlVisibilityProvider::new()),
        );
        let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw args");
        meerkat_core::AgentToolDispatcher::resolve_execution_plan(
            &control,
            meerkat_core::types::ToolCallView {
                id: "call-1",
                name,
                args: args.as_ref(),
            },
            &meerkat_core::ToolDispatchContext::default(),
            &meerkat_core::ToolExecutionResolutionContext::new(deadlines()),
        )
        .expect("catalog plan")
    }

    #[test]
    fn a_catalog_name_without_the_catalog_owner_target_is_unmapped() {
        // A host dispatcher may declare a tool named like a catalog tool; its
        // plan carries no catalog target, so it is not a catalog read.
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |_| {});
        for name in [TOOL_CATALOG_SEARCH_NAME, TOOL_CATALOG_LOAD_NAME] {
            assert!(
                matches!(
                    policy.authorize_operation(
                        &own,
                        &tool_binding(&session, name, None),
                        LocalPolicyPurpose::Operation,
                        5,
                    ),
                    Err(OperationAuthorizationError::Unavailable)
                ),
                "{name}"
            );
            // A genuine catalog target alongside another owner's target is
            // ambiguous: unmapped.
            let ambiguous = catalog_plan(name)
                .with_owner_witness(
                    meerkat_core::ToolExecutionOwnerWitness::new(
                        "execution-owner",
                        name,
                        meerkat_core::ephemeral_tool_catalog_binding_fingerprint(
                            &meerkat_core::ToolCatalogEntry::session_inline(
                                Arc::new(meerkat_core::ToolDef {
                                    name: name.into(),
                                    description: String::new(),
                                    input_schema: serde_json::json!({"type": "object"}),
                                    provenance: None,
                                    audience: Default::default(),
                                }),
                                true,
                            ),
                        ),
                    )
                    .expect("owner witness")
                    .with_operation_target(mcp("alpha", "read").expect("target")),
                )
                .expect("second witness");
            assert!(
                matches!(
                    policy.authorize_operation(
                        &own,
                        &plan_binding(name, ambiguous),
                        LocalPolicyPurpose::Operation,
                        5,
                    ),
                    Err(OperationAuthorizationError::Unavailable)
                ),
                "ambiguous {name}"
            );
        }
    }

    fn mcp(server: &str, raw: &str) -> Option<ToolOperationTarget> {
        mcp_bound(server, raw, meerkat_core::McpConnectorApproval::NoConnector)
    }

    fn mcp_bound(
        server: &str,
        raw: &str,
        connector: meerkat_core::McpConnectorApproval,
    ) -> Option<ToolOperationTarget> {
        mcp_stating(server, raw, connector, OperationAccount::Unavailable)
    }

    /// The target of `server`'s `raw` operation on a connection whose
    /// owners state `account`.
    fn mcp_stating(
        server: &str,
        raw: &str,
        connector: meerkat_core::McpConnectorApproval,
        account: OperationAccount,
    ) -> Option<ToolOperationTarget> {
        Some(ToolOperationTarget::McpServerOperation {
            server: server.to_string(),
            connection_generation: 7,
            raw_operation: raw.to_string(),
            connector,
            account: Arc::new(account),
        })
    }

    fn allowed_tuples(
        policy: &StockInvocationPolicy,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
    ) -> Vec<(String, String, String)> {
        policy
            .authorize_operation(association, binding, LocalPolicyPurpose::Operation, 5)
            .expect("allowed")
            .operation_values
            .into_iter()
            .map(|values| {
                assert!(values.resource_domain.authority == operator());
                assert!(
                    values.processor
                        == ProcessorRef::Principal {
                            principal: executor(),
                        }
                );
                assert!(values.audience == association.candidate().target.audience);
                (
                    values.action.feature,
                    values.action.action,
                    values.resource_domain.namespace,
                )
            })
            .collect()
    }

    #[test]
    fn a_configured_server_is_admitted_only_as_a_narrowed_installation() {
        let policy = policy();
        let mut ceilinged = installed_server();
        ceilinged.access = Some(meerkat_core::mcp_config::McpServerAccess::allowing([
            "read",
        ]));
        assert_eq!(policy.authorize_configured(&ceilinged), Ok(()));
        // No ceiling: not a narrowed installation.
        assert_eq!(
            policy.authorize_configured(&installed_server()),
            Err(McpAttachmentRefusal::Denied)
        );
        // A malformed ceiling admits nothing.
        let mut empty = installed_server();
        empty.access = Some(meerkat_core::mcp_config::McpServerAccess::allowing(Vec::<
            String,
        >::new(
        )));
        assert_eq!(
            policy.authorize_configured(&empty),
            Err(McpAttachmentRefusal::Denied)
        );
    }

    #[test]
    fn two_mcp_tools_are_reached_through_the_catalog_under_their_own_registrations() {
        // The catalog pair is a catalog read; each loaded MCP tool is then its
        // own registration's raw operation. Two servers exposing the same raw
        // operation stay distinct, and loading grants no invocation by itself.
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |_| {});
        for name in [TOOL_CATALOG_SEARCH_NAME, TOOL_CATALOG_LOAD_NAME] {
            assert_eq!(
                allowed_tuples(&policy, &own, &catalog_binding(name)),
                vec![(
                    "stock".to_string(),
                    "catalog-read".to_string(),
                    "tool-catalog".to_string()
                )],
                "{name}"
            );
        }
        let first = allowed_tuples(
            &policy,
            &own,
            &tool_binding(&session, "alpha_read", mcp("alpha", "read")),
        );
        let second = allowed_tuples(
            &policy,
            &own,
            &tool_binding(&session, "beta_read", mcp("beta", "read")),
        );
        assert_eq!(
            first,
            vec![
                (
                    "mcp".to_string(),
                    "read".to_string(),
                    "mcp-server/alpha".to_string()
                ),
                (
                    "mcp_connector".to_string(),
                    "alpha/none".to_string(),
                    "mcp-server/alpha".to_string()
                )
            ]
        );
        assert_eq!(
            second,
            vec![
                (
                    "mcp".to_string(),
                    "read".to_string(),
                    "mcp-server/beta".to_string()
                ),
                (
                    "mcp_connector".to_string(),
                    "beta/none".to_string(),
                    "mcp-server/beta".to_string()
                )
            ]
        );
        // A loaded tool's invocation is its own owner's operation: the
        // catalog read confers nothing on a tool without that target.
        assert_eq!(
            policy
                .authorize_operation(
                    &own,
                    &tool_binding(&session, "alpha_read", None),
                    LocalPolicyPurpose::Operation,
                    5,
                )
                .expect_err("no owner target"),
            OperationAuthorizationError::Unavailable
        );
    }

    /// The operation carries the connector binding of the connection it
    /// actually runs on, so the work's grant must have approved exactly that
    /// binding: an original whose grant approved account A covers none of
    /// the operations of a same-name server now bound to account B.
    #[test]
    fn an_mcp_operation_requires_the_actual_connections_connector_approval() {
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |_| {});
        let approval = |account: &str| {
            meerkat_core::McpConnectorApproval::of(Some(
                &meerkat_core::mcp_config::McpConnectorBinding {
                    realm_id: "alpha".into(),
                    slot_id: "records".into(),
                    issuer: "https://issuer.example".into(),
                    client: "client".into(),
                    resource: "https://api.example".into(),
                    strategy_id: "oidc".into(),
                    account: account.into(),
                    operations: std::collections::BTreeMap::new(),
                },
            ))
        };
        let on = |account: &str| {
            allowed_tuples(
                &policy,
                &own,
                &tool_binding(
                    &session,
                    "alpha_read",
                    mcp_bound("alpha", "read", approval(account)),
                ),
            )
        };
        let account_a = on("account-a");
        let account_b = on("account-b");
        assert_eq!(account_a[0], account_b[0], "the same raw operation");
        assert_eq!(
            account_a[1],
            (
                "mcp_connector".to_string(),
                format!("alpha/{}", approval("account-a").token()),
                "mcp-server/alpha".to_string()
            )
        );
        assert_ne!(account_a[1], account_b[1], "but another approved binding");
    }

    fn bundle(bundle: &str, operation: &str) -> Option<ToolOperationTarget> {
        bundle_stating(bundle, operation, OperationAccount::Unavailable)
    }

    /// The target of `bundle`'s `operation` under a registration that
    /// declared `account`.
    fn bundle_stating(
        bundle: &str,
        operation: &str,
        account: OperationAccount,
    ) -> Option<ToolOperationTarget> {
        Some(ToolOperationTarget::RegisteredToolBundleOperation {
            bundle: bundle.to_string(),
            operation: operation.to_string(),
            account: Arc::new(account),
        })
    }

    /// A registered bundle's operation maps to exactly its stable (bundle,
    /// operation) tuple, so a fresh plan of an intentional replacement under
    /// the same stable name maps identically and the retained grant covers
    /// it. A same-named tool without the owner target, a bundle target for
    /// another operation than the call's, and another bundle's operation
    /// never map to it.
    #[test]
    fn a_bundle_operation_maps_to_its_exact_stable_tuple() {
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |_| {});
        let read_mail = || {
            vec![(
                TOOL_BUNDLE_FEATURE.to_string(),
                "read_mail".to_string(),
                "tool-bundle/mail".to_string(),
            )]
        };
        assert_eq!(
            allowed_tuples(
                &policy,
                &own,
                &tool_binding(&session, "read_mail", bundle("mail", "read_mail")),
            ),
            read_mail()
        );
        assert_ne!(
            allowed_tuples(
                &policy,
                &own,
                &tool_binding(&session, "read_mail", bundle("other", "read_mail")),
            ),
            read_mail(),
            "another bundle's operation is its own tuple"
        );
        for (label, target) in [
            ("a same-named tool without the bundle target", None),
            (
                "a bundle target for another operation",
                bundle("mail", "send_draft"),
            ),
        ] {
            assert_eq!(
                policy
                    .authorize_operation(
                        &own,
                        &tool_binding(&session, "read_mail", target),
                        LocalPolicyPurpose::Operation,
                        5,
                    )
                    .expect_err(label),
                OperationAuthorizationError::Unavailable,
                "{label}"
            );
        }
    }

    /// A hosted-search delegate's model operation, minted the only way it
    /// can be: by a real governed dispatch of the builtin web_search call,
    /// here under a work that allows everything.
    async fn delegated_search_binding() -> PreparedAuthorizationBinding {
        struct Allowed;
        impl meerkat_core::authorization::PreparedOperationAuthorization for Allowed {
            fn review_tier(&self) -> OperationReviewTier {
                OperationReviewTier::R1
            }

            fn check_current(
                &self,
                _binding: &PreparedAuthorizationBinding,
            ) -> Result<(), OperationAuthorizationError> {
                Ok(())
            }
        }
        struct AllowAll;
        impl meerkat_core::authorization::WorkAuthorization for AllowAll {
            fn prepare(
                &self,
                _binding: &PreparedAuthorizationBinding,
            ) -> Result<
                Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
                OperationAuthorizationError,
            > {
                Ok(Arc::new(Allowed))
            }
        }
        let work = meerkat_core::WorkAuthorizationContext::new(
            Arc::new(AllowAll),
            OperationExecutionScope::Domain,
        );
        let attribution = meerkat_core::authorization::__test_mint_tool_delegate(
            &work,
            None,
            meerkat_core::authorization::ToolDelegateRole::HostedSearchInference,
        )
        .await
        .expect("the governed builtin call mints its delegate");
        let mut facts =
            controller_request("https://search.invalid/v1/messages", "search-wire-model");
        facts.hosted_capabilities = Arc::from([meerkat_core::ServerToolKind::WebSearch]);
        meerkat_core::LlmRequestAuthorization::for_tool_delegate(attribution)
            .prepare(facts)
            .expect("the delegate is prepared")
            .binding()
            .clone()
    }

    #[tokio::test]
    async fn unmapped_families_are_unavailable_and_rule_refusals_are_verdicts() {
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |_| {});
        let error_for = |binding: PreparedAuthorizationBinding, label: &str| {
            policy
                .authorize_operation(&own, &binding, LocalPolicyPurpose::Operation, 5)
                .expect_err(label)
        };
        let unmapped = |binding: PreparedAuthorizationBinding, label: &str| {
            assert_eq!(
                error_for(binding, label),
                OperationAuthorizationError::Unavailable,
                "{label} has no stock mapping: unavailable, not a verdict"
            );
        };
        let refused = |binding: PreparedAuthorizationBinding, label: &str| {
            assert!(
                is_denied_verdict(&error_for(binding, label)),
                "{label} is refused by a stock rule"
            );
        };
        unmapped(
            tool_binding(&session, "shell", None),
            "a builtin with no owner-produced target",
        );
        unmapped(
            tool_binding(
                &session,
                "web_search",
                Some(ToolOperationTarget::BuiltinOperation {
                    operation: meerkat_core::BuiltinToolOperation::WebSearch,
                }),
            ),
            "the builtin web_search operation",
        );
        let delegated = delegated_search_binding().await;
        assert!(delegated.delegate_attribution().is_some());
        unmapped(delegated, "a delegated hosted search");
        refused(
            operation_with(
                &session,
                AuthorizationOperation::Source(SourceAuthorizationFacts {
                    target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                        SessionId::new(),
                    )),
                    usage: SourceAuthorizationUse::Read,
                }),
            ),
            "another session's memory",
        );
        unmapped(
            operation_with(
                &session,
                AuthorizationOperation::Source(SourceAuthorizationFacts {
                    target: SourceAuthorizationTarget::Transcript {
                        session_id: session.clone(),
                        range: meerkat_core::memory::MessageRange::new(0, 1).expect("range"),
                    },
                    usage: SourceAuthorizationUse::Read,
                }),
            ),
            "a transcript range",
        );
        // The work's own session memory is mapped.
        assert_eq!(
            allowed_tuples(&policy, &own, &operation(&session)),
            vec![(
                "stock".to_string(),
                "memory-read".to_string(),
                "session-memory".to_string()
            )]
        );
        // Another requester's work is refused before any mapping.
        let stranger = association(&session, |c| {
            c.requester = principal(PrincipalKind::Human, "stranger");
        });
        let error = policy
            .authorize_operation(
                &stranger,
                &catalog_binding(TOOL_CATALOG_LOAD_NAME),
                LocalPolicyPurpose::Operation,
                5,
            )
            .expect_err("another requester");
        assert!(is_denied_verdict(&error));
    }

    /// Under a single grant lineage every operation resolves the work's one
    /// lineage, so the stock owner is what keeps peer and memory tool
    /// operations out of a stock realm: their owner-produced targets (each
    /// comms delivery kind, the peer directory, and the read of the work's
    /// own session memory through `memory_search`) have no stock mapping
    /// and are unavailable, never a stock tuple and never a verdict.
    #[test]
    fn peer_and_memory_tool_targets_are_unmapped_under_a_single_grant_lineage() {
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |_| {});
        assert!(matches!(
            own.candidate().authority_basis,
            WorkAuthorityBasis::GrantLineage { .. }
        ));
        let peer = meerkat_core::comms::PeerId::new();
        let delivery = |kind| ToolOperationTarget::PeerSend { peer, kind };
        for (name, target) in [
            (
                "send_message",
                delivery(meerkat_core::PeerSendKind::Message),
            ),
            (
                "reply_to_peer",
                delivery(meerkat_core::PeerSendKind::Message),
            ),
            (
                "send_request",
                delivery(meerkat_core::PeerSendKind::Request),
            ),
            (
                "send_response",
                delivery(meerkat_core::PeerSendKind::Response),
            ),
            ("peers", ToolOperationTarget::PeerDirectoryRead),
            (
                "memory_search",
                ToolOperationTarget::MemoryOperation {
                    scope: MemorySearchScope::for_session(session.clone()).scope_ref(),
                    operation: meerkat_core::MemoryOperation::Read,
                },
            ),
        ] {
            assert_eq!(
                policy
                    .authorize_operation(
                        &own,
                        &tool_binding(&session, name, Some(target)),
                        LocalPolicyPurpose::Operation,
                        5,
                    )
                    .expect_err(name),
                OperationAuthorizationError::Unavailable,
                "{name} has no stock mapping"
            );
        }
    }

    /// The stock mandate is the checked invocation itself, named by this
    /// policy's own principals and never by a lineage reference, whatever
    /// the operation; work the stock rule does not admit is refused as at
    /// authorization.
    #[test]
    fn the_review_mandate_is_the_checked_stock_invocation() {
        let session = SessionId::new();
        let grants = grant_owner();
        let policy = policy_over(Arc::clone(&grants));
        let own = granted(
            &session,
            &issue(
                &grants,
                "stock-grant",
                ExecutionRestrictions::unrestricted(),
            ),
        );
        assert_eq!(
            json(&operator()),
            r#"{"kind":"human","id":"operator","qualification":{"kind":"qualified","trust_domain_id":"stock-test"}}"#,
            "a principal is stated completely: kind, id and qualification"
        );
        let expected = stock_mandate(
            "actions any; resources any; processors any; audiences any; effective until no \
             expiry.",
        );
        for (label, binding) in [
            ("the work's own session memory", operation(&session)),
            (
                "an MCP operation",
                tool_binding(&session, "alpha_read", mcp("alpha", "read")),
            ),
        ] {
            assert_eq!(
                policy.review_mandate(&own, &binding, 5),
                Ok(Some(expected.clone())),
                "{label}"
            );
        }
        for (label, change) in [
            (
                "another requester",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.requester = principal(PrincipalKind::Human, "stranger");
                }) as Box<dyn FnOnce(&mut InputAuthorityAssociationCandidate)>,
            ),
            (
                "a represented subject",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.represented_subject = Some(principal(PrincipalKind::Human, "subject"));
                }),
            ),
            (
                "a host-policy mandate",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.authority_basis = WorkAuthorityBasis::HostPolicy {
                        policy: evidence("policy"),
                    };
                }),
            ),
            (
                "a service mandate",
                Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                    c.authority_basis = WorkAuthorityBasis::ServiceMandate {
                        mandate: evidence("mandate"),
                        commissioning_actor: operator(),
                        occurrence: id("occurrence-2"),
                    };
                }),
            ),
        ] {
            let refused = policy
                .review_mandate(&association(&session, change), &operation(&session), 5)
                .expect_err(label);
            assert!(is_denied_verdict(&refused), "{label} is refused");
        }
    }

    /// The mandate states the bounds of the work's own grant for the exact
    /// operation, resolved now, conjoined with the work's admitted ceiling,
    /// and the grant's own expiry, never this policy's decision window: two
    /// works under different grants, both allowed the same operation now,
    /// are presented with their different bounds; a targeted selection
    /// presents the lineage of the operation's own target; a revoked grant,
    /// and a target without a lineage, are refused as at authorization,
    /// and only for the operations they select.
    #[test]
    fn the_review_mandate_states_the_current_bounds_of_the_works_own_grant() {
        use meerkat_authorization::policy::LocalWorkPolicy;
        // 2100-01-01T00:00:00Z and 2099-01-01T00:00:00Z.
        const GRANT_UNTIL_MS: u64 = 4_102_444_800_000;
        const CEILING_UNTIL_MS: u64 = 4_070_908_800_000;
        let session = SessionId::new();
        let grants = grant_owner();
        let policy = policy_over(Arc::clone(&grants));
        let installed = Arc::new(
            meerkat_authorization::grant_policy::GrantBackedWorkPolicy::new(
                Arc::clone(&grants),
                Arc::clone(&policy) as Arc<dyn AdmittedWorkPolicyOwner>,
                Arc::clone(&policy) as Arc<dyn OperationPolicyOwner>,
            ),
        );
        let memory_read = ActionRef {
            feature: STOCK_FEATURE.into(),
            action: "memory-read".into(),
        };
        let session_memory = ResourceDomain {
            authority: operator(),
            namespace: "session-memory".into(),
        };
        let by_executor = ProcessorRef::Principal {
            principal: executor(),
        };
        let open_lineage = issue(&grants, "open-grant", ExecutionRestrictions::unrestricted());
        let narrow_lineage = issue(
            &grants,
            "narrow-grant",
            ExecutionRestrictions {
                actions: ExactRestriction::exact([memory_read.clone()]),
                resource_domains: ExactRestriction::exact([session_memory.clone()]),
                processors: ExactRestriction::exact([by_executor.clone()]),
                lifetime: LifetimeRestriction::window(0, GRANT_UNTIL_MS),
                ..ExecutionRestrictions::unrestricted()
            },
        );
        let open = granted(&session, &open_lineage);
        let narrow = granted(&session, &narrow_lineage);
        let read = operation(&session);
        for (label, work) in [("the open grant", &open), ("the narrow grant", &narrow)] {
            installed
                .evaluate_for(work, &read, LocalPolicyPurpose::Operation, 5)
                .expect(label);
        }
        let open_text = stock_mandate(
            "actions any; resources any; processors any; audiences any; effective until no \
             expiry.",
        );
        let narrow_bounds = |audiences: &str, until: &str| {
            format!(
                "actions {}; resources {}; processors {}; audiences {audiences}; effective until \
                 {until}.",
                json(std::slice::from_ref(&memory_read)),
                json(std::slice::from_ref(&session_memory)),
                json(std::slice::from_ref(&by_executor)),
            )
        };
        let narrow_text = stock_mandate(&narrow_bounds("any", "2100-01-01T00:00:00.000Z"));
        assert_eq!(
            policy.review_mandate(&open, &read, 5),
            Ok(Some(open_text.clone()))
        );
        assert_eq!(
            policy.review_mandate(&narrow, &read, 5),
            Ok(Some(narrow_text.clone()))
        );
        // The decision window bounds this policy's own answer, not the grant.
        let window = policy
            .authorize_admitted_work(&open, &read, LocalPolicyPurpose::Operation, 5)
            .expect("the decision");
        assert_eq!(window.expires_at_ms, 5 + DECISION_WINDOW_MS);
        for text in [&open_text, &narrow_text] {
            assert!(!text.contains("1970-01-01T00:01:00.005Z"), "{text}");
            assert!(!text.contains("open-grant") && !text.contains("narrow-grant"));
        }

        // The admitted ceiling narrows the grant's bounds and expiry.
        let destination = AudienceRef::Destination {
            authority: operator(),
            destination_id: session.to_string(),
        };
        let ceilinged = association(&session, |c| {
            c.authority_basis = WorkAuthorityBasis::GrantLineage {
                lineage: vec![narrow_lineage.clone()],
            };
            c.admitted_ceiling.audiences = ExactRestriction::exact([destination.clone()]);
            c.admitted_ceiling.lifetime = LifetimeRestriction::window(0, CEILING_UNTIL_MS);
        });
        assert_eq!(
            policy.review_mandate(&ceilinged, &read, 5),
            Ok(Some(stock_mandate(&narrow_bounds(
                &json(&[destination]),
                "2099-01-01T00:00:00.000Z"
            ))))
        );

        // A targeted selection: the base lineage for the session memory, the
        // MCP server's own lineage for its operation, none for another server.
        let targeted = association(&session, |c| {
            c.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
                base: Some(vec![narrow_lineage.clone()]),
                targets: vec![TargetGrantLineage {
                    target: GrantTarget::McpServer {
                        server: "alpha".into(),
                    },
                    lineage: vec![open_lineage.clone()],
                }],
            };
        });
        let alpha = tool_binding(&session, "alpha_read", mcp("alpha", "read"));
        let beta = tool_binding(&session, "beta_read", mcp("beta", "read"));
        assert_eq!(
            policy.review_mandate(&targeted, &read, 5),
            Ok(Some(narrow_text.clone()))
        );
        assert_eq!(
            policy.review_mandate(&targeted, &alpha, 5),
            Ok(Some(open_text.clone()))
        );
        assert!(is_denied_verdict(
            &policy
                .review_mandate(&targeted, &beta, 5)
                .expect_err("a target without a lineage")
        ));

        // A batch of compatible inputs, each under its own grant: every
        // input's own mandate, associated with its position and original
        // work, under their conjunction; inputs that agree are stated once,
        // and material over the field bound is unavailable, never cut.
        let input = |work: &str, lineage: &GrantLineageRef| {
            association(&session, |c| {
                c.original_work.work = id(work);
                c.authority_basis = WorkAuthorityBasis::GrantLineage {
                    lineage: vec![lineage.clone()],
                };
            })
        };
        let batch = |inputs: Vec<InputAuthorityAssociation>| {
            installed
                .work_context(inputs.into(), OperationExecutionScope::Domain)
                .expect("compatible inputs")
                .authorization()
                .review_operation_context(&read)
        };
        let material = |mandate: String| {
            Ok(
                meerkat_core::authorization::ReviewOperationContextResolution::Material(
                    meerkat_core::authorization::ReviewOperationContext::new(None, Some(mandate))
                        .expect("bounded material"),
                ),
            )
        };
        let original = |work: &str| {
            json(&OriginalWorkRef {
                authority: ingress(),
                work: id(work),
            })
        };
        assert_eq!(
            batch(vec![
                input("work-open", &open_lineage),
                input("work-narrow", &narrow_lineage),
            ]),
            material(format!(
                "The 2 inputs of this work hold their own mandates, and the operation must be \
                 within every one of them:\nInput 1 (original work {}): {open_text}\nInput 2 \
                 (original work {}): {narrow_text}",
                original("work-open"),
                original("work-narrow"),
            )),
            "every input's own mandate, never the first input's alone"
        );
        assert_eq!(
            batch(vec![
                input("work-a", &narrow_lineage),
                input("work-b", &narrow_lineage),
            ]),
            material(narrow_text),
            "inputs that agree are stated once"
        );
        // An input whose owner states no material (its review material is
        // not applicable) is still listed, as none, beside one that has
        // material, in either order; only when no input has any is the
        // work's material not applicable.
        let partly_silent = Arc::new(
            meerkat_authorization::grant_policy::GrantBackedWorkPolicy::new(
                Arc::clone(&grants),
                Arc::new(SilentFor(Arc::clone(&policy), id("work-silent")))
                    as Arc<dyn AdmittedWorkPolicyOwner>,
                Arc::clone(&policy) as Arc<dyn OperationPolicyOwner>,
            ),
        );
        let silent_batch = |inputs: Vec<InputAuthorityAssociation>| {
            partly_silent
                .work_context(inputs.into(), OperationExecutionScope::Domain)
                .expect("compatible inputs")
                .authorization()
                .review_operation_context(&read)
        };
        for (label, inputs, stated) in [
            (
                "material, then none",
                vec![
                    input("work-open", &open_lineage),
                    input("work-silent", &narrow_lineage),
                ],
                [
                    ("work-open", open_text.clone()),
                    ("work-silent", "none".to_owned()),
                ],
            ),
            (
                "none, then material",
                vec![
                    input("work-silent", &narrow_lineage),
                    input("work-open", &open_lineage),
                ],
                [
                    ("work-silent", "none".to_owned()),
                    ("work-open", open_text.clone()),
                ],
            ),
        ] {
            assert_eq!(
                silent_batch(inputs),
                material(format!(
                    "The 2 inputs of this work hold their own mandates, and the operation must be \
                     within every one of them:\nInput 1 (original work {}): {}\nInput 2 \
                     (original work {}): {}",
                    original(stated[0].0),
                    stated[0].1,
                    original(stated[1].0),
                    stated[1].1,
                )),
                "{label}"
            );
        }
        assert_eq!(
            silent_batch(vec![
                input("work-silent", &narrow_lineage),
                input("work-silent", &open_lineage),
            ]),
            Ok(meerkat_core::authorization::ReviewOperationContextResolution::NotApplicable),
            "no input has material"
        );
        let many: Vec<_> = (0..8_u64)
            .map(|n| {
                let lineage = issue(
                    &grants,
                    &format!("windowed-{n}"),
                    ExecutionRestrictions {
                        lifetime: LifetimeRestriction::window(0, GRANT_UNTIL_MS - n),
                        ..ExecutionRestrictions::unrestricted()
                    },
                );
                input(&format!("work-{n}"), &lineage)
            })
            .collect();
        assert_eq!(
            batch(many),
            Err(OperationAuthorizationError::Unavailable),
            "material over the field bound is unavailable"
        );

        // Revoking the narrow grant refuses exactly the operations it selects.
        grants
            .revoke(&operator(), &narrow_lineage, &mut NoController)
            .expect("revoked");
        for (label, work, binding) in [
            ("its own work", &narrow, &read),
            ("the targeted base", &targeted, &read),
        ] {
            assert!(
                is_denied_verdict(&policy.review_mandate(work, binding, 5).expect_err(label)),
                "{label}"
            );
        }
        assert_eq!(
            policy.review_mandate(&targeted, &alpha, 5),
            Ok(Some(open_text.clone()))
        );
        assert_eq!(policy.review_mandate(&open, &read, 5), Ok(Some(open_text)));
    }

    /// The action account is the operation owner's own statement. It is
    /// explicitly not applicable where stock selects none (the catalog read
    /// and the work's own session memory) and where the MCP server's owner
    /// configuration or the bundle's registration declares that the
    /// operation acts with none; it is the selected account's text where the
    /// owner selected one. An MCP or bundle operation whose owner states
    /// neither (with or without a connector binding: `NoConnector` is no
    /// such statement), a tool without an owner target, a transcript and a
    /// controller request are unavailable; another session's memory, work
    /// the stock rule does not admit and a bundle target for another
    /// operation than the call's are refused or unavailable.
    #[test]
    fn the_action_account_is_the_operation_owners_statement() {
        let session = SessionId::new();
        let policy = policy();
        let own = association(&session, |_| {});
        let account =
            |binding: &PreparedAuthorizationBinding| policy.review_action_account(&own, binding, 5);
        for name in [TOOL_CATALOG_SEARCH_NAME, TOOL_CATALOG_LOAD_NAME] {
            assert_eq!(account(&catalog_binding(name)), Ok(None), "{name}");
        }
        assert_eq!(
            account(&operation(&session)),
            Ok(None),
            "the work's own session memory"
        );
        let selected = || OperationAccount::selected("https://issuer.example", "account-a");
        for (label, binding) in [
            (
                "an MCP server declared to act with no account",
                tool_binding(
                    &session,
                    "alpha_read",
                    mcp_stating(
                        "alpha",
                        "read",
                        meerkat_core::McpConnectorApproval::NoConnector,
                        OperationAccount::DeclaredNone,
                    ),
                ),
            ),
            (
                "a bundle registration declaring no account",
                tool_binding(
                    &session,
                    "read_mail",
                    bundle_stating("mail", "read_mail", OperationAccount::DeclaredNone),
                ),
            ),
        ] {
            assert_eq!(account(&binding), Ok(None), "{label}");
        }
        for (label, binding) in [
            (
                "an MCP connection's selected account",
                tool_binding(
                    &session,
                    "alpha_read",
                    mcp_stating(
                        "alpha",
                        "read",
                        meerkat_core::McpConnectorApproval::NoConnector,
                        selected(),
                    ),
                ),
            ),
            (
                "a bundle registration's selected account",
                tool_binding(
                    &session,
                    "read_mail",
                    bundle_stating("mail", "read_mail", selected()),
                ),
            ),
        ] {
            assert_eq!(
                account(&binding),
                Ok(Some("account-a at https://issuer.example".to_string())),
                "{label}"
            );
        }
        assert_eq!(
            account(&tool_binding(
                &session,
                "send_draft",
                bundle_stating("mail", "read_mail", OperationAccount::DeclaredNone),
            )),
            Err(OperationAuthorizationError::Unavailable),
            "a bundle target for another operation than the call's"
        );
        let connector = meerkat_core::McpConnectorApproval::of(Some(
            &meerkat_core::mcp_config::McpConnectorBinding {
                realm_id: "alpha".into(),
                slot_id: "records".into(),
                issuer: "https://issuer.example".into(),
                client: "client".into(),
                resource: "https://api.example".into(),
                strategy_id: "oidc".into(),
                account: "account-a".into(),
                operations: std::collections::BTreeMap::new(),
            },
        ));
        for (label, binding) in [
            (
                "an undeclared MCP operation without a connector",
                tool_binding(&session, "alpha_read", mcp("alpha", "read")),
            ),
            (
                "an MCP operation with a connector whose connection states no account",
                tool_binding(
                    &session,
                    "alpha_read",
                    mcp_bound("alpha", "read", connector),
                ),
            ),
            (
                "a registered bundle operation whose registration declared none",
                tool_binding(&session, "read_mail", bundle("mail", "read_mail")),
            ),
            (
                "a tool without an owner target",
                tool_binding(&session, "read_mail", None),
            ),
            (
                "a transcript range",
                operation_with(
                    &session,
                    AuthorizationOperation::Source(SourceAuthorizationFacts {
                        target: SourceAuthorizationTarget::Transcript {
                            session_id: session.clone(),
                            range: meerkat_core::memory::MessageRange::new(0, 1).expect("range"),
                        },
                        usage: SourceAuthorizationUse::Read,
                    }),
                ),
            ),
            (
                "a controller request carrying its credential owner",
                operation_with(
                    &session,
                    AuthorizationOperation::Model(controller_request(
                        "https://models.invalid/v1/messages",
                        "controller-wire-model",
                    )),
                ),
            ),
        ] {
            assert_eq!(
                account(&binding),
                Err(OperationAuthorizationError::Unavailable),
                "{label}"
            );
        }
        assert!(is_denied_verdict(
            &account(&operation(&SessionId::new())).expect_err("another session's memory")
        ));
        let stranger = association(&session, |c| {
            c.requester = principal(PrincipalKind::Human, "stranger");
        });
        assert!(is_denied_verdict(
            &policy
                .review_action_account(&stranger, &catalog_binding(TOOL_CATALOG_LOAD_NAME), 5)
                .expect_err("another requester")
        ));
    }

    /// The composition the realm installs answers review material through
    /// its scoped wrapper over this one policy: the stock mandate without an
    /// action account for the work's own session memory, catalog reads and
    /// operations whose owners declare none, with the selected account for
    /// one whose owner selected it, unavailable for MCP and bundle
    /// operations whose owners state nothing, and the stock rules'
    /// refusals. It asks the installed owners for review material directly,
    /// for an association built here: it shows that the installed
    /// composition supplies this policy's material, not that admitted work
    /// executes under the grant or that a configured R2 or R3 review runs.
    #[tokio::test]
    async fn the_installed_stock_composition_answers_review_material() {
        use meerkat_authorization::policy::LocalWorkPolicy;
        let realm_dir = tempfile::tempdir().expect("realm directory");
        let grants = grant_owner();
        let paths = meerkat_store::realm_paths_in(realm_dir.path(), "governed");
        let stock = super::super::StockGovernance::new(realm(), principals());
        // A composition whose realm's review rules were never read is
        // refused, never composed with none.
        assert!(matches!(
            stock
                .composition(
                    Arc::clone(&grants),
                    Arc::new(Installed(vec![installed_server()])),
                    &paths,
                )
                .await,
            Err(super::super::StockGovernanceError::Review(_))
        ));
        stock
            .configure_review(super::super::review::StockReviewRules::default())
            .expect("the realm states no review rules");
        let configuration = stock
            .composition(
                Arc::clone(&grants),
                Arc::new(Installed(vec![installed_server()])),
                &paths,
            )
            .await
            .expect("the stock composition")
            .into_configuration();
        let installed = meerkat_authorization::grant_policy::GrantBackedWorkPolicy::new(
            Arc::clone(&grants),
            Arc::clone(&configuration.invocation_owner),
            Arc::clone(&configuration.operation_owner),
        );
        let session = SessionId::new();
        let own = granted(
            &session,
            &issue(
                &grants,
                "stock-grant",
                ExecutionRestrictions::unrestricted(),
            ),
        );
        let mandate = policy_over(Arc::clone(&grants))
            .review_mandate(&own, &operation(&session), 5)
            .expect("the stock mandate");
        let material = meerkat_core::authorization::ReviewOperationContextResolution::Material(
            meerkat_core::authorization::ReviewOperationContext::new(None, mandate.clone())
                .expect("bounded material"),
        );
        for (label, binding) in [
            ("the work's own session memory", operation(&session)),
            ("a catalog read", catalog_binding(TOOL_CATALOG_LOAD_NAME)),
            (
                "an MCP operation whose server is declared to act with none",
                tool_binding(
                    &session,
                    "alpha_read",
                    mcp_stating(
                        "alpha",
                        "read",
                        meerkat_core::McpConnectorApproval::NoConnector,
                        OperationAccount::DeclaredNone,
                    ),
                ),
            ),
        ] {
            assert_eq!(
                installed.review_context(&own, &binding, 5),
                Ok(material.clone()),
                "{label}"
            );
        }
        assert_eq!(
            installed.review_context(
                &own,
                &tool_binding(
                    &session,
                    "read_mail",
                    bundle_stating(
                        "mail",
                        "read_mail",
                        OperationAccount::selected("https://mail.example", "alice"),
                    ),
                ),
                5,
            ),
            Ok(
                meerkat_core::authorization::ReviewOperationContextResolution::Material(
                    meerkat_core::authorization::ReviewOperationContext::new(
                        Some("alice at https://mail.example".to_string()),
                        mandate,
                    )
                    .expect("bounded material"),
                )
            ),
            "a bundle registration's selected account"
        );
        for (label, binding) in [
            (
                "an MCP operation",
                tool_binding(&session, "alpha_read", mcp("alpha", "read")),
            ),
            (
                "a registered bundle operation",
                tool_binding(&session, "read_mail", bundle("mail", "read_mail")),
            ),
        ] {
            assert_eq!(
                installed.review_context(&own, &binding, 5),
                Err(OperationAuthorizationError::Unavailable),
                "{label}"
            );
        }
        assert!(is_denied_verdict(
            &installed
                .review_context(&own, &operation(&SessionId::new()), 5)
                .expect_err("another session's memory")
        ));
        let host_policy = association(&session, |c| {
            c.authority_basis = WorkAuthorityBasis::HostPolicy {
                policy: evidence("policy"),
            };
        });
        assert!(is_denied_verdict(
            &installed
                .review_context(&host_policy, &operation(&session), 5)
                .expect_err("a host-policy mandate")
        ));
    }

    #[test]
    fn an_attachment_is_allowed_only_for_the_operators_own_session_destination() {
        let session = SessionId::new();
        let policy = policy();
        let destination = StockSessionDestination::for_session(&operator(), &session, 4).unwrap();
        let control_for =
            |session: SessionId, requester: PrincipalRef, server: meerkat_core::McpServerConfig| {
                McpAttachmentControlRequest::from_trusted_ingress(
                    session,
                    server,
                    McpAttachmentFacts {
                        requester,
                        actor: ingress(),
                        realm: realm(),
                        destination: destination.mcp_destination(),
                    },
                    Arc::new(()),
                )
                .expect("trusted control")
            };
        let control = |session: SessionId, requester: PrincipalRef| {
            control_for(session, requester, installed_server())
        };
        AdmittedWorkPolicyOwner::authorize_mcp_attachment(
            policy.as_ref(),
            &control(session.clone(), operator()),
            &operation(&session),
            1,
        )
        .expect("the operator attaches to its own session");
        let stranger = AdmittedWorkPolicyOwner::authorize_mcp_attachment(
            policy.as_ref(),
            &control(session.clone(), principal(PrincipalKind::Human, "stranger")),
            &operation(&session),
            1,
        )
        .expect_err("another requester");
        assert!(is_denied_verdict(&stranger));
        let elsewhere = AdmittedWorkPolicyOwner::authorize_mcp_attachment(
            policy.as_ref(),
            &control(SessionId::new(), operator()),
            &operation(&session),
            1,
        )
        .expect_err("a destination of another session");
        assert!(is_denied_verdict(&elsewhere));
        // A server that is not one of the realm's installations is never
        // attached, even for the operator's own session.
        let mut uninstalled = installed_server();
        uninstalled.name = "uninstalled".to_string();
        let refused = AdmittedWorkPolicyOwner::authorize_mcp_attachment(
            policy.as_ref(),
            &control_for(session.clone(), operator(), uninstalled),
            &operation(&session),
            1,
        )
        .expect_err("not an installation");
        assert_eq!(refused, OperationAuthorizationError::Unavailable);
    }

    /// The stock owner's two attachment faces are one decision: for the
    /// operator's own session, another requester, another session and an
    /// uninstalled server, the operation face allows exactly when the
    /// work-owner face does and refuses with the same verdict.
    #[test]
    fn the_attachment_faces_agree() {
        let session = SessionId::new();
        let policy = policy();
        let destination = StockSessionDestination::for_session(&operator(), &session, 4).unwrap();
        let control_for =
            |session: SessionId, requester: PrincipalRef, server: meerkat_core::McpServerConfig| {
                McpAttachmentControlRequest::from_trusted_ingress(
                    session,
                    server,
                    McpAttachmentFacts {
                        requester,
                        actor: ingress(),
                        realm: realm(),
                        destination: destination.mcp_destination(),
                    },
                    Arc::new(()),
                )
                .expect("trusted control")
            };
        let mut uninstalled = installed_server();
        uninstalled.name = "uninstalled".to_string();
        for (control, allowed) in [
            (
                control_for(session.clone(), operator(), installed_server()),
                true,
            ),
            (
                control_for(
                    session.clone(),
                    principal(PrincipalKind::Human, "stranger"),
                    installed_server(),
                ),
                false,
            ),
            (
                control_for(SessionId::new(), operator(), installed_server()),
                false,
            ),
            (control_for(session.clone(), operator(), uninstalled), false),
        ] {
            // The control's own effect operations: applying it (hydrate the
            // server, publish to the destination) and persisting it.
            let mut bindings: Vec<(&str, PreparedAuthorizationBinding)> = control
                .effect_operations(McpAttachmentEffect::Apply)
                .into_iter()
                .map(|operation| {
                    let kind = match &operation {
                        AuthorizationOperation::Source(_) => "hydrate",
                        _ => "publish",
                    };
                    (kind, control.binding_for(operation))
                })
                .collect();
            for operation in control
                .effect_operations(McpAttachmentEffect::Persist(meerkat_core::McpScope::User))
            {
                bindings.push(("persist", control.binding_for(operation)));
            }
            assert_eq!(bindings.len(), 3);
            for (kind, binding) in &bindings {
                let work = AdmittedWorkPolicyOwner::authorize_mcp_attachment(
                    policy.as_ref(),
                    &control,
                    binding,
                    1,
                );
                let operation_face = OperationPolicyOwner::authorize_mcp_attachment(
                    policy.as_ref(),
                    &control,
                    binding,
                    1,
                );
                assert_eq!(work.is_ok(), allowed, "{kind}");
                match (work, operation_face) {
                    (Ok(work), Ok(operation_face)) => {
                        assert_eq!(operation_face.expires_at_ms, work.expires_at_ms);
                        let [values] = operation_face.operation_values.as_slice() else {
                            panic!("{kind}: one value");
                        };
                        let (action, namespace) = match *kind {
                            "hydrate" => {
                                ("attach", format!("mcp-server/{}", installed_server().name))
                            }
                            "publish" => ("publish", format!("session/{session}")),
                            other => panic!("{other} must not be allowed"),
                        };
                        assert_eq!(values.action.feature, MCP_FEATURE, "{kind}");
                        assert_eq!(values.action.action, action, "{kind}");
                        assert_eq!(values.resource_domain.authority, operator(), "{kind}");
                        assert_eq!(values.resource_domain.namespace, namespace, "{kind}");
                        assert!(
                            matches!(&values.processor, ProcessorRef::Principal { principal }
                                if *principal == executor()),
                            "{kind}"
                        );
                        assert_eq!(
                            values.audience,
                            StockSessionDestination::for_session(&operator(), &session, 1)
                                .unwrap()
                                .target()
                                .audience,
                            "{kind}"
                        );
                    }
                    // Configuration retention is unmapped on the operation
                    // face, though the work face admits the control.
                    (Ok(_), Err(operation_face)) if *kind == "persist" => {
                        assert_eq!(operation_face, OperationAuthorizationError::Unavailable);
                    }
                    (Err(work), Err(operation_face)) => assert_eq!(operation_face, work, "{kind}"),
                    (work, operation_face) => panic!(
                        "{kind}: the faces disagree: {:?} vs {:?}",
                        work.map(|_| ()),
                        operation_face.map(|_| ())
                    ),
                }
            }
        }
    }

    /// A trusted host's exact-target decision: it records every question it
    /// is asked and answers with its current answer.
    struct HostAccess {
        answer: std::sync::Mutex<Result<(), OperationAuthorizationError>>,
        /// The lifetime of the login the host admits.
        login: std::sync::Mutex<StockLoginLifetime>,
        asked: std::sync::Mutex<Vec<(PrincipalRef, PrincipalRef, PrincipalRef, SessionId)>>,
        /// The login evidence of every question.
        authentications: std::sync::Mutex<Vec<Option<HistoricalEvidenceRef>>>,
    }

    impl HostAccess {
        fn answering(answer: Result<(), OperationAuthorizationError>) -> Arc<Self> {
            Arc::new(Self {
                answer: std::sync::Mutex::new(answer),
                login: std::sync::Mutex::new(StockLoginLifetime::UntilRetired),
                asked: std::sync::Mutex::new(Vec::new()),
                authentications: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn login(&self, login: StockLoginLifetime) {
            *self.login.lock().unwrap() = login;
        }

        fn authentications(&self) -> Vec<Option<HistoricalEvidenceRef>> {
            self.authentications.lock().unwrap().clone()
        }

        fn answer(&self, answer: Result<(), OperationAuthorizationError>) {
            *self.answer.lock().unwrap() = answer;
        }

        fn asked(&self) -> Vec<(PrincipalRef, PrincipalRef, PrincipalRef, SessionId)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl super::super::caller::StockCallerAccess for HostAccess {
        fn authorize_caller(
            &self,
            request: &StockCallerRequest<'_>,
        ) -> Result<StockLoginLifetime, OperationAuthorizationError> {
            self.asked.lock().unwrap().push((
                request.caller().clone(),
                request.executor().clone(),
                request.owner().clone(),
                request.session().clone(),
            ));
            self.authentications
                .lock()
                .unwrap()
                .push(request.authentication().cloned());
            let login = *self.login.lock().unwrap();
            (*self.answer.lock().unwrap()).map(|()| login)
        }
    }

    /// The host answers for the exact login the caller's input was
    /// authenticated in, and a decision for that login stands no longer than
    /// the login: the admitted work's allowance and every operation's are
    /// bounded by its deadline, and a login past its deadline admits nothing
    /// even while the host still answers for it.
    #[test]
    fn a_host_callers_decisions_stand_no_longer_than_its_login() {
        let session = SessionId::new();
        let access = HostAccess::answering(Ok(()));
        let grants = grant_owner();
        let policy = host_policy_over(Arc::clone(&grants), &access);
        let lineage = issue(
            &grants,
            "caller-grant",
            ExecutionRestrictions::unrestricted(),
        );
        let invoked = association(&session, |c| {
            c.requester = caller();
            c.authority_basis = WorkAuthorityBasis::GrantLineage {
                lineage: vec![lineage],
            };
        });
        let work = || {
            policy
                .authorize_admitted_work(
                    &invoked,
                    &operation(&session),
                    LocalPolicyPurpose::Operation,
                    5,
                )
                .map(|allowance| allowance.expires_at_ms)
        };
        let tool = || {
            policy
                .authorize_operation(
                    &invoked,
                    &tool_binding(&session, "alpha_read", mcp("alpha", "read")),
                    LocalPolicyPurpose::Operation,
                    5,
                )
                .map(|allowance| allowance.expires_at_ms)
        };
        // A login that outlives the decision window leaves the window.
        access.login(StockLoginLifetime::ExpiresAt {
            unix_ms: 5 + DECISION_WINDOW_MS + 1,
        });
        assert_eq!(work(), Ok(5 + DECISION_WINDOW_MS));
        assert_eq!(tool(), Ok(5 + DECISION_WINDOW_MS));
        // A login that ends inside the window ends the decisions with it.
        access.login(StockLoginLifetime::ExpiresAt { unix_ms: 1_000 });
        assert_eq!(work(), Ok(1_000));
        assert_eq!(tool(), Ok(1_000));
        // Past its deadline it admits nothing, whatever the host answers.
        access.login(StockLoginLifetime::ExpiresAt { unix_ms: 5 });
        assert_eq!(work(), Err(denied().into()));
        assert_eq!(tool(), Err(denied().into()));
        // Every question named the exact login evidence the work was
        // admitted with, never the principal alone.
        let authentications = access.authentications();
        assert_eq!(authentications.len(), 6);
        assert!(
            authentications
                .iter()
                .all(|authentication| authentication.as_ref()
                    == Some(&invoked.candidate().original_authentication))
        );
    }

    /// The owner's review rules raise the tier of exactly the operations
    /// they name, and only of operations the stock rules allow: a rule
    /// naming an operation the stock rules leave unmapped admits nothing.
    /// Without rules every mapped operation stays R1.
    #[test]
    fn review_rules_raise_only_the_tier_of_operations_the_stock_rules_allow() {
        let rule = |target, tier, consent_assurance| meerkat_core::StockReviewRule {
            target,
            tier,
            consent_assurance,
        };
        let rules =
            super::super::review::StockReviewRules::from_config(&meerkat_core::StockReviewConfig {
                rules: vec![
                    rule(
                        meerkat_core::StockReviewTarget::McpServer {
                            server: "alpha".into(),
                        },
                        meerkat_core::StockReviewTier::R2,
                        None,
                    ),
                    rule(
                        meerkat_core::StockReviewTarget::McpOperation {
                            server: "alpha".into(),
                            operation: "send".into(),
                        },
                        meerkat_core::StockReviewTier::R3,
                        Some(meerkat_core::StockConsentAssurance::PersonAuthenticated),
                    ),
                    rule(
                        meerkat_core::StockReviewTarget::ToolBundleOperation {
                            bundle: "files".into(),
                            operation: "delete".into(),
                        },
                        meerkat_core::StockReviewTier::R3,
                        Some(meerkat_core::StockConsentAssurance::DeviceTrusted),
                    ),
                ],
                reviewer: None,
            })
            .expect("valid rules");
        let reviewed = StockInvocationPolicy::with_review_rules(
            grant_owner(),
            &principals(),
            realm(),
            Arc::new(Installed(vec![installed_server()])),
            StockCallerPosture::LocalOperator,
            rules,
        );
        let session = SessionId::new();
        let own = association(&session, |_| {});
        let tier = |policy: &Arc<StockInvocationPolicy>, name: &str, target| {
            policy
                .authorize_operation(
                    &own,
                    &tool_binding(&session, name, target),
                    LocalPolicyPurpose::Operation,
                    5,
                )
                .map(|allowance| allowance.review_tier)
        };
        let delete = || {
            Some(ToolOperationTarget::RegisteredToolBundleOperation {
                bundle: "files".into(),
                operation: "delete".into(),
                account: Arc::new(OperationAccount::Unavailable),
            })
        };
        assert_eq!(
            tier(&reviewed, "alpha_read", mcp("alpha", "read")),
            Ok(OperationReviewTier::R2)
        );
        assert_eq!(
            tier(&reviewed, "alpha_send", mcp("alpha", "send")),
            Ok(OperationReviewTier::R3)
        );
        assert_eq!(
            tier(&reviewed, "beta_send", mcp("beta", "send")),
            Ok(OperationReviewTier::R1)
        );
        assert_eq!(
            tier(&reviewed, "delete", delete()),
            Ok(OperationReviewTier::R3)
        );
        // The bundle's witness names another operation than the call's own:
        // unmapped, and the rule naming that operation admits nothing.
        assert_eq!(
            tier(&reviewed, "rename", delete()),
            Err(OperationAuthorizationError::Unavailable)
        );
        assert_eq!(
            tier(&policy(), "alpha_send", mcp("alpha", "send")),
            Ok(OperationReviewTier::R1)
        );
    }

    /// A human the host authenticated in its own identity domain: never the
    /// operator, and never the owner of the sessions it invokes.
    fn caller() -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::Human,
            "alice",
            TrustDomainId::new("console-idp").expect("domain"),
        )
        .expect("qualified caller")
    }

    fn host_policy(access: &Arc<HostAccess>) -> Arc<StockInvocationPolicy> {
        host_policy_over(grant_owner(), access)
    }

    fn host_policy_over(
        grants: Arc<LocalGrantAuthority>,
        access: &Arc<HostAccess>,
    ) -> Arc<StockInvocationPolicy> {
        StockInvocationPolicy::with_caller_posture(
            grants,
            &principals(),
            realm(),
            Arc::new(Installed(vec![installed_server()])),
            StockCallerPosture::HostAuthenticated(Arc::clone(access) as _),
        )
    }

    /// In the host-authenticated posture a caller the host authenticated is
    /// admitted at invocation and for its admitted work exactly when the
    /// host's decision for that exact session allows it; the session stays
    /// the operator's, and the host is asked about the actual caller,
    /// executor, owner and session.
    #[test]
    fn a_host_authenticated_caller_is_admitted_by_the_hosts_exact_decision() {
        let session = SessionId::new();
        let access = HostAccess::answering(Ok(()));
        let grants = grant_owner();
        let policy = host_policy_over(Arc::clone(&grants), &access);
        let lineage = issue(
            &grants,
            "caller-grant",
            ExecutionRestrictions::unrestricted(),
        );
        let invoked = association(&session, |c| {
            c.requester = caller();
            c.authority_basis = WorkAuthorityBasis::GrantLineage {
                lineage: vec![lineage],
            };
        });
        assert!(invoked.candidate().target.logical_owner == operator());
        policy
            .authorize_invocation(&ingress_context(caller()), &invoked)
            .expect("the host admits its caller");
        policy
            .authorize_admitted_work(
                &invoked,
                &operation(&session),
                LocalPolicyPurpose::Operation,
                5,
            )
            .expect("and its admitted work");
        let mandate = policy
            .review_mandate(&invoked, &operation(&session), 5)
            .expect("the checked invocation")
            .expect("a mandate");
        assert_eq!(
            mandate,
            format!(
                "The caller {} authenticated by the trusted host invoked the realm agent {} \
                 through the ingress {} in realm stock-realm, for a session owned by {}, as the \
                 host's access decision for that session allows, under a grant-lineage \
                 mandate. With the work's admitted ceiling it bounds this operation to: \
                 actions any; resources any; processors any; audiences any; effective until \
                 no expiry.",
                json(&caller()),
                json(&executor()),
                json(&ingress()),
                json(&operator()),
            )
        );
        let asked = access.asked();
        assert_eq!(asked.len(), 3);
        assert!(
            asked
                .iter()
                .all(|(who, executor_asked, owner, asked_session)| {
                    *who == caller()
                        && *executor_asked == executor()
                        && *owner == operator()
                        && asked_session == &session
                })
        );
    }

    /// The host's refusal is a local verdict and its unavailability local
    /// unavailability, at invocation and for admitted work alike: the two
    /// faces ask the same question and agree.
    #[test]
    fn the_hosts_refusal_and_unavailability_reach_both_faces() {
        let session = SessionId::new();
        let access = HostAccess::answering(Err(denied().into()));
        let policy = host_policy(&access);
        let invoked = association(&session, |c| c.requester = caller());
        assert!(matches!(
            policy.authorize_invocation(&ingress_context(caller()), &invoked),
            Err(NativeAdmissionError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::Denied
        ));
        let refused = policy
            .authorize_admitted_work(
                &invoked,
                &operation(&session),
                LocalPolicyPurpose::Operation,
                5,
            )
            .expect_err("refused work");
        assert!(is_denied_verdict(&refused));
        access.answer(Err(OperationAuthorizationError::Unavailable));
        assert!(matches!(
            policy.authorize_invocation(&ingress_context(caller()), &invoked),
            Err(NativeAdmissionError::Readiness(_))
        ));
        assert!(matches!(
            policy.authorize_admitted_work(
                &invoked,
                &operation(&session),
                LocalPolicyPurpose::Operation,
                5,
            ),
            Err(OperationAuthorizationError::Unavailable)
        ));
    }

    /// Each posture refuses the other's requester: the operator is no
    /// requester of a host-authenticated composition (the host is never
    /// asked), a caller is none of the local operator's, and a caller never
    /// owns the session it invokes.
    #[test]
    fn neither_posture_admits_the_others_requester() {
        let session = SessionId::new();
        let access = HostAccess::answering(Ok(()));
        let hosted = host_policy(&access);
        let operators = association(&session, |_| {});
        assert!(matches!(
            hosted.authorize_invocation(&ingress_context(operator()), &operators),
            Err(NativeAdmissionError::Refused(_))
        ));
        assert!(is_denied_verdict(
            &hosted
                .authorize_admitted_work(
                    &operators,
                    &operation(&session),
                    LocalPolicyPurpose::Operation,
                    5,
                )
                .expect_err("the operator is no requester here")
        ));
        let owned_by_caller = association(&session, |c| {
            c.requester = caller();
            c.target = StockSessionDestination::for_session(&caller(), &session, 1)
                .unwrap()
                .target()
                .clone();
        });
        assert!(matches!(
            hosted.authorize_invocation(&ingress_context(caller()), &owned_by_caller),
            Err(NativeAdmissionError::Refused(_))
        ));
        assert!(access.asked().is_empty(), "{:?}", access.asked().len());
        let local = policy();
        let callers = association(&session, |c| c.requester = caller());
        assert!(matches!(
            local.authorize_invocation(&ingress_context(caller()), &callers),
            Err(NativeAdmissionError::Refused(_))
        ));
        // An ingress that authenticated someone other than the association's
        // requester is refused before the host is asked.
        assert!(matches!(
            hosted.authorize_invocation(&ingress_context(operator()), &callers),
            Err(NativeAdmissionError::Refused(_))
        ));
        assert!(access.asked().is_empty());
    }

    /// The attachment face asks the host the same exact-session question.
    #[test]
    fn a_host_authenticated_attachment_asks_the_same_decision() {
        let session = SessionId::new();
        let access = HostAccess::answering(Ok(()));
        let policy = host_policy(&access);
        let destination = StockSessionDestination::for_session(&operator(), &session, 4).unwrap();
        let control = |requester: PrincipalRef| {
            McpAttachmentControlRequest::from_trusted_ingress(
                session.clone(),
                installed_server(),
                McpAttachmentFacts {
                    requester,
                    actor: ingress(),
                    realm: realm(),
                    destination: destination.mcp_destination(),
                },
                Arc::new(()),
            )
            .expect("trusted control")
        };
        AdmittedWorkPolicyOwner::authorize_mcp_attachment(
            policy.as_ref(),
            &control(caller()),
            &operation(&session),
            1,
        )
        .expect("the host admits its caller's attachment");
        assert_eq!(
            access.asked(),
            vec![(caller(), executor(), operator(), session.clone())]
        );
        access.answer(Err(denied().into()));
        assert!(is_denied_verdict(
            &AdmittedWorkPolicyOwner::authorize_mcp_attachment(
                policy.as_ref(),
                &control(caller()),
                &operation(&session),
                1,
            )
            .expect_err("refused by the host")
        ));
        assert!(is_denied_verdict(
            &AdmittedWorkPolicyOwner::authorize_mcp_attachment(
                policy.as_ref(),
                &control(operator()),
                &operation(&session),
                1,
            )
            .expect_err("the operator is no requester here")
        ));
    }

    /// A web_search call whose plan the real composite resolved, with the
    /// builtin registered as an agent's dispatcher registers it.
    fn web_search_binding() -> PreparedAuthorizationBinding {
        let mut composite = meerkat_tools::builtin::CompositeDispatcher::new(
            Arc::new(meerkat_tools::builtin::MemoryTaskStore::new()),
            &meerkat_tools::builtin::BuiltinToolConfig::default(),
            Some(std::path::PathBuf::from(
                std::env::var_os("CARGO_MANIFEST_DIR")
                    .expect("CARGO_MANIFEST_DIR is set for test processes"),
            )),
            None,
            None,
            None,
        )
        .expect("composite dispatcher");
        composite.register_web_search_tool(
            Arc::new(meerkat_tools::builtin::web_search::EmptyWebSearchExecutor),
            meerkat_core::ToolCategoryOverride::Enable,
        );
        let args = serde_json::value::RawValue::from_string(r#"{"query":"stock"}"#.to_string())
            .expect("raw args");
        let plan = meerkat_core::AgentToolDispatcher::resolve_execution_plan(
            &composite,
            meerkat_core::types::ToolCallView {
                id: "call-1",
                name: "web_search",
                args: args.as_ref(),
            },
            &meerkat_core::ToolDispatchContext::default(),
            &meerkat_core::ToolExecutionResolutionContext::new(deadlines()),
        )
        .expect("web_search plan");
        plan_binding("web_search", plan)
    }

    /// Stock issues no lineage for the builtin web_search operation, so a
    /// per-target stock selection has none and the grant owner refuses the
    /// call as a Denied verdict before the operation owner is asked; the
    /// selection's base and MCP lineages never stand in for it. Under a
    /// legacy single stock grant the owner is reached and leaves the family
    /// unmapped (Unavailable).
    #[test]
    fn a_per_target_stock_selection_refuses_web_search_without_a_lineage() {
        struct Counting(Arc<StockInvocationPolicy>, std::sync::atomic::AtomicUsize);
        impl OperationPolicyOwner for Counting {
            fn authorize_operation(
                &self,
                association: &InputAuthorityAssociation,
                binding: &PreparedAuthorizationBinding,
                purpose: LocalPolicyPurpose,
                now_ms: u64,
            ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
                self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.0
                    .authorize_operation(association, binding, purpose, now_ms)
            }
        }
        let session = SessionId::new();
        let grants = grant_owner();
        let policy = policy_over(Arc::clone(&grants));
        let lineage = issue(
            &grants,
            "stock-grant",
            ExecutionRestrictions::unrestricted(),
        );
        let owner = Arc::new(Counting(
            Arc::clone(&policy),
            std::sync::atomic::AtomicUsize::new(0),
        ));
        let installed = Arc::new(
            meerkat_authorization::grant_policy::GrantBackedWorkPolicy::new(
                Arc::clone(&grants),
                Arc::clone(&policy) as Arc<dyn AdmittedWorkPolicyOwner>,
                owner.clone() as Arc<dyn OperationPolicyOwner>,
            ),
        );
        let search = web_search_binding();
        let prepare = |association: InputAuthorityAssociation| {
            installed
                .work_context(vec![association].into(), OperationExecutionScope::Domain)
                .expect("context")
                .authorization()
                .prepare(&search)
                .map(|_| ())
        };
        let targeted = association(&session, |c| {
            c.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
                base: Some(vec![lineage.clone()]),
                targets: vec![TargetGrantLineage {
                    target: GrantTarget::McpServer {
                        server: "alpha".into(),
                    },
                    lineage: vec![lineage.clone()],
                }],
            };
        });
        let error = prepare(targeted).expect_err("no lineage for web_search");
        assert!(is_denied_verdict(&error), "{error:?}");
        assert_eq!(
            owner.1.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the operation owner is never asked"
        );
        let error = prepare(granted(&session, &lineage)).expect_err("unmapped under one grant");
        assert_eq!(error, OperationAuthorizationError::Unavailable);
        assert_eq!(owner.1.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// The stock operation owner, whose review account read interferes
    /// once: inside the review observation and before the work owner reads
    /// the work's grant for its mandate.
    struct InterferingReview {
        inner: Arc<StockInvocationPolicy>,
        pending: std::sync::Mutex<Option<ReviewInterference>>,
    }

    enum ReviewInterference {
        /// A helper thread begins an owner change on the grant publication
        /// and holds it until the test releases it.
        HoldOwnerChange {
            begin: std::sync::mpsc::SyncSender<()>,
            held: std::sync::mpsc::Receiver<()>,
        },
        Poison(meerkat_authorization::publication::LocalAuthorizationPublication),
    }

    impl OperationPolicyOwner for InterferingReview {
        fn authorize_operation(
            &self,
            association: &InputAuthorityAssociation,
            binding: &PreparedAuthorizationBinding,
            purpose: LocalPolicyPurpose,
            now_ms: u64,
        ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
            self.inner
                .authorize_operation(association, binding, purpose, now_ms)
        }

        fn review_action_account(
            &self,
            association: &InputAuthorityAssociation,
            binding: &PreparedAuthorizationBinding,
            now_ms: u64,
        ) -> Result<Option<String>, OperationAuthorizationError> {
            match self.pending.lock().expect("interference").take() {
                Some(ReviewInterference::HoldOwnerChange { begin, held }) => {
                    begin.send(()).expect("start the owner change");
                    held.recv_timeout(std::time::Duration::from_secs(5))
                        .expect("the owner change is in progress");
                }
                Some(ReviewInterference::Poison(publication)) => publication.poison(),
                None => {}
            }
            self.inner
                .review_action_account(association, binding, now_ms)
        }
    }

    /// A review read whose work grant is resolved while an owner change is
    /// in progress is a stale refusal the review observation classifies,
    /// never an unavailable review context; once the change completes the
    /// review reads its material. A poison at the same point stays
    /// unavailable.
    #[test]
    fn an_owner_change_during_the_review_mandate_read_is_a_stale_refusal() {
        let publication = meerkat_authorization::publication::LocalAuthorizationPublication::new();
        let grants = Arc::new(
            LocalGrantAuthority::new(
                meerkat_authorization::grants::LocalGrantConfiguration {
                    root: operator(),
                    namespace: id("stock-grants"),
                    generation: 1,
                },
                publication.clone(),
                Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
            )
            .expect("grant owner"),
        );
        let policy = policy_over(Arc::clone(&grants));
        let session = SessionId::new();
        let work = granted(
            &session,
            &issue(
                &grants,
                "stock-grant",
                ExecutionRestrictions::unrestricted(),
            ),
        );
        let read = operation(&session);
        let review = |interference: Option<ReviewInterference>| {
            Arc::new(
                meerkat_authorization::grant_policy::GrantBackedWorkPolicy::new(
                    Arc::clone(&grants),
                    Arc::clone(&policy) as Arc<dyn AdmittedWorkPolicyOwner>,
                    Arc::new(InterferingReview {
                        inner: Arc::clone(&policy),
                        pending: std::sync::Mutex::new(interference),
                    }) as Arc<dyn OperationPolicyOwner>,
                ),
            )
            .work_context(vec![work.clone()].into(), OperationExecutionScope::Domain)
            .expect("the work")
            .authorization()
            .review_operation_context(&read)
        };

        let (begin_tx, begin_rx) = std::sync::mpsc::sync_channel(1);
        let (held_tx, held_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let changer = publication.clone();
        let helper = std::thread::spawn(move || {
            begin_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("asked to change an owner");
            let change = changer
                .begin_owner_change()
                .expect("an owner change in progress");
            held_tx.send(()).expect("the change is held");
            release_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("released by the test");
            drop(change);
        });
        let stale = review(Some(ReviewInterference::HoldOwnerChange {
            begin: begin_tx,
            held: held_rx,
        }));
        release_tx.send(()).expect("release the owner change");
        helper.join().expect("helper thread");
        assert!(
            matches!(
                &stale,
                Err(OperationAuthorizationError::Refused(refusal))
                    if refusal.kind() == OperationRefusalKind::ReprepareRequired
            ),
            "a change in progress is a stale review read, never an unavailable context: \
             {stale:?}"
        );
        let fresh = review(None);
        assert!(
            fresh.is_ok(),
            "the review reads its material once the change completes: {fresh:?}"
        );

        let poisoned = review(Some(ReviewInterference::Poison(publication)));
        assert!(
            matches!(poisoned, Err(OperationAuthorizationError::Unavailable)),
            "a poison stays unavailable: {poisoned:?}"
        );
    }
}
