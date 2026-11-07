---- MODULE live_delegation_admitted_start_after_close_audit ----
\* Bounded audit of a client delegation admitted under its channel's exact
\* binding whose worker starts only after that channel closed (Turbo S S107).
\*
\* The delegation keeps its generated admission through the close. Its
\* transcript reconciliation (ReconcileLiveDelegationTranscript*), worker
\* start (AuthorizeLiveDelegationWorkerStart), consequential-effect authority
\* (AuthorizeLiveConsequentialEffect), busy-source requeue
\* (RequeueUnstartedLiveDelegation) and the steer of a continuation the user
\* spoke while it waited (AuthorizeLiveDelegationSteer,
\* ResolveLiveDelegationSteerDelivery, ReconcileLiveDelegationSteer*) are each
\* admitted under the channel's exact runtime/fence/generation binding or,
\* once the operation's OWN channel carries no binding at all (close unbinds
\* it) and its session is still registered, on the exact operation join
\* alone, as ResolveLiveDelegationWorkerStart already is (Turbo S S104 R7). A
\* worker that hands its item back blocked once the channel is unbound
\* settles Cancelled: no requeue can serve it again. This audit keeps the
\* generated MeerkatMachine model unchanged. A deterministic prefix reaches
\* one experimental channel with an admitted delegation (schedule Created,
\* reconciliation Provisional, no worker). The body explores the channel's
\* close and close settlement, every reconciliation outcome, worker-start
\* authorization, consequential-effect authority, worker-start resolution
\* (started or not), the busy-source requeue, the queued cancellation, the
\* steer and its delivery and reconciliation, each through the operation's
\* own channel and through a foreign never-bound channel, and the worker's
\* terminal record (completed, failed or blocked) and revoked-worker
\* reconciliation (fresh, or of a terminal recorded while bound). Every
\* explored state is checked against every generated invariant plus:
\*   AuditForeignChannelNeverServes: no reconciliation, worker start,
\*     consequential effect, start resolution, requeue or steer of the
\*     operation is enabled through a channel other than its own;
\*   AuditStaleBindingNeverServes: while the channel is still bound, none of
\*     them is enabled under a fence or generation other than the channel's;
\*   AuditClosedChannelHoldsNoWorkerCount: once the channel closed it holds
\*     no worker count (close removed it, and a worker started after the
\*     close never takes a slot the revoked-worker reconciliation would have
\*     to release);
\*   AuditRevokedBlockedIsCancelled: a blocked hand-back reconciled on the
\*     unbound channel leaves the operation Cancelled.
\* Goals (each required violated): after the close, the transcript reconciles
\* confirmed; it reconciles as a conflict or missing; the worker start is
\* authorized; the consequential effect is authorized; a start that failed
\* requeues; a worker authorized after the close settles through the
\* revoked-worker reconciliation; a steer is authorized, delivered and
\* reconciled; and a blocked hand-back settles, fresh and from a terminal
\* recorded while the channel was bound.
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
AuditAuthority == "authority_1"
AuditContinuation == "continuation_1"
AuditContinuationDigest == "digest_c"
AuditPrefixLength == 13

\* The audit's history of the operation, one flag each:
\*   closed           the channel's close was recorded;
\*   confirmed        the transcript reconciled confirmed after that close;
\*   unconfirmed      it reconciled as a conflict or missing after that close;
\*   authorized       the worker start was authorized after that close;
\*   consequential    the consequential effect was authorized after that close;
\*   requeued         a failed start requeued after that close;
\*   settled          a worker authorized after the close settled;
\*   steer_authorized the steer was authorized after the close;
\*   steer_resolved   that steer's delivery then resolved after the close;
\*   steer_settled    that steer then reconciled after the close;
\*   fresh_blocked    a blocked hand-back was reconciled on the unbound channel;
\*   custody_blocked  so was a blocked terminal recorded while the channel was bound.
VARIABLE audit

AuditFlags == {"closed", "confirmed", "unconfirmed", "authorized", "consequential",
               "requeued", "settled", "steer_authorized", "steer_resolved",
               "steer_settled", "fresh_blocked", "custody_blocked"}

AuditInit == audit = [flag \in AuditFlags |-> FALSE]

\* Set `flag` when `holds`; every other flag keeps its value.
Mark(flag, holds) == audit' = [audit EXCEPT ![flag] = @ \/ holds]

\* A deterministic prefix reaches a bound experimental channel with an
\* admitted delegation and no worker.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession, "Session")
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

AuditChannelUnbound ==
    /\ AuditChannel \notin DOMAIN live_execution_runtime_id_by_channel
    /\ AuditChannel \notin DOMAIN live_execution_fence_by_channel
    /\ AuditChannel \notin DOMAIN live_execution_generation_by_channel

AuditAfterClose == audit.closed /\ AuditChannelUnbound

\* Each action of the operation through `channel` under fence and generation
\* `binding` (the channel's own binding is 1).
ReconcileConfirmed(channel, binding) ==
    \/ ReconcileLiveDelegationTranscriptConfirmedIdle(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE)
    \/ ReconcileLiveDelegationTranscriptConfirmedAttached(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE)
    \/ ReconcileLiveDelegationTranscriptConfirmedRunning(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE)

ReconcileUnconfirmed(channel, binding) ==
    \/ ReconcileLiveDelegationTranscriptMaterialConflictIdle(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, TRUE, FALSE)
    \/ ReconcileLiveDelegationTranscriptMaterialConflictAttached(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, TRUE, FALSE)
    \/ ReconcileLiveDelegationTranscriptMaterialConflictRunning(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, TRUE, FALSE)
    \/ ReconcileLiveDelegationTranscriptMissingIdle(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, FALSE, FALSE)
    \/ ReconcileLiveDelegationTranscriptMissingAttached(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, FALSE, FALSE)
    \/ ReconcileLiveDelegationTranscriptMissingRunning(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, FALSE, FALSE)

AuthorizeStart(channel, binding) ==
    \/ AuthorizeLiveDelegationWorkerStartIdle(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, AuditWorker, "ExistingMember")
    \/ AuthorizeLiveDelegationWorkerStartAttached(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, AuditWorker, "ExistingMember")
    \/ AuthorizeLiveDelegationWorkerStartRunning(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, AuditWorker, "ExistingMember")

AuthorizeConsequential(channel, binding) ==
    \/ AuthorizeLiveConsequentialEffectIdle(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditAuthority)
    \/ AuthorizeLiveConsequentialEffectAttached(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditAuthority)
    \/ AuthorizeLiveConsequentialEffectRunning(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditAuthority)

ResolveStart(channel, binding, started) ==
    \/ ResolveLiveDelegationWorkerStartIdle(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditWorker, started)
    \/ ResolveLiveDelegationWorkerStartAttached(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditWorker, started)
    \/ ResolveLiveDelegationWorkerStartRunning(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditWorker, started)

RequeueUnstarted(channel, binding) ==
    \/ RequeueUnstartedLiveDelegationIdle(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation)
    \/ RequeueUnstartedLiveDelegationAttached(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation)
    \/ RequeueUnstartedLiveDelegationRunning(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation)

AuthorizeSteer(channel, binding) ==
    \/ AuthorizeLiveDelegationSteerIdle(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, AuditContinuation, AuditContinuationDigest)
    \/ AuthorizeLiveDelegationSteerAttached(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, AuditContinuation, AuditContinuationDigest)
    \/ AuthorizeLiveDelegationSteerRunning(channel, AuditRuntime, binding, binding, AuditInteraction, AuditOperation, AuditTurn, AuditContinuation, AuditContinuationDigest)

ResolveSteer(channel, binding, delivered) ==
    \/ ResolveLiveDelegationSteerDeliveryIdle(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, delivered)
    \/ ResolveLiveDelegationSteerDeliveryAttached(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, delivered)
    \/ ResolveLiveDelegationSteerDeliveryRunning(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, delivered)

ReconcileSteer(channel, binding) ==
    \/ ReconcileLiveDelegationSteerConfirmedIdle(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, TRUE, TRUE)
    \/ ReconcileLiveDelegationSteerConfirmedAttached(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, TRUE, TRUE)
    \/ ReconcileLiveDelegationSteerConfirmedRunning(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, TRUE, TRUE)
    \/ ReconcileLiveDelegationSteerMaterialConflictIdle(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, TRUE, FALSE)
    \/ ReconcileLiveDelegationSteerMaterialConflictAttached(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, TRUE, FALSE)
    \/ ReconcileLiveDelegationSteerMaterialConflictRunning(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, TRUE, FALSE)
    \/ ReconcileLiveDelegationSteerMissingIdle(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, FALSE, FALSE)
    \/ ReconcileLiveDelegationSteerMissingAttached(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, FALSE, FALSE)
    \/ ReconcileLiveDelegationSteerMissingRunning(channel, AuditRuntime, binding, binding, AuditOperation, AuditContinuation, FALSE, FALSE)

\* Whether any of those actions is enabled through `channel` under `binding`.
Serves(channel, binding) ==
    \/ ENABLED ReconcileConfirmed(channel, binding)
    \/ ENABLED ReconcileUnconfirmed(channel, binding)
    \/ ENABLED AuthorizeStart(channel, binding)
    \/ ENABLED AuthorizeConsequential(channel, binding)
    \/ ENABLED ResolveStart(channel, binding, TRUE)
    \/ ENABLED ResolveStart(channel, binding, FALSE)
    \/ ENABLED RequeueUnstarted(channel, binding)
    \/ ENABLED AuthorizeSteer(channel, binding)
    \/ ENABLED ResolveSteer(channel, binding, TRUE)
    \/ ENABLED ResolveSteer(channel, binding, FALSE)
    \/ ENABLED ReconcileSteer(channel, binding)

CancelQueued ==
    \/ CancelQueuedLiveDelegationIdle(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation)
    \/ CancelQueuedLiveDelegationAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation)
    \/ CancelQueuedLiveDelegationRunning(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation)
    \/ CancelQueuedLiveDelegationRetired(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation)
    \/ CancelQueuedLiveDelegationStopped(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation)

RecordTerminal(terminal) ==
    \/ RecordLiveDelegationWorkerTerminalIdle(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ RecordLiveDelegationWorkerTerminalAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ RecordLiveDelegationWorkerTerminalRunning(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, terminal)

ReconcileRevoked(terminal) ==
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshIdle(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshAttached(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshRunning(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshRetired(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartFreshStopped(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)

ReconcileRevokedCustody(terminal) ==
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyIdle(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyAttached(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyRunning(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyRetired(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)
    \/ ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyStopped(AuditSession, AuditChannel, AuditInteraction, AuditOperation, AuditWorker, terminal)

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

Channels == {AuditChannel, AuditForeignChannel}
Terminals == {"Completed", "Failed", "Blocked"}

AuditBody ==
    \/ Close /\ Mark("closed", TRUE)
    \/ CloseSettlement /\ UNCHANGED audit
    \/ \E channel \in Channels :
        ReconcileConfirmed(channel, 1) /\ Mark("confirmed", AuditAfterClose)
    \/ \E channel \in Channels :
        ReconcileUnconfirmed(channel, 1) /\ Mark("unconfirmed", AuditAfterClose)
    \/ \E channel \in Channels :
        AuthorizeStart(channel, 1) /\ Mark("authorized", AuditAfterClose)
    \/ \E channel \in Channels :
        AuthorizeConsequential(channel, 1) /\ Mark("consequential", AuditAfterClose)
    \/ \E channel \in Channels, started \in BOOLEAN :
        ResolveStart(channel, 1, started) /\ UNCHANGED audit
    \/ \E channel \in Channels :
        RequeueUnstarted(channel, 1) /\ Mark("requeued", AuditAfterClose)
    \/ \E channel \in Channels :
        AuthorizeSteer(channel, 1) /\ Mark("steer_authorized", AuditAfterClose)
    \/ \E channel \in Channels, delivered \in BOOLEAN :
        /\ ResolveSteer(channel, 1, delivered)
        /\ Mark("steer_resolved", AuditAfterClose /\ audit.steer_authorized)
    \/ \E channel \in Channels :
        /\ ReconcileSteer(channel, 1)
        /\ Mark("steer_settled", AuditAfterClose /\ audit.steer_resolved)
    \/ CancelQueued /\ UNCHANGED audit
    \/ \E terminal \in Terminals : RecordTerminal(terminal) /\ UNCHANGED audit
    \/ \E terminal \in Terminals :
        /\ ReconcileRevoked(terminal)
        /\ audit' = [audit EXCEPT !.settled = @ \/ audit.authorized,
                                  !.fresh_blocked = @ \/ terminal = "Blocked"]
    \/ \E terminal \in Terminals :
        ReconcileRevokedCustody(terminal) /\ Mark("custody_blocked", terminal = "Blocked")

AuditNext ==
    \/ AuditPrefix /\ UNCHANGED audit
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec ==
    /\ Init
    /\ AuditInit
    /\ [][AuditNext]_<<vars, audit>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* Safety: the operation is never served through another channel.
AuditForeignChannelNeverServes ==
    /\ ~Serves(AuditForeignChannel, 1)
    /\ ~Serves(AuditForeignChannel, 2)

\* Safety: a still-bound channel serves the operation only under its own
\* fence and generation.
AuditStaleBindingNeverServes ==
    ~AuditChannelUnbound => ~Serves(AuditChannel, 2)

\* Safety: a closed channel holds no worker count.
AuditClosedChannelHoldsNoWorkerCount ==
    AuditAfterClose => AuditChannel \notin DOMAIN live_delegation_active_worker_count_by_channel

\* Safety: a blocked hand-back reconciled on the unbound channel leaves the
\* operation Cancelled (nothing can requeue it).
AuditRevokedBlockedIsCancelled ==
    (audit.fresh_blocked \/ audit.custody_blocked)
        => live_delegation_schedule_state_by_operation[AuditOperation] = "Cancelled"

\* Goals (expected violated).
AuditNeverConfirmedAfterClose == ~audit.confirmed
AuditNeverUnconfirmedAfterClose == ~audit.unconfirmed
AuditNeverAuthorizedAfterClose == ~audit.authorized
AuditNeverConsequentialAfterClose == ~audit.consequential
AuditNeverRequeuedAfterClose == ~audit.requeued
AuditNeverSettledAfterClose == ~audit.settled
AuditNeverSteerSettledAfterClose == ~audit.steer_settled
AuditNeverFreshBlockedSettled == ~audit.fresh_blocked
AuditNeverCustodyBlockedSettled == ~audit.custody_blocked
====
