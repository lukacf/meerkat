---- MODULE live_unregister_cleanup_audit ----
\* Hand-written bounded audit of UnregisterSession against live channels
\* (#1476).
\*
\* UnregisterSession* is guarded on no live channel being open: nothing bound
\* or in flight, the close residue (receipts, execution mode/profile,
\* capability sets) present only for channels whose close is recorded, and
\* every forward recovery obligation settled. The close transitions stay the
\* single authority that settles live obligations. Unregister then clears the
\* close residue, the terminal context-preparation records and the deferred
\* runtime stop.
\*
\* The generated MeerkatMachine model has roughly nine hundred actions, so its
\* ci profile stops after one step. This audit keeps the generated model's
\* variables, Init, actions and invariants unchanged. From a deterministic
\* prefix that leaves one channel of a registered session in the start state
\* AuditStart, it explores that channel's context preparation, its close and
\* recovery transitions and the unregister drain in every phase.
\*
\* Start states, one TLC run each (live_unregister_cleanup_audit.sh):
\*   "admitted"  - open admission accepted, nothing staged;
\*   "staged"    - execution mode resolved and execution staged;
\*   "bound"     - staged, playback owner registered and execution bound by
\*                 an accepted WebRTC answer (Active);
\*   "running"   - staged, then a run started (phase Running);
\*   "retired"   - running, then retire requested during the run and the run
\*                 ended into Retired, still holding the staged channel.
\* Close-first starts, each forcing the production close order and banning
\* AbandonLiveOpenAdmission (the close leaves its tombstone residue):
\*   "closing-idle"      - admitted in Idle (no receipt to revoke), close
\*                         recorded;
\*   "closing-attached"  - staged, close custody revoked, close recorded;
\*   "closing-running"   - staged, run started, custody revoked, close
\*                         recorded in Running;
\*   "closing-retired"   - staged, run, retire during the run, run ended into
\*                         Retired, close recorded (no revoke in Retired);
\*   "closing-stopped"   - staged, runtime stopped and exited, close recorded
\*                         (no revoke in Stopped);
\*   "closing-retired-recovery", "closing-stopped-recovery" - as the two
\*                         above, after the bound channel's context append was
\*                         resolved ambiguous, so the closed channel still owes
\*                         a forward recovery to its replacement.
\*
\* Every explored state is checked against every generated invariant,
\* including live_channel_state_requires_registered_session, and against
\* AuditUnregisterNeverWhileBound. From every start the goal
\* AuditNeverUnregisters must be reported violated (unregister reachable).
\* From every close-first start the script additionally checks the per-state
\* no-wedge property on the dumped state graph: every reachable state can
\* still reach a completed unregister.
EXTENDS model

\* Upper bound on the steps after the deterministic prefix's first six.
CONSTANT AuditMaxSteps
\* The channel's start state (see above).
CONSTANT AuditStart

AuditSession == "sessionid_1"
AuditRuntime == "runtime_1"
AuditIdentity == "identity_1"
AuditChannel == "channel_a"
AuditReplacement == "channel_b"

AuditClosingStarts ==
    {"closing-idle", "closing-attached", "closing-running", "closing-retired",
     "closing-stopped", "closing-retired-recovery", "closing-stopped-recovery"}
AuditRecoveryStarts == {"closing-retired-recovery", "closing-stopped-recovery"}

AuditPrefixLength ==
    CASE AuditStart = "admitted" -> 4
      [] AuditStart = "staged" -> 6
      [] AuditStart = "bound" -> 8
      [] AuditStart = "running" -> 7
      [] AuditStart = "retired" -> 9
      [] AuditStart = "closing-idle" -> 4
      [] AuditStart = "closing-attached" -> 8
      [] AuditStart = "closing-running" -> 9
      [] AuditStart = "closing-retired" -> 10
      [] AuditStart = "closing-stopped" -> 9
      [] AuditStart = "closing-retired-recovery" -> 15
      [] AuditStart = "closing-stopped-recovery" -> 14

\* A run ending into Retired (any of its terminal transitions).
AuditRunEndsIntoRetired ==
    \/ CommitRunningToRetired("input_1", "run_1")
    \/ FailRunningToRetired("run_1")
    \/ CancelRunningToRetired("run_1")
    \/ RollbackRunRunningToRetired("run_1")
    \/ ServiceTurnCommittedRunningToRetired("run_1")

AuditStep(n, starts) == model_step_count = n /\ AuditStart \in starts

AuditStaging == (AuditClosingStarts \ {"closing-idle"}) \cup {"admitted", "staged", "bound", "running", "retired"}
AuditBoundStarts == {"bound"} \cup AuditRecoveryStarts

AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None)
    \* closing-idle: admitted without a runtime binding, close recorded.
    \/ AuditStep(2, {"closing-idle"}) /\ ResolveLiveOpenAdmissionAcceptedIdle(AuditSession, AuditChannel, AuditIdentity)
    \/ AuditStep(3, {"closing-idle"}) /\ RecordLiveCloseClosedIdle(AuditSession, AuditChannel, 1)
    \* Everything else: bind the runtime, admit, and (except "admitted") stage.
    \/ AuditStep(2, AuditStaging) /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ AuditStep(3, AuditStaging) /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditChannel, AuditIdentity)
    \/ AuditStep(4, AuditStaging \ {"admitted"}) /\ ResolveLiveExecutionModeAdmissionAttached(AuditSession, AuditChannel, "profile_1", "FunctionBridge", TRUE, FALSE)
    \/ AuditStep(5, AuditStaging \ {"admitted"}) /\ StageExperimentalLiveExecutionAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, 0, "pending_a")
    \* Bind the staged execution.
    \/ AuditStep(6, AuditBoundStarts) /\ RegisterLivePlaybackOwnerAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, "owner_1", "ready_1", "pending_a")
    \/ AuditStep(7, AuditBoundStarts) /\ RecordLiveWebrtcAnswerAcceptedAndBindExecutionAttached(AuditSession, AuditChannel, 1, AuditRuntime, 1, 1, 0, "activation_a")
    \* Recovery starts: a context append resolved ambiguous owes a forward
    \* recovery to the replacement channel.
    \/ AuditStep(8, AuditRecoveryStarts) /\ EnqueueLiveContextRowAttached(AuditChannel, AuditRuntime, 1, 1, "append_1", 1, "digest_1", "commit_1", "MirrorParentText", "Materializable", "Conversation", "User", None)
    \/ AuditStep(9, AuditRecoveryStarts) /\ AuthorizeLiveContextAppendAttached(AuditChannel, AuditRuntime, 1, 1, "append_1", 0, 1)
    \/ AuditStep(10, AuditRecoveryStarts) /\ ResolveLiveContextAppendAmbiguousAttached(AuditChannel, AuditRuntime, 1, 1, "append_1", 0, 1, AuditReplacement, 1, "Ambiguous")
    \* Close-first in Attached: revoke custody, then record the close.
    \/ AuditStep(6, {"closing-attached"}) /\ RevokeLiveChannelCloseCustodyAttached(AuditSession, AuditChannel, Some("pending_a"), None)
    \/ AuditStep(7, {"closing-attached"}) /\ RecordLiveCloseClosedAttached(AuditSession, AuditChannel, 1)
    \* Runs (running, retired and their close-first forms).
    \/ AuditStep(6, {"running", "retired", "closing-running", "closing-retired"}) /\ StartImmediateAppendAttached("run_1")
    \/ AuditStep(11, {"closing-retired-recovery"}) /\ StartImmediateAppendAttached("run_1")
    \/ AuditStep(7, {"closing-running"}) /\ RevokeLiveChannelCloseCustodyRunning(AuditSession, AuditChannel, Some("pending_a"), None)
    \/ AuditStep(8, {"closing-running"}) /\ RecordLiveCloseClosedRunning(AuditSession, AuditChannel, 1)
    \/ AuditStep(7, {"retired", "closing-retired"}) /\ RetireRequestedWhileRunBound(AuditSession)
    \/ AuditStep(12, {"closing-retired-recovery"}) /\ RetireRequestedWhileRunBound(AuditSession)
    \/ AuditStep(8, {"retired", "closing-retired"}) /\ AuditRunEndsIntoRetired
    \/ AuditStep(13, {"closing-retired-recovery"}) /\ AuditRunEndsIntoRetired
    \/ AuditStep(9, {"closing-retired"}) /\ RecordLiveCloseClosedRetired(AuditSession, AuditChannel, 1)
    \/ AuditStep(14, {"closing-retired-recovery"}) /\ RecordLiveCloseClosedRetired(AuditSession, AuditChannel, 1)
    \* Stopped: the runtime executor stops and exits, then the close.
    \/ AuditStep(6, {"closing-stopped"}) /\ StopRuntimeExecutorAttached("stopped")
    \/ AuditStep(11, {"closing-stopped-recovery"}) /\ StopRuntimeExecutorAttached("stopped")
    \/ AuditStep(7, {"closing-stopped"}) /\ RuntimeExecutorExitedFromAttached
    \/ AuditStep(12, {"closing-stopped-recovery"}) /\ RuntimeExecutorExitedFromAttached
    \/ AuditStep(8, {"closing-stopped"}) /\ RecordLiveCloseClosedStopped(AuditSession, AuditChannel, 1)
    \/ AuditStep(13, {"closing-stopped-recovery"}) /\ RecordLiveCloseClosedStopped(AuditSession, AuditChannel, 1)

\* The exact outbox complement a recovery cancellation keeps (as in
\* live_context_outbox_audit): a row stays while another live obligation or
\* the active channel's bound cursor (else staged seed) is still owed it.
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

AuditCancelArgs(Cancel(_, _, _, _, _, _, _, _, _)) ==
    Cancel(AuditSession, AuditChannel, AuditReplacement,
        AuditRetain(live_context_queued_session_by_append, AuditReplacement),
        AuditRetain(live_context_queued_cursor_by_append, AuditReplacement),
        AuditRetain(live_context_queued_digest_by_append, AuditReplacement),
        AuditRetain(live_context_queued_commit_token_by_append, AuditReplacement),
        AuditRetain(live_context_queued_disposition_by_append, AuditReplacement),
        AuditRetainByCursor(AuditReplacement))

\* The runtime binding the unregister inputs name: none for closing-idle.
AuditBindings ==
    IF AuditStart = "closing-idle"
    THEN {<<None, None, None>>}
    ELSE {<<Some(AuditRuntime), Some(1), Some(1)>>}

AuditBody ==
    \* Context preparation on the channel, so unregister meets its terminal
    \* records.
    \/ BeginLiveContextPreparationAttached(AuditSession, AuditChannel, "lease_1", 0, AuditRuntime, 1, 1)
    \/ GenerateLiveContextPreparationAttached(AuditSession, AuditChannel, "lease_1")
    \/ BeginLiveContextPreparationRunning(AuditSession, AuditChannel, "lease_1", 0, AuditRuntime, 1, 1)
    \/ GenerateLiveContextPreparationRunning(AuditSession, AuditChannel, "lease_1")
    \* The channel's close paths, in each phase. Close custody is revoked by
    \* exactly one receipt. The close-first starts ban abandoned admission.
    \/ RecordLiveCloseClosedIdle(AuditSession, AuditChannel, 1)
    \/ RecordLiveCloseClosedAttached(AuditSession, AuditChannel, 1)
    \/ RecordLiveCloseClosedRunning(AuditSession, AuditChannel, 1)
    \/ RecordLiveCloseClosedRetired(AuditSession, AuditChannel, 1)
    \/ RecordLiveCloseClosedStopped(AuditSession, AuditChannel, 1)
    \/ /\ AuditStart \notin AuditClosingStarts
       /\ \/ AbandonLiveOpenAdmissionAttached(AuditSession, AuditChannel)
          \/ AbandonLiveOpenAdmissionRunning(AuditSession, AuditChannel)
          \/ AbandonLiveOpenAdmissionRetired(AuditSession, AuditChannel)
    \/ \E receipt \in {<<Some("pending_a"), None>>, <<None, Some("activation_a")>>} :
        \/ RevokeLiveChannelCloseCustodyAttached(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyRunning(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyClosedReplayIdle(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyClosedReplayAttached(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyClosedReplayRunning(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyClosedReplayRetired(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyClosedReplayStopped(AuditSession, AuditChannel, receipt[1], receipt[2])
    \/ DeferLiveCloseSettlementAttached(AuditSession, AuditChannel)
    \/ DeferLiveCloseSettlementRunning(AuditSession, AuditChannel)
    \/ DeferLiveCloseSettlementRetired(AuditSession, AuditChannel)
    \* A closed channel's forward recovery, settled by cancellation.
    \/ /\ AuditStart \in AuditRecoveryStarts
       /\ \/ AuditCancelArgs(CancelLiveContextRecoveryObligationIdle)
          \/ AuditCancelArgs(CancelLiveContextRecoveryObligationAttached)
          \/ AuditCancelArgs(CancelLiveContextRecoveryObligationRunning)
          \/ AuditCancelArgs(CancelLiveContextRecoveryObligationRetired)
          \/ AuditCancelArgs(CancelLiveContextRecoveryObligationStopped)
    \* The unregister drain, in each phase.
    \/ \E b \in AuditBindings :
        \/ BeginUnregisterSessionIdle(AuditSession, b[1], b[2], b[3], None)
        \/ BeginUnregisterSessionAttached(AuditSession, b[1], b[2], b[3], None)
        \/ BeginUnregisterSessionRunning(AuditSession, b[1], b[2], b[3], None)
        \/ BeginUnregisterSessionRetainsSnapshotRetired(AuditSession, b[1], b[2], b[3], None)
        \/ BeginUnregisterSessionRetainsSnapshotStopped(AuditSession, b[1], b[2], b[3], None)
        \/ UnregisterSessionIdle(AuditSession, b[1], b[2], b[3], None)
        \/ UnregisterSessionAttached(AuditSession, b[1], b[2], b[3], None)
        \/ UnregisterSessionRunning(AuditSession, b[1], b[2], b[3], None)
        \/ UnregisterSessionRetired(AuditSession, b[1], b[2], b[3], None)
        \/ UnregisterSessionStopped(AuditSession, b[1], b[2], b[3], None)
    \/ \E forced \in BOOLEAN :
        \/ RuntimeLoopStoppedForUnregisterIdle(AuditSession, forced)
        \/ RuntimeLoopStoppedForUnregisterAttached(AuditSession, forced)
        \/ RuntimeLoopStoppedForUnregisterRunning(AuditSession, forced)
        \/ RuntimeLoopStoppedForUnregisterRetired(AuditSession, forced)
        \/ RuntimeLoopStoppedForUnregisterStopped(AuditSession, forced)
        \/ CommsDrainExitedForUnregisterIdle(AuditSession, forced)
        \/ CommsDrainExitedForUnregisterAttached(AuditSession, forced)
        \/ CommsDrainExitedForUnregisterRunning(AuditSession, forced)
        \/ CommsDrainExitedForUnregisterRetired(AuditSession, forced)
        \/ CommsDrainExitedForUnregisterStopped(AuditSession, forced)
    \/ CompletionWaitersResolvedForUnregisterIdle(AuditSession)
    \/ CompletionWaitersResolvedForUnregisterAttached(AuditSession)
    \/ CompletionWaitersResolvedForUnregisterRunning(AuditSession)
    \/ CompletionWaitersResolvedForUnregisterRetired(AuditSession)
    \/ CompletionWaitersResolvedForUnregisterStopped(AuditSession)

AuditNext ==
    \/ (model_step_count < AuditPrefixLength /\ AuditPrefix)
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec == Init /\ [][AuditNext]_vars

\* The close-first starts get AuditMaxSteps - 2 steps after their prefix (the
\* per-state no-wedge check needs that depth plus its finishing margin); the
\* other starts keep the absolute bound.
AuditStateConstraint ==
    IF AuditStart \in AuditClosingStarts
    THEN model_step_count <= AuditPrefixLength + AuditMaxSteps - 2
    ELSE model_step_count <= AuditMaxSteps

AuditUnregisters == session_id # None /\ session_id' = None

\* Safety: unregister never fires while a live channel is still bound.
AuditUnregisterNeverWhileBound ==
    [][AuditUnregisters => (DOMAIN live_channel_session_by_channel = {}
                            /\ DOMAIN live_active_channel_by_session = {})]_vars

\* Goal (no wedge, existential): unregister completes from the start state.
AuditNeverUnregisters == [][~AuditUnregisters]_vars

\* Goal (staged start): unregister meets terminal context-preparation records,
\* so their reset is exercised rather than vacuous.
AuditNeverUnregistersWithPreparation ==
    [][~(AuditUnregisters /\ DOMAIN live_context_preparation_phase_by_channel # {})]_vars

\* Goal (recovery starts): the closed channel's forward recovery is settled.
AuditNeverCancelsRecovery ==
    ~(AuditReplacement \in live_cancelled_recovery_channels)
====
