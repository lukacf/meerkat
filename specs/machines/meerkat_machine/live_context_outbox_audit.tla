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
\*   - a row queued after the authorization reaching the replacement, and
\*   - a plain reopen binding after a recovery whose replacement was never
\*     realized was cancelled, with no live obligation left.
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
\* A plain reopen, admitted after a recovery whose replacement was never
\* realized has been cancelled.
AuditThird == "channel_c"
AuditChannels == {AuditFirst, AuditSecond, AuditThird}
AuditCursors == 1..2
AuditSeeds == 0..2
\* One append identity per canonical cursor keeps the row set small.
AuditAppend(cursor) == IF cursor = 1 THEN "append_1" ELSE "append_2"
AuditPending(channel) ==
    CASE channel = AuditFirst -> "pending_a"
      [] channel = AuditSecond -> "pending_b"
      [] OTHER -> "pending_c"
AuditActivation(channel) ==
    CASE channel = AuditFirst -> "activation_a"
      [] channel = AuditSecond -> "activation_b"
      [] OTHER -> "activation_c"
AuditPrefixLength == 6

\* A single deterministic prefix reaches a registered, runtime-bound session
\* whose first channel is admitted and staged at seed 0.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ model_step_count = 3 /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditFirst, AuditIdentity, None)
    \/ model_step_count = 4 /\ ResolveLiveExecutionModeAdmissionAttached(AuditSession, AuditFirst, "profile_1", "FunctionBridge", TRUE, FALSE)
    \/ model_step_count = 5 /\ StageExperimentalLiveExecutionAttached(AuditSession, AuditFirst, AuditRuntime, 1, 1, 0, AuditPending(AuditFirst))

AuditLaterChannelOpen ==
    \E channel \in {AuditSecond, AuditThird} :
        \/ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, channel, AuditIdentity, None)
        \/ ResolveLiveExecutionModeAdmissionAttached(AuditSession, channel, "profile_1", "FunctionBridge", TRUE, FALSE)
        \/ \E seed \in AuditSeeds :
            StageExperimentalLiveExecutionAttached(AuditSession, channel, AuditRuntime, 1, 1, seed, AuditPending(channel))

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

\* The exact outbox complement a cancellation keeps, computed as the runtime
\* does: a row stays while another live obligation or the active channel's
\* bound cursor (else staged seed) is still owed it. The generated guard
\* checks exactness, so a wrong complement only disables the action and the
\* goal below then fails.
AuditOtherLiveObligation(replacement) ==
    \/ \E source \in DOMAIN live_context_recovery_replacement_by_channel :
        /\ live_context_recovery_replacement_by_channel[source] \notin live_cancelled_recovery_channels
        /\ live_context_recovery_replacement_by_channel[source] # replacement
    \/ \E source \in DOMAIN live_result_recovery_replacement_by_channel :
        /\ live_result_recovery_replacement_by_channel[source] \notin live_cancelled_recovery_channels
        /\ live_result_recovery_replacement_by_channel[source] # replacement
AuditActiveFloorHolds(cursor) ==
    /\ AuditSession \in DOMAIN live_active_channel_by_session
    /\ LET active == live_active_channel_by_session[AuditSession] IN
        IF active \in DOMAIN live_context_cursor_by_channel
        THEN cursor > live_context_cursor_by_channel[active]
        ELSE /\ active \in DOMAIN live_experimental_staged_seed_cursor_by_channel
             /\ cursor > live_experimental_staged_seed_cursor_by_channel[active]
AuditKeep(replacement, append) ==
    \/ AuditOtherLiveObligation(replacement)
    \/ AuditActiveFloorHolds(live_context_queued_cursor_by_append[append])
AuditKept(replacement) ==
    {append \in DOMAIN live_context_queued_cursor_by_append : AuditKeep(replacement, append)}
AuditRetain(map, replacement) == [append \in AuditKept(replacement) |-> map[append]]
AuditRetainByCursor(replacement) ==
    [cursor \in {c \in DOMAIN live_context_queued_append_by_cursor :
                    live_context_queued_append_by_cursor[c] \in AuditKept(replacement)}
        |-> live_context_queued_append_by_cursor[cursor]]

AuditCancelUnrealized ==
    CancelLiveContextRecoveryObligationAttached(AuditSession, AuditFirst, AuditSecond,
        AuditRetain(live_context_queued_session_by_append, AuditSecond),
        AuditRetain(live_context_queued_cursor_by_append, AuditSecond),
        AuditRetain(live_context_queued_digest_by_append, AuditSecond),
        AuditRetain(live_context_queued_commit_token_by_append, AuditSecond),
        AuditRetain(live_context_queued_disposition_by_append, AuditSecond),
        AuditRetainByCursor(AuditSecond))

AuditUnbind ==
    \E channel \in AuditChannels :
        \/ RecordLiveCloseClosedAttached(AuditSession, channel, 1)
        \/ AbandonLiveOpenAdmissionAttached(AuditSession, channel)

AuditBody ==
    \/ AuditLaterChannelOpen
    \/ AuditCancelUnrealized
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

\* Goal 4: after a recovery whose replacement was never realized, a plain
\* reopen binds with no live obligation left in the session.
AuditReopenAfterFailedRealization ==
    /\ AuditSecond \in live_cancelled_recovery_channels
    /\ AuditSecond \notin DOMAIN live_execution_runtime_id_by_channel
    /\ AuditThird \in DOMAIN live_context_cursor_by_channel
    /\ ~AuditOtherLiveObligation("")

AuditNeverCloseEndsALeftover == [][~AuditCloseEndsALeftover]_vars
AuditNeverRecoveryEndsCarriedRows == [][~AuditRecoveryEndsCarriedRows]_vars
AuditReplacementNeverReceivesALaterRow == ~AuditReplacementReceivesALaterRow
AuditNeverReopensAfterFailedRealization == ~AuditReopenAfterFailedRealization
====
