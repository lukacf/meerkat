use super::*;
use crate::live_ledger::{
    completion::LiveCompletionEvent,
    transcript::LiveDiscontinuity,
    transcript_authority::{LiveTranscriptChannelIngress, LiveTranscriptStoreOwner},
};
use meerkat_core::{
    live_execution::LiveChannelId,
    live_observation::{LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange},
};

fn text(value: impl Into<Box<str>>) -> TestResult<LiveTranscriptObservation> {
    Ok(LiveTranscriptObservation::new(
        LiveTranscriptDirection::Input,
        LiveTranscriptRange::new(0.0, 0.0)?,
        value,
    ))
}

async fn ingress(fixture: &Fixture) -> TestResult<LiveTranscriptChannelIngress> {
    install_grant_executor(fixture, &meerkat_core::RuntimeEpochId::new(), 0).await?;
    let version = fixture
        .store
        .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
        .await?
        .version()
        .ok_or("lifecycle")?
        .clone();
    Ok(LiveTranscriptStoreOwner::new(
        Arc::clone(&fixture.store),
        fixture.session.id().clone(),
        current_fence(),
    )
    .activate_channel(LiveChannelId::new("voice"), version)
    .await?)
}

async fn head(fixture: &Fixture) -> TestResult<LiveLedgerStoredHead> {
    fixture
        .ops()?
        .load_live_head(fixture.session.id())
        .await?
        .ok_or_else(|| "head".into())
}

async fn voice_ingress(fixture: &Fixture, name: &str) -> TestResult<LiveTranscriptChannelIngress> {
    install_grant_executor(fixture, &meerkat_core::RuntimeEpochId::new(), 0).await?;
    let version = fixture
        .store
        .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
        .await?
        .version()
        .ok_or("lifecycle")?
        .clone();
    Ok(LiveTranscriptStoreOwner::new(
        Arc::clone(&fixture.store),
        fixture.session.id().clone(),
        current_fence(),
    )
    .activate_voice_channel(LiveChannelId::new(name), version)
    .await?)
}

fn provider_start(
    name: &str,
) -> TestResult<meerkat_core::live_execution::observation::ContinuousLiveObservation> {
    Ok(
        meerkat_core::live_execution::observation::ContinuousLiveObservation::ProviderStarted {
            provider_session: meerkat_core::live_execution::request::LiveProviderReference::new(
                name,
            )?,
        },
    )
}

fn provider_diagnostic(
    count: u64,
) -> TestResult<meerkat_core::live_execution::observation::ContinuousLiveObservation> {
    use meerkat_core::live_execution::{backend::*, observation::ContinuousLiveObservation};
    Ok(ContinuousLiveObservation::Diagnostic(
        LiveProviderDiagnostic::new(
            LiveProviderDiagnosticCategory::BackendAdvisoryError,
            LiveBackendOwnership::Unowned {},
            std::num::NonZeroU64::new(count).ok_or("count")?,
        )?,
    ))
}

fn accepted_control(
    outcome: crate::live_ledger::transcript_authority::LiveProviderControlOutcome,
) -> TestResult<crate::live_ledger::transcript_authority::CommittedLiveProviderControl> {
    match outcome {
        crate::live_ledger::transcript_authority::LiveProviderControlOutcome::Accepted(receipt) => {
            Ok(receipt)
        }
        _ => Err("provider control not accepted".into()),
    }
}

#[tokio::test]
async fn native_provider_controls_replay_exact_facts_without_ordinary_mutation() -> TestResult {
    use crate::live_ledger::transcript_authority::{
        LiveProviderControlOutcome, dsl::LiveProviderControlRefusal,
    };
    use meerkat_core::live_execution::observation::LiveUsageSnapshot;
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = voice_ingress(&fixture, "control").await?;
        let actor = fixture.actor().await?;
        assert!(channel.read_provider_start().await?.is_none());
        let start = provider_start("private-provider-id")?;
        let receipt = accepted_control(channel.observe_provider_control(&start).await?)?;
        assert_eq!(channel.read_provider_start().await?, Some(receipt.clone()));
        let a = provider_diagnostic(1)?;
        let b = provider_diagnostic(2)?;
        let first = accepted_control(channel.observe_provider_control(&a).await?)?;
        channel.observe_provider_control(&b).await?;
        channel.append(text("exact text")?).await?;
        let before = head(&fixture).await?;
        assert_eq!(
            accepted_control(channel.observe_provider_control(&a).await?)?.sequence(),
            first.sequence()
        );
        assert_eq!(
            accepted_control(channel.observe_provider_control(&start).await?)?.sequence(),
            receipt.sequence()
        );
        assert_eq!(head(&fixture).await?, before);
        assert_eq!(
            channel
                .observe_provider_control(&provider_start("alien")?)
                .await?,
            LiveProviderControlOutcome::Refused(LiveProviderControlRefusal::IdentityConflict),
        );
        channel.close().await?;
        channel
            .observe_voice_usage(&LiveUsageSnapshot::CloseUnconfirmed {
                last_observed_seconds: None,
            })
            .await?;
        let closed = head(&fixture).await?;
        assert_eq!(
            accepted_control(channel.observe_provider_control(&a).await?)?.sequence(),
            first.sequence()
        );
        assert_eq!(
            channel
                .observe_provider_control(&provider_diagnostic(3)?)
                .await?,
            LiveProviderControlOutcome::Refused(LiveProviderControlRefusal::ObservationClosed),
        );
        assert_eq!(head(&fixture).await?, closed);
        assert_eq!(fixture.actor().await?, actor);
    }
    Ok(())
}

#[tokio::test]
async fn native_provider_controls_preserve_final_capacity_and_refuse_new_diagnostics() -> TestResult
{
    use crate::live_ledger::transcript_authority::{
        LiveProviderControlOutcome, dsl::LiveProviderControlRefusal,
    };
    use meerkat_core::live_execution::observation::{LiveUsageSnapshot, LiveVoiceDurationSeconds};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = voice_ingress(&fixture, &"\0".repeat(128)).await?;
        let start = provider_start(&"\0".repeat(128))?;
        let receipt = accepted_control(channel.observe_provider_control(&start).await?)?;
        for count in 1..60 {
            accepted_control(
                channel
                    .observe_provider_control(&provider_diagnostic(count)?)
                    .await?,
            )?;
        }
        let before = head(&fixture).await?;
        assert_eq!(before.payload.reserved.records, 4);
        accepted_control(
            channel
                .observe_provider_control(&provider_diagnostic(60)?)
                .await?,
        )?;
        let before = head(&fixture).await?;
        assert_eq!(before.payload.reserved.records, 3);
        assert_eq!(
            channel
                .observe_provider_control(&provider_diagnostic(61)?)
                .await?,
            LiveProviderControlOutcome::Refused(LiveProviderControlRefusal::Capacity),
        );
        assert_eq!(
            accepted_control(channel.observe_provider_control(&start).await?)?.sequence(),
            receipt.sequence()
        );
        assert_eq!(head(&fixture).await?, before);
        channel.close().await?;
        channel
            .observe_voice_usage(&LiveUsageSnapshot::SessionClosed {
                cumulative_seconds: LiveVoiceDurationSeconds::new(f64::MAX)?,
            })
            .await?;
        channel
            .observe_voice_usage(&LiveUsageSnapshot::CloseUnconfirmed {
                last_observed_seconds: None,
            })
            .await?;
        assert_eq!(head(&fixture).await?.payload.reserved.records, 0);
    }
    Ok(())
}

#[tokio::test]
async fn native_provider_control_receipts_survive_sqlite_close_reopen() -> TestResult {
    use crate::live_ledger::transcript_authority::{
        LiveProviderControlOutcome, dsl::LiveProviderControlRefusal,
    };
    use meerkat_core::live_execution::observation::LiveUsageSnapshot;
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let mut channel = voice_ingress(&fixture, "cold-control").await?;
        let start = provider_start("retained-private-id")?;
        let diagnostic = provider_diagnostic(19)?;
        let receipt = accepted_control(channel.observe_provider_control(&start).await?)?;
        let diagnostic_receipt =
            accepted_control(channel.observe_provider_control(&diagnostic).await?)?;
        channel.close().await?;
        channel
            .observe_voice_usage(&LiveUsageSnapshot::CloseUnconfirmed {
                last_observed_seconds: None,
            })
            .await?;
        let before = head(&fixture).await?;
        drop(channel);
        let Fixture {
            store,
            session,
            _directory,
            path,
        } = fixture;
        assert_eq!(Arc::strong_count(&store), 1);
        drop(store);
        let reopened: Arc<dyn RuntimeStore> = Arc::new(match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
            Backend::HeadCanonical => crate::store::SqliteRuntimeStore::new_head_canonical(&path)?,
            Backend::Memory => return Err("unexpected memory fixture".into()),
        });
        let owner = LiveTranscriptStoreOwner::new(
            Arc::clone(&reopened),
            session.id().clone(),
            current_fence(),
        );
        let id = LiveChannelId::new("cold-control");
        assert_eq!(
            owner
                .read_provider_start(&id)
                .await?
                .ok_or("start")?
                .sequence(),
            receipt.sequence()
        );
        assert_eq!(
            accepted_control(owner.observe_provider_control(&id, &diagnostic).await?)?.sequence(),
            diagnostic_receipt.sequence()
        );
        assert_eq!(
            owner
                .observe_provider_control(&id, &provider_diagnostic(20)?)
                .await?,
            LiveProviderControlOutcome::Refused(LiveProviderControlRefusal::ObservationClosed)
        );
        assert_eq!(
            reopened
                .live_ledger_ops()
                .ok_or("ops")?
                .load_live_head(session.id())
                .await?,
            Some(before)
        );
        drop(owner);
        drop(reopened);
        drop(_directory);
    }
    Ok(())
}

#[tokio::test]
async fn native_voice_accounting_retains_control_credit_through_observation_closure() -> TestResult
{
    use meerkat_core::live_execution::observation::{LiveUsageSnapshot, LiveVoiceDurationSeconds};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let actor = fixture.actor().await?;
        let mut channel = voice_ingress(&fixture, "voice-account").await?;
        let activated = head(&fixture).await?;
        for second in 0..101 {
            let periodic = LiveUsageSnapshot::Periodic {
                cumulative_seconds: LiveVoiceDurationSeconds::new(f64::from(second))?,
            };
            let committed = channel.observe_voice_usage(&periodic).await?;
            assert_eq!(committed.snapshot(), &periodic);
            assert_eq!(channel.observe_voice_usage(&periodic).await?, committed);
        }
        let periodic_head = head(&fixture).await?;
        assert_eq!(periodic_head.payload.reserved, activated.payload.reserved);
        channel.close().await?;
        let closed_ingress = head(&fixture).await?;
        assert!(closed_ingress.payload.reserved.records > 0);
        let final_usage = LiveUsageSnapshot::SessionClosed {
            cumulative_seconds: LiveVoiceDurationSeconds::new(100.5)?,
        };
        let final_commit = channel.observe_voice_usage(&final_usage).await?;
        assert_eq!(final_commit.snapshot(), &final_usage);
        assert_eq!(
            channel.observe_voice_usage(&final_usage).await?,
            final_commit
        );
        assert!(head(&fixture).await?.payload.reserved.records > 0);
        let observation_closed = LiveUsageSnapshot::CloseUnconfirmed {
            last_observed_seconds: None,
        };
        let settled = channel.observe_voice_usage(&observation_closed).await?;
        assert_eq!(
            settled.snapshot(),
            &final_usage,
            "EOF cannot downgrade an observed final"
        );
        assert_eq!(
            channel.observe_voice_usage(&observation_closed).await?,
            settled
        );
        assert_eq!(head(&fixture).await?.payload.reserved.records, 0);
        assert!(
            channel
                .observe_voice_usage(&LiveUsageSnapshot::Periodic {
                    cumulative_seconds: LiveVoiceDurationSeconds::new(101.0)?,
                })
                .await
                .is_err()
        );
        assert_eq!(fixture.actor().await?, actor);
        assert_eq!(head(&fixture).await?.reference, *settled.head());
    }
    Ok(())
}

#[tokio::test]
async fn native_voice_accounting_preserves_regressions_invalidity_and_conflicting_final()
-> TestResult {
    use meerkat_core::live_execution::observation::{
        LiveUsageDispute, LiveUsageSnapshot, LiveVoiceDurationSeconds,
    };
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = voice_ingress(&fixture, "voice-disputed").await?;
        let ten = LiveVoiceDurationSeconds::new(10.0)?;
        channel
            .observe_voice_usage(&LiveUsageSnapshot::Periodic {
                cumulative_seconds: ten,
            })
            .await?;
        let regression = channel
            .observe_voice_usage(&LiveUsageSnapshot::Periodic {
                cumulative_seconds: LiveVoiceDurationSeconds::new(9.0)?,
            })
            .await?;
        assert_eq!(
            regression.snapshot(),
            &LiveUsageSnapshot::Disputed {
                last_valid_seconds: Some(ten),
                reason: LiveUsageDispute::Regression,
            }
        );
        let invalid = LiveUsageSnapshot::Disputed {
            last_valid_seconds: None,
            reason: LiveUsageDispute::InvalidDuration,
        };
        assert_eq!(
            channel.observe_voice_usage(&invalid).await?.snapshot(),
            &LiveUsageSnapshot::Disputed {
                last_valid_seconds: Some(ten),
                reason: LiveUsageDispute::InvalidDuration,
            }
        );
        let eleven = LiveVoiceDurationSeconds::new(11.0)?;
        channel
            .observe_voice_usage(&LiveUsageSnapshot::SessionClosed {
                cumulative_seconds: eleven,
            })
            .await?;
        let conflicting = channel
            .observe_voice_usage(&LiveUsageSnapshot::SessionClosed {
                cumulative_seconds: LiveVoiceDurationSeconds::new(12.0)?,
            })
            .await?;
        assert_eq!(
            conflicting.snapshot(),
            &LiveUsageSnapshot::Disputed {
                last_valid_seconds: Some(eleven),
                reason: LiveUsageDispute::ConflictingFinal,
            }
        );
        channel.close().await?;
        let closed = channel
            .observe_voice_usage(&LiveUsageSnapshot::CloseUnconfirmed {
                last_observed_seconds: Some(LiveVoiceDurationSeconds::new(99.0)?),
            })
            .await?;
        assert_eq!(closed.snapshot(), conflicting.snapshot());
        assert_eq!(head(&fixture).await?.payload.reserved.records, 0);
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_voice_accounting_cold_reopen_preserves_unconfirmed_account_and_receipt()
-> TestResult {
    use meerkat_core::live_execution::observation::{LiveUsageSnapshot, LiveVoiceDurationSeconds};
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let mut channel = voice_ingress(&fixture, "cold-voice").await?;
        let seconds = LiveVoiceDurationSeconds::new(f64::from_bits(1))?;
        channel
            .observe_voice_usage(&LiveUsageSnapshot::Periodic {
                cumulative_seconds: seconds,
            })
            .await?;
        channel.close().await?;
        let observation = LiveUsageSnapshot::CloseUnconfirmed {
            last_observed_seconds: None,
        };
        let committed = channel.observe_voice_usage(&observation).await?;
        drop(channel);
        let Fixture {
            store,
            session,
            _directory,
            path,
        } = fixture;
        assert_eq!(Arc::strong_count(&store), 1);
        drop(store);
        let reopened: Arc<dyn RuntimeStore> = Arc::new(match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
            Backend::HeadCanonical => crate::store::SqliteRuntimeStore::new_head_canonical(&path)?,
            Backend::Memory => return Err("unexpected memory fixture".into()),
        });
        let owner = LiveTranscriptStoreOwner::new(reopened, session.id().clone(), current_fence());
        let channel = LiveChannelId::new("cold-voice");
        assert_eq!(
            owner.read_voice_usage(&channel).await?,
            Some(LiveUsageSnapshot::CloseUnconfirmed {
                last_observed_seconds: Some(seconds),
            })
        );
        assert_eq!(
            owner.observe_voice_usage(&channel, &observation).await?,
            committed
        );
        drop(owner);
        drop(_directory);
    }
    Ok(())
}

fn prepare_voice_usage(
    fixture: &Fixture,
    before: &LiveLedgerStoredHead,
    channel: &LiveChannelId,
    observation: &meerkat_core::live_execution::observation::LiveUsageSnapshot,
) -> TestResult<PreparedLiveLedgerCommit> {
    use crate::live_ledger::transcript_authority::{dsl, voice_usage};
    let sequence = meerkat_core::live_observation::LiveObservationSeq::new(
        before
            .reference
            .event_count
            .checked_add(1)
            .ok_or("sequence")?,
    )?;
    let input = voice_usage::input(channel.as_str(), sequence.get(), observation)?;
    let owner = dsl::LiveTranscriptMachineAuthority::recover_from_state(
        crate::generated::live_transcript_state::decode(&before.payload.transcript_snapshot)?,
    )?;
    let mut candidate = owner.prepare_authority();
    dsl::LiveTranscriptMachineMutator::apply(&mut candidate, input.clone())?;
    let snapshot = voice_usage::snapshot(candidate.state(), channel.as_str())?.ok_or("usage")?;
    let record = crate::live_ledger::record::LiveLedgerRecord::Completion(
        crate::live_ledger::completion::LiveCompletionRecord {
            format: crate::live_ledger::transcript::LiveLedgerFormatV1::V1,
            session_id: fixture.session.id().clone(),
            channel_id: channel.clone(),
            sequence,
            event: LiveCompletionEvent::ChannelUsage { snapshot },
        },
    );
    Ok(PreparedLiveLedgerCommit::from_transcript_record(
        fixture.session.id(),
        Some(before),
        input,
        record,
    )?
    .0)
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_provider_start_waiter_observes_closed_without_inventing_start() -> TestResult {
    use crate::live_ledger::transcript_authority::LiveTranscriptWriteError;
    use meerkat_core::live_execution::observation::ContinuousLiveObservation;
    use meerkat_live::host::LiveContinuousTranscriptIngress;
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let channel = voice_ingress(&fixture, "never-started")
            .await?
            .into_shared();
        let (waiting, closed) = tokio::join!(
            channel.wait_provider_start(std::time::Duration::from_secs(2)),
            channel.apply_voice_observation(&ContinuousLiveObservation::ObservationStreamEnded),
        );
        closed?;
        assert!(matches!(waiting, Err(LiveTranscriptWriteError::Retired)));
        assert!(channel.read_provider_start().await?.is_none());
        assert_eq!(head(&fixture).await?.payload.reserved.records, 0);
    }
    Ok(())
}

#[tokio::test]
async fn native_provider_controls_measure_receipt_growth_at_zero_free_quota() -> TestResult {
    use crate::live_ledger::transcript_authority::{dsl, provider_control};
    use meerkat_core::live_execution::backend::*;
    let diagnostic = LiveProviderDiagnostic::new(
        LiveProviderDiagnosticCategory::UncorrelatedContextAcknowledgment,
        LiveBackendOwnership::Owned {
            response: LiveBackendResponseKey {
                response: meerkat_core::live_execution::request::LiveProviderReference::new(
                    "\0".repeat(128),
                )?,
                delegation: None,
            },
        },
        std::num::NonZeroU64::MAX,
    )?;
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let channel = voice_ingress(&fixture, &"\0".repeat(128)).await?;
        let before = head(&fixture).await?;
        let quota = before.payload.used.checked_add(before.payload.reserved)?;
        for observation in [
            provider_start(&"\0".repeat(128))?,
            meerkat_core::live_execution::observation::ContinuousLiveObservation::Diagnostic(
                diagnostic.clone(),
            ),
        ] {
            let before = head(&fixture).await?;
            let sequence = meerkat_core::live_observation::LiveObservationSeq::new(
                before.reference.event_count + 1,
            )?;
            let event = match observation {
                meerkat_core::live_execution::observation::ContinuousLiveObservation::ProviderStarted { provider_session } =>
                    LiveCompletionEvent::ChannelProviderStarted {
                        provider_session: crate::live_ledger::completion::LiveCompletionText::new(provider_session.as_str())?,
                    },
                meerkat_core::live_execution::observation::ContinuousLiveObservation::Diagnostic(diagnostic) =>
                    LiveCompletionEvent::ChannelProviderDiagnostic { diagnostic },
                _ => return Err("fixture".into()),
            };
            let kind = match &event {
                LiveCompletionEvent::ChannelProviderStarted { .. } => {
                    dsl::LiveProviderControlKind::Started
                }
                _ => dsl::LiveProviderControlKind::Diagnostic,
            };
            let input = dsl::LiveTranscriptInput::ObserveProviderControl {
                channel: channel.channel_id().to_string(),
                sequence: sequence.get(),
                kind,
                digest: provider_control::digest(
                    fixture.session.id(),
                    channel.channel_id(),
                    &event,
                )?,
                record_bytes: 1,
            };
            let record = crate::live_ledger::record::LiveLedgerRecord::Completion(
                crate::live_ledger::completion::LiveCompletionRecord {
                    format: crate::live_ledger::transcript::LiveLedgerFormatV1::V1,
                    session_id: fixture.session.id().clone(),
                    channel_id: channel.channel_id().clone(),
                    sequence,
                    event,
                },
            );
            let (mut prepared, _) = PreparedLiveLedgerCommit::from_transcript_record(
                fixture.session.id(),
                Some(&before),
                input,
                record,
            )?;
            prepared.quota = quota;
            assert!(
                prepared
                    .successor
                    .payload
                    .used
                    .checked_add(prepared.successor.payload.reserved)?
                    .fits_within(quota)
            );
            assert!(matches!(
                fixture
                    .ops()?
                    .commit_live_ledger(prepared, current_fence())
                    .await?,
                LiveLedgerCommitOutcome::Committed { .. }
            ));
        }
        let snapshot = head(&fixture).await?.payload.transcript_snapshot;
        let mut value: serde_json::Value = serde_json::from_slice(&snapshot)?;
        value
            .as_object_mut()
            .ok_or("snapshot")?
            .remove("provider_control_sequences");
        assert!(
            crate::generated::live_transcript_state::decode(&serde_json::to_vec(&value)?).is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_voice_accounting_at_full_quota_preserves_owed_final_and_closure() -> TestResult {
    use meerkat_core::live_execution::observation::{LiveUsageSnapshot, LiveVoiceDurationSeconds};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = voice_ingress(&fixture, &"\0".repeat(128)).await?;
        let before = head(&fixture).await?;
        let quota = before.payload.used.checked_add(before.payload.reserved)?;
        let seconds = LiveVoiceDurationSeconds::new(f64::MAX)?;
        let mut periodic = prepare_voice_usage(
            &fixture,
            &before,
            channel.channel_id(),
            &LiveUsageSnapshot::Periodic {
                cumulative_seconds: seconds,
            },
        )?;
        periodic.quota = quota;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(periodic, current_fence())
                .await
                .is_err()
        );
        assert_eq!(
            head(&fixture).await?,
            before,
            "unaccepted periodic usage spends nothing"
        );
        channel.close().await?;
        for observation in [
            LiveUsageSnapshot::SessionClosed {
                cumulative_seconds: seconds,
            },
            LiveUsageSnapshot::CloseUnconfirmed {
                last_observed_seconds: None,
            },
        ] {
            let before = head(&fixture).await?;
            let mut prepared =
                prepare_voice_usage(&fixture, &before, channel.channel_id(), &observation)?;
            prepared.quota = quota;
            assert!(
                prepared
                    .successor
                    .payload
                    .used
                    .checked_add(prepared.successor.payload.reserved)?
                    .fits_within(quota)
            );
            assert!(matches!(
                fixture
                    .ops()?
                    .commit_live_ledger(prepared, current_fence())
                    .await?,
                LiveLedgerCommitOutcome::Committed { .. }
            ));
        }
        assert_eq!(head(&fixture).await?.payload.reserved.records, 0);
    }
    Ok(())
}

fn retained_source(
    result: crate::live_ledger::transcript_authority::LiveSourceReservationOutcome,
) -> TestResult<crate::live_source::LiveSourceReservationRecord> {
    use crate::live_ledger::transcript_authority::LiveSourceReservationOutcome;
    match result {
        LiveSourceReservationOutcome::Retained(entry) => match *entry {
            crate::live_source::LiveSourceEntryRecord::Reservation { record } => Ok(*record),
            _ => Err("unexpected cancellation-only entry".into()),
        },
        _ => Err("source was not retained".into()),
    }
}

fn delegation(
    name: &str,
) -> TestResult<meerkat_core::live_execution::request::LiveProviderReference> {
    Ok(meerkat_core::live_execution::request::LiveProviderReference::new(name)?)
}

#[tokio::test]
async fn rejected_transcript_range_preserves_known_receive_gap_before_source_selection()
-> TestResult {
    use crate::live_ledger::transcript_authority::{
        LiveSourceReservationOutcome, LiveTranscriptWriteError,
    };
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = ingress(&fixture).await?;
        let before = head(&fixture).await?;
        assert!(matches!(
            channel
                .append_received_observation(Err(
                    meerkat_core::live_observation::LiveObservationValueError::InvalidRange
                ),)
                .await,
            Err(LiveTranscriptWriteError::InvalidObservation(_))
        ));
        assert_eq!(channel.received_ordinal(), 1);
        assert_eq!(head(&fixture).await?, before);
        assert!(matches!(
            channel
                .reserve_client_source(delegation("rejected")?, 0.0, None)
                .await?,
            LiveSourceReservationOutcome::AwaitingObservationDurability
        ));
        channel.append(text("next accepted text")?).await?;
        let reserved = retained_source(
            channel
                .reserve_client_source(delegation("rejected")?, 0.0, None)
                .await?,
        )?;
        assert_eq!(
            reserved.disposition(),
            &crate::live_source::LiveSourceDisposition::Refused {
                reason: crate::live_source::LiveSourceRefusal::Gap,
            }
        );
        let stored = head(&fixture).await?;
        let state =
            crate::generated::live_transcript_state::decode(&stored.payload.transcript_snapshot)?;
        assert_eq!(state.receive_ordinals["voice"], 2);
        assert_eq!(state.gap_channels.len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn native_source_budget_and_precancellation_never_mint_work() -> TestResult {
    use crate::live_ledger::authority::store::LiveRequestStoreOwner;
    use crate::live_ledger::transcript_authority::LiveSourceReservationOutcome;
    use crate::live_source::{LiveSourceDisposition, LiveSourceRefusal};
    use meerkat_core::live_execution::request::{
        LiveRequestCancelIntent, LiveRequestCancellationReason, LiveSourceIdentity, LiveSourceKey,
    };
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = ingress(&fixture).await?;
        let cancelled_key = LiveSourceKey::new(
            fixture.session.id().clone(),
            channel.channel_id().clone(),
            LiveSourceIdentity::ClientDelegation {
                delegation: delegation("cancelled")?,
            },
        )?;
        let intent = LiveRequestCancelIntent {
            source: cancelled_key.clone(),
            reason: LiveRequestCancellationReason::OperatorRequested,
        };
        let owner =
            LiveRequestStoreOwner::new(Arc::clone(&fixture.store), fixture.session.id().clone());
        owner.cancel_source(intent.clone()).await?;
        let before = head(&fixture).await?;
        let LiveSourceReservationOutcome::Retained(entry) = channel
            .reserve_client_source(delegation("cancelled")?, 0.0, None)
            .await?
        else {
            return Err("precancellation lost".into());
        };
        assert_eq!(
            *entry,
            crate::live_source::LiveSourceEntryRecord::CancellationOnly { intent }
        );
        assert_eq!(head(&fixture).await?, before);
        channel
            .append(text(" x ".repeat(
                meerkat_core::live_execution::evidence::LIVE_REQUEST_TEXT_MAX_BYTES / 3 + 1,
            ))?)
            .await?;
        let large_head = head(&fixture).await?;
        let budget = retained_source(
            channel
                .reserve_client_source(delegation("large")?, 0.0, None)
                .await?,
        )?;
        assert_eq!(
            budget.disposition(),
            &LiveSourceDisposition::Refused {
                reason: LiveSourceRefusal::Budget
            }
        );
        assert_eq!(budget.reserved_frontier(), large_head.reference.event_count);
        assert!(budget.frozen_request().is_none());
        channel.append(text("not a truncated retry")?).await?;
        assert_eq!(
            retained_source(
                channel
                    .reserve_client_source(delegation("large")?, 0.0, None)
                    .await?
            )?,
            budget
        );
        let state = crate::generated::live_request_state::decode(
            &head(&fixture).await?.payload.request_snapshot,
        )?;
        assert!(state.request_ids.is_empty());
        assert!(state.request_inputs.is_empty());
        assert!(state.run_requests.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn native_source_empty_ranges_spend_once_without_minting_work() -> TestResult {
    use crate::live_source::{LiveSourceDisposition, LiveSourceRefusal};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let actor = fixture.actor().await?;
        let mut channel = ingress(&fixture).await?;
        let zero = retained_source(
            channel
                .reserve_client_source(delegation("zero")?, 0.0, None)
                .await?,
        )?;
        assert!(zero.context().interval.is_empty());
        for value in ["", " \n", "\u{2003}\u{3000}\u{a0}"] {
            channel.append(text(value)?).await?;
        }
        let first = retained_source(
            channel
                .reserve_client_source(delegation("d1")?, 0.0, None)
                .await?,
        )?;
        assert_eq!(first.context().interval.after(), zero.reserved_frontier());
        assert!(!first.context().interval.is_empty());
        assert_eq!(
            first.disposition(),
            &LiveSourceDisposition::Refused {
                reason: LiveSourceRefusal::Empty
            }
        );
        assert!(first.frozen_request().is_none());
        channel.append(text("later")?).await?;
        let replay = retained_source(
            channel
                .reserve_client_source(delegation("d1")?, 0.0, None)
                .await?,
        )?;
        assert_eq!(replay, first);
        let second = retained_source(
            channel
                .reserve_client_source(delegation("d2")?, 0.0, None)
                .await?,
        )?;
        assert_eq!(second.context().interval.after(), first.reserved_frontier());
        assert_eq!(
            second.disposition(),
            &LiveSourceDisposition::Refused {
                reason: LiveSourceRefusal::Permission
            }
        );
        channel.close().await?;
        assert_eq!(
            retained_source(
                channel
                    .reserve_client_source(delegation("d1")?, 0.0, None)
                    .await?
            )?,
            first
        );
        let current = head(&fixture).await?;
        let requests =
            crate::generated::live_request_state::decode(&current.payload.request_snapshot)?;
        assert!(requests.request_ids.is_empty());
        assert!(requests.request_inputs.is_empty());
        assert!(requests.run_requests.is_empty());
        assert!(requests.request_completion_obligations.is_empty());
        assert_eq!(requests.source_refusals.len(), 3);
        assert_eq!(fixture.actor().await?, actor);
        drop(channel);
        #[cfg(feature = "sqlite-store")]
        if !matches!(backend, Backend::Memory) {
            assert_eq!(Arc::strong_count(&fixture.store), 1);
            let Fixture {
                store,
                session: _,
                _directory,
                path,
            } = fixture;
            drop(store);
            let reopened = match backend {
                Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&path)?
                }
                Backend::Memory => return Err("expected disk backend".into()),
            };
            let row = reopened
                .live_ledger_ops()
                .ok_or("ops")?
                .lookup_live_source(first.source())
                .await?
                .ok_or("source")?;
            let crate::live_source::LiveSourceEntryRecord::Reservation { record } = row.record()?
            else {
                return Err("lost reservation".into());
            };
            assert_eq!(*record, first);
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_source_waiting_and_gap_never_become_empty() -> TestResult {
    use crate::live_ledger::transcript_authority::LiveSourceReservationOutcome;
    use crate::live_source::{LiveSourceDisposition, LiveSourceRefusal};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = ingress(&fixture).await?;
        assert!(channel.append(text("x".repeat(128 * 1024))?).await.is_err());
        let before = head(&fixture).await?;
        assert!(matches!(
            channel
                .reserve_client_source(delegation("gap")?, 0.0, None)
                .await?,
            LiveSourceReservationOutcome::AwaitingObservationDurability
        ));
        assert_eq!(head(&fixture).await?, before);
        channel.append(text("")?).await?;
        let source = retained_source(
            channel
                .reserve_client_source(delegation("gap")?, 0.0, None)
                .await?,
        )?;
        assert_eq!(
            source.disposition(),
            &LiveSourceDisposition::Refused {
                reason: LiveSourceRefusal::Gap
            }
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_source_reservation_pages_exact_text_before_delayed_admission() -> TestResult {
    use crate::live_source::LiveSourceDisposition;
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let epoch = meerkat_core::RuntimeEpochId::new();
        install_grant_executor(&fixture, &epoch, 1).await?;
        let grant = crate::live_grant::LiveExecutionGrantIssuer::new(Arc::clone(&fixture.store))
            .activate(grant_activation_request(&fixture, &epoch)?, current_fence())
            .await?;
        let version = fixture
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
            .await?
            .version()
            .ok_or("lifecycle")?
            .clone();
        let mut channel = LiveTranscriptStoreOwner::new(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
            current_fence(),
        )
        .activate_channel(LiveChannelId::new("voice"), version.clone())
        .await?;
        for _ in 0..257 {
            channel.append(text(" \u{2003}x\n")?).await?;
        }
        let watermark = head(&fixture).await?.reference.event_count;
        let mut foreign = LiveTranscriptStoreOwner::new(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
            current_fence(),
        )
        .activate_channel(LiveChannelId::new("foreign"), version)
        .await?;
        foreign.append(text("not this source")?).await?;
        let first = retained_source(
            channel
                .reserve_client_source(delegation("d1")?, 1.0, Some(&grant))
                .await?,
        )?;
        assert_eq!(first.disposition(), &LiveSourceDisposition::Reserved {});
        assert_eq!(first.reserved_frontier(), watermark);
        assert!(first.context().live_head.event_count > watermark);
        assert_eq!(
            first.frozen_request().ok_or("request")?.request().as_str(),
            " \u{2003}x\n".repeat(257)
        );
        channel.append(text(" second ")?).await?;
        let second = retained_source(
            channel
                .reserve_client_source(delegation("d2")?, 2.0, Some(&grant))
                .await?,
        )?;
        assert_eq!(second.context().interval.after(), first.reserved_frontier());
        assert_eq!(
            second.frozen_request().ok_or("request")?.request().as_str(),
            " second "
        );
        let current = head(&fixture).await?;
        let requests =
            crate::generated::live_request_state::decode(&current.payload.request_snapshot)?;
        assert_eq!(requests.request_ids.len(), 2);
        assert_eq!(requests.request_completion_obligations.len(), 2);
        assert!(requests.request_inputs.is_empty());
        assert!(requests.run_requests.is_empty());
        let explicit = retained_source(
            channel
                .reserve_application_source(
                    meerkat_core::live_execution::request::LiveApplicationRequestId::from_uuid(
                        uuid::Uuid::new_v4(),
                    ),
                    meerkat_core::live_execution::evidence::LiveObservationInterval::new(1, 2)?,
                    None,
                )
                .await?,
        )?;
        assert_eq!(
            explicit
                .frozen_request()
                .ok_or("explicit content")?
                .request()
                .as_str(),
            " \u{2003}x\n"
        );
        let explicit_head = head(&fixture).await?;
        let transcript = crate::generated::live_transcript_state::decode(
            &explicit_head.payload.transcript_snapshot,
        )?;
        assert_eq!(
            transcript.reservation_frontiers["voice"],
            second.reserved_frontier()
        );
        assert!(
            channel
                .reserve_client_source(delegation("d1")?, 2.0, Some(&grant))
                .await
                .is_err()
        );
        assert_eq!(head(&fixture).await?, explicit_head);
    }
    Ok(())
}

#[tokio::test]
async fn native_transcript_ingress_keeps_actor_and_request_independent() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let actor = fixture.actor().await?;
        let mut channel = ingress(&fixture).await?;
        let initial = head(&fixture).await?;
        channel.append(text("\0\n\"exact \u{1f600}\"")?).await?;
        let accepted = head(&fixture).await?;
        assert_eq!(fixture.actor().await?, actor);
        assert_eq!(
            accepted.payload.request_snapshot,
            initial.payload.request_snapshot
        );
        let state =
            crate::generated::live_transcript_state::decode(&accepted.payload.transcript_snapshot)?;
        assert_eq!(state.receive_ordinals["voice"], 1);
        assert_eq!(state.durable_watermarks["voice"], 2);
        assert_eq!(state.reservation_frontiers["voice"], 1);
        let records = fixture
            .ops()?
            .read_live_history(&crate::store::live_history::LiveHistoryReadRequest::new(
                accepted.reference,
                None,
                0,
                8,
            )?)
            .await?;
        assert!(
            matches!(&records.records()[1], LiveLedgerRecord::Observation(record)
            if record.record().observation.text() == "\0\n\"exact \u{1f600}\"")
        );
        channel.close().await?;
        let closed = head(&fixture).await?;
        assert_eq!(
            closed.payload.reserved,
            LiveResourceCharge {
                records: 0,
                encoded_bytes: 20
            }
        );
        assert_eq!(fixture.actor().await?, actor);
        assert!(channel.append(text("after close")?).await.is_err());
    }
    Ok(())
}

#[tokio::test]
async fn native_transcript_loss_closes_known_tail_with_last_credit() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = ingress(&fixture).await?;
        let count = crate::live_resources::LiveCompletionObligation::ChannelControl.record_limit();
        for _ in 1..count {
            assert!(
                channel
                    .append(text("x".repeat(
                        meerkat_contracts::wire::live_observation::LIVE_OBSERVATION_TEXT_MAX_BYTES
                            + 1,
                    ))?)
                    .await
                    .is_err()
            );
            channel.append(text("retained")?).await?;
        }
        let before = head(&fixture).await?;
        assert_eq!(before.payload.reserved.records, 1);
        assert!(
            channel
                .append(text("x".repeat(
                    meerkat_contracts::wire::live_observation::LIVE_OBSERVATION_TEXT_MAX_BYTES + 1,
                ))?)
                .await
                .is_err()
        );
        channel.close().await?;
        let closed = head(&fixture).await?;
        let state =
            crate::generated::live_transcript_state::decode(&closed.payload.transcript_snapshot)?;
        assert!(state.accepting_channels.is_empty());
        assert_eq!(state.receive_ordinals["voice"], channel.received_ordinal());
        assert_eq!(state.control_spent_records["voice"], count);
        assert_eq!(
            closed.payload.reserved,
            LiveResourceCharge {
                records: 0,
                encoded_bytes: 20
            }
        );
        let records = fixture
            .ops()?
            .read_live_history(&crate::store::live_history::LiveHistoryReadRequest::new(
                closed.reference,
                None,
                before.reference.event_count,
                1,
            )?)
            .await?;
        assert!(
            matches!(&records.records()[0], LiveLedgerRecord::Completion(record)
            if matches!(&record.event, LiveCompletionEvent::ChannelDiscontinuity {
                discontinuity: LiveDiscontinuity::KnownLocalGap { observed_bounds, .. }
            } if observed_bounds.through_received_ordinal() == channel.received_ordinal()
                && observed_bounds.after_received_ordinal() + 1 == channel.received_ordinal()))
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_transcript_activation_requires_current_lifecycle() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let epoch = meerkat_core::RuntimeEpochId::new();
        install_grant_executor(&fixture, &epoch, 0).await?;
        let version = fixture
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
            .await?
            .version()
            .ok_or("version")?
            .clone();
        install_grant_executor(&fixture, &epoch, 1).await?;
        let owner = LiveTranscriptStoreOwner::new(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
            current_fence(),
        );
        assert!(matches!(
            owner
                .activate_channel(LiveChannelId::new("voice"), version)
                .await,
            Err(
                crate::live_ledger::transcript_authority::LiveTranscriptWriteError::Store(
                    RuntimeStoreError::MachineLifecycleVersionConflict { .. }
                )
            )
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
async fn native_transcript_snapshot_codec_rejects_lost_or_unknown_authority() -> TestResult {
    let fixture = Fixture::new(Backend::Memory).await?;
    let _channel = ingress(&fixture).await?;
    let head = head(&fixture).await?;
    let original: serde_json::Value = serde_json::from_slice(&head.payload.transcript_snapshot)?;
    let decoded =
        crate::generated::live_transcript_state::decode(&head.payload.transcript_snapshot)?;
    let encoded = crate::generated::live_transcript_state::encode(&decoded)?;
    assert_eq!(
        encoded.as_slice(),
        head.payload.transcript_snapshot.as_slice()
    );
    for field in [
        "receive_ordinals",
        "reservation_frontiers",
        "control_spent_bytes",
    ] {
        let mut value = original.clone();
        value.as_object_mut().ok_or("object")?.remove(field);
        assert!(
            crate::generated::live_transcript_state::decode(&serde_json::to_vec(&value)?).is_err()
        );
    }
    for version in [0, 1, 2, 4, u64::MAX] {
        let mut value = original.clone();
        value["format"] = version.into();
        assert!(
            crate::generated::live_transcript_state::decode(&serde_json::to_vec(&value)?).is_err()
        );
    }
    let mut value = original.clone();
    value["unknown_authority"] = true.into();
    assert!(crate::generated::live_transcript_state::decode(&serde_json::to_vec(&value)?).is_err());
    let mut value = original;
    value["receive_ordinals"]
        .as_object_mut()
        .ok_or("map")?
        .remove("voice");
    let state = crate::generated::live_transcript_state::decode(&serde_json::to_vec(&value)?)?;
    assert!(crate::live_ledger::transcript_authority::dsl::LiveTranscriptMachineAuthority::recover_from_state(state).is_err());
    Ok(())
}

struct SwitchableTranscriptFence(std::sync::atomic::AtomicBool);

#[tokio::test]
async fn native_refused_source_cancellation_preserves_exact_refusal() -> TestResult {
    use crate::live_ledger::authority::store::LiveRequestStoreOwner;
    use meerkat_core::live_execution::request::{
        LiveRequestCancelIntent, LiveRequestCancellationReason,
    };
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut channel = ingress(&fixture).await?;
        channel.append(text("\u{2003}")?).await?;
        let original = retained_source(
            channel
                .reserve_client_source(delegation("d1")?, 0.0, None)
                .await?,
        )?;
        let owner =
            LiveRequestStoreOwner::new(Arc::clone(&fixture.store), fixture.session.id().clone());
        let intent = LiveRequestCancelIntent {
            source: original.source().clone(),
            reason: LiveRequestCancellationReason::OperatorRequested,
        };
        let cancellation = owner.cancel_source(intent.clone()).await?;
        assert!(cancellation.target.is_none());
        assert_eq!(cancellation.intent, intent);
        let cancelled = retained_source(
            channel
                .reserve_client_source(delegation("d1")?, 0.0, None)
                .await?,
        )?;
        assert_eq!(cancelled.disposition(), original.disposition());
        assert_eq!(cancelled.frozen_digest()?, original.frozen_digest()?);
        assert_eq!(cancelled.cancellation(), Some(&intent.reason));
        let entry = crate::live_source::LiveSourceEntryRecord::Reservation {
            record: Box::new(cancelled.clone()),
        };
        let mut altered = serde_json::to_value(&entry)?;
        altered["record"]["disposition"]["reason"] = "permission".into();
        let altered: crate::live_source::LiveSourceEntryRecord = serde_json::from_value(altered)?;
        assert!(!entry.preserves_frozen_content(&altered));
        owner.cancel_source(intent).await?;
        assert_eq!(
            retained_source(
                channel
                    .reserve_client_source(delegation("d1")?, 0.0, None)
                    .await?
            )?,
            cancelled
        );
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_source_missing_accepted_watermark_is_not_empty() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let mut channel = ingress(&fixture).await?;
        channel.append(text("")?).await?;
        let before = head(&fixture).await?;
        let conn = meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        assert_eq!(
            conn.execute(
                "DELETE FROM runtime_live_events WHERE session_id = ?1 AND sequence = ?2",
                rusqlite::params![
                    fixture.session.id().to_string(),
                    before.reference.event_count
                ],
            )?,
            1
        );
        assert!(
            channel
                .reserve_client_source(delegation("d1")?, 0.0, None)
                .await
                .is_err()
        );
        assert!(head(&fixture).await.is_err());
        let (revision, event_count, transcript, request): (u64, u64, Vec<u8>, Vec<u8>) = conn.query_row(
            "SELECT revision, event_count, transcript_snapshot, request_snapshot FROM runtime_live_heads WHERE session_id = ?1",
            [fixture.session.id().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        assert_eq!(revision, before.reference.revision);
        assert_eq!(event_count, before.reference.event_count);
        assert_eq!(transcript, *before.payload.transcript_snapshot);
        assert_eq!(request, *before.payload.request_snapshot);
        let count: u64 =
            conn.query_row("SELECT COUNT(*) FROM runtime_live_sources", [], |row| {
                row.get(0)
            })?;
        assert_eq!(count, 0);
    }
    Ok(())
}

struct UncertainSourceFence(std::sync::atomic::AtomicBool);

impl RuntimeStoreWriteFence for UncertainSourceFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        if !self.0.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(RuntimeStoreError::WriteFailed(
                "injected unclassified publication failure".into(),
            ));
        }
        operation()?;
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

#[tokio::test]
async fn native_source_publication_failure_never_spends_or_refreezes() -> TestResult {
    use crate::live_ledger::transcript_authority::LiveSourceReservationError;
    use std::sync::atomic::{AtomicBool, Ordering};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        install_grant_executor(&fixture, &meerkat_core::RuntimeEpochId::new(), 0).await?;
        let version = fixture
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
            .await?
            .version()
            .ok_or("version")?
            .clone();
        let fence = Arc::new(UncertainSourceFence(AtomicBool::new(true)));
        let mut channel = LiveTranscriptStoreOwner::new(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
            fence.clone(),
        )
        .activate_channel(LiveChannelId::new("voice"), version)
        .await?;
        channel.append(text("old")?).await?;
        let before = head(&fixture).await?;
        let actor = fixture.actor().await?;
        fence.0.store(false, Ordering::SeqCst);
        assert!(matches!(
            channel
                .reserve_client_source(delegation("d1")?, 0.0, None)
                .await,
            Err(LiveSourceReservationError::Store(
                RuntimeStoreError::WriteFailed(_)
            ))
        ));
        assert_eq!(head(&fixture).await?, before);
        fence.0.store(true, Ordering::SeqCst);
        channel.append(text("newer")?).await?;
        let advanced = head(&fixture).await?;
        for source in ["d1", "d2"] {
            assert!(matches!(
                channel
                    .reserve_client_source(delegation(source)?, 0.0, None)
                    .await,
                Err(LiveSourceReservationError::PriorSourceUnconfirmed)
            ));
        }
        assert_eq!(head(&fixture).await?, advanced);
        assert_eq!(fixture.actor().await?, actor);
        let state =
            crate::generated::live_transcript_state::decode(&advanced.payload.transcript_snapshot)?;
        assert_eq!(state.reservation_frontiers["voice"], 1);
    }
    Ok(())
}

impl RuntimeStoreWriteFence for SwitchableTranscriptFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            operation()?;
            Ok(RuntimeStoreWriteFenceOutcome::Applied)
        } else {
            Ok(RuntimeStoreWriteFenceOutcome::Backoff {
                reason: "test registration temporarily unobservable".into(),
            })
        }
    }
}

#[tokio::test]
async fn native_transcript_publication_fence_preserves_uncommitted_receive_gap() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        install_grant_executor(&fixture, &meerkat_core::RuntimeEpochId::new(), 0).await?;
        let version = fixture
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
            .await?
            .version()
            .ok_or("version")?
            .clone();
        let fence = Arc::new(SwitchableTranscriptFence(
            std::sync::atomic::AtomicBool::new(true),
        ));
        let mut channel = LiveTranscriptStoreOwner::new(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
            fence.clone(),
        )
        .activate_channel(LiveChannelId::new("voice"), version)
        .await?;
        let before = head(&fixture).await?;
        fence.0.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(channel.append(text("not durably accepted")?).await.is_err());
        assert_eq!(head(&fixture).await?, before);
        assert_eq!(channel.received_ordinal(), 1);
        fence.0.store(true, std::sync::atomic::Ordering::SeqCst);
        channel.append(text("accepted successor")?).await?;
        let after = head(&fixture).await?;
        let state =
            crate::generated::live_transcript_state::decode(&after.payload.transcript_snapshot)?;
        assert_eq!(state.receive_ordinals["voice"], 2);
        assert_eq!(state.gap_channels.len(), 1);
        let records = fixture
            .ops()?
            .read_live_history(&crate::store::live_history::LiveHistoryReadRequest::new(
                after.reference,
                None,
                1,
                2,
            )?)
            .await?;
        assert!(
            matches!(&records.records()[0], LiveLedgerRecord::Completion(record)
                if matches!(&record.event, LiveCompletionEvent::ChannelDiscontinuity {
                    discontinuity: LiveDiscontinuity::KnownLocalGap { observed_bounds, .. }
                } if observed_bounds.after_received_ordinal() == 0
                    && observed_bounds.through_received_ordinal() == 1))
        );
        assert!(
            matches!(&records.records()[1], LiveLedgerRecord::Observation(record)
                if record.record().observation.text() == "accepted successor")
        );
    }
    Ok(())
}
#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_transcript_cold_recovery_cannot_guess_uncommitted_receive_count() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let session = Session::new();
        let mut predecessor = None;
        let mut recovered_snapshot = None;
        for lost in [0, 7] {
            let fixture = Fixture::with_session(backend, session.clone()).await?;
            let mut channel = ingress(&fixture).await?;
            for _ in 0..lost {
                assert!(channel.append(text("x".repeat(
                    meerkat_contracts::wire::live_observation::LIVE_OBSERVATION_TEXT_MAX_BYTES + 1,
                ))?).await.is_err());
            }
            assert_eq!(channel.received_ordinal(), lost);
            let before = head(&fixture).await?;
            if let Some(expected) = &predecessor {
                assert_eq!(
                    &before, expected,
                    "same durable Live state despite distinct local receives"
                );
            } else {
                predecessor = Some(before.clone());
            }
            drop(channel);
            let Fixture {
                store,
                session,
                path,
                _directory,
            } = fixture;
            assert_eq!(
                Arc::strong_count(&store),
                1,
                "must actually close the store"
            );
            drop(store);
            let reopened: Arc<dyn RuntimeStore> = Arc::new(match backend {
                Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&path)?
                }
                Backend::Memory => return Err("not a cold backend".into()),
            });
            let owner = LiveTranscriptStoreOwner::new(
                Arc::clone(&reopened),
                session.id().clone(),
                current_fence(),
            );
            assert_eq!(owner.recover_unknown_tails().await?.len(), 1);
            assert!(owner.recover_unknown_tails().await?.is_empty());
            let ops = reopened.live_ledger_ops().ok_or("ops")?;
            let after = ops.load_live_head(session.id()).await?.ok_or("head")?;
            let state = crate::generated::live_transcript_state::decode(
                &after.payload.transcript_snapshot,
            )?;
            assert_eq!(state.receive_ordinals["voice"], 0);
            assert!(state.accepting_channels.is_empty());
            if let Some(expected) = &recovered_snapshot {
                assert_eq!(&after.payload.transcript_snapshot, expected);
            } else {
                recovered_snapshot = Some(Arc::clone(&after.payload.transcript_snapshot));
            }
            let records = ops
                .read_live_history(&crate::store::live_history::LiveHistoryReadRequest::new(
                    after.reference,
                    None,
                    1,
                    1,
                )?)
                .await?;
            assert!(
                matches!(&records.records()[0], LiveLedgerRecord::Completion(record)
                if matches!(&record.event, LiveCompletionEvent::ChannelDiscontinuity {
                    discontinuity: LiveDiscontinuity::UnknownExtentCrashDiscontinuity { last_accepted_head, .. }
                } if *last_accepted_head == before.reference))
            );
        }
    }
    Ok(())
}
