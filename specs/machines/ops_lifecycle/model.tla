---- MODULE model ----
EXTENDS Naturals, TLC

\* Abstract, architecture-level model of OpsLifecycleMachine.
\* This models the shared async-operation lifecycle substrate for mob-backed
\* child work and background tool operations.

CONSTANTS
    OperationIds,
    MaxWatchers,
    MaxProgress

ASSUME OperationIds # {}
ASSUME MaxWatchers \in Nat
ASSUME MaxProgress \in Nat

Statuses ==
    {"absent", "provisioning", "running", "retiring",
     "completed", "failed", "cancelled", "retired", "terminated"}

Kinds == {"none", "mob_member_child", "background_tool_op"}
TerminalStatuses == {"completed", "failed", "cancelled", "retired", "terminated"}
Outcomes == {"none", "completed", "failed", "cancelled", "retired", "terminated"}

VARIABLES
    operationStatus,
    operationKind,
    peerReady,
    progressCount,
    watcherCount,
    terminalOutcome,
    terminalBuffered

vars ==
    << operationStatus, operationKind, peerReady, progressCount,
       watcherCount, terminalOutcome, terminalBuffered >>

Init ==
    /\ operationStatus = [o \in OperationIds |-> "absent"]
    /\ operationKind = [o \in OperationIds |-> "none"]
    /\ peerReady = [o \in OperationIds |-> FALSE]
    /\ progressCount = [o \in OperationIds |-> 0]
    /\ watcherCount = [o \in OperationIds |-> 0]
    /\ terminalOutcome = [o \in OperationIds |-> "none"]
    /\ terminalBuffered = [o \in OperationIds |-> FALSE]

RegisterOperation(o, k) ==
    /\ o \in OperationIds
    /\ k \in {"mob_member_child", "background_tool_op"}
    /\ operationStatus[o] = "absent"
    /\ operationStatus' = [operationStatus EXCEPT ![o] = "provisioning"]
    /\ operationKind' = [operationKind EXCEPT ![o] = k]
    /\ peerReady' = [peerReady EXCEPT ![o] = FALSE]
    /\ progressCount' = [progressCount EXCEPT ![o] = 0]
    /\ watcherCount' = [watcherCount EXCEPT ![o] = 0]
    /\ terminalOutcome' = [terminalOutcome EXCEPT ![o] = "none"]
    /\ terminalBuffered' = [terminalBuffered EXCEPT ![o] = FALSE]

ProvisioningSucceeded(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] = "provisioning"
    /\ operationStatus' = [operationStatus EXCEPT ![o] = "running"]
    /\ UNCHANGED << operationKind, peerReady, progressCount,
                   watcherCount, terminalOutcome, terminalBuffered >>

ProvisioningFailed(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] = "provisioning"
    /\ operationStatus' = [operationStatus EXCEPT ![o] = "failed"]
    /\ terminalOutcome' = [terminalOutcome EXCEPT ![o] = "failed"]
    /\ terminalBuffered' = [terminalBuffered EXCEPT ![o] = TRUE]
    /\ UNCHANGED << operationKind, peerReady, progressCount, watcherCount >>

MarkPeerReady(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] \in {"running", "retiring"}
    /\ operationKind[o] = "mob_member_child"
    /\ peerReady[o] = FALSE
    /\ peerReady' = [peerReady EXCEPT ![o] = TRUE]
    /\ UNCHANGED << operationStatus, operationKind, progressCount,
                   watcherCount, terminalOutcome, terminalBuffered >>

RegisterWatcher(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] # "absent"
    /\ watcherCount[o] < MaxWatchers
    /\ watcherCount' = [watcherCount EXCEPT ![o] = @ + 1]
    /\ UNCHANGED << operationStatus, operationKind, peerReady,
                   progressCount, terminalOutcome, terminalBuffered >>

ReportProgress(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] \in {"running", "retiring"}
    /\ progressCount[o] < MaxProgress
    /\ progressCount' = [progressCount EXCEPT ![o] = @ + 1]
    /\ UNCHANGED << operationStatus, operationKind, peerReady,
                   watcherCount, terminalOutcome, terminalBuffered >>

CompleteOperation(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] \in {"running", "retiring"}
    /\ operationStatus' = [operationStatus EXCEPT ![o] = "completed"]
    /\ terminalOutcome' = [terminalOutcome EXCEPT ![o] = "completed"]
    /\ terminalBuffered' = [terminalBuffered EXCEPT ![o] = TRUE]
    /\ UNCHANGED << operationKind, peerReady, progressCount, watcherCount >>

FailOperation(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] \in {"provisioning", "running", "retiring"}
    /\ operationStatus' = [operationStatus EXCEPT ![o] = "failed"]
    /\ terminalOutcome' = [terminalOutcome EXCEPT ![o] = "failed"]
    /\ terminalBuffered' = [terminalBuffered EXCEPT ![o] = TRUE]
    /\ UNCHANGED << operationKind, peerReady, progressCount, watcherCount >>

CancelOperation(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] \in {"provisioning", "running", "retiring"}
    /\ operationStatus' = [operationStatus EXCEPT ![o] = "cancelled"]
    /\ terminalOutcome' = [terminalOutcome EXCEPT ![o] = "cancelled"]
    /\ terminalBuffered' = [terminalBuffered EXCEPT ![o] = TRUE]
    /\ UNCHANGED << operationKind, peerReady, progressCount, watcherCount >>

RequestRetire(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] = "running"
    /\ operationStatus' = [operationStatus EXCEPT ![o] = "retiring"]
    /\ UNCHANGED << operationKind, peerReady, progressCount,
                   watcherCount, terminalOutcome, terminalBuffered >>

MarkRetired(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] = "retiring"
    /\ operationStatus' = [operationStatus EXCEPT ![o] = "retired"]
    /\ terminalOutcome' = [terminalOutcome EXCEPT ![o] = "retired"]
    /\ terminalBuffered' = [terminalBuffered EXCEPT ![o] = TRUE]
    /\ UNCHANGED << operationKind, peerReady, progressCount, watcherCount >>

CollectTerminal(o) ==
    /\ o \in OperationIds
    /\ operationStatus[o] \in TerminalStatuses
    /\ terminalBuffered[o] = TRUE
    /\ terminalBuffered' = [terminalBuffered EXCEPT ![o] = FALSE]
    /\ UNCHANGED << operationStatus, operationKind, peerReady,
                   progressCount, watcherCount, terminalOutcome >>

OwnerTerminated ==
    /\ operationStatus' =
        [o \in OperationIds |->
            IF operationStatus[o] \in {"provisioning", "running", "retiring"}
            THEN "terminated"
            ELSE operationStatus[o]]
    /\ terminalOutcome' =
        [o \in OperationIds |->
            IF operationStatus[o] \in {"provisioning", "running", "retiring"}
            THEN "terminated"
            ELSE terminalOutcome[o]]
    /\ terminalBuffered' =
        [o \in OperationIds |->
            IF operationStatus[o] \in {"provisioning", "running", "retiring"}
            THEN TRUE
            ELSE terminalBuffered[o]]
    /\ UNCHANGED << operationKind, peerReady, progressCount, watcherCount >>

Next ==
    \/ \E o \in OperationIds, k \in {"mob_member_child", "background_tool_op"} :
        RegisterOperation(o, k)
    \/ \E o \in OperationIds : ProvisioningSucceeded(o)
    \/ \E o \in OperationIds : ProvisioningFailed(o)
    \/ \E o \in OperationIds : MarkPeerReady(o)
    \/ \E o \in OperationIds : RegisterWatcher(o)
    \/ \E o \in OperationIds : ReportProgress(o)
    \/ \E o \in OperationIds : CompleteOperation(o)
    \/ \E o \in OperationIds : FailOperation(o)
    \/ \E o \in OperationIds : CancelOperation(o)
    \/ \E o \in OperationIds : RequestRetire(o)
    \/ \E o \in OperationIds : MarkRetired(o)
    \/ \E o \in OperationIds : CollectTerminal(o)
    \/ OwnerTerminated

BufferedOnlyForTerminal ==
    \A o \in OperationIds :
        terminalBuffered[o] => operationStatus[o] \in TerminalStatuses

TerminalOutcomeMatchesStatus ==
    \A o \in OperationIds :
        /\ (operationStatus[o] = "completed") => terminalOutcome[o] = "completed"
        /\ (operationStatus[o] = "failed") => terminalOutcome[o] = "failed"
        /\ (operationStatus[o] = "cancelled") => terminalOutcome[o] = "cancelled"
        /\ (operationStatus[o] = "retired") => terminalOutcome[o] = "retired"
        /\ (operationStatus[o] = "terminated") => terminalOutcome[o] = "terminated"

PeerReadyImpliesMobMember ==
    \A o \in OperationIds :
        peerReady[o] => operationKind[o] = "mob_member_child"

PeerReadyImpliesExists ==
    \A o \in OperationIds :
        peerReady[o] => operationStatus[o] # "absent"

TerminalKindSticky ==
    \A o \in OperationIds :
        operationStatus[o] \in TerminalStatuses => operationKind[o] # "none"

Spec == Init /\ [][Next]_vars

THEOREM Spec => []BufferedOnlyForTerminal
THEOREM Spec => []TerminalOutcomeMatchesStatus
THEOREM Spec => []PeerReadyImpliesMobMember
THEOREM Spec => []PeerReadyImpliesExists
THEOREM Spec => []TerminalKindSticky

=============================================================================
