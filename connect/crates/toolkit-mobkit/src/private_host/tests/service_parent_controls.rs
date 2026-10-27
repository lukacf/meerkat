//! These controls exercise the actual private framing/forwarding owner. The
//! ProbeOwner and final observation are transport fixtures, not native grants.
use super::*;
use serde_json::Value;
use crate::private_originals::{ServiceParentOperationObservation, ServiceParentOperationOutcome,
    ServiceParentOperationReason, ServiceParentCustody};

#[test]
fn service_result_and_retire_cannot_supply_an_authorization_answer() {
    for kind in ["result_service_parent_administration", "retire_service_parent_administration"] {
        let valid = serde_json::json!({"version":1,"id":2,"command":{"kind":kind,"exchange_id":9}});
        assert!(serde_json::from_value::<Request>(valid.clone()).is_ok());
        for field in ["decision", "operator", "approved", "provenance", "proposal_sha256"] {
            let mut value = valid.clone(); value["command"][field] = serde_json::json!(true);
            assert!(serde_json::from_value::<Request>(value).is_err());
        }
    }
    let standing = serde_json::json!({"kind":"standing_rule_decision","enrollment_id":7});
    assert!(serde_json::from_value::<ServiceParentDecisionSource>(standing).is_err());
    let login = serde_json::json!({"kind":"current_login_decision"});
    assert!(serde_json::from_value::<ServiceParentDecisionSource>(login).is_err());
}

#[tokio::test]
async fn service_control_waiters_remain_available_when_result_waiters_are_full() {
    let (pipes, command_file, response_file) = pipes();
    let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
    let mut responses = BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
    let (remote, mut actor) = mpsc::channel(1);
    let mut connection = None;
    let mut held = Vec::new();
    let scenario = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        send(&mut commands, 1, Command::Bootstrap { selection: selection() }).await;
        let (_, _, opened) = PrivateHostConnection::bootstrap_composed(pipes,
            |_| async { Ok((ProbeOwner::new(true), None)) }, None, Some(remote)).await
            .map_err(|error| error.to_string())?;
        connection = Some(opened);
        let mut line = String::new();
        responses.read_line(&mut line).await.map_err(|error| error.to_string())?;
        for id in 2..18 {
            send(&mut commands, id, Command::ResultServiceParentAdministration { exchange_id: 7 }).await;
            match actor.recv().await {
                Some(RemoteControllerCommand::ServiceParent(ServiceParentCommand::Result { exchange_id: 7, response })) => held.push(response),
                _ => return Err("exact result observation was not forwarded".into()),
            }
        }
        send(&mut commands, 18, Command::SubmitServiceParentAdministration { exchange_id: 7,
            proposal_sha256: "a".repeat(64), decision: ServiceParentDecision::Denied }).await;
        let forwarded = match actor.recv().await {
            Some(RemoteControllerCommand::ServiceParent(ServiceParentCommand::Submit {
                exchange_id: 7, proposal_sha256, decision: ServiceParentDecision::Denied, response })) => {
                let _ = response.send(Ok(())); proposal_sha256
            }
            _ => return Err("negative answer blocked behind result observers".into()),
        };
        line.clear(); responses.read_line(&mut line).await.map_err(|error| error.to_string())?;
        let ack: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        send(&mut commands, 19, Command::RetireServiceParentAdministration { exchange_id: 7 }).await;
        let retire = match actor.recv().await {
            Some(RemoteControllerCommand::ServiceParent(ServiceParentCommand::Retire { exchange_id: 7, response })) => response,
            _ => return Err("retirement blocked behind result observers".into()),
        };
        let observed = ServiceParentOperationObservation { outcome: Some(ServiceParentOperationOutcome::Uncertain),
            custody: ServiceParentCustody::Joined, reason: Some(ServiceParentOperationReason::Cancelled),
            parent_issuance_id: None, parent_issued: false, child: None, source: None, tool_policy: None };
        for response in held.drain(..) { let _ = response.send(Ok(observed.clone())); }
        let _ = retire.send(Ok(observed));
        let mut results = Vec::new();
        for _ in 0..17 {
            line.clear(); responses.read_line(&mut line).await.map_err(|error| error.to_string())?;
            results.push(serde_json::from_str::<Value>(&line).map_err(|error| error.to_string())?);
        }
        Ok::<_, String>((forwarded, ack, results))
    }).await;
    // Even a timed-out fixture releases every response waiter and joins the
    // actual connection before asserting. No fake worker drain is claimed.
    for response in held.drain(..) { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
    drop(actor); drop(commands);
    let cleanup = match connection {
        Some(connection) => Some(tokio::time::timeout(std::time::Duration::from_secs(3), connection.finish()).await),
        None => None,
    };
    drop(responses);
    let (digest, ack, results) = scenario.expect("private service control timeout").unwrap();
    cleanup.expect("actual connection").expect("actual connection cleanup timeout").unwrap();
    assert_eq!(digest, "a".repeat(64));
    assert_eq!(ack["id"], 18);
    assert_eq!(ack["result"]["kind"], "service_parent_administration_submitted");
    assert_eq!(results.len(), 17);
    for response in results {
        assert_eq!(response["result"]["exchange_id"], 7);
        assert_eq!(response["result"]["observation"]["outcome"], "uncertain");
        assert_eq!(response["result"]["observation"]["custody"], "joined");
    }
}

fn policy_review_frame() -> ServiceParentAdministrationRequest {
    use crate::private_originals::{PrivateAccessChoice, PrivateServiceMcpAccessDecision,
        PrivateServiceMcpPermission, PrivateSourceEvidence, ServiceParentAuthorizationProposal,
        ServiceToolPolicyProposal, ServiceToolPolicySelection, ServiceToolPolicyVersion, MemberPolicyLifetime};
    use meerkat_authorization_contracts::{constraints::ExecutionRestrictions, evidence::EvidenceId};
    use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId, PolicyDigest, PolicyProviderId, PolicyId, PolicyRevision};
    use meerkat_toolkit::{authority::{LifetimeScope, RoleLifetime}, host_lifetime::policy::ServiceParentAdministration};
    use toolkit_package::Id;
    // Typed serialization control only. These values are not an authenticated
    // proposal, a compiled native policy, a service parent or an approval.
    let original = controller_review_submission().original;
    let app = PrincipalRef::in_domain(PrincipalKind::ServiceAccount, "application", TrustDomainId::new("frame-test").unwrap()).unwrap();
    let version = ServiceToolPolicyVersion { revision: PolicyRevision(2), digest: PolicyDigest::from_canonical_bytes(b"frame") };
    let tool_policy = ServiceToolPolicyProposal {
        provider_id: PolicyProviderId::new("application").unwrap(), policy_id: PolicyId::new("selected").unwrap(),
        member: "original".into(), selection: ServiceToolPolicySelection { tools: vec![], catalog: Some(meerkat_core::CompiledToolConsequence::R0) },
        catalog_session: Some(meerkat_core::SessionId::new().to_string()), prior: version.clone(), candidate: version,
        candidate_canonical_json: "frame-control".into(), lifetime: MemberPolicyLifetime::UntilSuperseded,
    };
    let proposal = ServiceParentAuthorizationProposal {
        operator: app.clone(), rule: ServiceParentAdministration::ApplicationPolicyTasks { routes: [original.route.clone()].into() },
        issuance_id: EvidenceId::new("issuance").unwrap(), deployment: Id::new("deployment").unwrap(), source_policy_sha256: "a".repeat(64),
        original: original.clone(), acquisition: toolkit_store::StoreKey { realm: "realm".into(), owner: "deployment".into(), id: "request".into() },
        instruction_sha256: "b".repeat(64), expires_in_ms: Some(1), application: app, represented_subject: None,
        decision: PrivateServiceMcpAccessDecision {
            route: original.route, decision: Id::new("decision").unwrap(),
            evidence: PrivateSourceEvidence { resource_id: EvidenceId::new("evidence").unwrap(), revision: EvidenceId::new("1").unwrap(), sha256: "c".repeat(64) },
            choice: PrivateAccessChoice::Approve, permission: PrivateServiceMcpPermission {
                target: Id::new("original").unwrap(), connection: Id::new("service").unwrap(), resource: Id::new("service").unwrap(),
                operations: [Id::new("read").unwrap()].into(), lifetime: RoleLifetime { scope: LifetimeScope::Task { task: Id::new("task").unwrap() }, expires_at_ms: Some(1) },
            }, connection_revision: toolkit_package::Revision::new("d".repeat(64)).unwrap(), live_attachment: None, catalog: None,
        }, connector_selection: serde_json::Value::Null, restrictions: ExecutionRestrictions::unrestricted(), tool_policy: Some(tool_policy),
    };
    ServiceParentAdministrationRequest { exchange_id: 1, proposal_sha256: proposal.proposal_sha256().unwrap(), proposal }
}

#[tokio::test]
async fn unsupported_policy_review_frame_is_local_and_next_parent_reply_survives() {
    let supported = policy_review_frame();
    supported.validate_private_response().unwrap();
    // A complete review can exceed the former 64 KiB cap. This exercises
    // transport capacity only, not canonical policy validity or approval.
    let mut large = supported.clone();
    large.proposal.tool_policy.as_mut().unwrap().candidate_canonical_json = "x".repeat(128 * 1024);
    large.validate_private_response().unwrap();
    let large_reply = service_parent_response(2, Ok(Reply::ServiceParentAdministrationRequest {
        exchange_id: large.exchange_id, proposal_sha256: large.proposal_sha256, proposal: large.proposal,
    }));
    assert!(matches!(&large_reply.result, Reply::ServiceParentAdministrationRequest { .. }));
    let mut large_frame = Vec::new();
    write_response(&mut large_frame, &large_reply).await.unwrap();
    assert!(large_frame.len() > 64 * 1024);
    assert!(large_frame.len() <= MAX_FRAME_BYTES as usize);
    for numeric in [false, true] {
        let mut request = supported.clone();
        let policy = request.proposal.tool_policy.as_mut().unwrap();
        if numeric { policy.candidate.revision = meerkat_core::PolicyRevision(9_007_199_254_740_992); }
        else { policy.candidate_canonical_json = "x".repeat(MAX_FRAME_BYTES as usize); }
        assert!(matches!(request.validate_private_response(), Err(HostLifetimeError::Unavailable)));
        let bounded = service_parent_response(2, Ok(Reply::ServiceParentAdministrationRequest {
            exchange_id: request.exchange_id, proposal_sha256: request.proposal_sha256, proposal: request.proposal,
        }));
        assert!(matches!(&bounded.result, Reply::Error { error: WireError::Unavailable }));
        let next = service_parent_response(3, Ok(Reply::ServiceParentAdministrationSubmitted { exchange_id: 1 }));
        let mut writer = Vec::new();
        write_response(&mut writer, &bounded).await.unwrap(); write_response(&mut writer, &next).await.unwrap();
        let lines: Vec<_> = writer.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).collect();
        assert_eq!(lines.len(), 2); assert!(lines[0].len() + 1 <= MAX_FRAME_BYTES as usize);
        assert_eq!(lines[1], serde_json::to_vec(&next).unwrap());
    }
}
