---- MODULE MobMachineTarget ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Experimental target-state model for the frozen MobMachine.

CONSTANTS
  AgentIdentityValues,
  AgentRuntimeIdValues,
  FlowIdValues,
  RunIdValues,
  StepIdValues,
  FrameIdValues,
  LoopIdValues,
  LoopInstanceIdValues,
  BranchIdValues,
  WorkRefValues,
  TaskIdValues,
  ProfileIdValues,
  RuntimeModeValues,
  RestoreReasonValues,
  FlowSingle,
  FlowTwoStep,
  FlowBranchFallback,
  FlowQuorum,
  FlowLoop,
  FlowRetry,
  FlowSupervisor,
  StepStart,
  StepFirst,
  StepSecond,
  StepLeft,
  StepRight,
  StepJoin,
  StepCollect,
  StepBody,
  StepFollow,
  BranchPrimary,
  BranchSecondary,
  MaxSteps

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]
OptionSet(S) == {None} \cup {Some(x) : x \in S}

MobPhaseValues == {"Creating", "Running", "Stopped", "Completed", "Destroyed"}
AuthorityStateValues == {"Absent", "Provisioning", "Active", "Retiring", "Destroyed"}
MemberStatusValues == {"Pending", "Active", "Retiring", "Broken", "Completed", "Destroyed"}
WorkStatusValues ==
  {"Pending", "Accepted", "Running", "Rejected", "Completed", "Failed", "Canceled"}
WorkTerminalOutcomeValues ==
  {"WorkRejected", "WorkCompleted", "WorkFailed", "WorkCanceled"}
RunStatusValues == {"Pending", "Running", "Completed", "Failed", "Canceled"}
StepStatusValues == {"Pending", "Dispatched", "Completed", "Failed", "Skipped", "Canceled"}
DispatchModeValues == {"FanOut", "OneToOne", "FanIn"}
DispatchWidthValues == 1..2
DependencyModeValues == {"All", "Any"}
CollectionPolicyKindValues == {"All", "Any", "Quorum"}
TaskStatusValues == {"Pending", "InProgress", "Blocked", "Completed", "Canceled"}

OptionRuntimeIdValues == OptionSet(AgentRuntimeIdValues)
OptionFlowIdValues == OptionSet(FlowIdValues)
OptionRunIdValues == OptionSet(RunIdValues)
OptionStepIdValues == OptionSet(StepIdValues)
OptionBranchIdValues == OptionSet(BranchIdValues)
OptionProfileIdValues == OptionSet(ProfileIdValues)
OptionRuntimeModeValues == OptionSet(RuntimeModeValues)
OptionWorkTerminalValues == OptionSet(WorkTerminalOutcomeValues)

NoIdentityOptionMap == [i \in AgentIdentityValues |-> None]
ZeroIdentityNatMap == [i \in AgentIdentityValues |-> 0]
FalseIdentityBoolMap == [i \in AgentIdentityValues |-> FALSE]
NoWorkIdentityMap == [w \in WorkRefValues |-> None]
NoWorkRuntimeMap == [w \in WorkRefValues |-> None]
NoWorkRunMap == [w \in WorkRefValues |-> None]
NoWorkStepMap == [w \in WorkRefValues |-> None]
NoWorkTerminalMap == [w \in WorkRefValues |-> None]
NoRunFlowMap == [r \in RunIdValues |-> None]
NoRunStatusMap == [r \in RunIdValues |-> None]
ZeroRunNatMap == [r \in RunIdValues |-> 0]
FalseRunBoolMap == [r \in RunIdValues |-> FALSE]
EmptyRunSeqMap == [r \in RunIdValues |-> <<>>]
NoTaskIdentityMap == [t \in TaskIdValues |-> None]
NoTaskRunMap == [t \in TaskIdValues |-> None]
ZeroTaskStatusMap == [t \in TaskIdValues |-> "Pending"]

DefaultStepDependencies == [s \in StepIdValues |-> {}]
DefaultStepDispatchModes == [s \in StepIdValues |-> "FanOut"]
DefaultStepDispatchWidths == [s \in StepIdValues |-> 0]
DefaultStepModes == [s \in StepIdValues |-> "All"]
DefaultStepConditions == [s \in StepIdValues |-> FALSE]
DefaultStepBranches == [s \in StepIdValues |-> None]
DefaultStepPolicies == [s \in StepIdValues |-> "All"]
DefaultStepThresholds == [s \in StepIdValues |-> 0]
DefaultStepObserved == [s \in StepIdValues |-> 0]
DefaultStepStatuses == [s \in StepIdValues |-> None]

NoRunDependencyMap == [r \in RunIdValues |-> DefaultStepDependencies]
NoRunDispatchModeMap == [r \in RunIdValues |-> DefaultStepDispatchModes]
NoRunDispatchWidthMap == [r \in RunIdValues |-> DefaultStepDispatchWidths]
NoRunDependencyModeMap == [r \in RunIdValues |-> DefaultStepModes]
NoRunConditionMap == [r \in RunIdValues |-> DefaultStepConditions]
NoRunBranchMap == [r \in RunIdValues |-> DefaultStepBranches]
NoRunPolicyMap == [r \in RunIdValues |-> DefaultStepPolicies]
NoRunThresholdMap == [r \in RunIdValues |-> DefaultStepThresholds]
NoRunObservedMap == [r \in RunIdValues |-> DefaultStepObserved]
NoRunStepStatusMap == [r \in RunIdValues |-> DefaultStepStatuses]

NoEffects ==
  [ provision_requests |-> {},
    retire_requests |-> {},
    destroy_requests |-> {},
    work_submissions |-> {},
    work_cancellations |-> {},
    bulk_work_cancellations |-> {},
    published_events |-> 0,
    persisted_runs |-> {},
    persisted_roster |-> FALSE,
    persisted_history |-> FALSE,
    persisted_tasks |-> FALSE ]

RECURSIVE SeqRemoveOne(_, _)
SeqRemoveOne(seq, value) ==
  IF Len(seq) = 0 THEN
    <<>>
  ELSE IF Head(seq) = value THEN
    Tail(seq)
  ELSE
    <<Head(seq)>> \o SeqRemoveOne(Tail(seq), value)

SeqToSet(seq) == {seq[i] : i \in 1..Len(seq)}
AppendUnique(seq, value) == IF value \in SeqToSet(seq) THEN seq ELSE Append(seq, value)
PickOne(S) == CHOOSE x \in S : TRUE
PickSubsetOfSize(S, n) == CHOOSE T \in SUBSET S : Cardinality(T) = n
Present(opt) == opt # None
CardinalityOf(S) == Cardinality(S)
Inc(n) == IF n < MaxSteps THEN n + 1 ELSE MaxSteps

KnownFlowIds ==
  {FlowSingle, FlowTwoStep, FlowBranchFallback, FlowQuorum, FlowLoop, FlowRetry, FlowSupervisor}

FlowOrdered(flow) ==
  IF flow = FlowSingle THEN
    <<StepStart>>
  ELSE IF flow = FlowTwoStep THEN
    <<StepFirst, StepSecond>>
  ELSE IF flow = FlowBranchFallback THEN
    <<StepStart, StepLeft, StepRight, StepJoin>>
  ELSE IF flow = FlowQuorum THEN
    <<StepCollect>>
  ELSE IF flow = FlowLoop THEN
    <<StepBody>>
  ELSE IF flow = FlowRetry THEN
    <<StepStart>>
  ELSE IF flow = FlowSupervisor THEN
    <<StepStart>>
  ELSE
    <<StepStart>>

FlowDependencyMap(flow) ==
  [s \in StepIdValues |->
    IF flow = FlowTwoStep /\ s = StepSecond THEN
      {StepFirst}
    ELSE IF flow = FlowBranchFallback /\ s \in {StepLeft, StepRight} THEN
      {StepStart}
    ELSE IF flow = FlowBranchFallback /\ s = StepJoin THEN
      {StepLeft, StepRight}
    ELSE
      {}]

FlowDependencyModeMap(flow) ==
  [s \in StepIdValues |->
    IF flow = FlowBranchFallback /\ s = StepJoin THEN "Any" ELSE "All"]

FlowConditionMap(flow) ==
  [s \in StepIdValues |->
    IF flow = FlowBranchFallback /\ s \in {StepLeft, StepRight} THEN TRUE ELSE FALSE]

FlowBranchMap(flow) ==
  [s \in StepIdValues |->
    IF flow = FlowBranchFallback /\ s = StepLeft THEN Some(BranchPrimary)
    ELSE IF flow = FlowBranchFallback /\ s = StepRight THEN Some(BranchSecondary)
    ELSE None]

FlowCollectionPolicyMap(flow) ==
  [s \in StepIdValues |->
    IF flow = FlowQuorum /\ s = StepCollect THEN "Quorum"
    ELSE "All"]

FlowQuorumThresholdMap(flow) ==
  [s \in StepIdValues |->
    IF flow = FlowQuorum /\ s = StepCollect THEN 2 ELSE 0]

FlowDispatchModeAssignments(flow) ==
  { [s \in StepIdValues |->
        IF s \in SeqToSet(FlowOrdered(flow)) THEN assignment[s] ELSE "FanOut"] :
      assignment \in [SeqToSet(FlowOrdered(flow)) -> DispatchModeValues] }

FlowDispatchWidthAssignmentOk(flow, dispatchModeMap, width) ==
  \A s \in SeqToSet(FlowOrdered(flow)) :
    /\ width[s] >= 1
    /\ IF dispatchModeMap[s] = "OneToOne" THEN width[s] = 1 ELSE width[s] <= 2
    /\ IF FlowCollectionPolicyMap(flow)[s] = "Quorum" THEN
         width[s] >= FlowQuorumThresholdMap(flow)[s]
       ELSE TRUE

FlowDispatchWidthAssignments(flow, dispatchModeMap) ==
  LET steps == SeqToSet(FlowOrdered(flow))
      validWidths ==
        { width \in [steps -> DispatchWidthValues] :
            FlowDispatchWidthAssignmentOk(flow, dispatchModeMap, width) }
  IN { [s \in StepIdValues |->
           IF s \in steps THEN width[s] ELSE 0] :
         width \in validWidths }

FlowMaxStepRetries(flow) == IF flow = FlowRetry THEN 2 ELSE 0
FlowEscalationThreshold(flow) == IF flow = FlowSupervisor THEN 3 ELSE 0

FlowFrameCount(flow) == IF flow = FlowLoop THEN 1 ELSE 0
FlowLoopCount(flow) == IF flow = FlowLoop THEN 1 ELSE 0
FlowLoopIterationCount(flow) == IF flow = FlowLoop THEN 1 ELSE 0

VARIABLES
  identity,
  lifecycle,
  roster,
  topology,
  provisioning,
  work,
  flows,
  tasks,
  recovery,
  history,
  effects,
  step_count

vars ==
  << identity, lifecycle, roster, topology, provisioning, work, flows, tasks, recovery, history, effects, step_count >>

IdentityType ==
  [ known : SUBSET AgentIdentityValues,
    generation : [AgentIdentityValues -> Nat],
    runtime_id : [AgentIdentityValues -> OptionRuntimeIdValues],
    fence_token : [AgentIdentityValues -> Nat],
    authority : [AgentIdentityValues -> AuthorityStateValues],
    binding_present : [AgentIdentityValues -> BOOLEAN],
    checkpoint_version : [AgentIdentityValues -> Nat] ]

LifecycleType ==
  [ phase : MobPhaseValues,
    active_run_count : Nat,
    cleanup_pending : BOOLEAN ]

RosterType ==
  [ present : SUBSET AgentIdentityValues,
    profile : [AgentIdentityValues -> OptionProfileIdValues],
    runtime_mode : [AgentIdentityValues -> OptionRuntimeModeValues],
    labels_present : [AgentIdentityValues -> BOOLEAN],
    member_status : [AgentIdentityValues -> MemberStatusValues],
    runtime_binding_live : [AgentIdentityValues -> BOOLEAN] ]

TopologyType ==
  [ coordinator : OptionSet(AgentIdentityValues),
    revision : Nat,
    edges : SUBSET (AgentIdentityValues \X AgentIdentityValues),
    external_specs_present : SUBSET AgentIdentityValues ]

ProvisioningType ==
  [ pending_spawn : SUBSET AgentIdentityValues,
    pending_runtime_id : [AgentIdentityValues -> OptionRuntimeIdValues],
    kickoff_pending : SUBSET AgentIdentityValues,
    restore_failed : SUBSET AgentIdentityValues,
    restore_reason_present : [AgentIdentityValues -> BOOLEAN] ]

WorkType ==
  [ known : SUBSET WorkRefValues,
    owner : [WorkRefValues -> OptionSet(AgentIdentityValues)],
    runtime_target : [WorkRefValues -> OptionRuntimeIdValues],
    run_target : [WorkRefValues -> OptionRunIdValues],
    step_target : [WorkRefValues -> OptionStepIdValues],
    status : [WorkRefValues -> WorkStatusValues],
    terminal : [WorkRefValues -> OptionWorkTerminalValues] ]

FlowsType ==
  [ known_runs : SUBSET RunIdValues,
    tracked_runs : SUBSET RunIdValues,
    cancel_tokens : SUBSET RunIdValues,
    stream_runs : SUBSET RunIdValues,
    flow_id : [RunIdValues -> OptionFlowIdValues],
    status : [RunIdValues -> OptionSet(RunStatusValues)],
    schema_version : [RunIdValues -> Nat],
    completed_at_present : [RunIdValues -> BOOLEAN],
    frame_count : [RunIdValues -> Nat],
    loop_count : [RunIdValues -> Nat],
    loop_iteration_count : [RunIdValues -> Nat],
    ordered_steps : [RunIdValues -> Seq(StepIdValues)],
    step_dependencies : [RunIdValues -> [StepIdValues -> SUBSET StepIdValues]],
    step_dispatch_mode : [RunIdValues -> [StepIdValues -> DispatchModeValues]],
    step_dispatch_width : [RunIdValues -> [StepIdValues -> Nat]],
    step_dependency_mode : [RunIdValues -> [StepIdValues -> DependencyModeValues]],
    step_has_condition : [RunIdValues -> [StepIdValues -> BOOLEAN]],
    step_branch : [RunIdValues -> [StepIdValues -> OptionBranchIdValues]],
    step_collection_policy : [RunIdValues -> [StepIdValues -> CollectionPolicyKindValues]],
    step_quorum_threshold : [RunIdValues -> [StepIdValues -> Nat]],
    step_collection_observed : [RunIdValues -> [StepIdValues -> Nat]],
    step_status : [RunIdValues -> [StepIdValues -> OptionSet(StepStatusValues)]],
    failure_count : [RunIdValues -> Nat],
    consecutive_failure_count : [RunIdValues -> Nat],
    max_step_retries : [RunIdValues -> Nat],
    escalation_threshold : [RunIdValues -> Nat] ]

TasksType ==
  [ known : SUBSET TaskIdValues,
    status : [TaskIdValues -> TaskStatusValues],
    owner : [TaskIdValues -> OptionSet(AgentIdentityValues)],
    run_binding : [TaskIdValues -> OptionRunIdValues] ]

RecoveryType ==
  [ restore_failures : SUBSET AgentIdentityValues,
    checkpoint_version : [AgentIdentityValues -> Nat],
    continuity_bound : [AgentIdentityValues -> BOOLEAN] ]

HistoryType ==
  [ next_seq : Nat,
    event_count_by_identity : [AgentIdentityValues -> Nat],
    last_seq_by_identity : [AgentIdentityValues -> Nat] ]

EffectsType ==
  [ provision_requests : SUBSET AgentIdentityValues,
    retire_requests : SUBSET AgentIdentityValues,
    destroy_requests : SUBSET AgentIdentityValues,
    work_submissions : SUBSET WorkRefValues,
    work_cancellations : SUBSET WorkRefValues,
    bulk_work_cancellations : SUBSET AgentIdentityValues,
    published_events : Nat,
    persisted_runs : SUBSET RunIdValues,
    persisted_roster : BOOLEAN,
    persisted_history : BOOLEAN,
    persisted_tasks : BOOLEAN ]

KnownSteps(run_id) == SeqToSet(flows.ordered_steps[run_id])
TerminalRunStatuses == {"Completed", "Failed", "Canceled"}
TerminalWorkStatuses == {"Rejected", "Completed", "Failed", "Canceled"}
LiveWorkStatuses == {"Pending", "Accepted", "Running"}
ActiveAuthorities == {"Active", "Retiring"}
LiveMemberStatuses == {"Pending", "Active", "Retiring", "Broken"}

TypeOK ==
  /\ identity \in IdentityType
  /\ lifecycle \in LifecycleType
  /\ roster \in RosterType
  /\ topology \in TopologyType
  /\ provisioning \in ProvisioningType
  /\ work \in WorkType
  /\ flows \in FlowsType
  /\ tasks \in TasksType
  /\ recovery \in RecoveryType
  /\ history \in HistoryType
  /\ effects \in EffectsType
  /\ step_count \in 0..MaxSteps
  /\ roster.present \subseteq identity.known
  /\ provisioning.pending_spawn \subseteq identity.known
  /\ provisioning.kickoff_pending \subseteq roster.present
  /\ provisioning.restore_failed \subseteq identity.known
  /\ recovery.restore_failures \subseteq identity.known
  /\ flows.tracked_runs \subseteq flows.known_runs
  /\ flows.cancel_tokens \subseteq flows.known_runs
  /\ flows.stream_runs \subseteq flows.known_runs
  /\ topology.coordinator = None \/ topology.coordinator.value \in roster.present
  /\ \A edge \in topology.edges : edge[1] \in roster.present /\ edge[2] \in roster.present
  /\ \A id \in AgentIdentityValues : roster.runtime_binding_live[id] => identity.binding_present[id]
  /\ \A id \in AgentIdentityValues : recovery.checkpoint_version[id] >= identity.checkpoint_version[id]

Init ==
  /\ identity =
      [ known |-> {},
        generation |-> ZeroIdentityNatMap,
        runtime_id |-> NoIdentityOptionMap,
        fence_token |-> ZeroIdentityNatMap,
        authority |-> [i \in AgentIdentityValues |-> "Absent"],
        binding_present |-> FalseIdentityBoolMap,
        checkpoint_version |-> ZeroIdentityNatMap ]
  /\ lifecycle =
      [ phase |-> "Creating",
        active_run_count |-> 0,
        cleanup_pending |-> FALSE ]
  /\ roster =
      [ present |-> {},
        profile |-> [i \in AgentIdentityValues |-> None],
        runtime_mode |-> [i \in AgentIdentityValues |-> None],
        labels_present |-> FalseIdentityBoolMap,
        member_status |-> [i \in AgentIdentityValues |-> "Destroyed"],
        runtime_binding_live |-> FalseIdentityBoolMap ]
  /\ topology =
      [ coordinator |-> None,
        revision |-> 0,
        edges |-> {},
        external_specs_present |-> {} ]
  /\ provisioning =
      [ pending_spawn |-> {},
        pending_runtime_id |-> NoIdentityOptionMap,
        kickoff_pending |-> {},
        restore_failed |-> {},
        restore_reason_present |-> FalseIdentityBoolMap ]
  /\ work =
      [ known |-> {},
        owner |-> NoWorkIdentityMap,
        runtime_target |-> NoWorkRuntimeMap,
        run_target |-> NoWorkRunMap,
        step_target |-> NoWorkStepMap,
        status |-> [w \in WorkRefValues |-> "Pending"],
        terminal |-> NoWorkTerminalMap ]
  /\ flows =
      [ known_runs |-> {},
        tracked_runs |-> {},
        cancel_tokens |-> {},
        stream_runs |-> {},
        flow_id |-> NoRunFlowMap,
        status |-> NoRunStatusMap,
        schema_version |-> ZeroRunNatMap,
        completed_at_present |-> FalseRunBoolMap,
        frame_count |-> ZeroRunNatMap,
        loop_count |-> ZeroRunNatMap,
        loop_iteration_count |-> ZeroRunNatMap,
        ordered_steps |-> EmptyRunSeqMap,
        step_dependencies |-> NoRunDependencyMap,
        step_dispatch_mode |-> NoRunDispatchModeMap,
        step_dispatch_width |-> NoRunDispatchWidthMap,
        step_dependency_mode |-> NoRunDependencyModeMap,
        step_has_condition |-> NoRunConditionMap,
        step_branch |-> NoRunBranchMap,
        step_collection_policy |-> NoRunPolicyMap,
        step_quorum_threshold |-> NoRunThresholdMap,
        step_collection_observed |-> NoRunObservedMap,
        step_status |-> NoRunStepStatusMap,
        failure_count |-> ZeroRunNatMap,
        consecutive_failure_count |-> ZeroRunNatMap,
        max_step_retries |-> ZeroRunNatMap,
        escalation_threshold |-> ZeroRunNatMap ]
  /\ tasks =
      [ known |-> {},
        status |-> ZeroTaskStatusMap,
        owner |-> NoTaskIdentityMap,
        run_binding |-> NoTaskRunMap ]
  /\ recovery =
      [ restore_failures |-> {},
        checkpoint_version |-> ZeroIdentityNatMap,
        continuity_bound |-> FalseIdentityBoolMap ]
  /\ history =
      [ next_seq |-> 0,
        event_count_by_identity |-> ZeroIdentityNatMap,
        last_seq_by_identity |-> ZeroIdentityNatMap ]
  /\ effects = NoEffects
  /\ step_count = 0

KnownIdentity(id) == id \in identity.known
HasRuntime(id) == identity.runtime_id[id] # None
HasAuthority(id) == identity.authority[id] \in ActiveAuthorities
IsProvisioning(id) == identity.authority[id] = "Provisioning"
IsRetiring(id) == identity.authority[id] = "Retiring"
HasBinding(id) == identity.binding_present[id]
Rostered(id) == id \in roster.present
ActiveMember(id) == Rostered(id) /\ roster.member_status[id] = "Active"
AutonomousMember(id) == roster.runtime_mode[id] = Some("autonomous")
BrokenMember(id) == Rostered(id) /\ roster.member_status[id] = "Broken"
KickoffPending(id) == id \in provisioning.kickoff_pending
DispatchCapable(id) ==
  /\ Rostered(id)
  /\ identity.authority[id] = "Active"
  /\ roster.runtime_binding_live[id]
  /\ identity.runtime_id[id] # None
DispatchCapacityAvailable == \E id \in AgentIdentityValues : DispatchCapable(id)
CoordinatorBound == topology.coordinator # None
TopologicallyLinked(a, b) == <<a, b>> \in topology.edges
HasExternalSpec(id) == id \in topology.external_specs_present
KnownWork(w) == w \in work.known
LiveWork(w) == KnownWork(w) /\ work.status[w] \in LiveWorkStatuses
TerminalWork(w) == KnownWork(w) /\ work.status[w] \in TerminalWorkStatuses
WorkTargetsRun(w, r) == KnownWork(w) /\ work.run_target[w] = Some(r)
KnownRun(r) == r \in flows.known_runs
TrackedRun(r) == r \in flows.tracked_runs
RunningRun(r) == flows.status[r] = Some("Running")
TerminalRun(r) == flows.status[r] \in {Some("Completed"), Some("Failed"), Some("Canceled")}
TerminalStepStatuses == {Some("Completed"), Some("Failed"), Some("Skipped"), Some("Canceled")}
BoundLiveWork(r, s) ==
  {w \in work.known :
     work.run_target[w] = Some(r) /\
     work.step_target[w] = Some(s) /\
     work.status[w] \in LiveWorkStatuses}
BoundWorkAll(r, s) ==
  {w \in work.known :
     work.run_target[w] = Some(r) /\
     work.step_target[w] = Some(s)}
BoundLiveWorkForRun(r) ==
  {w \in work.known :
     work.run_target[w] = Some(r) /\
     work.status[w] \in LiveWorkStatuses}
TerminalizeIncompleteRunSteps(stepMap, r, replacement) ==
  [s \in StepIdValues |->
     IF s \in KnownSteps(r) THEN
       IF stepMap[s] \in TerminalStepStatuses THEN stepMap[s] ELSE replacement
     ELSE
       None]
AllDependenciesSatisfied(r, s) ==
  \A dep \in flows.step_dependencies[r][s] :
    flows.step_status[r][dep] \in {Some("Completed"), Some("Skipped")}

AnyDependencySatisfied(r, s) ==
  \E dep \in flows.step_dependencies[r][s] :
    flows.step_status[r][dep] = Some("Completed")

OneToOneStep(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_dispatch_mode[r][s] = "OneToOne"

FanInStep(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_dispatch_mode[r][s] = "FanIn"

FanOutStep(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_dispatch_mode[r][s] = "FanOut"

ReadyDirectDispatchStep(r, s) ==
  /\ flows.status[r] = Some("Running")
  /\ flows.step_status[r][s] = None
  /\ flows.step_dependency_mode[r][s] = "All"
  /\ AllDependenciesSatisfied(r, s)

ReadyResolvedStep(r, s) ==
  /\ flows.status[r] = Some("Running")
  /\ flows.step_status[r][s] = Some("Pending")

QuorumReady(r, s) ==
  /\ flows.status[r] = Some("Running")
  /\ flows.step_collection_policy[r][s] = "Quorum"
  /\ flows.step_status[r][s] = Some("Dispatched")
  /\ flows.step_collection_observed[r][s] >= flows.step_quorum_threshold[r][s]

HasDispatchReadyStep(r) ==
  \E s \in KnownSteps(r) : ReadyDirectDispatchStep(r, s) \/ ReadyResolvedStep(r, s)

StructuredRun(r) ==
  KnownRun(r) /\ ((flows.frame_count[r] > 0) \/ (flows.loop_count[r] > 0) \/ (flows.loop_iteration_count[r] > 0))

LoopAwareRun(r) ==
  KnownRun(r) /\ ((flows.loop_count[r] > 0) \/ (flows.loop_iteration_count[r] > 0))

LoopStructuredRun(r) ==
  KnownRun(r) /\ flows.flow_id[r] = Some(FlowLoop)

ReadyStep(r, s) == ReadyDirectDispatchStep(r, s) \/ ReadyResolvedStep(r, s)

PendingStepHasNoBoundWork(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_status[r][s] = Some("Pending")
  /\ {w \in work.known :
        work.run_target[w] = Some(r) /\
        work.step_target[w] = Some(s)} = {}

DispatchedStepHasBoundWork(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_status[r][s] = Some("Dispatched")
  /\ {w \in work.known :
        work.run_target[w] = Some(r) /\
        work.step_target[w] = Some(s)} # {}

BranchMember(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_branch[r][s] # None

BranchJoinStep(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_dependency_mode[r][s] = "Any"

CollectionQuorumStep(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_collection_policy[r][s] = "Quorum"

AggregateScalarStep(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_dispatch_mode[r][s] # "FanIn"
  /\ flows.step_collection_policy[r][s] = "Any"

AggregateArrayStep(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_dispatch_mode[r][s] = "FanIn"

AggregateObjectStep(r, s) ==
  /\ KnownRun(r)
  /\ s \in KnownSteps(r)
  /\ flows.step_dispatch_mode[r][s] # "FanIn"
  /\ flows.step_collection_policy[r][s] # "Any"

AggregateShapePartitionInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      /\ AggregateScalarStep(r, s) \/ AggregateArrayStep(r, s) \/ AggregateObjectStep(r, s)
      /\ ~(AggregateScalarStep(r, s) /\ AggregateArrayStep(r, s))
      /\ ~(AggregateScalarStep(r, s) /\ AggregateObjectStep(r, s))
      /\ ~(AggregateArrayStep(r, s) /\ AggregateObjectStep(r, s))

DispatchWidthKnownInvariant ==
  \A r \in flows.known_runs :
    \A s \in StepIdValues :
      IF s \in KnownSteps(r) THEN
        IF flows.step_status[r][s] \in {Some("Dispatched"), Some("Completed"), Some("Failed")} \/
           BoundWorkAll(r, s) # {}
        THEN
          flows.step_dispatch_width[r][s] >= 1
        ELSE
          flows.step_dispatch_width[r][s] = 0
      ELSE
        flows.step_dispatch_width[r][s] = 0

OneToOneDispatchWidthInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      OneToOneStep(r, s) /\
      (flows.step_status[r][s] \in {Some("Dispatched"), Some("Completed"), Some("Failed")} \/
       flows.step_dispatch_width[r][s] > 0 \/
       BoundWorkAll(r, s) # {}) =>
        flows.step_dispatch_width[r][s] = 1

QuorumDispatchWidthInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      IF flows.step_collection_policy[r][s] = "Quorum" /\ flows.step_dispatch_width[r][s] > 0 THEN
        flows.step_dispatch_width[r][s] >= flows.step_quorum_threshold[r][s]
      ELSE TRUE

HasRestoreFailure(id) == id \in recovery.restore_failures
HasContinuityBinding(id) == recovery.continuity_bound[id]
HasHistory(id) == history.event_count_by_identity[id] > 0
HasTask(t) == t \in tasks.known
LiveTaskBinding(t) ==
  /\ HasTask(t)
  /\ tasks.run_binding[t] # None
  /\ tasks.status[t] \in {"Pending", "InProgress", "Blocked"}

HistoryIdentitySequence(id) ==
  history.last_seq_by_identity[id] = history.event_count_by_identity[id]

PendingSpawnConsistencyInvariant ==
  \A id \in provisioning.pending_spawn :
    /\ identity.authority[id] = "Provisioning"
    /\ identity.runtime_id[id] # None
    /\ identity.runtime_id[id] = provisioning.pending_runtime_id[id]
    /\ ~identity.binding_present[id]
    /\ IF id \in roster.present THEN
         /\ roster.member_status[id] \in {"Pending", "Broken"}
         /\ ~roster.runtime_binding_live[id]
       ELSE TRUE

AuthorityBindingInvariant ==
  \A id \in AgentIdentityValues :
    identity.authority[id] \in ActiveAuthorities =>
      /\ identity.runtime_id[id] # None
      /\ identity.binding_present[id]

DestroyedIdentityInvariant ==
  \A id \in AgentIdentityValues :
    identity.authority[id] = "Destroyed" =>
      /\ ~Rostered(id)
      /\ identity.runtime_id[id] = None
      /\ ~identity.binding_present[id]
      /\ id \notin provisioning.pending_spawn
      /\ id \notin provisioning.kickoff_pending

LifecycleActiveRunCountInvariant ==
  lifecycle.active_run_count =
    Cardinality({r \in flows.known_runs : flows.status[r] \in {Some("Pending"), Some("Running")}})

StoppedMobInvariant ==
  lifecycle.phase = "Stopped" =>
    /\ lifecycle.active_run_count = 0
    /\ provisioning.kickoff_pending = {}

CompletedMobInvariant ==
  lifecycle.phase = "Completed" =>
    /\ lifecycle.active_run_count = 0
    /\ provisioning.kickoff_pending = {}

DestroyedMobInvariant ==
  lifecycle.phase = "Destroyed" =>
    /\ lifecycle.active_run_count = 0
    /\ provisioning.kickoff_pending = {}
    /\ provisioning.pending_spawn = {}
    /\ roster.present = {}
    /\ {w \in work.known : work.status[w] \in LiveWorkStatuses} = {}
    /\ lifecycle.cleanup_pending = FALSE

CoordinatorRosterInvariant ==
  topology.coordinator = None \/ topology.coordinator.value \in roster.present

CoordinatorLivenessInvariant ==
  topology.coordinator = None \/
    LET id == topology.coordinator.value IN
      /\ id \in roster.present
      /\ identity.authority[id] = "Active"
      /\ roster.member_status[id] = "Active"
      /\ roster.runtime_binding_live[id]
      /\ identity.binding_present[id]
      /\ identity.runtime_id[id] # None

TopologySymmetricInvariant ==
  \A a \in AgentIdentityValues :
    \A b \in AgentIdentityValues :
      <<a, b>> \in topology.edges => <<b, a>> \in topology.edges

ExternalSpecInvariant ==
  topology.external_specs_present \subseteq roster.present

ExternalSpecLivenessInvariant ==
  \A id \in topology.external_specs_present :
    /\ id \in roster.present
    /\ identity.authority[id] = "Active"
    /\ roster.member_status[id] = "Active"
    /\ roster.runtime_binding_live[id]
    /\ identity.binding_present[id]
    /\ identity.runtime_id[id] # None

TrackedRunFlowBindingInvariant ==
  \A r \in flows.tracked_runs : flows.flow_id[r] # None

KnownRunBindingInvariant ==
  \A r \in flows.known_runs :
    /\ flows.flow_id[r] # None
    /\ flows.status[r] # None
    /\ flows.schema_version[r] >= 4

TrackedRunSubsetInvariant ==
  /\ flows.cancel_tokens \subseteq flows.tracked_runs
  /\ flows.stream_runs \subseteq flows.tracked_runs

RunCompletionTimestampInvariant ==
  \A r \in flows.known_runs :
    (flows.status[r] \in {Some("Completed"), Some("Failed"), Some("Canceled")}) = flows.completed_at_present[r]

OrderedStepsDistinctInvariant ==
  \A r \in flows.known_runs :
    Cardinality(KnownSteps(r)) = Len(flows.ordered_steps[r])

NonEmptyOrderedStepsInvariant ==
  \A r \in flows.known_runs : Len(flows.ordered_steps[r]) > 0

DependencyKnownInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      flows.step_dependencies[r][s] \subseteq KnownSteps(r)

DispatchModeKnownInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      flows.step_dispatch_mode[r][s] \in DispatchModeValues

NoSelfDependencyInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) : s \notin flows.step_dependencies[r][s]

BranchConditionInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      flows.step_branch[r][s] # None => flows.step_has_condition[r][s]

BranchGroupDependencyInvariant ==
  \A r \in flows.known_runs :
    \A a \in KnownSteps(r) :
      \A b \in KnownSteps(r) :
        flows.step_branch[r][a] # None /\
        flows.step_branch[r][a] = flows.step_branch[r][b] =>
          flows.step_dependencies[r][a] = flows.step_dependencies[r][b]

QuorumThresholdInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      IF flows.step_collection_policy[r][s] = "Quorum" THEN
        flows.step_quorum_threshold[r][s] > 0
      ELSE
        flows.step_quorum_threshold[r][s] = 0

QuorumCompletedObservedInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      IF flows.step_collection_policy[r][s] = "Quorum" /\
         flows.step_status[r][s] = Some("Completed")
      THEN
        flows.step_collection_observed[r][s] >= flows.step_quorum_threshold[r][s]
      ELSE TRUE

AnyDependencyInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      IF flows.step_dependency_mode[r][s] = "Any" THEN
        \E dep \in flows.step_dependencies[r][s] : flows.step_branch[r][dep] # None
      ELSE TRUE

AnyDependencyBranchCoverageInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      IF flows.step_dependency_mode[r][s] = "Any" THEN
        \A dep \in flows.step_dependencies[r][s] : flows.step_branch[r][dep] # None
      ELSE TRUE

PendingStepReadyInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      flows.step_status[r][s] = Some("Pending") =>
        /\ flows.step_dependency_mode[r][s] = "Any"
        /\ AnyDependencySatisfied(r, s)

DispatchedStepReadyInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      flows.step_status[r][s] = Some("Dispatched") =>
        IF flows.step_dependency_mode[r][s] = "Any" THEN
          AnyDependencySatisfied(r, s)
        ELSE
          AllDependenciesSatisfied(r, s)

PendingStepHasNoBoundWorkInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      flows.step_status[r][s] = Some("Pending") =>
        {w \in work.known :
           work.run_target[w] = Some(r) /\
           work.step_target[w] = Some(s)} = {}

DispatchedStepHasBoundWorkInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      flows.step_status[r][s] = Some("Dispatched") =>
        {w \in work.known :
           work.run_target[w] = Some(r) /\
           work.step_target[w] = Some(s)} # {}

DispatchedStepWidthInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      BoundWorkAll(r, s) # {} =>
        Cardinality(BoundWorkAll(r, s)) = flows.step_dispatch_width[r][s]

OneToOneBoundWorkInvariant ==
  \A r \in flows.known_runs :
    \A s \in KnownSteps(r) :
      OneToOneStep(r, s) =>
        Cardinality(BoundWorkAll(r, s)) <= 1

StructuredSchemaInvariant ==
  \A r \in flows.known_runs :
    ((flows.frame_count[r] > 0) \/ (flows.loop_count[r] > 0) \/ (flows.loop_iteration_count[r] > 0)) =>
      flows.schema_version[r] >= 4

StructuredFlowInvariant ==
  \A r \in flows.known_runs :
    StructuredRun(r) => LoopStructuredRun(r)

LoopFrameInvariant ==
  \A r \in flows.known_runs :
    /\ flows.loop_count[r] > 0 => flows.frame_count[r] > 0
    /\ flows.loop_iteration_count[r] > 0 => flows.loop_count[r] > 0 /\ flows.frame_count[r] > 0

LoopBodyInvariant ==
  \A r \in flows.known_runs :
    flows.loop_count[r] > 0 => StepBody \in KnownSteps(r)

FailureCounterInvariant ==
  \A r \in flows.known_runs :
    flows.consecutive_failure_count[r] <= flows.failure_count[r]

StepStatusKnownInvariant ==
  \A r \in flows.known_runs :
    \A s \in StepIdValues :
      flows.step_status[r][s] # None => s \in KnownSteps(r)

WorkTargetInvariant ==
  \A w \in work.known :
    /\ (work.owner[w] = None \/ work.owner[w].value \in identity.known)
    /\ (work.runtime_target[w] = None \/ work.runtime_target[w].value \in AgentRuntimeIdValues)
    /\ (work.run_target[w] = None \/ work.run_target[w].value \in flows.known_runs)
    /\ (work.step_target[w] = None \/ (work.run_target[w] # None /\ work.step_target[w].value \in KnownSteps(work.run_target[w].value)))

WorkStepStateInvariant ==
  \A w \in work.known :
    IF work.run_target[w] # None /\ work.step_target[w] # None THEN
      LET r == work.run_target[w].value
          s == work.step_target[w].value
      IN
        /\ flows.step_status[r][s] # None
        /\ work.status[w] \in LiveWorkStatuses => flows.step_status[r][s] = Some("Dispatched")
        /\ flows.step_status[r][s] \in {Some("Completed"), Some("Failed"), Some("Skipped"), Some("Canceled")} =>
             work.status[w] \notin LiveWorkStatuses
    ELSE TRUE

TerminalRunWorkInvariant ==
  \A w \in work.known :
    IF work.run_target[w] # None THEN
      TerminalRun(work.run_target[w].value) => work.status[w] \notin LiveWorkStatuses
    ELSE TRUE

TerminalRunStepInvariant ==
  \A r \in flows.known_runs :
    TerminalRun(r) =>
      \A s \in KnownSteps(r) :
        flows.step_status[r][s] \in TerminalStepStatuses

TerminalWorkInvariant ==
  \A w \in work.known :
    (work.status[w] \in TerminalWorkStatuses) = (work.terminal[w] # None)

KickoffProvisioningInvariant ==
  provisioning.kickoff_pending \subseteq roster.present

KickoffBarrierInvariant ==
  \A id \in provisioning.kickoff_pending :
    /\ Rostered(id)
    /\ identity.authority[id] = "Active"
    /\ roster.member_status[id] = "Active"
    /\ roster.runtime_binding_live[id]
    /\ identity.binding_present[id]
    /\ identity.runtime_id[id] # None
    /\ AutonomousMember(id)
    /\ id \notin provisioning.pending_spawn
    /\ id \notin recovery.restore_failures
    /\ lifecycle.phase = "Running"

PendingRuntimeInvariant ==
  \A id \in AgentIdentityValues :
    (id \in provisioning.pending_spawn) = (provisioning.pending_runtime_id[id] # None)

RestoreFailureInvariant ==
  recovery.restore_failures = provisioning.restore_failed

RestoreReasonInvariant ==
  \A id \in AgentIdentityValues :
    provisioning.restore_reason_present[id] = (id \in provisioning.restore_failed)

CheckpointVersionInvariant ==
  \A id \in AgentIdentityValues :
    identity.checkpoint_version[id] = recovery.checkpoint_version[id]

HistoryMonotonicInvariant ==
  \A id \in AgentIdentityValues :
    /\ history.last_seq_by_identity[id] <= history.next_seq
    /\ history.event_count_by_identity[id] <= history.next_seq
    /\ history.event_count_by_identity[id] <= history.last_seq_by_identity[id]

HistoryPresenceInvariant ==
  \A id \in AgentIdentityValues :
    (history.event_count_by_identity[id] = 0) = (history.last_seq_by_identity[id] = 0)

HistoryIdentitySequenceInvariant ==
  \A id \in AgentIdentityValues :
    history.last_seq_by_identity[id] = history.event_count_by_identity[id]

TaskBindingInvariant ==
  \A t \in tasks.known :
    /\ (tasks.owner[t] = None \/ tasks.owner[t].value \in identity.known)
    /\ (tasks.run_binding[t] = None \/ tasks.run_binding[t].value \in flows.known_runs)

LiveTaskRunBindingInvariant ==
  \A t \in tasks.known :
    tasks.run_binding[t] # None =>
      /\ tasks.status[t] \in {"Pending", "InProgress", "Blocked"}
      /\ ~TerminalRun(tasks.run_binding[t].value)

TerminalTaskBindingInvariant ==
  \A t \in tasks.known :
    tasks.status[t] \in {"Completed", "Canceled"} => tasks.run_binding[t] = None

Invariants ==
  /\ TypeOK
  /\ PendingSpawnConsistencyInvariant
  /\ AuthorityBindingInvariant
  /\ DestroyedIdentityInvariant
  /\ LifecycleActiveRunCountInvariant
  /\ StoppedMobInvariant
  /\ CompletedMobInvariant
  /\ DestroyedMobInvariant
  /\ CoordinatorRosterInvariant
  /\ CoordinatorLivenessInvariant
  /\ TopologySymmetricInvariant
  /\ ExternalSpecInvariant
  /\ ExternalSpecLivenessInvariant
  /\ TrackedRunFlowBindingInvariant
  /\ KnownRunBindingInvariant
  /\ TrackedRunSubsetInvariant
  /\ RunCompletionTimestampInvariant
  /\ OrderedStepsDistinctInvariant
  /\ NonEmptyOrderedStepsInvariant
  /\ DependencyKnownInvariant
  /\ DispatchModeKnownInvariant
  /\ AggregateShapePartitionInvariant
  /\ DispatchWidthKnownInvariant
  /\ OneToOneDispatchWidthInvariant
  /\ QuorumDispatchWidthInvariant
  /\ NoSelfDependencyInvariant
  /\ BranchConditionInvariant
  /\ BranchGroupDependencyInvariant
  /\ QuorumThresholdInvariant
  /\ QuorumCompletedObservedInvariant
  /\ AnyDependencyInvariant
  /\ AnyDependencyBranchCoverageInvariant
  /\ PendingStepReadyInvariant
  /\ DispatchedStepReadyInvariant
  /\ PendingStepHasNoBoundWorkInvariant
  /\ DispatchedStepHasBoundWorkInvariant
  /\ DispatchedStepWidthInvariant
  /\ OneToOneBoundWorkInvariant
  /\ StructuredSchemaInvariant
  /\ StructuredFlowInvariant
  /\ LoopFrameInvariant
  /\ LoopBodyInvariant
  /\ FailureCounterInvariant
  /\ StepStatusKnownInvariant
  /\ WorkTargetInvariant
  /\ WorkStepStateInvariant
  /\ TerminalRunWorkInvariant
  /\ TerminalRunStepInvariant
  /\ TerminalWorkInvariant
  /\ KickoffProvisioningInvariant
  /\ KickoffBarrierInvariant
  /\ PendingRuntimeInvariant
  /\ RestoreFailureInvariant
  /\ RestoreReasonInvariant
  /\ CheckpointVersionInvariant
  /\ HistoryMonotonicInvariant
  /\ HistoryPresenceInvariant
  /\ HistoryIdentitySequenceInvariant
  /\ TaskBindingInvariant
  /\ LiveTaskRunBindingInvariant
  /\ TerminalTaskBindingInvariant

AvailableIdentities == AgentIdentityValues \ identity.known
AvailableWork == WorkRefValues \ work.known
AvailableRuns == RunIdValues \ flows.known_runs
AvailableTasks == TaskIdValues \ tasks.known

RegisterIdentity ==
    /\ AvailableIdentities # {}
  /\ LET id == PickOne(AvailableIdentities) IN
       /\ identity' =
            [identity EXCEPT !.known = @ \cup {id}]
       /\ roster' = [roster EXCEPT !.member_status[id] = "Pending"]
       /\ UNCHANGED << lifecycle, topology, provisioning, work, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

ProvisionIncarnationFresh ==
    /\ \E id \in identity.known :
       /\ identity.authority[id] = "Absent"
       /\ ~Rostered(id)
       /\ id \notin provisioning.pending_spawn
  /\ LET id == PickOne({i \in identity.known : identity.authority[i] = "Absent" /\ ~Rostered(i) /\ i \notin provisioning.pending_spawn})
         rt == PickOne(AgentRuntimeIdValues)
     IN
       /\ identity' =
            [identity EXCEPT
              !.runtime_id[id] = Some(rt),
              !.fence_token[id] = Inc(@),
              !.authority[id] = "Provisioning"]
       /\ provisioning' =
            [provisioning EXCEPT
              !.pending_spawn = @ \cup {id},
              !.pending_runtime_id[id] = Some(rt)]
       /\ UNCHANGED << lifecycle, roster, topology, work, flows, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.provision_requests = {id}]
       /\ step_count' = Inc(step_count)

ProvisionIncarnationRecover ==
    /\ \E id \in identity.known :
       /\ identity.authority[id] = "Absent"
       /\ recovery.continuity_bound[id]
       /\ identity.runtime_id[id] # None
       /\ id \notin provisioning.pending_spawn
  /\ LET id == PickOne({i \in identity.known : identity.authority[i] = "Absent" /\ recovery.continuity_bound[i] /\ identity.runtime_id[i] # None /\ i \notin provisioning.pending_spawn})
     IN
       /\ identity' =
            [identity EXCEPT
              !.fence_token[id] = Inc(@),
              !.authority[id] = "Provisioning"]
       /\ provisioning' =
            [provisioning EXCEPT
              !.pending_spawn = @ \cup {id},
              !.pending_runtime_id[id] = identity.runtime_id[id]]
       /\ UNCHANGED << lifecycle, roster, topology, work, flows, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.provision_requests = {id}]
       /\ step_count' = Inc(step_count)

FinalizeSpawn ==
    /\ provisioning.pending_spawn # {}
  /\ LET id == PickOne(provisioning.pending_spawn)
         profile == PickOne(ProfileIdValues)
         mode == PickOne(RuntimeModeValues)
     IN
       /\ identity' =
            [identity EXCEPT
              !.authority[id] = "Active",
              !.binding_present[id] = TRUE,
              !.checkpoint_version[id] = recovery.checkpoint_version[id]]
       /\ lifecycle' =
            [lifecycle EXCEPT !.phase = "Running"]
       /\ roster' =
            [roster EXCEPT
              !.present = @ \cup {id},
              !.member_status[id] = "Active",
              !.runtime_binding_live[id] = TRUE,
              !.profile[id] = Some(profile),
              !.runtime_mode[id] = Some(mode)]
       /\ provisioning' =
            [provisioning EXCEPT
              !.pending_spawn = @ \ {id},
              !.pending_runtime_id[id] = None,
              !.kickoff_pending =
                IF mode = "autonomous" THEN @ \cup {id} ELSE @,
              !.restore_failed = @ \ {id},
              !.restore_reason_present[id] = FALSE]
       /\ recovery' =
            [recovery EXCEPT
              !.continuity_bound[id] = TRUE,
              !.restore_failures = @ \ {id}]
       /\ history' =
            [history EXCEPT
              !.next_seq = Inc(@),
              !.event_count_by_identity[id] = Inc(@),
              !.last_seq_by_identity[id] = Inc(@)]
       /\ UNCHANGED << topology, work, flows, tasks >>
       /\ effects' =
            [NoEffects EXCEPT
              !.published_events = 1,
              !.persisted_roster = TRUE,
              !.persisted_history = TRUE]
       /\ step_count' = Inc(step_count)

FailProvision ==
    /\ provisioning.pending_spawn # {}
  /\ LET id == PickOne(provisioning.pending_spawn) IN
       /\ identity' =
            [identity EXCEPT
              !.authority[id] = "Absent",
              !.binding_present[id] = FALSE]
       /\ provisioning' =
            [provisioning EXCEPT
              !.pending_spawn = @ \ {id},
              !.pending_runtime_id[id] = None,
              !.restore_failed = @ \cup {id},
              !.restore_reason_present[id] = TRUE]
       /\ recovery' =
            [recovery EXCEPT
              !.restore_failures = @ \cup {id}]
       /\ roster' =
            [roster EXCEPT !.member_status[id] = "Broken"]
       /\ topology' =
            [topology EXCEPT
              !.coordinator = IF @ # None /\ @.value = id THEN None ELSE @,
              !.external_specs_present = @ \ {id}]
       /\ UNCHANGED << lifecycle, work, flows, tasks, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

RetireMember ==
    /\ \E id \in roster.present : identity.authority[id] = "Active"
  /\ LET id == PickOne({i \in roster.present : identity.authority[i] = "Active"}) IN
       /\ identity' = [identity EXCEPT !.authority[id] = "Retiring"]
       /\ roster' = [roster EXCEPT !.member_status[id] = "Retiring"]
       /\ provisioning' = [provisioning EXCEPT !.kickoff_pending = @ \ {id}]
       /\ topology' =
            [topology EXCEPT
              !.coordinator = IF @ # None /\ @.value = id THEN None ELSE @,
              !.external_specs_present = @ \ {id}]
       /\ UNCHANGED << lifecycle, work, flows, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.retire_requests = {id}]
       /\ step_count' = Inc(step_count)

ResetMember ==
    /\ \E id \in identity.known : identity.authority[id] \in {"Active", "Retiring"}
  /\ LET id == PickOne({i \in identity.known : identity.authority[i] \in {"Active", "Retiring"}})
         rt == PickOne(AgentRuntimeIdValues)
     IN
       /\ identity' =
            [identity EXCEPT
              !.generation[id] = Inc(@),
              !.runtime_id[id] = Some(rt),
              !.fence_token[id] = Inc(@),
              !.authority[id] = "Provisioning",
              !.binding_present[id] = FALSE,
              !.checkpoint_version[id] = Inc(@)]
       /\ roster' =
            [roster EXCEPT
              !.member_status[id] = "Pending",
              !.runtime_binding_live[id] = FALSE]
       /\ provisioning' =
            [provisioning EXCEPT
              !.pending_spawn = @ \cup {id},
              !.pending_runtime_id[id] = Some(rt),
              !.kickoff_pending = @ \ {id}]
       /\ recovery' = [recovery EXCEPT !.checkpoint_version[id] = Inc(@)]
       /\ topology' =
            [topology EXCEPT
              !.coordinator = IF @ # None /\ @.value = id THEN None ELSE @,
              !.external_specs_present = @ \ {id}]
       /\ UNCHANGED << lifecycle, work, flows, tasks, history >>
       /\ effects' = [NoEffects EXCEPT !.provision_requests = {id}]
       /\ step_count' = Inc(step_count)

DestroyMember ==
    /\ \E id \in identity.known : identity.authority[id] \in {"Retiring", "Absent", "Provisioning"}
  /\ LET id == PickOne({i \in identity.known : identity.authority[i] \in {"Retiring", "Absent", "Provisioning"}}) IN
       /\ identity' =
            [identity EXCEPT
              !.authority[id] = "Destroyed",
              !.runtime_id[id] = None,
              !.binding_present[id] = FALSE]
       /\ roster' =
            [roster EXCEPT
              !.present = @ \ {id},
              !.member_status[id] = "Destroyed",
              !.runtime_binding_live[id] = FALSE,
              !.profile[id] = None,
              !.runtime_mode[id] = None]
       /\ provisioning' =
            [provisioning EXCEPT
              !.pending_spawn = @ \ {id},
              !.kickoff_pending = @ \ {id},
              !.restore_failed = @ \ {id},
              !.restore_reason_present[id] = FALSE,
              !.pending_runtime_id[id] = None]
       /\ recovery' =
            [recovery EXCEPT
              !.restore_failures = @ \ {id},
              !.continuity_bound[id] = FALSE]
       /\ topology' =
            [topology EXCEPT
              !.edges = {edge \in @ : edge[1] # id /\ edge[2] # id},
              !.external_specs_present = @ \ {id},
              !.coordinator = IF @ # None /\ @.value = id THEN None ELSE @]
       /\ UNCHANGED lifecycle
       /\ UNCHANGED << work, flows, tasks, history >>
       /\ effects' = [NoEffects EXCEPT !.destroy_requests = {id}, !.persisted_roster = TRUE]
       /\ step_count' = Inc(step_count)

BindCoordinator ==
    /\ {id \in roster.present :
        /\ identity.authority[id] = "Active"
        /\ roster.member_status[id] = "Active"
        /\ roster.runtime_binding_live[id]
        /\ identity.binding_present[id]
        /\ identity.runtime_id[id] # None} # {}
  /\ LET id == PickOne({i \in roster.present :
                          /\ identity.authority[i] = "Active"
                          /\ roster.member_status[i] = "Active"
                          /\ roster.runtime_binding_live[i]
                          /\ identity.binding_present[i]
                          /\ identity.runtime_id[i] # None})
     IN
       /\ topology' =
            [topology EXCEPT
              !.coordinator = Some(id),
              !.revision = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, provisioning, work, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

AddTopologyEdge ==
    /\ Cardinality(roster.present) >= 2
  /\ \E candidateA \in roster.present :
       \E candidateB \in roster.present :
         /\ candidateA # candidateB
         /\ <<candidateA, candidateB>> \notin topology.edges
  /\ LET edge == PickOne({<<a, b>> \in roster.present \X roster.present : a # b /\ <<a, b>> \notin topology.edges})
         a == edge[1]
         b == edge[2]
     IN
       /\ topology' =
            [topology EXCEPT
              !.edges = @ \cup {<<a, b>>, <<b, a>>},
              !.revision = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, provisioning, work, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

RemoveTopologyEdge ==
    /\ topology.edges # {}
  /\ LET edge == PickOne(topology.edges)
         a == edge[1]
         b == edge[2]
     IN
       /\ topology' =
            [topology EXCEPT
              !.edges = @ \ {<<a, b>>, <<b, a>>},
              !.revision = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, provisioning, work, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

PublishExternalSpec ==
    /\ \E id \in roster.present :
       /\ id \notin topology.external_specs_present
       /\ identity.authority[id] = "Active"
       /\ roster.member_status[id] = "Active"
       /\ roster.runtime_binding_live[id]
       /\ identity.binding_present[id]
       /\ identity.runtime_id[id] # None
  /\ LET id == PickOne({i \in roster.present :
                          /\ i \notin topology.external_specs_present
                          /\ identity.authority[i] = "Active"
                          /\ roster.member_status[i] = "Active"
                          /\ roster.runtime_binding_live[i]
                          /\ identity.binding_present[i]
                          /\ identity.runtime_id[i] # None})
     IN
       /\ topology' =
            [topology EXCEPT
              !.external_specs_present = @ \cup {id},
              !.revision = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, provisioning, work, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

ClearExternalSpec ==
    /\ topology.external_specs_present # {}
  /\ LET id == PickOne(topology.external_specs_present) IN
       /\ topology' =
            [topology EXCEPT
              !.external_specs_present = @ \ {id},
              !.revision = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, provisioning, work, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

SubmitWork ==
    /\ AvailableWork # {}
  /\ {i \in roster.present : DispatchCapable(i)} # {}
  /\ LET w == PickOne(AvailableWork)
         id == PickOne({i \in roster.present : DispatchCapable(i)})
     IN
       /\ work' =
            [work EXCEPT
              !.known = @ \cup {w},
              !.owner[w] = Some(id),
              !.runtime_target[w] = identity.runtime_id[id],
              !.run_target[w] = None,
              !.step_target[w] = None,
              !.status[w] = "Pending",
              !.terminal[w] = None]
       /\ flows' = flows
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.work_submissions = {w}]
       /\ step_count' = Inc(step_count)

AcceptWork ==
    /\ \E w \in work.known : work.status[w] = "Pending"
  /\ LET w == PickOne({w \in work.known : work.status[w] = "Pending"}) IN
       /\ work' = [work EXCEPT !.status[w] = "Accepted"]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

StartWork ==
    /\ \E w \in work.known : work.status[w] = "Accepted"
  /\ LET w == PickOne({w \in work.known : work.status[w] = "Accepted"}) IN
       /\ work' = [work EXCEPT !.status[w] = "Running"]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

RejectWork ==
    /\ \E w \in work.known : work.status[w] = "Pending"
  /\ LET w == PickOne({w \in work.known : work.status[w] = "Pending"}) IN
       /\ work' =
            [work EXCEPT
              !.status[w] = "Rejected",
              !.terminal[w] = Some("WorkRejected")]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

CompleteWork ==
    /\ \E w \in work.known : work.status[w] = "Running"
  /\ LET w == PickOne({w \in work.known : work.status[w] = "Running"}) IN
       /\ work' =
            [work EXCEPT
              !.status[w] = "Completed",
              !.terminal[w] = Some("WorkCompleted")]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

FailWork ==
    /\ \E w \in work.known : work.status[w] = "Running"
  /\ LET w == PickOne({w \in work.known : work.status[w] = "Running"}) IN
       /\ work' =
            [work EXCEPT
              !.status[w] = "Failed",
              !.terminal[w] = Some("WorkFailed")]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

CancelWork ==
    /\ \E w \in work.known : work.status[w] \in LiveWorkStatuses
  /\ LET w == PickOne({w \in work.known : work.status[w] \in LiveWorkStatuses}) IN
       /\ work' =
            [work EXCEPT
              !.status[w] = "Canceled",
              !.terminal[w] = Some("WorkCanceled")]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, flows, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.work_cancellations = {w}]
       /\ step_count' = Inc(step_count)

CancelAllWork ==
    /\ \E id \in identity.known :
       LET cancelSet ==
             {w \in work.known :
                work.owner[w] = Some(id) /\
                work.runtime_target[w] = identity.runtime_id[id] /\
                work.status[w] \in LiveWorkStatuses}
       IN cancelSet # {}
  /\ LET id ==
         PickOne({i \in identity.known :
                    LET cancelSet ==
                          {w \in work.known :
                             work.owner[w] = Some(i) /\
                             work.runtime_target[w] = identity.runtime_id[i] /\
                             work.status[w] \in LiveWorkStatuses}
                    IN cancelSet # {}})
         cancelSet ==
           {w \in work.known :
              work.owner[w] = Some(id) /\
              work.runtime_target[w] = identity.runtime_id[id] /\
              work.status[w] \in LiveWorkStatuses}
         newStatus ==
           [w \in WorkRefValues |->
              IF w \in cancelSet THEN "Canceled" ELSE work.status[w]]
         newTerminal ==
           [w \in WorkRefValues |->
              IF w \in cancelSet THEN Some("WorkCanceled") ELSE work.terminal[w]]
     IN
       /\ work' =
            [work EXCEPT
              !.status = newStatus,
              !.terminal = newTerminal]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, flows, tasks, recovery, history >>
       /\ effects' =
            [NoEffects EXCEPT
              !.work_cancellations = cancelSet,
              !.bulk_work_cancellations = {id}]
       /\ step_count' = Inc(step_count)

StartFlowRun ==
    /\ AvailableRuns # {}
  /\ DispatchCapacityAvailable
  /\ LET r == PickOne(AvailableRuns)
         flow == PickOne(KnownFlowIds)
     IN
       /\ \E dispatchModeMap \in FlowDispatchModeAssignments(flow) :
            /\ flows' =
                 [flows EXCEPT
                   !.known_runs = @ \cup {r},
                   !.flow_id[r] = Some(flow),
                   !.status[r] = Some("Running"),
                   !.schema_version[r] = 4,
                   !.completed_at_present[r] = FALSE,
                   !.frame_count[r] = FlowFrameCount(flow),
                   !.loop_count[r] = FlowLoopCount(flow),
                   !.loop_iteration_count[r] = FlowLoopIterationCount(flow),
                   !.ordered_steps[r] = FlowOrdered(flow),
                   !.step_dependencies[r] = FlowDependencyMap(flow),
                   !.step_dispatch_mode[r] = dispatchModeMap,
                   !.step_dispatch_width[r] = DefaultStepDispatchWidths,
                   !.step_dependency_mode[r] = FlowDependencyModeMap(flow),
                   !.step_has_condition[r] = FlowConditionMap(flow),
                   !.step_branch[r] = FlowBranchMap(flow),
                   !.step_collection_policy[r] = FlowCollectionPolicyMap(flow),
                   !.step_quorum_threshold[r] = FlowQuorumThresholdMap(flow),
                   !.step_collection_observed[r] = DefaultStepObserved,
                   !.step_status[r] = DefaultStepStatuses,
                   !.failure_count[r] = 0,
                   !.consecutive_failure_count[r] = 0,
                   !.max_step_retries[r] = FlowMaxStepRetries(flow),
                   !.escalation_threshold[r] = FlowEscalationThreshold(flow)]
            /\ lifecycle' = [lifecycle EXCEPT !.phase = "Running", !.active_run_count = Inc(@)]
            /\ UNCHANGED << identity, roster, topology, provisioning, work, tasks, recovery, history >>
            /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
            /\ step_count' = Inc(step_count)

TrackRun ==
    /\ flows.known_runs \ flows.tracked_runs # {}
  /\ LET r == PickOne(flows.known_runs \ flows.tracked_runs) IN
       /\ flows' =
            [flows EXCEPT
              !.tracked_runs = @ \cup {r},
              !.cancel_tokens = @ \cup {r},
              !.stream_runs = @ \cup {r}]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

CompleteRun ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] \in {Some("Pending"), Some("Running")}
       /\ \A s \in KnownSteps(r) :
            flows.step_status[r][s] \in {Some("Completed"), Some("Skipped")}
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] \in {Some("Pending"), Some("Running")} /\
                    \A s \in KnownSteps(run) :
                      flows.step_status[run][s] \in {Some("Completed"), Some("Skipped")}})
         clearedBindings ==
           [t \in TaskIdValues |->
              IF tasks.run_binding[t] = Some(r) THEN None ELSE tasks.run_binding[t]]
         boundWork == BoundLiveWorkForRun(r)
         finalizedStatus ==
           [w \in WorkRefValues |->
              IF w \in boundWork THEN
                IF work.step_target[w] # None /\
                   flows.step_status[r][work.step_target[w].value] = Some("Completed")
                THEN "Completed"
                ELSE "Canceled"
              ELSE work.status[w]]
         finalizedTerminal ==
           [w \in WorkRefValues |->
              IF w \in boundWork THEN
                IF work.step_target[w] # None /\
                   flows.step_status[r][work.step_target[w].value] = Some("Completed")
                THEN Some("WorkCompleted")
                ELSE Some("WorkCanceled")
              ELSE work.terminal[w]]
     IN
       /\ flows' =
            [flows EXCEPT
              !.status[r] = Some("Completed"),
              !.completed_at_present[r] = TRUE]
       /\ lifecycle' = [lifecycle EXCEPT !.active_run_count = @ - 1]
       /\ work' =
            IF boundWork = {} THEN
              work
            ELSE
              [work EXCEPT
                !.status = finalizedStatus,
                !.terminal = finalizedTerminal]
       /\ tasks' = [tasks EXCEPT !.run_binding = clearedBindings]
       /\ UNCHANGED << identity, roster, topology, provisioning, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

FailRun ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] \in {Some("Pending"), Some("Running")}
       /\ ((\E s \in KnownSteps(r) : flows.step_status[r][s] = Some("Failed")) \/ flows.failure_count[r] > 0)
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] \in {Some("Pending"), Some("Running")} /\
                    ((\E s \in KnownSteps(run) : flows.step_status[run][s] = Some("Failed")) \/ flows.failure_count[run] > 0)})
         clearedBindings ==
           [t \in TaskIdValues |->
              IF tasks.run_binding[t] = Some(r) THEN None ELSE tasks.run_binding[t]]
         boundWork == BoundLiveWorkForRun(r)
     IN
       /\ flows' =
            [flows EXCEPT
              !.status[r] = Some("Failed"),
              !.step_status[r] = TerminalizeIncompleteRunSteps(@, r, Some("Canceled")),
              !.completed_at_present[r] = TRUE,
              !.failure_count[r] = Inc(@),
              !.consecutive_failure_count[r] = Inc(@)]
       /\ lifecycle' = [lifecycle EXCEPT !.active_run_count = @ - 1]
       /\ work' =
            IF boundWork = {} THEN
              work
            ELSE
              [work EXCEPT
                !.status =
                  [w \in WorkRefValues |->
                     IF w \in boundWork THEN "Failed" ELSE work.status[w]],
                !.terminal =
                  [w \in WorkRefValues |->
                     IF w \in boundWork THEN Some("WorkFailed") ELSE work.terminal[w]]]
       /\ tasks' = [tasks EXCEPT !.run_binding = clearedBindings]
       /\ UNCHANGED << identity, roster, topology, provisioning, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

CancelRun ==
    /\ \E r \in flows.known_runs : flows.status[r] \in {Some("Pending"), Some("Running")}
  /\ LET r == PickOne({run \in flows.known_runs : flows.status[run] \in {Some("Pending"), Some("Running")}})
         clearedBindings ==
           [t \in TaskIdValues |->
              IF tasks.run_binding[t] = Some(r) THEN None ELSE tasks.run_binding[t]]
         boundWork == BoundLiveWorkForRun(r)
     IN
       /\ flows' =
            [flows EXCEPT
              !.status[r] = Some("Canceled"),
              !.step_status[r] = TerminalizeIncompleteRunSteps(@, r, Some("Canceled")),
              !.completed_at_present[r] = TRUE]
       /\ lifecycle' = [lifecycle EXCEPT !.active_run_count = @ - 1]
       /\ work' =
            IF boundWork = {} THEN
              work
            ELSE
              [work EXCEPT
                !.status =
                  [w \in WorkRefValues |->
                     IF w \in boundWork THEN "Canceled" ELSE work.status[w]],
                !.terminal =
                  [w \in WorkRefValues |->
                     IF w \in boundWork THEN Some("WorkCanceled") ELSE work.terminal[w]]]
       /\ tasks' = [tasks EXCEPT !.run_binding = clearedBindings]
       /\ UNCHANGED << identity, roster, topology, provisioning, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

FailRunNoDispatchCapacity ==
    /\ ~DispatchCapacityAvailable
  /\ \E candidateRun \in flows.known_runs :
       /\ flows.status[candidateRun] = Some("Running")
       /\ HasDispatchReadyStep(candidateRun)
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] = Some("Running") /\
                    HasDispatchReadyStep(run)})
         clearedBindings ==
           [t \in TaskIdValues |->
              IF tasks.run_binding[t] = Some(r) THEN None ELSE tasks.run_binding[t]]
         boundWork == BoundLiveWorkForRun(r)
     IN
       /\ flows' =
            [flows EXCEPT
              !.status[r] = Some("Failed"),
              !.step_status[r] = TerminalizeIncompleteRunSteps(@, r, Some("Canceled")),
              !.completed_at_present[r] = TRUE,
              !.failure_count[r] = Inc(@),
              !.consecutive_failure_count[r] = Inc(@)]
       /\ lifecycle' = [lifecycle EXCEPT !.active_run_count = @ - 1]
       /\ work' =
            IF boundWork = {} THEN
              work
            ELSE
              [work EXCEPT
                !.status =
                  [w \in WorkRefValues |->
                     IF w \in boundWork THEN "Failed" ELSE work.status[w]],
                !.terminal =
                  [w \in WorkRefValues |->
                     IF w \in boundWork THEN Some("WorkFailed") ELSE work.terminal[w]]]
       /\ tasks' = [tasks EXCEPT !.run_binding = clearedBindings]
       /\ UNCHANGED << identity, roster, topology, provisioning, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

DispatchStep ==
    /\ DispatchCapacityAvailable
  /\ \E candidateRun \in flows.known_runs :
       /\ flows.status[candidateRun] = Some("Running")
       /\ \E candidateStep \in KnownSteps(candidateRun) :
            ReadyDirectDispatchStep(candidateRun, candidateStep) \/
            ReadyResolvedStep(candidateRun, candidateStep)
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] = Some("Running") /\
                    \E s \in KnownSteps(run) : ReadyDirectDispatchStep(run, s) \/ ReadyResolvedStep(run, s)})
         s ==
           PickOne({step \in KnownSteps(r) :
                      ReadyDirectDispatchStep(r, step) \/ ReadyResolvedStep(r, step)})
         dispatchable == {i \in roster.present : DispatchCapable(i)}
         admissibleWidths ==
           { n \in DispatchWidthValues :
               /\ n <= Cardinality(AvailableWork)
               /\ IF OneToOneStep(r, s) THEN n = 1 ELSE TRUE
               /\ IF flows.step_collection_policy[r][s] = "Quorum" THEN
                    n >= flows.step_quorum_threshold[r][s]
                  ELSE TRUE }
         width == PickOne(admissibleWidths)
         workSet == PickSubsetOfSize(AvailableWork, width)
         ownerAssignment == CHOOSE assign \in [workSet -> dispatchable] : TRUE
     IN
       /\ admissibleWidths # {}
       /\ flows' =
            [flows EXCEPT
              !.step_status[r][s] = Some("Dispatched"),
              !.step_dispatch_width[r][s] = width]
       /\ work' =
            [work EXCEPT
              !.known = @ \cup workSet,
              !.owner =
                [x \in WorkRefValues |->
                   IF x \in workSet THEN Some(ownerAssignment[x]) ELSE work.owner[x]],
              !.runtime_target =
                [x \in WorkRefValues |->
                   IF x \in workSet THEN identity.runtime_id[ownerAssignment[x]] ELSE work.runtime_target[x]],
              !.run_target =
                [x \in WorkRefValues |->
                   IF x \in workSet THEN Some(r) ELSE work.run_target[x]],
              !.step_target =
                [x \in WorkRefValues |->
                   IF x \in workSet THEN Some(s) ELSE work.step_target[x]],
              !.status =
                [x \in WorkRefValues |->
                   IF x \in workSet THEN "Pending" ELSE work.status[x]],
              !.terminal =
                [x \in WorkRefValues |->
                   IF x \in workSet THEN None ELSE work.terminal[x]]]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.work_submissions = workSet, !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

ResolveAnyJoin ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ \E s \in KnownSteps(r) :
            /\ flows.step_dependency_mode[r][s] = "Any"
            /\ flows.step_status[r][s] = None
            /\ \E dep \in flows.step_dependencies[r][s] : flows.step_status[r][dep] = Some("Completed")
  /\ LET r == PickOne({run \in flows.known_runs :
                         flows.status[run] = Some("Running") /\
                         \E s \in KnownSteps(run) :
                           flows.step_dependency_mode[run][s] = "Any" /\
                           flows.step_status[run][s] = None /\
                           \E dep \in flows.step_dependencies[run][s] : flows.step_status[run][dep] = Some("Completed")})
         s == PickOne({step \in KnownSteps(r) :
                         flows.step_dependency_mode[r][step] = "Any" /\
                         flows.step_status[r][step] = None /\
                         \E dep \in flows.step_dependencies[r][step] : flows.step_status[r][dep] = Some("Completed")})
     IN
       /\ flows' = [flows EXCEPT !.step_status[r][s] = Some("Pending")]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

ResolveCollection ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ \E s \in KnownSteps(r) :
            QuorumReady(r, s)
  /\ LET r == PickOne({run \in flows.known_runs :
                         flows.status[run] = Some("Running") /\
                         \E s \in KnownSteps(run) : QuorumReady(run, s)})
         s == PickOne({step \in KnownSteps(r) : QuorumReady(r, step)})
     IN
       /\ flows' = [flows EXCEPT !.step_status[r][s] = Some("Completed")]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

RecordCollectionContribution ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ \E s \in KnownSteps(r) :
            /\ flows.step_collection_policy[r][s] = "Quorum"
            /\ flows.step_status[r][s] = Some("Dispatched")
            /\ flows.step_collection_observed[r][s] < flows.step_quorum_threshold[r][s]
            /\ flows.step_collection_observed[r][s] < flows.step_dispatch_width[r][s]
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] = Some("Running") /\
                    \E s \in KnownSteps(run) :
                      flows.step_collection_policy[run][s] = "Quorum" /\
                      flows.step_status[run][s] = Some("Dispatched") /\
                      flows.step_collection_observed[run][s] < flows.step_quorum_threshold[run][s] /\
                      flows.step_collection_observed[run][s] < flows.step_dispatch_width[run][s]})
         s ==
           PickOne({step \in KnownSteps(r) :
                      flows.step_collection_policy[r][step] = "Quorum" /\
                      flows.step_status[r][step] = Some("Dispatched") /\
                      flows.step_collection_observed[r][step] < flows.step_quorum_threshold[r][step] /\
                      flows.step_collection_observed[r][step] < flows.step_dispatch_width[r][step]})
     IN
       /\ flows' = [flows EXCEPT !.step_collection_observed[r][s] = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

MarkStepCompleted ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ \E s \in KnownSteps(r) :
            flows.step_status[r][s] = Some("Dispatched") /\
            flows.step_collection_policy[r][s] # "Quorum"
  /\ LET r == PickOne({run \in flows.known_runs :
                         flows.status[run] = Some("Running") /\
                         \E s \in KnownSteps(run) :
                           flows.step_status[run][s] = Some("Dispatched") /\
                           flows.step_collection_policy[run][s] # "Quorum"})
         s == PickOne({step \in KnownSteps(r) :
                         flows.step_status[r][step] = Some("Dispatched") /\
                         flows.step_collection_policy[r][step] # "Quorum"})
         boundWork == BoundLiveWork(r, s)
         runBoundLiveWork == BoundLiveWorkForRun(r)
         allTerminal ==
            \A step \in KnownSteps(r) :
              IF step = s THEN TRUE
              ELSE flows.step_status[r][step] \in {Some("Completed"), Some("Failed"), Some("Skipped"), Some("Canceled")}
         clearedBindings ==
           [t \in TaskIdValues |->
              IF tasks.run_binding[t] = Some(r) THEN None ELSE tasks.run_binding[t]]
     IN
       /\ flows' =
            IF allTerminal THEN
              [flows EXCEPT
                !.step_status[r][s] = Some("Completed"),
                !.status[r] = Some("Completed"),
                !.completed_at_present[r] = TRUE]
            ELSE
              [flows EXCEPT !.step_status[r][s] = Some("Completed")]
       /\ work' =
            IF allTerminal THEN
              [work EXCEPT
                !.status =
                  [x \in WorkRefValues |->
                     IF x \in runBoundLiveWork THEN
                       IF x \in boundWork THEN "Completed" ELSE "Canceled"
                     ELSE work.status[x]],
                !.terminal =
                  [x \in WorkRefValues |->
                     IF x \in runBoundLiveWork THEN
                       IF x \in boundWork THEN Some("WorkCompleted") ELSE Some("WorkCanceled")
                     ELSE work.terminal[x]]]
            ELSE IF boundWork = {} THEN
              work
            ELSE
              [work EXCEPT
                !.status =
                  [x \in WorkRefValues |->
                     IF x \in boundWork THEN "Completed" ELSE work.status[x]],
                !.terminal =
                  [x \in WorkRefValues |->
                     IF x \in boundWork THEN Some("WorkCompleted") ELSE work.terminal[x]]]
       /\ lifecycle' =
            IF allTerminal THEN
              [lifecycle EXCEPT !.active_run_count = @ - 1]
            ELSE
              lifecycle
       /\ tasks' =
            IF allTerminal THEN
              [tasks EXCEPT !.run_binding = clearedBindings]
            ELSE
              tasks
       /\ UNCHANGED << identity, roster, topology, provisioning, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

MarkStepFailed ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ \E s \in KnownSteps(r) : flows.step_status[r][s] = Some("Dispatched")
  /\ LET r == PickOne({run \in flows.known_runs : flows.status[run] = Some("Running") /\ \E s \in KnownSteps(run) : flows.step_status[run][s] = Some("Dispatched")})
         s == PickOne({step \in KnownSteps(r) : flows.step_status[r][step] = Some("Dispatched")})
         thresholdReached == Inc(flows.failure_count[r]) > flows.max_step_retries[r]
         boundWork == BoundLiveWork(r, s)
         runBoundLiveWork == BoundLiveWorkForRun(r)
         clearedBindings ==
           [t \in TaskIdValues |->
              IF tasks.run_binding[t] = Some(r) THEN None ELSE tasks.run_binding[t]]
     IN
       /\ flows' =
            IF thresholdReached THEN
              [flows EXCEPT
                !.step_status[r] = TerminalizeIncompleteRunSteps([@ EXCEPT ![s] = Some("Failed")], r, Some("Canceled")),
                !.failure_count[r] = Inc(@),
                !.consecutive_failure_count[r] = Inc(@),
                !.status[r] = Some("Failed"),
                !.completed_at_present[r] = TRUE]
            ELSE
              [flows EXCEPT
                !.step_status[r][s] = Some("Failed"),
                !.failure_count[r] = Inc(@),
                !.consecutive_failure_count[r] = Inc(@)]
       /\ work' =
            IF thresholdReached THEN
              [work EXCEPT
                !.status =
                  [x \in WorkRefValues |->
                     IF x \in runBoundLiveWork THEN "Failed" ELSE work.status[x]],
                !.terminal =
                  [x \in WorkRefValues |->
                     IF x \in runBoundLiveWork THEN Some("WorkFailed") ELSE work.terminal[x]]]
            ELSE IF boundWork = {} THEN
              work
            ELSE
              [work EXCEPT
                !.status =
                  [x \in WorkRefValues |->
                     IF x \in boundWork THEN "Failed" ELSE work.status[x]],
                !.terminal =
                  [x \in WorkRefValues |->
                     IF x \in boundWork THEN Some("WorkFailed") ELSE work.terminal[x]]]
       /\ lifecycle' =
            IF thresholdReached THEN
              [lifecycle EXCEPT !.active_run_count = @ - 1]
            ELSE
              lifecycle
       /\ tasks' =
            IF thresholdReached THEN
              [tasks EXCEPT !.run_binding = clearedBindings]
            ELSE
              tasks
       /\ UNCHANGED << identity, roster, topology, provisioning, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

MarkStepSkipped ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ \E s \in KnownSteps(r) : flows.step_status[r][s] = None
  /\ LET r == PickOne({run \in flows.known_runs : flows.status[run] = Some("Running") /\ \E s \in KnownSteps(run) : flows.step_status[run][s] = None})
         s == PickOne({step \in KnownSteps(r) : flows.step_status[r][step] = None})
         boundWork == BoundLiveWork(r, s)
     IN
       /\ flows' = [flows EXCEPT !.step_status[r][s] = Some("Skipped")]
       /\ work' =
            IF boundWork = {} THEN
              work
            ELSE
              [work EXCEPT
                !.status =
                  [x \in WorkRefValues |->
                     IF x \in boundWork THEN "Canceled" ELSE work.status[x]],
                !.terminal =
                  [x \in WorkRefValues |->
                     IF x \in boundWork THEN Some("WorkCanceled") ELSE work.terminal[x]]]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

MarkStepCanceled ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ \E s \in KnownSteps(r) : flows.step_status[r][s] \in {None, Some("Dispatched")}
  /\ LET r == PickOne({run \in flows.known_runs : flows.status[run] = Some("Running") /\ \E s \in KnownSteps(run) : flows.step_status[run][s] \in {None, Some("Dispatched")}})
         s == PickOne({step \in KnownSteps(r) : flows.step_status[r][step] \in {None, Some("Dispatched")}})
         boundWork == BoundLiveWork(r, s)
     IN
       /\ flows' = [flows EXCEPT !.step_status[r][s] = Some("Canceled")]
       /\ work' =
            IF boundWork = {} THEN
              work
            ELSE
              [work EXCEPT
                !.status =
                  [x \in WorkRefValues |->
                     IF x \in boundWork THEN "Canceled" ELSE work.status[x]],
                !.terminal =
                  [x \in WorkRefValues |->
                     IF x \in boundWork THEN Some("WorkCanceled") ELSE work.terminal[x]]]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

OpenFrame ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ LoopStructuredRun(r)
       /\ BoundLiveWorkForRun(r) = {}
       /\ flows.frame_count[r] = 0
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] = Some("Running") /\
                    LoopStructuredRun(run) /\
                    BoundLiveWorkForRun(run) = {} /\
                    flows.frame_count[run] = 0})
     IN
       /\ flows' =
            [flows EXCEPT
              !.frame_count[r] = Inc(@),
              !.schema_version[r] = IF @ < 4 THEN 4 ELSE @]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

CloseFrame ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ LoopStructuredRun(r)
       /\ flows.frame_count[r] > 0
       /\ flows.loop_count[r] = 0
       /\ BoundLiveWorkForRun(r) = {}
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] = Some("Running") /\
                    LoopStructuredRun(run) /\
                    flows.frame_count[run] > 0 /\
                    flows.loop_count[run] = 0 /\
                    BoundLiveWorkForRun(run) = {}})
     IN
       /\ flows' = [flows EXCEPT !.frame_count[r] = @ - 1]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

OpenLoop ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ LoopStructuredRun(r)
       /\ flows.frame_count[r] > 0
       /\ flows.loop_count[r] = 0
       /\ StepBody \in KnownSteps(r)
       /\ BoundLiveWorkForRun(r) = {}
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] = Some("Running") /\
                    LoopStructuredRun(run) /\
                    flows.frame_count[run] > 0 /\
                    flows.loop_count[run] = 0 /\
                    StepBody \in KnownSteps(run) /\
                    BoundLiveWorkForRun(run) = {}})
     IN
       /\ flows' = [flows EXCEPT !.loop_count[r] = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

AdvanceLoopIteration ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ LoopStructuredRun(r)
       /\ flows.loop_count[r] > 0
       /\ BoundLiveWorkForRun(r) = {}
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] = Some("Running") /\
                    LoopStructuredRun(run) /\
                    flows.loop_count[run] > 0 /\
                    BoundLiveWorkForRun(run) = {}})
     IN
       /\ flows' = [flows EXCEPT !.loop_iteration_count[r] = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

CloseLoop ==
    /\ \E r \in flows.known_runs :
       /\ flows.status[r] = Some("Running")
       /\ LoopStructuredRun(r)
       /\ flows.loop_count[r] > 0
       /\ BoundLiveWorkForRun(r) = {}
  /\ LET r ==
         PickOne({run \in flows.known_runs :
                    flows.status[run] = Some("Running") /\
                    LoopStructuredRun(run) /\
                    flows.loop_count[run] > 0 /\
                    BoundLiveWorkForRun(run) = {}})
     IN
       /\ flows' =
            [flows EXCEPT
              !.loop_count[r] = @ - 1,
              !.loop_iteration_count[r] =
                IF flows.loop_count[r] = 1 THEN 0 ELSE @]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, tasks, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_runs = {r}]
       /\ step_count' = Inc(step_count)

OpenTask ==
    /\ AvailableTasks # {}
  /\ identity.known # {}
  /\ LET t == PickOne(AvailableTasks)
         id == PickOne(identity.known)
     IN
       /\ tasks' =
            [tasks EXCEPT
              !.known = @ \cup {t},
              !.status[t] = "Pending",
              !.owner[t] = Some(id)]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, flows, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_tasks = TRUE]
       /\ step_count' = Inc(step_count)

UpdateTask ==
    /\ \E t \in tasks.known : tasks.status[t] \in {"Pending", "InProgress", "Blocked"}
  /\ LET t == PickOne({task \in tasks.known : tasks.status[task] \in {"Pending", "InProgress", "Blocked"}}) IN
       /\ tasks' =
            [tasks EXCEPT
              !.status[t] =
                IF @ = "Pending" THEN "InProgress"
                ELSE IF @ = "InProgress" THEN "Blocked"
                ELSE @,
              !.run_binding[t] =
                IF @ = None /\ {r \in flows.known_runs : ~TerminalRun(r)} # {}
                  THEN Some(PickOne({r \in flows.known_runs : ~TerminalRun(r)}))
                ELSE IF @ # None /\ TerminalRun(@.value) THEN None
                ELSE @]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, flows, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_tasks = TRUE]
       /\ step_count' = Inc(step_count)

CloseTask ==
    /\ \E t \in tasks.known : tasks.status[t] \in {"Pending", "InProgress", "Blocked"}
  /\ LET t == PickOne({task \in tasks.known : tasks.status[task] \in {"Pending", "InProgress", "Blocked"}}) IN
       /\ tasks' = [tasks EXCEPT !.status[t] = "Completed", !.run_binding[t] = None]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, flows, recovery, history >>
       /\ effects' = [NoEffects EXCEPT !.persisted_tasks = TRUE]
       /\ step_count' = Inc(step_count)

AppendHistoryEvent ==
    /\ identity.known # {}
  /\ LET id == PickOne(identity.known) IN
       /\ history' =
            [history EXCEPT
              !.next_seq = Inc(@),
              !.event_count_by_identity[id] = Inc(@),
              !.last_seq_by_identity[id] = Inc(@)]
       /\ UNCHANGED << identity, lifecycle, roster, topology, provisioning, work, flows, tasks, recovery >>
       /\ effects' = [NoEffects EXCEPT !.published_events = 1, !.persisted_history = TRUE]
       /\ step_count' = Inc(step_count)

RecordRestoreFailure ==
    /\ identity.known # {}
  /\ LET id == PickOne(identity.known) IN
       /\ provisioning' =
            [provisioning EXCEPT
              !.kickoff_pending = @ \ {id},
              !.restore_failed = @ \cup {id},
              !.restore_reason_present[id] = TRUE]
       /\ recovery' =
            [recovery EXCEPT !.restore_failures = @ \cup {id}]
       /\ roster' =
            IF Rostered(id) THEN [roster EXCEPT !.member_status[id] = "Broken"] ELSE roster
       /\ topology' =
            [topology EXCEPT
              !.coordinator = IF @ # None /\ @.value = id THEN None ELSE @,
              !.external_specs_present = @ \ {id}]
       /\ UNCHANGED << identity, lifecycle, work, flows, tasks, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

ClearRestoreFailure ==
    /\ recovery.restore_failures # {}
  /\ LET id == PickOne(recovery.restore_failures) IN
       /\ provisioning' =
            [provisioning EXCEPT
              !.restore_failed = @ \ {id},
              !.restore_reason_present[id] = FALSE]
       /\ recovery' =
            [recovery EXCEPT !.restore_failures = @ \ {id}]
       /\ roster' =
            IF Rostered(id) /\ identity.authority[id] = "Active"
              THEN [roster EXCEPT !.member_status[id] = "Active"]
              ELSE roster
       /\ UNCHANGED << identity, lifecycle, topology, work, flows, tasks, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

AdvanceCheckpointVersion ==
    /\ identity.known # {}
  /\ LET id == PickOne(identity.known) IN
       /\ identity' = [identity EXCEPT !.checkpoint_version[id] = Inc(@)]
       /\ recovery' = [recovery EXCEPT !.checkpoint_version[id] = Inc(@), !.continuity_bound[id] = TRUE]
       /\ UNCHANGED << lifecycle, roster, topology, provisioning, work, flows, tasks, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

ResolveKickoff ==
    /\ provisioning.kickoff_pending # {}
  /\ LET id == PickOne(provisioning.kickoff_pending) IN
       /\ provisioning' = [provisioning EXCEPT !.kickoff_pending = @ \ {id}]
       /\ UNCHANGED << identity, lifecycle, roster, topology, work, flows, tasks, recovery, history >>
       /\ effects' = NoEffects
       /\ step_count' = Inc(step_count)

StopMob ==
    /\ lifecycle.phase = "Running"
  /\ lifecycle.active_run_count = 0
  /\ provisioning.kickoff_pending = {}
  /\ lifecycle' = [lifecycle EXCEPT !.phase = "Stopped", !.cleanup_pending = FALSE]
  /\ UNCHANGED << identity, roster, topology, provisioning, work, flows, tasks, recovery, history >>
  /\ effects' = NoEffects
  /\ step_count' = Inc(step_count)

CompleteMob ==
    /\ lifecycle.phase \in {"Running", "Stopped"}
  /\ lifecycle.active_run_count = 0
  /\ provisioning.kickoff_pending = {}
  /\ lifecycle' = [lifecycle EXCEPT !.phase = "Completed", !.cleanup_pending = FALSE]
  /\ UNCHANGED << identity, roster, topology, provisioning, work, flows, tasks, recovery, history >>
  /\ effects' = NoEffects
  /\ step_count' = Inc(step_count)

DestroyMob ==
    /\ lifecycle.phase \in {"Stopped", "Completed"}
  /\ lifecycle.active_run_count = 0
  /\ roster.present = {}
  /\ provisioning.pending_spawn = {}
  /\ provisioning.kickoff_pending = {}
  /\ {w \in work.known : work.status[w] \in LiveWorkStatuses} = {}
  /\ lifecycle' = [lifecycle EXCEPT !.phase = "Destroyed", !.cleanup_pending = FALSE]
  /\ UNCHANGED << identity, roster, topology, provisioning, work, flows, tasks, recovery, history >>
  /\ effects' = NoEffects
  /\ step_count' = Inc(step_count)

OperationalNext ==
  \/ RegisterIdentity
  \/ ProvisionIncarnationFresh
  \/ ProvisionIncarnationRecover
  \/ FinalizeSpawn
  \/ FailProvision
  \/ RetireMember
  \/ ResetMember
  \/ DestroyMember
  \/ BindCoordinator
  \/ AddTopologyEdge
  \/ RemoveTopologyEdge
  \/ PublishExternalSpec
  \/ ClearExternalSpec
  \/ SubmitWork
  \/ AcceptWork
  \/ StartWork
  \/ RejectWork
  \/ CompleteWork
  \/ FailWork
  \/ CancelWork
  \/ CancelAllWork
  \/ StartFlowRun
  \/ TrackRun
  \/ CompleteRun
  \/ FailRun
  \/ CancelRun
  \/ FailRunNoDispatchCapacity
  \/ DispatchStep
  \/ ResolveAnyJoin
  \/ RecordCollectionContribution
  \/ ResolveCollection
  \/ MarkStepCompleted
  \/ MarkStepFailed
  \/ MarkStepSkipped
  \/ MarkStepCanceled
  \/ OpenFrame
  \/ CloseFrame
  \/ OpenLoop
  \/ AdvanceLoopIteration
  \/ CloseLoop
  \/ OpenTask
  \/ UpdateTask
  \/ CloseTask
  \/ AppendHistoryEvent
  \/ RecordRestoreFailure
  \/ ClearRestoreFailure
  \/ AdvanceCheckpointVersion
  \/ ResolveKickoff
  \/ StopMob
  \/ CompleteMob
  \/ DestroyMob

Next ==
  \/ /\ lifecycle.phase = "Destroyed"
     /\ UNCHANGED vars
  \/ /\ lifecycle.phase = "Stopped"
     /\ (CompleteMob \/ DestroyMob)
  \/ /\ lifecycle.phase = "Completed"
     /\ DestroyMob
  \/ /\ lifecycle.phase \in {"Creating", "Running"}
     /\ OperationalNext

ProvisionProgress ==
  \/ FinalizeSpawn
  \/ FailProvision

KickoffProgress ==
  \/ ResolveKickoff
  \/ RetireMember
  \/ ResetMember
  \/ DestroyMember
  \/ RecordRestoreFailure

WorkProgress ==
  \/ StartWork
  \/ RejectWork
  \/ CompleteWork
  \/ FailWork
  \/ CancelWork
  \/ CancelAllWork

WorkHarnessNext ==
  \/ RegisterIdentity
  \/ ProvisionIncarnationFresh
  \/ ProvisionIncarnationRecover
  \/ FinalizeSpawn
  \/ FailProvision
  \/ SubmitWork
  \/ AcceptWork
  \/ StartWork
  \/ RejectWork
  \/ CompleteWork
  \/ FailWork
  \/ CancelWork
  \/ CancelAllWork

RecoveryHarnessNext ==
  \/ RegisterIdentity
  \/ RecordRestoreFailure
  \/ ClearRestoreFailure
  \/ AdvanceCheckpointVersion

TaskHarnessNext ==
  \/ RegisterIdentity
  \/ OpenTask
  \/ UpdateTask
  \/ CloseTask

LifecycleOperationalNext ==
  \/ RegisterIdentity
  \/ ProvisionIncarnationFresh
  \/ ProvisionIncarnationRecover
  \/ FinalizeSpawn
  \/ FailProvision
  \/ ResolveKickoff
  \/ RetireMember
  \/ ResetMember
  \/ DestroyMember
  \/ RecordRestoreFailure
  \/ StopMob
  \/ CompleteMob
  \/ DestroyMob

LifecycleHarnessNext ==
  \/ /\ lifecycle.phase = "Destroyed"
     /\ UNCHANGED vars
  \/ /\ lifecycle.phase = "Stopped"
     /\ (CompleteMob \/ DestroyMob)
  \/ /\ lifecycle.phase = "Completed"
     /\ DestroyMob
  \/ /\ lifecycle.phase \in {"Creating", "Running"}
     /\ LifecycleOperationalNext

FlowHarnessNext ==
  \/ RegisterIdentity
  \/ ProvisionIncarnationFresh
  \/ ProvisionIncarnationRecover
  \/ FinalizeSpawn
  \/ FailProvision
  \/ StartFlowRun
  \/ TrackRun
  \/ AcceptWork
  \/ StartWork
  \/ RejectWork
  \/ CompleteWork
  \/ FailWork
  \/ CancelWork
  \/ CancelAllWork
  \/ DispatchStep
  \/ ResolveAnyJoin
  \/ RecordCollectionContribution
  \/ ResolveCollection
  \/ MarkStepCompleted
  \/ MarkStepFailed
  \/ MarkStepSkipped
  \/ MarkStepCanceled
  \/ OpenFrame
  \/ CloseFrame
  \/ OpenLoop
  \/ AdvanceLoopIteration
  \/ CloseLoop
  \/ CompleteRun
  \/ FailRun
  \/ CancelRun
  \/ FailRunNoDispatchCapacity

FlowProgress ==
  \/ DispatchStep
  \/ ResolveAnyJoin
  \/ RecordCollectionContribution
  \/ ResolveCollection
  \/ MarkStepCompleted
  \/ MarkStepFailed
  \/ MarkStepSkipped
  \/ MarkStepCanceled
  \/ OpenFrame
  \/ CloseFrame
  \/ OpenLoop
  \/ AdvanceLoopIteration
  \/ CloseLoop
  \/ CompleteRun
  \/ FailRun
  \/ CancelRun
  \/ FailRunNoDispatchCapacity

FlowDispatchProgress ==
  \/ DispatchStep
  \/ ResolveAnyJoin
  \/ RecordCollectionContribution
  \/ ResolveCollection

FlowTerminalProgress ==
  \/ MarkStepCompleted
  \/ MarkStepFailed
  \/ MarkStepSkipped
  \/ MarkStepCanceled
  \/ CompleteRun
  \/ FailRun
  \/ CancelRun
  \/ FailRunNoDispatchCapacity

FlowStructureProgress ==
  \/ OpenFrame
  \/ CloseFrame
  \/ OpenLoop
  \/ AdvanceLoopIteration
  \/ CloseLoop

RecoveryProgress ==
  \/ ClearRestoreFailure
  \/ RecordRestoreFailure
  \/ AdvanceCheckpointVersion

HistoryTaskProgress ==
  \/ AppendHistoryEvent
  \/ OpenTask
  \/ UpdateTask
  \/ CloseTask

Spec == Init /\ [][Next]_vars
LifecycleSpec == Init /\ [][LifecycleHarnessNext]_vars
RecoverySpec == Init /\ [][RecoveryHarnessNext]_vars
TaskSpec == Init /\ [][TaskHarnessNext]_vars
WorkSpec == Init /\ [][WorkHarnessNext]_vars
FlowSpec == Init /\ [][FlowHarnessNext]_vars

TargetFairness ==
  /\ WF_vars(ProvisionProgress)
  /\ WF_vars(KickoffProgress)
  /\ WF_vars(WorkProgress)
  /\ WF_vars(FlowDispatchProgress)
  /\ WF_vars(FlowTerminalProgress)
  /\ WF_vars(FlowStructureProgress)
  /\ WF_vars(RecoveryProgress)
  /\ WF_vars(HistoryTaskProgress)

FairSpec == Spec /\ TargetFairness
LifecycleFairness ==
  /\ WF_vars(ProvisionProgress)
  /\ WF_vars(KickoffProgress)

LifecycleFairSpec == LifecycleSpec /\ LifecycleFairness
RecoveryFairness ==
  /\ WF_vars(ClearRestoreFailure)
  /\ WF_vars(AdvanceCheckpointVersion)

RecoveryFairSpec == RecoverySpec /\ RecoveryFairness
TaskFairness ==
  /\ WF_vars(CloseTask)

TaskFairSpec == TaskSpec /\ TaskFairness
WorkFairness ==
  /\ WF_vars(ProvisionProgress)
  /\ WF_vars(WorkProgress)

WorkFairSpec == WorkSpec /\ WorkFairness
FlowFairness ==
  /\ WF_vars(ProvisionProgress)
  /\ WF_vars(WorkProgress)
  /\ WF_vars(FlowDispatchProgress)
  /\ WF_vars(FlowTerminalProgress)
  /\ WF_vars(FlowStructureProgress)

FlowFairSpec == FlowSpec /\ FlowFairness

DestroyedAbsorbingProp ==
  [](lifecycle.phase = "Destroyed" => [](lifecycle.phase = "Destroyed"))

PendingSpawnEventuallyClearsProp ==
  \A id \in AgentIdentityValues :
    [](id \in provisioning.pending_spawn => <>(id \notin provisioning.pending_spawn))

KickoffEventuallyClearsProp ==
  \A id \in AgentIdentityValues :
    [](id \in provisioning.kickoff_pending => <>(id \notin provisioning.kickoff_pending))

RestoreFailureEventuallyClearsProp ==
  \A id \in AgentIdentityValues :
    [](id \in recovery.restore_failures => <>(id \notin recovery.restore_failures))

LiveTaskEventuallyClosesProp ==
  \A t \in TaskIdValues :
    []((HasTask(t) /\ tasks.status[t] \in {"Pending", "InProgress", "Blocked"}) => <>(tasks.status[t] = "Completed"))

AcceptedWorkEventuallyLeavesAcceptedProp ==
  \A w \in WorkRefValues :
    []((w \in work.known /\ work.status[w] = "Accepted") => <>(work.status[w] # "Accepted"))

RunningWorkEventuallyTerminalizesProp ==
  \A w \in WorkRefValues :
    []((w \in work.known /\ work.status[w] = "Running") => <>(work.status[w] \in TerminalWorkStatuses))

PendingStepEventuallyLeavesPendingProp ==
  \A r \in RunIdValues :
    \A s \in StepIdValues :
      []((KnownRun(r) /\ s \in KnownSteps(r) /\ flows.step_status[r][s] = Some("Pending")) => <>(flows.step_status[r][s] # Some("Pending")))

DispatchedStepEventuallyLeavesDispatchedProp ==
  \A r \in RunIdValues :
    \A s \in StepIdValues :
      []((KnownRun(r) /\ s \in KnownSteps(r) /\ flows.step_status[r][s] = Some("Dispatched")) => <>(flows.step_status[r][s] # Some("Dispatched")))

StateConstraint == step_count < MaxSteps

====
