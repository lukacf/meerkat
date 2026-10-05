---- MODULE live_context_result_barrier_audit ----
\* Hand-written bounded audit of the live-context result barrier.
\*
\* A delegation result on a channel with a late bootstrap summary is released
\* after the summary's provider acknowledgement, and only after it. Queued
\* context rows (replays of speech the provider already heard, typed and
\* runtime-work rows) do not hold the result: they wait for a provider turn
\* boundary, so a barrier that waited for them would hold results for as long
\* as the user keeps speaking (Turbo S S99). ObserveLiveContextDeliveryReadiness
\* and the AuthorizeLiveDelegationResultDelivery guard must state the same rule;
\* when they disagreed, the release task spun on refused authorizations.
\*
\* This audit keeps the generated MeerkatMachine model unchanged. A
\* deterministic prefix reaches one experimental channel whose bootstrap
\* summary is in flight behind a queued voiced row, with one delegation
\* whose worker completed and whose result release is authorized. The body
\* explores the summary's ACK cut and delivered resolution, result-delivery authorization and readiness
\* observation. Every explored state is checked against every generated
\* invariant plus:
\*   AuditResultFollowsSummary: a result delivery is authorized only once the
\*     summary is acknowledged.
\* live_context_result_barrier_audit.sh runs the goal below as the only
\* property and requires TLC to report it violated:
\*   - a result delivery is authorized while a replay is still queued.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* live_context_result_barrier_audit.sh. Run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/live_context_result_barrier_audit.sh 26
EXTENDS model

CONSTANT AuditMaxSteps

AuditSession == "sessionid_1"
AuditRuntime == "runtime_1"
AuditIdentity == "identity_1"
AuditChannel == "channel_a"
AuditLease == "lease_1"
AuditDigest == "digest_s"
AuditBootstrap == "append_boot"
AuditInteraction == "interaction_1"
AuditOperation == "operation_1"
AuditTurn == "turn_1"
AuditWorker == "worker_1"
AuditPrefixLength == 18

\* A deterministic prefix reaches a bound experimental channel whose summary
\* is in flight after the user started speaking, with a released delegation
\* result waiting.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ model_step_count = 3 /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditChannel, AuditIdentity)
    \/ model_step_count = 4 /\ ResolveLiveExecutionModeAdmissionAttached(AuditSession, AuditChannel, "profile_1", "ClientContext", FALSE, TRUE)
    \/ model_step_count = 5 /\ StageExperimentalLiveExecutionAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, 0, "pending_a")
    \/ model_step_count = 6 /\ BeginLiveContextPreparationAttached(AuditSession, AuditChannel, AuditLease, 1, AuditRuntime, 1, 1)
    \/ model_step_count = 7 /\ GenerateLiveContextPreparationAttached(AuditSession, AuditChannel, AuditLease)
    \/ model_step_count = 8 /\ RegisterLivePlaybackOwnerAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, "owner_1", "ready_1", "pending_a")
    \/ model_step_count = 9 /\ RecordLiveWebrtcAnswerAcceptedAndBindExecutionAttached(AuditSession, AuditChannel, 1, AuditRuntime, 1, 1, 0, "activation_a")
    \* A voiced row committed while the summary is pending is queued behind
    \* it above the reserved cursor; queuing it starts the conversation on the
    \* channel, which releases the held late summary.
    \/ model_step_count = 10 /\ EnqueueLiveContextRowAttached(AuditChannel, AuditRuntime, 1, 1, "append_2", 2, "digest_1", "commit_1", "MirrorParentText", "Materializable", "Conversation", "User", None)
    \/ model_step_count = 11 /\ AuthorizeLiveContextBootstrapAppendAttached(AuditSession, AuditChannel, AuditLease, AuditBootstrap, AuditDigest, 1)
    \/ model_step_count = 12 /\ AdmitLiveInteractionDelegationAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE, TRUE)
    \/ model_step_count = 13 /\ ReconcileLiveDelegationTranscriptConfirmedAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE)
    \/ model_step_count = 14 /\ AuthorizeLiveDelegationWorkerStartAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, AuditWorker, "ExistingMember")
    \/ model_step_count = 15 /\ ResolveLiveDelegationWorkerStartAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, TRUE)
    \/ model_step_count = 16 /\ RecordLiveDelegationWorkerTerminalAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, "Completed")
    \/ model_step_count = 17 /\ AuthorizeLiveDelegationResultReleaseAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn)

AuditBody ==
    \/ RecordLiveContextBootstrapAckCutAttached(AuditSession, AuditChannel, AuditLease, AuditRuntime, 1, 1, AuditBootstrap, AuditDigest, 1)
    \* The only queued row is above the reserved cursor, so the exact
    \* complement is the whole outbox.
    \/ ResolveLiveContextBootstrapAppendAttached(AuditSession, AuditChannel, AuditLease, AuditBootstrap, AuditDigest, 1, "Delivered",
            live_context_queued_session_by_append, live_context_queued_cursor_by_append,
            live_context_queued_digest_by_append, live_context_queued_commit_token_by_append,
            live_context_queued_disposition_by_append, live_context_queued_append_by_cursor)
    \/ \E disposition \in LiveDelegationResultDispositionValues :
        AuthorizeLiveDelegationResultDeliveryAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, "result_1", disposition)
    \/ ObserveLiveContextDeliveryReadinessAttached(AuditSession, AuditChannel)

AuditNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditAcknowledged ==
    /\ AuditChannel \in DOMAIN live_context_preparation_phase_by_channel
    /\ live_context_preparation_phase_by_channel[AuditChannel] = "ProviderAcknowledged"

AuditResultAuthorized == AuditOperation \in DOMAIN live_result_delivery_digest_by_operation

\* Safety: a result delivery is authorized only once the summary is
\* acknowledged.
AuditResultFollowsSummary == AuditResultAuthorized => AuditAcknowledged

\* Goal: a result delivery is authorized while a replay is still queued.
AuditNeverDeliveredWithQueuedReplay ==
    ~(AuditResultAuthorized /\ DOMAIN live_context_queued_append_by_cursor # {})

\* Prefix reachability probe (run by hand): the prefix completes.
AuditNeverPrefixComplete == model_step_count < AuditPrefixLength
====
