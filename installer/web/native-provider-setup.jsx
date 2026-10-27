import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";
import { startingContextDetails } from "./forge-starting-context.mjs";
import "./native-provider-setup.css";

const terminal = value => value?.joined === true && ["selected", "cancelled", "failed"].includes(value.status);
const actions = { use: "Use this model", enter_key: "Enter API key", sign_in: "Sign in", back: "Back to models" };
const text = value => typeof value === "string" ? value : "";
function safeLink(value) {
  try { const url = new URL(value); return url.protocol === "https:" && !url.username && !url.password ? url.href : null; }
  catch { return null; }
}
function Fields({ value, label = "Native details" }) {
  return <details className="native-provider-details"><summary>{label}</summary><pre>{JSON.stringify(value, null, 2)}</pre></details>;
}
function Link({ value, children }) {
  const href = safeLink(value);
  return href ? <a href={href} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer">{children}</a>
    : <p role="alert">The native sign-in link cannot be opened here.</p>;
}

export function NativeProviderPrompt({ prompt, disabled, onAnswer }) {
  const [choice, setChoice] = useState("");
  const [secret, setSecret] = useState("");
  const send = answer => { setSecret(""); onAnswer(answer); };
  const selection = prompt.selection;
  return <div className="native-provider-prompt">
    {prompt.kind === "choices" ? <form onSubmit={event => {
      event.preventDefault(); if (!disabled && choice) send({ kind: "choice", choice_id: choice });
    }}>
      <label htmlFor="native-provider-choice">Model and sign-in method</label>
      <select id="native-provider-choice" value={choice} disabled={disabled} onChange={event => setChoice(event.target.value)}>
        <option value="">Choose a model</option>
        {(prompt.choices || []).map(row => <option key={row.choice_id} value={row.choice_id} disabled={row.selectable !== true}>
          {[row.provider, row.model, row.backend, row.auth].filter(Boolean).join(" / ")}{row.selectable !== true ? " (unavailable)" : ""}
        </option>)}
      </select>
      {choice && <Fields value={prompt.choices.find(row => row.choice_id === choice)} label="Selected model details" />}
      <button className="setup-provider-primary" disabled={disabled || !choice} type="submit">Continue</button>
    </form> : prompt.kind === "target_review" ? <>
      <h3>Review model setup</h3>
      <p>{selection?.provider} / {selection?.model} / {selection?.auth}</p>
      <p>Confirm the native configuration below before it is saved.</p>
      <pre className="native-provider-review">{JSON.stringify(prompt, null, 2)}</pre>
      <button type="button" className="setup-provider-primary" disabled={disabled} onClick={() => send({ kind: "confirm" })}>Confirm model setup</button>
    </> : prompt.kind === "action" ? <>
      <h3>{selection?.model || "Model access"}</h3>
      <p>{selection?.provider} / {selection?.auth}</p>
      <p>Credential status: <strong>{text(prompt.credential)}</strong></p>
      <div className="setup-provider-actions">{(prompt.actions || []).map(action => actions[action] &&
        <button key={action} type="button" disabled={disabled} onClick={() => send({ kind: "action", action })}>{actions[action]}</button>)}</div>
      <Fields value={prompt} />
    </> : prompt.kind === "key" || prompt.kind === "browser_manual" ? <>
      {prompt.kind === "browser_manual" && <>
        <h3>Sign in with your provider</h3>
        <Link value={prompt.authorization_url}>Open provider sign-in (new tab)</Link>
        <p>After signing in, paste the complete return URL below. The native provider checks this exact attempt.</p>
        <p>Expected return address: <code>{text(prompt.redirect)}</code></p>
      </>}
      <form onSubmit={event => {
        event.preventDefault(); if (disabled || !secret.trim()) return;
        send(prompt.kind === "key" ? { kind: "key", key: secret } : { kind: "callback", url: secret });
      }}>
        <label htmlFor="native-provider-secret">{prompt.kind === "key" ? "API key" : "Complete return URL"}</label>
        <input id="native-provider-secret" type="password" autoComplete="off" autoCorrect="off" spellCheck={false}
          maxLength={prompt.kind === "key" ? 8192 : 16384} value={secret} disabled={disabled}
          onChange={event => setSecret(event.target.value)} />
        <p>Sent privately to this native setup interaction. It is not sent to your conversation.</p>
        <button type="submit" className="setup-provider-primary" disabled={disabled || !secret.trim()}>
          {prompt.kind === "key" ? "Save API key" : "Submit return URL"}</button>
      </form>
    </> : prompt.kind === "browser" ? <>
      <h3>Sign in with your provider</h3>
      <Link value={prompt.authorization_url}>Open provider sign-in (new tab)</Link>
      <p>Waiting for the native callback at <code>{text(prompt.redirect)}</code>.</p>
    </> : prompt.kind === "device" ? <>
      <h3>Sign in with this code</h3>
      <code className="native-provider-code">{text(prompt.user_code)}</code>
      <Link value={prompt.verification_uri}>Open device sign-in (new tab)</Link>
      <p>Enter this code on the provider's page. This view will update when the native sign-in finishes.</p>
    </> : <>
      <p role="alert">This native prompt is not supported by this Forge version. Cancel setup to leave it safely.</p>
      <Fields value={prompt} label="Unrecognized native prompt" />
    </>}
  </div>;
}

export function NativeProviderSetup({ onClose, firstRun = false, idea = "", startingContext = null }) {
  const [value, setValue] = useState(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [unknown, setUnknown] = useState(false);
  const owner = useRef(null);
  const reading = useRef(null);
  const pending = useRef(false);
  const epoch = useRef(0);
  const uncertain = useRef(null);
  const latest = useRef(null);
  const heading = useRef(null);
  latest.current = value;

  function observe(next) {
    if (uncertain.current && (terminal(next) ||
        next.prompt_id && next.prompt_id !== uncertain.current.prompt_id)) {
      uncertain.current = null; setUnknown(false);
    }
    setValue(next); setError("");
  }
  async function check() {
    const mount = owner.current, ticket = epoch.current;
    if (!mount || reading.current?.mount === mount || pending.current) return;
    const reader = { mount, ticket };
    reading.current = reader;
    try {
      const next = await request("/api/provider-setup", { timeoutMs: 15000 });
      if (owner.current === mount && epoch.current === ticket) observe(next);
    } catch (failure) {
      if (owner.current === mount && epoch.current === ticket) {
        setError(failure.status === 401 || failure.status === 403
          ? "This sign-in session is no longer available. Sign in again, then check setup."
          : "Native setup could not be read. Check setup before continuing.");
        // Hide private prompts, codes and notices after any failed read. A
        // transport failure is never an observed terminal result.
        setValue(null);
      }
    } finally { if (reading.current === reader) reading.current = null; }
  }
  useEffect(() => {
    const mount = {}; owner.current = mount;
    check();
    if (firstRun) heading.current?.focus({ preventScroll: true });
    const timer = setInterval(() => {
      if (latest.current && !terminal(latest.current) && latest.current.status !== "not_started") check();
    }, 1000);
    return () => { if (owner.current === mount) { owner.current = null; epoch.current++; } clearInterval(timer); };
  }, []);

  async function command(action, answer) {
    const mount = owner.current;
    if (!mount || pending.current || !value || unknown && action !== "cancel") return;
    const promptId = value.prompt_id;
    pending.current = true; epoch.current++; setBusy(true); setError("");
    try {
      const next = await request(`/api/provider-setup/${action}`, { method: "POST", timeoutMs: 30000,
        body: action === "answer" ? { prompt_id: promptId, answer } : {} });
      if (owner.current === mount) observe(next);
    } catch (failure) {
      if (owner.current === mount) {
        uncertain.current = { prompt_id: promptId }; setUnknown(true);
        setValue(null);
        setError(failure.status === 401 || failure.status === 403
          ? "This sign-in session is no longer available. Sign in again, then check setup."
          : "The result was not confirmed. Check setup to observe the original request; it will not be sent again.");
      }
    } finally {
      pending.current = false;
      if (owner.current === mount) setBusy(false);
    }
  }
  const context = startingContextDetails(startingContext);
  const active = value && !terminal(value) && value.status !== "not_started";
  return <section className="setup-provider native-provider" aria-label="Setup guide provider">
    <div className="setup-provider-heading"><h2 ref={heading} tabIndex={-1}>Choose your model</h2>
      <button type="button" onClick={onClose}>Back to conversation</button></div>
    <p>Choose from the models and sign-in methods offered by this native host. Your draft stays unsent.</p>
    {firstRun && idea && <div className="setup-provider-idea"><span>Your idea - not sent</span><p>{idea}</p></div>}
    {firstRun && context && <div className="setup-provider-starting-context"><span>Starting point - not sent</span>
      <p>{context.audience.label} / {context.kind.label}</p></div>}
    {value?.status === "not_started" && <button type="button" className="setup-provider-primary" disabled={busy || unknown}
      onClick={() => command("begin")}>Choose model and sign-in</button>}
    {value?.status === "prompt" && value.prompt && <NativeProviderPrompt key={value.prompt_id}
      prompt={value.prompt} disabled={busy || unknown} onAnswer={answer => command("answer", answer)} />}
    {value && !["not_started", "prompt", "selected", "cancelled", "failed"].includes(value.status) && <p role="status">{
      ["joining", "closing"].includes(value.status) ? "Waiting for native setup to finish and close..." : "Waiting for native setup..."}</p>}
    {value?.notices?.map((notice, index) => <div key={index} role="status"><p>Native notice: {text(notice?.kind)}</p>
      <Fields value={notice} label="Native setup notice" /></div>)}
    {value?.status === "selected" && <>
      <h3>Model selected</h3><p>{value.selection?.provider} / {value.selection?.model}</p>
      <p>Credential status: {value.selection?.credential}. Live model access has not been checked.</p>
      <p>Continue returns to your draft. The host must prepare the guide with this selection before it can use the model.</p>
      <button type="button" className="setup-provider-primary" disabled={!value.joined || busy || unknown} onClick={onClose}>Continue</button>
    </>}
    {value?.status === "cancelled" && <p role="status">Native setup cancelled{value.joined ? " and closed" : ""}. Saved native changes may remain; status must be reobserved before another setup.</p>}
    {value?.status === "failed" && <p role="alert">Native setup failed. {value.terminal?.reason || "The native result could not be confirmed."}</p>}
    {value?.status === "failed" && !value.joined && <p role="status">Native cleanup is not confirmed yet. Checking the original setup...</p>}
    {value?.terminal && <Fields value={value.terminal} label="Native completion details" />}
    {unknown && <p role="status">The previous request is still unconfirmed. Its answer cannot be submitted again.</p>}
    {error && <p role="alert">{error}</p>}
    {!value && !error && <p role="status">Reading native setup...</p>}
    <div className="setup-provider-actions">
      <button type="button" disabled={busy} onClick={check}>Check setup</button>
      {(active || value?.status === "failed" && !value.joined) && <button type="button" disabled={busy}
        onClick={() => command("cancel")}>Cancel setup</button>}
    </div>
  </section>;
}
