---- MODULE writer_fence_audit ----
\* Hand-written bounded audit of the generated HostStoreWriterFenceMachine
\* model, driven by a host store and a second writer.
\*
\* The store grants tokens in order (each grant answers the next token above
\* every earlier one), records one holder's lease on its own clock, and keeps
\* it live while its holder renews. The writer under audit opens with token 1
\* and runs the generated machine: its renews, window closings, re-acquires
\* (an exclusive acquire, a takeover of its own recorded lease), listings,
\* resume renews and commits are answered by the store. An acquire the writer
\* gave up on (timed out) stays with the store, which may still decide it
\* later and grant it to nobody: the hazard of an abandoned acquire. A second
\* writer may take the fence over once, renew, commit a batch and stop. Leases
\* nobody renews lapse. The waited-out end of a holder may come before the
\* store's own end (a store that answers the remaining lease rounded down),
\* so the writer may meet the holder again while its lease is live; that
\* holder's lease moved later only when it renews it. A renew or a commit
\* the writer sent while live may answer after it stopped, but before any
\* re-acquire begins: the realm never overlaps either with a re-acquire (the
\* renew gate, the fence gate).
\*
\* Properties:
\*
\*   AuditNoFalseLoss: the writer loses the realm only when the second writer
\*     acquired, or when a takeover it gave up on was granted after a later
\*     grant of its own (the documented residual: that late grant supersedes
\*     the writer's live token).
\*   AuditNoResumeWithoutProof: a live writer has not stopped since a clean
\*     listing proved its grant (it never resumes on a renew).
\*   AuditNoResumeOverForeignBatch: no grant is installed while a batch of
\*     the second writer is unseen.
\*   AuditWaitsAgainOnlyUnrenewed: no holder that renews its lease is waited
\*     out again (a waited-out holder still holding with its lease moved
\*     renewed it, and loses the realm; one whose lease did not move is
\*     waited out again).
\*   AuditTokenNeverAboveTheStore: the writer commits with a token the store
\*     granted.
\*   AuditNoReacquireAfterFailedOpen: a realm whose open failed its listing
\*     never acquires again.
\*   AuditClassificationsMonotone (an action property): no quarantined key,
\*     conflicted incarnation's record or poison is removed, and no record or
\*     cause changes, before a reopen.
\*
\* Reachability (the runner): seven scenarios of the fence part are checked
\* as expected violations of their negations: a resume after a stop
\* (ResumedAfterStop); a holder that was the writer's own late grant waited
\* out, then a resume (WaitedOutOwnGrantResumed); a waited-out holder that
\* renewed loses the realm (HolderRenewed); a foreign batch found after a
\* wait (ForeignAfterWait); a listing that failed after a grant, then a
\* resume on a takeover of that grant (ListingFailedResumed); the documented
\* residual (Residual); and a holder nobody renewed, met again after an end
\* recorded early, waited out again, then a resume (WaitedOutAgainResumed).
\* Four of the scopes part: a quarantine and a poison kept across a stop, a
\* re-acquire and a resume (QuarantineKeptAcrossReacquire,
\* PoisonKeptAcrossReacquire), a re-acquire refused after a failed open
\* listing (FailedOpenRefusesReacquire), and a chunk's conflict that
\* quarantines its owner's key too (ChunkOwnerQuarantined).
\* With --mutants the runner seeds one defect
\* at a time into a copy of the generated model and requires TLC to refuse
\* each with the property it breaks.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* writer_fence_audit.sh:
\*   specs/machines/host_store_writer_fence/writer_fence_audit.sh 15 --mutants
EXTENDS model

CONSTANTS AuditMaxSteps, AuditPart

\* AuditPart "fence": a second writer and acquires the writer gave up on,
\* no scope classification. AuditPart "scopes": scope classification
\* and the open's listing, no second writer and no abandoned acquire.

\* The highest token any grant may answer, and the most acquires the writer
\* may have given up on at once.
AuditMaxToken == 5
AuditMaxPending == 1

\* The generated model renders an absent option as a record whose value is a
\* string, so TLC cannot compare it with a present token (an integer value):
\* it compares the value fields and fails. The generated ci sweep never holds
\* a present token, while this audit does. The runner overrides the model's
\* None with this record of one field: a record of other fields is unequal to
\* every present option without a value comparison, and nothing reads the
\* value of an absent option.
AuditNone == [tag |-> "none"]

VARIABLES
    st_high,               \* the highest token the store granted, 0 before the open
    st_holder,             \* the token whose lease the store records
    st_live,               \* whether that lease is live on the store's clock
    other,                 \* the second writer's token, 0 while it holds none
    other_alive,           \* the second writer runs: it renews and may commit
    ever_other,            \* the second writer acquired the fence
    foreign_unseen,        \* a batch of the second writer is visible and no listing saw it
    pending,               \* acquires the writer gave up on: <<step, grants when given up>>
    grants,                \* grants the writer's re-acquires received
    late_after_grant,      \* a given-up acquire was granted after a later grant of the writer
    own_late,              \* tokens the store granted to given-up acquires of the writer
    stopped_since_proof,   \* the writer stopped and no clean listing proved a grant since
    resumed_over_foreign,  \* a grant was installed while a foreign batch was unseen
    waited,                \* holders the writer waited out
    waited_renewed_twice,  \* a holder that renews was waited out a second time
    w_waited_again,        \* a holder was waited out a second time
    w_waited_own,          \* the writer waited out a late grant of its own
    w_listing_failed,      \* a listing failed after a grant
    renew_open,            \* a renew sent while the writer was live may still answer
    commit_open,           \* a commit sent while the writer was live may still answer
    open_listed,           \* the open's listing ended, listed or failed
    w_failed_open,         \* the open's listing failed
    w_refused_after_failed_open, \* a re-acquire was refused after it
    w_q_early,             \* a scope was quarantined before the first re-acquire grant
    w_p_early              \* a scope was poisoned before the first re-acquire grant

storeVars == << st_high, st_holder, st_live >>
otherVars == << other, other_alive, ever_other, foreign_unseen >>
lateVars == << pending, grants, late_after_grant, own_late >>
proofVars == << stopped_since_proof, resumed_over_foreign >>
waitVars == << waited, waited_renewed_twice, w_waited_again, w_waited_own, w_listing_failed >>
scopeVars == << open_listed, w_failed_open, w_refused_after_failed_open, w_q_early, w_p_early >>
auditVars == << st_high, st_holder, st_live, other, other_alive, ever_other,
                foreign_unseen, pending, grants, late_after_grant, own_late,
                stopped_since_proof, resumed_over_foreign, waited, waited_renewed_twice,
                w_waited_again, w_waited_own, w_listing_failed, renew_open, commit_open,
                open_listed, w_failed_open, w_refused_after_failed_open, w_q_early,
                w_p_early >>

AuditInit ==
    /\ Init
    /\ st_high = 0
    /\ st_holder = 0
    /\ st_live = FALSE
    /\ other = 0
    /\ other_alive = FALSE
    /\ ever_other = FALSE
    /\ foreign_unseen = FALSE
    /\ pending = {}
    /\ grants = 0
    /\ late_after_grant = FALSE
    /\ own_late = {}
    /\ stopped_since_proof = FALSE
    /\ resumed_over_foreign = FALSE
    /\ waited = {}
    /\ waited_renewed_twice = FALSE
    /\ w_waited_again = FALSE
    /\ w_waited_own = FALSE
    /\ w_listing_failed = FALSE
    /\ renew_open = FALSE
    /\ commit_open = FALSE
    /\ open_listed = FALSE
    /\ w_failed_open = FALSE
    /\ w_refused_after_failed_open = FALSE
    /\ w_q_early = FALSE
    /\ w_p_early = FALSE

\* Whether the machine stops the writer in this step.
Stops == phase = "Live" /\ phase' = "Stopped"

\* --- the open ------------------------------------------------------------

\* The open's exclusive acquire on an empty realm grants token 1. In the
\* fence part its listing passes at once; in the scopes part it ends later
\* (AuditOpenListed, AuditOpenListingFails).
AuditOpen ==
    /\ st_high = 0
    /\ Open(1)
    /\ st_high' = 1
    /\ st_holder' = 1
    /\ st_live' = TRUE
    /\ open_listed' = (AuditPart = "fence")
    /\ UNCHANGED << otherVars, lateVars, proofVars, waitVars,
                    w_failed_open, w_refused_after_failed_open, w_q_early, w_p_early >>

\* --- the renewal tick -----------------------------------------------------

\* A renew of the writer's token while it is current: the store extends the
\* lease, whether the renew started inside the window or after it closed; or
\* the renew fails or times out (a renew that timed out may still extend the
\* lease, which changes nothing here).
AuditRenewCurrent ==
    /\ phase = "Live"
    /\ st_high = token
    /\ \/ /\ RenewedInWindow("Renewed", token, token, TRUE)
          /\ st_live' = TRUE
       \/ /\ RenewedAfterTheWindow("Renewed", token, token, FALSE)
          /\ st_live' = TRUE
       \/ /\ RenewFailedLive("Failed", token, token, TRUE)
          /\ UNCHANGED st_live
       \/ /\ RenewTimedOutLive("TimedOut", token, token, TRUE)
          /\ UNCHANGED st_live
    /\ stopped_since_proof' = (stopped_since_proof \/ Stops)
    \* The renew answered; a commit may still be in flight at a stop.
    /\ renew_open' = FALSE
    /\ commit_open' \in IF Stops THEN BOOLEAN ELSE {commit_open}
    /\ UNCHANGED << st_high, st_holder, otherVars, lateVars, resumed_over_foreign, waitVars >>

\* A renew once a higher token exists answers fenced out, naming it.
AuditRenewSuperseded ==
    /\ phase = "Live"
    /\ st_high > token
    /\ RenewFencedOutLive("FencedOut", token, st_high, TRUE)
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, waitVars >>

\* A renew sent while the writer was live answers once it stopped: it moves
\* nothing (or loses the realm when fenced out). The realm never overlaps a
\* renew with a re-acquire, so it answers before any re-acquire begins.
AuditRenewAnswersStopped ==
    /\ phase = "Stopped"
    /\ renew_open
    /\ renew_open' = FALSE
    /\ UNCHANGED commit_open
    /\ \/ RenewAnsweredStopped("Renewed", token, token, TRUE)
       \/ /\ st_high > token
          /\ RenewFencedOutStopped("FencedOut", token, st_high, TRUE)
    /\ UNCHANGED << storeVars, otherVars, lateVars, waitVars, resumed_over_foreign >>
    /\ stopped_since_proof' = (stopped_since_proof \/ phase' = "Live")

\* --- the window -----------------------------------------------------------

\* The window closes while the writer is live, a renew or a commit in flight
\* or not.
AuditWindowCloses ==
    /\ WindowClosedLive
    /\ renew_open' \in BOOLEAN
    /\ commit_open' \in BOOLEAN
    /\ stopped_since_proof' = TRUE
    /\ UNCHANGED << storeVars, otherVars, lateVars, resumed_over_foreign, waitVars >>

\* --- leases on the store's clock ------------------------------------------

\* A lease nobody renews ends: not the live writer's, not a running second
\* writer's.
AuditLeaseLapses ==
    /\ st_live
    /\ ~(phase = "Live" /\ st_holder = token)
    /\ ~(other_alive /\ st_holder = other)
    /\ st_live' = FALSE
    /\ UNCHANGED << vars, st_high, st_holder, otherVars, lateVars, proofVars, waitVars >>

\* --- the re-acquire ---------------------------------------------------------

\* A re-acquire begins only once no renew and no commit is in flight and the
\* open's listing ended; after a failed open listing it is refused.
AuditBeginReacquire ==
    /\ ~renew_open
    /\ ~commit_open
    /\ open_listed
    /\ \/ BeginReacquire
       \/ BeginReacquireAfterFailedOpen
    /\ w_refused_after_failed_open' =
           (w_refused_after_failed_open \/ (open_listing_failed /\ phase' = "Stopped"))
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, waitVars,
                    open_listed, w_failed_open, w_q_early, w_p_early >>

\* The acquire in flight is granted: an exclusive acquire on a fence no live
\* lease holds, or the takeover of the writer's own lease.
AuditAcquireGranted ==
    /\ phase = "Acquiring"
    /\ step = "TakeoverOwn" \/ ~st_live
    /\ st_high < AuditMaxToken
    /\ AcquireGranted("Granted", st_high + 1, FALSE)
    /\ st_high' = st_high + 1
    /\ st_holder' = st_high + 1
    /\ st_live' = TRUE
    /\ grants' = grants + 1
    /\ UNCHANGED << otherVars, pending, late_after_grant, own_late, proofVars, waitVars >>

\* An exclusive acquire meets the live holder the store records. Its lease
\* moved later since the writer recorded its end only when its holder renews
\* it: the second writer while it runs. A late grant of the writer's own, and
\* a second writer that stopped, renew nothing.
AuditAcquireHeld ==
    /\ phase = "Acquiring"
    /\ step = "Exclusive"
    /\ st_live
    /\ LET moved == other_alive /\ st_holder = other IN
       /\ \/ AcquireHeldByOwnLease("Held", st_holder, moved)
          \/ AcquireHeldByAnother("Held", st_holder, moved)
          \/ AcquireHeldByRenewedHolder("Held", st_holder, moved)
          \/ AcquireHeldByUnrenewedHolder("Held", st_holder, moved)
          \/ AcquireHeldByPossibleLateGrant("Held", st_holder, moved)
       /\ waited_renewed_twice' =
              (waited_renewed_twice \/ (phase' = "WaitingOut" /\ st_holder \in waited /\ moved))
    /\ waited' = IF phase' = "WaitingOut" THEN waited \cup {st_holder} ELSE waited
    /\ w_waited_again' = (w_waited_again \/ (phase' = "WaitingOut" /\ st_holder \in waited))
    /\ w_waited_own' = (w_waited_own \/ (phase' = "WaitingOut" /\ st_holder \in own_late))
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, w_listing_failed >>

\* The acquire in flight times out: the store may still decide it later.
AuditAcquireTimedOut ==
    /\ AuditPart = "fence"
    /\ phase = "Acquiring"
    /\ Cardinality(pending) < AuditMaxPending
    /\ pending' = pending \cup {<< step, grants >>}
    /\ AcquireTimedOut("TimedOut", 0, FALSE)
    /\ UNCHANGED << storeVars, otherVars, grants, late_after_grant, own_late, proofVars, waitVars >>

\* The acquire in flight fails: nothing is acquired.
AuditAcquireFailed ==
    /\ AcquireFailed("Failed", 0, FALSE)
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, waitVars >>

\* The store decides an acquire the writer gave up on, and its answer reaches
\* nobody: a grant (a takeover, or an exclusive acquire on a fence no live
\* lease holds), or a held answer that changes nothing.
AuditLateDecision ==
    /\ \E r \in pending :
        /\ pending' = pending \ {r}
        /\ \/ /\ r[1] = "TakeoverOwn" \/ ~st_live
              /\ st_high < AuditMaxToken
              /\ st_high' = st_high + 1
              /\ st_holder' = st_high + 1
              /\ st_live' = TRUE
              /\ own_late' = own_late \cup {st_high + 1}
              /\ late_after_grant' = (late_after_grant \/ r[2] < grants)
           \/ /\ r[1] = "Exclusive"
              /\ st_live
              /\ UNCHANGED << storeVars, own_late, late_after_grant >>
    /\ UNCHANGED << vars, otherVars, grants, proofVars, waitVars >>

\* The writer's clock passes the recorded end of the waited-out holder's
\* lease. A store that answers the remaining lease rounded down makes that
\* end come before its own, so the holder may still be live then.
AuditLeaseEnded ==
    /\ phase = "WaitingOut"
    /\ LeaseEnded
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, waitVars >>

\* The listing that proves the grant: clean unless a batch of the second
\* writer is unseen, or failed.
AuditListing ==
    /\ phase = "Proving"
    /\ \/ /\ ~foreign_unseen
          /\ ListingClean("Clean", granted.value)
       \/ /\ foreign_unseen
          /\ ListingForeign("ForeignBatch", granted.value)
       \/ /\ ListingFailed("Failed", granted.value)
    /\ resumed_over_foreign' = (resumed_over_foreign \/ (phase' = "Resuming" /\ foreign_unseen))
    /\ stopped_since_proof' = IF phase' = "Resuming" THEN FALSE ELSE stopped_since_proof
    /\ foreign_unseen' = IF phase' = "Resuming" THEN FALSE ELSE foreign_unseen
    /\ w_listing_failed' = (w_listing_failed \/ phase' = "Stopped")
    /\ UNCHANGED << storeVars, other, other_alive, ever_other, lateVars, waited, waited_renewed_twice, w_waited_again, w_waited_own >>

\* The renew right after the install.
AuditResumeRenew ==
    /\ phase = "Resuming"
    /\ \/ /\ st_high = token
          /\ ResumeRenewed("Renewed", token, token, TRUE)
          /\ st_live' = TRUE
       \/ /\ st_high > token
          /\ ResumeFencedOut("FencedOut", token, st_high, TRUE)
          /\ UNCHANGED st_live
       \/ /\ ResumeRenewFailed("Failed", token, token, TRUE)
          /\ UNCHANGED st_live
       \/ /\ ResumeRenewTimedOut("TimedOut", token, token, TRUE)
          /\ UNCHANGED st_live
    /\ UNCHANGED << st_high, st_holder, otherVars, lateVars, proofVars, waitVars >>

\* A commit of the writer once a higher token exists is fenced out.
AuditCommitFencedOut ==
    /\ \/ phase = "Live"
       \/ phase = "Stopped" /\ commit_open
    /\ st_high > token
    /\ Poison("FencedOutCommit")
    /\ commit_open' = FALSE
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, waitVars, renew_open >>

\* A commit in flight at the stop settles otherwise (it lands, or its outcome
\* stays unknown and its scopes are poisoned): the machine hears nothing.
AuditCommitSettles ==
    /\ phase = "Stopped"
    /\ commit_open
    /\ commit_open' = FALSE
    /\ UNCHANGED << vars, storeVars, otherVars, lateVars, proofVars, waitVars, renew_open >>

\* --- the second writer ----------------------------------------------------

\* It takes the fence over once, whoever holds it.
AuditOtherAcquires ==
    /\ AuditPart = "fence"
    /\ ~ever_other
    /\ st_high > 0
    /\ st_high < AuditMaxToken
    /\ st_high' = st_high + 1
    /\ st_holder' = st_high + 1
    /\ st_live' = TRUE
    /\ other' = st_high + 1
    /\ other_alive' = TRUE
    /\ ever_other' = TRUE
    /\ UNCHANGED << vars, foreign_unseen, lateVars, proofVars, waitVars >>

\* While its token is current it commits a batch.
AuditOtherCommits ==
    /\ other_alive
    /\ other = st_high
    /\ ~foreign_unseen
    /\ foreign_unseen' = TRUE
    /\ UNCHANGED << vars, storeVars, other, other_alive, ever_other, lateVars, proofVars, waitVars >>

\* It stops, and renews no more.
AuditOtherStops ==
    /\ other_alive
    /\ other_alive' = FALSE
    /\ UNCHANGED << vars, storeVars, other, ever_other, foreign_unseen, lateVars, proofVars, waitVars >>

\* --- the scopes part: classification and the open's listing ----------------

\* The conflicted incarnations the store may report, each with the key it is
\* reported under: two incarnations of key "k1", and a chunk "c", reported
\* under its own name (the open's listing) and under its owner "k2" (the
\* owner's load meets the chunk's conflict). The scopes a decision site may
\* poison.
AuditReports == {<< "k1", "k1~a" >>, << "k1", "k1~b" >>, << "c", "c" >>, << "k2", "c" >>}
AuditScopeKeys == {"k1", "k2"}

\* Every generated arm of a conflicted incarnation's report.
QuarantineArms(key, name, version, commit_ids) ==
    \/ ScopeQuarantinedLive(key, name, version, commit_ids)
    \/ ScopeQuarantinedStopped(key, name, version, commit_ids)
    \/ ScopeQuarantinedAcquiring(key, name, version, commit_ids)
    \/ ScopeQuarantinedProving(key, name, version, commit_ids)
    \/ ScopeQuarantinedResuming(key, name, version, commit_ids)
    \/ ScopeQuarantinedWaitingOut(key, name, version, commit_ids)
    \/ ScopeQuarantinedNewKeyLive(key, name, version, commit_ids)
    \/ ScopeQuarantinedNewKeyStopped(key, name, version, commit_ids)
    \/ ScopeQuarantinedNewKeyAcquiring(key, name, version, commit_ids)
    \/ ScopeQuarantinedNewKeyProving(key, name, version, commit_ids)
    \/ ScopeQuarantinedNewKeyResuming(key, name, version, commit_ids)
    \/ ScopeQuarantinedNewKeyWaitingOut(key, name, version, commit_ids)
    \/ ScopeQuarantinedAgainLive(key, name, version, commit_ids)
    \/ ScopeQuarantinedAgainStopped(key, name, version, commit_ids)
    \/ ScopeQuarantinedAgainAcquiring(key, name, version, commit_ids)
    \/ ScopeQuarantinedAgainProving(key, name, version, commit_ids)
    \/ ScopeQuarantinedAgainResuming(key, name, version, commit_ids)
    \/ ScopeQuarantinedAgainWaitingOut(key, name, version, commit_ids)
    \/ ScopeQuarantinedPoisonedLive(key, name, version, commit_ids)
    \/ ScopeQuarantinedPoisonedStopped(key, name, version, commit_ids)
    \/ ScopeQuarantinedPoisonedAcquiring(key, name, version, commit_ids)
    \/ ScopeQuarantinedPoisonedProving(key, name, version, commit_ids)
    \/ ScopeQuarantinedPoisonedResuming(key, name, version, commit_ids)
    \/ ScopeQuarantinedPoisonedWaitingOut(key, name, version, commit_ids)
    \/ ScopeQuarantinedUnopened(key, name, version, commit_ids)
    \/ ScopeQuarantinedLost(key, name, version, commit_ids)

\* Every generated arm of a scope's poison.
PoisonArms(scope, cause) ==
    \/ ScopePoisonedLive(scope, cause)
    \/ ScopePoisonedStopped(scope, cause)
    \/ ScopePoisonedAcquiring(scope, cause)
    \/ ScopePoisonedProving(scope, cause)
    \/ ScopePoisonedResuming(scope, cause)
    \/ ScopePoisonedWaitingOut(scope, cause)
    \/ ScopePoisonedAgainLive(scope, cause)
    \/ ScopePoisonedAgainStopped(scope, cause)
    \/ ScopePoisonedAgainAcquiring(scope, cause)
    \/ ScopePoisonedAgainProving(scope, cause)
    \/ ScopePoisonedAgainResuming(scope, cause)
    \/ ScopePoisonedAgainWaitingOut(scope, cause)
    \/ ScopePoisonedQuarantinedLive(scope, cause)
    \/ ScopePoisonedQuarantinedStopped(scope, cause)
    \/ ScopePoisonedQuarantinedAcquiring(scope, cause)
    \/ ScopePoisonedQuarantinedProving(scope, cause)
    \/ ScopePoisonedQuarantinedResuming(scope, cause)
    \/ ScopePoisonedQuarantinedWaitingOut(scope, cause)
    \/ ScopePoisonedUnopened(scope, cause)
    \/ ScopePoisonedLost(scope, cause)

\* The open's listing ends: it listed, and nothing is reported.
AuditOpenListed ==
    /\ AuditPart = "scopes"
    /\ phase # "Unopened"
    /\ ~open_listed
    /\ open_listed' = TRUE
    /\ UNCHANGED << vars, storeVars, otherVars, lateVars, proofVars, waitVars,
                    w_failed_open, w_refused_after_failed_open, w_q_early, w_p_early >>

\* The open's listing failed: the realm reports it before any re-acquire may
\* begin (the open holds the gate a re-acquire takes).
AuditOpenListingFails ==
    /\ AuditPart = "scopes"
    /\ ~open_listed
    /\ \/ OpenListingFailedLive
       \/ OpenListingFailedStopped
    /\ open_listed' = TRUE
    /\ w_failed_open' = TRUE
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, waitVars,
                    w_refused_after_failed_open, w_q_early, w_p_early >>

\* A conflicted incarnation is reported: by the open's listing, a load that
\* meets the conflict, or the runtime store's open.
AuditQuarantine ==
    /\ AuditPart = "scopes"
    /\ \E report \in AuditReports, version \in 1..2, commit_ids \in {{}, {"c1"}} :
          QuarantineArms(report[1], report[2], version, commit_ids)
    /\ w_q_early' = (w_q_early \/ (grants = 0 /\ quarantined_keys' # {}))
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, waitVars,
                    open_listed, w_failed_open, w_refused_after_failed_open, w_p_early >>

\* A decision site poisons a scope.
AuditPoison ==
    /\ AuditPart = "scopes"
    /\ \E scope \in AuditScopeKeys, cause \in {"Unsettled", "Dropped"} :
          PoisonArms(scope, cause)
    /\ w_p_early' = (w_p_early \/ (grants = 0 /\ DOMAIN poisoned_scopes' # {}))
    /\ UNCHANGED << storeVars, otherVars, lateVars, proofVars, waitVars,
                    open_listed, w_failed_open, w_refused_after_failed_open, w_q_early >>

AuditNext ==
    \/ /\ \/ AuditOpen
          \/ AuditBeginReacquire
          \/ AuditOpenListed
          \/ AuditOpenListingFails
          \/ AuditQuarantine
          \/ AuditPoison
       /\ UNCHANGED << renew_open, commit_open >>
    \/ /\ \/ AuditRenewAnswersStopped
          \/ AuditWindowCloses
          \/ AuditRenewCurrent
          \/ AuditCommitFencedOut
          \/ AuditCommitSettles
          \/ /\ \/ AuditRenewSuperseded
                \/ AuditLeaseLapses
                \/ AuditAcquireGranted
                \/ AuditAcquireHeld
                \/ AuditAcquireTimedOut
                \/ AuditAcquireFailed
                \/ AuditLateDecision
                \/ AuditLeaseEnded
                \/ AuditListing
                \/ AuditResumeRenew
                \/ AuditOtherAcquires
                \/ AuditOtherCommits
                \/ AuditOtherStops
             /\ UNCHANGED << renew_open, commit_open >>
       /\ UNCHANGED scopeVars

AuditSpec == AuditInit /\ [][AuditNext]_<< vars, auditVars >>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* --- properties -------------------------------------------------------------

AuditNoFalseLoss ==
    phase = "Lost" => (ever_other \/ late_after_grant)

AuditNoResumeWithoutProof ==
    phase = "Live" => ~stopped_since_proof

AuditNoResumeOverForeignBatch ==
    ~resumed_over_foreign

AuditWaitsAgainOnlyUnrenewed ==
    ~waited_renewed_twice

AuditTokenNeverAboveTheStore ==
    token <= st_high

\* A realm whose open failed its listing never acquires again.
AuditNoReacquireAfterFailedOpen ==
    w_failed_open => phase \notin {"Acquiring", "Proving", "Resuming"}

\* The classifications only grow: no key, record or poison is removed, and
\* no record or cause changes, before a reopen.
AuditClassificationsMonotone ==
    [][ /\ quarantined_keys \subseteq quarantined_keys'
        /\ DOMAIN quarantine_versions \subseteq DOMAIN quarantine_versions'
        /\ \A name \in DOMAIN quarantine_versions :
              /\ quarantine_versions'[name] = quarantine_versions[name]
              /\ quarantine_commit_ids'[name] = quarantine_commit_ids[name]
        /\ DOMAIN poisoned_scopes \subseteq DOMAIN poisoned_scopes'
        /\ \A scope \in DOMAIN poisoned_scopes : poisoned_scopes'[scope] = poisoned_scopes[scope]
      ]_<< vars, auditVars >>

\* --- witnesses (the runner requires each negation violated) ----------------

AuditWitnessResumedAfterStop == phase = "Live" /\ grants >= 1
NotAuditWitnessResumedAfterStop == ~AuditWitnessResumedAfterStop

AuditWitnessWaitedOutOwnGrantResumed == w_waited_own /\ phase = "Live"
NotAuditWitnessWaitedOutOwnGrantResumed == ~AuditWitnessWaitedOutOwnGrantResumed

AuditWitnessHolderRenewed == phase = "Lost" /\ lost_cause = "HolderRenewed"
NotAuditWitnessHolderRenewed == ~AuditWitnessHolderRenewed

AuditWitnessForeignAfterWait == phase = "Lost" /\ lost_cause = "ForeignBatch" /\ waited # {}
NotAuditWitnessForeignAfterWait == ~AuditWitnessForeignAfterWait

AuditWitnessListingFailedResumed == w_listing_failed /\ phase = "Live"
NotAuditWitnessListingFailedResumed == ~AuditWitnessListingFailedResumed

AuditWitnessResidual == phase = "Lost" /\ ~ever_other /\ late_after_grant
NotAuditWitnessResidual == ~AuditWitnessResidual

AuditWitnessWaitedOutAgainResumed == w_waited_again /\ phase = "Live"
NotAuditWitnessWaitedOutAgainResumed == ~AuditWitnessWaitedOutAgainResumed

\* The scopes part's witnesses.
AuditWitnessQuarantineKeptAcrossReacquire ==
    w_q_early /\ grants >= 1 /\ phase = "Live" /\ quarantined_keys # {}
NotAuditWitnessQuarantineKeptAcrossReacquire == ~AuditWitnessQuarantineKeptAcrossReacquire

AuditWitnessPoisonKeptAcrossReacquire ==
    w_p_early /\ grants >= 1 /\ phase = "Live" /\ DOMAIN poisoned_scopes # {}
NotAuditWitnessPoisonKeptAcrossReacquire == ~AuditWitnessPoisonKeptAcrossReacquire

AuditWitnessFailedOpenRefusesReacquire == w_refused_after_failed_open
NotAuditWitnessFailedOpenRefusesReacquire == ~AuditWitnessFailedOpenRefusesReacquire

\* A chunk's conflict recorded under the chunk's own name and met again for
\* its owner: both keys quarantined over one record.
AuditWitnessChunkOwnerQuarantined ==
    /\ {"c", "k2"} \subseteq quarantined_keys
    /\ "c" \in DOMAIN quarantine_versions
NotAuditWitnessChunkOwnerQuarantined == ~AuditWitnessChunkOwnerQuarantined

=============================================================================
