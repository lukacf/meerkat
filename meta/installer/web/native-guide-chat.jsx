import React, { useEffect, useMemo, useRef, useState } from "react";
import { ConversationTranscript } from "../../packages/console-components/src/conversation/conversation-transcript";
import { Icon as ConsoleIcon, SpriteSheet } from "../../console/src/icon";
import { createGuideFrameLog, ingestGuideTimeline, guideConversation, nextGuideTimelineCursor } from "./native-guide-chat.mjs";
import { request } from "./state.mjs";
import { createGuideStartupRecovery, guideStartupPresentation, guideStartupDetail } from "./guide-startup.mjs";

export function useGuideTimeline(state) {
  const [snapshot, setSnapshot] = useState({ guideId: null, frames: [], bindings: [], error: "", available: false });
  const latestState = useRef(state);
  const wakePoll = useRef(null);
  const retryStartup = useRef(null);
  const checkStartup = useRef(null);
  latestState.current = state;
  useEffect(() => {
    if (!state?.id) return;
    let stopped = false, timer, after, inFlight = false;
    const currentLog = createGuideFrameLog();
    const seenBindings = new Set();
    setSnapshot({ guideId: state.id, frames: [], bindings: [], error: "", available: false });
    const recovery = createGuideStartupRecovery({ guideId: state.id,
      currentGuideId: () => latestState.current?.id,
      request, onUpdate: (patch) => setSnapshot((previous) => previous.guideId === state.id
        ? { ...previous, ...patch } : previous),
      onRecovered: () => wake(),
    });
    async function poll() {
      if (stopped || inFlight || recovery.isRetrying()) return;
      const generation = recovery.generation();
      inFlight = true;
      let delay = latestState.current?.operation?.status === "running" ? 1000 : 4000;
      try {
        const query = new URLSearchParams({ limit: "500" });
        if (after) query.set("after", after);
        const page = await request(`/api/guide/${encodeURIComponent(state.id)}/timeline?${query}`, { timeoutMs: 30000 });
        if (stopped || !recovery.current(generation)) return;
        recovery.timelineRecovered();
        const frames = ingestGuideTimeline(currentLog, page);
        setSnapshot({ guideId: state.id, frames: [...frames], bindings: page.bindings || [], available: page.available !== false, error: "", startup: null, retrying: false, retryError: "" });
        const next = nextGuideTimelineCursor(after, page, seenBindings);
        const hasMore = next.replay || page.exhausted === false && next.after && next.after !== after;
        after = next.after;
        if (hasMore) delay = 0;
      } catch (failure) {
        if (stopped || !recovery.current(generation)) return;
        setSnapshot((previous) => ({ ...previous, error: failure.message }));
        await recovery.observeFailure(generation);
        delay = 2500;
      } finally {
        inFlight = false;
        if (!stopped) timer = setTimeout(poll, recovery.generation() === generation ? delay : 0);
      }
    }
    const wake = () => { clearTimeout(timer); poll(); };
    wakePoll.current = wake;
    const retry = () => recovery.retry();
    retryStartup.current = retry;
    const check = () => recovery.checkStatus();
    checkStartup.current = check;
    poll();
    return () => {
      stopped = true;
      recovery.dispose();
      clearTimeout(timer);
      if (wakePoll.current === wake) wakePoll.current = null;
      if (retryStartup.current === retry) retryStartup.current = null;
      if (checkStartup.current === check) checkStartup.current = null;
    };
  }, [state?.id]);
  useEffect(() => {
    if (state?.operation?.status === "running") wakePoll.current?.();
  }, [state?.id, state?.operation?.request_id, state?.operation?.status]);
  const retrySetupTeam = retryStartup.current;
  const checkSetupTeamStatus = checkStartup.current;
  return snapshot.guideId === state?.id ? { ...snapshot, retrySetupTeam, checkSetupTeamStatus }
    : { frames: [], bindings: [], error: "", available: false };
}

export function NativeGuideChat({ state, snapshot, pendingMessage, Icon, onActivity }) {
  const conversation = useMemo(() => guideConversation({ state,
    frames: snapshot.frames, bindings: snapshot.bindings, pendingMessage,
  }), [state, snapshot.frames, snapshot.bindings, pendingMessage]);
  useEffect(() => onActivity?.(conversation.entries), [conversation.entries, onActivity]);
  const startup = guideStartupPresentation(state, snapshot);
  const startupDetail = snapshot.guideId === state.id ? guideStartupDetail(snapshot.startup) : null;
  return (
    <div className="setup-native-chat" data-native-console-chat="true">
      <SpriteSheet />
      <ConversationTranscript viewState={conversation.viewState} Icon={ConsoleIcon} showTurnDiff={false} />
      {(startup === "failed" || startup === "retrying" || startup === "unavailable" || snapshot.retryError || snapshot.checkingStatus) && <div className="setup-error" role="status">
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
      {state.operation?.status === "running" && !startup && <div className="setup-native-phase" role="status">
        <span className="setup-native-phase-dot" />
        <span>{conversation.phase === "tool-executing" ? "Using tools"
          : conversation.phase === "generating" ? "Streaming guide activity" : "Working through your agent design"}</span>
      </div>}
    </div>
  );
}
