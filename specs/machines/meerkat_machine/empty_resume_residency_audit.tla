---- MODULE empty_resume_residency_audit ----
\* Hand-written bounded audit of the empty-resume retirement verdict.
\*
\* A recovered registration whose resume found nothing to run asks the
\* machine whether to retire it (ResolveEmptyResumeRetirement). A
\* Session-owned registration with nothing queued retires: that is the
\* standalone recovery teardown. Queued input, or a Composition-owned
\* registration, keeps it serving: the composition owns a mob member's
\* residency, so an empty resume never unloads a seated member. The owner is
\* typed machine state (residency_owner): a placement through the
\* composition's binding route claims the registration (PrepareBindings with
\* the Composition owner, sticky for the registration's lifetime), the
\* composition's boot recovery re-claims the exact recovered binding
\* (ClaimCompositionResidency), and a standalone placement never downgrades
\* it.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and drives,
\* for one session and one runtime binding, the initialization, the
\* registration, the placement with either owner (the fresh binding and its
\* exact re-assertion), the composition's claim, and the verdict with and
\* without queued input in Idle and Attached. Properties:
\*
\*   every generated invariant of ci.cfg (kept);
\*   AuditVerdictExactlyOne (invariant): in Idle and Attached exactly one
\*     verdict arm answers each queued-input observation;
\*   AuditRetireOnlySessionOwned (action property): the Retire verdict fires
\*     only for a Session-owned registration with nothing queued;
\*   AuditPlacementOwnership (action property): a Composition placement
\*     leaves the registration Composition-owned; a Session placement leaves
\*     its owner as it was.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. With --mutants
\* the runner also checks that two seeded defects are refused: the Retire
\* verdict without its owner guard (an empty resume would unload a seated mob
\* member) and a placement that records no owner.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* empty_resume_residency_audit.sh.
EXTENDS model

CONSTANT AuditMaxSteps

AuditSession == "sessionid_1"
AuditRuntime == "runtime_1"
AuditOwners == {"Session", "Composition"}

\* The placement with either owner: the fresh binding (Idle to Attached) and
\* its exact re-assertion (the idempotent arm, which may still claim).
AuditPlace(owner) ==
    \/ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession, owner)
    \/ PrepareBindingsIdempotentAttached(AuditRuntime, 1, Some(1), None, AuditSession, owner)

\* The composition's claim of the exact recovered binding.
AuditClaim == ClaimCompositionResidencyAttached(AuditSession, AuditRuntime, 1, Some(1))

AuditRetire(queued) ==
    \/ ResolveEmptyResumeRetirementRetireIdle(queued)
    \/ ResolveEmptyResumeRetirementRetireAttached(queued)

AuditKeepServing(queued) ==
    \/ ResolveEmptyResumeRetirementKeepServingIdle(queued)
    \/ ResolveEmptyResumeRetirementKeepServingAttached(queued)

AuditNext ==
    \/ Initialize
    \/ RegisterSessionIdle(AuditSession, None, {})
    \/ \E owner \in AuditOwners : AuditPlace(owner)
    \/ AuditClaim
    \/ \E queued \in BOOLEAN : AuditRetire(queued) \/ AuditKeepServing(queued)

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* Exactly one verdict arm answers each queued-input observation.
AuditVerdictExactlyOne ==
    phase \in {"Idle", "Attached"} =>
        \A queued \in BOOLEAN :
            (ENABLED AuditRetire(queued)) # (ENABLED AuditKeepServing(queued))

\* The Retire verdict fires only for a Session-owned registration with
\* nothing queued.
AuditRetireOnlySessionOwned ==
    [][\A queued \in BOOLEAN :
         AuditRetire(queued) => (~queued /\ residency_owner = "Session")
      ]_vars

\* A Composition placement claims the registration; a Session placement never
\* downgrades it.
AuditPlacementOwnership ==
    [][\A owner \in AuditOwners :
         AuditPlace(owner) =>
             residency_owner' = (IF owner = "Composition" THEN "Composition" ELSE residency_owner)
      ]_vars

\* Reachability goals (step goals), each checked as an expected violation of
\* its negation.
\* A Session-owned registration with nothing queued retires.
AuditWitnessSessionOwnedRetiresStep ==
    residency_owner = "Session" /\ ResolveEmptyResumeRetirementRetireAttached(FALSE)
\* A Composition-owned registration with nothing queued keeps serving.
AuditWitnessCompositionKeepsServingStep ==
    residency_owner = "Composition" /\ ResolveEmptyResumeRetirementKeepServingAttached(FALSE)
\* Queued input keeps a Session-owned registration serving.
AuditWitnessQueuedKeepsServingStep ==
    residency_owner = "Session" /\ ResolveEmptyResumeRetirementKeepServingAttached(TRUE)
\* The composition's claim moves a Session-owned binding to Composition.
AuditWitnessCompositionClaimedStep ==
    residency_owner = "Session" /\ AuditClaim

NotAuditWitnessSessionOwnedRetires == [][~AuditWitnessSessionOwnedRetiresStep]_vars
NotAuditWitnessCompositionKeepsServing == [][~AuditWitnessCompositionKeepsServingStep]_vars
NotAuditWitnessQueuedKeepsServing == [][~AuditWitnessQueuedKeepsServingStep]_vars
NotAuditWitnessCompositionClaimed == [][~AuditWitnessCompositionClaimedStep]_vars
====
