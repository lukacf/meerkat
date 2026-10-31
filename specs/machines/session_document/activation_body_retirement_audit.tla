---- MODULE activation_body_retirement_audit ----
\* Hand-written audit of the activation-body retirement region of the
\* generated SessionDocumentMachine model.
\*
\* The machine holds, per session and per instruction activation key, how
\* many activation rows of the key the transcript holds, how many of them,
\* oldest first, carry a retired body, and whether the key's newest row is
\* the session's own. This audit drives the generated arms as the shell does
\* (one session, two keys, up to three rows per key, own and copied heads):
\* a first recovery from any projection, torn ones included, which the
\* recovery guard must refuse; an own activation of a key, which the shell
\* sees as a recovery with one more row of that key and that key's head its
\* own; a fork, which the shell sees as a recovery of a prefix of each key's
\* rows with every head copied and a copied row keeping its retired body;
\* and the retirement decision. It checks the machine's own state.
\*
\* Invariants (each a property of the last step, through audit_breach, or of
\* the state):
\*
\*   AuditRetiredChangesOnlyAtTheDecision: a recovery carries the retired
\*     counts the transcript holds (an activation retires nothing, a fork
\*     copies its prefix's retired rows); only the decision retires a body.
\*   AuditSupersededRowsRetired: after the decision every row a later row of
\*     its key follows is retired.
\*   AuditOwnHeadKeepsItsBody: a key whose newest row is the session's own
\*     keeps that row's body, whatever the step.
\*   AuditDecisionNeverLowersACount: the decision never lowers a recovered
\*     retired count (a copied head a fork copied retired stays retired).
\*   AuditDecisionKeepsRowsAndHeads: the decision changes the retired counts
\*     only.
\*
\* The witnesses (NotAuditWitness*, expected violations, each checked in a
\* TLC run of its own without -continue): a decision retired a body, a
\* decision kept a copied retired head as it was, and a key with three rows
\* retired two. The runner's --mutants mode seeds five defects into a copy of
\* the generated model and requires TLC to refuse each with the invariant it
\* targets, run as its only invariant:
\* retire every row (the decision's count taken as the rows), retire nothing
\* new (the decision's update dropped), drop the own-head clause from the
\* recovery guard, retire inside the recovery, and lower a recovered count
\* (the decision's count taken as the rows less one, whatever was retired).
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* activation_body_retirement_audit.sh (its constants, its invariants and
\* its step constraint, unchanged):
\*   specs/machines/session_document/activation_body_retirement_audit.sh --mutants
EXTENDS model

AuditSession == CHOOSE s \in SessionIdValues : TRUE
AuditKeys == {"k1", "k2"}
AuditMaxRows == 3
AuditOrigins == {"Own", "Copied"}

\* Every projection the shell may hand the recovery, torn ones included:
\* rows, retired counts and head origins over one set of keys.
AuditProjections ==
    UNION { [rows : [keys -> 0..AuditMaxRows],
             retired : [keys -> 0..AuditMaxRows],
             head : [keys -> AuditOrigins]] : keys \in SUBSET AuditKeys }

AuditCount(map, key) == IF key \in DOMAIN map THEN map[key] ELSE 0
AuditMin(left, right) == IF left < right THEN left ELSE right

VARIABLES audit_breach, witnessed

audit_vars == << audit_breach, witnessed >>
audit_all_vars == << vars, audit_vars >>

AuditSeeded == AuditSession \in DOMAIN session_activation_rows

AuditRows == session_activation_rows[AuditSession]
AuditRetired == session_activation_retired[AuditSession]
AuditHeads == session_activation_head_origin[AuditSession]
AuditRowsNext == session_activation_rows'[AuditSession]
AuditRetiredNext == session_activation_retired'[AuditSession]
AuditHeadsNext == session_activation_head_origin'[AuditSession]

AuditInit ==
    /\ Init
    /\ audit_breach = {}
    /\ witnessed = {}

\* The shell recovers the session the first time from any projection; the
\* recovery guard refuses a torn one.
AuditSeed ==
    /\ ~AuditSeeded
    /\ \E projection \in AuditProjections :
           RecoverSessionActivationBodies(AuditSession, projection.rows, projection.retired,
                                          projection.head)
    /\ audit_breach' = {}
    /\ UNCHANGED witnessed

\* An own activation of `key`: the shell recovers the session with one more
\* row of the key, whose newest row is now the session's own. It retires
\* nothing.
AuditActivate(key) ==
    /\ AuditSeeded
    /\ AuditCount(AuditRows, key) < AuditMaxRows
    /\ LET keys == DOMAIN AuditRows \cup {key}
       IN RecoverSessionActivationBodies(
              AuditSession,
              [k \in keys |-> IF k = key THEN AuditCount(AuditRows, key) + 1 ELSE AuditRows[k]],
              [k \in keys |-> AuditCount(AuditRetired, k)],
              [k \in keys |-> IF k = key THEN "Own" ELSE AuditHeads[k]])
    /\ audit_breach' =
           IF AuditRetiredNext # [k \in DOMAIN AuditRowsNext |-> AuditCount(AuditRetired, k)]
           THEN {"RetiredChangedOutsideTheDecision"} ELSE {}
    /\ UNCHANGED witnessed

\* A fork child: the shell recovers a prefix of each key's rows (none, for
\* a key the prefix leaves out), every head copied, and a copied row keeps
\* the retired body it had in the source.
AuditFork ==
    /\ AuditSeeded
    /\ \E cut \in [DOMAIN AuditRows -> 0..AuditMaxRows] :
           /\ \A k \in DOMAIN AuditRows : cut[k] <= AuditRows[k]
           /\ LET keys == {k \in DOMAIN AuditRows : cut[k] > 0}
              IN RecoverSessionActivationBodies(
                     AuditSession,
                     [k \in keys |-> cut[k]],
                     [k \in keys |-> AuditMin(AuditRetired[k], cut[k])],
                     [k \in keys |-> "Copied"])
    /\ audit_breach' =
           IF AuditRetiredNext
                # [k \in DOMAIN AuditRowsNext |-> AuditMin(AuditRetired[k], AuditRowsNext[k])]
           THEN {"RetiredChangedOutsideTheDecision"} ELSE {}
    /\ UNCHANGED witnessed

\* A compaction asks which bodies to retire.
AuditDecide ==
    /\ AuditSeeded
    /\ AuthorizeActivationBodyRetirement(AuditSession)
    /\ audit_breach' =
           (IF \E k \in DOMAIN AuditRowsNext : AuditCount(AuditRetiredNext, k) + 1 < AuditRowsNext[k]
            THEN {"SupersededRowLeftLive"} ELSE {})
           \cup (IF \E k \in DOMAIN AuditRetired : AuditCount(AuditRetiredNext, k) < AuditRetired[k]
                 THEN {"RecoveredCountLowered"} ELSE {})
           \cup (IF AuditRowsNext # AuditRows \/ AuditHeadsNext # AuditHeads
                 THEN {"DecisionChangedRowsOrHeads"} ELSE {})
    /\ witnessed' =
           witnessed
           \cup (IF \E k \in DOMAIN AuditRetired : AuditCount(AuditRetiredNext, k) > AuditRetired[k]
                 THEN {"BodyRetired"} ELSE {})
           \cup (IF \E k \in DOMAIN AuditRetired :
                     /\ AuditHeads[k] = "Copied"
                     /\ AuditRetired[k] = AuditRows[k]
                     /\ AuditCount(AuditRetiredNext, k) = AuditRows[k]
                 THEN {"CopiedRetiredHeadKept"} ELSE {})
           \cup (IF \E k \in DOMAIN AuditRowsNext :
                     AuditRowsNext[k] = 3 /\ AuditCount(AuditRetiredNext, k) = 2
                 THEN {"ThreeRowsRetireTwo"} ELSE {})

AuditNext ==
    \/ AuditSeed
    \/ \E key \in AuditKeys : AuditActivate(key)
    \/ AuditFork
    \/ AuditDecide

AuditSpec == AuditInit /\ [][AuditNext]_audit_all_vars

AuditRetiredChangesOnlyAtTheDecision == "RetiredChangedOutsideTheDecision" \notin audit_breach
AuditSupersededRowsRetired == "SupersededRowLeftLive" \notin audit_breach
AuditDecisionNeverLowersACount == "RecoveredCountLowered" \notin audit_breach
AuditDecisionKeepsRowsAndHeads == "DecisionChangedRowsOrHeads" \notin audit_breach
AuditOwnHeadKeepsItsBody ==
    AuditSeeded =>
        \A k \in DOMAIN AuditRows :
            (k \in DOMAIN AuditHeads /\ AuditHeads[k] = "Own") => AuditCount(AuditRetired, k) < AuditRows[k]

\* Reachability goals, checked as expected violations.
NotAuditWitnessBodyRetired == "BodyRetired" \notin witnessed
NotAuditWitnessCopiedRetiredHeadKept == "CopiedRetiredHeadKept" \notin witnessed
NotAuditWitnessThreeRowsRetireTwo == "ThreeRowsRetireTwo" \notin witnessed
====
