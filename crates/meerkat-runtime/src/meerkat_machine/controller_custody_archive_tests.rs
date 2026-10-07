//! Source tests of generated run custody and existing post-commit archival.
//! No snapshot is presented as authenticated cold recovery. The ingress/client
//! fixtures supply trusted test inputs; they never authorize model operations.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::input::{Input, PromptInput};
use crate::input_authority::{
    NativeIngressContext,
    tests::{TestIngress, input},
};
use crate::input_state::{InputAbandonReason, InputStatePersistenceRecord, StoredInputState};
use crate::meerkat_machine::dsl as mm;
use crate::store::{InMemoryRuntimeStore, RuntimeStore};
use crate::traits::RuntimeDriver;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::{ControllerModelClient, ControllerModelSelection, InputId, RunId};

pub(super) struct SelectedClient(pub(super) ControllerModelSelection);
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::AgentLlmClient for SelectedClient {
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(self.0.clone())
    }
    async fn stream_response(
        &self,
        _: &[meerkat_core::Message],
        _: &[Arc<meerkat_core::ToolDef>],
        _: u32,
        _: Option<f32>,
        _: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
        panic!("archive fixture must never dispatch a model")
    }
    fn provider(&self) -> meerkat_core::Provider {
        self.0.provider()
    }
    fn model(&self) -> &str {
        self.0.model()
    }
}

type Driver = Arc<crate::tokio::sync::Mutex<DriverEntry>>;
struct Staged {
    driver: Driver,
    session_id: SessionId,
    runtime_id: crate::LogicalRuntimeId,
    input_id: InputId,
    run_id: RunId,
    grant: Option<GrantLineageRef>,
}

async fn register_and_stage(machine: &MeerkatMachine, governed: bool) -> Staged {
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("native registration");
    let (driver, runtime_id) = {
        let sessions = machine.sessions.read().await;
        let entry = sessions.get(&session_id).expect("native session");
        (Arc::clone(&entry.driver), entry.runtime_id.clone())
    };
    let (prompt, grant) = if governed {
        let mut prompt = input("caller");
        let original = Arc::clone(
            prompt
                .header()
                .ingress_context
                .as_ref()
                .expect("actual ingress"),
        );
        let mut candidate = prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        candidate.target.logical_runtime =
            EvidenceId::new(runtime_id.to_string()).expect("runtime");
        let grant = candidate.controller_grant_lineage[0].clone();
        let selection = candidate
            .controller_model
            .clone()
            .expect("controller route");
        prompt.header_mut().authority_association =
            Some(InputAuthorityAssociation::new(candidate).expect("claims"));
        let ingress = NativeIngressContext::from_trusted_ingress(
            &prompt,
            original.requester().clone(),
            original.ingress_actor().clone(),
            original.realm().clone(),
            original.authentication().clone(),
        )
        .expect("final submission")
        .with_controller_client(
            &prompt,
            ControllerModelClient::new(selection.clone(), Arc::new(SelectedClient(selection))),
        )
        .expect("retained actual fixture client");
        (
            prompt.with_ingress_context(ingress).expect("exact ingress"),
            Some(grant),
        )
    } else {
        (Input::Prompt(PromptInput::new("legacy work", None)), None)
    };
    let input_id = prompt.id().clone();
    let run_id = RunId::new();
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless test")
        };
        driver.set_executor_work_authorization_support(true);
        assert!(
            driver
                .accept_input(prompt)
                .await
                .expect("actual native admission")
                .is_accepted()
        );
        driver
            .contract_begin_run_authority(run_id.clone())
            .expect("generated Prepare");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![input_id.clone()],
                    run_id.clone(),
                ),
            )
            .expect("generated StageForRun");
    }
    Staged {
        driver,
        session_id,
        runtime_id,
        input_id,
        run_id,
        grant,
    }
}

fn exact_abandoned_archive(row: &StoredInputState) -> mm::MeerkatMachineInput {
    assert!(matches!(
        row.seed.terminal_outcome,
        Some(crate::input_state::InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Stopped
        })
    ));
    mm::MeerkatMachineInput::ArchiveTerminalInput {
        input_id: row.state.input_id.to_string(),
        phase: mm::InputPhase::Abandoned,
        terminal_kind: mm::InputTerminalKind::Abandoned,
        superseded_by: None,
        aggregate_id: None,
        abandon_reason: Some(mm::InputAbandonReason::Stopped),
        abandon_attempt_count: Some(u64::from(row.seed.attempt_count)),
        attempt_count: u64::from(row.seed.attempt_count),
        run_id: row.seed.last_run_id.as_ref().map(mm::RunId::from_domain),
        boundary_sequence: row.seed.last_boundary_sequence,
        admission_sequence: row.seed.admission_sequence,
        idempotency_key: row.state.idempotency_key.as_ref().map(|key| key.0.clone()),
    }
}

#[derive(Clone, Copy)]
enum Ending {
    Completed,
    Failed,
    Cancelled,
}
impl Ending {
    fn input(self, run: &RunId) -> mm::MeerkatMachineInput {
        let run_id = mm::RunId::from_domain(run);
        match self {
            Self::Completed => mm::MeerkatMachineInput::RunCompleted { run_id },
            Self::Cancelled => mm::MeerkatMachineInput::RunCancelled { run_id },
            Self::Failed => mm::MeerkatMachineInput::RunFailed {
                run_id,
                runtime_apply_failure_cause: None,
                runtime_apply_failure_message: None,
                machine_terminal_failure_observed: false,
                terminal_failure_source: None,
                error: "fixture physical run failure".into(),
            },
        }
    }
}

async fn finish(staged: &Staged, ending: Ending) {
    let locked = staged.driver.lock().await;
    let authority = locked.shared_dsl_authority();
    let mut authority = authority.lock().expect("generated owner");
    mm::MeerkatMachineMutator::apply(&mut *authority, ending.input(&staged.run_id))
        .expect("actual generated run terminal");
    assert_eq!(
        authority.state().lifecycle_phase,
        mm::MeerkatPhase::Running,
        "run terminal witness, not lifecycle-only heuristic, closes custody"
    );
    assert_eq!(
        authority.state().turn_terminal_run_id,
        Some(mm::RunId::from_domain(&staged.run_id))
    );
}

#[tokio::test]
async fn abandoned_governed_original_keeps_binding_until_each_actual_run_terminal() {
    for ending in [Ending::Completed, Ending::Failed, Ending::Cancelled] {
        let machine = MeerkatMachine::ephemeral();
        let host = Arc::new(TestIngress::new(machine.generated_auth_lease_handle()));
        let machine = machine
            .with_native_work_authorization_host(host)
            .expect("host");
        let staged = register_and_stage(&machine, true).await;
        let grant = staged.grant.as_ref().expect("controller");
        let store = InMemoryRuntimeStore::new();
        {
            let mut locked = staged.driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *locked else {
                panic!("storeless")
            };
            assert_eq!(
                driver
                    .abandon_all_non_terminal(InputAbandonReason::Stopped)
                    .expect("abandon delivery"),
                1
            );
            let row = driver
                .stored_input_state(&staged.input_id)
                .expect("terminal original");
            // Execute the actual existing row transaction before its cleanup.
            store
                .persist_input_states_atomically(
                    &staged.runtime_id,
                    &[
                        InputStatePersistenceRecord::from_machine_snapshot(row.clone())
                            .expect("native seed"),
                    ],
                )
                .await
                .expect("existing memory-store commit");
            let authority = driver.shared_dsl_authority();
            {
                let mut authority = authority.lock().expect("generated owner");
                let before = authority.state().clone();
                let refused = mm::MeerkatMachineMutator::apply(
                    &mut *authority,
                    exact_abandoned_archive(&row),
                );
                assert!(matches!(
                    refused,
                    Err(mm::MeerkatMachineTransitionError::GuardRejected { .. })
                ));
                assert_eq!(
                    authority.state(),
                    &before,
                    "rejection must erase no binding"
                );
            }
            driver
                .archive_archivable_terminal_inputs_after_durable_commit(std::slice::from_ref(
                    &staged.input_id,
                ))
                .expect("post-commit helper defers generated refusal");
            assert!(driver.ledger().get(&staged.input_id).is_some());
            assert!(
                authority
                    .lock()
                    .expect("owner")
                    .state()
                    .input_authority_bindings
                    .contains_key(&staged.input_id.to_string())
            );
            assert!(
                driver
                    .unfinished_work_references_controller(grant)
                    .expect("actual run remains")
            );
        }
        {
            let mut invoked = false;
            let mut custody = machine
                .try_controller_grant_mutation()
                .expect("native custody");
            assert_eq!(
                custody.with_unreferenced_controller_grant(grant, || {
                    invoked = true;
                    Ok::<_, ()>(())
                }),
                Err(ControllerCustodyRefusal::ControllerInUse)
            );
            assert!(!invoked);
        }
        finish(&staged, ending).await;
        {
            let mut custody = machine
                .try_controller_grant_mutation()
                .expect("terminal native custody");
            assert_eq!(
                custody.with_unreferenced_controller_grant(grant, || Ok::<_, ()>(7)),
                Ok(Ok(7))
            );
        }
        {
            let mut locked = staged.driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *locked else {
                panic!("storeless")
            };
            driver
                .archive_archivable_terminal_inputs_after_durable_commit(std::slice::from_ref(
                    &staged.input_id,
                ))
                .expect("existing exact cleanup after run terminal");
            assert!(driver.ledger().get(&staged.input_id).is_none());
            let authority = driver.shared_dsl_authority();
            let authority = authority.lock().expect("owner");
            let key = staged.input_id.to_string();
            assert!(!authority.state().input_phases.contains_key(&key));
            assert!(
                !authority
                    .state()
                    .input_authority_bindings
                    .contains_key(&key)
            );
            assert!(
                !authority
                    .state()
                    .input_authority_batch_keys
                    .contains_key(&key)
            );
        }
        assert!(
            store
                .load_input_state(&staged.runtime_id, &staged.input_id)
                .await
                .expect("durable history")
                .is_some(),
            "live archival does not erase historical row"
        );
    }
}

#[tokio::test]
async fn ungoverned_terminal_original_keeps_existing_archive_behavior_during_run() {
    let machine = MeerkatMachine::ephemeral();
    let staged = register_and_stage(&machine, false).await;
    let mut locked = staged.driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    driver
        .abandon_all_non_terminal(InputAbandonReason::Stopped)
        .expect("abandon delivery");
    driver
        .archive_archivable_terminal_inputs_after_durable_commit(std::slice::from_ref(
            &staged.input_id,
        ))
        .expect("unchanged ungoverned cleanup");
    assert!(driver.ledger().get(&staged.input_id).is_none());
    let authority = driver.shared_dsl_authority();
    let authority = authority.lock().expect("owner");
    assert_eq!(
        authority.state().current_run_id,
        Some(mm::RunId::from_domain(&staged.run_id))
    );
    assert_eq!(authority.state().turn_terminal_run_id, None);
}

#[tokio::test]
async fn controller_custody_checks_all_attached_runs_not_only_one_terminal_driver() {
    let machine = MeerkatMachine::ephemeral();
    let host = Arc::new(TestIngress::new(machine.generated_auth_lease_handle()));
    let machine = machine
        .with_native_work_authorization_host(host)
        .expect("host");
    let first = register_and_stage(&machine, true).await;
    let second = register_and_stage(&machine, true).await;
    assert_eq!(first.grant, second.grant);
    for staged in [&first, &second] {
        let mut locked = staged.driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver
            .abandon_all_non_terminal(InputAbandonReason::Stopped)
            .expect("abandon delivery");
    }
    finish(&first, Ending::Completed).await;
    let grant = first.grant.as_ref().expect("shared controller");
    {
        let mut custody = machine
            .try_controller_grant_mutation()
            .expect("all attached owners");
        assert_eq!(
            custody.with_unreferenced_controller_grant(grant, || Ok::<_, ()>(())),
            Err(ControllerCustodyRefusal::ControllerInUse)
        );
    }
    finish(&second, Ending::Cancelled).await;
    let mut custody = machine
        .try_controller_grant_mutation()
        .expect("both run terminals");
    assert_eq!(
        custody.with_unreferenced_controller_grant(grant, || Ok::<_, ()>(())),
        Ok(Ok(()))
    );
}

#[tokio::test]
async fn actual_cold_registration_settles_old_run_before_omitting_closed_terminal_original() {
    let machine = MeerkatMachine::ephemeral();
    let host = Arc::new(TestIngress::new(machine.generated_auth_lease_handle()));
    let machine = machine
        .with_native_work_authorization_host(host)
        .expect("host");
    let staged = register_and_stage(&machine, true).await;
    let store = Arc::new(InMemoryRuntimeStore::new());
    {
        let mut locked = staged.driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless source")
        };
        driver
            .abandon_all_non_terminal(InputAbandonReason::Stopped)
            .expect("abandon delivery");
        let row = driver
            .stored_input_state(&staged.input_id)
            .expect("deferred original");
        assert!(!row.state.authority_contributors.is_empty());
        assert!(!crate::store::input_state_is_pending_terminal_owner(
            &row.state
        ));
        // Persist actual generated Running binding and this exact terminal row
        // together. The store fixture models the bytes surviving process loss.
        let lifecycle = crate::store::MachineLifecycleCommit::new_with_binding(
            driver.runtime_state(),
            driver.machine_lifecycle_binding_facts(),
            driver.supervisor_authority_snapshot(),
        );
        store
            .commit_machine_lifecycle(
                &staged.runtime_id,
                lifecycle,
                &[InputStatePersistenceRecord::from_machine_snapshot(row).expect("native seed")],
            )
            .await
            .expect("existing lifecycle/input transaction");
        driver
            .archive_archivable_terminal_inputs_after_durable_commit(std::slice::from_ref(
                &staged.input_id,
            ))
            .expect("live cleanup defers while exact run unfinished");
        assert!(driver.ledger().get(&staged.input_id).is_some());
        assert!(
            driver
                .unfinished_work_references_controller(staged.grant.as_ref().expect("grant"))
                .expect("old active reference")
        );
    }
    let Staged {
        driver,
        session_id,
        runtime_id,
        input_id,
        run_id,
        grant,
    } = staged;
    drop(driver);
    drop(machine);
    assert_eq!(
        crate::store::load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .expect("old durable lifecycle"),
        Some(crate::RuntimeState::Running)
    );

    // Invoke the real cold registration convergence owner, not recover_from_state
    // on a hand-edited in-memory image. Its CAS closes the old runtime before
    // input recovery is allowed to omit quiescent terminal history.
    let fresh_epoch = meerkat_core::RuntimeEpochId::new();
    let recovered = crate::meerkat_machine::driver::reconcile_runtime_authority_for_cold_recovery(
        store.as_ref(),
        &runtime_id,
        &session_id,
        &fresh_epoch,
    )
    .await
    .expect("actual registration-authorized lifecycle convergence");
    assert_eq!(
        crate::store::load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .expect("converged durable lifecycle"),
        Some(crate::RuntimeState::Idle)
    );
    assert_eq!(
        recovered.authority.state().lifecycle_phase,
        mm::MeerkatPhase::Idle
    );
    assert_eq!(recovered.authority.state().current_run_id, None);
    assert_eq!(
        recovered.authority.state().active_runtime_epoch_id,
        Some(mm::RuntimeEpochId::from_domain(&fresh_epoch))
    );
    let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
    let mut cold = crate::driver::persistent::PersistentRuntimeDriver::new(
        runtime_id.clone(),
        store.clone(),
        blobs,
    );
    cold.inner_mut()
        .replace_runtime_authority(recovered.authority);
    cold.recover_inputs_after_runtime_authority(recovered.unregister_progress.as_ref())
        .await
        .expect("actual exact-set input recovery after lifecycle convergence");
    assert!(
        cold.inner_ref().ledger().get(&input_id).is_none(),
        "quiescent terminal history does not become active work"
    );
    assert!(
        !cold
            .inner_ref()
            .unfinished_work_references_controller(grant.as_ref().expect("grant"))
            .expect("no active recovered reference")
    );
    assert!(
        cold.inner_ref()
            .batch_work_authorization(&run_id, std::slice::from_ref(&input_id))
            .is_err(),
        "historical association cannot reconstruct active controller custody"
    );
    let retained = store
        .load_input_state(&runtime_id, &input_id)
        .await
        .expect("history")
        .expect("protected row retained");
    assert!(!retained.state.authority_contributors.is_empty());
    assert!(
        retained.state.controller_client.is_none(),
        "no process client restored from storage"
    );
}
