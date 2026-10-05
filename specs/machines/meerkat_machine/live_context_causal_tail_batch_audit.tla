---- MODULE live_context_causal_tail_batch_audit ----
\* Hand-written bounded audit of the live-context causal-tail batch edge.
\*
\* The heard-speech replays queued behind a late bootstrap summary (rows
\* reasserting the causal tail or the assistant's output) are delivered as ONE
\* append covering the exact cursor run (AuthorizeLiveContextCausalTailBatch),
\* instead of one append per row (Turbo S S99). The batch must cover exactly
\* the run (previous, next], every row of which is a queued heard-speech
\* replay of the channel's session; the resolve edges settle a pending append
\* through its own pinned previous/next cursors, so a batch resolves exactly
\* like a single row.
\*
\* This audit keeps the generated MeerkatMachine model unchanged. A
\* deterministic prefix reaches one experimental channel whose bootstrap
\* summary was delivered and acknowledged, with two heard-speech replays
\* queued behind it (cursors 2 and 3) and one ordinary voiced row after them
\* (cursor 4). The body explores the batch edge over every subset of cursors as
\* the claimed tail, the one-row authorize edges, and all four resolve edges
\* (delivered, rejected, ambiguous, interrupted by close). Every explored state
\* is checked against every generated invariant, plus two action properties
\* over ANY step that makes a multi-row append pending:
\*   AuditPendingRunCoversOnlyHeardSpeechReplays: every cursor of the new
\*     pending run was a queued heard-speech replay of the session;
\*   AuditPendingRunSkipsNoQueuedRow: no queued row is left inside the run.
\* and three more over every step:
\*   AuditResolveIsPinned: a resolve (delivered, rejected, ambiguous,
\*     interrupted by close) fires only with the pending append's recorded
\*     previous and next cursors, for a single row and a batch alike (the
\*     body tries every cursor pair, so the property is not vacuous);
\*   AuditNewPendingStartsAtCursor: a new pending append starts at the
\*     channel cursor (no cursor before it is skipped);
\*   AuditNoCursorDeliveredTwice: no new pending append covers a cursor
\*     already delivered (no duplicate).
\* After a rejected batch the shell re-enqueues each carried row at its own
\* canonical cursor (EnqueueLiveContextRow); the body models exactly that.
\* live_context_causal_tail_batch_audit.sh runs each goal below as the only
\* property and requires TLC to report it violated:
\*   - a batch append covering at least two rows is authorized;
\*   - a batch is delivered: the cursor reaches the last tail cursor and no
\*     heard-speech replay is left queued;
\*   - a batch is rejected;
\*   - after a rejected batch, the re-enqueued rows are delivered again: the
\*     cursor reaches the last tail cursor.
\* With --mutants it also requires the safety run to refuse a model whose
\* batch edge drops the exact-range length check (a gap is admitted), one
\* whose batch edge drops the disposition check, and one whose resolve edges
\* drop the pending next-cursor match.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* live_context_causal_tail_batch_audit.sh. Run a deeper bound by hand:
\*   specs/machines/meerkat_machine/live_context_causal_tail_batch_audit.sh 26
EXTENDS model

CONSTANT AuditMaxSteps

AuditSession == "sessionid_1"
AuditRuntime == "runtime_1"
AuditIdentity == "identity_1"
AuditChannel == "channel_a"
AuditLease == "lease_1"
AuditDigest == "digest_s"
AuditBootstrap == "append_boot"
AuditTailA == "append_2"
AuditTailB == "append_3"
AuditVoiced == "append_4"
AuditAppends == {AuditTailA, AuditTailB, AuditVoiced}
AuditCursors == 1..4
AuditHeardSpeech == {"ReassertCausalTail", "ReassertAssistantOutput"}
AuditPrefixLength == 17

VARIABLES
    audit_batch_delivered,     \* a multi-row pending append was delivered
    audit_batch_rejected,      \* a multi-row pending append was rejected
    audit_reenqueued,          \* a carried row was re-enqueued after that
    audit_delivered_cursors    \* every cursor a delivered append covered

auditVars == <<audit_batch_delivered, audit_batch_rejected, audit_reenqueued, audit_delivered_cursors>>

\* A carried heard-speech replay re-enqueued by the shell at its own
\* canonical cursor after a rejection.
ReenqueueCarried ==
    \/ EnqueueLiveContextRowAttached(AuditChannel, AuditRuntime, 1, 1, AuditTailA, 2, "digest_2", "commit_2", "AlreadyPresentInLiveChannel", "Materializable", "Conversation", "User", Some("obs_1"))
    \/ EnqueueLiveContextRowAttached(AuditChannel, AuditRuntime, 1, 1, AuditTailB, 3, "digest_3", "commit_3", "AlreadyPresentInLiveChannel", "Materializable", "Conversation", "Assistant", Some("obs_2"))

\* A deterministic prefix: a bound experimental channel with two observed
\* heard-speech replays and one voiced row queued behind its summary, which
\* is then delivered and acknowledged.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ model_step_count = 3 /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditChannel, AuditIdentity)
    \/ model_step_count = 4 /\ ResolveLiveExecutionModeAdmissionAttached(AuditSession, AuditChannel, "profile_1", "ClientContext", FALSE, TRUE)
    \/ model_step_count = 5 /\ StageExperimentalLiveExecutionAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, 0, "pending_a")
    \/ model_step_count = 6 /\ BeginLiveContextPreparationAttached(AuditSession, AuditChannel, AuditLease, 1, AuditRuntime, 1, 1)
    \/ model_step_count = 7 /\ GenerateLiveContextPreparationAttached(AuditSession, AuditChannel, AuditLease)
    \/ model_step_count = 8 /\ RegisterLivePlaybackOwnerAttached(AuditSession, AuditChannel, AuditRuntime, 1, 1, "owner_1", "ready_1", "pending_a")
    \/ model_step_count = 9 /\ RecordLiveWebrtcAnswerAcceptedAndBindExecutionAttached(AuditSession, AuditChannel, 1, AuditRuntime, 1, 1, 0, "activation_a")
    \/ model_step_count = 10 /\ RecordLiveContextObservationAttached(AuditSession, AuditChannel, AuditLease, AuditRuntime, 1, 1, "obs_1", AuditLease, AuditChannel)
    \/ model_step_count = 11 /\ RecordLiveContextObservationAttached(AuditSession, AuditChannel, AuditLease, AuditRuntime, 1, 1, "obs_2", AuditLease, AuditChannel)
    \* The user's speech the provider already heard: a causal-tail replay.
    \/ model_step_count = 12 /\ EnqueueLiveContextRowAttached(AuditChannel, AuditRuntime, 1, 1, AuditTailA, 2, "digest_2", "commit_2", "AlreadyPresentInLiveChannel", "Materializable", "Conversation", "User", Some("obs_1"))
    \* The assistant's output the provider already produced.
    \/ model_step_count = 13 /\ EnqueueLiveContextRowAttached(AuditChannel, AuditRuntime, 1, 1, AuditTailB, 3, "digest_3", "commit_3", "AlreadyPresentInLiveChannel", "Materializable", "Conversation", "Assistant", Some("obs_2"))
    \* An ordinary voiced row after the run: never part of a batch.
    \/ model_step_count = 14 /\ EnqueueLiveContextRowAttached(AuditChannel, AuditRuntime, 1, 1, AuditVoiced, 4, "digest_4", "commit_4", "MirrorParentText", "Materializable", "Conversation", "User", None)
    \/ model_step_count = 15 /\ AuthorizeLiveContextBootstrapAppendAttached(AuditSession, AuditChannel, AuditLease, AuditBootstrap, AuditDigest, 1)
    \/ model_step_count = 16 /\ RecordLiveContextBootstrapAckCutAttached(AuditSession, AuditChannel, AuditLease, AuditRuntime, 1, 1, AuditBootstrap, AuditDigest, 1)

\* The bootstrap summary resolution is part of the body (its exact
\* complement arguments are the queued maps above the reserved cursor).
ResolveBootstrapDelivered ==
    ResolveLiveContextBootstrapAppendAttached(AuditSession, AuditChannel, AuditLease, AuditBootstrap, AuditDigest, 1, "Delivered",
        live_context_queued_session_by_append, live_context_queued_cursor_by_append,
        live_context_queued_digest_by_append, live_context_queued_commit_token_by_append,
        live_context_queued_disposition_by_append, live_context_queued_append_by_cursor)

AuditPendingRange(a) ==
    live_context_pending_next_cursor_by_append[a] - live_context_pending_previous_cursor_by_append[a]

AuditIsBatch(a) == a \in DOMAIN live_context_pending_channel_by_append /\ AuditPendingRange(a) >= 2

Resolve(observation, a, p, n) ==
    \E seed \in 0..4 :
        \/ observation = "Delivered" /\ ResolveLiveContextAppendDeliveredAttached(AuditChannel, AuditRuntime, 1, 1, a, p, n, "", seed, observation)
        \/ observation = "Rejected" /\ ResolveLiveContextAppendRejectedAttached(AuditChannel, AuditRuntime, 1, 1, a, p, n, "", seed, observation)
        \/ observation = "Ambiguous" /\ ResolveLiveContextAppendAmbiguousAttached(AuditChannel, AuditRuntime, 1, 1, a, p, n, "", seed, observation)
        \/ observation = "InterruptedByClose" /\ ResolveLiveContextAppendInterruptedByCloseAttached(AuditChannel, AuditRuntime, 1, 1, a, p, n, "", seed, observation)

\* The batch's next cursor: the largest claimed tail cursor (or previous + 2
\* for an empty claim, which every guard refuses).
Max4(tail, p) == IF tail = {} THEN p + 2 ELSE CHOOSE m \in tail : \A t \in tail : t <= m

AuditBody ==
    \/ (\/ ResolveBootstrapDelivered
        \/ \E a \in AuditAppends, p \in 0..3, tail \in SUBSET (2..4) :
            AuthorizeLiveContextCausalTailBatchAttached(AuditChannel, AuditRuntime, 1, 1, a, p, Max4(tail, p), tail)
        \/ \E a \in AuditAppends, p \in 0..3 :
            \/ AuthorizeLiveContextAppendAttached(AuditChannel, AuditRuntime, 1, 1, a, p, p + 1)
            \/ AuthorizeLiveContextAppendSupersededAttached(AuditChannel, AuditRuntime, 1, 1, a, p, p + 1)
            \/ AuthorizeLiveContextAppendPendingReplayAttached(AuditChannel, AuditRuntime, 1, 1, a, p, p + 1)
            \/ AuthorizeLiveContextAppendDeliveredReplayAttached(AuditChannel, AuditRuntime, 1, 1, a, p, p + 1))
       /\ UNCHANGED auditVars
    \/ ReenqueueCarried
       /\ audit_reenqueued' = (audit_reenqueued \/ audit_batch_rejected)
       /\ UNCHANGED <<audit_batch_delivered, audit_batch_rejected, audit_delivered_cursors>>
    \* Every resolve is tried with every cursor pair, not only the pinned one.
    \/ \E a \in AuditAppends, observation \in {"Delivered", "Rejected", "Ambiguous", "InterruptedByClose"},
          p \in 0..4, n \in 0..4 :
        /\ a \in DOMAIN live_context_pending_channel_by_append
        /\ Resolve(observation, a, p, n)
        /\ audit_batch_delivered' = (audit_batch_delivered \/ (observation = "Delivered" /\ AuditIsBatch(a)))
        /\ audit_batch_rejected' = (audit_batch_rejected \/ (observation = "Rejected" /\ AuditIsBatch(a)))
        /\ audit_delivered_cursors' =
             IF observation = "Delivered"
             THEN audit_delivered_cursors \cup ((live_context_pending_previous_cursor_by_append[a] + 1)..live_context_pending_next_cursor_by_append[a])
             ELSE audit_delivered_cursors
        /\ UNCHANGED audit_reenqueued

AuditNext ==
    \/ AuditPrefix /\ UNCHANGED auditVars
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec ==
    /\ Init
    /\ audit_batch_delivered = FALSE
    /\ audit_batch_rejected = FALSE
    /\ audit_reenqueued = FALSE
    /\ audit_delivered_cursors = {}
    /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* An append that becomes pending in this step and covers more than one row.
NewlyPendingRun(a) ==
    /\ a \notin DOMAIN live_context_pending_channel_by_append
    /\ a \in DOMAIN live_context_pending_channel_by_append'
    /\ live_context_pending_next_cursor_by_append'[a] - live_context_pending_previous_cursor_by_append'[a] >= 2

RunCursors(a) ==
    (live_context_pending_previous_cursor_by_append'[a] + 1)..live_context_pending_next_cursor_by_append'[a]

\* Safety: every cursor of a new multi-row pending run was a queued
\* heard-speech replay of the channel's session.
AuditPendingRunCoversOnlyHeardSpeechReplays ==
    [][\A a \in AuditAppends :
        NewlyPendingRun(a) =>
            \A c \in RunCursors(a) :
                /\ c \in DOMAIN live_context_queued_append_by_cursor
                /\ live_context_queued_append_by_cursor[c] \in DOMAIN live_context_queued_disposition_by_append
                /\ live_context_queued_disposition_by_append[live_context_queued_append_by_cursor[c]] \in AuditHeardSpeech
                /\ live_context_queued_session_by_append[live_context_queued_append_by_cursor[c]] = AuditSession]_<<vars, auditVars>>

\* Safety: a new multi-row pending run leaves no queued row inside it.
AuditPendingRunSkipsNoQueuedRow ==
    [][\A a \in AuditAppends :
        NewlyPendingRun(a) =>
            \A c \in RunCursors(a) : c \notin DOMAIN live_context_queued_append_by_cursor']_<<vars, auditVars>>

\* Safety: a resolve fires only with the pending append's recorded cursors.
AuditResolveIsPinned ==
    [][\A a \in AuditAppends, observation \in {"Delivered", "Rejected", "Ambiguous", "InterruptedByClose"},
         p \in 0..4, n \in 0..4 :
        (a \in DOMAIN live_context_pending_channel_by_append /\ Resolve(observation, a, p, n))
            => /\ n = live_context_pending_next_cursor_by_append[a]
               /\ p = live_context_pending_previous_cursor_by_append[a]]_<<vars, auditVars>>

NewlyPending(a) ==
    /\ a \notin DOMAIN live_context_pending_channel_by_append
    /\ a \in DOMAIN live_context_pending_channel_by_append'

\* Safety: a new pending append starts at the channel cursor (no skip).
AuditNewPendingStartsAtCursor ==
    [][\A a \in AuditAppends :
        NewlyPending(a) =>
            live_context_pending_previous_cursor_by_append'[a] = live_context_cursor_by_channel[AuditChannel]]_<<vars, auditVars>>

\* Safety: no new pending append covers an already delivered cursor.
AuditNoCursorDeliveredTwice ==
    [][\A a \in AuditAppends :
        NewlyPending(a) =>
            ((live_context_pending_previous_cursor_by_append'[a] + 1)..live_context_pending_next_cursor_by_append'[a])
                \cap audit_delivered_cursors = {}]_<<vars, auditVars>>

QueuedHeardSpeech ==
    {c \in DOMAIN live_context_queued_append_by_cursor :
        live_context_queued_append_by_cursor[c] \in DOMAIN live_context_queued_disposition_by_append
        /\ live_context_queued_disposition_by_append[live_context_queued_append_by_cursor[c]] \in AuditHeardSpeech}

\* Goals (expected violated).
AuditNeverBatchAuthorized == ~(\E a \in AuditAppends : AuditIsBatch(a))
AuditNeverBatchDelivered ==
    ~(/\ audit_batch_delivered
      /\ AuditChannel \in DOMAIN live_context_cursor_by_channel
      /\ live_context_cursor_by_channel[AuditChannel] = 3
      /\ QueuedHeardSpeech = {})
AuditNeverBatchRejected == ~audit_batch_rejected
AuditNeverRedeliveredAfterReject ==
    ~(/\ audit_reenqueued
      /\ AuditChannel \in DOMAIN live_context_cursor_by_channel
      /\ live_context_cursor_by_channel[AuditChannel] = 3)
====
