---- MODULE member_dispatch_admission_audit ----
\* Hand-written audit of the member dispatch admission arms of the generated
\* MobMachine model.
\*
\* A host's tool dispatch admission (its approval gate) is process-local: no
\* member build copies one. Every build of a member whose lineage may run
\* under one (a fork seat, a respawn successor, and every rebuild of a member
\* seated from such a lineage) is decided by ResolveMemberDispatchAdmission
\* from the lineage MobMachine records for the input's identity at its
\* current incarnation (member_dispatch_admission_lineage, read against
\* identity_to_runtime) and two raw observations of the shell: what it
\* observed of the build's lineage (seven values) and what the host's spawn
\* customizer derived for the build (three values). Four arms answer it, in
\* each of the four phases: AskHost, Inherit, Refused and NoneRequired. The
\* lineage Unknown is a fork-derived member MobMachine holds no record for
\* (rows written before spawn rows recorded one replay so): it is built as a
\* fresh spawn the customizer answers, asking the host first and running
\* under what it derived, an admission or none, never refused. The record
\* is made by RecordMemberDispatchAdmissionLineage, for the identity's
\* current incarnation of a local member only. The DSL declares state
\* invariants only, and the generated Next is a disjunction, so
\* machine-verify never checks that the arms partition the observations,
\* that a lineage that runs under an admission is never built without one,
\* or that a record of an earlier incarnation never matches. This audit
\* checks the generated TLA guards, independently of the Rust tables.
\*
\* The verdict arms keep their phase and change only the step count; the
\* record arms keep their phase and change only the record and the step
\* count. Their guards read no state but phase, identity_to_runtime,
\* member_dispatch_admission_lineage and (the record arms) member_placement,
\* and each arm's copies in the other phases have exactly the guards of its
\* Running copy but for the phase. The runner checks that premise on the
\* generated model before it runs TLC and fails if it stops holding. So the
\* initial state (phase Running), with those three variables substituted by
\* every state the audit member can be in (RecordStates, through a
\* parameterized INSTANCE of the model), stands for every reachable state
\* of every phase, and the audit checks it alone, with ENABLED over all 21
\* observation pairs in each of them. Invariants:
\*
\*   AuditVerdictIsTotalAndDisjoint: exactly one arm is enabled for every
\*     record state and pair.
\*   AuditVerdictTable: the arm enabled is the verdict the shell mirrors: a
\*     lineage MobMachine records for the current incarnation, or one the
\*     shell observed to carry an admission (Recorded, SourceBuiltGated,
\*     SourceRebuildGated), asks the host first (AskHost), inherits what the
\*     host derived (Inherit), and is refused when the host derived none
\*     (Refused); an Unknown lineage with no current record asks the host
\*     first, inherits what it derived, and needs none when it derived none
\*     (NoneRequired); any other lineage needs none, whatever the host
\*     answered.
\*   AuditGatedLineageNeverBuiltUngated: no lineage that carries an
\*     admission, recorded or observed, enables NoneRequired.
\*   AuditStaleRecordNeverMatches: a record of an incarnation that is not
\*     the identity's current one (a stale record, or one of an identity no
\*     member holds) answers as no record does.
\*   AuditInheritNeedsDerived: Inherit is enabled only when the host derived
\*     an admission.
\*   AuditRefusedOnlyForGatedNoneDerived: Refused is enabled only for a
\*     lineage that carries an admission and a host that derived none.
\*   AuditHostAskedOnlyForGatedOrUnknownLineage: AskHost is enabled only for
\*     a lineage that carries an admission or is Unknown, before the host was
\*     asked.
\*   AuditUnknownLineageUngatedOnlyWhenTheHostDerivedNone: an Unknown
\*     lineage without a current record enables NoneRequired only once the
\*     host was asked and derived none.
\*   AuditRecordOnlyTheCurrentLocalIncarnation: the record is enabled only
\*     for the identity's current incarnation of a local member.
\*
\* The step run (AuditStepSpec) takes one classification step from the
\* initial state over every identity and pair, with TLC coverage, so the
\* runner can check that each Running verdict arm fires.
\*
\* AuditWitnessRunning is the reachability goal the runner checks as an
\* expected violation: the initial state is Running, so no invariant holds
\* vacuously. The runner's --mutants mode seeds one defect into each verdict
\* arm and each guard the record added, in a copy of the generated model,
\* and requires TLC to refuse each with the invariant it targets.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* member_dispatch_admission_audit.sh, which gives the identity and runtime
\* id constants the audit member's values:
\*   specs/machines/mob_machine/member_dispatch_admission_audit.sh --mutants
EXTENDS model

\* The audit member, its two incarnations and a host it may be placed on.
AuditMember == "audit-member"
AuditFirst == "audit-runtime-1"
AuditSecond == "audit-runtime-2"
AuditHost == "audit-host"

NoEntries == [x \in {} |-> None]
Entry(v) == [x \in {AuditMember} |-> v]

\* Every state the audit member can be in, as the verdict and the record
\* read it: whether it is seated and at which incarnation, which
\* incarnation the record names, and whether it is placed on a host.
RecordStates == {
    [seated |-> "none", recorded |-> "none", placed |-> FALSE],
    [seated |-> "first", recorded |-> "none", placed |-> FALSE],
    [seated |-> "first", recorded |-> "first", placed |-> FALSE],
    [seated |-> "second", recorded |-> "first", placed |-> FALSE],
    [seated |-> "none", recorded |-> "first", placed |-> FALSE],
    [seated |-> "first", recorded |-> "none", placed |-> TRUE]
}

Incarnation(name) == IF name = "first" THEN AuditFirst ELSE AuditSecond
Itr(s) == IF s.seated = "none" THEN NoEntries ELSE Entry(Incarnation(s.seated))
Lin(s) == IF s.recorded = "none" THEN NoEntries ELSE Entry(Incarnation(s.recorded))
Placement(s) == IF s.placed THEN Entry(AuditHost) ELSE NoEntries

\* Whether the record names the audit member's current incarnation.
Current(s) == s.seated /= "none" /\ s.recorded = s.seated

Hyp(itr, lin, placement) == INSTANCE model WITH
    identity_to_runtime <- itr,
    member_dispatch_admission_lineage <- lin,
    member_placement <- placement

\* The record writes member_dispatch_admission_lineage, so its check leaves
\* that variable free and substitutes only what its guards read.
HypRecord(itr, placement) == INSTANCE model WITH
    identity_to_runtime <- itr,
    member_placement <- placement

\* The lineages that carry an admission by the shell's observation.
GatedLineages == {"Recorded", "SourceBuiltGated", "SourceRebuildGated"}

Verdicts == {"AskHost", "Inherit", "Refused", "NoneRequired"}

\* The verdicts whose generated Running arm is enabled for one record state
\* and pair.
VerdictArms(s, lineage, host_answer) ==
    { verdict \in Verdicts :
        IF verdict = "AskHost"
        THEN ENABLED Hyp(Itr(s), Lin(s), Placement(s))!ResolveMemberDispatchAdmissionAskHostRunning(AuditMember, lineage, host_answer)
        ELSE IF verdict = "Inherit"
        THEN ENABLED Hyp(Itr(s), Lin(s), Placement(s))!ResolveMemberDispatchAdmissionInheritRunning(AuditMember, lineage, host_answer)
        ELSE IF verdict = "Refused"
        THEN ENABLED Hyp(Itr(s), Lin(s), Placement(s))!ResolveMemberDispatchAdmissionRefusedRunning(AuditMember, lineage, host_answer)
        ELSE ENABLED Hyp(Itr(s), Lin(s), Placement(s))!ResolveMemberDispatchAdmissionNoneRequiredRunning(AuditMember, lineage, host_answer) }

\* The verdict the shell mirrors for an observation alone (no current record).
ObservedVerdict(lineage, host_answer) ==
    IF lineage = "Unknown"
    THEN IF host_answer = "NotAsked" THEN "AskHost"
         ELSE IF host_answer = "Derived" THEN "Inherit"
         ELSE "NoneRequired"
    ELSE IF lineage \notin GatedLineages THEN "NoneRequired"
    ELSE IF host_answer = "NotAsked" THEN "AskHost"
    ELSE IF host_answer = "Derived" THEN "Inherit"
    ELSE "Refused"

\* The verdict the shell mirrors for one record state and pair.
ExpectedVerdict(s, lineage, host_answer) ==
    IF Current(s)
    THEN IF host_answer = "NotAsked" THEN "AskHost"
         ELSE IF host_answer = "Derived" THEN "Inherit"
         ELSE "Refused"
    ELSE ObservedVerdict(lineage, host_answer)

Pairs == MemberDispatchAdmissionLineageValues \X MemberDispatchAdmissionHostAnswerValues

Cases == RecordStates \X Pairs

Arms(c) == VerdictArms(c[1], c[2][1], c[2][2])

Gated(c) == Current(c[1]) \/ c[2][1] \in GatedLineages

AuditVerdictIsTotalAndDisjoint ==
    phase = "Running" => \A c \in Cases : Cardinality(Arms(c)) = 1

AuditVerdictTable ==
    phase = "Running" =>
        \A c \in Cases : Arms(c) = {ExpectedVerdict(c[1], c[2][1], c[2][2])}

AuditGatedLineageNeverBuiltUngated ==
    phase = "Running" => \A c \in Cases : Gated(c) => "NoneRequired" \notin Arms(c)

AuditStaleRecordNeverMatches ==
    phase = "Running" =>
        \A c \in Cases :
            ~Current(c[1]) => Arms(c) = {ObservedVerdict(c[2][1], c[2][2])}

AuditInheritNeedsDerived ==
    phase = "Running" => \A c \in Cases : "Inherit" \in Arms(c) => c[2][2] = "Derived"

AuditRefusedOnlyForGatedNoneDerived ==
    phase = "Running" =>
        \A c \in Cases :
            "Refused" \in Arms(c) => (Gated(c) /\ c[2][2] = "NoneDerived")

AuditHostAskedOnlyForGatedOrUnknownLineage ==
    phase = "Running" =>
        \A c \in Cases :
            "AskHost" \in Arms(c) =>
                ((Gated(c) \/ c[2][1] = "Unknown") /\ c[2][2] = "NotAsked")

AuditUnknownLineageUngatedOnlyWhenTheHostDerivedNone ==
    phase = "Running" =>
        \A c \in Cases :
            (c[2][1] = "Unknown" /\ "NoneRequired" \in Arms(c)) => c[2][2] = "NoneDerived"

\* The record is enabled exactly for the audit member's current incarnation
\* of a local member.
AuditRecordOnlyTheCurrentLocalIncarnation ==
    phase = "Running" =>
        \A s \in RecordStates :
            \A r \in {AuditFirst, AuditSecond} :
                (ENABLED HypRecord(Itr(s), Placement(s))!RecordMemberDispatchAdmissionLineageRunning(AuditMember, r))
                    = (~s.placed /\ s.seated /= "none" /\ Incarnation(s.seated) = r)

\* Mutant runs only: names how many cases enable no arm or several, with one
\* example of each, so a defect's gap is visible in its log. Always true.
AuditReportGaps ==
    LET none == {c \in Cases : Arms(c) = {}}
        many == {c \in Cases : Cardinality(Arms(c)) > 1}
    IN /\ PrintT(<<"cases with no arm", Cardinality(none),
                   IF none = {} THEN "none" ELSE CHOOSE c \in none : TRUE>>)
       /\ PrintT(<<"cases with several arms", Cardinality(many),
                   IF many = {} THEN "none" ELSE CHOOSE c \in many : TRUE>>)

\* The audit checks the initial state alone; the premise above makes it,
\* with the substituted record states, stand for every reachable state.
AuditSpec == Init /\ [][FALSE]_vars

\* One classification step from the initial state, over every identity and
\* pair.
AuditStepNext ==
    \E agent_identity \in AgentIdentityValues,
       lineage \in MemberDispatchAdmissionLineageValues,
       host_answer \in MemberDispatchAdmissionHostAnswerValues :
        \/ ResolveMemberDispatchAdmissionAskHostRunning(agent_identity, lineage, host_answer)
        \/ ResolveMemberDispatchAdmissionInheritRunning(agent_identity, lineage, host_answer)
        \/ ResolveMemberDispatchAdmissionRefusedRunning(agent_identity, lineage, host_answer)
        \/ ResolveMemberDispatchAdmissionNoneRequiredRunning(agent_identity, lineage, host_answer)

AuditStepSpec == Init /\ [][AuditStepNext]_vars

AuditStepConstraint == model_step_count <= 1

\* The reachability goal, checked as an expected violation.
AuditWitnessRunning == phase = "Running"
NotAuditWitnessRunning == ~AuditWitnessRunning
====
