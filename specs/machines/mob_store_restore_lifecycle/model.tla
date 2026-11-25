---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobStoreRestoreLifecycleMachine.

CONSTANTS BooleanValues, MobQuarantineClassValues, MobStoreCreateOutcomeValues, MobStoreLoadTriggerValues, MobStoreOriginValues, MobStoreRestoreOutcomeValues, MobStoreRestoreRefusalValues, MobStoreRetryTriggerValues, MobStoreStopValues, MobStoreUnloadOutcomeValues, NatValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

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

VARIABLES phase, model_step_count, class, attempts, origin, served, awaits_launch, vacant, stop, known

vars == << phase, model_step_count, class, attempts, origin, served, awaits_launch, vacant, stop, known >>

Init ==
    /\ phase = "Dormant"
    /\ model_step_count = 0
    /\ class = "StoreUnavailable"
    /\ attempts = 0
    /\ origin = "Persisted"
    /\ served = FALSE
    /\ awaits_launch = FALSE
    /\ vacant = FALSE
    /\ stop = "Unread"
    /\ known = FALSE

TerminalStutter ==
    /\ phase = "Gone"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_0aba66d09de2ab40 == UNCHANGED << class, attempts, origin, served, awaits_launch, vacant, stop, known >>
UnchangedFrame_25c74862e10ddfa3 == UNCHANGED << class, attempts, origin, served, awaits_launch, known >>
UnchangedFrame_26d62b576035da69 == UNCHANGED << class, attempts, awaits_launch, vacant, stop, known >>
UnchangedFrame_398ffd36ff3da1bc == UNCHANGED << class, attempts, origin, awaits_launch, known >>
UnchangedFrame_49173af48d3fde50 == UNCHANGED << class, attempts, origin, awaits_launch, stop, known >>
UnchangedFrame_6fdaf65eef2c71d1 == UNCHANGED << class, attempts, origin, served, vacant, stop, known >>
UnchangedFrame_6ff547923a3e1dd7 == UNCHANGED << attempts, origin, served, awaits_launch, known >>
UnchangedFrame_8384089d43c0e5ba == UNCHANGED << origin, served, awaits_launch, vacant, stop, known >>
UnchangedFrame_890ee1709d70ddab == UNCHANGED << class, attempts, origin, served, awaits_launch, vacant, stop >>
UnchangedFrame_8e46bff3f2987187 == UNCHANGED << class, attempts, origin, served, awaits_launch, vacant >>
UnchangedFrame_9ff2625bf9d562fe == UNCHANGED << class, attempts, served, awaits_launch, vacant, stop, known >>
UnchangedFrame_a39e1e572ff356e5 == UNCHANGED << class, attempts, awaits_launch >>
UnchangedFrame_a79bd908df5696bf == UNCHANGED << class, attempts, origin, awaits_launch, vacant, stop, known >>
UnchangedFrame_ab419cf1a3dc51ed == UNCHANGED << origin, awaits_launch, vacant, stop, known >>
UnchangedFrame_bc7adf1618479025 == UNCHANGED << origin, served, awaits_launch, stop, known >>
UnchangedFrame_da170ef0f312b60b == UNCHANGED << class, attempts, served, stop >>
UnchangedFrame_ef2a7147dcb466d7 == UNCHANGED << class, attempts, origin, served, awaits_launch, stop, known >>
UnchangedFrame_fcf85284d505914c == UNCHANGED << class, attempts, origin, served, awaits_launch, vacant, known >>

CensusDormant ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ known' = TRUE
    /\ UnchangedFrame_890ee1709d70ddab


CensusQuarantined ==
    /\ phase = "Quarantined"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ class' = "StoreUnavailable"
    /\ attempts' = 0
    /\ UnchangedFrame_8384089d43c0e5ba


CensusLoading ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


CensusManaged ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


CensusHeld ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


CensusRetrying ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


CensusUnloading ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


CensusGone ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


CensusCreating ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterDormant(holder_ended) ==
    /\ phase = "Dormant"
    /\ (origin # "Declared")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ origin' = "HostHandle"
    /\ served' = TRUE
    /\ vacant' = FALSE
    /\ stop' = "Unread"
    /\ known' = TRUE
    /\ UnchangedFrame_a39e1e572ff356e5


RegisterDormantLaunchOwned(holder_ended) ==
    /\ phase = "Dormant"
    /\ (origin = "Declared")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterManagedHostHandle(holder_ended) ==
    /\ phase = "Managed"
    /\ (origin = "HostHandle")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterManagedHostHandleSetRun(holder_ended) ==
    /\ phase = "Managed"
    /\ (IF (origin = "Persisted") THEN TRUE ELSE (origin = "Volatile"))
    /\ (holder_ended = FALSE)
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterManagedHostHandleOverEnded(holder_ended) ==
    /\ phase = "Managed"
    /\ (IF (origin = "Persisted") THEN TRUE ELSE (origin = "Volatile"))
    /\ (holder_ended = TRUE)
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ origin' = "HostHandle"
    /\ UnchangedFrame_9ff2625bf9d562fe


RegisterManagedLaunchOwned(holder_ended) ==
    /\ phase = "Managed"
    /\ (origin = "Declared")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterLoading(holder_ended) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterHeld(holder_ended) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterQuarantined(holder_ended) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterRetrying(holder_ended) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterUnloading(holder_ended) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterGone(holder_ended) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RegisterCreating(holder_ended) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareDormant(arg_vacant) ==
    /\ phase = "Dormant"
    /\ (origin # "Declared")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ origin' = "Declared"
    /\ awaits_launch' = TRUE
    /\ vacant' = arg_vacant
    /\ known' = TRUE
    /\ UnchangedFrame_da170ef0f312b60b


DeclareDormantDeclared(arg_vacant) ==
    /\ phase = "Dormant"
    /\ (origin = "Declared")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareLoading(arg_vacant) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareManaged(arg_vacant) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareUnloading(arg_vacant) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareHeld(arg_vacant) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareQuarantined(arg_vacant) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareRetrying(arg_vacant) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareGone(arg_vacant) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DeclareCreating(arg_vacant) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitDormant ==
    /\ phase = "Dormant"
    /\ awaits_launch
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ awaits_launch' = FALSE
    /\ UnchangedFrame_6fdaf65eef2c71d1


EndLaunchWaitDormantNotAwaited ==
    /\ phase = "Dormant"
    /\ ~(awaits_launch)
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitLoading ==
    /\ phase = "Loading"
    /\ awaits_launch
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ awaits_launch' = FALSE
    /\ UnchangedFrame_6fdaf65eef2c71d1


EndLaunchWaitLoadingNotAwaited ==
    /\ phase = "Loading"
    /\ ~(awaits_launch)
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitManaged ==
    /\ phase = "Managed"
    /\ awaits_launch
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ awaits_launch' = FALSE
    /\ UnchangedFrame_6fdaf65eef2c71d1


EndLaunchWaitManagedNotAwaited ==
    /\ phase = "Managed"
    /\ ~(awaits_launch)
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitUnloading ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitHeld ==
    /\ phase = "Held"
    /\ awaits_launch
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ awaits_launch' = FALSE
    /\ UnchangedFrame_6fdaf65eef2c71d1


EndLaunchWaitHeldNotAwaited ==
    /\ phase = "Held"
    /\ ~(awaits_launch)
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitQuarantined ==
    /\ phase = "Quarantined"
    /\ awaits_launch
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ awaits_launch' = FALSE
    /\ UnchangedFrame_6fdaf65eef2c71d1


EndLaunchWaitQuarantinedNotAwaited ==
    /\ phase = "Quarantined"
    /\ ~(awaits_launch)
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitRetrying ==
    /\ phase = "Retrying"
    /\ awaits_launch
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ awaits_launch' = FALSE
    /\ UnchangedFrame_6fdaf65eef2c71d1


EndLaunchWaitRetryingNotAwaited ==
    /\ phase = "Retrying"
    /\ ~(awaits_launch)
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitGone ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


EndLaunchWaitCreating ==
    /\ phase = "Creating"
    /\ (awaits_launch = TRUE)
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ awaits_launch' = FALSE
    /\ UnchangedFrame_6fdaf65eef2c71d1


EndLaunchWaitCreatingNotAwaited ==
    /\ phase = "Creating"
    /\ (awaits_launch = FALSE)
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadDormant(trigger) ==
    /\ phase = "Dormant"
    /\ (IF (origin # "Declared") THEN TRUE ELSE (IF (trigger = "Launch") THEN TRUE ELSE ((trigger = "Lookup") /\ ~(awaits_launch) /\ ~(vacant))))
    /\ (trigger # "Demand")
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ known' = TRUE
    /\ UnchangedFrame_890ee1709d70ddab


RequestLoadDormantAwaitsLaunch(trigger) ==
    /\ phase = "Dormant"
    /\ (origin = "Declared")
    /\ (IF (trigger = "Lookup") THEN TRUE ELSE (trigger = "Demand"))
    /\ awaits_launch
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadDormantVacant(trigger) ==
    /\ phase = "Dormant"
    /\ (origin = "Declared")
    /\ (IF (trigger = "Lookup") THEN TRUE ELSE (trigger = "Demand"))
    /\ ~(awaits_launch)
    /\ vacant
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadDormantLaunchOwned(trigger) ==
    /\ phase = "Dormant"
    /\ (origin = "Declared")
    /\ (trigger = "Pass")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadDormantDemand(trigger) ==
    /\ phase = "Dormant"
    /\ (trigger = "Demand")
    /\ (IF (origin # "Declared") THEN TRUE ELSE (~(awaits_launch) /\ ~(vacant)))
    /\ (stop = "NotStopped")
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadDormantStopped(trigger) ==
    /\ phase = "Dormant"
    /\ (trigger = "Demand")
    /\ (IF (origin # "Declared") THEN TRUE ELSE (~(awaits_launch) /\ ~(vacant)))
    /\ (stop = "Stopped")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadDormantStopUnread(trigger) ==
    /\ phase = "Dormant"
    /\ (trigger = "Demand")
    /\ (IF (origin # "Declared") THEN TRUE ELSE (~(awaits_launch) /\ ~(vacant)))
    /\ (stop = "Unread")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadLoading(trigger) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadManaged(trigger) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadHeld(trigger) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadQuarantined(trigger) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadRetrying(trigger) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadUnloading(trigger) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadGone(trigger) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestLoadCreating(trigger) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveLoadManaged(outcome, arg_class) ==
    /\ phase = "Loading"
    /\ (outcome = "Managed")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ served' = TRUE
    /\ vacant' = FALSE
    /\ stop' = "Unread"
    /\ UnchangedFrame_398ffd36ff3da1bc


ResolveLoadHeld(outcome, arg_class) ==
    /\ phase = "Loading"
    /\ (outcome = "Held")
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ vacant' = FALSE
    /\ stop' = "Unread"
    /\ UnchangedFrame_25c74862e10ddfa3


ResolveLoadSettled(outcome, arg_class) ==
    /\ phase = "Loading"
    /\ (outcome = "Settled")
    /\ (origin # "Declared")
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ stop' = "Unread"
    /\ UnchangedFrame_fcf85284d505914c


ResolveLoadSettledDeclared(outcome, arg_class) ==
    /\ phase = "Loading"
    /\ (outcome = "Settled")
    /\ (origin = "Declared")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ vacant' = TRUE
    /\ stop' = "Unread"
    /\ UnchangedFrame_25c74862e10ddfa3


ResolveLoadFailed(outcome, arg_class) ==
    /\ phase = "Loading"
    /\ (outcome = "Failed")
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ class' = arg_class
    /\ vacant' = FALSE
    /\ stop' = "Unread"
    /\ UnchangedFrame_6ff547923a3e1dd7


ResolveLoadDormant(outcome, arg_class) ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveLoadWhileManaged(outcome, arg_class) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveLoadWhileHeld(outcome, arg_class) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveLoadQuarantined(outcome, arg_class) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveLoadRetrying(outcome, arg_class) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveLoadUnloading(outcome, arg_class) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveLoadGone(outcome, arg_class) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveLoadCreating(outcome, arg_class) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryQuarantined(trigger) ==
    /\ phase = "Quarantined"
    /\ (IF (trigger = "Explicit") THEN TRUE ELSE (class = "StoreUnavailable"))
    /\ (IF (trigger = "Explicit") THEN TRUE ELSE ~(awaits_launch))
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryQuarantinedNotRetryable(trigger) ==
    /\ phase = "Quarantined"
    /\ (trigger = "OnDemand")
    /\ (class # "StoreUnavailable")
    /\ ~(awaits_launch)
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryQuarantinedAwaitsLaunch(trigger) ==
    /\ phase = "Quarantined"
    /\ (trigger = "OnDemand")
    /\ awaits_launch
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryRetrying(trigger) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryDormant(trigger) ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryLoading(trigger) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryManaged(trigger) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryHeld(trigger) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryUnloading(trigger) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryGone(trigger) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestRetryCreating(trigger) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveRetryFailed(outcome, arg_class) ==
    /\ phase = "Retrying"
    /\ (outcome = "Failed")
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ class' = arg_class
    /\ attempts' = (attempts) + 1
    /\ UnchangedFrame_8384089d43c0e5ba


ResolveRetryManaged(outcome, arg_class) ==
    /\ phase = "Retrying"
    /\ (outcome = "Managed")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ class' = "StoreUnavailable"
    /\ attempts' = (attempts) + 1
    /\ served' = TRUE
    /\ UnchangedFrame_ab419cf1a3dc51ed


ResolveRetryHeld(outcome, arg_class) ==
    /\ phase = "Retrying"
    /\ (outcome = "Held")
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ class' = "StoreUnavailable"
    /\ attempts' = (attempts) + 1
    /\ UnchangedFrame_8384089d43c0e5ba


ResolveRetrySettled(outcome, arg_class) ==
    /\ phase = "Retrying"
    /\ (outcome = "Settled")
    /\ (origin # "Declared")
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ class' = "StoreUnavailable"
    /\ attempts' = (attempts) + 1
    /\ UnchangedFrame_8384089d43c0e5ba


ResolveRetrySettledDeclared(outcome, arg_class) ==
    /\ phase = "Retrying"
    /\ (outcome = "Settled")
    /\ (origin = "Declared")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ class' = "StoreUnavailable"
    /\ attempts' = (attempts) + 1
    /\ vacant' = TRUE
    /\ UnchangedFrame_bc7adf1618479025


ResolveRetryDormant(outcome, arg_class) ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveRetryLoading(outcome, arg_class) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveRetryWhileManaged(outcome, arg_class) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveRetryWhileHeld(outcome, arg_class) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveRetryQuarantined(outcome, arg_class) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveRetryUnloading(outcome, arg_class) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveRetryGone(outcome, arg_class) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveRetryCreating(outcome, arg_class) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadManaged ==
    /\ phase = "Managed"
    /\ (origin = "Persisted")
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ served' = FALSE
    /\ UnchangedFrame_a79bd908df5696bf


RequestUnloadManagedNotReloadable ==
    /\ phase = "Managed"
    /\ (origin # "Persisted")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadUnloading ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadDormant ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadLoading ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadHeld ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadQuarantined ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadRetrying ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadGone ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestUnloadCreating ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveUnloadClosed(outcome, arg_stop) ==
    /\ phase = "Unloading"
    /\ (outcome = "Closed")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ stop' = arg_stop
    /\ UnchangedFrame_fcf85284d505914c


ResolveUnloadBusy(outcome, arg_stop) ==
    /\ phase = "Unloading"
    /\ (outcome = "Busy")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ served' = TRUE
    /\ UnchangedFrame_a79bd908df5696bf


ResolveUnloadDormant(outcome, arg_stop) ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveUnloadLoading(outcome, arg_stop) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveUnloadWhileManaged(outcome, arg_stop) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveUnloadHeld(outcome, arg_stop) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveUnloadQuarantined(outcome, arg_stop) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveUnloadRetrying(outcome, arg_stop) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveUnloadGone(outcome, arg_stop) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveUnloadCreating(outcome, arg_stop) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldManaged(outcome) ==
    /\ phase = "Held"
    /\ (outcome = "Managed")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ served' = TRUE
    /\ UnchangedFrame_a79bd908df5696bf


ReleaseHeldSettled(outcome) ==
    /\ phase = "Held"
    /\ (outcome = "Settled")
    /\ (origin # "Declared")
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldSettledDeclared(outcome) ==
    /\ phase = "Held"
    /\ (outcome = "Settled")
    /\ (origin = "Declared")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ vacant' = TRUE
    /\ UnchangedFrame_ef2a7147dcb466d7


ReleaseHeldKept(outcome) ==
    /\ phase = "Held"
    /\ (IF (outcome = "Held") THEN TRUE ELSE (outcome = "Failed"))
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldDormant(outcome) ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldLoading(outcome) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldWhileManaged(outcome) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldQuarantined(outcome) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldRetrying(outcome) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldUnloading(outcome) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldGone(outcome) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReleaseHeldCreating(outcome) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyManaged ==
    /\ phase = "Managed"
    /\ (origin # "Declared")
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ served' = FALSE
    /\ UnchangedFrame_a79bd908df5696bf


DestroyManagedLaunchOwned ==
    /\ phase = "Managed"
    /\ (origin = "Declared")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyDormant ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyLoading ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyHeld ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyQuarantined ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyRetrying ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyUnloading ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyGone ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


DestroyCreating ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateDormant(launch) ==
    /\ phase = "Dormant"
    /\ (origin # "Declared")
    /\ (launch = FALSE)
    /\ (known = FALSE)
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ stop' = "Unread"
    /\ known' = TRUE
    /\ UnchangedFrame_8e46bff3f2987187


RequestCreateDormantKnown(launch) ==
    /\ phase = "Dormant"
    /\ (origin # "Declared")
    /\ (launch = FALSE)
    /\ (known = TRUE)
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateDormantNotDeclared(launch) ==
    /\ phase = "Dormant"
    /\ (origin # "Declared")
    /\ (launch = TRUE)
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateDormantLaunch(launch) ==
    /\ phase = "Dormant"
    /\ (origin = "Declared")
    /\ (launch = TRUE)
    /\ (vacant = TRUE)
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ stop' = "Unread"
    /\ UnchangedFrame_fcf85284d505914c


RequestCreateDormantLaunchHoldsAMob(launch) ==
    /\ phase = "Dormant"
    /\ (origin = "Declared")
    /\ (launch = TRUE)
    /\ (vacant = FALSE)
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateDormantLaunchOwned(launch) ==
    /\ phase = "Dormant"
    /\ (origin = "Declared")
    /\ (launch = FALSE)
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateLoading(launch) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateManaged(launch) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateUnloading(launch) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateHeld(launch) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateQuarantined(launch) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateRetrying(launch) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateGone(launch) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


RequestCreateCreating(launch) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateCreated(outcome, in_memory) ==
    /\ phase = "Creating"
    /\ (outcome = "Created")
    /\ (origin # "Declared")
    /\ (in_memory = FALSE)
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ origin' = "Persisted"
    /\ served' = TRUE
    /\ UnchangedFrame_26d62b576035da69


ResolveCreateCreatedInMemory(outcome, in_memory) ==
    /\ phase = "Creating"
    /\ (outcome = "Created")
    /\ (origin # "Declared")
    /\ (in_memory = TRUE)
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ origin' = "Volatile"
    /\ served' = TRUE
    /\ UnchangedFrame_26d62b576035da69


ResolveCreateCreatedDeclared(outcome, in_memory) ==
    /\ phase = "Creating"
    /\ (outcome = "Created")
    /\ (origin = "Declared")
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ served' = TRUE
    /\ vacant' = FALSE
    /\ UnchangedFrame_49173af48d3fde50


ResolveCreateFailed(outcome, in_memory) ==
    /\ phase = "Creating"
    /\ (outcome = "Failed")
    /\ (origin # "Declared")
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateFailedDeclared(outcome, in_memory) ==
    /\ phase = "Creating"
    /\ (outcome = "Failed")
    /\ (origin = "Declared")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateDormant(outcome, in_memory) ==
    /\ phase = "Dormant"
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateLoading(outcome, in_memory) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateManaged(outcome, in_memory) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateUnloading(outcome, in_memory) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateHeld(outcome, in_memory) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateQuarantined(outcome, in_memory) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateRetrying(outcome, in_memory) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ResolveCreateGone(outcome, in_memory) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopDormantStopped(stopped) ==
    /\ phase = "Dormant"
    /\ (stop = "Unread")
    /\ (stopped = TRUE)
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ stop' = "Stopped"
    /\ known' = TRUE
    /\ UnchangedFrame_8e46bff3f2987187


ReadStopDormantNotStopped(stopped) ==
    /\ phase = "Dormant"
    /\ (stop = "Unread")
    /\ (stopped = FALSE)
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ stop' = "NotStopped"
    /\ known' = TRUE
    /\ UnchangedFrame_8e46bff3f2987187


ReadStopDormantKnown(stopped) ==
    /\ phase = "Dormant"
    /\ (stop # "Unread")
    /\ phase' = "Dormant"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopLoading(stopped) ==
    /\ phase = "Loading"
    /\ phase' = "Loading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopManaged(stopped) ==
    /\ phase = "Managed"
    /\ phase' = "Managed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopUnloading(stopped) ==
    /\ phase = "Unloading"
    /\ phase' = "Unloading"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopHeld(stopped) ==
    /\ phase = "Held"
    /\ phase' = "Held"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopQuarantined(stopped) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopRetrying(stopped) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopGone(stopped) ==
    /\ phase = "Gone"
    /\ phase' = "Gone"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


ReadStopCreating(stopped) ==
    /\ phase = "Creating"
    /\ phase' = "Creating"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0aba66d09de2ab40


Next ==
    \/ CensusDormant
    \/ CensusQuarantined
    \/ CensusLoading
    \/ CensusManaged
    \/ CensusHeld
    \/ CensusRetrying
    \/ CensusUnloading
    \/ CensusGone
    \/ CensusCreating
    \/ (phase = "Dormant") /\ \E holder_ended \in BOOLEAN : RegisterDormant(holder_ended)
    \/ (phase = "Dormant") /\ \E holder_ended \in BOOLEAN : RegisterDormantLaunchOwned(holder_ended)
    \/ (phase = "Managed") /\ \E holder_ended \in BOOLEAN : RegisterManagedHostHandle(holder_ended)
    \/ RegisterManagedHostHandleSetRun(FALSE)
    \/ RegisterManagedHostHandleOverEnded(TRUE)
    \/ (phase = "Managed") /\ \E holder_ended \in BOOLEAN : RegisterManagedLaunchOwned(holder_ended)
    \/ (phase = "Loading") /\ \E holder_ended \in BOOLEAN : RegisterLoading(holder_ended)
    \/ (phase = "Held") /\ \E holder_ended \in BOOLEAN : RegisterHeld(holder_ended)
    \/ (phase = "Quarantined") /\ \E holder_ended \in BOOLEAN : RegisterQuarantined(holder_ended)
    \/ (phase = "Retrying") /\ \E holder_ended \in BOOLEAN : RegisterRetrying(holder_ended)
    \/ (phase = "Unloading") /\ \E holder_ended \in BOOLEAN : RegisterUnloading(holder_ended)
    \/ (phase = "Gone") /\ \E holder_ended \in BOOLEAN : RegisterGone(holder_ended)
    \/ (phase = "Creating") /\ \E holder_ended \in BOOLEAN : RegisterCreating(holder_ended)
    \/ (phase = "Dormant") /\ \E arg_vacant \in BOOLEAN : DeclareDormant(arg_vacant)
    \/ (phase = "Dormant") /\ \E arg_vacant \in BOOLEAN : DeclareDormantDeclared(arg_vacant)
    \/ (phase = "Loading") /\ \E arg_vacant \in BOOLEAN : DeclareLoading(arg_vacant)
    \/ (phase = "Managed") /\ \E arg_vacant \in BOOLEAN : DeclareManaged(arg_vacant)
    \/ (phase = "Unloading") /\ \E arg_vacant \in BOOLEAN : DeclareUnloading(arg_vacant)
    \/ (phase = "Held") /\ \E arg_vacant \in BOOLEAN : DeclareHeld(arg_vacant)
    \/ (phase = "Quarantined") /\ \E arg_vacant \in BOOLEAN : DeclareQuarantined(arg_vacant)
    \/ (phase = "Retrying") /\ \E arg_vacant \in BOOLEAN : DeclareRetrying(arg_vacant)
    \/ (phase = "Gone") /\ \E arg_vacant \in BOOLEAN : DeclareGone(arg_vacant)
    \/ (phase = "Creating") /\ \E arg_vacant \in BOOLEAN : DeclareCreating(arg_vacant)
    \/ EndLaunchWaitDormant
    \/ EndLaunchWaitDormantNotAwaited
    \/ EndLaunchWaitLoading
    \/ EndLaunchWaitLoadingNotAwaited
    \/ EndLaunchWaitManaged
    \/ EndLaunchWaitManagedNotAwaited
    \/ EndLaunchWaitUnloading
    \/ EndLaunchWaitHeld
    \/ EndLaunchWaitHeldNotAwaited
    \/ EndLaunchWaitQuarantined
    \/ EndLaunchWaitQuarantinedNotAwaited
    \/ EndLaunchWaitRetrying
    \/ EndLaunchWaitRetryingNotAwaited
    \/ EndLaunchWaitGone
    \/ EndLaunchWaitCreating
    \/ EndLaunchWaitCreatingNotAwaited
    \/ (phase = "Dormant") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadDormant(trigger)
    \/ (phase = "Dormant") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadDormantAwaitsLaunch(trigger)
    \/ (phase = "Dormant") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadDormantVacant(trigger)
    \/ (phase = "Dormant") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadDormantLaunchOwned(trigger)
    \/ (phase = "Dormant") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadDormantDemand(trigger)
    \/ (phase = "Dormant") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadDormantStopped(trigger)
    \/ (phase = "Dormant") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadDormantStopUnread(trigger)
    \/ (phase = "Loading") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadLoading(trigger)
    \/ (phase = "Managed") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadManaged(trigger)
    \/ (phase = "Held") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadHeld(trigger)
    \/ (phase = "Quarantined") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadQuarantined(trigger)
    \/ (phase = "Retrying") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadRetrying(trigger)
    \/ (phase = "Unloading") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadUnloading(trigger)
    \/ (phase = "Gone") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadGone(trigger)
    \/ (phase = "Creating") /\ \E trigger \in MobStoreLoadTriggerValues : RequestLoadCreating(trigger)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadManaged(outcome, arg_class)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadHeld(outcome, arg_class)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadSettled(outcome, arg_class)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadSettledDeclared(outcome, arg_class)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadFailed(outcome, arg_class)
    \/ (phase = "Dormant") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadDormant(outcome, arg_class)
    \/ (phase = "Managed") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadWhileManaged(outcome, arg_class)
    \/ (phase = "Held") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadWhileHeld(outcome, arg_class)
    \/ (phase = "Quarantined") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadQuarantined(outcome, arg_class)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadRetrying(outcome, arg_class)
    \/ (phase = "Unloading") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadUnloading(outcome, arg_class)
    \/ (phase = "Gone") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadGone(outcome, arg_class)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveLoadCreating(outcome, arg_class)
    \/ (phase = "Quarantined") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryQuarantined(trigger)
    \/ (phase = "Quarantined") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryQuarantinedNotRetryable(trigger)
    \/ (phase = "Quarantined") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryQuarantinedAwaitsLaunch(trigger)
    \/ (phase = "Retrying") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryRetrying(trigger)
    \/ (phase = "Dormant") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryDormant(trigger)
    \/ (phase = "Loading") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryLoading(trigger)
    \/ (phase = "Managed") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryManaged(trigger)
    \/ (phase = "Held") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryHeld(trigger)
    \/ (phase = "Unloading") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryUnloading(trigger)
    \/ (phase = "Gone") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryGone(trigger)
    \/ (phase = "Creating") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryCreating(trigger)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryFailed(outcome, arg_class)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryManaged(outcome, arg_class)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryHeld(outcome, arg_class)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetrySettled(outcome, arg_class)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetrySettledDeclared(outcome, arg_class)
    \/ (phase = "Dormant") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryDormant(outcome, arg_class)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryLoading(outcome, arg_class)
    \/ (phase = "Managed") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryWhileManaged(outcome, arg_class)
    \/ (phase = "Held") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryWhileHeld(outcome, arg_class)
    \/ (phase = "Quarantined") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryQuarantined(outcome, arg_class)
    \/ (phase = "Unloading") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryUnloading(outcome, arg_class)
    \/ (phase = "Gone") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryGone(outcome, arg_class)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryCreating(outcome, arg_class)
    \/ RequestUnloadManaged
    \/ RequestUnloadManagedNotReloadable
    \/ RequestUnloadUnloading
    \/ RequestUnloadDormant
    \/ RequestUnloadLoading
    \/ RequestUnloadHeld
    \/ RequestUnloadQuarantined
    \/ RequestUnloadRetrying
    \/ RequestUnloadGone
    \/ RequestUnloadCreating
    \/ (phase = "Unloading") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadClosed(outcome, arg_stop)
    \/ (phase = "Unloading") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadBusy(outcome, arg_stop)
    \/ (phase = "Dormant") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadDormant(outcome, arg_stop)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadLoading(outcome, arg_stop)
    \/ (phase = "Managed") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadWhileManaged(outcome, arg_stop)
    \/ (phase = "Held") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadHeld(outcome, arg_stop)
    \/ (phase = "Quarantined") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadQuarantined(outcome, arg_stop)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadRetrying(outcome, arg_stop)
    \/ (phase = "Gone") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadGone(outcome, arg_stop)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreUnloadOutcomeValues : \E arg_stop \in MobStoreStopValues : ResolveUnloadCreating(outcome, arg_stop)
    \/ (phase = "Held") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldManaged(outcome)
    \/ (phase = "Held") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldSettled(outcome)
    \/ (phase = "Held") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldSettledDeclared(outcome)
    \/ (phase = "Held") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldKept(outcome)
    \/ (phase = "Dormant") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldDormant(outcome)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldLoading(outcome)
    \/ (phase = "Managed") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldWhileManaged(outcome)
    \/ (phase = "Quarantined") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldQuarantined(outcome)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldRetrying(outcome)
    \/ (phase = "Unloading") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldUnloading(outcome)
    \/ (phase = "Gone") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldGone(outcome)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreRestoreOutcomeValues : ReleaseHeldCreating(outcome)
    \/ DestroyManaged
    \/ DestroyManagedLaunchOwned
    \/ DestroyDormant
    \/ DestroyLoading
    \/ DestroyHeld
    \/ DestroyQuarantined
    \/ DestroyRetrying
    \/ DestroyUnloading
    \/ DestroyGone
    \/ DestroyCreating
    \/ RequestCreateDormant(FALSE)
    \/ RequestCreateDormantKnown(FALSE)
    \/ RequestCreateDormantNotDeclared(TRUE)
    \/ RequestCreateDormantLaunch(TRUE)
    \/ RequestCreateDormantLaunchHoldsAMob(TRUE)
    \/ RequestCreateDormantLaunchOwned(FALSE)
    \/ (phase = "Loading") /\ \E launch \in BOOLEAN : RequestCreateLoading(launch)
    \/ (phase = "Managed") /\ \E launch \in BOOLEAN : RequestCreateManaged(launch)
    \/ (phase = "Unloading") /\ \E launch \in BOOLEAN : RequestCreateUnloading(launch)
    \/ (phase = "Held") /\ \E launch \in BOOLEAN : RequestCreateHeld(launch)
    \/ (phase = "Quarantined") /\ \E launch \in BOOLEAN : RequestCreateQuarantined(launch)
    \/ (phase = "Retrying") /\ \E launch \in BOOLEAN : RequestCreateRetrying(launch)
    \/ (phase = "Gone") /\ \E launch \in BOOLEAN : RequestCreateGone(launch)
    \/ (phase = "Creating") /\ \E launch \in BOOLEAN : RequestCreateCreating(launch)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreCreateOutcomeValues : ResolveCreateCreated(outcome, FALSE)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreCreateOutcomeValues : ResolveCreateCreatedInMemory(outcome, TRUE)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateCreatedDeclared(outcome, in_memory)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateFailed(outcome, in_memory)
    \/ (phase = "Creating") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateFailedDeclared(outcome, in_memory)
    \/ (phase = "Dormant") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateDormant(outcome, in_memory)
    \/ (phase = "Loading") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateLoading(outcome, in_memory)
    \/ (phase = "Managed") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateManaged(outcome, in_memory)
    \/ (phase = "Unloading") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateUnloading(outcome, in_memory)
    \/ (phase = "Held") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateHeld(outcome, in_memory)
    \/ (phase = "Quarantined") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateQuarantined(outcome, in_memory)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateRetrying(outcome, in_memory)
    \/ (phase = "Gone") /\ \E outcome \in MobStoreCreateOutcomeValues : \E in_memory \in BOOLEAN : ResolveCreateGone(outcome, in_memory)
    \/ ReadStopDormantStopped(TRUE)
    \/ ReadStopDormantNotStopped(FALSE)
    \/ (phase = "Dormant") /\ \E stopped \in BOOLEAN : ReadStopDormantKnown(stopped)
    \/ (phase = "Loading") /\ \E stopped \in BOOLEAN : ReadStopLoading(stopped)
    \/ (phase = "Managed") /\ \E stopped \in BOOLEAN : ReadStopManaged(stopped)
    \/ (phase = "Unloading") /\ \E stopped \in BOOLEAN : ReadStopUnloading(stopped)
    \/ (phase = "Held") /\ \E stopped \in BOOLEAN : ReadStopHeld(stopped)
    \/ (phase = "Quarantined") /\ \E stopped \in BOOLEAN : ReadStopQuarantined(stopped)
    \/ (phase = "Retrying") /\ \E stopped \in BOOLEAN : ReadStopRetrying(stopped)
    \/ (phase = "Gone") /\ \E stopped \in BOOLEAN : ReadStopGone(stopped)
    \/ (phase = "Creating") /\ \E stopped \in BOOLEAN : ReadStopCreating(stopped)
    \/ TerminalStutter

only_a_quarantine_holds_a_class == (IF (phase = "Quarantined") THEN TRUE ELSE (IF (phase = "Retrying") THEN TRUE ELSE (class = "StoreUnavailable")))
only_a_persisted_store_waits_unloaded == (IF (origin = "Persisted") THEN TRUE ELSE (IF (origin = "Declared") THEN TRUE ELSE (IF (phase = "Managed") THEN TRUE ELSE (phase = "Gone"))))
only_a_declared_store_awaits_its_launch == (IF ~(awaits_launch) THEN TRUE ELSE (origin = "Declared"))
a_store_that_is_gone_awaits_no_launch == (IF ~(awaits_launch) THEN TRUE ELSE (phase # "Gone"))
a_store_whose_unload_is_admitted_awaits_no_launch == (IF ~(awaits_launch) THEN TRUE ELSE (phase # "Unloading"))
a_declared_store_is_never_gone == (IF (origin # "Declared") THEN TRUE ELSE (phase # "Gone"))
only_a_declared_store_that_is_not_loaded_is_vacant == (IF ~(vacant) THEN TRUE ELSE ((origin = "Declared") /\ (IF (phase = "Dormant") THEN TRUE ELSE (IF (phase = "Loading") THEN TRUE ELSE (phase = "Creating")))))
only_a_store_that_is_not_loaded_knows_its_mobs_stop == (IF (stop = "Unread") THEN TRUE ELSE (IF (phase = "Dormant") THEN TRUE ELSE (phase = "Loading")))
a_mob_is_handed_out_exactly_while_managed == (IF (served /\ (phase = "Managed")) THEN TRUE ELSE (~(served) /\ (phase # "Managed")))
a_record_that_stands_for_nothing_is_as_it_was_filed == (IF known THEN TRUE ELSE ((phase = "Dormant") /\ (origin = "Persisted") /\ ~(served) /\ ~(awaits_launch) /\ ~(vacant) /\ (stop = "Unread") /\ (attempts = 0) /\ (class = "StoreUnavailable")))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []only_a_quarantine_holds_a_class
THEOREM Spec => []only_a_persisted_store_waits_unloaded
THEOREM Spec => []only_a_declared_store_awaits_its_launch
THEOREM Spec => []a_store_that_is_gone_awaits_no_launch
THEOREM Spec => []a_store_whose_unload_is_admitted_awaits_no_launch
THEOREM Spec => []a_declared_store_is_never_gone
THEOREM Spec => []only_a_declared_store_that_is_not_loaded_is_vacant
THEOREM Spec => []only_a_store_that_is_not_loaded_knows_its_mobs_stop
THEOREM Spec => []a_mob_is_handed_out_exactly_while_managed
THEOREM Spec => []a_record_that_stands_for_nothing_is_as_it_was_filed

=============================================================================
