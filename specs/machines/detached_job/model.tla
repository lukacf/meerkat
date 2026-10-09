---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for DetachedJobMachine.

\* RustU64Max is the TLA boundary for Expr::U64Max; production generated Rust renders u64::MAX.
CONSTANTS BooleanValues, DetachedJobRestartClassValues, DetachedJobTerminalKindValues, NatValues, SetOfStringValues, StringValues, RustU64Max

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
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied

vars == << phase, model_step_count, job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>

Init ==
    /\ phase = "Unsubmitted"
    /\ model_step_count = 0
    /\ job_id = ""
    /\ restart_class = "NonResumable"
    /\ attempt_count = 0
    /\ current_attempt_id = None
    /\ current_fence = 0
    /\ current_worker_id = None
    /\ lease_expires_at_ms = None
    /\ heartbeat_at_ms = None
    /\ checkpoint_ref = None
    /\ runner_handle = None
    /\ progress_cursor = 0
    /\ lease_expired = FALSE
    /\ retry_due_at_ms = None
    /\ cancel_requested = FALSE
    /\ delivery_sequence = 0
    /\ notification_ids = {}
    /\ notification_idempotency_keys = {}
    /\ notification_id_by_key = [x \in {} |-> None]
    /\ notification_delivery_ids = [x \in {} |-> None]
    /\ notification_sequences = [x \in {} |-> None]
    /\ notification_applied = {}
    /\ terminal_kind = None
    /\ terminal_delivery_sequence = 0
    /\ terminal_delivery_applied = FALSE

TerminalStutter ==
    /\ phase = "Succeeded" \/ phase = "Failed" \/ phase = "Cancelled" \/ phase = "WorkerLost" \/ phase = "NeedsAttention"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_00a452fa4cc718bb == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_3a5843a4ed579a65 == UNCHANGED << job_id, restart_class, checkpoint_ref, progress_cursor, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_45de88d8b6f2bd60 == UNCHANGED << attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_5e36bcf22033484b == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_delivery_applied >>
UnchangedFrame_62df8988a689d1a1 == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_67cc11bdbd6b59f8 == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence >>
UnchangedFrame_6a2368947c4d0d9a == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_7468de882ad45281 == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_8369fc6d372f51cc == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_a18023e59389a49e == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_delivery_applied >>
UnchangedFrame_b0c9d3b6b38c4ae5 == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_c1c16a30acd6351c == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, retry_due_at_ms, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_cb074d8a0d7ceda7 == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, lease_expired, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>
UnchangedFrame_fbd9e3fab5adcf39 == UNCHANGED << job_id, restart_class, attempt_count, current_attempt_id, current_fence, current_worker_id, lease_expires_at_ms, heartbeat_at_ms, checkpoint_ref, runner_handle, progress_cursor, retry_due_at_ms, cancel_requested, delivery_sequence, notification_ids, notification_idempotency_keys, notification_id_by_key, notification_delivery_ids, notification_sequences, notification_applied, terminal_kind, terminal_delivery_sequence, terminal_delivery_applied >>

SubmitQueued(arg_job_id, arg_restart_class) ==
    /\ phase = "Unsubmitted"
    /\ (arg_job_id # "")
    /\ phase' = "Queued"
    /\ model_step_count' = model_step_count + 1
    /\ job_id' = arg_job_id
    /\ restart_class' = arg_restart_class
    /\ UnchangedFrame_45de88d8b6f2bd60


ClaimQueued(attempt_id, worker_id, claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle) ==
    /\ phase = "Queued"
    /\ ((attempt_id # "") /\ (worker_id # "") /\ (arg_runner_handle # "") /\ (arg_lease_expires_at_ms > claimed_at_ms))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ attempt_count' = (attempt_count) + 1
    /\ current_attempt_id' = Some(attempt_id)
    /\ current_fence' = (current_fence) + 1
    /\ current_worker_id' = Some(worker_id)
    /\ lease_expires_at_ms' = Some(arg_lease_expires_at_ms)
    /\ heartbeat_at_ms' = Some(claimed_at_ms)
    /\ runner_handle' = Some(arg_runner_handle)
    /\ lease_expired' = FALSE
    /\ retry_due_at_ms' = None
    /\ cancel_requested' = FALSE
    /\ UnchangedFrame_3a5843a4ed579a65


ClaimRetryScheduled(attempt_id, worker_id, claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle) ==
    /\ phase = "RetryScheduled"
    /\ ((attempt_id # "") /\ (worker_id # "") /\ (arg_runner_handle # "") /\ (retry_due_at_ms # None) /\ (claimed_at_ms >= (IF "value" \in DOMAIN retry_due_at_ms THEN retry_due_at_ms["value"] ELSE None)) /\ (arg_lease_expires_at_ms > claimed_at_ms) /\ (attempt_id # (IF "value" \in DOMAIN current_attempt_id THEN current_attempt_id["value"] ELSE None)))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ attempt_count' = (attempt_count) + 1
    /\ current_attempt_id' = Some(attempt_id)
    /\ current_fence' = (current_fence) + 1
    /\ current_worker_id' = Some(worker_id)
    /\ lease_expires_at_ms' = Some(arg_lease_expires_at_ms)
    /\ heartbeat_at_ms' = Some(claimed_at_ms)
    /\ runner_handle' = Some(arg_runner_handle)
    /\ lease_expired' = FALSE
    /\ retry_due_at_ms' = None
    /\ cancel_requested' = FALSE
    /\ UnchangedFrame_3a5843a4ed579a65


RenewRunningLease(attempt_id, fence, arg_heartbeat_at_ms, arg_lease_expires_at_ms) ==
    /\ phase = "Running"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (heartbeat_at_ms # None) /\ (arg_heartbeat_at_ms > (IF "value" \in DOMAIN heartbeat_at_ms THEN heartbeat_at_ms["value"] ELSE None)) /\ (arg_lease_expires_at_ms > arg_heartbeat_at_ms) /\ (arg_lease_expires_at_ms > (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ lease_expires_at_ms' = Some(arg_lease_expires_at_ms)
    /\ heartbeat_at_ms' = Some(arg_heartbeat_at_ms)
    /\ UnchangedFrame_8369fc6d372f51cc


RenewExternalWaitLease(attempt_id, fence, arg_heartbeat_at_ms, arg_lease_expires_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (heartbeat_at_ms # None) /\ (arg_heartbeat_at_ms > (IF "value" \in DOMAIN heartbeat_at_ms THEN heartbeat_at_ms["value"] ELSE None)) /\ (arg_lease_expires_at_ms > arg_heartbeat_at_ms) /\ (arg_lease_expires_at_ms > (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ lease_expires_at_ms' = Some(arg_lease_expires_at_ms)
    /\ heartbeat_at_ms' = Some(arg_heartbeat_at_ms)
    /\ UnchangedFrame_8369fc6d372f51cc


ReportRunningProgress(attempt_id, fence, cursor, observed_at_ms) ==
    /\ phase = "Running"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)) /\ (cursor > progress_cursor))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ progress_cursor' = cursor
    /\ UnchangedFrame_00a452fa4cc718bb


ReportExternalWaitProgress(attempt_id, fence, cursor, observed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)) /\ (cursor > progress_cursor))
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ progress_cursor' = cursor
    /\ UnchangedFrame_00a452fa4cc718bb


EmitRunningNotification(attempt_id, fence, notification_id, idempotency_key, runtime_delivery_id, observed_at_ms) ==
    /\ phase = "Running"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)) /\ (notification_id # "") /\ (idempotency_key # "") /\ (runtime_delivery_id # "") /\ ((notification_id \in notification_ids) = FALSE) /\ ((idempotency_key \in notification_idempotency_keys) = FALSE) /\ (delivery_sequence < RustU64Max))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ notification_ids' = (notification_ids \cup {notification_id})
    /\ notification_idempotency_keys' = (notification_idempotency_keys \cup {idempotency_key})
    /\ notification_id_by_key' = MapSet(notification_id_by_key, idempotency_key, notification_id)
    /\ notification_delivery_ids' = MapSet(notification_delivery_ids, notification_id, runtime_delivery_id)
    /\ notification_sequences' = MapSet(notification_sequences, notification_id, ((delivery_sequence) + 1))
    /\ UnchangedFrame_62df8988a689d1a1


EmitExternalWaitNotification(attempt_id, fence, notification_id, idempotency_key, runtime_delivery_id, observed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)) /\ (notification_id # "") /\ (idempotency_key # "") /\ (runtime_delivery_id # "") /\ ((notification_id \in notification_ids) = FALSE) /\ ((idempotency_key \in notification_idempotency_keys) = FALSE) /\ (delivery_sequence < RustU64Max))
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ notification_ids' = (notification_ids \cup {notification_id})
    /\ notification_idempotency_keys' = (notification_idempotency_keys \cup {idempotency_key})
    /\ notification_id_by_key' = MapSet(notification_id_by_key, idempotency_key, notification_id)
    /\ notification_delivery_ids' = MapSet(notification_delivery_ids, notification_id, runtime_delivery_id)
    /\ notification_sequences' = MapSet(notification_sequences, notification_id, ((delivery_sequence) + 1))
    /\ UnchangedFrame_62df8988a689d1a1


SuppressRunningNotificationReplay(attempt_id, fence, notification_id, idempotency_key, runtime_delivery_id, observed_at_ms) ==
    /\ phase = "Running"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)) /\ (notification_id # "") /\ (runtime_delivery_id # "") /\ (idempotency_key \in notification_idempotency_keys))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


SuppressExternalWaitNotificationReplay(attempt_id, fence, notification_id, idempotency_key, runtime_delivery_id, observed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)) /\ (notification_id # "") /\ (runtime_delivery_id # "") /\ (idempotency_key \in notification_idempotency_keys))
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


RecordRunningCheckpoint(attempt_id, fence, arg_checkpoint_ref, observed_at_ms) ==
    /\ phase = "Running"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)) /\ (arg_checkpoint_ref # ""))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ checkpoint_ref' = Some(arg_checkpoint_ref)
    /\ UnchangedFrame_7468de882ad45281


RecordExternalWaitCheckpoint(attempt_id, fence, arg_checkpoint_ref, observed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)) /\ (arg_checkpoint_ref # ""))
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ checkpoint_ref' = Some(arg_checkpoint_ref)
    /\ UnchangedFrame_7468de882ad45281


WaitExternalFromRunning(attempt_id, fence, observed_at_ms) ==
    /\ phase = "Running"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ResumeRunningFromExternal(attempt_id, fence, observed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


RequestCancelRunning ==
    /\ phase = "Running"
    /\ (cancel_requested = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_requested' = TRUE
    /\ UnchangedFrame_c1c16a30acd6351c


RequestCancelWaitingExternal ==
    /\ phase = "WaitingExternal"
    /\ (cancel_requested = FALSE)
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_requested' = TRUE
    /\ UnchangedFrame_c1c16a30acd6351c


RequestCancelAlreadyRequestedRunning ==
    /\ phase = "Running"
    /\ cancel_requested
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


RequestCancelAlreadyRequestedWaitingExternal ==
    /\ phase = "WaitingExternal"
    /\ cancel_requested
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


RequestCancelAlreadyCancelled ==
    /\ phase = "Cancelled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


RequestCancelQueued ==
    /\ phase = "Queued"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_requested' = TRUE
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Cancelled")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_a18023e59389a49e


RequestCancelRetryScheduled ==
    /\ phase = "RetryScheduled"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_requested' = TRUE
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Cancelled")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_a18023e59389a49e


RequestCancelLossObserved ==
    /\ phase = "LossObserved"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_requested' = TRUE
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Cancelled")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_a18023e59389a49e


LeaseExpiresRunning(attempt_id, fence, observed_at_ms) ==
    /\ phase = "Running"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms > (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "LossObserved"
    /\ model_step_count' = model_step_count + 1
    /\ lease_expired' = TRUE
    /\ UnchangedFrame_fbd9e3fab5adcf39


LeaseExpiresWaitingExternal(attempt_id, fence, observed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (observed_at_ms > (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "LossObserved"
    /\ model_step_count' = model_step_count + 1
    /\ lease_expired' = TRUE
    /\ UnchangedFrame_fbd9e3fab5adcf39


ScheduleRetryAfterLoss(arg_retry_due_at_ms) ==
    /\ phase = "LossObserved"
    /\ (lease_expired /\ (restart_class # "NonResumable") /\ (IF (restart_class # "CheckpointResumable") THEN TRUE ELSE (checkpoint_ref # None)) /\ (lease_expires_at_ms # None) /\ (arg_retry_due_at_ms > (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "RetryScheduled"
    /\ model_step_count' = model_step_count + 1
    /\ retry_due_at_ms' = Some(arg_retry_due_at_ms)
    /\ UnchangedFrame_cb074d8a0d7ceda7


ClassifyNonResumableWorkerLoss(observed_at_ms) ==
    /\ phase = "LossObserved"
    /\ (lease_expired /\ (restart_class = "NonResumable") /\ (lease_expires_at_ms # None) /\ (observed_at_ms > (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "WorkerLost"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("WorkerLost")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


CompleteRunningAttempt(attempt_id, fence, completed_at_ms) ==
    /\ phase = "Running"
    /\ ((cancel_requested = FALSE) /\ (current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (completed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "Succeeded"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Succeeded")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


CompleteWaitingExternalAttempt(attempt_id, fence, completed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((cancel_requested = FALSE) /\ (current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (completed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "Succeeded"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Succeeded")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


FailRunningAttempt(attempt_id, fence, failed_at_ms) ==
    /\ phase = "Running"
    /\ ((cancel_requested = FALSE) /\ (current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (failed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Failed")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


FailWaitingExternalAttempt(attempt_id, fence, failed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ ((cancel_requested = FALSE) /\ (current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (failed_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Failed")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


AcknowledgeRunningCancel(attempt_id, fence, acknowledged_at_ms) ==
    /\ phase = "Running"
    /\ (cancel_requested /\ (current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (acknowledged_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Cancelled")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


AcknowledgeWaitingExternalCancel(attempt_id, fence, acknowledged_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ (cancel_requested /\ (current_attempt_id = Some(attempt_id)) /\ (current_fence = fence) /\ (lease_expired = FALSE) /\ (lease_expires_at_ms # None) /\ (acknowledged_at_ms <= (IF "value" \in DOMAIN lease_expires_at_ms THEN lease_expires_at_ms["value"] ELSE None)))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("Cancelled")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


MarkQueuedNeedsAttention(observed_at_ms) ==
    /\ phase = "Queued"
    /\ (observed_at_ms > 0)
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("NeedsAttention")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


MarkRunningNeedsAttention(observed_at_ms) ==
    /\ phase = "Running"
    /\ (observed_at_ms > 0)
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("NeedsAttention")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


MarkWaitingExternalNeedsAttention(observed_at_ms) ==
    /\ phase = "WaitingExternal"
    /\ (observed_at_ms > 0)
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("NeedsAttention")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


MarkLossObservedNeedsAttention(observed_at_ms) ==
    /\ phase = "LossObserved"
    /\ (observed_at_ms > 0)
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("NeedsAttention")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


MarkRetryScheduledNeedsAttention(observed_at_ms) ==
    /\ phase = "RetryScheduled"
    /\ (observed_at_ms > 0)
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_sequence' = (delivery_sequence) + 1
    /\ terminal_kind' = Some("NeedsAttention")
    /\ terminal_delivery_sequence' = (delivery_sequence + 1)
    /\ UnchangedFrame_5e36bcf22033484b


ApplySucceededDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Succeeded"
    /\ ((delivery_id = "terminal") /\ (terminal_delivery_applied = FALSE) /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "Succeeded"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_delivery_applied' = TRUE
    /\ UnchangedFrame_67cc11bdbd6b59f8


ApplyFailedDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Failed"
    /\ ((delivery_id = "terminal") /\ (terminal_delivery_applied = FALSE) /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_delivery_applied' = TRUE
    /\ UnchangedFrame_67cc11bdbd6b59f8


ApplyCancelledDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Cancelled"
    /\ ((delivery_id = "terminal") /\ (terminal_delivery_applied = FALSE) /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_delivery_applied' = TRUE
    /\ UnchangedFrame_67cc11bdbd6b59f8


ApplyWorkerLostDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "WorkerLost"
    /\ ((delivery_id = "terminal") /\ (terminal_delivery_applied = FALSE) /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "WorkerLost"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_delivery_applied' = TRUE
    /\ UnchangedFrame_67cc11bdbd6b59f8


ApplyNeedsAttentionDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "NeedsAttention"
    /\ ((delivery_id = "terminal") /\ (terminal_delivery_applied = FALSE) /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_delivery_applied' = TRUE
    /\ UnchangedFrame_67cc11bdbd6b59f8


ObserveSucceededDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Succeeded"
    /\ ((delivery_id = "terminal") /\ terminal_delivery_applied /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "Succeeded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveFailedDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Failed"
    /\ ((delivery_id = "terminal") /\ terminal_delivery_applied /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveCancelledDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Cancelled"
    /\ ((delivery_id = "terminal") /\ terminal_delivery_applied /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveWorkerLostDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "WorkerLost"
    /\ ((delivery_id = "terminal") /\ terminal_delivery_applied /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "WorkerLost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveNeedsAttentionDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "NeedsAttention"
    /\ ((delivery_id = "terminal") /\ terminal_delivery_applied /\ (arg_delivery_sequence = terminal_delivery_sequence))
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ApplyRunningNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Running"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ApplyWaitingExternalNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "WaitingExternal"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ApplyLossObservedNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "LossObserved"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "LossObserved"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ApplyRetryScheduledNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "RetryScheduled"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "RetryScheduled"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ApplySucceededNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Succeeded"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "Succeeded"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ApplyFailedNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Failed"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ApplyCancelledNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Cancelled"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ApplyWorkerLostNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "WorkerLost"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "WorkerLost"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ApplyNeedsAttentionNotificationDelivery(delivery_id, arg_delivery_sequence) ==
    /\ phase = "NeedsAttention"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~(((delivery_id \in notification_applied) = FALSE)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ ((delivery_id \in notification_applied) = FALSE) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ notification_applied' = (notification_applied \cup {delivery_id})
    /\ UnchangedFrame_6a2368947c4d0d9a


ObserveRunningNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Running"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveWaitingExternalNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "WaitingExternal"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "WaitingExternal"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveLossObservedNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "LossObserved"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "LossObserved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveRetryScheduledNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "RetryScheduled"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "RetryScheduled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveSucceededNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Succeeded"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "Succeeded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveFailedNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Failed"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveCancelledNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "Cancelled"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveWorkerLostNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "WorkerLost"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "WorkerLost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


ObserveNeedsAttentionNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence) ==
    /\ phase = "NeedsAttention"
    /\ ((IF ~((delivery_id \in notification_ids)) THEN TRUE ELSE (IF ~((delivery_id \in notification_applied)) THEN TRUE ELSE (delivery_id \in DOMAIN notification_sequences))) /\ ((delivery_id \in notification_ids) /\ (delivery_id \in notification_applied) /\ ((notification_sequences)[delivery_id] = arg_delivery_sequence)))
    /\ phase' = "NeedsAttention"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_b0c9d3b6b38c4ae5


Next ==
    \/ (phase = "Unsubmitted") /\ \E arg_job_id \in StringValues : \E arg_restart_class \in DetachedJobRestartClassValues : SubmitQueued(arg_job_id, arg_restart_class)
    \/ (phase = "Queued") /\ \E attempt_id \in StringValues : \E worker_id \in StringValues : \E claimed_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : \E arg_runner_handle \in StringValues : ClaimQueued(attempt_id, worker_id, claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle)
    \/ (phase = "RetryScheduled") /\ \E attempt_id \in StringValues : \E worker_id \in StringValues : \E claimed_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : \E arg_runner_handle \in StringValues : ClaimRetryScheduled(attempt_id, worker_id, claimed_at_ms, arg_lease_expires_at_ms, arg_runner_handle)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E arg_heartbeat_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : RenewRunningLease(attempt_id, fence, arg_heartbeat_at_ms, arg_lease_expires_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E arg_heartbeat_at_ms \in 0..2 : \E arg_lease_expires_at_ms \in 0..2 : RenewExternalWaitLease(attempt_id, fence, arg_heartbeat_at_ms, arg_lease_expires_at_ms)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E cursor \in 0..2 : \E observed_at_ms \in 0..2 : ReportRunningProgress(attempt_id, fence, cursor, observed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E cursor \in 0..2 : \E observed_at_ms \in 0..2 : ReportExternalWaitProgress(attempt_id, fence, cursor, observed_at_ms)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E notification_id \in StringValues : \E idempotency_key \in StringValues : \E runtime_delivery_id \in StringValues : \E observed_at_ms \in 0..2 : EmitRunningNotification(attempt_id, fence, notification_id, idempotency_key, runtime_delivery_id, observed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E notification_id \in StringValues : \E idempotency_key \in StringValues : \E runtime_delivery_id \in StringValues : \E observed_at_ms \in 0..2 : EmitExternalWaitNotification(attempt_id, fence, notification_id, idempotency_key, runtime_delivery_id, observed_at_ms)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E notification_id \in StringValues : \E idempotency_key \in StringValues : \E runtime_delivery_id \in StringValues : \E observed_at_ms \in 0..2 : SuppressRunningNotificationReplay(attempt_id, fence, notification_id, idempotency_key, runtime_delivery_id, observed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E notification_id \in StringValues : \E idempotency_key \in StringValues : \E runtime_delivery_id \in StringValues : \E observed_at_ms \in 0..2 : SuppressExternalWaitNotificationReplay(attempt_id, fence, notification_id, idempotency_key, runtime_delivery_id, observed_at_ms)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E arg_checkpoint_ref \in StringValues : \E observed_at_ms \in 0..2 : RecordRunningCheckpoint(attempt_id, fence, arg_checkpoint_ref, observed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E arg_checkpoint_ref \in StringValues : \E observed_at_ms \in 0..2 : RecordExternalWaitCheckpoint(attempt_id, fence, arg_checkpoint_ref, observed_at_ms)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E observed_at_ms \in 0..2 : WaitExternalFromRunning(attempt_id, fence, observed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E observed_at_ms \in 0..2 : ResumeRunningFromExternal(attempt_id, fence, observed_at_ms)
    \/ RequestCancelRunning
    \/ RequestCancelWaitingExternal
    \/ RequestCancelAlreadyRequestedRunning
    \/ RequestCancelAlreadyRequestedWaitingExternal
    \/ RequestCancelAlreadyCancelled
    \/ RequestCancelQueued
    \/ RequestCancelRetryScheduled
    \/ RequestCancelLossObserved
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E observed_at_ms \in 0..2 : LeaseExpiresRunning(attempt_id, fence, observed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E observed_at_ms \in 0..2 : LeaseExpiresWaitingExternal(attempt_id, fence, observed_at_ms)
    \/ (phase = "LossObserved") /\ \E arg_retry_due_at_ms \in 0..2 : ScheduleRetryAfterLoss(arg_retry_due_at_ms)
    \/ (phase = "LossObserved") /\ \E observed_at_ms \in 0..2 : ClassifyNonResumableWorkerLoss(observed_at_ms)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E completed_at_ms \in 0..2 : CompleteRunningAttempt(attempt_id, fence, completed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E completed_at_ms \in 0..2 : CompleteWaitingExternalAttempt(attempt_id, fence, completed_at_ms)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E failed_at_ms \in 0..2 : FailRunningAttempt(attempt_id, fence, failed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E failed_at_ms \in 0..2 : FailWaitingExternalAttempt(attempt_id, fence, failed_at_ms)
    \/ (phase = "Running") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E acknowledged_at_ms \in 0..2 : AcknowledgeRunningCancel(attempt_id, fence, acknowledged_at_ms)
    \/ (phase = "WaitingExternal") /\ \E attempt_id \in StringValues : \E fence \in 0..2 : \E acknowledged_at_ms \in 0..2 : AcknowledgeWaitingExternalCancel(attempt_id, fence, acknowledged_at_ms)
    \/ (phase = "Queued") /\ \E observed_at_ms \in 0..2 : MarkQueuedNeedsAttention(observed_at_ms)
    \/ (phase = "Running") /\ \E observed_at_ms \in 0..2 : MarkRunningNeedsAttention(observed_at_ms)
    \/ (phase = "WaitingExternal") /\ \E observed_at_ms \in 0..2 : MarkWaitingExternalNeedsAttention(observed_at_ms)
    \/ (phase = "LossObserved") /\ \E observed_at_ms \in 0..2 : MarkLossObservedNeedsAttention(observed_at_ms)
    \/ (phase = "RetryScheduled") /\ \E observed_at_ms \in 0..2 : MarkRetryScheduledNeedsAttention(observed_at_ms)
    \/ (phase = "Succeeded") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplySucceededDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "Failed") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyFailedDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "Cancelled") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyCancelledDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "WorkerLost") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyWorkerLostDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "NeedsAttention") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyNeedsAttentionDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "Succeeded") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveSucceededDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "Failed") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveFailedDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "Cancelled") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveCancelledDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "WorkerLost") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveWorkerLostDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "NeedsAttention") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveNeedsAttentionDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "Running") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyRunningNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "WaitingExternal") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyWaitingExternalNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "LossObserved") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyLossObservedNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "RetryScheduled") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyRetryScheduledNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "Succeeded") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplySucceededNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "Failed") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyFailedNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "Cancelled") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyCancelledNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "WorkerLost") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyWorkerLostNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "NeedsAttention") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ApplyNeedsAttentionNotificationDelivery(delivery_id, arg_delivery_sequence)
    \/ (phase = "Running") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveRunningNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "WaitingExternal") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveWaitingExternalNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "LossObserved") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveLossObservedNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "RetryScheduled") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveRetryScheduledNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "Succeeded") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveSucceededNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "Failed") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveFailedNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "Cancelled") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveCancelledNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "WorkerLost") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveWorkerLostNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ (phase = "NeedsAttention") /\ \E delivery_id \in StringValues : \E arg_delivery_sequence \in 0..2 : ObserveNeedsAttentionNotificationDeliveryAlreadyApplied(delivery_id, arg_delivery_sequence)
    \/ TerminalStutter

fence_tracks_claim_count == (current_fence = attempt_count)
no_attempt_has_no_attempt_authority == (IF (attempt_count # 0) THEN TRUE ELSE ((current_attempt_id = None) /\ (current_worker_id = None) /\ (lease_expires_at_ms = None) /\ (heartbeat_at_ms = None) /\ (runner_handle = None)))
checkpoint_requires_attempt == (IF (checkpoint_ref = None) THEN TRUE ELSE (attempt_count > 0))
active_execution_requires_attempt_authority == (IF ((phase # "Running") /\ (phase # "WaitingExternal") /\ (phase # "LossObserved") /\ (phase # "RetryScheduled")) THEN TRUE ELSE ((current_attempt_id # None) /\ (current_worker_id # None) /\ (lease_expires_at_ms # None) /\ (runner_handle # None) /\ (current_fence > 0)))
terminal_requires_delivery == (IF ((phase # "Succeeded") /\ (phase # "Failed") /\ (phase # "Cancelled") /\ (phase # "WorkerLost") /\ (phase # "NeedsAttention")) THEN TRUE ELSE ((terminal_kind # None) /\ (terminal_delivery_sequence > 0) /\ (terminal_delivery_sequence <= delivery_sequence)))
notification_identity_and_sequence_cardinality_match == ((Cardinality(notification_ids) = Cardinality(notification_idempotency_keys)) /\ (DOMAIN notification_id_by_key = notification_idempotency_keys) /\ (DOMAIN notification_delivery_ids = notification_ids) /\ (DOMAIN notification_sequences = notification_ids))
applied_notifications_are_committed == (Cardinality(notification_applied) <= Cardinality(notification_ids))
nonterminal_has_no_terminal_delivery == (IF (phase = "Succeeded") THEN TRUE ELSE (IF (phase = "Failed") THEN TRUE ELSE (IF (phase = "Cancelled") THEN TRUE ELSE (IF (phase = "WorkerLost") THEN TRUE ELSE (IF (phase = "NeedsAttention") THEN TRUE ELSE ((terminal_kind = None) /\ (terminal_delivery_sequence = 0) /\ (terminal_delivery_applied = FALSE)))))))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(notification_ids) <= 1 /\ Cardinality(notification_idempotency_keys) <= 1 /\ Cardinality(DOMAIN notification_id_by_key) <= 1 /\ Cardinality(DOMAIN notification_delivery_ids) <= 1 /\ Cardinality(DOMAIN notification_sequences) <= 1 /\ Cardinality(notification_applied) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(notification_ids) <= 2 /\ Cardinality(notification_idempotency_keys) <= 2 /\ Cardinality(DOMAIN notification_id_by_key) <= 2 /\ Cardinality(DOMAIN notification_delivery_ids) <= 2 /\ Cardinality(DOMAIN notification_sequences) <= 2 /\ Cardinality(notification_applied) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []fence_tracks_claim_count
THEOREM Spec => []no_attempt_has_no_attempt_authority
THEOREM Spec => []checkpoint_requires_attempt
THEOREM Spec => []active_execution_requires_attempt_authority
THEOREM Spec => []terminal_requires_delivery
THEOREM Spec => []notification_identity_and_sequence_cardinality_match
THEOREM Spec => []applied_notifications_are_committed
THEOREM Spec => []nonterminal_has_no_terminal_delivery

=============================================================================
