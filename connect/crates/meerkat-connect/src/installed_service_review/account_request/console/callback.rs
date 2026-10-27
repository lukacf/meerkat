//! Bounded delivery into one already-started native attempt. No login, target,
//! redirect, issuer or approval owner can be supplied by callback bytes.

use super::*;
use crate::stdio::Exchange;
use serde::Deserialize;

pub(crate) enum CallbackResult {
    Completed,
    Refused,
    CleanupUnconfirmed,
}

pub(super) struct CallbackInput {
    // Preserve the bounded original provider query through delivery, without
    // logging or using its other fields as authority.
    _encoded_query: String,
    pub(super) state: String,
    pub(super) code: Option<String>,
    pub(super) response: oneshot::Sender<CallbackResult>,
}

/// Only a delivery handle. The future verified HTTP owner retains it with the
/// exact request/login and checks both before forwarding and after awaiting.
/// It is neither authentication nor a callback registry. Native Control checks
/// the same state again and supplies its own target, redirect and attempt handle.
pub(crate) struct CallbackDelivery {
    expected_state: String,
    send: Option<oneshot::Sender<CallbackInput>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    query: String,
}

impl CallbackDelivery {
    pub(super) fn channel(state: &str) -> (Self, oneshot::Receiver<CallbackInput>) {
        let (send, receive) = oneshot::channel();
        (
            Self {
                expected_state: state.into(),
                send: Some(send),
            },
            receive,
        )
    }

    /// Called only after the actual HTTP owner has checked current J1 human,
    /// access, same login, Origin and request. Invalid input leaves this exact
    /// delivery available; an accepted delivery consumes it even if its answer
    /// is later lost. Repeating a POST cannot re-enter native completion.
    pub(crate) fn deliver_json(
        &mut self,
        body: &[u8],
    ) -> Result<oneshot::Receiver<CallbackResult>, ConnectError> {
        self.deliver_json_authorized(body, || Ok(()))
    }

    pub(crate) fn deliver_json_authorized(&mut self, body: &[u8],
        before_delivery: impl FnOnce() -> Result<(), ConnectError>,
    ) -> Result<oneshot::Receiver<CallbackResult>, ConnectError> {
        if body.len() > 64 * 1024 {
            return Err(invalid());
        }
        let envelope: Envelope = serde_json::from_slice(body).map_err(|_| invalid())?;
        let (state, code) = parse_query(&envelope.query)?;
        if state != self.expected_state {
            return Err(invalid());
        }
        if self.send.as_ref().is_none_or(|send| send.is_closed()) { return Err(invalid()); }
        before_delivery()?;
        let send = self.send.take().ok_or_else(invalid)?;
        let (response, result) = oneshot::channel();
        send.send(CallbackInput {
            _encoded_query: envelope.query,
            state,
            code,
            response,
        })
        .map_err(|_| invalid())?;
        Ok(result)
    }
}

fn parse_query(query: &str) -> Result<(String, Option<String>), ConnectError> {
    if query.len() > 16 * 1024 || query.contains('#') {
        return Err(invalid());
    }
    // URL parsing is data decoding only. This fixed URL is never fetched and
    // does not select the callback origin or any native owner.
    let mut url = reqwest::Url::parse("http://callback.invalid/").expect("constant URL");
    url.set_query(Some(query.strip_prefix('?').unwrap_or(query)));
    let mut state = None;
    let mut code = None;
    let mut error = None;
    let mut description = None;
    for (key, value) in url.query_pairs() {
        let (slot, limit) = match key.as_ref() {
            "state" => (&mut state, 1024),
            "code" => (&mut code, 8192),
            "error" => (&mut error, 256),
            "error_description" => (&mut description, 2048),
            _ => continue,
        };
        if slot.is_some() || value.len() > limit || value.is_empty() {
            return Err(invalid());
        }
        *slot = Some(value.into_owned());
    }
    let state = state.ok_or_else(invalid)?;
    if code.is_some() == error.is_some() || description.is_some() && error.is_none() {
        return Err(invalid());
    }
    // Provider errors are private data, not approval or a new target. Retain
    // only their presence so raw provider strings cannot reach a public error.
    Ok((state, code))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    review_id: u64,
    approval_id: String,
    decision: crate::stdio::wire::Decision,
}

/// Forward only into the existing retained Exchange. The host's same-viewer
/// route owns `review_id` and the slot; bytes cannot choose a new prompt or
/// reconstruct an ApprovalRecord. Native ApprovalPrompt::decide remains the
/// decision owner, and rechecks eligibility after transport refresh.
pub(crate) fn answer_retained(
    review_id: u64,
    exchange: &mut Option<Exchange>,
    body: &[u8],
) -> Result<(), ConnectError> {
    answer_retained_authorized(review_id, exchange, body, || Ok(()))
}

pub(crate) fn answer_retained_authorized(
    review_id: u64, exchange: &mut Option<Exchange>, body: &[u8],
    before_delivery: impl FnOnce() -> Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    if body.len() > 4096 {
        return Err(invalid());
    }
    let answer: Answer = serde_json::from_slice(body).map_err(|_| invalid())?;
    if answer.review_id > 9_007_199_254_740_991
        || answer.review_id != review_id
        || answer.approval_id.is_empty()
        || answer.approval_id.len() > 512
    {
        return Err(invalid());
    }
    let label = match answer.decision {
        crate::stdio::wire::Decision::Approve => "approve",
        crate::stdio::wire::Decision::Deny => "deny",
    };
    let matches = exchange.as_ref().is_some_and(|exchange| {
        exchange.prompt["kind"].as_str() == Some("approval")
            && exchange.prompt["approval_id"].as_str() == Some(answer.approval_id.as_str())
            && exchange.prompt["allowed_decisions"]
                .as_array()
                .is_some_and(|allowed| allowed.iter().any(|value| value.as_str() == Some(label)))
            && !exchange.response.is_closed()
    });
    if !matches {
        return Err(invalid());
    }
    before_delivery()?;
    exchange
        .take()
        .expect("matched retained exchange")
        .response
        .send(Ok(PromptAnswer::Approval {
            decision: answer.decision,
        }))
        .map_err(|_| invalid())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignInCancellation { review_id: u64 }

/// Cancel only the provider notice retained by this exact Active review. Each
/// Active owns at most one account attempt; its id is never reused for another
/// attempt or a later request. Approval prompts still require their own decision.
/// The mounted caller holds callback then prompt custody and authenticates the
/// same current action before invoking this synchronous delivery.
pub(crate) fn cancel_retained_sign_in_authorized(
    review_id: u64, exchange: &mut Option<Exchange>, callback: &mut Option<CallbackDelivery>,
    body: &[u8], before_delivery: impl FnOnce() -> Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    if body.len() > 256 { return Err(invalid()); }
    let request: SignInCancellation = serde_json::from_slice(body).map_err(|_| invalid())?;
    if request.review_id == 0 || request.review_id > 9_007_199_254_740_991
        || request.review_id != review_id
        || !exchange.as_ref().is_some_and(|exchange|
            exchange.prompt["kind"].as_str() == Some("provider_browser") && !exchange.response.is_closed())
    { return Err(invalid()); }
    before_delivery()?;
    // Remove an undelivered callback so it cannot start completion after this
    // request wins. None also covers an already-entered completion, whose driver
    // retains the same notice receiver and joins cancellation through Control.
    callback.take();
    exchange.take().expect("matched retained provider notice").response
        .send(Ok(PromptAnswer::Cancel)).map_err(|_| invalid())
}

fn invalid() -> ConnectError {
    refused("The protected setup response is unavailable or does not match")
}
