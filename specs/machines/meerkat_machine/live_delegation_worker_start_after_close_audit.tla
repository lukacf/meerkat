---- MODULE live_delegation_worker_start_after_close_audit ----
\* Bounded audit (tlc-gate review of fix/live-durable-worker-start-after-close)
\* of a durable delegation worker start that resolves after its channel closed.
\*
\* ResolveLiveDelegationWorkerStart resolves a StartAuthorized worker either
\* under the channel's exact runtime/fence/generation binding, or, once the
\* operation's OWN channel carries no binding at all (close unbinds it), on the
\* exact worker authority alone (Turbo S S104 R7). This audit keeps the
\* generated MeerkatMachine model unchanged. A deterministic prefix reaches one
\* experimental channel with a delegation whose worker start is authorized. The
\* body explores the channel's close and close settlement, worker-start
\* resolution (started or not) through the operation's channel and through a
\* foreign never-bound channel, and the worker's terminal record. Every explored
\* state is checked against every generated invariant (including the
\* live_delegation schedule/worker-count and terminal-is-worker-bound
\* invariants) plus:
\*   AuditForeignChannelNeverResolves: no worker-start resolution of the
\*     operation is enabled through a channel other than its own.
\* Goals (each required violated):
\*   - the worker start resolves after the channel closed;
\*   - that worker then settles through the revoked-worker reconciliation
\*     (its channel is unbound, so the bound terminal arm no longer applies).
EXTENDS model

CONSTANT AuditMaxSteps

AuditSession == "sessionid_1"
AuditRuntime == "runtime_1"
AuditIdentity == "identity_1"
AuditChannel == "channel_a"
AuditForeignChannel == "channel_b"
AuditLease == "lease_1"
AuditDigest == "digest_s"
AuditBootstrap == "append_boot"
AuditInteraction == "interaction_1"
AuditOperation == "operation_1"
AuditTurn == "turn_1"
AuditWorker == "worker_1"
AuditPrefixLength == 15

VARIABLES
    audit_closed,                \* the channel's close was recorded
    audit_resolved_after_close,  \* the worker start resolved after that close
    audit_settled_after_close    \* that worker then settled by revoked-worker reconciliation

auditVars == <<audit_closed, audit_resolved_after_close, audit_settled_after_close>>

\* A deterministic prefix reaches a bound experimental channel with an
\* authorized durable worker start.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ model_step_count = 3 /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditChannel, AuditIdentity, None)
    \/ model_step_count = 4 /\ ResolveLiveExecutionModeAdmissionAttached(AuditSession, AuditChannel, "profile_1", "ClientContext", FALSE, TRUE)
    \/ model_step_count = 5 /\ StageExperimentalLiveExecutionAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, 0, "pending_a")
    \/ model_step_count = 6 /\ BeginLiveContextPreparationAttached(AuditSession, AuditChannel, AuditLease, 1, AuditRuntime, 1, 1)
    \/ model_step_count = 7 /\ GenerateLiveContextPreparationAttached(AuditSession, AuditChannel, AuditLease)
    \/ model_step_count = 8 /\ RegisterLivePlaybackOwnerAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, "owner_1", "ready_1", "pending_a")
    \/ model_step_count = 9 /\ RecordLiveWebrtcAnswerAcceptedAndBindExecutionAttached(AuditSession, AuditChannel, 1, AuditRuntime, 1, 1, 0, "activation_a")
    \/ model_step_count = 10 /\ EnqueueLiveContextRowAttached(AuditChannel, AuditRuntime, 1, 1, "append_2", 2, "digest_1", "commit_1", "MirrorParentText", "Materializable", "Conversation", "User", None)
    \/ model_step_count = 11 /\ AuthorizeLiveContextBootstrapAppendAttached(AuditSession, AuditChannel, AuditLease, AuditBootstrap, AuditDigest, 1)
    \/ model_step_count = 12 /\ AdmitLiveInteractionDelegationAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE, TRUE)
    \/ model_step_count = 13 /\ ReconcileLiveDelegationTranscriptConfirmedAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE)
    \/ model_step_count = 14 /\ AuthorizeLiveDelegationWorkerStartAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, AuditWorker, "ExistingMember")

AuditChannelUnbound ==
    /\ AuditChannel \notin DOMAIN live_execution_runtime_id_by_channel
    /\ AuditChannel \notin DOMAIN live_execution_fence_by_channel
    /\ AuditChannel \notin DOMAIN live_execution_generation_by_channel

ResolveStart(channel, started) ==
    \/ ResolveLiveDelegationWorkerStartIdle(channel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, started)
    \/ ResolveLiveDelegationWorkerStartAttached(channel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, started)
    \/ ResolveLiveDelegationWorkerStartRunning(channel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, started)

RecordTerminal(terminal) ==
    \/ RecordLiveDelegationWorkerTerminalIdle(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ RecordLiveDelegationWorkerTerminalAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ RecordLiveDelegationWorkerTerminalRunning(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, terminal)

Reconcile(terminal) ==
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshIdle(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshAttached(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshRunning(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshRetired(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshStopped(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyIdle(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyAttached(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyRunning(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyRetired(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyStopped(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayIdle(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayAttached(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayRunning(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayRetired(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayStopped(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)

Close ==
    \E sequence \in 0..2 :
        \/ RecordLiveCloseClosedIdle(AuditSession, AuditChannel, sequence)
        \/ RecordLiveCloseClosedAttached(AuditSession, AuditChannel, sequence)
        \/ RecordLiveCloseClosedRunning(AuditSession, AuditChannel, sequence)

CloseSettlement ==
    \/ DeferLiveCloseSettlementIdle(AuditSession, AuditChannel)
    \/ DeferLiveCloseSettlementAttached(AuditSession, AuditChannel)
    \/ DeferLiveCloseSettlementRunning(AuditSession, AuditChannel)
    \/ ResolveLiveCloseSettlementIdle(AuditSession, AuditChannel)
    \/ ResolveLiveCloseSettlementAttached(AuditSession, AuditChannel)
    \/ ResolveLiveCloseSettlementRunning(AuditSession, AuditChannel)

AuditBody ==
    \/ Close
       /\ audit_closed' = TRUE
       /\ UNCHANGED <<audit_resolved_after_close, audit_settled_after_close>>
    \/ CloseSettlement /\ UNCHANGED auditVars
    \/ \E channel \in {AuditChannel, AuditForeignChannel}, started \in BOOLEAN :
        /\ ResolveStart(channel, started)
        /\ audit_resolved_after_close' = (audit_resolved_after_close \/ (audit_closed /\ AuditChannelUnbound))
        /\ UNCHANGED <<audit_closed, audit_settled_after_close>>
    \/ \E terminal \in {"Completed", "Failed"} : RecordTerminal(terminal) /\ UNCHANGED auditVars
    \/ \E terminal \in {"Completed", "Failed"} :
        /\ Reconcile(terminal)
        /\ audit_settled_after_close' = (audit_settled_after_close \/ audit_resolved_after_close)
        /\ UNCHANGED <<audit_closed, audit_resolved_after_close>>

AuditNext ==
    \/ AuditPrefix /\ UNCHANGED auditVars
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec ==
    /\ Init
    /\ audit_closed = FALSE
    /\ audit_resolved_after_close = FALSE
    /\ audit_settled_after_close = FALSE
    /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* Safety: the operation's worker start never resolves through another channel.
AuditForeignChannelNeverResolves ==
    \A started \in BOOLEAN :
        /\ ~ENABLED ResolveLiveDelegationWorkerStartIdle(AuditForeignChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, started)
        /\ ~ENABLED ResolveLiveDelegationWorkerStartAttached(AuditForeignChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, started)
        /\ ~ENABLED ResolveLiveDelegationWorkerStartRunning(AuditForeignChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, started)

\* Goals (expected violated).
AuditNeverResolvedAfterClose == ~audit_resolved_after_close
AuditNeverSettledAfterClose == ~audit_settled_after_close
====
