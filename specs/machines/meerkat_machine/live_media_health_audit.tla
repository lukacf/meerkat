---- MODULE live_media_health_audit ----
\* Hand-written bounded audit of the live media health edges.
\*
\* The generated MeerkatMachine model's ci profile never binds a live channel,
\* so it cannot reach RequestLiveMediaHealth or ObserveLiveChannelMediaHealth
\* (both guard an Active channel with its exact runtime binding). This audit
\* keeps the generated model's variables, Init, actions and invariants
\* unchanged. A deterministic prefix registers a session, binds its runtime,
\* and opens and binds one live channel. Everything after the prefix is
\* explored exhaustively over the media-health surface of one session and two
\* channels: requesting media health for an output (with or without a
\* transcript), the three judgements (audible, silent with the session's one
\* reopen, silent after that reopen is spent), the channel's status reporting
\* closed, closing the channel, and opening and binding the second channel on
\* the same session (the reopen a media fault recommends).
\*
\* The TLC config is DERIVED from the generated ci.cfg by
\* live_media_health_audit.sh (every generated constant and invariant,
\* including live_media_health_budget_and_verdicts_are_consistent, plus the
\* audit values, invariants, action properties and step bound). The script also
\* proves each judgement reachable: for every goal it checks the goal's
\* negation and requires TLC to find a counterexample.
EXTENDS model

CONSTANT AuditMaxSteps

AuditSession == "session_1"
AuditFirstChannel == "channel_1"
AuditSecondChannel == "channel_2"
AuditChannels == {AuditFirstChannel, AuditSecondChannel}
AuditRuntime == "runtime_1"
AuditIdentity == "identity_1"
AuditOutputs == {"output_1", "output_2"}
\* Raw client counters: no audible frame and a peak below the 2000
\* micro-RMS floor, a peak at the floor, and audible frames.
AuditAudibleFrames == {0, 5}
AuditPeaks == {400, 2000}
AuditPrefixLength == 5

AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None)
    \/ model_step_count = 2 /\ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)
    \/ model_step_count = 3 /\ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditFirstChannel, AuditIdentity)
    \/ model_step_count = 4 /\ BindLiveExecutionChannelAttached(AuditSession, AuditFirstChannel, AuditRuntime, 1, 1, 0)

AuditMediaHealth ==
    \/ \E c \in AuditChannels : \E o \in AuditOutputs : \E nonempty \in BOOLEAN :
        \/ RequestLiveMediaHealthIdle(AuditSession, c, AuditRuntime, 1, 1, o, nonempty)
        \/ RequestLiveMediaHealthAttached(AuditSession, c, AuditRuntime, 1, 1, o, nonempty)
    \/ \E c \in AuditChannels : \E o \in AuditOutputs : \E audible \in AuditAudibleFrames : \E peak \in AuditPeaks :
        \/ ObserveLiveChannelMediaHealthAudibleIdle(AuditSession, c, o, 48000, audible, peak)
        \/ ObserveLiveChannelMediaHealthAudibleAttached(AuditSession, c, o, 48000, audible, peak)
        \/ ObserveLiveChannelMediaHealthSilentReopenIdle(AuditSession, c, o, 48000, audible, peak)
        \/ ObserveLiveChannelMediaHealthSilentReopenAttached(AuditSession, c, o, 48000, audible, peak)
        \/ ObserveLiveChannelMediaHealthSilentExhaustedIdle(AuditSession, c, o, 48000, audible, peak)
        \/ ObserveLiveChannelMediaHealthSilentExhaustedAttached(AuditSession, c, o, 48000, audible, peak)

AuditChannelLifecycle ==
    \/ \E c \in AuditChannels :
        \/ RecordLiveChannelStatusIdle(c, "Closed", 1, None, None)
        \/ RecordLiveChannelStatusAttached(c, "Closed", 1, None, None)
    \/ \E c \in AuditChannels :
        \/ RecordLiveCloseClosedIdle(AuditSession, c, 1)
        \/ RecordLiveCloseClosedAttached(AuditSession, c, 1)
    \/ ResolveLiveOpenAdmissionAcceptedIdle(AuditSession, AuditSecondChannel, AuditIdentity)
    \/ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditSecondChannel, AuditIdentity)
    \/ BindLiveExecutionChannelIdle(AuditSession, AuditSecondChannel, AuditRuntime, 1, 1, 0)
    \/ BindLiveExecutionChannelAttached(AuditSession, AuditSecondChannel, AuditRuntime, 1, 1, 0)

AuditNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ (AuditMediaHealth \/ AuditChannelLifecycle))

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* The session's one reopen: at most one channel ever carries a recommended
\* reopen (the audit has one session).
AuditAtMostOneRecommendedReopen ==
    Cardinality({c \in DOMAIN live_media_fault_reopen_recommended_by_channel :
        live_media_fault_reopen_recommended_by_channel[c] = TRUE}) <= 1

\* A reopen is recommended only while the session's budget records that one
\* fault, and a fault without the recommendation only after it.
AuditVerdictsFollowTheBudget ==
    \A c \in DOMAIN live_media_fault_reopen_recommended_by_channel :
        /\ AuditSession \in DOMAIN live_media_fault_reopens_by_session
        /\ live_media_fault_reopens_by_session[AuditSession] = 1

\* `LiveChannelStatusResolved.media_fault_reopen_recommended` is not a model
\* variable: the generated effect reads this verdict map directly (asserted by
\* the Rust generated-authority test). The goal GoalFaultedChannelReportsClosed
\* proves a faulted channel's closed status is reachable, the state that
\* projection reads from.

\* Judged once: a judged channel stays judged with an unchanged verdict.
AuditJudgedOnce ==
    [][\A c \in live_media_health_judged_channels :
        /\ c \in live_media_health_judged_channels'
        /\ (c \in DOMAIN live_media_fault_reopen_recommended_by_channel)
            = (c \in DOMAIN live_media_fault_reopen_recommended_by_channel')
        /\ c \in DOMAIN live_media_fault_reopen_recommended_by_channel =>
            live_media_fault_reopen_recommended_by_channel'[c]
                = live_media_fault_reopen_recommended_by_channel[c]]_vars

\* First output only: once a channel's output is requested, the requested
\* output never changes.
AuditFirstOutputOnly ==
    [][\A c \in DOMAIN live_media_health_requested_output_by_channel :
        /\ c \in DOMAIN live_media_health_requested_output_by_channel'
        /\ live_media_health_requested_output_by_channel'[c]
            = live_media_health_requested_output_by_channel[c]]_vars

\* Reachability goals: the script checks each negation and requires a
\* counterexample, proving the judgement is reachable within the bound.
GoalAudible ==
    \E c \in live_media_health_judged_channels :
        c \notin DOMAIN live_media_fault_reopen_recommended_by_channel
GoalSilentReopen ==
    \E c \in DOMAIN live_media_fault_reopen_recommended_by_channel :
        live_media_fault_reopen_recommended_by_channel[c] = TRUE
GoalSilentExhausted ==
    \E c \in DOMAIN live_media_fault_reopen_recommended_by_channel :
        live_media_fault_reopen_recommended_by_channel[c] = FALSE
GoalFaultedChannelReportsClosed ==
    \E c \in DOMAIN live_media_fault_reopen_recommended_by_channel :
        /\ c \in DOMAIN live_channel_status_by_channel
        /\ live_channel_status_by_channel[c] = "Closed"
NotGoalAudible == ~GoalAudible
NotGoalSilentReopen == ~GoalSilentReopen
NotGoalSilentExhausted == ~GoalSilentExhausted
NotGoalFaultedChannelReportsClosed == ~GoalFaultedChannelReportsClosed
====
