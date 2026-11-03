import React, { useId, useState } from "react";

export function ForgeOrganizationContext({ update, disabled = false, onAdopt }) {
  if (!update) return null;
  const organization = update.context;
  return <details className="forge-starting-help forge-organization-start">
    <summary>Workplace guidance from {organization.label}</summary>
    <p>Include these instructions in this setup and future Builds.</p>
    <pre style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{organization.guide_context || organization.description}</pre>
    <button type="button" disabled={disabled} onClick={() => onAdopt(update.fingerprint)}>Use for this setup</button>
  </details>;
}

// This is an editable draft picker. It does not send, install or grant anything.
export function ForgeOrganizationStart({ organization, draft = "", disabled = false, onChoose, onReturnToTyping }) {
  const id = useId();
  const [previewId, setPreviewId] = useState(null);
  const examples = organization.starting_examples || [];
  const preview = examples.find(example => example.id === previewId);
  const keepIdea = () => {
    if (disabled) return;
    setPreviewId(null);
    onReturnToTyping?.();
  };
  const choose = example => {
    if (disabled) return;
    if (draft.trim()) setPreviewId(example.id);
    else {
      setPreviewId(null);
      onChoose(example.initial_brief);
    }
  };
  return <section className="forge-starting-help forge-organization-start" aria-labelledby={id}>
    <div className="forge-organization-start__header">
      <div><h2 id={id}>Start with your work</h2>
        <p>{organization.description || (examples.length ? `Ideas for ${organization.label}. Choose one and make it your own.` : `Describe what you would like help with at ${organization.label}.`)}</p></div>
      <button type="button" disabled={disabled} onClick={keepIdea}>Describe my own idea <span aria-hidden="true">&#8594;</span></button>
    </div>
    <div className="setup-starting-points">
      {examples.map(example => <button key={example.id} type="button" disabled={disabled}
        aria-label={`${draft.trim() ? "Preview" : "Use"} ${example.title}`}
        onClick={() => choose(example)}>
        <span className="setup-starting-point-copy"><strong>{example.title}</strong><small>{example.outcome}</small></span>
        <span className="forge-example-action" aria-hidden="true">{draft.trim() ? "Preview" : "Use idea"} <span>&#8594;</span></span>
      </button>)}
      {preview && <details key={preview.id} open>
        <summary>Example: {preview.title}</summary>
        <p>{preview.initial_brief}</p>
        <small>Your writing is kept. Add this inspiration only if it helps.</small>
        <div className="setup-choice-actions">
          <button type="button" disabled={disabled} onClick={() => {
            if (disabled) return;
            onChoose(`${draft}\n\n${preview.initial_brief}`);
            setPreviewId(null);
          }}>Add inspiration to my idea</button>
          <button type="button" disabled={disabled} onClick={keepIdea}>Keep my idea</button>
        </div>
      </details>}
    </div>
    {examples.length > 0 && <p className="forge-organization-start__hint">An editable starting point. Your guide will explore the details with you.</p>}
  </section>;
}
