---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for auth_lease_bundle.

CONSTANTS AuthLifecyclePhaseValues, BooleanValues, CredentialUseDispositionValues, CredentialUseIntentValues, NatValues, RefreshFailureDispositionValues, SetOfStringValues, StringValues

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

VARIABLES auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, obligation_auth_machine_auth_release_oauth_flow_drain, obligation_auth_machine_auth_lease_lifecycle_publication, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, obligation_auth_machine_auth_release_oauth_flow_drain, obligation_auth_machine_auth_lease_lifecycle_publication, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_0ec772f63e2ab753 == UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_142968104a9da3b5 == UNCHANGED << auth_machine_credential_generation, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_2f784fb0b4b0fa86 == UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_33a078928811255f == UNCHANGED << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, obligation_auth_machine_auth_lease_lifecycle_publication, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_386d7041634a8604 == UNCHANGED << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, obligation_auth_machine_auth_release_oauth_flow_drain, obligation_auth_machine_auth_lease_lifecycle_publication, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_55ac2e7c2bf5dba1 == UNCHANGED << auth_machine_phase, auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, obligation_auth_machine_auth_release_oauth_flow_drain, obligation_auth_machine_auth_lease_lifecycle_publication, pending_routes, delivered_routes, emitted_effects, observed_transitions >>
UnchangedFrame_6d338e6154d09149 == UNCHANGED << obligation_auth_machine_auth_release_oauth_flow_drain, obligation_auth_machine_auth_lease_lifecycle_publication >>
UnchangedFrame_758184af7afe5f8d == UNCHANGED << auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_7d08f82f20f4c9b9 == UNCHANGED << auth_machine_credential_generation, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_87048e7ba4c974a0 == UNCHANGED << obligation_auth_machine_auth_release_oauth_flow_drain >>
UnchangedFrame_91362fe975d968ca == UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_release_draining, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_9505f18efa8439f4 == UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_b8305a9f03dbd4c5 == UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_c72cb1d65d4e320f == UNCHANGED << auth_machine_last_refresh, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_device_poll_ids, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_c794524b689d9327 == UNCHANGED << obligation_auth_machine_auth_lease_lifecycle_publication >>
UnchangedFrame_e66abac924d246d9 == UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_release_draining, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_fc530d92825d2bc1 == UNCHANGED << auth_machine_expires_at, auth_machine_last_refresh, auth_machine_refresh_attempt, auth_machine_credential_present, auth_machine_credential_generation, auth_machine_credential_published_at_millis, auth_machine_oauth_browser_flow_ids, auth_machine_oauth_browser_flow_providers, auth_machine_oauth_browser_flow_redirect_uris, auth_machine_oauth_browser_flow_expires_at_millis, auth_machine_oauth_device_flow_ids, auth_machine_oauth_device_flow_providers, auth_machine_oauth_device_flow_expires_at_millis, auth_machine_oauth_outstanding_flow_count, auth_machine_release_draining, witness_current_script_input, witness_remaining_script_inputs >>
UnchangedFrame_ff767348ef3f1efe == UNCHANGED << witness_current_script_input, witness_remaining_script_inputs >>

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
    /\ auth_machine_release_draining = FALSE
    /\ obligation_auth_machine_auth_release_oauth_flow_drain = {}
    /\ obligation_auth_machine_auth_lease_lifecycle_publication = {}
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

WitnessInit_freshness_expiry ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(1)], source_kind |-> "entry", source_route |-> "witness:freshness_expiry:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(1)], source_kind |-> "entry", source_route |-> "witness:freshness_expiry:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(1)], source_kind |-> "entry", source_route |-> "witness:freshness_expiry:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "auth_machine", variant |-> "ObserveCredentialFreshness", payload |-> [now_ts |-> 0, refresh_window_secs |-> 0], source_kind |-> "entry", source_route |-> "witness:freshness_expiry:2", source_machine |-> "external_entry", source_effect |-> "ObserveCredentialFreshness", effect_id |-> 0], [machine |-> "auth_machine", variant |-> "ObserveCredentialFreshness", payload |-> [now_ts |-> 2, refresh_window_secs |-> 0], source_kind |-> "entry", source_route |-> "witness:freshness_expiry:3", source_machine |-> "external_entry", source_effect |-> "ObserveCredentialFreshness", effect_id |-> 0]>>

WitnessInit_expiring_refresh ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:expiring_refresh:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:expiring_refresh:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:expiring_refresh:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "auth_machine", variant |-> "ObserveCredentialFreshness", payload |-> [now_ts |-> 1, refresh_window_secs |-> 2], source_kind |-> "entry", source_route |-> "witness:expiring_refresh:2", source_machine |-> "external_entry", source_effect |-> "ObserveCredentialFreshness", effect_id |-> 0], [machine |-> "auth_machine", variant |-> "BeginRefresh", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:expiring_refresh:3", source_machine |-> "external_entry", source_effect |-> "BeginRefresh", effect_id |-> 0], [machine |-> "auth_machine", variant |-> "CompleteRefresh", payload |-> [credential_published_at_millis |-> 2, new_expires_at |-> Some(2), now_ts |-> 1], source_kind |-> "entry", source_route |-> "witness:expiring_refresh:4", source_machine |-> "external_entry", source_effect |-> "CompleteRefresh", effect_id |-> 0]>>

WitnessInit_release_drains_oauth_flow ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_flow:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_flow:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_flow:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "auth_machine", variant |-> "AdmitOAuthBrowserFlow", payload |-> [expires_at_millis |-> 2, flow_id |-> "flow_1", max_outstanding_flows |-> 1, observed_global_outstanding_flows |-> 0, provider |-> "provider_1", redirect_uri |-> "uri_1"], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_flow:2", source_machine |-> "external_entry", source_effect |-> "AdmitOAuthBrowserFlow", effect_id |-> 0], [machine |-> "auth_machine", variant |-> "BeginRelease", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_flow:3", source_machine |-> "external_entry", source_effect |-> "BeginRelease", effect_id |-> 0], [machine |-> "auth_machine", variant |-> "Release", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_flow:4", source_machine |-> "external_entry", source_effect |-> "Release", effect_id |-> 0]>>

WitnessInit_release_drains_oauth_device_flow ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_device_flow:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_device_flow:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "auth_machine", variant |-> "Acquire", payload |-> [credential_published_at_millis |-> 1, expires_at_ts |-> Some(2)], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_device_flow:1", source_machine |-> "external_entry", source_effect |-> "Acquire", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "auth_machine", variant |-> "AdmitOAuthDeviceFlow", payload |-> [expires_at_millis |-> 2, flow_id |-> "flow_1", max_outstanding_flows |-> 1, observed_global_outstanding_flows |-> 0, provider |-> "provider_1"], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_device_flow:2", source_machine |-> "external_entry", source_effect |-> "AdmitOAuthDeviceFlow", effect_id |-> 0], [machine |-> "auth_machine", variant |-> "BeginRelease", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_device_flow:3", source_machine |-> "external_entry", source_effect |-> "BeginRelease", effect_id |-> 0], [machine |-> "auth_machine", variant |-> "Release", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:release_drains_oauth_device_flow:4", source_machine |-> "external_entry", source_effect |-> "Release", effect_id |-> 0]>>

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
       /\ UnchangedFrame_c72cb1d65d4e320f
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (auth_machine_credential_generation + 1), credential_published_at_millis |-> Some(packet.payload.credential_published_at_millis), expires_at |-> packet.payload.expires_at_ts, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "Acquire"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "Acquire", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> packet.payload.expires_at_ts, credential_generation |-> (auth_machine_credential_generation + 1), credential_published_at_millis |-> Some(packet.payload.credential_published_at_millis)]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkExpiring"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "Expiring"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpiringFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpiringFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpiredFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpiredFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpiredFromExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpiredFromExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCredentialFreshnessReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ObserveCredentialFreshnessReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Released", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginRefreshFromValid ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRefresh"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "Refreshing"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromValid"], [machine |-> "auth_machine", variant |-> "WakeRefreshLoop", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginRefreshFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginRefreshFromExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRefresh"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ auth_machine_phase' = "Refreshing"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpiring"], [machine |-> "auth_machine", variant |-> "WakeRefreshLoop", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginRefreshFromExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginRefreshFromExpired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRefresh"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ auth_machine_phase' = "Refreshing"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpired"], [machine |-> "auth_machine", variant |-> "WakeRefreshLoop", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRefreshFromExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginRefreshFromExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (auth_machine_credential_generation + 1), credential_published_at_millis |-> Some(packet.payload.credential_published_at_millis), expires_at |-> packet.payload.new_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteRefresh"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "CompleteRefresh", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> packet.payload.new_expires_at, credential_generation |-> (auth_machine_credential_generation + 1), credential_published_at_millis |-> Some(packet.payload.credential_published_at_millis)]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveRefreshFailureDispositionTransientRefreshing(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveRefreshFailureDisposition"
       /\ packet.payload.http_status = arg_http_status
       /\ packet.payload.oauth_error_code = arg_oauth_error_code
       /\ packet.payload.local_credential_unusable = arg_local_credential_unusable
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.local_credential_unusable = FALSE) /\ (packet.payload.http_status # Some(401)) /\ (packet.payload.http_status # Some(403)) /\ (packet.payload.oauth_error_code # Some("invalid_grant")) /\ (packet.payload.oauth_error_code # Some("invalid_client")) /\ (packet.payload.oauth_error_code # Some("unauthorized_client")) /\ (packet.payload.oauth_error_code # Some("invalid_scope")) /\ (packet.payload.oauth_error_code # Some("access_denied")) /\ (packet.payload.oauth_error_code # Some("permission_denied")) /\ (packet.payload.oauth_error_code # Some("expired_token")))
       /\ auth_machine_phase' = "Refreshing"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "RefreshFailureDispositionResolved", payload |-> [disposition |-> "Transient"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveRefreshFailureDispositionTransientRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveRefreshFailureDispositionTransientRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveRefreshFailureDispositionPermanentRefreshing(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveRefreshFailureDisposition"
       /\ packet.payload.http_status = arg_http_status
       /\ packet.payload.oauth_error_code = arg_oauth_error_code
       /\ packet.payload.local_credential_unusable = arg_local_credential_unusable
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (IF (packet.payload.local_credential_unusable = TRUE) THEN TRUE ELSE (IF (packet.payload.http_status = Some(401)) THEN TRUE ELSE (IF (packet.payload.http_status = Some(403)) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_grant")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_client")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("unauthorized_client")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_scope")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("access_denied")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("permission_denied")) THEN TRUE ELSE (packet.payload.oauth_error_code = Some("expired_token")))))))))))
       /\ auth_machine_phase' = "Refreshing"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "RefreshFailureDispositionResolved", payload |-> [disposition |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveRefreshFailureDispositionPermanentRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveRefreshFailureDispositionPermanentRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_RefreshFailedTransient(arg_disposition) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RefreshFailed"
       /\ packet.payload.disposition = arg_disposition
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.disposition = "Transient")
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_refresh_attempt' = (auth_machine_refresh_attempt + 1)
       /\ UnchangedFrame_0ec772f63e2ab753
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RefreshFailedTransient"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RefreshFailedTransient", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_RefreshFailedPermanent(arg_disposition) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RefreshFailed"
       /\ packet.payload.disposition = arg_disposition
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (packet.payload.disposition = "ReauthRequired")
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_refresh_attempt' = (auth_machine_refresh_attempt + 1)
       /\ UnchangedFrame_0ec772f63e2ab753
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RefreshFailedPermanent"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RefreshFailedPermanent", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromValid ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromExpired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_MarkReauthRequiredFromRefreshing ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "MarkReauthRequired"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkReauthRequiredFromRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "MarkReauthRequiredFromRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_142968104a9da3b5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ClearCredentialLifecycle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ClearCredentialLifecycle", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> None, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_142968104a9da3b5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ReleaseCredentialLifecycleWithOAuth"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ReleaseCredentialLifecycleWithOAuth", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> None, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ auth_machine_release_draining' = FALSE
       /\ UnchangedFrame_7d08f82f20f4c9b9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "ReleaseCredentialLifecycleWithoutOAuth"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ReleaseCredentialLifecycleWithoutOAuth", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Released", expires_at |-> None, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseDrainingOAuthFlowsValid ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (auth_machine_oauth_outstanding_flow_count > 0)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CancelOAuthFlowsForRelease", payload |-> [browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids], effect_id |-> (model_step_count + 1), source_transition |-> "BeginReleaseDrainingOAuthFlowsValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseDrainingOAuthFlowsValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_release_oauth_flow_drain' = obligation_auth_machine_auth_release_oauth_flow_drain \cup {[effect_id |-> (model_step_count + 1), browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids]}
       /\ UnchangedFrame_c794524b689d9327
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseDrainingOAuthFlowsExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (auth_machine_oauth_outstanding_flow_count > 0)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CancelOAuthFlowsForRelease", payload |-> [browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids], effect_id |-> (model_step_count + 1), source_transition |-> "BeginReleaseDrainingOAuthFlowsExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseDrainingOAuthFlowsExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_release_oauth_flow_drain' = obligation_auth_machine_auth_release_oauth_flow_drain \cup {[effect_id |-> (model_step_count + 1), browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids]}
       /\ UnchangedFrame_c794524b689d9327
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseDrainingOAuthFlowsExpired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (auth_machine_oauth_outstanding_flow_count > 0)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CancelOAuthFlowsForRelease", payload |-> [browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids], effect_id |-> (model_step_count + 1), source_transition |-> "BeginReleaseDrainingOAuthFlowsExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseDrainingOAuthFlowsExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_release_oauth_flow_drain' = obligation_auth_machine_auth_release_oauth_flow_drain \cup {[effect_id |-> (model_step_count + 1), browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids]}
       /\ UnchangedFrame_c794524b689d9327
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseDrainingOAuthFlowsRefreshing ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (auth_machine_oauth_outstanding_flow_count > 0)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CancelOAuthFlowsForRelease", payload |-> [browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids], effect_id |-> (model_step_count + 1), source_transition |-> "BeginReleaseDrainingOAuthFlowsRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseDrainingOAuthFlowsRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_release_oauth_flow_drain' = obligation_auth_machine_auth_release_oauth_flow_drain \cup {[effect_id |-> (model_step_count + 1), browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids]}
       /\ UnchangedFrame_c794524b689d9327
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseDrainingOAuthFlowsReauthRequired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (auth_machine_oauth_outstanding_flow_count > 0)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CancelOAuthFlowsForRelease", payload |-> [browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids], effect_id |-> (model_step_count + 1), source_transition |-> "BeginReleaseDrainingOAuthFlowsReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseDrainingOAuthFlowsReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_release_oauth_flow_drain' = obligation_auth_machine_auth_release_oauth_flow_drain \cup {[effect_id |-> (model_step_count + 1), browser_flow_ids |-> auth_machine_oauth_browser_flow_ids, device_flow_ids |-> auth_machine_oauth_device_flow_ids]}
       /\ UnchangedFrame_c794524b689d9327
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseWithoutOAuthFlowsValid ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (auth_machine_oauth_outstanding_flow_count = 0)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseWithoutOAuthFlowsValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseWithoutOAuthFlowsExpiring ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (auth_machine_oauth_outstanding_flow_count = 0)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseWithoutOAuthFlowsExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseWithoutOAuthFlowsExpired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (auth_machine_oauth_outstanding_flow_count = 0)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseWithoutOAuthFlowsExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseWithoutOAuthFlowsRefreshing ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (auth_machine_oauth_outstanding_flow_count = 0)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseWithoutOAuthFlowsRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseWithoutOAuthFlowsReauthRequired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (auth_machine_oauth_outstanding_flow_count = 0)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_release_draining' = TRUE
       /\ UnchangedFrame_9505f18efa8439f4
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseWithoutOAuthFlowsReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_BeginReleaseReleased ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "BeginRelease"
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginReleaseReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_Release ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "Release"
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
       /\ auth_machine_release_draining' = FALSE
       /\ UnchangedFrame_7d08f82f20f4c9b9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "Release"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "Release", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Released", expires_at |-> None, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> None]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> None, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> None]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ auth_machine_release_draining' = FALSE
       /\ UnchangedFrame_ff767348ef3f1efe
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> None, expires_at |-> None, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Released", expires_at |-> None, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> None]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_2f784fb0b4b0fa86
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ auth_machine_release_draining' = FALSE
       /\ UnchangedFrame_758184af7afe5f8d
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis, expires_at |-> packet.payload.expires_at, new_state |-> "Released"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreAuthoritySnapshotReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreAuthoritySnapshotReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Released", expires_at |-> packet.payload.expires_at, credential_generation |-> (IF (packet.payload.credential_generation > auth_machine_credential_generation) THEN packet.payload.credential_generation ELSE auth_machine_credential_generation), credential_published_at_millis |-> packet.payload.credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.redirect_uri # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.redirect_uri THEN packet.payload.redirect_uri["value"] ELSE None))
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.provider # None)
       /\ (packet.payload.expires_at_millis # None)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.provider THEN packet.payload.provider["value"] ELSE None))
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, (IF "value" \in DOMAIN packet.payload.expires_at_millis THEN packet.payload.expires_at_millis["value"] ELSE None))
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = IF ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE) THEN (auth_machine_oauth_outstanding_flow_count + 1) ELSE auth_machine_oauth_outstanding_flow_count
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollExpired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_RestoreOAuthDevicePollReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "RestoreOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ (auth_machine_release_draining = FALSE)
       /\ (packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \cup {packet.payload.flow_id})
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "RestoreOAuthDevicePollReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "RestoreOAuthDevicePollReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_browser_flow_ids' = (auth_machine_oauth_browser_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_browser_flow_providers' = MapSet(auth_machine_oauth_browser_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_browser_flow_redirect_uris' = MapSet(auth_machine_oauth_browser_flow_redirect_uris, packet.payload.flow_id, packet.payload.redirect_uri)
       /\ auth_machine_oauth_browser_flow_expires_at_millis' = MapSet(auth_machine_oauth_browser_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ReopenReleasedForOAuthBrowserFlowAdmission"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ReopenReleasedForOAuthBrowserFlowAdmission", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_91362fe975d968ca
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthBrowserFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowAbsentValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Valid"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowAbsentValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowAbsentExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowAbsentExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowAbsentExpired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Expired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowAbsentExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowAbsentRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Refreshing"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowAbsentRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowAbsentReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowAbsentReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthBrowserFlowReleased(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthBrowserFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthBrowserFlowReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Valid"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expiring"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Expired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "Refreshing"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ (auth_machine_release_draining = FALSE)
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ (auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)
       /\ (packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ auth_machine_oauth_device_flow_ids' = (auth_machine_oauth_device_flow_ids \cup {packet.payload.flow_id})
       /\ auth_machine_oauth_device_flow_providers' = MapSet(auth_machine_oauth_device_flow_providers, packet.payload.flow_id, packet.payload.provider)
       /\ auth_machine_oauth_device_flow_expires_at_millis' = MapSet(auth_machine_oauth_device_flow_expires_at_millis, packet.payload.flow_id, packet.payload.expires_at_millis)
       /\ auth_machine_oauth_device_poll_ids' = (auth_machine_oauth_device_poll_ids \ {packet.payload.flow_id})
       /\ auth_machine_oauth_outstanding_flow_count' = (auth_machine_oauth_outstanding_flow_count + 1)
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "AdmitOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ReopenReleasedForOAuthDeviceFlowAdmission"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ReopenReleasedForOAuthDeviceFlowAdmission", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ConfirmOAuthDurableAdmissionReleased(arg_observed_global_outstanding_flows, arg_max_outstanding_flows) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ConfirmOAuthDurableAdmission"
       /\ packet.payload.observed_global_outstanding_flows = arg_observed_global_outstanding_flows
       /\ packet.payload.max_outstanding_flows = arg_max_outstanding_flows
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConfirmOAuthDurableAdmissionReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "VerifyOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "VerifyOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginOAuthDevicePollReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "BeginOAuthDevicePollReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_fc530d92825d2bc1
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishOAuthDevicePollReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollAbsentValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Valid"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollAbsentValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollAbsentExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollAbsentExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollAbsentExpired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Expired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollAbsentExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollAbsentRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "Refreshing"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollAbsentRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollAbsentReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollAbsentReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_FinishOAuthDevicePollReleased(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "FinishOAuthDevicePoll"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "FinishOAuthDevicePollReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ConsumeOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ConsumeOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Valid"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Valid", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expiring"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expiring", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Expired"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Expired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "Refreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "Refreshing", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
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
       /\ UnchangedFrame_e66abac924d246d9
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "EmitLifecycleEvent", payload |-> [credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis, expires_at |-> auth_machine_expires_at, new_state |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ExpireOAuthDeviceFlowReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ obligation_auth_machine_auth_lease_lifecycle_publication' = obligation_auth_machine_auth_lease_lifecycle_publication \cup {[effect_id |-> (model_step_count + 1), new_state |-> "ReauthRequired", expires_at |-> auth_machine_expires_at, credential_generation |-> auth_machine_credential_generation, credential_published_at_millis |-> auth_machine_credential_published_at_millis]}
       /\ UnchangedFrame_87048e7ba4c974a0
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowAbsentValid(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Valid"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Valid"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowAbsentValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowAbsentExpiring(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expiring"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Expiring"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowAbsentExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowAbsentExpired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Expired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Expired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowAbsentExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowAbsentRefreshing(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Refreshing"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "Refreshing"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowAbsentRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowAbsentReauthRequired(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowAbsentReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ExpireOAuthDeviceFlowReleased(arg_flow_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ExpireOAuthDeviceFlow"
       /\ packet.payload.flow_id = arg_flow_id
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ExpireOAuthDeviceFlowReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionValidUseAuthorizedValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionValidUseAuthorizedValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionValidHoldAuthorizedValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionValidHoldAuthorizedValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionValidBeginRefreshValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionValidBeginRefreshValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionValidNoCredentialValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionValidNoCredentialValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiringUseRefreshExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiringUseRefreshExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiringNoCredentialExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiringNoCredentialExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiredUseRefreshExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiredUseRefreshExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiredHoldRefreshExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiredHoldRefreshExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiredBeginRefreshExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiredBeginRefreshExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionExpiredNoCredentialExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionExpiredNoCredentialExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "AlreadyRefreshing"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionReauthRequiredReauthRequired(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "ReauthRequired"
       /\ auth_machine_phase' = "ReauthRequired"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "ReauthRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionReauthRequiredReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionReauthRequiredReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_ResolveCredentialUseAdmissionReleasedReleased(arg_intent) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "auth_machine"
       /\ packet.variant = "ResolveCredentialUseAdmission"
       /\ packet.payload.intent = arg_intent
       /\ ~HigherPriorityReady("auth_machine_authority")
       /\ auth_machine_phase = "Released"
       /\ auth_machine_phase' = "Released"
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "LeaseAbsent"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveCredentialUseAdmissionReleasedReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveCredentialUseAdmissionReleasedReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "Authorized"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionUseCachedValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionUseCachedValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshValidValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshValidValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Valid"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshRequired"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expiring"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Expired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Refreshing"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "ReauthRequired"]}
       /\ UnchangedFrame_6d338e6154d09149
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
       /\ UnchangedFrame_b8305a9f03dbd4c5
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "auth_machine", variant |-> "CredentialUseAdmissionResolved", payload |-> [disposition |-> "RefreshDisallowed"], effect_id |-> (model_step_count + 1), source_transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "auth_machine", transition |-> "ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased", actor |-> "auth_machine_authority", step |-> (model_step_count + 1), from_phase |-> auth_machine_phase, to_phase |-> "Released"]}
       /\ UnchangedFrame_6d338e6154d09149
       /\ model_step_count' = model_step_count + 1


auth_machine_oauth_flow_membership_consistent == ((DOMAIN auth_machine_oauth_browser_flow_providers = auth_machine_oauth_browser_flow_ids) /\ (DOMAIN auth_machine_oauth_browser_flow_redirect_uris = auth_machine_oauth_browser_flow_ids) /\ (DOMAIN auth_machine_oauth_browser_flow_expires_at_millis = auth_machine_oauth_browser_flow_ids) /\ (DOMAIN auth_machine_oauth_device_flow_providers = auth_machine_oauth_device_flow_ids) /\ (DOMAIN auth_machine_oauth_device_flow_expires_at_millis = auth_machine_oauth_device_flow_ids) /\ (\A flow_id \in auth_machine_oauth_device_poll_ids : (flow_id \in auth_machine_oauth_device_flow_ids)) /\ (auth_machine_oauth_outstanding_flow_count = (Cardinality(auth_machine_oauth_browser_flow_ids) + Cardinality(auth_machine_oauth_device_flow_ids))))
auth_machine_released_oauth_membership_drained == (IF (auth_machine_phase # "Released") THEN TRUE ELSE (auth_machine_oauth_outstanding_flow_count = 0))
auth_machine_released_not_release_draining == (IF (auth_machine_phase # "Released") THEN TRUE ELSE (auth_machine_release_draining = FALSE))

EntryPacketAdmissible_auth_machine(packet) ==
    \/ /\ (packet.variant = "Acquire") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released")
    \/ /\ (packet.variant = "MarkExpiring") /\ (auth_machine_phase = "Valid")
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "Valid") /\ ((IF (auth_machine_expires_at = None) THEN TRUE ELSE ((packet.payload.now_ts + packet.payload.refresh_window_secs) <= (IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None))))
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "Valid") /\ ((IF (auth_machine_expires_at = None) THEN FALSE ELSE ((packet.payload.now_ts < (IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None)) /\ ((IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None) < (packet.payload.now_ts + packet.payload.refresh_window_secs)))))
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "Valid") /\ ((IF (auth_machine_expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None) <= packet.payload.now_ts)))
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "Expiring") /\ ((IF (auth_machine_expires_at = None) THEN TRUE ELSE (packet.payload.now_ts < (IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None))))
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "Expiring") /\ ((IF (auth_machine_expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN auth_machine_expires_at THEN auth_machine_expires_at["value"] ELSE None) <= packet.payload.now_ts)))
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "Expired")
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "Refreshing")
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "ReauthRequired")
    \/ /\ (packet.variant = "ObserveCredentialFreshness") /\ (auth_machine_phase = "Released")
    \/ /\ (packet.variant = "BeginRefresh") /\ (auth_machine_phase = "Valid")
    \/ /\ (packet.variant = "BeginRefresh") /\ (auth_machine_phase = "Expiring")
    \/ /\ (packet.variant = "BeginRefresh") /\ (auth_machine_phase = "Expired")
    \/ /\ (packet.variant = "CompleteRefresh") /\ (auth_machine_phase = "Refreshing") /\ ((IF (packet.payload.new_expires_at = None) THEN TRUE ELSE (packet.payload.now_ts < (IF "value" \in DOMAIN packet.payload.new_expires_at THEN packet.payload.new_expires_at["value"] ELSE None))))
    \/ /\ (packet.variant = "ResolveRefreshFailureDisposition") /\ (auth_machine_phase = "Refreshing") /\ (((packet.payload.local_credential_unusable = FALSE) /\ (packet.payload.http_status # Some(401)) /\ (packet.payload.http_status # Some(403)) /\ (packet.payload.oauth_error_code # Some("invalid_grant")) /\ (packet.payload.oauth_error_code # Some("invalid_client")) /\ (packet.payload.oauth_error_code # Some("unauthorized_client")) /\ (packet.payload.oauth_error_code # Some("invalid_scope")) /\ (packet.payload.oauth_error_code # Some("access_denied")) /\ (packet.payload.oauth_error_code # Some("permission_denied")) /\ (packet.payload.oauth_error_code # Some("expired_token"))))
    \/ /\ (packet.variant = "ResolveRefreshFailureDisposition") /\ (auth_machine_phase = "Refreshing") /\ ((IF (packet.payload.local_credential_unusable = TRUE) THEN TRUE ELSE (IF (packet.payload.http_status = Some(401)) THEN TRUE ELSE (IF (packet.payload.http_status = Some(403)) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_grant")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_client")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("unauthorized_client")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("invalid_scope")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("access_denied")) THEN TRUE ELSE (IF (packet.payload.oauth_error_code = Some("permission_denied")) THEN TRUE ELSE (packet.payload.oauth_error_code = Some("expired_token"))))))))))))
    \/ /\ (packet.variant = "RefreshFailed") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.disposition = "Transient"))
    \/ /\ (packet.variant = "RefreshFailed") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.disposition = "ReauthRequired"))
    \/ /\ (packet.variant = "MarkReauthRequired") /\ (auth_machine_phase = "Valid")
    \/ /\ (packet.variant = "MarkReauthRequired") /\ (auth_machine_phase = "Expiring")
    \/ /\ (packet.variant = "MarkReauthRequired") /\ (auth_machine_phase = "Expired")
    \/ /\ (packet.variant = "MarkReauthRequired") /\ (auth_machine_phase = "Refreshing")
    \/ /\ (packet.variant = "ClearCredentialLifecycle") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released")
    \/ /\ (packet.variant = "ReleaseCredentialLifecycle") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ ((auth_machine_oauth_outstanding_flow_count > 0))
    \/ /\ (packet.variant = "ReleaseCredentialLifecycle") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ ((auth_machine_oauth_outstanding_flow_count = 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_oauth_outstanding_flow_count > 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Expiring") /\ ((auth_machine_oauth_outstanding_flow_count > 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Expired") /\ ((auth_machine_oauth_outstanding_flow_count > 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Refreshing") /\ ((auth_machine_oauth_outstanding_flow_count > 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "ReauthRequired") /\ ((auth_machine_oauth_outstanding_flow_count > 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_oauth_outstanding_flow_count = 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Expiring") /\ ((auth_machine_oauth_outstanding_flow_count = 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Expired") /\ ((auth_machine_oauth_outstanding_flow_count = 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Refreshing") /\ ((auth_machine_oauth_outstanding_flow_count = 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "ReauthRequired") /\ ((auth_machine_oauth_outstanding_flow_count = 0))
    \/ /\ (packet.variant = "BeginRelease") /\ (auth_machine_phase = "Released")
    \/ /\ (packet.variant = "Release") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ ((auth_machine_oauth_outstanding_flow_count = 0))
    \/ /\ (packet.variant = "RestoreCredentialLifecycleSnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = Some("Valid")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreCredentialLifecycleSnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = Some("Expiring")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreCredentialLifecycleSnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = Some("Refreshing")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreCredentialLifecycleSnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = Some("Expired")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreCredentialLifecycleSnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = Some("ReauthRequired")) /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreCredentialLifecycleSnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ ((IF (packet.payload.credential_present = FALSE) THEN TRUE ELSE (IF (packet.payload.lifecycle_phase = None) THEN TRUE ELSE (packet.payload.lifecycle_phase = Some("Released"))))) /\ ((IF (auth_machine_oauth_outstanding_flow_count > 0) THEN TRUE ELSE packet.payload.restored_oauth_membership_observed))
    \/ /\ (packet.variant = "RestoreCredentialLifecycleSnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ ((IF (packet.payload.credential_present = FALSE) THEN TRUE ELSE (IF (packet.payload.lifecycle_phase = None) THEN TRUE ELSE (packet.payload.lifecycle_phase = Some("Released"))))) /\ (((auth_machine_oauth_outstanding_flow_count = 0) /\ (packet.payload.restored_oauth_membership_observed = FALSE)))
    \/ /\ (packet.variant = "RestoreAuthoritySnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = "Valid") /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreAuthoritySnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = "Expiring") /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreAuthoritySnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = "Refreshing") /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreAuthoritySnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = "Expired") /\ packet.payload.credential_present /\ (packet.payload.credential_published_at_millis # None)))
    \/ /\ (packet.variant = "RestoreAuthoritySnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = "ReauthRequired") /\ (IF (packet.payload.credential_present = FALSE) THEN TRUE ELSE (packet.payload.credential_published_at_millis # None))))
    \/ /\ (packet.variant = "RestoreAuthoritySnapshot") /\ (auth_machine_phase = "Valid" \/ auth_machine_phase = "Expiring" \/ auth_machine_phase = "Expired" \/ auth_machine_phase = "Refreshing" \/ auth_machine_phase = "ReauthRequired" \/ auth_machine_phase = "Released") /\ (((packet.payload.lifecycle_phase = "Released") /\ (packet.payload.credential_present = FALSE) /\ (packet.payload.credential_published_at_millis = None) /\ (auth_machine_oauth_outstanding_flow_count = 0)))
    \/ /\ (packet.variant = "RestoreOAuthBrowserFlow") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.redirect_uri # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthBrowserFlow") /\ (auth_machine_phase = "Expiring") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.redirect_uri # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthBrowserFlow") /\ (auth_machine_phase = "Expired") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.redirect_uri # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthBrowserFlow") /\ (auth_machine_phase = "Refreshing") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.redirect_uri # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthBrowserFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.redirect_uri # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthDeviceFlow") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthDeviceFlow") /\ (auth_machine_phase = "Expiring") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthDeviceFlow") /\ (auth_machine_phase = "Expired") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthDeviceFlow") /\ (auth_machine_phase = "Refreshing") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthDeviceFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.provider # None)) /\ ((packet.payload.expires_at_millis # None))
    \/ /\ (packet.variant = "RestoreOAuthDevicePoll") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "RestoreOAuthDevicePoll") /\ (auth_machine_phase = "Expiring") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "RestoreOAuthDevicePoll") /\ (auth_machine_phase = "Expired") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "RestoreOAuthDevicePoll") /\ (auth_machine_phase = "Refreshing") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "RestoreOAuthDevicePoll") /\ (auth_machine_phase = "ReauthRequired") /\ ((auth_machine_release_draining = FALSE)) /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "AdmitOAuthBrowserFlow") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthBrowserFlow") /\ (auth_machine_phase = "Expiring") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthBrowserFlow") /\ (auth_machine_phase = "Expired") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthBrowserFlow") /\ (auth_machine_phase = "Refreshing") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthBrowserFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthBrowserFlow") /\ (auth_machine_phase = "Released") /\ (((auth_machine_credential_present = FALSE) /\ (auth_machine_credential_published_at_millis = None))) /\ ((auth_machine_oauth_outstanding_flow_count = 0)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "VerifyOAuthBrowserFlow") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "VerifyOAuthBrowserFlow") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "VerifyOAuthBrowserFlow") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "VerifyOAuthBrowserFlow") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "VerifyOAuthBrowserFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthBrowserFlow") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthBrowserFlow") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthBrowserFlow") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthBrowserFlow") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthBrowserFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_providers THEN auth_machine_oauth_browser_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_redirect_uris THEN auth_machine_oauth_browser_flow_redirect_uris[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.redirect_uri))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_browser_flow_expires_at_millis THEN auth_machine_oauth_browser_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Valid") /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Expiring") /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Expired") /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Refreshing") /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "ReauthRequired") /\ (((packet.payload.flow_id \in auth_machine_oauth_browser_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthBrowserFlow") /\ (auth_machine_phase = "Released")
    \/ /\ (packet.variant = "AdmitOAuthDeviceFlow") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthDeviceFlow") /\ (auth_machine_phase = "Expiring") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthDeviceFlow") /\ (auth_machine_phase = "Expired") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthDeviceFlow") /\ (auth_machine_phase = "Refreshing") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthDeviceFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((auth_machine_release_draining = FALSE)) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "AdmitOAuthDeviceFlow") /\ (auth_machine_phase = "Released") /\ (((auth_machine_credential_present = FALSE) /\ (auth_machine_credential_published_at_millis = None))) /\ ((auth_machine_oauth_outstanding_flow_count = 0)) /\ ((auth_machine_oauth_outstanding_flow_count < packet.payload.max_outstanding_flows)) /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "ConfirmOAuthDurableAdmission") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "ConfirmOAuthDurableAdmission") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "ConfirmOAuthDurableAdmission") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "ConfirmOAuthDurableAdmission") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "ConfirmOAuthDurableAdmission") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.observed_global_outstanding_flows < packet.payload.max_outstanding_flows))
    \/ /\ (packet.variant = "ConfirmOAuthDurableAdmission") /\ (auth_machine_phase = "Released")
    \/ /\ (packet.variant = "VerifyOAuthDeviceFlow") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "VerifyOAuthDeviceFlow") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "VerifyOAuthDeviceFlow") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "VerifyOAuthDeviceFlow") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "VerifyOAuthDeviceFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "BeginOAuthDevicePoll") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "BeginOAuthDevicePoll") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "BeginOAuthDevicePoll") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "BeginOAuthDevicePoll") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "BeginOAuthDevicePoll") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None))) /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Valid") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Expiring") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Expired") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Refreshing") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "ReauthRequired") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_poll_ids) = FALSE))
    \/ /\ (packet.variant = "FinishOAuthDevicePoll") /\ (auth_machine_phase = "Released")
    \/ /\ (packet.variant = "ConsumeOAuthDeviceFlow") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthDeviceFlow") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthDeviceFlow") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthDeviceFlow") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ConsumeOAuthDeviceFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids)) /\ (((IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_providers THEN auth_machine_oauth_device_flow_providers[packet.payload.flow_id] ELSE "None")) ELSE None) = Some(packet.payload.provider))) /\ ((packet.payload.now_millis <= (IF "value" \in DOMAIN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis) THEN Some((IF packet.payload.flow_id \in DOMAIN auth_machine_oauth_device_flow_expires_at_millis THEN auth_machine_oauth_device_flow_expires_at_millis[packet.payload.flow_id] ELSE 0)) ELSE None)["value"] ELSE None)))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Valid") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Valid") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Expiring") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Expired") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Refreshing") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "ReauthRequired") /\ (((packet.payload.flow_id \in auth_machine_oauth_device_flow_ids) = FALSE))
    \/ /\ (packet.variant = "ExpireOAuthDeviceFlow") /\ (auth_machine_phase = "Released")
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Valid") /\ (((packet.payload.intent = "UseCredential") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Valid") /\ (((packet.payload.intent = "HoldAuthority") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Valid") /\ (((packet.payload.intent = "BeginRefresh") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_credential_present = FALSE))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Expiring") /\ (((packet.payload.intent = "UseCredential") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Expiring") /\ (((packet.payload.intent = "HoldAuthority") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Expiring") /\ (((packet.payload.intent = "BeginRefresh") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Expiring") /\ ((auth_machine_credential_present = FALSE))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Expired") /\ (((packet.payload.intent = "UseCredential") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Expired") /\ (((packet.payload.intent = "HoldAuthority") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Expired") /\ (((packet.payload.intent = "BeginRefresh") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Expired") /\ ((auth_machine_credential_present = FALSE))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Refreshing") /\ (((packet.payload.intent = "UseCredential") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Refreshing") /\ (((packet.payload.intent = "HoldAuthority") /\ auth_machine_credential_present))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.intent = "BeginRefresh"))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Refreshing") /\ (((auth_machine_credential_present = FALSE) /\ (IF (packet.payload.intent = "UseCredential") THEN TRUE ELSE (packet.payload.intent = "HoldAuthority"))))
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "ReauthRequired")
    \/ /\ (packet.variant = "ResolveCredentialUseAdmission") /\ (auth_machine_phase = "Released")
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Valid") /\ ((auth_machine_credential_present /\ packet.payload.credential_present /\ (packet.payload.force_refresh = FALSE)))
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Valid") /\ ((~((auth_machine_credential_present /\ packet.payload.credential_present /\ (packet.payload.force_refresh = FALSE))) /\ packet.payload.refresh_allowed))
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Valid") /\ ((~((auth_machine_credential_present /\ packet.payload.credential_present /\ (packet.payload.force_refresh = FALSE))) /\ (packet.payload.refresh_allowed = FALSE)))
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Expiring") /\ (packet.payload.refresh_allowed)
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Expired") /\ (packet.payload.refresh_allowed)
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Refreshing") /\ (packet.payload.refresh_allowed)
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "ReauthRequired") /\ (packet.payload.refresh_allowed)
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Released") /\ (packet.payload.refresh_allowed)
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Expiring") /\ ((packet.payload.refresh_allowed = FALSE))
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Expired") /\ ((packet.payload.refresh_allowed = FALSE))
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Refreshing") /\ ((packet.payload.refresh_allowed = FALSE))
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "ReauthRequired") /\ ((packet.payload.refresh_allowed = FALSE))
    \/ /\ (packet.variant = "ResolveOAuthLoginCredentialDisposition") /\ (auth_machine_phase = "Released") /\ ((packet.payload.refresh_allowed = FALSE))

EntryPacketAdmissible(packet) ==
    CASE
      packet.machine = "auth_machine" -> EntryPacketAdmissible_auth_machine(packet)
      [] OTHER -> FALSE

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UnchangedFrame_386d7041634a8604

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_freshness_expiry ==
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
    /\ UnchangedFrame_55ac2e7c2bf5dba1

WitnessInjectNext_expiring_refresh ==
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
    /\ UnchangedFrame_55ac2e7c2bf5dba1

WitnessInjectNext_release_drains_oauth_flow ==
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
    /\ UnchangedFrame_55ac2e7c2bf5dba1

WitnessInjectNext_release_drains_oauth_device_flow ==
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
    /\ UnchangedFrame_55ac2e7c2bf5dba1

WitnessScriptComplete_freshness_expiry ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (auth_machine_phase = "Expired")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessExpiredFromValid")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ObserveCredentialFreshnessValid" /\ later.machine = "auth_machine" /\ later.transition = "ObserveCredentialFreshnessExpiredFromValid" /\ earlier.step < later.step)

WitnessScriptComplete_expiring_refresh ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (auth_machine_phase = "Valid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessExpiringFromValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginRefreshFromExpiring")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "CompleteRefresh")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ObserveCredentialFreshnessExpiringFromValid" /\ later.machine = "auth_machine" /\ later.transition = "BeginRefreshFromExpiring" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginRefreshFromExpiring" /\ later.machine = "auth_machine" /\ later.transition = "CompleteRefresh" /\ earlier.step < later.step)

WitnessScriptComplete_release_drains_oauth_flow ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (auth_machine_phase = "Released")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "AdmitOAuthBrowserFlowValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginReleaseDrainingOAuthFlowsValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ExpireOAuthBrowserFlowValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Release")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginReleaseDrainingOAuthFlowsValid" /\ later.machine = "auth_machine" /\ later.transition = "ExpireOAuthBrowserFlowValid" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ExpireOAuthBrowserFlowValid" /\ later.machine = "auth_machine" /\ later.transition = "Release" /\ earlier.step < later.step)

WitnessScriptComplete_release_drains_oauth_device_flow ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ (auth_machine_phase = "Released")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "AdmitOAuthDeviceFlowValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginReleaseDrainingOAuthFlowsValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ExpireOAuthDeviceFlowValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Release")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginReleaseDrainingOAuthFlowsValid" /\ later.machine = "auth_machine" /\ later.transition = "ExpireOAuthDeviceFlowValid" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ExpireOAuthDeviceFlowValid" /\ later.machine = "auth_machine" /\ later.transition = "Release" /\ earlier.step < later.step)

WitnessNoPrematureStutter_freshness_expiry ==
    \/ WitnessScriptComplete_freshness_expiry
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_expiring_refresh ==
    \/ WitnessScriptComplete_expiring_refresh
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_release_drains_oauth_flow ==
    \/ WitnessScriptComplete_release_drains_oauth_flow
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_release_drains_oauth_device_flow ==
    \/ WitnessScriptComplete_release_drains_oauth_device_flow
    \/ model_step_count' # model_step_count

WitnessSatisfiedStutter_freshness_expiry ==
    /\ WitnessScriptComplete_freshness_expiry
    /\ (auth_machine_phase = "Expired")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessExpiredFromValid")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ObserveCredentialFreshnessValid" /\ later.machine = "auth_machine" /\ later.transition = "ObserveCredentialFreshnessExpiredFromValid" /\ earlier.step < later.step)
    /\ UNCHANGED vars

WitnessSatisfiedStutter_expiring_refresh ==
    /\ WitnessScriptComplete_expiring_refresh
    /\ (auth_machine_phase = "Valid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessExpiringFromValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginRefreshFromExpiring")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "CompleteRefresh")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ObserveCredentialFreshnessExpiringFromValid" /\ later.machine = "auth_machine" /\ later.transition = "BeginRefreshFromExpiring" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginRefreshFromExpiring" /\ later.machine = "auth_machine" /\ later.transition = "CompleteRefresh" /\ earlier.step < later.step)
    /\ UNCHANGED vars

WitnessSatisfiedStutter_release_drains_oauth_flow ==
    /\ WitnessScriptComplete_release_drains_oauth_flow
    /\ (auth_machine_phase = "Released")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "AdmitOAuthBrowserFlowValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginReleaseDrainingOAuthFlowsValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ExpireOAuthBrowserFlowValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Release")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginReleaseDrainingOAuthFlowsValid" /\ later.machine = "auth_machine" /\ later.transition = "ExpireOAuthBrowserFlowValid" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ExpireOAuthBrowserFlowValid" /\ later.machine = "auth_machine" /\ later.transition = "Release" /\ earlier.step < later.step)
    /\ UNCHANGED vars

WitnessSatisfiedStutter_release_drains_oauth_device_flow ==
    /\ WitnessScriptComplete_release_drains_oauth_device_flow
    /\ (auth_machine_phase = "Released")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "AdmitOAuthDeviceFlowValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginReleaseDrainingOAuthFlowsValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ExpireOAuthDeviceFlowValid")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Release")
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginReleaseDrainingOAuthFlowsValid" /\ later.machine = "auth_machine" /\ later.transition = "ExpireOAuthDeviceFlowValid" /\ earlier.step < later.step)
    /\ (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ExpireOAuthDeviceFlowValid" /\ later.machine = "auth_machine" /\ later.transition = "Release" /\ earlier.step < later.step)
    /\ UNCHANGED vars

\* Liveness: eventual feedback: release_lease discharges the drain by firing a terminal Expire* feedback per drained flow id before committing `Release`; the Release guard `oauth_release_drained` is the machine-side completion witness
OwnerFeedback_auth_machine_auth_release_oauth_flow_drain_ExpireOAuthBrowserFlow ==
    /\ obligation_auth_machine_auth_release_oauth_flow_drain /= {}
    /\ \E token \in obligation_auth_machine_auth_release_oauth_flow_drain :
        /\ \E member_browser_flow_ids \in token.browser_flow_ids : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "auth_machine", variant |-> "ExpireOAuthBrowserFlow", source_kind |-> "owner", source_machine |-> "auth_machine", source_effect |-> "CancelOAuthFlowsForRelease", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [flow_id |-> member_browser_flow_ids]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "auth_machine", variant |-> "ExpireOAuthBrowserFlow", source_kind |-> "owner", source_machine |-> "auth_machine", source_effect |-> "CancelOAuthFlowsForRelease", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [flow_id |-> member_browser_flow_ids]]} /\ obligation_auth_machine_auth_release_oauth_flow_drain' = (IF [token EXCEPT !.browser_flow_ids = @ \ {member_browser_flow_ids}].browser_flow_ids = {} /\ [token EXCEPT !.browser_flow_ids = @ \ {member_browser_flow_ids}].device_flow_ids = {} THEN obligation_auth_machine_auth_release_oauth_flow_drain \ {token} ELSE (obligation_auth_machine_auth_release_oauth_flow_drain \ {token}) \cup {[token EXCEPT !.browser_flow_ids = @ \ {member_browser_flow_ids}]}) /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_33a078928811255f

OwnerFeedback_auth_machine_auth_release_oauth_flow_drain_ExpireOAuthDeviceFlow ==
    /\ obligation_auth_machine_auth_release_oauth_flow_drain /= {}
    /\ \E token \in obligation_auth_machine_auth_release_oauth_flow_drain :
        /\ \E member_device_flow_ids \in token.device_flow_ids : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "auth_machine", variant |-> "ExpireOAuthDeviceFlow", source_kind |-> "owner", source_machine |-> "auth_machine", source_effect |-> "CancelOAuthFlowsForRelease", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [flow_id |-> member_device_flow_ids]]) /\ observed_inputs' = observed_inputs \cup {[machine |-> "auth_machine", variant |-> "ExpireOAuthDeviceFlow", source_kind |-> "owner", source_machine |-> "auth_machine", source_effect |-> "CancelOAuthFlowsForRelease", source_route |-> "none", effect_id |-> token.effect_id, payload |-> [flow_id |-> member_device_flow_ids]]} /\ obligation_auth_machine_auth_release_oauth_flow_drain' = (IF [token EXCEPT !.device_flow_ids = @ \ {member_device_flow_ids}].browser_flow_ids = {} /\ [token EXCEPT !.device_flow_ids = @ \ {member_device_flow_ids}].device_flow_ids = {} THEN obligation_auth_machine_auth_release_oauth_flow_drain \ {token} ELSE (obligation_auth_machine_auth_release_oauth_flow_drain \ {token}) \cup {[token EXCEPT !.device_flow_ids = @ \ {member_device_flow_ids}]}) /\ model_step_count' = model_step_count + 1)
    /\ UnchangedFrame_33a078928811255f

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
    \/ \E arg_http_status \in OptionU64Values : \E arg_oauth_error_code \in OptionStringValues : auth_machine_ResolveRefreshFailureDispositionTransientRefreshing(arg_http_status, arg_oauth_error_code, FALSE)
    \/ \E arg_http_status \in OptionU64Values : \E arg_oauth_error_code \in OptionStringValues : \E arg_local_credential_unusable \in BOOLEAN : auth_machine_ResolveRefreshFailureDispositionPermanentRefreshing(arg_http_status, arg_oauth_error_code, arg_local_credential_unusable)
    \/ \E arg_disposition \in RefreshFailureDispositionValues : auth_machine_RefreshFailedTransient(arg_disposition)
    \/ \E arg_disposition \in RefreshFailureDispositionValues : auth_machine_RefreshFailedPermanent(arg_disposition)
    \/ auth_machine_MarkReauthRequiredFromValid
    \/ auth_machine_MarkReauthRequiredFromExpiring
    \/ auth_machine_MarkReauthRequiredFromExpired
    \/ auth_machine_MarkReauthRequiredFromRefreshing
    \/ auth_machine_ClearCredentialLifecycle
    \/ auth_machine_ReleaseCredentialLifecycleWithOAuth
    \/ auth_machine_ReleaseCredentialLifecycleWithoutOAuth
    \/ auth_machine_BeginReleaseDrainingOAuthFlowsValid
    \/ auth_machine_BeginReleaseDrainingOAuthFlowsExpiring
    \/ auth_machine_BeginReleaseDrainingOAuthFlowsExpired
    \/ auth_machine_BeginReleaseDrainingOAuthFlowsRefreshing
    \/ auth_machine_BeginReleaseDrainingOAuthFlowsReauthRequired
    \/ auth_machine_BeginReleaseWithoutOAuthFlowsValid
    \/ auth_machine_BeginReleaseWithoutOAuthFlowsExpiring
    \/ auth_machine_BeginReleaseWithoutOAuthFlowsExpired
    \/ auth_machine_BeginReleaseWithoutOAuthFlowsRefreshing
    \/ auth_machine_BeginReleaseWithoutOAuthFlowsReauthRequired
    \/ auth_machine_BeginReleaseReleased
    \/ auth_machine_Release
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotValid(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotExpiring(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotRefreshing(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotExpired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotReauthRequired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_restored_oauth_membership_observed \in BOOLEAN : auth_machine_RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, arg_restored_oauth_membership_observed)
    \/ \E arg_lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, FALSE)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotValid(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotExpiring(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotRefreshing(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotExpired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotReauthRequired(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E arg_lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : auth_machine_RestoreAuthoritySnapshotReleased(arg_lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, FALSE, arg_credential_generation, arg_credential_published_at_millis)
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
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowAbsentValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowAbsentExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowAbsentExpired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowAbsentRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowAbsentReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthBrowserFlowReleased(arg_flow_id)
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
    \/ \E arg_observed_global_outstanding_flows \in 0..2 : \E arg_max_outstanding_flows \in 0..2 : auth_machine_ConfirmOAuthDurableAdmissionReleased(arg_observed_global_outstanding_flows, arg_max_outstanding_flows)
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
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollAbsentValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollAbsentExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollAbsentExpired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollAbsentRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollAbsentReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_FinishOAuthDevicePollReleased(arg_flow_id)
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
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowAbsentValid(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowAbsentExpiring(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowAbsentExpired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowAbsentRefreshing(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowAbsentReauthRequired(arg_flow_id)
    \/ \E arg_flow_id \in StringValues : auth_machine_ExpireOAuthDeviceFlowReleased(arg_flow_id)
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
    \/ \E arg_refresh_allowed \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionUseCachedValid(TRUE, FALSE, arg_refresh_allowed)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshValidValid(arg_credential_present, arg_force_refresh, TRUE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid(arg_credential_present, arg_force_refresh, FALSE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring(arg_credential_present, arg_force_refresh, TRUE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired(arg_credential_present, arg_force_refresh, TRUE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing(arg_credential_present, arg_force_refresh, TRUE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired(arg_credential_present, arg_force_refresh, TRUE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased(arg_credential_present, arg_force_refresh, TRUE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring(arg_credential_present, arg_force_refresh, FALSE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired(arg_credential_present, arg_force_refresh, FALSE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing(arg_credential_present, arg_force_refresh, FALSE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired(arg_credential_present, arg_force_refresh, FALSE)
    \/ \E arg_credential_present \in BOOLEAN : \E arg_force_refresh \in BOOLEAN : auth_machine_ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased(arg_credential_present, arg_force_refresh, FALSE)
    \/ OwnerFeedback_auth_machine_auth_release_oauth_flow_drain_ExpireOAuthBrowserFlow
    \/ OwnerFeedback_auth_machine_auth_release_oauth_flow_drain_ExpireOAuthDeviceFlow
    \/ QuiescentStutter

InjectNext ==
    FALSE

Next ==
    \/ CoreNext

WitnessNext_freshness_expiry ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "Acquire" /\ auth_machine_Acquire(witness_packet.payload.expires_at_ts, witness_packet.payload.credential_published_at_millis)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "ObserveCredentialFreshness" /\ auth_machine_ObserveCredentialFreshnessValid(witness_packet.payload.now_ts, witness_packet.payload.refresh_window_secs)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "ObserveCredentialFreshness" /\ auth_machine_ObserveCredentialFreshnessExpiredFromValid(witness_packet.payload.now_ts, witness_packet.payload.refresh_window_secs)
    \/ WitnessSatisfiedStutter_freshness_expiry
    \/ WitnessInjectNext_freshness_expiry

WitnessNext_expiring_refresh ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "Acquire" /\ auth_machine_Acquire(witness_packet.payload.expires_at_ts, witness_packet.payload.credential_published_at_millis)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "ObserveCredentialFreshness" /\ auth_machine_ObserveCredentialFreshnessExpiringFromValid(witness_packet.payload.now_ts, witness_packet.payload.refresh_window_secs)
    \/ auth_machine_BeginRefreshFromExpiring
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "CompleteRefresh" /\ auth_machine_CompleteRefresh(witness_packet.payload.new_expires_at, witness_packet.payload.now_ts, witness_packet.payload.credential_published_at_millis)
    \/ WitnessSatisfiedStutter_expiring_refresh
    \/ WitnessInjectNext_expiring_refresh

WitnessNext_release_drains_oauth_flow ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "Acquire" /\ auth_machine_Acquire(witness_packet.payload.expires_at_ts, witness_packet.payload.credential_published_at_millis)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "AdmitOAuthBrowserFlow" /\ auth_machine_AdmitOAuthBrowserFlowValid(witness_packet.payload.flow_id, witness_packet.payload.provider, witness_packet.payload.redirect_uri, witness_packet.payload.expires_at_millis, witness_packet.payload.max_outstanding_flows, witness_packet.payload.observed_global_outstanding_flows)
    \/ auth_machine_BeginReleaseDrainingOAuthFlowsValid
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "ExpireOAuthBrowserFlow" /\ auth_machine_ExpireOAuthBrowserFlowValid(witness_packet.payload.flow_id)
    \/ auth_machine_Release
    \/ OwnerFeedback_auth_machine_auth_release_oauth_flow_drain_ExpireOAuthBrowserFlow
    \/ WitnessSatisfiedStutter_release_drains_oauth_flow
    \/ WitnessInjectNext_release_drains_oauth_flow

WitnessNext_release_drains_oauth_device_flow ==
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "Acquire" /\ auth_machine_Acquire(witness_packet.payload.expires_at_ts, witness_packet.payload.credential_published_at_millis)
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "AdmitOAuthDeviceFlow" /\ auth_machine_AdmitOAuthDeviceFlowValid(witness_packet.payload.flow_id, witness_packet.payload.provider, witness_packet.payload.expires_at_millis, witness_packet.payload.max_outstanding_flows, witness_packet.payload.observed_global_outstanding_flows)
    \/ auth_machine_BeginReleaseDrainingOAuthFlowsValid
    \/ \E witness_packet \in SeqElements(pending_inputs) : /\ witness_packet.machine = "auth_machine" /\ witness_packet.variant = "ExpireOAuthDeviceFlow" /\ auth_machine_ExpireOAuthDeviceFlowValid(witness_packet.payload.flow_id)
    \/ auth_machine_Release
    \/ OwnerFeedback_auth_machine_auth_release_oauth_flow_drain_ExpireOAuthDeviceFlow
    \/ WitnessSatisfiedStutter_release_drains_oauth_device_flow
    \/ WitnessInjectNext_release_drains_oauth_device_flow


auth_release_oauth_flow_drain_protocol_covered == TRUE
auth_lease_lifecycle_publication_protocol_covered == TRUE

NoOpenObligationsOnTerminal_auth_machine_auth_release_oauth_flow_drain == (auth_machine_phase = "Released") => obligation_auth_machine_auth_release_oauth_flow_drain = {}
OwnerFeedbackHasProtocolProvenance ==
    \A input_packet \in observed_inputs :
        input_packet.source_kind /= "owner"
        \/ ((/\ input_packet.machine = "auth_machine" /\ input_packet.variant = "ExpireOAuthBrowserFlow" /\ input_packet.source_machine = "auth_machine" /\ input_packet.source_effect = "CancelOAuthFlowsForRelease" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "auth_machine" /\ effect_packet.variant = "CancelOAuthFlowsForRelease" /\ effect_packet.effect_id = input_packet.effect_id) \/ (/\ input_packet.machine = "auth_machine" /\ input_packet.variant = "ExpireOAuthDeviceFlow" /\ input_packet.source_machine = "auth_machine" /\ input_packet.source_effect = "CancelOAuthFlowsForRelease" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "auth_machine" /\ effect_packet.variant = "CancelOAuthFlowsForRelease" /\ effect_packet.effect_id = input_packet.effect_id))

CoverageInstrumentation == TRUE

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 0
DeepStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 2 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 2 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 2
WitnessStateConstraint_freshness_expiry == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 3 /\ Cardinality(observed_inputs) <= 5 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 4 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 0
WitnessStateConstraint_expiring_refresh == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 4 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 5 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 0 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 0 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 0
WitnessStateConstraint_release_drains_oauth_flow == /\ model_step_count <= 9 /\ Len(pending_inputs) <= 4 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 7 /\ Cardinality(observed_transitions) <= 9 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 2 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 2 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 2
WitnessStateConstraint_release_drains_oauth_device_flow == /\ model_step_count <= 9 /\ Len(pending_inputs) <= 4 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 0 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 7 /\ Cardinality(observed_transitions) <= 9 /\ Cardinality(auth_machine_oauth_browser_flow_ids) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_providers) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_redirect_uris) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_browser_flow_expires_at_millis) <= 2 /\ Cardinality(auth_machine_oauth_device_flow_ids) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_providers) <= 2 /\ Cardinality(DOMAIN auth_machine_oauth_device_flow_expires_at_millis) <= 2 /\ Cardinality(auth_machine_oauth_device_poll_ids) <= 2

Spec ==
    /\ Init
    /\ [][Next]_vars

WitnessSpec_freshness_expiry ==
    /\ WitnessInit_freshness_expiry
    /\ [] [WitnessNext_freshness_expiry]_vars

WitnessSpec_expiring_refresh ==
    /\ WitnessInit_expiring_refresh
    /\ [] [WitnessNext_expiring_refresh]_vars

WitnessSpec_release_drains_oauth_flow ==
    /\ WitnessInit_release_drains_oauth_flow
    /\ [] [WitnessNext_release_drains_oauth_flow]_vars

WitnessSpec_release_drains_oauth_device_flow ==
    /\ WitnessInit_release_drains_oauth_device_flow
    /\ [] [WitnessNext_release_drains_oauth_device_flow]_vars

WitnessStateObserved_freshness_expiry_1 == WitnessScriptComplete_freshness_expiry => (auth_machine_phase = "Expired")
WitnessTransitionObserved_freshness_expiry_auth_machine_Acquire == WitnessScriptComplete_freshness_expiry => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
WitnessTransitionObserved_freshness_expiry_auth_machine_ObserveCredentialFreshnessValid == WitnessScriptComplete_freshness_expiry => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessValid")
WitnessTransitionObserved_freshness_expiry_auth_machine_ObserveCredentialFreshnessExpiredFromValid == WitnessScriptComplete_freshness_expiry => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessExpiredFromValid")
WitnessTransitionOrder_freshness_expiry_1 == WitnessScriptComplete_freshness_expiry => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ObserveCredentialFreshnessValid" /\ later.machine = "auth_machine" /\ later.transition = "ObserveCredentialFreshnessExpiredFromValid" /\ earlier.step < later.step)
WitnessStateObserved_expiring_refresh_1 == WitnessScriptComplete_expiring_refresh => (auth_machine_phase = "Valid")
WitnessTransitionObserved_expiring_refresh_auth_machine_Acquire == WitnessScriptComplete_expiring_refresh => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
WitnessTransitionObserved_expiring_refresh_auth_machine_ObserveCredentialFreshnessExpiringFromValid == WitnessScriptComplete_expiring_refresh => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ObserveCredentialFreshnessExpiringFromValid")
WitnessTransitionObserved_expiring_refresh_auth_machine_BeginRefreshFromExpiring == WitnessScriptComplete_expiring_refresh => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginRefreshFromExpiring")
WitnessTransitionObserved_expiring_refresh_auth_machine_CompleteRefresh == WitnessScriptComplete_expiring_refresh => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "CompleteRefresh")
WitnessTransitionOrder_expiring_refresh_1 == WitnessScriptComplete_expiring_refresh => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ObserveCredentialFreshnessExpiringFromValid" /\ later.machine = "auth_machine" /\ later.transition = "BeginRefreshFromExpiring" /\ earlier.step < later.step)
WitnessTransitionOrder_expiring_refresh_2 == WitnessScriptComplete_expiring_refresh => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginRefreshFromExpiring" /\ later.machine = "auth_machine" /\ later.transition = "CompleteRefresh" /\ earlier.step < later.step)
WitnessStateObserved_release_drains_oauth_flow_1 == WitnessScriptComplete_release_drains_oauth_flow => (auth_machine_phase = "Released")
WitnessTransitionObserved_release_drains_oauth_flow_auth_machine_Acquire == WitnessScriptComplete_release_drains_oauth_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
WitnessTransitionObserved_release_drains_oauth_flow_auth_machine_AdmitOAuthBrowserFlowValid == WitnessScriptComplete_release_drains_oauth_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "AdmitOAuthBrowserFlowValid")
WitnessTransitionObserved_release_drains_oauth_flow_auth_machine_BeginReleaseDrainingOAuthFlowsValid == WitnessScriptComplete_release_drains_oauth_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginReleaseDrainingOAuthFlowsValid")
WitnessTransitionObserved_release_drains_oauth_flow_auth_machine_ExpireOAuthBrowserFlowValid == WitnessScriptComplete_release_drains_oauth_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ExpireOAuthBrowserFlowValid")
WitnessTransitionObserved_release_drains_oauth_flow_auth_machine_Release == WitnessScriptComplete_release_drains_oauth_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Release")
WitnessTransitionOrder_release_drains_oauth_flow_1 == WitnessScriptComplete_release_drains_oauth_flow => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginReleaseDrainingOAuthFlowsValid" /\ later.machine = "auth_machine" /\ later.transition = "ExpireOAuthBrowserFlowValid" /\ earlier.step < later.step)
WitnessTransitionOrder_release_drains_oauth_flow_2 == WitnessScriptComplete_release_drains_oauth_flow => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ExpireOAuthBrowserFlowValid" /\ later.machine = "auth_machine" /\ later.transition = "Release" /\ earlier.step < later.step)
WitnessStateObserved_release_drains_oauth_device_flow_1 == WitnessScriptComplete_release_drains_oauth_device_flow => (auth_machine_phase = "Released")
WitnessTransitionObserved_release_drains_oauth_device_flow_auth_machine_Acquire == WitnessScriptComplete_release_drains_oauth_device_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Acquire")
WitnessTransitionObserved_release_drains_oauth_device_flow_auth_machine_AdmitOAuthDeviceFlowValid == WitnessScriptComplete_release_drains_oauth_device_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "AdmitOAuthDeviceFlowValid")
WitnessTransitionObserved_release_drains_oauth_device_flow_auth_machine_BeginReleaseDrainingOAuthFlowsValid == WitnessScriptComplete_release_drains_oauth_device_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "BeginReleaseDrainingOAuthFlowsValid")
WitnessTransitionObserved_release_drains_oauth_device_flow_auth_machine_ExpireOAuthDeviceFlowValid == WitnessScriptComplete_release_drains_oauth_device_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "ExpireOAuthDeviceFlowValid")
WitnessTransitionObserved_release_drains_oauth_device_flow_auth_machine_Release == WitnessScriptComplete_release_drains_oauth_device_flow => (\E packet \in observed_transitions : /\ packet.machine = "auth_machine" /\ packet.transition = "Release")
WitnessTransitionOrder_release_drains_oauth_device_flow_1 == WitnessScriptComplete_release_drains_oauth_device_flow => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "BeginReleaseDrainingOAuthFlowsValid" /\ later.machine = "auth_machine" /\ later.transition = "ExpireOAuthDeviceFlowValid" /\ earlier.step < later.step)
WitnessTransitionOrder_release_drains_oauth_device_flow_2 == WitnessScriptComplete_release_drains_oauth_device_flow => (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "auth_machine" /\ earlier.transition = "ExpireOAuthDeviceFlowValid" /\ later.machine = "auth_machine" /\ later.transition = "Release" /\ earlier.step < later.step)

THEOREM Spec => []auth_release_oauth_flow_drain_protocol_covered
THEOREM Spec => []auth_lease_lifecycle_publication_protocol_covered
THEOREM Spec => []auth_machine_oauth_flow_membership_consistent
THEOREM Spec => []auth_machine_released_oauth_membership_drained
THEOREM Spec => []auth_machine_released_not_release_draining
THEOREM Spec => []NoOpenObligationsOnTerminal_auth_machine_auth_release_oauth_flow_drain
THEOREM Spec => []OwnerFeedbackHasProtocolProvenance

=============================================================================
