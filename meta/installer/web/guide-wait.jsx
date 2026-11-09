import React, { useEffect, useMemo, useState } from "react";
import { guideWaitBinding, guideWaitElapsed } from "./guide-wait.mjs";
import { guideStartupPresentation } from "./guide-startup.mjs";
import { guideCurrentToolActivity } from "./native-guide-chat.mjs";

export function GuideWaitStatus({ state, snapshot, submitting = false, following = true, onFollow, firstIdea = false }) {
  const binding = guideWaitBinding(state);
  const key = binding?.key || null;
  const activity = useMemo(() => snapshot?.guideId === state?.id && snapshot.available === true
    ? guideCurrentToolActivity({ state, frames: snapshot.frames, bindings: snapshot.bindings }) : null,
  [state, snapshot?.guideId, snapshot?.available, snapshot?.frames, snapshot?.bindings]);
  const [clock, setClock] = useState(() => ({ key, now: Date.now() }));
  useEffect(() => {
    setClock({ key, now: Date.now() });
    if (!key) return;
    const timer = setInterval(() => setClock({ key, now: Date.now() }), 1000);
    return () => clearInterval(timer);
  }, [key]);
  if (!binding) return submitting ? <div className="setup-guide-wait" aria-label="Guide request status">
    <div className="setup-guide-wait-heading">
      <span className="setup-guide-sending-mark" aria-hidden="true" />
      <p role="status">Sending to your guide</p>
    </div>
    <small>Waiting for confirmation. You can keep writing below.</small>
  </div> : null;
  const elapsed = guideWaitElapsed(binding.startedAt, clock.key === key ? clock.now : Date.now());
  const startup = guideStartupPresentation(state, snapshot);
  const label = startup === "failed" ? "Setup team startup needs a retry"
    : startup === "retrying" ? "Retrying setup team startup"
    : startup === "starting" ? "Setup team is starting"
    : startup === "unavailable" ? "Setup team is unavailable"
    : startup === "reconnecting" ? "Live updates interrupted. Reconnecting."
    : firstIdea ? "Saved - your guide is working" : "Guide request running";
  return <div className="setup-guide-wait" aria-label="Guide request status"
    data-guide-id={binding.guideId} data-request-id={binding.requestId}>
    <div className="setup-guide-wait-heading">
      {!startup && <span className="setup-native-phase-dot" aria-hidden="true" />}
      <p role="status">{label}</p>
      {elapsed !== null && <span className="setup-guide-wait-clock" aria-live="off">{elapsed} elapsed</span>}
    </div>
    {activity && <small>{startup ? "Last observed tool" : activity.status === "pending" ? "Using tool" : "Last tool"}: {activity.name.replaceAll("_", " ")}
      {activity.status === "success" ? " - completed" : activity.status === "error" ? " - failed" : ""}</small>}
    {(!firstIdea || startup) && <small>Your submitted answer is saved. You can keep writing below.</small>}
    {!following && !startup && snapshot?.guideId === state.id && snapshot.available === true && typeof onFollow === "function" &&
      <button type="button" className="setup-guide-follow" onClick={onFollow}>
        Follow live activity
      </button>}
  </div>;
}
