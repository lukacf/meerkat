use super::*;
use crate::live_ledger::authority::dsl::LiveRequestInput as Input;
use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
use std::sync::atomic::{AtomicU64, Ordering};

struct AdvanceClockFence {
    clock: Arc<AtomicU64>,
    now: u64,
}

impl RuntimeStoreWriteFence for AdvanceClockFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        self.clock.store(self.now, Ordering::SeqCst);
        operation()?;
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

fn timed_commands() -> Vec<Input> {
    let mut commands = generated_request_setup();
    commands.push(Input::RestoreScope {
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
    });
    commands.push(generated_effect_claim());
    commands
}

#[tokio::test]
async fn native_source_expiry_before_publication_is_not_an_uncertain_write() -> TestResult {
    use crate::live_ledger::authority::store::source_reservation::SourceSelection;
    use crate::live_ledger::transcript_authority::{
        LiveSourceReservationError, LiveSourceReservationOutcome, LiveTranscriptStoreOwner,
    };
    use meerkat_core::live_execution::{
        LiveChannelId,
        request::{LiveProviderReference, LiveSourceIdentity, LiveSourceKey},
    };
    use meerkat_core::live_observation::{
        LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
    };
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let epoch = meerkat_core::RuntimeEpochId::new();
        install_grant_executor(&fixture, &epoch, 1).await?;
        let grant = crate::live_grant::LiveExecutionGrantIssuer::new(Arc::clone(&fixture.store))
            .activate(grant_activation_request(&fixture, &epoch)?, current_fence())
            .await?;
        let lifecycle = fixture
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
        .activate_channel(LiveChannelId::new("voice"), lifecycle)
        .await?;
        channel
            .append(LiveTranscriptObservation::new(
                LiveTranscriptDirection::Input,
                LiveTranscriptRange::new(0.0, 1.0)?,
                "exact",
            ))
            .await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&before.payload.request_snapshot)?;
        let clock = Arc::new(AtomicU64::new(state.grant_expiry - 1));
        let read_clock = Arc::clone(&clock);
        let owner =
            LiveRequestStoreOwner::new(Arc::clone(&fixture.store), fixture.session.id().clone())
                .with_clock(Arc::new(move || Ok(read_clock.load(Ordering::SeqCst))));
        let source = LiveSourceKey::new(
            fixture.session.id().clone(),
            channel.channel_id().clone(),
            LiveSourceIdentity::ClientDelegation {
                delegation: LiveProviderReference::new("expiry")?,
            },
        )?;
        let selection = || -> TestResult<SourceSelection> {
            Ok(SourceSelection {
                source: source.clone(),
                fingerprint: crate::live_source::LiveSourceFingerprint::client_delegation(0.0)?,
                received: channel.received_ordinal(),
                ingress_generation: 1,
                explicit: None,
            })
        };
        let mut unconfirmed = None;
        assert!(matches!(
            owner
                .reserve_selected_source(
                    selection()?,
                    Some(&grant),
                    Arc::new(AdvanceClockFence {
                        clock: Arc::clone(&clock),
                        now: state.grant_expiry
                    }),
                    &mut unconfirmed,
                )
                .await,
            Err(LiveSourceReservationError::Store(
                RuntimeStoreError::LiveRequestPublicationRejected { .. }
            ))
        ));
        assert!(unconfirmed.is_none());
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(before)
        );
        assert!(fixture.ops()?.lookup_live_source(&source).await?.is_none());
        let LiveSourceReservationOutcome::Retained(entry) = owner
            .reserve_selected_source(
                selection()?,
                Some(&grant),
                current_fence(),
                &mut unconfirmed,
            )
            .await?
        else {
            return Err("expired source was not durably refused".into());
        };
        let crate::live_source::LiveSourceEntryRecord::Reservation { record } = *entry else {
            return Err("unexpected cancellation".into());
        };
        assert_eq!(
            record.disposition(),
            &crate::live_source::LiveSourceDisposition::Refused {
                reason: crate::live_source::LiveSourceRefusal::Permission,
            }
        );
        assert!(unconfirmed.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn generated_store_rechecks_each_timed_command_inside_registration_and_store_fences()
-> TestResult {
    for backend in backends() {
        for command_index in 0..timed_commands().len() {
            let fixture = Fixture::new(backend).await?;
            let clock = Arc::new(AtomicU64::new(1));
            let read_clock = Arc::clone(&clock);
            let owner = LiveRequestStoreOwner::new(
                Arc::clone(&fixture.store),
                fixture.session.id().clone(),
            )
            .with_clock(Arc::new(move || Ok(read_clock.load(Ordering::SeqCst))));
            let commands = timed_commands();
            for command in &commands[..command_index] {
                owner.commit(command.clone(), current_fence()).await?;
            }
            let before = fixture.ops()?.load_live_head(fixture.session.id()).await?;
            let result = owner
                .commit(
                    commands[command_index].clone(),
                    Arc::new(AdvanceClockFence {
                        clock: Arc::clone(&clock),
                        now: 100,
                    }),
                )
                .await;
            assert!(
                matches!(
                    result,
                    Err(LiveRequestAuthorityError::Store(RuntimeStoreError::LiveRequestPublicationRejected {
                        ref reason
                    })) if reason.contains("Live request authority changed before publication")
                ),
                "{backend:?}, command {command_index}: {:?}",
                result.as_ref().map(|_| ())
            );
            assert_eq!(
                fixture.ops()?.load_live_head(fixture.session.id()).await?,
                before,
                "{backend:?}, command {command_index}"
            );

            // A rolled-back claim/admission must not spend an identity or quota.
            clock.store(1, Ordering::SeqCst);
            let committed = owner
                .commit(commands[command_index].clone(), current_fence())
                .await?;
            assert_eq!(
                committed.head.revision,
                before.map_or(1, |head| head.reference.revision + 1)
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn generated_store_uses_owner_clock_instead_of_command_timestamp() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let owner =
            LiveRequestStoreOwner::new(Arc::clone(&fixture.store), fixture.session.id().clone())
                .with_clock(Arc::new(|| Ok(100)));
        assert!(matches!(
            owner.commit(generated_activation(1), current_fence()).await,
            Err(LiveRequestAuthorityError::Transition(_))
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
async fn generated_store_clock_failure_does_not_publish_or_prevent_untimed_revocation() -> TestResult
{
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let clock = Arc::new(AtomicU64::new(1));
        let read_clock = Arc::clone(&clock);
        let owner =
            LiveRequestStoreOwner::new(Arc::clone(&fixture.store), fixture.session.id().clone())
                .with_clock(Arc::new(move || {
                    let now = read_clock.load(Ordering::SeqCst);
                    if now == u64::MAX {
                        Err(RuntimeStoreError::WriteFailed(
                            "test clock unavailable".into(),
                        ))
                    } else {
                        Ok(now)
                    }
                }));
        owner
            .commit(generated_activation(1), current_fence())
            .await?;
        let before = fixture.ops()?.load_live_head(fixture.session.id()).await?;
        let reserve = timed_commands().remove(1);
        assert!(matches!(
            owner
                .commit(
                    reserve,
                    Arc::new(AdvanceClockFence {
                        clock: Arc::clone(&clock),
                        now: u64::MAX,
                    }),
                )
                .await,
            Err(LiveRequestAuthorityError::Store(RuntimeStoreError::LiveRequestPublicationRejected {
                reason
            })) if reason.contains("test clock unavailable")
        ));
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            before
        );
        owner
            .commit(
                Input::Revoke {
                    grant_id: "grant".into(),
                    generation: 1,
                },
                current_fence(),
            )
            .await?;
    }
    Ok(())
}
