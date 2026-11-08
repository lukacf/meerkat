---- MODULE bind_attempt ----
EXTENDS Naturals, Sequences, FiniteSets, TLC

\* Bind-attempt effect-mark ownership, after Stage A's meerkat_core::protected_section.
\* An outermost GovernedBind section opens one attempt (effect mark, decision point),
\* shared by everything nested inside it. Inline writers note their mark in the bind
\* task before writing; the peer door's source record precedes the section and is
\* noted at entry; each blocking writer marks through the attempt's EffectMark taken in
\* the bind and enters the attempt on its thread for the write witness only
\* (EffectMark::enter), then makes one or more writes, each after its own lookup. The
\* binder answers not-now only while its attempt is unmarked, records its decision point
\* after the admitted check, and Meta's within_marked backstop refuses a not-now returned
\* with the outer mark set. The G3 write witness (assert_effect_marked) sits at the
\* grants.change and toolkit_store funnels.
\*
\* The first-send write inventory is Stage A's, its errata included (the class input
\* selection record), plus a write no erratum lists: the service setup child's observation
\* record (service.rs:379), whose mark here is this model's proposal for Stage A.
\* A bind can end between two of its writes: dropped at an await point (a writer's join,
\* an awaited store write, a wait in place), refused by a failed write or a check after an
\* earlier write, or interrupted with its process. A dropped bind's blocking writer runs on
\* alone (a spawn_blocking task is not cancelled); the native operation and peer operation
\* writers hold the original's source writer for their whole run, as the class input path
\* and the first binding's write do, so those serialize. A later attempt then finds the
\* earlier writes and makes the next one first: the partial-issuance states. So does a
\* later original whose shared parents another original issued (a class's callback
\* grants, a native operation root): the Shared initial.
\* Stage B prepares every first-send write before the send, outside protection.
\* Contract model only: not the Stage A code, real locks, Core or the native store.

CONSTANTS Fault, StageValues, DoorValues, InitialValues, MaxAttempts, Interrupts

ASSUME MaxAttempts \in 1..4
ASSUME Interrupts \subseteq {"fail", "crash"}

Attempts == 1..MaxAttempts
\* A nested section that wrongly opens an attempt of its own (a mutant) uses these ids.
Nested(a) == MaxAttempts + a
AllAttempts == 1..(2 * MaxAttempts)

\* One first-send write: the artifact it makes, who makes it ("task" inline in the bind's
\* task, else the blocking writer's spawn site), its mark, its G3 funnel, its site, and
\* whether it is an awaited store write (an await point follows it).
W(art, writer, mark, funnel, site, store) ==
    [art |-> art, writer |-> writer, mark |-> mark, funnel |-> funnel, site |-> site, store |-> store]
CG  == W("controller_grant", "first_input.rs:486", "original.controller_grant", "grants.change",
         "FI:533", FALSE)
FB  == W("first_binding", "task", "original.first_binding", "toolkit_store.compare_and_swap",
         "PO:1610", TRUE)
CR  == W("callback_root", "task", "original.callback_grant", "grants.change", "callbacks.rs:106", FALSE)
CL  == W("callback_leaves", "task", "original.callback_grant", "grants.change", "callbacks.rs:165", FALSE)
CI  == W("class_input_selection", "task", "original.class_input", "toolkit_store.compare_and_swap",
         "class_input.rs:284", TRUE)
SC  == W("service_child", "task", "original.service_setup_grant", "grants.change", "service.rs:325", FALSE)
SCO == W("service_setup_observation", "task", "original.service_setup_observation",
         "toolkit_store.compare_and_swap", "service.rs:379", TRUE)
CSR == W("configured_setup_record", "task", "original.configured_setup", "toolkit_store.compare_and_swap",
         "configured_setup.rs:414", TRUE)
CSC == W("configured_setup_child", "configured_setup.rs:263", "original.configured_setup_grant",
         "grants.change", "configured_setup.rs:293", FALSE)
CST == W("configured_setup_targets", "configured_setup.rs:263", "original.setup_target_grant",
         "grants.change", "setup_targets.rs:154", FALSE)
SRC == W("setup_request_child", "setup_request.rs:280", "original.setup_request_grant", "grants.change",
         "setup_request.rs:318", FALSE)
SRT == W("setup_request_targets", "setup_request.rs:280", "original.setup_target_grant", "grants.change",
         "setup_targets.rs:154", FALSE)
NOR == W("native_operation_root", "native_operations.rs:804", "original.native_operation_grant",
         "grants.change", "native_operations.rs:894", FALSE)
NOC == W("native_operation_child", "native_operations.rs:804", "original.native_operation_grant",
         "grants.change", "native_operations.rs:938", FALSE)
POC == W("peer_operation_children", "peer_ingress.rs:581", "peer.operation_grant", "grants.change",
         "peer_ingress.rs:615", FALSE)

\* The first-send writes each door's bind makes before its converted sites, in order
\* (binding_for_selected_input: the class callbacks and the class input selection record,
\* then the setup request or configured setup, then the peer operations or the service
\* setup child with its observation record, then the native operations, a class route's
\* included, then FI:533 and PO:1610).
Steps(d) ==
    CASE d \in {"Find", "Launch", "Console"} -> <<CG, FB>>
      [] d = "FindNative" -> <<NOR, NOC, CG, FB>>
      [] d = "ClassInput" -> <<CR, CL, CI, CG, FB>>
      [] d = "ClassNative" -> <<CR, CL, CI, NOR, NOC, CG, FB>>
      [] d = "BatchSetup" -> <<SC, SCO, CG, FB>>
      [] d = "ConfiguredSetup" -> <<CSR, CSC, CST, CG, FB>>
      [] d = "SetupRequest" -> <<SRC, SRT, CG, FB>>
      [] d = "Peer" -> <<POC, CG, FB>>
      [] d = "Helper" -> <<>>
StepArts(d) == {Steps(d)[i].art : i \in 1..Len(Steps(d))}
AllArts == {"controller_grant", "first_binding", "callback_root", "callback_leaves", "class_input_selection",
            "service_child", "service_setup_observation", "configured_setup_record", "configured_setup_child",
            "configured_setup_targets", "setup_request_child", "setup_request_targets",
            "native_operation_root", "native_operation_child", "peer_operation_children"}
\* Writes that can fail after an earlier write of the same run succeeded (a check or the
\* grant authority between them refuses: the class selection's check, class_input.rs:280;
\* a service setup child's observation, service.rs:343): the earlier writes stay issued.
FailArts == {"callback_leaves", "class_input_selection", "service_setup_observation",
             "configured_setup_targets", "setup_request_targets", "native_operation_child"}
\* Blocking writers that hold the original's source writer for their whole run
\* (native_operations.rs:811, peer_ingress.rs:586).
SerialWriters == {"native_operations.rs:804", "peer_ingress.rs:581"}
\* Inline writes the bind makes under the source writer: the class input path holds it
\* across the callback grants and the selection record (class_input.rs:205); the first
\* binding's write takes it.
SourceWriterArts == {"callback_root", "callback_leaves", "class_input_selection", "first_binding"}

\* A blocking writer's group: the consecutive steps it makes.
RECURSIVE GroupEndFrom(_, _)
GroupEndFrom(seq, i) == IF i < Len(seq) /\ seq[i + 1].writer = seq[i].writer THEN GroupEndFrom(seq, i + 1) ELSE i

\* Post-decision writes stay marked (GS:1084, GS:1102; SH:1566 for the helper).
PostSites(d) ==
    IF d = "Helper" THEN {"SH:1566"}
    ELSE IF d \in {"BatchSetup", "Launch"} THEN {"GS:1084", "GS:1102"}
    ELSE {"GS:1102"}
PostFirst(d) ==
    IF d = "Helper" THEN "SH:1566" ELSE IF d \in {"BatchSetup", "Launch"} THEN "GS:1084" ELSE "GS:1102"
PostMark(d) ==
    IF d = "Helper" THEN "helper.custody_attempt"
    ELSE IF d \in {"BatchSetup", "Launch"} THEN "original.bound_input"
    ELSE "original.custody_attempt"
DecisionOf(d) == IF d = "Helper" THEN "helper.source" ELSE "original.admitted"

\* One note call each (the native root and child notes are separate calls with one mark
\* name, as are the callback root and leaf notes).
MarkDropped(s) ==
    \/ s.art = "callback_root" /\ Fault = "drop-callback-mark"
    \/ s.art = "callback_leaves" /\ Fault = "drop-callback-leaf-mark"
    \/ s.art = "class_input_selection" /\ Fault = "drop-class-input-mark"
    \/ s.art = "service_child" /\ Fault = "drop-service-setup-mark"
    \/ s.art = "service_setup_observation" /\ Fault = "drop-service-observation-mark"
    \/ s.art = "controller_grant" /\ Fault \in {"drop-controller-mark", "silent-blocking-writer"}
    \/ s.art = "first_binding" /\ Fault = "drop-first-binding-mark"
    \/ s.art = "configured_setup_record" /\ Fault = "drop-configured-setup-mark"
    \/ s.art = "configured_setup_child" /\ Fault = "drop-configured-setup-grant-mark"
    \/ s.art \in {"configured_setup_targets", "setup_request_targets"} /\ Fault = "drop-setup-target-mark"
    \/ s.art = "setup_request_child" /\ Fault = "drop-setup-request-grant-mark"
    \/ s.art = "native_operation_root" /\ Fault = "drop-native-root-mark"
    \/ s.art = "native_operation_child" /\ Fault = "drop-native-child-mark"
    \/ s.art = "peer_operation_children"
       /\ Fault \in {"drop-peer-operation-mark", "drop-peer-operation-and-entry-note"}
PeerNoteDropped == Fault \in {"peer-entry-unmarked", "drop-peer-operation-and-entry-note"}

\* Partial is the one-write state: every grant and record issued, the binding not saved.
\* Shared: the parents another original issued before this original's first send
\* (a class's callback grants are per class bundle, callbacks.rs:32-38; a native operation
\* root shared by originals).
SharedArts == {"callback_root", "callback_leaves", "native_operation_root"}
InitialIssued(d, i) ==
    IF i = "Bound" THEN StepArts(d)
    ELSE IF i = "Partial" THEN StepArts(d) \ {"first_binding"}
    ELSE IF i = "Shared" THEN StepArts(d) \cap SharedArts
    ELSE {}

IdleWriter == [st |-> "Idle", pos |-> 0, last |-> 0, phase |-> "lookup", handle |-> 0, vis |-> 0,
               ok |-> FALSE, dup |-> FALSE]

VARIABLES
    stage, door, initial,      \* fixed for a behavior
    issued,                    \* durable grants and records that exist
    issue_count,               \* successful issuances per artifact
    delivery_record,           \* the peer delivery's source record exists
    attempt,                   \* the current bind attempt (0 before the first)
    effect, decided, wrote,    \* per attempt: effect mark, decision point, a durable write made
    nested,                    \* the attempt the shared binder's section runs in
    pc, step,                  \* the bind task's position and its next first-send write
    after_store,               \* the task just awaited a store write (an await point)
    bw,                        \* per attempt: its current blocking writer
    gate, gate_moves,          \* the source gate seen at the converted sites
    answer, outcome,           \* the binder's answer; the send's or delivery's outcome
    offers, waited, notnow_seen, dup_refused,
    backstop, witness, silent, misplaced,
    firstw,                    \* per attempt: the site of its first durable write
    ended,                     \* per attempt: how it ended
    crashed                    \* the process was interrupted (at most once)

vars == <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided, wrote,
          nested, pc, step, after_store, bw, gate, gate_moves, answer, outcome, offers, waited,
          notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

Init ==
    /\ stage \in StageValues
    /\ door \in DoorValues
    /\ initial \in InitialValues
    /\ issued = InitialIssued(door, initial)
    /\ issue_count = [x \in AllArts |-> IF x \in InitialIssued(door, initial) THEN 1 ELSE 0]
    \* A peer original's record precedes its first binding (PI:872-876 before the shared
    \* bind): a peer original already bound, even in part, was offered before.
    /\ delivery_record = (door = "Peer" /\ initial \in {"Partial", "Bound"})
    /\ attempt = 0
    /\ effect = [a \in AllAttempts |-> ""]
    /\ decided = [a \in AllAttempts |-> ""]
    /\ wrote = [a \in AllAttempts |-> FALSE]
    /\ nested = 0
    /\ pc = "Idle"
    /\ step = 0
    /\ after_store = FALSE
    /\ bw = [a \in Attempts |-> IdleWriter]
    /\ gate \in {"Free", "Held"}
    /\ gate_moves = 0
    /\ answer = ""
    /\ outcome = ""
    /\ offers = 0
    /\ waited = FALSE
    /\ notnow_seen = FALSE
    /\ dup_refused = FALSE
    /\ backstop = FALSE
    /\ witness = {}
    /\ silent = {}
    /\ misplaced = {}
    /\ firstw = [a \in Attempts |-> ""]
    /\ ended = [a \in Attempts |-> ""]
    /\ crashed = FALSE

Cur == Steps(door)[step]
GroupEnd(i) == GroupEndFrom(Steps(door), i)
GroupArts(i) == {Steps(door)[j].art : j \in i..GroupEnd(i)}
AfterIndex(next) == IF next > Len(Steps(door)) THEN "P" ELSE "Steps"
NoteInto(s, a) == IF MarkDropped(s) \/ effect[a] # "" THEN effect ELSE [effect EXCEPT ![a] = s.mark]
FirstW(a, site) == IF firstw[a] = "" THEN [firstw EXCEPT ![a] = site] ELSE firstw
Active(w) == bw[w].st \in {"Queued", "Running"}
\* A blocking writer that holds the original's source writer is running.
SourceWriterHeld ==
    \E v \in Attempts : bw[v].st = "Running" /\ Steps(door)[bw[v].pos].writer \in SerialWriters

\* A bind attempt begins: a new send, Meta's rebind after a not-now wait, a re-offer of
\* a pending peer delivery, or a send after a dropped or interrupted bind.
Begin ==
    /\ pc \in {"Idle", "Rebind"}
    /\ attempt < MaxAttempts
    /\ attempt' = attempt + 1
    /\ step' = 1
    /\ answer' = ""
    /\ waited' = (pc = "Rebind" /\ waited)
    /\ notnow_seen' = (pc = "Rebind" /\ notnow_seen)
    /\ offers' = IF door = "Peer" /\ ~(stage = "B" /\ pc = "Rebind") THEN offers + 1 ELSE offers
    /\ pc' = IF stage = "B" THEN "Prepare" ELSE IF door = "Peer" THEN "PeerRecord" ELSE "Enter"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, effect, decided, wrote,
                   nested, after_store, bw, gate, gate_moves, outcome, dup_refused, backstop, witness,
                   silent, misplaced, firstw, ended, crashed>>

\* Stage B: every first-send write is prepared before the send, outside protection (the
\* one flip of the write inventory: every write of it, FI:533 and PO:1610; the
\* Console and peer records too). No attempt exists while they run: nothing to mark.
PrepareOutside ==
    /\ pc = "Prepare"
    /\ issued' = issued \cup StepArts(door)
    /\ issue_count' = [x \in AllArts |-> IF x \in StepArts(door) \ issued THEN issue_count[x] + 1
                                         ELSE issue_count[x]]
    /\ delivery_record' = (delivery_record \/ door = "Peer")
    /\ pc' = "Enter"
    /\ UNCHANGED <<stage, door, initial, attempt, effect, decided, wrote, nested, step, after_store, bw,
                   gate, gate_moves, answer, outcome, offers, waited, notnow_seen, dup_refused, backstop,
                   witness, silent, misplaced, firstw, ended, crashed>>

\* The peer door writes its source record before the shared bind (PI:872-876).
PeerRecord ==
    /\ pc = "PeerRecord"
    /\ delivery_record' = TRUE
    /\ firstw' = IF delivery_record THEN firstw ELSE FirstW(attempt, "PI:872-876")
    /\ pc' = "Enter"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, attempt, effect, decided, wrote, nested,
                   step, after_store, bw, gate, gate_moves, answer, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, ended, crashed>>

\* The outermost GovernedBind section opens the attempt: Meta's within_marked for a send,
\* PI's within for a peer delivery. The shared binder under the Console door runs in the
\* same attempt: a nested section reuses enclosing_attempt(). PI notes its record, written
\* before the section, at entry (PI:954).
Enter ==
    /\ pc = "Enter"
    /\ LET a == attempt
           peer_mark == stage = "A" /\ door = "Peer" /\ ~PeerNoteDropped
       IN /\ nested' = IF Fault = "nested-fresh-attempt" /\ door = "Console" THEN Nested(a) ELSE a
          /\ effect' = IF peer_mark THEN [effect EXCEPT ![a] = "peer.source_record"] ELSE effect
          /\ wrote' = [wrote EXCEPT ![a] = (stage = "A" /\ door = "Peer" /\ delivery_record)]
          /\ decided' = IF Fault = "early-decision" THEN [decided EXCEPT ![a] = DecisionOf(door)]
                        ELSE decided
          /\ pc' = IF door = "Helper" THEN "HelperCheck"
                   ELSE IF stage = "A" /\ door = "Console" THEN "ConsoleWrite"
                   ELSE IF stage = "A" /\ Len(Steps(door)) > 0 THEN "Steps"
                   ELSE "P"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, step, after_store,
                   bw, gate, gate_moves, answer, outcome, offers, waited, notnow_seen, dup_refused,
                   backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* The Console door retains its event inside the outer section (console_ingress.rs:410),
\* noting console.source_record first, then binds its delegate, the shared binder.
ConsoleWrite ==
    /\ pc = "ConsoleWrite"
    /\ LET a == attempt
           e1 == IF effect[a] = "" THEN [effect EXCEPT ![a] = "console.source_record"] ELSE effect
       IN /\ effect' = e1
          /\ wrote' = [wrote EXCEPT ![a] = TRUE]
          /\ witness' = IF e1[a] = "" /\ decided[a] = ""
                        THEN witness \cup {<<"toolkit_store", "console_ingress.rs:410", a>>} ELSE witness
          /\ misplaced' = IF decided[a] # "" THEN misplaced \cup {"console_ingress.rs:410"} ELSE misplaced
          /\ firstw' = FirstW(a, "console_ingress.rs:410")
          /\ pc' = "Steps"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, decided, nested,
                   step, after_store, bw, gate, gate_moves, answer, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, silent, ended, crashed>>

\* The lookup finds the inline artifact, or every artifact of a blocking writer's group:
\* no write and no mark (callbacks.rs:99-104, :151-156, service.rs:319-321, FI:523,
\* PO:1591-1604 and the inventory's other lookups).
SkipIssued ==
    /\ pc = "Steps"
    /\ IF Cur.writer = "task" THEN Cur.art \in issued ELSE GroupArts(step) \subseteq issued
    /\ LET next == IF Cur.writer = "task" THEN step + 1 ELSE GroupEnd(step) + 1
       IN /\ step' = next
          /\ pc' = AfterIndex(next)
    /\ after_store' = FALSE
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, bw, gate, gate_moves, answer, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* An inline first-send write in the bind task: note_effect into the section's attempt,
\* then the funnel's witness, then the durable write. A store write is awaited. A write
\* made under the source writer waits for a writer that holds it.
InlineWrite ==
    /\ pc = "Steps"
    /\ Cur.writer = "task"
    /\ Cur.art \notin issued
    /\ (Cur.art \in SourceWriterArts => ~SourceWriterHeld)
    /\ LET s == Cur
           a == attempt
           sec == nested
           e1 == NoteInto(s, sec)
       IN /\ issued' = issued \cup {s.art}
          /\ issue_count' = [issue_count EXCEPT ![s.art] = @ + 1]
          /\ effect' = e1
          /\ wrote' = [wrote EXCEPT ![a] = TRUE]
          /\ witness' = IF e1[sec] = "" /\ decided[sec] = ""
                        THEN witness \cup {<<s.funnel, s.site, a>>} ELSE witness
          /\ misplaced' = IF decided[sec] # "" THEN misplaced \cup {s.site} ELSE misplaced
          /\ firstw' = FirstW(a, s.site)
          /\ step' = step + 1
          /\ pc' = AfterIndex(step + 1)
          /\ after_store' = s.store
    /\ UNCHANGED <<stage, door, initial, delivery_record, attempt, decided, nested, bw, gate, gate_moves,
                   answer, outcome, offers, waited, notnow_seen, dup_refused, backstop, silent, ended,
                   crashed>>

\* An inline write that fails after its mark (a check or the grant authority refuses):
\* nothing is written and the bind ends refused; earlier writes of the attempt stay. The
\* callback root and its leaves are issued in one synchronous closure (callbacks.rs:83-198):
\* only such a failure or an interruption ends a bind between them, never a drop. In the
\* notnow-after-failed-write mutant the binder answers not-now there instead, with its
\* attempt's mark set: Meta's backstop must refuse it (G4).
InlineFail ==
    /\ "fail" \in Interrupts
    /\ pc = "Steps"
    /\ Cur.writer = "task"
    /\ Cur.art \notin issued
    /\ Cur.art \in FailArts
    /\ effect' = NoteInto(Cur, nested)
    /\ answer' = IF Fault = "notnow-after-failed-write" THEN "NotNow" ELSE "Refused"
    /\ pc' = "Answered"
    /\ after_store' = FALSE
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, decided, wrote,
                   nested, step, bw, gate, gate_moves, outcome, offers, waited, notnow_seen, dup_refused,
                   backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* The bind takes its attempt's handle (effect_mark()) and hands a group of writes to a
\* blocking thread (spawn_blocking, block_on inside), which enters the attempt; the bind
\* awaits its join. A writer that holds the source writer queues for it first.
SpawnWriter ==
    /\ pc = "Steps"
    /\ Cur.writer # "task"
    /\ ~(GroupArts(step) \subseteq issued)
    /\ bw' = [bw EXCEPT ![attempt] =
                 [st |-> IF Cur.writer \in SerialWriters THEN "Queued" ELSE "Running",
                  pos |-> step, last |-> GroupEnd(step), phase |-> "lookup", handle |-> nested,
                  vis |-> IF Fault = "silent-blocking-writer" /\ Cur.writer = "first_input.rs:486" THEN 0
                          ELSE nested,
                  ok |-> TRUE, dup |-> FALSE]]
    /\ pc' = "Join"
    /\ after_store' = FALSE
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, gate, gate_moves, answer, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* The writer takes the original's source writer (native_operations.rs:811,
\* peer_ingress.rs:586) and holds it until it ends.
WriterAcquire(w) ==
    /\ bw[w].st = "Queued"
    /\ ~SourceWriterHeld
    /\ bw' = [bw EXCEPT ![w] = [@ EXCEPT !.st = "Running"]]
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, pc, step, after_store, gate, gate_moves, answer, outcome, offers, waited,
                   notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* On the blocking thread, per write: the lookup; found, the next write; not found, the
\* mark through the handle before the write is attempted.
WriterLookup(w) ==
    /\ bw[w].st = "Running"
    /\ bw[w].phase = "lookup"
    /\ LET s == Steps(door)[bw[w].pos]
           found == s.art \in issued
       IN /\ effect' = IF found THEN effect ELSE NoteInto(s, bw[w].handle)
          /\ bw' = [bw EXCEPT ![w] = IF ~found THEN [@ EXCEPT !.phase = "issue"]
                                     ELSE IF @.pos = @.last THEN [@ EXCEPT !.st = "Done"]
                                     ELSE [@ EXCEPT !.pos = @ + 1]]
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, decided, wrote,
                   nested, pc, step, after_store, gate, gate_moves, answer, outcome, offers, waited,
                   notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* The write: the funnel's witness for the attempt the thread entered, then the issuance,
\* which refuses a second issuance of the same artifact (GrantRefusal::Duplicate, or a
\* stale CAS); the writer maps every refusal to Unavailable.
WriterIssue(w) ==
    /\ bw[w].st = "Running"
    /\ bw[w].phase = "issue"
    /\ LET s == Steps(door)[bw[w].pos]
           h == bw[w].handle
           vis == bw[w].vis
           dup == s.art \in issued /\ Fault # "duplicate-issue"
       IN /\ witness' = IF ~dup /\ vis # 0 /\ effect[vis] = "" /\ decided[vis] = ""
                        THEN witness \cup {<<s.funnel, s.site, w>>} ELSE witness
          /\ silent' = IF ~dup /\ vis = 0 /\ effect[h] = "" /\ decided[h] = ""
                       THEN silent \cup {s.site} ELSE silent
          /\ misplaced' = IF ~dup /\ decided[h] # "" THEN misplaced \cup {s.site} ELSE misplaced
          /\ issued' = IF dup THEN issued ELSE issued \cup {s.art}
          /\ issue_count' = IF dup THEN issue_count ELSE [issue_count EXCEPT ![s.art] = @ + 1]
          /\ wrote' = IF dup THEN wrote ELSE [wrote EXCEPT ![w] = TRUE]
          /\ firstw' = IF dup THEN firstw ELSE FirstW(w, s.site)
          /\ bw' = [bw EXCEPT ![w] =
                       IF dup THEN [@ EXCEPT !.st = "Done", !.ok = FALSE, !.dup = TRUE]
                       ELSE IF @.pos = @.last THEN [@ EXCEPT !.st = "Done"]
                       ELSE [@ EXCEPT !.pos = @ + 1, !.phase = "lookup"]]
    /\ UNCHANGED <<stage, door, initial, delivery_record, attempt, effect, decided, nested, pc, step,
                   after_store, gate, gate_moves, answer, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, ended, crashed>>

\* A later write of a run fails after its mark (a check between the writes, or the grant
\* authority, refuses): nothing written; the writer ends and the earlier writes stay.
WriterFail(w) ==
    /\ "fail" \in Interrupts
    /\ bw[w].st = "Running"
    /\ bw[w].phase = "issue"
    /\ Steps(door)[bw[w].pos].art \in FailArts
    /\ bw' = [bw EXCEPT ![w] = [@ EXCEPT !.st = "Done", !.ok = FALSE]]
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, pc, step, after_store, gate, gate_moves, answer, outcome, offers,
                   waited, notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended,
                   crashed>>

\* The bind resumes after its writer: a refused or failed write ends the bind refused.
Join ==
    /\ pc = "Join"
    /\ bw[attempt].st = "Done"
    /\ LET ok == bw[attempt].ok
           next == bw[attempt].last + 1
       IN /\ step' = IF ok THEN next ELSE step
          /\ pc' = IF ok THEN AfterIndex(next) ELSE "Answered"
          /\ answer' = IF ok THEN answer ELSE "Refused"
          /\ dup_refused' = (dup_refused \/ (~ok /\ bw[attempt].dup))
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, after_store, bw, gate, gate_moves, outcome, offers, waited, notnow_seen,
                   backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* A task spawned inside the bind does not inherit the attempt's scope, so its
\* durable write is neither marked nor witnessed. The write inventory's sweeps found no
\* such write; this action exists only in the spawned-writer profile.
SpawnedWrite ==
    /\ Fault = "spawned-writer"
    /\ stage = "A"
    /\ pc = "P"
    /\ ~wrote[attempt]
    /\ wrote' = [wrote EXCEPT ![attempt] = TRUE]
    /\ silent' = IF effect[nested] = "" /\ decided[nested] = "" THEN silent \cup {"spawned_task"}
                 ELSE silent
    /\ misplaced' = IF decided[nested] # "" THEN misplaced \cup {"spawned_task"} ELSE misplaced
    /\ firstw' = FirstW(attempt, "spawned_task")
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   nested, pc, step, after_store, bw, gate, gate_moves, answer, outcome, offers, waited,
                   notnow_seen, dup_refused, backstop, witness, ended, crashed>>

\* (P), prepare_authenticated_input's retained check: not ready answers not-now while the
\* attempt is unmarked, else the door's shipped refusal.
SiteP ==
    /\ pc = "P"
    /\ answer' = IF gate = "Held" THEN (IF effect[nested] # "" THEN "Refused" ELSE "NotNow") ELSE answer
    /\ pc' = IF gate = "Held" THEN "Answered" ELSE "A"
    /\ after_store' = FALSE
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, bw, gate, gate_moves, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* (A), prepared.admitted(): not ready answers not-now while unmarked; marked, the shipped
\* wait in place (an A baseline site, owner B). Past it, the decision point.
SiteA ==
    /\ pc = "A"
    /\ LET wait == gate = "Held" /\ ((effect[nested] # "" /\ stage = "A")
                                     \/ (stage = "B" /\ Fault = "wait-in-protected-B"))
       IN /\ pc' = IF gate = "Free" THEN "Post" ELSE IF wait THEN "WaitInPlace" ELSE "Answered"
          /\ waited' = (waited \/ wait)
          /\ answer' = IF gate = "Held" /\ ~wait THEN "NotNow" ELSE answer
          /\ decided' = IF gate = "Free" /\ decided[nested] = ""
                        THEN [decided EXCEPT ![nested] = DecisionOf(door)] ELSE decided
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, wrote,
                   nested, step, after_store, bw, gate, gate_moves, outcome, offers, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

WaitDone ==
    /\ pc = "WaitInPlace"
    /\ gate = "Free"
    /\ decided' = IF decided[nested] = "" THEN [decided EXCEPT ![nested] = DecisionOf(door)]
                  ELSE decided
    /\ pc' = "Post"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, wrote,
                   nested, step, after_store, bw, gate, gate_moves, answer, outcome, offers, waited,
                   notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* The helper submission: its source check (SH:1505) precedes every write of the send;
\* past it the decision point helper.source, then its custody attempt (SH:1566).
HelperCheck ==
    /\ pc = "HelperCheck"
    /\ answer' = IF gate = "Held" THEN "NotNow" ELSE answer
    /\ pc' = IF gate = "Held" THEN "Answered" ELSE "Post"
    /\ decided' = IF gate = "Free" /\ decided[nested] = ""
                  THEN [decided EXCEPT ![nested] = "helper.source"] ELSE decided
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, wrote,
                   nested, step, after_store, bw, gate, gate_moves, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* Writes past the decision point, still marked (note_effect "kept marked past the
\* decision"), then the answer. In the late-not-now profile a not-now site follows them.
PostWrites ==
    /\ pc = "Post"
    /\ LET a == attempt
           sec == nested
       IN /\ effect' = IF effect[sec] = "" THEN [effect EXCEPT ![sec] = PostMark(door)] ELSE effect
          /\ wrote' = [wrote EXCEPT ![a] = TRUE]
          /\ misplaced' = IF decided[sec] = "" THEN misplaced \cup PostSites(door) ELSE misplaced
          /\ firstw' = FirstW(a, PostFirst(door))
          /\ answer' = IF Fault = "late-not-now" /\ gate = "Held" THEN "NotNow" ELSE "Bound"
          /\ pc' = "Answered"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, decided, nested,
                   step, after_store, bw, gate, gate_moves, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, ended, crashed>>

\* The answer reaches its owner. Meta's within_marked returns the outer attempt's mark: a
\* not-now with it set is a binder fault (debug assertion), refused in release. PI maps
\* every non-Bound answer to a pending delivery in A; in B it waits a not-now out with
\* the request's waiter inside the same bind call. In the refused-waits mutant Meta's loop
\* treats a refusal as a not-now and waits to bind again.
Handle ==
    /\ pc = "Answered"
    /\ LET a == attempt
           kind == IF answer = "Bound" THEN "Admitted"
                   ELSE IF door = "Peer" THEN (IF stage = "B" /\ answer = "NotNow" THEN "PeerWait" ELSE "Pending")
                   ELSE IF answer = "Refused" THEN (IF Fault = "refused-waits" THEN "MetaWait" ELSE "Refused")
                   ELSE IF effect[a] # "" THEN "Backstop"
                   ELSE "MetaWait"
       IN /\ pc' = IF kind = "Admitted" THEN "Done" ELSE IF kind \in {"PeerWait", "MetaWait"} THEN kind
                   ELSE "Idle"
          /\ outcome' = IF kind \in {"Admitted", "Pending", "Refused"} THEN kind
                        ELSE IF kind = "Backstop" THEN "Refused" ELSE outcome
          /\ backstop' = (backstop \/ kind = "Backstop")
          /\ notnow_seen' = (notnow_seen \/ kind \in {"PeerWait", "MetaWait"})
          /\ ended' = [ended EXCEPT ![a] = IF kind \in {"PeerWait", "MetaWait"} THEN "NotNow"
                                           ELSE IF kind = "Backstop" THEN "Refused" ELSE kind]
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, after_store, bw, gate, gate_moves, answer, offers, waited,
                   dup_refused, witness, silent, misplaced, firstw, crashed>>

\* Meta drops the lifecycle lock and waits within the door's budget, then binds again
\* from the top; past the budget the send ends Held (nothing submitted).
MetaRebind ==
    /\ pc = "MetaWait" /\ gate = "Free" /\ attempt < MaxAttempts
    /\ pc' = "Rebind"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, after_store, bw, gate, gate_moves, answer, outcome, offers,
                   waited, notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended,
                   crashed>>
MetaExpire ==
    /\ pc = "MetaWait"
    /\ outcome' = "Held"
    /\ pc' = "Idle"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, after_store, bw, gate, gate_moves, answer, offers, waited,
                   notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* Stage B peer door: the waiter's one deadline; past it the delivery stays pending.
PeerWaiter ==
    /\ pc = "PeerWait" /\ gate = "Free" /\ attempt < MaxAttempts
    /\ pc' = "Rebind"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, after_store, bw, gate, gate_moves, answer, outcome, offers,
                   waited, notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended,
                   crashed>>
PeerWaitExpire ==
    /\ pc = "PeerWait"
    /\ outcome' = "Pending"
    /\ pc' = "Idle"
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, after_store, bw, gate, gate_moves, answer, offers, waited,
                   notnow_seen, dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

\* The bind future is dropped at an await point (a writer's join, an awaited store write,
\* a wait in place): the attempt ends without an answer. A running writer runs on alone.
DropBind ==
    /\ \/ pc \in {"Join", "WaitInPlace"}
       \/ after_store /\ pc \in {"Steps", "P"}
    /\ outcome' = "Gone"
    /\ ended' = [ended EXCEPT ![attempt] = "Gone"]
    /\ pc' = "Idle"
    /\ after_store' = FALSE
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, bw, gate, gate_moves, answer, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, firstw, crashed>>

\* The process stops (at most once): the bind and every blocking writer end where they
\* are; durable writes stay; a later send runs in the restarted process as a new attempt.
Crash ==
    /\ "crash" \in Interrupts
    /\ ~crashed
    /\ pc \notin {"Idle", "Done"} \/ \E w \in Attempts : Active(w)
    /\ crashed' = TRUE
    /\ bw' = [w \in Attempts |-> IF Active(w) THEN [bw[w] EXCEPT !.st = "Done", !.ok = FALSE] ELSE bw[w]]
    /\ ended' = IF pc \notin {"Idle", "Done"} /\ ended[attempt] = "" THEN [ended EXCEPT ![attempt] = "Crashed"]
                ELSE ended
    /\ outcome' = IF pc \notin {"Idle", "Done"} THEN "Crashed" ELSE outcome
    /\ pc' = IF pc = "Done" THEN "Done" ELSE "Idle"
    /\ after_store' = FALSE
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, step, gate, gate_moves, answer, offers, waited, notnow_seen, dup_refused,
                   backstop, witness, silent, misplaced, firstw>>

\* Another holder takes or releases the source (collection or publication), at most twice.
Hold ==
    /\ gate = "Free" /\ gate_moves < 2
    /\ gate' = "Held" /\ gate_moves' = gate_moves + 1
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, pc, step, after_store, bw, answer, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>
Release ==
    /\ gate = "Held" /\ gate_moves < 2
    /\ gate' = "Free" /\ gate_moves' = gate_moves + 1
    /\ UNCHANGED <<stage, door, initial, issued, issue_count, delivery_record, attempt, effect, decided,
                   wrote, nested, pc, step, after_store, bw, answer, outcome, offers, waited, notnow_seen,
                   dup_refused, backstop, witness, silent, misplaced, firstw, ended, crashed>>

Finished ==
    /\ \/ pc = "Done"
       \/ pc = "Idle" /\ attempt = MaxAttempts
    /\ \A w \in Attempts : ~Active(w)
    /\ UNCHANGED vars

Next ==
    \/ Begin
    \/ PrepareOutside
    \/ PeerRecord
    \/ Enter
    \/ ConsoleWrite
    \/ SkipIssued
    \/ InlineWrite
    \/ InlineFail
    \/ SpawnWriter
    \/ \E w \in Attempts : WriterAcquire(w)
    \/ \E w \in Attempts : WriterLookup(w)
    \/ \E w \in Attempts : WriterIssue(w)
    \/ \E w \in Attempts : WriterFail(w)
    \/ Join
    \/ SpawnedWrite
    \/ SiteP
    \/ SiteA
    \/ WaitDone
    \/ HelperCheck
    \/ PostWrites
    \/ Handle
    \/ MetaRebind
    \/ MetaExpire
    \/ PeerWaiter
    \/ PeerWaitExpire
    \/ DropBind
    \/ Crash
    \/ Hold
    \/ Release
    \/ Finished

Spec == Init /\ [][Next]_vars

MarkValues == {"", "original.callback_grant", "original.class_input", "original.service_setup_grant",
               "original.service_setup_observation", "original.controller_grant",
               "original.first_binding", "original.configured_setup", "original.configured_setup_grant",
               "original.setup_target_grant", "original.setup_request_grant",
               "original.native_operation_grant", "peer.operation_grant", "console.source_record",
               "peer.source_record", "original.bound_input", "original.custody_attempt",
               "helper.custody_attempt"}
TypeOK ==
    /\ attempt \in 0..MaxAttempts
    /\ effect \in [AllAttempts -> MarkValues]
    /\ decided \in [AllAttempts -> {"", "original.admitted", "helper.source"}]
    /\ wrote \in [AllAttempts -> BOOLEAN]
    /\ issued \subseteq AllArts
    /\ issue_count \in [AllArts -> 0..3]
    /\ \A w \in Attempts : bw[w].st \in {"Idle", "Queued", "Running", "Done"}
    /\ ended \in [Attempts -> {"", "Admitted", "Pending", "Refused", "NotNow", "Gone", "Crashed"}]
    /\ gate \in {"Free", "Held"}
    /\ crashed \in BOOLEAN

\* G3: no durable write runs inside a GovernedBind attempt before its decision point with
\* its mark unset (assert_effect_marked never fires).
G3_WitnessQuiet == witness = {}
\* No such write escapes both the mark and the witness (the witness cannot see it).
G3_NoSilentUnmarkedWrite == silent = {}
\* At every converted site in A the attempt's mark is set exactly when it wrote: the
\* protected slice's atomic mark-at-write assumption, discharged for nested, pre-section,
\* inline and blocking writers (one or several writes each) under drops, failures and an
\* interruption.
MarkReflectsWrites ==
    (stage = "A" /\ pc \in {"P", "A", "HelperCheck"}) => ((effect[nested] # "") <=> wrote[attempt])
\* Stage A's I4: the binder answers not-now only before its attempt's first durable write.
I4_NoNotNowAfterEffect == answer = "NotNow" => ~wrote[attempt]
\* Meta and the peer waiter never wait to bind again after a durable write.
MetaNeverWaitsAfterEffect == pc \in {"MetaWait", "PeerWait"} => ~wrote[attempt]
\* Meta's backstop (G4) never has to refuse a binder's not-now.
G4_BackstopQuiet == ~backstop
\* Writes before the admitted check see no decision; writes after it see exactly one.
DecisionPlacement == misplaced = {}
\* One issuance per artifact, even when a dropped bind's writer races a resend.
GrantUnique == \A x \in AllArts : issue_count[x] <= 1
\* Stage B never waits in place inside the bind (I1).
I1_NoWaitInProtectedB == stage = "B" => pc # "WaitInPlace"

\* Rows, as negated end states (each profile expects its own one violated: reachable).
NotRowClassFirstSendA ==
    ~(stage = "A" /\ door = "ClassInput" /\ initial = "First" /\ pc = "Done" /\ outcome = "Admitted"
      /\ attempt = 1 /\ waited /\ ~notnow_seen /\ effect[1] = "original.callback_grant")
NotRowBatchFirstSendA ==
    ~(stage = "A" /\ door = "BatchSetup" /\ initial = "First" /\ pc = "Done" /\ outcome = "Admitted"
      /\ attempt = 1 /\ waited /\ ~notnow_seen /\ effect[1] = "original.service_setup_grant")
NotRowClassFirstSendB ==
    ~(stage = "B" /\ door = "ClassInput" /\ initial = "First" /\ pc = "Done" /\ outcome = "Admitted"
      /\ notnow_seen /\ attempt = 2 /\ ~waited)
NotRowBatchFirstSendB ==
    ~(stage = "B" /\ door = "BatchSetup" /\ initial = "First" /\ pc = "Done" /\ outcome = "Admitted"
      /\ notnow_seen /\ attempt = 2 /\ ~waited)
NotRowBoundResendA ==
    ~(stage = "A" /\ door = "Find" /\ initial = "Bound" /\ pc = "Done" /\ outcome = "Admitted"
      /\ notnow_seen /\ attempt = 2)
\* A bind dropped while its blocking writer runs, then a resend whose lookup misses the
\* orphan's grant: issue_child answers Duplicate and the resend ends refused. Fail-closed
\* and retryable (the next send's lookup finds the grant); stage B must look up again on
\* Duplicate and continue only with a current issued child.
NotRowOrphanDuplicate ==
    ~(dup_refused /\ issue_count["controller_grant"] = 1 /\ outcome = "Refused")
NotRowOrphanDuplicateRetry ==
    ~(dup_refused /\ issue_count["controller_grant"] = 1 /\ outcome = "Admitted")
\* The partial-issuance states: a write that is first in a later attempt only because an
\* earlier attempt ended between its writes (dropped, refused or interrupted).
FirstAfter(site, ends) ==
    \E a \in Attempts : firstw[a] = site /\ \E b \in Attempts : b < a /\ ended[b] \in ends
NotRowLeafFirstAfterRefusal == ~FirstAfter("callbacks.rs:165", {"Refused"})
NotRowLeafFirstAfterCrash == ~FirstAfter("callbacks.rs:165", {"Crashed"})
NotRowNativeChildFirstAfterDrop == ~FirstAfter("native_operations.rs:938", {"Gone"})
NotRowNativeChildFirstAfterCrash == ~FirstAfter("native_operations.rs:938", {"Crashed"})
NotRowTargetsFirstAfterDrop == ~FirstAfter("setup_targets.rs:154", {"Gone"})
NotRowConfiguredGrantFirstAfterDrop == ~FirstAfter("configured_setup.rs:293", {"Gone"})
\* A class input's selection record is its bind's first write when the class's callback
\* grants exist (another original of the class issued them), or after the check before it
\* refused; a native child is first under a root another original issued (no drop needed).
NotRowClassSelectionFirstShared ==
    ~(stage = "A" /\ door \in {"ClassInput", "ClassNative"} /\ initial = "Shared" /\ attempt = 1
      /\ firstw[1] = "class_input.rs:284")
NotRowClassSelectionFirstAfterRefusal == ~FirstAfter("class_input.rs:284", {"Refused"})
NotRowNativeChildFirstShared ==
    ~(stage = "A" /\ door = "FindNative" /\ initial = "Shared" /\ attempt = 1
      /\ firstw[1] = "native_operations.rs:938")
\* The service setup child's observation record is first after an attempt that issued the
\* child and ended before the record: its observation refused, or the process interrupted.
NotRowServiceObservationFirstAfterRefusal == ~FirstAfter("service.rs:379", {"Refused"})
NotRowServiceObservationFirstAfterCrash == ~FirstAfter("service.rs:379", {"Crashed"})
=============================================================================
