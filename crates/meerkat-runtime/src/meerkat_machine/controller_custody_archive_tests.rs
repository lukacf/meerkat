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
    register_and_stage_with_ancestor(machine, governed, false).await
}

async fn register_and_stage_with_ancestor(
    machine: &MeerkatMachine,
    governed: bool,
    include_ancestor: bool,
) -> Staged {
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
        if include_ancestor {
            let mut ancestor = grant.clone();
            ancestor.grant_id = EvidenceId::new("controller-ancestor").expect("ancestor");
            candidate.controller_grant_lineage.insert(0, ancestor);
        }
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

#[cfg(all(
    feature = "sqlite-store",
    feature = "local-authorization",
    any(target_os = "macos", target_os = "linux", windows)
))]
#[tokio::test]
async fn sqlite_detached_terminal_original_vetoes_controller_mutation_until_cold_convergence() {
    use meerkat_authorization::publication::LocalAuthorizationPublication;
    use std::cell::Cell;

    let machine = MeerkatMachine::ephemeral();
    let host = Arc::new(TestIngress::new(machine.generated_auth_lease_handle()));
    let credential = host
        .input("caller")
        .header()
        .authority_association
        .as_ref()
        .expect("fixture claims")
        .candidate()
        .controller_model
        .as_ref()
        .expect("actual selected controller")
        .credential()
        .clone();
    let machine = machine
        .with_native_work_authorization_host(host)
        .expect("actual native ingress owner");
    let credentials = meerkat_auth_core::auth_store::TokenStoreBackend::Ephemeral
        .open_with_refresh_authority()
        .expect("actual coordinated credential store");
    let published = meerkat_auth_core::save_tokens_and_publish_lifecycle(
        credentials.clone(),
        machine.generated_auth_lease_handle(),
        credential.clone(),
        meerkat_core::auth::PersistedTokens::api_key("retained-controller-credential"),
    )
    .await
    .expect("publish the actual fixture credential before admitting work");
    let staged = register_and_stage_with_ancestor(&machine, true, true).await;
    let dir = tempfile::TempDir::new().expect("SQLite fixture directory");
    let path = dir.path().join("runtime.sqlite3");
    let store = crate::store::SqliteRuntimeStore::new_whole_blob(path.clone())
        .expect("actual SQLite runtime store");
    let lineage = {
        let mut locked = staged.driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("actual staged source")
        };
        assert_eq!(
            driver
                .abandon_all_non_terminal(InputAbandonReason::Stopped)
                .expect("stop input delivery without ending its run"),
            1
        );
        let row = driver
            .stored_input_state(&staged.input_id)
            .expect("actual terminal original");
        assert!(!crate::store::input_state_is_pending_terminal_owner(
            &row.state
        ));
        let lineage = row.state.authority_contributors[0]
            .association()
            .candidate()
            .controller_grant_lineage
            .clone();
        assert_eq!(lineage.len(), 2, "leaf and its retained ancestor");
        assert_eq!(driver.runtime_state(), crate::RuntimeState::Running);
        assert!(
            driver
                .unfinished_work_references_controller(&lineage[0])
                .expect("generated unfinished run binding")
        );
        store
            .commit_machine_lifecycle(
                &staged.runtime_id,
                crate::store::MachineLifecycleCommit::new_with_binding_run_and_unregister_progress(
                    driver.runtime_state(),
                    driver.machine_lifecycle_binding_facts(),
                    crate::store::MachineLifecycleRunFacts::new(
                        driver.current_run_id(),
                        driver.pre_run_phase().map(|phase| match phase {
                            crate::RuntimeState::Idle => {
                                crate::store::MachineLifecyclePreRunPhase::Idle
                            }
                            crate::RuntimeState::Attached => {
                                crate::store::MachineLifecyclePreRunPhase::Attached
                            }
                            crate::RuntimeState::Retired => {
                                crate::store::MachineLifecyclePreRunPhase::Retired
                            }
                            other => panic!("unexpected actual pre-run phase: {other:?}"),
                        }),
                    ),
                    driver.supervisor_authority_snapshot(),
                    None,
                ),
                &[InputStatePersistenceRecord::from_machine_snapshot(row)
                    .expect("actual generated input seed")],
            )
            .await
            .expect("atomic Running lifecycle and terminal original");
        lineage
    };
    let Staged {
        driver,
        session_id,
        runtime_id,
        input_id,
        ..
    } = staged;
    drop(driver);
    drop(machine);
    drop(store);

    let store = Arc::new(
        crate::store::SqliteRuntimeStore::new_whole_blob(path.clone())
            .expect("reopened physical SQLite store"),
    );
    let machine = MeerkatMachine::persistent_with_mode(store.clone(), None, true)
        .expect("actual governed physical execution claim");
    let host = Arc::new(TestIngress::new(machine.generated_auth_lease_handle()));
    let machine = machine
        .install_native_work_authorization(|_| host)
        .expect("native fixture ingress installation under governed custody");
    assert!(machine.sessions.read().await.is_empty());
    let read_bytes = || {
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("independent physical observation");
        let lifecycle: Vec<u8> = connection
            .query_row(
                "SELECT runtime_state_json FROM runtime_states WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| row.get(0),
            )
            .expect("physical lifecycle bytes");
        let input: Vec<u8> = connection
            .query_row(
                "SELECT state_json FROM runtime_input_states
                 WHERE runtime_id = ?1 AND input_id = ?2",
                rusqlite::params![runtime_id.to_string(), input_id.to_string()],
                |row| row.get(0),
            )
            .expect("physical original bytes");
        (lifecycle, input)
    };
    {
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("physical catalog observation");
        let catalog_count: u64 = connection
            .query_row("SELECT COUNT(*) FROM runtime_session_catalog", [], |row| {
                row.get(0)
            })
            .expect("catalog count");
        assert_eq!(
            catalog_count, 0,
            "detached rows cannot rely on catalog listing"
        );
    }
    let before = read_bytes();
    let lease = meerkat_core::handles::LeaseKey::from_credential_identity(&credential);
    let token_key = meerkat_core::auth::TokenKey::from_credential_identity(&credential);
    let auth = machine.generated_auth_lease_handle();
    meerkat_core::rehydrate_marked_tokens_for_status_for_identity(
        credentials.token_store().as_ref(),
        &auth,
        &credential,
        meerkat_core::auth::PersistedAuthMode::ApiKey,
        chrono::Utc::now(),
    )
    .await
    .expect("restore the actual marked credential in the fresh owner")
    .expect("retained credential");
    let lease_before = auth.snapshot(&lease);
    assert!(
        auth.release_lease(&lease).is_err(),
        "detached unfinished work vetoes release"
    );
    let clear = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
            credentials.clone(),
            auth.clone(),
            credential.clone(),
        ),
    )
    .await
    .expect("finite credential removal");
    assert!(
        matches!(
            clear,
            Err(meerkat_core::auth::CredentialMutationError::AuthLifecycle(
                _
            ))
        ),
        "actual coordinated removal must reach the native custody veto: {clear:?}"
    );
    let replace = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        meerkat_auth_core::save_tokens_and_publish_lifecycle(
            credentials.clone(),
            auth.clone(),
            credential.clone(),
            meerkat_core::auth::PersistedTokens::api_key("replacement-before-convergence"),
        ),
    )
    .await
    .expect("finite credential replacement");
    assert!(
        matches!(
            replace,
            Err(meerkat_core::auth::CredentialMutationError::AuthLifecycle(
                _
            ))
        ),
        "actual coordinated replacement must reach the native custody veto: {replace:?}"
    );
    assert_eq!(auth.snapshot(&lease), lease_before);
    assert_eq!(
        credentials.token_store().load(&token_key).await.unwrap(),
        Some(published)
    );
    assert_eq!(
        read_bytes(),
        before,
        "credential refusal changes no native rows"
    );
    let publication = LocalAuthorizationPublication::new();
    // This is the real publication primitive used as a callback-entry witness.
    // Synthetic TestIngress lineage does not prove LocalGrantAuthority revocation.
    for reference in &lineage {
        let entered = Cell::new(0);
        let ((), stamp) = publication.observe(|| ()).expect("before mutation");
        let result = machine
            .try_controller_grant_mutation()
            .and_then(|mut custody| {
                custody.with_unreferenced_controller_grant(reference, || {
                    entered.set(entered.get() + 1);
                    let _publication = publication
                        .begin_owner_change()
                        .expect("entered publication");
                    Ok::<_, ()>(())
                })
            });
        assert_eq!(
            entered.get(),
            0,
            "unfinished controller mutation must not enter"
        );
        assert_eq!(
            read_bytes(),
            before,
            "refusal must preserve exact stored bytes"
        );
        stamp
            .check_current()
            .expect("refusal must not publish a change");
        assert_eq!(
            result,
            Err(ControllerCustodyRefusal::ControllerInUse),
            "the detached unfinished run protects every controller ancestor"
        );
    }

    let mut unrelated = lineage[0].clone();
    unrelated.authority_namespace = EvidenceId::new("unrelated-namespace").expect("namespace");
    let entered = Cell::new(0);
    let result = machine
        .try_controller_grant_mutation()
        .and_then(|mut custody| {
            custody.with_unreferenced_controller_grant(&unrelated, || {
                entered.set(entered.get() + 1);
                Ok::<_, ()>(7)
            })
        });
    assert_eq!(
        result,
        Ok(Ok(7)),
        "complete scope does not veto an unrelated grant"
    );
    assert_eq!(entered.get(), 1);
    assert_eq!(read_bytes(), before);

    // Actual registration performs lifecycle convergence before input recovery;
    // the test neither patches a recovered image nor restores a process client.
    machine
        .register_session(session_id.clone())
        .await
        .expect("actual cold registration convergence");
    assert_eq!(
        crate::store::load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .expect("converged durable lifecycle"),
        Some(crate::RuntimeState::Idle)
    );
    {
        let sessions = machine.sessions.read().await;
        let entry = sessions.get(&session_id).expect("cold registered owner");
        let driver = entry.driver.lock().await;
        let DriverEntry::Persistent(driver) = &*driver else {
            panic!("real SQLite driver")
        };
        assert!(driver.inner_ref().ledger().get(&input_id).is_none());
    }
    let retained = store
        .load_input_state(&runtime_id, &input_id)
        .await
        .expect("retained physical history")
        .expect("original remains retained");
    assert!(retained.state.controller_client.is_none());
    assert_eq!(
        read_bytes().1,
        before.1,
        "convergence preserves the original bytes"
    );
    let after_convergence = read_bytes();
    for reference in &lineage {
        let entered = Cell::new(0);
        let ((), stamp) = publication.observe(|| ()).expect("before permitted entry");
        let result = machine
            .try_controller_grant_mutation()
            .and_then(|mut custody| {
                custody.with_unreferenced_controller_grant(reference, || {
                    entered.set(entered.get() + 1);
                    let _publication = publication
                        .begin_owner_change()
                        .expect("entered publication");
                    Ok::<_, ()>(())
                })
            });
        assert_eq!(
            result,
            Ok(Ok(())),
            "converged old run no longer vetoes entry"
        );
        assert_eq!(entered.get(), 1);
        assert_eq!(
            stamp.check_current(),
            Err(meerkat_authorization::publication::PublicationError::Changed)
        );
        assert_eq!(read_bytes(), after_convergence);
    }
    let replacement = meerkat_auth_core::save_tokens_and_publish_lifecycle(
        credentials.clone(),
        auth.clone(),
        credential.clone(),
        meerkat_core::auth::PersistedTokens::api_key("replacement-after-convergence"),
    )
    .await
    .expect("completed work permits actual credential replacement");
    assert_eq!(
        credentials.token_store().load(&token_key).await.unwrap(),
        Some(replacement)
    );
    meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
        credentials.clone(),
        auth.clone(),
        credential,
    )
    .await
    .expect("completed work permits actual credential removal");
    assert!(
        credentials
            .token_store()
            .load(&token_key)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!auth.snapshot(&lease).credential_present);
    assert_eq!(
        read_bytes(),
        after_convergence,
        "credential mutation preserves native history"
    );
}
