//! Service-administration on the same private actor as protected reviews.
//! The native operation owns completion. Parent values retain attribution only;
//! the parent SDK retains the actual authenticated exclusion until Joined.
use super::*;
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
use tokio::sync::{mpsc, oneshot};
use toolkit_mobkit::{private_host::{ServiceParentCommand, ServiceParentEndpoint,
    ServiceParentResponse, ServiceParentAdministrationRequest, ServiceParentDecision,
    ServiceParentDecisionSource, ConsoleLoginHandle, ConsoleLoginReservation},
    private_originals::{ApplicationServiceOperation, ServiceParentAuthorization,
        ServiceParentAdministrationReservation, ServiceParentAuthorizationProposal,
        ServiceParentOperationObservation, ServiceParentCustody, ServiceToolPolicyApproval}};
use meerkat_core::PrincipalRef;
use meerkat_toolkit::host_lifetime::policy::ServiceParentAdministration;
use serde_json::{Map, Value, json};

type NextReply = ServiceParentResponse<ServiceParentAdministrationRequest>;
type ResultReply = ServiceParentResponse<ServiceParentOperationObservation>;

struct ParentExclusion {
    open: AtomicBool,
    closed: PrivateHostClosed,
    login: Option<ConsoleLoginHandle>,
    lifetime: Mutex<Option<ConsoleLoginReservation>>,
}
impl ParentExclusion {
    fn check(&self) -> Result<(), HostLifetimeError> {
        if !self.open.load(Ordering::Acquire) { return Err(HostLifetimeError::Denied); }
        if self.closed.is_closed_for_presenter() { return Err(HostLifetimeError::Unavailable); }
        if let Some(login) = &self.login {
            ServiceParentEndpoint::check_parent_login_lifetime(login)?;
        }
        Ok(())
    }
    fn fence(&self) { self.open.store(false, Ordering::Release); }
    fn joined(&self, observed: &ServiceParentOperationObservation) {
        // Never hold the slot/approval/phase mutex while a lifetime drops.
        let retained = self.lifetime.lock().ok().and_then(|mut lifetime| lifetime.take());
        if let Some(retained) = retained {
            if observed.outcome == Some(toolkit_mobkit::private_originals::ServiceParentOperationOutcome::Uncertain) {
                drop(retained);
            } else { retained.settled(); }
        }
    }
}
struct AuthorizedParent {
    proposal: ServiceParentAuthorizationProposal,
    digest: String,
    provenance: Map<String, Value>,
    exclusion: Arc<ParentExclusion>,
    tool_policy: Option<ServiceToolPolicyApproval>,
}
impl ServiceParentAdministrationReservation for AuthorizedParent {
    fn operator(&self) -> &PrincipalRef { &self.proposal.operator }
    fn rule(&self) -> &ServiceParentAdministration { &self.proposal.rule }
    fn proposal_sha256(&self) -> &str { &self.digest }
    fn provenance(&self) -> &Map<String, Value> { &self.provenance }
    fn check_current(&self) -> Result<(), HostLifetimeError> { self.exclusion.check() }
    fn tool_policy_approval(&self) -> Option<&ServiceToolPolicyApproval> { self.tool_policy.as_ref() }
}

fn authorize(endpoint: &ServiceParentEndpoint, closed: &PrivateHostClosed,
    request: &ServiceParentAdministrationRequest, decision: ServiceParentDecision,
) -> Result<(ServiceParentAuthorization, Option<Arc<ParentExclusion>>), HostLifetimeError> {
    let (source, authorization_ref, provenance, tool_policy) = match decision {
        ServiceParentDecision::Authorized { source, authorization_ref, provenance }
            if request.proposal.tool_policy.is_none() => (source, authorization_ref, provenance, None),
        ServiceParentDecision::AuthorizedWithToolPolicy { source, authorization_ref, provenance, tool_policy_sha256 } => {
            let proposal = request.proposal.tool_policy.as_ref().ok_or(HostLifetimeError::Denied)?;
            let approval = ServiceToolPolicyApproval::until_superseded(proposal)?;
            if approval.proposal_sha256() != tool_policy_sha256 { return Err(HostLifetimeError::Denied); }
            (source, authorization_ref, provenance, Some(approval))
        }
        ServiceParentDecision::RequiresHuman => return Ok((ServiceParentAuthorization::RequiresHuman, None)),
        ServiceParentDecision::Unavailable => return Err(HostLifetimeError::Unavailable),
        _ => return Err(HostLifetimeError::Denied),
    };
    if authorization_ref.trim().is_empty() || authorization_ref.len() > 512 || provenance.is_empty()
        || serde_json::to_vec(&provenance).map_err(|_| HostLifetimeError::InvalidSelection)?.len() > 12 * 1024
    { return Err(HostLifetimeError::InvalidSelection); }
    let login = match source {
        ServiceParentDecisionSource::CurrentLoginDecision { enrollment_id } => Some(endpoint.parent_login_lifetime(enrollment_id)?),
        ServiceParentDecisionSource::StandingRuleDecision => None,
    };
    let lifetime = login.as_ref().map(ConsoleLoginHandle::reserve).transpose()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    let exclusion = Arc::new(ParentExclusion { open: AtomicBool::new(true), closed: closed.clone(), login,
        lifetime: Mutex::new(lifetime) });
    exclusion.check()?;
    // Enrollment IDs and transient observation timestamps are deliberately not
    // immutable authorization evidence. A retry retains the same actual rule
    // decision reference, while reobserving its current real guard separately.
    let provenance = Map::from_iter([
        ("authorization_ref".into(), json!(authorization_ref)),
        ("application_provenance".into(), Value::Object(provenance)),
    ]);
    let reserved = AuthorizedParent { proposal: request.proposal.clone(), digest: request.proposal_sha256.clone(),
        provenance, exclusion: exclusion.clone(), tool_policy };
    Ok((ServiceParentAuthorization::Reserved(Box::new(reserved)), Some(exclusion)))
}

/// This owns the operation before the first administrative wait or entry.
/// Dropping a drive observation cannot drop the retained worker/source future.
/// Final operation observations remain here until explicit exchange retirement.
pub(super) async fn drive(
    exchange_id: u64, mut operation: ApplicationServiceOperation,
    endpoint: &ServiceParentEndpoint, requests: &mut mpsc::Receiver<RemoteControllerCommand>,
    next: &mut Option<NextReply>, closed: &PrivateHostClosed,
) {
    let control = operation.control();
    let mut request: Option<ServiceParentAdministrationRequest> = None;
    let mut answer: Option<ServiceParentResponse<ServiceParentAuthorization>> = None;
    let mut submitted = false;
    let mut exclusion: Option<Arc<ParentExclusion>> = None;
    let mut final_observation = None;
    let mut results: Vec<ResultReply> = Vec::new();
    let mut retirements: Vec<ResultReply> = Vec::new();
    let mut retiring = false;
    let mut closing = false;
    let mut receiver_done = false;
    loop {
        // Login expiry/retirement is supplementary to the real application's
        // authorization guard. Never turn it into verified Human identity.
        let login = exclusion.as_ref().and_then(|held| held.login.clone());
        tokio::select! {
            _ = control.wait_retiring(), if !retiring => {
                retiring = true;
                if let Some(held) = &exclusion { held.fence(); }
                if let Some(response) = answer.take() { let _ = response.send(Err(HostLifetimeError::Denied)); }
            }
            _ = closed.wait_closed(), if !closing => {
                closing = true; retiring = true; requests.close(); control.begin_retire();
                if let Some(held) = &exclusion { held.fence(); }
                if let Some(response) = answer.take() { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
            }
            _ = async { if let Some(login) = &login { login.closed().await; }
                else { std::future::pending::<()>().await; } }, if !retiring => {
                retiring = true; control.begin_retire();
                if let Some(held) = &exclusion { held.fence(); }
            }
            observed = operation.drive(), if final_observation.is_none() => {
                // Core drive only returns its actual owner observation. Joined
                // is checked rather than inferred from future completion.
                if observed.custody != ServiceParentCustody::Joined { continue; }
                if let Some(held) = &exclusion { held.joined(&observed); }
                for response in results.drain(..) { let _ = response.send(Ok(observed.clone())); }
                final_observation = Some(observed);
            }
            command = requests.recv(), if !receiver_done => {
                let Some(command) = command else {
                    receiver_done = true; closing = true; retiring = true; control.begin_retire();
                    if let Some(held) = &exclusion { held.fence(); }
                    if let Some(response) = answer.take() { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
                    continue;
                };
                match command {
                    RemoteControllerCommand::ServiceParent(ServiceParentCommand::Reserve { proposal, observer, response }) => {
                        if request.is_some() || retiring || !control.same_operation(&observer) {
                            let _ = response.send(Err(HostLifetimeError::WrongOwner));
                            continue;
                        }
                        match proposal.proposal_sha256() {
                            Ok(proposal_sha256) => {
                                let candidate = ServiceParentAdministrationRequest { exchange_id, proposal_sha256, proposal };
                                // Check the complete existing reply before publishing an
                                // exchange. Oversize/unsupported values refuse this request,
                                // not the shared private channel or another retained owner.
                                if candidate.validate_private_response().is_err() {
                                    let _ = response.send(Err(HostLifetimeError::Unavailable));
                                    if let Some(waiter) = next.take() {
                                        let _ = waiter.send(Err(HostLifetimeError::Unavailable));
                                    }
                                    continue;
                                }
                                request = Some(candidate);
                                answer = Some(response);
                                if let Some(waiter) = next.take() { let _ = waiter.send(Ok(request.as_ref().expect("retained request").clone())); }
                            }
                            Err(error) => { let _ = response.send(Err(error)); }
                        }
                    }
                    RemoteControllerCommand::ServiceParent(ServiceParentCommand::Next { response }) => {
                        if let Some(request) = &request { let _ = response.send(Ok(request.clone())); }
                        else if next.is_none() { *next = Some(response); }
                        else { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
                    }
                    RemoteControllerCommand::ServiceParent(ServiceParentCommand::Submit { exchange_id: id, proposal_sha256, decision, response }) => {
                        let matching = request.as_ref().filter(|request| id == exchange_id
                            && proposal_sha256 == request.proposal_sha256 && !submitted && !retiring);
                        if let Some(request) = matching {
                            let Some(sender) = answer.take() else { let _ = response.send(Err(HostLifetimeError::Denied)); continue; };
                            // Once a matching answer is attempted it is consumed
                            // even on typed refusal or a lost forwarding ACK.
                            submitted = true;
                            let authorized = authorize(endpoint, closed, request, decision);
                            let forwarded = match authorized {
                                Ok((decision, held)) => { exclusion = held; sender.send(Ok(decision)).is_ok() }
                                Err(error) => sender.send(Err(error)).is_ok(),
                            };
                            let _ = response.send(if forwarded { Ok(()) } else { Err(HostLifetimeError::Unavailable) });
                        } else { let _ = response.send(Err(HostLifetimeError::Denied)); }
                    }
                    RemoteControllerCommand::ServiceParent(ServiceParentCommand::Result { exchange_id: id, response }) if id == exchange_id && request.is_some() => {
                        if let Some(observed) = &final_observation { let _ = response.send(Ok(observed.clone())); }
                        else if results.len() < 16 { results.push(response); }
                        else { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
                    }
                    RemoteControllerCommand::ServiceParent(ServiceParentCommand::Retire { exchange_id: id, response }) if id == exchange_id && request.is_some() => {
                        retiring = true; control.begin_retire();
                        if let Some(held) = &exclusion { held.fence(); }
                        if let Some(sender) = answer.take() { let _ = sender.send(Err(HostLifetimeError::Denied)); }
                        if retirements.len() < 16 { retirements.push(response); }
                        else { let _ = response.send(Err(HostLifetimeError::NotSettled)); }
                    }
                    other => other.refuse().await,
                }
            }
        }
        if let Some(observed) = &final_observation {
            if closing || !retirements.is_empty() || request.is_none() {
                for response in retirements.drain(..) { let _ = response.send(Ok(observed.clone())); }
                break;
            }
        }
    }
}
