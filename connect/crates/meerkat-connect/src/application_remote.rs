//! Protected remote presentation inside the existing application gateway.
//! Parent authentication claims the exact answer; the compiled operator and
//! native LocalApprovals remain the issuance and decision owners respectively.

mod service_parent;

use crate::{
    ConnectError,
    application_operator::{review_application_controller_parent, review_application_setup_request},
    approval::{ApprovalPresenter, LocalApprovals},
    stdio::{Exchange, PromptBridge, PromptAnswer, present_approvals},
};
use std::{path::PathBuf, sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};
use toolkit_mobkit::{
    host_gateway::ToolkitGatewayHost,
    private_host::{PrivateHostClosed, RemoteControllerCommand, RemoteControllerReviews,
        RemoteControllerLogin, RemoteReviewDecision, RemoteReviewReply, RemoteReviewStatus},
};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

fn unavailable() -> ConnectError {
    ConnectError::InvalidInput("The protected application review is unavailable".into())
}
fn expiry(login: &RemoteControllerLogin) -> Result<SystemTime, ConnectError> {
    if login.scope_id.is_empty() || login.scope_id.len() > 512 { return Err(unavailable()); }
    let expires = UNIX_EPOCH.checked_add(Duration::from_millis(login.expires_at_ms))
        .ok_or_else(unavailable)?;
    if expires <= SystemTime::now() { return Err(unavailable()); }
    Ok(expires)
}

/// Run alongside the single existing gateway. The channel does not provide a
/// native principal: the selected operator view is checked before any prompt.
/// Only one protected review is retained here. A second begin is unavailable.
pub async fn serve_remote_operator(
    host: Arc<ToolkitGatewayHost>, requests: RemoteControllerReviews,
    artifact_parent: PathBuf, closed: PrivateHostClosed,
) -> Result<(), ConnectError> {
    serve(host, requests, artifact_parent, closed, None).await
}

/// Bundled final composition supplies this connection's actual endpoint. The
/// existing human-only entry remains available and cannot authorize services.
pub async fn serve_remote_operator_with_service_parent(
    host: Arc<ToolkitGatewayHost>, requests: RemoteControllerReviews,
    artifact_parent: PathBuf, closed: PrivateHostClosed,
    endpoint: toolkit_mobkit::private_host::ServiceParentEndpoint,
) -> Result<(), ConnectError> {
    serve(host, requests, artifact_parent, closed, Some(endpoint)).await
}
async fn serve(
    host: Arc<ToolkitGatewayHost>, requests: RemoteControllerReviews,
    artifact_parent: PathBuf, closed: PrivateHostClosed,
    endpoint: Option<toolkit_mobkit::private_host::ServiceParentEndpoint>,
) -> Result<(), ConnectError> {
    let mut requests = requests.into_receiver();
    let mut next_service = None;
    let mut service_id = 0u64;
    loop {
        let command = tokio::select! { biased;
            _ = closed.wait_closed() => {
                requests.close();
                while let Some(command) = requests.recv().await { command.refuse().await; }
                return Ok(());
            },
            command = requests.recv() => match command { Some(command) => command, None => return Ok(()) },
        };
        let command = match command {
            RemoteControllerCommand::ServiceParent(toolkit_mobkit::private_host::ServiceParentCommand::Next { response }) => {
                if endpoint.is_none() { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
                else if next_service.is_none() { next_service = Some(response); }
                else { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
                continue;
            }
            RemoteControllerCommand::ServiceParent(toolkit_mobkit::private_host::ServiceParentCommand::Adopt { mut operation }) => {
                let Some(endpoint) = &endpoint else { operation.retire().await; continue; };
                let Some(id) = service_id.checked_add(1) else { operation.retire().await; continue; };
                service_id = id;
                service_parent::drive(id, operation, endpoint, &mut requests, &mut next_service, &closed).await;
                continue;
            }
            command => command,
        };
        let RemoteControllerCommand::Begin { purpose, review_id, submission, login, response } = command else {
            command.refuse().await;
            continue;
        };
        // Native preparation owns its actual work until return, including on
        // parent loss. It supplies a real pin and exits the identity lease
        // without input admission before the human interaction begins.
        let result = async {
            let expires = expiry(&login)?;
            let prepared = host.prepare_local_controller_setup(submission).await
                .map_err(|error| ConnectError::native("prepare application controller review", error))?;
            host.application_operator_setup_for(&prepared)
                .map_err(|error| ConnectError::native("bind selected application operator", error))?;
            if closed.is_closed_for_presenter() { return Err(unavailable()); }
            let actor = meerkat_core::ApprovalPrincipalId::new("connect-application-operator")
                .map_err(|error| ConnectError::native("identify protected decision presenter", error))?;
            let (approvals, presenter) = LocalApprovals::create_browser(&artifact_parent, actor, expires)?;
            approvals.bind_session(prepared.session_id())?;
            let review = async {
                match purpose {
                    toolkit_mobkit::private_host::ParentReviewKind::Controller => review_application_controller_parent(&host, prepared, &approvals, &login.scope_id).await,
                    toolkit_mobkit::private_host::ParentReviewKind::SetupRequest => review_application_setup_request(&host, prepared, &approvals, &login.scope_id).await,
                }
            };
            drive_review(review_id, expires, &approvals, presenter, review, &mut requests, closed.wait_closed()).await
        }.await;
        let _ = response.send(result.map_err(|_| HostLifetimeError::Unavailable));
    }
}

// This drives the existing PromptBridge projection rather than reconstructing
// an ApprovalRecord from wire data. The sole retained Exchange owns the answer.
async fn drive_review<T>(
    review_id: u64,
    expires: SystemTime,
    approvals: &LocalApprovals,
    presenter: ApprovalPresenter,
    review: impl std::future::Future<Output = Result<T, ConnectError>>,
    requests: &mut tokio::sync::mpsc::Receiver<RemoteControllerCommand>,
    closed: impl std::future::Future<Output = ()>,
) -> Result<T, ConnectError> {
    let (bridge, mut prompts, retire) = PromptBridge::channel();
    let presentation = present_approvals(presenter, bridge);
    tokio::pin!(presentation, review, closed);
    let lifetime = expires.duration_since(SystemTime::now()).map_err(|_| unavailable())?;
    let deadline = tokio::time::sleep(lifetime);
    tokio::pin!(deadline);
    let mut current: Option<Exchange> = None;
    let mut forwarded = false;
    let mut presentation_done = false;
    let mut retired_reply: Option<RemoteReviewReply> = None;
    let result = loop {
        tokio::select! { biased;
            result = &mut review => break Some(result),
            _ = &mut closed => break None,
            _ = &mut deadline => break None,
            _ = &mut presentation, if !presentation_done => {
                presentation_done = true;
                break None;
            }
            prompt = prompts.recv(), if current.is_none() && !forwarded => {
                let Some(prompt) = prompt else { break None; };
                current = Some(prompt);
            }
            command = requests.recv() => {
                let Some(command) = command else { break None; };
                match command {
                    RemoteControllerCommand::Observe { review_id: id, response } if id == review_id => {
                        let status = if let Some(prompt) = &current {
                            RemoteReviewStatus::Prompt { prompt: prompt.prompt.clone() }
                        } else if forwarded { RemoteReviewStatus::AnswerForwarded }
                        else { RemoteReviewStatus::AwaitingPrompt };
                        let _ = response.send(Ok(status));
                    }
                    RemoteControllerCommand::Answer { review_id: id, approval_id, decision, response } if id == review_id => {
                        let matches = !forwarded && current.as_ref().is_some_and(|prompt| {
                            prompt.prompt["approval_id"].as_str() == Some(approval_id.as_str())
                        });
                        let result = if matches {
                            let prompt = current.take().expect("matched retained prompt");
                            let choice = match decision {
                                RemoteReviewDecision::Approve => crate::stdio::wire::Decision::Approve,
                                RemoteReviewDecision::Deny => crate::stdio::wire::Decision::Deny,
                            };
                            if prompt.response.send(Ok(PromptAnswer::Approval { decision: choice })).is_ok() {
                                forwarded = true;
                                Ok(RemoteReviewStatus::AnswerForwarded)
                            } else { Err(HostLifetimeError::Unavailable) }
                        } else { Err(HostLifetimeError::Denied) };
                        let _ = response.send(result);
                    }
                    RemoteControllerCommand::Retire { review_id: id, response } if id == review_id => {
                        retired_reply = Some(response);
                        break None;
                    }
                    other => other.refuse().await,
                }
            }
        }
    };
    // Retire the decision invocation, then drop/join the sole presenter before
    // awaiting an undecided review. This releases queued one-shot senders too.
    // A decision already returned to issuance remains immutable and is joined.
    let retired = approvals.retire_checked();
    retire.send_replace(true);
    drop(current);
    if !presentation_done { let _ = presentation.await; }
    let result = match result { Some(result) => result, None => review.await };
    if let Some(response) = retired_reply {
        let status = if retired.is_ok() { Ok(RemoteReviewStatus::Retired) }
            else { Err(HostLifetimeError::Unavailable) };
        let _ = response.send(status);
    }
    retired?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::*;
    use tokio::sync::{mpsc, oneshot};

    fn request(session: SessionId) -> ApprovalRequest {
        ApprovalRequest {
            requester: ApprovalPrincipalId::new("requester").unwrap(),
            owner: ApprovalOwnerRef::Session { session_id: session },
            resource: ApprovalResourceRef { kind: ApprovalResourceKind::Runtime, id: ApprovalResourceId::new("controller") },
            proposed_action: ApprovalProposedAction { kind: ApprovalActionKind::Other,
                summary: "Allow this application to use the selected model".into(),
                body: Some(serde_json::json!({"actual":"retained proposal"})) },
            risk: ApprovalRisk::High, request_body: None,
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
            expires_at: Some((SystemTime::now() + Duration::from_secs(120)).into()),
            metadata: Default::default(), request_provenance: None,
        }
    }
    async fn observe(send: &mpsc::Sender<RemoteControllerCommand>, id: u64) -> RemoteReviewStatus {
        let (response, receive) = oneshot::channel();
        send.send(RemoteControllerCommand::Observe { review_id:id, response }).await.unwrap();
        receive.await.unwrap().unwrap()
    }
    #[tokio::test]
    async fn protected_exchange_preserves_native_request_and_rejects_wrong_prompt() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let directory = tempfile::tempdir().unwrap();
            let expires = SystemTime::now() + Duration::from_secs(120);
            let (approvals, presenter) = LocalApprovals::create_browser(directory.path(),
                ApprovalPrincipalId::new("fixed-presenter").unwrap(), expires).unwrap();
            let session = SessionId::new(); approvals.bind_session(&session).unwrap();
            let input = request(session);
            let review = approvals.review_access(input.clone());
            let (send, mut receive) = mpsc::channel(1);
            let driver = drive_review(12, expires, &approvals, presenter, review, &mut receive, std::future::pending());
            let parent = async {
                let prompt = loop {
                    if let RemoteReviewStatus::Prompt { prompt } = observe(&send,12).await { break prompt; }
                    tokio::task::yield_now().await;
                };
                assert_eq!(prompt["body"], input.proposed_action.body.clone().unwrap());
                let (response, received) = oneshot::channel();
                send.send(RemoteControllerCommand::Answer { review_id:12, approval_id:"other".into(),
                    decision:RemoteReviewDecision::Approve, response }).await.unwrap();
                assert!(matches!(received.await.unwrap(), Err(HostLifetimeError::Denied)));
                let (response, received) = oneshot::channel();
                send.send(RemoteControllerCommand::Answer { review_id:12,
                    approval_id:prompt["approval_id"].as_str().unwrap().into(),
                    decision:RemoteReviewDecision::Approve, response }).await.unwrap();
                assert!(matches!(received.await.unwrap().unwrap(), RemoteReviewStatus::AnswerForwarded));
            };
            let (result, ()) = tokio::join!(driver,parent);
            let record = result.unwrap();
            assert_eq!(record.status, ApprovalStatus::Approved);
            assert_eq!(record.owner, input.owner);
            assert_eq!(record.proposed_action, input.proposed_action);
        }).await.expect("protected review did not settle");
    }
    #[tokio::test]
    async fn parent_close_retires_queued_prompt_without_human_deadline() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let directory = tempfile::tempdir().unwrap();
            let (approvals,presenter) = LocalApprovals::create(directory.path()).unwrap();
            let session=SessionId::new(); approvals.bind_session(&session).unwrap();
            let review=approvals.review_access(request(session));
            let (_send, mut receive)=mpsc::channel(1);
            // Actual native request is first polled by drive_review; ready EOF
            // then retires and joins the sole queued presenter without an answer.
            assert!(drive_review(12,SystemTime::now()+Duration::from_secs(120),&approvals,
                presenter,review,&mut receive,std::future::ready(())).await.is_err());
        }).await.expect("parent close waited for the review deadline");
    }
    #[tokio::test]
    async fn exact_retirement_cannot_retire_another_review() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let directory=tempfile::tempdir().unwrap();
            let (approvals,presenter)=LocalApprovals::create(directory.path()).unwrap();
            let session=SessionId::new(); approvals.bind_session(&session).unwrap();
            let review=approvals.review_access(request(session));
            let (send,mut receive)=mpsc::channel(1);
            let driver=drive_review(7,SystemTime::now()+Duration::from_secs(120),&approvals,
                presenter,review,&mut receive,std::future::pending());
            let parent=async {
                let (response,received)=oneshot::channel();
                send.send(RemoteControllerCommand::Retire {review_id:8,response}).await.unwrap();
                assert!(received.await.unwrap().is_err());
                let (response,received)=oneshot::channel();
                send.send(RemoteControllerCommand::Retire {review_id:7,response}).await.unwrap();
                assert!(matches!(received.await.unwrap().unwrap(),RemoteReviewStatus::Retired));
            };
            let (result,())=tokio::join!(driver,parent);
            assert!(result.is_err());
        }).await.expect("retired review did not join");
    }
}
