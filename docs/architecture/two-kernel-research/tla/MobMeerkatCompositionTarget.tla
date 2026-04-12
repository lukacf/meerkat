---- MODULE MobMeerkatCompositionTarget ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Target-state seam composition between MobMachine and the tiny Mob-visible
\* Meerkat contract. This does not reprove Meerkat internals or the full Mob
\* flow algebra; it proves that Mob lifecycle/work/flow-step coupling only
\* needs the bridge alphabet and a small Meerkat-visible state machine.

CONSTANTS
  AgentIdentityValues,
  RuntimeIdValues,
  WorkRefValues,
  MaxSteps

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]
OptionSet(S) == {None} \cup {Some(x) : x \in S}

ProvisionModeValues == {"Fresh", "Recover"}
LifecycleEventKindValues == {"Ready", "ProvisionFailed", "Retired", "Destroyed"}
WorkEventKindValues == {"Accepted", "Rejected", "Completed", "Failed", "Canceled"}
AuthorityStateValues == {"Absent", "Provisioning", "Active", "Retiring"}
MemberStatusValues == {"Absent", "Provisioning", "Ready", "Retiring", "Destroyed", "ProvisionFailed"}
BridgeRuntimeStateValues == {"Absent", "Provisioning", "Ready", "Retiring", "Destroyed"}
WorkStatusValues == {"None", "PendingDecision", "Accepted", "Rejected", "Completed", "Failed", "Canceled"}
FlowStatusValues == {"Idle", "Running", "Completed", "Failed", "Canceled"}
StepStatusValues == {"None", "Pending", "Dispatched", "Completed", "Failed", "Canceled"}

OptionRuntimeIdValues == OptionSet(RuntimeIdValues)
OptionIdentityValues == OptionSet(AgentIdentityValues)
OptionWorkRefValues == OptionSet(WorkRefValues)
OptionProvisionModeValues == OptionSet(ProvisionModeValues)
OptionLifecycleEventKindValues == OptionSet(LifecycleEventKindValues)
OptionWorkEventKindValues == OptionSet(WorkEventKindValues)

FlowId == "flow_0"
StepId == "step_0"

ZeroIdentityNatMap == [i \in AgentIdentityValues |-> 0]
FalseIdentityBoolMap == [i \in AgentIdentityValues |-> FALSE]
NoIdentityRuntimeMap == [i \in AgentIdentityValues |-> None]
NoIdentityOptionMap == [i \in AgentIdentityValues |-> None]
NoWorkStatusMap == [w \in WorkRefValues |-> "None"]
NoWorkIdentityMap == [w \in WorkRefValues |-> None]
NoWorkRuntimeMap == [w \in WorkRefValues |-> None]
ZeroWorkNatMap == [w \in WorkRefValues |-> 0]
FalseWorkBoolMap == [w \in WorkRefValues |-> FALSE]
NoLifecycleEventMap == [i \in AgentIdentityValues |-> None]
NoWorkEventMap == [w \in WorkRefValues |-> None]

PickOne(S) == CHOOSE x \in S : TRUE

MobType ==
  [ known : SUBSET AgentIdentityValues,
    generation : [AgentIdentityValues -> Nat],
    runtime_id : [AgentIdentityValues -> OptionRuntimeIdValues],
    fence_token : [AgentIdentityValues -> Nat],
    authority : [AgentIdentityValues -> AuthorityStateValues],
    member_status : [AgentIdentityValues -> MemberStatusValues],
    retire_observed : [AgentIdentityValues -> BOOLEAN],
    work_known : SUBSET WorkRefValues,
    work_owner : [WorkRefValues -> OptionIdentityValues],
    work_runtime_target : [WorkRefValues -> OptionRuntimeIdValues],
    work_status : [WorkRefValues -> WorkStatusValues],
    flow_active : BOOLEAN,
    flow_status : FlowStatusValues,
    step_status : StepStatusValues,
    step_owner : OptionIdentityValues,
    step_work : OptionWorkRefValues ]

BridgeType ==
  [ runtime_state : [RuntimeIdValues -> BridgeRuntimeStateValues],
    runtime_owner : [RuntimeIdValues -> OptionIdentityValues],
    runtime_fence : [RuntimeIdValues -> Nat],
    recoverable : [AgentIdentityValues -> BOOLEAN],
    ready_reported : [RuntimeIdValues -> BOOLEAN],
    failed_reported : [RuntimeIdValues -> BOOLEAN],
    retired_reported : [RuntimeIdValues -> BOOLEAN],
    destroyed_reported : [RuntimeIdValues -> BOOLEAN],
    work_state : [WorkRefValues -> WorkStatusValues],
    work_runtime : [WorkRefValues -> OptionRuntimeIdValues],
    work_fence : [WorkRefValues -> Nat],
    accepted_reported : [WorkRefValues -> BOOLEAN],
    terminal_reported : [WorkRefValues -> BOOLEAN] ]

SeamType ==
  [ provision_mode : [AgentIdentityValues -> OptionProvisionModeValues],
    provision_runtime : [AgentIdentityValues -> OptionRuntimeIdValues],
    provision_fence : [AgentIdentityValues -> Nat],
    retire_pending : [AgentIdentityValues -> BOOLEAN],
    retire_runtime : [AgentIdentityValues -> OptionRuntimeIdValues],
    retire_fence : [AgentIdentityValues -> Nat],
    destroy_pending : [AgentIdentityValues -> BOOLEAN],
    destroy_runtime : [AgentIdentityValues -> OptionRuntimeIdValues],
    destroy_fence : [AgentIdentityValues -> Nat],
    submit_pending : [WorkRefValues -> BOOLEAN],
    submit_runtime : [WorkRefValues -> OptionRuntimeIdValues],
    submit_fence : [WorkRefValues -> Nat],
    cancel_pending : [WorkRefValues -> BOOLEAN],
    cancel_runtime : [WorkRefValues -> OptionRuntimeIdValues],
    cancel_fence : [WorkRefValues -> Nat],
    cancel_all_pending : [AgentIdentityValues -> BOOLEAN],
    cancel_all_runtime : [AgentIdentityValues -> OptionRuntimeIdValues],
    cancel_all_fence : [AgentIdentityValues -> Nat],
    lifecycle_evt : [AgentIdentityValues -> OptionLifecycleEventKindValues],
    lifecycle_evt_runtime : [AgentIdentityValues -> OptionRuntimeIdValues],
    lifecycle_evt_fence : [AgentIdentityValues -> Nat],
    work_evt : [WorkRefValues -> OptionWorkEventKindValues],
    work_evt_runtime : [WorkRefValues -> OptionRuntimeIdValues],
    work_evt_fence : [WorkRefValues -> Nat] ]

VARIABLES mob, bridge, seam, step_count

vars == << mob, bridge, seam, step_count >>

Init ==
  /\ mob =
      [ known |-> {},
        generation |-> ZeroIdentityNatMap,
        runtime_id |-> NoIdentityRuntimeMap,
        fence_token |-> ZeroIdentityNatMap,
        authority |-> [i \in AgentIdentityValues |-> "Absent"],
        member_status |-> [i \in AgentIdentityValues |-> "Absent"],
        retire_observed |-> FalseIdentityBoolMap,
        work_known |-> {},
        work_owner |-> NoWorkIdentityMap,
        work_runtime_target |-> NoWorkRuntimeMap,
        work_status |-> NoWorkStatusMap,
        flow_active |-> FALSE,
        flow_status |-> "Idle",
        step_status |-> "None",
        step_owner |-> None,
        step_work |-> None ]
  /\ bridge =
      [ runtime_state |-> [r \in RuntimeIdValues |-> "Absent"],
        runtime_owner |-> [r \in RuntimeIdValues |-> None],
        runtime_fence |-> [r \in RuntimeIdValues |-> 0],
        recoverable |-> FalseIdentityBoolMap,
        ready_reported |-> [r \in RuntimeIdValues |-> FALSE],
        failed_reported |-> [r \in RuntimeIdValues |-> FALSE],
        retired_reported |-> [r \in RuntimeIdValues |-> FALSE],
        destroyed_reported |-> [r \in RuntimeIdValues |-> FALSE],
        work_state |-> NoWorkStatusMap,
        work_runtime |-> NoWorkRuntimeMap,
        work_fence |-> ZeroWorkNatMap,
        accepted_reported |-> FalseWorkBoolMap,
        terminal_reported |-> FalseWorkBoolMap ]
  /\ seam =
      [ provision_mode |-> NoIdentityOptionMap,
        provision_runtime |-> NoIdentityRuntimeMap,
        provision_fence |-> ZeroIdentityNatMap,
        retire_pending |-> FalseIdentityBoolMap,
        retire_runtime |-> NoIdentityRuntimeMap,
        retire_fence |-> ZeroIdentityNatMap,
        destroy_pending |-> FalseIdentityBoolMap,
        destroy_runtime |-> NoIdentityRuntimeMap,
        destroy_fence |-> ZeroIdentityNatMap,
        submit_pending |-> FalseWorkBoolMap,
        submit_runtime |-> NoWorkRuntimeMap,
        submit_fence |-> ZeroWorkNatMap,
        cancel_pending |-> FalseWorkBoolMap,
        cancel_runtime |-> NoWorkRuntimeMap,
        cancel_fence |-> ZeroWorkNatMap,
        cancel_all_pending |-> FalseIdentityBoolMap,
        cancel_all_runtime |-> NoIdentityRuntimeMap,
        cancel_all_fence |-> ZeroIdentityNatMap,
        lifecycle_evt |-> NoLifecycleEventMap,
        lifecycle_evt_runtime |-> NoIdentityRuntimeMap,
        lifecycle_evt_fence |-> ZeroIdentityNatMap,
        work_evt |-> NoWorkEventMap,
        work_evt_runtime |-> NoWorkRuntimeMap,
        work_evt_fence |-> ZeroWorkNatMap ]
  /\ step_count = 0

KnownIdentity(id) == id \in mob.known
CurrentRuntime(id) == mob.runtime_id[id]
CurrentFence(id) == mob.fence_token[id]
HasCurrentRuntime(id) == CurrentRuntime(id) # None
FlowLive == mob.flow_active /\ mob.flow_status = "Running"
FlowStepBoundWork == IF mob.step_work = None THEN {} ELSE {mob.step_work.value}

FreeRuntimeIds ==
  {rt \in RuntimeIdValues : bridge.runtime_state[rt] = "Absent" /\ bridge.runtime_owner[rt] = None}

LiveAcceptedWorkForIdentity(id) ==
  {w \in mob.work_known :
    mob.work_owner[w] = Some(id) /\ mob.work_status[w] = "Accepted"}

LifecycleEventMatchesCurrent(id) ==
  /\ seam.lifecycle_evt_runtime[id] = mob.runtime_id[id]
  /\ seam.lifecycle_evt_fence[id] = mob.fence_token[id]

LifecycleEventStale(id) ==
  /\ seam.lifecycle_evt[id] # None
  /\ ~LifecycleEventMatchesCurrent(id)

WorkEventMatchesCurrent(w) ==
  /\ mob.work_owner[w] # None
  /\ seam.work_evt_runtime[w] = mob.work_runtime_target[w]
  /\ seam.work_evt_fence[w] = mob.fence_token[mob.work_owner[w].value]

WorkEventStale(w) ==
  /\ seam.work_evt[w] # None
  /\ mob.work_owner[w] # None
  /\ ~WorkEventMatchesCurrent(w)

SubmitTargetsReadyRuntime(w) ==
  /\ seam.submit_runtime[w] # None
  /\ LET rt == seam.submit_runtime[w].value IN
     /\ bridge.runtime_state[rt] = "Ready"
     /\ bridge.runtime_fence[rt] = seam.submit_fence[w]

BridgeReadyForCurrent(id) ==
  /\ mob.runtime_id[id] # None
  /\ LET rt == mob.runtime_id[id].value IN
     /\ bridge.runtime_state[rt] = "Ready"
     /\ bridge.runtime_owner[rt] = Some(id)
     /\ bridge.runtime_fence[rt] = mob.fence_token[id]

RegisterIdentity ==
  /\ mob.known # AgentIdentityValues
  /\ LET id == PickOne(AgentIdentityValues \ mob.known) IN
     /\ mob' =
         [mob EXCEPT
            !.known = @ \cup {id},
            !.member_status[id] = "Absent"]
     /\ UNCHANGED << bridge, seam >>
     /\ step_count' = step_count + 1

ProvisionFresh ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Absent"
       /\ seam.provision_mode[id] = None
       /\ FreeRuntimeIds # {}
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Absent" /\
                          seam.provision_mode[i] = None})
         rt == PickOne(FreeRuntimeIds)
         f == mob.fence_token[id] + 1
     IN
     /\ mob' =
         [mob EXCEPT
            !.runtime_id[id] = Some(rt),
            !.fence_token[id] = f,
            !.authority[id] = "Provisioning",
            !.member_status[id] = "Provisioning",
            !.retire_observed[id] = FALSE]
     /\ seam' =
         [seam EXCEPT
            !.provision_mode[id] = Some("Fresh"),
            !.provision_runtime[id] = Some(rt),
            !.provision_fence[id] = f]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

ProvisionRecover ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Absent"
       /\ seam.provision_mode[id] = None
       /\ bridge.recoverable[id]
       /\ mob.runtime_id[id] # None
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Absent" /\
                          seam.provision_mode[i] = None /\
                          bridge.recoverable[i] /\
                          mob.runtime_id[i] # None})
         f == mob.fence_token[id] + 1
     IN
     /\ mob' =
         [mob EXCEPT
            !.fence_token[id] = f,
            !.authority[id] = "Provisioning",
            !.member_status[id] = "Provisioning",
            !.retire_observed[id] = FALSE]
     /\ seam' =
         [seam EXCEPT
            !.provision_mode[id] = Some("Recover"),
            !.provision_runtime[id] = mob.runtime_id[id],
            !.provision_fence[id] = f]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

ResetMember ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Active"
       /\ LiveAcceptedWorkForIdentity(id) = {}
       /\ seam.provision_mode[id] = None
       /\ ~seam.retire_pending[id]
       /\ FreeRuntimeIds # {}
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Active" /\
                          LiveAcceptedWorkForIdentity(i) = {} /\
                          seam.provision_mode[i] = None /\
                          ~seam.retire_pending[i]})
         oldrt == mob.runtime_id[id]
         oldf == mob.fence_token[id]
         newrt == PickOne(FreeRuntimeIds)
         newf == oldf + 1
     IN
     /\ mob' =
         [mob EXCEPT
            !.generation[id] = @ + 1,
            !.runtime_id[id] = Some(newrt),
            !.fence_token[id] = newf,
            !.authority[id] = "Provisioning",
            !.member_status[id] = "Provisioning",
            !.retire_observed[id] = FALSE]
     /\ seam' =
         [seam EXCEPT
            !.provision_mode[id] = Some("Fresh"),
            !.provision_runtime[id] = Some(newrt),
            !.provision_fence[id] = newf,
            !.retire_pending[id] = TRUE,
            !.retire_runtime[id] = oldrt,
            !.retire_fence[id] = oldf]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

BridgeStartProvision ==
  /\ \E id \in AgentIdentityValues :
       /\ seam.provision_mode[id] # None
       /\ seam.provision_runtime[id] # None
       /\ bridge.runtime_state[seam.provision_runtime[id].value] = "Absent"
  /\ LET id == PickOne({i \in AgentIdentityValues :
                          seam.provision_mode[i] # None /\
                          seam.provision_runtime[i] # None /\
                          bridge.runtime_state[seam.provision_runtime[i].value] = "Absent"})
         rt == seam.provision_runtime[id].value
         f == seam.provision_fence[id]
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.runtime_state[rt] = "Provisioning",
            !.runtime_owner[rt] = Some(id),
            !.runtime_fence[rt] = f,
            !.ready_reported[rt] = FALSE,
            !.failed_reported[rt] = FALSE,
            !.retired_reported[rt] = FALSE,
            !.destroyed_reported[rt] = FALSE,
            !.recoverable[id] = FALSE]
     /\ seam' =
         [seam EXCEPT
            !.provision_mode[id] = None,
            !.provision_runtime[id] = None,
            !.provision_fence[id] = 0]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

BridgeEmitReady ==
  /\ \E rt \in RuntimeIdValues :
       /\ bridge.runtime_state[rt] = "Provisioning"
       /\ ~bridge.ready_reported[rt]
       /\ seam.lifecycle_evt[bridge.runtime_owner[rt].value] = None
  /\ LET rt == PickOne({r \in RuntimeIdValues :
                          bridge.runtime_state[r] = "Provisioning" /\
                          ~bridge.ready_reported[r] /\
                          seam.lifecycle_evt[bridge.runtime_owner[r].value] = None})
         id == bridge.runtime_owner[rt].value
         f == bridge.runtime_fence[rt]
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.runtime_state[rt] = "Ready",
            !.ready_reported[rt] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = Some("Ready"),
            !.lifecycle_evt_runtime[id] = Some(rt),
            !.lifecycle_evt_fence[id] = f]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

BridgeEmitProvisionFailed ==
  /\ \E rt \in RuntimeIdValues :
       /\ bridge.runtime_state[rt] = "Provisioning"
       /\ ~bridge.failed_reported[rt]
       /\ seam.lifecycle_evt[bridge.runtime_owner[rt].value] = None
  /\ LET rt == PickOne({r \in RuntimeIdValues :
                          bridge.runtime_state[r] = "Provisioning" /\
                          ~bridge.failed_reported[r] /\
                          seam.lifecycle_evt[bridge.runtime_owner[r].value] = None})
         id == bridge.runtime_owner[rt].value
         f == bridge.runtime_fence[rt]
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.runtime_state[rt] = "Destroyed",
            !.failed_reported[rt] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = Some("ProvisionFailed"),
            !.lifecycle_evt_runtime[id] = Some(rt),
            !.lifecycle_evt_fence[id] = f]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

MobConsumeReady ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Provisioning"
       /\ seam.lifecycle_evt[id] = Some("Ready")
       /\ LifecycleEventMatchesCurrent(id)
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Provisioning" /\
                          seam.lifecycle_evt[i] = Some("Ready") /\
                          LifecycleEventMatchesCurrent(i)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.authority[id] = "Active",
            !.member_status[id] = "Ready"]
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = None,
            !.lifecycle_evt_runtime[id] = None,
            !.lifecycle_evt_fence[id] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

MobConsumeProvisionFailed ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Provisioning"
       /\ seam.lifecycle_evt[id] = Some("ProvisionFailed")
       /\ LifecycleEventMatchesCurrent(id)
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Provisioning" /\
                          seam.lifecycle_evt[i] = Some("ProvisionFailed") /\
                          LifecycleEventMatchesCurrent(i)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.authority[id] = "Absent",
            !.member_status[id] = "ProvisionFailed",
            !.retire_observed[id] = FALSE]
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = None,
            !.lifecycle_evt_runtime[id] = None,
            !.lifecycle_evt_fence[id] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

MobIgnoreStaleLifecycleEvent ==
  /\ \E id \in AgentIdentityValues : LifecycleEventStale(id)
  /\ LET id == PickOne({i \in AgentIdentityValues : LifecycleEventStale(i)})
     IN
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = None,
            !.lifecycle_evt_runtime[id] = None,
            !.lifecycle_evt_fence[id] = 0]
     /\ UNCHANGED << mob, bridge >>
     /\ step_count' = step_count + 1

RetireMember ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Active"
       /\ ~seam.retire_pending[id]
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Active" /\
                          ~seam.retire_pending[i]})
     IN
     /\ mob' =
         [mob EXCEPT
            !.authority[id] = "Retiring",
            !.member_status[id] = "Retiring",
            !.retire_observed[id] = FALSE]
     /\ seam' =
         [seam EXCEPT
            !.retire_pending[id] = TRUE,
            !.retire_runtime[id] = mob.runtime_id[id],
            !.retire_fence[id] = mob.fence_token[id]]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

BridgeBeginRetire ==
  /\ \E id \in AgentIdentityValues :
       /\ seam.retire_pending[id]
       /\ seam.retire_runtime[id] # None
       /\ LET rt == seam.retire_runtime[id].value IN
          /\ bridge.runtime_owner[rt] = Some(id)
          /\ bridge.runtime_fence[rt] = seam.retire_fence[id]
          /\ bridge.runtime_state[rt] \in {"Ready", "Provisioning"}
  /\ LET id == PickOne({i \in AgentIdentityValues :
                          seam.retire_pending[i] /\
                          seam.retire_runtime[i] # None /\
                          LET rt == seam.retire_runtime[i].value IN
                            bridge.runtime_owner[rt] = Some(i) /\
                            bridge.runtime_fence[rt] = seam.retire_fence[i] /\
                            bridge.runtime_state[rt] \in {"Ready", "Provisioning"}})
         rt == seam.retire_runtime[id].value
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.runtime_state[rt] = "Retiring"]
     /\ seam' =
         [seam EXCEPT
            !.retire_pending[id] = FALSE,
            !.retire_runtime[id] = None,
            !.retire_fence[id] = 0]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

BridgeEmitRetired ==
  /\ \E rt \in RuntimeIdValues :
       /\ bridge.runtime_state[rt] = "Retiring"
       /\ ~bridge.retired_reported[rt]
       /\ seam.lifecycle_evt[bridge.runtime_owner[rt].value] = None
  /\ LET rt == PickOne({r \in RuntimeIdValues :
                          bridge.runtime_state[r] = "Retiring" /\
                          ~bridge.retired_reported[r] /\
                          seam.lifecycle_evt[bridge.runtime_owner[r].value] = None})
         id == bridge.runtime_owner[rt].value
         f == bridge.runtime_fence[rt]
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.retired_reported[rt] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = Some("Retired"),
            !.lifecycle_evt_runtime[id] = Some(rt),
            !.lifecycle_evt_fence[id] = f]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

MobConsumeRetired ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Retiring"
       /\ seam.lifecycle_evt[id] = Some("Retired")
       /\ LifecycleEventMatchesCurrent(id)
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Retiring" /\
                          seam.lifecycle_evt[i] = Some("Retired") /\
                          LifecycleEventMatchesCurrent(i)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.retire_observed[id] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = None,
            !.lifecycle_evt_runtime[id] = None,
            !.lifecycle_evt_fence[id] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

DestroyMember ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Retiring"
       /\ mob.retire_observed[id]
       /\ ~seam.destroy_pending[id]
       /\ mob.runtime_id[id] # None
       /\ bridge.runtime_state[mob.runtime_id[id].value] # "Destroyed"
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Retiring" /\
                          mob.retire_observed[i] /\
                          ~seam.destroy_pending[i] /\
                          mob.runtime_id[i] # None /\
                          bridge.runtime_state[mob.runtime_id[i].value] # "Destroyed"})
     IN
     /\ seam' =
         [seam EXCEPT
            !.destroy_pending[id] = TRUE,
            !.destroy_runtime[id] = mob.runtime_id[id],
            !.destroy_fence[id] = mob.fence_token[id]]
     /\ UNCHANGED << mob, bridge >>
     /\ step_count' = step_count + 1

BridgeBeginDestroy ==
  /\ \E id \in AgentIdentityValues :
       /\ seam.destroy_pending[id]
       /\ seam.destroy_runtime[id] # None
       /\ LET rt == seam.destroy_runtime[id].value IN
          /\ bridge.runtime_owner[rt] = Some(id)
          /\ bridge.runtime_fence[rt] = seam.destroy_fence[id]
          /\ bridge.runtime_state[rt] \in {"Retiring", "Ready"}
  /\ LET id == PickOne({i \in AgentIdentityValues :
                          seam.destroy_pending[i] /\
                          seam.destroy_runtime[i] # None /\
                          LET rt == seam.destroy_runtime[i].value IN
                            bridge.runtime_owner[rt] = Some(i) /\
                            bridge.runtime_fence[rt] = seam.destroy_fence[i] /\
                            bridge.runtime_state[rt] \in {"Retiring", "Ready"}})
         rt == seam.destroy_runtime[id].value
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.runtime_state[rt] = "Destroyed",
            !.recoverable[id] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.destroy_pending[id] = FALSE,
            !.destroy_runtime[id] = None,
            !.destroy_fence[id] = 0]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

BridgeEmitDestroyed ==
  /\ \E rt \in RuntimeIdValues :
       /\ bridge.runtime_state[rt] = "Destroyed"
       /\ ~bridge.destroyed_reported[rt]
       /\ seam.lifecycle_evt[bridge.runtime_owner[rt].value] = None
  /\ LET rt == PickOne({r \in RuntimeIdValues :
                          bridge.runtime_state[r] = "Destroyed" /\
                          ~bridge.destroyed_reported[r] /\
                          seam.lifecycle_evt[bridge.runtime_owner[r].value] = None})
         id == bridge.runtime_owner[rt].value
         f == bridge.runtime_fence[rt]
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.destroyed_reported[rt] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = Some("Destroyed"),
            !.lifecycle_evt_runtime[id] = Some(rt),
            !.lifecycle_evt_fence[id] = f]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

MobConsumeDestroyed ==
  /\ \E id \in mob.known :
       /\ mob.authority[id] = "Retiring"
       /\ seam.lifecycle_evt[id] = Some("Destroyed")
       /\ LifecycleEventMatchesCurrent(id)
  /\ LET id == PickOne({i \in mob.known :
                          mob.authority[i] = "Retiring" /\
                          seam.lifecycle_evt[i] = Some("Destroyed") /\
                          LifecycleEventMatchesCurrent(i)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.authority[id] = "Absent",
            !.member_status[id] = "Destroyed",
            !.retire_observed[id] = FALSE]
     /\ seam' =
         [seam EXCEPT
            !.lifecycle_evt[id] = None,
            !.lifecycle_evt_runtime[id] = None,
            !.lifecycle_evt_fence[id] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

BridgeCleanupDestroyed ==
  /\ \E rt \in RuntimeIdValues :
       /\ bridge.runtime_state[rt] = "Destroyed"
       /\ bridge.destroyed_reported[rt]
       /\ seam.lifecycle_evt[bridge.runtime_owner[rt].value] = None
  /\ LET rt == PickOne({r \in RuntimeIdValues :
                          bridge.runtime_state[r] = "Destroyed" /\
                          bridge.destroyed_reported[r] /\
                          seam.lifecycle_evt[bridge.runtime_owner[r].value] = None})
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.runtime_state[rt] = "Absent",
            !.runtime_owner[rt] = None,
            !.runtime_fence[rt] = 0,
            !.ready_reported[rt] = FALSE,
            !.failed_reported[rt] = FALSE,
            !.retired_reported[rt] = FALSE,
            !.destroyed_reported[rt] = FALSE]
     /\ UNCHANGED << mob, seam >>
     /\ step_count' = step_count + 1

StartFlowRun ==
  /\ ~mob.flow_active
  /\ \E id \in mob.known : mob.authority[id] = "Active"
  /\ LET id == PickOne({i \in mob.known : mob.authority[i] = "Active"})
     IN
     /\ mob' =
         [mob EXCEPT
            !.flow_active = TRUE,
            !.flow_status = "Running",
            !.step_status = "Pending",
            !.step_owner = Some(id),
            !.step_work = None]
     /\ UNCHANGED << bridge, seam >>
     /\ step_count' = step_count + 1

DispatchFlowStep ==
  /\ mob.flow_active
  /\ mob.flow_status = "Running"
  /\ mob.step_status = "Pending"
  /\ mob.step_owner # None
  /\ LET id == mob.step_owner.value IN
     /\ mob.authority[id] = "Active"
     /\ mob.member_status[id] = "Ready"
     /\ WorkRefValues \ mob.work_known # {}
  /\ LET id == mob.step_owner.value
         w == PickOne(WorkRefValues \ mob.work_known)
     IN
     /\ mob' =
         [mob EXCEPT
            !.work_known = @ \cup {w},
            !.work_owner[w] = Some(id),
            !.work_runtime_target[w] = mob.runtime_id[id],
            !.work_status[w] = "PendingDecision",
            !.step_status = "Dispatched",
            !.step_work = Some(w)]
     /\ seam' =
         [seam EXCEPT
            !.submit_pending[w] = TRUE,
            !.submit_runtime[w] = mob.runtime_id[id],
            !.submit_fence[w] = mob.fence_token[id]]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

BridgeReceiveSubmit ==
  /\ \E w \in WorkRefValues :
       /\ seam.submit_pending[w]
       /\ seam.submit_runtime[w] # None
       /\ LET rt == seam.submit_runtime[w].value IN
          /\ bridge.runtime_state[rt] = "Ready"
          /\ bridge.runtime_fence[rt] = seam.submit_fence[w]
  /\ LET w == PickOne({x \in WorkRefValues :
                         seam.submit_pending[x] /\
                         seam.submit_runtime[x] # None /\
                         LET rt == seam.submit_runtime[x].value IN
                           bridge.runtime_state[rt] = "Ready" /\
                           bridge.runtime_fence[rt] = seam.submit_fence[x]})
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.work_state[w] = "PendingDecision",
            !.work_runtime[w] = seam.submit_runtime[w],
            !.work_fence[w] = seam.submit_fence[w],
            !.accepted_reported[w] = FALSE,
            !.terminal_reported[w] = FALSE]
     /\ seam' =
         [seam EXCEPT
            !.submit_pending[w] = FALSE,
            !.submit_runtime[w] = None,
            !.submit_fence[w] = 0]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

BridgeRejectInvalidSubmit ==
  /\ \E w \in WorkRefValues :
       /\ seam.submit_pending[w]
       /\ seam.work_evt[w] = None
       /\ ~SubmitTargetsReadyRuntime(w)
  /\ LET w == PickOne({x \in WorkRefValues :
                         seam.submit_pending[x] /\
                         seam.work_evt[x] = None /\
                         ~SubmitTargetsReadyRuntime(x)})
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.work_state[w] = "Rejected",
            !.work_runtime[w] = seam.submit_runtime[w],
            !.work_fence[w] = seam.submit_fence[w],
            !.terminal_reported[w] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.submit_pending[w] = FALSE,
            !.submit_runtime[w] = None,
            !.submit_fence[w] = 0,
            !.work_evt[w] = Some("Rejected"),
            !.work_evt_runtime[w] = bridge.work_runtime[w],
            !.work_evt_fence[w] = bridge.work_fence[w]]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

BridgeEmitAccepted ==
  /\ \E w \in WorkRefValues :
       /\ bridge.work_state[w] = "PendingDecision"
       /\ ~bridge.accepted_reported[w]
       /\ seam.work_evt[w] = None
  /\ LET w == PickOne({x \in WorkRefValues :
                         bridge.work_state[x] = "PendingDecision" /\
                         ~bridge.accepted_reported[x] /\
                         seam.work_evt[x] = None})
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.work_state[w] = "Accepted",
            !.accepted_reported[w] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = Some("Accepted"),
            !.work_evt_runtime[w] = bridge.work_runtime[w],
            !.work_evt_fence[w] = bridge.work_fence[w]]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

BridgeEmitCompleted ==
  /\ \E w \in WorkRefValues :
       /\ bridge.work_state[w] = "Accepted"
       /\ ~bridge.terminal_reported[w]
       /\ seam.work_evt[w] = None
  /\ LET w == PickOne({x \in WorkRefValues :
                         bridge.work_state[x] = "Accepted" /\
                         ~bridge.terminal_reported[x] /\
                         seam.work_evt[x] = None})
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.work_state[w] = "Completed",
            !.terminal_reported[w] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = Some("Completed"),
            !.work_evt_runtime[w] = bridge.work_runtime[w],
            !.work_evt_fence[w] = bridge.work_fence[w]]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

BridgeEmitFailed ==
  /\ \E w \in WorkRefValues :
       /\ bridge.work_state[w] = "Accepted"
       /\ ~bridge.terminal_reported[w]
       /\ seam.work_evt[w] = None
  /\ LET w == PickOne({x \in WorkRefValues :
                         bridge.work_state[x] = "Accepted" /\
                         ~bridge.terminal_reported[x] /\
                         seam.work_evt[x] = None})
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.work_state[w] = "Failed",
            !.terminal_reported[w] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = Some("Failed"),
            !.work_evt_runtime[w] = bridge.work_runtime[w],
            !.work_evt_fence[w] = bridge.work_fence[w]]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

MobConsumeAccepted ==
  /\ \E w \in mob.work_known :
       /\ mob.work_status[w] = "PendingDecision"
       /\ seam.work_evt[w] = Some("Accepted")
       /\ WorkEventMatchesCurrent(w)
  /\ LET w == PickOne({x \in mob.work_known :
                         mob.work_status[x] = "PendingDecision" /\
                         seam.work_evt[x] = Some("Accepted") /\
                         WorkEventMatchesCurrent(x)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.work_status[w] = "Accepted"]
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = None,
            !.work_evt_runtime[w] = None,
            !.work_evt_fence[w] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

MobConsumeRejected ==
  /\ \E w \in mob.work_known :
       /\ mob.work_status[w] = "PendingDecision"
       /\ seam.work_evt[w] = Some("Rejected")
       /\ WorkEventMatchesCurrent(w)
  /\ LET w == PickOne({x \in mob.work_known :
                         mob.work_status[x] = "PendingDecision" /\
                         seam.work_evt[x] = Some("Rejected") /\
                         WorkEventMatchesCurrent(x)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.work_status[w] = "Rejected",
            !.flow_status = "Failed",
            !.step_status = "Failed"]
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = None,
            !.work_evt_runtime[w] = None,
            !.work_evt_fence[w] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

MobConsumeCompleted ==
  /\ \E w \in mob.work_known :
       /\ mob.work_status[w] = "Accepted"
       /\ seam.work_evt[w] = Some("Completed")
       /\ WorkEventMatchesCurrent(w)
  /\ LET w == PickOne({x \in mob.work_known :
                         mob.work_status[x] = "Accepted" /\
                         seam.work_evt[x] = Some("Completed") /\
                         WorkEventMatchesCurrent(x)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.work_status[w] = "Completed",
            !.flow_status = "Completed",
            !.step_status = "Completed"]
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = None,
            !.work_evt_runtime[w] = None,
            !.work_evt_fence[w] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

MobConsumeFailed ==
  /\ \E w \in mob.work_known :
       /\ mob.work_status[w] = "Accepted"
       /\ seam.work_evt[w] = Some("Failed")
       /\ WorkEventMatchesCurrent(w)
  /\ LET w == PickOne({x \in mob.work_known :
                         mob.work_status[x] = "Accepted" /\
                         seam.work_evt[x] = Some("Failed") /\
                         WorkEventMatchesCurrent(x)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.work_status[w] = "Failed",
            !.flow_status = "Failed",
            !.step_status = "Failed"]
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = None,
            !.work_evt_runtime[w] = None,
            !.work_evt_fence[w] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

MobCancelWork ==
  /\ \E w \in mob.work_known :
       /\ mob.work_status[w] = "Accepted"
       /\ ~seam.cancel_pending[w]
  /\ LET w == PickOne({x \in mob.work_known :
                         mob.work_status[x] = "Accepted" /\
                         ~seam.cancel_pending[x]})
     IN
     /\ seam' =
         [seam EXCEPT
            !.cancel_pending[w] = TRUE,
            !.cancel_runtime[w] = mob.work_runtime_target[w],
            !.cancel_fence[w] =
              mob.fence_token[mob.work_owner[w].value]]
     /\ UNCHANGED << mob, bridge >>
     /\ step_count' = step_count + 1

BridgeApplyCancel ==
  /\ \E w \in WorkRefValues :
       /\ seam.cancel_pending[w]
       /\ seam.cancel_runtime[w] # None
       /\ bridge.work_state[w] = "Accepted"
       /\ bridge.work_runtime[w] = seam.cancel_runtime[w]
       /\ bridge.work_fence[w] = seam.cancel_fence[w]
       /\ seam.work_evt[w] = None
  /\ LET w == PickOne({x \in WorkRefValues :
                         seam.cancel_pending[x] /\
                         seam.cancel_runtime[x] # None /\
                         bridge.work_state[x] = "Accepted" /\
                         bridge.work_runtime[x] = seam.cancel_runtime[x] /\
                         bridge.work_fence[x] = seam.cancel_fence[x] /\
                         seam.work_evt[x] = None})
     IN
     /\ bridge' =
         [bridge EXCEPT
            !.work_state[w] = "Canceled",
            !.terminal_reported[w] = TRUE]
     /\ seam' =
         [seam EXCEPT
            !.cancel_pending[w] = FALSE,
            !.cancel_runtime[w] = None,
            !.cancel_fence[w] = 0,
            !.work_evt[w] = Some("Canceled"),
            !.work_evt_runtime[w] = bridge.work_runtime[w],
            !.work_evt_fence[w] = bridge.work_fence[w]]
     /\ UNCHANGED mob
     /\ step_count' = step_count + 1

MobConsumeCanceled ==
  /\ \E w \in mob.work_known :
       /\ mob.work_status[w] = "Accepted"
       /\ seam.work_evt[w] = Some("Canceled")
       /\ WorkEventMatchesCurrent(w)
  /\ LET w == PickOne({x \in mob.work_known :
                         mob.work_status[x] = "Accepted" /\
                         seam.work_evt[x] = Some("Canceled") /\
                         WorkEventMatchesCurrent(x)})
     IN
     /\ mob' =
         [mob EXCEPT
            !.work_status[w] = "Canceled",
            !.flow_status = "Canceled",
            !.step_status = "Canceled"]
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = None,
            !.work_evt_runtime[w] = None,
            !.work_evt_fence[w] = 0]
     /\ UNCHANGED bridge
     /\ step_count' = step_count + 1

MobIgnoreStaleWorkEvent ==
  /\ \E w \in mob.work_known : WorkEventStale(w)
  /\ LET w == PickOne({x \in mob.work_known : WorkEventStale(x)})
     IN
     /\ seam' =
         [seam EXCEPT
            !.work_evt[w] = None,
            !.work_evt_runtime[w] = None,
            !.work_evt_fence[w] = 0]
     /\ UNCHANGED << mob, bridge >>
     /\ step_count' = step_count + 1

MachineAction ==
  \/ RegisterIdentity
  \/ ProvisionFresh
  \/ ProvisionRecover
  \/ ResetMember
  \/ BridgeStartProvision
  \/ BridgeEmitReady
  \/ BridgeEmitProvisionFailed
  \/ MobConsumeReady
  \/ MobConsumeProvisionFailed
  \/ MobIgnoreStaleLifecycleEvent
  \/ RetireMember
  \/ BridgeBeginRetire
  \/ BridgeEmitRetired
  \/ MobConsumeRetired
  \/ DestroyMember
  \/ BridgeBeginDestroy
  \/ BridgeEmitDestroyed
  \/ MobConsumeDestroyed
  \/ BridgeCleanupDestroyed
  \/ StartFlowRun
  \/ DispatchFlowStep
  \/ BridgeReceiveSubmit
  \/ BridgeRejectInvalidSubmit
  \/ BridgeEmitAccepted
  \/ BridgeEmitCompleted
  \/ BridgeEmitFailed
  \/ MobConsumeAccepted
  \/ MobConsumeRejected
  \/ MobConsumeCompleted
  \/ MobConsumeFailed
  \/ MobCancelWork
  \/ BridgeApplyCancel
  \/ MobConsumeCanceled
  \/ MobIgnoreStaleWorkEvent

Next ==
  \/ /\ step_count <= MaxSteps
     /\ MachineAction
  \/ UNCHANGED vars

TypeInvariant ==
  /\ mob \in MobType
  /\ bridge \in BridgeType
  /\ seam \in SeamType
  /\ step_count \in Nat

ActiveMemberBackedByReadyRuntimeInvariant ==
  \A id \in mob.known :
    mob.member_status[id] = "Ready" => BridgeReadyForCurrent(id)

ProvisioningCommandInvariant ==
  \A id \in mob.known :
    mob.authority[id] = "Provisioning" =>
      /\ seam.provision_mode[id] # None
         \/ BridgeReadyForCurrent(id)
         \/ seam.lifecycle_evt[id] # None
         \/ (mob.runtime_id[id] # None
             /\ LET rt == mob.runtime_id[id].value IN
                /\ bridge.runtime_state[rt] = "Provisioning"
                /\ bridge.runtime_owner[rt] = Some(id)
                /\ bridge.runtime_fence[rt] = mob.fence_token[id])
      /\ mob.runtime_id[id] # None

RetireObservedOnlyForRetiringInvariant ==
  \A id \in mob.known :
    mob.retire_observed[id] => mob.authority[id] = "Retiring"

DispatchedStepHasBoundWorkInvariant ==
  mob.step_status = "Dispatched" =>
    /\ mob.step_work # None
    /\ mob.step_work.value \in mob.work_known
    /\ mob.work_status[mob.step_work.value] \in {"PendingDecision", "Accepted"}

AcceptedWorkUsesCurrentRuntimeInvariant ==
  \A w \in mob.work_known :
    mob.work_status[w] = "Accepted" =>
      /\ mob.work_owner[w] # None
      /\ mob.work_runtime_target[w] = mob.runtime_id[mob.work_owner[w].value]
      /\ bridge.work_state[w] \in {"Accepted", "Completed", "Failed", "Canceled"}

TerminalFlowMatchesStepInvariant ==
  /\ mob.flow_status = "Completed" => mob.step_status = "Completed"
  /\ mob.flow_status = "Failed" => mob.step_status = "Failed"
  /\ mob.flow_status = "Canceled" => mob.step_status = "Canceled"

PendingSubmitMatchesBoundWorkInvariant ==
  \A w \in WorkRefValues :
    seam.submit_pending[w] =>
      /\ mob.work_owner[w] # None
      /\ seam.submit_runtime[w] = mob.work_runtime_target[w]
      /\ seam.submit_runtime[w] # None
      /\ seam.submit_fence[w] > 0
      /\ LET rt == seam.submit_runtime[w].value
             id == mob.work_owner[w].value
         IN
         /\ bridge.runtime_owner[rt] = None
            \/ /\ bridge.runtime_owner[rt] = Some(id)
               /\ bridge.runtime_fence[rt] = seam.submit_fence[w]

NoDestroyOnNonRetiredMemberInvariant ==
  \A id \in mob.known :
    seam.destroy_pending[id] =>
      /\ mob.authority[id] = "Retiring"
      /\ mob.retire_observed[id]

Invariants ==
  /\ TypeInvariant
  /\ ActiveMemberBackedByReadyRuntimeInvariant
  /\ ProvisioningCommandInvariant
  /\ RetireObservedOnlyForRetiringInvariant
  /\ DispatchedStepHasBoundWorkInvariant
  /\ AcceptedWorkUsesCurrentRuntimeInvariant
  /\ TerminalFlowMatchesStepInvariant
  /\ PendingSubmitMatchesBoundWorkInvariant
  /\ NoDestroyOnNonRetiredMemberInvariant

StateConstraint == step_count <= MaxSteps

Spec == Init /\ [][Next]_vars

LifecycleHarnessNext ==
  \/ RegisterIdentity
  \/ ProvisionFresh
  \/ ResetMember
  \/ BridgeStartProvision
  \/ BridgeEmitReady
  \/ BridgeEmitProvisionFailed
  \/ MobConsumeReady
  \/ MobConsumeProvisionFailed
  \/ MobIgnoreStaleLifecycleEvent
  \/ RetireMember
  \/ BridgeBeginRetire
  \/ BridgeEmitRetired
  \/ MobConsumeRetired
  \/ DestroyMember
  \/ BridgeBeginDestroy
  \/ BridgeEmitDestroyed
  \/ MobConsumeDestroyed
  \/ BridgeCleanupDestroyed

LifecycleSpec == Init /\ [][LifecycleHarnessNext]_vars

ProvisioningEventuallyLeavesProvisioningProp ==
  \A id \in AgentIdentityValues :
    mob.authority[id] = "Provisioning" ~> mob.authority[id] # "Provisioning"

StaleLifecycleEventEventuallyClearsProp ==
  \A id \in AgentIdentityValues :
    LifecycleEventStale(id) ~> seam.lifecycle_evt[id] = None

RetireObservedEventuallyLeavesRetiringProp ==
  \A id \in AgentIdentityValues :
    mob.retire_observed[id] ~> mob.member_status[id] = "Destroyed"

LifecycleFairness ==
  /\ WF_vars(RegisterIdentity)
  /\ WF_vars(ProvisionFresh)
  /\ WF_vars(ResetMember)
  /\ WF_vars(BridgeStartProvision)
  /\ WF_vars(BridgeEmitReady)
  /\ WF_vars(BridgeEmitProvisionFailed)
  /\ WF_vars(MobConsumeReady)
  /\ WF_vars(MobConsumeProvisionFailed)
  /\ WF_vars(MobIgnoreStaleLifecycleEvent)
  /\ WF_vars(RetireMember)
  /\ WF_vars(BridgeBeginRetire)
  /\ WF_vars(BridgeEmitRetired)
  /\ WF_vars(MobConsumeRetired)
  /\ WF_vars(DestroyMember)
  /\ WF_vars(BridgeBeginDestroy)
  /\ WF_vars(BridgeEmitDestroyed)
  /\ WF_vars(MobConsumeDestroyed)
  /\ WF_vars(BridgeCleanupDestroyed)

LifecycleFairSpec == LifecycleSpec /\ LifecycleFairness

FlowWorkHarnessNext ==
  \/ RegisterIdentity
  \/ ProvisionFresh
  \/ BridgeStartProvision
  \/ BridgeEmitReady
  \/ MobConsumeReady
  \/ StartFlowRun
  \/ DispatchFlowStep
  \/ BridgeReceiveSubmit
  \/ BridgeRejectInvalidSubmit
  \/ BridgeEmitAccepted
  \/ BridgeEmitCompleted
  \/ BridgeEmitFailed
  \/ MobConsumeAccepted
  \/ MobConsumeRejected
  \/ MobConsumeCompleted
  \/ MobConsumeFailed
  \/ MobCancelWork
  \/ BridgeApplyCancel
  \/ MobConsumeCanceled
  \/ MobIgnoreStaleWorkEvent

FlowWorkSpec == Init /\ [][FlowWorkHarnessNext]_vars

SubmittedWorkEventuallyLeavesPendingDecisionProp ==
  \A w \in WorkRefValues :
    mob.work_status[w] = "PendingDecision" ~> mob.work_status[w] # "PendingDecision"

AcceptedWorkEventuallyTerminalizesProp ==
  \A w \in WorkRefValues :
    mob.work_status[w] = "Accepted" ~>
      mob.work_status[w] \in {"Completed", "Failed", "Canceled"}

DispatchedStepEventuallyLeavesDispatchedProp ==
  mob.step_status = "Dispatched" ~> mob.step_status # "Dispatched"

FlowWorkFairness ==
  /\ WF_vars(RegisterIdentity)
  /\ WF_vars(ProvisionFresh)
  /\ WF_vars(BridgeStartProvision)
  /\ WF_vars(BridgeEmitReady)
  /\ WF_vars(MobConsumeReady)
  /\ WF_vars(StartFlowRun)
  /\ WF_vars(DispatchFlowStep)
  /\ WF_vars(BridgeReceiveSubmit)
  /\ WF_vars(BridgeRejectInvalidSubmit)
  /\ WF_vars(BridgeEmitAccepted)
  /\ WF_vars(BridgeEmitCompleted)
  /\ WF_vars(BridgeEmitFailed)
  /\ WF_vars(MobConsumeAccepted)
  /\ WF_vars(MobConsumeRejected)
  /\ WF_vars(MobConsumeCompleted)
  /\ WF_vars(MobConsumeFailed)
  /\ WF_vars(MobCancelWork)
  /\ WF_vars(BridgeApplyCancel)
  /\ WF_vars(MobConsumeCanceled)
  /\ WF_vars(MobIgnoreStaleWorkEvent)

FlowWorkFairSpec == FlowWorkSpec /\ FlowWorkFairness

====
