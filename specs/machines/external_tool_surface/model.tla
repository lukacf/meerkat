---- MODULE model ----
EXTENDS Naturals, TLC

\* Abstract, architecture-level model of ExternalToolSurfaceMachine.
\* This models staged ops, pending activation/reload, visibility, and
\* inflight-aware removal semantics without transport payload detail. The
\* outward contract is one canonical typed delta per surface.

CONSTANTS
    Surfaces,
    MaxInflight

ASSUME Surfaces # {}
ASSUME MaxInflight \in Nat

BaseStates == {"absent", "active", "removing", "removed"}
PendingOps == {"none", "add", "reload"}
StagedOps == {"none", "add", "remove", "reload"}
DeltaOps == {"none", "add", "remove", "reload"}
DeltaPhases == {"none", "pending", "applied", "draining", "forced", "failed"}

VARIABLES
    baseState,
    pendingOp,
    stagedOp,
    inflight,
    lastDeltaOp,
    lastDeltaPhase

vars ==
    << baseState, pendingOp, stagedOp, inflight, lastDeltaOp, lastDeltaPhase >>

Init ==
    /\ baseState = [s \in Surfaces |-> "absent"]
    /\ pendingOp = [s \in Surfaces |-> "none"]
    /\ stagedOp = [s \in Surfaces |-> "none"]
    /\ inflight = [s \in Surfaces |-> 0]
    /\ lastDeltaOp = [s \in Surfaces |-> "none"]
    /\ lastDeltaPhase = [s \in Surfaces |-> "none"]

StageAdd(s) ==
    /\ s \in Surfaces
    /\ stagedOp' = [stagedOp EXCEPT ![s] = "add"]
    /\ UNCHANGED << baseState, pendingOp, inflight, lastDeltaOp, lastDeltaPhase >>

StageRemove(s) ==
    /\ s \in Surfaces
    /\ stagedOp' = [stagedOp EXCEPT ![s] = "remove"]
    /\ UNCHANGED << baseState, pendingOp, inflight, lastDeltaOp, lastDeltaPhase >>

StageReload(s) ==
    /\ s \in Surfaces
    /\ baseState[s] = "active"
    /\ stagedOp' = [stagedOp EXCEPT ![s] = "reload"]
    /\ UNCHANGED << baseState, pendingOp, inflight, lastDeltaOp, lastDeltaPhase >>

ApplyAdd(s) ==
    /\ s \in Surfaces
    /\ stagedOp[s] = "add"
    /\ baseState[s] \in {"absent", "active", "removed"}
    /\ stagedOp' = [stagedOp EXCEPT ![s] = "none"]
    /\ pendingOp' = [pendingOp EXCEPT ![s] = "add"]
    /\ lastDeltaOp' = [lastDeltaOp EXCEPT ![s] = "add"]
    /\ lastDeltaPhase' = [lastDeltaPhase EXCEPT ![s] = "pending"]
    /\ UNCHANGED << baseState, inflight >>

ApplyReload(s) ==
    /\ s \in Surfaces
    /\ stagedOp[s] = "reload"
    /\ baseState[s] = "active"
    /\ stagedOp' = [stagedOp EXCEPT ![s] = "none"]
    /\ pendingOp' = [pendingOp EXCEPT ![s] = "reload"]
    /\ lastDeltaOp' = [lastDeltaOp EXCEPT ![s] = "reload"]
    /\ lastDeltaPhase' = [lastDeltaPhase EXCEPT ![s] = "pending"]
    /\ UNCHANGED << baseState, inflight >>

ApplyRemove(s) ==
    /\ s \in Surfaces
    /\ stagedOp[s] = "remove"
    /\ stagedOp' = [stagedOp EXCEPT ![s] = "none"]
    /\ pendingOp' = [pendingOp EXCEPT ![s] = "none"]
    /\ baseState' =
        [baseState EXCEPT
            ![s] =
                IF baseState[s] = "active"
                THEN "removing"
                ELSE baseState[s]]
    /\ inflight' = inflight
    /\ lastDeltaOp' =
        [lastDeltaOp EXCEPT
            ![s] =
                IF baseState[s] = "active"
                THEN "remove"
                ELSE "none"]
    /\ lastDeltaPhase' =
        [lastDeltaPhase EXCEPT
            ![s] =
                IF baseState[s] = "active"
                THEN "draining"
                ELSE "none"]

PendingSucceeded(s) ==
    /\ s \in Surfaces
    /\ pendingOp[s] \in {"add", "reload"}
    /\ pendingOp' = [pendingOp EXCEPT ![s] = "none"]
    /\ baseState' = [baseState EXCEPT ![s] = "active"]
    /\ lastDeltaOp' = [lastDeltaOp EXCEPT ![s] = pendingOp[s]]
    /\ lastDeltaPhase' = [lastDeltaPhase EXCEPT ![s] = "applied"]
    /\ UNCHANGED << stagedOp, inflight >>

PendingFailed(s) ==
    /\ s \in Surfaces
    /\ pendingOp[s] \in {"add", "reload"}
    /\ pendingOp' = [pendingOp EXCEPT ![s] = "none"]
    /\ lastDeltaOp' = [lastDeltaOp EXCEPT ![s] = pendingOp[s]]
    /\ lastDeltaPhase' = [lastDeltaPhase EXCEPT ![s] = "failed"]
    /\ UNCHANGED << baseState, stagedOp, inflight >>

StartCall(s) ==
    /\ s \in Surfaces
    /\ baseState[s] = "active"
    /\ inflight[s] < MaxInflight
    /\ inflight' = [inflight EXCEPT ![s] = @ + 1]
    /\ UNCHANGED << baseState, pendingOp, stagedOp, lastDeltaOp, lastDeltaPhase >>

FinishCall(s) ==
    /\ s \in Surfaces
    /\ baseState[s] \in {"active", "removing"}
    /\ inflight[s] > 0
    /\ inflight' = [inflight EXCEPT ![s] = @ - 1]
    /\ UNCHANGED << baseState, pendingOp, stagedOp, lastDeltaOp, lastDeltaPhase >>

FinalizeRemovalClean(s) ==
    /\ s \in Surfaces
    /\ baseState[s] = "removing"
    /\ inflight[s] = 0
    /\ baseState' = [baseState EXCEPT ![s] = "removed"]
    /\ lastDeltaOp' = [lastDeltaOp EXCEPT ![s] = "remove"]
    /\ lastDeltaPhase' = [lastDeltaPhase EXCEPT ![s] = "applied"]
    /\ UNCHANGED << pendingOp, stagedOp, inflight >>

FinalizeRemovalForced(s) ==
    /\ s \in Surfaces
    /\ baseState[s] = "removing"
    /\ inflight[s] > 0
    /\ baseState' = [baseState EXCEPT ![s] = "removed"]
    /\ inflight' = [inflight EXCEPT ![s] = 0]
    /\ lastDeltaOp' = [lastDeltaOp EXCEPT ![s] = "remove"]
    /\ lastDeltaPhase' = [lastDeltaPhase EXCEPT ![s] = "forced"]
    /\ UNCHANGED << pendingOp, stagedOp >>

Shutdown ==
    /\ baseState' = [s \in Surfaces |-> "absent"]
    /\ pendingOp' = [s \in Surfaces |-> "none"]
    /\ stagedOp' = [s \in Surfaces |-> "none"]
    /\ inflight' = [s \in Surfaces |-> 0]
    /\ lastDeltaOp' = [s \in Surfaces |-> "none"]
    /\ lastDeltaPhase' = [s \in Surfaces |-> "none"]

Next ==
    \/ \E s \in Surfaces : StageAdd(s)
    \/ \E s \in Surfaces : StageRemove(s)
    \/ \E s \in Surfaces : StageReload(s)
    \/ \E s \in Surfaces : ApplyAdd(s)
    \/ \E s \in Surfaces : ApplyReload(s)
    \/ \E s \in Surfaces : ApplyRemove(s)
    \/ \E s \in Surfaces : PendingSucceeded(s)
    \/ \E s \in Surfaces : PendingFailed(s)
    \/ \E s \in Surfaces : StartCall(s)
    \/ \E s \in Surfaces : FinishCall(s)
    \/ \E s \in Surfaces : FinalizeRemovalClean(s)
    \/ \E s \in Surfaces : FinalizeRemovalForced(s)
    \/ Shutdown

PendingConstrainedByBaseState ==
    \A s \in Surfaces :
        /\ (baseState[s] = "removing") => pendingOp[s] = "none"
        /\ (baseState[s] = "removed") => pendingOp[s] \in {"none", "add"}

NoInflightOnAbsentOrRemoved ==
    \A s \in Surfaces :
        baseState[s] \in {"absent", "removed"} => inflight[s] = 0

ReloadRequiresActiveBase ==
    \A s \in Surfaces :
        pendingOp[s] = "reload" => baseState[s] = "active"

ForcedPhaseMatchesRemoved ==
    \A s \in Surfaces :
        lastDeltaPhase[s] = "forced" => baseState[s] = "removed"

PendingPhaseHasPendingOp ==
    \A s \in Surfaces :
        lastDeltaPhase[s] = "pending" => pendingOp[s] \in {"add", "reload"}

Spec == Init /\ [][Next]_vars

THEOREM Spec => []PendingConstrainedByBaseState
THEOREM Spec => []NoInflightOnAbsentOrRemoved
THEOREM Spec => []ReloadRequiresActiveBase
THEOREM Spec => []ForcedPhaseMatchesRemoved
THEOREM Spec => []PendingPhaseHasPendingOp

=============================================================================
