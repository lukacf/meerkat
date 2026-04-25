---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for auth_lease_bundle.

CONSTANTS NatValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))
AppendIfMissing(seq, value) == IF value \in SeqElements(seq) THEN seq ELSE Append(seq, value)
Machines == {
    <<"auth_machine", "AuthMachine", "auth_machine_authority">>
}

RouteNames == {
}

Actors == {
    "auth_machine_authority"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "auth_machine" -> "auth_machine_authority"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    "unknown_machine"

RouteEffect(route_name) ==
    "unknown_effect"

RouteTargetMachine(route_name) ==
    "unknown_machine"

RouteTargetInput(route_name) ==
    "unknown_input"

RouteTargetKind(route_name) ==
    "Unknown"

RouteDeliveryKind(route_name) ==
    "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, obligation_auth_lease_lifecycle_publication, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, obligation_auth_lease_lifecycle_publication, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ auth_machine_phase = "Valid"
    /\ auth_machine_expires_at = None
    /\ auth_machine_last_refresh = None
    /\ auth_machine_refresh_attempt = 0
    /\ obligation_auth_lease_lifecycle_publication = {}
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

WitnessInit_auth_lease_lifecycle_publication_round_trip ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

auth_machine_Acquire(arg_expires_at_ts) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "Acquire"
       /\ packet.payload.expires_at_ts = arg_expires_at_ts
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_expires_at' = packet.payload.expires_at_ts
       /\ auth_machine_refresh_attempt' = 0
       /\ UNCHANGED << auth_machine_last_refresh, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "Acquire"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "Acquire", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkExpiring"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MarkExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginRefreshFromValid ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRefresh"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromValid"], [machine |-> "auth_machine", variant |-> "WakeRefreshLoop", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginRefreshFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginRefreshFromExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRefresh"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpiring"], [machine |-> "auth_machine", variant |-> "WakeRefreshLoop", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginRefreshFromExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_CompleteRefresh(arg_new_expires_at, arg_now_ts) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "CompleteRefresh"
       /\ packet.payload.new_expires_at = arg_new_expires_at
       /\ packet.payload.now_ts = arg_now_ts
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_expires_at' = packet.payload.new_expires_at
       /\ auth_machine_last_refresh' = Some(packet.payload.now_ts)
       /\ auth_machine_refresh_attempt' = 0
       /\ UNCHANGED << witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteRefresh"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "CompleteRefresh", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RefreshFailedTransient ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RefreshFailedTransient"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_refresh_attempt' = (auth_machine_refresh_attempt + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "RefreshFailedTransient"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RefreshFailedTransient", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RefreshFailedPermanent ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RefreshFailedPermanent"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_refresh_attempt' = (auth_machine_refresh_attempt + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "RefreshFailedPermanent"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RefreshFailedPermanent", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromValid ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromRefreshing ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_Release ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "Release"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "Release"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "Release", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, obligation_auth_lease_lifecycle_publication, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_auth_lease_lifecycle_publication_round_trip ==
    FALSE

CoreNext ==
    \/ \E arg_expires_at_ts \in OptionU64Values : auth_machine_Acquire(arg_expires_at_ts)
    \/ auth_machine_MarkExpiring
    \/ auth_machine_BeginRefreshFromValid
    \/ auth_machine_BeginRefreshFromExpiring
    \/ \E arg_new_expires_at \in OptionU64Values : \E arg_now_ts \in 0..2 : auth_machine_CompleteRefresh(arg_new_expires_at, arg_now_ts)
    \/ auth_machine_RefreshFailedTransient
    \/ auth_machine_RefreshFailedPermanent
    \/ auth_machine_MarkReauthRequiredFromValid
    \/ auth_machine_MarkReauthRequiredFromExpiring
    \/ auth_machine_MarkReauthRequiredFromRefreshing
    \/ auth_machine_Release
    \/ QuiescentStutter

InjectNext ==
    FALSE

Next ==
    \/ CoreNext

WitnessNext_auth_lease_lifecycle_publication_round_trip ==
    \/ CoreNext
    \/ WitnessInjectNext_auth_lease_lifecycle_publication_round_trip


auth_lease_lifecycle_publication_protocol_covered == TRUE

NoOpenObligationsOnTerminal_auth_lease_lifecycle_publication == (auth_machine_phase = "Released") => obligation_auth_lease_lifecycle_publication = {}

CoverageInstrumentation == TRUE

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6
WitnessStateConstraint_auth_lease_lifecycle_publication_round_trip == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8

Spec == Init /\ [][Next]_vars
WitnessSpec_auth_lease_lifecycle_publication_round_trip == WitnessInit_auth_lease_lifecycle_publication_round_trip /\ [] [WitnessNext_auth_lease_lifecycle_publication_round_trip]_vars /\ WF_vars(\E arg_expires_at_ts \in OptionU64Values : auth_machine_Acquire(arg_expires_at_ts)) /\ WF_vars(auth_machine_MarkExpiring) /\ WF_vars(auth_machine_BeginRefreshFromValid) /\ WF_vars(auth_machine_BeginRefreshFromExpiring) /\ WF_vars(\E arg_new_expires_at \in OptionU64Values : \E arg_now_ts \in 0..2 : auth_machine_CompleteRefresh(arg_new_expires_at, arg_now_ts)) /\ WF_vars(auth_machine_RefreshFailedTransient) /\ WF_vars(auth_machine_RefreshFailedPermanent) /\ WF_vars(auth_machine_MarkReauthRequiredFromValid) /\ WF_vars(auth_machine_MarkReauthRequiredFromExpiring) /\ WF_vars(auth_machine_MarkReauthRequiredFromRefreshing) /\ WF_vars(auth_machine_Release)


THEOREM Spec => []auth_lease_lifecycle_publication_protocol_covered
THEOREM Spec => []NoOpenObligationsOnTerminal_auth_lease_lifecycle_publication

=============================================================================
