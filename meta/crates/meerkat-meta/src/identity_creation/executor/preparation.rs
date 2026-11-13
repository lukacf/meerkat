//! The seam's call into Core's settle of an agent's preparation.
//!
//! An agent of a profile its host prepares is committed pending its
//! preparation: Core holds it with no member, and it runs nothing. The
//! host's operator settles it once the host has prepared what the agent
//! needs. The settle is one call of the mob's handle
//! (`settle_identity_preparation`): Core writes the settle's receipt and the
//! agent's record, present again, in one store transaction and seats the
//! agent from its record, dormant, or live on its first message, which runs
//! once. Core owns the whole of it: which agent awaits a preparation, the
//! replay of a settle asked again, and what a settle is refused for.
//!
//! A host that gives up on an agent it cannot prepare removes it: that is
//! the removal every agent has, and nothing here.

use meerkat_mob::{
    IdentityChangeRequesterInput, IdentityOperation, IdentityPolicyOutcome,
    IdentityPreparationSettle, MobError, MobHandle,
};
use serde_json::json;

use super::super::config::ChangeClass;
use super::super::digest::canonical_digest;
use super::super::operation::OperationId;
use super::super::refusal::{IdentityRefusal, IdentityRefusalKind};
use super::{
    ChangeCommitted, ChangeConsent, ChangeRequester, ExecutorError, committed_receipt, core_name,
    outcome_of_change, outcome_of_receipt, refusal_of_error,
};

/// One settle of an agent's preparation through the seam: the agent, the
/// operation the settle commits as, who asks, and the mob.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct PreparationSettle<'a> {
    /// The mob: Core's handle to it.
    pub mob: &'a MobHandle,
    /// The operation the settle commits as ([`OperationId::for_request`]).
    pub operation_id: &'a OperationId,
    /// Who asks. Only the host's operator settles a preparation.
    pub requester: &'a ChangeRequester,
    /// The agent whose preparation is settled.
    pub name: &'a str,
}

impl<'a> PreparationSettle<'a> {
    /// The settle of `name`'s preparation as `operation_id`, by
    /// `requester`, in `mob`.
    pub fn new(
        mob: &'a MobHandle,
        operation_id: &'a OperationId,
        requester: &'a ChangeRequester,
        name: &'a str,
    ) -> Self {
        Self {
            mob,
            operation_id,
            requester,
            name,
        }
    }
}

impl std::fmt::Debug for PreparationSettle<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparationSettle")
            .field("mob", &self.mob.mob_id())
            .field("operation_id", &self.operation_id)
            .field("requester", &self.requester)
            .field("name", &self.name)
            .finish()
    }
}

/// The refusal of a settle asked by anyone but the host's operator: the
/// host prepares its agents, so its operator alone says an agent is
/// prepared.
fn not_the_operator() -> IdentityRefusal {
    IdentityRefusal::new(IdentityRefusalKind::ChangeDenied {
        change: ChangeClass::Create,
    })
    .with_detail("Only the host's operator settles an agent's preparation: nothing was changed.")
}

/// `refusal`, about the settle's `name` when it says the name is no agent.
fn at_name(refusal: IdentityRefusal) -> IdentityRefusal {
    if refusal.field.is_some() {
        return refusal;
    }
    match refusal.kind {
        IdentityRefusalKind::IdentityUnknown | IdentityRefusalKind::InvalidName => {
            refusal.at("name")
        }
        _ => refusal,
    }
}

/// Settle the preparation of one agent: Core's settle, as the host's
/// operator, under the consent of the launch's policy, which names the
/// preparation. The settle's request digest is over its own canonical form
/// (the change it is, the agent, who asks), so another change under the
/// same operation id is Core's `operation_reused` and the same settle asked
/// again is Core's replay: it writes nothing and answers the same outcome.
pub(super) async fn settle(
    settle: &PreparationSettle<'_>,
) -> Result<ChangeCommitted, ExecutorError> {
    let ChangeRequester::Host {
        surface,
        operator: true,
    } = settle.requester
    else {
        return Ok(ChangeCommitted::refused(not_the_operator()));
    };
    let member = match core_name(settle.name, "name") {
        Ok(member) => member,
        Err(refusal) => return Ok(ChangeCommitted::refused(refusal)),
    };
    let digest = canonical_digest(&json!({
        "change": "settle_preparation",
        "name": settle.name,
        "requester": {"kind": "host", "surface": surface},
    }));
    let operation = IdentityOperation::new(settle.operation_id.as_str(), digest.as_str());
    let change = IdentityPreparationSettle::new(
        operation,
        IdentityChangeRequesterInput::host_operator(surface.as_str()),
        member,
    );
    match settle
        .mob
        .settle_identity_preparation(change, ChangeConsent::Policy(IdentityPolicyOutcome::Allow))
        .await
    {
        Ok(settled) => outcome_of_change(settle.mob, settle.operation_id, &settled.change),
        Err(error) => after_core_error(settle, digest.as_str(), error).await,
    }
}

/// What a settle answers when Core's call erred. Core's own refusal says
/// nothing was written: it is the typed refusal. Any other error may follow
/// Core's commit (Core commits the settle, then seats the agent, which can
/// fail on its own), so Core's receipt is read first: a receipt of this
/// settle means it committed, and it is answered as committed, with the
/// agent's seat owed. A receipt of another request under the operation id
/// is `operation_reused`. Without a receipt a member step in flight is the
/// retryable `admission_unsettled`, and anything else is the error it was.
async fn after_core_error(
    settle: &PreparationSettle<'_>,
    digest: &str,
    error: MobError,
) -> Result<ChangeCommitted, ExecutorError> {
    if matches!(error, MobError::IdentityChangeRefused { .. }) {
        return Ok(ChangeCommitted::refused(at_name(refusal_of_error(error)?)));
    }
    let Ok(receipt) = committed_receipt(settle.mob, settle.operation_id).await else {
        return Err(ExecutorError::Core(Box::new(error)));
    };
    let Some(receipt) = receipt else {
        return Ok(ChangeCommitted::refused(refusal_of_error(error)?));
    };
    if receipt.request_digest != digest {
        return Ok(ChangeCommitted::refused(IdentityRefusal::new(
            IdentityRefusalKind::OperationReused,
        )));
    }
    let outcome = outcome_of_receipt(settle.mob, settle.operation_id, &receipt)?;
    Ok(ChangeCommitted::new(outcome, false))
}
