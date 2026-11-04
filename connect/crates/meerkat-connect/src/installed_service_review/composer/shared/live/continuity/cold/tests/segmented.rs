//! Positive R5 settlement of real queued inputs from an exited HeadCanonical
//! owner. This is deliberately distinct from full routable-consent adoption:
//! the reused native owner fixture is Domain scoped and must still refuse the
//! final adoption constituent. It manufactures no input rows or receipts.
use super::*;
use crate::application_consent::{
    ApplicationConsentOwner, ConsentApplicationIdentity, ConsentAssociation,
    ConsentIssuerSelection, ConsentOwnerError, tests as issuer_fixture,
};
use meerkat_core::{
    ApprovalRecord, ApprovalStatus, Session, lifecycle::core_executor::BoundSessionCommit,
    session_store::PreparedHeadCanonicalMutation,
};
use meerkat_runtime::{
    Input, PreparedRuntimeSessionCommit, PromptInput, input_state::InputLifecycleState,
    service_ext::SessionServiceRuntimeExt,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::Stdio,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const CHILD: &str = "installed_service_review::composer::shared::live::continuity::cold::tests::segmented::positive_segmented_predecessor_child";
const ROOT_ENV: &str = "CONNECT_R5_SEGMENTED_PREDECESSOR_ROOT";
const READY: &str = "CONNECT_R5_SEGMENTED_READY=";
const EVENT_BOUND: Duration = Duration::from_secs(30);

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;
struct Native {
    service: Arc<Service>,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
    live: Arc<LiveSharedSetup>,
}
impl Native {
    async fn open(root: &Path) -> Self {
        // The exact production Connect realm constructor supplies co-located
        // segmented stores and hosting paths. No store profile is substituted.
        let storage = crate::RealmOptions {
            context_root: root.to_owned(),
            state_root: Some(root.join("realms")),
            user_config_root: Some(root.join("home")),
        }
        .resolve("r5-positive-segmented")
        .unwrap();
        let mut startup = crate::session::SessionStartup::open(storage).await.unwrap();
        let store = startup.persistence().await.unwrap().runtime_store();
        assert_eq!(
            store.session_persistence_profile(),
            RuntimeSessionPersistenceProfile::HeadCanonicalV1
        );
        let persistence = startup.persistence.take().unwrap();
        let live = super::super::super::tests::unbound_live();
        let (service, machine) = meerkat::build_persistent_service_with_runtime_adapter(
            startup.factory,
            startup.config,
            1,
            persistence,
        )
        .unwrap();
        let service = Arc::new(service);
        // No fallible fixture checks after creating the real owners. The
        // parent stores this value in its outer cleanup slot before binding.
        Self {
            service,
            machine,
            store,
            live,
        }
    }
    fn bind(&self, owner: &Arc<ApplicationConsentOwner>) {
        self.machine
            .install_consent_owner_loss_settlement(Arc::new(owner.approvals()))
            .unwrap();
        let spec = meerkat_mobkit::MobBootstrapSpec::new(
            meerkat_mob::MobDefinition::explicit(meerkat_mob::MobId::from("r5-segmented-control")),
            meerkat_mob::MobStorage::in_memory(),
            self.service.clone(),
        )
        .with_session_runtime_adapter(self.machine.clone())
        .unwrap();
        self.live.bind_consent_owner(Some(owner.clone())).unwrap();
        self.live.capture_native(&spec).unwrap();
        let captured = self.live.native_context().unwrap();
        assert!(Arc::ptr_eq(&captured.service, &spec.session_service));
        assert!(Arc::ptr_eq(captured.machine().unwrap(), &self.machine));
    }
    async fn close(&self, session: &SessionId) -> Result<(), String> {
        let removed = if let Some(witness) = self
            .machine
            .current_session_registration_witness(session)
            .await
        {
            self.machine
                .unregister_session_registration_until_terminal_if_current(&witness)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        } else {
            Ok(())
        };
        let service = self.service.try_shutdown().await.map_err(|e| e.to_string());
        let live = self
            .live
            .join_after_native_close(removed.is_ok() && service.is_ok())
            .await
            .map_err(|e| e.to_string());
        removed?;
        service?;
        live
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    issuer: ConsentIssuerSelection,
    identity: ConsentApplicationIdentity,
    association: ConsentAssociation,
    session: SessionId,
    queued: Vec<meerkat_runtime::input_state::StoredInputState>,
    pending: ApprovalRecord,
    approved: ApprovalRecord,
}

/// Actual old process exits without Rust disposal only on the parent's byte.
/// This retains genuine native queued rows and unspent issued consent on disk.
#[tokio::test]
#[ignore = "private child of positive segmented R5 control"]
async fn positive_segmented_predecessor_child() {
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("parent-selected isolated realm"));
    let identity = issuer_fixture::identity();
    let owner = ApplicationConsentOwner::create(&root, identity.clone()).unwrap();
    let native = Native::open(&root).await;
    native.bind(&owner);
    let session = Session::new();
    let id = session.id().clone();
    let claim = native.machine.grant_session_hosting(&id).unwrap();
    assert!(claim.is_cross_process());
    // This is an ordinary production initial document commit, with no run or
    // input receipt. Runtime admission below creates every queued input row.
    let initial = PreparedHeadCanonicalMutation::prepare(&session, None).unwrap();
    native
        .store
        .commit_prepared_session_boundary(
            &LogicalRuntimeId::for_session(&id),
            PreparedRuntimeSessionCommit::snapshot_only(
                BoundSessionCommit::head_canonical_from_session(&session, initial).unwrap(),
            ),
        )
        .await
        .unwrap();
    native.machine.register_session(id.clone()).await.unwrap();
    let selection_path = root.join("native-association.json");
    let selected = toolkit_package::canonical_json(
        &json!({"kind":"real-segmented-owner-control","session":id}),
    )
    .unwrap();
    let artifact = issuer_fixture::artifact(&selection_path, &selected);
    let association = owner.bind_association(artifact, &claim).unwrap();
    let context = issuer_fixture::context(&owner, &claim);
    let probe = Arc::new(issuer_fixture::Probe::default());
    let pending = issuer_fixture::request(&probe, &context, "segmented-pending").await;
    let approved = issuer_fixture::request(&probe, &context, "segmented-approved").await;
    issuer_fixture::approve(&owner, &approved);
    for prompt in ["first real abandoned input", "second real abandoned input"] {
        let input = Input::Prompt(PromptInput::new(prompt, None));
        assert!(
            native
                .machine
                .accept_input(&id, input)
                .await
                .unwrap()
                .is_accepted()
        );
    }
    let queued = native
        .store
        .load_input_states_strict(&LogicalRuntimeId::for_session(&id))
        .await
        .unwrap();
    assert_eq!(queued.len(), 2);
    assert!(
        queued
            .iter()
            .all(|row| row.seed.phase == InputLifecycleState::Queued
                && row.seed.terminal_outcome.is_none())
    );
    assert!(
        native
            .machine
            .current_executor_attachment_witness(&id)
            .await
            .is_none()
    );
    assert_eq!(probe.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    let ready = Ready {
        issuer: owner.selection().clone(),
        identity,
        association,
        session: id,
        queued,
        pending: owner.approvals().get(&pending).unwrap(),
        approved: owner.approvals().get(&approved).unwrap(),
    };
    assert_eq!(ready.pending.status, ApprovalStatus::Pending);
    assert_eq!(ready.approved.status, ApprovalStatus::Approved);
    for row in [&ready.pending, &ready.approved] {
        let consent = row.consent.as_ref().unwrap();
        assert!(consent.withdrawal.is_none() && consent.consumed_at.is_none());
        assert!(
            !consent.routable(),
            "this owner control cannot claim native notice delivery"
        );
    }
    println!("{READY}{}", serde_json::to_string(&ready).unwrap());
    std::io::stdout().flush().unwrap();
    let mut released = [0];
    std::io::stdin().read_exact(&mut released).unwrap();
    assert_eq!(released, [1]);
    std::process::exit(77);
}

#[tokio::test]
async fn segmented_r5_settles_real_exited_owner_and_repeats_without_new_convergence() {
    let temp = crate::application_consent::tests::private_root();
    let root = temp.path().canonicalize().unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            CHILD,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROOT_ENV, &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut runtime: Option<Native> = None;
    let mut retained: Option<Arc<tokio::sync::Mutex<ColdState>>> = None;
    let mut selected_session = None;
    let caught = AssertUnwindSafe(async {
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let ready: Ready = tokio::time::timeout(EVENT_BOUND, async {
            loop {
                let line = lines.next_line().await.unwrap().expect("old native owner exited before readiness");
                if let Some(index) = line.find(READY) { break serde_json::from_str(&line[index+READY.len()..]).unwrap(); }
            }
        }).await.expect("actual old-owner readiness");
        selected_session = Some(ready.session.clone());
        assert!(matches!(ApplicationConsentOwner::reopen(ready.issuer.clone(), &ready.identity), Err(ConsentOwnerError::OwnerHeld)));
        child.stdin.take().unwrap().write_all(&[1]).await.unwrap();
        let status = tokio::time::timeout(EVENT_BOUND,child.wait()).await.unwrap().unwrap();
        assert_eq!(status.code(),Some(77),"actual old process exit, not Rust scope disposal");
        let owner = ApplicationConsentOwner::reopen(ready.issuer.clone(), &ready.identity).unwrap();
        assert_eq!(owner.approvals().get(&ready.pending.approval_id).unwrap(),ready.pending);
        assert_eq!(owner.approvals().get(&ready.approved.approval_id).unwrap(),ready.approved);
        runtime = Some(Native::open(&root).await); let native = runtime.as_ref().unwrap();
        native.bind(&owner);
        let reopened = native.store.load_input_states_strict(&LogicalRuntimeId::for_session(&ready.session)).await.unwrap();
        assert_eq!(reopened.len(),ready.queued.len());
        for previous in &ready.queued {
            let actual = reopened.iter().find(|row|row.state.input_id==previous.state.input_id).unwrap();
            assert_eq!(serde_json::to_value(actual).unwrap(),serde_json::to_value(previous).unwrap(),
                "the actual old queued row survived process exit unchanged before R5");
        }
        assert!(native.machine.current_session_registration_witness(&ready.session).await.is_none());
        let claim = native.machine.grant_session_hosting(&ready.session).unwrap();
        assert!(claim.is_cross_process() && claim.completed_owner_loss().is_none());
        owner.bind_association(ready.association.artifact().clone(),&claim).unwrap();
        retained = Some(Arc::new(tokio::sync::Mutex::new(ColdState { claim:Some(claim), ..Default::default() })));
        let state = retained.as_ref().unwrap();
        let captured = native.live.native_context().unwrap();
        let first = state.lock().await.observe_native(&captured,&ready.session,
            Instant::now().checked_add(Duration::from_secs(30)).unwrap()).await.unwrap();
        let first = first.as_ref().as_ref().expect("real segmented R5 settlement must succeed");
        assert_eq!(first.session_id,ready.session); assert_eq!(first.durable_rows_closed,2);
        assert!(first.terminals_owed.is_empty() && first.migrated_without_receipt.is_empty());
        let Some(PredecessorOwnerLoss::Completed(proof)) = &first.owner_loss else { panic!("first real claim must complete the actual issuer settlement") };
        assert_eq!(state.lock().await.claim.as_ref().unwrap().completed_owner_loss().as_ref(),Some(proof));
        assert_eq!(first.inputs.len(),2);
        assert!(first.inputs.iter().all(|input| matches!(&input.settled_by,meerkat_runtime::SettledBy::Ended)));
        for input in &first.inputs {
            assert_eq!(input.receipt.scope(), &InputTerminalReceiptScope::RuntimeTermination);
            assert!(input.receipt.run_id().is_none());
            assert_eq!(input.receipt.outcome().class(), CompletionClass::RuntimeTerminated);
            assert_eq!(retained_receipt_scope(input.receipt.scope()).unwrap(),
                json!({"type":"runtime_termination"}));
            assert!(unknown_outcome(&input.receipt).unwrap().is_none(),
                "ending a real queued input is not the receiptless consumed OutcomeUnknown case");
        }
        let baseline: HashSet<_> = ready.queued.iter().map(|row|row.state.input_id.clone()).collect();
        let (ids,census) = cold_session_census(&native.machine,&ready.session,&baseline,first).await.unwrap();
        assert_eq!(ids,baseline); assert_eq!(census.len(),2);
        assert!(census.iter().all(|row|row["kind"] == "finalized" && row["outcome_unknown"].is_null()));
        assert!(census.iter().all(|row|row["scope"] == json!({"type":"runtime_termination"})
            && row["run_id"].is_null() && row["completion_class"] == "RuntimeTerminated"),
            "the production census preserves native runless termination, not a run or unknown outcome");
        for old in [&ready.pending,&ready.approved] {
            let row = owner.approvals().get(&old.approval_id).unwrap();
            let consent = row.consent.as_ref().unwrap();
            assert!(consent.withdrawal.as_ref().is_some_and(|value| value.reason == meerkat_core::approval::ApprovalConsentWithdrawalReason::Abandoned));
            assert!(consent.consumed_at.is_none());
            assert_eq!(row.status,if old.status==ApprovalStatus::Pending {ApprovalStatus::Cancelled} else {ApprovalStatus::Approved});
        }
        let second = state.lock().await.observe_native(&captured,&ready.session,
            Instant::now().checked_add(Duration::from_secs(30)).unwrap()).await.unwrap();
        let second = second.as_ref().as_ref().expect("repeat on same claim must converge without replacement");
        assert!(second.inputs.is_empty()); assert_eq!(second.durable_rows_closed,2);
        assert!(matches!(&second.owner_loss,Some(PredecessorOwnerLoss::NotOwed(repeated)) if repeated==proof));
        let (again,reports) = cold_session_census(&native.machine,&ready.session,&baseline,second).await.unwrap();
        assert_eq!(again,ids); assert_eq!(reports,census);
        assert!(native.machine.current_session_registration_witness(&ready.session).await.is_none());
        let held = state.lock().await;
        assert_eq!(held.native.len(),2);
        assert!(owner.observe_abandoned_after_native(&ready.association,held.claim.as_ref().unwrap(),proof,&ids).is_err(),
            "nonroutable owner rows are not full consent-delivery or physical adoption evidence");
        assert!(!held.adoption_claimed && held.physical.is_none());
    }).catch_unwind().await;
    // The old OS process and current service are joined before rethrowing any
    // assertion. Native R5 owns its bounded operation; no new physical setup
    // is started by this intentionally incomplete aggregate control.
    let mut failures = Vec::new();
    if child.try_wait().map(|s| s.is_none()).unwrap_or(true) {
        let _ = child.start_kill();
        failures.push("old owner needed forced cleanup");
    }
    if child.wait().await.is_err() {
        failures.push("old owner reap failed");
    }
    if let (Some(native), Some(session)) = (&runtime, &selected_session)
        && native.close(session).await.is_err()
    {
        failures.push("native fixture close failed");
    }
    drop(retained);
    drop(runtime);
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    assert!(failures.is_empty(), "{failures:?}");
}
