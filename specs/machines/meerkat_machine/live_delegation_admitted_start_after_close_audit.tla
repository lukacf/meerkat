---- MODULE live_delegation_admitted_start_after_close_audit ----
\* Bounded audit of a client delegation admitted under its channel's exact
\* binding whose worker starts only after that channel closed (Turbo S S107).
\*
\* The delegation keeps its generated admission through the close. Its
\* transcript reconciliation (ReconcileLiveDelegationTranscript*), worker
\* start (AuthorizeLiveDelegationWorkerStart), consequential-effect authority
\* (AuthorizeLiveConsequentialEffect) and busy-source requeue
\* (RequeueUnstartedLiveDelegation) are each admitted under the channel's
\* exact runtime/fence/generation binding or, once the operation's OWN
\* channel carries no binding at all (close unbinds it), on the exact
\* operation join alone, as ResolveLiveDelegationWorkerStart already is
\* (Turbo S S104 R7). This audit keeps the generated MeerkatMachine model
\* unchanged. A deterministic prefix reaches one experimental channel with an
\* admitted delegation (schedule Created, reconciliation Provisional, no
\* worker). The body explores the channel's close and close settlement, every
\* reconciliation outcome, worker-start authorization, consequential-effect
\* authority, worker-start resolution (started or not), the busy-source
\* requeue and the queued cancellation, each through the operation's own
\* channel and through a foreign never-bound channel, and the worker's
\* terminal record and revoked-worker reconciliation. Every explored state is
\* checked against every generated invariant plus:
\*   AuditForeignChannelNeverServes: no reconciliation, worker start,
\*     consequential effect, start resolution or requeue of the operation is
\*     enabled through a channel other than its own;
\*   AuditStaleBindingNeverServes: while the channel is still bound, none of
\*     them is enabled under a fence or generation other than the channel's;
\*   AuditClosedChannelHoldsNoWorkerCount: once the channel closed it holds
\*     no worker count (close removed it, and a worker started after the
\*     close never takes a slot the revoked-worker reconciliation would have
\*     to release).
\* Goals (each required violated): after the close, the transcript reconciles
\* confirmed; it reconciles as a conflict or missing; the worker start is
\* authorized; the consequential effect is authorized; a start that failed
\* requeues; and a worker authorized after the close settles through the
\* revoked-worker reconciliation.
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
AuditPrefixLength == 13

VARIABLES
    audit_closed,                    \* the channel's close was recorded
    audit_confirmed_after_close,     \* the transcript reconciled confirmed after that close
    audit_unconfirmed_after_close,   \* it reconciled as a conflict or missing after that close
    audit_authorized_after_close,    \* the worker start was authorized after that close
    audit_consequential_after_close, \* the consequential effect was authorized after that close
    audit_requeued_after_close,      \* a failed start requeued after that close
    audit_settled_after_close        \* a worker authorized after the close settled

auditVars == <<audit_closed, audit_confirmed_after_close, audit_unconfirmed_after_close,
               audit_authorized_after_close, audit_consequential_after_close,
               audit_requeued_after_close, audit_settled_after_close>>

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

AuditAfterClose == audit_closed /\ AuditChannelUnbound

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

\* Whether any of those actions is enabled through `channel` under `binding`.
Serves(channel, binding) ==
    \/ ENABLED ReconcileConfirmed(channel, binding)
    \/ ENABLED ReconcileUnconfirmed(channel, binding)
    \/ ENABLED AuthorizeStart(channel, binding)
    \/ ENABLED AuthorizeConsequential(channel, binding)
    \/ ENABLED ResolveStart(channel, binding, TRUE)
    \/ ENABLED ResolveStart(channel, binding, FALSE)
    \/ ENABLED RequeueUnstarted(channel, binding)

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

AuditBody ==
    \/ Close
       /\ audit_closed' = TRUE
       /\ UNCHANGED <<audit_confirmed_after_close, audit_unconfirmed_after_close,
                      audit_authorized_after_close, audit_consequential_after_close,
                      audit_requeued_after_close, audit_settled_after_close>>
    \/ CloseSettlement /\ UNCHANGED auditVars
    \/ \E channel \in Channels :
        /\ ReconcileConfirmed(channel, 1)
        /\ audit_confirmed_after_close' = (audit_confirmed_after_close \/ AuditAfterClose)
        /\ UNCHANGED <<audit_closed, audit_unconfirmed_after_close, audit_authorized_after_close,
                       audit_consequential_after_close, audit_requeued_after_close,
                       audit_settled_after_close>>
    \/ \E channel \in Channels :
        /\ ReconcileUnconfirmed(channel, 1)
        /\ audit_unconfirmed_after_close' = (audit_unconfirmed_after_close \/ AuditAfterClose)
        /\ UNCHANGED <<audit_closed, audit_confirmed_after_close, audit_authorized_after_close,
                       audit_consequential_after_close, audit_requeued_after_close,
                       audit_settled_after_close>>
    \/ \E channel \in Channels :
        /\ AuthorizeStart(channel, 1)
        /\ audit_authorized_after_close' = (audit_authorized_after_close \/ AuditAfterClose)
        /\ UNCHANGED <<audit_closed, audit_confirmed_after_close, audit_unconfirmed_after_close,
                       audit_consequential_after_close, audit_requeued_after_close,
                       audit_settled_after_close>>
    \/ \E channel \in Channels :
        /\ AuthorizeConsequential(channel, 1)
        /\ audit_consequential_after_close' = (audit_consequential_after_close \/ AuditAfterClose)
        /\ UNCHANGED <<audit_closed, audit_confirmed_after_close, audit_unconfirmed_after_close,
                       audit_authorized_after_close, audit_requeued_after_close,
                       audit_settled_after_close>>
    \/ \E channel \in Channels, started \in BOOLEAN :
        /\ ResolveStart(channel, 1, started)
        /\ UNCHANGED auditVars
    \/ \E channel \in Channels :
        /\ RequeueUnstarted(channel, 1)
        /\ audit_requeued_after_close' = (audit_requeued_after_close \/ AuditAfterClose)
        /\ UNCHANGED <<audit_closed, audit_confirmed_after_close, audit_unconfirmed_after_close,
                       audit_authorized_after_close, audit_consequential_after_close,
                       audit_settled_after_close>>
    \/ CancelQueued /\ UNCHANGED auditVars
    \/ \E terminal \in {"Completed", "Failed"} : RecordTerminal(terminal) /\ UNCHANGED auditVars
    \/ \E terminal \in {"Completed", "Failed"} :
        /\ ReconcileRevoked(terminal)
        /\ audit_settled_after_close' = (audit_settled_after_close \/ audit_authorized_after_close)
        /\ UNCHANGED <<audit_closed, audit_confirmed_after_close, audit_unconfirmed_after_close,
                       audit_authorized_after_close, audit_consequential_after_close,
                       audit_requeued_after_close>>

AuditNext ==
    \/ AuditPrefix /\ UNCHANGED auditVars
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec ==
    /\ Init
    /\ audit_closed = FALSE
    /\ audit_confirmed_after_close = FALSE
    /\ audit_unconfirmed_after_close = FALSE
    /\ audit_authorized_after_close = FALSE
    /\ audit_consequential_after_close = FALSE
    /\ audit_requeued_after_close = FALSE
    /\ audit_settled_after_close = FALSE
    /\ [][AuditNext]_<<vars, auditVars>>

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

\* Goals (expected violated).
AuditNeverConfirmedAfterClose == ~audit_confirmed_after_close
AuditNeverUnconfirmedAfterClose == ~audit_unconfirmed_after_close
AuditNeverAuthorizedAfterClose == ~audit_authorized_after_close
AuditNeverConsequentialAfterClose == ~audit_consequential_after_close
AuditNeverRequeuedAfterClose == ~audit_requeued_after_close
AuditNeverSettledAfterClose == ~audit_settled_after_close
====
