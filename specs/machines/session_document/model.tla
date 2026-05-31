---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for SessionDocumentMachine.

CONSTANTS BooleanValues, NatValues, SessionFirstTurnPhaseValues, SessionIdValues, SessionInitialPromptStageDecisionValues, SystemContextAppendDecisionValues, SystemContextSourceValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapSessionIdBoolValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in BOOLEAN }
MapSessionIdSessionFirstTurnPhaseValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in SessionFirstTurnPhaseValues }
MapSessionIdU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in SessionIdValues, v \in NatValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
Count(seq, value) == Cardinality({i \in DOMAIN seq : seq[i] = value})
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count

vars == << phase, model_step_count, session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>

append_is_new(idempotency_key_present, existing_key_matches, existing_key_conflicts) == ((idempotency_key_present = FALSE) \/ ((existing_key_matches = FALSE) /\ (existing_key_conflicts = FALSE)))
append_is_duplicate(idempotency_key_present, existing_key_matches, existing_key_conflicts) == (idempotency_key_present /\ existing_key_matches /\ (existing_key_conflicts = FALSE))
append_is_conflict(idempotency_key_present, existing_key_conflicts) == (idempotency_key_present /\ existing_key_conflicts)
append_is_empty(trimmed_text_byte_count) == (trimmed_text_byte_count = 0)
should_store_initial_prompt(arg_phase, prompt_has_content) == ((arg_phase = "Pending") /\ prompt_has_content)
phase_allows_initial_turn_overrides(arg_phase) == (arg_phase = "Pending")

Init ==
    /\ phase = "Ready"
    /\ model_step_count = 0
    /\ session_first_turn_phase = [x \in {} |-> None]
    /\ session_pending_initial_prompt_present = [x \in {} |-> None]
    /\ session_pending_tool_results_count = [x \in {} |-> None]

MarkSessionInitialTurnPendingInactiveOrPending(session_id) ==
    /\ phase = "Ready"
    /\ (((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") \/ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ UNCHANGED << session_pending_initial_prompt_present, session_pending_tool_results_count >>


MarkSessionInitialTurnPendingConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


StartSessionInitialTurnPending(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ UNCHANGED << session_pending_initial_prompt_present, session_pending_tool_results_count >>


StartSessionInitialTurnInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


StartSessionInitialTurnConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSessionFirstTurnOverridesAllowed(session_id) ==
    /\ phase = "Ready"
    /\ phase_allows_initial_turn_overrides((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSessionFirstTurnOverridesDenied(session_id) ==
    /\ phase = "Ready"
    /\ (phase_allows_initial_turn_overrides((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None)) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


StageSessionInitialPromptStore(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ should_store_initial_prompt((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None), prompt_has_content)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, TRUE)
    /\ UNCHANGED << session_first_turn_phase, session_pending_tool_results_count >>


StageSessionInitialPromptClear(session_id, prompt_has_content) ==
    /\ phase = "Ready"
    /\ (should_store_initial_prompt((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None), prompt_has_content) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ UNCHANGED << session_first_turn_phase, session_pending_tool_results_count >>


StageSessionToolResults(session_id, result_count) ==
    /\ phase = "Ready"
    /\ (((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive") \/ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending") \/ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, result_count)
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present >>


ConsumeSessionDeferredInputsPending(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Pending")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Consumed")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)


ConsumeSessionDeferredInputsInactive(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Inactive")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UNCHANGED << session_first_turn_phase >>


ConsumeSessionDeferredInputsConsumed(session_id) ==
    /\ phase = "Ready"
    /\ ((IF "value" \in DOMAIN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None) THEN (IF (session_id \in DOMAIN session_first_turn_phase) THEN Some((IF session_id \in DOMAIN session_first_turn_phase THEN session_first_turn_phase[session_id] ELSE "None")) ELSE None)["value"] ELSE None) = "Consumed")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, FALSE)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, 0)
    /\ UNCHANGED << session_first_turn_phase >>


RestoreSessionConsumedInputs(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ restore_first_turn_pending
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, "Pending")
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)


RestoreSessionConsumedInputsNoPhaseRollback(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ (restore_first_turn_pending = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)
    /\ UNCHANGED << session_first_turn_phase >>


RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count) ==
    /\ phase = "Ready"
    /\ ((arg_phase = "Inactive") \/ (arg_phase = "Pending") \/ (arg_phase = "Consumed"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ session_first_turn_phase' = MapSet(session_first_turn_phase, session_id, arg_phase)
    /\ session_pending_initial_prompt_present' = MapSet(session_pending_initial_prompt_present, session_id, pending_initial_prompt_present)
    /\ session_pending_tool_results_count' = MapSet(session_pending_tool_results_count, session_id, pending_tool_result_message_count)


ResolveSystemContextAppendEmpty(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ append_is_empty(trimmed_text_byte_count)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextAppendConflict(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_conflict(idempotency_key_present, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextAppendDuplicate(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_duplicate(idempotency_key_present, existing_key_matches, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextAppendNew(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped) ==
    /\ phase = "Ready"
    /\ ((append_is_empty(trimmed_text_byte_count) = FALSE) /\ append_is_new(idempotency_key_present, existing_key_matches, existing_key_conflicts))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextPendingApplyItemRuntimeSteer(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "RuntimeSteer")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextPendingApplyItemNormal(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "Normal")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextSteerCleanupItemRuntimeSteer(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "RuntimeSteer")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


ResolveSystemContextSteerCleanupItemNormal(source_kind) ==
    /\ phase = "Ready"
    /\ (source_kind = "Normal")
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


RestoreSystemContextSnapshot(active_keys_have_known_pending_or_seen, seen_keys_match_known_appends) ==
    /\ phase = "Ready"
    /\ (active_keys_have_known_pending_or_seen /\ seen_keys_match_known_appends)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_first_turn_phase, session_pending_initial_prompt_present, session_pending_tool_results_count >>


Next ==
    \/ \E session_id \in SessionIdValues : MarkSessionInitialTurnPendingInactiveOrPending(session_id)
    \/ \E session_id \in SessionIdValues : MarkSessionInitialTurnPendingConsumed(session_id)
    \/ \E session_id \in SessionIdValues : StartSessionInitialTurnPending(session_id)
    \/ \E session_id \in SessionIdValues : StartSessionInitialTurnInactive(session_id)
    \/ \E session_id \in SessionIdValues : StartSessionInitialTurnConsumed(session_id)
    \/ \E session_id \in SessionIdValues : ResolveSessionFirstTurnOverridesAllowed(session_id)
    \/ \E session_id \in SessionIdValues : ResolveSessionFirstTurnOverridesDenied(session_id)
    \/ \E session_id \in SessionIdValues : \E prompt_has_content \in BOOLEAN : StageSessionInitialPromptStore(session_id, prompt_has_content)
    \/ \E session_id \in SessionIdValues : \E prompt_has_content \in BOOLEAN : StageSessionInitialPromptClear(session_id, prompt_has_content)
    \/ \E session_id \in SessionIdValues : \E result_count \in 0..2 : StageSessionToolResults(session_id, result_count)
    \/ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsPending(session_id)
    \/ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsInactive(session_id)
    \/ \E session_id \in SessionIdValues : ConsumeSessionDeferredInputsConsumed(session_id)
    \/ \E session_id \in SessionIdValues : \E restore_first_turn_pending \in BOOLEAN : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RestoreSessionConsumedInputs(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E session_id \in SessionIdValues : \E restore_first_turn_pending \in BOOLEAN : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RestoreSessionConsumedInputsNoPhaseRollback(session_id, restore_first_turn_pending, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E session_id \in SessionIdValues : \E arg_phase \in SessionFirstTurnPhaseValues : \E pending_initial_prompt_present \in BOOLEAN : \E pending_tool_result_message_count \in 0..2 : RecoverSessionFirstTurnPhase(session_id, arg_phase, pending_initial_prompt_present, pending_tool_result_message_count)
    \/ \E trimmed_text_byte_count \in 0..2 : \E idempotency_key_present \in BOOLEAN : \E existing_key_matches \in BOOLEAN : \E existing_key_conflicts \in BOOLEAN : \E active_turn_scoped \in BOOLEAN : ResolveSystemContextAppendEmpty(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
    \/ \E trimmed_text_byte_count \in 0..2 : \E idempotency_key_present \in BOOLEAN : \E existing_key_matches \in BOOLEAN : \E existing_key_conflicts \in BOOLEAN : \E active_turn_scoped \in BOOLEAN : ResolveSystemContextAppendConflict(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
    \/ \E trimmed_text_byte_count \in 0..2 : \E idempotency_key_present \in BOOLEAN : \E existing_key_matches \in BOOLEAN : \E existing_key_conflicts \in BOOLEAN : \E active_turn_scoped \in BOOLEAN : ResolveSystemContextAppendDuplicate(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
    \/ \E trimmed_text_byte_count \in 0..2 : \E idempotency_key_present \in BOOLEAN : \E existing_key_matches \in BOOLEAN : \E existing_key_conflicts \in BOOLEAN : \E active_turn_scoped \in BOOLEAN : ResolveSystemContextAppendNew(trimmed_text_byte_count, idempotency_key_present, existing_key_matches, existing_key_conflicts, active_turn_scoped)
    \/ \E source_kind \in SystemContextSourceValues : ResolveSystemContextPendingApplyItemRuntimeSteer(source_kind)
    \/ \E source_kind \in SystemContextSourceValues : ResolveSystemContextPendingApplyItemNormal(source_kind)
    \/ \E source_kind \in SystemContextSourceValues : ResolveSystemContextSteerCleanupItemRuntimeSteer(source_kind)
    \/ \E source_kind \in SystemContextSourceValues : ResolveSystemContextSteerCleanupItemNormal(source_kind)
    \/ \E active_keys_have_known_pending_or_seen \in BOOLEAN : \E seen_keys_match_known_appends \in BOOLEAN : RestoreSystemContextSnapshot(active_keys_have_known_pending_or_seen, seen_keys_match_known_appends)


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(DOMAIN session_first_turn_phase) <= 1 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 1 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN session_first_turn_phase) <= 2 /\ Cardinality(DOMAIN session_pending_initial_prompt_present) <= 2 /\ Cardinality(DOMAIN session_pending_tool_results_count) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
