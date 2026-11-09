---- MODULE custody_end_audit ----
\* Hand-written bounded audit of the custody end of the generated
\* GrantAuthorityMachine model: the host's close (ReleaseCustody, into the
\* terminal Released) and an append whose outcome is unknown
\* (RecordUncertainAppend, into Uncertain, which a later close releases with
\* ReleaseUncertainCustody; a second close is ReleaseCustodyAgain).
\*
\* This audit keeps the generated model unchanged and drives every generated
\* arm over the generated finite domains, as the generated Next does, through
\* wrappers that record history. Properties:
\*
\*   AuditEndedOwnerAcceptsNothing: once the custody ended (Released or
\*     Uncertain) the owner accepts no input other than a release: no
\*     configuration, issuance, revocation (new or repeated), use, spend of
\*     a use (its approver current or not), or second uncertain append.
\*   AuditReleaseEndsInReleased: after a release the owner is Released.
\*   AuditUncertainAppendEndsCustody: after an uncertain append, until a
\*     release, the owner is Uncertain.
\*   AuditCustodyEndLatches (action property): an ended custody never
\*     returns to Active, and Released never changes phase.
\*   AuditCustodyEndKeepsTheFacts (action property): the step that ends the
\*     custody and every later step keep the owner's identity, revision,
\*     records, revocations, spent uses and carried decisions, so a
\*     successor restores exactly what the store retained.
\*
\* Reachability (the runner): four scenarios are checked as expected
\* violations of their negations: a release after an issuance
\* (ReleasedAfterIssuance), an uncertain append then a release
\* (UncertainThenReleased), a second release (ReleasedTwice), and an
\* uncertain append after a revocation (UncertainAfterRevocation). With
\* --mutants the runner seeds one defect at a time into a copy of the
\* generated model and requires TLC to refuse it with the property it breaks.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* custody_end_audit.sh:
\*   specs/machines/grant_authority/custody_end_audit.sh 6 --mutants
EXTENDS model

CONSTANT AuditMaxSteps

AuditEnded == {"Released", "Uncertain"}

VARIABLES
    audit_accepted_after_end, \* an input other than a release was accepted after the custody ended
    audit_released,           \* a release was accepted
    audit_released_again,     \* a release was accepted by an owner Released already
    audit_uncertain           \* an uncertain append was recorded

audit_vars == << audit_accepted_after_end, audit_released, audit_released_again, audit_uncertain >>

all_vars == << vars, audit_vars >>

AuditInit ==
    /\ Init
    /\ audit_accepted_after_end = FALSE
    /\ audit_released = FALSE
    /\ audit_released_again = FALSE
    /\ audit_uncertain = FALSE

\* An accepted input other than a release: accepted after the custody ended,
\* it is recorded.
AcceptedNotRelease ==
    /\ audit_accepted_after_end' = (audit_accepted_after_end \/ phase \in AuditEnded)
    /\ UNCHANGED << audit_released, audit_released_again, audit_uncertain >>

AuditConfigure ==
    /\ \E arg_root \in GrantPrincipalValues : \E arg_namespace \in EvidenceIdValues :
       \E arg_generation \in 0..2 : \E arg_incarnation \in GrantAuthorityIncarnationValues :
           Configure(arg_root, arg_namespace, arg_generation, arg_incarnation)
    /\ AcceptedNotRelease

AuditIssueRoot ==
    /\ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : IssueRoot(actor, record)
    /\ AcceptedNotRelease

AuditIssueChild ==
    /\ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues :
       \E derived \in DerivedChildRestrictionsValues : \E chain \in SeqOfGrantRecordValues :
       \E now_ms \in 0..2 : \E terms_digest \in Sha256DigestValues :
           IssueChild(actor, record, derived, chain, now_ms, terms_digest)
    /\ AcceptedNotRelease

AuditRevokeNew ==
    /\ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : RevokeNew(actor, record)
    /\ AcceptedNotRelease

AuditRevokeAlready ==
    /\ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues :
           RevokeAlready(actor, record)
    /\ AcceptedNotRelease

AuditResolveUse ==
    /\ \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 :
       \E arg_incarnation \in GrantAuthorityIncarnationValues : \E executor \in GrantPrincipalValues :
       \E represented_subject \in OptionGrantPrincipalValues :
       \E requester \in OptionGrantPrincipalValues : \E work \in OptionOriginalWorkRefValues :
       \E operation \in OptionGrantOperationBindingValues : \E leaf \in GrantRecordValues :
       \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 :
           ResolveUse(arg_namespace, arg_generation, arg_incarnation, executor,
                      represented_subject, requester, work, operation, leaf, chain, now_ms)
    /\ AcceptedNotRelease

\* The spend of an approval-carried leaf's use, as the generated Next drives
\* it: its approver current (ConsumeUse) or no longer current
\* (ConsumeUseRefusedApproverRevoked, accepted with nothing spent). Either
\* arm, in every phase, accepts only a leaf that carries an approval and has
\* an entry in `spent`, so only such a leaf is tried: the same transitions,
\* without enumerating the use of a leaf no spend arm accepts.
AuditSpendLeaves == {leaf \in GrantRecordValues : leaf.approval # None /\ leaf.id \in DOMAIN spent}

AuditConsumeUse ==
    /\ \E leaf \in AuditSpendLeaves :
       \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 :
       \E arg_incarnation \in GrantAuthorityIncarnationValues : \E executor \in GrantPrincipalValues :
       \E represented_subject \in OptionGrantPrincipalValues :
       \E requester \in OptionGrantPrincipalValues : \E work \in OptionOriginalWorkRefValues :
       \E operation \in OptionGrantOperationBindingValues :
       \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 :
           ConsumeUse(arg_namespace, arg_generation, arg_incarnation, executor,
                      represented_subject, requester, work, operation, leaf, chain, now_ms,
                      TRUE)
    /\ AcceptedNotRelease

AuditConsumeUseRefusedApproverRevoked ==
    /\ \E leaf \in AuditSpendLeaves :
       \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 :
       \E arg_incarnation \in GrantAuthorityIncarnationValues : \E executor \in GrantPrincipalValues :
       \E represented_subject \in OptionGrantPrincipalValues :
       \E requester \in OptionGrantPrincipalValues : \E work \in OptionOriginalWorkRefValues :
       \E operation \in OptionGrantOperationBindingValues :
       \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 :
           ConsumeUseRefusedApproverRevoked(arg_namespace, arg_generation, arg_incarnation,
                                            executor, represented_subject, requester, work,
                                            operation, leaf, chain, now_ms, FALSE)
    /\ AcceptedNotRelease

AuditRecordUncertainAppend ==
    /\ RecordUncertainAppend
    /\ audit_accepted_after_end' = (audit_accepted_after_end \/ phase \in AuditEnded)
    /\ audit_uncertain' = TRUE
    /\ UNCHANGED << audit_released, audit_released_again >>

AuditReleaseCustody ==
    /\ ReleaseCustody
    /\ audit_released' = TRUE
    /\ UNCHANGED << audit_accepted_after_end, audit_released_again, audit_uncertain >>

AuditReleaseUncertainCustody ==
    /\ ReleaseUncertainCustody
    /\ audit_released' = TRUE
    /\ UNCHANGED << audit_accepted_after_end, audit_released_again, audit_uncertain >>

AuditReleaseCustodyAgain ==
    /\ ReleaseCustodyAgain
    /\ audit_released' = TRUE
    /\ audit_released_again' = TRUE
    /\ UNCHANGED << audit_accepted_after_end, audit_uncertain >>

AuditNext ==
    \/ AuditConfigure
    \/ AuditIssueRoot
    \/ AuditIssueChild
    \/ AuditRevokeNew
    \/ AuditRevokeAlready
    \/ AuditResolveUse
    \/ AuditConsumeUse
    \/ AuditConsumeUseRefusedApproverRevoked
    \/ AuditRecordUncertainAppend
    \/ AuditReleaseCustody
    \/ AuditReleaseUncertainCustody
    \/ AuditReleaseCustodyAgain

AuditSpec == AuditInit /\ [][AuditNext]_all_vars

AuditStateConstraint ==
    /\ model_step_count <= AuditMaxSteps
    /\ Cardinality(DOMAIN records) <= 2
    /\ Cardinality(revoked) <= 2
    /\ Cardinality(DOMAIN spent) <= 2
    /\ Cardinality(carried_decisions) <= 2

\* --- properties --------------------------------------------------------

AuditEndedOwnerAcceptsNothing == ~audit_accepted_after_end

AuditReleaseEndsInReleased == audit_released => phase = "Released"

AuditUncertainAppendEndsCustody == (audit_uncertain /\ ~audit_released) => phase = "Uncertain"

AuditCustodyEndLatches ==
    [][ /\ (phase \in AuditEnded => phase' \in AuditEnded)
        /\ (phase = "Released" => phase' = "Released") ]_all_vars

AuditCustodyEndKeepsTheFacts ==
    [][ (phase \in AuditEnded \/ phase' \in AuditEnded)
          => UNCHANGED << root, namespace, generation, incarnation, revision, records, revoked,
                          spent, consumed, carried_decisions >> ]_all_vars

\* --- witnesses (the runner requires each negation violated) -----------

NotAuditWitnessReleasedAfterIssuance == ~(phase = "Released" /\ revision > 0)

NotAuditWitnessUncertainThenReleased == ~(phase = "Released" /\ audit_uncertain)

NotAuditWitnessReleasedTwice == ~audit_released_again

NotAuditWitnessUncertainAfterRevocation == ~(phase = "Uncertain" /\ revoked # {})

=============================================================================
