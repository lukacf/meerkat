---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for workgraph_flow_bundle.

CONSTANTS BooleanValues, NatValues, StringValues, WorkExecutionEvidenceKindValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionWorkExecutionEvidenceKindValues == {None} \cup {Some(x) : x \in WorkExecutionEvidenceKindValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
Count(seq, value) == Cardinality({i \in DOMAIN seq : seq[i] = value})
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))
AppendIfMissing(seq, value) == IF value \in SeqElements(seq) THEN seq ELSE Append(seq, value)
Machines == {
    <<"work_execution", "WorkExecutionLifecycleMachine", "work_execution_authority">>
}

RouteNames == {
}

Actors == {
    "work_execution_authority"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "work_execution" -> "work_execution_authority"

RouteSource(route_name) ==
    "unresolved_route_source_machine"

RouteEffect(route_name) ==
    "unresolved_route_effect"

RouteTargetMachine(route_name) ==
    "unresolved_route_target_machine"

RouteTargetInput(route_name) ==
    "unresolved_route_target_input"

RouteTargetKind(route_name) ==
    "Unknown"

RouteDeliveryKind(route_name) ==
    "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_01c5c5c26fe104c3 == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_0a24b0a9815bff81 == UNCHANGED << work_execution_binding_id, work_execution_run_id, work_execution_last_failure_detail, work_execution_evidence_kind, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_1eb4735f61fd60ac == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_200ffc0060c74508 == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_315673c263c47bfd == UNCHANGED << work_execution_binding_id, work_execution_run_id, work_execution_evidence_kind, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_3de6261a21bd344d == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_5dd7d648a23596a6 == UNCHANGED << work_execution_binding_id, work_execution_run_id, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_6da688a13f8a6f43 == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_71eb061ce3c69fe6 == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_79f3e86b7290e9f5 == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_7ca0a3794d2bfdc6 == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_932d08003e4e619d == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_a2479c921439746b == UNCHANGED << obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_a2a13c8cc26dadb5 == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_aadcaeaaa5488654 == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_b20dd0068997aa3e == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_b4a6d6a85f2cba01 == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_bcb8917e76f5fc09 == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_d029cf0d3e83cf55 == UNCHANGED << work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_d063a6d561bc8142 == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection >>
UnchangedFrame_d1d90666ffe4859e == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_d2e8cf6c034eddd0 == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_e7d524a4c3f938ee == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_eb1b34b9f4544149 == UNCHANGED << work_execution_phase, work_execution_binding_id, work_execution_run_id, work_execution_revision, work_execution_last_failure_detail, work_execution_evidence_kind, obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_ef196b98e0ca2516 == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_cancellation_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_fc1db005e292de48 == UNCHANGED << obligation_work_execution_work_execution_flow_launch, obligation_work_execution_work_execution_flow_observation, obligation_work_execution_work_execution_uncertain_launch_resolution, obligation_work_execution_work_execution_quarantined_launch_resolution, obligation_work_execution_work_execution_success_evidence_projection, obligation_work_execution_work_execution_failure_evidence_projection, obligation_work_execution_work_execution_launch_failure_evidence_projection, obligation_work_execution_work_execution_work_closure >>
UnchangedFrame_ff767348ef3f1efe == UNCHANGED << witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ work_execution_phase = "Absent"
    /\ work_execution_binding_id = ""
    /\ work_execution_run_id = ""
    /\ work_execution_revision = 0
    /\ work_execution_last_failure_detail = None
    /\ work_execution_evidence_kind = None
    /\ obligation_work_execution_work_execution_flow_launch = {}
    /\ obligation_work_execution_work_execution_flow_observation = {}
    /\ obligation_work_execution_work_execution_uncertain_launch_resolution = {}
    /\ obligation_work_execution_work_execution_quarantined_launch_resolution = {}
    /\ obligation_work_execution_work_execution_success_evidence_projection = {}
    /\ obligation_work_execution_work_execution_failure_evidence_projection = {}
    /\ obligation_work_execution_work_execution_cancellation_evidence_projection = {}
    /\ obligation_work_execution_work_execution_launch_failure_evidence_projection = {}
    /\ obligation_work_execution_work_execution_work_closure = {}
    /\ model_step_count = 0
    /\ pending_routes = <<>>
    /\ delivered_routes = {}
    /\ emitted_effects = {}
    /\ observed_transitions = {}

Init ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_workgraph_flow_success_closure ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_success_closure:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_success_closure:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_success_closure:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_workgraph_flow_failure_evidence ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_failure_evidence:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_failure_evidence:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_failure_evidence:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_workgraph_flow_cancellation_evidence ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_cancellation_evidence:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_cancellation_evidence:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_cancellation_evidence:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_workgraph_flow_uncertain_abandonment ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_uncertain_abandonment:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_uncertain_abandonment:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_uncertain_abandonment:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_workgraph_flow_launch_quarantine ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_launch_quarantine:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_launch_quarantine:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> "binding_1", run_id |-> "run_1"], source_kind |-> "entry", source_route |-> "witness:workgraph_flow_launch_quarantine:1", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<>>

work_execution_BindExecution(arg_binding_id, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Bind"
       /\ packet.payload.binding_id = arg_binding_id
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Absent"
       /\ ((packet.payload.binding_id # "") /\ (packet.payload.run_id # ""))
       /\ work_execution_phase' = "LaunchRequested"
       /\ work_execution_binding_id' = packet.payload.binding_id
       /\ work_execution_run_id' = packet.payload.run_id
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = None
       /\ work_execution_evidence_kind' = None
       /\ UnchangedFrame_ff767348ef3f1efe
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchRequested", payload |-> [binding_id |-> packet.payload.binding_id, run_id |-> packet.payload.run_id], effect_id |-> (model_step_count + 1), source_transition |-> "BindExecution"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "BindExecution", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchRequested"]}
       /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \cup {[effect_id |-> (model_step_count + 1), binding_id |-> packet.payload.binding_id, run_id |-> packet.payload.run_id]}
       /\ UnchangedFrame_a2479c921439746b
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverLaunchRequest ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchRequested"
       /\ work_execution_phase' = "LaunchRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchRequested", payload |-> [binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverLaunchRequest"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverLaunchRequest", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchRequested"]}
       /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id]}
       /\ UnchangedFrame_a2479c921439746b
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverUncertainLaunch ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchUncertain"
       /\ work_execution_phase' = "LaunchUncertain"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchUncertain", payload |-> [binding_id |-> work_execution_binding_id, detail |-> (IF "value" \in DOMAIN work_execution_last_failure_detail THEN work_execution_last_failure_detail["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverUncertainLaunch"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverUncertainLaunch", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchUncertain"]}
       /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, detail |-> (IF "value" \in DOMAIN work_execution_last_failure_detail THEN work_execution_last_failure_detail["value"] ELSE None)]}
       /\ UnchangedFrame_b4a6d6a85f2cba01
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverQuarantinedLaunch ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchQuarantined"
       /\ work_execution_phase' = "LaunchQuarantined"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchQuarantined", payload |-> [binding_id |-> work_execution_binding_id, detail |-> (IF "value" \in DOMAIN work_execution_last_failure_detail THEN work_execution_last_failure_detail["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverQuarantinedLaunch"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverQuarantinedLaunch", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchQuarantined"]}
       /\ obligation_work_execution_work_execution_quarantined_launch_resolution' = obligation_work_execution_work_execution_quarantined_launch_resolution \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, detail |-> (IF "value" \in DOMAIN work_execution_last_failure_detail THEN work_execution_last_failure_detail["value"] ELSE None)]}
       /\ UnchangedFrame_ef196b98e0ca2516
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Running"
       /\ work_execution_phase' = "Running"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchAccepted", payload |-> [binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverRunning", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "Running"]}
       /\ obligation_work_execution_work_execution_flow_observation' = obligation_work_execution_work_execution_flow_observation \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id]}
       /\ UnchangedFrame_e7d524a4c3f938ee
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverEvidenceProjection ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "EvidenceProjectionRequested"
       /\ work_execution_phase' = "EvidenceProjectionRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "EvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, kind |-> (IF "value" \in DOMAIN work_execution_evidence_kind THEN work_execution_evidence_kind["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverEvidenceProjection"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverEvidenceProjection", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "EvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_success_evidence_projection' = obligation_work_execution_work_execution_success_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, kind |-> (IF "value" \in DOMAIN work_execution_evidence_kind THEN work_execution_evidence_kind["value"] ELSE None)]}
       /\ UnchangedFrame_200ffc0060c74508
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverWorkClosure ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "WorkClosureRequested"
       /\ work_execution_phase' = "WorkClosureRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "WorkClosureRequested", payload |-> [binding_id |-> work_execution_binding_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverWorkClosure"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverWorkClosure", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "WorkClosureRequested"]}
       /\ obligation_work_execution_work_execution_work_closure' = obligation_work_execution_work_execution_work_closure \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id]}
       /\ UnchangedFrame_d063a6d561bc8142
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverFlowFailureEvidenceProjection ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "FailureEvidenceProjectionRequested"
       /\ work_execution_phase' = "FailureEvidenceProjectionRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowFailureEvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, kind |-> (IF "value" \in DOMAIN work_execution_evidence_kind THEN work_execution_evidence_kind["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFlowFailureEvidenceProjection"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverFlowFailureEvidenceProjection", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FailureEvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_failure_evidence_projection' = obligation_work_execution_work_execution_failure_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, kind |-> (IF "value" \in DOMAIN work_execution_evidence_kind THEN work_execution_evidence_kind["value"] ELSE None)]}
       /\ UnchangedFrame_3de6261a21bd344d
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverFlowCancellationEvidenceProjection ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "CancellationEvidenceProjectionRequested"
       /\ work_execution_phase' = "CancellationEvidenceProjectionRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowCancellationEvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, kind |-> (IF "value" \in DOMAIN work_execution_evidence_kind THEN work_execution_evidence_kind["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFlowCancellationEvidenceProjection"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverFlowCancellationEvidenceProjection", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "CancellationEvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_cancellation_evidence_projection' = obligation_work_execution_work_execution_cancellation_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, kind |-> (IF "value" \in DOMAIN work_execution_evidence_kind THEN work_execution_evidence_kind["value"] ELSE None)]}
       /\ UnchangedFrame_fc1db005e292de48
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverLaunchFailureEvidenceProjection ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchFailureEvidenceProjectionRequested"
       /\ work_execution_phase' = "LaunchFailureEvidenceProjectionRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "LaunchFailureEvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, detail |-> (IF "value" \in DOMAIN work_execution_last_failure_detail THEN work_execution_last_failure_detail["value"] ELSE None), kind |-> (IF "value" \in DOMAIN work_execution_evidence_kind THEN work_execution_evidence_kind["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverLaunchFailureEvidenceProjection"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverLaunchFailureEvidenceProjection", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchFailureEvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_launch_failure_evidence_projection' = obligation_work_execution_work_execution_launch_failure_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, detail |-> (IF "value" \in DOMAIN work_execution_last_failure_detail THEN work_execution_last_failure_detail["value"] ELSE None), kind |-> (IF "value" \in DOMAIN work_execution_evidence_kind THEN work_execution_evidence_kind["value"] ELSE None)]}
       /\ UnchangedFrame_bcb8917e76f5fc09
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverFlowFailure ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "FlowFailed"
       /\ work_execution_phase' = "FlowFailed"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowFailed", payload |-> [binding_id |-> work_execution_binding_id, detail |-> work_execution_last_failure_detail, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFlowFailure"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverFlowFailure", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FlowFailed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverFlowCancellation ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "FlowCanceled"
       /\ work_execution_phase' = "FlowCanceled"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowCanceled", payload |-> [binding_id |-> work_execution_binding_id, detail |-> work_execution_last_failure_detail, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFlowCancellation"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverFlowCancellation", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FlowCanceled"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverEvidenceProjected ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "EvidenceProjected"
       /\ work_execution_phase' = "EvidenceProjected"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "EvidenceProjected", payload |-> [binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverEvidenceProjected"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverEvidenceProjected", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "EvidenceProjected"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverClosedWork ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "WorkClosed"
       /\ work_execution_phase' = "WorkClosed"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "WorkClosed", payload |-> [binding_id |-> work_execution_binding_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverClosedWork"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverClosedWork", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "WorkClosed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_RecoverLaunchFailure ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchFailed"
       /\ work_execution_phase' = "LaunchFailed"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "LaunchFailed", payload |-> [binding_id |-> work_execution_binding_id, detail |-> (IF "value" \in DOMAIN work_execution_last_failure_detail THEN work_execution_last_failure_detail["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverLaunchFailure"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecoverLaunchFailure", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchFailed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_AcceptFlowLaunch ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ConfirmFlowStarted"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined"
       /\ work_execution_phase' = "Running"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = None
       /\ work_execution_evidence_kind' = None
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchAccepted", payload |-> [binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptFlowLaunch"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "AcceptFlowLaunch", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "Running"]}
       /\ obligation_work_execution_work_execution_flow_observation' = obligation_work_execution_work_execution_flow_observation \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id]}
       /\ UnchangedFrame_e7d524a4c3f938ee
       /\ model_step_count' = model_step_count + 1


work_execution_ObserveRunningFlow ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ObserveFlowRunning"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Running" \/ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined"
       /\ work_execution_phase' = "Running"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = None
       /\ work_execution_evidence_kind' = None
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchAccepted", payload |-> [binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveRunningFlow"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ObserveRunningFlow", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "Running"]}
       /\ obligation_work_execution_work_execution_flow_observation' = obligation_work_execution_work_execution_flow_observation \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id]}
       /\ UnchangedFrame_e7d524a4c3f938ee
       /\ model_step_count' = model_step_count + 1


work_execution_ObserveCompletedFlow ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ObserveFlowCompleted"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Running" \/ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined"
       /\ work_execution_phase' = "EvidenceProjectionRequested"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = None
       /\ work_execution_evidence_kind' = Some("Completed")
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "EvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, kind |-> (IF "value" \in DOMAIN Some("Completed") THEN Some("Completed")["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCompletedFlow"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ObserveCompletedFlow", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "EvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_success_evidence_projection' = obligation_work_execution_work_execution_success_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, kind |-> (IF "value" \in DOMAIN Some("Completed") THEN Some("Completed")["value"] ELSE None)]}
       /\ UnchangedFrame_200ffc0060c74508
       /\ model_step_count' = model_step_count + 1


work_execution_ObserveFailedFlow(arg_detail) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ObserveFlowFailed"
       /\ packet.payload.detail = arg_detail
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Running" \/ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined"
       /\ work_execution_phase' = "FailureEvidenceProjectionRequested"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = packet.payload.detail
       /\ work_execution_evidence_kind' = Some("Failed")
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowFailureEvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, kind |-> (IF "value" \in DOMAIN Some("Failed") THEN Some("Failed")["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveFailedFlow"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ObserveFailedFlow", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FailureEvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_failure_evidence_projection' = obligation_work_execution_work_execution_failure_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, kind |-> (IF "value" \in DOMAIN Some("Failed") THEN Some("Failed")["value"] ELSE None)]}
       /\ UnchangedFrame_3de6261a21bd344d
       /\ model_step_count' = model_step_count + 1


work_execution_ObserveCanceledFlow(arg_detail) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ObserveFlowCanceled"
       /\ packet.payload.detail = arg_detail
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Running" \/ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined"
       /\ work_execution_phase' = "CancellationEvidenceProjectionRequested"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = packet.payload.detail
       /\ work_execution_evidence_kind' = Some("Canceled")
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowCancellationEvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, kind |-> (IF "value" \in DOMAIN Some("Canceled") THEN Some("Canceled")["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCanceledFlow"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ObserveCanceledFlow", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "CancellationEvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_cancellation_evidence_projection' = obligation_work_execution_work_execution_cancellation_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, kind |-> (IF "value" \in DOMAIN Some("Canceled") THEN Some("Canceled")["value"] ELSE None)]}
       /\ UnchangedFrame_fc1db005e292de48
       /\ model_step_count' = model_step_count + 1


work_execution_ObserveLostRun(arg_detail) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ObserveRunLost"
       /\ packet.payload.detail = arg_detail
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Running"
       /\ work_execution_phase' = "FailureEvidenceProjectionRequested"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = Some(packet.payload.detail)
       /\ work_execution_evidence_kind' = Some("RunLost")
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowFailureEvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, kind |-> (IF "value" \in DOMAIN Some("RunLost") THEN Some("RunLost")["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveLostRun"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ObserveLostRun", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FailureEvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_failure_evidence_projection' = obligation_work_execution_work_execution_failure_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, kind |-> (IF "value" \in DOMAIN Some("RunLost") THEN Some("RunLost")["value"] ELSE None)]}
       /\ UnchangedFrame_3de6261a21bd344d
       /\ model_step_count' = model_step_count + 1


work_execution_ObserveLostCompletedRunBeforeEvidence(arg_detail) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ObserveRunLost"
       /\ packet.payload.detail = arg_detail
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "EvidenceProjectionRequested"
       /\ work_execution_phase' = "FailureEvidenceProjectionRequested"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = Some(packet.payload.detail)
       /\ work_execution_evidence_kind' = Some("RunLost")
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowFailureEvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, kind |-> (IF "value" \in DOMAIN Some("RunLost") THEN Some("RunLost")["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveLostCompletedRunBeforeEvidence"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ObserveLostCompletedRunBeforeEvidence", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FailureEvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_failure_evidence_projection' = obligation_work_execution_work_execution_failure_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, kind |-> (IF "value" \in DOMAIN Some("RunLost") THEN Some("RunLost")["value"] ELSE None)]}
       /\ UnchangedFrame_3de6261a21bd344d
       /\ model_step_count' = model_step_count + 1


work_execution_RecordUncertainLaunch(arg_detail) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "MarkLaunchUncertain"
       /\ packet.payload.detail = arg_detail
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchRequested"
       /\ work_execution_phase' = "LaunchUncertain"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = Some(packet.payload.detail)
       /\ work_execution_evidence_kind' = None
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchUncertain", payload |-> [binding_id |-> work_execution_binding_id, detail |-> (IF "value" \in DOMAIN Some(packet.payload.detail) THEN Some(packet.payload.detail)["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecordUncertainLaunch"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecordUncertainLaunch", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchUncertain"]}
       /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, detail |-> (IF "value" \in DOMAIN Some(packet.payload.detail) THEN Some(packet.payload.detail)["value"] ELSE None)]}
       /\ UnchangedFrame_b4a6d6a85f2cba01
       /\ model_step_count' = model_step_count + 1


work_execution_QuarantineLaunch(arg_detail) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "QuarantineLaunch"
       /\ packet.payload.detail = arg_detail
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain"
       /\ work_execution_phase' = "LaunchQuarantined"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = Some(packet.payload.detail)
       /\ work_execution_evidence_kind' = None
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowLaunchQuarantined", payload |-> [binding_id |-> work_execution_binding_id, detail |-> (IF "value" \in DOMAIN Some(packet.payload.detail) THEN Some(packet.payload.detail)["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "QuarantineLaunch"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "QuarantineLaunch", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchQuarantined"]}
       /\ obligation_work_execution_work_execution_quarantined_launch_resolution' = obligation_work_execution_work_execution_quarantined_launch_resolution \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, detail |-> (IF "value" \in DOMAIN Some(packet.payload.detail) THEN Some(packet.payload.detail)["value"] ELSE None)]}
       /\ UnchangedFrame_ef196b98e0ca2516
       /\ model_step_count' = model_step_count + 1


work_execution_FailLaunch(arg_detail) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ResolveLaunchFailed"
       /\ packet.payload.detail = arg_detail
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain"
       /\ work_execution_phase' = "LaunchFailureEvidenceProjectionRequested"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = Some(packet.payload.detail)
       /\ work_execution_evidence_kind' = Some("LaunchFailed")
       /\ UnchangedFrame_5dd7d648a23596a6
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "LaunchFailureEvidenceProjectionRequested", payload |-> [binding_id |-> work_execution_binding_id, detail |-> (IF "value" \in DOMAIN Some(packet.payload.detail) THEN Some(packet.payload.detail)["value"] ELSE None), kind |-> (IF "value" \in DOMAIN Some("LaunchFailed") THEN Some("LaunchFailed")["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "FailLaunch"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "FailLaunch", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchFailureEvidenceProjectionRequested"]}
       /\ obligation_work_execution_work_execution_launch_failure_evidence_projection' = obligation_work_execution_work_execution_launch_failure_evidence_projection \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id, detail |-> (IF "value" \in DOMAIN Some(packet.payload.detail) THEN Some(packet.payload.detail)["value"] ELSE None), kind |-> (IF "value" \in DOMAIN Some("LaunchFailed") THEN Some("LaunchFailed")["value"] ELSE None)]}
       /\ UnchangedFrame_bcb8917e76f5fc09
       /\ model_step_count' = model_step_count + 1


work_execution_CommitLaunchFailureEvidenceProjection ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ConfirmLaunchFailureEvidenceProjected"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchFailureEvidenceProjectionRequested"
       /\ work_execution_phase' = "LaunchFailed"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ UnchangedFrame_0a24b0a9815bff81
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "LaunchFailed", payload |-> [binding_id |-> work_execution_binding_id, detail |-> (IF "value" \in DOMAIN work_execution_last_failure_detail THEN work_execution_last_failure_detail["value"] ELSE None), run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "CommitLaunchFailureEvidenceProjection"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "CommitLaunchFailureEvidenceProjection", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchFailed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_CommitEvidenceProjection ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ConfirmEvidenceProjected"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "EvidenceProjectionRequested"
       /\ work_execution_phase' = "WorkClosureRequested"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ UnchangedFrame_0a24b0a9815bff81
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "WorkClosureRequested", payload |-> [binding_id |-> work_execution_binding_id], effect_id |-> (model_step_count + 1), source_transition |-> "CommitEvidenceProjection"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "CommitEvidenceProjection", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "WorkClosureRequested"]}
       /\ obligation_work_execution_work_execution_work_closure' = obligation_work_execution_work_execution_work_closure \cup {[effect_id |-> (model_step_count + 1), binding_id |-> work_execution_binding_id]}
       /\ UnchangedFrame_d063a6d561bc8142
       /\ model_step_count' = model_step_count + 1


work_execution_CommitFlowFailureEvidenceProjection ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ConfirmFlowFailureEvidenceProjected"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "FailureEvidenceProjectionRequested"
       /\ work_execution_phase' = "FlowFailed"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ UnchangedFrame_0a24b0a9815bff81
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowFailed", payload |-> [binding_id |-> work_execution_binding_id, detail |-> work_execution_last_failure_detail, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "CommitFlowFailureEvidenceProjection"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "CommitFlowFailureEvidenceProjection", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FlowFailed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_CommitFlowCancellationEvidenceProjection ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ConfirmFlowCancellationEvidenceProjected"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "CancellationEvidenceProjectionRequested"
       /\ work_execution_phase' = "FlowCanceled"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ UnchangedFrame_0a24b0a9815bff81
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "FlowCanceled", payload |-> [binding_id |-> work_execution_binding_id, detail |-> work_execution_last_failure_detail, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "CommitFlowCancellationEvidenceProjection"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "CommitFlowCancellationEvidenceProjection", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FlowCanceled"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_CommitWorkClosure ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ConfirmWorkClosed"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "WorkClosureRequested"
       /\ work_execution_phase' = "WorkClosed"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = None
       /\ UnchangedFrame_315673c263c47bfd
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "WorkClosed", payload |-> [binding_id |-> work_execution_binding_id], effect_id |-> (model_step_count + 1), source_transition |-> "CommitWorkClosure"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "CommitWorkClosure", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "WorkClosed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_RecordWorkClosureRefusal(arg_detail) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "RefuseWorkClosure"
       /\ packet.payload.detail = arg_detail
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "WorkClosureRequested"
       /\ work_execution_phase' = "EvidenceProjected"
       /\ work_execution_revision' = (work_execution_revision) + 1
       /\ work_execution_last_failure_detail' = Some(packet.payload.detail)
       /\ UnchangedFrame_315673c263c47bfd
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "EvidenceProjected", payload |-> [binding_id |-> work_execution_binding_id, run_id |-> work_execution_run_id], effect_id |-> (model_step_count + 1), source_transition |-> "RecordWorkClosureRefusal"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "RecordWorkClosureRefusal", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "EvidenceProjected"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityTerminalFlowFailed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "FlowFailed"
       /\ work_execution_phase' = "FlowFailed"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> TRUE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityTerminalFlowFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityTerminalFlowFailed", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FlowFailed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityTerminalFlowCanceled ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "FlowCanceled"
       /\ work_execution_phase' = "FlowCanceled"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> TRUE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityTerminalFlowCanceled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityTerminalFlowCanceled", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FlowCanceled"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityTerminalEvidenceProjected ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "EvidenceProjected"
       /\ work_execution_phase' = "EvidenceProjected"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> TRUE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityTerminalEvidenceProjected"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityTerminalEvidenceProjected", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "EvidenceProjected"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityTerminalWorkClosed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "WorkClosed"
       /\ work_execution_phase' = "WorkClosed"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> TRUE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityTerminalWorkClosed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityTerminalWorkClosed", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "WorkClosed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityTerminalLaunchFailed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchFailed"
       /\ work_execution_phase' = "LaunchFailed"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> TRUE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityTerminalLaunchFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityTerminalLaunchFailed", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchFailed"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveAbsent ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Absent"
       /\ work_execution_phase' = "Absent"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveAbsent"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveAbsent", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "Absent"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveLaunchRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchRequested"
       /\ work_execution_phase' = "LaunchRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveLaunchRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveLaunchRequested", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchRequested"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveLaunchUncertain ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchUncertain"
       /\ work_execution_phase' = "LaunchUncertain"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveLaunchUncertain"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveLaunchUncertain", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchUncertain"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveLaunchQuarantined ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchQuarantined"
       /\ work_execution_phase' = "LaunchQuarantined"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveLaunchQuarantined"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveLaunchQuarantined", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchQuarantined"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "Running"
       /\ work_execution_phase' = "Running"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveRunning", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "Running"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveEvidenceProjectionRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "EvidenceProjectionRequested"
       /\ work_execution_phase' = "EvidenceProjectionRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveEvidenceProjectionRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveEvidenceProjectionRequested", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "EvidenceProjectionRequested"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveFailureEvidenceProjectionRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "FailureEvidenceProjectionRequested"
       /\ work_execution_phase' = "FailureEvidenceProjectionRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveFailureEvidenceProjectionRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveFailureEvidenceProjectionRequested", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "FailureEvidenceProjectionRequested"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveCancellationEvidenceProjectionRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "CancellationEvidenceProjectionRequested"
       /\ work_execution_phase' = "CancellationEvidenceProjectionRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveCancellationEvidenceProjectionRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveCancellationEvidenceProjectionRequested", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "CancellationEvidenceProjectionRequested"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveLaunchFailureEvidenceProjectionRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "LaunchFailureEvidenceProjectionRequested"
       /\ work_execution_phase' = "LaunchFailureEvidenceProjectionRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveLaunchFailureEvidenceProjectionRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveLaunchFailureEvidenceProjectionRequested", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "LaunchFailureEvidenceProjectionRequested"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_ClassifyRetryEligibilityLiveWorkClosureRequested ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "work_execution"
       /\ packet.variant = "ClassifyRetryEligibility"
       /\ ~HigherPriorityReady("work_execution_authority")
       /\ work_execution_phase = "WorkClosureRequested"
       /\ work_execution_phase' = "WorkClosureRequested"
       /\ UnchangedFrame_d029cf0d3e83cf55
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "work_execution", variant |-> "RetryEligibilityClassified", payload |-> [eligible |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyRetryEligibilityLiveWorkClosureRequested"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "work_execution", transition |-> "ClassifyRetryEligibilityLiveWorkClosureRequested", actor |-> "work_execution_authority", step |-> (model_step_count + 1), from_phase |-> work_execution_phase, to_phase |-> "WorkClosureRequested"]}
       /\ UnchangedFrame_7ca0a3794d2bfdc6
       /\ model_step_count' = model_step_count + 1


work_execution_identified_after_bind == (IF (work_execution_phase = "Absent") THEN TRUE ELSE ((work_execution_binding_id # "") /\ (work_execution_run_id # "")))
work_execution_evidence_projection_is_typed == (IF ((work_execution_phase # "EvidenceProjectionRequested") /\ (work_execution_phase # "FailureEvidenceProjectionRequested") /\ (work_execution_phase # "CancellationEvidenceProjectionRequested") /\ (work_execution_phase # "LaunchFailureEvidenceProjectionRequested")) THEN TRUE ELSE (work_execution_evidence_kind # None))

EntryPacketAdmissible_work_execution(packet) ==
    \/ /\ (packet.variant = "Bind") /\ (work_execution_phase = "Absent") /\ (((packet.payload.binding_id # "") /\ (packet.payload.run_id # "")))
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "LaunchRequested")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "LaunchUncertain")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "LaunchQuarantined")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "Running")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "EvidenceProjectionRequested")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "WorkClosureRequested")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "FailureEvidenceProjectionRequested")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "CancellationEvidenceProjectionRequested")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "LaunchFailureEvidenceProjectionRequested")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "FlowFailed")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "FlowCanceled")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "EvidenceProjected")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "WorkClosed")
    \/ /\ (packet.variant = "Recover") /\ (work_execution_phase = "LaunchFailed")
    \/ /\ (packet.variant = "ConfirmFlowStarted") /\ (work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined")
    \/ /\ (packet.variant = "ObserveFlowRunning") /\ (work_execution_phase = "Running" \/ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined")
    \/ /\ (packet.variant = "ObserveFlowCompleted") /\ (work_execution_phase = "Running" \/ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined")
    \/ /\ (packet.variant = "ObserveFlowFailed") /\ (work_execution_phase = "Running" \/ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined")
    \/ /\ (packet.variant = "ObserveFlowCanceled") /\ (work_execution_phase = "Running" \/ work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain" \/ work_execution_phase = "LaunchQuarantined")
    \/ /\ (packet.variant = "ObserveRunLost") /\ (work_execution_phase = "Running")
    \/ /\ (packet.variant = "ObserveRunLost") /\ (work_execution_phase = "EvidenceProjectionRequested")
    \/ /\ (packet.variant = "MarkLaunchUncertain") /\ (work_execution_phase = "LaunchRequested")
    \/ /\ (packet.variant = "QuarantineLaunch") /\ (work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain")
    \/ /\ (packet.variant = "ResolveLaunchFailed") /\ (work_execution_phase = "LaunchRequested" \/ work_execution_phase = "LaunchUncertain")
    \/ /\ (packet.variant = "ConfirmLaunchFailureEvidenceProjected") /\ (work_execution_phase = "LaunchFailureEvidenceProjectionRequested")
    \/ /\ (packet.variant = "ConfirmEvidenceProjected") /\ (work_execution_phase = "EvidenceProjectionRequested")
    \/ /\ (packet.variant = "ConfirmFlowFailureEvidenceProjected") /\ (work_execution_phase = "FailureEvidenceProjectionRequested")
    \/ /\ (packet.variant = "ConfirmFlowCancellationEvidenceProjected") /\ (work_execution_phase = "CancellationEvidenceProjectionRequested")
    \/ /\ (packet.variant = "ConfirmWorkClosed") /\ (work_execution_phase = "WorkClosureRequested")
    \/ /\ (packet.variant = "RefuseWorkClosure") /\ (work_execution_phase = "WorkClosureRequested")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "FlowFailed")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "FlowCanceled")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "EvidenceProjected")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "WorkClosed")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "LaunchFailed")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "Absent")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "LaunchRequested")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "LaunchUncertain")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "LaunchQuarantined")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "Running")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "EvidenceProjectionRequested")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "FailureEvidenceProjectionRequested")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "CancellationEvidenceProjectionRequested")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "LaunchFailureEvidenceProjectionRequested")
    \/ /\ (packet.variant = "ClassifyRetryEligibility") /\ (work_execution_phase = "WorkClosureRequested")

EntryPacketAdmissible(packet) ==
    CASE
      packet.machine = "work_execution" -> EntryPacketAdmissible_work_execution(packet)
      [] OTHER -> FALSE

Inject_bind_execution(arg_binding_id, arg_run_id) ==
    /\ ~([machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> arg_binding_id, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "bind_execution", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ EntryPacketAdmissible([machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> arg_binding_id, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "bind_execution", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0])
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> arg_binding_id, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "bind_execution", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "Bind", payload |-> [binding_id |-> arg_binding_id, run_id |-> arg_run_id], source_kind |-> "entry", source_route |-> "bind_execution", source_machine |-> "external_entry", source_effect |-> "Bind", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d1d90666ffe4859e

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UnchangedFrame_79f3e86b7290e9f5

RejectPendingEntryInput ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.source_kind = "entry"
       /\ ~EntryPacketAdmissible(packet)
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions
       /\ model_step_count' = model_step_count + 1
       /\ UnchangedFrame_a2a13c8cc26dadb5

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_workgraph_flow_success_closure ==
    FALSE

WitnessInjectNext_workgraph_flow_failure_evidence ==
    FALSE

WitnessInjectNext_workgraph_flow_cancellation_evidence ==
    FALSE

WitnessInjectNext_workgraph_flow_uncertain_abandonment ==
    FALSE

WitnessInjectNext_workgraph_flow_launch_quarantine ==
    FALSE

WitnessScriptComplete_workgraph_flow_success_closure ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCompletedFlow")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitEvidenceProjection")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitWorkClosure")

WitnessScriptComplete_workgraph_flow_failure_evidence ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveFailedFlow")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitFlowFailureEvidenceProjection")

WitnessScriptComplete_workgraph_flow_cancellation_evidence ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCanceledFlow")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitFlowCancellationEvidenceProjection")

WitnessScriptComplete_workgraph_flow_uncertain_abandonment ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "RecordUncertainLaunch")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "FailLaunch")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitLaunchFailureEvidenceProjection")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "BindExecution" /\ later.machine = "work_execution" /\ later.transition = "RecordUncertainLaunch" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "RecordUncertainLaunch" /\ later.machine = "work_execution" /\ later.transition = "FailLaunch" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "FailLaunch" /\ later.machine = "work_execution" /\ later.transition = "CommitLaunchFailureEvidenceProjection" /\ earlier.step < later.step)

WitnessScriptComplete_workgraph_flow_launch_quarantine ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "QuarantineLaunch")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCompletedFlow")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitEvidenceProjection")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitWorkClosure")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "BindExecution" /\ later.machine = "work_execution" /\ later.transition = "QuarantineLaunch" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "QuarantineLaunch" /\ later.machine = "work_execution" /\ later.transition = "ObserveCompletedFlow" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "ObserveCompletedFlow" /\ later.machine = "work_execution" /\ later.transition = "CommitEvidenceProjection" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "CommitEvidenceProjection" /\ later.machine = "work_execution" /\ later.transition = "CommitWorkClosure" /\ earlier.step < later.step)

WitnessNoPrematureStutter_workgraph_flow_success_closure ==
    \/ WitnessScriptComplete_workgraph_flow_success_closure
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_workgraph_flow_failure_evidence ==
    \/ WitnessScriptComplete_workgraph_flow_failure_evidence
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_workgraph_flow_cancellation_evidence ==
    \/ WitnessScriptComplete_workgraph_flow_cancellation_evidence
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_workgraph_flow_uncertain_abandonment ==
    \/ WitnessScriptComplete_workgraph_flow_uncertain_abandonment
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_workgraph_flow_launch_quarantine ==
    \/ WitnessScriptComplete_workgraph_flow_launch_quarantine
    \/ model_step_count' # model_step_count

WitnessSatisfiedStutter_workgraph_flow_success_closure ==
    /\ WitnessScriptComplete_workgraph_flow_success_closure
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCompletedFlow")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitEvidenceProjection")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitWorkClosure")
    /\ UNCHANGED vars

WitnessSatisfiedStutter_workgraph_flow_failure_evidence ==
    /\ WitnessScriptComplete_workgraph_flow_failure_evidence
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveFailedFlow")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitFlowFailureEvidenceProjection")
    /\ UNCHANGED vars

WitnessSatisfiedStutter_workgraph_flow_cancellation_evidence ==
    /\ WitnessScriptComplete_workgraph_flow_cancellation_evidence
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCanceledFlow")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitFlowCancellationEvidenceProjection")
    /\ UNCHANGED vars

WitnessSatisfiedStutter_workgraph_flow_uncertain_abandonment ==
    /\ WitnessScriptComplete_workgraph_flow_uncertain_abandonment
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "RecordUncertainLaunch")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "FailLaunch")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitLaunchFailureEvidenceProjection")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "BindExecution" /\ later.machine = "work_execution" /\ later.transition = "RecordUncertainLaunch" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "RecordUncertainLaunch" /\ later.machine = "work_execution" /\ later.transition = "FailLaunch" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "FailLaunch" /\ later.machine = "work_execution" /\ later.transition = "CommitLaunchFailureEvidenceProjection" /\ earlier.step < later.step)
    /\ UNCHANGED vars

WitnessSatisfiedStutter_workgraph_flow_launch_quarantine ==
    /\ WitnessScriptComplete_workgraph_flow_launch_quarantine
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "QuarantineLaunch")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCompletedFlow")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitEvidenceProjection")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitWorkClosure")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "BindExecution" /\ later.machine = "work_execution" /\ later.transition = "QuarantineLaunch" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "QuarantineLaunch" /\ later.machine = "work_execution" /\ later.transition = "ObserveCompletedFlow" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "ObserveCompletedFlow" /\ later.machine = "work_execution" /\ later.transition = "CommitEvidenceProjection" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "CommitEvidenceProjection" /\ later.machine = "work_execution" /\ later.transition = "CommitWorkClosure" /\ earlier.step < later.step)
    /\ UNCHANGED vars

OwnerFeedback_work_execution_work_execution_flow_launch_ConfirmFlowStarted ==
    /\ obligation_work_execution_work_execution_flow_launch /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_launch :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ConfirmFlowStarted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ConfirmFlowStarted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_01c5c5c26fe104c3

OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowRunning ==
    /\ obligation_work_execution_work_execution_flow_launch /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_launch :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowRunning", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowRunning", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_01c5c5c26fe104c3

OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowCompleted ==
    /\ obligation_work_execution_work_execution_flow_launch /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_launch :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowCompleted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowCompleted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_01c5c5c26fe104c3

OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowFailed ==
    /\ obligation_work_execution_work_execution_flow_launch /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_launch :
        /\ \E owner_ctx_observed_failure_detail \in OptionStringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_failure_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_failure_detail]]} /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_01c5c5c26fe104c3

OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowCanceled ==
    /\ obligation_work_execution_work_execution_flow_launch /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_launch :
        /\ \E owner_ctx_observed_cancellation_detail \in OptionStringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowCanceled", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_cancellation_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowCanceled", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_cancellation_detail]]} /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_01c5c5c26fe104c3

OwnerFeedback_work_execution_work_execution_flow_launch_MarkLaunchUncertain ==
    /\ obligation_work_execution_work_execution_flow_launch /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_launch :
        /\ \E owner_ctx_launch_uncertainty_detail \in StringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "MarkLaunchUncertain", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_uncertainty_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "MarkLaunchUncertain", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_uncertainty_detail]]} /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_01c5c5c26fe104c3

OwnerFeedback_work_execution_work_execution_flow_launch_QuarantineLaunch ==
    /\ obligation_work_execution_work_execution_flow_launch /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_launch :
        /\ \E owner_ctx_launch_quarantine_detail \in StringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "QuarantineLaunch", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_quarantine_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "QuarantineLaunch", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_quarantine_detail]]} /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_01c5c5c26fe104c3

OwnerFeedback_work_execution_work_execution_flow_launch_ResolveLaunchFailed ==
    /\ obligation_work_execution_work_execution_flow_launch /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_launch :
        /\ \E owner_ctx_launch_failure_detail \in StringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ResolveLaunchFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_failure_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ResolveLaunchFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_failure_detail]]} /\ obligation_work_execution_work_execution_flow_launch' = obligation_work_execution_work_execution_flow_launch \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_01c5c5c26fe104c3

OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowRunning ==
    /\ obligation_work_execution_work_execution_flow_observation /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_observation :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowRunning", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowRunning", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_flow_observation' = obligation_work_execution_work_execution_flow_observation \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_1eb4735f61fd60ac

OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowCompleted ==
    /\ obligation_work_execution_work_execution_flow_observation /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_observation :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowCompleted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowCompleted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_flow_observation' = obligation_work_execution_work_execution_flow_observation \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_1eb4735f61fd60ac

OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowFailed ==
    /\ obligation_work_execution_work_execution_flow_observation /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_observation :
        /\ \E owner_ctx_observed_failure_detail \in OptionStringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_failure_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_failure_detail]]} /\ obligation_work_execution_work_execution_flow_observation' = obligation_work_execution_work_execution_flow_observation \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_1eb4735f61fd60ac

OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowCanceled ==
    /\ obligation_work_execution_work_execution_flow_observation /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_observation :
        /\ \E owner_ctx_observed_cancellation_detail \in OptionStringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowCanceled", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_cancellation_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowCanceled", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_cancellation_detail]]} /\ obligation_work_execution_work_execution_flow_observation' = obligation_work_execution_work_execution_flow_observation \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_1eb4735f61fd60ac

OwnerFeedback_work_execution_work_execution_flow_observation_ObserveRunLost ==
    /\ obligation_work_execution_work_execution_flow_observation /= {}
    /\ \E token \in obligation_work_execution_work_execution_flow_observation :
        /\ \E owner_ctx_lost_run_detail \in StringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveRunLost", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_lost_run_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveRunLost", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchAccepted", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_lost_run_detail]]} /\ obligation_work_execution_work_execution_flow_observation' = obligation_work_execution_work_execution_flow_observation \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_1eb4735f61fd60ac

OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ConfirmFlowStarted ==
    /\ obligation_work_execution_work_execution_uncertain_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_uncertain_launch_resolution :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ConfirmFlowStarted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ConfirmFlowStarted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_eb1b34b9f4544149

OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowRunning ==
    /\ obligation_work_execution_work_execution_uncertain_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_uncertain_launch_resolution :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowRunning", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowRunning", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_eb1b34b9f4544149

OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowCompleted ==
    /\ obligation_work_execution_work_execution_uncertain_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_uncertain_launch_resolution :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowCompleted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowCompleted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_eb1b34b9f4544149

OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowFailed ==
    /\ obligation_work_execution_work_execution_uncertain_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_uncertain_launch_resolution :
        /\ \E owner_ctx_observed_failure_detail \in OptionStringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_failure_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_failure_detail]]} /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_eb1b34b9f4544149

OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowCanceled ==
    /\ obligation_work_execution_work_execution_uncertain_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_uncertain_launch_resolution :
        /\ \E owner_ctx_observed_cancellation_detail \in OptionStringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowCanceled", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_cancellation_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowCanceled", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_cancellation_detail]]} /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_eb1b34b9f4544149

OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ResolveLaunchFailed ==
    /\ obligation_work_execution_work_execution_uncertain_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_uncertain_launch_resolution :
        /\ \E owner_ctx_launch_failure_detail \in StringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ResolveLaunchFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_failure_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ResolveLaunchFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_failure_detail]]} /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_eb1b34b9f4544149

OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_QuarantineLaunch ==
    /\ obligation_work_execution_work_execution_uncertain_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_uncertain_launch_resolution :
        /\ \E owner_ctx_launch_quarantine_detail \in StringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "QuarantineLaunch", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_quarantine_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "QuarantineLaunch", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchUncertain", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_launch_quarantine_detail]]} /\ obligation_work_execution_work_execution_uncertain_launch_resolution' = obligation_work_execution_work_execution_uncertain_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_eb1b34b9f4544149

OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ConfirmFlowStarted ==
    /\ obligation_work_execution_work_execution_quarantined_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_quarantined_launch_resolution :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ConfirmFlowStarted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ConfirmFlowStarted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_quarantined_launch_resolution' = obligation_work_execution_work_execution_quarantined_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_71eb061ce3c69fe6

OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowRunning ==
    /\ obligation_work_execution_work_execution_quarantined_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_quarantined_launch_resolution :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowRunning", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowRunning", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_quarantined_launch_resolution' = obligation_work_execution_work_execution_quarantined_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_71eb061ce3c69fe6

OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowCompleted ==
    /\ obligation_work_execution_work_execution_quarantined_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_quarantined_launch_resolution :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowCompleted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowCompleted", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_quarantined_launch_resolution' = obligation_work_execution_work_execution_quarantined_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_71eb061ce3c69fe6

OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowFailed ==
    /\ obligation_work_execution_work_execution_quarantined_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_quarantined_launch_resolution :
        /\ \E owner_ctx_observed_failure_detail \in OptionStringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_failure_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowFailed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_failure_detail]]} /\ obligation_work_execution_work_execution_quarantined_launch_resolution' = obligation_work_execution_work_execution_quarantined_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_71eb061ce3c69fe6

OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowCanceled ==
    /\ obligation_work_execution_work_execution_quarantined_launch_resolution /= {}
    /\ \E token \in obligation_work_execution_work_execution_quarantined_launch_resolution :
        /\ \E owner_ctx_observed_cancellation_detail \in OptionStringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveFlowCanceled", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_cancellation_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveFlowCanceled", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowLaunchQuarantined", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_observed_cancellation_detail]]} /\ obligation_work_execution_work_execution_quarantined_launch_resolution' = obligation_work_execution_work_execution_quarantined_launch_resolution \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_71eb061ce3c69fe6

OwnerFeedback_work_execution_work_execution_success_evidence_projection_ConfirmEvidenceProjected ==
    /\ obligation_work_execution_work_execution_success_evidence_projection /= {}
    /\ \E token \in obligation_work_execution_work_execution_success_evidence_projection :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ConfirmEvidenceProjected", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "EvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ConfirmEvidenceProjected", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "EvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_success_evidence_projection' = obligation_work_execution_work_execution_success_evidence_projection \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_d2e8cf6c034eddd0

OwnerFeedback_work_execution_work_execution_success_evidence_projection_ObserveRunLost ==
    /\ obligation_work_execution_work_execution_success_evidence_projection /= {}
    /\ \E token \in obligation_work_execution_work_execution_success_evidence_projection :
        /\ \E owner_ctx_lost_run_detail \in StringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ObserveRunLost", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "EvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_lost_run_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ObserveRunLost", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "EvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_lost_run_detail]]} /\ obligation_work_execution_work_execution_success_evidence_projection' = obligation_work_execution_work_execution_success_evidence_projection \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_d2e8cf6c034eddd0

OwnerFeedback_work_execution_work_execution_failure_evidence_projection_ConfirmFlowFailureEvidenceProjected ==
    /\ obligation_work_execution_work_execution_failure_evidence_projection /= {}
    /\ \E token \in obligation_work_execution_work_execution_failure_evidence_projection :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ConfirmFlowFailureEvidenceProjected", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowFailureEvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ConfirmFlowFailureEvidenceProjected", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowFailureEvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_failure_evidence_projection' = obligation_work_execution_work_execution_failure_evidence_projection \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_932d08003e4e619d

OwnerFeedback_work_execution_work_execution_cancellation_evidence_projection_ConfirmFlowCancellationEvidenceProjected ==
    /\ obligation_work_execution_work_execution_cancellation_evidence_projection /= {}
    /\ \E token \in obligation_work_execution_work_execution_cancellation_evidence_projection :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ConfirmFlowCancellationEvidenceProjected", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowCancellationEvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ConfirmFlowCancellationEvidenceProjected", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "FlowCancellationEvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_cancellation_evidence_projection' = obligation_work_execution_work_execution_cancellation_evidence_projection \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_b20dd0068997aa3e

OwnerFeedback_work_execution_work_execution_launch_failure_evidence_projection_ConfirmLaunchFailureEvidenceProjected ==
    /\ obligation_work_execution_work_execution_launch_failure_evidence_projection /= {}
    /\ \E token \in obligation_work_execution_work_execution_launch_failure_evidence_projection :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ConfirmLaunchFailureEvidenceProjected", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "LaunchFailureEvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ConfirmLaunchFailureEvidenceProjected", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "LaunchFailureEvidenceProjectionRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_launch_failure_evidence_projection' = obligation_work_execution_work_execution_launch_failure_evidence_projection \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_6da688a13f8a6f43

OwnerFeedback_work_execution_work_execution_work_closure_ConfirmWorkClosed ==
    /\ obligation_work_execution_work_execution_work_closure /= {}
    /\ \E token \in obligation_work_execution_work_execution_work_closure :
        /\ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "ConfirmWorkClosed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "WorkClosureRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "ConfirmWorkClosed", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "WorkClosureRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [tag |-> "unit"]]} /\ obligation_work_execution_work_execution_work_closure' = obligation_work_execution_work_execution_work_closure \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_aadcaeaaa5488654

OwnerFeedback_work_execution_work_execution_work_closure_RefuseWorkClosure ==
    /\ obligation_work_execution_work_execution_work_closure /= {}
    /\ \E token \in obligation_work_execution_work_execution_work_closure :
        /\ \E owner_ctx_work_closure_refusal_detail \in StringValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "work_execution", variant |-> "RefuseWorkClosure", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "WorkClosureRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_work_closure_refusal_detail]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "work_execution", variant |-> "RefuseWorkClosure", source_kind |-> "owner", source_machine |-> "work_execution", source_effect |-> "WorkClosureRequested", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [detail |-> owner_ctx_work_closure_refusal_detail]]} /\ obligation_work_execution_work_execution_work_closure' = obligation_work_execution_work_execution_work_closure \ {token} /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_aadcaeaaa5488654

CoreNext ==
    \/ RejectPendingEntryInput
    \/ \E arg_binding_id \in StringValues : \E arg_run_id \in StringValues : work_execution_BindExecution(arg_binding_id, arg_run_id)
    \/ work_execution_RecoverLaunchRequest
    \/ work_execution_RecoverUncertainLaunch
    \/ work_execution_RecoverQuarantinedLaunch
    \/ work_execution_RecoverRunning
    \/ work_execution_RecoverEvidenceProjection
    \/ work_execution_RecoverWorkClosure
    \/ work_execution_RecoverFlowFailureEvidenceProjection
    \/ work_execution_RecoverFlowCancellationEvidenceProjection
    \/ work_execution_RecoverLaunchFailureEvidenceProjection
    \/ work_execution_RecoverFlowFailure
    \/ work_execution_RecoverFlowCancellation
    \/ work_execution_RecoverEvidenceProjected
    \/ work_execution_RecoverClosedWork
    \/ work_execution_RecoverLaunchFailure
    \/ work_execution_AcceptFlowLaunch
    \/ work_execution_ObserveRunningFlow
    \/ work_execution_ObserveCompletedFlow
    \/ \E arg_detail \in OptionStringValues : work_execution_ObserveFailedFlow(arg_detail)
    \/ \E arg_detail \in OptionStringValues : work_execution_ObserveCanceledFlow(arg_detail)
    \/ \E arg_detail \in StringValues : work_execution_ObserveLostRun(arg_detail)
    \/ \E arg_detail \in StringValues : work_execution_ObserveLostCompletedRunBeforeEvidence(arg_detail)
    \/ \E arg_detail \in StringValues : work_execution_RecordUncertainLaunch(arg_detail)
    \/ \E arg_detail \in StringValues : work_execution_QuarantineLaunch(arg_detail)
    \/ \E arg_detail \in StringValues : work_execution_FailLaunch(arg_detail)
    \/ work_execution_CommitLaunchFailureEvidenceProjection
    \/ work_execution_CommitEvidenceProjection
    \/ work_execution_CommitFlowFailureEvidenceProjection
    \/ work_execution_CommitFlowCancellationEvidenceProjection
    \/ work_execution_CommitWorkClosure
    \/ \E arg_detail \in StringValues : work_execution_RecordWorkClosureRefusal(arg_detail)
    \/ work_execution_ClassifyRetryEligibilityTerminalFlowFailed
    \/ work_execution_ClassifyRetryEligibilityTerminalFlowCanceled
    \/ work_execution_ClassifyRetryEligibilityTerminalEvidenceProjected
    \/ work_execution_ClassifyRetryEligibilityTerminalWorkClosed
    \/ work_execution_ClassifyRetryEligibilityTerminalLaunchFailed
    \/ work_execution_ClassifyRetryEligibilityLiveAbsent
    \/ work_execution_ClassifyRetryEligibilityLiveLaunchRequested
    \/ work_execution_ClassifyRetryEligibilityLiveLaunchUncertain
    \/ work_execution_ClassifyRetryEligibilityLiveLaunchQuarantined
    \/ work_execution_ClassifyRetryEligibilityLiveRunning
    \/ work_execution_ClassifyRetryEligibilityLiveEvidenceProjectionRequested
    \/ work_execution_ClassifyRetryEligibilityLiveFailureEvidenceProjectionRequested
    \/ work_execution_ClassifyRetryEligibilityLiveCancellationEvidenceProjectionRequested
    \/ work_execution_ClassifyRetryEligibilityLiveLaunchFailureEvidenceProjectionRequested
    \/ work_execution_ClassifyRetryEligibilityLiveWorkClosureRequested
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ConfirmFlowStarted
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowRunning
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowFailed
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowCanceled
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_MarkLaunchUncertain
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_QuarantineLaunch
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ResolveLaunchFailed
    \/ OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowRunning
    \/ OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowFailed
    \/ OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowCanceled
    \/ OwnerFeedback_work_execution_work_execution_flow_observation_ObserveRunLost
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ConfirmFlowStarted
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowRunning
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowFailed
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowCanceled
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ResolveLaunchFailed
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_QuarantineLaunch
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ConfirmFlowStarted
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowRunning
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowFailed
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowCanceled
    \/ OwnerFeedback_work_execution_work_execution_success_evidence_projection_ConfirmEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_success_evidence_projection_ObserveRunLost
    \/ OwnerFeedback_work_execution_work_execution_failure_evidence_projection_ConfirmFlowFailureEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_cancellation_evidence_projection_ConfirmFlowCancellationEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_launch_failure_evidence_projection_ConfirmLaunchFailureEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_work_closure_ConfirmWorkClosed
    \/ OwnerFeedback_work_execution_work_execution_work_closure_RefuseWorkClosure
    \/ QuiescentStutter

InjectNext ==
    \/ \E arg_binding_id \in StringValues : \E arg_run_id \in StringValues : Inject_bind_execution(arg_binding_id, arg_run_id)

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_workgraph_flow_success_closure ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "Bind" /\ work_execution_BindExecution(witness_packet.payload.binding_id, witness_packet.payload.run_id)
    \/ work_execution_ObserveCompletedFlow
    \/ work_execution_CommitEvidenceProjection
    \/ work_execution_CommitWorkClosure
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_success_evidence_projection_ConfirmEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_work_closure_ConfirmWorkClosed
    \/ WitnessSatisfiedStutter_workgraph_flow_success_closure
    \/ WitnessInjectNext_workgraph_flow_success_closure

WitnessNext_workgraph_flow_failure_evidence ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "Bind" /\ work_execution_BindExecution(witness_packet.payload.binding_id, witness_packet.payload.run_id)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "ObserveFlowFailed" /\ work_execution_ObserveFailedFlow(witness_packet.payload.detail)
    \/ work_execution_CommitFlowFailureEvidenceProjection
    \/ OwnerFeedback_work_execution_work_execution_failure_evidence_projection_ConfirmFlowFailureEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowFailed
    \/ OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowFailed
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowFailed
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowFailed
    \/ WitnessSatisfiedStutter_workgraph_flow_failure_evidence
    \/ WitnessInjectNext_workgraph_flow_failure_evidence

WitnessNext_workgraph_flow_cancellation_evidence ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "Bind" /\ work_execution_BindExecution(witness_packet.payload.binding_id, witness_packet.payload.run_id)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "ObserveFlowCanceled" /\ work_execution_ObserveCanceledFlow(witness_packet.payload.detail)
    \/ work_execution_CommitFlowCancellationEvidenceProjection
    \/ OwnerFeedback_work_execution_work_execution_cancellation_evidence_projection_ConfirmFlowCancellationEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_ObserveFlowCanceled
    \/ OwnerFeedback_work_execution_work_execution_flow_observation_ObserveFlowCanceled
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowCanceled
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ObserveFlowCanceled
    \/ WitnessSatisfiedStutter_workgraph_flow_cancellation_evidence
    \/ WitnessInjectNext_workgraph_flow_cancellation_evidence

WitnessNext_workgraph_flow_uncertain_abandonment ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "Bind" /\ work_execution_BindExecution(witness_packet.payload.binding_id, witness_packet.payload.run_id)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "MarkLaunchUncertain" /\ work_execution_RecordUncertainLaunch(witness_packet.payload.detail)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "ResolveLaunchFailed" /\ work_execution_FailLaunch(witness_packet.payload.detail)
    \/ work_execution_CommitLaunchFailureEvidenceProjection
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_MarkLaunchUncertain
    \/ OwnerFeedback_work_execution_work_execution_launch_failure_evidence_projection_ConfirmLaunchFailureEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_uncertain_launch_resolution_ResolveLaunchFailed
    \/ WitnessSatisfiedStutter_workgraph_flow_uncertain_abandonment
    \/ WitnessInjectNext_workgraph_flow_uncertain_abandonment

WitnessNext_workgraph_flow_launch_quarantine ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "Bind" /\ work_execution_BindExecution(witness_packet.payload.binding_id, witness_packet.payload.run_id)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "work_execution" /\ witness_packet.variant = "QuarantineLaunch" /\ work_execution_QuarantineLaunch(witness_packet.payload.detail)
    \/ work_execution_ObserveCompletedFlow
    \/ work_execution_CommitEvidenceProjection
    \/ work_execution_CommitWorkClosure
    \/ OwnerFeedback_work_execution_work_execution_flow_launch_QuarantineLaunch
    \/ OwnerFeedback_work_execution_work_execution_quarantined_launch_resolution_ObserveFlowCompleted
    \/ OwnerFeedback_work_execution_work_execution_success_evidence_projection_ConfirmEvidenceProjected
    \/ OwnerFeedback_work_execution_work_execution_work_closure_ConfirmWorkClosed
    \/ WitnessSatisfiedStutter_workgraph_flow_launch_quarantine
    \/ WitnessInjectNext_workgraph_flow_launch_quarantine


flow_launch_handoff == TRUE
flow_observation_handoff == TRUE
uncertain_launch_handoff == TRUE
quarantined_launch_handoff == TRUE
success_evidence_handoff == TRUE
failure_evidence_handoff == TRUE
cancellation_evidence_handoff == TRUE
launch_failure_evidence_handoff == TRUE
work_closure_handoff == TRUE

NoOpenObligationsOnTerminal_work_execution_work_execution_flow_launch == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_flow_launch = {}
NoOpenObligationsOnTerminal_work_execution_work_execution_flow_observation == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_flow_observation = {}
NoOpenObligationsOnTerminal_work_execution_work_execution_uncertain_launch_resolution == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_uncertain_launch_resolution = {}
NoOpenObligationsOnTerminal_work_execution_work_execution_quarantined_launch_resolution == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_quarantined_launch_resolution = {}
NoOpenObligationsOnTerminal_work_execution_work_execution_success_evidence_projection == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_success_evidence_projection = {}
NoOpenObligationsOnTerminal_work_execution_work_execution_failure_evidence_projection == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_failure_evidence_projection = {}
NoOpenObligationsOnTerminal_work_execution_work_execution_cancellation_evidence_projection == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_cancellation_evidence_projection = {}
NoOpenObligationsOnTerminal_work_execution_work_execution_launch_failure_evidence_projection == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_launch_failure_evidence_projection = {}
NoOpenObligationsOnTerminal_work_execution_work_execution_work_closure == (work_execution_phase = "FlowFailed" \/ work_execution_phase = "FlowCanceled" \/ work_execution_phase = "EvidenceProjected" \/ work_execution_phase = "WorkClosed" \/ work_execution_phase = "LaunchFailed") => obligation_work_execution_work_execution_work_closure = {}
OwnerFeedbackHasProtocolProvenance ==
    \A input_packet \in observed_inputs :
        input_packet.source_kind /= "owner"
        \/ ((/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ConfirmFlowStarted" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowRunning" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowCompleted" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowFailed" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowCanceled" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "MarkLaunchUncertain" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "QuarantineLaunch" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ResolveLaunchFailed" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowRunning" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchAccepted" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchAccepted" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowCompleted" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchAccepted" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchAccepted" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowFailed" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchAccepted" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchAccepted" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowCanceled" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchAccepted" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchAccepted" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveRunLost" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchAccepted" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchAccepted" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ConfirmFlowStarted" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchUncertain" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchUncertain" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowRunning" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchUncertain" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchUncertain" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowCompleted" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchUncertain" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchUncertain" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowFailed" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchUncertain" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchUncertain" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowCanceled" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchUncertain" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchUncertain" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ResolveLaunchFailed" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchUncertain" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchUncertain" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "QuarantineLaunch" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchUncertain" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchUncertain" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ConfirmFlowStarted" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchQuarantined" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchQuarantined" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowRunning" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchQuarantined" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchQuarantined" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowCompleted" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchQuarantined" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchQuarantined" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowFailed" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchQuarantined" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchQuarantined" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveFlowCanceled" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowLaunchQuarantined" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowLaunchQuarantined" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ConfirmEvidenceProjected" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "EvidenceProjectionRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "EvidenceProjectionRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ObserveRunLost" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "EvidenceProjectionRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "EvidenceProjectionRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ConfirmFlowFailureEvidenceProjected" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowFailureEvidenceProjectionRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowFailureEvidenceProjectionRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ConfirmFlowCancellationEvidenceProjected" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "FlowCancellationEvidenceProjectionRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "FlowCancellationEvidenceProjectionRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ConfirmLaunchFailureEvidenceProjected" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "LaunchFailureEvidenceProjectionRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "LaunchFailureEvidenceProjectionRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "ConfirmWorkClosed" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "WorkClosureRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "WorkClosureRequested" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "work_execution" /\ input_packet.variant = "RefuseWorkClosure" /\ input_packet.source_machine = "work_execution" /\ input_packet.source_effect = "WorkClosureRequested" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "work_execution" /\ effect_packet.variant = "WorkClosureRequested" /\ effect_packet.effect_id = input_packet.effect_id))

CoverageInstrumentation == TRUE

CiStateConstraint == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 12
DeepStateConstraint == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 12
WitnessStateConstraint_workgraph_flow_success_closure == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 12
WitnessStateConstraint_workgraph_flow_failure_evidence == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 12
WitnessStateConstraint_workgraph_flow_cancellation_evidence == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 12
WitnessStateConstraint_workgraph_flow_uncertain_abandonment == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 12
WitnessStateConstraint_workgraph_flow_launch_quarantine == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 8 /\ Cardinality(observed_transitions) <= 12

Spec ==
    /\ Init
    /\ [][Next]_vars

WitnessSpec_workgraph_flow_success_closure ==
    /\ WitnessInit_workgraph_flow_success_closure
    /\ [] [WitnessNext_workgraph_flow_success_closure]_vars

WitnessSpec_workgraph_flow_failure_evidence ==
    /\ WitnessInit_workgraph_flow_failure_evidence
    /\ [] [WitnessNext_workgraph_flow_failure_evidence]_vars

WitnessSpec_workgraph_flow_cancellation_evidence ==
    /\ WitnessInit_workgraph_flow_cancellation_evidence
    /\ [] [WitnessNext_workgraph_flow_cancellation_evidence]_vars

WitnessSpec_workgraph_flow_uncertain_abandonment ==
    /\ WitnessInit_workgraph_flow_uncertain_abandonment
    /\ [] [WitnessNext_workgraph_flow_uncertain_abandonment]_vars

WitnessSpec_workgraph_flow_launch_quarantine ==
    /\ WitnessInit_workgraph_flow_launch_quarantine
    /\ [] [WitnessNext_workgraph_flow_launch_quarantine]_vars

WitnessTransitionObserved_workgraph_flow_success_closure_work_execution_BindExecution == WitnessScriptComplete_workgraph_flow_success_closure => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
WitnessTransitionObserved_workgraph_flow_success_closure_work_execution_ObserveCompletedFlow == WitnessScriptComplete_workgraph_flow_success_closure => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCompletedFlow")
WitnessTransitionObserved_workgraph_flow_success_closure_work_execution_CommitEvidenceProjection == WitnessScriptComplete_workgraph_flow_success_closure => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitEvidenceProjection")
WitnessTransitionObserved_workgraph_flow_success_closure_work_execution_CommitWorkClosure == WitnessScriptComplete_workgraph_flow_success_closure => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitWorkClosure")
WitnessTransitionObserved_workgraph_flow_failure_evidence_work_execution_BindExecution == WitnessScriptComplete_workgraph_flow_failure_evidence => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
WitnessTransitionObserved_workgraph_flow_failure_evidence_work_execution_ObserveFailedFlow == WitnessScriptComplete_workgraph_flow_failure_evidence => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveFailedFlow")
WitnessTransitionObserved_workgraph_flow_failure_evidence_work_execution_CommitFlowFailureEvidenceProjection == WitnessScriptComplete_workgraph_flow_failure_evidence => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitFlowFailureEvidenceProjection")
WitnessTransitionObserved_workgraph_flow_cancellation_evidence_work_execution_BindExecution == WitnessScriptComplete_workgraph_flow_cancellation_evidence => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
WitnessTransitionObserved_workgraph_flow_cancellation_evidence_work_execution_ObserveCanceledFlow == WitnessScriptComplete_workgraph_flow_cancellation_evidence => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCanceledFlow")
WitnessTransitionObserved_workgraph_flow_cancellation_evidence_work_execution_CommitFlowCancellationEvidenceProjection == WitnessScriptComplete_workgraph_flow_cancellation_evidence => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitFlowCancellationEvidenceProjection")
WitnessTransitionObserved_workgraph_flow_uncertain_abandonment_work_execution_BindExecution == WitnessScriptComplete_workgraph_flow_uncertain_abandonment => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
WitnessTransitionObserved_workgraph_flow_uncertain_abandonment_work_execution_RecordUncertainLaunch == WitnessScriptComplete_workgraph_flow_uncertain_abandonment => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "RecordUncertainLaunch")
WitnessTransitionObserved_workgraph_flow_uncertain_abandonment_work_execution_FailLaunch == WitnessScriptComplete_workgraph_flow_uncertain_abandonment => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "FailLaunch")
WitnessTransitionObserved_workgraph_flow_uncertain_abandonment_work_execution_CommitLaunchFailureEvidenceProjection == WitnessScriptComplete_workgraph_flow_uncertain_abandonment => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitLaunchFailureEvidenceProjection")
WitnessTransitionOrder_workgraph_flow_uncertain_abandonment_1 == WitnessScriptComplete_workgraph_flow_uncertain_abandonment => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "BindExecution" /\ later.machine = "work_execution" /\ later.transition = "RecordUncertainLaunch" /\ earlier.step < later.step)
WitnessTransitionOrder_workgraph_flow_uncertain_abandonment_2 == WitnessScriptComplete_workgraph_flow_uncertain_abandonment => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "RecordUncertainLaunch" /\ later.machine = "work_execution" /\ later.transition = "FailLaunch" /\ earlier.step < later.step)
WitnessTransitionOrder_workgraph_flow_uncertain_abandonment_3 == WitnessScriptComplete_workgraph_flow_uncertain_abandonment => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "FailLaunch" /\ later.machine = "work_execution" /\ later.transition = "CommitLaunchFailureEvidenceProjection" /\ earlier.step < later.step)
WitnessTransitionObserved_workgraph_flow_launch_quarantine_work_execution_BindExecution == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "BindExecution")
WitnessTransitionObserved_workgraph_flow_launch_quarantine_work_execution_QuarantineLaunch == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "QuarantineLaunch")
WitnessTransitionObserved_workgraph_flow_launch_quarantine_work_execution_ObserveCompletedFlow == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "ObserveCompletedFlow")
WitnessTransitionObserved_workgraph_flow_launch_quarantine_work_execution_CommitEvidenceProjection == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitEvidenceProjection")
WitnessTransitionObserved_workgraph_flow_launch_quarantine_work_execution_CommitWorkClosure == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E packet \in observed_transitions : /\ packet.machine = "work_execution" /\ packet.transition = "CommitWorkClosure")
WitnessTransitionOrder_workgraph_flow_launch_quarantine_1 == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "BindExecution" /\ later.machine = "work_execution" /\ later.transition = "QuarantineLaunch" /\ earlier.step < later.step)
WitnessTransitionOrder_workgraph_flow_launch_quarantine_2 == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "QuarantineLaunch" /\ later.machine = "work_execution" /\ later.transition = "ObserveCompletedFlow" /\ earlier.step < later.step)
WitnessTransitionOrder_workgraph_flow_launch_quarantine_3 == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "ObserveCompletedFlow" /\ later.machine = "work_execution" /\ later.transition = "CommitEvidenceProjection" /\ earlier.step < later.step)
WitnessTransitionOrder_workgraph_flow_launch_quarantine_4 == WitnessScriptComplete_workgraph_flow_launch_quarantine => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "work_execution" /\ earlier.transition = "CommitEvidenceProjection" /\ later.machine = "work_execution" /\ later.transition = "CommitWorkClosure" /\ earlier.step < later.step)

THEOREM Spec => []flow_launch_handoff
THEOREM Spec => []flow_observation_handoff
THEOREM Spec => []uncertain_launch_handoff
THEOREM Spec => []quarantined_launch_handoff
THEOREM Spec => []success_evidence_handoff
THEOREM Spec => []failure_evidence_handoff
THEOREM Spec => []cancellation_evidence_handoff
THEOREM Spec => []launch_failure_evidence_handoff
THEOREM Spec => []work_closure_handoff
THEOREM Spec => []work_execution_identified_after_bind
THEOREM Spec => []work_execution_evidence_projection_is_typed
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_flow_launch
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_flow_observation
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_uncertain_launch_resolution
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_quarantined_launch_resolution
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_success_evidence_projection
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_failure_evidence_projection
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_cancellation_evidence_projection
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_launch_failure_evidence_projection
THEOREM Spec => []NoOpenObligationsOnTerminal_work_execution_work_execution_work_closure
THEOREM Spec => []OwnerFeedbackHasProtocolProvenance

=============================================================================
