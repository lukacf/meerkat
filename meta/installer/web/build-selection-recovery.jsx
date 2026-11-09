import React, { useEffect, useRef, useState } from "react";
import { captureGuideSelection, guideSelectionAvailable } from "./guide-selection.mjs";
import { request } from "./state.mjs";

const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const digest = /^[a-f0-9]{64}$/;
export function savedBuildChoices(value, state) {
  if (value?.guide_id !== state?.id || value?.version !== state?.version || !Array.isArray(value.candidates)
      || !Number.isSafeInteger(value.unavailable_count) || value.unavailable_count < 0
      || value.candidates.some(row => !uuid.test(row?.build_id) || !digest.test(row?.selection_token)
        || !Number.isSafeInteger(row?.guide_version) || row.guide_version < 1)
      || new Set(value.candidates.map(row => row.build_id)).size !== value.candidates.length)
    throw new Error("The saved build list changed. Refresh build status before choosing a build.");
  return value;
}

export function BuildSelectionRecovery({ state, selectionScope, blocked = false, onRefresh }) {
  const token = selectionScope?.token();
  const unavailable = blocked || !guideSelectionAvailable(selectionScope, state, token);
  const key = JSON.stringify([state?.id, state?.version, token, unavailable]);
  const selection = useRef(null), operation = useRef(null), refresh = useRef(onRefresh);
  refresh.current = onRefresh;
  if (selection.current?.key !== key) selection.current = { key };
  const owner = selection.current;
  const [snapshot, setSnapshot] = useState(null);
  const url = `/api/guide/${encodeURIComponent(state.id)}/implementation/saved-selection`;

  async function inspect() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, token);
    if (unavailable || !ownerCurrent || operation.current?.owner === owner) return;
    const work = { owner, controller: new AbortController() };
    operation.current = work;
    const current = () => selection.current === owner && operation.current === work && ownerCurrent();
    setSnapshot({ owner, phase: "loading" });
    try {
      const value = savedBuildChoices(await request(url, { timeoutMs: 30000, signal: work.controller.signal }), state);
      if (current()) setSnapshot({ owner, phase: "ready", value });
    } catch (error) {
      if (current()) setSnapshot({ owner, phase: "error", message: error.message || "Saved work could not be checked." });
    } finally { if (operation.current === work) operation.current = null; }
  }

  useEffect(() => {
    if (!unavailable) inspect();
    return () => {
      // Retire presentation and cancel only reads. Selection writes have their
      // own server-side compare-absent fence and must never be retried here.
      if (operation.current?.owner === owner) {
        operation.current.controller?.abort();
        operation.current = null;
      }
    };
  }, [owner]);

  async function restore(candidate) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, token);
    if (unavailable || !ownerCurrent || selection.current !== owner || operation.current?.owner === owner
        || snapshot?.owner !== owner || snapshot.phase !== "ready"
        || !snapshot.value.candidates.includes(candidate)) return;
    const work = { owner };
    operation.current = work;
    const current = () => selection.current === owner && operation.current === work && ownerCurrent();
    setSnapshot({ owner, phase: "restoring", value: snapshot.value, buildId: candidate.build_id });
    try {
      const result = await request(url, { method: "POST", timeoutMs: 30000, body: {
        version: state.version, build_id: candidate.build_id, selection_token: candidate.selection_token,
      } });
      if (!current()) return;
      if (result?.guide_id !== state.id || result?.version !== state.version
          || result?.build_id !== candidate.build_id || result?.selection_restored !== true)
        throw new Error("The restored selection could not be confirmed. Refresh build status to check it.");
      setSnapshot({ owner, phase: "done" });
      refresh.current?.();
    } catch (error) {
      if (current()) {
        setSnapshot({ owner, phase: "uncertain", message: error.code === "read_timeout"
          ? "The saved-build selection response is taking too long. It may still finish. Check saved work before choosing again."
          : error.message || "The selection could not be confirmed." });
        refresh.current?.();
      }
    } finally { if (operation.current === work) operation.current = null; }
  }

  const view = unavailable ? { phase: "paused" }
    : snapshot?.owner === owner ? snapshot : { phase: "loading" };
  const waiting = view.phase === "loading" || view.phase === "restoring";
  return <section className="implementation-selection-recovery" aria-labelledby={`saved-selection-${state.id}`}>
    <h3 id={`saved-selection-${state.id}`}>Find your saved build</h3>
    <p>Saved source can survive an interrupted setup. Restore its selection to inspect the work and see what needs attention.</p>
    {view.phase === "paused" && <p role="status">Checking saved work is paused until this setup is available again.</p>}
    {view.phase === "loading" && <p role="status">Checking saved work...</p>}
    {view.phase === "done" && <p role="status">Selection restored. Checking this build...</p>}
    {view.value && <ul>{view.value.candidates.map(candidate => {
      const date = new Date(candidate.created_at);
      const label = typeof candidate.project_name === "string" && candidate.project_name.trim()
        ? candidate.project_name : "Saved application";
      return <li key={candidate.build_id}>
        <div><strong>{label}</strong><p>Proposal {candidate.guide_version}
          {Number.isFinite(date.getTime()) ? ` - ${date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}` : ""}</p>
          {view.value.candidates.length > 1 && <p className="implementation-detail">Build {candidate.build_id.slice(0, 8)}</p>}
          {candidate.cleanup_pending !== false && <p className="implementation-detail">Worker shutdown is unconfirmed. Restarting stays unavailable until cleanup is confirmed.</p>}
        </div>
        <button className="setup-action" type="button" disabled={unavailable || waiting || view.phase !== "ready"}
          aria-label={`Restore saved build for ${label}, proposal ${candidate.guide_version}`} onClick={() => restore(candidate)}>
          {view.phase === "restoring" && view.buildId === candidate.build_id ? "Restoring selection..." : "Restore saved build"}
        </button>
      </li>;
    })}</ul>}
    {view.phase === "ready" && !view.value.candidates.length && <p role="status">No complete saved build could be identified.</p>}
    {view.value?.unavailable_count > 0 && <p className="implementation-detail">Some saved work lacks the information needed to restore a selection. It is preserved.</p>}
    {(view.phase === "error" || view.phase === "uncertain") && <p role="alert">{view.message}</p>}
    {(view.phase === "error" || view.phase === "uncertain") && <button className="implementation-link" type="button"
      disabled={unavailable || waiting} onClick={inspect}>Check saved work again</button>}
    <p className="implementation-detail">Restoring a selection keeps the original source and cleanup state. It does not start a builder.</p>
  </section>;
}
