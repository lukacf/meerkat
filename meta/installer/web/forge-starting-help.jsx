import React, { useEffect, useId, useRef, useState } from "react";
import { startingAudiences, startingKinds, startingContextDetails } from "./forge-starting-context.mjs";
import "./forge-starting-conversation.css";
import { ForgeOrganizationStart } from "./forge-organization-start.jsx";

const audiences = [
  { id: "personal", label: "Just me", description: "Personal projects or everyday work", context: "For myself, " },
  { id: "household", label: "My household", description: "Things we share at home", context: "For my household, " },
  { id: "team", label: "A team", description: "Work shared by a group", context: "For a team, " },
  { id: "many", label: "Many people", description: "Customers, members, or an organization", context: "For many people, " },
  { id: "exploring", label: "I'm still exploring", description: "Help me find a starting point", context: "I am still exploring who this could help. " },
];
const kinds = [
  { id: "organizing", label: "Keeping track of things", description: "Information, plans, or commitments", context: "I would like help organizing information and commitments.", prompt: "What's one thing you need to keep track of?", example: "Keep track of decisions and deadlines from our weekly meeting." },
  { id: "understanding", label: "Figuring something out", description: "Research, compare, or reach a decision", context: "I would like help researching and understanding something.", prompt: "What question would you like help answering?", example: "Compare two options and explain which one fits my needs." },
  { id: "repetitive", label: "Repetitive work", description: "The same manual steps, again and again", context: "I would like help with repetitive work.", prompt: "What's one task you keep doing by hand?", example: "Turn our weekly project notes into a clear update." },
  { id: "other", label: "Something else", description: "Something you'd describe in your own words", context: "I have something else in mind.", prompt: "What would a useful result look like?", example: "Help me plan a project and keep the next steps clear." },
  { id: "exploring", label: "Still exploring", description: "Start with a frustration or an unfinished idea", context: "I am still exploring what kind of help would be useful.", prompt: "What took more time than you'd like this week?", example: "Each Friday, I spend an hour finding the updates I need." },
];

function appliedHandoff(hint, draft) {
  if (!hint || typeof hint.prefix !== "string" || typeof hint.inserted !== "string" || typeof draft !== "string") return null;
  const audience = audiences.find(option => option.id === hint.audience_id);
  const kind = kinds.find(option => option.id === hint.kind_id);
  if (!audience || !kind || hint.inserted !== `${hint.prefix}${audience.context}${kind.context}` || !draft.startsWith(hint.inserted)) return null;
  return { ...hint, audience: audience.label, kind: kind.label, prompt: kind.prompt, example: kind.example };
}

// Optional context only. The main composer is the editor, and sending stays explicit.
export function ForgeStartingHelp(props) {
  if (props.organization) return <ForgeOrganizationStart {...props} />;
  // Existing saved prose remains editable under its original ownership contract.
  return props.separateContext && (!props.appliedContext || props.appliedContext.version === 2)
    ? <ForgeStartingConversation {...props} /> : <LegacyStartingHelp {...props} />;
}

export function ForgeStartingContext({ context, composerId, disabled = false, onEdit, onRemove }) {
  const id = useId();
  const details = startingContextDetails(context);
  if (!details) return null;
  const audiencePhrase = {
    personal: "For myself", household: "For my household", team: "For a team", many: "For many people",
    exploring: "For someone I'm still figuring out",
  }[details.audience.id];
  const kindPhrase = details.kind.id === "exploring" ? "finding a starting point" : details.kind.label.toLowerCase();
  return <div className="forge-starting-context" role="group" aria-label="Starting point, not sent">
    <div className="forge-starting-context__heading"><span className="forge-starting-context__label">Starting point</span><span>Not sent</span></div>
    <div className="forge-starting-context__answers">
      <h2 id={composerId ? `${composerId}-starting-context` : `${id}-starting-context`} className="forge-starting-context__sentence" tabIndex={-1}>
        <button type="button" disabled={disabled} aria-label={`Change who this helps: ${details.audience.label}`}
          onClick={event => onEdit?.("audience", event.currentTarget)}>{audiencePhrase}</button>{", help with "}
        <button type="button" disabled={disabled} aria-label={`Change the kind of work: ${details.kind.label}`}
          onClick={event => onEdit?.("help", event.currentTarget)}>{kindPhrase}</button>{"."}
      </h2>
      <button type="button" disabled={disabled} className="forge-starting-context__remove"
        aria-label="Remove starting point" onClick={() => onRemove?.()}>Remove</button>
    </div>
    <p className="forge-starting-context__hint setup-visually-hidden">Change either part. The other stays as it is.</p>
  </div>;
}

// Inspiration only. It never changes the composer or submits a starting point.
export function ForgeStartingExample({ context, composerId }) {
  const id = useId();
  const details = startingContextDetails(context);
  const exampleId = typeof composerId === "string" && composerId ? `${composerId}-starting-example` : `${id}-starting-example`;
  if (!details) return null;
  return <p id={exampleId} className="forge-starting-example setup-visually-hidden">
    <span className="forge-starting-example__label">For example</span>{" "}
    <span>{details.kind.example}</span>
  </p>;
}

function ForgeStartingConversation({ draft = "", disabled = false, composerId, appliedContext, editRequest, onChooseContext, onReturnToTyping }) {
  const id = useId(), heading = useRef(null), section = useRef(null);
  const editSnapshot = useRef(null), cancelFocus = useRef(null);
  const details = startingContextDetails(appliedContext);
  const [open, setOpen] = useState(false), [step, setStep] = useState("audience");
  const [route, setRoute] = useState(() => details ? "choose" : "write");
  const [editField, setEditField] = useState(null);
  const [audienceId, setAudienceId] = useState(null), [kindId, setKindId] = useState(null);
  const [announcement, setAnnouncement] = useState("");
  const audience = startingAudiences.find(option => option.id === audienceId);

  function begin(nextStep = "audience", fieldEdit = false, requestedTrigger = null) {
    if (disabled) return;
    const scope = section.current?.parentElement;
    const trigger = requestedTrigger?.isConnected && requestedTrigger.closest(".forge-starting-context")?.parentElement === scope
      ? requestedTrigger : scope?.ownerDocument.activeElement;
    editSnapshot.current = fieldEdit && details ? {
      audienceId: details.audience.id, kindId: details.kind.id, scope,
      trigger: trigger?.tagName === "BUTTON" && trigger.closest(".forge-starting-context")?.parentElement === scope ? trigger : null,
    } : null;
    cancelFocus.current = null;
    setAudienceId(details?.audience.id || null); setKindId(details?.kind.id || null);
    setEditField(fieldEdit && details ? nextStep === "help" ? "help" : "audience" : null);
    setStep(nextStep === "help" && details ? "help" : "audience"); setRoute("choose"); setOpen(true);
  }
  useEffect(() => { if (editRequest) begin(editRequest.step, true, editRequest.trigger); }, [editRequest]);
  useEffect(() => {
    if (details) {
      const previous = editSnapshot.current;
      if (previous && (previous.audienceId !== details.audience.id || previous.kindId !== details.kind.id)) {
        editSnapshot.current = null; cancelFocus.current = null;
        setOpen(false); setRoute("choose"); setEditField(null);
        setAudienceId(details.audience.id); setKindId(details.kind.id);
      }
      return;
    }
    // Removing applied context also abandons the edit staged from those choices.
    editSnapshot.current = null; cancelFocus.current = null;
    setOpen(false); setRoute("write"); setAudienceId(null); setKindId(null); setEditField(null);
  }, [appliedContext]);
  useEffect(() => {
    if (!open) {
      const pending = cancelFocus.current;
      cancelFocus.current = null;
      if (!pending || !details || pending.audienceId !== details.audience.id || pending.kindId !== details.kind.id ||
          !pending.scope?.isConnected || section.current?.parentElement !== pending.scope ||
          !pending.trigger?.isConnected || pending.trigger.closest(".forge-starting-context")?.parentElement !== pending.scope) return;
      const currentFocus = pending.scope.ownerDocument.activeElement;
      if (currentFocus === pending.owner || currentFocus === pending.scope.ownerDocument.body &&
          pending.owner && (!pending.owner.isConnected || pending.owner.closest("[hidden]"))) pending.trigger.focus();
      return;
    }
    heading.current?.focus({ preventScroll: true });
    heading.current?.scrollIntoView?.({ block: "nearest", behavior: "auto" });
  }, [open, step]);

  function returnToTyping() {
    if (disabled) return;
    editSnapshot.current = null; cancelFocus.current = null;
    setOpen(false); setRoute("write"); setEditField(null);
    onReturnToTyping?.();
  }
  function reviewAnswers() {
    if (disabled || !details) return;
    setRoute("choose");
    // The review is the next sibling in this composer, never another setup's summary.
    const review = section.current?.nextElementSibling;
    if (!review?.matches(".forge-starting-context")) return;
    const sentence = review.querySelector(".forge-starting-context__sentence");
    sentence?.focus({ preventScroll: true });
    sentence?.scrollIntoView?.({ block: "nearest", behavior: "auto" });
  }
  function cancel() {
    if (disabled) return;
    if (!editField || !details) { returnToTyping(); return; }
    const previous = editSnapshot.current;
    cancelFocus.current = previous && previous.audienceId === details.audience.id && previous.kindId === details.kind.id
      ? { ...previous, owner: section.current?.ownerDocument.activeElement } : null;
    editSnapshot.current = null;
    setOpen(false); setRoute("choose"); setEditField(null);
    setAudienceId(details.audience.id); setKindId(details.kind.id);
  }
  function commit(nextAudienceId, nextKindId) {
    if (typeof onChooseContext !== "function") return;
    editSnapshot.current = null; cancelFocus.current = null;
    onChooseContext({ version: 2, audience_id: nextAudienceId, kind_id: nextKindId });
    setAudienceId(nextAudienceId); setKindId(nextKindId); setOpen(false); setRoute("choose"); setEditField(null);
    setAnnouncement(`${details ? "Starting point updated" : "Starting point added"}. Your writing is unchanged. Nothing has been sent.`);
  }
  function choose(option) {
    if (disabled) return;
    if (step === "audience") {
      if (editField === "audience" && details) { commit(option.id, details.kind.id); return; }
      setAudienceId(option.id); setStep("help"); return;
    }
    if (audience) commit(audience.id, option.id);
  }

  return <section ref={section} className="forge-starting-help forge-starting-conversation" data-open={open} data-route={route}
    data-review={!!details && !open}
    aria-label="Help with your first idea" onKeyDown={event => {
      if (open && event.key === "Escape" && !disabled) { event.preventDefault(); event.stopPropagation(); cancel(); }
    }}>
    <p className="forge-starting-conversation__lead">Two ways to begin. Switch any time - your words are kept.</p>
    <div className="forge-starting-conversation__invitation" role="group" aria-label="How would you like to start?">
      <button type="button" disabled={disabled} aria-pressed={route === "write"} aria-controls={composerId || undefined}
        onClick={returnToTyping}>
        <span className="forge-starting-conversation__route-label">Write</span>
        <strong>In my own words</strong><small>Describe a task or a rough idea</small>
      </button>
      <button type="button" disabled={disabled} aria-pressed={route === "choose"} aria-expanded={details && !open ? undefined : open} aria-controls={details && !open ? composerId ? `${composerId}-starting-context` : undefined : `${id}-content`}
        onClick={() => { if (!open) { if (details) reviewAnswers(); else begin(); } }}>
        <span className="forge-starting-conversation__route-label">Choose</span>
        <strong>{details && !open ? "Your two answers" : "Answer two questions"}</strong><small>{details && !open ? `${details.audience.label} / ${details.kind.label}` : "Pick who it helps and what kind of work"}</small>
      </button>
    </div>
    <div id={`${id}-content`} hidden={!open} className="forge-starting-conversation__content">
      <div className="forge-starting-conversation__header">
        <div className="forge-starting-conversation__progress"><span>{editField ? step === "audience" ? "Change who this helps" : "Change the kind of work" : `Question ${step === "audience" ? "1" : "2"} of 2`}</span>
          {!editField && <span className="forge-starting-conversation__progress-marks" aria-hidden="true">
            <i data-current={step === "audience"} /><i data-current={step === "help"} />
          </span>}
        </div>
        <button type="button" disabled={disabled} onClick={cancel}>{editField ? "Cancel change" : draft.trim() ? "Back to my words" : "Write it myself instead"}</button></div>
      {open && draft.trim().length > 0 && <div className="forge-starting-conversation__draft">
        <span>Your idea - not sent</span>
        <p>{draft}</p>
      </div>}
      {editField === "audience" && details && <div className="forge-starting-conversation__answered">
        <span>Keeping <strong>{details.kind.label}</strong></span>
      </div>}
      {step === "help" && audience && <div className="forge-starting-conversation__answered">
        <span>Who is it for? <strong>{audience.label}</strong></span>
        <button type="button" disabled={disabled} onClick={() => { if (!disabled) { if (editField) setEditField("audience"); setStep("audience"); } }}>Change</button>
      </div>}
      <h3 ref={heading} tabIndex={-1} id={`${id}-question`}>{step === "audience" ? "Who is it for?" : "What would help most?"}</h3>
      <div className="forge-starting-conversation__choices" role="group" aria-labelledby={`${id}-question`}>
        {(step === "audience" ? startingAudiences : startingKinds).map(option => <button type="button" key={option.id}
          disabled={disabled} aria-pressed={(step === "audience" ? audienceId : kindId) === option.id}
          onClick={() => choose(option)}>
          <span className="forge-starting-conversation__choice-marker" aria-hidden="true" />
          <span className="forge-starting-conversation__choice-copy"><strong>{option.label}</strong><small>{option.description}</small></span>
          <span className="forge-starting-conversation__choice-arrow" aria-hidden="true">&#8594;</span>
        </button>)}
      </div>
      <p className="forge-starting-conversation__hint">{draft.length ? "Your writing is kept. Nothing is sent." : "Not sure is a fine answer. Nothing is sent."}</p>
    </div>
    <span className="setup-visually-hidden" role="status" aria-live="polite">{announcement}</span>
  </section>;
}

function LegacyStartingHelp({ draft = "", disabled = false, inlineStart = false, composerId, appliedContext, onChoose, onReturnToTyping }) {
  const id = useId(), disclosure = useRef(null), heading = useRef(null);
  const [open, setOpen] = useState(false);
  const [step, setStep] = useState("audience");
  const [audienceId, setAudienceId] = useState(() => inlineStart ? appliedHandoff(appliedContext, draft)?.audience_id || null : null);
  const [kindId, setKindId] = useState(() => inlineStart ? appliedHandoff(appliedContext, draft)?.kind_id || null : null);
  const [handoff, setHandoff] = useState(null);
  const audience = audiences.find(option => option.id === audienceId);
  const kind = kinds.find(option => option.id === kindId);
  const hasDraft = typeof draft === "string" && draft.length > 0;
  const preview = audience && kind ? `${audience.context}${kind.context}` : "";
  const canChoose = !disabled && !!preview && typeof onChoose === "function";
  const controlledContext = inlineStart && appliedContext !== undefined;
  const currentHandoff = inlineStart ? appliedHandoff(controlledContext ? appliedContext : handoff, draft) : null;

  useEffect(() => {
    // The user owns all edits. Never reconstruct a context block they changed.
    if (!controlledContext && handoff && !appliedHandoff(handoff, draft)) setHandoff(null);
  }, [draft]);

  useEffect(() => {
    if (!controlledContext) return;
    const restored = appliedHandoff(appliedContext, draft);
    setAudienceId(restored?.audience_id || null); setKindId(restored?.kind_id || null);
    if (!restored) { setOpen(false); setStep("audience"); }
  }, [appliedContext]);

  useEffect(() => {
    if (!open) return;
    heading.current?.focus({ preventScroll: true });
    heading.current?.scrollIntoView?.({ block: "nearest", behavior: "auto" });
    // Only navigation moves focus. Draft and busy updates keep the user's position.
  }, [open, step]);

  function chooseAudience(option) {
    if (disabled) return;
    setAudienceId(option.id); setStep("help");
  }
  function chooseKind(option) {
    if (disabled) return;
    setKindId(option.id);
    if (inlineStart && typeof onChoose === "function") applyContext(option);
    else setStep("preview");
  }
  function close() { setOpen(false); disclosure.current?.focus(); }
  function returnToTyping() {
    if (disabled) return;
    close();
    if (typeof onReturnToTyping === "function") onReturnToTyping();
  }
  function applyContext(option) {
    if (disabled || !audience || !option || typeof onChoose !== "function") return;
    const context = `${audience.context}${option.context}`;
    const prefix = currentHandoff ? currentHandoff.prefix : hasDraft ? `${draft}\n\n` : "";
    const inserted = `${prefix}${context}`;
    const suffix = currentHandoff ? draft.slice(currentHandoff.inserted.length) : "";
    const hint = { audience_id: audience.id, kind_id: option.id, prefix, inserted };
    if (inlineStart && !controlledContext) setHandoff(hint);
    close();
    if (inlineStart) onChoose(`${inserted}${suffix}`, hint);
    else onChoose(`${inserted}${suffix}`);
  }
  function usePreview() { if (canChoose) applyContext(kind); }
  function toggleQuestions() {
    if (disabled) return;
    if (!open && inlineStart) setStep("audience");
    setOpen(value => !value);
  }

  return <section className={`forge-starting-help${inlineStart ? " forge-starting-help--inline" : ""}`}>
    <div className="forge-starting-help__invitation" role={inlineStart ? "group" : undefined}
      aria-label={inlineStart ? "Help with your first idea" : undefined}
      data-context-summary={inlineStart && !open && currentHandoff ? "true" : undefined}
      data-entry-options={inlineStart && !open && !currentHandoff ? "true" : undefined}>
      {inlineStart && !open && currentHandoff && <span className="forge-starting-help__summary">{currentHandoff.audience} <span aria-hidden="true">-</span> {currentHandoff.kind}</span>}
      {inlineStart && !open && !currentHandoff && <span className="forge-starting-help__encouragement">Need a starting point?</span>}
      <button ref={disclosure} type="button" className="forge-starting-help__toggle" disabled={disabled}
        aria-expanded={open} aria-controls={`${id}-content`}
        onClick={inlineStart && open ? returnToTyping : toggleQuestions}>
        {inlineStart && !open && !currentHandoff
          ? "Help me get started"
          : inlineStart && open ? "Write it myself" : currentHandoff ? "Change choices" : "Help me get started"}
      </button>
      {!inlineStart && <p>Two quick questions can help you find a starting point.</p>}
    </div>
    {!open && currentHandoff && <div className="forge-starting-help__handoff" aria-label="A starting point for your idea">
      {composerId ? <label className="forge-starting-help__task-prompt" htmlFor={composerId}>{currentHandoff.prompt}</label>
        : <p className="forge-starting-help__task-prompt">{currentHandoff.prompt}</p>}
      <p className="forge-starting-help__task-example">For example: {currentHandoff.example} <span>Add one task, or send this draft and your guide will help you find one.</span></p>
    </div>}
    <div id={`${id}-content`} className="forge-starting-help__content" hidden={!open} data-step={step}>
      <div className="forge-starting-help__step-header">
        <p className="forge-starting-help__progress">{step === "preview" ? "Your starting point" : `Question ${step === "audience" ? "1" : "2"} of 2`}</p>
        <span className="forge-starting-help__step-marks" aria-hidden="true"><i data-current={step === "audience"} /><i data-current={step !== "audience"} /></span>
        {inlineStart && step === "help" && <button type="button" disabled={disabled} className="forge-starting-help__back"
          onClick={() => { if (!disabled) setStep("audience"); }}>Back</button>}
      </div>
      {step !== "preview" ? <>
        <h3 ref={heading} tabIndex={-1} id={`${id}-${step}-question`}>{step === "audience" ? "Who would this help?" : "What kind of work would you like help with?"}</h3>
        {step === "help" && <p className="forge-starting-help__answer">{inlineStart
          ? <>{audience.label}. Pick one to add context. Nothing is sent.</>
          : <>For: {audience.label}</>}</p>}
        <div className="forge-starting-help__choices" role="group" aria-labelledby={`${id}-${step}-question`}>
          {(step === "audience" ? audiences : kinds).map(option => <button key={option.id} type="button" disabled={disabled}
            className="forge-starting-help__choice" aria-pressed={(step === "audience" ? audienceId : kindId) === option.id}
            onClick={() => step === "audience" ? chooseAudience(option) : chooseKind(option)}>
            <span className="forge-starting-help__choice-mark" aria-hidden="true" />
            <span className="forge-starting-help__choice-copy"><strong>{option.label}</strong><small>{option.description}</small></span>
          </button>)}
        </div>
        {!inlineStart && <div className="forge-starting-help__navigation">
          {step === "help" && <button type="button" disabled={disabled} className="forge-starting-help__back"
            onClick={() => { if (!disabled) setStep("audience"); }}>Back</button>}
          {!inlineStart && <button type="button" disabled={disabled} className="forge-starting-help__keep" onClick={returnToTyping}>Back to typing</button>}
        </div>}
      </> : <div className="forge-starting-help__preview">
        <h3 ref={heading} tabIndex={-1} id={`${id}-preview-heading`}>{inlineStart ? kind.prompt : "Your starting context"}</h3>
        {inlineStart ? <div className="forge-starting-help__context" aria-label="Your starting context">
          <span>{audience.label}</span><span>{kind.label}</span>
        </div> : <p className="forge-starting-help__suggestion">{preview}</p>}
        <p>{inlineStart
          ? currentHandoff ? "Update the choices you added. Your own writing stays exactly as it is."
            : "Add these choices to your draft, then describe the task in your own words. You can edit everything before you send."
          : hasDraft ? "Your existing text is kept. Add this context, or keep your idea as it is. Nothing is sent."
            : "You can edit this in the idea box before sending. Nothing is sent."}</p>
        <div className="forge-starting-help__preview-actions">
          <button type="button" disabled={!canChoose} className="forge-starting-help__use" onClick={usePreview}>
            {inlineStart ? currentHandoff ? "Update context" : hasDraft ? "Add to my draft" : "Use this context" : hasDraft ? "Add to my idea" : "Use this starting point"}
          </button>
          <button type="button" disabled={disabled} className="forge-starting-help__keep" onClick={returnToTyping}>
            {hasDraft ? "Keep my idea" : "Back to typing"}
          </button>
        </div>
        <div className="forge-starting-help__navigation">
          <button type="button" disabled={disabled} className="forge-starting-help__back"
            onClick={() => { if (!disabled) setStep("help"); }}>Back to questions</button>
        </div>
      </div>}
    </div>
  </section>;
}
