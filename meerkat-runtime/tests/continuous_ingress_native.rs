#![cfg(all(
    feature = "live",
    feature = "sqlite-store",
    not(target_arch = "wasm32")
))]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterObservation, LiveAdapterStatus,
};
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_execution::observation::{
    ContinuousLiveObservation, LiveUsageSnapshot, LiveVoiceDurationSeconds,
};
use meerkat_core::live_execution::request::LiveProviderReference;
use meerkat_core::live_observation::{
    LiveObservationReceiveClock, LiveTranscriptDirection, LiveTranscriptObservation,
    LiveTranscriptRange,
};
use meerkat_live::host::{
    LiveAdapterHost, LiveContinuousTranscriptIngress, NoOpProjectionSink, ObservationOutcome,
};
use meerkat_runtime::live_ledger::transcript_authority::{
    LiveSourceReservationOutcome, LiveTranscriptStoreOwner,
};
use meerkat_runtime::store::{
    RuntimeStore, RuntimeStoreError, RuntimeStoreWriteFence, RuntimeStoreWriteFenceOutcome,
    SerializedSessionSnapshot, SqliteRuntimeStore,
};
use meerkat_runtime::{LogicalRuntimeId, MeerkatMachine};
use meerkat_store::SessionStore;
use tokio::sync::Mutex;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct LegacyDispatcher(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl meerkat_live::host::LiveToolDispatcher for LegacyDispatcher {
    async fn dispatch_live_tool_call(
        &self,
        _: &meerkat_core::SessionId,
        _: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_live::host::LiveToolDispatchError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(meerkat_live::host::LiveToolDispatchError::Rejected(
            "legacy dispatch fixture".into(),
        ))
    }
}

struct CurrentFence;
impl RuntimeStoreWriteFence for CurrentFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        operation()?;
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

struct Observations(
    Mutex<tokio::sync::mpsc::Receiver<LiveAdapterObservation>>,
    Arc<AtomicUsize>,
    Option<Arc<tokio::sync::Notify>>,
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseMode {
    Manual,
    Drain,
    CancelledWaiter,
    TimedOut,
}
#[async_trait::async_trait]
impl LiveAdapter for Observations {
    async fn send_command(&self, _: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        Err(LiveAdapterError::Closed)
    }
    async fn next_observation(&self) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
        Ok(self.0.lock().await.recv().await)
    }
    fn status(&self) -> LiveAdapterStatus {
        LiveAdapterStatus::Ready
    }
    async fn close(&self) -> Result<(), LiveAdapterError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.2 {
            gate.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn native_host_counts_received_text_before_apply_and_only_returns_committed_records()
-> TestResult {
    for (head_canonical, mode) in [
        (false, CloseMode::Manual),
        (true, CloseMode::Manual),
        (false, CloseMode::Drain),
        (true, CloseMode::Drain),
        (false, CloseMode::CancelledWaiter),
        (true, CloseMode::CancelledWaiter),
        (false, CloseMode::TimedOut),
        (true, CloseMode::TimedOut),
    ] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("runtime.sqlite3");
        let session = meerkat_core::Session::new();
        if head_canonical {
            meerkat_store::SqliteSessionStore::open(&path)?
                .save(&session)
                .await?;
        }
        let initial = SqliteRuntimeStore::new_whole_blob(&path)?;
        initial
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(session.id()),
                SerializedSessionSnapshot {
                    session_snapshot: Arc::new(serde_json::to_vec(&session)?),
                },
            )
            .await?;
        drop(initial);
        let store: Arc<dyn RuntimeStore> = Arc::new(if head_canonical {
            SqliteRuntimeStore::new_head_canonical(&path)?
        } else {
            SqliteRuntimeStore::new_whole_blob(&path)?
        });
        let machine = MeerkatMachine::persistent(
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let _bindings = machine.prepare_bindings(session.id().clone()).await?;
        let channel_id = LiveChannelId::new(uuid::Uuid::new_v4().to_string());
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-live-1".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let admission = machine
            .resolve_live_open_admission(session.id(), &channel_id, &identity)
            .await?;
        let legacy_calls = Arc::new(AtomicUsize::new(0));
        let host = Arc::new(
            LiveAdapterHost::new(Arc::new(NoOpProjectionSink))
                .with_live_tool_dispatcher(Arc::new(LegacyDispatcher(Arc::clone(&legacy_calls)))),
        );
        host.open_channel_with_authority(
            admission.channel_open_authority().ok_or("open authority")?,
        )
        .await?;
        let lifecycle = store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(session.id()))
            .await?
            .version()
            .ok_or("lifecycle")?
            .clone();
        let ingress = LiveTranscriptStoreOwner::new(
            store.clone(),
            session.id().clone(),
            Arc::new(CurrentFence),
        )
        .activate_voice_channel(channel_id.clone(), lifecycle)
        .await?
        .into_shared();
        let text = LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(0.5, 1.25)?,
            " \n exact \u{0000} ",
        );
        let legacy = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: meerkat_core::ToolCallId::new("not-a-scope"),
            tool_name: meerkat_core::ToolName::from("invoke_meerkat"),
            arguments: serde_json::json!({"request":"must not dispatch"}),
        };
        let closes = Arc::new(AtomicUsize::new(0));
        let mut observations = VecDeque::from([
            LiveAdapterObservation::Continuous {
                event: ContinuousLiveObservation::Transcript(text.clone()),
                receive: None,
            },
            legacy.clone(),
            LiveAdapterObservation::Continuous {
                event: ContinuousLiveObservation::VoiceUsage(LiveUsageSnapshot::Periodic {
                    cumulative_seconds: LiveVoiceDurationSeconds::new(2.25)?,
                }),
                receive: None,
            },
            LiveAdapterObservation::Continuous {
                event: ContinuousLiveObservation::ProviderClosed {
                    usage: LiveUsageSnapshot::SessionClosed {
                        cumulative_seconds: LiveVoiceDurationSeconds::new(3.25)?,
                    },
                },
                receive: None,
            },
            LiveAdapterObservation::Continuous {
                event: ContinuousLiveObservation::ObservationStreamEnded,
                receive: None,
            },
        ]);
        if mode != CloseMode::Manual {
            observations.insert(
                2,
                LiveAdapterObservation::Continuous {
                    event: ContinuousLiveObservation::Transcript(LiveTranscriptObservation::new(
                        LiveTranscriptDirection::Input,
                        LiveTranscriptRange::new(2.0, 3.0)?,
                        "queued during close",
                    )),
                    receive: None,
                },
            );
        }
        let close_gate = matches!(mode, CloseMode::CancelledWaiter | CloseMode::TimedOut)
            .then(|| Arc::new(tokio::sync::Notify::new()));
        observations.push_front(LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::Diagnostic(
                meerkat_core::live_execution::backend::LiveProviderDiagnostic::new(
                    meerkat_core::live_execution::backend::LiveProviderDiagnosticCategory::BackendAdvisoryError,
                    meerkat_core::live_execution::backend::LiveBackendOwnership::Unowned {},
                    std::num::NonZeroU64::MIN,
                )?,
            ),
            receive: None,
        });
        observations.push_front(LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::ProviderStarted {
                provider_session: LiveProviderReference::new("private-provider-session")?,
            },
            receive: None,
        });
        let (send_observation, receive_observation) = tokio::sync::mpsc::channel(1);
        let producer = tokio::spawn(async move {
            for observation in observations {
                send_observation
                    .send(observation)
                    .await
                    .map_err(|_| "observation receiver closed")?;
            }
            Ok::<(), &'static str>(())
        });
        let adapter = Arc::new(Observations(
            Mutex::new(receive_observation),
            Arc::clone(&closes),
            close_gate.clone(),
        ));
        host.attach_continuous_adapter(&channel_id, adapter, ingress.clone())
            .await?;
        let mut observation_pump = host.claim_observation_pump(&channel_id).await?;
        assert!(host.claim_observation_pump(&channel_id).await.is_err());
        assert!(host.next_observation_raw(&channel_id).await.is_err());
        let actor = store
            .load_session_boundary_authority(&LogicalRuntimeId::for_session(session.id()))
            .await?;
        assert!(ingress.read_provider_start().await?.is_none());
        assert!(matches!(
            ingress.wait_provider_start(std::time::Duration::from_millis(1)).await,
            Err(meerkat_runtime::live_ledger::transcript_authority::LiveTranscriptWriteError::ProviderStartWaitTimedOut)
        ));
        let waiter = {
            let ingress = Arc::clone(&ingress);
            tokio::spawn(async move {
                ingress
                    .wait_provider_start(std::time::Duration::from_secs(2))
                    .await
            })
        };
        for index in 0..2 {
            let raw = observation_pump
                .next_observation()
                .await?
                .ok_or("control observation")?;
            assert!(meerkat_contracts::WireLiveAdapterObservation::encode(raw.clone()).is_err());
            assert!(matches!(
                host.apply_observation(&channel_id, &raw).await?,
                ObservationOutcome::ContinuousApplied
            ));
            assert_eq!(
                ingress
                    .read_provider_start()
                    .await?
                    .ok_or("committed start")?
                    .sequence()
                    .get(),
                2
            );
            assert_eq!(ingress.receive_clock().received_ordinal(), 0);
            if index == 0 {
                assert!(matches!(
                    raw,
                    LiveAdapterObservation::Continuous {
                        event: ContinuousLiveObservation::ProviderStarted { .. },
                        ..
                    }
                ));
            }
        }
        assert_eq!(waiter.await??.sequence().get(), 2);
        assert_eq!(
            ingress
                .wait_provider_start(std::time::Duration::from_secs(2))
                .await?
                .sequence()
                .get(),
            2
        );
        if mode == CloseMode::Manual {
            use meerkat_core::live_execution::backend::{
                LiveBackendOwnership, LiveProviderDiagnostic, LiveProviderDiagnosticCategory,
            };
            for count in 2..=61 {
                let control = LiveAdapterObservation::Continuous {
                    event: ContinuousLiveObservation::Diagnostic(LiveProviderDiagnostic::new(
                        LiveProviderDiagnosticCategory::BackendAdvisoryError,
                        LiveBackendOwnership::Unowned {},
                        std::num::NonZeroU64::new(count).ok_or("count")?,
                    )?),
                    receive: None,
                };
                let before = store
                    .live_ledger_ops()
                    .ok_or("ops")?
                    .load_live_head(session.id())
                    .await?;
                let outcome = host.apply_observation(&channel_id, &control).await?;
                if count == 61 {
                    assert!(matches!(outcome, ObservationOutcome::ContinuousControlUnaccepted {
                        reason: meerkat_core::live_execution::observation::LiveProviderControlRefusal::Capacity
                    }));
                    assert_eq!(
                        store
                            .live_ledger_ops()
                            .ok_or("ops")?
                            .load_live_head(session.id())
                            .await?,
                        before
                    );
                } else {
                    assert!(matches!(outcome, ObservationOutcome::ContinuousApplied));
                }
            }
        }
        let raw = observation_pump
            .next_observation()
            .await?
            .ok_or("received")?;
        assert_eq!(ingress.receive_clock().received_ordinal(), 1);
        assert!(matches!(
            ingress
                .reserve_client_source(LiveProviderReference::new("pending")?, 0.0, None)
                .await?,
            LiveSourceReservationOutcome::AwaitingObservationDurability
        ));
        assert!(meerkat_contracts::WireLiveAdapterObservation::encode(raw.clone()).is_err());
        let ObservationOutcome::ContinuousTranscriptCommitted { record } =
            host.apply_observation(&channel_id, &raw).await?
        else {
            return Err("no committed continuous observation".into());
        };
        assert_eq!(
            record.sequence.get(),
            if mode == CloseMode::Manual { 63 } else { 4 }
        );
        assert_eq!(record.observation, text);
        assert_eq!(record.channel_id, channel_id);
        assert_eq!(
            store
                .load_session_boundary_authority(&LogicalRuntimeId::for_session(session.id()))
                .await?,
            actor
        );
        assert!(host.apply_observation(&channel_id, &legacy).await.is_err());
        assert!(observation_pump.next_observation().await.is_err());
        assert_eq!(legacy_calls.load(Ordering::SeqCst), 0);
        assert!(
            host.apply_observation(&channel_id, &raw).await.is_err(),
            "same receive cannot publish twice"
        );
        let foreign = LiveObservationReceiveClock::default();
        foreign.record_received()?;
        let receipt = foreign.record_received()?;
        let forged = LiveAdapterObservation::Continuous {
            event: ContinuousLiveObservation::Transcript(text),
            receive: Some(receipt),
        };
        assert!(
            host.apply_observation(&channel_id, &forged).await.is_err(),
            "foreign receive clock cannot be repaired"
        );
        let serialized = serde_json::to_value(
            meerkat_contracts::WireLiveAdapterObservation::LiveObservationCommitted { record },
        )?;
        assert_eq!(serialized["observation"], "live_observation_committed");
        assert!(matches!(
            ingress
                .reserve_client_source(LiveProviderReference::new("pending")?, 0.0, None)
                .await?,
            LiveSourceReservationOutcome::Retained(_)
        ));
        let usage_owner = LiveTranscriptStoreOwner::new(
            store.clone(),
            session.id().clone(),
            Arc::new(CurrentFence),
        );
        let close = host.reserve_channel_close_observation(&channel_id).await?;
        assert!(!ingress.observation_closure_committed().await?);
        assert_eq!(closes.load(Ordering::SeqCst), 0);
        let mut expected_closes = 1;
        if mode == CloseMode::CancelledWaiter {
            drop(observation_pump);
            let waiter = {
                let host = Arc::clone(&host);
                let close = close.clone();
                tokio::spawn(async move { host.prepare_channel_physical_close(&close).await })
            };
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while !ingress.observation_closure_committed().await? {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                Ok::<(), meerkat_live::host::LiveProjectionError>(())
            })
            .await??;
            assert_eq!(closes.load(Ordering::SeqCst), 1);
            waiter.abort();
            let Err(error) = waiter.await else {
                return Err("close waiter finished before physical release".into());
            };
            assert!(error.is_cancelled());
            close_gate.as_ref().ok_or("close gate")?.notify_one();
            host.prepare_channel_physical_close(&close).await?;
            assert_eq!(ingress.receive_clock().received_ordinal(), 2);
            assert_eq!(
                usage_owner.read_voice_usage(&channel_id).await?,
                Some(LiveUsageSnapshot::SessionClosed {
                    cumulative_seconds: LiveVoiceDurationSeconds::new(3.25)?
                },)
            );
        } else if mode == CloseMode::TimedOut {
            assert!(matches!(
                observation_pump
                    .drain(std::time::Duration::from_millis(20))
                    .await,
                Err(meerkat_live::host::LiveAdapterHostError::ObservationDrainTimedOut),
            ));
            expected_closes = closes.load(Ordering::SeqCst) + 1;
            drop(observation_pump);
            let retained = host.claim_observation_pump(&channel_id).await?;
            drop(retained);
            close_gate.as_ref().ok_or("close gate")?.notify_one();
            host.prepare_channel_physical_close(&close).await?;
        } else if mode == CloseMode::Drain {
            observation_pump
                .drain(std::time::Duration::from_secs(2))
                .await?;
            assert_eq!(ingress.receive_clock().received_ordinal(), 2);
            assert_eq!(
                usage_owner.read_voice_usage(&channel_id).await?,
                Some(LiveUsageSnapshot::SessionClosed {
                    cumulative_seconds: LiveVoiceDurationSeconds::new(3.25)?,
                },)
            );
        } else {
            for expected in [
                LiveUsageSnapshot::Periodic {
                    cumulative_seconds: LiveVoiceDurationSeconds::new(2.25)?,
                },
                LiveUsageSnapshot::SessionClosed {
                    cumulative_seconds: LiveVoiceDurationSeconds::new(3.25)?,
                },
                LiveUsageSnapshot::SessionClosed {
                    cumulative_seconds: LiveVoiceDurationSeconds::new(3.25)?,
                },
            ] {
                let event = observation_pump
                    .next_observation()
                    .await?
                    .ok_or("voice observation")?;
                assert!(
                    meerkat_contracts::WireLiveAdapterObservation::encode(event.clone()).is_err()
                );
                assert_eq!(
                    host.apply_observation(&channel_id, &event).await?,
                    ObservationOutcome::ContinuousApplied
                );
                assert_eq!(
                    usage_owner.read_voice_usage(&channel_id).await?,
                    Some(expected)
                );
            }
            drop(observation_pump);
        }
        host.prepare_channel_physical_close(&close).await?;
        host.prepare_channel_physical_close(&close).await?;
        assert_eq!(closes.load(Ordering::SeqCst), expected_closes);
        assert_eq!(legacy_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .load_session_boundary_authority(&LogicalRuntimeId::for_session(session.id()))
                .await?,
            actor
        );
        producer.await??;
    }
    Ok(())
}
