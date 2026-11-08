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
//!   ([`crate::UnifiedRuntime::decision_approver_currentness`]).
//!
//! The approver map is the access configuration's `approvers`, the one
//! source of who may decide for an approver id: Console subjects and groups,
//! the people a declared ingress attests (`ingress_subjects`), the stdio
//! operator (`{ ingress = "operator" }`), and people whose identities span
//! ingresses (`people`).

mod approvers;
pub(crate) mod console;
mod ingress;
mod owner;
mod stdio;
mod wire;

pub use approvers::INGRESS_TRUST_DOMAIN_PREFIX;
pub(crate) use approvers::MapApproverCurrentness;
pub use ingress::{
    DecisionIngressDeclaration, DecisionIngressDeclarations, DecisionIngressError,
    check_approver_ingresses,
};
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
