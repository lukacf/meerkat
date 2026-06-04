---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for WorkAttentionLifecycleMachine.

CONSTANTS AttentionDelegatedAuthorityValues, BooleanValues, NatValues, WorkAttentionBindingKeyValues, WorkAttentionModeValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
OptionWorkAttentionBindingKeyValues == {None} \cup {Some(x) : x \in WorkAttentionBindingKeyValues}

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

VARIABLES phase, model_step_count, revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms

vars == << phase, model_step_count, revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>

attention_can_close_own_review_item(mode, delegated_authority) == ((IF (mode = "Review") THEN TRUE ELSE (mode = "Falsify")) /\ (delegated_authority = "CloseOwnReviewItem"))
attention_can_link(mode) == (mode = "Coordinate")
attention_can_create(mode) == (mode = "Coordinate")
attention_can_block(mode) == (mode = "Pursue")
attention_can_update(mode) == (IF (mode = "Pursue") THEN TRUE ELSE (mode = "Coordinate"))
attention_can_release(mode) == (mode = "Pursue")
attention_can_add_evidence(mode) == (mode # "Observe")
attention_can_get(mode) == (IF (mode = "Pursue") THEN TRUE ELSE (IF (mode = "Coordinate") THEN TRUE ELSE (IF (mode = "Review") THEN TRUE ELSE (IF (mode = "Falsify") THEN TRUE ELSE (IF (mode = "Judge") THEN TRUE ELSE (mode = "Observe"))))))
attention_is_adversarial(mode) == (IF (mode = "Review") THEN TRUE ELSE (IF (mode = "Falsify") THEN TRUE ELSE (mode = "Observe")))
attention_can_close_if_policy_allows(mode, delegated_authority) == ((delegated_authority = "CloseIfPolicyAllows") /\ (attention_is_adversarial(mode) = FALSE))
attention_can_link_derived_from(mode) == attention_can_link(mode)
attention_can_link_related(mode) == attention_can_link(mode)
attention_can_link_parent(mode) == attention_can_link(mode)

Init ==
    /\ phase = "Active"
    /\ model_step_count = 0
    /\ revision = 1
    /\ paused_until_utc_ms = None
    /\ superseded_by_binding_key = None
    /\ terminal_at_utc_ms = None

TerminalStutter ==
    /\ phase = "Superseded" \/ phase = "Stopped"
    /\ UNCHANGED vars

PauseActive(expected_revision, until_utc_ms) ==
    /\ phase = "Active"
    /\ (revision = expected_revision)
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = until_utc_ms
    /\ UNCHANGED << superseded_by_binding_key, terminal_at_utc_ms >>


PausePaused(expected_revision, until_utc_ms) ==
    /\ phase = "Paused"
    /\ (revision = expected_revision)
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = until_utc_ms
    /\ UNCHANGED << superseded_by_binding_key, terminal_at_utc_ms >>


ResumePaused(expected_revision) ==
    /\ phase = "Paused"
    /\ (revision = expected_revision)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ UNCHANGED << superseded_by_binding_key, terminal_at_utc_ms >>


SupersedeActive(expected_revision, arg_superseded_by_binding_key, at_utc_ms) ==
    /\ phase = "Active"
    /\ (revision = expected_revision)
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ superseded_by_binding_key' = Some(arg_superseded_by_binding_key)
    /\ terminal_at_utc_ms' = Some(at_utc_ms)


SupersedePaused(expected_revision, arg_superseded_by_binding_key, at_utc_ms) ==
    /\ phase = "Paused"
    /\ (revision = expected_revision)
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ superseded_by_binding_key' = Some(arg_superseded_by_binding_key)
    /\ terminal_at_utc_ms' = Some(at_utc_ms)


StopActive(expected_revision, at_utc_ms) ==
    /\ phase = "Active"
    /\ (revision = expected_revision)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << superseded_by_binding_key >>


StopPaused(expected_revision, at_utc_ms) ==
    /\ phase = "Paused"
    /\ (revision = expected_revision)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << superseded_by_binding_key >>


ClassifyEligibilityActive(now_utc_ms) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


ClassifyEligibilityPausedElapsed(now_utc_ms) ==
    /\ phase = "Paused"
    /\ ((paused_until_utc_ms # None) /\ ((IF "value" \in DOMAIN paused_until_utc_ms THEN paused_until_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


ClassifyEligibilityPausedPending(now_utc_ms) ==
    /\ phase = "Paused"
    /\ (IF (paused_until_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN paused_until_utc_ms THEN paused_until_utc_ms["value"] ELSE None) > now_utc_ms))
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


ClassifyEligibilitySuperseded(now_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


ClassifyEligibilityStopped(now_utc_ms) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


ClassifyAuthorityActive(mode, delegated_authority) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


ClassifyAuthorityPaused(mode, delegated_authority) ==
    /\ phase = "Paused"
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


ClassifyAuthoritySuperseded(mode, delegated_authority) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


ClassifyAuthorityStopped(mode, delegated_authority) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>


Next ==
    \/ \E expected_revision \in {revision} : \E until_utc_ms \in OptionU64Values : PauseActive(expected_revision, until_utc_ms)
    \/ \E expected_revision \in {revision} : \E until_utc_ms \in OptionU64Values : PausePaused(expected_revision, until_utc_ms)
    \/ \E expected_revision \in {revision} : ResumePaused(expected_revision)
    \/ \E expected_revision \in {revision} : \E arg_superseded_by_binding_key \in WorkAttentionBindingKeyValues : \E at_utc_ms \in 0..2 : SupersedeActive(expected_revision, arg_superseded_by_binding_key, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E arg_superseded_by_binding_key \in WorkAttentionBindingKeyValues : \E at_utc_ms \in 0..2 : SupersedePaused(expected_revision, arg_superseded_by_binding_key, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : StopActive(expected_revision, at_utc_ms)
    \/ \E expected_revision \in {revision} : \E at_utc_ms \in 0..2 : StopPaused(expected_revision, at_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyEligibilityActive(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyEligibilityPausedElapsed(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyEligibilityPausedPending(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyEligibilitySuperseded(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyEligibilityStopped(now_utc_ms)
    \/ \E mode \in WorkAttentionModeValues : \E delegated_authority \in AttentionDelegatedAuthorityValues : ClassifyAuthorityActive(mode, delegated_authority)
    \/ \E mode \in WorkAttentionModeValues : \E delegated_authority \in AttentionDelegatedAuthorityValues : ClassifyAuthorityPaused(mode, delegated_authority)
    \/ \E mode \in WorkAttentionModeValues : \E delegated_authority \in AttentionDelegatedAuthorityValues : ClassifyAuthoritySuperseded(mode, delegated_authority)
    \/ \E mode \in WorkAttentionModeValues : \E delegated_authority \in AttentionDelegatedAuthorityValues : ClassifyAuthorityStopped(mode, delegated_authority)
    \/ TerminalStutter

live_has_no_terminal_time == (IF ((phase # "Active") /\ (phase # "Paused")) THEN TRUE ELSE (terminal_at_utc_ms = None))
paused_has_pause_state == (IF (phase = "Paused") THEN TRUE ELSE (paused_until_utc_ms = None))
superseded_records_successor == (IF (phase # "Superseded") THEN TRUE ELSE (superseded_by_binding_key # None))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []live_has_no_terminal_time
THEOREM Spec => []paused_has_pause_state
THEOREM Spec => []superseded_records_successor

=============================================================================
