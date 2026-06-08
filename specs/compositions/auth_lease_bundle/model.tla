---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for auth_lease_bundle.

CONSTANTS AuthLifecyclePhaseValues, BooleanValues, CredentialUseDispositionValues, CredentialUseIntentValues, NatValues, SetOfStringValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in StringValues }
MapStringU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in NatValues }
OptionAuthLifecyclePhaseValues == {None} \cup {Some(x) : x \in AuthLifecyclePhaseValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

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

VARIABLES auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, obligation_auth_lease_lifecycle_publication, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, obligation_auth_lease_lifecycle_publication, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ auth_machine_phase = "Valid"
    /\ auth_machine_expires_at = None
    /\ auth_machine_last_refresh = None
    /\ auth_machine_refresh_attempt = 0
    /\ auth_machine_credential_present = FALSE
    /\ auth_machine_credential_generation = 0
    /\ auth_machine_credential_published_at_millis = None
    /\ auth_machine_oauth_browser_flow_ids = {}
    /\ auth_machine_oauth_browser_flow_providers = [x \in {} |-> None]
    /\ auth_machine_oauth_browser_flow_redirect_uris = [x \in {} |-> None]
    /\ auth_machine_oauth_browser_flow_expires_at_millis = [x \in {} |-> None]
    /\ auth_machine_oauth_device_flow_ids = {}
    /\ auth_machine_oauth_device_flow_providers = [x \in {} |-> None]
    /\ auth_machine_oauth_device_flow_expires_at_millis = [x \in {} |-> None]
    /\ auth_machine_oauth_device_poll_ids = {}
    /\ auth_machine_oauth_outstanding_flow_count = 0
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

auth_machine_Acquire(arg_expires_at_ts, arg_credential_published_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "Acquire"
       /\ packet.payload.expires_at_ts = arg_expires_at_ts
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_expires_at' = packet.payload.expires_at_ts
       /\ auth_machine_refresh_attempt' = 0
       /\ auth_machine_credential_present' = TRUE
       /\ auth_machine_credential_generation' = (auth_machine_credential_generation + 1)
       /\ auth_machine_credential_published_at_millis' = Some(packet.payload.credential_published_at_millis)
       /\ UNCHANGED << auth_machine_last_refresh, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (auth_machine_credential_generation + 1), credential_published_at_millis |-> Some(packet.payload.credential_published_at_millis), expires_at |-> packet.payload.expires_at_ts, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "Acquire"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "Acquire", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> packet.payload.expires_at_ts, credential_generation |-> (auth_machine_credential_generation + 1), credential_published_at_millis |-> Some(packet.payload.credential_published_at_millis)]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkExpiring"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessValid(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (IF (auth_machine_expires_at = None) THEN TRUE ELSE ((packet.payload.now_ts + packet.payload.refresh_window_secs) <= (IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None)))
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessExpiringFromValid(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (IF (auth_machine_expires_at = None) THEN FALSE ELSE ((packet.payload.now_ts < (IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None)) /\ ((IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None) < (packet.payload.now_ts + packet.payload.refresh_window_secs))))
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpiringFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpiringFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessExpiredFromValid(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (IF (auth_machine_expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None) <= packet.payload.now_ts))
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpiredFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpiredFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessExpiring(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (IF (auth_machine_expires_at = None) THEN TRUE ELSE (packet.payload.now_ts < (IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None)))
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessExpiredFromExpiring(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (IF (auth_machine_expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None) <= packet.payload.now_ts))
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpiredFromExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpiredFromExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessExpired(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessRefreshing(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessReauthRequired(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ObserveCredentialFreshnessReleased(arg_now_ts, arg_refresh_window_secs) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ObserveCredentialFreshness"
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.refresh_window_secs = arg_refresh_window_secs
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Released", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginRefreshFromValid ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRefresh"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromValid"], [machine |-> "auth_machine", variant |-> "WakeRefreshLoop", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginRefreshFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginRefreshFromExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRefresh"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpiring"], [machine |-> "auth_machine", variant |-> "WakeRefreshLoop", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginRefreshFromExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginRefreshFromExpired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRefresh"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpired"], [machine |-> "auth_machine", variant |-> "WakeRefreshLoop", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginRefreshFromExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_CompleteRefresh(arg_new_expires_at, arg_now_ts, arg_credential_published_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "CompleteRefresh"
       /\ packet.payload.new_expires_at = arg_new_expires_at
       /\ packet.payload.now_ts = arg_now_ts
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (IF (packet.payload.new_expires_at = None) THEN TRUE ELSE (packet.payload.now_ts < (IF "value" \in DOMAIN packet.payload.new_expires_at THEN packet.payload.new_expires_at["value"] ELSE None)))
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_expires_at' = packet.payload.new_expires_at
       /\ auth_machine_last_refresh' = Some(packet.payload.now_ts)
       /\ auth_machine_refresh_attempt' = 0
       /\ auth_machine_credential_present' = TRUE
       /\ auth_machine_credential_generation' = (auth_machine_credential_generation + 1)
       /\ auth_machine_credential_published_at_millis' = Some(packet.payload.credential_published_at_millis)
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (auth_machine_credential_generation + 1), credential_published_at_millis |-> Some(packet.payload.credential_published_at_millis), expires_at |-> packet.payload.new_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteRefresh"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "CompleteRefresh", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> packet.payload.new_expires_at, credential_generation |-> (auth_machine_credential_generation + 1), credential_published_at_millis |-> Some(packet.payload.credential_published_at_millis)]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RefreshFailedTransient(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RefreshFailed"
       /\ packet.payload.http_status = arg_http_status
       /\ packet.payload.oauth_error_code = arg_oauth_error_code
       /\ packet.payload.local_credential_unusable = arg_local_credential_unusable
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.local_credential_unusable = FALSE) /\ (packet.payload.http_status # Some(401)) /\ (packet.payload.http_status # Some(403)) /\ (packet.payload.oauth_error_code # Some("invalid_grant")) /\ (packet.payload.oauth_error_code # Some("invalid_client")) /\ (packet.payload.oauth_error_code # Some("unauthorized_client")) /\ (packet.payload.oauth_error_code # Some("invalid_scope")) /\ (packet.payload.oauth_error_code # Some("access_denied")) /\ (packet.payload.oauth_error_code # Some("permission_denied")) /\ (packet.payload.oauth_error_code # Some("expired_token")))
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_refresh_attempt' = (auth_machine_refresh_attempt + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RefreshFailedTransient"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RefreshFailedTransient", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RefreshFailedPermanent(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RefreshFailed"
       /\ packet.payload.http_status = arg_http_status
       /\ packet.payload.oauth_error_code = arg_oauth_error_code
       /\ packet.payload.local_credential_unusable = arg_local_credential_unusable
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (IF (packet.payload.local_credential_unusable = TRUE) THEN TRUE ELSE (IF (packet.payload.http_status = Some(401)) THEN TRUE ELSE (IF (packet.payload.http_status = Some(403)) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_grant")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_client")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("unauthorized_client")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_scope")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("access_denied")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("permission_denied")) THEN TRUE ELSE (packet.payload.oauth_error_code = Some("expired_token")))))))))))
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_refresh_attempt' = (auth_machine_refresh_attempt + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RefreshFailedPermanent"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RefreshFailedPermanent", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromValid ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromExpired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromRefreshing ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ClearCredentialLifecycle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ClearCredentialLifecycle"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_expires_at' = None
       /\ auth_machine_last_refresh' = None
       /\ auth_machine_refresh_attempt' = 0
       /\ auth_machine_credential_present' = FALSE
       /\ auth_machine_credential_published_at_millis' = None
       /\ UNCHANGED << auth_machine_credential_generation, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ClearCredentialLifecycle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ClearCredentialLifecycle", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> None, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ReleaseCredentialLifecycleWithOAuth ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ReleaseCredentialLifecycle"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ (auth_machine_oauth_outstanding_flow_count > 0)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_expires_at' = None
       /\ auth_machine_last_refresh' = None
       /\ auth_machine_refresh_attempt' = 0
       /\ auth_machine_credential_present' = FALSE
       /\ auth_machine_credential_published_at_millis' = None
       /\ UNCHANGED << auth_machine_credential_generation, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ReleaseCredentialLifecycleWithOAuth"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ReleaseCredentialLifecycleWithOAuth", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> None, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ReleaseCredentialLifecycleWithoutOAuth ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ReleaseCredentialLifecycle"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ (auth_machine_oauth_outstanding_flow_count = 0)
       /\ auth_machine_phase' = "Released"
       /\ auth_machine_expires_at' = None
       /\ auth_machine_last_refresh' = None
       /\ auth_machine_refresh_attempt' = 0
       /\ auth_machine_credential_present' = FALSE
       /\ auth_machine_credential_published_at_millis' = None
       /\ auth_machine_oauth_browser_flow_ids' = {}
       /\ auth_machine_oauth_browser_flow_providers' = [x \in {} |-> None]
       /\ auth_machine_oauth_browser_flow_redirect_uris' = [x \in {} |-> None]
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_flow_ids' = {}
       /\ auth_machine_oauth_device_flow_providers' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_flow_expires_at_millis' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_poll_ids' = {}
       /\ auth_machine_oauth_outstanding_flow_count' = 0
       /\ UNCHANGED << auth_machine_credential_generation, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "ReleaseCredentialLifecycleWithoutOAuth"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ReleaseCredentialLifecycleWithoutOAuth", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Released", expires_at |-> None, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ model_step_count' = model_step_count + 1


auth_machine_Release ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "Release"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ auth_machine_expires_at' = None
       /\ auth_machine_last_refresh' = None
       /\ auth_machine_refresh_attempt' = 0
       /\ auth_machine_credential_present' = FALSE
       /\ auth_machine_credential_published_at_millis' = None
       /\ auth_machine_oauth_browser_flow_ids' = {}
       /\ auth_machine_oauth_browser_flow_providers' = [x \in {} |-> None]
       /\ auth_machine_oauth_browser_flow_redirect_uris' = [x \in {} |-> None]
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_flow_ids' = {}
       /\ auth_machine_oauth_device_flow_providers' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_flow_expires_at_millis' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_poll_ids' = {}
       /\ auth_machine_oauth_outstanding_flow_count' = 0
       /\ UNCHANGED << auth_machine_credential_generation, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "Release"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "Release", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Released", expires_at |-> None, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreCredentialLifecycleSnapshotValid(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreCredentialLifecycleSnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ packet.payload.restored_oauth_membership_observed = arg_restored_oauth_membership_observed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = Some("Valid")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreCredentialLifecycleSnapshotExpiring(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreCredentialLifecycleSnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ packet.payload.restored_oauth_membership_observed = arg_restored_oauth_membership_observed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = Some("Expiring")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreCredentialLifecycleSnapshotRefreshing(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreCredentialLifecycleSnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ packet.payload.restored_oauth_membership_observed = arg_restored_oauth_membership_observed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = Some("Refreshing")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreCredentialLifecycleSnapshotExpired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreCredentialLifecycleSnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ packet.payload.restored_oauth_membership_observed = arg_restored_oauth_membership_observed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = Some("Expired")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreCredentialLifecycleSnapshotReauthRequired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreCredentialLifecycleSnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ packet.payload.restored_oauth_membership_observed = arg_restored_oauth_membership_observed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = Some("ReauthRequired")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreCredentialLifecycleSnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ packet.payload.restored_oauth_membership_observed = arg_restored_oauth_membership_observed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ (IF (packet.payload.credential_present = FALSE) THEN TRUE ELSE (IF (packet.payload.lifecycle_phase = None) THEN TRUE ELSE (packet.payload.lifecycle_phase = Some("Released"))))
       /\ (IF (auth_machine_oauth_outstanding_flow_count > 0) THEN TRUE ELSE packet.payload.restored_oauth_membership_observed)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_expires_at' = None
       /\ auth_machine_last_refresh' = None
       /\ auth_machine_refresh_attempt' = 0
       /\ auth_machine_credential_present' = FALSE
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = None
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> None, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreCredentialLifecycleSnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ packet.payload.restored_oauth_membership_observed = arg_restored_oauth_membership_observed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ (IF (packet.payload.credential_present = FALSE) THEN TRUE ELSE (IF (packet.payload.lifecycle_phase = None) THEN TRUE ELSE (packet.payload.lifecycle_phase = Some("Released"))))
       /\ ((auth_machine_oauth_outstanding_flow_count = 0) /\ (packet.payload.restored_oauth_membership_observed = FALSE))
       /\ auth_machine_phase' = "Released"
       /\ auth_machine_expires_at' = None
       /\ auth_machine_last_refresh' = None
       /\ auth_machine_refresh_attempt' = 0
       /\ auth_machine_credential_present' = FALSE
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = None
       /\ auth_machine_oauth_browser_flow_ids' = {}
       /\ auth_machine_oauth_browser_flow_providers' = [x \in {} |-> None]
       /\ auth_machine_oauth_browser_flow_redirect_uris' = [x \in {} |-> None]
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_flow_ids' = {}
       /\ auth_machine_oauth_device_flow_providers' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_flow_expires_at_millis' = [x \in {} |-> None]
       /\ auth_machine_oauth_device_poll_ids' = {}
       /\ auth_machine_oauth_outstanding_flow_count' = 0
       /\ UNCHANGED << witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Released", expires_at |-> None, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreAuthoritySnapshotValid(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreAuthoritySnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = "Valid") /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreAuthoritySnapshotExpiring(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreAuthoritySnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = "Expiring") /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreAuthoritySnapshotRefreshing(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreAuthoritySnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = "Refreshing") /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreAuthoritySnapshotExpired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreAuthoritySnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = "Expired") /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None))
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreAuthoritySnapshotReauthRequired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreAuthoritySnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = "ReauthRequired") /\ (IF (packet.payload.credential_present = FALSE) THEN TRUE ELSE (packet.payload.credential_published_at_millis # None)))
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreAuthoritySnapshotReleased(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreAuthoritySnapshot"
       /\ packet.payload.lifecycle_phase = arg_lifecycle_phase
       /\ packet.payload.expires_at = arg_expires_at
       /\ packet.payload.last_refresh = arg_last_refresh
       /\ packet.payload.refresh_attempt = arg_refresh_attempt
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.credential_generation = arg_credential_generation
       /\ packet.payload.credential_published_at_millis = arg_credential_published_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released"
       /\ ((packet.payload.lifecycle_phase = "Released") /\ (packet.payload.credential_present = FALSE) /\ (packet.payload.credential_published_at_millis = None) /\ (auth_machine_oauth_outstanding_flow_count = 0))
       /\ auth_machine_phase' = "Released"
       /\ auth_machine_expires_at' = packet.payload.expires_at
       /\ auth_machine_last_refresh' = packet.payload.last_refresh
       /\ auth_machine_refresh_attempt' = packet.payload.refresh_attempt
       /\ auth_machine_credential_present' = packet.payload.credential_present
       /\ auth_machine_credential_generation' = IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation
       /\ auth_machine_credential_published_at_millis' = packet.payload.credential_published_at_millis
       /\ UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Released", expires_at |-> packet.payload.expires_at, credential_generation |-> IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation, credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_expires_at_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollExpired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ReopenReleasedForOAuthBrowserFlowAdmission(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ ((auth_machine_credential_present = FALSE) /\ (auth_machine_credential_published_at_millis = None))
       /\ (auth_machine_oauth_outstanding_flow_count = 0)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ReopenReleasedForOAuthBrowserFlowAdmission"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ReopenReleasedForOAuthBrowserFlowAdmission", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.redirect_uri = arg_redirect_uri
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowExpired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ auth_machine_oauth_browser_flow_providers' = MapRemove(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapRemove(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapRemove(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_AdmitOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ReopenReleasedForOAuthDeviceFlowAdmission(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "AdmitOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.expires_at_millis = arg_expires_at_millis
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ ((auth_machine_credential_present = FALSE) /\ (auth_machine_credential_published_at_millis = None))
       /\ (auth_machine_oauth_outstanding_flow_count = 0)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ReopenReleasedForOAuthDeviceFlowAdmission"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ReopenReleasedForOAuthDeviceFlowAdmission", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConfirmOAuthDurableAdmissionValid(arg_observed_global_outstanding_flows, arg_max_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConfirmOAuthDurableAdmission"
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ConfirmOAuthDurableAdmissionExpiring(arg_observed_global_outstanding_flows, arg_max_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConfirmOAuthDurableAdmission"
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ConfirmOAuthDurableAdmissionExpired(arg_observed_global_outstanding_flows, arg_max_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConfirmOAuthDurableAdmission"
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ConfirmOAuthDurableAdmissionRefreshing(arg_observed_global_outstanding_flows, arg_max_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConfirmOAuthDurableAdmission"
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ConfirmOAuthDurableAdmissionReauthRequired(arg_observed_global_outstanding_flows, arg_max_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConfirmOAuthDurableAdmission"
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_VerifyOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "VerifyOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollValid(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollExpiring(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollExpired(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollRefreshing(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginOAuthDevicePollReauthRequired(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollExpired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_poll_ids)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ConsumeOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_now_millis) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConsumeOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ packet.payload.provider = arg_provider
       /\ packet.payload.now_millis = arg_now_millis
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ ((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))
       /\ (packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowExpired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
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
       /\ auth_machine_oauth_device_flow_providers' = MapRemove(auth_machine_oauth_device_flow_providers, packet.payload.flow_id)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapRemove(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count - 1)
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_lease_lifecycle_publication' = obligation_auth_lease_lifecycle_publication \cup {[new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionValidUseAuthorizedValid(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.intent = "UseCredential") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionValidUseAuthorizedValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionValidUseAuthorizedValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionValidHoldAuthorizedValid(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.intent = "HoldAuthority") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionValidHoldAuthorizedValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionValidHoldAuthorizedValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionValidBeginRefreshValid(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.intent = "BeginRefresh") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionValidBeginRefreshValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionValidBeginRefreshValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionValidNoCredentialValid(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (auth_machine_credential_present = FALSE)
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionValidNoCredentialValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionValidNoCredentialValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionExpiringUseRefreshExpiring(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.intent = "UseCredential") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiringUseRefreshExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiringUseRefreshExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.intent = "HoldAuthority") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.intent = "BeginRefresh") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionExpiringNoCredentialExpiring(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (auth_machine_credential_present = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiringNoCredentialExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiringNoCredentialExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionExpiredUseRefreshExpired(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ ((packet.payload.intent = "UseCredential") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiredUseRefreshExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiredUseRefreshExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionExpiredHoldRefreshExpired(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ ((packet.payload.intent = "HoldAuthority") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiredHoldRefreshExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiredHoldRefreshExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionExpiredBeginRefreshExpired(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ ((packet.payload.intent = "BeginRefresh") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiredBeginRefreshExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiredBeginRefreshExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionExpiredNoCredentialExpired(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (auth_machine_credential_present = FALSE)
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiredNoCredentialExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiredNoCredentialExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.intent = "UseCredential") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.intent = "HoldAuthority") /\ auth_machine_credential_present)
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.intent = "BeginRefresh")
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "AlreadyRefreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((auth_machine_credential_present = FALSE) /\ (IF (packet.payload.intent = "UseCredential") THEN TRUE ELSE (packet.payload.intent = "HoldAuthority")))
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionReauthRequiredReauthRequired(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionReauthRequiredReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionReauthRequiredReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionReleasedReleased(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionReleasedReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionReleasedReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionUseCachedValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (auth_machine_credential_present /\ packet.payload.credential_present /\ (packet.payload.force_refresh = FALSE))
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionUseCachedValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionUseCachedValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshValidValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (~((auth_machine_credential_present /\ packet.payload.credential_present /\ (packet.payload.force_refresh = FALSE))) /\ packet.payload.refresh_allowed)
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshValidValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshValidValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (~((auth_machine_credential_present /\ packet.payload.credential_present /\ (packet.payload.force_refresh = FALSE))) /\ (packet.payload.refresh_allowed = FALSE))
       /\ auth_machine_phase' = "Valid"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ packet.payload.refresh_allowed
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ packet.payload.refresh_allowed
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ packet.payload.refresh_allowed
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ packet.payload.refresh_allowed
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ packet.payload.refresh_allowed
       /\ auth_machine_phase' = "Released"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (packet.payload.refresh_allowed = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (packet.payload.refresh_allowed = FALSE)
       /\ auth_machine_phase' = "Expired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.refresh_allowed = FALSE)
       /\ auth_machine_phase' = "Refreshing"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (packet.payload.refresh_allowed = FALSE)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased(arg_credential_present, arg_force_refresh, arg_refresh_allowed) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveOAuthLoginCredentialDisposition"
       /\ packet.payload.credential_present = arg_credential_present
       /\ packet.payload.force_refresh = arg_force_refresh
       /\ packet.payload.refresh_allowed = arg_refresh_allowed
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ (packet.payload.refresh_allowed = FALSE)
       /\ auth_machine_phase' = "Released"
       /\ UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UNCHANGED << obligation_auth_lease_lifecycle_publication >>
       /\ model_step_count' = model_step_count + 1


auth_machine_oauth_flow_membership_consistent == ((DOMAIN auth_machine_oauth_browser_flow_providers = auth_machine_oauth_browser_flow_ids) /\ (DOMAIN auth_machine_oauth_browser_flow_redirect_uris = auth_machine_oauth_browser_flow_ids) /\ (DOMAIN auth_machine_oauth_browser_flow_expires_at_millis = auth_machine_oauth_browser_flow_ids) /\ (DOMAIN auth_machine_oauth_device_flow_providers = auth_machine_oauth_device_flow_ids) /\ (DOMAIN auth_machine_oauth_device_flow_expires_at_millis = auth_machine_oauth_device_flow_ids) /\ (\A flow_id \in auth_machine_oauth_device_poll_ids : (flow_id \in auth_machine_oauth_device_flow_ids)) /\ (auth_machine_oauth_outstanding_flow_count = (Cardinality(auth_machine_oauth_browser_flow_ids) + Cardinality(auth_machine_oauth_device_flow_ids))))

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, obligation_auth_lease_lifecycle_publication, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_auth_lease_lifecycle_publication_round_trip ==
    FALSE

CoreNext ==
    \/ \E arg_expires_at_ts \in OptionU64Values : \E arg_credential_published_at_millis \in 0..2 : auth_machine_Acquire(arg_expires_at_ts, arg_credential_published_at_millis)
    \/ auth_machine_MarkExpiring
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessValid(arg_now_ts, arg_refresh_window_secs)
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpiringFromValid(arg_now_ts, arg_refresh_window_secs)
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpiredFromValid(arg_now_ts, arg_refresh_window_secs)
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpiring(arg_now_ts, arg_refresh_window_secs)
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpiredFromExpiring(arg_now_ts, arg_refresh_window_secs)
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpired(arg_now_ts, arg_refresh_window_secs)
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessRefreshing(arg_now_ts, arg_refresh_window_secs)
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessReauthRequired(arg_now_ts, arg_refresh_window_secs)
    \/ \E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessReleased(arg_now_ts, arg_refresh_window_secs)
    \/ auth_machine_BeginRefreshFromValid
    \/ auth_machine_BeginRefreshFromExpiring
    \/ auth_machine_BeginRefreshFromExpired
    \/ \E arg_new_expires_at \in OptionU64Values : \E arg_now_ts \in 0..2 : \E arg_credential_published_at_millis \in 0..2 : auth_machine_CompleteRefresh(arg_new_expires_at, arg_now_ts, arg_credential_published_at_millis)
    \/ \E arg_http_status \in OptionU64Values : \E arg_oauth_error_code \in OptionStringValues : \E arg_local_credential_unusable \in BOOLEAN : auth_machine_RefreshFailedTransient(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable)
    \/ \E arg_http_status \in OptionU64Values : \E arg_oauth_error_code \in OptionStringValues : \E arg_local_credential_unusable \in BOOLEAN : auth_machine_RefreshFailedPermanent(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable)
    \/ auth_machine_MarkReauthRequiredFromValid
    \/ auth_machine_MarkReauthRequiredFromExpiring
    \/ auth_machine_MarkReauthRequiredFromExpired
    \/ auth_machine_MarkReauthRequiredFromRefreshing
    \/ auth_machine_ClearCredentialLifecycle
    \/ auth_machine_ReleaseCredentialLifecycleWithOAuth
    \/ auth_machine_ReleaseCredentialLifecycleWithoutOAuth
    \/ auth_machine_Release
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotValid(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotExpiring(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotRefreshing(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotExpired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotReauthRequired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotValid(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotExpiring(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotRefreshing(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotExpired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotReauthRequired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotReleased(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_expires_at_millis)
    \/ \E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollExpired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_ReopenReleasedForOAuthBrowserFlowAdmission(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowExpired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_ReopenReleasedForOAuthDeviceFlowAdmission(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows)
    \/ \E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionValid(arg_observed_global_outstanding_flows, arg_max_outstanding_flows)
    \/ \E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionExpiring(arg_observed_global_outstanding_flows, arg_max_outstanding_flows)
    \/ \E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionExpired(arg_observed_global_outstanding_flows, arg_max_outstanding_flows)
    \/ \E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionRefreshing(arg_observed_global_outstanding_flows, arg_max_outstanding_flows)
    \/ \E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionReauthRequired(arg_observed_global_outstanding_flows, arg_max_outstanding_flows)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollValid(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollExpiring(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollExpired(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollRefreshing(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollReauthRequired(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollExpired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_now_millis)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowExpired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowReauthRequired(arg_flow_id)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionValidUseAuthorizedValid(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionValidHoldAuthorizedValid(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionValidBeginRefreshValid(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionValidNoCredentialValid(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiringUseRefreshExpiring(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiringNoCredentialExpiring(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiredUseRefreshExpired(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiredHoldRefreshExpired(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiredBeginRefreshExpired(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiredNoCredentialExpired(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionReauthRequiredReauthRequired(arg_intent)
    \/ \E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionReleasedReleased(arg_intent)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionUseCachedValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshValidValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased(arg_credential_present, arg_force_refresh, arg_refresh_allowed)
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

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 0
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 2 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 2 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 2
WitnessStateConstraint_auth_lease_lifecycle_publication_round_trip == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 0

Spec ==
    /\ Init
    /\ [][Next]_vars

WitnessFairness_auth_lease_lifecycle_publication_round_trip_1 ==
    /\ WF_vars(\E arg_expires_at_ts \in OptionU64Values : \E arg_credential_published_at_millis \in 0..2 : auth_machine_Acquire(arg_expires_at_ts, arg_credential_published_at_millis))
    /\ WF_vars(auth_machine_MarkExpiring)
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessValid(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpiringFromValid(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpiredFromValid(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpiring(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpiredFromExpiring(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessExpired(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessRefreshing(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessReauthRequired(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(\E arg_now_ts \in 0..2 : \E arg_refresh_window_secs \in 0..2 : auth_machine_ObserveCredentialFreshnessReleased(arg_now_ts, arg_refresh_window_secs))
    /\ WF_vars(auth_machine_BeginRefreshFromValid)
    /\ WF_vars(auth_machine_BeginRefreshFromExpiring)
    /\ WF_vars(auth_machine_BeginRefreshFromExpired)
    /\ WF_vars(\E arg_new_expires_at \in OptionU64Values : \E arg_now_ts \in 0..2 : \E arg_credential_published_at_millis \in 0..2 : auth_machine_CompleteRefresh(arg_new_expires_at, arg_now_ts, arg_credential_published_at_millis))
    /\ WF_vars(\E arg_http_status \in OptionU64Values : \E arg_oauth_error_code \in OptionStringValues : \E arg_local_credential_unusable \in BOOLEAN : auth_machine_RefreshFailedTransient(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable))
    /\ WF_vars(\E arg_http_status \in OptionU64Values : \E arg_oauth_error_code \in OptionStringValues : \E arg_local_credential_unusable \in BOOLEAN : auth_machine_RefreshFailedPermanent(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable))
    /\ WF_vars(auth_machine_MarkReauthRequiredFromValid)
    /\ WF_vars(auth_machine_MarkReauthRequiredFromExpiring)
    /\ WF_vars(auth_machine_MarkReauthRequiredFromExpired)
    /\ WF_vars(auth_machine_MarkReauthRequiredFromRefreshing)
    /\ WF_vars(auth_machine_ClearCredentialLifecycle)
    /\ WF_vars(auth_machine_ReleaseCredentialLifecycleWithOAuth)
    /\ WF_vars(auth_machine_ReleaseCredentialLifecycleWithoutOAuth)

WitnessFairness_auth_lease_lifecycle_publication_round_trip_2 ==
    /\ WF_vars(auth_machine_Release)
    /\ WF_vars(\E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotValid(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed))
    /\ WF_vars(\E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotExpiring(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed))
    /\ WF_vars(\E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotRefreshing(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed))
    /\ WF_vars(\E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotExpired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed))
    /\ WF_vars(\E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotReauthRequired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed))
    /\ WF_vars(\E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed))
    /\ WF_vars(\E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed))
    /\ WF_vars(\E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotValid(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis))
    /\ WF_vars(\E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotExpiring(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis))
    /\ WF_vars(\E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotRefreshing(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis))
    /\ WF_vars(\E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotExpired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis))
    /\ WF_vars(\E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotReauthRequired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis))
    /\ WF_vars(\E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotReleased(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_redirect_uri \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_expires_at_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in OptionStringValues : \E arg_expires_at_millis \in OptionU64Values : auth_machine_RestoreOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_expires_at_millis))

WitnessFairness_auth_lease_lifecycle_publication_round_trip_3 ==
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollValid(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollExpiring(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollExpired(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollRefreshing(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_RestoreOAuthDevicePollReauthRequired(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_ReopenReleasedForOAuthBrowserFlowAdmission(arg_flow_id, arg_provider, arg_redirect_uri, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowValid(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowExpiring(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowExpired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowRefreshing(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_redirect_uri \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthBrowserFlowReauthRequired(arg_flow_id, arg_provider, arg_redirect_uri, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowValid(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowExpiring(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowExpired(arg_flow_id))

WitnessFairness_auth_lease_lifecycle_publication_round_trip_4 ==
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowRefreshing(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowReauthRequired(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_AdmitOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_expires_at_millis \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : \E arg_observed_global_outstanding_flows \in 0..2 : auth_machine_ReopenReleasedForOAuthDeviceFlowAdmission(arg_flow_id, arg_provider, arg_expires_at_millis, arg_max_outstanding_flows, arg_observed_global_outstanding_flows))
    /\ WF_vars(\E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionValid(arg_observed_global_outstanding_flows, arg_max_outstanding_flows))
    /\ WF_vars(\E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionExpiring(arg_observed_global_outstanding_flows, arg_max_outstanding_flows))
    /\ WF_vars(\E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionExpired(arg_observed_global_outstanding_flows, arg_max_outstanding_flows))
    /\ WF_vars(\E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionRefreshing(arg_observed_global_outstanding_flows, arg_max_outstanding_flows))
    /\ WF_vars(\E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionReauthRequired(arg_observed_global_outstanding_flows, arg_max_outstanding_flows))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_VerifyOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollValid(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollExpiring(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollExpired(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollRefreshing(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_BeginOAuthDevicePollReauthRequired(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollValid(arg_flow_id))

WitnessFairness_auth_lease_lifecycle_publication_round_trip_5 ==
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollExpiring(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollExpired(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollRefreshing(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollReauthRequired(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowValid(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowExpiring(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowExpired(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowRefreshing(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : \E arg_provider \in StringValues : \E arg_now_millis \in 0..2 : auth_machine_ConsumeOAuthDeviceFlowReauthRequired(arg_flow_id, arg_provider, arg_now_millis))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowValid(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowExpiring(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowExpired(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowRefreshing(arg_flow_id))
    /\ WF_vars(\E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowReauthRequired(arg_flow_id))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionValidUseAuthorizedValid(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionValidHoldAuthorizedValid(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionValidBeginRefreshValid(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionValidNoCredentialValid(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiringUseRefreshExpiring(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiringNoCredentialExpiring(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiredUseRefreshExpired(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiredHoldRefreshExpired(arg_intent))

WitnessFairness_auth_lease_lifecycle_publication_round_trip_6 ==
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiredBeginRefreshExpired(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionExpiredNoCredentialExpired(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionReauthRequiredReauthRequired(arg_intent))
    /\ WF_vars(\E arg_intent \in CredentialUseIntentValues : auth_machine_ResolveCredentialUseAdmissionReleasedReleased(arg_intent))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionUseCachedValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshValidValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired(arg_credential_present, arg_force_refresh, arg_refresh_allowed))
    /\ WF_vars(\E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased(arg_credential_present, arg_force_refresh, arg_refresh_allowed))

WitnessSpec_auth_lease_lifecycle_publication_round_trip ==
    /\ WitnessInit_auth_lease_lifecycle_publication_round_trip
    /\ [] [WitnessNext_auth_lease_lifecycle_publication_round_trip]_vars
    /\ WitnessFairness_auth_lease_lifecycle_publication_round_trip_1
    /\ WitnessFairness_auth_lease_lifecycle_publication_round_trip_2
    /\ WitnessFairness_auth_lease_lifecycle_publication_round_trip_3
    /\ WitnessFairness_auth_lease_lifecycle_publication_round_trip_4
    /\ WitnessFairness_auth_lease_lifecycle_publication_round_trip_5
    /\ WitnessFairness_auth_lease_lifecycle_publication_round_trip_6


THEOREM Spec => []auth_lease_lifecycle_publication_protocol_covered
THEOREM Spec => []auth_machine_oauth_flow_membership_consistent
THEOREM Spec => []NoOpenObligationsOnTerminal_auth_lease_lifecycle_publication

=============================================================================
