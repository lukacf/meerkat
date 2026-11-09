import React from "react";

function recipientName(plan, identity, kind) {
  if (kind === "class") {
    const role = plan?.agent_classes?.find(item => item.id === identity);
    return `${role?.name || identity} - all current and future members`;
  }
  const [classId, memberKey] = identity.split(":");
  const role = plan?.agent_classes?.find(item => item.id === classId);
  const member = role?.members?.find(item => item.key === memberKey);
  return member?.name || (role?.instances === 1 ? role.name : role ? `${role.name} ${memberKey}` : identity);
}

export function IncomingDataReview({ brief, plan, disabled = false, onRequestChange }) {
  const sources = brief?.incoming_data || [];
  if (!sources.length) return null;
  const requirements = new Map((brief.requirements || []).map(item => [item.id, item]));
  return <section className="ep-incoming-data" aria-label="Incoming information">
    <div className="ep-section-heading"><h3>What reaches your team</h3><span>Proposed routing</span></div>
    <div className="ep-card">
      {sources.map(source => {
        const requirement = requirements.get(source.requirement_id);
        const postponed = ["cancelled", "deferred", "handoff"].includes(requirement?.disposition);
        return <details key={source.id} className="ep-source-intent">
          <summary><strong>{source.name}</strong><span>{postponed ? "Outside this version" : source.routes.some(route => route.recipient_kind === "class") ? "Includes an agent class" : source.routes.length ? `${source.routes.length} ${source.routes.length === 1 ? "recipient" : "recipients"}` : "Recipient to decide"}</span></summary>
          <p>{source.purpose}</p>
          <dl className="ep-facts">
            <div><dt>Which information</dt><dd>{source.scope}</dd></div>
            <div><dt>Who may see it</dt><dd>{source.audience || "Still to decide"}</dd></div>
            <div><dt>Existing history</dt><dd>{source.history === "new_only" ? "Start with new changes" : source.history === "bounded_history" ? source.history_window : "Still to decide"}</dd></div>
            <div><dt>Who receives it</dt><dd>{source.routes.length ? <ul>{source.routes.map(route => <li key={`${route.recipient_kind || "identity"}:${route.recipient}`}>
              <strong>{recipientName(plan, route.recipient, route.recipient_kind)}</strong>{" - "}
              {{direct: "receives matching updates directly", triage: "judges relevance and routes updates", conversation: "participates in the conversation"}[route.delivery]}
            </li>)}</ul> : "Still to decide"}</dd></div>
          </dl>
          <p className="ep-assessment-note">{postponed ? "Kept here as part of your scope decisions." : "This describes your plan. Collection and access still need their setup and checks."}</p>
          {requirement?.extension && <p>{requirement.extension}</p>}
          <button type="button" className="ep-text-button" disabled={disabled || !onRequestChange}
            onClick={() => onRequestChange?.(`Review the incoming information plan for ${source.name} (${source.id}). Help me choose who should receive it, which information they may see, and whether existing history is useful. Preserve the other requirements and ask a focused multiple-choice question before changing this plan.`)}>
            Refine with your guide
          </button>
        </details>;
      })}
    </div>
  </section>;
}
