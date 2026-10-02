---- MODULE live_unregister_cleanup_audit ----
\* Hand-written bounded audit of UnregisterSession against live channels
\* (#1476).
\*
\* UnregisterSession* is guarded on every live channel being closed first: the
\* close transitions stay the single authority that settles live obligations
\* (result deliveries, bridge operations, close custody). Unregister then
\* clears the session's terminal context-preparation records and its deferred
\* runtime stop.
\*
\* The generated MeerkatMachine model has roughly nine hundred actions, so its
\* ci profile stops after one step. This audit keeps the generated model's
\* variables, Init, actions and invariants unchanged. From a deterministic
\* prefix that leaves one channel of a registered session in the start state
\* AuditStart, it explores only that channel's context preparation, its close
\* transitions and the unregister drain.
\*
\* The start states, one TLC run each (live_unregister_cleanup_audit.sh):
\*   "admitted" - open admission accepted, nothing staged;
\*   "staged"   - execution mode resolved and execution staged;
\*   "bound"    - playback owner registered and the execution bound by an
\*                accepted WebRTC answer (Active);
\*   "running"  - staged, then a run started (phase Running);
\*   "retired"  - running, then retire requested during the run and the run
\*                ended into Retired, still holding the staged channel.
\*
\* Every explored state is checked against every generated invariant,
\* including live_channel_state_requires_registered_session, and against
\* AuditUnregisterNeverWhileBound (safety: the guard holds). Non-vacuity and
\* no-wedge are checked separately: from each start, the goal
\* AuditNeverUnregisters must be reported violated, proving that unregister
\* stays reachable through the close transitions. From "staged" the goal
\* AuditNeverUnregistersWithPreparation must also be reported violated:
\* unregister meets terminal context-preparation records.
EXTENDS model

\* Upper bound on model_step_count, including the deterministic prefix.
CONSTANT AuditMaxSteps
\* The channel's start state: "admitted", "staged", "bound", "running" or
\* "retired".
CONSTANT AuditStart

AuditSession == "sessionid_1"
AuditRuntime == "runtime_1"
AuditIdentity == "identity_1"
AuditChannel == "channel_a"

AuditPrefixLength ==
    CASE AuditStart = "admitted" -> 4
      [] AuditStart = "staged" -> 6
      [] AuditStart = "running" -> 7
      [] AuditStart = "retired" -> 9
      [] OTHER -> 8

AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None)
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ model_step_count = 3 /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditChannel, AuditIdentity)
    \/ model_step_count = 4 /\ ResolveLiveExecutionModeAdmissionAttached(AuditSession, AuditChannel, "profile_1", "FunctionBridge", TRUE, FALSE)
    \/ model_step_count = 5 /\ StageExperimentalLiveExecutionAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, 0, "pending_a")
    \* "bound": bind the staged execution.
    \/ AuditStart = "bound" /\ model_step_count = 6 /\ RegisterLivePlaybackOwnerAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, "owner_1", "ready_1", "pending_a")
    \/ AuditStart = "bound" /\ model_step_count = 7 /\ RecordLiveWebrtcAnswerAcceptedAndBindExecutionAttached(AuditSession, AuditChannel, 1, AuditRuntime, 1, 1, 0, "activation_a")
    \* "running" and "retired": start a run on the staged session.
    \/ AuditStart \in {"running", "retired"} /\ model_step_count = 6 /\ StartImmediateAppendAttached("run_1")
    \* "retired": retire during the run; the run ends into Retired.
    \/ AuditStart = "retired" /\ model_step_count = 7 /\ RetireRequestedWhileRunBound(AuditSession)
    \/ AuditStart = "retired" /\ model_step_count = 8 /\
        \/ CommitRunningToRetired("input_1", "run_1")
        \/ FailRunningToRetired("run_1")
        \/ CancelRunningToRetired("run_1")
        \/ RollbackRunRunningToRetired("run_1")
        \/ ServiceTurnCommittedRunningToRetired("run_1")

AuditBody ==
    \* Context preparation on the channel, so unregister meets its terminal
    \* records.
    \/ BeginLiveContextPreparationAttached(AuditSession, AuditChannel, "lease_1", 0, AuditRuntime, 1, 1)
    \/ GenerateLiveContextPreparationAttached(AuditSession, AuditChannel, "lease_1")
    \/ BeginLiveContextPreparationRunning(AuditSession, AuditChannel, "lease_1", 0, AuditRuntime, 1, 1)
    \/ GenerateLiveContextPreparationRunning(AuditSession, AuditChannel, "lease_1")
    \* The channel's close paths, in each phase the session can be in. Close
    \* custody is revoked by exactly one receipt: the staged pending receipt
    \* or the bound activation receipt.
    \/ RecordLiveCloseClosedAttached(AuditSession, AuditChannel, 1)
    \/ RecordLiveCloseClosedRunning(AuditSession, AuditChannel, 1)
    \/ RecordLiveCloseClosedRetired(AuditSession, AuditChannel, 1)
    \/ AbandonLiveOpenAdmissionAttached(AuditSession, AuditChannel)
    \/ AbandonLiveOpenAdmissionRunning(AuditSession, AuditChannel)
    \/ AbandonLiveOpenAdmissionRetired(AuditSession, AuditChannel)
    \/ \E receipt \in {<<Some("pending_a"), None>>, <<None, Some("activation_a")>>} :
        \/ RevokeLiveChannelCloseCustodyAttached(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyRunning(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyClosedReplayAttached(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyClosedReplayRunning(AuditSession, AuditChannel, receipt[1], receipt[2])
        \/ RevokeLiveChannelCloseCustodyClosedReplayRetired(AuditSession, AuditChannel, receipt[1], receipt[2])
    \/ DeferLiveCloseSettlementAttached(AuditSession, AuditChannel)
    \/ DeferLiveCloseSettlementRunning(AuditSession, AuditChannel)
    \/ DeferLiveCloseSettlementRetired(AuditSession, AuditChannel)
    \* The unregister drain, in each phase.
    \/ BeginUnregisterSessionAttached(AuditSession, Some(AuditRuntime), Some(1), Some(1), None)
    \/ BeginUnregisterSessionRunning(AuditSession, Some(AuditRuntime), Some(1), Some(1), None)
    \/ BeginUnregisterSessionRetainsSnapshotRetired(AuditSession, Some(AuditRuntime), Some(1), Some(1), None)
    \/ \E forced \in BOOLEAN :
        \/ RuntimeLoopStoppedForUnregisterAttached(AuditSession, forced)
        \/ RuntimeLoopStoppedForUnregisterRunning(AuditSession, forced)
        \/ RuntimeLoopStoppedForUnregisterRetired(AuditSession, forced)
        \/ CommsDrainExitedForUnregisterAttached(AuditSession, forced)
        \/ CommsDrainExitedForUnregisterRunning(AuditSession, forced)
        \/ CommsDrainExitedForUnregisterRetired(AuditSession, forced)
    \/ CompletionWaitersResolvedForUnregisterAttached(AuditSession)
    \/ CompletionWaitersResolvedForUnregisterRunning(AuditSession)
    \/ CompletionWaitersResolvedForUnregisterRetired(AuditSession)
    \/ UnregisterSessionAttached(AuditSession, Some(AuditRuntime), Some(1), Some(1), None)
    \/ UnregisterSessionRunning(AuditSession, Some(AuditRuntime), Some(1), Some(1), None)
    \/ UnregisterSessionRetired(AuditSession, Some(AuditRuntime), Some(1), Some(1), None)

AuditNext ==
    \/ (model_step_count < AuditPrefixLength /\ AuditPrefix)
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditUnregisters == session_id # None /\ session_id' = None

\* Safety: unregister never fires while a live channel is still bound.
AuditUnregisterNeverWhileBound ==
    [][AuditUnregisters => (DOMAIN live_channel_session_by_channel = {}
                            /\ DOMAIN live_active_channel_by_session = {})]_vars

\* Goal (no wedge): unregister completes from the start state.
AuditNeverUnregisters == [][~AuditUnregisters]_vars

\* Goal (staged start): unregister meets terminal context-preparation records,
\* so their reset is exercised rather than vacuous.
AuditNeverUnregistersWithPreparation ==
    [][~(AuditUnregisters /\ DOMAIN live_context_preparation_phase_by_channel # {})]_vars
====
