import React, { useEffect, useMemo, useRef, useState } from "react";
import { ConversationTranscript } from "../../packages/console-components/src/conversation/conversation-transcript";
import { groupConversationTranscriptTurns } from "../../packages/console-components/src/conversation/conversation-turns";
import { transcriptDayKey } from "../../packages/console-core/src/index.ts";
import { Icon as ConsoleIcon, SpriteSheet } from "../../console/src/icon";
import { createGuideFrameLog, ingestGuideTimeline, guideConversation, guideTranscriptSections, nextGuideTimelineCursor } from "./native-guide-chat.mjs";
import { request } from "./state.mjs";
import { createGuideStartupRecovery, guideStartupPresentation, guideStartupDetail } from "./guide-startup.mjs";
import { isSignInRequired } from "./sign-in-recovery.mjs";
import { SignInRecovery } from "./sign-in-recovery.jsx";

export function useGuideTimeline(state) {
  const [snapshot, setSnapshot] = useState({ guideId: null, frames: [], bindings: [], error: "", available: false });
  const latestState = useRef(state);
  const wakePoll = useRef(null);
  const retryStartup = useRef(null);
  const checkStartup = useRef(null);
  const requireSignIn = useRef(null);
  latestState.current = state;
  useEffect(() => {
    if (!state?.id) return;
    let stopped = false, timer, after, inFlight = false, authenticationRequired = false, authRevision = 0;
    const currentLog = createGuideFrameLog();
    const seenBindings = new Set();
    setSnapshot({ guideId: state.id, frames: [], bindings: [], error: "", available: false });
    const recovery = createGuideStartupRecovery({ guideId: state.id,
      currentGuideId: () => latestState.current?.id,
      request, onUpdate: (patch) => setSnapshot((previous) => previous.guideId === state.id
        ? { ...previous, ...patch } : previous),
      onRecovered: () => { if (!authenticationRequired) wake(); },
    });
    async function poll() {
      if (stopped || inFlight || recovery.isRetrying()) return false;
      const generation = recovery.generation();
      const currentAuthRevision = authRevision;
      inFlight = true;
      let delay = latestState.current?.operation?.status === "running" ? 1000 : 4000;
      try {
        const query = new URLSearchParams({ limit: "500" });
        if (after) query.set("after", after);
        const page = await request(`/api/guide/${encodeURIComponent(state.id)}/timeline?${query}`, { timeoutMs: 30000 });
        if (stopped || !recovery.current(generation) || authRevision !== currentAuthRevision) return false;
        authenticationRequired = false;
        recovery.timelineRecovered();
        const frames = ingestGuideTimeline(currentLog, page);
        setSnapshot({ guideId: state.id, frames: [...frames], bindings: page.bindings || [], available: page.available !== false, error: "", startup: null, retrying: false, retryError: "" });
        const next = nextGuideTimelineCursor(after, page, seenBindings);
        const hasMore = next.replay || page.exhausted === false && next.after && next.after !== after;
        after = next.after;
        if (hasMore) delay = 0;
        return true;
      } catch (failure) {
        if (stopped || !recovery.current(generation) || authRevision !== currentAuthRevision) return false;
        if (isSignInRequired(failure) || authenticationRequired) {
          blockForSignIn();
          return false;
        }
        setSnapshot((previous) => ({ ...previous, error: failure.message }));
        await recovery.observeFailure(generation);
        delay = 2500;
        return false;
      } finally {
        inFlight = false;
        if (!stopped && !authenticationRequired) timer = setTimeout(poll, recovery.generation() === generation ? delay : 0);
      }
    }
    const wake = () => { clearTimeout(timer); return poll(); };
    function blockForSignIn() {
      if (stopped || latestState.current?.id !== state.id) return;
      authenticationRequired = true;
      authRevision += 1;
      clearTimeout(timer);
      setSnapshot(previous => previous.guideId === state.id ? { ...previous,
        authenticationRequired: true, error: "", startup: null, retryError: "" } : previous);
    }
    requireSignIn.current = blockForSignIn;
    wakePoll.current = wake;
    const retry = () => authenticationRequired ? false : recovery.retry();
    retryStartup.current = retry;
    const check = () => authenticationRequired ? false : recovery.checkStatus();
    checkStartup.current = check;
    poll();
    return () => {
      stopped = true;
      recovery.dispose();
      clearTimeout(timer);
      if (wakePoll.current === wake) wakePoll.current = null;
      if (retryStartup.current === retry) retryStartup.current = null;
      if (checkStartup.current === check) checkStartup.current = null;
      if (requireSignIn.current === blockForSignIn) requireSignIn.current = null;
    };
  }, [state?.id]);
  useEffect(() => {
    if (state?.operation?.status === "running") wakePoll.current?.();
  }, [state?.id, state?.operation?.request_id, state?.operation?.status]);
  const retrySetupTeam = retryStartup.current;
  const checkSetupTeamStatus = checkStartup.current;
  return snapshot.guideId === state?.id ? { ...snapshot, retrySetupTeam, checkSetupTeamStatus,
    checkSignIn: wakePoll.current, requireSignIn: requireSignIn.current }
    : { frames: [], bindings: [], error: "", available: false };
}

export function NativeGuideChat({ state, snapshot, pendingMessage, Icon, onActivity, signInNoticeExternal = false, activityStatusExternal = false }) {
  const conversation = useMemo(() => guideConversation({ state,
    frames: snapshot.frames, bindings: snapshot.bindings, pendingMessage,
  }), [state, snapshot.frames, snapshot.bindings, pendingMessage]);
  useEffect(() => onActivity?.(conversation.entries), [conversation.entries, onActivity]);
  const startup = guideStartupPresentation(state, snapshot);
  const startupDetail = snapshot.guideId === state.id ? guideStartupDetail(snapshot.startup) : null;
  const turns = groupConversationTranscriptTurns(conversation.viewState.groups);
  let previousDay = null;
  return (
    <div className="setup-native-chat" data-native-console-chat="true">
      <SpriteSheet />
      {turns.map((turn, index) => {
        const sections = guideTranscriptSections(turn.groups);
        const days = sections.conversation.flatMap(group => group.entries
          .map(entry => transcriptDayKey(entry.createdAt)).filter(Boolean));
        const repeatsDay = days.length > 0 && days[0] === previousDay;
        if (days.length) previousDay = days.at(-1);
        return <div key={`${state.id}:${turn.id}`} className="setup-guide-turn"
          data-current-guide-transcript={index === turns.length - 1 ? "true" : undefined}
          data-guide-turn={index + 1} data-repeated-day={repeatsDay || undefined}>
          <ConversationTranscript viewState={{ ...conversation.viewState, groups: sections.conversation }}
            Icon={ConsoleIcon} showTurnDiff={false} turnIndexOffset={index} />
          {sections.activity.length > 0 && <details className="setup-conversation-history" data-guide-technical-activity="true">
            <summary>Technical activity <span className="setup-visually-hidden">for conversation turn {index + 1}</span></summary>
            <ConversationTranscript viewState={{ ...conversation.viewState, groups: sections.activity }}
              Icon={ConsoleIcon} compact showTurnDiff={false} turnIndexOffset={index} />
          </details>}
        </div>;
      })}
      {startup === "sign_in_required" && !signInNoticeExternal && <SignInRecovery key={state.id} onCheck={snapshot.checkSignIn} />}
      {startup !== "sign_in_required" && (startup === "failed" || startup === "retrying" || startup === "unavailable" || snapshot.retryError || snapshot.checkingStatus) && <div className="setup-error" role="status">
        <strong>{startup === "retrying" ? "Retrying setup team" : startup === "failed" ? startupDetail?.title || "Setup team could not start"
          : startup === "unavailable" ? "Setup team is unavailable" : "Check setup team status"}</strong>
        {startup === "failed" && startupDetail && <p>{startupDetail.detail}</p>}
        <p>{startup === "unavailable" ? "Check its current status. This does not send or resend an answer."
          : "Your conversation is retained. Starting the setup team does not resend your answer."}</p>
        {snapshot.retryError && <p>{snapshot.retryError}</p>}
        {(startup === "failed" || startup === "retrying") && <button type="button"
          disabled={startup === "retrying" || snapshot.retryPending || snapshot.checkingStatus || !snapshot.retrySetupTeam}
          onClick={() => snapshot.retrySetupTeam?.()}>Retry setup team</button>}
        <button type="button" disabled={snapshot.checkingStatus || !snapshot.checkSetupTeamStatus}
          onClick={() => snapshot.checkSetupTeamStatus?.()}>
          {snapshot.checkingStatus ? "Checking setup team status..." : "Check setup team status"}
        </button>
      </div>}
      {startup === "starting" && <div className="setup-stream-status" role="status">
        <strong>{startupDetail?.title || "Setup team is starting"}</strong>
        <p>{startupDetail?.detail || "Waiting for live updates."}</p>
      </div>}
      {startup === "reconnecting" && <p className="setup-stream-status" role="status">Live updates interrupted. Reconnecting...</p>}
      {state.operation?.status === "running" && !startup && !activityStatusExternal && <div className="setup-native-phase" role="status">
        <span className="setup-native-phase-dot" />
        <span>{conversation.phase === "tool-executing" ? "Using tools"
          : conversation.phase === "generating" ? "Streaming guide activity" : "Working through your agent design"}</span>
      </div>}
    </div>
  );
}
