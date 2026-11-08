//! Approval-carried governed child grants: the `grant.child` decision kind.
//!
//! An agent offers the agent that asked it, for that work's requester, one
//! exact operation of a governed source it can reach (`access_offer`, in the
//! `decisions` tool family): Core holds a `grant.child` durable decision for
//! the offering grant's child rule. When a person approves it, the
//! [`GrantDecisionExecutor`] opens the decision's cleanup, then a one-time
//! approval branch of the agent that asked (a member forked from it with an
//! empty transcript, which only the mob's own approval branch path seats),
//! issues the branch its controller grant (a child of the asking agent's
//! controller grant) and the approval-carried grant (a child of the offering
//! grant carrying the decision), and records the decision executed. Its
//! notice admits the approved work into that branch
//! ([`ApprovalCarriedIngress`]), then tells the origin it was approved: one
//! operation, the approved one, whose one use is spent at its entry, then one
//! reply to the requester (`reply_to_requester`), settled by the decision
//! before it is delivered, whose authority is Core's rule of the approved
//! work. When the approved work ends or the grant's window does, whichever
//! is first, the branch is retired, its controller grant revoked and the
//! decision's cleanup settled; Core's pass sweeps whatever a stop or a busy
//! custody left, from the grant owner's unrevoked controller grants and the
//! host's open branches ([`crate::decision::DecisionExecutor::sweep`]).
//!
//! The host composes [`ApprovalGrants`] with its decision owner, its grant
//! owner, its approval branches ([`ApprovalBranchHost`]), its requester reply
//! route ([`RequesterReplyRoute`]) and the native custody that revokes a
//! retired branch's controller grant ([`ApprovalControllerCustody`]), gives
//! agents the `decisions` tools ([`ApprovalGrants::tools`]), registers the
//! executor ([`ApprovalGrants::executor`]) in the [`crate::decision::DecisionExecutorLoop`]
//! of the runtime that holds the grant owner's single-writer custody, and
//! composes its grant owner with [`DecisionApprovalUseCurrentness`], which
//! answers at each use's spend whether the person who decided is still
//! current.

use std::sync::{Arc, Weak};

use meerkat_authorization::grants::LocalGrantAuthority;
use meerkat_core::PrincipalRef;
use meerkat_core::approval::decision::{DecisionKind, DecisionRenderer};

mod body;
mod currentness;
mod executor;
mod ingress;
mod offer;
mod seams;
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests;
mod tools;

pub use body::{
    GrantChildRequest, GrantChildRequestError, OfferedOperation, approval_branch_principal,
    approval_grant_id, approved_work_id, branch_controller_grant_id,
};
pub use currentness::DecisionApprovalUseCurrentness;
pub use executor::GrantDecisionExecutor;
pub use ingress::{ApprovalCarriedIngress, ApprovalCarriedIngressError};
pub use offer::{
    ACCESS_OFFER_SUMMARY_MAX_BYTES, AccessOfferRefusal, AccessOfferRefusalKind,
    AccessOfferRefusalReason,
};
pub use seams::{
    AccessOutcome, ApprovalBranchError, ApprovalBranchHost, ApprovalBranchRequest,
    ApprovalBranchSeat, ApprovalControllerCustody, ApprovalGranteeBasis, ApprovalOrigin,
    ApprovedWorkAdmission, ReplyReceipt, ReplyRouteError, RequesterReplyRoute,
};
pub use tools::DecisionsToolDispatcher;

use crate::decision::{DecisionClock, DurableDecisionService, SystemDecisionClock};

/// The decision kind of access offers.
pub const GRANT_CHILD_KIND: &str = "grant.child";
/// The renderer of access offers.
pub const GRANT_CHILD_RENDERER: &str = "grant.render-v1";
/// The format of an access offer's held request.
pub const GRANT_CHILD_REQUEST_FORMAT: &str = "grant.child.v1";
pub use meerkat_core::approval::decision::APPROVAL_BRANCH_LABEL;
/// The tool that holds an access offer.
pub const ACCESS_OFFER_TOOL: &str = "access_offer";
/// The tool that replies to the requester of an approved work.
pub const REPLY_TO_REQUESTER_TOOL: &str = "reply_to_requester";

/// Default most uses of an approval-carried grant.
pub const DEFAULT_APPROVAL_GRANT_MAX_USES: u64 = 1;
/// Default longest window of an approval-carried grant: 15 minutes.
pub const DEFAULT_APPROVAL_GRANT_MAX_WINDOW_MS: u64 = 15 * 60 * 1000;
/// Default time an approver has to decide an access offer: one hour.
pub const DEFAULT_ACCESS_OFFER_DECISION_WINDOW_MS: u64 = 60 * 60 * 1000;

/// The host's bounds on access offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApprovalGrantPolicy {
    /// Most uses one offer may ask for.
    pub max_uses: u64,
    /// Longest window one offer may ask for, never past the host's own grant
    /// maximum.
    pub max_window_ms: u64,
    /// How long the approver has to decide an offer, at most: an offer whose
    /// offering grant's window ends sooner is due by that window's end less
    /// its requested window.
    pub decision_window_ms: u64,
}

impl Default for ApprovalGrantPolicy {
    fn default() -> Self {
        Self {
            max_uses: DEFAULT_APPROVAL_GRANT_MAX_USES,
            max_window_ms: DEFAULT_APPROVAL_GRANT_MAX_WINDOW_MS,
            decision_window_ms: DEFAULT_ACCESS_OFFER_DECISION_WINDOW_MS,
        }
    }
}

impl ApprovalGrantPolicy {
    /// At most `max_uses` uses and a window of at most `max_window_ms` per
    /// offer, decided within `decision_window_ms`.
    #[must_use]
    pub fn new(max_uses: u64, max_window_ms: u64, decision_window_ms: u64) -> Self {
        Self {
            max_uses,
            max_window_ms,
            decision_window_ms,
        }
    }
}

/// What a host composes approval-carried grants from.
#[non_exhaustive]
pub struct ApprovalGrantsConfiguration {
    /// The decision owner access offers are held in.
    pub decisions: DurableDecisionService,
    /// The grant owner the offering grants live in.
    pub grants: Arc<LocalGrantAuthority>,
    pub branches: Arc<dyn ApprovalBranchHost>,
    pub replies: Arc<dyn RequesterReplyRoute>,
    /// The native custody that revokes a retired branch's controller grant
    /// (the runtime the branches run in). Held weakly: the runtime's agents
    /// hold this composition through their tools.
    pub controller_custody: Weak<dyn ApprovalControllerCustody>,
    /// The runtime's decision ingress: the ingress actor of every approved
    /// work.
    pub decision_ingress: PrincipalRef,
    pub policy: ApprovalGrantPolicy,
    /// The clock offers are held and executed at: the host's wall clock by
    /// default.
    pub clock: Arc<dyn DecisionClock>,
}

impl ApprovalGrantsConfiguration {
    /// Approval-carried grants over these owners, with the default policy.
    #[must_use]
    pub fn new(
        decisions: DurableDecisionService,
        grants: Arc<LocalGrantAuthority>,
        branches: Arc<dyn ApprovalBranchHost>,
        replies: Arc<dyn RequesterReplyRoute>,
        controller_custody: Weak<dyn ApprovalControllerCustody>,
        decision_ingress: PrincipalRef,
    ) -> Self {
        Self {
            decisions,
            grants,
            branches,
            replies,
            controller_custody,
            decision_ingress,
            policy: ApprovalGrantPolicy::default(),
            clock: Arc::new(SystemDecisionClock),
        }
    }

    /// The same configuration reading `clock`.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn DecisionClock>) -> Self {
        self.clock = clock;
        self
    }

    /// The same configuration under `policy`.
    #[must_use]
    pub fn with_policy(mut self, policy: ApprovalGrantPolicy) -> Self {
        self.policy = policy;
        self
    }
}

/// The host's composition of approval-carried grants (see the module docs).
#[derive(Clone)]
pub struct ApprovalGrants(Arc<Inner>);

struct Inner {
    decisions: DurableDecisionService,
    grants: Arc<LocalGrantAuthority>,
    branches: Arc<dyn ApprovalBranchHost>,
    replies: Arc<dyn RequesterReplyRoute>,
    controller_custody: Weak<dyn ApprovalControllerCustody>,
    decision_ingress: PrincipalRef,
    policy: ApprovalGrantPolicy,
    clock: Arc<dyn DecisionClock>,
    kind: DecisionKind,
    renderer: DecisionRenderer,
    /// The decisions whose approved work this process watches: one watcher
    /// per decision, so a redelivered notice watches nothing twice.
    watched:
        std::sync::Mutex<std::collections::BTreeSet<meerkat_core::approval::decision::DecisionId>>,
}

impl std::fmt::Debug for ApprovalGrants {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalGrants")
            .field("policy", &self.0.policy)
            .finish_non_exhaustive()
    }
}

/// Why approval-carried grants could not be composed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ApprovalGrantsError {
    /// The decision ingress is not a qualified principal.
    #[error("the decision ingress must be a qualified principal")]
    UnqualifiedDecisionIngress,
    /// A policy bound of zero.
    #[error("every approval grant policy bound must be at least 1")]
    EmptyPolicy,
    /// The kind or renderer token does not validate (never for this
    /// version's tokens).
    #[error("the grant.child kind or renderer token is invalid")]
    InvalidToken,
}

impl ApprovalGrants {
    /// Compose approval-carried grants from `configuration`.
    ///
    /// # Errors
    /// [`ApprovalGrantsError`] for an unqualified decision ingress or an
    /// empty policy bound.
    pub fn new(configuration: ApprovalGrantsConfiguration) -> Result<Self, ApprovalGrantsError> {
        let ApprovalGrantsConfiguration {
            decisions,
            grants,
            branches,
            replies,
            controller_custody,
            decision_ingress,
            policy,
            clock,
        } = configuration;
        decision_ingress
            .validate_qualified()
            .map_err(|_| ApprovalGrantsError::UnqualifiedDecisionIngress)?;
        if policy.max_uses == 0 || policy.max_window_ms == 0 || policy.decision_window_ms == 0 {
            return Err(ApprovalGrantsError::EmptyPolicy);
        }
        let (Some(kind), Ok(renderer)) = (
            offer::grant_child_kind(),
            DecisionRenderer::new(GRANT_CHILD_RENDERER),
        ) else {
            return Err(ApprovalGrantsError::InvalidToken);
        };
        Ok(Self(Arc::new(Inner {
            decisions,
            grants,
            branches,
            replies,
            controller_custody,
            decision_ingress,
            policy,
            clock,
            kind,
            renderer,
            watched: std::sync::Mutex::default(),
        })))
    }

    /// The configured clock now, in UNIX milliseconds.
    pub(crate) fn now_ms(&self) -> Option<u64> {
        self.0.clock.now_ms().ok()
    }

    /// The `decisions` tools for agents of this host: `access_offer` and
    /// `reply_to_requester`. Both run only in governed work.
    #[must_use]
    pub fn tools(&self) -> Arc<dyn meerkat_core::AgentToolDispatcher> {
        Arc::new(DecisionsToolDispatcher::new(self.clone()))
    }

    /// The executor of `grant.child` decisions, for the
    /// [`crate::decision::DecisionExecutorLoop`] of the runtime that holds
    /// the grant owner's single-writer custody.
    #[must_use]
    pub fn executor(&self) -> Arc<dyn crate::decision::DecisionExecutor> {
        Arc::new(GrantDecisionExecutor::new(self.clone()))
    }
}
