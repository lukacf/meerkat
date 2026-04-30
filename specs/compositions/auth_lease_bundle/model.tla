---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for auth_lease_bundle.

CONSTANTS NatValues, SetOfStringValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

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

VARIABLES auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, obligation_auth_lease_lifecycle_publication, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, obligation_auth_lease_lifecycle_publication, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ auth_machine_phase = "Valid"
    /\ auth_machine_expires_at = None
    /\ auth_machine_last_refresh = None
    /\ auth_machine_refresh_attempt = 0
    /\ auth_machine_oauth_browser_flow_ids = {}
    /\ auth_machine_oauth_device_flow_ids = {}
    /\ auth_machine_oauth_device_poll_ids = {}
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
       /\ UNCHANGED << auth_machine_last_refresh, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ auth_machine_oauth_browser_flow_ids' = {}
       /\ auth_machine_oauth_device_flow_ids' = {}
       /\ auth_machine_oauth_device_poll_ids' = {}
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "Release"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "Release", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_poll_ids)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_poll_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_poll_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_poll_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> auth_machine_phase]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [new_state |-> auth_machine_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
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
       /\ UNCHANGED << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_poll_ids, obligation_auth_lease_lifecycle_publication, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

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
    \/ \E arg_flow_id \in StringValues : auth_machine_AdmitOAuthBrowserFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_AdmitOAuthBrowserFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_AdmitOAuthBrowserFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_AdmitOAuthBrowserFlowReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_VerifyOAuthBrowserFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_VerifyOAuthBrowserFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_VerifyOAuthBrowserFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_VerifyOAuthBrowserFlowReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthBrowserFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthBrowserFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthBrowserFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthBrowserFlowReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_AdmitOAuthDeviceFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_AdmitOAuthDeviceFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_AdmitOAuthDeviceFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_AdmitOAuthDeviceFlowReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_VerifyOAuthDeviceFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_VerifyOAuthDeviceFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_VerifyOAuthDeviceFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_VerifyOAuthDeviceFlowReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_BeginOAuthDevicePollValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_BeginOAuthDevicePollExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_BeginOAuthDevicePollRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_BeginOAuthDevicePollReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthDeviceFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthDeviceFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthDeviceFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthDeviceFlowReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowReauthRequired(arg_flow_id)
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

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 0 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 0 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 0
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 2 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 2 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 2
WitnessStateConstraint_auth_lease_lifecycle_publication_round_trip == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 0 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 0 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 0

Spec == Init /\ [][Next]_vars
WitnessSpec_auth_lease_lifecycle_publication_round_trip == WitnessInit_auth_lease_lifecycle_publication_round_trip /\ [] [WitnessNext_auth_lease_lifecycle_publication_round_trip]_vars /\ WF_vars(\E arg_expires_at_ts \in OptionU64Values : auth_machine_Acquire(arg_expires_at_ts)) /\ WF_vars(auth_machine_MarkExpiring) /\ WF_vars(auth_machine_BeginRefreshFromValid) /\ WF_vars(auth_machine_BeginRefreshFromExpiring) /\ WF_vars(\E arg_new_expires_at \in OptionU64Values : \E arg_now_ts \in 0..2 : auth_machine_CompleteRefresh(arg_new_expires_at, arg_now_ts)) /\ WF_vars(auth_machine_RefreshFailedTransient) /\ WF_vars(auth_machine_RefreshFailedPermanent) /\ WF_vars(auth_machine_MarkReauthRequiredFromValid) /\ WF_vars(auth_machine_MarkReauthRequiredFromExpiring) /\ WF_vars(auth_machine_MarkReauthRequiredFromRefreshing) /\ WF_vars(auth_machine_Release) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_AdmitOAuthBrowserFlowValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_AdmitOAuthBrowserFlowExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_AdmitOAuthBrowserFlowRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_AdmitOAuthBrowserFlowReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_VerifyOAuthBrowserFlowValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_VerifyOAuthBrowserFlowExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_VerifyOAuthBrowserFlowRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_VerifyOAuthBrowserFlowReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthBrowserFlowValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthBrowserFlowExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthBrowserFlowRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthBrowserFlowReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_AdmitOAuthDeviceFlowValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_AdmitOAuthDeviceFlowExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_AdmitOAuthDeviceFlowRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_AdmitOAuthDeviceFlowReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_VerifyOAuthDeviceFlowValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_VerifyOAuthDeviceFlowExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_VerifyOAuthDeviceFlowRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_VerifyOAuthDeviceFlowReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_BeginOAuthDevicePollValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_BeginOAuthDevicePollExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_BeginOAuthDevicePollRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_BeginOAuthDevicePollReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthDeviceFlowValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthDeviceFlowExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthDeviceFlowRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ConsumeOAuthDeviceFlowReauthRequired(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowValid(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowExpiring(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowRefreshing(arg_flow_id)) /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowReauthRequired(arg_flow_id))


THEOREM Spec => []auth_lease_lifecycle_publication_protocol_covered
THEOREM Spec => []NoOpenObligationsOnTerminal_auth_lease_lifecycle_publication

=============================================================================
