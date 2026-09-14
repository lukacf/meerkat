use super::*;
use crate::identifiers::LogicalRuntimeId;
use crate::live_resources::LIVE_LEDGER_MAX_CHARGE;
use crate::store::live_read::{
    LiveCompositeReadRequest, LiveLedgerWriteProfile, RuntimeLiveLedgerOps, read_live_composite,
};
use crate::store::{
    InMemoryRuntimeStore, RuntimeStore, RuntimeStoreWriteFence, RuntimeStoreWriteFenceOutcome,
    SerializedSessionSnapshot,
};
use meerkat_core::{Session, SessionStore};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[path = "joint_input_tests.rs"]
mod joint_input_tests;

#[cfg(not(target_arch = "wasm32"))]
#[path = "commit_clock_tests.rs"]
mod commit_clock_tests;

#[cfg(not(target_arch = "wasm32"))]
#[path = "owned_admission_tests.rs"]
mod owned_admission_tests;

#[cfg(not(target_arch = "wasm32"))]
#[path = "credit_tests.rs"]
mod credit_tests;

#[cfg(not(target_arch = "wasm32"))]
#[path = "transcript_tests.rs"]
mod transcript_tests;

#[derive(Clone, Copy, Debug)]
enum Backend {
    Memory,
    #[cfg(feature = "sqlite-store")]
    WholeBlob,
    #[cfg(feature = "sqlite-store")]
    HeadCanonical,
}

fn backends() -> Vec<Backend> {
    vec![
        Backend::Memory,
        #[cfg(feature = "sqlite-store")]
        Backend::WholeBlob,
        #[cfg(feature = "sqlite-store")]
        Backend::HeadCanonical,
    ]
}

struct Fixture {
    store: Arc<dyn RuntimeStore>,
    session: Session,
    _directory: tempfile::TempDir,
    #[cfg(feature = "sqlite-store")]
    path: std::path::PathBuf,
}

impl Fixture {
    async fn new(backend: Backend) -> TestResult<Self> {
        Self::with_session(backend, Session::new()).await
    }

    async fn with_session(backend: Backend, session: Session) -> TestResult<Self> {
        let directory = tempfile::tempdir()?;
        #[cfg(feature = "sqlite-store")]
        let path = directory.path().join("runtime.sqlite3");
        let store: Arc<dyn RuntimeStore> = match backend {
            Backend::Memory => {
                let store = InMemoryRuntimeStore::new();
                save_actor(&store, &session).await?;
                Arc::new(store)
            }
            #[cfg(feature = "sqlite-store")]
            Backend::WholeBlob | Backend::HeadCanonical => {
                if matches!(backend, Backend::HeadCanonical) {
                    meerkat_store::SqliteSessionStore::open(&path)?
                        .save(&session)
                        .await?;
                }
                let initial = crate::store::SqliteRuntimeStore::new_whole_blob(&path)?;
                save_actor(&initial, &session).await?;
                if matches!(backend, Backend::HeadCanonical) {
                    drop(initial);
                    Arc::new(crate::store::SqliteRuntimeStore::new_head_canonical(&path)?)
                } else {
                    Arc::new(initial)
                }
            }
        };
        Ok(Self {
            store,
            session,
            _directory: directory,
            #[cfg(feature = "sqlite-store")]
            path,
        })
    }

    fn ops(&self) -> TestResult<&dyn RuntimeLiveLedgerOps> {
        self.store
            .live_ledger_ops()
            .ok_or_else(|| "missing Live capability".into())
    }

    async fn actor(&self) -> TestResult<RuntimeSessionAuthority> {
        self.store
            .load_session_boundary_authority(&LogicalRuntimeId::for_session(self.session.id()))
            .await?
            .ok_or_else(|| "actor authority missing".into())
    }
}

async fn save_actor(store: &dyn RuntimeStore, session: &Session) -> TestResult {
    store
        .commit_session_snapshot(
            &LogicalRuntimeId::for_session(session.id()),
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(serde_json::to_vec(session)?),
            },
        )
        .await?;
    Ok(())
}

#[derive(Clone)]
struct Fence(RuntimeStoreWriteFenceOutcome);

impl RuntimeStoreWriteFence for Fence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        if self.0 == RuntimeStoreWriteFenceOutcome::Applied {
            operation()?;
        }
        Ok(self.0.clone())
    }
}

fn current_fence() -> Arc<dyn RuntimeStoreWriteFence> {
    Arc::new(Fence(RuntimeStoreWriteFenceOutcome::Applied))
}

#[cfg(not(target_arch = "wasm32"))]
trait FixtureClockOwner {
    fn with_fixture_clock(store: Arc<dyn RuntimeStore>, session_id: SessionId) -> Self;
}

#[cfg(not(target_arch = "wasm32"))]
impl FixtureClockOwner for crate::live_ledger::authority::store::LiveRequestStoreOwner {
    fn with_fixture_clock(store: Arc<dyn RuntimeStore>, session_id: SessionId) -> Self {
        Self::new(store, session_id).with_clock(Arc::new(|| Ok(1)))
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn install_grant_executor(
    fixture: &Fixture,
    epoch: &meerkat_core::RuntimeEpochId,
    generation: u64,
) -> TestResult {
    fixture
        .store
        .commit_machine_lifecycle(
            &LogicalRuntimeId::for_session(fixture.session.id()),
            crate::store::MachineLifecycleCommit::new_with_binding(
                crate::RuntimeState::Idle,
                crate::store::MachineLifecycleBindingFacts::new(
                    Some("registered-executor".into()),
                    Some(1),
                    Some(generation),
                    Some(epoch.to_string()),
                ),
                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
            ),
            &[],
        )
        .await?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn grant_activation_request(
    fixture: &Fixture,
    epoch: &meerkat_core::RuntimeEpochId,
) -> TestResult<crate::live_grant::LiveGrantActivationRequest<()>> {
    use crate::live_grant::LiveGrantActivationRequest;
    let session_id = fixture.session.id();
    Ok(LiveGrantActivationRequest {
        activation_id: meerkat_core::live_execution::activation::LiveActivationId::parse(
            "voice-activation",
        )?,
        declaration: serde_json::from_value(serde_json::json!({
            "issuer_realm": "owner",
            "profile_id": "voice",
            "profile_revision": vec![2; 32],
            "requesting_realms": ["caller"],
            "executor": {"kind": "session", "session_id": session_id},
            "allowed_evidence": ["application_snapshot"],
            "permission": {
                "allowed_mutations": ["read_only"],
                "tools": {"kind": "allow_listed", "names": ["allowed_tool"]},
                "limits": {
                    "max_requests": 2, "max_concurrent_requests": 1,
                    "max_effects_per_request": 2, "max_tokens_per_request": 1000,
                    "max_duration_ms": 100
                }
            },
            "generation": 1,
            "revoke_policy": "cancel_pending_and_request_running_cancellation"
        }))?,
        requesting_realm: serde_json::from_value(serde_json::json!("caller"))?,
        executor: serde_json::from_value(serde_json::json!({
            "selector": {"kind": "session", "session_id": session_id},
            "binding": {
                "session_id": session_id, "realm": "executor",
                "runtime_epoch": epoch, "binding_generation": 1
            }
        }))?,
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn trusted_native_issuer_commits_complete_permission_to_current_lifecycle() -> TestResult {
    use crate::live_grant::{LiveExecutionGrantIssuer, LiveGrantActivationError};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let epoch = meerkat_core::RuntimeEpochId::new();
        let issuer = LiveExecutionGrantIssuer::new(Arc::clone(&fixture.store));
        assert!(matches!(
            issuer
                .activate(grant_activation_request(&fixture, &epoch)?, current_fence())
                .await,
            Err(LiveGrantActivationError::ExecutorNotCurrent)
        ));
        install_grant_executor(&fixture, &epoch, 1).await?;
        let wrong_epoch = meerkat_core::RuntimeEpochId::new();
        assert!(matches!(
            issuer
                .activate(
                    grant_activation_request(&fixture, &wrong_epoch)?,
                    current_fence()
                )
                .await,
            Err(LiveGrantActivationError::ExecutorNotCurrent)
        ));
        let blocked = Arc::new(Fence(RuntimeStoreWriteFenceOutcome::Conflict {
            reason: "registration replaced".into(),
        }));
        assert!(
            issuer
                .activate(grant_activation_request(&fixture, &epoch)?, blocked)
                .await
                .is_err()
        );
        assert!(
            fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .is_none()
        );
        let grant = issuer
            .activate(grant_activation_request(&fixture, &epoch)?, current_fence())
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(grant.activation_commit(), &head.reference);
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert_eq!(state.grant_record, serde_json::to_string(grant.record())?);
        assert_eq!(state.grant_max_requests, 2);
        assert_eq!(state.grant_max_concurrent_requests, 1);
        assert_eq!(state.grant_max_effects, 2);
        assert_eq!(state.grant_max_tokens, 1000);
        assert_eq!(state.grant_max_duration_ms, 100);
        assert_eq!(state.grant_tools, ["allowed_tool".into()].into());
        assert!(state.grant_tools_restricted);
        assert_eq!(
            state.grant_mutations,
            [meerkat_core::ToolMutationClass::ReadOnly].into()
        );
        assert_eq!(
            state.grant_evidence,
            [meerkat_core::live_execution::request::LiveRequestEvidenceKind::ApplicationSnapshot]
                .into()
        );
        assert_eq!(grant.record().grant_ref().issuer_realm.as_str(), "owner");
        assert_eq!(grant.record().requesting_realm().as_str(), "caller");
        assert_eq!(grant.record().executor().binding.realm.as_str(), "executor");
        drop(issuer);
        #[cfg(feature = "sqlite-store")]
        if !matches!(backend, Backend::Memory) {
            let Fixture {
                store,
                session,
                _directory,
                path,
            } = fixture;
            drop(store);
            let reopened = match backend {
                Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&path)?
                }
                Backend::Memory => return Err("expected durable backend".into()),
            };
            assert_eq!(
                reopened
                    .live_ledger_ops()
                    .ok_or("Live ops")?
                    .load_live_head(session.id())
                    .await?,
                Some(head)
            );
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn activation_commit_compares_lifecycle_inside_each_store_transaction() -> TestResult {
    use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let epoch = meerkat_core::RuntimeEpochId::new();
        install_grant_executor(&fixture, &epoch, 1).await?;
        let observation = fixture
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
            .await?;
        let stale = observation.version().ok_or("version")?.clone();
        install_grant_executor(&fixture, &epoch, 2).await?;
        let owner = LiveRequestStoreOwner::with_fixture_clock(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
        );
        assert!(matches!(
            owner
                .commit_for_runtime(generated_activation(1), stale, current_fence())
                .await,
            Err(LiveRequestAuthorityError::Store(
                RuntimeStoreError::MachineLifecycleVersionConflict { .. }
            ))
        ));
        assert!(
            fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .is_none()
        );
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn generated_activation(generation: u64) -> crate::live_ledger::authority::dsl::LiveRequestInput {
    use crate::live_ledger::authority::dsl::{LiveRequestEvidenceKind, ToolMutationClass};
    crate::live_ledger::authority::dsl::LiveRequestInput::Activate {
        grant_id: "grant".into(),
        generation,
        expires_at: 100,
        executor: "binding".into(),
        record: format!("complete-grant-record-{generation}"),
        profile_revision: "profile-revision".into(),
        evidence: [LiveRequestEvidenceKind::ApplicationSnapshot].into(),
        mutations: [ToolMutationClass::ReadOnly].into(),
        tools_restricted: true,
        tools: ["allowed_tool".into()].into(),
        max_requests: 2,
        max_concurrent_requests: 1,
        max_effects: 2,
        max_tokens: 1000,
        max_duration_ms: 100,
        now: 1,
    }
}

#[cfg(not(target_arch = "wasm32"))]
const GENERATED_REQUEST_ID: &str = "00000000-0000-4000-8000-000000000001";
const GENERATED_CLAIM_ID: &str = "00000000-0000-4000-8000-000000000002";

fn generated_request_setup() -> Vec<crate::live_ledger::authority::dsl::LiveRequestInput> {
    use crate::live_ledger::authority::dsl::{LiveRequestEvidenceKind, LiveRequestInput as Input};
    let budget =
        crate::live_ledger::authority::store::request_credits::RequestCompletionBudget::measured()
            .expect("measured request completion budget");
    vec![
        generated_activation(1),
        Input::Reserve {
            content_complete: true,
            content_discontinuous: false,
            content_empty: false,
            content_fits: true,
            request_id: GENERATED_REQUEST_ID.into(),
            source: "source".into(),
            payload: "payload".into(),
            evidence: LiveRequestEvidenceKind::ApplicationSnapshot,
            profile_revision: "profile-revision".into(),
            parent_scope: "".into(),
            grant_id: "grant".into(),
            generation: 1,
            executor: "binding".into(),
            now: 2,
            credit_records: budget.envelope.total().records,
            credit_bytes: budget.envelope.total().encoded_bytes,
            snapshot_ceiling: budget.snapshot_ceiling,
        },
        Input::Admit {
            request_id: GENERATED_REQUEST_ID.into(),
            source_ingress_open: true,
            source: "source".into(),
            payload: "payload".into(),
            input_id: "input".into(),
            admission_commit: "admission-commit".into(),
            ingress_generation: 1,
            credit_records: budget.envelope.total().records,
            credit_bytes: budget.envelope.total().encoded_bytes,
            snapshot_ceiling: budget.snapshot_ceiling,
            profile_revision: "profile-revision".into(),
            now: 3,
        },
        Input::Stage {
            request_id: GENERATED_REQUEST_ID.into(),
            input_id: "input".into(),
            admission_commit: "admission-commit".into(),
            run_id: "run".into(),
            scope_id: "scope".into(),
            scope_record: "scope-digest".into(),
            executor: "binding".into(),
            profile_revision: "profile-revision".into(),
            now: 4,
        },
    ]
}

#[cfg(not(target_arch = "wasm32"))]
fn generated_effect_claim() -> crate::live_ledger::authority::dsl::LiveRequestInput {
    let budget =
        crate::live_ledger::authority::store::effect_credits::EffectCompletionBudget::for_kind(
            meerkat_core::execution_scope::ScopedEffectKind::ToolDispatch,
        )
        .expect("measured completion budget");
    crate::live_ledger::authority::dsl::LiveRequestInput::ClaimEffect {
        request_id: GENERATED_REQUEST_ID.into(),
        input_id: "input".into(),
        admission_commit: "admission-commit".into(),
        run_id: "run".into(),
        scope_id: "scope".into(),
        scope_record: "scope-digest".into(),
        parent_scope: "".into(),
        executor: "binding".into(),
        claim_id: GENERATED_CLAIM_ID.into(),
        claim_record: "claim-record".into(),
        effect_id: "effect".into(),
        chain_id: "effect".into(),
        attempt: 0,
        target: "target-and-arguments".into(),
        kind: meerkat_core::execution_scope::ScopedEffectKind::ToolDispatch,
        tool: "allowed_tool".into(),
        mutation: meerkat_core::ToolMutationClass::ReadOnly,
        profile_revision: "profile-revision".into(),
        policy_revision: "synthetic-policy-observation".into(),
        policy_permits: true,
        credit_schema: crate::live_ledger::completion_budget::CompletionCreditSchema::V1,
        credit_records: budget.envelope.total().records,
        credit_bytes: budget.envelope.total().encoded_bytes,
        minimum_record_charge: budget.minimum_record_charge,
        maximum_record_charge: budget.maximum_record_charge,
        snapshot_ceiling: budget.snapshot_ceiling,
        available_records: crate::live_resources::LIVE_LEDGER_MAX_CHARGE.records,
        available_bytes: crate::live_resources::LIVE_LEDGER_MAX_CHARGE.encoded_bytes,
        now: 5,
    }
}

async fn commit_generated_unknown(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
) -> TestResult<crate::live_ledger::authority::dsl::LiveRequestInput> {
    use crate::live_ledger::authority::dsl::{
        LiveRequestMachineAuthority, LiveRequestMachineMutator,
    };
    let ops = store.live_ledger_ops().ok_or("Live ops")?;
    let head = ops.load_live_head(session_id).await?.ok_or("head")?;
    let owner = LiveRequestMachineAuthority::recover_from_state(
        crate::generated::live_request_state::decode(&head.payload.request_snapshot)?,
    )?;
    let completion = credit_tests::record(
        session_id,
        meerkat_core::ops::OperationId(uuid::Uuid::parse_str(GENERATED_REQUEST_ID)?),
        meerkat_core::ops::OperationId(uuid::Uuid::parse_str(GENERATED_CLAIM_ID)?),
        head.reference.event_count + 1,
        crate::live_ledger::completion::LivePhysicalEffectOutcome::Unknown,
    )?;
    let input = credit_tests::settlement(&completion)?;
    let mut candidate = owner.prepare_authority();
    LiveRequestMachineMutator::apply(&mut candidate, input.clone())?;
    let prepared = PreparedLiveLedgerCommit::from_request_transition_with_completions(
        session_id,
        Some(&head),
        &candidate,
        vec![completion],
    )?;
    assert!(matches!(
        ops.commit_live_ledger(prepared, current_fence()).await?,
        LiveLedgerCommitOutcome::Committed { .. }
    ));
    Ok(input)
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn generated_request_transition_is_returned_only_after_actual_store_commit() -> TestResult {
    use crate::live_ledger::authority::dsl::LiveRequestInput as Input;
    use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let owner = LiveRequestStoreOwner::with_fixture_clock(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
        );
        for input in generated_request_setup() {
            let committed = owner.commit(input, current_fence()).await?;
            assert!(!committed.transition.effects().is_empty());
            assert_eq!(
                fixture
                    .ops()?
                    .load_live_head(fixture.session.id())
                    .await?
                    .ok_or("head")?
                    .reference,
                committed.head,
            );
        }
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let rejected = owner
            .commit(
                generated_effect_claim(),
                Arc::new(Fence(RuntimeStoreWriteFenceOutcome::Conflict {
                    reason: "request owner superseded".into(),
                })),
            )
            .await;
        assert!(matches!(rejected, Err(LiveRequestAuthorityError::Store(_))));
        assert_eq!(
            fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?,
            before,
        );
        let committed = owner
            .commit(generated_effect_claim(), current_fence())
            .await?;
        let restored_owner = LiveRequestStoreOwner::with_fixture_clock(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
        );
        assert!(
            restored_owner
                .commit(generated_effect_claim(), current_fence())
                .await
                .is_err()
        );
        restored_owner
            .commit(
                Input::Revoke {
                    grant_id: "grant".into(),
                    generation: 1,
                },
                current_fence(),
            )
            .await?;
        commit_generated_unknown(fixture.store.as_ref(), fixture.session.id()).await?;
        assert!(committed.head.revision > before.reference.revision);
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn generated_store_claim_reads_revocation_after_an_actual_policy_await() -> TestResult {
    use crate::live_ledger::authority::dsl::LiveRequestInput as Input;
    use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
    for (backend, changes) in backends().into_iter().flat_map(|backend| {
        [
            vec![Input::Revoke {
                grant_id: "grant".into(),
                generation: 1,
            }],
            vec![Input::Cancel {
                request_id: GENERATED_REQUEST_ID.into(),
            }],
            vec![Input::FenceExecutor {
                executor: "replacement-binding".into(),
            }],
            vec![
                Input::FenceExecutor {
                    executor: "replacement-binding".into(),
                },
                Input::FenceExecutor {
                    executor: "binding".into(),
                },
            ],
        ]
        .into_iter()
        .map(move |change| (backend, change))
    }) {
        let fixture = Fixture::new(backend).await?;
        let owner = Arc::new(LiveRequestStoreOwner::with_fixture_clock(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
        ));
        for input in generated_request_setup() {
            owner.commit(input, current_fence()).await?;
        }
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let claimant = Arc::clone(&owner);
        let task = tokio::spawn(async move {
            entered_tx.send(()).map_err(|()| "lost entry receiver")?;
            resume_rx.await.map_err(|_| "lost policy result")?;
            Ok::<_, &'static str>(matches!(
                claimant
                    .commit(generated_effect_claim(), current_fence())
                    .await,
                Err(LiveRequestAuthorityError::Transition(_))
            ))
        });
        entered_rx.await?;
        for change in changes {
            owner.commit(change, current_fence()).await?;
        }
        let revoked = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        resume_tx.send(()).map_err(|()| "lost claimant")?;
        assert!(task.await??);
        assert_eq!(
            fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?,
            revoked,
        );
    }
    Ok(())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]
#[tokio::test]
async fn generated_request_scope_and_spent_claim_survive_closed_store_reopen() -> TestResult {
    use crate::live_ledger::authority::dsl::LiveRequestInput as Input;
    use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let owner = LiveRequestStoreOwner::with_fixture_clock(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
        );
        let mut setup = generated_request_setup();
        let stage = setup.pop().ok_or("missing stage")?;
        for input in setup {
            owner.commit(input, current_fence()).await?;
        }
        owner
            .commit(Input::CloseIngress {}, current_fence())
            .await?;
        let closed = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let Fixture {
            store,
            session,
            _directory,
            path,
        } = fixture;
        drop(owner);
        drop(store);
        let open = || -> TestResult<Arc<dyn RuntimeStore>> {
            Ok(Arc::new(match backend {
                Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&path)?
                }
                Backend::Memory => return Err("expected durable backend".into()),
            }))
        };
        let store = open()?;
        let ops = store.live_ledger_ops().ok_or("missing Live ops")?;
        assert_eq!(
            ops.load_live_head(session.id()).await?,
            Some(closed.clone())
        );
        let owner =
            LiveRequestStoreOwner::with_fixture_clock(Arc::clone(&store), session.id().clone());
        owner.commit(stage.clone(), current_fence()).await?;
        assert!(matches!(
            owner.commit(stage, current_fence()).await,
            Err(LiveRequestAuthorityError::Transition(_))
        ));
        let staged = ops.load_live_head(session.id()).await?.ok_or("head")?;
        let restore = || Input::RestoreScope {
            request_id: GENERATED_REQUEST_ID.into(),
            input_id: "input".into(),
            admission_commit: "admission-commit".into(),
            run_id: "run".into(),
            scope_id: "scope".into(),
            scope_record: "scope-digest".into(),
            parent_scope: "".into(),
            executor: "binding".into(),
            profile_revision: "profile-revision".into(),
            now: 5,
        };
        for field in 0..8 {
            let mut changed = restore();
            let Input::RestoreScope {
                request_id,
                input_id,
                run_id,
                scope_id,
                scope_record,
                parent_scope,
                executor,
                admission_commit,
                ..
            } = &mut changed
            else {
                return Err("wrong restore variant".into());
            };
            [
                request_id,
                input_id,
                run_id,
                scope_id,
                scope_record,
                parent_scope,
                executor,
                admission_commit,
            ][field]
                .push_str("-wrong");
            assert!(matches!(
                owner.commit(changed, current_fence()).await,
                Err(LiveRequestAuthorityError::Transition(_))
            ));
            assert_eq!(
                ops.load_live_head(session.id()).await?,
                Some(staged.clone())
            );
        }
        owner.commit(restore(), current_fence()).await?;
        owner
            .commit(generated_effect_claim(), current_fence())
            .await?;
        drop(owner);
        drop(store);
        let store = open()?;
        let owner =
            LiveRequestStoreOwner::with_fixture_clock(Arc::clone(&store), session.id().clone());
        assert!(matches!(
            owner
                .commit(generated_effect_claim(), current_fence())
                .await,
            Err(LiveRequestAuthorityError::Transition(_))
        ));
        let feedback = commit_generated_unknown(store.as_ref(), session.id()).await?;
        assert!(matches!(
            owner.commit(feedback, current_fence()).await,
            Err(LiveRequestAuthorityError::Transition(_))
        ));
        drop(owner);
        drop(store);
        drop(_directory);
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn generated_request_close_before_admission_does_not_create_a_run() -> TestResult {
    use crate::live_ledger::authority::dsl::LiveRequestInput as Input;
    use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let owner = LiveRequestStoreOwner::with_fixture_clock(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
        );
        let mut setup = generated_request_setup().into_iter();
        for input in setup.by_ref().take(2) {
            owner.commit(input, current_fence()).await?;
        }
        owner
            .commit(Input::CloseIngress {}, current_fence())
            .await?;
        let closed = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        for input in setup {
            assert!(matches!(
                owner.commit(input, current_fence()).await,
                Err(LiveRequestAuthorityError::Transition(_))
            ));
            assert_eq!(
                fixture.ops()?.load_live_head(fixture.session.id()).await?,
                Some(closed.clone())
            );
        }
        let state = crate::generated::live_request_state::decode(&closed.payload.request_snapshot)?;
        assert!(state.request_runs.is_empty());
        assert!(state.admitted_requests.is_empty());
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn generated_request_snapshot_codec_rejects_unknown_missing_and_future_fields() -> TestResult {
    use crate::generated::live_request_state::{decode, encode};
    use crate::live_ledger::authority::dsl::{
        LiveRequestMachineAuthority as Authority, LiveRequestMachineMutator as Mutator,
    };
    let mut owner = Authority::new();
    for input in generated_request_setup() {
        Mutator::apply(&mut owner, input)?;
    }
    let bytes = encode(owner.state())?;
    let recovered = Authority::recover_from_state(decode(&bytes)?)?;
    assert_eq!(encode(recovered.state())?, bytes);
    let object: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&bytes)?;
    for field in object.keys() {
        let mut missing = object.clone();
        missing.remove(field);
        assert!(
            decode(&serde_json::to_vec(&missing)?).is_err(),
            "accepted missing {field}"
        );
    }
    let current_format = object
        .get("format")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing encoded format")?;
    let future_format = current_format.checked_add(1).ok_or("format overflow")?;
    for invalid_format in (0..current_format).chain([future_format, u64::MAX]) {
        let mut invalid = object.clone();
        invalid.insert("format".into(), invalid_format.into());
        assert!(decode(&serde_json::to_vec(&invalid)?).is_err());
    }
    let mut unknown = object;
    unknown.insert("unrecognized_authority".into(), true.into());
    assert!(decode(&serde_json::to_vec(&unknown)?).is_err());
    Ok(())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]
#[tokio::test]
async fn private_recovery_import_cannot_supply_public_request_scope_authority() -> TestResult {
    use crate::RuntimeState;
    use crate::live_execution::LiveBridgeRecoveryImage;
    use crate::live_ledger::authority::dsl::LiveEffectPhase;
    use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
    use crate::store::{
        MachineLifecycleBindingFacts, MachineLifecycleCommit, SupervisorAuthoritySnapshot,
        load_machine_lifecycle,
    };
    use meerkat_core::live_execution::LiveBridgeSubmissionState;

    let operations = [
            LiveBridgeSubmissionState::SubmissionAttemptClaimed,
            LiveBridgeSubmissionState::LocalWriteCompletedAwaitingProof,
            LiveBridgeSubmissionState::SubmissionAmbiguous,
            LiveBridgeSubmissionState::CallAbandonedByClose,
        ].into_iter().enumerate().map(|(index, submission)| serde_json::json!({
            "operation_id": if index == 0 { GENERATED_REQUEST_ID.to_owned() } else { format!("request-{index}") },
            "channel_id": format!("private-channel-{index}"),
            "interaction_id": format!("private-interaction-{index}"),
            "provider_turn_ref": format!("private-turn-{index}"),
            "provider_delegation_ref": format!("private-delegation-{index}"),
            "provider_call_ref": format!("private-call-{index}"),
            "source_agent_identity": "binding",
            "canonical_context_revision": "scope-digest",
            "request_digest": "sha256:private-request",
            "phase": "execution_terminal",
            "execution_started": true,
            "outcome_receipt_required": true,
            "outcome_receipt_recorded": true,
            "terminal": "completed",
            "result_digest": "sha256:private-result",
            "cancellation_reason": "restart",
            "submission_output_kind": "success",
            "submission_digest": "sha256:private-submission",
            "submission_state": submission,
            "current_for_channel": false,
            "channel_revoked": true,
        })).collect::<Vec<_>>();
    let imported_private: LiveBridgeRecoveryImage = serde_json::from_value(serde_json::json!({
        "operations": operations
    }))?;
    let mut private_state = crate::meerkat_machine::dsl::MeerkatMachineAuthority::new()
        .state()
        .clone();
    imported_private.restore_into(&mut private_state)?;
    assert_eq!(
        LiveBridgeRecoveryImage::capture(&private_state)?,
        imported_private
    );

    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let runtime_id = LogicalRuntimeId::for_session(fixture.session.id());
        fixture
            .store
            .commit_machine_lifecycle(
                &runtime_id,
                MachineLifecycleCommit::new_with_binding_unregister_progress_and_live_bridge(
                    RuntimeState::Idle,
                    MachineLifecycleBindingFacts::default(),
                    SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    None,
                    imported_private.clone(),
                ),
                &[],
            )
            .await?;
        let private_before = fixture
            .store
            .load_machine_lifecycle_record(&runtime_id)
            .await?
            .ok_or("private lifecycle")?;
        let Fixture {
            store,
            session,
            _directory,
            path,
        } = fixture;
        drop(store);
        let open = || -> TestResult<Arc<dyn RuntimeStore>> {
            Ok(Arc::new(match backend {
                Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&path)?
                }
                Backend::Memory => return Err("expected durable backend".into()),
            }))
        };
        let store = open()?;
        assert_eq!(
            store.load_machine_lifecycle_record(&runtime_id).await?,
            Some(private_before.clone())
        );
        let ops = store.live_ledger_ops().ok_or("Live ops")?;
        assert!(ops.load_live_head(session.id()).await?.is_none());
        let owner =
            LiveRequestStoreOwner::with_fixture_clock(Arc::clone(&store), session.id().clone());
        for unauthorized in generated_request_setup()
            .into_iter()
            .skip(1)
            .chain([generated_effect_claim()])
        {
            let before = ops.load_live_head(session.id()).await?;
            if matches!(
                unauthorized,
                crate::live_ledger::authority::dsl::LiveRequestInput::Reserve { .. }
            ) {
                let refused = owner.commit(unauthorized, current_fence()).await?;
                assert!(matches!(
                    refused.transition.effects(),
                    [
                        crate::live_ledger::authority::dsl::LiveRequestEffect::SourceRefused {
                            reason: crate::live_source::LiveSourceRefusal::Permission,
                            ..
                        }
                    ]
                ));
                let head = ops
                    .load_live_head(session.id())
                    .await?
                    .ok_or("refusal head")?;
                let state =
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
                assert!(state.request_ids.is_empty());
                assert!(state.request_inputs.is_empty());
                assert!(state.run_requests.is_empty());
                assert!(state.claim_phases.is_empty());
            } else {
                assert!(matches!(
                    owner.commit(unauthorized, current_fence()).await,
                    Err(LiveRequestAuthorityError::Transition(_))
                ));
                assert_eq!(ops.load_live_head(session.id()).await?, before);
            }
        }
        assert_eq!(
            store.load_machine_lifecycle_record(&runtime_id).await?,
            Some(private_before.clone())
        );

        for mut input in generated_request_setup()
            .into_iter()
            .chain([generated_effect_claim()])
        {
            use crate::live_ledger::authority::dsl::LiveRequestInput;
            if let LiveRequestInput::Reserve { source, .. }
            | LiveRequestInput::Admit { source, .. } = &mut input
            {
                *source = "fresh-source-after-activation".into();
            }
            owner.commit(input, current_fence()).await?;
        }
        let public_before = ops
            .load_live_head(session.id())
            .await?
            .ok_or("public head")?;
        drop(owner);
        drop(store);
        let store = open()?;
        let ops = store.live_ledger_ops().ok_or("Live ops")?;
        assert_eq!(
            ops.load_live_head(session.id()).await?,
            Some(public_before.clone())
        );
        let private_after = store
            .load_machine_lifecycle_record(&runtime_id)
            .await?
            .ok_or("private lifecycle after public recovery")?;
        assert_eq!(private_after, private_before);
        let private_snapshot = load_machine_lifecycle(store.as_ref(), &runtime_id)
            .await?
            .ok_or("decoded private lifecycle")?;
        assert_eq!(private_snapshot.live_bridge_recovery(), &imported_private);
        let public =
            crate::generated::live_request_state::decode(&public_before.payload.request_snapshot)?;
        assert_eq!(
            public.claim_phases.get(GENERATED_CLAIM_ID),
            Some(&LiveEffectPhase::Claimed)
        );
        assert!(
            crate::generated::live_request_state::decode(&serde_json::to_vec(
                private_snapshot.live_bridge_recovery()
            )?)
            .is_err()
        );
        assert!(
            serde_json::from_slice::<LiveBridgeRecoveryImage>(
                &public_before.payload.request_snapshot
            )
            .is_err()
        );
        let owner =
            LiveRequestStoreOwner::with_fixture_clock(Arc::clone(&store), session.id().clone());
        assert!(matches!(
            owner
                .commit(generated_effect_claim(), current_fence())
                .await,
            Err(LiveRequestAuthorityError::Transition(_))
        ));
        assert_eq!(ops.load_live_head(session.id()).await?, Some(public_before));
        commit_generated_unknown(store.as_ref(), session.id()).await?;
        let settled = ops
            .load_live_head(session.id())
            .await?
            .ok_or("settled head")?;
        let public =
            crate::generated::live_request_state::decode(&settled.payload.request_snapshot)?;
        assert_eq!(
            public.claim_phases.get(GENERATED_CLAIM_ID),
            Some(&LiveEffectPhase::Unknown)
        );
        assert_eq!(
            store.load_machine_lifecycle_record(&runtime_id).await?,
            Some(private_before)
        );
    }
    Ok(())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]
#[tokio::test]
async fn generated_executor_binding_aba_stays_revoked_after_reopen() -> TestResult {
    use crate::live_ledger::authority::dsl::LiveRequestInput as Input;
    use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let owner = LiveRequestStoreOwner::with_fixture_clock(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
        );
        for input in generated_request_setup() {
            owner.commit(input, current_fence()).await?;
        }
        for executor in ["replacement-binding", "binding"] {
            owner
                .commit(
                    Input::FenceExecutor {
                        executor: executor.into(),
                    },
                    current_fence(),
                )
                .await?;
        }
        let Fixture {
            store,
            session,
            _directory,
            path,
        } = fixture;
        drop(owner);
        drop(store);
        let store: Arc<dyn RuntimeStore> = Arc::new(match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
            Backend::HeadCanonical => crate::store::SqliteRuntimeStore::new_head_canonical(&path)?,
            Backend::Memory => return Err("expected durable backend".into()),
        });
        let owner =
            LiveRequestStoreOwner::with_fixture_clock(Arc::clone(&store), session.id().clone());
        let restore = || Input::RestoreScope {
            request_id: GENERATED_REQUEST_ID.into(),
            input_id: "input".into(),
            admission_commit: "admission-commit".into(),
            run_id: "run".into(),
            scope_id: "scope".into(),
            scope_record: "scope-digest".into(),
            parent_scope: "".into(),
            executor: "binding".into(),
            profile_revision: "profile-revision".into(),
            now: 5,
        };
        for generation in [None, Some(2)] {
            if let Some(generation) = generation {
                owner
                    .commit(generated_activation(generation), current_fence())
                    .await?;
            }
            let head = store
                .live_ledger_ops()
                .ok_or("Live ops")?
                .load_live_head(session.id())
                .await?;
            for input in [restore(), generated_effect_claim()] {
                assert!(matches!(
                    owner.commit(input, current_fence()).await,
                    Err(LiveRequestAuthorityError::Transition(_))
                ));
            }
            assert_eq!(
                store
                    .live_ledger_ops()
                    .ok_or("Live ops")?
                    .load_live_head(session.id())
                    .await?,
                head
            );
        }
        drop(owner);
        drop(store);
        drop(_directory);
    }
    Ok(())
}

fn observation(sequence: u64, text: &str) -> TestResult<LiveLedgerRecord> {
    use meerkat_contracts::wire::live_observation::{
        LiveObservationRecord, LiveObservationWireCodecV1,
    };
    use meerkat_core::live_observation::{
        LiveObservationSeq, LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
    };
    let fit = LiveObservationWireCodecV1::check_record_fit(LiveObservationRecord {
        sequence: LiveObservationSeq::new(sequence)?,
        channel_id: meerkat_core::live_execution::LiveChannelId::new("voice"),
        observation: LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(0.0, 1.0)?,
            text,
        ),
    })?;
    Ok(LiveLedgerRecord::Observation(
        super::super::transcript::StoredLiveObservation::from_fit(&fit),
    ))
}

#[tokio::test]
async fn archive_tombstone_compares_missing_lifecycle_in_each_store_transaction() -> TestResult {
    use crate::live_ledger::authority::dsl as request;
    use crate::live_ledger::transcript_authority::dsl as transcript;
    for backend in backends() {
        for inserted in [false, true] {
            let fixture = Fixture::new(backend).await?;
            let actor = fixture.actor().await?;
            let mut request = request::LiveRequestMachineAuthority::new().prepare_authority();
            request::LiveRequestMachineMutator::apply(
                &mut request,
                request::LiveRequestInput::CloseIngress,
            )?;
            let mut transcript =
                transcript::LiveTranscriptMachineAuthority::new().prepare_authority();
            transcript::LiveTranscriptMachineMutator::apply(
                &mut transcript,
                transcript::LiveTranscriptInput::CloseCurrentIngress,
            )?;
            let commit = PreparedLiveLedgerCommit::from_request_transition(
                fixture.session.id(),
                None,
                &request,
            )?
            .with_transcript_transition(&transcript)?
            .with_archive_existence_fence(
                crate::store::MachineLifecycleExpectedVersion::Missing,
                Some(actor.clone()),
            );
            if inserted {
                install_grant_executor(&fixture, &meerkat_core::RuntimeEpochId::new(), 0).await?;
            }
            let result = fixture
                .ops()?
                .commit_live_ledger(commit, current_fence())
                .await;
            if inserted {
                assert!(matches!(
                    result,
                    Err(RuntimeStoreError::MachineLifecycleVersionConflict { .. })
                ));
                assert!(
                    fixture
                        .ops()?
                        .load_live_head(fixture.session.id())
                        .await?
                        .is_none()
                );
            } else {
                assert!(matches!(result?, LiveLedgerCommitOutcome::Committed { .. }));
                let head = fixture
                    .ops()?
                    .load_live_head(fixture.session.id())
                    .await?
                    .ok_or("tombstone")?;
                assert_eq!(head.reference.event_count, 0);
                assert_eq!(head.payload.reserved, LiveResourceCharge::default());
            }
            assert_eq!(fixture.actor().await?, actor);
        }
    }
    Ok(())
}

#[cfg(all(feature = "live", feature = "sqlite-store"))]
#[tokio::test]
async fn cold_existing_session_archive_retains_tombstone_after_full_store_reopen() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let runtime_id = LogicalRuntimeId::for_session(fixture.session.id());
        assert!(matches!(
            fixture.store.observe_machine_lifecycle(&runtime_id).await?,
            crate::store::MachineLifecycleObservation::Missing
        ));
        let machine = crate::MeerkatMachine::persistent(
            Arc::clone(&fixture.store),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let lease = machine
            .prepare_session_archive_lease(fixture.session.id())
            .await?
            .ok_or("cold archive lease")?;
        machine.retire_session_with_archive_lease(lease).await?;
        let closed = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("closed head")?;
        assert_eq!(closed.reference.event_count, 0);
        assert_eq!(closed.payload.reserved, LiveResourceCharge::default());
        drop(machine);
        assert_eq!(
            Arc::strong_count(&fixture.store),
            1,
            "archive must release the physical store before reopen"
        );
        let Fixture {
            store,
            session,
            _directory,
            path,
        } = fixture;
        drop(store);
        let reopened: Arc<dyn RuntimeStore> = Arc::new(match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
            Backend::HeadCanonical => crate::store::SqliteRuntimeStore::new_head_canonical(&path)?,
            Backend::Memory => return Err("expected SQLite profile".into()),
        });
        let cold = crate::MeerkatMachine::persistent(
            Arc::clone(&reopened),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let duplicate = cold.prepare_session_archive_lease(session.id()).await?;
        if let Some(lease) = duplicate {
            cold.retire_session_with_archive_lease(lease).await?;
        }
        assert_eq!(
            reopened
                .live_ledger_ops()
                .ok_or("ledger")?
                .load_live_head(session.id())
                .await?,
            Some(closed)
        );
    }
    Ok(())
}

// Only this cfg(test) child can construct a synthetic prepared transition.
// Production construction remains reserved for the generated Live owner.
fn prepared(
    session_id: &SessionId,
    before: Option<&LiveLedgerStoredHead>,
    records: Vec<LiveLedgerRecord>,
) -> TestResult<PreparedLiveLedgerCommit> {
    let generation = before.map_or(1, |head| head.reference.generation);
    let mut reference = before.map_or_else(
        || LiveHeadReference {
            format: LiveLedgerFormatV1::V1,
            session_id: session_id.clone(),
            generation,
            revision: 0,
            event_count: 0,
            prefix_digest: LiveLedgerPrefixDigest::empty(session_id, generation),
        },
        |head| head.reference.clone(),
    );
    reference.revision += 1;
    let mut payload = before.map_or_else(
        || LiveLedgerPayloadState {
            used: LiveResourceCharge {
                records: 0,
                encoded_bytes: LIVE_HEAD_STORAGE_ALLOWANCE_BYTES,
            },
            reserved: LiveResourceCharge::default(),
            ingress_generation: 1,
            transcript_snapshot: Arc::new(Vec::new()),
            request_snapshot: Arc::new(Vec::new()),
        },
        |head| head.payload.clone(),
    );
    for record in &records {
        let bytes = record.encode()?;
        reference.event_count += 1;
        reference.prefix_digest = reference.prefix_digest.appended(record.sequence(), &bytes);
        payload.used = payload
            .used
            .checked_add(LiveResourceCharge::for_event_record(&bytes)?)?;
    }
    Ok(PreparedLiveLedgerCommit {
        purpose: LiveLedgerWritePurpose::ComponentMutation,
        expected: before.map(|head| head.reference.clone()),
        expected_actor: None,
        expected_lifecycle: None,
        successor: LiveLedgerStoredHead { reference, payload },
        records,
        sources: Vec::new(),
        input_admission: None,
        input_stage: None,
        input_read_fences: Vec::new(),
        quota: LIVE_LEDGER_MAX_CHARGE,
    })
}

// Explicit synthetic content for transport tests; uses the real backend CAS.
pub(crate) async fn append_observation_fixture(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
    texts: &[&str],
) -> Result<(), RuntimeStoreError> {
    let ops = store
        .live_ledger_ops()
        .ok_or_else(|| RuntimeStoreError::Unsupported("fixture ledger".into()))?;
    let before = ops.load_live_head(session_id).await?;
    let start = before.as_ref().map_or(0, |head| head.reference.event_count);
    let records = texts
        .iter()
        .enumerate()
        .map(|(index, text)| observation(start + index as u64 + 1, text))
        .collect::<TestResult<Vec<_>>>()
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    let change = prepared(session_id, before.as_ref(), records)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    let outcome = ops.commit_live_ledger(change, current_fence()).await?;
    assert!(matches!(outcome, LiveLedgerCommitOutcome::Committed { .. }));
    Ok(())
}

fn copy_prepared(value: &PreparedLiveLedgerCommit) -> PreparedLiveLedgerCommit {
    PreparedLiveLedgerCommit {
        purpose: value.purpose,
        expected: value.expected.clone(),
        expected_actor: value.expected_actor.clone(),
        expected_lifecycle: value.expected_lifecycle.clone(),
        input_admission: value.input_admission.clone(),
        input_read_fences: value.input_read_fences.clone(),
        input_stage: value.input_stage.as_ref().map(|stage| {
            Box::new(LiveInputStageMutation {
                input: stage.input.clone(),
                lifecycle: stage.lifecycle.clone(),
            })
        }),
        successor: value.successor.clone(),
        records: value.records.clone(),
        sources: value
            .sources
            .iter()
            .map(|source| super::super::source::PreparedLiveSourceMutation {
                expected: source.expected,
                replacement: source.replacement.clone(),
            })
            .collect(),
        quota: value.quota,
    }
}

async fn reserved_source(
    fixture: &Fixture,
    delegation: &str,
) -> TestResult<super::super::source::LiveSourceRow> {
    use crate::live_source::{
        LiveSourceContextReference, LiveSourceEntryRecord, LiveSourceFingerprint,
    };
    use meerkat_core::live_execution::LiveChannelId;
    use meerkat_core::live_execution::evidence::LiveObservationInterval;
    use meerkat_core::live_execution::request::{
        LiveProviderReference, LiveSourceIdentity, LiveSourceKey,
    };
    let source = LiveSourceKey::new(
        fixture.session.id().clone(),
        LiveChannelId::new("voice"),
        LiveSourceIdentity::ClientDelegation {
            delegation: LiveProviderReference::new(delegation)?,
        },
    )?;
    let read = read_live_composite(
        fixture.ops()?,
        LiveCompositeReadRequest::new(
            fixture.session.id().clone(),
            Some(LiveChannelId::new("voice")),
            0,
            64,
        )?,
    )
    .await?
    .ok_or("composite")?;
    let interval =
        LiveObservationInterval::new(0, read.authority().live_head().ok_or("head")?.event_count)?;
    let context = LiveSourceContextReference::from_composite(&read, &source, interval)?;
    let record = serde_json::from_value(serde_json::json!({
        "source": source, "request_id": uuid::Uuid::new_v4(),
        "fingerprint": LiveSourceFingerprint::client_delegation(2.5)?,
        "context": context,
        "frozen_request": {"kind":"application_snapshot","observations":interval,"request":" original request "},
        "grant": {"id":uuid::Uuid::new_v4(),"issuer_realm":"owner","generation":1},
        "cancellation":null,"disposition":{"kind":"reserved"}
    }))?;
    Ok(super::super::source::LiveSourceRow::encode(
        &LiveSourceEntryRecord::Reservation {
            record: Box::new(record),
        },
    )?)
}

fn add_source(
    change: &mut PreparedLiveLedgerCommit,
    current: Option<&super::super::source::LiveSourceRow>,
    replacement: super::super::source::LiveSourceRow,
) -> TestResult {
    if let Some(current) = current {
        change.successor.payload.used = change
            .successor
            .payload
            .used
            .checked_sub(current.charge()?)?;
    }
    change.successor.payload.used = change
        .successor
        .payload
        .used
        .checked_add(replacement.charge()?)?;
    change
        .sources
        .push(super::super::source::PreparedLiveSourceMutation {
            expected: current.map(super::super::source::LiveSourceRow::digest),
            replacement,
        });
    Ok(())
}

#[tokio::test]
async fn live_input_materializes_frozen_source_across_admission_and_cancellation() -> TestResult {
    use crate::live_request::{LiveExecutionRequestRecord, LiveRequestMaterializationError};
    use crate::live_source::LiveSourceEntryRecord;
    use meerkat_core::lifecycle::InputId;
    use meerkat_core::lifecycle::run_primitive::{ConversationAppendRole, CoreRenderable};
    use meerkat_core::live_execution::evidence::DelegatedRequestProvenance;

    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let source = reserved_source(&fixture, "materialize").await?;
        let LiveSourceEntryRecord::Reservation { record } = source.record()? else {
            return Err("expected reservation fixture".into());
        };
        let evidence = record.frozen_request().ok_or("evidence")?;
        let provenance = DelegatedRequestProvenance::new(
            record.request_id().clone(),
            record.source().clone(),
            evidence.kind(),
            evidence.request().digest(),
        )?;
        let reference = LiveExecutionRequestRecord::LiveRequest {
            provenance: provenance.clone(),
            source_row: record.frozen_digest()?,
        };
        let input_id = InputId::new();
        assert!(matches!(
            reference
                .materialize(fixture.store.as_ref(), &input_id)
                .await,
            Err(LiveRequestMaterializationError::MissingSource)
        ));
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut reserve = prepared(fixture.session.id(), Some(&before), vec![])?;
        reserve.expected_actor = Some(fixture.actor().await?);
        add_source(&mut reserve, None, source.clone())?;
        fixture
            .ops()?
            .commit_live_ledger(reserve, current_fence())
            .await?;
        assert!(matches!(
            reference
                .materialize(fixture.store.as_ref(), &input_id)
                .await,
            Err(LiveRequestMaterializationError::NotAdmitted)
        ));

        let mut image = serde_json::to_value(source.record()?)?;
        image["record"]["disposition"] = serde_json::json!({
            "kind": "admitted",
            "receipt": {
                "source": record.source(), "input_id": input_id,
                "executor": {
                    "session_id": fixture.session.id(), "realm": "owner",
                    "runtime_epoch": uuid::Uuid::new_v4(), "binding_generation": 1
                },
                "grant": record.grant().ok_or("grant")?,
                "ingress_generation_at_admission": 1,
                "commit": {"revision": before.reference.revision + 2, "digest": vec![1;32]}
            }
        });
        let mut previous = source;
        for cancelled in [false, true] {
            if cancelled {
                image["record"]["cancellation"] = serde_json::json!("operator_requested");
            }
            let replacement = super::super::source::LiveSourceRow::encode(
                &serde_json::from_value(image.clone())?,
            )?;
            assert_ne!(previous.digest(), replacement.digest());
            let LiveSourceEntryRecord::Reservation { record: updated } = replacement.record()?
            else {
                return Err("expected replacement reservation".into());
            };
            assert_eq!(updated.frozen_digest()?, record.frozen_digest()?);
            let before = fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?;
            let mut mutation = prepared(fixture.session.id(), Some(&before), vec![])?;
            add_source(&mut mutation, Some(&previous), replacement.clone())?;
            fixture
                .ops()?
                .commit_live_ledger(mutation, current_fence())
                .await?;
            let append = reference
                .materialize(fixture.store.as_ref(), &input_id)
                .await?
                .ok_or("original request must materialize a delegated append")?;
            assert_eq!(
                append.role,
                ConversationAppendRole::DelegatedRequest {
                    provenance: Box::new(provenance.clone())
                }
            );
            assert_eq!(
                append.content,
                CoreRenderable::Text {
                    text: " original request ".into()
                }
            );
            assert!(matches!(
                reference
                    .materialize(fixture.store.as_ref(), &InputId::new())
                    .await,
                Err(LiveRequestMaterializationError::AdmissionMismatch)
            ));
            previous = replacement;
        }
    }
    Ok(())
}

#[tokio::test]
async fn source_cas_commits_with_head_events_and_preserves_immutable_reservation() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let start = prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?;
        fixture
            .ops()?
            .commit_live_ledger(start, current_fence())
            .await?;
        let source = reserved_source(&fixture, "\0opaque-source-key").await?;
        assert!(
            fixture
                .ops()?
                .lookup_live_source(source.source())
                .await?
                .is_none()
        );
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut reserve = prepared(
            fixture.session.id(),
            Some(&before),
            vec![observation(2, "concurrent append")?],
        )?;
        reserve.expected_actor = Some(fixture.actor().await?);
        add_source(&mut reserve, None, source.clone())?;
        let replay = copy_prepared(&reserve);
        fixture
            .ops()?
            .commit_live_ledger(reserve, current_fence())
            .await?;
        let stored = fixture
            .ops()?
            .lookup_live_source(source.source())
            .await?
            .ok_or("source")?;
        assert_eq!(stored.bytes(), source.bytes());
        assert_eq!(stored.digest(), source.digest());
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        let current_head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut duplicate = prepared(fixture.session.id(), Some(&current_head), vec![])?;
        add_source(&mut duplicate, None, source.clone())?;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(duplicate, current_fence())
                .await?,
            LiveLedgerCommitOutcome::SourceConflict { .. }
        ));
        for field in [
            "request_id",
            "fingerprint",
            "frozen_request",
            "grant",
            "context",
        ] {
            let mut value = serde_json::to_value(source.record()?)?;
            let record = &mut value["record"];
            match field {
                "request_id" => record[field] = serde_json::json!(uuid::Uuid::new_v4()),
                "fingerprint" => record[field] = serde_json::json!(vec![9; 32]),
                "frozen_request" => record[field]["request"] = serde_json::json!("changed request"),
                "grant" => record[field]["generation"] = serde_json::json!(2),
                "context" => record[field]["actor"]["revision"] = serde_json::json!(2),
                _ => unreachable!(),
            }

            let replacement =
                super::super::source::LiveSourceRow::encode(&serde_json::from_value(value)?)?;
            let mut mutation = prepared(fixture.session.id(), Some(&current_head), vec![])?;
            add_source(&mut mutation, Some(&source), replacement)?;
            assert!(
                fixture
                    .ops()?
                    .commit_live_ledger(mutation, current_fence())
                    .await
                    .is_err(),
                "{backend:?} {field}"
            );
            assert_eq!(
                fixture
                    .ops()?
                    .lookup_live_source(source.source())
                    .await?
                    .ok_or("source")?
                    .bytes(),
                source.bytes()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn source_replacement_charges_deltas_and_replay_binds_all_source_expectations() -> TestResult
{
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let source = reserved_source(&fixture, "replace").await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut initial = prepared(fixture.session.id(), Some(&before), vec![])?;
        initial.expected_actor = Some(fixture.actor().await?);
        add_source(&mut initial, None, source.clone())?;
        fixture
            .ops()?
            .commit_live_ledger(initial, current_fence())
            .await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut value = serde_json::to_value(source.record()?)?;
        value["record"]["cancellation"] = serde_json::json!("operator_requested");
        value["record"]["disposition"] =
            serde_json::json!({"kind":"cancelled_without_run","reason":"operator_requested"});
        let replacement =
            super::super::source::LiveSourceRow::encode(&serde_json::from_value(value)?)?;
        let mut update = prepared(fixture.session.id(), Some(&before), vec![])?;
        add_source(&mut update, Some(&source), replacement.clone())?;
        let replay = copy_prepared(&update);
        fixture
            .ops()?
            .commit_live_ledger(update, current_fence())
            .await?;
        let after = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after.payload.used.records, before.payload.used.records);
        assert_eq!(
            after.payload.used.encoded_bytes,
            before.payload.used.encoded_bytes - source.charge()?.encoded_bytes
                + replacement.charge()?.encoded_bytes
        );
        let mut changed_expectation = copy_prepared(&replay);
        changed_expectation.sources[0].expected = None;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(changed_expectation, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        let mut omitted = copy_prepared(&replay);
        omitted.sources.clear();
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(omitted, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn initial_source_reservation_requires_its_composite_actor_and_head_fence() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let source = reserved_source(&fixture, "fenced").await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut missing_actor = prepared(fixture.session.id(), Some(&head), vec![])?;
        add_source(&mut missing_actor, None, source.clone())?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(missing_actor, current_fence())
                .await
                .is_err()
        );
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(
                    fixture.session.id(),
                    Some(&head),
                    vec![observation(2, "new frontier")?],
                )?,
                current_fence(),
            )
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut wrong_frontier = prepared(fixture.session.id(), Some(&head), vec![])?;
        wrong_frontier.expected_actor = Some(fixture.actor().await?);
        add_source(&mut wrong_frontier, None, source.clone())?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(wrong_frontier, current_fence())
                .await
                .is_err()
        );
        assert!(
            fixture
                .ops()?
                .lookup_live_source(source.source())
                .await?
                .is_none()
        );
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_late_source_failure_rolls_back_all_sources_head_events_and_receipt() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let first = reserved_source(&fixture, "first").await?;
        let second = reserved_source(&fixture, "second").await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut change = prepared(
            fixture.session.id(),
            Some(&head),
            vec![observation(2, "atomic")?],
        )?;
        change.expected_actor = Some(fixture.actor().await?);
        add_source(&mut change, None, first.clone())?;
        add_source(&mut change, None, second.clone())?;
        let conn = meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        let receipt: Vec<u8> =
            conn.query_row("SELECT commit_digest FROM runtime_live_heads", [], |row| {
                row.get(0)
            })?;
        conn.execute_batch(
            "CREATE TRIGGER reject_second_source BEFORE INSERT ON runtime_live_sources
             WHEN (SELECT count(*) FROM runtime_live_sources)=1
             BEGIN SELECT RAISE(ABORT, 'injected late source failure'); END;",
        )?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await
                .is_err()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
        assert!(
            fixture
                .ops()?
                .lookup_live_source(first.source())
                .await?
                .is_none()
        );
        assert!(
            fixture
                .ops()?
                .lookup_live_source(second.source())
                .await?
                .is_none()
        );
        assert_eq!(
            conn.query_row("SELECT commit_digest FROM runtime_live_heads", [], |row| {
                row.get::<_, Vec<u8>>(0)
            })?,
            receipt
        );
        conn.execute_batch("DROP TRIGGER reject_second_source")?;
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        assert_eq!(
            fixture
                .ops()?
                .lookup_live_source(first.source())
                .await?
                .ok_or("source")?
                .bytes(),
            first.bytes()
        );
        assert_eq!(
            fixture
                .ops()?
                .lookup_live_source(second.source())
                .await?
                .ok_or("source")?
                .bytes(),
            second.bytes()
        );
    }
    Ok(())
}

#[tokio::test]
async fn source_key_bytes_and_external_fence_are_part_of_atomic_publication() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let source = reserved_source(&fixture, &"\0".repeat(64)).await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut change = prepared(fixture.session.id(), Some(&head), vec![])?;
        change.expected_actor = Some(fixture.actor().await?);
        add_source(&mut change, None, source.clone())?;
        let mut missing_key_charge = copy_prepared(&change);
        missing_key_charge.successor.payload.used.encoded_bytes -=
            super::super::source::encoded_source_identity(source.source())?.len() as u64;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(missing_key_charge, current_fence())
                .await
                .is_err()
        );
        let fence = Arc::new(Fence(RuntimeStoreWriteFenceOutcome::Conflict {
            reason: "revoked epoch".into(),
        }));
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), fence)
                .await,
            Err(RuntimeStoreError::WriteFenceConflict { .. })
        ));
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
        assert!(
            fixture
                .ops()?
                .lookup_live_source(source.source())
                .await?
                .is_none()
        );
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn source_rows_survive_reopen_and_corruption_cannot_become_an_absent_row() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        for corrupt in [
            "UPDATE runtime_live_sources SET record=zeroblob(1048576)",
            "UPDATE runtime_live_sources SET record_digest=zeroblob(1048576)",
            "UPDATE runtime_live_sources SET record_digest=zeroblob(32)",
            "PRAGMA foreign_keys=OFF; DELETE FROM runtime_live_heads; DELETE FROM runtime_live_events;",
        ] {
            let fixture = Fixture::new(backend).await?;
            fixture
                .ops()?
                .commit_live_ledger(
                    prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                    current_fence(),
                )
                .await?;
            let source = reserved_source(&fixture, "reopen").await?;
            let head = fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?;
            let mut change = prepared(fixture.session.id(), Some(&head), vec![])?;
            change.expected_actor = Some(fixture.actor().await?);
            add_source(&mut change, None, source.clone())?;
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await?;
            let reopened = match backend {
                Backend::WholeBlob => {
                    crate::store::SqliteRuntimeStore::new_whole_blob(&fixture.path)?
                }
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&fixture.path)?
                }
                Backend::Memory => unreachable!(),
            };
            let loaded = reopened
                .lookup_live_source(source.source())
                .await?
                .ok_or("source")?;
            assert_eq!(loaded.bytes(), source.bytes());
            assert_eq!(loaded.digest(), source.digest());
            let conn =
                meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
            conn.pragma_update(None, "ignore_check_constraints", "ON")?;
            conn.execute_batch(corrupt)?;
            assert!(
                matches!(
                    reopened.lookup_live_source(source.source()).await,
                    Err(RuntimeStoreError::ReadFailed(_))
                ),
                "{backend:?} {corrupt}"
            );
            if corrupt.contains("DELETE") {
                assert!(
                    read_live_composite(
                        &reopened,
                        LiveCompositeReadRequest::new(fixture.session.id().clone(), None, 0, 64,)?
                    )
                    .await
                    .is_err()
                );
                assert!(
                    reopened
                        .commit_live_ledger(
                            prepared(fixture.session.id(), None, vec![])?,
                            current_fence()
                        )
                        .await
                        .is_err()
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn source_channel_storage_bounds_match_across_backends_before_publication() -> TestResult {
    use crate::live_source::LiveSourceEntryRecord;
    use meerkat_core::live_execution::LiveChannelId;
    use meerkat_core::live_execution::request::{
        LiveProviderReference, LiveRequestCancelIntent, LiveRequestCancellationReason,
        LiveSourceIdentity, LiveSourceKey,
    };
    let mut violations = Vec::new();
    for backend in backends() {
        for (channel, valid) in [
            ("x".repeat(128), true),
            ("x".repeat(129), false),
            ("\u{e9}".repeat(64), true),
            (format!("{}x", "\u{e9}".repeat(64)), false),
            ("\u{1f680}".repeat(32), true),
            (format!("{}x", "\u{1f680}".repeat(32)), false),
        ] {
            let fixture = Fixture::new(backend).await?;
            let source = LiveSourceKey::new(
                fixture.session.id().clone(),
                LiveChannelId::new(&channel),
                LiveSourceIdentity::ClientDelegation {
                    delegation: LiveProviderReference::new("d")?,
                },
            )?;
            let record = LiveSourceEntryRecord::CancellationOnly {
                intent: LiveRequestCancelIntent {
                    source: source.clone(),
                    reason: LiveRequestCancellationReason::OperatorRequested,
                },
            };
            match super::super::source::LiveSourceRow::encode(&record) {
                Ok(row) => {
                    let mut change = prepared(fixture.session.id(), None, vec![])?;
                    add_source(&mut change, None, row)?;
                    let result = fixture
                        .ops()?
                        .commit_live_ledger(change, current_fence())
                        .await;
                    if valid {
                        assert!(matches!(result?, LiveLedgerCommitOutcome::Committed { .. }));
                        assert!(fixture.ops()?.lookup_live_source(&source).await?.is_some());
                    } else {
                        violations.push(format!(
                            "{backend:?}: channel_bytes={} encode accepted; publication={result:?}",
                            channel.len()
                        ));
                    }
                }
                Err(error) => {
                    assert!(!valid, "{backend:?}: valid boundary refused: {error}");
                    assert!(
                        fixture
                            .ops()?
                            .load_live_head(fixture.session.id())
                            .await?
                            .is_none()
                    );
                    assert!(fixture.ops()?.lookup_live_source(&source).await.is_err());
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "invalid source keys reached publication: {violations:?}"
    );
    Ok(())
}

#[tokio::test]
async fn captured_history_prefix_survives_later_event_and_metadata_commits() -> TestResult {
    use crate::store::live_history::LiveHistoryReadRequest;
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let first = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "one")?, observation(2, "two")?],
        )?;
        let event_head = first.successor.reference.clone();
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let metadata = prepared(fixture.session.id(), Some(&before), vec![])?;
        let metadata_head = metadata.successor.reference.clone();
        fixture
            .ops()?
            .commit_live_ledger(metadata, current_fence())
            .await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(
                    fixture.session.id(),
                    Some(&before),
                    vec![observation(3, "not in captured prefix")?],
                )?,
                current_fence(),
            )
            .await?;
        for captured in [event_head, metadata_head] {
            let mut after = 0;
            let mut reconstructed = Vec::new();
            loop {
                let request = LiveHistoryReadRequest::new(captured.clone(), None, after, 1)?;
                let window = fixture.ops()?.read_live_history(&request).await?;
                assert_eq!(window.head(), &captured);
                for record in window.records() {
                    reconstructed.push(record.encode()?);
                }
                if !window.has_more() {
                    break;
                }
                let next = window
                    .records()
                    .last()
                    .ok_or("non-progressing page")?
                    .sequence()
                    .get();
                assert!(next > after);
                after = next;
            }
            assert_eq!(
                reconstructed,
                vec![
                    observation(1, "one")?.encode()?,
                    observation(2, "two")?.encode()?
                ]
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn committed_observation_pages_skip_control_windows_without_losing_progress() -> TestResult {
    use crate::live_ledger::completion::{
        LiveChannelControlOutcome, LiveCompletionEvent, LiveCompletionRecord, LiveCompletionText,
    };
    use crate::live_ledger::history::{LiveObservationHistoryQuery, read_observation_page};
    use meerkat_contracts::wire::live_observation::{
        LiveObservationCoverage, LiveObservationFilter, LiveObservationOwner,
        LiveObservationWireCodecV1,
    };
    use meerkat_core::live_execution::LiveChannelId;
    use meerkat_core::live_observation::LiveObservationSeq;

    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut records = Vec::new();
        for sequence in 1..=270 {
            records.push(if [2, 269].contains(&sequence) {
                observation(sequence, &format!("exact {sequence}\n\\\0"))?
            } else {
                LiveLedgerRecord::Completion(LiveCompletionRecord {
                    format: LiveLedgerFormatV1::V1,
                    session_id: fixture.session.id().clone(),
                    channel_id: LiveChannelId::new("voice"),
                    sequence: LiveObservationSeq::new(sequence)?,
                    event: LiveCompletionEvent::ChannelControl {
                        outcome: LiveChannelControlOutcome::RecoveryFenced,
                        diagnostic: LiveCompletionText::new("control")?,
                    },
                })
            });
        }
        // Individual real CAS batches stay inside the writer's bounded window.
        for chunk in records.chunks(100) {
            let before = fixture.ops()?.load_live_head(fixture.session.id()).await?;
            fixture
                .ops()?
                .commit_live_ledger(
                    prepared(fixture.session.id(), before.as_ref(), chunk.to_vec())?,
                    current_fence(),
                )
                .await?;
        }
        let captured = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(
                    fixture.session.id(),
                    Some(&captured),
                    vec![observation(271, "later")?],
                )?,
                current_fence(),
            )
            .await?;
        let mut cursor = None;
        for expected in [2, 269] {
            let page = read_observation_page(
                fixture.ops()?,
                LiveObservationHistoryQuery {
                    owner: LiveObservationOwner::Session {
                        session_id: fixture.session.id().clone(),
                    },
                    filter: LiveObservationFilter::AllChannels {},
                    head: captured.reference.clone(),
                    coverage: LiveObservationCoverage::CompleteAcceptedPrefix,
                    cursor,
                    limit: 1,
                },
            )
            .await?;
            assert_eq!(page.records.len(), 1);
            assert_eq!(page.records[0].sequence.get(), expected);
            assert_eq!(page.has_more, expected == 2);
            LiveObservationWireCodecV1::encode_reply(&page)?;
            cursor = page.next_cursor;
        }
        assert!(cursor.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn a_middle_of_atomic_batch_is_not_a_historical_head_even_with_its_real_prefix_hash()
-> TestResult {
    use crate::store::live_history::{LiveHistoryReadError, LiveHistoryReadRequest};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let first = prepared(
            fixture.session.id(),
            None,
            vec![
                observation(1, "one")?,
                observation(2, "two")?,
                observation(3, "three")?,
            ],
        )?;
        let mut invented = first.successor.reference.clone();
        invented.event_count = 2;
        invented.prefix_digest = first
            .prefix_witnesses(&first.encoded_records()?)
            .nth(1)
            .ok_or("prefix witness")?
            .prefix;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let request = LiveHistoryReadRequest::new(invented, None, 0, 64)?;
        assert!(matches!(
            fixture.ops()?.read_live_history(&request).await,
            Err(LiveHistoryReadError::InvalidSnapshot)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn head_and_events_commit_together_without_changing_actor_authority() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let actor = fixture.actor().await?;
        let ops = fixture.ops()?;
        assert_eq!(
            ops.ledger_write_profile(),
            LiveLedgerWriteProfile::AtomicHeadEventsSourcesLifecycleAdmissionStageExecution
        );
        let change = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "one")?, observation(2, "two")?],
        )?;
        let expected = change.successor.clone();
        assert_eq!(
            ops.commit_live_ledger(change, current_fence()).await?,
            LiveLedgerCommitOutcome::Committed {
                head: expected.reference.clone()
            }
        );
        assert_eq!(
            ops.load_live_head(fixture.session.id()).await?,
            Some(expected.clone())
        );
        assert_eq!(fixture.actor().await?, actor, "{backend:?}");
        let read = read_live_composite(
            ops,
            LiveCompositeReadRequest::new(fixture.session.id().clone(), None, 0, 64)?,
        )
        .await?
        .ok_or("composite")?;
        assert_eq!(read.authority().live_head(), Some(&expected.reference));
        assert_eq!(read.authority().actor(), &actor);
        assert_eq!(read.records().len(), 2);
        assert!(!read.has_more());
    }
    Ok(())
}

#[tokio::test]
async fn concurrent_different_successors_have_one_winner_and_exact_replay() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let a = prepared(fixture.session.id(), None, vec![observation(1, "a")?])?;
        let b = prepared(fixture.session.id(), None, vec![observation(1, "b")?])?;
        let ops = fixture.ops()?;
        let (left, right) = tokio::join!(
            ops.commit_live_ledger(copy_prepared(&a), current_fence()),
            ops.commit_live_ledger(copy_prepared(&b), current_fence()),
        );
        let (left, right) = (left?, right?);
        let winner = match (&left, &right) {
            (
                LiveLedgerCommitOutcome::Committed { .. },
                LiveLedgerCommitOutcome::Conflict { .. },
            ) => a,
            (
                LiveLedgerCommitOutcome::Conflict { .. },
                LiveLedgerCommitOutcome::Committed { .. },
            ) => b,
            _ => return Err(format!("non-exclusive CAS: {backend:?} {left:?} {right:?}").into()),
        };
        assert!(matches!(
            ops.commit_live_ledger(winner, current_fence()).await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        assert_eq!(
            ops.load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?
                .reference
                .event_count,
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn same_key_or_head_does_not_substitute_for_exact_replay_bytes() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "original")?],
        )?;
        let mut replay = copy_prepared(&change);
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        replay.records[0] = observation(1, "different")?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await
                .is_err()
        );
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut replay = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "original")?],
        )?;
        replay.successor.payload.reserved.records = 1;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
    }
    Ok(())
}

#[tokio::test]
async fn alternate_predecessor_and_suffix_are_not_the_original_committed_operation() -> TestResult {
    let mut false_replays = Vec::new();
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let first = prepared(fixture.session.id(), None, vec![observation(1, "r1")?])?;
        let first_head = first.successor.clone();
        let never_committed = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "r1")?, observation(2, "r2")?],
        )?
        .successor;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let real = prepared(
            fixture.session.id(),
            Some(&first_head),
            vec![observation(2, "r2")?, observation(3, "r3")?],
        )?;
        let suffix = prepared(
            fixture.session.id(),
            Some(&never_committed),
            vec![observation(3, "r3")?],
        )?;
        assert_ne!(real.expected, suffix.expected);
        assert_eq!(real.successor, suffix.successor);
        assert!(suffix.encoded_records().is_ok());
        fixture
            .ops()?
            .commit_live_ledger(real, current_fence())
            .await?;
        let result = fixture
            .ops()?
            .commit_live_ledger(suffix, current_fence())
            .await?;
        if !matches!(result, LiveLedgerCommitOutcome::Conflict { .. }) {
            false_replays.push(format!("{backend:?}: {result:?}"));
        }
    }
    assert!(
        false_replays.is_empty(),
        "different operations treated as exact replay: {false_replays:?}"
    );
    Ok(())
}

#[tokio::test]
async fn exact_replay_binds_actor_expectation_and_quota_even_when_successor_matches() -> TestResult
{
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(fixture.session.id(), None, vec![observation(1, "r1")?])?;
        fixture
            .ops()?
            .commit_live_ledger(copy_prepared(&change), current_fence())
            .await?;
        let mut altered_actor = copy_prepared(&change);
        altered_actor.expected_actor = Some(fixture.actor().await?);
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(altered_actor, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        let mut altered_quota = copy_prepared(&change);
        altered_quota.quota.encoded_bytes -= 1;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(altered_quota, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn oversized_corrupt_replay_rows_and_head_metadata_fail_before_blob_fetch() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        for corruption in [
            "UPDATE runtime_live_events SET record=zeroblob(1048576)",
            "UPDATE runtime_live_events SET record_digest=zeroblob(1048576)",
            "UPDATE runtime_live_events SET channel_id=CAST(zeroblob(1048576) AS TEXT)",
            "UPDATE runtime_live_heads SET commit_digest=zeroblob(1048576)",
            "UPDATE runtime_live_heads SET prefix_digest=zeroblob(1048576)",
            "UPDATE runtime_live_heads SET transcript_snapshot=zeroblob(1048576)",
            "UPDATE runtime_live_heads SET request_snapshot=zeroblob(1048576)",
        ] {
            let fixture = Fixture::new(backend).await?;
            let change = prepared(fixture.session.id(), None, vec![observation(1, "r1")?])?;
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await?;
            let conn =
                meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
            conn.pragma_update(None, "ignore_check_constraints", "ON")?;
            conn.execute_batch(corruption)?;
            let result = fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await;
            assert!(
                matches!(result, Err(RuntimeStoreError::ReadFailed(ref error))
                if error.contains("bound") || error.contains("width")),
                "{backend:?}: {corruption}: {result:?}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn replay_cannot_omit_any_original_batch_member() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "a")?, observation(2, "b")?],
        )?;
        let mut replay = copy_prepared(&change);
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        replay.records.pop();
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&replay), current_fence())
                .await
                .is_err()
        );
        replay.records.clear();
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await
                .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn exact_concurrent_replays_append_the_batch_once() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "a")?, observation(2, "b")?],
        )?;
        let ops = fixture.ops()?;
        let (left, right) = tokio::join!(
            ops.commit_live_ledger(copy_prepared(&change), current_fence()),
            ops.commit_live_ledger(change, current_fence()),
        );
        let results = [left?, right?];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, LiveLedgerCommitOutcome::Committed { .. }))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, LiveLedgerCommitOutcome::AlreadyCommitted { .. }))
                .count(),
            1
        );
        assert_eq!(
            ops.load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?
                .reference
                .event_count,
            2
        );
    }
    Ok(())
}

#[tokio::test]
async fn current_actor_identity_cannot_be_replaced_with_another_sessions_authority() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let other = Fixture::new(backend).await?;
        let mut change = prepared(fixture.session.id(), None, vec![observation(1, "a")?])?;
        change.expected_actor = Some(other.actor().await?);
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await?,
            LiveLedgerCommitOutcome::ActorConflict { .. }
        ));
        assert!(
            fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .is_none()
        );
    }
    Ok(())
}

#[tokio::test]
async fn reserved_bytes_cannot_be_reused_by_unfunded_new_records() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let record = observation(1, "funded")?;
        let charge = LiveResourceCharge::for_event_record(&record.encode()?)?;
        let mut first = prepared(fixture.session.id(), None, vec![])?;
        first.successor.payload.reserved = charge;
        let quota = first.successor.payload.used.checked_add(charge)?;
        first.quota = quota;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut next = prepared(fixture.session.id(), Some(&head), vec![record])?;
        next.quota = quota;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&next), current_fence())
                .await
                .is_err()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
        next.successor.payload.reserved = LiveResourceCharge::default();
        fixture
            .ops()?
            .commit_live_ledger(next, current_fence())
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(head.payload.used, quota);
        assert_eq!(head.payload.reserved, LiveResourceCharge::default());
    }
    Ok(())
}

#[tokio::test]
async fn last_operation_receipt_has_explicit_once_per_head_storage_charge() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut change = prepared(fixture.session.id(), None, vec![])?;
        change.quota.encoded_bytes = crate::live_resources::LIVE_RECORD_STORAGE_ALLOWANCE_BYTES;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await
                .is_err()
        );
        change.quota.encoded_bytes += 32;
        assert_eq!(
            change.successor.payload.used.encoded_bytes,
            change.quota.encoded_bytes
        );
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        let first = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let next = prepared(fixture.session.id(), Some(&first), vec![])?;
        fixture
            .ops()?
            .commit_live_ledger(next, current_fence())
            .await?;
        let second = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(first.payload.used, second.payload.used);
    }
    Ok(())
}

#[tokio::test]
async fn snapshots_replace_their_charge_and_control_only_commits_are_fenced() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut first = prepared(fixture.session.id(), None, vec![observation(1, "one")?])?;
        first.successor.payload.transcript_snapshot = Arc::new(vec![17; 4096]);
        first.successor.payload.request_snapshot = Arc::new(vec![31; 1024]);
        first.successor.payload.used.encoded_bytes += 5120;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let old = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut next = prepared(fixture.session.id(), Some(&old), vec![])?;
        next.successor.payload.transcript_snapshot = Arc::new(vec![1]);
        next.successor.payload.request_snapshot = Arc::new(vec![2]);
        next.successor.payload.used.encoded_bytes -= 5118;
        next.successor.payload.ingress_generation += 1;
        fixture
            .ops()?
            .commit_live_ledger(next, current_fence())
            .await?;
        let new = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(new.payload.used.records, 1);
        assert_eq!(
            new.payload.used.encoded_bytes + 5118,
            old.payload.used.encoded_bytes
        );
        assert_eq!(new.reference.prefix_digest, old.reference.prefix_digest);
        assert_eq!(new.payload.ingress_generation, 2);
    }
    Ok(())
}

#[tokio::test]
async fn invalid_prepared_counters_prefix_and_scope_never_publish_partial_state() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        for defect in 0..13 {
            let mut change = prepared(
                fixture.session.id(),
                None,
                vec![observation(1, "one")?, observation(2, "two")?],
            )?;
            match defect {
                0 => change.successor.reference.event_count += 1,
                1 => {
                    change.successor.reference.prefix_digest =
                        LiveLedgerPrefixDigest::from_sha256([0; 32]);
                }
                2 => change.successor.reference.revision += 1,
                3 => change.successor.payload.used.records -= 1,
                4 => change.successor.payload.used.encoded_bytes -= 1,
                5 => change.successor.payload.used.encoded_bytes += 1,
                6 => {
                    change.successor.payload.reserved.encoded_bytes =
                        LIVE_LEDGER_MAX_CHARGE.encoded_bytes;
                }
                7 => change.successor.payload.ingress_generation = 0,
                8 => change.records[1] = observation(1, "duplicate")?,
                9 => change.records[1] = observation(3, "gap")?,
                10 => change.successor.reference.generation = 0,
                11 => change.successor.reference.generation = u64::MAX,
                12 => change.successor.payload.request_snapshot = Arc::new(vec![1]),
                _ => unreachable!(),
            }
            assert!(
                fixture
                    .ops()?
                    .commit_live_ledger(change, current_fence())
                    .await
                    .is_err(),
                "{backend:?} defect={defect}"
            );
            assert!(
                fixture
                    .ops()?
                    .load_live_head(fixture.session.id())
                    .await?
                    .is_none()
            );
        }
        let good = prepared(fixture.session.id(), None, vec![observation(1, "success")?])?;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(good, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Committed { .. }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn external_epoch_fence_conflict_and_backoff_leave_no_head_or_events() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        for decision in [
            RuntimeStoreWriteFenceOutcome::Conflict {
                reason: "revoked".into(),
            },
            RuntimeStoreWriteFenceOutcome::Backoff {
                reason: "unavailable".into(),
            },
        ] {
            let change = prepared(fixture.session.id(), None, vec![observation(1, "one")?])?;
            let result = fixture
                .ops()?
                .commit_live_ledger(change, Arc::new(Fence(decision.clone())))
                .await;
            match decision {
                RuntimeStoreWriteFenceOutcome::Conflict { .. } => assert!(matches!(
                    result,
                    Err(RuntimeStoreError::WriteFenceConflict { .. })
                )),
                RuntimeStoreWriteFenceOutcome::Backoff { .. } => assert!(matches!(
                    result,
                    Err(RuntimeStoreError::WriteFenceBackoff { .. })
                )),
                RuntimeStoreWriteFenceOutcome::Applied => unreachable!(),
            }
            assert!(
                fixture
                    .ops()?
                    .load_live_head(fixture.session.id())
                    .await?
                    .is_none()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn actor_fence_is_exact_but_ordinary_live_append_commutes_with_actor_writes() -> TestResult {
    for backend in [
        Backend::Memory,
        #[cfg(feature = "sqlite-store")]
        Backend::WholeBlob,
    ] {
        let fixture = Fixture::new(backend).await?;
        let old_actor = fixture.actor().await?;
        let mut first = prepared(fixture.session.id(), None, vec![observation(1, "one")?])?;
        first.expected_actor = Some(old_actor.clone());
        let replay = copy_prepared(&first);
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        assert_eq!(fixture.actor().await?, old_actor);
        let mut updated = fixture.session.clone();
        updated.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("ordinary turn"),
        ));
        save_actor(fixture.store.as_ref(), &updated).await?;
        let current_actor = fixture.actor().await?;
        assert_ne!(current_actor, old_actor);
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut stale = prepared(
            fixture.session.id(),
            Some(&head),
            vec![observation(2, "next")?],
        )?;
        stale.expected_actor = Some(old_actor);
        assert_eq!(
            fixture
                .ops()?
                .commit_live_ledger(stale, current_fence())
                .await?,
            LiveLedgerCommitOutcome::ActorConflict {
                current: Some(current_actor.clone())
            }
        );
        let independent = prepared(
            fixture.session.id(),
            Some(&head),
            vec![observation(2, "next")?],
        )?;
        fixture
            .ops()?
            .commit_live_ledger(independent, current_fence())
            .await?;
        assert_eq!(fixture.actor().await?, current_actor);
    }
    Ok(())
}

#[tokio::test]
async fn missing_actor_cannot_acquire_a_live_head() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let missing = SessionId::new();
        let change = prepared(&missing, None, vec![observation(1, "one")?])?;
        assert_eq!(
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await?,
            LiveLedgerCommitOutcome::ActorConflict { current: None }
        );
        assert!(fixture.ops()?.load_live_head(&missing).await?.is_none());
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_late_insert_failure_rolls_back_head_events_and_snapshot_charges() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let first = prepared(fixture.session.id(), None, vec![observation(1, "one")?])?;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let conn = meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        conn.execute_batch(
            "CREATE TABLE unrelated_owner (value TEXT NOT NULL);
             INSERT INTO unrelated_owner VALUES ('preserve');
             CREATE TRIGGER reject_third BEFORE INSERT ON runtime_live_events
             WHEN NEW.sequence=3 BEGIN SELECT RAISE(ABORT, 'injected late failure'); END;",
        )?;
        let next = prepared(
            fixture.session.id(),
            Some(&head),
            vec![observation(2, "two")?, observation(3, "three")?],
        )?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&next), current_fence())
                .await
                .is_err()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
        let count: i64 = conn.query_row("SELECT count(*) FROM runtime_live_events", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);
        let foreign: String =
            conn.query_row("SELECT value FROM unrelated_owner", [], |row| row.get(0))?;
        assert_eq!(foreign, "preserve");
        conn.execute_batch("DROP TRIGGER reject_third")?;
        fixture
            .ops()?
            .commit_live_ledger(next, current_fence())
            .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_restart_replays_exact_payload_and_rejects_corrupted_replay() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(fixture.session.id(), None, vec![observation(1, "durable")?])?;
        let replay = copy_prepared(&change);
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        let reopened = match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&fixture.path)?,
            Backend::HeadCanonical => {
                crate::store::SqliteRuntimeStore::new_head_canonical(&fixture.path)?
            }
            Backend::Memory => unreachable!(),
        };
        assert_eq!(
            reopened.load_live_head(fixture.session.id()).await?,
            Some(replay.successor.clone())
        );
        assert!(matches!(
            reopened
                .commit_live_ledger(copy_prepared(&replay), current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        let conn = meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        conn.execute(
            "UPDATE runtime_live_events SET record_digest=zeroblob(32)",
            [],
        )?;
        assert!(
            reopened
                .commit_live_ledger(replay, current_fence())
                .await
                .is_err()
        );
    }
    Ok(())
}
