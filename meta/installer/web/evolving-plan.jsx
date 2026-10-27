import React, { useState } from "react";
import { canResizeClass, classInstanceLimit } from "./state.mjs";
import { setupRequirementGroups } from "./wizard-state.mjs";
import {
  guidePlan,
  guideStagedDesign,
  guideDesignBrief,
  guideVisualizationFeedback,
  hasDecision,
  toolSummary,
  connectionDetails,
  setupFacts,
  classCountRequest,
} from "./evolving-plan-state.mjs";
import "./evolving-plan.css";
import { ProposalSimulation } from "./proposal-simulation.jsx";
import { ForgeWelcomePortrait } from "./first-experience.jsx";
import { firstProposalStatus } from "./first-proposal-status.mjs";
import { packagePreparationReason } from "./package-availability.mjs";
import { NativeBundleSelection } from "./native-bundle-selection.jsx";
import { IncomingDataReview } from "./incoming-data-review.jsx";

function Arrow({ size = 16 }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M4 10h12m-5-5 5 5-5 5"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function AgentRow({ agent, disabled, instanceLimit, canDecrease, onRequestChange, onResize }) {
  const [expanded, setExpanded] = useState(false);
  const tools = toolSummary(agent.tools);
  return (
    <div className="ep-agent-row">
      <div className="ep-agent-info">
        <div className="ep-agent-name">
          <strong>{agent.name}</strong>
        </div>
        <p>{agent.description}</p>
        {agent.instances === 0 && <small>Reusable class - no initial agents. Create real context owners through the application when needed.</small>}
        {agent.members?.length > 0 && (
          <div className="ep-member-list">
            {agent.members.length > 6 && <span>{agent.members.length} named identities. Explore them in the simulation.</span>}
            {agent.members.slice(0, 6).map((member) => (
              <span key={member.key}>{member.name}</span>
            ))}
          </div>
        )}
        <button
          className="ep-text-button ep-tools-toggle"
          onClick={() => setExpanded(!expanded)}
          aria-expanded={expanded}
        >
          {expanded ? "Hide role details" : "Role details"}
          {!expanded && tools.length > 0 && <span> {tools.length}</span>}
        </button>
        {expanded && (
          <div className="ep-role-details">
            <p className="ep-role-instructions">{agent.instructions}</p>
            <div className="ep-tool-tags">
              {tools.length ? (
                tools.map((tool) => <span key={tool}>{tool}</span>)
              ) : (
                <span>No optional tools granted</span>
              )}
            </div>
            <NativeBundleSelection tools={agent.tools} />
            <small>
              {agent.provider} / {agent.model}
            </small>
          </div>
        )}
      </div>
      <div className="ep-agent-controls">
        <div className="ep-stepper" aria-label={`${agent.name} agent count`}>
          <button
            disabled={disabled || !canDecrease}
            aria-label={`Ask guide for fewer ${agent.name} agents`}
            onClick={() =>
              onResize
                ? onResize(agent.id, agent.instances - 1)
                : onRequestChange?.(
                    classCountRequest(agent, agent.instances - 1),
                  )
            }
          >
            -
          </button>
          <span>{agent.instances}</span>
          <button
            disabled={disabled || agent.instances >= instanceLimit}
            aria-label={`Ask guide for more ${agent.name} agents`}
            onClick={() =>
              onResize
                ? onResize(agent.id, agent.instances + 1)
                : onRequestChange?.(
                    classCountRequest(agent, agent.instances + 1),
                  )
            }
          >
            +
          </button>
        </div>
      </div>
    </div>
  );
}

export function AcquisitionReview({ plan, state, disabled = false, onRequestChange }) {
  const selection = plan?.acquisition;
  if (!selection) return null;
  const [classId, memberKey] = selection.helper_identity.split(":");
  const agent = plan.agent_classes.find((item) => item.id === classId);
  const member = agent?.members?.find((item) => item.key === memberKey);
  const helperName = member?.name || (agent?.instances > 1 ? `${agent.name} ${memberKey}` : agent?.name);
  const packageReason = packagePreparationReason(state, plan);
  return <section aria-label="Connection preparation">
    <div className="ep-section-heading"><h3>Connection preparation</h3><span>Planned capability</span></div>
    <div className="ep-card">
      <dl className="ep-facts">
        <div><dt>Responsible role</dt><dd>{helperName || "Setup role to review"}</dd></div>
        <div><dt>Planned capability</dt><dd>{selection.helper_tools === "checked_installation"
          ? "Find connections, prepare setup, inspect candidate packages, and install requested software after its exact review."
          : selection.helper_tools === "inert_acquisition"
            ? "Find connections, prepare setup, and download and inspect candidate packages."
            : "Find connections and prepare their setup."}</dd></div>
        <div><dt>Working context</dt><dd>{selection.helper_profile === "exclusive_application_role"
          ? "Requests use of the selected application role."
          : "Requests a separate preparation profile."}</dd></div>
        <div><dt>Access boundary</dt><dd>{selection.helper_tools === "checked_installation"
          ? "The exact software installation must be reviewed before it can run. Choosing this capability installs nothing and connects no account. Review the role's ordinary tools separately."
          : "Preparing a connection does not activate it or grant account access. Review the role's ordinary tools separately."}</dd></div>
      </dl>
      <p role={packageReason ? "status" : undefined}>{packageReason || "This capability is part of your proposed design. It must be implemented and checked before installation."}</p>
      <button className="ep-text-button" disabled={disabled || !onRequestChange}
        onClick={() => onRequestChange?.("Discuss removing connection preparation from my plan. Explain what I would lose and how the remaining roles would handle setup. Preserve the rest of my requirements and ask before changing the plan.")}>
        Discuss removing this setup helper
      </button>
    </div>
  </section>;
}

export function ScopeDecisions({ state, compact = false }) {
  const decisions = (state?.brief?.requirements || []).filter((item) => ["deferred", "cancelled"].includes(item.disposition));
  if (!decisions.length) return null;
  return <details className="ep-scope-decisions" open={!compact && (state.status === "review" || state.status === "ready")}>
    <summary>Your scope decisions <span>{decisions.length}</span></summary>
    <ul>{decisions.map((item) => <li key={item.id}><strong>{item.disposition === "cancelled" ? "Removed by you" : "Agreed for later"}</strong><p>{item.outcome}</p>{item.disposition === "deferred" && item.extension && <small>{item.extension}</small>}</li>)}</ul>
  </details>;
}

export function SetupRequirementGroups({ state, groups: suppliedGroups, className = "" }) {
  const groups = suppliedGroups || setupRequirementGroups(state);
  const sections = [
    ["decisions", "Decisions to make"],
    ["implementation", "Application work to build"],
    ["checks", "Other checks"],
  ].filter(([key]) => groups[key]?.length);
  if (!sections.length) return null;
  return <div className={className}>
    {sections.map(([key, title]) => <section key={key} aria-label={title}>
      <strong>{title}</strong>
      <ul>{groups[key].map((item, index) => <li key={index}>{item}</li>)}</ul>
    </section>)}
  </div>;
}

function ConnectionRow({ connection, plan, onRequestChange, disabled }) {
  const detail = connectionDetails(connection, plan);
  return (
    <div className="ep-connection-row">
      <div>
        <strong>{connection.name}</strong>
        <span title={detail.source}>{detail.source}</span>
        <small>
          {detail.environment.length
            ? "Configure credentials and settings at installation"
            : detail.users.length
              ? `Used by ${detail.users.map((agent) => agent.name).join(", ")}`
              : "Available to assign to an agent"}
        </small>
      </div>
      <button
        className="ep-connection-status"
        disabled={disabled}
        title="Ask your guide what this connection needs"
        onClick={() =>
          onRequestChange?.(
            `Explain how to set up ${connection.name} (${connection.id}) for this plan. What access and credentials will I need, and which of my agents will use it? Keep my current plan unless I ask for a change.`,
          )
        }
      >
        {detail.status}
        <Arrow size={12} />
      </button>
    </div>
  );
}

export function EvolvingPlan({
  state,
  plan: visiblePlan,
  busy = false,
  pending = false,
  onAdjust,
  onRequestChange,
  onExpand,
  onResize,
  onConfigure,
  rehearsalActivity,
  applicationProgress,
  welcome = false,
  welcomeDraft = "",
  guideTimeline,
  onConversation,
}) {
  const plan = visiblePlan || guidePlan(state);
  const staged = !pending && guideStagedDesign(state);
  const designBrief = pending ? state?.brief : guideDesignBrief(state, plan);
  const visualizationFeedback = pending ? state?.visualization_feedback : guideVisualizationFeedback(state, plan);
  const previewingDraft = !!staged && staged.blueprint === plan;
  const pendingDraftContext = previewingDraft && staged.context_pending === true;
  const checkedDraftContext = previewingDraft && !pendingDraftContext && staged.brief === designBrief;
  const requirementGroups = setupRequirementGroups(state);
  const requirementCount = Object.values(requirementGroups).reduce((total, items) => total + items.length, 0);
  const count =
    plan?.agent_classes.reduce((total, agent) => total + agent.instances, 0) ||
    0;
  const disabled = busy || pending;
  const packageReason = packagePreparationReason(state, plan);
  const facts = setupFacts(pendingDraftContext
    ? { brief: null, answers: [], completed_stages: [], status: "question" }
    : checkedDraftContext ? { ...state, brief: designBrief } : state, plan);
  const stageKnown = hasDecision(state, "connections");
  const briefFacts = Object.entries(designBrief?.facts || {}).filter(
    ([, fact]) => fact.summary,
  );
  const hasAssessmentContext = briefFacts.length > 0 ||
    [state?.brief?.requirements, state?.feature_gaps, state?.warnings]
      .some((items) => Array.isArray(items) && items.length > 0);
  const illustratedEmpty = !plan && !pending;
  const proposalStatus = illustratedEmpty ? firstProposalStatus(state, guideTimeline) : null;
  const workspace = plan?.agent_classes.some((agent) => agent.tools?.workspace);
  return (
    <aside className={`evolving-plan${illustratedEmpty ? " ep-illustrated" : ""}`} aria-label="Your evolving deployment plan">
      <div className="ep-inner">
        {proposalStatus && <div className="ep-first-proposal-status" data-tone={proposalStatus.tone}>
          <span aria-hidden="true" />
          <div>
            <div role="status" aria-live="polite"><strong>{proposalStatus.title}</strong><small>{proposalStatus.detail}</small></div>
            {proposalStatus.showConversation && onConversation && <button type="button" onClick={onConversation}>
              Return to conversation <Arrow size={14} />
            </button>}
          </div>
        </div>}
        {illustratedEmpty && <ForgeWelcomePortrait draft={welcome ? welcomeDraft : ""} />}
        <header className="ep-header">
          <div>
            <div className="ep-eyebrow">Your team so far</div>
            <h2>{plan?.project.name || "Your idea, brought to life."}</h2>
          </div>
          {plan && (
            <div className="ep-stats">
              <div>
                <strong>{count}</strong>
                <span>{count === 1 ? "agent" : "agents"}</span>
              </div>
              <div>
                <strong>{plan.agent_classes.length}</strong>
                <span>
                  {plan.agent_classes.length === 1 ? "role" : "roles"}
                </span>
              </div>
            </div>
          )}
        </header>
        {plan && (
          <div className="ep-plan-status">
            <span
              className={`ep-status-dot ${!previewingDraft && state?.status === "ready" ? "is-ready" : ""}`}
            />
            <span>
              {pending
                ? "Your changes are waiting for guide review"
                : pendingDraftContext
                  ? "Provisional topology - guide context still in progress"
                : previewingDraft
                  ? "Checked draft preview - guide response still in progress"
                : applicationProgress
                  ? applicationProgress.title
                : state?.status === "ready"
                  ? packageReason ? "Accepted design - package preparation pending" : "Accepted plan - ready to package"
                  : state?.status === "review"
                    ? "Proposal ready for your review"
                    : "Draft plan, still taking shape"}
            </span>
          </div>
        )}
        <ProposalSimulation setupId={state?.id} plan={plan} interfaceFact={designBrief?.facts?.interfaces} designBrief={designBrief} visualizationFeedback={visualizationFeedback} pending={pending} busy={busy} onRequestChange={onRequestChange} onExpand={onExpand} rehearsalActivity={rehearsalActivity} />
        {briefFacts.length > 0 && (
          <details className="ep-brief" open={!plan}>
            <summary>
              <strong>What your guide understands</strong>
              <span>
                {briefFacts.length}{" "}
                {briefFacts.length === 1 ? "detail" : "details"}
              </span>
            </summary>
            {checkedDraftContext && <p className="ep-assessment-note">Checked draft understanding for this proposal preview. The guide response is still in progress.</p>}
            <dl>
              {briefFacts.map(([key, fact]) => (
                <div key={key}>
                  <dt>
                    {{
                      outcome: "The job",
                      audience: "Who it helps",
                      interfaces: "How you use it",
                      ownership: "Agent ownership",
                      inputs: "Information",
                      workflow: "How work happens",
                      knowledge: "Knowledge",
                      authority: "Permissions",
                      destination: "Where it runs",
                      first_version: "First version",
                    }[key] || key}
                    <span className={`ep-fact-status ${fact.status}`}>
                      {fact.status === "confirmed"
                        ? "From you"
                        : fact.status === "proposed"
                          ? "Guide suggestion"
                          : "To decide"}
                    </span>
                  </dt>
                  <dd>{fact.summary}</dd>
                </div>
              ))}
            </dl>
          </details>
        )}
        <ScopeDecisions state={state} compact />
        <IncomingDataReview brief={designBrief} plan={plan} disabled={disabled} onRequestChange={onRequestChange} />

        {!plan && hasAssessmentContext && requirementCount > 0 && (
          <div className="ep-requirements">
            <SetupRequirementGroups groups={requirementGroups} />
          </div>
        )}
        {plan ? (
          <>
            {requirementCount > 0 && (
              <details
                className="ep-requirements"
                open={false}
              >
                <summary>
                  <strong>
                    {previewingDraft ? "Last saved design checks" : applicationProgress ? "Guide's last design assessment" : `${requirementCount} ${requirementCount === 1 ? "thing" : "things"} to resolve before running`}
                  </strong>
                  <span>View</span>
                </summary>
                {previewingDraft && <p className="ep-assessment-note">These checks belong to your last saved design. The guide is still reviewing the draft preview.</p>}
                {applicationProgress && <p className="ep-assessment-note">These are design requirements. Use the build and installation checks to see what is implemented and verified.</p>}
                <SetupRequirementGroups groups={requirementGroups} />
              </details>
            )}
            <section>
              <div className="ep-section-heading">
                <h3>Your team</h3>
                <span>{plan.agent_classes.length} distinct roles</span>
              </div>
              <div className="ep-card">
                {plan.agent_classes.map((agent) => (
                  <AgentRow
                    key={agent.id}
                    agent={agent}
                    instanceLimit={classInstanceLimit(plan, agent.id)}
                    canDecrease={canResizeClass(plan, agent.id, agent.instances - 1)}
                    disabled={disabled || !(onResize || onRequestChange)}
                    onRequestChange={onRequestChange}
                    onResize={onResize}
                  />
                ))}
              </div>
            </section>
            <AcquisitionReview plan={plan} state={state} disabled={disabled} onRequestChange={onRequestChange} />
            <div className="ep-details-grid">
              <section>
                <div className="ep-section-heading">
                  <h3>Connections</h3>
                  <span>
                    {plan.connections.length
                      ? `${plan.connections.length} planned`
                      : stageKnown
                        ? "Your sources"
                        : "Coming next"}
                  </span>
                </div>
                <div className="ep-card">
                  {plan.connections.map((connection) => (
                    <ConnectionRow
                      key={connection.id}
                      connection={connection}
                      plan={plan}
                      disabled={disabled || !onRequestChange}
                      onRequestChange={onRequestChange}
                    />
                  ))}
                  {workspace && (
                    <div className="ep-local-source">
                      <strong>Workspace files</strong>
                      <span>
                        Text files you add to the system's workspace folder.
                      </span>
                    </div>
                  )}
                  {!plan.connections.length && !workspace && (
                    <p className="ep-empty-detail">
                      {stageKnown
                        ? "Start with conversation. Add services whenever you need them."
                        : "Your guide will help choose useful sources and services."}
                    </p>
                  )}
                </div>
              </section>
              <section>
                <div className="ep-section-heading">
                  <h3>Setup</h3>
                </div>
                <dl className="ep-card ep-facts">
                  {facts.map((fact) => (
                    <div key={fact.label}>
                      <dt>{fact.label}</dt>
                      <dd className={fact.pending ? "is-undecided" : ""}>
                        {fact.value}
                      </dd>
                    </div>
                  ))}
                </dl>
              </section>
            </div>
            {plan.flows.length > 2 && (
              <section className="ep-workflows">
                <div className="ep-section-heading">
                  <h3>All workflows</h3>
                  <span>{plan.flows.length} planned</span>
                </div>
                <div className="ep-card">
                  {plan.flows.map((flow) => (
                    <details key={flow.id}>
                      <summary>
                        <strong>{flow.name}</strong>
                        <span>{flow.steps.length} steps</span>
                      </summary>
                      {flow.description && <p>{flow.description}</p>}
                      <ol>
                        {flow.steps.map((step) => (
                          <li key={step.id}>
                            <code>{step.agent}</code>
                            <span>{step.prompt}</span>
                          </li>
                        ))}
                      </ol>
                    </details>
                  ))}
                </div>
              </section>
            )}
            <div className="ep-adjust">
              <span>Manual changes go back to your guide.</span>
              <button
                className="ep-text-button"
                disabled={busy || !onAdjust}
                onClick={onAdjust}
              >
                Advanced editor
                <Arrow size={14} />
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="ep-empty-facts">
              <div>
                <span>01</span>
                <strong>Describe your idea</strong>
                <p>No need to know the technical details.</p>
              </div>
              <div>
                <span>02</span>
                <strong>Shape it together</strong>
                <p>Your guide chooses agents and useful tools.</p>
              </div>
              <div>
                <span>03</span>
                <strong>Make it your own</strong>
                <p>Get a setup package for your computer or cloud.</p>
              </div>
            </div>
            <div className="ep-adjust">
              <span>Manual changes go back to your guide.</span>
              <button
                className="ep-text-button"
                disabled={busy || !onAdjust}
                onClick={onAdjust}
              >
                Advanced editor
                <Arrow size={14} />
              </button>
            </div>
          </>
        )}
      </div>
    </aside>
  );
}

export function SetupPlan({
  plan,
  state,
  pending,
  busy,
  onResize,
  onDetails,
  onAsk,
  onExpand,
  onConfigure,
  rehearsalActivity,
  applicationProgress,
  welcome = false,
  welcomeDraft = "",
  guideTimeline,
  onConversation,
}) {
  return (
    <EvolvingPlan
      plan={plan}
      state={state}
      pending={pending}
      busy={busy}
      onResize={onResize}
      onAdjust={onDetails}
      onRequestChange={onAsk}
      onExpand={onExpand}
      onConfigure={onConfigure}
      rehearsalActivity={rehearsalActivity}
      applicationProgress={applicationProgress}
      welcome={welcome}
      welcomeDraft={welcomeDraft}
      guideTimeline={guideTimeline}
      onConversation={onConversation}
    />
  );
}
