import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";
import { startingContextDetails } from "./forge-starting-context.mjs";
import { guideStartupObservation, guideStartupDetail } from "./guide-startup.mjs";
import "./provider-setup.css";

const LABELS = { openai: "OpenAI", anthropic: "Anthropic", gemini: "Google Gemini" };
const MODELS = { openai: "gpt-6.1-sol", anthropic: "claude-sonnet-4-6", gemini: "gemini-3.1-pro-preview" };
const KEY_HELP = {
  openai: "https://platform.openai.com/api-keys",
  anthropic: "https://platform.claude.com/docs/en/get-api-key",
  gemini: "https://ai.google.dev/gemini-api/docs/api-key",
};

function hostManagedProvider(item) {
  return item?.source === "environment" || item?.source === "native_host_config";
}

export function ProviderSetup({ onRuntime, onClose, firstRun = false, idea = "", startingContext = null, guideReady = false }) {
  const [settings, setSettings] = useState(null);
  const [provider, setProvider] = useState("openai");
  const [apiKey, setApiKey] = useState("");
  const [pending, setPending] = useState(null);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [saveUnknown, setSaveUnknown] = useState(false);
  const [runtimeUnknown, setRuntimeUnknown] = useState(false);
  const [startCheck, setStartCheck] = useState(null);
  const [startupObservation, setStartupObservation] = useState(null);
  const mounted = useRef(null);
  const admitting = useRef(null);
  const startObservation = useRef(null);
  const startReading = useRef(null);
  const settingsRead = useRef(0);
  const heading = useRef(null);
  const actionFocus = useRef(null);

  useEffect(() => {
    if (firstRun) heading.current?.focus({ preventScroll: true });
  }, [firstRun]);

  async function refresh(ownerCurrent = () => true, draftProvider = null) {
    const mount = mounted.current;
    if (!mount || !ownerCurrent()) return;
    const ticket = ++settingsRead.current;
    const current = () => mounted.current === mount && ticket === settingsRead.current && ownerCurrent();
    try {
      const next = await request("/api/provider-settings", { timeoutMs: 30000 });
      if (!current()) return;
      setSettings(next);
      if (draftProvider && (
        (next.locked_provider && next.locked_provider !== draftProvider)
        || hostManagedProvider(next.providers?.find(item => item.provider === draftProvider))
      )) setApiKey("");
      setProvider(provider => next.locked_provider || draftProvider || next.effective_provider || provider);
      return next;
    } catch (failure) {
      if (current()) throw failure;
    }
  }
  useEffect(() => {
    const mount = {};
    mounted.current = mount;
    const reading = refresh(), ticket = settingsRead.current;
    reading.catch(() => {
      if (mounted.current === mount && ticket === settingsRead.current)
        setError("Provider settings could not be read. Check again before saving.");
    });
    return () => {
      if (mounted.current !== mount) return;
      mounted.current = null;
      settingsRead.current += 1;
      admitting.current = null;
      startObservation.current = null;
      startReading.current = null;
      clearActionFocus();
    };
  }, []);
  useEffect(() => {
    const owner = admitting.current;
    if (pending !== "start" || owner?.action !== "start") return;
    let reading = false, stopped = false;
    const timer = setInterval(async () => {
      if (reading || stopped || owner.observed) return;
      reading = true;
      try {
        const runtime = await request("/api/architect/status", { timeoutMs: 30000 });
        if (!stopped && presents(owner)) {
          setStartupObservation(guideStartupObservation(runtime));
          onRuntime?.(runtime);
        }
      } catch {} finally { reading = false; }
    }, 4000);
    return () => { stopped = true; clearInterval(timer); };
  }, [pending, onRuntime]);

  function begin(action) {
    if (!mounted.current || admitting.current || startReading.current) return null;
    clearActionFocus();
    const owner = { action, mount: mounted.current };
    admitting.current = owner;
    startObservation.current = action === "start" ? owner : null;
    setStartCheck(null);
    setStartupObservation(null);
    settingsRead.current += 1;
    setPending(action); setError(""); setMessage("");
    return owner;
  }
  function owns(owner) {
    return mounted.current === owner.mount && admitting.current === owner;
  }
  function presents(owner) {
    return owns(owner) && !owner.observed;
  }
  function finish(owner) {
    if (admitting.current !== owner) return;
    admitting.current = null;
    if (mounted.current === owner.mount) setPending(null);
  }

  function clearActionFocus() {
    const transition = actionFocus.current;
    actionFocus.current = null;
    transition?.detach();
  }
  function actionIsHidden(source) {
    if (!source.isConnected) return true;
    for (let parent = source; parent; parent = parent.parentElement)
      if (parent.hidden || parent.tagName === "DETAILS" && !parent.open) return true;
    return false;
  }
  function retainActionFocus(owner, source) {
    if (!firstRun || !source?.isConnected || source.ownerDocument.activeElement !== source ||
        !["save", "start", "check"].includes(owner.action)) return;
    const document = source.ownerDocument;
    const transition = { owner, source, step: firstRunStep, committed: false };
    const depart = event => {
      if (event.type === "keydown" && !["Tab", "Escape"].includes(event.key)) return;
      if (event.type === "focusout" && (!event.relatedTarget || event.relatedTarget === document.body)) {
        // Browsers can blur immediately before React removes or hides the action.
        queueMicrotask(() => {
          if (actionFocus.current === transition && !actionIsHidden(source) && !source.disabled)
            clearActionFocus();
        });
        return;
      }
      if (event.type !== "focusout" && event.type !== "keydown" && source.contains(event.target)) return;
      if (actionFocus.current === transition) clearActionFocus();
    };
    const events = ["focusin", "focusout", "pointerdown", "keydown", "input", "change"];
    transition.detach = () => events.forEach(name => document.removeEventListener(name, depart, true));
    events.forEach(name => document.addEventListener(name, depart, true));
    actionFocus.current = transition;
  }

  async function checkSettings() {
    const owner = begin("check");
    if (!owner) return;
    const draftProvider = saveUnknown || runtimeUnknown ? null : provider;
    if (!draftProvider) setApiKey("");
    try {
      const next = await refresh(() => owns(owner), draftProvider);
      if (!next || !owns(owner)) return;
      setSaveUnknown(false);
      const runtime = await request("/api/architect/status", { timeoutMs: 30000 });
      if (owns(owner)) {
        setStartupObservation(guideStartupObservation(runtime));
        onRuntime?.(runtime); setRuntimeUnknown(false);
      }
    } catch {
      if (owns(owner)) setError("Current settings or team status could not be read. Try checking again.");
    } finally { finish(owner); }
  }

  async function checkGuideStatus(actionSource) {
    const source = startObservation.current, mount = mounted.current;
    if (!source || !mount || startReading.current) return;
    const reader = { source, mount, action: "check" };
    clearActionFocus();
    retainActionFocus(reader, actionSource);
    startReading.current = reader;
    source.observed = true;
    settingsRead.current += 1;
    setStartCheck("checking"); setStartupObservation(null); setError(""); setMessage("");
    const current = () => mounted.current === mount && startReading.current === reader
      && startObservation.current === source;
    try {
      const next = await request("/api/provider-settings", { timeoutMs: 30000 });
      if (!current()) return;
      const runtime = await request("/api/architect/status", { timeoutMs: 30000 });
      if (!current()) return;
      setSettings(next);
      setProvider(provider => next.locked_provider || provider);
      setSaveUnknown(false); setRuntimeUnknown(false);
      setStartupObservation(guideStartupObservation(runtime));
      onRuntime?.(runtime);
      if (!current()) return;
      const ready = next.runtime_active === true && next.runtime_status === "ready" && runtime.status === "ready";
      if (ready && actionFocus.current?.owner === reader) actionFocus.current.committed = true;
      setStartCheck(ready ? "ready" : "not_ready");
      setMessage(ready ? "Your guide is ready. Return to your idea when you're ready to send it."
        : "The guide has not reported ready. Check its status again before continuing.");
    } catch {
      if (current()) {
        setRuntimeUnknown(true); setStartCheck("unknown");
        setError("Guide status could not be confirmed. Check again; the original Start request will not be repeated.");
      }
    } finally {
      if (startReading.current === reader) startReading.current = null;
    }
  }

  async function act(action, source) {
    if (admitting.current || startReading.current || !settings || runtimeUnknown || saveUnknown) return;
    const owner = begin(action);
    if (!owner) return;
    retainActionFocus(owner, source);
    try {
      if (action === "save") {
        const next = await request("/api/provider-settings", { method: "PUT", timeoutMs: 30000,
          body: { provider, api_key: apiKey } });
        if (!owns(owner)) return;
        if (actionFocus.current?.owner === owner) actionFocus.current.committed = true;
        setSettings(next); setApiKey("");
        setSaveUnknown(false);
        setMessage("Saved privately on the computer running Forge. Start the guide to use it; saving does not check provider access.");
        try {
          const runtime = await request("/api/architect/status", { timeoutMs: 30000 });
          if (owns(owner)) onRuntime?.(runtime);
        } catch {
          if (owns(owner)) setError("The key was saved. Team status could not be refreshed; check saved settings before starting.");
        }
      } else {
        const runtime = await request(`/api/architect/${action}`, { method: "POST" });
        if (!presents(owner)) return;
        setStartupObservation(guideStartupObservation(runtime));
        onRuntime?.(runtime);
        if (presents(owner)) setMessage(action === "stop"
          ? "Setup team stopped. Saved conversations are retained."
          : runtime.status === "ready"
            ? "Setup team ready. Your next message will use this provider; provider access is checked when it runs."
            : "The setup team has not reported ready. Check its status before retrying.");
        try {
          await refresh(() => presents(owner));
          if (presents(owner) && actionFocus.current?.owner === owner && runtime.status === "ready")
            actionFocus.current.committed = true;
        }
        catch { if (presents(owner)) setError("Team status was received, but local settings could not be refreshed. Check saved settings."); }
      }
    } catch {
      if (presents(owner)) {
        // Never display an arbitrary response that may contain submitted secrets.
        setApiKey("");
        if (action === "save") setSaveUnknown(true);
        else {
          setRuntimeUnknown(true);
          setStartupObservation(previous => previous?.status === "failed" ? previous : null);
        }
        setError(action === "save"
          ? "Saving was not confirmed. Check saved settings before continuing or entering a replacement key."
          : action === "stop" ? "The team could not stop. Finish current work and check its status."
            : "Starting was not confirmed. Check guide status before changing settings or trying again.");
      }
    } finally {
      finish(owner);
    }
  }

  const selected = settings?.providers?.find(item => item.provider === provider);
  const readonly = hostManagedProvider(selected);
  const editable = !!settings && !runtimeUnknown && !settings.provider_binding_error && !settings.runtime_active && !settings.work_active && !readonly;
  const effective = settings?.effective_provider;
  const effectiveModel = effective && typeof settings?.effective_model === "string" && settings.effective_model.trim()
    ? settings.effective_model.trim() : null;
  const displayedModel = provider === effective && effectiveModel ? effectiveModel : MODELS[provider];
  const checkingStart = startCheck === "checking";
  const startupDetail = guideStartupDetail(startupObservation);
  const checkStart = pending === "start" || !!startCheck;
  const needsCheck = saveUnknown || runtimeUnknown || !!error || !!settings?.load_error || !!settings?.provider_binding_error;
  const readyToReturn = !!settings && guideReady === true && settings.runtime_active === true
    && (!startCheck || startCheck === "ready") && (!pending || pending === "start" && startCheck === "ready") && !needsCheck;
  const firstRunStep = readyToReturn ? "ready" : pending === "check" || checkingStart || needsCheck ? "check"
    : pending === "save" ? "saving" : pending === "start" ? "starting" : pending === "stop" ? "stopping"
      : !settings ? "loading" : settings.work_active ? "work" : settings.runtime_active ? "active" : effective ? "start" : "connect";
  const firstRunHeading = startupObservation?.status === "failed" && startupDetail
    ? "Your guide needs attention" : {
    loading: "A model for your guide", connect: "A model for your guide", saving: "Saving your model key",
    start: "Start your setup guide", starting: "Starting your setup guide", stopping: "Stopping your setup guide",
    ready: "Ready for your first message", active: "Check your guide's status", check: "Let's check your setup",
    work: "Existing work needs attention",
  }[firstRunStep];
  useEffect(() => {
    const transition = actionFocus.current;
    if (!transition) return;
    if (!firstRun || mounted.current !== transition.owner.mount || transition.owner.observed ||
        transition.owner.action === "check" && startObservation.current !== transition.owner.source) {
      clearActionFocus(); return;
    }
    if (transition.owner.action === "check" ? checkingStart : pending) return;
    const { source } = transition, document = source.ownerDocument;
    const nextStep = transition.owner.action === "save" ? "start" : "ready";
    const canFocus = transition.committed && firstRunStep === nextStep && firstRunStep !== transition.step && actionIsHidden(source) &&
      (document.activeElement === source || document.activeElement === document.body) && heading.current?.isConnected;
    clearActionFocus();
    if (canFocus) heading.current.focus({ preventScroll: true });
  }, [firstRun, firstRunStep, pending, startCheck]);
  const retainedIdea = typeof idea === "string" && idea.trim() ? idea : "";
  const retainedContext = startingContextDetails(startingContext);
  const hasStartingInput = !!(retainedIdea || retainedContext);
  const returnGuidance = retainedContext
    ? retainedIdea ? "Next: send your idea and starting point. Both are still unsent."
      : "Next: send your starting point, or add your own words. Your choices are still unsent."
    : retainedIdea ? "Next: send your idea. Your draft is still unsent."
      : "Return to Forge to describe what you need. A rough idea is enough.";
  const primaryAction = firstRunStep === "ready" ? "return"
    : checkStart ? "check"
    : firstRunStep === "check" || firstRunStep === "active" || firstRunStep === "stopping" || firstRunStep === "work" ? "check"
      : !effective || apiKey.trim() || firstRunStep === "saving" ? "save" : "start";
  const providerFields = settings && <>
    <div className="setup-provider-picker">
      <div className="setup-provider-field-heading"><label htmlFor="setup-guide-provider">Provider</label>
        <p className="setup-provider-detail">{firstRun
          ? effective ? `${LABELS[effective]} is configured for your guide.` : "Use a model service you already have an API account with, or choose one below."
          : settings.locked_provider
          ? `This saved setup team uses ${LABELS[settings.locked_provider]}.`
          : effective ? `${LABELS[effective]} is currently used by the setup team.`
            : `${LABELS[provider]} uses ${MODELS[provider]} by default.`}</p>
      </div>
      <select id="setup-guide-provider" value={provider} disabled={!!pending || checkingStart || !!settings.locked_provider || settings.runtime_active}
      onChange={event => { setProvider(event.target.value); setApiKey(""); setMessage(""); }}>
      {Object.entries(LABELS).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
      </select>
      {displayedModel && <p className="setup-provider-model"><span>{provider === effective && effectiveModel ? "Current model" : "Default model"}</span><strong>{displayedModel}</strong></p>}
    </div>
    {effective && effective !== provider && <p className="setup-provider-detail">Saving another key does not switch the guide. {LABELS[effective]} remains the active provider.</p>}
    {readonly ? <p className="setup-provider-detail">{selected.source === "native_host_config"
      ? "Your guide uses a model configured by this Forge host. You do not need to enter an API key here."
      : "Configured by the server environment. Its key stays private and cannot be replaced here."}</p>
      : <div className="setup-provider-key">
        <div className="setup-provider-key-heading"><label htmlFor="setup-provider-key">API key</label>
          <a href={KEY_HELP[provider]} target="_blank" rel="noopener noreferrer"
            aria-label={`API key help for ${LABELS[provider]} (opens in a new tab)`}>Get an API key <span aria-hidden="true">↗</span></a>
        </div>
        {firstRun && <p id="setup-provider-prerequisite" className="setup-provider-prerequisite">
          Use a key from your {LABELS[provider]} API account. API usage is billed by your model provider.
        </p>}
        <input id="setup-provider-key" type="password" value={apiKey} disabled={!editable || !!pending || checkingStart}
        autoComplete="off" autoCorrect="off" spellCheck={false} maxLength={4096}
        onChange={event => setApiKey(event.target.value)}
        aria-describedby={firstRun ? "setup-provider-prerequisite setup-provider-private" : "setup-provider-private"} /></div>}
    <small id="setup-provider-private">{selected?.source === "native_host_config"
      ? "Model access is managed by this Forge host. Credentials are never sent to the setup conversation or included in deployment downloads."
      : "Keys are saved in private settings on the computer running Forge. They are never sent to the setup conversation or included in deployment downloads."}</small>
    {firstRun && !effective && <details className="setup-provider-key-help">
      <summary>I do not have a key yet</summary>
      <p>An API key lets Forge use models from your {LABELS[provider]} API account.</p>
      <ol>
        <li>Open Get an API key above and sign in to the provider's API account.</li>
        <li>Create a key there, then paste it above and choose Save locally.</li>
      </ol>
      <p>API usage is billed by your model provider.</p>
    </details>}
  </>;
  const saveControl = !readonly && <button type="button"
    className={(!firstRun || primaryAction === "save") && (!effective || apiKey.trim()) ? "setup-provider-primary" : ""}
    disabled={!editable || !!pending || checkingStart || saveUnknown || !apiKey.trim()}
    onClick={event => act("save", event.currentTarget)}>{pending === "save" ? "Saving..." : "Save locally"}</button>;
  const runtimeControl = settings?.runtime_active ? <button type="button" disabled={!!pending || checkingStart || runtimeUnknown || saveUnknown || settings.work_active}
    onClick={() => act("stop")}>{pending === "stop" ? "Stopping..." : "Stop setup team"}</button>
    : <button type="button" className={(!firstRun || primaryAction === "start") && effective && !apiKey.trim() ? "setup-provider-primary" : ""}
      disabled={!!pending || checkingStart || runtimeUnknown || saveUnknown || !effective || !!apiKey.trim() || settings?.work_active || !!settings?.provider_binding_error}
      onClick={event => act("start", event.currentTarget)}>{pending === "start" ? "Starting guide..." : "Start guide"}</button>;
  const checkControl = <button type="button"
    className={firstRun && primaryAction === "check" ? "setup-provider-primary" : "setup-provider-check"}
    disabled={checkStart ? checkingStart : !!pending}
    onClick={event => checkStart ? checkGuideStatus(event.currentTarget) : checkSettings()}>
    {checkStart ? checkingStart ? "Checking guide status..." : "Check guide status"
      : firstRun && pending === "check" ? "Checking settings..." : "Check saved settings"}</button>;
  return <section className={`setup-provider${firstRun ? " setup-provider-first-run" : ""}`} aria-label="Setup guide provider"
    data-provider-step={firstRun ? firstRunStep : undefined}>
    <div className="setup-provider-heading">{firstRun ? <div>
      <span className="setup-provider-eyebrow">{firstRunStep === "ready" ? "YOUR IDEA IS NEXT"
        : firstRunStep === "work" ? "SETUP CONTROLS PAUSED"
          : firstRunStep === "check" ? "CHECK BEFORE CONTINUING" : firstRunStep === "active" ? "GUIDE STATUS"
          : effective ? "2. START YOUR GUIDE" : "1. CONNECT A MODEL"}</span>
      <h2 ref={heading} tabIndex={-1}>{firstRunHeading}</h2>
    </div> : <h2>Connect a model provider</h2>}
      <button type="button" onClick={onClose} disabled={checkingStart || !!pending && !readyToReturn} aria-label={firstRun ? "Back to my idea" : "Close provider settings"}>{firstRun ? "Back to my idea" : "Close"}</button>
    </div>
    <p className="setup-provider-intro">{firstRun
      ? `Your guide uses an AI model to talk with you and shape your team. Connecting does not send your ${retainedContext ? retainedIdea ? "draft or starting point" : "starting point" : "draft"}.`
      : "Powers the guide, builder and animation agent. App and connector credentials are set up separately."}</p>
    {firstRun && retainedContext && <div className="setup-provider-starting-context" role="group" aria-label="Starting point, not sent">
      <div className="setup-provider-starting-context__heading"><span>Starting point</span><span>Not sent</span></div>
      <div className="setup-provider-starting-context__answers"><span>{retainedContext.audience.label}</span><span>{retainedContext.kind.label}</span></div>
      <small>Your choices stay separate from your writing. You can change or send them after connecting.</small>
    </div>}
    {firstRun && retainedIdea && <div className="setup-provider-idea">
      <div><span>Your idea</span><span>Not sent</span></div>
      <p>{retainedIdea}</p>
      <small>Kept in your message box. You send it when you're ready.</small>
    </div>}
    {firstRun && readyToReturn && <div className="setup-provider-return">
      <p role={hasStartingInput ? "status" : undefined}>{returnGuidance}</p>
      <button type="button" className="setup-provider-primary" aria-describedby="setup-provider-first-message-check"
        onClick={onClose}>{retainedIdea ? "Back to my draft" : retainedContext ? "Back to my starting point" : "Return to Forge"}</button>
      <small id="setup-provider-first-message-check" className="setup-provider-access-note">Provider access is checked when you send your first message.</small>
    </div>}
    {!settings && !error && <p role="status">Reading local settings...</p>}
    {settings && <>
      {firstRun ? <>
        {effective && <div className="setup-provider-configured">
          <span className="setup-provider-configured-mark" aria-hidden="true">{LABELS[effective]?.[0]}</span>
          <div><strong>{LABELS[effective]}</strong><p>{settings.providers?.find(item => item.provider === effective)?.source === "native_host_config"
            ? "Configured by this Forge host"
            : hostManagedProvider(settings.providers?.find(item => item.provider === effective))
              ? "Configured by this server" : "Configured in Forge"}</p></div>
          <span className="setup-provider-configured-label">{effectiveModel || MODELS[effective] ? <>
            <span>{effectiveModel ? "Current model" : "Default model"}</span>
            <strong>{effectiveModel || MODELS[effective]}</strong>
          </> : "Provider configured"}</span>
        </div>}
        {settings.work_active && !readyToReturn && <div className="setup-provider-work" role="status">
          <span className="setup-provider-work-mark" aria-hidden="true" />
          <p>Earlier work or a shutdown that has not been confirmed still needs Forge. Changing models and starting or stopping the guide are paused.</p>
        </div>}
        {!effective && <div className="setup-provider-connect-fields">
          {providerFields}
          <div className="setup-provider-actions">{saveControl}</div>
        </div>}
        {startupDetail && <div className="setup-provider-startup" role="status"
          data-startup-status={startupObservation.status} data-startup-phase={startupDetail.phase || undefined}>
          <span className="setup-provider-startup-mark" aria-hidden="true" />
          <div><span className="setup-provider-startup-label">{startupObservation.status === "failed" ? "NEEDS ATTENTION" : "NOW"}</span>
            <strong>{startupDetail.title}</strong><p>{startupDetail.detail}</p></div>
        </div>}
        {(primaryAction === "start" || primaryAction === "check") && <div className="setup-provider-next-action">
          {primaryAction === "check" ? checkControl : runtimeControl}
          {primaryAction === "start" && <p>Starting prepares your guide. It does not send your draft.</p>}
          {primaryAction === "check" && settings.work_active && <p>Checking reads saved settings and guide status. It does not stop work, change keys, or send your idea.</p>}
        </div>}
        {effective && <details className="setup-provider-secondary setup-provider-model-settings">
          <summary>Model settings</summary>
          <div className="setup-provider-connect-fields">{providerFields}
            {saveControl && <div className="setup-provider-actions">{saveControl}</div>}
          </div>
        </details>}
        <details className="setup-provider-secondary setup-provider-routine-controls">
          <summary>Guide controls</summary>
          <div className="setup-provider-actions">
            {primaryAction !== "start" && runtimeControl}
            {primaryAction !== "check" && checkControl}
          </div>
        </details>
      </> : <>
        {providerFields}
        {settings.work_active && <p className="setup-provider-work-note" role="status">Existing work or unconfirmed shutdown is keeping model settings and team controls paused. Check saved settings to refresh the status.</p>}
        <div className="setup-provider-actions">{saveControl}{runtimeControl}{checkControl}</div>
      </>}
      {settings.load_error && <p role="alert">Saved provider settings need attention. Existing environment configuration remains available.</p>}
      {settings.provider_binding_error && <p role="alert">The saved setup team's provider could not be verified. Preserve its native state before changing credentials.</p>}
      {saveUnknown && <p role="status">Check saved settings before entering a replacement key.</p>}
      {runtimeUnknown && <p role="status">The team may already have received the request. Check saved settings and team status before another action.</p>}
    </>}
    {message && <p role="status">{message}</p>}
    {error && <p role="alert">{error}</p>}
    {!settings && error && <button type="button" disabled={!!pending} onClick={checkSettings}>Check settings</button>}
  </section>;
}
