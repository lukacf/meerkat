---- MODULE lock_order ----
EXTENDS Naturals, Sequences, FiniteSets, TLC

\* I10: one checked order over the locks of the Connect source owner's synchronization.
\* Every actor runs one shape: a nested sequence of acquisitions (blocking or try) and
\* releases, inside a protected section or none. A blocking acquisition waits while
\* another actor holds the lock; a try acquisition refuses at once and the actor gives up.
\* Mode A is the current code's shapes (each step cites its site); mode B is the planned
\* design: the effect runner takes the publication writer before the store transaction,
\* no SQLite callback takes it, the bind decides over a snapshot without locks, native
\* readers answer from published state. Contract model: not the real mutexes or tokio.

CONSTANTS Fault, Mode

\* Locks, in the declared order of the planned design (rank). Tokio mutexes first (the source writer
\* before the class callback exclusion, as the code takes and documents them: "Source
\* writer precedes this existing policy exclusion", callbacks.rs:57), then the shared
\* publication writer ("acquire the publication guard before owner write guards",
\* meerkat-authorization publication.rs), then the store connection with its IMMEDIATE
\* transaction, then owner state. Lines are the current code's.
Rank == [SW |-> 1,     \* source writer (tokio), original_ingress source_writer()
         CX |-> 2,     \* class callback exclusion (tokio), callbacks.rs:66-67, member_classes.rs:113
         PUB |-> 3,    \* publication writer (std), begin_owner_change / reserve_owner_change
         CONN |-> 4,   \* store connection mutex and SQLite IMMEDIATE transaction, sqlite.rs:215, :706
         RC |-> 5,     \* recovery control state (std), custody_recovery.rs Control.state
         OC |-> 6,     \* originals collection (std), original_ingress originals; the
                       \* collector reads the same owner's collection (collector_originals.rs)
         GO |-> 7,     \* grant owner state (std), grants/mod.rs owner
         GF |-> 8]     \* grant file writer (std), FileGrantOwnerStore writer
Locks == DOMAIN Rank
\* The earlier table ranked the exclusion first; the old-table-order profile checks
\* the code's shapes against it.
OldRank == [Rank EXCEPT !.CX = 1, !.SW = 2]
RankOf == IF Fault = "old-table-order" THEN OldRank ELSE Rank

Acq(l, how) == [kind |-> "acq", lock |-> l, how |-> how]
Rel == [kind |-> "rel", lock |-> "", how |-> ""]
Nest(l, inner) == <<Acq(l, "block")>> \o inner \o <<Rel>>
TryNest(l, inner) == <<Acq(l, "try")>> \o inner \o <<Rel>>

\* A store write: the connection, then (in A) the record-change hook's publication inside
\* the transaction (store.begin_record_change, original_ingress.rs:577-585, sqlite.rs:729).
StoreWrite(hook) == Nest("CONN", IF hook THEN Nest("PUB", <<>>) ELSE <<>>)
\* A grant change: the publication reservation, the owner state, the file append.
GrantChange == Nest("PUB", Nest("GO", Nest("GF", <<>>)))
\* An owner change: the publication writer, then the originals collection.
OwnerChange == Nest("PUB", Nest("OC", <<>>))

ShapesA == <<
    \* The shared binder on a first send, inside Meta's lifecycle lock (GovernedBind):
    \* persist under the source writer (private_originals.rs:1332, then a store CAS),
    \* the admitted check's slow path (original_ingress.rs:2723-2731), enrollment under the
    \* recovery control (custody_recovery.rs:603-617, then record_candidate).
    [name |-> "bind_first_send", section |-> "LC",
     steps |-> Nest("SW", StoreWrite(TRUE)) \o OwnerChange \o Nest("RC", OwnerChange)],
    \* FI:484's blocking controller-grant writer (issue_child).
    [name |-> "controller_grant_writer", section |-> "none", steps |-> GrantChange],
    \* Any store write elsewhere (another task's transaction with the hook).
    [name |-> "store_write", section |-> "none", steps |-> StoreWrite(TRUE)],
    \* Historical admission callback under Core's turn boundary and driver lock
    \* (recovered_publish, custody_recovery.rs:1459-1478): the control blocking, then a
    \* try of the source writer, the publication and the collection
    \* (recover_retained_custody, historical_admission.rs:424-437).
    [name |-> "historical_admission", section |-> "TB",
     steps |-> Nest("RC", TryNest("SW", TryNest("PUB", TryNest("OC", <<>>))))],
    \* Native admission read under Core's driver lock (collector.disposition,
    \* collector_originals.rs:142-146): the owner's collection, blocking.
    [name |-> "native_read", section |-> "DL", steps |-> Nest("OC", <<>>)],
    \* A class input inside the bind (class_input_basis): the source writer
    \* (class_input.rs:205), then the callback targets under the exclusion
    \* (callbacks.rs:66-67) issue the root and leaves, then the selection record's CAS
    \* (class_input.rs:284) through the store with its record-change hook.
    [name |-> "class_input", section |-> "LC",
     steps |-> Nest("SW", Nest("CX", GrantChange) \o StoreWrite(TRUE))],
    \* The native operation writer (native_operations.rs:811) on a member-context route:
    \* the source writer, then the exclusion through with_current_member
    \* (member_classes.rs:113) around the grant change.
    [name |-> "native_operation_writer", section |-> "none",
     steps |-> Nest("SW", Nest("CX", GrantChange))]
>>

\* Mode B: the same work, with the publication writer first and nothing blocking inside
\* a protected section. Each fault restores one A edge.
ShapesB == <<
    [name |-> "effect_runner", section |-> "none",
     steps |-> Nest("SW", Nest("PUB", Nest("CONN", Nest("OC", <<>>))))],
    [name |-> "store_write", section |-> "none",
     steps |-> StoreWrite(Fault = "publication-inside-store")],
    [name |-> "grant_change", section |-> "none", steps |-> GrantChange],
    \* The class preparation outside protection, in the code's order; the
    \* exclusion-before-writer mutant follows the earlier table (the exclusion first).
    [name |-> "class_prepare", section |-> "none",
     steps |-> IF Fault = "exclusion-before-writer"
               THEN Nest("CX", Nest("SW", GrantChange))
               ELSE Nest("SW", Nest("CX", GrantChange) \o StoreWrite(FALSE))],
    [name |-> "native_prepare", section |-> "none", steps |-> Nest("SW", Nest("CX", GrantChange))],
    [name |-> "recovery", section |-> "none",
     steps |-> IF Fault = "control-before-publication"
               THEN Nest("RC", OwnerChange)
               ELSE Nest("PUB", Nest("RC", Nest("OC", <<>>)))],
    [name |-> "recovery_round", section |-> "none",
     steps |-> Nest("PUB", Nest("RC", Nest("OC", <<>>)))],
    [name |-> "historical_admission", section |-> "TB",
     steps |-> IF Fault = "historical-waits" THEN Nest("RC", OwnerChange)
               ELSE TryNest("PUB", TryNest("OC", <<>>))],
    [name |-> "native_read", section |-> "DL",
     steps |-> IF Fault = "blocking-native-read" THEN Nest("OC", <<>>) ELSE <<>>],
    [name |-> "bind", section |-> "LC",
     steps |-> IF Fault = "bind-takes-lock" THEN Nest("SW", <<>>) ELSE <<>>]
>>

Shapes == IF Mode = "A" THEN ShapesA ELSE ShapesB
Actors == 1..Len(Shapes)

VARIABLES pc, held, owner, gave_up

vars == <<pc, held, owner, gave_up>>

Init ==
    /\ pc = [a \in Actors |-> 1]
    /\ held = [a \in Actors |-> <<>>]
    /\ owner = [l \in Locks |-> 0]
    /\ gave_up = {}

StepOf(a) == Shapes[a].steps[pc[a]]
Running(a) == pc[a] <= Len(Shapes[a].steps)

\* A blocking acquisition: possible only while the lock is free (otherwise the actor waits).
Block(a) ==
    /\ Running(a)
    /\ StepOf(a).kind = "acq" /\ StepOf(a).how = "block"
    /\ owner[StepOf(a).lock] = 0
    /\ owner' = [owner EXCEPT ![StepOf(a).lock] = a]
    /\ held' = [held EXCEPT ![a] = Append(@, StepOf(a).lock)]
    /\ pc' = [pc EXCEPT ![a] = @ + 1]
    /\ UNCHANGED gave_up

\* A try acquisition: takes a free lock, or refuses and the actor releases everything and
\* ends (a refusal, never a wait).
Try(a) ==
    /\ Running(a)
    /\ StepOf(a).kind = "acq" /\ StepOf(a).how = "try"
    /\ IF owner[StepOf(a).lock] = 0
       THEN /\ owner' = [owner EXCEPT ![StepOf(a).lock] = a]
            /\ held' = [held EXCEPT ![a] = Append(@, StepOf(a).lock)]
            /\ pc' = [pc EXCEPT ![a] = @ + 1]
            /\ UNCHANGED gave_up
       ELSE /\ owner' = [l \in Locks |-> IF owner[l] = a THEN 0 ELSE owner[l]]
            /\ held' = [held EXCEPT ![a] = <<>>]
            /\ pc' = [pc EXCEPT ![a] = Len(Shapes[a].steps) + 1]
            /\ gave_up' = gave_up \cup {a}

Release(a) ==
    /\ Running(a)
    /\ StepOf(a).kind = "rel"
    /\ owner' = [owner EXCEPT ![held[a][Len(held[a])]] = 0]
    /\ held' = [held EXCEPT ![a] = SubSeq(@, 1, Len(@) - 1)]
    /\ pc' = [pc EXCEPT ![a] = @ + 1]
    /\ UNCHANGED gave_up

Finished ==
    /\ \A a \in Actors : ~Running(a)
    /\ UNCHANGED vars

Next ==
    \/ \E a \in Actors : Block(a)
    \/ \E a \in Actors : Try(a)
    \/ \E a \in Actors : Release(a)
    \/ Finished

Spec == Init /\ [][Next]_vars

HeldSet(a) == {held[a][i] : i \in 1..Len(held[a])}

\* I10: an actor about to wait for a lock holds only locks ranked before it.
I10_DeclaredOrder ==
    \A a \in Actors :
        (Running(a) /\ StepOf(a).kind = "acq" /\ StepOf(a).how = "block")
            => \A h \in HeldSet(a) : RankOf[h] < RankOf[StepOf(a).lock]

\* I1: nothing waits inside a protected section (Meta's lifecycle lock during the bind,
\* Core's driver lock and turn boundary during native reads and historical admission).
I1_NoBlockingInSection ==
    \A a \in Actors :
        (Shapes[a].section # "none" /\ Running(a) /\ StepOf(a).kind = "acq")
            => StepOf(a).how = "try"

\* Mutual exclusion of the model itself.
OwnersConsistent ==
    \A l \in Locks : owner[l] # 0 => l \in HeldSet(owner[l])
=============================================================================
