//! Actual private-pipe login lifetime plus the existing synthetic package.
//! This does not supply a verified Human, canonical task or install authority.
use super::*;
use futures_util::FutureExt;
use meerkat_toolkit::host_lifetime::control::*;
use std::{fs::File, os::fd::IntoRawFd};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use toolkit_mobkit::private_host::{InheritedHostPipes, PrivateHostConnection, ConsoleLoginHandle};

// Closed transport fixture only: component opens refuse. No native runtime,
// grant, source owner or policy is supplied by this test lifetime adapter.
struct PipeOwner(HostCloseHandle);
impl PipeOwner {
    fn observation(&self) -> HostCloseObservation {
        HostCloseObservation { handle: self.0.clone(), state: HostCloseState::Settled, failure: None }
    }
}
#[async_trait]
impl PrivateHostLifetimeControl for PipeOwner {
    fn validate_current_selection(&self) -> Result<(), HostLifetimeError> { Ok(()) }
    fn open_component(&self, _: &toolkit_package::Id) -> Result<HostComponentObservation, HostLifetimeError> { Err(HostLifetimeError::Denied) }
    fn begin_close_component(&self, _: &toolkit_package::Id, _: HostCloseReason) -> Result<HostCloseObservation, HostLifetimeError> { Err(HostLifetimeError::Denied) }
    fn begin_close_activation(&self, _: HostCloseReason) -> Result<HostCloseObservation, HostLifetimeError> { Ok(self.observation()) }
    fn observe_close(&self, handle: &HostCloseHandle) -> Result<HostCloseObservation, HostLifetimeError> {
        if handle != &self.0 { return Err(HostLifetimeError::WrongOwner); } Ok(self.observation())
    }
    async fn join_close(&self, handle: &HostCloseHandle) -> Result<HostCloseObservation, HostLifetimeError> { self.observe_close(handle) }
    fn host_lost(&self) -> Result<HostCloseObservation, HostLifetimeError> { Ok(self.observation()) }
}
struct LoginPipe {
    commands: tokio::fs::File,
    responses: BufReader<tokio::fs::File>,
    connection: PrivateHostConnection,
    login: ConsoleLoginHandle,
}
impl LoginPipe {
    async fn send(&mut self, id: u64, command: Value) {
        send(&mut self.commands, id, command).await;
    }
    async fn response(&mut self) -> Value {
        let mut line = String::new();
        assert!(self.responses.read_line(&mut line).await.unwrap() > 0);
        serde_json::from_str(&line).unwrap()
    }
    async fn new() -> Self {
        let (child_read, parent_write) = nix::unistd::pipe().unwrap();
        let (parent_read, child_write) = nix::unistd::pipe().unwrap();
        for fd in [&child_read, &parent_write, &parent_read, &child_write] {
            nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC)).unwrap();
        }
        let pipes = InheritedHostPipes::adopt(child_read.into_raw_fd(), child_write.into_raw_fd()).unwrap();
        let mut commands = tokio::fs::File::from_std(File::from(parent_write));
        let responses = BufReader::new(tokio::fs::File::from_std(File::from(parent_read)));
        let artifact = json!({"path":"/fixture/transport-only.json","sha256":"0".repeat(64)});
        send(&mut commands, 1, json!({"kind":"bootstrap", "selection":{
            "deployment_root":"/fixture", "activation":artifact, "policy_manifest":artifact, "local_policy":artifact}})).await;
        let (_, connection) = PrivateHostConnection::bootstrap(pipes, |_| Ok(PipeOwner(serde_json::from_value(json!({
            "incarnation":"00000000-0000-4000-8000-000000000001",
            "close_id":"00000000-0000-4000-8000-000000000002", "scope":{"kind":"activation"}
        })).unwrap()))).await.unwrap();
        // Bootstrap gives the live exact pipe owner before any login enrollment.
        let mut initial = responses;
        let mut line = String::new(); initial.read_line(&mut line).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["result"]["kind"], "bootstrap_accepted");
        let expires = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000;
        send(&mut commands, 2, json!({"kind":"enroll_console_login", "login":{
            "issuer":"fixture-issuer", "subject":"fixture-subject", "login_scope_id":"fixture-scope", "expires_at_ms":expires}})).await;
        line.clear(); initial.read_line(&mut line).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["result"]["kind"], "console_login_enrolled");
        let login = connection.console_logins().resolve("fixture-issuer", "fixture-subject", "fixture-scope").unwrap();
        Self { commands, responses: initial, connection, login }
    }
    async fn finish(self) { self.connection.finish().await.unwrap(); }
}
async fn send(writer: &mut tokio::fs::File, id: u64, command: Value) {
    let mut bytes = serde_json::to_vec(&json!({"version":1,"id":id,"command":command})).unwrap();
    bytes.push(b'\n'); writer.write_all(&bytes).await.unwrap(); writer.flush().await.unwrap();
}

#[tokio::test]
async fn actual_dispatch_loss_and_unknown_drain_login_without_clearing_uncertain_attempt() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for outcome in ["committed", "unknown", "dropped"] {
            let mut pipe = LoginPipe::new().await;
            let mut f = Fixture::local(false, if outcome == "unknown" { 5 } else { 2 });
            f.check().await;
            let run = f.install();
            let mut prompt = f.prompt().await;
            let approval = prompt.record.approval_id.clone();
            prompt.decide(ApprovalDecision::Approve).unwrap();
            f.package.entered.notified().await;
            // Mechanical custody injection only, after the actual synthetic
            // dispatcher entered. It does not exercise task/Human selection.
            f.approvals.shared.state().unwrap().attempt.as_mut().unwrap().service_entry = Some(pipe.login.reserve().unwrap());
            pipe.send(3, json!({"kind":"retire_console_login","enrollment_id":2})).await;
            assert_eq!(pipe.login.closed().await, toolkit_mobkit::private_host::ConsoleLoginRefusal::Retired);
            assert!(pipe.response().now_or_never().is_none(), "retirement must wait for this entered dispatch");
            if outcome == "dropped" {
                run.abort(); assert!(run.await.unwrap_err().is_cancelled());
            } else {
                f.package.release.notify_one();
                assert_eq!(run.await.unwrap().is_ok(), outcome == "committed");
            }
            let reply = pipe.response().await;
            assert_eq!(reply["id"], 3);
            assert_eq!(reply["result"]["observation"]["reservations_drained"], true);
            assert_eq!(reply["result"]["observation"]["uncertain"], outcome != "committed");
            let state = f.approvals.shared.state().unwrap();
            let attempt = state.attempt.as_ref().unwrap();
            assert!(attempt.service_entry.is_none());
            assert_eq!(attempt.custody, if outcome == "committed" { Custody::Settled(LiveBridgeEffectOutcome::Committed) } else { Custody::SpentUncertain });
            assert!(attempt.readback.is_some()); drop(state);
            assert_eq!(f.approvals.shared.service.get(&approval).unwrap().status, ApprovalStatus::Approved);
            assert!(f.install().await.unwrap().is_err()); assert_eq!(f.count(), 1);
            pipe.finish().await;
        }
    }).await.expect("dispatch reservation did not settle or drain");
}

#[tokio::test]
async fn exact_decision_refusal_does_not_record_or_wake_software_dispatch() {
    let mut f = Fixture::local(false, 0); f.check().await;
    let run = f.install(); let mut prompt = f.prompt().await;
    let id = prompt.record.approval_id.clone();
    assert!(prompt.decide_current(ApprovalDecision::Approve, Some(Arc::new(|| false))).is_err());
    assert_eq!(f.approvals.shared.service.get(&id).unwrap().status, ApprovalStatus::Pending);
    assert_eq!(f.count(), 0);
    // No synthetic Human/task was supplied, so the Console entry API refuses.
    assert!(prompt.decide_console(ApprovalDecision::Approve, Arc::new(|| true)).is_err());
    assert_eq!(f.approvals.shared.service.get(&id).unwrap().status, ApprovalStatus::Pending);
    f.approvals.retire(); drop(prompt); assert!(run.await.unwrap().is_err()); assert_eq!(f.count(), 0);
}


#[cfg(feature = "console")]
#[tokio::test]
async fn status_pages_are_bounded_by_actual_login_owner_and_stale_reads_cannot_publish() {
    use crate::application_gateway::console_setup::{replace_status_offer, StatusOffer};
    let mut first = LoginPipe::new().await;
    let second = LoginPipe::new().await;
    let result = std::panic::AssertUnwindSafe(async {
        let mut offers: Vec<StatusOffer<&str>> = Vec::new();
        // These two actual pipe owners deliberately use identical enrollment
        // labels. Presentation replacement must still keep them separate.
        assert!(!first.login.same_owner(&second.login));
        replace_status_offer(&mut offers, &first.login, None, Some((7, "first"))).unwrap();
        replace_status_offer(&mut offers, &second.login, None, Some((8, "second"))).unwrap();
        replace_status_offer(&mut offers, &first.login, None, Some((9, "first refreshed"))).unwrap();
        assert_eq!(offers.len(), 2);
        assert!(offers.iter().any(|offer| offer.id == 8 && offer.value == "second" && offer.login.same_owner(&second.login)));
        assert!(!offers.iter().any(|offer| offer.id == 7));
        // The same page owner exists even without an access policy. A read
        // that selected page 7 cannot replace page 9 after a newer read won.
        assert!(replace_status_offer(&mut offers, &first.login, Some(7), Some((11, "stale next"))).is_err());
        assert!(replace_status_offer(&mut offers, &second.login, Some(9), Some((12, "wrong login"))).is_err());
        assert_eq!(offers.iter().map(|offer| offer.id).collect::<Vec<_>>(), [8, 9]);
        replace_status_offer(&mut offers, &first.login, Some(9), Some((13, "current refresh"))).unwrap();
        assert_eq!(offers.iter().map(|offer| offer.id).collect::<Vec<_>>(), [8, 13]);
        replace_status_offer(&mut offers, &first.login, None, None).unwrap();
        assert_eq!(offers.len(), 1); assert_eq!(offers[0].id, 8);
        replace_status_offer(&mut offers, &first.login, None, Some((10, "first"))).unwrap();
        let expires = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000;
        let mut extra = Vec::new();
        for id in 3..=257_u64 {
            let scope = format!("offer-{id}");
            first.send(id, json!({"kind":"enroll_console_login", "login":{
                "issuer":"fixture-issuer", "subject":"fixture-subject", "login_scope_id":scope, "expires_at_ms":expires}})).await;
            assert_eq!(first.response().await["result"]["kind"], "console_login_enrolled");
            extra.push(first.connection.console_logins().resolve("fixture-issuer", "fixture-subject", &scope).unwrap());
        }
        for (index, login) in extra.iter().take(254).enumerate() {
            replace_status_offer(&mut offers, login, None, Some((100 + index as u64, "other"))).unwrap();
        }
        assert_eq!(offers.len(), 256);
        let before = offers.iter().map(|offer| offer.id).collect::<Vec<_>>();
        assert!(replace_status_offer(&mut offers, &extra[254], None, Some((500, "new"))).is_err());
        assert_eq!(offers.iter().map(|offer| offer.id).collect::<Vec<_>>(), before,
            "a poll at capacity cannot evict another current login's offer");
        replace_status_offer(&mut offers, &second.login, None, Some((501, "second refreshed"))).unwrap();
        assert_eq!(offers.len(), 256);
        first.send(258, json!({"kind":"retire_console_login", "enrollment_id":3})).await;
        assert_eq!(first.response().await["result"]["observation"]["reservations_drained"], true);
        replace_status_offer(&mut offers, &extra[254], None, Some((502, "new"))).unwrap();
        assert_eq!(offers.len(), 256);
        assert!(!offers.iter().any(|offer| offer.login.same_owner(&extra[0])));
        assert!(offers.iter().any(|offer| offer.id == 501 && offer.login.same_owner(&second.login)));
    }).catch_unwind().await;
    // Join both actual private-pipe owners even when a presentation assertion
    // fails. No native Human, grant or application policy was synthesized.
    first.finish().await; second.finish().await;
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}
