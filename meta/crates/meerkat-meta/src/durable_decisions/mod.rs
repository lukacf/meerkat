//! Durable decisions on Meta's surfaces.
//!
//! Core's durable decision machine owns every decision's lifecycle. Meta
//! serves it, over the runtime's one decision owner ([`RuntimeDecisions`],
//! the state directory's `approval_decisions.sqlite3`):
//!
//! - the stdio host's `meta/decisions/*` verbs (`list`, `get`,
//!   `rendering`, `present`, `decide`, `withdraw`) and the
//!   `meta/decisions/notice` notification ([`StdioDecisionConnection`]);
//! - the Console's reads and its administrator's withdraw, under the
//!   decision kind's rule of who sees what (`console`);
//! - the decision ingresses a launch declares
//!   ([`DecisionIngressDeclarations`]), whose people the approver map lists
//!   as holders;
//! - the approver currentness Core asks for when a decision executes,
//!   answered from the live approver map
//!   ([`crate::UnifiedRuntime::decision_approver_currentness`]);
//! - the executors of the decision kinds the runtime owns, driven for it
//!   ([`DecisionExecutorDriver`]): the `identity.*` kinds, one per change of
//!   an agent (`identity.create`, `identity.update`, `identity.archive`,
//!   `identity.unarchive`, `identity.remove`, `identity.adopt`), whose held
//!   requests [`crate::identity_creation::IdentityDecisions`] holds and
//!   carries out.
//!
//! The approver map is the access configuration's `approvers`, the one
//! source of who may decide for an approver id: Console subjects and groups,
//! the people a declared ingress attests (`ingress_subjects`), the stdio
//! operator (`{ ingress = "operator" }`), and people whose identities span
//! ingresses (`people`). A person never decides a request its decision
//! records as theirs: a decide by the decision's requester, or by an
//! identity the map links with the requester in one person, is refused
//! `self_approval`. A decision records a person where its holder was given
//! one (a signed-in Console person; for a change of an agent, also a person
//! a host names as the request's proposer by their principal key); a
//! request a host carries for somebody without naming them is the host's.

mod approvers;
pub(crate) mod console;
mod executors;
mod ingress;
mod owner;
mod stdio;
mod wire;

pub use approvers::INGRESS_TRUST_DOMAIN_PREFIX;
pub(crate) use approvers::{MapApproverCurrentness, local_operator_principal};
pub use executors::DecisionExecutorDriver;
pub use ingress::{
    DecisionIngressDeclaration, DecisionIngressDeclarations, DecisionIngressError,
    check_approver_ingresses,
};
pub(crate) use owner::DecisionEventBinding;
pub use owner::RuntimeDecisions;
pub use stdio::{DecisionNoticeSink, StdioDecisionConnection};
pub use wire::{
    DECISION_NOTICE_METHOD, DECISIONS_METHOD_PREFIX, DecisionAttestationRequest,
    MetaDecisionDecideParams, MetaDecisionListResult, MetaDecisionNotice, MetaDecisionNoticeEvent,
    MetaDecisionStatus, MetaDecisionView,
};

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests;
