---- MODULE live_context_outbox_audit ----
\* Hand-written bounded audit of the live-context outbox across channel close
\* and ambiguity recovery.
\*
\* The generated MeerkatMachine model has roughly nine hundred actions, so its
\* ci profile stops after one step and its deep profile does not finish on any
\* local or CI budget. This audit keeps the generated model's variables, Init,
\* actions and invariants unchanged and explores only the actions that move one
\* session's outbox across two channels: row enqueue, staging and seed advance,
\* playback readiness and bind, append authorization and resolution (delivered
\* and ambiguous), close, abandoned admission, and a second channel opened as a
\* plain reopen or as the ambiguity recovery replacement.
\*
\* Every explored state is checked against every generated invariant,
\* including live_context_outbox_has_no_closed_channel_leftover and
\* live_context_outbox_is_above_every_seed. Non-vacuity is checked separately:
\* live_context_outbox_audit.sh runs each goal below as the only property and
\* requires TLC to report it violated, proving the bound reaches
\*   - a close that ends an outbox still holding a queued row,
\*   - a recovery authorization that ends the rows its replacement's seed
\*     carries, and
\*   - a row queued after the authorization reaching the replacement.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* live_context_outbox_audit.sh, so a DSL change can never leave the audit on
\* a stale constant block or a narrower invariant set. Run a deeper bound by
\* hand with:
\*   specs/machines/meerkat_machine/live_context_outbox_audit.sh 22
EXTENDS model

\* Upper bound on model_step_count, including the deterministic prefix.
CONSTANT AuditMaxSteps

AuditSession == "sessionid_1"
AuditRuntime == "runtime_1"
AuditIdentity == "identity_1"
AuditFirst == "channel_a"
AuditSecond == "channel_b"
AuditChannels == {AuditFirst, AuditSecond}
AuditCursors == 1..2
AuditSeeds == 0..2
\* One append identity per canonical cursor keeps the row set small.
AuditAppend(cursor) == IF cursor = 1 THEN "append_1" ELSE "append_2"
AuditPending(channel) == IF channel = AuditFirst THEN "pending_a" ELSE "pending_b"
AuditActivation(channel) == IF channel = AuditFirst THEN "activation_a" ELSE "activation_b"
AuditPrefixLength == 6

\* A single deterministic prefix reaches a registered, runtime-bound session
\* whose first channel is admitted and staged at seed 0.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None)
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ model_step_count = 3 /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditFirst, AuditIdentity)
    \/ model_step_count = 4 /\ ResolveLiveExecutionModeAdmissionAttached(AuditSession, AuditFirst, "profile_1", "FunctionBridge", TRUE, FALSE)
    \/ model_step_count = 5 /\ StageExperimentalLiveExecutionAttached(AuditSession, AuditFirst, AuditRuntime, 1, 1, 0, AuditPending(AuditFirst))

AuditSecondChannelOpen ==
    \/ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditSecond, AuditIdentity)
    \/ ResolveLiveExecutionModeAdmissionAttached(AuditSession, AuditSecond, "profile_1", "FunctionBridge", TRUE, FALSE)
    \/ \E seed \in AuditSeeds :
        StageExperimentalLiveExecutionAttached(AuditSession, AuditSecond, AuditRuntime, 1, 1, seed, AuditPending(AuditSecond))

AuditStageAndBind ==
    \E channel \in AuditChannels :
        \/ RegisterLivePlaybackOwnerAttached(AuditSession, channel, AuditRuntime, 1, 1, "owner_1", "ready_1", AuditPending(channel))
        \/ \E seed \in AuditSeeds :
            RecordLiveWebrtcAnswerAcceptedAndBindExecutionAttached(AuditSession, channel, 1, AuditRuntime, 1, 1, seed, AuditActivation(channel))
        \/ \E previous \in 0..1 :
            AdvanceLiveExperimentalStagedSeedAttached(AuditSession, channel, AuditRuntime, 1, 1, previous, previous + 1)

AuditOutbox ==
    \E channel \in AuditChannels : \E cursor \in AuditCursors :
        \/ EnqueueLiveContextRowAttached(channel, AuditRuntime, 1, 1, AuditAppend(cursor), cursor, "digest_1", "commit_1", "MirrorParentText", "Materializable", "Conversation", "User", None)
        \/ AuthorizeLiveContextAppendAttached(channel, AuditRuntime, 1, 1, AuditAppend(cursor), cursor - 1, cursor)
        \/ ResolveLiveContextAppendDeliveredAttached(channel, AuditRuntime, 1, 1, AuditAppend(cursor), cursor - 1, cursor, "", 0, "Delivered")

AuditRecovery ==
    \E cursor \in AuditCursors : \E seed \in AuditSeeds :
        \/ ResolveLiveContextAppendAmbiguousAttached(AuditFirst, AuditRuntime, 1, 1, AuditAppend(cursor), cursor - 1, cursor, AuditSecond, seed, "Ambiguous")
        \/ BindLiveContextRecoveryChannelAttached(AuditSession, AuditFirst, AuditSecond, 1, AuditRuntime, 1, 1, AuditAppend(cursor), seed, AuditActivation(AuditSecond))

AuditUnbind ==
    \E channel \in AuditChannels :
        \/ RecordLiveCloseClosedAttached(AuditSession, channel, 1)
        \/ AbandonLiveOpenAdmissionAttached(AuditSession, channel)

AuditBody ==
    \/ AuditSecondChannelOpen
    \/ AuditStageAndBind
    \/ AuditOutbox
    \/ AuditRecovery
    \/ AuditUnbind

AuditNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditQueued == DOMAIN live_context_queued_append_by_cursor

\* Goal 1: unbinding a channel ends an outbox that still holds a queued row.
AuditCloseEndsALeftover ==
    /\ AuditQueued # {}
    /\ \E channel \in AuditChannels :
        /\ channel \in DOMAIN live_channel_session_by_channel
        /\ channel \notin DOMAIN live_channel_session_by_channel'
    /\ DOMAIN live_context_queued_append_by_cursor' = {}

\* Goal 2: a recovery authorization ends the queued rows its seed carries.
AuditRecoveryEndsCarriedRows ==
    /\ AuditQueued # {}
    /\ AuditFirst \notin DOMAIN live_context_recovery_replacement_by_channel
    /\ AuditFirst \in DOMAIN live_context_recovery_replacement_by_channel'
    /\ DOMAIN live_context_queued_append_by_cursor' = {}

\* Goal 3: a row queued after the authorization reaches the bound replacement.
AuditReplacementReceivesALaterRow ==
    /\ "append_1" \in live_context_ambiguous_no_retry
    /\ AuditSecond \in DOMAIN live_context_pending_append_by_channel
    /\ live_context_pending_append_by_channel[AuditSecond] = "append_2"

AuditNeverCloseEndsALeftover == [][~AuditCloseEndsALeftover]_vars
AuditNeverRecoveryEndsCarriedRows == [][~AuditRecoveryEndsCarriedRows]_vars
AuditReplacementNeverReceivesALaterRow == ~AuditReplacementReceivesALaterRow
====
