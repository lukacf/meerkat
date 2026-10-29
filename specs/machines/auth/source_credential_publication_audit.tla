---- MODULE source_credential_publication_audit ----
\* Hand-written bounded audit of the AuthMachine's source-credential
\* publication: the fingerprint of the credential value the lease's current
\* publication carries (credential_material_fingerprint), the Acquire arms
\* (Acquire with its guard credential_material_publishable outside a run-out
\* window, AcquireFromExpiring and AcquireFromExpired inside one), and the
\* read-only classifier ResolveSourceCredentialPublication.
\*
\* A publisher that re-reads its credential value from a source (an
\* environment variable, a command, an external resolver) asks the
\* classifier with the value's fingerprint and fires Acquire naming the value
\* only on its Publish verdict. A value its consumer refused
\* (ReauthRequired) or an operator logged out (Released) must never be
\* published again while the lease records it; a different value heals the
\* lease, and a declared freshness window that ran out (Expiring, Expired)
\* renews the same value.
\*
\* This audit keeps the generated AuthMachine model unchanged and explores
\* every generated action over two credential values ("alpha", "beta"),
\* with OAuth flow membership left out by the state constraint (no flow
\* action reads or writes the fingerprint). Properties:
\*
\*   AuditWithdrawnMaterialNeverRepublished (action property): from a
\*     ReauthRequired or Released lease that records a value, no step reaches
\*     Valid with that same value recorded.
\*   AuditVerdictMatchesAcquire (invariant): in every reachable state and for
\*     every value, exactly one classifier verdict is enabled, and it is
\*     Publish exactly when an Acquire arm naming the value is enabled, so a
\*     publisher that mirrors the verdict fires exactly the Acquire the
\*     machine accepts.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous: a refused value
\* healed by another, a logged-out value healed by another, a declared window
\* that ran out renewing the same value, and a refused and a logged-out value
\* whose Acquire is disabled while another value's is enabled. With
\* --mutants the runner seeds two defects into copies of the generated model
\* and requires TLC to refuse each: Acquire without its guard (refused by the
\* action property), and the renewal arms AcquireFromExpiring and
\* AcquireFromExpired disabled (refused by AuditVerdictMatchesAcquire).
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* source_credential_publication_audit.sh. Run a deeper bound by hand with:
\*   specs/machines/auth/source_credential_publication_audit.sh 8
EXTENDS model

CONSTANT AuditMaxSteps

VARIABLES
    audit_refusal_healed, \* a refused value was followed by a different one
    audit_logout_healed,  \* a logged-out value was followed by a different one
    audit_window_renewed  \* a run-out window republished the same value

auditVars == <<audit_refusal_healed, audit_logout_healed, audit_window_renewed>>

\* A lease that records the value it carries and is refused or logged out.
AuditWithdrawn ==
    /\ phase \in {"ReauthRequired", "Released"}
    /\ credential_material_fingerprint # None

\* A step that publishes a credential: a new generation, valid.
AuditPublished ==
    /\ credential_generation' > credential_generation
    /\ phase' = "Valid"

\* A step from a lease in `from` that records a value to a publication
\* naming a different value.
AuditHealedFrom(from) ==
    /\ phase = from
    /\ credential_material_fingerprint # None
    /\ AuditPublished
    /\ credential_material_fingerprint' # None
    /\ credential_material_fingerprint' # credential_material_fingerprint

AuditRenewedSameValue ==
    /\ phase \in {"Expiring", "Expired"}
    /\ credential_material_fingerprint # None
    /\ AuditPublished
    /\ credential_material_fingerprint' = credential_material_fingerprint

AuditInit ==
    /\ Init
    /\ audit_refusal_healed = FALSE
    /\ audit_logout_healed = FALSE
    /\ audit_window_renewed = FALSE

AuditNext ==
    /\ Next
    /\ audit_refusal_healed' = (audit_refusal_healed \/ AuditHealedFrom("ReauthRequired"))
    /\ audit_logout_healed' = (audit_logout_healed \/ AuditHealedFrom("Released"))
    /\ audit_window_renewed' = (audit_window_renewed \/ AuditRenewedSameValue)

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint ==
    /\ model_step_count <= AuditMaxSteps
    /\ oauth_outstanding_flow_count = 0

AuditWithdrawnMaterialNeverRepublished ==
    [][AuditWithdrawn
       => ~(phase' = "Valid"
            /\ credential_material_fingerprint' = credential_material_fingerprint)]_<<vars, auditVars>>

\* The classifier's arms, one per phase (the per_phase expansion of the
\* catalog transitions).
AuditVerdictPublish(m) ==
    \/ ENABLED ResolveSourceCredentialPublicationNewMaterialValid(m)
    \/ ENABLED ResolveSourceCredentialPublicationNewMaterialExpiring(m)
    \/ ENABLED ResolveSourceCredentialPublicationNewMaterialExpired(m)
    \/ ENABLED ResolveSourceCredentialPublicationNewMaterialRefreshing(m)
    \/ ENABLED ResolveSourceCredentialPublicationNewMaterialReauthRequired(m)
    \/ ENABLED ResolveSourceCredentialPublicationNewMaterialReleased(m)
    \/ ENABLED ResolveSourceCredentialPublicationRenewalExpiring(m)
    \/ ENABLED ResolveSourceCredentialPublicationRenewalExpired(m)

AuditVerdictAlreadyPublished(m) ==
    \/ ENABLED ResolveSourceCredentialPublicationAlreadyPublishedValid(m)
    \/ ENABLED ResolveSourceCredentialPublicationAlreadyPublishedRefreshing(m)
    \/ ENABLED ResolveSourceCredentialPublicationAlreadyPublishedReauthRequired(m)
    \/ ENABLED ResolveSourceCredentialPublicationAlreadyPublishedReleased(m)

\* Some arm of Acquire accepts a publication naming `fingerprint` (an option;
\* the expiry and publication time enter no guard): Acquire outside a run-out
\* window, AcquireFromExpiring and AcquireFromExpired inside one. The
\* parameter is not called `value`: TLC orders record fields by the order it
\* first meets their names, and a `value` met before `tag` makes it compare
\* None's string value with Some's number first, which it refuses.
AuditAcquireEnabled(fingerprint) ==
    \/ ENABLED Acquire(None, 1, fingerprint)
    \/ ENABLED AcquireFromExpiring(None, 1, fingerprint)
    \/ ENABLED AcquireFromExpired(None, 1, fingerprint)

\* Acquire naming value `m`.
AuditAcquireAdmits(m) == AuditAcquireEnabled(Some(m))

AuditVerdictMatchesAcquire ==
    \A m \in StringValues :
        /\ AuditVerdictPublish(m) # AuditVerdictAlreadyPublished(m)
        /\ AuditVerdictPublish(m) = AuditAcquireAdmits(m)

\* Reachability goals, each checked as an expected violation of its negation.
AuditWitnessRefusalHealed == audit_refusal_healed
AuditWitnessLogoutHealed == audit_logout_healed
AuditWitnessWindowRenewed == audit_window_renewed

AuditValueBlockedIn(from) ==
    /\ phase = from
    /\ credential_material_fingerprint # None
    /\ ~AuditAcquireEnabled(credential_material_fingerprint)
    /\ \E m \in StringValues :
        /\ Some(m) # credential_material_fingerprint
        /\ AuditAcquireAdmits(m)

AuditWitnessRefusedValueBlocked == AuditValueBlockedIn("ReauthRequired")
AuditWitnessLoggedOutValueBlocked == AuditValueBlockedIn("Released")

NotAuditWitnessRefusalHealed == ~AuditWitnessRefusalHealed
NotAuditWitnessLogoutHealed == ~AuditWitnessLogoutHealed
NotAuditWitnessWindowRenewed == ~AuditWitnessWindowRenewed
NotAuditWitnessRefusedValueBlocked == ~AuditWitnessRefusedValueBlocked
NotAuditWitnessLoggedOutValueBlocked == ~AuditWitnessLoggedOutValueBlocked
====
