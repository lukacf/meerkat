import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";
import { savedSetupBuildStatus, unknownSavedBuildStatus } from "./saved-setup-build-status.mjs";

/** Parent admits only visible rows and owns actual setup selection/navigation. */
export function SavedSetupBuildStatus({ setup, active = true, disabled = false, compact = false, isCurrent, onOpenBuild }) {
  const selection = useRef(null), mounted = useRef(false), operation = useRef(null), timer = useRef(null);
  const latest = useRef(null);
  latest.current = { setup, active, disabled, isCurrent, onOpenBuild };
  if (selection.current?.setup !== setup || selection.current.active !== active)
    selection.current = { setup, active };
  const owner = selection.current;
  const [snapshot, setSnapshot] = useState(null);
  const eligible = () => {
    if (selection.current !== owner || !latest.current.active ||
        typeof setup?.id !== "string" || !setup.id) return false;
    try { return typeof latest.current.isCurrent !== "function" || latest.current.isCurrent(setup) === true; }
    catch { return false; }
  };
  const owns = () => mounted.current && eligible();

  async function inspect() {
    if (!owns() || operation.current?.owner === owner) return;
    clearTimeout(timer.current);
    const work = { owner, controller: new AbortController() };
    operation.current = work;
    const current = () => owns() && operation.current === work;
    setSnapshot(previous => previous?.owner === owner && previous.phase === "known"
      ? previous : { owner, phase: "loading" });
    let poll = false;
    try {
      const receipt = await request(`/api/guide/${encodeURIComponent(setup.id)}/implementation`, {
        timeoutMs: 30000, signal: work.controller.signal,
      });
      if (!current()) return;
      const view = savedSetupBuildStatus(setup, receipt);
      setSnapshot({ owner, phase: "known", view });
      poll = view.poll;
    } catch (error) {
      if (!current()) return;
      setSnapshot({ owner, phase: error.status === 404 ? "absent" : "unknown" });
    } finally {
      if (operation.current === work) operation.current = null;
      if (poll && owns()) timer.current = setTimeout(inspect, 6000);
    }
  }

  useEffect(() => {
    mounted.current = true;
    inspect();
    return () => {
      mounted.current = false;
      clearTimeout(timer.current);
      if (operation.current?.owner === owner) {
        operation.current.controller.abort();
        operation.current = null;
      }
    };
  }, [owner]);

  if (!active || !setup?.id || !eligible()) return null;
  const saved = snapshot?.owner === owner ? snapshot : { phase: "loading" };
  if (saved.phase === "absent") return null;
  const view = saved.phase === "known" ? saved.view : saved.phase === "loading"
    ? { kind: "loading", label: "Checking application build", detail: "", action: null }
    : unknownSavedBuildStatus();
  const canOpen = !disabled && owns() && typeof onOpenBuild === "function" && !!view.action;
  const actionLabel = compact ? view.action === "Review completed build" ? "Review build" : "Open build" : view.action;
  const setupName = setup.name || "Untitled setup";
  return <div className="saved-setup-build-status" data-build-status={view.kind}>
    <p role="status"><strong>{view.label}</strong>{view.timing && <small> - {view.timing}</small>}</p>
    {view.detail && <p>{view.detail}</p>}
    {view.action && <button type="button" disabled={!canOpen} aria-label={`${view.action} for ${setupName}`} onClick={event => {
      event.stopPropagation();
      if (!owns() || latest.current.disabled || snapshot?.owner !== owner || snapshot.phase !== "known" ||
          typeof latest.current.onOpenBuild !== "function" || !snapshot.view.action) return;
      latest.current.onOpenBuild({ guideId: setup.id, buildId: snapshot.view.buildId,
        guideVersion: snapshot.view.guideVersion, currentGuideVersion: snapshot.view.currentGuideVersion });
    }}>{actionLabel}</button>}
    {view.kind === "unknown" && <button type="button" disabled={disabled || !owns()}
      aria-label={`Check build status for ${setupName}`} onClick={event => {
      event.stopPropagation();
      if (!latest.current.disabled) inspect();
    }}>{compact ? "Check status" : "Check build status"}</button>}
  </div>;
}
