import React, { useEffect, useRef, useState } from "react";
import { request, normalizeError } from "./state.mjs";
import { savedToolkitErrorMessage } from "./toolkit-saved-errors.mjs";
import "./toolkit-saved-setup.css";

const MAX_MANIFEST_BYTES = 64 * 1024;
const pretty = value => JSON.stringify(value, null, 2);
const fieldLabel = name => name.replaceAll("_", " ");

export function savedToolsGuideMessage(selection, review) {
  return `Help me work out how this saved ${review.profile} connection fits the agent design we are discussing. ` +
    `Read the saved_toolkit_review evidence in inspect_blueprint for this setup. ` +
    `The selected historical request is ${selection.request_sha256}. ` +
    "Which agent needs this connection, and what distinct expertise or private context should it have? " +
    "These are proposed settings for discussion. Historical operations are not current selections. " +
    "Do not install, authenticate, activate tools or infer new grants from this review.";
}

/** A host-selected historical request stays separate from the chat draft. */
export function ToolkitSavedSetup({ guideId, version, disabled, onDiscuss, onClose }) {
  const [state, setState] = useState(null), [preview, setPreview] = useState(null);
  const [approved, setApproved] = useState(false), [pending, setPending] = useState(null);
  const [error, setError] = useState("");
  const picker = useRef(null), generation = useRef(0), alive = useRef(true);
  const base = `/api/guide/${guideId}/toolkit-saved`;
  const initialVersion = useRef(version);
  const changed = initialVersion.current !== version;
  const blocked = disabled || changed || pending !== null;

  useEffect(() => {
    alive.current = true;
    const token = ++generation.current;
    request(base).then(value => {
      if (alive.current && token === generation.current) setState(value);
    }).catch(() => {
      if (alive.current && token === generation.current) setError("Saved tools could not be read. Your conversation and draft are kept.");
    });
    return () => { alive.current = false; ++generation.current; };
  }, [base]);

  useEffect(() => {
    if (state?.status !== "preparing") return;
    let stopped = false, timer;
    async function poll() {
      try {
        const value = await request(base);
        if (!stopped) { setState(value); setError(""); }
      } catch {
        if (!stopped) setError("The review is still being prepared or its status could not be read. Reopen Saved tools to check the retained request.");
      }
      if (!stopped) timer = setTimeout(poll, 1000);
    }
    timer = setTimeout(poll, 500);
    return () => { stopped = true; clearTimeout(timer); };
  }, [base, state?.status]);

  async function act(name, operation, accept) {
    if (blocked || state?.status === "preparing") return;
    const token = ++generation.current;
    setPending(name); setError("");
    try {
      const value = await operation();
      if (alive.current && token === generation.current) accept(value);
    } catch (failure) {
      if (alive.current && token === generation.current) setError(savedToolkitErrorMessage(failure));
    } finally {
      if (alive.current && token === generation.current) setPending(null);
    }
  }

  async function upload(file) {
    if (!file || blocked || state?.status === "preparing") return;
    if (file.size < 1 || file.size > MAX_MANIFEST_BYTES) {
      setError("Choose a saved Toolkit recovery JSON file of at most 64 KiB.");
      return;
    }
    await act("upload", async () => {
      const response = await fetch(`${base}/manifest?version=${version}`, {
        method: "POST", headers: { "Content-Type": "application/json" }, body: file,
      });
      const value = await response.json();
      if (!response.ok) throw new Error(normalizeError(value, "This recovery file could not be checked."));
      return value;
    }, value => { setState(value); setPreview(null); setApproved(false); });
  }

  function choose(row) {
    setApproved(false); setPreview(null);
    setState(value => ({ ...value, review: null, selection: null }));
    act("preview", () => request(`${base}/preview`, { method: "POST", body: {
      version, manifest_sha256: state.manifest_sha256, request_id: row.request_id,
    } }), setPreview);
  }

  function review() {
    if (!approved || !preview) return;
    act("review", () => request(`${base}/review`, { method: "POST", body: {
      version, manifest_sha256: preview.manifest_sha256,
      request_id: preview.selection.request_id, request_sha256: preview.selection.request_sha256,
      host_fingerprint: preview.host_fingerprint,
      audience_approved: true,
    } }), setState);
  }

  const current = state?.review;
  const waiting = state?.status === "preparing";
  const host = preview?.host || state?.host;
  return <section className="setup-saved-tools" aria-label="Review saved tools">
    <header><div><span className="setup-eyebrow">CONTINUE FROM YOUR OWN CONTEXT</span>
      <h2>Review saved tools</h2></div>
      <button type="button" aria-label="Close saved tools" onClick={onClose}>Close</button></header>
    <p>Bring an earlier connection into this setup as a proposal. Choose one request from your Toolkit recovery file, then review it against the current tooling.</p>
    {changed && <p role="status">Your setup changed while this panel was open. Close and reopen it before choosing another request.</p>}
    {!state && !error && <p role="status">Reading saved tools...</p>}
    {state?.available === false && <p role="status">{savedToolkitErrorMessage("saved_settings_host_unavailable")}</p>}
    {state?.available && <>
      {host && <p className="setup-saved-tools-host">Review on <strong>{host.target}</strong> with <strong>{host.families?.join(", ")}</strong>.</p>}
      <button type="button" className="setup-action" disabled={blocked || waiting}
        onClick={() => picker.current?.click()}>Choose recovery file</button>
      <input ref={picker} type="file" accept=".json,application/json" aria-label="Toolkit recovery file" hidden
        onChange={event => { const file = event.target.files?.[0]; event.target.value = ""; upload(file); }} />
      {!!state.requests?.length && <div className="setup-saved-tools-requests" role="group" aria-label="Saved connection requests">
        <p>Which connection do you want to review?</p>
        {state.requests.map(row => <button type="button" key={row.request_id}
          disabled={blocked || waiting} aria-pressed={(preview?.selection || state.selection)?.request_id === row.request_id}
          onClick={() => choose(row)}><strong>{row.profile}</strong><span>{row.family}</span>
          <small>Request {row.request_id.slice(0, 8)}</small></button>)}
      </div>}
      {state.manifest_sha256 && !state.requests?.length && <p>No configured connection requests are available in this file.</p>}
      {preview && !waiting && !current && <div className="setup-saved-tools-preview">
        <h3>Check the selected request</h3>
        <p>The full saved request below will be copied into a fresh local review. Inspect its contents before sharing them with this setup.</p>
        <pre tabIndex={0} aria-label="Full saved connection request">{preview.request_text}</pre>
        <label className="setup-saved-tools-approval"><input type="checkbox" checked={approved} disabled={blocked}
          onChange={event => setApproved(event.target.checked)} />
          <span>I have checked the full request. It contains no secrets and is suitable for this setup's audience.</span></label>
        <button type="button" className="setup-action" disabled={blocked || !approved} onClick={review}>Review current settings</button>
      </div>}
      {(waiting || pending) && <p role="status">{waiting || pending === "review"
        ? "Preparing current tool help and proposed settings. Keep Forge running; your conversation and draft are kept."
        : pending === "preview" ? "Checking the selected request..." : "Checking your recovery file..."}</p>}
      {current && <div className="setup-saved-tools-current">
        <h3>{current.status === "review_required" ? `${current.profile}: current settings to review` : "This saved connection needs attention"}</h3>
        {current.status === "profile_unavailable" && <p>{savedToolkitErrorMessage("saved_settings_profile_unavailable")}</p>}
        {current.values && <dl>{Object.entries(current.values).map(([key, value]) => <div key={key}>
          <dt>{fieldLabel(key)}</dt><dd>{typeof value === "string" ? value : pretty(value)}</dd>
        </div>)}</dl>}
        {!!current.unapplied?.length && <div role="status"><strong>These saved settings need a new decision</strong>
          <pre>{pretty(current.unapplied)}</pre></div>}
        <div className="setup-saved-tools-operations"><strong>No current operations selected</strong>
          <p>Previously requested: {current.prior_operations?.join(", ") || "none reported"}. Choose current operations again before any later installation.</p></div>
        {current.help && <details><summary>Current tool documentation</summary><pre>{pretty(current.help)}</pre></details>}
        <p>This review has not installed a connector, signed into an account, or granted tools to an agent.</p>
        {current.status === "review_required" && onDiscuss && <button type="button" className="setup-action" disabled={blocked || waiting}
          onClick={() => { onDiscuss(savedToolsGuideMessage(state.selection, current)); onClose(); }}>Discuss how this fits my mob</button>}
      </div>}
    </>}
    {state?.error && <p role="alert">{savedToolkitErrorMessage(state.error)}</p>}
    {error && <p role="alert">{error}</p>}
  </section>;
}
