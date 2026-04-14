---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MeerkatMachine.

CONSTANTS AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, InputIdValues, NatValues, PeerReachabilityReasonValues, PeerReachabilityValues, ReachabilityKeyValues, RunIdValues, SessionIdValues, SetOfReachabilityKeyValues, SetOfStringValues, StringValues, ToolFilterValues, ToolVisibilityWitnessValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionGenerationValues == {None} \cup {Some(x) : x \in GenerationValues}
OptionPeerReachabilityReasonValues == {None} \cup {Some(x) : x \in PeerReachabilityReasonValues}
OptionSessionIdValues == {None} \cup {Some(x) : x \in SessionIdValues}
OptionWorkIdValues == {None} \cup {Some(x) : x \in WorkIdValues}
MapReachabilityKeyOptionPeerReachabilityReasonValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in ReachabilityKeyValues, v \in OptionPeerReachabilityReasonValues }
MapReachabilityKeyPeerReachabilityValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in ReachabilityKeyValues, v \in PeerReachabilityValues }
MapStringToolVisibilityWitnessValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ToolVisibilityWitnessValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision

vars == << phase, model_step_count, session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>

FilterWitnessKeys == DOMAIN filter_witnesses
RequestedWitnessKeys == DOMAIN requested_witnesses
HasPendingVisibilityPromotion == (staged_visibility_revision > active_visibility_revision)

Init ==
    /\ phase = "Initializing"
    /\ model_step_count = 0
    /\ session_id = None
    /\ active_runtime_id = None
    /\ active_fence_token = None
    /\ active_generation = None
    /\ active_work_id = None
    /\ wake_pending = FALSE
    /\ process_pending = FALSE
    /\ peer_ingress_configured = FALSE
    /\ drain_running = FALSE
    /\ resolved_peer_keys = {}
    /\ peer_reachability = [x \in {} |-> None]
    /\ peer_last_reason = [x \in {} |-> None]
    /\ interrupt_pending = FALSE
    /\ shutdown_pending = FALSE
    /\ inherited_base_filter = "All"
    /\ active_filter = "All"
    /\ staged_filter = "All"
    /\ active_requested_deferred_names = {}
    /\ staged_requested_deferred_names = {}
    /\ requested_witnesses = [x \in {} |-> None]
    /\ filter_witnesses = [x \in {} |-> None]
    /\ active_visibility_revision = 0
    /\ staged_visibility_revision = 0
    /\ committed_visibility_revision = 0

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

RECURSIVE StagePersistentFilterAttached_ForEach0_filter_witnesses(_, _, _)
StagePersistentFilterAttached_ForEach0_filter_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN StagePersistentFilterAttached_ForEach0_filter_witnesses(next_acc, remaining \ {item}, outer_witnesses)

RECURSIVE StagePersistentFilterRunning_ForEach1_filter_witnesses(_, _, _)
StagePersistentFilterRunning_ForEach1_filter_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN StagePersistentFilterRunning_ForEach1_filter_witnesses(next_acc, remaining \ {item}, outer_witnesses)

RECURSIVE RequestDeferredToolsAttached_ForEach2_staged_requested_deferred_names(_, _)
RequestDeferredToolsAttached_ForEach2_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN RequestDeferredToolsAttached_ForEach2_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE RequestDeferredToolsAttached_ForEach3_requested_witnesses(_, _, _)
RequestDeferredToolsAttached_ForEach3_requested_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN RequestDeferredToolsAttached_ForEach3_requested_witnesses(next_acc, remaining \ {item}, outer_witnesses)

RECURSIVE RequestDeferredToolsRunning_ForEach4_staged_requested_deferred_names(_, _)
RequestDeferredToolsRunning_ForEach4_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN RequestDeferredToolsRunning_ForEach4_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE RequestDeferredToolsRunning_ForEach5_requested_witnesses(_, _, _)
RequestDeferredToolsRunning_ForEach5_requested_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN RequestDeferredToolsRunning_ForEach5_requested_witnesses(next_acc, remaining \ {item}, outer_witnesses)

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RegisterSession(arg_session_id) ==
    /\ phase = "Idle" \/ phase = "Stopped" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


UnregisterSession(arg_session_id) ==
    /\ phase = "Idle" \/ phase = "Stopped" \/ phase = "Retired"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ active_work_id' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE
    /\ peer_ingress_configured' = FALSE
    /\ drain_running' = FALSE
    /\ resolved_peer_keys' = {}
    /\ peer_reachability' = [x \in {} |-> None]
    /\ peer_last_reason' = [x \in {} |-> None]
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ inherited_base_filter' = "All"
    /\ active_filter' = "All"
    /\ staged_filter' = "All"
    /\ active_requested_deferred_names' = {}
    /\ staged_requested_deferred_names' = {}
    /\ requested_witnesses' = [x \in {} |-> None]
    /\ filter_witnesses' = [x \in {} |-> None]
    /\ active_visibility_revision' = 0
    /\ staged_visibility_revision' = 0
    /\ committed_visibility_revision' = 0


StagePersistentFilterAttached(filter, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ staged_filter' = filter
    /\ filter_witnesses' = StagePersistentFilterAttached_ForEach0_filter_witnesses(filter_witnesses, DOMAIN witnesses, witnesses)
    /\ staged_visibility_revision' = (staged_visibility_revision) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, active_visibility_revision, committed_visibility_revision >>


StagePersistentFilterRunning(filter, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ staged_filter' = filter
    /\ filter_witnesses' = StagePersistentFilterRunning_ForEach1_filter_witnesses(filter_witnesses, DOMAIN witnesses, witnesses)
    /\ staged_visibility_revision' = (staged_visibility_revision) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, active_visibility_revision, committed_visibility_revision >>


RequestDeferredToolsAttached(names, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = RequestDeferredToolsAttached_ForEach2_staged_requested_deferred_names(staged_requested_deferred_names, names)
    /\ requested_witnesses' = RequestDeferredToolsAttached_ForEach3_requested_witnesses(requested_witnesses, DOMAIN witnesses, witnesses)
    /\ staged_visibility_revision' = (staged_visibility_revision) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, filter_witnesses, active_visibility_revision, committed_visibility_revision >>


RequestDeferredToolsRunning(names, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = RequestDeferredToolsRunning_ForEach4_staged_requested_deferred_names(staged_requested_deferred_names, names)
    /\ requested_witnesses' = RequestDeferredToolsRunning_ForEach5_requested_witnesses(requested_witnesses, DOMAIN witnesses, witnesses)
    /\ staged_visibility_revision' = (staged_visibility_revision) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, filter_witnesses, active_visibility_revision, committed_visibility_revision >>


PrepareBindings(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Idle" \/ phase = "Stopped" \/ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ active_generation' = Some(generation)
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SetPeerIngressContextAttached(keep_alive) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_configured' = TRUE
    /\ drain_running' = keep_alive
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SetPeerIngressContextRunning(keep_alive) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_configured' = TRUE
    /\ drain_running' = keep_alive
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


NotifyDrainExitedAttached(reason) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


NotifyDrainExitedRunning(reason) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ReconcileResolvedDirectoryAttached(keys, reachability, last_reason) ==
    /\ phase = "Attached"
    /\ (\A k \in DOMAIN reachability : (k \in keys))
    /\ (\A k \in DOMAIN last_reason : (k \in keys))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ resolved_peer_keys' = keys
    /\ peer_reachability' = reachability
    /\ peer_last_reason' = last_reason
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ReconcileResolvedDirectoryRunning(keys, reachability, last_reason) ==
    /\ phase = "Running"
    /\ (\A k \in DOMAIN reachability : (k \in keys))
    /\ (\A k \in DOMAIN last_reason : (k \in keys))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ resolved_peer_keys' = keys
    /\ peer_reachability' = reachability
    /\ peer_last_reason' = last_reason
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecordSendSucceededAttached(key) ==
    /\ phase = "Attached"
    /\ (key \in resolved_peer_keys)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_reachability' = MapSet(peer_reachability, key, "Reachable")
    /\ peer_last_reason' = MapSet(peer_last_reason, key, None)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecordSendSucceededRunning(key) ==
    /\ phase = "Running"
    /\ (key \in resolved_peer_keys)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_reachability' = MapSet(peer_reachability, key, "Reachable")
    /\ peer_last_reason' = MapSet(peer_last_reason, key, None)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecordSendFailedAttached(key, reason) ==
    /\ phase = "Attached"
    /\ (key \in resolved_peer_keys)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_reachability' = MapSet(peer_reachability, key, "Unreachable")
    /\ peer_last_reason' = MapSet(peer_last_reason, key, Some(reason))
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecordSendFailedRunning(key, reason) ==
    /\ phase = "Running"
    /\ (key \in resolved_peer_keys)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_reachability' = MapSet(peer_reachability, key, "Unreachable")
    /\ peer_last_reason' = MapSet(peer_last_reason, key, Some(reason))
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


BeginRunFromIdle(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = Some(work_id)
    /\ wake_pending' = TRUE
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


InterruptCurrentRun ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = TRUE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CancelAfterBoundary ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ shutdown_pending' = TRUE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


BoundaryAppliedPromote(revision) ==
    /\ phase = "Running"
    /\ HasPendingVisibilityPromotion
    /\ (staged_visibility_revision = revision)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_filter' = staged_filter
    /\ active_requested_deferred_names' = staged_requested_deferred_names
    /\ active_visibility_revision' = staged_visibility_revision
    /\ committed_visibility_revision' = revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, staged_filter, staged_requested_deferred_names, requested_witnesses, filter_witnesses, staged_visibility_revision >>


BoundaryAppliedNoop(revision) ==
    /\ phase = "Running"
    /\ ~(HasPendingVisibilityPromotion)
    /\ (revision <= active_visibility_revision)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ committed_visibility_revision' = revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision >>


PublishCommittedVisibleSetAttached(revision) ==
    /\ phase = "Attached"
    /\ (active_visibility_revision = revision)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ committed_visibility_revision' = revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision >>


PublishCommittedVisibleSetRunning(revision) ==
    /\ phase = "Running"
    /\ (active_visibility_revision = revision)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ committed_visibility_revision' = revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision >>


RunCompleted(work_id) ==
    /\ phase = "Running"
    /\ (active_work_id = Some(work_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RunFailed(work_id) ==
    /\ phase = "Running"
    /\ (active_work_id = Some(work_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RunCancelled(work_id) ==
    /\ phase = "Running"
    /\ (active_work_id = Some(work_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecoverFromIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecoverFromAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RetireRequestedFromIdle ==
    /\ phase = "Idle" \/ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


Reset ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Recovering" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ active_work_id' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE
    /\ drain_running' = FALSE
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StopRuntimeExecutor ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Recovering" \/ phase = "Retired"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ drain_running' = FALSE
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


Destroy ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Recovering" \/ phase = "Retired" \/ phase = "Stopped"
    /\ (active_runtime_id # None)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE
    /\ drain_running' = FALSE
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SetSilentIntentsIdle(arg_session_id, intents) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ContainsSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SessionHasExecutorIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SessionHasCommsIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


OpsLifecycleRegistryIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


InputStateIdle(arg_session_id, input_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ListActiveInputsIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AbortAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AbortRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


WaitAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


WaitRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AbortAllAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AbortAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AbortAllRecovering ==
    /\ phase = "Recovering"
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AbortAllRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AbortAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


EnsureDrainRunningAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (peer_ingress_configured = TRUE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = TRUE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


EnsureDrainRunningRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (peer_ingress_configured = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ drain_running' = TRUE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


IngestAttached(runtime_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


IngestRunning(runtime_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PublishEventAttached(kind) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PublishEventRunning(kind) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AcceptWithCompletionAttached(input_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AcceptWithCompletionRunning(input_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AcceptWithoutWakeAttached(input_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AcceptWithoutWakeRunning(input_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ClassifyExternalEnvelopeAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ClassifyExternalEnvelopeRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ClassifyPlainEventAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ClassifyPlainEventRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RuntimeStateAttached(runtime_id) ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RuntimeStateRunning(runtime_id) ==
    /\ phase = "Running"
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


LoadBoundaryReceiptAttached(runtime_id, sequence) ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


LoadBoundaryReceiptRunning(runtime_id, sequence) ==
    /\ phase = "Running"
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PrepareAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StartConversationRunAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StartImmediateAppendAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StartImmediateContextAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CommitRunning(input_id, run_id) ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


FailRunning(run_id) ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AdmitQueuedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AdmitConsumedOnAcceptRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StageDrainSnapshotRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SupersedeQueuedInputRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CoalesceQueuedInputsRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SetSilentIntentOverridesRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PrimitiveAppliedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


LlmReturnedToolCallsRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


LlmReturnedTerminalRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RegisterPendingOpsRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ToolCallsResolvedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


OpsBarrierSatisfiedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


BoundaryContinueRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


BoundaryCompleteRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecoverableFailureRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


FatalFailureRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RetryRequestedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CancelNowRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CancellationObservedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AcknowledgeTerminalRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


TurnLimitReachedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


BudgetExhaustedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


TimeBudgetExceededRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


EnterExtractionRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ExtractionValidationPassedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ExtractionValidationFailedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ExtractionStartRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ForceCancelNoRunRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RegisterOperationRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ProvisioningSucceededRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ProvisioningFailedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


AbortProvisioningRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PeerReadyRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RegisterWatcherRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ProgressReportedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CompleteOperationRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


FailOperationRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CancelOperationRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RetireRequestedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RetireCompletedRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CollectTerminalRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


BeginWaitAllRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CancelWaitAllRunning ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StageAddAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StageAddRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StageRemoveAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StageRemoveRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StageReloadAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


StageReloadRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ApplySurfaceBoundaryAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ApplySurfaceBoundaryRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PendingSucceededAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PendingSucceededRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PendingFailedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


PendingFailedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CallStartedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CallStartedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CallFinishedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


CallFinishedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


FinalizeRemovalCleanAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


FinalizeRemovalCleanRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


FinalizeRemovalForcedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


FinalizeRemovalForcedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SnapshotAlignedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


SnapshotAlignedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ShutdownSurfaceAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


ShutdownSurfaceRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, peer_ingress_configured, drain_running, resolved_peer_keys, peer_reachability, peer_last_reason, interrupt_pending, shutdown_pending, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecycleFromIdleOrRetired ==
    /\ phase = "Idle" \/ phase = "Retired"
    /\ (active_runtime_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ active_work_id' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE
    /\ peer_ingress_configured' = FALSE
    /\ drain_running' = FALSE
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


RecycleFromAttached ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ active_work_id' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE
    /\ peer_ingress_configured' = FALSE
    /\ drain_running' = FALSE
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << session_id, resolved_peer_keys, peer_reachability, peer_last_reason, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_visibility_revision, staged_visibility_revision, committed_visibility_revision >>


Next ==
    \/ Initialize
    \/ \E arg_session_id \in SessionIdValues : RegisterSession(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : UnregisterSession(arg_session_id)
    \/ \E filter \in ToolFilterValues : \E witnesses \in MapStringToolVisibilityWitnessValues : StagePersistentFilterAttached(filter, witnesses)
    \/ \E filter \in ToolFilterValues : \E witnesses \in MapStringToolVisibilityWitnessValues : StagePersistentFilterRunning(filter, witnesses)
    \/ \E names \in SetOfStringValues : \E witnesses \in MapStringToolVisibilityWitnessValues : RequestDeferredToolsAttached(names, witnesses)
    \/ \E names \in SetOfStringValues : \E witnesses \in MapStringToolVisibilityWitnessValues : RequestDeferredToolsRunning(names, witnesses)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : PrepareBindings(agent_runtime_id, fence_token, generation)
    \/ \E keep_alive \in BOOLEAN : SetPeerIngressContextAttached(keep_alive)
    \/ \E keep_alive \in BOOLEAN : SetPeerIngressContextRunning(keep_alive)
    \/ \E reason \in StringValues : NotifyDrainExitedAttached(reason)
    \/ \E reason \in StringValues : NotifyDrainExitedRunning(reason)
    \/ \E keys \in SetOfReachabilityKeyValues : \E reachability \in MapReachabilityKeyPeerReachabilityValues : \E last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : ReconcileResolvedDirectoryAttached(keys, reachability, last_reason)
    \/ \E keys \in SetOfReachabilityKeyValues : \E reachability \in MapReachabilityKeyPeerReachabilityValues : \E last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : ReconcileResolvedDirectoryRunning(keys, reachability, last_reason)
    \/ \E key \in ReachabilityKeyValues : RecordSendSucceededAttached(key)
    \/ \E key \in ReachabilityKeyValues : RecordSendSucceededRunning(key)
    \/ \E key \in ReachabilityKeyValues : \E reason \in PeerReachabilityReasonValues : RecordSendFailedAttached(key, reason)
    \/ \E key \in ReachabilityKeyValues : \E reason \in PeerReachabilityReasonValues : RecordSendFailedRunning(key, reason)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : BeginRunFromIdle(agent_runtime_id, fence_token, work_id)
    \/ InterruptCurrentRun
    \/ CancelAfterBoundary
    \/ \E revision \in 0..2 : BoundaryAppliedPromote(revision)
    \/ \E revision \in 0..2 : BoundaryAppliedNoop(revision)
    \/ \E revision \in 0..2 : PublishCommittedVisibleSetAttached(revision)
    \/ \E revision \in 0..2 : PublishCommittedVisibleSetRunning(revision)
    \/ \E work_id \in WorkIdValues : RunCompleted(work_id)
    \/ \E work_id \in WorkIdValues : RunFailed(work_id)
    \/ \E work_id \in WorkIdValues : RunCancelled(work_id)
    \/ RecoverFromIdle
    \/ RecoverFromAttached
    \/ RetireRequestedFromIdle
    \/ Reset
    \/ StopRuntimeExecutor
    \/ Destroy
    \/ \E arg_session_id \in SessionIdValues : EnsureSessionWithExecutorIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : \E intents \in SetOfStringValues : SetSilentIntentsIdle(arg_session_id, intents)
    \/ \E arg_session_id \in SessionIdValues : ContainsSessionIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : SessionHasExecutorIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : SessionHasCommsIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : OpsLifecycleRegistryIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : \E input_id \in InputIdValues : InputStateIdle(arg_session_id, input_id)
    \/ \E arg_session_id \in SessionIdValues : ListActiveInputsIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : AbortAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : AbortRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : WaitAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : WaitRunning(arg_session_id)
    \/ AbortAllAttached
    \/ AbortAllRunning
    \/ AbortAllRecovering
    \/ AbortAllRetired
    \/ AbortAllStopped
    \/ EnsureDrainRunningAttached
    \/ EnsureDrainRunningRunning
    \/ \E runtime_id \in StringValues : IngestAttached(runtime_id)
    \/ \E runtime_id \in StringValues : IngestRunning(runtime_id)
    \/ \E kind \in StringValues : PublishEventAttached(kind)
    \/ \E kind \in StringValues : PublishEventRunning(kind)
    \/ \E input_id \in InputIdValues : AcceptWithCompletionAttached(input_id)
    \/ \E input_id \in InputIdValues : AcceptWithCompletionRunning(input_id)
    \/ \E input_id \in InputIdValues : AcceptWithoutWakeAttached(input_id)
    \/ \E input_id \in InputIdValues : AcceptWithoutWakeRunning(input_id)
    \/ ClassifyExternalEnvelopeAttached
    \/ ClassifyExternalEnvelopeRunning
    \/ ClassifyPlainEventAttached
    \/ ClassifyPlainEventRunning
    \/ \E runtime_id \in StringValues : RuntimeStateAttached(runtime_id)
    \/ \E runtime_id \in StringValues : RuntimeStateRunning(runtime_id)
    \/ \E runtime_id \in StringValues : \E sequence \in 0..2 : LoadBoundaryReceiptAttached(runtime_id, sequence)
    \/ \E runtime_id \in StringValues : \E sequence \in 0..2 : LoadBoundaryReceiptRunning(runtime_id, sequence)
    \/ \E arg_session_id \in SessionIdValues : PrepareAttached(arg_session_id)
    \/ StartConversationRunAttached
    \/ StartImmediateAppendAttached
    \/ StartImmediateContextAttached
    \/ \E input_id \in InputIdValues : \E run_id \in RunIdValues : CommitRunning(input_id, run_id)
    \/ \E run_id \in RunIdValues : FailRunning(run_id)
    \/ AdmitQueuedRunning
    \/ AdmitConsumedOnAcceptRunning
    \/ StageDrainSnapshotRunning
    \/ SupersedeQueuedInputRunning
    \/ CoalesceQueuedInputsRunning
    \/ SetSilentIntentOverridesRunning
    \/ PrimitiveAppliedRunning
    \/ LlmReturnedToolCallsRunning
    \/ LlmReturnedTerminalRunning
    \/ RegisterPendingOpsRunning
    \/ ToolCallsResolvedRunning
    \/ OpsBarrierSatisfiedRunning
    \/ BoundaryContinueRunning
    \/ BoundaryCompleteRunning
    \/ RecoverableFailureRunning
    \/ FatalFailureRunning
    \/ RetryRequestedRunning
    \/ CancelNowRunning
    \/ CancellationObservedRunning
    \/ AcknowledgeTerminalRunning
    \/ TurnLimitReachedRunning
    \/ BudgetExhaustedRunning
    \/ TimeBudgetExceededRunning
    \/ EnterExtractionRunning
    \/ ExtractionValidationPassedRunning
    \/ ExtractionValidationFailedRunning
    \/ ExtractionStartRunning
    \/ ForceCancelNoRunRunning
    \/ RegisterOperationRunning
    \/ ProvisioningSucceededRunning
    \/ ProvisioningFailedRunning
    \/ AbortProvisioningRunning
    \/ PeerReadyRunning
    \/ RegisterWatcherRunning
    \/ ProgressReportedRunning
    \/ CompleteOperationRunning
    \/ FailOperationRunning
    \/ CancelOperationRunning
    \/ RetireRequestedRunning
    \/ RetireCompletedRunning
    \/ CollectTerminalRunning
    \/ BeginWaitAllRunning
    \/ CancelWaitAllRunning
    \/ StageAddAttached
    \/ StageAddRunning
    \/ StageRemoveAttached
    \/ StageRemoveRunning
    \/ StageReloadAttached
    \/ StageReloadRunning
    \/ ApplySurfaceBoundaryAttached
    \/ ApplySurfaceBoundaryRunning
    \/ PendingSucceededAttached
    \/ PendingSucceededRunning
    \/ PendingFailedAttached
    \/ PendingFailedRunning
    \/ CallStartedAttached
    \/ CallStartedRunning
    \/ CallFinishedAttached
    \/ CallFinishedRunning
    \/ FinalizeRemovalCleanAttached
    \/ FinalizeRemovalCleanRunning
    \/ FinalizeRemovalForcedAttached
    \/ FinalizeRemovalForcedRunning
    \/ SnapshotAlignedAttached
    \/ SnapshotAlignedRunning
    \/ ShutdownSurfaceAttached
    \/ ShutdownSurfaceRunning
    \/ RecycleFromIdleOrRetired
    \/ RecycleFromAttached
    \/ TerminalStutter

running_has_active_work == ((phase # "Running") \/ (active_work_id # None))
bound_runtime_has_fence == ((active_runtime_id = None) \/ (active_fence_token # None))
destroyed_has_no_active_work == ((phase # "Destroyed") \/ (active_work_id = None))
interrupt_pending_only_while_active == ((interrupt_pending = FALSE) \/ (phase = "Running"))
drain_requires_ingress_context == ((drain_running = FALSE) \/ (peer_ingress_configured = TRUE))
peer_reachability_keys_are_resolved == (\A key \in DOMAIN peer_reachability : (key \in resolved_peer_keys))
peer_last_reason_keys_are_resolved == (\A key \in DOMAIN peer_last_reason : (key \in resolved_peer_keys))
active_visibility_revision_not_ahead_of_staged == (active_visibility_revision <= staged_visibility_revision)
active_requested_names_subset_of_staged == (\A name \in active_requested_deferred_names : (name \in staged_requested_deferred_names))
equal_visibility_revision_means_equal_active_and_staged_state == ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
committed_visibility_not_ahead_of_active == (committed_visibility_revision <= active_visibility_revision)

CiStateConstraint == /\ model_step_count <= 8 /\ Cardinality(resolved_peer_keys) <= 1 /\ Cardinality(DOMAIN peer_reachability) <= 1 /\ Cardinality(DOMAIN peer_last_reason) <= 1 /\ Cardinality(active_requested_deferred_names) <= 1 /\ Cardinality(staged_requested_deferred_names) <= 1 /\ Cardinality(DOMAIN requested_witnesses) <= 1 /\ Cardinality(DOMAIN filter_witnesses) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(resolved_peer_keys) <= 2 /\ Cardinality(DOMAIN peer_reachability) <= 2 /\ Cardinality(DOMAIN peer_last_reason) <= 2 /\ Cardinality(active_requested_deferred_names) <= 2 /\ Cardinality(staged_requested_deferred_names) <= 2 /\ Cardinality(DOMAIN requested_witnesses) <= 2 /\ Cardinality(DOMAIN filter_witnesses) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []running_has_active_work
THEOREM Spec => []bound_runtime_has_fence
THEOREM Spec => []destroyed_has_no_active_work
THEOREM Spec => []interrupt_pending_only_while_active
THEOREM Spec => []drain_requires_ingress_context
THEOREM Spec => []peer_reachability_keys_are_resolved
THEOREM Spec => []peer_last_reason_keys_are_resolved
THEOREM Spec => []active_visibility_revision_not_ahead_of_staged
THEOREM Spec => []active_requested_names_subset_of_staged
THEOREM Spec => []equal_visibility_revision_means_equal_active_and_staged_state
THEOREM Spec => []committed_visibility_not_ahead_of_active

=============================================================================
