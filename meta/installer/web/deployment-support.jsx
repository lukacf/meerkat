import React from "react";
import "./deployment-support.css";

const titles = {
  unknown: "Checking installation support",
  eligible: "Keep installation support up to date",
  prepared: "Your support update is ready",
  "in-progress": "Continue your support update",
  completed: "Installation support is up to date",
  refused: "Installation support",
};
const phases = {
  prepared: "Review saved", retaining: "Retaining current support", retained: "Current support retained",
  installing: "Installing prepared support", installed: "Prepared support installed",
  publishing: "Recording the completed update", completed: "Update complete",
};

// This view presents the checked host projection. It never derives health,
// creates commands or resumes an operation itself.
export function DeploymentSupportPanel({ view, receipt, intent, disabled, busy, onAction, onCheck }) {
  const selected = view.selected;
  const step = view.state === "completed" ? 3 : view.state === "eligible" ? 1 :
    ["prepared", "in-progress"].includes(view.state) ? 2 : 0;
  const actionLabel = view.continuation ? "Resume this update" : view.action === "prepare-support" ? "Review update" :
    view.state === "prepared" ? "Apply support update" : "Continue support update";
  const description = view.state === "eligible"
    ? "Review an update to the background installation support before applying it."
    : view.state === "prepared"
    ? "Review saved. Apply this support update when you are ready."
    : view.continuation
    ? "The saved update needs your next step. Resume continues this exact update."
    : view.state === "completed"
    ? receipt?.support_request?.phase === "current"
      ? "Installation support already matches this Forge version. Your application is still stopped."
      : "The support update is complete. Your application is still stopped."
    : view.detail;
  return <section className="deployment-support" data-support-state={view.state} aria-label="Installation support">
    <div className="deployment-support-heading">
      <span className="deployment-support-mark" aria-hidden="true">{view.state === "completed" ? "✓" : "↻"}</span>
      <div><span className="deployment-support-eyebrow">Background installation</span>
        <h3>{titles[view.state] || view.label}</h3></div>
    </div>
    {step > 0 && view.state !== "completed" && <ol className="deployment-support-steps" aria-label="Support update steps">
      {["Review", "Apply", "Done"].map((label, index) => <li key={label} data-step={index + 1 === step ? "current" : index + 1 < step ? "done" : "next"}
        aria-current={index + 1 === step ? "step" : undefined}><span aria-hidden="true">{index + 1 < step ? "✓" : index + 1}</span>{label}</li>)}
    </ol>}
    <p className="deployment-support-description" role="status">{description}</p>
    {selected && ["prepared", "in-progress"].includes(view.state) && <div className="deployment-support-review">
      <span><strong>{selected.staged_files}</strong> support files prepared</span>
      <span><strong>{selected.retained_files}</strong> current support files {["prepared", "retaining"].includes(selected.phase) ? "to retain" : "retained"}</span>
    </div>}
    {["eligible", "prepared", "in-progress"].includes(view.state) &&
      <p className="deployment-support-preserved">Your application, private settings and agent history stay in place.</p>}
    {selected && view.state === "in-progress" &&
      <p className="deployment-support-phase">{phases[selected.phase] || "Checking the saved update"}</p>}
    {(view.action || view.state === "unknown") && <div className="deployment-support-actions">
      {view.action && <button type="button" className="setup-action primary" disabled={disabled || busy}
        onClick={() => onAction(view.action)}>{busy ? "Checking update..." : actionLabel}</button>}
      {view.state === "unknown" && <button type="button" className="setup-action" disabled={busy}
        onClick={onCheck}>Check support status</button>}
    </div>}
    {(selected || intent) && <details className="deployment-support-details"><summary>Technical details</summary>
      <dl>
        {intent && <><dt>Saved command</dt><dd><code>{intent.action_request_id}</code></dd></>}
        {selected && <><dt>Review</dt><dd><code>{selected.revision}</code></dd>
          <dt>Descriptor</dt><dd><code>{selected.descriptor_sha256}</code></dd>
          <dt>Current phase</dt><dd>{selected.phase}</dd></>}
        {receipt?.support_request?.service_digest && <><dt>Installation support</dt>
          <dd><code>{receipt.support_request.service_digest}</code></dd></>}
      </dl>
    </details>}
  </section>;
}
