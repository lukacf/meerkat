---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for job_runtime_delivery.

\* RustU64Max is the TLA boundary for Expr::U64Max; production generated Rust renders u64::MAX.
CONSTANTS BooleanValues, DetachedJobRestartClassValues, DetachedJobTerminalKindValues, NatValues, SetOfStringValues, SetOfU64Values, StringValues, RustU64Max

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in StringValues }
MapStringU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in NatValues }
OptionDetachedJobTerminalKindValues == {None} \cup {Some(x) : x \in DetachedJobTerminalKindValues}
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
    <<"job", "DetachedJobMachine", "job_authority">>,
    <<"runtime_delivery", "RuntimeDeliveryMachine", "runtime_delivery_authority">>
}

RouteNames == {
    "job_terminal_enters_runtime_inbox",
    "job_notification_enters_runtime_inbox",
    "runtime_delivery_commit_acknowledges_job_outbox",
    "runtime_delivery_reuse_acknowledges_job_outbox"
}

Actors == {
    "job_authority",
    "runtime_delivery_authority"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "job" -> "job_authority"
      [] machine_id = "runtime_delivery" -> "runtime_delivery_authority"

RouteSource(route_name) ==
    CASE route_name = "job_terminal_enters_runtime_inbox" -> "job"
      [] route_name = "job_notification_enters_runtime_inbox" -> "job"
      [] route_name = "runtime_delivery_commit_acknowledges_job_outbox" -> "runtime_delivery"
      [] route_name = "runtime_delivery_reuse_acknowledges_job_outbox" -> "runtime_delivery"

RouteEffect(route_name) ==
    CASE route_name = "job_terminal_enters_runtime_inbox" -> "TerminalCommitted"
      [] route_name = "job_notification_enters_runtime_inbox" -> "NotificationCommitted"
      [] route_name = "runtime_delivery_commit_acknowledges_job_outbox" -> "DeliveryCommitted"
      [] route_name = "runtime_delivery_reuse_acknowledges_job_outbox" -> "DeliveryReused"

RouteTargetMachine(route_name) ==
    CASE route_name = "job_terminal_enters_runtime_inbox" -> "runtime_delivery"
      [] route_name = "job_notification_enters_runtime_inbox" -> "runtime_delivery"
      [] route_name = "runtime_delivery_commit_acknowledges_job_outbox" -> "job"
      [] route_name = "runtime_delivery_reuse_acknowledges_job_outbox" -> "job"

RouteTargetInput(route_name) ==
    CASE route_name = "job_terminal_enters_runtime_inbox" -> "CommitDelivery"
      [] route_name = "job_notification_enters_runtime_inbox" -> "CommitDelivery"
      [] route_name = "runtime_delivery_commit_acknowledges_job_outbox" -> "MarkDeliveryApplied"
      [] route_name = "runtime_delivery_reuse_acknowledges_job_outbox" -> "MarkDeliveryApplied"

RouteTargetKind(route_name) ==
    CASE route_name = "job_terminal_enters_runtime_inbox" -> "Input"
      [] route_name = "job_notification_enters_runtime_inbox" -> "Input"
      [] route_name = "runtime_delivery_commit_acknowledges_job_outbox" -> "Input"
      [] route_name = "runtime_delivery_reuse_acknowledges_job_outbox" -> "Input"
      [] OTHER -> "Unknown"

RouteDeliveryKind(route_name) ==
    CASE route_name = "job_terminal_enters_runtime_inbox" -> "Enqueue"
      [] route_name = "job_notification_enters_runtime_inbox" -> "Enqueue"
      [] route_name = "runtime_delivery_commit_acknowledges_job_outbox" -> "Enqueue"
      [] route_name = "runtime_delivery_reuse_acknowledges_job_outbox" -> "Enqueue"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ job_phase = "Unsubmitted"
    /\ job_job_id = ""
    /\ job_restart_class = "NonResumable"
    /\ job_attempt_count = 0
    /\ job_current_attempt_id = None
    /\ job_current_fence = 0
    /\ job_current_worker_id = None
    /\ job_lease_expires_at_ms = None
    /\ job_heartbeat_at_ms = None
    /\ job_checkpoint_ref = None
    /\ job_runner_handle = None
    /\ job_progress_cursor = 0
    /\ job_lease_expired = FALSE
    /\ job_retry_due_at_ms = None
    /\ job_cancel_requested = FALSE
    /\ job_delivery_sequence = 0
    /\ job_notification_ids = {}
    /\ job_notification_idempotency_keys = {}
    /\ job_notification_id_by_key = [x \in {} |-> None]
    /\ job_notification_delivery_ids = [x \in {} |-> None]
    /\ job_notification_sequences = [x \in {} |-> None]
    /\ job_notification_applied = {}
    /\ job_terminal_kind = None
    /\ job_terminal_delivery_sequence = 0
    /\ job_terminal_delivery_applied = FALSE
    /\ runtime_delivery_phase = "Active"
    /\ runtime_delivery_delivery_ids = {}
    /\ runtime_delivery_delivery_sequences = [x \in {} |-> None]
    /\ runtime_delivery_delivery_source_sequences = [x \in {} |-> None]
    /\ runtime_delivery_committed_sequences = {}
    /\ runtime_delivery_next_sequence = 0
    /\ runtime_delivery_applied_cursor = 0
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

WitnessInit_runtime_delivery_first_commit ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "Adoptable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_first_commit:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "Adoptable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_first_commit:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "Adoptable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_first_commit:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "job", variant |-> "ClaimAttempt", payload |-> [attempt_id |-> "attempt_1", claimed_at_ms |-> 1, lease_expires_at_ms |-> 2, runner_handle |-> "runner_1", worker_id |-> "worker_1"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_first_commit:2", source_machine |-> "external_entry", source_effect |-> "ClaimAttempt", effect_id |-> 0], [machine |-> "job", variant |-> "CompleteAttempt", payload |-> [attempt_id |-> "attempt_1", completed_at_ms |-> 2, fence |-> 1], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_first_commit:3", source_machine |-> "external_entry", source_effect |-> "CompleteAttempt", effect_id |-> 0]>>

WitnessInit_runtime_delivery_notification_commit ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "CheckpointResumable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_notification_commit:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "CheckpointResumable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_notification_commit:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "CheckpointResumable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_notification_commit:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "job", variant |-> "ClaimAttempt", payload |-> [attempt_id |-> "attempt_1", claimed_at_ms |-> 1, lease_expires_at_ms |-> 2, runner_handle |-> "runner_1", worker_id |-> "worker_1"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_notification_commit:2", source_machine |-> "external_entry", source_effect |-> "ClaimAttempt", effect_id |-> 0], [machine |-> "job", variant |-> "EmitNotification", payload |-> [attempt_id |-> "attempt_1", fence |-> 1, idempotency_key |-> "key_1", notification_id |-> "notification_1", observed_at_ms |-> 2, runtime_delivery_id |-> "job_1:notification:notification_1"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_notification_commit:3", source_machine |-> "external_entry", source_effect |-> "EmitNotification", effect_id |-> 0]>>

WitnessInit_runtime_delivery_crash_retry_reuse ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "Adoptable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_crash_retry_reuse:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "Adoptable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_crash_retry_reuse:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "job", variant |-> "Submit", payload |-> [job_id |-> "job_1", restart_class |-> "Adoptable"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_crash_retry_reuse:1", source_machine |-> "external_entry", source_effect |-> "Submit", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "job", variant |-> "ClaimAttempt", payload |-> [attempt_id |-> "attempt_1", claimed_at_ms |-> 1, lease_expires_at_ms |-> 2, runner_handle |-> "runner_1", worker_id |-> "worker_1"], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_crash_retry_reuse:2", source_machine |-> "external_entry", source_effect |-> "ClaimAttempt", effect_id |-> 0], [machine |-> "job", variant |-> "CompleteAttempt", payload |-> [attempt_id |-> "attempt_1", completed_at_ms |-> 2, fence |-> 1], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_crash_retry_reuse:3", source_machine |-> "external_entry", source_effect |-> "CompleteAttempt", effect_id |-> 0], [machine |-> "runtime_delivery", variant |-> "CommitDelivery", payload |-> [delivery_id |-> "job_1", source_sequence |-> 1], source_kind |-> "entry", source_route |-> "witness:runtime_delivery_crash_retry_reuse:4", source_machine |-> "external_entry", source_effect |-> "CommitDelivery", effect_id |-> 0]>>

job_SubmitQueued(arg_job_id, arg_restart_class) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "Submit"
       /\ packet.payload.job_id = arg_job_id
       /\ packet.payload.restart_class = arg_restart_class
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Unsubmitted"
       /\ (packet.payload.job_id # "")
       /\ job_phase' = "Queued"
       /\ job_job_id' = packet.payload.job_id
       /\ job_restart_class' = packet.payload.restart_class
       /\ UNCHANGED << job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "JobSubmitted", payload |-> [job_id |-> packet.payload.job_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitQueued"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "SubmitQueued", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Queued"]}
       /\ model_step_count' = model_step_count + 1


job_ClaimQueued(arg_attempt_id, arg_worker_id, arg_claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "ClaimAttempt"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.worker_id = arg_worker_id
       /\ packet.payload.claimed_at_ms = arg_claimed_at_ms
       /\ packet.payload.lease_expires_at_ms = arg_lease_expires_at_ms
       /\ packet.payload.runner_handle = arg_runner_handle
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Queued"
       /\ ((packet.payload.attempt_id # "") /\ (packet.payload.worker_id # "") /\ (packet.payload.runner_handle # "") /\ (packet.payload.lease_expires_at_ms > packet.payload.claimed_at_ms))
       /\ job_phase' = "Running"
       /\ job_attempt_count' = (job_attempt_count) + 1
       /\ job_current_attempt_id' = Some(packet.payload.attempt_id)
       /\ job_current_fence' = (job_current_fence) + 1
       /\ job_current_worker_id' = Some(packet.payload.worker_id)
       /\ job_lease_expires_at_ms' = Some(packet.payload.lease_expires_at_ms)
       /\ job_heartbeat_at_ms' = Some(packet.payload.claimed_at_ms)
       /\ job_runner_handle' = Some(packet.payload.runner_handle)
       /\ job_lease_expired' = FALSE
       /\ job_retry_due_at_ms' = None
       /\ job_cancel_requested' = FALSE
       /\ UNCHANGED << job_job_id, job_restart_class, job_checkpoint_ref, job_progress_cursor, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "AttemptClaimed", payload |-> [attempt_count |-> (job_attempt_count) + 1, attempt_id |-> packet.payload.attempt_id, fence |-> (job_current_fence) + 1, lease_expires_at_ms |-> packet.payload.lease_expires_at_ms, resume_checkpoint |-> job_checkpoint_ref], effect_id |-> (model_step_count + 1), source_transition |-> "ClaimQueued"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ClaimQueued", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_ClaimRetryScheduled(arg_attempt_id, arg_worker_id, arg_claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "ClaimAttempt"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.worker_id = arg_worker_id
       /\ packet.payload.claimed_at_ms = arg_claimed_at_ms
       /\ packet.payload.lease_expires_at_ms = arg_lease_expires_at_ms
       /\ packet.payload.runner_handle = arg_runner_handle
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "RetryScheduled"
       /\ ((packet.payload.attempt_id # "") /\ (packet.payload.worker_id # "") /\ (packet.payload.runner_handle # "") /\ (job_retry_due_at_ms # None) /\ (packet.payload.claimed_at_ms >= (IF "value" \in DOMAIN job_retry_due_at_ms THEN job_retry_due_at_ms["value"] ELSE None)) /\ (packet.payload.lease_expires_at_ms > packet.payload.claimed_at_ms) /\ (packet.payload.attempt_id # (IF "value" \in DOMAIN job_current_attempt_id THEN job_current_attempt_id["value"] ELSE None)))
       /\ job_phase' = "Running"
       /\ job_attempt_count' = (job_attempt_count) + 1
       /\ job_current_attempt_id' = Some(packet.payload.attempt_id)
       /\ job_current_fence' = (job_current_fence) + 1
       /\ job_current_worker_id' = Some(packet.payload.worker_id)
       /\ job_lease_expires_at_ms' = Some(packet.payload.lease_expires_at_ms)
       /\ job_heartbeat_at_ms' = Some(packet.payload.claimed_at_ms)
       /\ job_runner_handle' = Some(packet.payload.runner_handle)
       /\ job_lease_expired' = FALSE
       /\ job_retry_due_at_ms' = None
       /\ job_cancel_requested' = FALSE
       /\ UNCHANGED << job_job_id, job_restart_class, job_checkpoint_ref, job_progress_cursor, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "AttemptClaimed", payload |-> [attempt_count |-> (job_attempt_count) + 1, attempt_id |-> packet.payload.attempt_id, fence |-> (job_current_fence) + 1, lease_expires_at_ms |-> packet.payload.lease_expires_at_ms, resume_checkpoint |-> job_checkpoint_ref], effect_id |-> (model_step_count + 1), source_transition |-> "ClaimRetryScheduled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ClaimRetryScheduled", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_RenewRunningLease(arg_attempt_id, arg_fence, arg_heartbeat_at_ms, arg_lease_expires_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RenewLease"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.heartbeat_at_ms = arg_heartbeat_at_ms
       /\ packet.payload.lease_expires_at_ms = arg_lease_expires_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (job_heartbeat_at_ms # None) /\ (packet.payload.heartbeat_at_ms > (IF "value" \in DOMAIN job_heartbeat_at_ms THEN job_heartbeat_at_ms["value"] ELSE None)) /\ (packet.payload.heartbeat_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.lease_expires_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "Running"
       /\ job_lease_expires_at_ms' = Some(packet.payload.lease_expires_at_ms)
       /\ job_heartbeat_at_ms' = Some(packet.payload.heartbeat_at_ms)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "LeaseRenewed", payload |-> [lease_expires_at_ms |-> packet.payload.lease_expires_at_ms], effect_id |-> (model_step_count + 1), source_transition |-> "RenewRunningLease"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RenewRunningLease", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_RenewExternalWaitLease(arg_attempt_id, arg_fence, arg_heartbeat_at_ms, arg_lease_expires_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RenewLease"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.heartbeat_at_ms = arg_heartbeat_at_ms
       /\ packet.payload.lease_expires_at_ms = arg_lease_expires_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (job_heartbeat_at_ms # None) /\ (packet.payload.heartbeat_at_ms > (IF "value" \in DOMAIN job_heartbeat_at_ms THEN job_heartbeat_at_ms["value"] ELSE None)) /\ (packet.payload.heartbeat_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.lease_expires_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "WaitingExternal"
       /\ job_lease_expires_at_ms' = Some(packet.payload.lease_expires_at_ms)
       /\ job_heartbeat_at_ms' = Some(packet.payload.heartbeat_at_ms)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "LeaseRenewed", payload |-> [lease_expires_at_ms |-> packet.payload.lease_expires_at_ms], effect_id |-> (model_step_count + 1), source_transition |-> "RenewExternalWaitLease"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RenewExternalWaitLease", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_ReportRunningProgress(arg_attempt_id, arg_fence, arg_cursor, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "ReportProgress"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.cursor = arg_cursor
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.cursor > job_progress_cursor))
       /\ job_phase' = "Running"
       /\ job_progress_cursor' = packet.payload.cursor
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "ProgressAccepted", payload |-> [cursor |-> packet.payload.cursor], effect_id |-> (model_step_count + 1), source_transition |-> "ReportRunningProgress"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ReportRunningProgress", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_ReportExternalWaitProgress(arg_attempt_id, arg_fence, arg_cursor, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "ReportProgress"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.cursor = arg_cursor
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.cursor > job_progress_cursor))
       /\ job_phase' = "WaitingExternal"
       /\ job_progress_cursor' = packet.payload.cursor
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "ProgressAccepted", payload |-> [cursor |-> packet.payload.cursor], effect_id |-> (model_step_count + 1), source_transition |-> "ReportExternalWaitProgress"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ReportExternalWaitProgress", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_EmitRunningNotification(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "EmitNotification"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.notification_id = arg_notification_id
       /\ packet.payload.idempotency_key = arg_idempotency_key
       /\ packet.payload.runtime_delivery_id = arg_runtime_delivery_id
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.notification_id # "") /\ (packet.payload.idempotency_key # "") /\ (packet.payload.runtime_delivery_id # "") /\ ((packet.payload.notification_id \in job_notification_ids) = FALSE) /\ ((packet.payload.idempotency_key \in job_notification_idempotency_keys) = FALSE) /\ (job_delivery_sequence < RustU64Max))
       /\ job_phase' = "Running"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_notification_ids' = (job_notification_ids \cup {packet.payload.notification_id})
       /\ job_notification_idempotency_keys' = (job_notification_idempotency_keys \cup {packet.payload.idempotency_key})
       /\ job_notification_id_by_key' = MapSet(job_notification_id_by_key, packet.payload.idempotency_key, packet.payload.notification_id)
       /\ job_notification_delivery_ids' = MapSet(job_notification_delivery_ids, packet.payload.notification_id, packet.payload.runtime_delivery_id)
       /\ job_notification_sequences' = MapSet(job_notification_sequences, packet.payload.notification_id, (job_delivery_sequence) + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_notification_enters_runtime_inbox", source_machine |-> "job", effect |-> "NotificationCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> packet.payload.runtime_delivery_id, source_sequence |-> (job_delivery_sequence) + 1], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "EmitRunningNotification"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "NotificationCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence) + 1, idempotency_key |-> packet.payload.idempotency_key, notification_id |-> packet.payload.notification_id, runtime_delivery_id |-> packet.payload.runtime_delivery_id], effect_id |-> (model_step_count + 1), source_transition |-> "EmitRunningNotification"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "EmitRunningNotification", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_EmitExternalWaitNotification(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "EmitNotification"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.notification_id = arg_notification_id
       /\ packet.payload.idempotency_key = arg_idempotency_key
       /\ packet.payload.runtime_delivery_id = arg_runtime_delivery_id
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.notification_id # "") /\ (packet.payload.idempotency_key # "") /\ (packet.payload.runtime_delivery_id # "") /\ ((packet.payload.notification_id \in job_notification_ids) = FALSE) /\ ((packet.payload.idempotency_key \in job_notification_idempotency_keys) = FALSE) /\ (job_delivery_sequence < RustU64Max))
       /\ job_phase' = "WaitingExternal"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_notification_ids' = (job_notification_ids \cup {packet.payload.notification_id})
       /\ job_notification_idempotency_keys' = (job_notification_idempotency_keys \cup {packet.payload.idempotency_key})
       /\ job_notification_id_by_key' = MapSet(job_notification_id_by_key, packet.payload.idempotency_key, packet.payload.notification_id)
       /\ job_notification_delivery_ids' = MapSet(job_notification_delivery_ids, packet.payload.notification_id, packet.payload.runtime_delivery_id)
       /\ job_notification_sequences' = MapSet(job_notification_sequences, packet.payload.notification_id, (job_delivery_sequence) + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_notification_enters_runtime_inbox", source_machine |-> "job", effect |-> "NotificationCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> packet.payload.runtime_delivery_id, source_sequence |-> (job_delivery_sequence) + 1], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "EmitExternalWaitNotification"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "NotificationCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence) + 1, idempotency_key |-> packet.payload.idempotency_key, notification_id |-> packet.payload.notification_id, runtime_delivery_id |-> packet.payload.runtime_delivery_id], effect_id |-> (model_step_count + 1), source_transition |-> "EmitExternalWaitNotification"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "EmitExternalWaitNotification", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_SuppressRunningNotificationReplay(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "EmitNotification"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.notification_id = arg_notification_id
       /\ packet.payload.idempotency_key = arg_idempotency_key
       /\ packet.payload.runtime_delivery_id = arg_runtime_delivery_id
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.notification_id # "") /\ (packet.payload.runtime_delivery_id # "") /\ (packet.payload.idempotency_key \in job_notification_idempotency_keys))
       /\ job_phase' = "Running"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "NotificationSuppressed", payload |-> [delivery_sequence |-> (IF "value" \in DOMAIN (IF ((IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_sequences) THEN Some((IF (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_sequences THEN job_notification_sequences[(IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None)] ELSE 0)) ELSE None) THEN (IF ((IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_sequences) THEN Some((IF (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_sequences THEN job_notification_sequences[(IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None)] ELSE 0)) ELSE None)["value"] ELSE None), idempotency_key |-> packet.payload.idempotency_key, notification_id |-> (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None), runtime_delivery_id |-> (IF "value" \in DOMAIN (IF ((IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_delivery_ids) THEN Some((IF (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_delivery_ids THEN job_notification_delivery_ids[(IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None)] ELSE "None")) ELSE None) THEN (IF ((IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_delivery_ids) THEN Some((IF (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_delivery_ids THEN job_notification_delivery_ids[(IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None)] ELSE "None")) ELSE None)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "SuppressRunningNotificationReplay"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "SuppressRunningNotificationReplay", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_SuppressExternalWaitNotificationReplay(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "EmitNotification"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.notification_id = arg_notification_id
       /\ packet.payload.idempotency_key = arg_idempotency_key
       /\ packet.payload.runtime_delivery_id = arg_runtime_delivery_id
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.notification_id # "") /\ (packet.payload.runtime_delivery_id # "") /\ (packet.payload.idempotency_key \in job_notification_idempotency_keys))
       /\ job_phase' = "WaitingExternal"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "NotificationSuppressed", payload |-> [delivery_sequence |-> (IF "value" \in DOMAIN (IF ((IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_sequences) THEN Some((IF (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_sequences THEN job_notification_sequences[(IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None)] ELSE 0)) ELSE None) THEN (IF ((IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_sequences) THEN Some((IF (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_sequences THEN job_notification_sequences[(IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None)] ELSE 0)) ELSE None)["value"] ELSE None), idempotency_key |-> packet.payload.idempotency_key, notification_id |-> (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None), runtime_delivery_id |-> (IF "value" \in DOMAIN (IF ((IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_delivery_ids) THEN Some((IF (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_delivery_ids THEN job_notification_delivery_ids[(IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None)] ELSE "None")) ELSE None) THEN (IF ((IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_delivery_ids) THEN Some((IF (IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None) \in DOMAIN job_notification_delivery_ids THEN job_notification_delivery_ids[(IF "value" \in DOMAIN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None) THEN (IF (packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key) THEN Some((IF packet.payload.idempotency_key \in DOMAIN job_notification_id_by_key THEN job_notification_id_by_key[packet.payload.idempotency_key] ELSE "None")) ELSE None)["value"] ELSE None)] ELSE "None")) ELSE None)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "SuppressExternalWaitNotificationReplay"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "SuppressExternalWaitNotificationReplay", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_RecordRunningCheckpoint(arg_attempt_id, arg_fence, arg_checkpoint_ref, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RecordCheckpoint"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.checkpoint_ref = arg_checkpoint_ref
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.checkpoint_ref # ""))
       /\ job_phase' = "Running"
       /\ job_checkpoint_ref' = Some(packet.payload.checkpoint_ref)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "CheckpointAccepted", payload |-> [checkpoint_ref |-> packet.payload.checkpoint_ref], effect_id |-> (model_step_count + 1), source_transition |-> "RecordRunningCheckpoint"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RecordRunningCheckpoint", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_RecordExternalWaitCheckpoint(arg_attempt_id, arg_fence, arg_checkpoint_ref, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RecordCheckpoint"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.checkpoint_ref = arg_checkpoint_ref
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.checkpoint_ref # ""))
       /\ job_phase' = "WaitingExternal"
       /\ job_checkpoint_ref' = Some(packet.payload.checkpoint_ref)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "CheckpointAccepted", payload |-> [checkpoint_ref |-> packet.payload.checkpoint_ref], effect_id |-> (model_step_count + 1), source_transition |-> "RecordExternalWaitCheckpoint"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RecordExternalWaitCheckpoint", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_WaitExternalFromRunning(arg_attempt_id, arg_fence, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "WaitExternal"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "WaitingExternal"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "ExternalWaitAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "WaitExternalFromRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "WaitExternalFromRunning", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_ResumeRunningFromExternal(arg_attempt_id, arg_fence, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "ResumeRunning"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "Running"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "RunningResumed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ResumeRunningFromExternal"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ResumeRunningFromExternal", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_RequestCancelRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RequestCancel"
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ (job_cancel_requested = FALSE)
       /\ job_phase' = "Running"
       /\ job_cancel_requested' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "CancelRequested", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RequestCancelRunning", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_RequestCancelWaitingExternal ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RequestCancel"
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ (job_cancel_requested = FALSE)
       /\ job_phase' = "WaitingExternal"
       /\ job_cancel_requested' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "CancelRequested", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelWaitingExternal"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RequestCancelWaitingExternal", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_RequestCancelAlreadyRequestedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RequestCancel"
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ job_cancel_requested
       /\ job_phase' = "Running"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "CancelRequested", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelAlreadyRequestedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RequestCancelAlreadyRequestedRunning", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_RequestCancelAlreadyRequestedWaitingExternal ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RequestCancel"
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ job_cancel_requested
       /\ job_phase' = "WaitingExternal"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "CancelRequested", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelAlreadyRequestedWaitingExternal"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RequestCancelAlreadyRequestedWaitingExternal", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_RequestCancelAlreadyCancelled ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RequestCancel"
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Cancelled"
       /\ job_phase' = "Cancelled"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "CancelRequested", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelAlreadyCancelled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RequestCancelAlreadyCancelled", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_RequestCancelQueued ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RequestCancel"
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Queued"
       /\ job_phase' = "Cancelled"
       /\ job_cancel_requested' = TRUE
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Cancelled")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelQueued"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelQueued"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RequestCancelQueued", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_RequestCancelRetryScheduled ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RequestCancel"
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "RetryScheduled"
       /\ job_phase' = "Cancelled"
       /\ job_cancel_requested' = TRUE
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Cancelled")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelRetryScheduled"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelRetryScheduled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RequestCancelRetryScheduled", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_RequestCancelLossObserved ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "RequestCancel"
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "LossObserved"
       /\ job_phase' = "Cancelled"
       /\ job_cancel_requested' = TRUE
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Cancelled")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelLossObserved"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "RequestCancelLossObserved"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "RequestCancelLossObserved", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_LeaseExpiresRunning(arg_attempt_id, arg_fence, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "LeaseExpired"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "LossObserved"
       /\ job_lease_expired' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "LeaseExpiryRecorded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiresRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "LeaseExpiresRunning", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "LossObserved"]}
       /\ model_step_count' = model_step_count + 1


job_LeaseExpiresWaitingExternal(arg_attempt_id, arg_fence, arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "LeaseExpired"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "LossObserved"
       /\ job_lease_expired' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "LeaseExpiryRecorded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiresWaitingExternal"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "LeaseExpiresWaitingExternal", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "LossObserved"]}
       /\ model_step_count' = model_step_count + 1


job_ScheduleRetryAfterLoss(arg_retry_due_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "ScheduleRetry"
       /\ packet.payload.retry_due_at_ms = arg_retry_due_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "LossObserved"
       /\ (job_lease_expired /\ (job_restart_class # "NonResumable") /\ (IF (job_restart_class # "CheckpointResumable") THEN TRUE ELSE (job_checkpoint_ref # None)) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.retry_due_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "RetryScheduled"
       /\ job_retry_due_at_ms' = Some(packet.payload.retry_due_at_ms)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "RetryScheduled", payload |-> [retry_due_at_ms |-> packet.payload.retry_due_at_ms], effect_id |-> (model_step_count + 1), source_transition |-> "ScheduleRetryAfterLoss"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ScheduleRetryAfterLoss", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "RetryScheduled"]}
       /\ model_step_count' = model_step_count + 1


job_ClassifyNonResumableWorkerLoss(arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "ClassifyWorkerLoss"
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "LossObserved"
       /\ (job_lease_expired /\ (job_restart_class = "NonResumable") /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "WorkerLost"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("WorkerLost")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyNonResumableWorkerLoss"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "WorkerLost"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyNonResumableWorkerLoss"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ClassifyNonResumableWorkerLoss", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WorkerLost"]}
       /\ model_step_count' = model_step_count + 1


job_CompleteRunningAttempt(arg_attempt_id, arg_fence, arg_completed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "CompleteAttempt"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.completed_at_ms = arg_completed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.completed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "Succeeded"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Succeeded")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CompleteRunningAttempt"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Succeeded"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteRunningAttempt"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "CompleteRunningAttempt", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Succeeded"]}
       /\ model_step_count' = model_step_count + 1


job_CompleteWaitingExternalAttempt(arg_attempt_id, arg_fence, arg_completed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "CompleteAttempt"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.completed_at_ms = arg_completed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.completed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "Succeeded"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Succeeded")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CompleteWaitingExternalAttempt"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Succeeded"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteWaitingExternalAttempt"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "CompleteWaitingExternalAttempt", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Succeeded"]}
       /\ model_step_count' = model_step_count + 1


job_FailRunningAttempt(arg_attempt_id, arg_fence, arg_failed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "FailAttempt"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.failed_at_ms = arg_failed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.failed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "Failed"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Failed")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "FailRunningAttempt"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "FailRunningAttempt"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "FailRunningAttempt", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


job_FailWaitingExternalAttempt(arg_attempt_id, arg_fence, arg_failed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "FailAttempt"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.failed_at_ms = arg_failed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.failed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "Failed"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Failed")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "FailWaitingExternalAttempt"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Failed"], effect_id |-> (model_step_count + 1), source_transition |-> "FailWaitingExternalAttempt"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "FailWaitingExternalAttempt", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


job_AcknowledgeRunningCancel(arg_attempt_id, arg_fence, arg_acknowledged_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "AcknowledgeCancel"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.acknowledged_at_ms = arg_acknowledged_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ (job_cancel_requested /\ (job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.acknowledged_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "Cancelled"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Cancelled")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "AcknowledgeRunningCancel"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "AcknowledgeRunningCancel"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "AcknowledgeRunningCancel", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_AcknowledgeWaitingExternalCancel(arg_attempt_id, arg_fence, arg_acknowledged_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "AcknowledgeCancel"
       /\ packet.payload.attempt_id = arg_attempt_id
       /\ packet.payload.fence = arg_fence
       /\ packet.payload.acknowledged_at_ms = arg_acknowledged_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ (job_cancel_requested /\ (job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.acknowledged_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)))
       /\ job_phase' = "Cancelled"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("Cancelled")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "AcknowledgeWaitingExternalCancel"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "Cancelled"], effect_id |-> (model_step_count + 1), source_transition |-> "AcknowledgeWaitingExternalCancel"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "AcknowledgeWaitingExternalCancel", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_MarkQueuedNeedsAttention(arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkNeedsAttention"
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Queued"
       /\ (packet.payload.observed_at_ms > 0)
       /\ job_phase' = "NeedsAttention"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("NeedsAttention")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "MarkQueuedNeedsAttention"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "NeedsAttention"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkQueuedNeedsAttention"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "MarkQueuedNeedsAttention", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_MarkRunningNeedsAttention(arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkNeedsAttention"
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ (packet.payload.observed_at_ms > 0)
       /\ job_phase' = "NeedsAttention"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("NeedsAttention")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "MarkRunningNeedsAttention"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "NeedsAttention"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkRunningNeedsAttention"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "MarkRunningNeedsAttention", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_MarkWaitingExternalNeedsAttention(arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkNeedsAttention"
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ (packet.payload.observed_at_ms > 0)
       /\ job_phase' = "NeedsAttention"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("NeedsAttention")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "MarkWaitingExternalNeedsAttention"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "NeedsAttention"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkWaitingExternalNeedsAttention"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "MarkWaitingExternalNeedsAttention", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_MarkLossObservedNeedsAttention(arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkNeedsAttention"
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "LossObserved"
       /\ (packet.payload.observed_at_ms > 0)
       /\ job_phase' = "NeedsAttention"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("NeedsAttention")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "MarkLossObservedNeedsAttention"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "NeedsAttention"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkLossObservedNeedsAttention"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "MarkLossObservedNeedsAttention", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_MarkRetryScheduledNeedsAttention(arg_observed_at_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkNeedsAttention"
       /\ packet.payload.observed_at_ms = arg_observed_at_ms
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "RetryScheduled"
       /\ (packet.payload.observed_at_ms > 0)
       /\ job_phase' = "NeedsAttention"
       /\ job_delivery_sequence' = (job_delivery_sequence) + 1
       /\ job_terminal_kind' = Some("NeedsAttention")
       /\ job_terminal_delivery_sequence' = (job_delivery_sequence + 1)
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "job_terminal_enters_runtime_inbox", source_machine |-> "job", effect |-> "TerminalCommitted", target_machine |-> "runtime_delivery", target_input |-> "CommitDelivery", payload |-> [delivery_id |-> job_job_id, source_sequence |-> (job_delivery_sequence + 1)], actor |-> "runtime_delivery_authority", effect_id |-> (model_step_count + 1), source_transition |-> "MarkRetryScheduledNeedsAttention"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "TerminalCommitted", payload |-> [delivery_sequence |-> (job_delivery_sequence + 1), job_id |-> job_job_id, terminal_kind |-> "NeedsAttention"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkRetryScheduledNeedsAttention"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "MarkRetryScheduledNeedsAttention", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_ApplySucceededDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Succeeded"
       /\ ((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "Succeeded"
       /\ job_terminal_delivery_applied' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplySucceededDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplySucceededDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Succeeded"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyFailedDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Failed"
       /\ ((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "Failed"
       /\ job_terminal_delivery_applied' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyFailedDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyFailedDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyCancelledDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Cancelled"
       /\ ((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "Cancelled"
       /\ job_terminal_delivery_applied' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyCancelledDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyCancelledDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyWorkerLostDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WorkerLost"
       /\ ((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "WorkerLost"
       /\ job_terminal_delivery_applied' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyWorkerLostDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyWorkerLostDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WorkerLost"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyNeedsAttentionDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "NeedsAttention"
       /\ ((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "NeedsAttention"
       /\ job_terminal_delivery_applied' = TRUE
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyNeedsAttentionDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyNeedsAttentionDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveSucceededDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Succeeded"
       /\ ((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "Succeeded"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveSucceededDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveSucceededDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Succeeded"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveFailedDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Failed"
       /\ ((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "Failed"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveFailedDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveFailedDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveCancelledDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Cancelled"
       /\ ((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "Cancelled"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCancelledDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveCancelledDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveWorkerLostDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WorkerLost"
       /\ ((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "WorkerLost"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveWorkerLostDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveWorkerLostDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WorkerLost"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveNeedsAttentionDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "NeedsAttention"
       /\ ((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence))
       /\ job_phase' = "NeedsAttention"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveNeedsAttentionDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveNeedsAttentionDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyRunningNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "Running"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyRunningNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyRunningNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyWaitingExternalNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "WaitingExternal"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyWaitingExternalNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyWaitingExternalNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyLossObservedNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "LossObserved"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "LossObserved"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLossObservedNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyLossObservedNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "LossObserved"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyRetryScheduledNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "RetryScheduled"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "RetryScheduled"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyRetryScheduledNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyRetryScheduledNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "RetryScheduled"]}
       /\ model_step_count' = model_step_count + 1


job_ApplySucceededNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Succeeded"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "Succeeded"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplySucceededNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplySucceededNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Succeeded"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyFailedNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Failed"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "Failed"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyFailedNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyFailedNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyCancelledNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Cancelled"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "Cancelled"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyCancelledNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyCancelledNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyWorkerLostNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WorkerLost"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "WorkerLost"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyWorkerLostNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyWorkerLostNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WorkerLost"]}
       /\ model_step_count' = model_step_count + 1


job_ApplyNeedsAttentionNotificationDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "NeedsAttention"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "NeedsAttention"
       /\ job_notification_applied' = (job_notification_applied \cup {packet.payload.delivery_id})
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyNeedsAttentionNotificationDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ApplyNeedsAttentionNotificationDelivery", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveRunningNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Running"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "Running"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveRunningNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveRunningNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveWaitingExternalNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WaitingExternal"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "WaitingExternal"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveWaitingExternalNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveWaitingExternalNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WaitingExternal"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveLossObservedNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "LossObserved"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "LossObserved"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveLossObservedNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveLossObservedNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "LossObserved"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveRetryScheduledNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "RetryScheduled"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "RetryScheduled"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveRetryScheduledNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveRetryScheduledNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "RetryScheduled"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveSucceededNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Succeeded"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "Succeeded"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveSucceededNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveSucceededNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Succeeded"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveFailedNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Failed"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "Failed"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveFailedNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveFailedNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Failed"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveCancelledNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "Cancelled"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "Cancelled"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveCancelledNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveCancelledNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "Cancelled"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveWorkerLostNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "WorkerLost"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "WorkerLost"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveWorkerLostNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveWorkerLostNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "WorkerLost"]}
       /\ model_step_count' = model_step_count + 1


job_ObserveNeedsAttentionNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "job"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("job_authority")
       /\ job_phase = "NeedsAttention"
       /\ ((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence))
       /\ job_phase' = "NeedsAttention"
       /\ UNCHANGED << job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "job", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveNeedsAttentionNotificationDeliveryAlreadyApplied"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "job", transition |-> "ObserveNeedsAttentionNotificationDeliveryAlreadyApplied", actor |-> "job_authority", step |-> (model_step_count + 1), from_phase |-> job_phase, to_phase |-> "NeedsAttention"]}
       /\ model_step_count' = model_step_count + 1


job_fence_tracks_claim_count == (job_current_fence = job_attempt_count)
job_no_attempt_has_no_attempt_authority == (IF (job_attempt_count # 0) THEN TRUE ELSE ((job_current_attempt_id = None) /\ (job_current_worker_id = None) /\ (job_lease_expires_at_ms = None) /\ (job_heartbeat_at_ms = None) /\ (job_runner_handle = None)))
job_checkpoint_requires_attempt == (IF (job_checkpoint_ref = None) THEN TRUE ELSE (job_attempt_count > 0))
job_active_execution_requires_attempt_authority == (IF ((job_phase # "Running") /\ (job_phase # "WaitingExternal") /\ (job_phase # "LossObserved") /\ (job_phase # "RetryScheduled")) THEN TRUE ELSE ((job_current_attempt_id # None) /\ (job_current_worker_id # None) /\ (job_lease_expires_at_ms # None) /\ (job_runner_handle # None) /\ (job_current_fence > 0)))
job_terminal_requires_delivery == (IF ((job_phase # "Succeeded") /\ (job_phase # "Failed") /\ (job_phase # "Cancelled") /\ (job_phase # "WorkerLost") /\ (job_phase # "NeedsAttention")) THEN TRUE ELSE ((job_terminal_kind # None) /\ (job_terminal_delivery_sequence > 0) /\ (job_terminal_delivery_sequence <= job_delivery_sequence)))
job_notification_identity_and_sequence_cardinality_match == ((Cardinality(job_notification_ids) = Cardinality(job_notification_idempotency_keys)) /\ (Cardinality(job_notification_ids) = Len(job_notification_id_by_key)) /\ (Cardinality(job_notification_ids) = Len(job_notification_delivery_ids)) /\ (Cardinality(job_notification_ids) = Len(job_notification_sequences)))
job_applied_notifications_are_committed == (Cardinality(job_notification_applied) <= Cardinality(job_notification_ids))
job_nonterminal_has_no_terminal_delivery == (IF (job_phase = "Succeeded") THEN TRUE ELSE (IF (job_phase = "Failed") THEN TRUE ELSE (IF (job_phase = "Cancelled") THEN TRUE ELSE (IF (job_phase = "WorkerLost") THEN TRUE ELSE (IF (job_phase = "NeedsAttention") THEN TRUE ELSE ((job_terminal_kind = None) /\ (job_terminal_delivery_sequence = 0) /\ (job_terminal_delivery_applied = FALSE)))))))

runtime_delivery_CommitNewDelivery(arg_delivery_id, arg_source_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_delivery"
       /\ packet.variant = "CommitDelivery"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.source_sequence = arg_source_sequence
       /\ ~HigherPriorityReady("runtime_delivery_authority")
       /\ runtime_delivery_phase = "Active"
       /\ (((packet.payload.delivery_id \in runtime_delivery_delivery_ids) = FALSE) /\ (packet.payload.source_sequence > 0) /\ (runtime_delivery_next_sequence < RustU64Max))
       /\ runtime_delivery_phase' = "Active"
       /\ runtime_delivery_delivery_ids' = (runtime_delivery_delivery_ids \cup {packet.payload.delivery_id})
       /\ runtime_delivery_delivery_sequences' = MapSet(runtime_delivery_delivery_sequences, packet.payload.delivery_id, (runtime_delivery_next_sequence) + 1)
       /\ runtime_delivery_delivery_source_sequences' = MapSet(runtime_delivery_delivery_source_sequences, packet.payload.delivery_id, packet.payload.source_sequence)
       /\ runtime_delivery_committed_sequences' = (runtime_delivery_committed_sequences \cup {(runtime_delivery_next_sequence) + 1})
       /\ runtime_delivery_next_sequence' = (runtime_delivery_next_sequence) + 1
       /\ UNCHANGED << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "runtime_delivery_commit_acknowledges_job_outbox", source_machine |-> "runtime_delivery", effect |-> "DeliveryCommitted", target_machine |-> "job", target_input |-> "MarkDeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.source_sequence], actor |-> "job_authority", effect_id |-> (model_step_count + 1), source_transition |-> "CommitNewDelivery"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_delivery", variant |-> "DeliveryCommitted", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> (runtime_delivery_next_sequence) + 1, source_sequence |-> packet.payload.source_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "CommitNewDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_delivery", transition |-> "CommitNewDelivery", actor |-> "runtime_delivery_authority", step |-> (model_step_count + 1), from_phase |-> runtime_delivery_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_delivery_ReuseCommittedDelivery(arg_delivery_id, arg_source_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_delivery"
       /\ packet.variant = "CommitDelivery"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.source_sequence = arg_source_sequence
       /\ ~HigherPriorityReady("runtime_delivery_authority")
       /\ runtime_delivery_phase = "Active"
       /\ ((packet.payload.delivery_id \in runtime_delivery_delivery_ids) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_source_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_source_sequences THEN runtime_delivery_delivery_source_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_source_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_source_sequences THEN runtime_delivery_delivery_source_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.source_sequence))
       /\ runtime_delivery_phase' = "Active"
       /\ UNCHANGED << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = AppendIfMissing(pending_routes, [route |-> "runtime_delivery_reuse_acknowledges_job_outbox", source_machine |-> "runtime_delivery", effect |-> "DeliveryReused", target_machine |-> "job", target_input |-> "MarkDeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.source_sequence], actor |-> "job_authority", effect_id |-> (model_step_count + 1), source_transition |-> "ReuseCommittedDelivery"])
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_delivery", variant |-> "DeliveryReused", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> (IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None), source_sequence |-> packet.payload.source_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ReuseCommittedDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_delivery", transition |-> "ReuseCommittedDelivery", actor |-> "runtime_delivery_authority", step |-> (model_step_count + 1), from_phase |-> runtime_delivery_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_delivery_ApplyNextDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_delivery"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("runtime_delivery_authority")
       /\ runtime_delivery_phase = "Active"
       /\ ((packet.payload.delivery_id \in runtime_delivery_delivery_ids) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence) /\ (packet.payload.delivery_sequence > runtime_delivery_applied_cursor) /\ ((packet.payload.delivery_sequence - 1) = runtime_delivery_applied_cursor))
       /\ runtime_delivery_phase' = "Active"
       /\ runtime_delivery_applied_cursor' = packet.payload.delivery_sequence
       /\ UNCHANGED << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_delivery", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyNextDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_delivery", transition |-> "ApplyNextDelivery", actor |-> "runtime_delivery_authority", step |-> (model_step_count + 1), from_phase |-> runtime_delivery_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_delivery_ObserveAlreadyAppliedDelivery(arg_delivery_id, arg_delivery_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "runtime_delivery"
       /\ packet.variant = "MarkDeliveryApplied"
       /\ packet.payload.delivery_id = arg_delivery_id
       /\ packet.payload.delivery_sequence = arg_delivery_sequence
       /\ ~HigherPriorityReady("runtime_delivery_authority")
       /\ runtime_delivery_phase = "Active"
       /\ ((packet.payload.delivery_id \in runtime_delivery_delivery_ids) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence) /\ (packet.payload.delivery_sequence <= runtime_delivery_applied_cursor))
       /\ runtime_delivery_phase' = "Active"
       /\ UNCHANGED << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "runtime_delivery", variant |-> "DeliveryApplied", payload |-> [delivery_id |-> packet.payload.delivery_id, delivery_sequence |-> packet.payload.delivery_sequence], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveAlreadyAppliedDelivery"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "runtime_delivery", transition |-> "ObserveAlreadyAppliedDelivery", actor |-> "runtime_delivery_authority", step |-> (model_step_count + 1), from_phase |-> runtime_delivery_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


runtime_delivery_applied_cursor_does_not_pass_committed_sequence == (runtime_delivery_applied_cursor <= runtime_delivery_next_sequence)
runtime_delivery_empty_delivery_set_has_zero_sequence == (IF (Cardinality(runtime_delivery_delivery_ids) # 0) THEN TRUE ELSE (runtime_delivery_next_sequence = 0))
runtime_delivery_delivery_identity_and_sequence_cardinality_match == (Cardinality(runtime_delivery_delivery_ids) = Cardinality(runtime_delivery_committed_sequences))
runtime_delivery_committed_sequence_cardinality_tracks_high_water == (Cardinality(runtime_delivery_committed_sequences) = runtime_delivery_next_sequence)

EntryPacketAdmissible_job(packet) ==
    \/ /\ (packet.variant = "Submit") /\ (job_phase = "Unsubmitted") /\ ((packet.payload.job_id # ""))
    \/ /\ (packet.variant = "ClaimAttempt") /\ (job_phase = "Queued") /\ (((packet.payload.attempt_id # "") /\ (packet.payload.worker_id # "") /\ (packet.payload.runner_handle # "") /\ (packet.payload.lease_expires_at_ms > packet.payload.claimed_at_ms)))
    \/ /\ (packet.variant = "ClaimAttempt") /\ (job_phase = "RetryScheduled") /\ (((packet.payload.attempt_id # "") /\ (packet.payload.worker_id # "") /\ (packet.payload.runner_handle # "") /\ (job_retry_due_at_ms # None) /\ (packet.payload.claimed_at_ms >= (IF "value" \in DOMAIN job_retry_due_at_ms THEN job_retry_due_at_ms["value"] ELSE None)) /\ (packet.payload.lease_expires_at_ms > packet.payload.claimed_at_ms) /\ (packet.payload.attempt_id # (IF "value" \in DOMAIN job_current_attempt_id THEN job_current_attempt_id["value"] ELSE None))))
    \/ /\ (packet.variant = "RenewLease") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (job_heartbeat_at_ms # None) /\ (packet.payload.heartbeat_at_ms > (IF "value" \in DOMAIN job_heartbeat_at_ms THEN job_heartbeat_at_ms["value"] ELSE None)) /\ (packet.payload.heartbeat_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.lease_expires_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "RenewLease") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (job_heartbeat_at_ms # None) /\ (packet.payload.heartbeat_at_ms > (IF "value" \in DOMAIN job_heartbeat_at_ms THEN job_heartbeat_at_ms["value"] ELSE None)) /\ (packet.payload.heartbeat_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.lease_expires_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "ReportProgress") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.cursor > job_progress_cursor)))
    \/ /\ (packet.variant = "ReportProgress") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.cursor > job_progress_cursor)))
    \/ /\ (packet.variant = "EmitNotification") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.notification_id # "") /\ (packet.payload.idempotency_key # "") /\ (packet.payload.runtime_delivery_id # "") /\ ((packet.payload.notification_id \in job_notification_ids) = FALSE) /\ ((packet.payload.idempotency_key \in job_notification_idempotency_keys) = FALSE) /\ (job_delivery_sequence < RustU64Max)))
    \/ /\ (packet.variant = "EmitNotification") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.notification_id # "") /\ (packet.payload.idempotency_key # "") /\ (packet.payload.runtime_delivery_id # "") /\ ((packet.payload.notification_id \in job_notification_ids) = FALSE) /\ ((packet.payload.idempotency_key \in job_notification_idempotency_keys) = FALSE) /\ (job_delivery_sequence < RustU64Max)))
    \/ /\ (packet.variant = "EmitNotification") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.notification_id # "") /\ (packet.payload.runtime_delivery_id # "") /\ (packet.payload.idempotency_key \in job_notification_idempotency_keys)))
    \/ /\ (packet.variant = "EmitNotification") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.notification_id # "") /\ (packet.payload.runtime_delivery_id # "") /\ (packet.payload.idempotency_key \in job_notification_idempotency_keys)))
    \/ /\ (packet.variant = "RecordCheckpoint") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.checkpoint_ref # "")))
    \/ /\ (packet.variant = "RecordCheckpoint") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None)) /\ (packet.payload.checkpoint_ref # "")))
    \/ /\ (packet.variant = "WaitExternal") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "ResumeRunning") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "RequestCancel") /\ (job_phase = "Running") /\ ((job_cancel_requested = FALSE))
    \/ /\ (packet.variant = "RequestCancel") /\ (job_phase = "WaitingExternal") /\ ((job_cancel_requested = FALSE))
    \/ /\ (packet.variant = "RequestCancel") /\ (job_phase = "Running") /\ (job_cancel_requested)
    \/ /\ (packet.variant = "RequestCancel") /\ (job_phase = "WaitingExternal") /\ (job_cancel_requested)
    \/ /\ (packet.variant = "RequestCancel") /\ (job_phase = "Cancelled")
    \/ /\ (packet.variant = "RequestCancel") /\ (job_phase = "Queued")
    \/ /\ (packet.variant = "RequestCancel") /\ (job_phase = "RetryScheduled")
    \/ /\ (packet.variant = "RequestCancel") /\ (job_phase = "LossObserved")
    \/ /\ (packet.variant = "LeaseExpired") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "LeaseExpired") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "ScheduleRetry") /\ (job_phase = "LossObserved") /\ ((job_lease_expired /\ (job_restart_class # "NonResumable") /\ (IF (job_restart_class # "CheckpointResumable") THEN TRUE ELSE (job_checkpoint_ref # None)) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.retry_due_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "ClassifyWorkerLoss") /\ (job_phase = "LossObserved") /\ ((job_lease_expired /\ (job_restart_class = "NonResumable") /\ (job_lease_expires_at_ms # None) /\ (packet.payload.observed_at_ms > (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "CompleteAttempt") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.completed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "CompleteAttempt") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.completed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "FailAttempt") /\ (job_phase = "Running") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.failed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "FailAttempt") /\ (job_phase = "WaitingExternal") /\ (((job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.failed_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "AcknowledgeCancel") /\ (job_phase = "Running") /\ ((job_cancel_requested /\ (job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.acknowledged_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "AcknowledgeCancel") /\ (job_phase = "WaitingExternal") /\ ((job_cancel_requested /\ (job_current_attempt_id = Some(packet.payload.attempt_id)) /\ (job_current_fence = packet.payload.fence) /\ (job_lease_expired = FALSE) /\ (job_lease_expires_at_ms # None) /\ (packet.payload.acknowledged_at_ms <= (IF "value" \in DOMAIN job_lease_expires_at_ms THEN job_lease_expires_at_ms["value"] ELSE None))))
    \/ /\ (packet.variant = "MarkNeedsAttention") /\ (job_phase = "Queued") /\ ((packet.payload.observed_at_ms > 0))
    \/ /\ (packet.variant = "MarkNeedsAttention") /\ (job_phase = "Running") /\ ((packet.payload.observed_at_ms > 0))
    \/ /\ (packet.variant = "MarkNeedsAttention") /\ (job_phase = "WaitingExternal") /\ ((packet.payload.observed_at_ms > 0))
    \/ /\ (packet.variant = "MarkNeedsAttention") /\ (job_phase = "LossObserved") /\ ((packet.payload.observed_at_ms > 0))
    \/ /\ (packet.variant = "MarkNeedsAttention") /\ (job_phase = "RetryScheduled") /\ ((packet.payload.observed_at_ms > 0))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Succeeded") /\ (((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Failed") /\ (((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Cancelled") /\ (((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "WorkerLost") /\ (((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "NeedsAttention") /\ (((packet.payload.delivery_id = "terminal") /\ (job_terminal_delivery_applied = FALSE) /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Succeeded") /\ (((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Failed") /\ (((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Cancelled") /\ (((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "WorkerLost") /\ (((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "NeedsAttention") /\ (((packet.payload.delivery_id = "terminal") /\ job_terminal_delivery_applied /\ (packet.payload.delivery_sequence = job_terminal_delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Running") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "WaitingExternal") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "LossObserved") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "RetryScheduled") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Succeeded") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Failed") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Cancelled") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "WorkerLost") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "NeedsAttention") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ ((packet.payload.delivery_id \in job_notification_applied) = FALSE) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Running") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "WaitingExternal") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "LossObserved") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "RetryScheduled") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Succeeded") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Failed") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "Cancelled") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "WorkerLost") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (job_phase = "NeedsAttention") /\ (((packet.payload.delivery_id \in job_notification_ids) /\ (packet.payload.delivery_id \in job_notification_applied) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN job_notification_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN job_notification_sequences THEN job_notification_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence)))

EntryPacketAdmissible_runtime_delivery(packet) ==
    \/ /\ (packet.variant = "CommitDelivery") /\ (runtime_delivery_phase = "Active") /\ ((((packet.payload.delivery_id \in runtime_delivery_delivery_ids) = FALSE) /\ (packet.payload.source_sequence > 0) /\ (runtime_delivery_next_sequence < RustU64Max)))
    \/ /\ (packet.variant = "CommitDelivery") /\ (runtime_delivery_phase = "Active") /\ (((packet.payload.delivery_id \in runtime_delivery_delivery_ids) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_source_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_source_sequences THEN runtime_delivery_delivery_source_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_source_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_source_sequences THEN runtime_delivery_delivery_source_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.source_sequence)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (runtime_delivery_phase = "Active") /\ (((packet.payload.delivery_id \in runtime_delivery_delivery_ids) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence) /\ (packet.payload.delivery_sequence > runtime_delivery_applied_cursor) /\ ((packet.payload.delivery_sequence - 1) = runtime_delivery_applied_cursor)))
    \/ /\ (packet.variant = "MarkDeliveryApplied") /\ (runtime_delivery_phase = "Active") /\ (((packet.payload.delivery_id \in runtime_delivery_delivery_ids) /\ ((IF "value" \in DOMAIN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None) THEN (IF (packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences) THEN Some((IF packet.payload.delivery_id \in DOMAIN runtime_delivery_delivery_sequences THEN runtime_delivery_delivery_sequences[packet.payload.delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = packet.payload.delivery_sequence) /\ (packet.payload.delivery_sequence <= runtime_delivery_applied_cursor)))

EntryPacketAdmissible(packet) ==
    CASE
      packet.machine = "job" -> EntryPacketAdmissible_job(packet)
      [] packet.machine = "runtime_delivery" -> EntryPacketAdmissible_runtime_delivery(packet)
      [] OTHER -> FALSE

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_runtime_delivery_first_commit ==
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
    /\ UNCHANGED << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_runtime_delivery_notification_commit ==
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
    /\ UNCHANGED << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_runtime_delivery_crash_retry_reuse ==
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
    /\ UNCHANGED << job_phase, job_job_id, job_restart_class, job_attempt_count, job_current_attempt_id, job_current_fence, job_current_worker_id, job_lease_expires_at_ms, job_heartbeat_at_ms, job_checkpoint_ref, job_runner_handle, job_progress_cursor, job_lease_expired, job_retry_due_at_ms, job_cancel_requested, job_delivery_sequence, job_notification_ids, job_notification_idempotency_keys, job_notification_id_by_key, job_notification_delivery_ids, job_notification_sequences, job_notification_applied, job_terminal_kind, job_terminal_delivery_sequence, job_terminal_delivery_applied, runtime_delivery_phase, runtime_delivery_delivery_ids, runtime_delivery_delivery_sequences, runtime_delivery_delivery_source_sequences, runtime_delivery_committed_sequences, runtime_delivery_next_sequence, runtime_delivery_applied_cursor, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessScriptComplete_runtime_delivery_first_commit ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ \E packet \in delivered_routes : packet.route = "job_terminal_enters_runtime_inbox"
    /\ \E packet \in delivered_routes : packet.route = "runtime_delivery_commit_acknowledges_job_outbox"
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "CompleteRunningAttempt")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplySucceededDelivery")

WitnessScriptComplete_runtime_delivery_notification_commit ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ \E packet \in delivered_routes : packet.route = "job_notification_enters_runtime_inbox"
    /\ \E packet \in delivered_routes : packet.route = "runtime_delivery_commit_acknowledges_job_outbox"
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "EmitRunningNotification")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplyRunningNotificationDelivery")

WitnessScriptComplete_runtime_delivery_crash_retry_reuse ==
    /\ Len(witness_remaining_script_inputs) = 0
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_routes) = 0
    /\ \E packet \in delivered_routes : packet.route = "job_terminal_enters_runtime_inbox"
    /\ \E packet \in delivered_routes : packet.route = "runtime_delivery_reuse_acknowledges_job_outbox"
    /\ (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "CompleteRunningAttempt")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "ReuseCommittedDelivery")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplySucceededDelivery")

WitnessNoPrematureStutter_runtime_delivery_first_commit ==
    \/ WitnessScriptComplete_runtime_delivery_first_commit
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_runtime_delivery_notification_commit ==
    \/ WitnessScriptComplete_runtime_delivery_notification_commit
    \/ model_step_count' # model_step_count

WitnessNoPrematureStutter_runtime_delivery_crash_retry_reuse ==
    \/ WitnessScriptComplete_runtime_delivery_crash_retry_reuse
    \/ model_step_count' # model_step_count

WitnessSatisfiedStutter_runtime_delivery_first_commit ==
    /\ WitnessScriptComplete_runtime_delivery_first_commit
    /\ \E packet \in delivered_routes : packet.route = "job_terminal_enters_runtime_inbox"
    /\ \E packet \in delivered_routes : packet.route = "runtime_delivery_commit_acknowledges_job_outbox"
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "CompleteRunningAttempt")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplySucceededDelivery")
    /\ UNCHANGED vars

WitnessSatisfiedStutter_runtime_delivery_notification_commit ==
    /\ WitnessScriptComplete_runtime_delivery_notification_commit
    /\ \E packet \in delivered_routes : packet.route = "job_notification_enters_runtime_inbox"
    /\ \E packet \in delivered_routes : packet.route = "runtime_delivery_commit_acknowledges_job_outbox"
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "EmitRunningNotification")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplyRunningNotificationDelivery")
    /\ UNCHANGED vars

WitnessSatisfiedStutter_runtime_delivery_crash_retry_reuse ==
    /\ WitnessScriptComplete_runtime_delivery_crash_retry_reuse
    /\ \E packet \in delivered_routes : packet.route = "job_terminal_enters_runtime_inbox"
    /\ \E packet \in delivered_routes : packet.route = "runtime_delivery_reuse_acknowledges_job_outbox"
    /\ (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "CompleteRunningAttempt")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "ReuseCommittedDelivery")
    /\ (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplySucceededDelivery")
    /\ UNCHANGED vars

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_job_id \in StringValues : \E arg_restart_class \in DetachedJobRestartClassValues : job_SubmitQueued(arg_job_id, arg_restart_class)
    \/ \E arg_attempt_id \in StringValues : \E arg_worker_id \in StringValues : \E arg_claimed_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : \E arg_runner_handle \in StringValues : job_ClaimQueued(arg_attempt_id, arg_worker_id, arg_claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle)
    \/ \E arg_attempt_id \in StringValues : \E arg_worker_id \in StringValues : \E arg_claimed_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : \E arg_runner_handle \in StringValues : job_ClaimRetryScheduled(arg_attempt_id, arg_worker_id, arg_claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_heartbeat_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : job_RenewRunningLease(arg_attempt_id, arg_fence, arg_heartbeat_at_ms, arg_lease_expires_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_heartbeat_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : job_RenewExternalWaitLease(arg_attempt_id, arg_fence, arg_heartbeat_at_ms, arg_lease_expires_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_cursor \in 0..2 : \E arg_observed_at_ms \in 0..2 : job_ReportRunningProgress(arg_attempt_id, arg_fence, arg_cursor, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_cursor \in 0..2 : \E arg_observed_at_ms \in 0..2 : job_ReportExternalWaitProgress(arg_attempt_id, arg_fence, arg_cursor, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_notification_id \in StringValues : \E arg_idempotency_key \in StringValues : \E arg_runtime_delivery_id \in StringValues : \E arg_observed_at_ms \in 0..2 : job_EmitRunningNotification(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_notification_id \in StringValues : \E arg_idempotency_key \in StringValues : \E arg_runtime_delivery_id \in StringValues : \E arg_observed_at_ms \in 0..2 : job_EmitExternalWaitNotification(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_notification_id \in StringValues : \E arg_idempotency_key \in StringValues : \E arg_runtime_delivery_id \in StringValues : \E arg_observed_at_ms \in 0..2 : job_SuppressRunningNotificationReplay(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_notification_id \in StringValues : \E arg_idempotency_key \in StringValues : \E arg_runtime_delivery_id \in StringValues : \E arg_observed_at_ms \in 0..2 : job_SuppressExternalWaitNotificationReplay(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_checkpoint_ref \in StringValues : \E arg_observed_at_ms \in 0..2 : job_RecordRunningCheckpoint(arg_attempt_id, arg_fence, arg_checkpoint_ref, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_checkpoint_ref \in StringValues : \E arg_observed_at_ms \in 0..2 : job_RecordExternalWaitCheckpoint(arg_attempt_id, arg_fence, arg_checkpoint_ref, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_observed_at_ms \in 0..2 : job_WaitExternalFromRunning(arg_attempt_id, arg_fence, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_observed_at_ms \in 0..2 : job_ResumeRunningFromExternal(arg_attempt_id, arg_fence, arg_observed_at_ms)
    \/ job_RequestCancelRunning
    \/ job_RequestCancelWaitingExternal
    \/ job_RequestCancelAlreadyRequestedRunning
    \/ job_RequestCancelAlreadyRequestedWaitingExternal
    \/ job_RequestCancelAlreadyCancelled
    \/ job_RequestCancelQueued
    \/ job_RequestCancelRetryScheduled
    \/ job_RequestCancelLossObserved
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_observed_at_ms \in 0..2 : job_LeaseExpiresRunning(arg_attempt_id, arg_fence, arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_observed_at_ms \in 0..2 : job_LeaseExpiresWaitingExternal(arg_attempt_id, arg_fence, arg_observed_at_ms)
    \/ \E arg_retry_due_at_ms \in 0..2 : job_ScheduleRetryAfterLoss(arg_retry_due_at_ms)
    \/ \E arg_observed_at_ms \in 0..2 : job_ClassifyNonResumableWorkerLoss(arg_observed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_completed_at_ms \in 0..2 : job_CompleteRunningAttempt(arg_attempt_id, arg_fence, arg_completed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_completed_at_ms \in 0..2 : job_CompleteWaitingExternalAttempt(arg_attempt_id, arg_fence, arg_completed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_failed_at_ms \in 0..2 : job_FailRunningAttempt(arg_attempt_id, arg_fence, arg_failed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_failed_at_ms \in 0..2 : job_FailWaitingExternalAttempt(arg_attempt_id, arg_fence, arg_failed_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_acknowledged_at_ms \in 0..2 : job_AcknowledgeRunningCancel(arg_attempt_id, arg_fence, arg_acknowledged_at_ms)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_acknowledged_at_ms \in 0..2 : job_AcknowledgeWaitingExternalCancel(arg_attempt_id, arg_fence, arg_acknowledged_at_ms)
    \/ \E arg_observed_at_ms \in 0..2 : job_MarkQueuedNeedsAttention(arg_observed_at_ms)
    \/ \E arg_observed_at_ms \in 0..2 : job_MarkRunningNeedsAttention(arg_observed_at_ms)
    \/ \E arg_observed_at_ms \in 0..2 : job_MarkWaitingExternalNeedsAttention(arg_observed_at_ms)
    \/ \E arg_observed_at_ms \in 0..2 : job_MarkLossObservedNeedsAttention(arg_observed_at_ms)
    \/ \E arg_observed_at_ms \in 0..2 : job_MarkRetryScheduledNeedsAttention(arg_observed_at_ms)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplySucceededDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyFailedDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyCancelledDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyWorkerLostDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyNeedsAttentionDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveSucceededDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveFailedDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveCancelledDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveWorkerLostDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveNeedsAttentionDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyRunningNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyWaitingExternalNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyLossObservedNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyRetryScheduledNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplySucceededNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyFailedNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyCancelledNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyWorkerLostNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyNeedsAttentionNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveRunningNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveWaitingExternalNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveLossObservedNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveRetryScheduledNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveSucceededNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveFailedNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveCancelledNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveWorkerLostNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ObserveNeedsAttentionNotificationDeliveryAlreadyApplied(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_source_sequence \in 0..2 : runtime_delivery_CommitNewDelivery(arg_delivery_id, arg_source_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_source_sequence \in 0..2 : runtime_delivery_ReuseCommittedDelivery(arg_delivery_id, arg_source_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : runtime_delivery_ApplyNextDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : runtime_delivery_ObserveAlreadyAppliedDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ QuiescentStutter

InjectNext ==
    FALSE

Next ==
    \/ CoreNext

WitnessNext_runtime_delivery_first_commit ==
    \/ DeliverQueuedRoute
    \/ \E arg_job_id \in StringValues : \E arg_restart_class \in DetachedJobRestartClassValues : job_SubmitQueued(arg_job_id, arg_restart_class)
    \/ \E arg_attempt_id \in StringValues : \E arg_worker_id \in StringValues : \E arg_claimed_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : \E arg_runner_handle \in StringValues : job_ClaimQueued(arg_attempt_id, arg_worker_id, arg_claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_completed_at_ms \in 0..2 : job_CompleteRunningAttempt(arg_attempt_id, arg_fence, arg_completed_at_ms)
    \/ \E arg_delivery_id \in StringValues : \E arg_source_sequence \in 0..2 : runtime_delivery_CommitNewDelivery(arg_delivery_id, arg_source_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplySucceededDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ WitnessSatisfiedStutter_runtime_delivery_first_commit
    \/ WitnessInjectNext_runtime_delivery_first_commit

WitnessNext_runtime_delivery_notification_commit ==
    \/ DeliverQueuedRoute
    \/ \E arg_job_id \in StringValues : \E arg_restart_class \in DetachedJobRestartClassValues : job_SubmitQueued(arg_job_id, arg_restart_class)
    \/ \E arg_attempt_id \in StringValues : \E arg_worker_id \in StringValues : \E arg_claimed_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : \E arg_runner_handle \in StringValues : job_ClaimQueued(arg_attempt_id, arg_worker_id, arg_claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_notification_id \in StringValues : \E arg_idempotency_key \in StringValues : \E arg_runtime_delivery_id \in StringValues : \E arg_observed_at_ms \in 0..2 : job_EmitRunningNotification(arg_attempt_id, arg_fence, arg_notification_id, arg_idempotency_key, arg_runtime_delivery_id, arg_observed_at_ms)
    \/ \E arg_delivery_id \in StringValues : \E arg_source_sequence \in 0..2 : runtime_delivery_CommitNewDelivery(arg_delivery_id, arg_source_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplyRunningNotificationDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ WitnessSatisfiedStutter_runtime_delivery_notification_commit
    \/ WitnessInjectNext_runtime_delivery_notification_commit

WitnessNext_runtime_delivery_crash_retry_reuse ==
    \/ DeliverQueuedRoute
    \/ \E arg_delivery_id \in StringValues : \E arg_source_sequence \in 0..2 : runtime_delivery_CommitNewDelivery(arg_delivery_id, arg_source_sequence)
    \/ \E arg_job_id \in StringValues : \E arg_restart_class \in DetachedJobRestartClassValues : job_SubmitQueued(arg_job_id, arg_restart_class)
    \/ \E arg_attempt_id \in StringValues : \E arg_worker_id \in StringValues : \E arg_claimed_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : \E arg_runner_handle \in StringValues : job_ClaimQueued(arg_attempt_id, arg_worker_id, arg_claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle)
    \/ \E arg_attempt_id \in StringValues : \E arg_fence \in 0..2 : \E arg_completed_at_ms \in 0..2 : job_CompleteRunningAttempt(arg_attempt_id, arg_fence, arg_completed_at_ms)
    \/ \E arg_delivery_id \in StringValues : \E arg_source_sequence \in 0..2 : runtime_delivery_ReuseCommittedDelivery(arg_delivery_id, arg_source_sequence)
    \/ \E arg_delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : job_ApplySucceededDelivery(arg_delivery_id, arg_delivery_sequence)
    \/ WitnessSatisfiedStutter_runtime_delivery_crash_retry_reuse
    \/ WitnessInjectNext_runtime_delivery_crash_retry_reuse


job_terminal_reaches_runtime_delivery_authority == \E route_name \in RouteNames : /\ RouteSource(route_name) = "job" /\ RouteEffect(route_name) = "TerminalCommitted" /\ RouteTargetMachine(route_name) = "runtime_delivery" /\ RouteTargetInput(route_name) = "CommitDelivery"
job_notification_reaches_runtime_delivery_authority == \E route_name \in RouteNames : /\ RouteSource(route_name) = "job" /\ RouteEffect(route_name) = "NotificationCommitted" /\ RouteTargetMachine(route_name) = "runtime_delivery" /\ RouteTargetInput(route_name) = "CommitDelivery"
runtime_commit_ack_is_the_job_delivery_ack_source == \A input_packet \in observed_inputs : ((input_packet.machine = "job" /\ input_packet.variant = "MarkDeliveryApplied" /\ input_packet.source_route = "runtime_delivery_commit_acknowledges_job_outbox") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "runtime_delivery" /\ input_packet.source_effect = "DeliveryCommitted" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "runtime_delivery" /\ effect_packet.variant = "DeliveryCommitted" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = "runtime_delivery_commit_acknowledges_job_outbox" /\ route_packet.source_machine = "runtime_delivery" /\ route_packet.effect = "DeliveryCommitted" /\ route_packet.target_machine = "job" /\ route_packet.target_input = "MarkDeliveryApplied" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
runtime_reuse_ack_is_the_retry_ack_source == \A input_packet \in observed_inputs : ((input_packet.machine = "job" /\ input_packet.variant = "MarkDeliveryApplied" /\ input_packet.source_route = "runtime_delivery_reuse_acknowledges_job_outbox") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "runtime_delivery" /\ input_packet.source_effect = "DeliveryReused" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "runtime_delivery" /\ effect_packet.variant = "DeliveryReused" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = "runtime_delivery_reuse_acknowledges_job_outbox" /\ route_packet.source_machine = "runtime_delivery" /\ route_packet.effect = "DeliveryReused" /\ route_packet.target_machine = "job" /\ route_packet.target_input = "MarkDeliveryApplied" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))

RouteObserved_job_terminal_enters_runtime_inbox == \E packet \in delivered_routes : packet.route = "job_terminal_enters_runtime_inbox"
RouteCoverage_job_terminal_enters_runtime_inbox == (RouteObserved_job_terminal_enters_runtime_inbox \/ ~RouteObserved_job_terminal_enters_runtime_inbox)
RouteObserved_job_notification_enters_runtime_inbox == \E packet \in delivered_routes : packet.route = "job_notification_enters_runtime_inbox"
RouteCoverage_job_notification_enters_runtime_inbox == (RouteObserved_job_notification_enters_runtime_inbox \/ ~RouteObserved_job_notification_enters_runtime_inbox)
RouteObserved_runtime_delivery_commit_acknowledges_job_outbox == \E packet \in delivered_routes : packet.route = "runtime_delivery_commit_acknowledges_job_outbox"
RouteCoverage_runtime_delivery_commit_acknowledges_job_outbox == (RouteObserved_runtime_delivery_commit_acknowledges_job_outbox \/ ~RouteObserved_runtime_delivery_commit_acknowledges_job_outbox)
RouteObserved_runtime_delivery_reuse_acknowledges_job_outbox == \E packet \in delivered_routes : packet.route = "runtime_delivery_reuse_acknowledges_job_outbox"
RouteCoverage_runtime_delivery_reuse_acknowledges_job_outbox == (RouteObserved_runtime_delivery_reuse_acknowledges_job_outbox \/ ~RouteObserved_runtime_delivery_reuse_acknowledges_job_outbox)
CoverageInstrumentation == RouteCoverage_job_terminal_enters_runtime_inbox /\ RouteCoverage_job_notification_enters_runtime_inbox /\ RouteCoverage_runtime_delivery_commit_acknowledges_job_outbox /\ RouteCoverage_runtime_delivery_reuse_acknowledges_job_outbox

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(job_notification_ids) <= 0 /\ Cardinality(job_notification_idempotency_keys) <= 0 /\ Cardinality(DOMAIN job_notification_id_by_key) <= 0 /\ Cardinality(DOMAIN job_notification_delivery_ids) <= 0 /\ Cardinality(DOMAIN job_notification_sequences) <= 0 /\ Cardinality(job_notification_applied) <= 0 /\ Cardinality(runtime_delivery_delivery_ids) <= 0 /\ Cardinality(DOMAIN runtime_delivery_delivery_sequences) <= 0 /\ Cardinality(DOMAIN runtime_delivery_delivery_source_sequences) <= 0 /\ Cardinality(runtime_delivery_committed_sequences) <= 0
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(job_notification_ids) <= 2 /\ Cardinality(job_notification_idempotency_keys) <= 2 /\ Cardinality(DOMAIN job_notification_id_by_key) <= 2 /\ Cardinality(DOMAIN job_notification_delivery_ids) <= 2 /\ Cardinality(DOMAIN job_notification_sequences) <= 2 /\ Cardinality(job_notification_applied) <= 2 /\ Cardinality(runtime_delivery_delivery_ids) <= 2 /\ Cardinality(DOMAIN runtime_delivery_delivery_sequences) <= 2 /\ Cardinality(DOMAIN runtime_delivery_delivery_source_sequences) <= 2 /\ Cardinality(runtime_delivery_committed_sequences) <= 2
WitnessStateConstraint_runtime_delivery_first_commit == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 13 /\ Len(pending_routes) <= 4 /\ Cardinality(delivered_routes) <= 3 /\ Cardinality(emitted_effects) <= 6 /\ Cardinality(observed_transitions) <= 12 /\ Cardinality(job_notification_ids) <= 2 /\ Cardinality(job_notification_idempotency_keys) <= 2 /\ Cardinality(DOMAIN job_notification_id_by_key) <= 2 /\ Cardinality(DOMAIN job_notification_delivery_ids) <= 2 /\ Cardinality(DOMAIN job_notification_sequences) <= 2 /\ Cardinality(job_notification_applied) <= 2 /\ Cardinality(runtime_delivery_delivery_ids) <= 2 /\ Cardinality(DOMAIN runtime_delivery_delivery_sequences) <= 2 /\ Cardinality(DOMAIN runtime_delivery_delivery_source_sequences) <= 2 /\ Cardinality(runtime_delivery_committed_sequences) <= 2
WitnessStateConstraint_runtime_delivery_notification_commit == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 13 /\ Len(pending_routes) <= 4 /\ Cardinality(delivered_routes) <= 3 /\ Cardinality(emitted_effects) <= 6 /\ Cardinality(observed_transitions) <= 12 /\ Cardinality(job_notification_ids) <= 2 /\ Cardinality(job_notification_idempotency_keys) <= 2 /\ Cardinality(DOMAIN job_notification_id_by_key) <= 2 /\ Cardinality(DOMAIN job_notification_delivery_ids) <= 2 /\ Cardinality(DOMAIN job_notification_sequences) <= 2 /\ Cardinality(job_notification_applied) <= 2 /\ Cardinality(runtime_delivery_delivery_ids) <= 2 /\ Cardinality(DOMAIN runtime_delivery_delivery_sequences) <= 2 /\ Cardinality(DOMAIN runtime_delivery_delivery_source_sequences) <= 2 /\ Cardinality(runtime_delivery_committed_sequences) <= 2
WitnessStateConstraint_runtime_delivery_crash_retry_reuse == /\ model_step_count <= 12 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 13 /\ Len(pending_routes) <= 4 /\ Cardinality(delivered_routes) <= 3 /\ Cardinality(emitted_effects) <= 6 /\ Cardinality(observed_transitions) <= 12 /\ Cardinality(job_notification_ids) <= 2 /\ Cardinality(job_notification_idempotency_keys) <= 2 /\ Cardinality(DOMAIN job_notification_id_by_key) <= 2 /\ Cardinality(DOMAIN job_notification_delivery_ids) <= 2 /\ Cardinality(DOMAIN job_notification_sequences) <= 2 /\ Cardinality(job_notification_applied) <= 2 /\ Cardinality(runtime_delivery_delivery_ids) <= 2 /\ Cardinality(DOMAIN runtime_delivery_delivery_sequences) <= 2 /\ Cardinality(DOMAIN runtime_delivery_delivery_source_sequences) <= 2 /\ Cardinality(runtime_delivery_committed_sequences) <= 2

Spec ==
    /\ Init
    /\ [][Next]_vars

WitnessSpec_runtime_delivery_first_commit ==
    /\ WitnessInit_runtime_delivery_first_commit
    /\ [] [WitnessNext_runtime_delivery_first_commit]_vars

WitnessSpec_runtime_delivery_notification_commit ==
    /\ WitnessInit_runtime_delivery_notification_commit
    /\ [] [WitnessNext_runtime_delivery_notification_commit]_vars

WitnessSpec_runtime_delivery_crash_retry_reuse ==
    /\ WitnessInit_runtime_delivery_crash_retry_reuse
    /\ [] [WitnessNext_runtime_delivery_crash_retry_reuse]_vars

WitnessRouteObserved_runtime_delivery_first_commit_job_terminal_enters_runtime_inbox == WitnessScriptComplete_runtime_delivery_first_commit => (RouteObserved_job_terminal_enters_runtime_inbox)
WitnessRouteObserved_runtime_delivery_first_commit_runtime_delivery_commit_acknowledges_job_outbox == WitnessScriptComplete_runtime_delivery_first_commit => (RouteObserved_runtime_delivery_commit_acknowledges_job_outbox)
WitnessTransitionObserved_runtime_delivery_first_commit_job_SubmitQueued == WitnessScriptComplete_runtime_delivery_first_commit => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
WitnessTransitionObserved_runtime_delivery_first_commit_job_ClaimQueued == WitnessScriptComplete_runtime_delivery_first_commit => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
WitnessTransitionObserved_runtime_delivery_first_commit_job_CompleteRunningAttempt == WitnessScriptComplete_runtime_delivery_first_commit => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "CompleteRunningAttempt")
WitnessTransitionObserved_runtime_delivery_first_commit_runtime_delivery_CommitNewDelivery == WitnessScriptComplete_runtime_delivery_first_commit => (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
WitnessTransitionObserved_runtime_delivery_first_commit_job_ApplySucceededDelivery == WitnessScriptComplete_runtime_delivery_first_commit => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplySucceededDelivery")
WitnessRouteObserved_runtime_delivery_notification_commit_job_notification_enters_runtime_inbox == WitnessScriptComplete_runtime_delivery_notification_commit => (RouteObserved_job_notification_enters_runtime_inbox)
WitnessRouteObserved_runtime_delivery_notification_commit_runtime_delivery_commit_acknowledges_job_outbox == WitnessScriptComplete_runtime_delivery_notification_commit => (RouteObserved_runtime_delivery_commit_acknowledges_job_outbox)
WitnessTransitionObserved_runtime_delivery_notification_commit_job_SubmitQueued == WitnessScriptComplete_runtime_delivery_notification_commit => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
WitnessTransitionObserved_runtime_delivery_notification_commit_job_ClaimQueued == WitnessScriptComplete_runtime_delivery_notification_commit => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
WitnessTransitionObserved_runtime_delivery_notification_commit_job_EmitRunningNotification == WitnessScriptComplete_runtime_delivery_notification_commit => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "EmitRunningNotification")
WitnessTransitionObserved_runtime_delivery_notification_commit_runtime_delivery_CommitNewDelivery == WitnessScriptComplete_runtime_delivery_notification_commit => (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
WitnessTransitionObserved_runtime_delivery_notification_commit_job_ApplyRunningNotificationDelivery == WitnessScriptComplete_runtime_delivery_notification_commit => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplyRunningNotificationDelivery")
WitnessRouteObserved_runtime_delivery_crash_retry_reuse_job_terminal_enters_runtime_inbox == WitnessScriptComplete_runtime_delivery_crash_retry_reuse => (RouteObserved_job_terminal_enters_runtime_inbox)
WitnessRouteObserved_runtime_delivery_crash_retry_reuse_runtime_delivery_reuse_acknowledges_job_outbox == WitnessScriptComplete_runtime_delivery_crash_retry_reuse => (RouteObserved_runtime_delivery_reuse_acknowledges_job_outbox)
WitnessTransitionObserved_runtime_delivery_crash_retry_reuse_runtime_delivery_CommitNewDelivery == WitnessScriptComplete_runtime_delivery_crash_retry_reuse => (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "CommitNewDelivery")
WitnessTransitionObserved_runtime_delivery_crash_retry_reuse_job_SubmitQueued == WitnessScriptComplete_runtime_delivery_crash_retry_reuse => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "SubmitQueued")
WitnessTransitionObserved_runtime_delivery_crash_retry_reuse_job_ClaimQueued == WitnessScriptComplete_runtime_delivery_crash_retry_reuse => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ClaimQueued")
WitnessTransitionObserved_runtime_delivery_crash_retry_reuse_job_CompleteRunningAttempt == WitnessScriptComplete_runtime_delivery_crash_retry_reuse => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "CompleteRunningAttempt")
WitnessTransitionObserved_runtime_delivery_crash_retry_reuse_runtime_delivery_ReuseCommittedDelivery == WitnessScriptComplete_runtime_delivery_crash_retry_reuse => (\E packet \in observed_transitions : /\ packet.machine = "runtime_delivery" /\ packet.transition = "ReuseCommittedDelivery")
WitnessTransitionObserved_runtime_delivery_crash_retry_reuse_job_ApplySucceededDelivery == WitnessScriptComplete_runtime_delivery_crash_retry_reuse => (\E packet \in observed_transitions : /\ packet.machine = "job" /\ packet.transition = "ApplySucceededDelivery")

THEOREM Spec => []job_terminal_reaches_runtime_delivery_authority
THEOREM Spec => []job_notification_reaches_runtime_delivery_authority
THEOREM Spec => []runtime_commit_ack_is_the_job_delivery_ack_source
THEOREM Spec => []runtime_reuse_ack_is_the_retry_ack_source
THEOREM Spec => []job_fence_tracks_claim_count
THEOREM Spec => []job_no_attempt_has_no_attempt_authority
THEOREM Spec => []job_checkpoint_requires_attempt
THEOREM Spec => []job_active_execution_requires_attempt_authority
THEOREM Spec => []job_terminal_requires_delivery
THEOREM Spec => []job_notification_identity_and_sequence_cardinality_match
THEOREM Spec => []job_applied_notifications_are_committed
THEOREM Spec => []job_nonterminal_has_no_terminal_delivery
THEOREM Spec => []runtime_delivery_applied_cursor_does_not_pass_committed_sequence
THEOREM Spec => []runtime_delivery_empty_delivery_set_has_zero_sequence
THEOREM Spec => []runtime_delivery_delivery_identity_and_sequence_cardinality_match
THEOREM Spec => []runtime_delivery_committed_sequence_cardinality_tracks_high_water

=============================================================================
