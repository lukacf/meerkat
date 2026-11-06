//! What a mob boot reads from the runtime store to plan which members it
//! materializes, in two store-wide reads that load no runtime body.

use std::collections::HashSet;

use super::MeerkatMachine;
use crate::delivery_inbox::{RuntimeDeliveryError, runtime_delivery_backlog_in};
use crate::identifiers::LogicalRuntimeId;
use crate::store::RuntimeStoreError;

/// What a mob boot reads to plan which members it materializes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBootObligations {
    /// Runtimes whose durable state holds recovery work, as far as the store
    /// can tell.
    pub recovery_work: RuntimeRecoveryWork,
    /// Runtimes holding committed-but-unapplied deliveries, and runtimes the
    /// store cannot answer for until it is reopened, which may hold them.
    pub pending_deliveries: HashSet<LogicalRuntimeId>,
}

/// Which runtimes hold recovery work, as far as the store can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeRecoveryWork {
    /// The store listed them.
    Observed(HashSet<LogicalRuntimeId>),
    /// The store cannot tell, for the typed reason: a boot reads every member
    /// as holding work.
    Unobserved(RuntimeRecoveryWorkUnobserved),
}

/// Why a store cannot tell which runtimes hold recovery work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRecoveryWorkUnobserved {
    /// The store does not implement the recovery-work scan
    /// ([`RuntimeStore::list_runtimes_with_recovery_work`](crate::store::RuntimeStore::list_runtimes_with_recovery_work)).
    RecoveryScanUnsupported,
    /// The store cannot list its delivery authorities, so a delivery may be
    /// pending for any runtime.
    DeliveryListingUnsupported,
}

/// What one session's runtime holds durably, read cold
/// ([`MeerkatMachine::session_work_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionWorkState {
    /// A registration of the session lives in this process. Its live
    /// authority is what counts (a run it marked resumable, the holds on its
    /// run starts and its queues live only there): read it live.
    Registered,
    /// The store holds no durable work for the session's runtime: no
    /// recovery work and no pending delivery.
    Idle,
    /// The store holds durable work for the session's runtime.
    HoldsWork(SessionDurableWork),
    /// The store cannot tell, for the typed cause. Never read it as idle.
    Unobserved(RuntimeRecoveryWorkUnobserved),
}

/// The durable work [`SessionWorkState::HoldsWork`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionDurableWork {
    /// The runtime holds recovery work, by the predicate a mob boot asks
    /// ([`RuntimeStore::list_runtimes_with_recovery_work`](crate::store::RuntimeStore::list_runtimes_with_recovery_work)):
    /// nonterminal inputs or owed operation work, pending terminal owners,
    /// a provisional tail (an open run), a lifecycle record that names a
    /// current run or does not decode, or input rows with no lifecycle
    /// record.
    pub recovery_work: bool,
    /// The runtime holds committed deliveries it has not applied, or the
    /// store cannot answer for it until it is reopened.
    pub pending_deliveries: bool,
}

impl MeerkatMachine {
    /// The runtimes of this machine's store that hold durable work: the
    /// runtimes with recovery work
    /// ([`RuntimeStore::list_runtimes_with_recovery_work`](crate::store::RuntimeStore::list_runtimes_with_recovery_work))
    /// and the runtimes with pending deliveries (the population
    /// [`RuntimeDeliveryInbox::runtime_delivery_backlog`](crate::RuntimeDeliveryInbox::runtime_delivery_backlog)
    /// reads). Two store-wide reads, neither of which loads a runtime body.
    ///
    /// A runtime the store cannot answer for until it is reopened (a host
    /// store whose scope of that runtime is poisoned) is an obligation: the
    /// recovery scan lists it as holding work, and it counts as holding
    /// pending deliveries. The answer stays observed for every other
    /// runtime, so one such runtime never makes a boot wake every member.
    ///
    /// A store that cannot list either answers
    /// [`RuntimeRecoveryWork::Unobserved`] with the read it could not make:
    /// the boot cannot tell which members hold work, so it materializes all
    /// of them. A machine without a store holds nothing durable: no recovery
    /// work and no pending deliveries.
    ///
    /// # Errors
    /// Any other store failure, and a delivery authority that does not
    /// decode.
    pub async fn boot_recovery_obligations(
        &self,
    ) -> Result<RuntimeBootObligations, RuntimeStoreError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(RuntimeBootObligations {
                recovery_work: RuntimeRecoveryWork::Observed(HashSet::new()),
                pending_deliveries: HashSet::new(),
            });
        };
        let recovery_work = match store.list_runtimes_with_recovery_work().await {
            Ok(runtimes) => RuntimeRecoveryWork::Observed(runtimes.into_iter().collect()),
            Err(RuntimeStoreError::Unsupported(_)) => RuntimeRecoveryWork::Unobserved(
                RuntimeRecoveryWorkUnobserved::RecoveryScanUnsupported,
            ),
            Err(error) => return Err(error),
        };
        let pending_deliveries = match runtime_delivery_backlog_in(store.as_ref()).await {
            Ok(backlog) => backlog
                .pending
                .into_iter()
                .chain(
                    backlog
                        .unavailable
                        .into_iter()
                        .map(|unavailable| unavailable.runtime_id),
                )
                .collect(),
            // Deliveries the store cannot list may be pending for any member.
            Err(RuntimeDeliveryError::Store(RuntimeStoreError::Unsupported(_))) => {
                return Ok(RuntimeBootObligations {
                    recovery_work: RuntimeRecoveryWork::Unobserved(
                        RuntimeRecoveryWorkUnobserved::DeliveryListingUnsupported,
                    ),
                    pending_deliveries: HashSet::new(),
                });
            }
            Err(RuntimeDeliveryError::Store(error)) => return Err(error),
            Err(error) => return Err(RuntimeStoreError::ReadFailed(error.to_string())),
        };
        Ok(RuntimeBootObligations {
            recovery_work,
            pending_deliveries,
        })
    }

    /// What `session_id`'s runtime holds durably, read cold.
    ///
    /// RED stub: answers an empty `HoldsWork` for every session.
    pub async fn session_work_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionWorkState, RuntimeStoreError> {
        let _ = session_id;
        Ok(SessionWorkState::HoldsWork(SessionDurableWork {
            recovery_work: false,
            pending_deliveries: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
    use meerkat_core::types::SessionId;

    use super::*;
    use crate::delivery_inbox::{
        RuntimeDeliveryId, RuntimeDeliveryInbox, RuntimeDeliveryKind, RuntimeDeliverySubmission,
    };
    use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
    use crate::runtime_state::RuntimeState;
    use crate::store::{
        InMemoryRuntimeStore, InputStateRow, MachineLifecycleBindingFacts, MachineLifecycleCommit,
        PreparedWholeBlobRewriteStoreParts, RuntimeDeliveryAuthorityRecord,
        RuntimeSessionAuthorityOps, RuntimeStore, SerializedSessionSnapshot,
        SupervisorAuthoritySnapshot, WholeBlobStoreAuthority,
    };

    /// How a [`ScriptedScanStore`] answers one of its two store-wide reads.
    #[derive(Clone, Copy)]
    enum Answer {
        /// The wrapped in-memory store answers.
        Forward,
        /// The read is not supported.
        Unsupported,
        /// The read fails.
        Fail,
    }

    impl Answer {
        fn refusal(self, what: &str) -> Option<RuntimeStoreError> {
            match self {
                Self::Forward => None,
                Self::Unsupported => Some(RuntimeStoreError::Unsupported(what.to_string())),
                Self::Fail => Some(RuntimeStoreError::ReadFailed(format!("{what} failed"))),
            }
        }
    }

    /// An in-memory store whose obligation scan and delivery listing answer
    /// as scripted; every other call is the wrapped store's.
    struct ScriptedScanStore {
        inner: Arc<InMemoryRuntimeStore>,
        recovery_scan: Answer,
        delivery_listing: Answer,
    }

    #[async_trait::async_trait]
    impl RuntimeStore for ScriptedScanStore {
        fn session_authority_ops(&self) -> &dyn RuntimeSessionAuthorityOps {
            self.inner.session_authority_ops()
        }

        async fn list_runtimes_with_recovery_work(
            &self,
        ) -> Result<Vec<LogicalRuntimeId>, RuntimeStoreError> {
            match self
                .recovery_scan
                .refusal("list_runtimes_with_recovery_work")
            {
                Some(refusal) => Err(refusal),
                None => self.inner.list_runtimes_with_recovery_work().await,
            }
        }

        async fn list_runtime_delivery_authorities(
            &self,
        ) -> Result<Vec<(LogicalRuntimeId, RuntimeDeliveryAuthorityRecord)>, RuntimeStoreError>
        {
            match self
                .delivery_listing
                .refusal("list_runtime_delivery_authorities")
            {
                Some(refusal) => Err(refusal),
                None => self.inner.list_runtime_delivery_authorities().await,
            }
        }

        async fn load_runtime_delivery_authority(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<RuntimeDeliveryAuthorityRecord>, RuntimeStoreError> {
            match self
                .delivery_listing
                .refusal("load_runtime_delivery_authority")
            {
                Some(refusal) => Err(refusal),
                None => self.inner.load_runtime_delivery_authority(runtime_id).await,
            }
        }

        async fn commit_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SerializedSessionSnapshot,
        ) -> Result<(), RuntimeStoreError> {
            self.inner
                .commit_session_snapshot(runtime_id, session_delta)
                .await
        }

        async fn commit_prepared_whole_blob_rewrite_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            boundary: PreparedWholeBlobRewriteStoreParts,
        ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
            self.inner
                .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
                .await
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<SerializedSessionSnapshot>,
            receipt: RunBoundaryReceipt,
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: Option<meerkat_core::types::SessionId>,
        ) -> Result<(), RuntimeStoreError> {
            self.inner
                .atomic_apply(
                    runtime_id,
                    session_delta,
                    receipt,
                    input_updates,
                    session_store_key,
                )
                .await
        }

        async fn load_input_states(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<InputStateRow>, RuntimeStoreError> {
            self.inner.load_input_states(runtime_id).await
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
            sequence: u64,
        ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
            self.inner
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Arc<Vec<u8>>>, RuntimeStoreError> {
            self.inner.load_session_snapshot(runtime_id).await
        }

        async fn clear_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<(), RuntimeStoreError> {
            self.inner.clear_session_snapshot(runtime_id).await
        }

        async fn replace_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
            replacement: Vec<u8>,
        ) -> Result<bool, RuntimeStoreError> {
            self.inner
                .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
                .await
        }

        async fn clear_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
        ) -> Result<bool, RuntimeStoreError> {
            self.inner
                .clear_session_snapshot_if_current(runtime_id, expected_current)
                .await
        }

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &InputStatePersistenceRecord,
        ) -> Result<(), RuntimeStoreError> {
            self.inner.persist_input_state(runtime_id, state).await
        }

        async fn load_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_id: &InputId,
        ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
            self.inner.load_input_state(runtime_id, input_id).await
        }

        async fn load_machine_lifecycle_record(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            self.inner.load_machine_lifecycle_record(runtime_id).await
        }

        async fn commit_machine_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            commit: MachineLifecycleCommit,
            input_states: &[InputStatePersistenceRecord],
        ) -> Result<(), RuntimeStoreError> {
            self.inner
                .commit_machine_lifecycle(runtime_id, commit, input_states)
                .await
        }
    }

    /// The runtimes a seeded store holds.
    struct Seeded {
        /// A nonterminal input.
        working: LogicalRuntimeId,
        /// A pending delivery.
        delivered: LogicalRuntimeId,
    }

    /// Seed `store` with a runtime holding a nonterminal input, a runtime
    /// holding a pending delivery, and an idle runtime holding neither.
    async fn seed(store: &Arc<InMemoryRuntimeStore>) -> Seeded {
        let working = LogicalRuntimeId::new("boot-obligations-working");
        store
            .persist_input_state(
                &working,
                &InputStatePersistenceRecord::from_machine_snapshot(
                    StoredInputState::new_accepted(InputId::new()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let delivered = LogicalRuntimeId::new("boot-obligations-delivered");
        RuntimeDeliveryInbox::new(Arc::clone(store) as Arc<dyn RuntimeStore>)
            .submit(
                &delivered,
                RuntimeDeliverySubmission::new(
                    RuntimeDeliveryId::new("boot-obligations-delivery").unwrap(),
                    RuntimeDeliveryKind::JobTerminal,
                    "boot-obligations-job",
                    1,
                    "boot-obligations-interaction",
                    b"payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let idle = LogicalRuntimeId::new("boot-obligations-idle");
        store
            .commit_machine_lifecycle(
                &idle,
                MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Idle,
                    MachineLifecycleBindingFacts::new(
                        Some(idle.0.clone()),
                        Some(1),
                        Some(1),
                        Some("epoch-1".to_string()),
                    ),
                    SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                &[],
            )
            .await
            .unwrap();
        Seeded { working, delivered }
    }

    fn scripted(
        inner: &Arc<InMemoryRuntimeStore>,
        recovery_scan: Answer,
        delivery_listing: Answer,
    ) -> MeerkatMachine {
        let store: Arc<dyn RuntimeStore> = Arc::new(ScriptedScanStore {
            inner: Arc::clone(inner),
            recovery_scan,
            delivery_listing,
        });
        MeerkatMachine::persistent_without_blobs(store).unwrap()
    }

    #[tokio::test]
    async fn boot_obligation_scan_machine_without_a_store_answers_no_work() {
        let obligations = MeerkatMachine::ephemeral()
            .boot_recovery_obligations()
            .await
            .unwrap();
        assert_eq!(
            obligations,
            RuntimeBootObligations {
                recovery_work: RuntimeRecoveryWork::Observed(HashSet::new()),
                pending_deliveries: HashSet::new(),
            }
        );
    }

    #[tokio::test]
    async fn boot_obligation_scan_machine_reads_recovery_work_and_pending_deliveries() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let seeded = seed(&store).await;
        let machine =
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store) as Arc<dyn RuntimeStore>)
                .unwrap();
        let obligations = machine.boot_recovery_obligations().await.unwrap();
        assert_eq!(
            obligations,
            RuntimeBootObligations {
                recovery_work: RuntimeRecoveryWork::Observed(HashSet::from([seeded.working])),
                pending_deliveries: HashSet::from([seeded.delivered]),
            }
        );
    }

    #[tokio::test]
    async fn boot_obligation_scan_machine_without_the_store_scan_answers_unobserved() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let seeded = seed(&store).await;
        let obligations = scripted(&store, Answer::Unsupported, Answer::Forward)
            .boot_recovery_obligations()
            .await
            .unwrap();
        assert_eq!(
            obligations,
            RuntimeBootObligations {
                recovery_work: RuntimeRecoveryWork::Unobserved(
                    RuntimeRecoveryWorkUnobserved::RecoveryScanUnsupported,
                ),
                pending_deliveries: HashSet::from([seeded.delivered]),
            }
        );
    }

    #[tokio::test]
    async fn boot_obligation_scan_machine_without_the_delivery_listing_answers_unobserved() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        seed(&store).await;
        let obligations = scripted(&store, Answer::Forward, Answer::Unsupported)
            .boot_recovery_obligations()
            .await
            .unwrap();
        assert_eq!(
            obligations,
            RuntimeBootObligations {
                recovery_work: RuntimeRecoveryWork::Unobserved(
                    RuntimeRecoveryWorkUnobserved::DeliveryListingUnsupported,
                ),
                pending_deliveries: HashSet::new(),
            }
        );
    }

    /// A host store realm with one poisoned runtime scope still answers the
    /// boot's scan, observed: the poisoned runtime is an obligation (its
    /// facts cannot be trusted, so it holds recovery work and may hold a
    /// pending delivery), and every healthy runtime answers as its memory
    /// holds it. Before, both reads refused, and every mob boot over the
    /// realm planned every member as holding work.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn a_poisoned_runtime_is_an_obligation_and_the_scan_stays_observed() {
        use crate::store::spi::test_host::TestHost;
        use crate::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};

        let host = Arc::new(TestHost::new());
        let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
            .await
            .expect("the realm opens");
        let store: Arc<dyn RuntimeStore> =
            Arc::new(SpiRuntimeStore::open(realm).await.expect("the store opens"));
        let input = |state| {
            InputStatePersistenceRecord::from_machine_snapshot(state).expect("a machine snapshot")
        };
        let working = LogicalRuntimeId::new("boot-obligations-working");
        store
            .persist_input_state(
                &working,
                &input(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .expect("a nonterminal input lands");
        let delivered = LogicalRuntimeId::new("boot-obligations-delivered");
        RuntimeDeliveryInbox::new(Arc::clone(&store))
            .submit(
                &delivered,
                RuntimeDeliverySubmission::new(
                    RuntimeDeliveryId::new("boot-obligations-delivery").unwrap(),
                    RuntimeDeliveryKind::JobTerminal,
                    "boot-obligations-job",
                    1,
                    "boot-obligations-interaction",
                    b"payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .expect("a delivery lands");
        let doomed = LogicalRuntimeId::new("boot-obligations-doomed");
        host.faults.lock().unwrap().conflict = 1;
        assert!(
            store
                .persist_input_state(
                    &doomed,
                    &input(StoredInputState::new_accepted(InputId::new()))
                )
                .await
                .is_err(),
            "the store refuses the batch, and the runtime's scope is poisoned"
        );

        let obligations = MeerkatMachine::persistent_without_blobs(store)
            .unwrap()
            .boot_recovery_obligations()
            .await
            .expect("the scan answers");
        assert_eq!(
            obligations,
            RuntimeBootObligations {
                recovery_work: RuntimeRecoveryWork::Observed(HashSet::from([
                    working,
                    doomed.clone(),
                ])),
                pending_deliveries: HashSet::from([delivered, doomed]),
            }
        );
    }

    #[tokio::test]
    async fn boot_obligation_scan_machine_returns_a_failed_read() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        seed(&store).await;
        for (recovery_scan, delivery_listing) in [
            (Answer::Fail, Answer::Forward),
            (Answer::Forward, Answer::Fail),
        ] {
            let read = scripted(&store, recovery_scan, delivery_listing)
                .boot_recovery_obligations()
                .await;
            assert!(
                matches!(read, Err(RuntimeStoreError::ReadFailed(_))),
                "{read:?}"
            );
        }
    }

    /// The sessions [`seed_sessions`] wrote, keyed by session.
    struct SeededSessions {
        /// A nonterminal input.
        working: SessionId,
        /// A pending delivery.
        delivered: SessionId,
        /// A lifecycle with no run and nothing else.
        idle: SessionId,
    }

    /// [`seed`], with each runtime the runtime of a session.
    async fn seed_sessions(store: &Arc<InMemoryRuntimeStore>) -> SeededSessions {
        let working = SessionId::new();
        store
            .persist_input_state(
                &LogicalRuntimeId::for_session(&working),
                &InputStatePersistenceRecord::from_machine_snapshot(
                    StoredInputState::new_accepted(InputId::new()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let delivered = SessionId::new();
        RuntimeDeliveryInbox::new(Arc::clone(store) as Arc<dyn RuntimeStore>)
            .submit(
                &LogicalRuntimeId::for_session(&delivered),
                RuntimeDeliverySubmission::new(
                    RuntimeDeliveryId::new("session-work-delivery").unwrap(),
                    RuntimeDeliveryKind::JobTerminal,
                    "session-work-job",
                    1,
                    "session-work-interaction",
                    b"payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let idle = SessionId::new();
        let idle_runtime = LogicalRuntimeId::for_session(&idle);
        store
            .commit_machine_lifecycle(
                &idle_runtime,
                MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Idle,
                    MachineLifecycleBindingFacts::new(
                        Some(idle_runtime.0.clone()),
                        Some(1),
                        Some(1),
                        Some("epoch-1".to_string()),
                    ),
                    SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                &[],
            )
            .await
            .unwrap();
        SeededSessions {
            working,
            delivered,
            idle,
        }
    }

    /// One session's durable work, read cold, with the predicates the boot
    /// read applies to the whole store: recovery work and pending
    /// deliveries are told apart, an idle runtime and a session the store
    /// never held hold nothing, and nothing is registered by the read. RED
    /// before: no per-session read.
    #[tokio::test]
    async fn session_work_state_reads_one_runtime_cold() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let seeded = seed_sessions(&store).await;
        let machine =
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store) as Arc<dyn RuntimeStore>)
                .unwrap();
        for (session, expected) in [
            (
                &seeded.working,
                SessionWorkState::HoldsWork(SessionDurableWork {
                    recovery_work: true,
                    pending_deliveries: false,
                }),
            ),
            (
                &seeded.delivered,
                SessionWorkState::HoldsWork(SessionDurableWork {
                    recovery_work: false,
                    pending_deliveries: true,
                }),
            ),
            (&seeded.idle, SessionWorkState::Idle),
            (&SessionId::new(), SessionWorkState::Idle),
        ] {
            assert_eq!(
                machine.session_work_state(session).await.unwrap(),
                expected,
                "{session}"
            );
            assert!(
                machine
                    .current_session_registration_witness(session)
                    .await
                    .is_none(),
                "{session}: the read registers nothing"
            );
        }
    }

    /// A store that cannot answer one of the two reads answers the typed
    /// cause for every session, never a guessed idle. RED before: no
    /// per-session read.
    #[tokio::test]
    async fn session_work_state_without_a_store_read_answers_unobserved() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let seeded = seed_sessions(&store).await;
        for (recovery_scan, delivery_listing, cause) in [
            (
                Answer::Unsupported,
                Answer::Forward,
                RuntimeRecoveryWorkUnobserved::RecoveryScanUnsupported,
            ),
            (
                Answer::Forward,
                Answer::Unsupported,
                RuntimeRecoveryWorkUnobserved::DeliveryListingUnsupported,
            ),
        ] {
            let machine = scripted(&store, recovery_scan, delivery_listing);
            for session in [&seeded.working, &seeded.delivered, &seeded.idle] {
                assert_eq!(
                    machine.session_work_state(session).await.unwrap(),
                    SessionWorkState::Unobserved(cause),
                    "{session}: the store cannot tell, and says why"
                );
            }
        }
    }

    /// A session registered in this process answers `Registered`: its live
    /// authority is what counts. RED before: no per-session read.
    #[tokio::test]
    async fn session_work_state_of_a_registered_session_answers_registered() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let machine =
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store) as Arc<dyn RuntimeStore>)
                .unwrap();
        let session = SessionId::new();
        machine
            .register_session(session.clone())
            .await
            .expect("the session registers");
        assert_eq!(
            machine.session_work_state(&session).await.unwrap(),
            SessionWorkState::Registered
        );
    }

    /// A machine without a store holds nothing durable. RED before: no
    /// per-session read.
    #[tokio::test]
    async fn session_work_state_machine_without_a_store_answers_idle() {
        assert_eq!(
            MeerkatMachine::ephemeral()
                .session_work_state(&SessionId::new())
                .await
                .unwrap(),
            SessionWorkState::Idle
        );
    }

    /// A failed store read is an error, never an answer. RED before: no
    /// per-session read.
    #[tokio::test]
    async fn session_work_state_returns_a_failed_read() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let seeded = seed_sessions(&store).await;
        for (recovery_scan, delivery_listing) in [
            (Answer::Fail, Answer::Forward),
            (Answer::Forward, Answer::Fail),
        ] {
            let read = scripted(&store, recovery_scan, delivery_listing)
                .session_work_state(&seeded.idle)
                .await;
            assert!(
                matches!(read, Err(RuntimeStoreError::ReadFailed(_))),
                "{read:?}"
            );
        }
    }

    /// A host store runtime whose scope is poisoned holds work, as the boot
    /// read lists it: its facts cannot be trusted either way, so it is never
    /// read as idle. RED before: no per-session read.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn session_work_state_of_a_poisoned_runtime_holds_work() {
        use crate::store::spi::test_host::TestHost;
        use crate::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};

        let host = Arc::new(TestHost::new());
        let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
            .await
            .expect("the realm opens");
        let store: Arc<dyn RuntimeStore> =
            Arc::new(SpiRuntimeStore::open(realm).await.expect("the store opens"));
        let doomed = SessionId::new();
        host.faults.lock().unwrap().conflict = 1;
        assert!(
            store
                .persist_input_state(
                    &LogicalRuntimeId::for_session(&doomed),
                    &InputStatePersistenceRecord::from_machine_snapshot(
                        StoredInputState::new_accepted(InputId::new())
                    )
                    .expect("a machine snapshot"),
                )
                .await
                .is_err(),
            "the store refuses the batch, and the runtime's scope is poisoned"
        );
        let machine = MeerkatMachine::persistent_without_blobs(store).unwrap();
        assert_eq!(
            machine.session_work_state(&doomed).await.unwrap(),
            SessionWorkState::HoldsWork(SessionDurableWork {
                recovery_work: true,
                pending_deliveries: true,
            })
        );
        assert_eq!(
            machine.session_work_state(&SessionId::new()).await.unwrap(),
            SessionWorkState::Idle,
            "a healthy runtime answers as its memory holds it"
        );
    }
}
