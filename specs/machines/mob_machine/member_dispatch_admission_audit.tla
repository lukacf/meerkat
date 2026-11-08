---- MODULE member_dispatch_admission_audit ----
\* Hand-written audit of the member dispatch admission arms of the generated
\* MobMachine model.
\*
\* A host's tool dispatch admission (its approval gate) is process-local: no
\* member build copies one. Every build of a member whose lineage may run
\* under one (a fork seat, a respawn successor, and every rebuild of a member
\* seated from such a lineage) is decided by ResolveMemberDispatchAdmission
\* from two raw observations of the shell: what it observed of the build's
\* lineage (seven values) and what the host's spawn customizer derived for
\* the build (three values). Four arms answer it, in each of the four phases:
\* AskHost, Inherit, Refused and NoneRequired. The lineage Unknown is a
\* fork-derived member whose spawn row records none (rows written before
\* they recorded one read so): it is built as a fresh spawn the customizer
\* answers, asking the host first and running under what it derived, an
\* admission or none, never refused. The DSL declares state invariants only,
\* and the generated Next is a disjunction, so machine-verify never checks
\* that the arms partition the observations, or that a lineage that runs
\* under an admission is never built without one. This audit checks the
\* generated TLA guards, independently of the Rust tables.
\*
\* The arms are verdicts: each keeps its phase, changes only the step count,
\* and its guard reads no state but phase. The copies of each arm in the
\* Stopped, Completed and Destroyed phases have exactly the guards of its
\* Running copy but for the phase. The runner checks that premise on the
\* generated model before it runs TLC and fails if it stops holding. So the
\* initial state (phase Running) stands for every reachable state of every
\* phase, and the audit checks it alone, with ENABLED over all 21 observation
\* pairs. Invariants:
\*
\*   AuditVerdictIsTotalAndDisjoint: exactly one arm is enabled for every
\*     pair.
\*   AuditVerdictTable: the arm enabled for each pair is the verdict the
\*     shell mirrors: a lineage that carries an admission (Recorded,
\*     SourceBuiltGated, SourceRebuildGated) asks the host first (AskHost),
\*     inherits what the host derived (Inherit), and is refused when the host
\*     derived none (Refused); an Unknown lineage asks the host first,
\*     inherits what it derived, and needs none when it derived none
\*     (NoneRequired); any other lineage needs none, whatever the host
\*     answered.
\*   AuditGatedLineageNeverBuiltUngated: no lineage that carries an
\*     admission enables NoneRequired, for any host answer.
\*   AuditInheritNeedsDerived: Inherit is enabled only when the host derived
\*     an admission.
\*   AuditRefusedOnlyForGatedNoneDerived: Refused is enabled only for a
\*     lineage that carries an admission and a host that derived none.
\*   AuditHostAskedOnlyForGatedOrUnknownLineage: AskHost is enabled only for
\*     a lineage that carries an admission or is Unknown, before the host was
\*     asked.
\*   AuditUnknownLineageUngatedOnlyWhenTheHostDerivedNone: an Unknown
\*     lineage enables NoneRequired only once the host was asked and derived
\*     none.
\*
\* The step run (AuditStepSpec) takes one classification step from the
\* initial state over every pair, with TLC coverage, so the runner can check
\* that each Running arm fires.
\*
\* AuditWitnessRunning is the reachability goal the runner checks as an
\* expected violation: the initial state is Running, so no invariant holds
\* vacuously. The runner's --mutants mode seeds one defect into each verdict
\* arm of a copy of the generated model and requires TLC to refuse each with
\* the invariant it targets.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* member_dispatch_admission_audit.sh:
\*   specs/machines/mob_machine/member_dispatch_admission_audit.sh --mutants
EXTENDS model

\* The lineages that carry an admission.
GatedLineages == {"Recorded", "SourceBuiltGated", "SourceRebuildGated"}

Verdicts == {"AskHost", "Inherit", "Refused", "NoneRequired"}

\* The verdicts whose generated Running arm is enabled for one pair.
VerdictArms(lineage, host_answer) ==
    { verdict \in Verdicts :
        IF verdict = "AskHost"
        THEN ENABLED ResolveMemberDispatchAdmissionAskHostRunning(lineage, host_answer)
        ELSE IF verdict = "Inherit"
        THEN ENABLED ResolveMemberDispatchAdmissionInheritRunning(lineage, host_answer)
        ELSE IF verdict = "Refused"
        THEN ENABLED ResolveMemberDispatchAdmissionRefusedRunning(lineage, host_answer)
        ELSE ENABLED ResolveMemberDispatchAdmissionNoneRequiredRunning(lineage, host_answer) }

\* The verdict the shell mirrors for one pair.
ExpectedVerdict(lineage, host_answer) ==
    IF lineage = "Unknown"
    THEN IF host_answer = "NotAsked" THEN "AskHost"
         ELSE IF host_answer = "Derived" THEN "Inherit"
         ELSE "NoneRequired"
    ELSE IF lineage \notin GatedLineages THEN "NoneRequired"
    ELSE IF host_answer = "NotAsked" THEN "AskHost"
    ELSE IF host_answer = "Derived" THEN "Inherit"
    ELSE "Refused"

Pairs == MemberDispatchAdmissionLineageValues \X MemberDispatchAdmissionHostAnswerValues

Arms(p) == VerdictArms(p[1], p[2])

AuditVerdictIsTotalAndDisjoint ==
    phase = "Running" => \A p \in Pairs : Cardinality(Arms(p)) = 1

AuditVerdictTable ==
    phase = "Running" => \A p \in Pairs : Arms(p) = {ExpectedVerdict(p[1], p[2])}

AuditGatedLineageNeverBuiltUngated ==
    phase = "Running" =>
        \A p \in Pairs : p[1] \in GatedLineages => "NoneRequired" \notin Arms(p)

AuditInheritNeedsDerived ==
    phase = "Running" => \A p \in Pairs : "Inherit" \in Arms(p) => p[2] = "Derived"

AuditRefusedOnlyForGatedNoneDerived ==
    phase = "Running" =>
        \A p \in Pairs :
            "Refused" \in Arms(p) => (p[1] \in GatedLineages /\ p[2] = "NoneDerived")

AuditHostAskedOnlyForGatedOrUnknownLineage ==
    phase = "Running" =>
        \A p \in Pairs :
            "AskHost" \in Arms(p) =>
                (p[1] \in GatedLineages \cup {"Unknown"} /\ p[2] = "NotAsked")

AuditUnknownLineageUngatedOnlyWhenTheHostDerivedNone ==
    phase = "Running" =>
        \A p \in Pairs :
            (p[1] = "Unknown" /\ "NoneRequired" \in Arms(p)) => p[2] = "NoneDerived"

\* Mutant runs only: names how many pairs enable no arm or several, with one
\* example of each, so a defect's gap is visible in its log. Always true.
AuditReportGaps ==
    LET none == {p \in Pairs : Arms(p) = {}}
        many == {p \in Pairs : Cardinality(Arms(p)) > 1}
    IN /\ PrintT(<<"pairs with no arm", Cardinality(none),
                   IF none = {} THEN "none" ELSE CHOOSE p \in none : TRUE>>)
       /\ PrintT(<<"pairs with several arms", Cardinality(many),
                   IF many = {} THEN "none" ELSE CHOOSE p \in many : TRUE>>)

\* The audit checks the initial state alone; the premise above makes it
\* stand for every reachable state.
AuditSpec == Init /\ [][FALSE]_vars

\* One classification step from the initial state, over every pair.
AuditStepNext ==
    \E lineage \in MemberDispatchAdmissionLineageValues,
       host_answer \in MemberDispatchAdmissionHostAnswerValues :
        \/ ResolveMemberDispatchAdmissionAskHostRunning(lineage, host_answer)
        \/ ResolveMemberDispatchAdmissionInheritRunning(lineage, host_answer)
        \/ ResolveMemberDispatchAdmissionRefusedRunning(lineage, host_answer)
        \/ ResolveMemberDispatchAdmissionNoneRequiredRunning(lineage, host_answer)

AuditStepSpec == Init /\ [][AuditStepNext]_vars

AuditStepConstraint == model_step_count <= 1

\* The reachability goal, checked as an expected violation.
AuditWitnessRunning == phase = "Running"
NotAuditWitnessRunning == ~AuditWitnessRunning
====
