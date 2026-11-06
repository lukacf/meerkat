---- MODULE approval_grant_audit ----
\* Hand-written bounded audit of the generated GrantAuthorityMachine model:
\* approval-carried child grants.
\*
\* A grant under a child rule has children only through a person's decision
\* for the rule's approver id. Such a child carries the decision's terms:
\* the approved request's digest, the one requester and the one work that
\* may use it, the exact operation, and how many uses it serves. Each use is
\* spent in the owner's journal before it enters. The generated model is
\* the owner; this audit keeps it unchanged and drives it through wrappers
\* that read each step's input and record what the step did.
\*
\* The environment. The configured root "root" issues one parent grant
\* "parent" to "owner": without a child rule, under a rule for the approver
\* id "approver" that refuses device trust, or under one that accepts it.
\* "owner" asks for children of it, each for the approval branch "branch",
\* with a window that ends at 2: "c1" carrying the valid approval or the
\* same with exactly one defect (a device-trusted or a service-credential
\* decision, another approver id, a review-tier decision), at depth 0 or 1,
\* issued with the approved request's terms digest or another; "c2"
\* carrying the valid approval's decision; and a plain "c1". The valid
\* approval is decision "d1" by "approver", human tier, the person
\* authenticated, for request digest "digest_a", requester "req_a", work
\* "work_a" and operation "op_a", one use. A use names its leaf, requester,
\* work and operation, a clock reading in 0..3 and the host's answer
\* whether the decision's approver is still current: the valid use at 0 or
\* 1, or the same with exactly one defect (another or no requester, another
\* or no work, another or no operation, a reading at 2 or 3, an approver no
\* longer current). The root may revoke the parent or a child.
\*
\* The owner's journal. Every change the owner applies is acknowledged in
\* its journal first (audit_durable holds the acknowledged state), except an
\* uncertain spend: its write may or may not have landed, the owner fails
\* closed (it applies nothing more), and the use does not enter. A restart
\* rebuilds the owner from its journal; the audit models the rebuild as the
\* acknowledged state, which is what the owner's rebuild computes by folding
\* the journal through this same machine. One restart is modeled.
\*
\* Two concurrent attempts. An attempt prepares the valid use of a leaf (a
\* resolution, at the revision it read) and enters (spends) only while that
\* revision is current; one prepared before a change is stale, and prepares
\* again. An attempt's executor keeps that discipline: the audit assumes
\* it, as the governed admission's final currentness check does.
\*
\* Properties. Each step property is checked on every step by its wrapper,
\* which records the property in audit_broken when the step breaks it; the
\* invariant of the same name requires that it never was:
\*
\*   AuditUseOnlyExactOperation: a resolution or a spend of an approval leaf
\*     names the leaf's bound operation exactly.
\*   AuditUseOnlyBoundRequesterAndWork: a resolution or a spend of an
\*     approval leaf names the leaf's own requester and work.
\*   AuditNoChildWithoutApprovalUnderRule: a child of a grant under a rule is
\*     issued only with an approval that meets the rule: a human-tier
\*     decision for the rule's approver id, the person authenticated, or
\*     device-trusted where the rule accepts it.
\*   AuditRuleAssuranceHolds: an approval child under a rule is issued only
\*     with an assurance the rule accepts: the person authenticated, or
\*     device-trusted where the rule accepts it; a service credential never.
\*   AuditNoApprovalChildWithoutRule: an approval child is issued only under
\*     a grant with a rule.
\*   AuditIssuedTermsAreApproved: an approval child is issued only with the
\*     approved request's terms (its terms digest is the approval's request
\*     digest), at depth 0, in a window, with at least one use.
\*   AuditNoSpendAfterApproverRevoked: a use is spent only while the
\*     decision's approver is current; for one who is not, nothing is spent.
\*   AuditSpendAdvancesPublication: a spend advances the owner's revision,
\*     so every decision prepared before it is stale.
\* and three state invariants:
\*   AuditNoUseBeyondUses: the uses spent under an approval child, and the
\*     uses that entered under it, never exceed its uses.
\*   AuditOneGrantPerDecision: no two grants carry the same decision.
\*   AuditSpentSurvivesRestart: every use that entered is spent in the
\*     journal and in the owner, so a restart keeps it spent.
\*
\* Reachability (the runner): every wrapper fires in the bounded space,
\* checked from TLC's coverage, and fourteen scenarios are checked as
\* expected violations of their negations: an approval child issued; a use
\* entered once; a second use refused; a use with another operation,
\* another requester or another work refused, and one after the window,
\* each where the valid use is open; a use refused after the parent was
\* revoked; a spend refused for an approver no longer current, nothing
\* spent; a device-trusted decision refused under a rule that refuses device
\* trust, and a plain child refused under a rule, each where the valid child
\* is open; a spent use still spent after a restart; two attempts prepared
\* at one revision, one entered and the other stale and refused; and an
\* uncertain spend that landed, never entered and still spent after the
\* restart. With --mutants the runner seeds one defect per property into a
\* copy of the generated model (or of this audit's journal) and requires TLC
\* to refuse each with that property.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* approval_grant_audit.sh:
\*   specs/machines/grant_authority/approval_grant_audit.sh 6 --mutants
EXTENDS model

CONSTANT AuditMaxSteps

AuditRoot == "root"
AuditOwner == "owner"
AuditBranch == "branch"
AuditRequester == "req_a"
AuditOtherRequester == "req_b"
AuditWork == "work_a"
AuditOtherWork == "work_b"
AuditDigest == "digest_a"
AuditOtherDigest == "digest_b"
AuditOperation == "op_a"
AuditOtherOperation == "op_b"
AuditApprover == "approver"
AuditOtherApprover == "other_approver"
AuditDecision == "d1"
AuditNamespace == "ns"
AuditGeneration == 1
AuditIncarnation == "inc"
AuditParentId == "parent"
AuditChildIds == {"c1", "c2"}
AuditSlots == {1, 2}

VARIABLES
    audit_broken,     \* the step properties some step broke
    audit_seen,       \* the scenario marks the witnesses read
    audit_entries,    \* per child, the uses that entered under it
    audit_durable,    \* the state the owner's journal acknowledged
    audit_poisoned,   \* an uncertain journal write: the owner fails closed
    audit_restarted,  \* the one restart happened
    audit_slots       \* the two concurrent attempts

auditVars == << audit_broken, audit_seen, audit_entries, audit_durable, audit_poisoned,
                audit_restarted, audit_slots >>

\* --- values -------------------------------------------------------------

\* Restrictions with no action, resource, processor or audience bound, at a
\* depth of `edges`, in the window 0..expires.
AuditRestrictions(edges, expires) ==
    [actions |-> "unrestricted_actions",
     resource_domains |-> "unrestricted_resources",
     processors |-> "unrestricted_processors",
     audiences |-> "unrestricted_audiences",
     lifetime |-> [bound |-> [tag |-> "Window", not_before_ms |-> 0, expires_at_ms |-> expires],
                   not_before_ms |-> 0, expires_at_ms |-> expires, unresolved |-> {}],
     delegation_depth |-> [bound |-> [tag |-> "Remaining", edges |-> edges],
                           remaining_edges |-> edges, unresolved |-> {}]]

\* The parent is current at every reading; a child's window ends at 2.
AuditParentRestrictions == AuditRestrictions(2, 4)
AuditChildRestrictions == AuditRestrictions(0, 2)
AuditDeepChildRestrictions == AuditRestrictions(1, 2)

AuditRule(device_trust) == [approver_id |-> AuditApprover, accepts_device_trust |-> device_trust]

AuditParentRules == {None, Some(AuditRule("Refused")), Some(AuditRule("Accepted"))}

AuditParent(rule) ==
    [id |-> AuditParentId, authority_incarnation |-> AuditIncarnation, parent |-> None,
     issuer |-> AuditRoot, grantee |-> AuditOwner, represented_subject |-> None,
     issued_revision |-> revision + 1, restrictions |-> AuditParentRestrictions,
     child_approval |-> rule, approval |-> None]

AuditValidApproval ==
    [decision_id |-> AuditDecision, approver_id |-> AuditApprover, tier |-> "Human",
     assurance |-> "PersonAuthenticated", request_digest |-> AuditDigest,
     requester |-> AuditRequester, work |-> AuditWork, operation |-> Some(AuditOperation),
     uses |-> 1, review_before_return |-> None]

\* The valid approval, and the same with exactly one defect.
AuditApprovalShapes ==
    { AuditValidApproval,
      [AuditValidApproval EXCEPT !.assurance = "DeviceTrusted"],
      [AuditValidApproval EXCEPT !.assurance = "ServiceCredential"],
      [AuditValidApproval EXCEPT !.approver_id = AuditOtherApprover],
      [AuditValidApproval EXCEPT !.tier = "Review"] }

AuditIssueOf(id, approval, restrictions, terms) ==
    [id |-> id, approval |-> approval, restrictions |-> restrictions, terms |-> terms]

AuditValidIssue == AuditIssueOf("c1", Some(AuditValidApproval), AuditChildRestrictions, AuditDigest)

AuditIssueInputs ==
    { AuditIssueOf("c1", Some(a), r, t) :
        a \in AuditApprovalShapes,
        r \in {AuditChildRestrictions, AuditDeepChildRestrictions},
        t \in {AuditDigest, AuditOtherDigest} }
    \cup { AuditIssueOf("c2", Some(AuditValidApproval), AuditChildRestrictions, AuditDigest),
           AuditIssueOf("c1", None, AuditChildRestrictions, AuditDigest) }

AuditChild(in) ==
    [id |-> in.id, authority_incarnation |-> AuditIncarnation, parent |-> Some(AuditParentId),
     issuer |-> AuditOwner, grantee |-> AuditBranch, represented_subject |-> None,
     issued_revision |-> revision + 1, restrictions |-> in.restrictions,
     child_approval |-> None, approval |-> in.approval]

AuditDerived(in) ==
    [parent |-> AuditParentRestrictions, requested |-> in.restrictions,
     effective |-> in.restrictions]

AuditValidUse ==
    [requester |-> Some(AuditRequester), work |-> Some(AuditWork),
     operation |-> Some(AuditOperation), now_ms |-> 1, approver_current |-> TRUE]

\* The valid use at a reading in the window, and the same with exactly one
\* defect.
AuditUseShapes ==
    { [AuditValidUse EXCEPT !.now_ms = n] : n \in 0..1 }
    \cup { [AuditValidUse EXCEPT !.requester = r] : r \in {None, Some(AuditOtherRequester)} }
    \cup { [AuditValidUse EXCEPT !.work = w] : w \in {None, Some(AuditOtherWork)} }
    \cup { [AuditValidUse EXCEPT !.operation = o] : o \in {None, Some(AuditOtherOperation)} }
    \cup { [AuditValidUse EXCEPT !.now_ms = n] : n \in 2..3 }
    \cup { [AuditValidUse EXCEPT !.approver_current = FALSE] }

AuditUseOf(leaf, shape) ==
    [leaf |-> leaf, requester |-> shape.requester, work |-> shape.work,
     operation |-> shape.operation, now_ms |-> shape.now_ms,
     approver_current |-> shape.approver_current]

AuditUseInputs == { AuditUseOf(leaf, shape) : leaf \in AuditChildIds, shape \in AuditUseShapes }

\* --- the generated arms, applied to an input ----------------------------

AuditIssuable == AuditParentId \in DOMAIN records

IssueChildArm(Arm(_, _, _, _, _, _), in) ==
    Arm(AuditOwner, AuditChild(in), AuditDerived(in), <<records[AuditParentId]>>, 0, in.terms)

AuditUsable(in) == {AuditParentId, in.leaf} \subseteq DOMAIN records

AuditChainOf(in) == <<records[AuditParentId], records[in.leaf]>>

ResolveArm(Arm(_, _, _, _, _, _, _, _, _, _, _), in) ==
    Arm(AuditNamespace, AuditGeneration, AuditIncarnation, AuditBranch, None, in.requester,
        in.work, in.operation, records[in.leaf], AuditChainOf(in), in.now_ms)

ConsumeArm(Arm(_, _, _, _, _, _, _, _, _, _, _, _), in) ==
    Arm(AuditNamespace, AuditGeneration, AuditIncarnation, AuditBranch, None, in.requester,
        in.work, in.operation, records[in.leaf], AuditChainOf(in), in.now_ms,
        in.approver_current)

\* --- the journal --------------------------------------------------------

AuditSnapshot(r, v, s, c, d, n) ==
    [records |-> r, revoked |-> v, spent |-> s, consumed |-> c, carried_decisions |-> d,
     revision |-> n]

AuditSnapshotAfter ==
    AuditSnapshot(records', revoked', spent', consumed', carried_decisions', revision')

\* An issue or a revocation is acknowledged before the owner applies it.
AuditJournalChange == audit_durable' = AuditSnapshotAfter

\* A spend is acknowledged before the owner applies it and the use enters.
AuditJournalConsumed ==
    audit_durable' = AuditSnapshotAfter

\* --- step properties ----------------------------------------------------

AuditValueOf(option) == option["value"]

AuditAssuranceAccepted(rule, approval) ==
    \/ approval.assurance = "PersonAuthenticated"
    \/ approval.assurance = "DeviceTrusted" /\ rule.accepts_device_trust = "Accepted"

\* An issue of the child `in` names, under the retained parent.
AuditIssueBreaks(in) ==
    LET parent == records[AuditParentId]
        rule == AuditValueOf(parent.child_approval)
        approval == AuditValueOf(in.approval)
    IN (IF /\ parent.child_approval # None
           /\ ~(/\ in.approval # None
                /\ approval.approver_id = rule.approver_id
                /\ approval.tier = "Human"
                /\ AuditAssuranceAccepted(rule, approval))
        THEN {"NoChildWithoutApprovalUnderRule"} ELSE {})
       \cup (IF /\ parent.child_approval # None
                /\ in.approval # None
                /\ ~AuditAssuranceAccepted(rule, approval)
             THEN {"RuleAssuranceHolds"} ELSE {})
       \cup (IF in.approval # None /\ parent.child_approval = None
             THEN {"NoApprovalChildWithoutRule"} ELSE {})
       \cup (IF /\ in.approval # None
                /\ ~(/\ in.terms = approval.request_digest
                     /\ in.restrictions.delegation_depth.bound.tag = "Remaining"
                     /\ in.restrictions.delegation_depth.remaining_edges = 0
                     /\ in.restrictions.lifetime.bound.tag = "Window"
                     /\ approval.uses >= 1)
             THEN {"IssuedTermsAreApproved"} ELSE {})

\* A resolution or a spend of the use `in`, on the retained leaf.
AuditUseBreaks(in) ==
    LET leaf == records[in.leaf]
        approval == AuditValueOf(leaf.approval)
    IN IF leaf.approval = None THEN {}
       ELSE (IF in.requester = Some(approval.requester) /\ in.work = Some(approval.work)
             THEN {} ELSE {"UseOnlyBoundRequesterAndWork"})
            \cup (IF approval.operation = None \/ in.operation = approval.operation
                  THEN {} ELSE {"UseOnlyExactOperation"})

\* A spend of the use `in`.
AuditSpendBreaks(in) ==
    (IF in.approver_current THEN {} ELSE {"NoSpendAfterApproverRevoked"})
    \cup (IF revision' > revision THEN {} ELSE {"SpendAdvancesPublication"})

\* --- wrappers -----------------------------------------------------------

AuditIdleSlot == [st |-> "idle", rev |-> 0, leaf |-> "c1"]

AuditInit ==
    /\ Init
    /\ audit_broken = {}
    /\ audit_seen = {}
    /\ audit_entries = [id \in AuditChildIds |-> 0]
    /\ audit_durable = AuditSnapshot([x \in {} |-> None], {}, [x \in {} |-> None], 0, {}, 0)
    /\ audit_poisoned = FALSE
    /\ audit_restarted = FALSE
    /\ audit_slots = [s \in AuditSlots |-> AuditIdleSlot]

AuditConfigure ==
    /\ Configure(AuditRoot, AuditNamespace, AuditGeneration, AuditIncarnation)
    /\ UNCHANGED auditVars

AuditIssueRoot(rule) ==
    /\ ~audit_poisoned
    /\ IssueRoot(AuditRoot, AuditParent(rule))
    /\ AuditJournalChange
    /\ UNCHANGED << audit_broken, audit_seen, audit_entries, audit_poisoned, audit_restarted,
                    audit_slots >>

AuditIssueChild(in) ==
    /\ ~audit_poisoned
    /\ AuditIssuable
    /\ IssueChildArm(IssueChild, in)
    /\ audit_broken' = audit_broken \cup AuditIssueBreaks(in)
    /\ AuditJournalChange
    /\ UNCHANGED << audit_seen, audit_entries, audit_poisoned, audit_restarted, audit_slots >>

AuditRevoke(id) ==
    /\ ~audit_poisoned
    /\ id \in DOMAIN records
    /\ RevokeNew(AuditRoot, records[id])
    /\ AuditJournalChange
    /\ UNCHANGED << audit_broken, audit_seen, audit_entries, audit_poisoned, audit_restarted,
                    audit_slots >>

\* A resolution alone: a prepared decision that never enters.
AuditResolve(in) ==
    /\ ~audit_poisoned
    /\ AuditUsable(in)
    /\ ResolveArm(ResolveUse, in)
    /\ audit_broken' = audit_broken \cup AuditUseBreaks(in)
    /\ UNCHANGED << audit_seen, audit_entries, audit_durable, audit_poisoned, audit_restarted,
                    audit_slots >>

\* A use spent, acknowledged, and entered.
AuditSpend(in) ==
    /\ ~audit_poisoned
    /\ AuditUsable(in)
    /\ ConsumeArm(ConsumeUse, in)
    /\ audit_broken' = audit_broken \cup AuditUseBreaks(in) \cup AuditSpendBreaks(in)
    /\ AuditJournalConsumed
    /\ audit_entries' = [audit_entries EXCEPT ![in.leaf] = @ + 1]
    /\ UNCHANGED << audit_seen, audit_poisoned, audit_restarted, audit_slots >>

\* A spend for an approver no longer current: nothing spent, nothing entered.
AuditSpendRefusedApproverRevoked(in) ==
    /\ ~audit_poisoned
    /\ AuditUsable(in)
    /\ ConsumeArm(ConsumeUseRefusedApproverRevoked, in)
    /\ audit_broken' = audit_broken \cup AuditUseBreaks(in)
                       \cup (IF spent' = spent /\ revision' = revision
                             THEN {} ELSE {"NoSpendAfterApproverRevoked"})
    /\ audit_seen' = audit_seen \cup (IF audit_entries[in.leaf] = 0
                                       THEN {"ApproverRevokedRefused"} ELSE {})
    /\ UNCHANGED << audit_entries, audit_durable, audit_poisoned, audit_restarted, audit_slots >>

\* A spend whose journal write is uncertain: it landed or it did not, the
\* owner fails closed until the restart, and the use does not enter.
AuditSpendUncertain(in) ==
    /\ ~audit_poisoned
    /\ AuditUsable(in)
    /\ ConsumeArm(ConsumeUse, in)
    /\ audit_broken' = audit_broken \cup AuditUseBreaks(in) \cup AuditSpendBreaks(in)
    /\ \/ /\ audit_durable' = AuditSnapshotAfter
          /\ audit_seen' = audit_seen \cup {"UncertainLanded"}
       \/ UNCHANGED << audit_durable, audit_seen >>
    /\ audit_poisoned' = TRUE
    /\ UNCHANGED << audit_entries, audit_restarted, audit_slots >>

\* The owner restarts from its journal; attempts in flight are lost.
AuditRestart ==
    /\ phase = "Active"
    /\ ~audit_restarted
    /\ records' = audit_durable.records
    /\ revoked' = audit_durable.revoked
    /\ spent' = audit_durable.spent
    /\ consumed' = audit_durable.consumed
    /\ carried_decisions' = audit_durable.carried_decisions
    /\ revision' = audit_durable.revision
    /\ UNCHANGED << phase, model_step_count, root, namespace, generation, incarnation >>
    /\ audit_poisoned' = FALSE
    /\ audit_restarted' = TRUE
    /\ audit_slots' = [s \in AuditSlots |-> AuditIdleSlot]
    /\ audit_seen' = audit_seen \cup (IF \E id \in AuditChildIds : audit_entries[id] > 0
                                       THEN {"RestartedAfterSpend"} ELSE {})
    /\ UNCHANGED << audit_broken, audit_entries, audit_durable >>

AuditSlotUse(leaf) == AuditUseOf(leaf, AuditValidUse)

\* An attempt prepares the valid use of an approval leaf at the current
\* revision.
AuditPrepare(s, leaf) ==
    /\ ~audit_poisoned
    /\ audit_slots[s].st \in {"idle", "stale"}
    /\ AuditUsable(AuditSlotUse(leaf))
    /\ records[leaf].approval # None
    /\ ResolveArm(ResolveUse, AuditSlotUse(leaf))
    /\ audit_broken' = audit_broken \cup AuditUseBreaks(AuditSlotUse(leaf))
    /\ audit_slots' = [audit_slots EXCEPT ![s] = [st |-> "prepared", rev |-> revision, leaf |-> leaf]]
    /\ UNCHANGED << audit_seen, audit_entries, audit_durable, audit_poisoned, audit_restarted >>

\* A prepared attempt whose revision is current spends and enters.
AuditEnter(s) ==
    /\ ~audit_poisoned
    /\ audit_slots[s].st = "prepared"
    /\ audit_slots[s].rev = revision
    /\ AuditUsable(AuditSlotUse(audit_slots[s].leaf))
    /\ ConsumeArm(ConsumeUse, AuditSlotUse(audit_slots[s].leaf))
    /\ audit_broken' = audit_broken \cup AuditUseBreaks(AuditSlotUse(audit_slots[s].leaf))
                       \cup AuditSpendBreaks(AuditSlotUse(audit_slots[s].leaf))
    /\ AuditJournalConsumed
    /\ audit_entries' = [audit_entries EXCEPT ![audit_slots[s].leaf] = @ + 1]
    /\ audit_slots' = [audit_slots EXCEPT ![s].st = "entered"]
    /\ UNCHANGED << audit_seen, audit_poisoned, audit_restarted >>

\* A prepared attempt whose revision is no longer current is stale.
AuditEnterStale(s) ==
    /\ ~audit_poisoned
    /\ audit_slots[s].st = "prepared"
    /\ audit_slots[s].rev # revision
    /\ audit_slots' = [audit_slots EXCEPT ![s].st = "stale"]
    /\ UNCHANGED << vars, audit_broken, audit_seen, audit_entries, audit_durable, audit_poisoned,
                    audit_restarted >>

AuditNext ==
    \/ AuditConfigure
    \/ \E rule \in AuditParentRules : AuditIssueRoot(rule)
    \/ \E in \in AuditIssueInputs : AuditIssueChild(in)
    \/ \E id \in {AuditParentId} \cup AuditChildIds : AuditRevoke(id)
    \/ \E in \in AuditUseInputs : AuditResolve(in)
    \/ \E in \in AuditUseInputs : AuditSpend(in)
    \/ \E in \in AuditUseInputs : AuditSpendRefusedApproverRevoked(in)
    \/ \E in \in AuditUseInputs : AuditSpendUncertain(in)
    \/ AuditRestart
    \/ \E s \in AuditSlots, leaf \in AuditChildIds : AuditPrepare(s, leaf)
    \/ \E s \in AuditSlots : AuditEnter(s)
    \/ \E s \in AuditSlots : AuditEnterStale(s)

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* --- properties ---------------------------------------------------------

AuditUseOnlyExactOperation == "UseOnlyExactOperation" \notin audit_broken
AuditUseOnlyBoundRequesterAndWork == "UseOnlyBoundRequesterAndWork" \notin audit_broken
AuditNoChildWithoutApprovalUnderRule == "NoChildWithoutApprovalUnderRule" \notin audit_broken
AuditRuleAssuranceHolds == "RuleAssuranceHolds" \notin audit_broken
AuditNoApprovalChildWithoutRule == "NoApprovalChildWithoutRule" \notin audit_broken
AuditIssuedTermsAreApproved == "IssuedTermsAreApproved" \notin audit_broken
AuditNoSpendAfterApproverRevoked == "NoSpendAfterApproverRevoked" \notin audit_broken
AuditSpendAdvancesPublication == "SpendAdvancesPublication" \notin audit_broken

AuditNoUseBeyondUses ==
    \A id \in AuditChildIds :
        /\ id \in DOMAIN spent =>
               /\ id \in DOMAIN records
               /\ records[id].approval # None
               /\ spent[id] <= AuditValueOf(records[id].approval).uses
        /\ \/ audit_entries[id] = 0
           \/ /\ id \in DOMAIN records
              /\ records[id].approval # None
              /\ audit_entries[id] <= AuditValueOf(records[id].approval).uses

AuditOneGrantPerDecision ==
    \A i, j \in DOMAIN records :
        (/\ i # j
         /\ records[i].approval # None
         /\ records[j].approval # None)
            => AuditValueOf(records[i].approval).decision_id
                   # AuditValueOf(records[j].approval).decision_id

AuditSpentSurvivesRestart ==
    \A id \in AuditChildIds :
        audit_entries[id] > 0 =>
            /\ id \in DOMAIN audit_durable.spent
            /\ audit_durable.spent[id] >= audit_entries[id]
            /\ id \in DOMAIN spent
            /\ spent[id] >= audit_entries[id]

\* --- reachability goals, each checked as an expected violation of its
\* negation ---------------------------------------------------------------

AuditApprovalLeaf(id) == id \in DOMAIN records /\ records[id].approval # None

\* A use refused at both its resolution and its spend.
AuditUseRefused(in) ==
    /\ AuditUsable(in)
    /\ ~ENABLED ResolveArm(ResolveUse, in)
    /\ ~ENABLED ConsumeArm(ConsumeUse, in)

\* The valid use of an approval leaf is open, and the same use with the one
\* defect of `shape` is refused.
AuditOnlyDefectRefused(shape) ==
    \E id \in AuditChildIds :
        /\ AuditApprovalLeaf(id)
        /\ AuditUsable(AuditSlotUse(id))
        /\ ENABLED ConsumeArm(ConsumeUse, AuditSlotUse(id))
        /\ AuditUseRefused(AuditUseOf(id, shape))

\* The valid child is open, and `in` is refused.
AuditOnlyIssueRefused(in) ==
    /\ AuditIssuable
    /\ ENABLED IssueChildArm(IssueChild, AuditValidIssue)
    /\ ~ENABLED IssueChildArm(IssueChild, in)

AuditWitnessApprovalChildIssued == \E id \in AuditChildIds : AuditApprovalLeaf(id)
AuditWitnessUsedOnce == \E id \in AuditChildIds : audit_entries[id] = 1
AuditWitnessSecondUseRefused ==
    \E id \in AuditChildIds :
        /\ audit_entries[id] = 1
        /\ AuditApprovalLeaf(id)
        /\ revoked = {}
        /\ AuditUseRefused(AuditSlotUse(id))
AuditWitnessOtherDigestRefused ==
    AuditOnlyDefectRefused([AuditValidUse EXCEPT !.operation = Some(AuditOtherOperation)])
AuditWitnessOtherRequesterRefused ==
    AuditOnlyDefectRefused([AuditValidUse EXCEPT !.requester = Some(AuditOtherRequester)])
AuditWitnessOtherWorkRefused ==
    AuditOnlyDefectRefused([AuditValidUse EXCEPT !.work = Some(AuditOtherWork)])
AuditWitnessWindowElapsedRefused ==
    AuditOnlyDefectRefused([AuditValidUse EXCEPT !.now_ms = 2])
AuditWitnessParentRevokedBeforeUseRefused ==
    \E id \in AuditChildIds :
        /\ AuditApprovalLeaf(id)
        /\ audit_entries[id] = 0
        /\ AuditParentId \in revoked
        /\ AuditUseRefused(AuditSlotUse(id))
AuditWitnessApproverRevokedBeforeUseRefused ==
    /\ "ApproverRevokedRefused" \in audit_seen
    /\ \A id \in AuditChildIds : audit_entries[id] = 0
    /\ \A id \in DOMAIN spent : spent[id] = 0
AuditWitnessDeviceTrustRefusedUnderRule ==
    /\ AuditIssuable
    /\ records[AuditParentId].child_approval = Some(AuditRule("Refused"))
    /\ AuditOnlyIssueRefused(
           AuditIssueOf("c1", Some([AuditValidApproval EXCEPT !.assurance = "DeviceTrusted"]),
                        AuditChildRestrictions, AuditDigest))
AuditWitnessSpentThenCrashStillSpent ==
    /\ "RestartedAfterSpend" \in audit_seen
    /\ \E id \in AuditChildIds :
           /\ audit_entries[id] = 1
           /\ id \in DOMAIN spent
           /\ spent[id] = 1
AuditWitnessRacingSecondAttemptRefused ==
    \E s, t \in AuditSlots :
        /\ s # t
        /\ audit_slots[s].st = "entered"
        /\ audit_slots[t].st = "stale"
        /\ audit_slots[t].rev = audit_slots[s].rev
        /\ audit_slots[t].leaf = audit_slots[s].leaf
        /\ AuditUseRefused(AuditSlotUse(audit_slots[t].leaf))
AuditWitnessPlainChildRefusedUnderRule ==
    /\ AuditIssuable
    /\ records[AuditParentId].child_approval # None
    /\ AuditOnlyIssueRefused(AuditIssueOf("c1", None, AuditChildRestrictions, AuditDigest))
AuditWitnessUncertainSpendNotEntered ==
    /\ "UncertainLanded" \in audit_seen
    /\ audit_restarted
    /\ \E id \in AuditChildIds :
           /\ id \in DOMAIN spent
           /\ spent[id] = 1
           /\ audit_entries[id] = 0

NotAuditWitnessApprovalChildIssued == ~AuditWitnessApprovalChildIssued
NotAuditWitnessUsedOnce == ~AuditWitnessUsedOnce
NotAuditWitnessSecondUseRefused == ~AuditWitnessSecondUseRefused
NotAuditWitnessOtherDigestRefused == ~AuditWitnessOtherDigestRefused
NotAuditWitnessOtherRequesterRefused == ~AuditWitnessOtherRequesterRefused
NotAuditWitnessOtherWorkRefused == ~AuditWitnessOtherWorkRefused
NotAuditWitnessWindowElapsedRefused == ~AuditWitnessWindowElapsedRefused
NotAuditWitnessParentRevokedBeforeUseRefused == ~AuditWitnessParentRevokedBeforeUseRefused
NotAuditWitnessApproverRevokedBeforeUseRefused == ~AuditWitnessApproverRevokedBeforeUseRefused
NotAuditWitnessDeviceTrustRefusedUnderRule == ~AuditWitnessDeviceTrustRefusedUnderRule
NotAuditWitnessSpentThenCrashStillSpent == ~AuditWitnessSpentThenCrashStillSpent
NotAuditWitnessRacingSecondAttemptRefused == ~AuditWitnessRacingSecondAttemptRefused
NotAuditWitnessPlainChildRefusedUnderRule == ~AuditWitnessPlainChildRefusedUnderRule
NotAuditWitnessUncertainSpendNotEntered == ~AuditWitnessUncertainSpendNotEntered
====
