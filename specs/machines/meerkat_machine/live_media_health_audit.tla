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
\* channels, in the Attached phase and, once a run starts (PrepareAttached),
\* in Running: requesting media health for an output (the request's guard
\* admits only a non-empty transcript), the three judgements (audible, silent
\* with the session's one reopen, silent after that reopen is spent), the
\* channel's status reporting closed, closing the channel, and opening and
\* binding the second channel on the same session (the reopen a media fault
\* recommends), and stopping and resuming or unregistering the session
\* (media health is per session lifetime: the resumed session earns its reopen
\* again; unregistering clears it too). The edges have no Idle variant: an idle session has no attached
\* runtime to bind a live channel to.
\*
\* The TLC config is DERIVED from the generated ci.cfg by
\* live_media_health_audit.sh (every generated constant and invariant,
\* including live_media_health_budget_and_verdicts_are_consistent, plus the
\* audit values, invariants, action properties and step bound). The script
\* also proves each judgement reachable (for every goal it checks the goal's
\* negation and requires a counterexample) and each new transition firing
\* (for every Never* action property it requires TLC to report a violation).
EXTENDS model

CONSTANT AuditMaxSteps

\* Audit history: how many times a reopen was recommended along the
\* behavior. The budget allows one per session lifetime, so a second one
\* proves the budget reset with the session.
VARIABLE audit_recommended_reopens

AuditSession == "session_1"
AuditFirstChannel == "channel_1"
AuditSecondChannel == "channel_2"
AuditChannels == {AuditFirstChannel, AuditSecondChannel}
AuditRuntime == "runtime_1"
AuditIdentity == "identity_1"
AuditRun == "run_1"
AuditStopReason == "stop"
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

AuditRequestAttached ==
    \E c \in AuditChannels : \E o \in AuditOutputs : \E nonempty \in BOOLEAN :
        RequestLiveMediaHealthAttached(AuditSession, c, AuditRuntime, 1, 1, o, nonempty)
AuditRequestRunning ==
    \E c \in AuditChannels : \E o \in AuditOutputs : \E nonempty \in BOOLEAN :
        RequestLiveMediaHealthRunning(AuditSession, c, AuditRuntime, 1, 1, o, nonempty)
AuditAudibleAttached ==
    \E c \in AuditChannels : \E o \in AuditOutputs : \E audible \in AuditAudibleFrames : \E peak \in AuditPeaks :
        ObserveLiveChannelMediaHealthAudibleAttached(AuditSession, c, o, 48000, audible, peak)
AuditAudibleRunning ==
    \E c \in AuditChannels : \E o \in AuditOutputs : \E audible \in AuditAudibleFrames : \E peak \in AuditPeaks :
        ObserveLiveChannelMediaHealthAudibleRunning(AuditSession, c, o, 48000, audible, peak)
AuditSilentReopenAttached ==
    \E c \in AuditChannels : \E o \in AuditOutputs : \E audible \in AuditAudibleFrames : \E peak \in AuditPeaks :
        ObserveLiveChannelMediaHealthSilentReopenAttached(AuditSession, c, o, 48000, audible, peak)
AuditSilentReopenRunning ==
    \E c \in AuditChannels : \E o \in AuditOutputs : \E audible \in AuditAudibleFrames : \E peak \in AuditPeaks :
        ObserveLiveChannelMediaHealthSilentReopenRunning(AuditSession, c, o, 48000, audible, peak)
AuditSilentExhaustedAttached ==
    \E c \in AuditChannels : \E o \in AuditOutputs : \E audible \in AuditAudibleFrames : \E peak \in AuditPeaks :
        ObserveLiveChannelMediaHealthSilentExhaustedAttached(AuditSession, c, o, 48000, audible, peak)
AuditSilentExhaustedRunning ==
    \E c \in AuditChannels : \E o \in AuditOutputs : \E audible \in AuditAudibleFrames : \E peak \in AuditPeaks :
        ObserveLiveChannelMediaHealthSilentExhaustedRunning(AuditSession, c, o, 48000, audible, peak)

AuditMediaHealth ==
    \/ AuditRequestAttached \/ AuditRequestRunning
    \/ AuditAudibleAttached \/ AuditAudibleRunning
    \/ AuditSilentReopenAttached \/ AuditSilentReopenRunning
    \/ AuditSilentExhaustedAttached \/ AuditSilentExhaustedRunning

AuditChannelLifecycle ==
    \/ \E c \in AuditChannels :
        \/ RecordLiveChannelStatusAttached(c, "Closed", 1, None, None)
        \/ RecordLiveChannelStatusRunning(c, "Closed", 1, None, None)
    \/ \E c \in AuditChannels :
        \/ RecordLiveCloseClosedAttached(AuditSession, c, 1)
        \/ RecordLiveCloseClosedRunning(AuditSession, c, 1)
    \/ ResolveLiveOpenAdmissionAcceptedAttached(AuditSession, AuditSecondChannel, AuditIdentity)
    \/ ResolveLiveOpenAdmissionAcceptedRunning(AuditSession, AuditSecondChannel, AuditIdentity)
    \/ BindLiveExecutionChannelAttached(AuditSession, AuditSecondChannel, AuditRuntime, 1, 1, 0)
    \/ BindLiveExecutionChannelRunning(AuditSession, AuditSecondChannel, AuditRuntime, 1, 1, 0)

\* A run starts on the attached session; the media-health edges then run in
\* Running.
AuditRunLifecycle == PrepareAttached(AuditSession, AuditRun)

\* The session lifecycle around media health: unregistering (begin, the
\* three drains, unregister; the machine then stays draining and a new
\* registration starts on a fresh authority), and stopping the runtime and
\* resuming the stopped session with a fresh runtime binding (the same
\* machine continues, so its media-health state must reset with the new
\* lifetime).
\* The two lifecycles are explored apart: an unregister never begins with a
\* runtime stop deferred, and a stop is never requested while draining.
\* (Interleaving them reaches a pre-existing gap outside media health:
\* UnregisterSession leaves runtime_stop_deferred set while moving to Idle,
\* which violates the generated deferred_stop_requires_active_runtime_phase.)
AuditSessionLifecycle ==
    \/ runtime_stop_deferred = FALSE
        /\ BeginUnregisterSessionAttached(AuditSession, active_runtime_id, active_fence_token, active_runtime_generation, active_runtime_epoch_id)
    \/ RuntimeLoopStoppedForUnregisterAttached(AuditSession, FALSE)
    \/ CommsDrainExitedForUnregisterAttached(AuditSession, FALSE)
    \/ CompletionWaitersResolvedForUnregisterAttached(AuditSession)
    \/ UnregisterSessionAttached(AuditSession, active_runtime_id, active_fence_token, active_runtime_generation, active_runtime_epoch_id)
    \/ registration_phase # "Draining" /\ StopRuntimeExecutorAttached(AuditStopReason)
    \/ RuntimeExecutorExitedFromAttached
    \/ RegisterSessionResumesStopped(AuditSession, None)
    \/ PrepareBindingsIdle(AuditRuntime, 1, Some(1), None, AuditSession)

AuditModelNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength
        /\ (AuditMediaHealth \/ AuditChannelLifecycle \/ AuditRunLifecycle \/ AuditSessionLifecycle))

AuditNewlyRecommended ==
    \E c \in DOMAIN live_media_fault_reopen_recommended_by_channel' :
        /\ live_media_fault_reopen_recommended_by_channel'[c] = TRUE
        /\ ~(c \in DOMAIN live_media_fault_reopen_recommended_by_channel
              /\ live_media_fault_reopen_recommended_by_channel[c] = TRUE)

AuditNext ==
    /\ AuditModelNext
    /\ audit_recommended_reopens' =
        audit_recommended_reopens + (IF AuditNewlyRecommended THEN 1 ELSE 0)

AuditInit == Init /\ audit_recommended_reopens = 0

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, audit_recommended_reopens>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* The session's one reopen: at most one channel ever carries a recommended
\* reopen (the audit has one session).
AuditAtMostOneRecommendedReopen ==
    Cardinality({c \in DOMAIN live_media_fault_reopen_recommended_by_channel :
        live_media_fault_reopen_recommended_by_channel[c] = TRUE}) <= 1

\* Every verdict is recorded against the session's spent budget, and a fault
\* without the recommendation exists only beside the earlier fault that spent
\* it (some channel of the session carries the recommended reopen).
AuditVerdictsFollowTheBudget ==
    /\ \A c \in DOMAIN live_media_fault_reopen_recommended_by_channel :
        /\ AuditSession \in DOMAIN live_media_fault_reopens_by_session
        /\ live_media_fault_reopens_by_session[AuditSession] = 1
    /\ \A c \in DOMAIN live_media_fault_reopen_recommended_by_channel :
        live_media_fault_reopen_recommended_by_channel[c] = FALSE =>
            \E earlier \in DOMAIN live_media_fault_reopen_recommended_by_channel :
                live_media_fault_reopen_recommended_by_channel[earlier] = TRUE

\* `LiveChannelStatusResolved.media_fault_reopen_recommended` is not a model
\* variable: the generated effect reads this verdict map directly (asserted by
\* the Rust generated-authority test). GoalFaultedChannelReportsClosed proves
\* a faulted channel's closed status is reachable, the state that projection
\* reads from.

\* While its channel stays open within the session's lifetime: still bound,
\* on the registered session, with its runtime still attached (a stop and
\* resume, or an unregister, ends the lifetime and clears the runtime).
AuditStaysOpen(c) ==
    /\ c \in DOMAIN live_channel_session_by_channel'
    /\ session_id' # None
    /\ active_runtime_id' # None

\* Judged once: while the channel stays open, a judged channel stays judged
\* with an unchanged verdict.
AuditJudgedOnce ==
    [][\A c \in live_media_health_judged_channels :
        AuditStaysOpen(c) =>
            /\ c \in live_media_health_judged_channels'
            /\ (c \in DOMAIN live_media_fault_reopen_recommended_by_channel)
                = (c \in DOMAIN live_media_fault_reopen_recommended_by_channel')
            /\ c \in DOMAIN live_media_fault_reopen_recommended_by_channel =>
                live_media_fault_reopen_recommended_by_channel'[c]
                    = live_media_fault_reopen_recommended_by_channel[c]]_vars

\* First output only: while the channel stays open, its requested output
\* never changes.
AuditFirstOutputOnly ==
    [][\A c \in DOMAIN live_media_health_requested_output_by_channel :
        AuditStaysOpen(c) =>
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
\* The session earns its reopen again after its runtime stops and the
\* stopped session resumes: a second recommended reopen along one behavior.
GoalReRegisteredSilentReopen ==
    /\ audit_recommended_reopens >= 2
    \* earned on the fresh channel opened after the first one closed
    /\ AuditFirstChannel \notin DOMAIN live_channel_session_by_channel
    /\ AuditSecondChannel \in DOMAIN live_media_fault_reopen_recommended_by_channel
    /\ live_media_fault_reopen_recommended_by_channel[AuditSecondChannel] = TRUE
\* A session unregisters after a media fault was recommended: the state in
\* which the generated invariant requires every media map to be empty, so its
\* unregistered-session clause is checked on a reachable state, not vacuously.
GoalUnregisteredAfterFault ==
    /\ session_id = None
    /\ audit_recommended_reopens >= 1
NotGoalAudible == ~GoalAudible
NotGoalSilentReopen == ~GoalSilentReopen
NotGoalSilentExhausted == ~GoalSilentExhausted
NotGoalFaultedChannelReportsClosed == ~GoalFaultedChannelReportsClosed
NotGoalReRegisteredSilentReopen == ~GoalReRegisteredSilentReopen
NotGoalUnregisteredAfterFault == ~GoalUnregisteredAfterFault

\* Firing: each new transition must fire in the explored space. The script
\* checks every action property below and requires TLC to report it
\* violated, so an edge that never fires fails the audit instead of reading
\* as coverage.
NeverRequestAttached == [][~AuditRequestAttached]_vars
NeverRequestRunning == [][~AuditRequestRunning]_vars
NeverAudibleAttached == [][~AuditAudibleAttached]_vars
NeverAudibleRunning == [][~AuditAudibleRunning]_vars
NeverSilentReopenAttached == [][~AuditSilentReopenAttached]_vars
NeverSilentReopenRunning == [][~AuditSilentReopenRunning]_vars
NeverSilentExhaustedAttached == [][~AuditSilentExhaustedAttached]_vars
NeverSilentExhaustedRunning == [][~AuditSilentExhaustedRunning]_vars
====
