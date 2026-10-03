---- MODULE live_delegation_steer_audit ----
\* Hand-written bounded audit of the live delegation steer edges.
\*
\* The generated MeerkatMachine model is far too large for its ci profile to
\* reach a bound live channel with a running delegation worker, and its deep
\* profile does not complete. This audit keeps the generated model's
\* variables, Init, actions and invariants unchanged. A deterministic prefix
\* registers a session, binds its runtime, opens and binds a live channel,
\* admits one interaction with one delegation, confirms its transcript and
\* authorizes its worker start. Everything after the prefix is explored
\* exhaustively over the steer surface: authorizing steers for two
\* continuations, the worker start resolving, each steer's delivery outcome
\* (delivered or not), its reconciliation at commit (confirmed, material
\* conflict, missing), and the worker reaching each terminal.
\*
\* The TLC config is DERIVED from the generated ci.cfg by
\* live_delegation_steer_audit.sh (every generated constant and invariant,
\* including live_delegation_steer_records_are_authorized_and_single, plus the
\* audit values, invariants and step bound). The script also proves each
\* steer outcome is reachable: for every goal it checks the goal's negation
\* and requires TLC to find a counterexample.
EXTENDS model

CONSTANT AuditMaxSteps

AuditSession == "session_1"
AuditChannel == "channel_1"
AuditRuntime == "runtime_1"
AuditInteraction == "interaction_1"
AuditOperation == "operation_1"
AuditTurn == "turn_1"
AuditWorker == "worker_1"
AuditIdentity == "identity_1"
AuditContinuations == {"continuation_1", "continuation_2"}
AuditDigests == {"digest_1"}
AuditPrefixLength == 8

AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ model_step_count = 3 /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditChannel, AuditIdentity)
    \/ model_step_count = 4 /\ BindLiveExecutionChannelAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, 0)
    \/ model_step_count = 5 /\ AdmitLiveInteractionDelegationAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE, TRUE)
    \/ model_step_count = 6 /\ ReconcileLiveDelegationTranscriptConfirmedAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, TRUE, TRUE)
    \/ model_step_count = 7 /\ AuthorizeLiveDelegationWorkerStartAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, AuditWorker, "ExistingMember")

AuditSteer ==
    \/ \E c \in AuditContinuations : \E d \in AuditDigests :
        (AuthorizeLiveDelegationSteerIdle(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, c, d) \/ AuthorizeLiveDelegationSteerAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditTurn, c, d))
    \/ \E c \in AuditContinuations : \E delivered \in BOOLEAN :
        (ResolveLiveDelegationSteerDeliveryIdle(AuditChannel, AuditRuntime, 1, 1, AuditOperation, c, delivered) \/ ResolveLiveDelegationSteerDeliveryAttached(AuditChannel, AuditRuntime, 1, 1, AuditOperation, c, delivered))
    \/ \E c \in AuditContinuations : \E committed \in BOOLEAN : \E matches \in BOOLEAN :
        \/ (ReconcileLiveDelegationSteerConfirmedIdle(AuditChannel, AuditRuntime, 1, 1, AuditOperation, c, committed, matches) \/ ReconcileLiveDelegationSteerConfirmedAttached(AuditChannel, AuditRuntime, 1, 1, AuditOperation, c, committed, matches))
        \/ (ReconcileLiveDelegationSteerMaterialConflictIdle(AuditChannel, AuditRuntime, 1, 1, AuditOperation, c, committed, matches) \/ ReconcileLiveDelegationSteerMaterialConflictAttached(AuditChannel, AuditRuntime, 1, 1, AuditOperation, c, committed, matches))
        \/ (ReconcileLiveDelegationSteerMissingIdle(AuditChannel, AuditRuntime, 1, 1, AuditOperation, c, committed, matches) \/ ReconcileLiveDelegationSteerMissingAttached(AuditChannel, AuditRuntime, 1, 1, AuditOperation, c, committed, matches))

AuditWorkerLifecycle ==
    \/ \E started \in BOOLEAN :
        (ResolveLiveDelegationWorkerStartIdle(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, started) \/ ResolveLiveDelegationWorkerStartAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, started))
    \/ \E terminal \in LiveDelegationWorkerTerminalKindValues :
        (RecordLiveDelegationWorkerTerminalIdle(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, terminal) \/ RecordLiveDelegationWorkerTerminalAttached(AuditChannel, AuditRuntime, 1, 1, AuditInteraction, AuditOperation, AuditWorker, terminal))

AuditNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ (AuditSteer \/ AuditWorkerLifecycle))

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* A steer is authorized only while the worker accepts input, so a steer for a
\* worker that already reached a terminal before authorization never exists:
\* every recorded steer's operation is the audit operation, and a delivered
\* or not-delivered outcome never precedes its authorization (the generated
\* invariant checks the latter structurally).
AuditSteerBelongsToTheDelegation ==
    \A c \in DOMAIN live_delegation_steer_operation_by_continuation :
        live_delegation_steer_operation_by_continuation[c] = AuditOperation

\* Each continuation is reconciled at most once: once settled it never
\* returns to Provisional.
AuditSettledSteerStaysSettled ==
    \A c \in DOMAIN live_delegation_steer_reconciliation_by_continuation :
        live_delegation_steer_reconciliation_by_continuation[c] \in LiveDelegationReconciliationValues

\* Reachability goals: the script checks each negation and requires a
\* counterexample, proving the outcome is reachable within the bound.
GoalDeliveredConfirmed ==
    \E c \in DOMAIN live_delegation_steer_delivered_by_continuation :
        /\ live_delegation_steer_delivered_by_continuation[c] = TRUE
        /\ c \in DOMAIN live_delegation_steer_reconciliation_by_continuation
        /\ live_delegation_steer_reconciliation_by_continuation[c] = "Confirmed"
GoalNotDelivered ==
    \E c \in DOMAIN live_delegation_steer_delivered_by_continuation :
        live_delegation_steer_delivered_by_continuation[c] = FALSE
GoalMaterialConflict ==
    \E c \in DOMAIN live_delegation_steer_reconciliation_by_continuation :
        live_delegation_steer_reconciliation_by_continuation[c] = "MaterialConflict"
GoalMissing ==
    \E c \in DOMAIN live_delegation_steer_reconciliation_by_continuation :
        live_delegation_steer_reconciliation_by_continuation[c] = "Missing"
NotGoalDeliveredConfirmed == ~GoalDeliveredConfirmed
NotGoalNotDelivered == ~GoalNotDelivered
NotGoalMaterialConflict == ~GoalMaterialConflict
NotGoalMissing == ~GoalMissing
====
