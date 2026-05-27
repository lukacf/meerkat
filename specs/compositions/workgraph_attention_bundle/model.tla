---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for workgraph_attention_bundle.

CONSTANTS NatValues, SetOfWorkDependencyPathKeyValues, SetOfWorkEdgeKeyValues, SetOfWorkItemKeyValues, WorkAttentionBindingKeyValues, WorkCompletionPolicyValues, WorkDependencyPathKeyValues, WorkEdgeKeyValues, WorkEdgeKindValues, WorkItemKeyValues, WorkOwnerKeyValues

WorkOwnerKeyValuesCi == {}

WorkOwnerKeyValuesDeep == {[kind |-> "Principal", id |-> "alpha"], [kind |-> "Agent", id |-> "beta"], [kind |-> "Session", id |-> "alpha"]}

WorkOwnerKeyValuesWitnessclose_stops_attention_route == {[kind |-> "Principal", id |-> "alpha"], [kind |-> "Agent", id |-> "beta"]}

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
OptionWorkAttentionBindingKeyValues == {None} \cup {Some(x) : x \in WorkAttentionBindingKeyValues}
OptionWorkOwnerKeyValues == {None} \cup {Some(x) : x \in WorkOwnerKeyValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))
AppendIfMissing(seq, value) == IF value \in SeqElements(seq) THEN seq ELSE Append(seq, value)
Machines == {
    <<"workgraph", "WorkGraphLifecycleMachine", "workgraph_authority">>,
    <<"attention", "WorkAttentionLifecycleMachine", "attention_authority">>
}

RouteNames == {
    "work_item_close_stops_attention"
}

Actors == {
    "workgraph_authority",
    "attention_authority"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "workgraph" -> "workgraph_authority"
      [] machine_id = "attention" -> "attention_authority"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    CASE route_name = "work_item_close_stops_attention" -> "workgraph"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "work_item_close_stops_attention" -> "Closed"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "work_item_close_stops_attention" -> "attention"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "work_item_close_stops_attention" -> "Stop"
      [] OTHER -> "unknown_input"

RouteTargetKind(route_name) ==
    CASE route_name = "work_item_close_stops_attention" -> "Input"
      [] OTHER -> "Unknown"

RouteDeliveryKind(route_name) ==
    CASE route_name = "work_item_close_stops_attention" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ workgraph_phase = "Absent"
    /\ workgraph_revision = 0
    /\ workgraph_unresolved_blocker_count = 0
    /\ workgraph_topology_item_keys = {}
    /\ workgraph_topology_edge_keys = {}
    /\ workgraph_blocks_reachability = {}
    /\ workgraph_parent_reachability = {}
    /\ workgraph_claim_owner_key = None
    /\ workgraph_claimed_at_utc_ms = None
    /\ workgraph_lease_expires_at_utc_ms = None
    /\ workgraph_due_at_utc_ms = None
    /\ workgraph_not_before_utc_ms = None
    /\ workgraph_snoozed_until_utc_ms = None
    /\ workgraph_completion_policy = "SelfAttest"
    /\ workgraph_completion_supervisor_owner_key = None
    /\ workgraph_completion_reviewer_quorum_threshold = None
    /\ workgraph_terminal_at_utc_ms = None
    /\ workgraph_evidence_count = 0
    /\ attention_phase = "Active"
    /\ attention_revision = 1
    /\ attention_paused_until_utc_ms = None
    /\ attention_superseded_by_binding_key = None
    /\ attention_terminal_at_utc_ms = None
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

WitnessInit_close_stops_attention_route ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "workgraph", variant |-> "CreateOpen", payload |-> [completion_policy |-> "SelfAttest", completion_reviewer_quorum_threshold |-> None, completion_supervisor_owner_key |-> None, due_at_utc_ms |-> None, not_before_utc_ms |-> None, snoozed_until_utc_ms |-> None, unresolved_blocker_count |-> 0], source_kind |-> "entry", source_route |-> "witness:close_stops_attention_route:1", source_machine |-> "external_entry", source_effect |-> "CreateOpen", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "workgraph", variant |-> "CreateOpen", payload |-> [completion_policy |-> "SelfAttest", completion_reviewer_quorum_threshold |-> None, completion_supervisor_owner_key |-> None, due_at_utc_ms |-> None, not_before_utc_ms |-> None, snoozed_until_utc_ms |-> None, unresolved_blocker_count |-> 0], source_kind |-> "entry", source_route |-> "witness:close_stops_attention_route:1", source_machine |-> "external_entry", source_effect |-> "CreateOpen", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "workgraph", variant |-> "CreateOpen", payload |-> [completion_policy |-> "SelfAttest", completion_reviewer_quorum_threshold |-> None, completion_supervisor_owner_key |-> None, due_at_utc_ms |-> None, not_before_utc_ms |-> None, snoozed_until_utc_ms |-> None, unresolved_blocker_count |-> 0], source_kind |-> "entry", source_route |-> "witness:close_stops_attention_route:1", source_machine |-> "external_entry", source_effect |-> "CreateOpen", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "workgraph", variant |-> "CloseCompleted", payload |-> [at_utc_ms |-> 1, expected_revision |-> 1], source_kind |-> "entry", source_route |-> "witness:close_stops_attention_route:2", source_machine |-> "external_entry", source_effect |-> "CloseCompleted", effect_id |-> 0]>>

workgraph__completion_policy_payload_valid(policy, supervisor_owner_key, reviewer_quorum_threshold) == (IF (policy = "Supervisor") THEN ((supervisor_owner_key # None) /\ (reviewer_quorum_threshold = None)) ELSE (IF (policy = "ReviewerQuorum") THEN ((supervisor_owner_key = None) /\ (reviewer_quorum_threshold # None) /\ ((IF "value" \in DOMAIN reviewer_quorum_threshold THEN reviewer_quorum_threshold["value"] ELSE None) > 0)) ELSE ((supervisor_owner_key = None) /\ (reviewer_quorum_threshold = None))))

workgraph_CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CreateOpen"
       /\ packet.payload.due_at_utc_ms = arg_due_at_utc_ms
       /\ packet.payload.not_before_utc_ms = arg_not_before_utc_ms
       /\ packet.payload.snoozed_until_utc_ms = arg_snoozed_until_utc_ms
       /\ packet.payload.completion_policy = arg_completion_policy
       /\ packet.payload.completion_supervisor_owner_key = arg_completion_supervisor_owner_key
       /\ packet.payload.completion_reviewer_quorum_threshold = arg_completion_reviewer_quorum_threshold
       /\ packet.payload.unresolved_blocker_count = arg_unresolved_blocker_count
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Absent"
       /\ workgraph__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold)
       /\ workgraph_phase' = "Open"
       /\ workgraph_revision' = 1
       /\ workgraph_unresolved_blocker_count' = packet.payload.unresolved_blocker_count
       /\ workgraph_due_at_utc_ms' = packet.payload.due_at_utc_ms
       /\ workgraph_not_before_utc_ms' = packet.payload.not_before_utc_ms
       /\ workgraph_snoozed_until_utc_ms' = packet.payload.snoozed_until_utc_ms
       /\ workgraph_completion_policy' = packet.payload.completion_policy
       /\ workgraph_completion_supervisor_owner_key' = packet.payload.completion_supervisor_owner_key
       /\ workgraph_completion_reviewer_quorum_threshold' = packet.payload.completion_reviewer_quorum_threshold
       /\ UNCHANGED << workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Created", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CreateOpen"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CreateOpen", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Open"]}
       /\ model_step_count' = model_step_count + 1


workgraph_CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CreateBlocked"
       /\ packet.payload.due_at_utc_ms = arg_due_at_utc_ms
       /\ packet.payload.not_before_utc_ms = arg_not_before_utc_ms
       /\ packet.payload.snoozed_until_utc_ms = arg_snoozed_until_utc_ms
       /\ packet.payload.completion_policy = arg_completion_policy
       /\ packet.payload.completion_supervisor_owner_key = arg_completion_supervisor_owner_key
       /\ packet.payload.completion_reviewer_quorum_threshold = arg_completion_reviewer_quorum_threshold
       /\ packet.payload.unresolved_blocker_count = arg_unresolved_blocker_count
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Absent"
       /\ workgraph__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold)
       /\ workgraph_phase' = "Blocked"
       /\ workgraph_revision' = 1
       /\ workgraph_unresolved_blocker_count' = packet.payload.unresolved_blocker_count
       /\ workgraph_due_at_utc_ms' = packet.payload.due_at_utc_ms
       /\ workgraph_not_before_utc_ms' = packet.payload.not_before_utc_ms
       /\ workgraph_snoozed_until_utc_ms' = packet.payload.snoozed_until_utc_ms
       /\ workgraph_completion_policy' = packet.payload.completion_policy
       /\ workgraph_completion_supervisor_owner_key' = packet.payload.completion_supervisor_owner_key
       /\ workgraph_completion_reviewer_quorum_threshold' = packet.payload.completion_reviewer_quorum_threshold
       /\ UNCHANGED << workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Created", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CreateBlocked"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CreateBlocked", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Blocked"]}
       /\ model_step_count' = model_step_count + 1


workgraph_UpdateOpen(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Update"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.due_at_utc_ms = arg_due_at_utc_ms
       /\ packet.payload.not_before_utc_ms = arg_not_before_utc_ms
       /\ packet.payload.snoozed_until_utc_ms = arg_snoozed_until_utc_ms
       /\ packet.payload.completion_policy = arg_completion_policy
       /\ packet.payload.completion_supervisor_owner_key = arg_completion_supervisor_owner_key
       /\ packet.payload.completion_reviewer_quorum_threshold = arg_completion_reviewer_quorum_threshold
       /\ packet.payload.unresolved_blocker_count = arg_unresolved_blocker_count
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Open"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold)
       /\ workgraph_phase' = "Open"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_unresolved_blocker_count' = packet.payload.unresolved_blocker_count
       /\ workgraph_due_at_utc_ms' = packet.payload.due_at_utc_ms
       /\ workgraph_not_before_utc_ms' = packet.payload.not_before_utc_ms
       /\ workgraph_snoozed_until_utc_ms' = packet.payload.snoozed_until_utc_ms
       /\ workgraph_completion_policy' = packet.payload.completion_policy
       /\ workgraph_completion_supervisor_owner_key' = packet.payload.completion_supervisor_owner_key
       /\ workgraph_completion_reviewer_quorum_threshold' = packet.payload.completion_reviewer_quorum_threshold
       /\ UNCHANGED << workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Updated", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UpdateOpen"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "UpdateOpen", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Open"]}
       /\ model_step_count' = model_step_count + 1


workgraph_UpdateInProgress(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Update"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.due_at_utc_ms = arg_due_at_utc_ms
       /\ packet.payload.not_before_utc_ms = arg_not_before_utc_ms
       /\ packet.payload.snoozed_until_utc_ms = arg_snoozed_until_utc_ms
       /\ packet.payload.completion_policy = arg_completion_policy
       /\ packet.payload.completion_supervisor_owner_key = arg_completion_supervisor_owner_key
       /\ packet.payload.completion_reviewer_quorum_threshold = arg_completion_reviewer_quorum_threshold
       /\ packet.payload.unresolved_blocker_count = arg_unresolved_blocker_count
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold)
       /\ workgraph_phase' = "InProgress"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_unresolved_blocker_count' = packet.payload.unresolved_blocker_count
       /\ workgraph_due_at_utc_ms' = packet.payload.due_at_utc_ms
       /\ workgraph_not_before_utc_ms' = packet.payload.not_before_utc_ms
       /\ workgraph_snoozed_until_utc_ms' = packet.payload.snoozed_until_utc_ms
       /\ workgraph_completion_policy' = packet.payload.completion_policy
       /\ workgraph_completion_supervisor_owner_key' = packet.payload.completion_supervisor_owner_key
       /\ workgraph_completion_reviewer_quorum_threshold' = packet.payload.completion_reviewer_quorum_threshold
       /\ UNCHANGED << workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Updated", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UpdateInProgress"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "UpdateInProgress", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "InProgress"]}
       /\ model_step_count' = model_step_count + 1


workgraph_UpdateBlocked(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Update"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.due_at_utc_ms = arg_due_at_utc_ms
       /\ packet.payload.not_before_utc_ms = arg_not_before_utc_ms
       /\ packet.payload.snoozed_until_utc_ms = arg_snoozed_until_utc_ms
       /\ packet.payload.completion_policy = arg_completion_policy
       /\ packet.payload.completion_supervisor_owner_key = arg_completion_supervisor_owner_key
       /\ packet.payload.completion_reviewer_quorum_threshold = arg_completion_reviewer_quorum_threshold
       /\ packet.payload.unresolved_blocker_count = arg_unresolved_blocker_count
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Blocked"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold)
       /\ workgraph_phase' = "Blocked"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_unresolved_blocker_count' = packet.payload.unresolved_blocker_count
       /\ workgraph_due_at_utc_ms' = packet.payload.due_at_utc_ms
       /\ workgraph_not_before_utc_ms' = packet.payload.not_before_utc_ms
       /\ workgraph_snoozed_until_utc_ms' = packet.payload.snoozed_until_utc_ms
       /\ workgraph_completion_policy' = packet.payload.completion_policy
       /\ workgraph_completion_supervisor_owner_key' = packet.payload.completion_supervisor_owner_key
       /\ workgraph_completion_reviewer_quorum_threshold' = packet.payload.completion_reviewer_quorum_threshold
       /\ UNCHANGED << workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Updated", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UpdateBlocked"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "UpdateBlocked", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Blocked"]}
       /\ model_step_count' = model_step_count + 1


workgraph_ClaimOpen(arg_expected_revision, arg_owner_key, arg_now_utc_ms, arg_lease_expires_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Claim"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.owner_key = arg_owner_key
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ packet.payload.lease_expires_at_utc_ms = arg_lease_expires_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Open"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ (workgraph_unresolved_blocker_count = 0)
       /\ (IF (workgraph_due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_due_at_utc_ms THEN workgraph_due_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ (IF (workgraph_not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_not_before_utc_ms THEN workgraph_not_before_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ (IF (workgraph_snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_snoozed_until_utc_ms THEN workgraph_snoozed_until_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ workgraph_phase' = "InProgress"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = Some(packet.payload.owner_key)
       /\ workgraph_claimed_at_utc_ms' = Some(packet.payload.now_utc_ms)
       /\ workgraph_lease_expires_at_utc_ms' = packet.payload.lease_expires_at_utc_ms
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Claimed", payload |-> [owner_key |-> packet.payload.owner_key], effect_id |-> (model_step_count + 1), source_transition |-> "ClaimOpen"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "ClaimOpen", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "InProgress"]}
       /\ model_step_count' = model_step_count + 1


workgraph_ClaimExpiredInProgress(arg_expected_revision, arg_owner_key, arg_now_utc_ms, arg_lease_expires_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Claim"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.owner_key = arg_owner_key
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ packet.payload.lease_expires_at_utc_ms = arg_lease_expires_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ (workgraph_claim_owner_key # None)
       /\ (workgraph_lease_expires_at_utc_ms # None)
       /\ (IF (workgraph_lease_expires_at_utc_ms = None) THEN FALSE ELSE ((IF "value" \in DOMAIN workgraph_lease_expires_at_utc_ms THEN workgraph_lease_expires_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ (workgraph_unresolved_blocker_count = 0)
       /\ (IF (workgraph_due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_due_at_utc_ms THEN workgraph_due_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ (IF (workgraph_not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_not_before_utc_ms THEN workgraph_not_before_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ (IF (workgraph_snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_snoozed_until_utc_ms THEN workgraph_snoozed_until_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ workgraph_phase' = "InProgress"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = Some(packet.payload.owner_key)
       /\ workgraph_claimed_at_utc_ms' = Some(packet.payload.now_utc_ms)
       /\ workgraph_lease_expires_at_utc_ms' = packet.payload.lease_expires_at_utc_ms
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Claimed", payload |-> [owner_key |-> packet.payload.owner_key], effect_id |-> (model_step_count + 1), source_transition |-> "ClaimExpiredInProgress"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "ClaimExpiredInProgress", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "InProgress"]}
       /\ model_step_count' = model_step_count + 1


workgraph_ReleaseInProgress(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Release"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ ((workgraph_revision = packet.payload.expected_revision) /\ (workgraph_claim_owner_key # None))
       /\ workgraph_phase' = "Open"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Released", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ReleaseInProgress"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "ReleaseInProgress", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Open"]}
       /\ model_step_count' = model_step_count + 1


workgraph_BlockOpen(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Block"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Open"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Blocked"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Blocked", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BlockOpen"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "BlockOpen", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Blocked"]}
       /\ model_step_count' = model_step_count + 1


workgraph_BlockInProgress(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Block"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Blocked"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Blocked", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BlockInProgress"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "BlockInProgress", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Blocked"]}
       /\ model_step_count' = model_step_count + 1


workgraph_BlockBlocked(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "Block"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Blocked"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Blocked"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Blocked", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BlockBlocked"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "BlockBlocked", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Blocked"]}
       /\ model_step_count' = model_step_count + 1


workgraph_RefreshEligibilityOpen(arg_unresolved_blocker_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "RefreshEligibility"
       /\ packet.payload.unresolved_blocker_count = arg_unresolved_blocker_count
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Open"
       /\ workgraph_phase' = "Open"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_unresolved_blocker_count' = packet.payload.unresolved_blocker_count
       /\ UNCHANGED << workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "RefreshEligibilityOpen", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Open"]}
       /\ model_step_count' = model_step_count + 1


workgraph_RefreshEligibilityInProgress(arg_unresolved_blocker_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "RefreshEligibility"
       /\ packet.payload.unresolved_blocker_count = arg_unresolved_blocker_count
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ workgraph_phase' = "InProgress"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_unresolved_blocker_count' = packet.payload.unresolved_blocker_count
       /\ UNCHANGED << workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "RefreshEligibilityInProgress", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "InProgress"]}
       /\ model_step_count' = model_step_count + 1


workgraph_RefreshEligibilityBlocked(arg_unresolved_blocker_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "RefreshEligibility"
       /\ packet.payload.unresolved_blocker_count = arg_unresolved_blocker_count
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Blocked"
       /\ workgraph_phase' = "Blocked"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_unresolved_blocker_count' = packet.payload.unresolved_blocker_count
       /\ UNCHANGED << workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "RefreshEligibilityBlocked", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Blocked"]}
       /\ model_step_count' = model_step_count + 1


workgraph_ValidateLink(arg_kind, arg_from_item_key, arg_to_item_key, arg_edge_key, arg_reverse_path_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "ValidateLink"
       /\ packet.payload.kind = arg_kind
       /\ packet.payload.from_item_key = arg_from_item_key
       /\ packet.payload.to_item_key = arg_to_item_key
       /\ packet.payload.edge_key = arg_edge_key
       /\ packet.payload.reverse_path_key = arg_reverse_path_key
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Absent"
       /\ (packet.payload.from_item_key \in workgraph_topology_item_keys)
       /\ (packet.payload.to_item_key \in workgraph_topology_item_keys)
       /\ (packet.payload.from_item_key # packet.payload.to_item_key)
       /\ ((packet.payload.edge_key \in workgraph_topology_edge_keys) = FALSE)
       /\ ((packet.payload.kind # "Blocks") \/ ((packet.payload.reverse_path_key \in workgraph_blocks_reachability) = FALSE))
       /\ ((packet.payload.kind # "Parent") \/ ((packet.payload.reverse_path_key \in workgraph_parent_reachability) = FALSE))
       /\ workgraph_phase' = "Absent"
       /\ UNCHANGED << workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "LinkValidated", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ValidateLink"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "ValidateLink", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Absent"]}
       /\ model_step_count' = model_step_count + 1


workgraph_CloseOpenCompleted(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseCompleted"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Open"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Completed"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseOpenCompleted"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseOpenCompleted"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseOpenCompleted", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Completed"]}
           /\ model_step_count' = model_step_count + 1


workgraph_CloseInProgressCompleted(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseCompleted"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Completed"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseInProgressCompleted"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseInProgressCompleted"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseInProgressCompleted", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Completed"]}
           /\ model_step_count' = model_step_count + 1


workgraph_CloseBlockedCompleted(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseCompleted"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Blocked"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Completed"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseBlockedCompleted"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseBlockedCompleted"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseBlockedCompleted", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Completed"]}
           /\ model_step_count' = model_step_count + 1


workgraph_CloseOpenCancelled(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseCancelled"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Open"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Cancelled"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseOpenCancelled"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseOpenCancelled"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseOpenCancelled", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Cancelled"]}
           /\ model_step_count' = model_step_count + 1


workgraph_CloseInProgressCancelled(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseCancelled"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Cancelled"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseInProgressCancelled"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseInProgressCancelled"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseInProgressCancelled", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Cancelled"]}
           /\ model_step_count' = model_step_count + 1


workgraph_CloseBlockedCancelled(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseCancelled"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Blocked"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Cancelled"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseBlockedCancelled"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseBlockedCancelled"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseBlockedCancelled", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Cancelled"]}
           /\ model_step_count' = model_step_count + 1


workgraph_CloseOpenFailed(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseFailed"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Open"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Failed"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseOpenFailed"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseOpenFailed"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseOpenFailed", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Failed"]}
           /\ model_step_count' = model_step_count + 1


workgraph_CloseInProgressFailed(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseFailed"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Failed"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseInProgressFailed"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseInProgressFailed"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseInProgressFailed", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Failed"]}
           /\ model_step_count' = model_step_count + 1


workgraph_CloseBlockedFailed(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "CloseFailed"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Blocked"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Failed"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_claim_owner_key' = None
       /\ workgraph_claimed_at_utc_ms' = None
       /\ workgraph_lease_expires_at_utc_ms' = None
       /\ workgraph_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ \E route_owner_ctx_work_item_close_stops_attention_expected_revision \in {attention_revision} :
           /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)])
           /\ observed_inputs' = observed_inputs \cup {[machine |-> "attention", variant |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], source_kind |-> "route", source_route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", source_effect |-> "Closed", effect_id |-> (model_step_count + 1)]}
           /\ pending_routes' = pending_routes
           /\ delivered_routes' = delivered_routes \cup { [route |-> "work_item_close_stops_attention", source_machine |-> "workgraph", effect |-> "Closed", target_machine |-> "attention", target_input |-> "Stop", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, expected_revision |-> route_owner_ctx_work_item_close_stops_attention_expected_revision], actor |-> "attention_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CloseBlockedFailed"] }
           /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "Closed", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, terminal_state |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "CloseBlockedFailed"] }
           /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "CloseBlockedFailed", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Failed"]}
           /\ model_step_count' = model_step_count + 1


workgraph_AddEvidenceOpen(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "AddEvidence"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Open"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Open"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_evidence_count' = (workgraph_evidence_count) + 1
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "EvidenceAdded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AddEvidenceOpen"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "AddEvidenceOpen", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Open"]}
       /\ model_step_count' = model_step_count + 1


workgraph_AddEvidenceInProgress(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "AddEvidence"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "InProgress"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "InProgress"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_evidence_count' = (workgraph_evidence_count) + 1
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "EvidenceAdded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AddEvidenceInProgress"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "AddEvidenceInProgress", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "InProgress"]}
       /\ model_step_count' = model_step_count + 1


workgraph_AddEvidenceBlocked(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "AddEvidence"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Blocked"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Blocked"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_evidence_count' = (workgraph_evidence_count) + 1
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "EvidenceAdded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AddEvidenceBlocked"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "AddEvidenceBlocked", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Blocked"]}
       /\ model_step_count' = model_step_count + 1


workgraph_AddEvidenceCompleted(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "AddEvidence"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Completed"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Completed"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_evidence_count' = (workgraph_evidence_count) + 1
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "EvidenceAdded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AddEvidenceCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "AddEvidenceCompleted", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


workgraph_AddEvidenceCancelled(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "AddEvidence"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Cancelled"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Cancelled"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_evidence_count' = (workgraph_evidence_count) + 1
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "EvidenceAdded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AddEvidenceCancelled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "AddEvidenceCancelled", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


workgraph_AddEvidenceFailed(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "workgraph"
       /\ packet.variant = "AddEvidence"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("workgraph_authority")
       /\ workgraph_phase = "Failed"
       /\ (workgraph_revision = packet.payload.expected_revision)
       /\ workgraph_phase' = "Failed"
       /\ workgraph_revision' = (workgraph_revision) + 1
       /\ workgraph_evidence_count' = (workgraph_evidence_count) + 1
       /\ UNCHANGED << workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "workgraph", variant |-> "EvidenceAdded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AddEvidenceFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "workgraph", transition |-> "AddEvidenceFailed", actor |-> "workgraph_authority", step |-> (model_step_count + 1), from_phase |-> workgraph_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


workgraph_absent_has_zero_revision == ((workgraph_phase # "Absent") \/ (workgraph_revision = 0))
workgraph_live_has_positive_revision == ((workgraph_phase = "Absent") \/ (workgraph_revision > 0))
workgraph_topology_snapshot_is_stateless == ((workgraph_topology_item_keys = {}) \/ (workgraph_topology_edge_keys = {}) \/ (workgraph_phase = "Absent"))
workgraph_terminal_has_terminal_time == (((workgraph_phase # "Completed") /\ (workgraph_phase # "Cancelled") /\ (workgraph_phase # "Failed")) \/ (workgraph_terminal_at_utc_ms # None))
workgraph_claim_only_in_progress == ((workgraph_claim_owner_key = None) \/ (workgraph_phase = "InProgress"))
workgraph_blocked_has_no_claim == ((workgraph_phase # "Blocked") \/ (workgraph_claim_owner_key = None))
workgraph_terminal_has_no_claim == (((workgraph_phase # "Completed") /\ (workgraph_phase # "Cancelled") /\ (workgraph_phase # "Failed")) \/ (workgraph_claim_owner_key = None))
workgraph_supervisor_policy_has_owner == ((workgraph_completion_policy # "Supervisor") \/ (workgraph_completion_supervisor_owner_key # None))
workgraph_non_supervisor_policy_has_no_owner == ((workgraph_completion_policy = "Supervisor") \/ (workgraph_completion_supervisor_owner_key = None))
workgraph_reviewer_quorum_policy_has_positive_threshold == ((workgraph_completion_policy # "ReviewerQuorum") \/ ((workgraph_completion_reviewer_quorum_threshold # None) /\ ((IF "value" \in DOMAIN workgraph_completion_reviewer_quorum_threshold THEN workgraph_completion_reviewer_quorum_threshold["value"] ELSE None) > 0)))
workgraph_non_reviewer_quorum_policy_has_no_threshold == ((workgraph_completion_policy = "ReviewerQuorum") \/ (workgraph_completion_reviewer_quorum_threshold = None))

attention_PauseActive(arg_expected_revision, arg_until_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "attention"
       /\ packet.variant = "Pause"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.until_utc_ms = arg_until_utc_ms
       /\ ~HigherPriorityReady("attention_authority")
       /\ attention_phase = "Active"
       /\ (attention_revision = packet.payload.expected_revision)
       /\ attention_phase' = "Paused"
       /\ attention_revision' = (attention_revision) + 1
       /\ attention_paused_until_utc_ms' = packet.payload.until_utc_ms
       /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "attention", variant |-> "AttentionPaused", payload |-> [revision |-> (attention_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "PauseActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "attention", transition |-> "PauseActive", actor |-> "attention_authority", step |-> (model_step_count + 1), from_phase |-> attention_phase, to_phase |-> "Paused"]}
       /\ model_step_count' = model_step_count + 1


attention_PausePaused(arg_expected_revision, arg_until_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "attention"
       /\ packet.variant = "Pause"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.until_utc_ms = arg_until_utc_ms
       /\ ~HigherPriorityReady("attention_authority")
       /\ attention_phase = "Paused"
       /\ (attention_revision = packet.payload.expected_revision)
       /\ attention_phase' = "Paused"
       /\ attention_revision' = (attention_revision) + 1
       /\ attention_paused_until_utc_ms' = packet.payload.until_utc_ms
       /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "attention", variant |-> "AttentionPaused", payload |-> [revision |-> (attention_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "PausePaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "attention", transition |-> "PausePaused", actor |-> "attention_authority", step |-> (model_step_count + 1), from_phase |-> attention_phase, to_phase |-> "Paused"]}
       /\ model_step_count' = model_step_count + 1


attention_ResumePaused(arg_expected_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "attention"
       /\ packet.variant = "Resume"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ ~HigherPriorityReady("attention_authority")
       /\ attention_phase = "Paused"
       /\ (attention_revision = packet.payload.expected_revision)
       /\ attention_phase' = "Active"
       /\ attention_revision' = (attention_revision) + 1
       /\ attention_paused_until_utc_ms' = None
       /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "attention", variant |-> "AttentionResumed", payload |-> [revision |-> (attention_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "ResumePaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "attention", transition |-> "ResumePaused", actor |-> "attention_authority", step |-> (model_step_count + 1), from_phase |-> attention_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


attention_SupersedeActive(arg_expected_revision, arg_superseded_by_binding_key, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "attention"
       /\ packet.variant = "Supersede"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.superseded_by_binding_key = arg_superseded_by_binding_key
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("attention_authority")
       /\ attention_phase = "Active"
       /\ (attention_revision = packet.payload.expected_revision)
       /\ attention_phase' = "Superseded"
       /\ attention_revision' = (attention_revision) + 1
       /\ attention_paused_until_utc_ms' = None
       /\ attention_superseded_by_binding_key' = Some(packet.payload.superseded_by_binding_key)
       /\ attention_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "attention", variant |-> "AttentionSuperseded", payload |-> [revision |-> (attention_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedeActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "attention", transition |-> "SupersedeActive", actor |-> "attention_authority", step |-> (model_step_count + 1), from_phase |-> attention_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


attention_SupersedePaused(arg_expected_revision, arg_superseded_by_binding_key, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "attention"
       /\ packet.variant = "Supersede"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.superseded_by_binding_key = arg_superseded_by_binding_key
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("attention_authority")
       /\ attention_phase = "Paused"
       /\ (attention_revision = packet.payload.expected_revision)
       /\ attention_phase' = "Superseded"
       /\ attention_revision' = (attention_revision) + 1
       /\ attention_paused_until_utc_ms' = None
       /\ attention_superseded_by_binding_key' = Some(packet.payload.superseded_by_binding_key)
       /\ attention_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "attention", variant |-> "AttentionSuperseded", payload |-> [revision |-> (attention_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "attention", transition |-> "SupersedePaused", actor |-> "attention_authority", step |-> (model_step_count + 1), from_phase |-> attention_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


attention_StopActive(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "attention"
       /\ packet.variant = "Stop"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("attention_authority")
       /\ attention_phase = "Active"
       /\ (attention_revision = packet.payload.expected_revision)
       /\ attention_phase' = "Stopped"
       /\ attention_revision' = (attention_revision) + 1
       /\ attention_paused_until_utc_ms' = None
       /\ attention_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_superseded_by_binding_key, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "attention", variant |-> "AttentionStopped", payload |-> [revision |-> (attention_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "StopActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "attention", transition |-> "StopActive", actor |-> "attention_authority", step |-> (model_step_count + 1), from_phase |-> attention_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


attention_StopPaused(arg_expected_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "attention"
       /\ packet.variant = "Stop"
       /\ packet.payload.expected_revision = arg_expected_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("attention_authority")
       /\ attention_phase = "Paused"
       /\ (attention_revision = packet.payload.expected_revision)
       /\ attention_phase' = "Stopped"
       /\ attention_revision' = (attention_revision) + 1
       /\ attention_paused_until_utc_ms' = None
       /\ attention_terminal_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_superseded_by_binding_key, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "attention", variant |-> "AttentionStopped", payload |-> [revision |-> (attention_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "StopPaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "attention", transition |-> "StopPaused", actor |-> "attention_authority", step |-> (model_step_count + 1), from_phase |-> attention_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


attention_live_has_no_terminal_time == (((attention_phase # "Active") /\ (attention_phase # "Paused")) \/ (attention_terminal_at_utc_ms = None))
attention_paused_has_pause_state == ((attention_phase = "Paused") \/ (attention_paused_until_utc_ms = None))
attention_superseded_records_successor == ((attention_phase # "Superseded") \/ (attention_superseded_by_binding_key # None))

workgraph__entry_packet__completion_policy_payload_valid(policy, supervisor_owner_key, reviewer_quorum_threshold) == (IF (policy = "Supervisor") THEN ((supervisor_owner_key # None) /\ (reviewer_quorum_threshold = None)) ELSE (IF (policy = "ReviewerQuorum") THEN ((supervisor_owner_key = None) /\ (reviewer_quorum_threshold # None) /\ ((IF "value" \in DOMAIN reviewer_quorum_threshold THEN reviewer_quorum_threshold["value"] ELSE None) > 0)) ELSE ((supervisor_owner_key = None) /\ (reviewer_quorum_threshold = None))))

EntryPacketAdmissible_workgraph(packet) ==
    \/ /\ (packet.variant = "CreateOpen") /\ (workgraph_phase = "Absent") /\ (workgraph__entry_packet__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold))
    \/ /\ (packet.variant = "CreateBlocked") /\ (workgraph_phase = "Absent") /\ (workgraph__entry_packet__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold))
    \/ /\ (packet.variant = "Update") /\ (workgraph_phase = "Open") /\ ((workgraph_revision = packet.payload.expected_revision)) /\ (workgraph__entry_packet__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold))
    \/ /\ (packet.variant = "Update") /\ (workgraph_phase = "InProgress") /\ ((workgraph_revision = packet.payload.expected_revision)) /\ (workgraph__entry_packet__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold))
    \/ /\ (packet.variant = "Update") /\ (workgraph_phase = "Blocked") /\ ((workgraph_revision = packet.payload.expected_revision)) /\ (workgraph__entry_packet__completion_policy_payload_valid(packet.payload.completion_policy, packet.payload.completion_supervisor_owner_key, packet.payload.completion_reviewer_quorum_threshold))
    \/ /\ (packet.variant = "Claim") /\ (workgraph_phase = "Open") /\ ((workgraph_revision = packet.payload.expected_revision)) /\ ((workgraph_unresolved_blocker_count = 0)) /\ ((IF (workgraph_due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_due_at_utc_ms THEN workgraph_due_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))) /\ ((IF (workgraph_not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_not_before_utc_ms THEN workgraph_not_before_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))) /\ ((IF (workgraph_snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_snoozed_until_utc_ms THEN workgraph_snoozed_until_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms)))
    \/ /\ (packet.variant = "Claim") /\ (workgraph_phase = "InProgress") /\ ((workgraph_revision = packet.payload.expected_revision)) /\ ((workgraph_claim_owner_key # None)) /\ ((workgraph_lease_expires_at_utc_ms # None)) /\ ((IF (workgraph_lease_expires_at_utc_ms = None) THEN FALSE ELSE ((IF "value" \in DOMAIN workgraph_lease_expires_at_utc_ms THEN workgraph_lease_expires_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))) /\ ((workgraph_unresolved_blocker_count = 0)) /\ ((IF (workgraph_due_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_due_at_utc_ms THEN workgraph_due_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))) /\ ((IF (workgraph_not_before_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_not_before_utc_ms THEN workgraph_not_before_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))) /\ ((IF (workgraph_snoozed_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN workgraph_snoozed_until_utc_ms THEN workgraph_snoozed_until_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms)))
    \/ /\ (packet.variant = "Release") /\ (workgraph_phase = "InProgress") /\ (((workgraph_revision = packet.payload.expected_revision) /\ (workgraph_claim_owner_key # None)))
    \/ /\ (packet.variant = "Block") /\ (workgraph_phase = "Open") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "Block") /\ (workgraph_phase = "InProgress") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "Block") /\ (workgraph_phase = "Blocked") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "RefreshEligibility") /\ (workgraph_phase = "Open")
    \/ /\ (packet.variant = "RefreshEligibility") /\ (workgraph_phase = "InProgress")
    \/ /\ (packet.variant = "RefreshEligibility") /\ (workgraph_phase = "Blocked")
    \/ /\ (packet.variant = "ValidateLink") /\ (workgraph_phase = "Absent") /\ ((packet.payload.from_item_key \in workgraph_topology_item_keys)) /\ ((packet.payload.to_item_key \in workgraph_topology_item_keys)) /\ ((packet.payload.from_item_key # packet.payload.to_item_key)) /\ (((packet.payload.edge_key \in workgraph_topology_edge_keys) = FALSE)) /\ (((packet.payload.kind # "Blocks") \/ ((packet.payload.reverse_path_key \in workgraph_blocks_reachability) = FALSE))) /\ (((packet.payload.kind # "Parent") \/ ((packet.payload.reverse_path_key \in workgraph_parent_reachability) = FALSE)))
    \/ /\ (packet.variant = "CloseCompleted") /\ (workgraph_phase = "Open") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "CloseCompleted") /\ (workgraph_phase = "InProgress") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "CloseCompleted") /\ (workgraph_phase = "Blocked") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "CloseCancelled") /\ (workgraph_phase = "Open") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "CloseCancelled") /\ (workgraph_phase = "InProgress") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "CloseCancelled") /\ (workgraph_phase = "Blocked") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "CloseFailed") /\ (workgraph_phase = "Open") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "CloseFailed") /\ (workgraph_phase = "InProgress") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "CloseFailed") /\ (workgraph_phase = "Blocked") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "AddEvidence") /\ (workgraph_phase = "Open") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "AddEvidence") /\ (workgraph_phase = "InProgress") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "AddEvidence") /\ (workgraph_phase = "Blocked") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "AddEvidence") /\ (workgraph_phase = "Completed") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "AddEvidence") /\ (workgraph_phase = "Cancelled") /\ ((workgraph_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "AddEvidence") /\ (workgraph_phase = "Failed") /\ ((workgraph_revision = packet.payload.expected_revision))

EntryPacketAdmissible_attention(packet) ==
    \/ /\ (packet.variant = "Pause") /\ (attention_phase = "Active") /\ ((attention_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "Pause") /\ (attention_phase = "Paused") /\ ((attention_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "Resume") /\ (attention_phase = "Paused") /\ ((attention_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "Supersede") /\ (attention_phase = "Active") /\ ((attention_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "Supersede") /\ (attention_phase = "Paused") /\ ((attention_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "Stop") /\ (attention_phase = "Active") /\ ((attention_revision = packet.payload.expected_revision))
    \/ /\ (packet.variant = "Stop") /\ (attention_phase = "Paused") /\ ((attention_revision = packet.payload.expected_revision))

EntryPacketAdmissible(packet) ==
    CASE
      packet.machine = "workgraph" -> EntryPacketAdmissible_workgraph(packet)
      [] packet.machine = "attention" -> EntryPacketAdmissible_attention(packet)
      [] OTHER -> FALSE

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_close_stops_attention_route ==
    LET next_script_input == IF Len(witness_remaining_script_inputs) > 0 THEN Head(witness_remaining_script_inputs) ELSE witness_current_script_input
        next_remaining_script_inputs == IF Len(witness_remaining_script_inputs) > 0 THEN Tail(witness_remaining_script_inputs) ELSE <<>>
    IN
    /\ witness_current_script_input # None
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ EntryPacketAdmissible(next_script_input)
    /\ Len(pending_inputs) = 0
    /\ Len(pending_routes) = 0
    /\ Len(witness_remaining_script_inputs) > 0
    /\ pending_inputs' = Append(pending_inputs, next_script_input)
    /\ observed_inputs' = observed_inputs \cup {next_script_input}
    /\ witness_current_script_input' = next_script_input
    /\ witness_remaining_script_inputs' = next_remaining_script_inputs
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << workgraph_phase, workgraph_revision, workgraph_unresolved_blocker_count, workgraph_topology_item_keys, workgraph_topology_edge_keys, workgraph_blocks_reachability, workgraph_parent_reachability, workgraph_claim_owner_key, workgraph_claimed_at_utc_ms, workgraph_lease_expires_at_utc_ms, workgraph_due_at_utc_ms, workgraph_not_before_utc_ms, workgraph_snoozed_until_utc_ms, workgraph_completion_policy, workgraph_completion_supervisor_owner_key, workgraph_completion_reviewer_quorum_threshold, workgraph_terminal_at_utc_ms, workgraph_evidence_count, attention_phase, attention_revision, attention_paused_until_utc_ms, attention_superseded_by_binding_key, attention_terminal_at_utc_ms, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_UpdateOpen(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_UpdateInProgress(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_UpdateBlocked(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_owner_key \in WorkOwnerKeyValues : \E arg_now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : workgraph_ClaimOpen(arg_expected_revision, arg_owner_key, arg_now_utc_ms, arg_lease_expires_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_owner_key \in WorkOwnerKeyValues : \E arg_now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : workgraph_ClaimExpiredInProgress(arg_expected_revision, arg_owner_key, arg_now_utc_ms, arg_lease_expires_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_ReleaseInProgress(arg_expected_revision)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_BlockOpen(arg_expected_revision)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_BlockInProgress(arg_expected_revision)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_BlockBlocked(arg_expected_revision)
    \/ \E arg_unresolved_blocker_count \in 0..2 : workgraph_RefreshEligibilityOpen(arg_unresolved_blocker_count)
    \/ \E arg_unresolved_blocker_count \in 0..2 : workgraph_RefreshEligibilityInProgress(arg_unresolved_blocker_count)
    \/ \E arg_unresolved_blocker_count \in 0..2 : workgraph_RefreshEligibilityBlocked(arg_unresolved_blocker_count)
    \/ \E arg_kind \in WorkEdgeKindValues : \E arg_from_item_key \in WorkItemKeyValues : \E arg_to_item_key \in WorkItemKeyValues : \E arg_edge_key \in WorkEdgeKeyValues : \E arg_reverse_path_key \in WorkDependencyPathKeyValues : workgraph_ValidateLink(arg_kind, arg_from_item_key, arg_to_item_key, arg_edge_key, arg_reverse_path_key)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseOpenCompleted(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseInProgressCompleted(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseBlockedCompleted(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseOpenCancelled(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseInProgressCancelled(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseBlockedCancelled(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseOpenFailed(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseInProgressFailed(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseBlockedFailed(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceOpen(arg_expected_revision)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceInProgress(arg_expected_revision)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceBlocked(arg_expected_revision)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceCompleted(arg_expected_revision)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceCancelled(arg_expected_revision)
    \/ \E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceFailed(arg_expected_revision)
    \/ \E arg_expected_revision \in {attention_revision} : \E arg_until_utc_ms \in OptionU64Values : attention_PauseActive(arg_expected_revision, arg_until_utc_ms)
    \/ \E arg_expected_revision \in {attention_revision} : \E arg_until_utc_ms \in OptionU64Values : attention_PausePaused(arg_expected_revision, arg_until_utc_ms)
    \/ \E arg_expected_revision \in {attention_revision} : attention_ResumePaused(arg_expected_revision)
    \/ \E arg_expected_revision \in {attention_revision} : \E arg_superseded_by_binding_key \in WorkAttentionBindingKeyValues : \E arg_at_utc_ms \in 0..2 : attention_SupersedeActive(arg_expected_revision, arg_superseded_by_binding_key, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {attention_revision} : \E arg_superseded_by_binding_key \in WorkAttentionBindingKeyValues : \E arg_at_utc_ms \in 0..2 : attention_SupersedePaused(arg_expected_revision, arg_superseded_by_binding_key, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {attention_revision} : \E arg_at_utc_ms \in 0..2 : attention_StopActive(arg_expected_revision, arg_at_utc_ms)
    \/ \E arg_expected_revision \in {attention_revision} : \E arg_at_utc_ms \in 0..2 : attention_StopPaused(arg_expected_revision, arg_at_utc_ms)
    \/ QuiescentStutter

InjectNext ==
    FALSE

Next ==
    \/ CoreNext

WitnessNext_close_stops_attention_route ==
    \/ CoreNext
    \/ WitnessInjectNext_close_stops_attention_route


closed_work_item_routes_to_attention_stop == \E route_name \in RouteNames : /\ RouteSource(route_name) = "workgraph" /\ RouteEffect(route_name) = "Closed" /\ RouteTargetMachine(route_name) = "attention" /\ RouteTargetInput(route_name) = "Stop"
attention_stop_originates_from_work_item_close == \A input_packet \in observed_inputs : ((input_packet.machine = "attention" /\ input_packet.variant = "Stop" /\ input_packet.source_route = "work_item_close_stops_attention") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "workgraph" /\ input_packet.source_effect = "Closed" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "workgraph" /\ effect_packet.variant = "Closed" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = "work_item_close_stops_attention" /\ route_packet.source_machine = "workgraph" /\ route_packet.effect = "Closed" /\ route_packet.target_machine = "attention" /\ route_packet.target_input = "Stop" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))

RouteObserved_work_item_close_stops_attention == \E packet \in RoutePackets : packet.route = "work_item_close_stops_attention"
RouteCoverage_work_item_close_stops_attention == (RouteObserved_work_item_close_stops_attention \/ ~RouteObserved_work_item_close_stops_attention)
CoverageInstrumentation == RouteCoverage_work_item_close_stops_attention

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(workgraph_topology_item_keys) <= 0 /\ Cardinality(workgraph_topology_edge_keys) <= 0 /\ Cardinality(workgraph_blocks_reachability) <= 0 /\ Cardinality(workgraph_parent_reachability) <= 0
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(workgraph_topology_item_keys) <= 2 /\ Cardinality(workgraph_topology_edge_keys) <= 2 /\ Cardinality(workgraph_blocks_reachability) <= 2 /\ Cardinality(workgraph_parent_reachability) <= 2
WitnessStateConstraint_close_stops_attention_route == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 11 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 3 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(workgraph_topology_item_keys) <= 0 /\ Cardinality(workgraph_topology_edge_keys) <= 0 /\ Cardinality(workgraph_blocks_reachability) <= 0 /\ Cardinality(workgraph_parent_reachability) <= 0

Spec ==
    /\ Init
    /\ [][Next]_vars

WitnessFairness_close_stops_attention_route_1 ==
    /\ WF_vars(DeliverQueuedRoute)
    /\ WF_vars(\E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_CreateOpen(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count))
    /\ WF_vars(\E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_CreateBlocked(arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_UpdateOpen(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_UpdateInProgress(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_due_at_utc_ms \in OptionU64Values : \E arg_not_before_utc_ms \in OptionU64Values : \E arg_snoozed_until_utc_ms \in OptionU64Values : \E arg_completion_policy \in WorkCompletionPolicyValues : \E arg_completion_supervisor_owner_key \in OptionWorkOwnerKeyValues : \E arg_completion_reviewer_quorum_threshold \in OptionU64Values : \E arg_unresolved_blocker_count \in 0..2 : workgraph_UpdateBlocked(arg_expected_revision, arg_due_at_utc_ms, arg_not_before_utc_ms, arg_snoozed_until_utc_ms, arg_completion_policy, arg_completion_supervisor_owner_key, arg_completion_reviewer_quorum_threshold, arg_unresolved_blocker_count))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_owner_key \in WorkOwnerKeyValues : \E arg_now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : workgraph_ClaimOpen(arg_expected_revision, arg_owner_key, arg_now_utc_ms, arg_lease_expires_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_owner_key \in WorkOwnerKeyValues : \E arg_now_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in OptionU64Values : workgraph_ClaimExpiredInProgress(arg_expected_revision, arg_owner_key, arg_now_utc_ms, arg_lease_expires_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_ReleaseInProgress(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_BlockOpen(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_BlockInProgress(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_BlockBlocked(arg_expected_revision))
    /\ WF_vars(\E arg_unresolved_blocker_count \in 0..2 : workgraph_RefreshEligibilityOpen(arg_unresolved_blocker_count))
    /\ WF_vars(\E arg_unresolved_blocker_count \in 0..2 : workgraph_RefreshEligibilityInProgress(arg_unresolved_blocker_count))
    /\ WF_vars(\E arg_unresolved_blocker_count \in 0..2 : workgraph_RefreshEligibilityBlocked(arg_unresolved_blocker_count))
    /\ WF_vars(\E arg_kind \in WorkEdgeKindValues : \E arg_from_item_key \in WorkItemKeyValues : \E arg_to_item_key \in WorkItemKeyValues : \E arg_edge_key \in WorkEdgeKeyValues : \E arg_reverse_path_key \in WorkDependencyPathKeyValues : workgraph_ValidateLink(arg_kind, arg_from_item_key, arg_to_item_key, arg_edge_key, arg_reverse_path_key))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseOpenCompleted(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseInProgressCompleted(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseBlockedCompleted(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseOpenCancelled(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseInProgressCancelled(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseBlockedCancelled(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseOpenFailed(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseInProgressFailed(arg_expected_revision, arg_at_utc_ms))

WitnessFairness_close_stops_attention_route_2 ==
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : \E arg_at_utc_ms \in 0..2 : workgraph_CloseBlockedFailed(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceOpen(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceInProgress(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceBlocked(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceCompleted(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceCancelled(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {workgraph_revision} : workgraph_AddEvidenceFailed(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {attention_revision} : \E arg_until_utc_ms \in OptionU64Values : attention_PauseActive(arg_expected_revision, arg_until_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {attention_revision} : \E arg_until_utc_ms \in OptionU64Values : attention_PausePaused(arg_expected_revision, arg_until_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {attention_revision} : attention_ResumePaused(arg_expected_revision))
    /\ WF_vars(\E arg_expected_revision \in {attention_revision} : \E arg_superseded_by_binding_key \in WorkAttentionBindingKeyValues : \E arg_at_utc_ms \in 0..2 : attention_SupersedeActive(arg_expected_revision, arg_superseded_by_binding_key, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {attention_revision} : \E arg_superseded_by_binding_key \in WorkAttentionBindingKeyValues : \E arg_at_utc_ms \in 0..2 : attention_SupersedePaused(arg_expected_revision, arg_superseded_by_binding_key, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {attention_revision} : \E arg_at_utc_ms \in 0..2 : attention_StopActive(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_expected_revision \in {attention_revision} : \E arg_at_utc_ms \in 0..2 : attention_StopPaused(arg_expected_revision, arg_at_utc_ms))
    /\ WF_vars(WitnessInjectNext_close_stops_attention_route)

WitnessSpec_close_stops_attention_route ==
    /\ WitnessInit_close_stops_attention_route
    /\ [] [WitnessNext_close_stops_attention_route]_vars
    /\ WitnessFairness_close_stops_attention_route_1
    /\ WitnessFairness_close_stops_attention_route_2

WitnessRouteObserved_close_stops_attention_route_work_item_close_stops_attention == <> RouteObserved_work_item_close_stops_attention
WitnessTransitionObserved_close_stops_attention_route_workgraph_CreateOpen == <> (\E packet \in observed_transitions : /\ packet.machine = "workgraph" /\ packet.transition = "CreateOpen")
WitnessTransitionObserved_close_stops_attention_route_workgraph_CloseOpenCompleted == <> (\E packet \in observed_transitions : /\ packet.machine = "workgraph" /\ packet.transition = "CloseOpenCompleted")
WitnessTransitionObserved_close_stops_attention_route_attention_StopActive == <> (\E packet \in observed_transitions : /\ packet.machine = "attention" /\ packet.transition = "StopActive")
WitnessTransitionOrder_close_stops_attention_route_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "workgraph" /\ earlier.transition = "CloseOpenCompleted" /\ later.machine = "attention" /\ later.transition = "StopActive" /\ earlier.step < later.step)

THEOREM Spec => []closed_work_item_routes_to_attention_stop
THEOREM Spec => []attention_stop_originates_from_work_item_close
THEOREM Spec => []workgraph_absent_has_zero_revision
THEOREM Spec => []workgraph_live_has_positive_revision
THEOREM Spec => []workgraph_topology_snapshot_is_stateless
THEOREM Spec => []workgraph_terminal_has_terminal_time
THEOREM Spec => []workgraph_claim_only_in_progress
THEOREM Spec => []workgraph_blocked_has_no_claim
THEOREM Spec => []workgraph_terminal_has_no_claim
THEOREM Spec => []workgraph_supervisor_policy_has_owner
THEOREM Spec => []workgraph_non_supervisor_policy_has_no_owner
THEOREM Spec => []workgraph_reviewer_quorum_policy_has_positive_threshold
THEOREM Spec => []workgraph_non_reviewer_quorum_policy_has_no_threshold
THEOREM Spec => []attention_live_has_no_terminal_time
THEOREM Spec => []attention_paused_has_pause_state
THEOREM Spec => []attention_superseded_records_successor

=============================================================================
