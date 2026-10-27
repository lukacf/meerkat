//! Private credential input for one retained Console approval.
//! Values never enter PromptAnswer, a model request or a public projection.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::{
    ConnectError,
    service_install::ServiceCredentialSlot,
    stdio::{Exchange, PromptAnswer, wire::Decision},
    terminal::ServiceSecret,
};

/// No Clone, Debug or Serialize implementation. The native mutation caller
/// borrows each value only after the existing owner records the exact approval.
pub(crate) struct CredentialValues {
    pub(crate) values: BTreeMap<String, ServiceSecret>,
}

/// Presentation custody only, not native approval or administration authority.
/// The caller keeps this sender with the same Active review and exact login.
pub(crate) struct CredentialDelivery {
    slots: Vec<ServiceCredentialSlot>,
    send: oneshot::Sender<CredentialValues>,
}

impl CredentialDelivery {
    pub(crate) fn channel(
        slots: Vec<ServiceCredentialSlot>,
    ) -> Result<(Self, oneshot::Receiver<CredentialValues>), ConnectError> {
        let names = slots.iter().map(ServiceCredentialSlot::environment_variable)
            .collect::<BTreeSet<_>>();
        if slots.is_empty() || names.len() != slots.len() {
            return Err(invalid());
        }
        let (send, receive) = oneshot::channel();
        Ok((Self { slots, send }, receive))
    }

    /// Checked declaration names and display labels only. No submitted value
    /// is kept in this presentation handle or copied back to the browser.
    pub(crate) fn projection(&self) -> Value {
        json!({
            "kind": "declared_secret_slots",
            "slots": self.slots.iter().map(|slot| json!({
                "slot": slot.environment_variable(),
                "label": slot.label(),
            })).collect::<Vec<_>>(),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    review_id: u64,
    approval_id: String,
    decision: Decision,
    credentials: Vec<Credential>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    slot: String,
    value: String,
}

/// The mounted HTTP owner first verifies Origin, current native Human, the
/// retained login and exact active task. Its final synchronous callback pins
/// the accepted action before either one-shot is consumed. LocalApprovals
/// remains the decision owner; the credential receiver cannot publish effects.
pub(crate) fn answer_retained_credentials(
    review_id: u64,
    delivery: &mut Option<CredentialDelivery>,
    exchange: &mut Option<Exchange>,
    body: &[u8],
    before_delivery: impl FnOnce() -> Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    if body.len() > 64 * 1024 {
        return Err(invalid());
    }
    let answer: Answer = serde_json::from_slice(body).map_err(|_| invalid())?;
    if answer.review_id == 0 || answer.review_id > 9_007_199_254_740_991
        || answer.review_id != review_id || answer.approval_id.is_empty()
        || answer.approval_id.len() > 512
    {
        return Err(invalid());
    }
    let decision = match &answer.decision {
        Decision::Approve => "approve",
        Decision::Deny => "deny",
    };
    let offered = delivery.as_ref().ok_or_else(invalid)?;
    if offered.send.is_closed() || !exchange.as_ref().is_some_and(|exchange| {
        exchange.prompt["kind"].as_str() == Some("approval")
            && exchange.prompt["approval_id"].as_str() == Some(answer.approval_id.as_str())
            && exchange.prompt["allowed_decisions"].as_array().is_some_and(|allowed|
                allowed.iter().any(|value| value.as_str() == Some(decision)))
            && !exchange.response.is_closed()
    }) {
        return Err(invalid());
    }
    let expected = offered.slots.iter().map(|slot| slot.environment_variable().to_owned())
        .collect::<BTreeSet<_>>();
    let mut values = BTreeMap::new();
    for credential in answer.credentials {
        if !expected.contains(&credential.slot) || values.contains_key(&credential.slot) {
            return Err(invalid());
        }
        let secret = ServiceSecret::from_hidden_input(credential.value).map_err(|_| invalid())?;
        values.insert(credential.slot, secret);
    }
    match &answer.decision {
        Decision::Approve if values.keys().cloned().collect::<BTreeSet<_>>() != expected => {
            return Err(invalid());
        }
        Decision::Deny if !values.is_empty() => return Err(invalid()),
        _ => {}
    }
    before_delivery()?;
    let offered = delivery.take().ok_or_else(invalid)?;
    // Values move only to the same retained review future. It first awaits
    // the persisted decision; a failed prompt delivery cannot authorize use.
    offered.send.send(CredentialValues { values }).map_err(|_| invalid())?;
    exchange.take().ok_or_else(invalid)?.response
        .send(Ok(PromptAnswer::Approval { decision: answer.decision }))
        .map_err(|_| invalid())
}

fn invalid() -> ConnectError {
    ConnectError::InvalidInput("The protected credential response is unavailable".into())
}

#[cfg(test)]
mod tests;
