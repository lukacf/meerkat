import React, { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { ConsoleComposer } from "../../packages/console-components/src/composer/console-composer";
import { GUIDE_ATTACHMENT_LIMIT, guideAttachmentAccept, guideAttachmentSelectionError } from "./guided-attachments.mjs";
import { GuideAttachmentChips, useGuideAttachments } from "./guided-attachments.jsx";
import {
  request,
  requestBlob,
  clone,
  identities,
  fingerprint,
  canonicalJson,
  resizeClass,
} from "./state.mjs";
import {
  GUIDE_KEY,
  GUIDE_MANUAL_KEY,
  pendingManual,
  GUIDE_STAGES,
  toggleAnswer,
  canCombineOptions,
  canSubmitAnswer,
  manualFeedback,
} from "./guided-state.mjs";
import { SetupPlan, ScopeDecisions, SetupRequirementGroups } from "./evolving-plan.jsx";
import { guidePlan } from "./evolving-plan-state.mjs";
import { NativeGuideChat, useGuideTimeline } from "./native-guide-chat.jsx";
import { GuideWaitStatus } from "./guide-wait.jsx";
import { guideWaitBinding } from "./guide-wait.mjs";
import { guideStartupPresentation } from "./guide-startup.mjs";
import { guideAdmissionMatches } from "./guide-admission.mjs";
import { ImplementationPanel } from "./implementation-panel.jsx";
import { DeploymentPanel } from "./deployment-panel.jsx";
import { deploymentCanPrepare, nativeTeamCanInstall, deploymentSourceKey } from "./deployment-panel.mjs";
import { gcpProjectFeedbackCurrent, gcpProjectBlueprint, gcpProjectBlueprintMatches } from "./gcp-preflight.mjs";
import { nativeTeamPackagePlan, setupNeedsApplicationBuild, setupProgress, setupProgressHasLifecycle, setupGuideResume, admitSetupGuideResume } from "./setup-progress.mjs";
import { RehearsalPanel } from "./rehearsal-panel.jsx";
import { ProviderSetup } from "./provider-setup.jsx";
import { hasForgeOpeningContext, isForgeOpening, ForgeWelcomeIntro, ForgeWelcomeNext, ForgeWelcomeOrientation, ForgeIllustrationMotionProvider, useForgeProposalNotice } from "./first-experience.jsx";
import { ForgeOrganizationStart, ForgeOrganizationContext } from "./forge-organization-start.jsx";
import { ForgeStartingHelp, ForgeStartingContext, ForgeStartingExample } from "./forge-starting-help.jsx";
import { startingContextDetails } from "./forge-starting-context.mjs";
import { ToolkitSavedSetup } from "./toolkit-saved-setup.jsx";
import { SavedSetupBuildStatus } from "./saved-setup-build-status.jsx";
import { captureSavedBuildNavigation, savedBuildNavigationMatches } from "./saved-build-navigation.mjs";
import { rehearsalNavigationIntent } from "./rehearsal-panel.mjs";
import { implementationCanDownload, implementationPackageUrl } from "./implementation-panel.mjs";
import { packagePreparationReason, PACKAGE_REVISION_PROMPT } from "./package-availability.mjs";
import {
  createGuideStorage,
  GUIDE_CREATION_KEY,
  guideCreationRequest,
  inputKey,
  inputStartingHelp,
  inputSubmissionText,
  readInput,
  writeInput,
  guideInputForState,
  admitNewGuideInput,
  recoverCompletedInput,
  admitSubmittedInput,
  manualKey,
  installSteps,
  unresolvedRequirements,
  implementationHandoff,
  currentGuideActivity,
  captureGuideResumeInput,
  preservedGuideResume,
  recoverGuideResumeInput,
  createGuideLoadGate,
  recoverMissingGuideSelection,
  guideOperationReadMatches,
} from "./wizard-state.mjs";
import "./guided.css";
import "./launch-design.css";
import "./first-experience.css";
import "./first-turn.css";

const narrowSetupQuery = "(max-width: 760px)";
const narrowSetupMatches = () => globalThis.matchMedia?.(narrowSetupQuery).matches === true;
function subscribeNarrowSetup(listener) {
  const media = globalThis.matchMedia?.(narrowSetupQuery);
  media?.addEventListener("change", listener);
  return () => media?.removeEventListener("change", listener);
}

export function GuideRecoveryNotice({ persistent }) {
  return persistent ? null : <p role="status">Browser recovery is unavailable. Your draft and choices remain in this open page. Saved guide responses are kept on the setup server.</p>;
}

export function SetupStartingPoints({ draft, disabled, onChoose, onReturnToTyping, showHelp = true, organization }) {
  const [preview, setPreview] = useState(null);
  if (organization) return <ForgeOrganizationStart organization={organization} draft={draft} disabled={disabled}
    onChoose={onChoose} onReturnToTyping={onReturnToTyping} />;
  const examples = [
    ["Stay on top of my projects", "I want help keeping track of my personal projects. Turn scattered notes into a short weekly check-in showing what changed, what needs my attention, and the next useful step. Help me work out which information it needs.", "A weekly check-in with clear next steps."],
    ["Keep our household in sync", "I want help preparing a simple daily household brief with upcoming school dates, family plans, and tasks that need attention. Ask who it should help and where we already keep this information.", "Today's plans, dates and things to remember."],
    ["Follow up on team requests", "I want help making sure requests to our team don't get lost. Keep track of what was asked, who is handling it, and what needs a follow-up. Help me work out how our team works before suggesting a setup.", "See who is handling what and what needs a reply."],
    ["Run a club with a simple member page", "I want a system for our club that helps members find upcoming activities, handles common questions, and passes decisions that need a person to our organizers. It should have a simple member page. Ask about our club, who will use it, and how we work before suggesting the team or technical setup.", "A member page, helpful answers and organizer handoffs."],
  ];
  const choose = (label, text) => {
    if (disabled) return;
    if (draft) setPreview({ label, text });
    else { setPreview(null); onChoose(text); }
  };
  const addInspiration = () => {
    if (disabled || !preview) return;
    onChoose(`${draft}\n\n${preview.text}`);
    setPreview(null);
  };
  const keepIdea = () => {
    if (disabled) return;
    setPreview(null);
    if (typeof onReturnToTyping === "function") onReturnToTyping();
  };
  return <div className="setup-starting-points">
    {showHelp && <div className="forge-starting-point-help"><span>Not sure yet?</span>
      <button type="button" disabled={disabled} onClick={() => choose("Find a starting point together",
        "Help me find a useful starting point. Ask me about my day-to-day work, what takes too much time, and who this would help before suggesting an agent system.")}>Find a starting point together <span aria-hidden="true">&#8594;</span></button>
    </div>}
    <span className="forge-examples-heading">A few starting points <small>{draft ? "Preview without changing your idea" : "Adds a draft you can make your own"}</small></span>
    {examples.map(([label, text, description]) => <button key={label} disabled={disabled}
      aria-label={`${draft ? "Preview" : "Use"} ${label}${draft ? "" : " as an editable starting point"}`}
      onClick={() => choose(label, text)}><span className="setup-starting-point-copy"><strong>{label}</strong><small>{description}</small></span><span className="forge-example-action" aria-hidden="true">{draft ? "Preview" : "Use idea"}<span>&#8594;</span></span></button>)}
    {preview && <details key={preview.label} open>
      <summary>Example: {preview.label}</summary>
      <p>{preview.text}</p>
      <small>Your description is still in the message box. Use this example as inspiration while editing it.</small>
      <div className="setup-choice-actions">
        <button type="button" disabled={disabled} onClick={addInspiration}>Add inspiration to my idea</button>
        <button type="button" disabled={disabled} onClick={keepIdea}>Keep my idea</button>
      </div>
    </details>}
  </div>;
}

export function setupGuideHelpMessage(question, selected, note) {
  const parts = [question.options?.length
    ? "Briefly recommend a sensible choice or combination and explain the main tradeoff for this decision."
    : "Help me describe this: ask for one concrete example and explain what detail would be useful."];
  const tentative = (question.options || []).filter(option => selected.includes(option.id));
  if (tentative.length || note.trim()) {
    parts.push("", "Tentative context for this question, not a confirmed answer:");
    if (tentative.length) parts.push("Options I am considering:",
      ...tentative.map(option => `- ${option.label}: ${option.description}`));
    if (note.trim()) parts.push("Full unsent note:", note);
    parts.push("Keep these choices proposed while explaining the tradeoffs. This is a discussion, not confirmation or permission to change tool grants.");
  }
  return parts.join("\n");
}

export function SetupGuideChoices({ question, selected, draft = "", helpDraft = draft, combineOptions, disabled, Icon,
  onSelect, onCombine, onAsk, showCaption = true }) {
  const combined = combineOptions === true && canCombineOptions(question);
  const multiple = question.multiple === true || combined;
  return <>
    {(showCaption || !multiple && question.options?.length > 0) && <div className="setup-choice-caption">
      {combined ? "Select the combination you want, then use Send answer below. Your guide will check how it fits together."
        : question.options?.length ? multiple ? "Select all that fit, then use Send answer below. You can ask for help first." : "Select what fits, then use Send answer below. You can ask for help first."
        : "Your own words are best here. Answer below."}
    </div>}
    <div className="setup-choice-grid" role="group" aria-label={question.title}>
      {(question.options || []).map((option, index) => <button key={option.id}
        className={`setup-choice ${selected.includes(option.id) ? "selected" : ""}`}
        disabled={disabled} aria-pressed={selected.includes(option.id)}
        onClick={() => onSelect(toggleAnswer(selected, option.id, multiple))}>
        <span className={`setup-choice-marker ${multiple ? "square" : ""}`}>
          {selected.includes(option.id) ? <Icon name="check" size={12} /> : multiple ? "" : index + 1}
        </span>
        <span><strong>{option.label}</strong><small>{option.description}</small></span>
      </button>)}
    </div>
    <div className="setup-choice-actions">
      <button className="setup-text-action" disabled={disabled} onClick={() => onAsk(setupGuideHelpMessage(question, selected, helpDraft))}>
        {question.options?.length ? "Help me decide" : "Help me describe it"}
      </button>
      {canCombineOptions(question) && <button className="setup-text-action" disabled={disabled}
        aria-pressed={combined} onClick={() => onCombine(!combined)}>
        {combined ? "Choose one instead" : "More than one fits"}
      </button>}
    </div>
  </>;
}

/** Navigate an existing progress action without admitting any new work. */
export function navigateSetupProgress(action, progress, { onTrial, onInstall, targets, revealTarget }) {
  if (!progress?.actions?.includes(action) || !["trial", "build", "deployment", "remote", "package"].includes(action.target)) return false;
  const target = targets[action.target]?.current;
  if (!target || typeof target.scrollIntoView !== "function") return false;
  if (action.target === "deployment" && ["native-team", "application"].includes(action.sourceKind))
    return typeof onInstall === "function" && onInstall(action.sourceKind);
  if (action.target === "trial" && action.mode) onTrial(action.mode);
  if (revealTarget) revealTarget(target);
  else target.scrollIntoView({ behavior: "smooth", block: "start" });
  return true;
}

/** Mirror the current trial navigation without starting or replacing a trial. */
export function SetupTrialShortcut({ progress, onNavigate, onDesign }) {
  const action = progress?.actions?.find((item) => item.target === "trial");
  return <a href="#setup-trial" title={action?.label || "Try the proposed agent design"}
    onClick={(event) => {
      event.preventDefault();
      if (action) onNavigate(action);
      else onDesign();
    }}>{action?.label || "Try design"}</a>;
}

export function SetupProgressCard({ progress, primary = false, resume, onResume, onNavigate, disabled, Icon }) {
  if (!progress) return null;
  const Heading = primary ? "h1" : "h2";
  const discovery = progress.kind === "discovery";
  return <section className="setup-progress-card" aria-label={discovery ? "Continue your setup" : "Current application status"}>
    <span className="setup-eyebrow">{discovery ? "SETUP CONVERSATION" : "CURRENT APPLICATION STATUS"}</span>
    <Heading>{progress.title}</Heading>
    <p>{progress.detail}</p>
    <div>{progress.actions.map(action => action.target === "console"
      ? <a key={action.target} href={action.url} target="_blank" rel="noreferrer">{action.label} <Icon name="arrow" size={14} /></a>
      : <button key={action.target} onClick={() => onNavigate(action)}>{action.label} <Icon name="arrow" size={14} /></button>)}
      {resume && <button type="button" disabled={disabled} onClick={() => onResume(resume)}>{discovery ? "Continue setup " : "Ask the guide what is next "}<Icon name="arrow" size={14} /></button>}
    </div>
  </section>;
}

/** Keep the current receipt projection visible above the retained conversation. */
export function SetupCurrentStatusAnchor({ progress, statusText, onNavigate, disabled, Icon }) {
  if (!progress || statusText !== progress.title) return null;
  const action = progress.actions?.[0];
  return <>
    <section className="setup-current-status" aria-label="Current application status">
      <div className="setup-current-status-copy">
        <span className="setup-eyebrow">CURRENT APPLICATION STATUS</span>
        <h2>{progress.title}</h2>
      </div>
      {action && (action.target === "console"
        ? <a href={action.url} target="_blank" rel="noreferrer" aria-disabled={disabled || undefined}
          onClick={event => {
            if (disabled || !event.currentTarget.isConnected || onNavigate?.(action) === false) event.preventDefault();
          }}>{action.label} <Icon name="arrow" size={14} /></a>
        : <button type="button" disabled={disabled} onClick={event => {
          if (!disabled && event.currentTarget.isConnected) onNavigate(action);
        }}>{action.label} <Icon name="arrow" size={14} /></button>)}
    </section>
    <div className="setup-conversation-history-boundary">
      <span>Conversation history</span>
      <small>Earlier messages are kept as written. Current status is above.</small>
    </div>
  </>;
}

/** Keep the saved question actionable without treating it as runtime progress. */
export function SetupGuideQuestionHeading({ question, lifecycle = false, waiting = false }) {
  const Heading = lifecycle ? "h2" : "h1";
  return <>
    {waiting ? <span className="setup-eyebrow">SAVED QUESTION</span>
      : lifecycle && <span className="setup-eyebrow">DESIGN CONVERSATION</span>}
    <Heading>{question.title}</Heading>
    {waiting && <p>Your guide is responding. Choices are paused; you can keep writing below.</p>}
    {question.description && <p>{question.description}</p>}
  </>;
}

export function SetupGuideStatusCaption({ lifecycle, statusText, ready, review }) {
  return <small>{lifecycle ? statusText : ready ? "Plan accepted"
    : review ? "Review your proposal" : "One decision at a time"}</small>;
}

function savedGuideFailure(state) {
  return ["failed", "interrupted"].includes(state?.operation?.status) ? state.operation : null;
}

function savedGuideResponse(state) {
  return ["completed", "failed", "interrupted"].includes(state?.operation?.status) ? state.operation : null;
}

function primaryGuideTranscript(scroll) {
  return scroll?.querySelector?.(':scope > [data-native-console-chat="true"]');
}

function guideActivityScrollTop(scroll) {
  const transcript = primaryGuideTranscript(scroll);
  if (!transcript) return null;
  return Math.max(0, Math.min(scroll.scrollHeight - scroll.clientHeight,
    scroll.scrollTop + transcript.getBoundingClientRect().bottom -
      scroll.getBoundingClientRect().top - scroll.clientHeight + 16));
}

export function guideStatusText({ state, error, appError, busy, ready, review, question, progress, agentTeamAvailable = true }) {
  if (error || appError) return "The guide needs your attention. Your saved setup is preserved.";
  if (busy) return state?.operation?.detail || "Your guide is checking the design. You can keep writing below.";
  const failure = savedGuideFailure(state);
  if (failure) return failure.detail || "The guide could not finish this response. Your saved setup is preserved. Retry when ready.";
  if (progress) return progress.title;
  if (ready) return agentTeamAvailable ? "Plan accepted. Download your agent team, or build a custom application."
    : "Plan accepted. Build your application to install it.";
  if (review) return "Your proposal is ready to review.";
  if (state?.status === "refining") return "Your updated draft is saved. Keep refining it, or explore the next step below.";
  return question?.title || "Your guide is ready.";
}

export function guideResponseSignal(state, busy, error) {
  return JSON.stringify([state?.id, state?.version, state?.operation?.request_id,
    state?.operation?.status, state?.operation?.started_at, busy, error]);
}

/** A saved assistant publication, distinct from terminal failure or native activity. */
export function guidePublicationKey(state) {
  const operation = state?.operation, message = state?.messages?.at(-1);
  const started = Date.parse(operation?.started_at), published = Date.parse(message?.created_at);
  if (!state?.id || operation?.status !== "completed" || !operation.request_id ||
      !Number.isInteger(operation.version) || !Number.isInteger(state.version) || state.version <= operation.version ||
      message?.role !== "assistant" || !message.id || typeof message.text !== "string" || !message.text.trim() ||
      !operation.request?.action || message.action !== operation.request.action ||
      !Number.isFinite(started) || !Number.isFinite(published) || published < started) return null;
  return JSON.stringify([state.id, message.id]);
}

/** Human navigation releases following; only an unseen saved publication creates a notice. */
export function createGuidePublicationUnread() {
  const seen = new Set();
  return {
    observe(state) {
      const key = guidePublicationKey(state);
      return key && !seen.has(key) ? key : null;
    },
    markSeen(state, expectedKey) {
      const key = guidePublicationKey(state);
      if (!key || key !== expectedKey) return false;
      seen.add(key);
      return true;
    },
  };
}

export function guideResponseTarget(state, { scroll, busy, failure, question, response }) {
  const operation = savedGuideFailure(state);
  if (!busy && operation) {
    const matches = failure?.dataset?.guideId === state.id &&
      failure?.dataset?.requestId === operation.request_id &&
      failure?.dataset?.operationStatus === operation.status &&
      (failure?.dataset?.operationStartedAt || "") === (operation.started_at || "");
    return matches ? failure : response;
  }
  const latestAssistant = [...(primaryGuideTranscript(scroll)?.querySelectorAll(
    ".cc-conversation-turn:last-child .cc-message--assistant",
  ) || [])].at(-1);
  return latestAssistant || response;
}

function currentGuideQuestion(state, question) {
  return state?.question && question?.isConnected !== false && question?.dataset?.guideId === state.id &&
    question.dataset.guideVersion === String(state.version);
}

/** The question and its first choice must actually fit in this conversation viewport. */
export function guideQuestionVisible(state, { scroll, question }) {
  if (!currentGuideQuestion(state, question) || !(scroll?.clientHeight > 0) || !(scroll.clientWidth > 0)) return false;
  const viewport = scroll.getBoundingClientRect(), heading = question.getBoundingClientRect();
  const choice = question.querySelector?.(".setup-choice")?.getBoundingClientRect() || heading;
  return heading.height > 0 && choice.height > 0 && heading.top >= viewport.top &&
    choice.bottom <= viewport.top + scroll.clientHeight;
}

/** Long automatic replies leave context above the decision; explicit reading still starts at the reply. */
export function guideResponseScrollTop(state, targets) {
  const { scroll, question, response, preferQuestion } = targets;
  const target = guideResponseTarget(state, { ...targets, busy: false });
  if (!scroll || !target) return null;
  const viewport = scroll.getBoundingClientRect(), reply = target.getBoundingClientRect();
  let top = scroll.scrollTop + reply.top - viewport.top - 16;
  if (preferQuestion && !savedGuideFailure(state) && target !== response &&
      currentGuideQuestion(state, question) && scroll.clientHeight > 0 && scroll.clientWidth > 0) {
    const heading = question.getBoundingClientRect();
    const choice = question.querySelector?.(".setup-choice")?.getBoundingClientRect() || heading;
    // Short replies keep their opening visible. Decisions below a long reply need their own arrival position.
    if (heading.height > 0 && choice.height > 0 && choice.bottom - reply.top + 16 > scroll.clientHeight - 24) {
      const context = scroll.clientWidth <= 520 ? 16 : Math.min(160, scroll.clientHeight / 3);
      const inset = Math.max(16, Math.min(context, scroll.clientHeight - 24 - (choice.bottom - heading.top)));
      top = scroll.scrollTop + heading.top - viewport.top - inset;
    }
  }
  return Math.max(0, Math.min(scroll.scrollHeight - scroll.clientHeight, top));
}

export function guidePublicationVisible(state, targets) {
  const key = guidePublicationKey(state), { scroll, response } = targets;
  if (!key || !scroll || response?.dataset?.guideId !== state.id || response.dataset.guidePublication !== key) return false;
  const target = guideResponseTarget(state, { ...targets, busy: false });
  if (!target || target === response || target.isConnected === false) return false;
  const top = target.getBoundingClientRect().top, viewport = scroll.getBoundingClientRect();
  return Number.isFinite(top) && top >= viewport.top && top < viewport.top + scroll.clientHeight;
}

export function GuideResponseNotice({ state, publicationKey, busy, onReveal, Icon }) {
  if (busy || !publicationKey || publicationKey !== guidePublicationKey(state)) return null;
  return <button type="button" className="setup-new-response" onClick={() => onReveal(publicationKey)}>
    New response <Icon name="arrow" size={14} />
  </button>;
}

/** Keep the revealed guide response in view while its own transcript restores. */
export function createGuideResponseFollow() {
  let selected = null, released = null, preferQuestion = false;
  const key = (state) => savedGuideResponse(state) && state?.id && state.operation.request_id
    ? guideResponseSignal(state, false, "") : null;
  return {
    select(state, targets, explicit = false) {
      const next = key(state);
      if (!next || !guideResponseTarget(state, { ...targets, busy: false }) ||
          released === next && !explicit) return false;
      selected = next;
      released = null;
      preferQuestion = !explicit && targets.preferQuestion === true;
      return true;
    },
    release(state) {
      const next = key(state);
      const following = Boolean(selected || next);
      selected = null;
      released = next;
      return following;
    },
    maintain(state, targets) {
      const { scroll } = targets;
      if (!selected || selected !== key(state) || !scroll) return false;
      const top = guideResponseScrollTop(state, { ...targets,
        preferQuestion: preferQuestion && targets.preferQuestion !== false });
      if (top === null) return false;
      if (Math.abs(top - scroll.scrollTop) < 1) return false;
      scroll.scrollTo({ top, behavior: "auto" });
      return true;
    },
  };
}

export function SetupOperationFailure({ state, busy, failureRef, onRetry, onReviewDraft, reviewDraftDisabled }) {
  const operation = savedGuideFailure(state);
  if (!operation || busy) return null;
  return <div className="setup-error setup-operation-failure" role="alert" ref={failureRef} tabIndex={-1}
    data-guide-id={state.id} data-request-id={operation.request_id} data-operation-status={operation.status}
    data-operation-started-at={operation.started_at || ""}>
    <strong>Your submitted answer is saved</strong>
    <p>{operation.detail}</p>
    <blockquote>{operation.request?.text || operation.request?.selected?.join(", ")}</blockquote>
    <div className="setup-operation-failure-actions">
      <button onClick={onRetry}>Retry saved answer</button>
      {onReviewDraft && <button type="button" disabled={reviewDraftDisabled} onClick={onReviewDraft}>Review your draft</button>}
    </div>
    {onReviewDraft && <p className="setup-operation-failure-note">Retry uses the saved answer. Your current writing stays separate.</p>}
  </div>;
}

export { nativeTeamPackagePlan } from "./setup-progress.mjs";

export function nativeTeamPackageKey(state) {
  const plan = nativeTeamPackagePlan(state);
  return plan ? JSON.stringify([state.id, state.version, plan, state.brief]) : null;
}

export function setupProposalReviewable(state) {
  if (!state?.id || !Number.isInteger(state.version) || !state.proposal || state.question != null || state.operation?.status === "running") return false;
  return state.status === "review" || state.status === "refining" && state.readiness?.context_ready === true &&
    Array.isArray(state.readiness?.decision_blockers) && state.readiness.decision_blockers.length === 0;
}

function PackageInstallSteps({ plan }) {
  return <ol className="setup-install-steps">{installSteps(plan).map((step, index) => <li key={index}>
    <strong>{step.title}</strong>
    {step.commands?.map((command) => <code key={command}>{command}</code>)}
    {step.detail && <span>{step.detail}</span>}
  </li>)}</ol>;
}

export function NativeTeamPackage({ state, packageBusy, blocked, onPackage, onInstall, Icon, targetRef, downloadedKey,
  deploymentObservation = "known", onRevise }) {
  const plan = nativeTeamPackagePlan(state);
  if (!plan) return null;
  const proposed = state.status !== "ready" || !!state.proposal;
  const remaining = Array.isArray(state.readiness?.implementation_blockers) ? state.readiness.implementation_blockers : [];
  const optional = remaining.length > 0;
  const installationKnown = deploymentObservation === "known";
  const packageReason = packagePreparationReason(state, plan);
  const content = <section className="setup-review-card setup-team-package" aria-label="Agent team package" ref={optional ? undefined : targetRef}>
    <span className="setup-eyebrow">{proposed ? "CURRENT PROPOSED PLAN" : "ACCEPTED PLAN"}</span>
    <h2>{packageReason ? "Your agent team design" : "Use your agent team"}</h2>
    {packageReason ? <div role="status"><p>{packageReason}</p>
      <p>Your design and choices are kept. You can continue refining them with your guide.</p>
      {onRevise && <button className="setup-action" disabled={blocked}
        onClick={() => onRevise(PACKAGE_REVISION_PROMPT, { id: state.id, version: state.version })}>Revise with guide <Icon name="arrow" size={16} /></button>}
    </div> : <><p>Use the Console with your agents, configured tools and permitted notes. Download this package without waiting for a custom application build.</p>
    {proposed && <p>This exports the current proposal, including its proposed defaults. Downloading does not accept the plan.</p>}</>}
    {nativeTeamCanInstall(state) && onInstall && <button className="setup-action primary" disabled={blocked}
      onClick={() => onInstall(state)}>{installationKnown ? "Install agent team" : "Review installation status"} <Icon name="arrow" size={16} /></button>}
    <button className={`setup-action${nativeTeamCanInstall(state) && onInstall ? "" : " primary"}`} disabled={blocked || !!packageReason} onClick={() => { if (!packagePreparationReason(state, plan)) onPackage(true, "native", state); }}>
      {packageBusy ? "Preparing package..." : "Download agent team package"} <Icon name="download" size={16} />
    </button>
    {downloadedKey && downloadedKey === nativeTeamPackageKey(state) && <small className="setup-download-success" role="status"><Icon name="check" size={13} /> Package download started</small>}
    {!packageReason && <p className="setup-manual-note">{nativeTeamCanInstall(state)
      ? installationKnown ? "Check your destination and private setup, or download the package for the manual steps below."
        : "Check the current installation status below. You can also download the package for the manual steps below."
      : "Install the downloaded package using the steps below."} Credentials and connection prerequisites are set up on the destination computer. The agent team package does not implement remaining custom application work.</p>}
    {remaining.length > 0 && <details className="setup-team-remaining">
      <summary>Remaining implementation work</summary>
      <ul>{remaining.map((detail, index) => <li key={index}>{detail}</li>)}</ul>
    </details>}
    {!packageReason && <details className="setup-team-install"><summary>Install and start the package</summary>
      <PackageInstallSteps plan={plan} />
    </details>}
  </section>;
  return optional ? <details className="setup-manual-export" ref={targetRef}>
    <summary>Agent team package (optional)</summary>
    {content}
  </details> : content;
}

export function ManualSetupExports({ plan, showPackage, checkedApplication, packageBusy, blocked,
  downloaded, saving, onPackage, onBrief, onSave, onAdjust, Icon, briefBlocked = blocked }) {
  return <details className="setup-manual-export">
    <summary>Manual exports and terminal setup (optional)</summary>
    <p>{showPackage ? "Inspect your setup files, save a revision or download the implementation brief here."
      : "Download the implementation brief to keep your current plan for reference."}</p>
    {showPackage && <div className="setup-review-card">
      <span className="setup-eyebrow">{checkedApplication ? "CHECKED APPLICATION PACKAGE" : "NATIVE SETUP ONLY"}</span>
      <p>{checkedApplication
        ? <>The checked package for <strong>{plan.project.name}</strong> includes the authored application and native setup. Its remaining integrations still need destination verification.</>
        : <>The native setup for <strong>{plan.project.name}</strong> contains the team, runtime host and configuration. It does not include the builder's custom application code.</>}</p>
      <div className="setup-package-tags"><code>install.sh</code><code>mob.toml</code><code>README.md</code>
        {plan.target === "gcp" && <code>deploy-gcp.sh</code>}
      </div>
      <button className="setup-action" onClick={() => onPackage(true)} disabled={blocked}>
        {packageBusy ? "Preparing package..." : checkedApplication ? "Download checked application" : "Download native setup only"}
        <Icon name="download" size={16} />
      </button>
      {downloaded && <small className="setup-download-success"><Icon name="check" size={13} /> Package download started</small>}
      <p className="setup-manual-note">This checklist follows the plan. Use the downloaded package's README for its application-specific setup.</p>
      <PackageInstallSteps plan={plan} />
      <div className="setup-package-links">
        <button onClick={() => onPackage(false)} disabled={blocked}>Inspect native setup files</button>
        <button onClick={onSave} disabled={blocked || saving}>{saving ? "Saving..." : "Save revision"}</button>
        <button onClick={onAdjust} disabled={blocked}>Adjust details</button>
      </div>
    </div>}
    <p>The implementation brief records your current understanding, native setup and remaining integrations for reference.</p>
    <button className="setup-action" onClick={onBrief} disabled={briefBlocked}>Download implementation brief <Icon name="download" size={16} /></button>
  </details>;
}

function GuideAvatar() {
  return (
    <span className="setup-avatar">
      <img src="/assets/meerkat-transparent.png" alt="" />
    </span>
  );
}
function AgentMessage({ children, className = "" }) {
  return (
    <div className={`setup-agent-message ${className}`}>
      <GuideAvatar />
      <div className="setup-agent-body">{children}</div>
    </div>
  );
}

export function GuidedSetup({
  active,
  bp,
  onBlueprint,
  onAdvanced,
  manualReturn,
  config,
  Icon,
  onSave,
  saving,
  appError,
  appToast,
  onDismissError,
}) {
  const [state, setState] = useState(null),
    [loadError, setLoadError] = useState("");
  const [providerOpen, setProviderOpen] = useState(false);
  const narrow = useSyncExternalStore(subscribeNarrowSetup, narrowSetupMatches, () => false);
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);
  const [composerFocused, setComposerFocused] = useState(false);
  const [startingHelpEdit, setStartingHelpEdit] = useState(null);
  const mobileMenuButton = useRef(null);
  const [savedToolsGuide, setSavedToolsGuide] = useState(null);
  const [deploymentNavigation, setDeploymentNavigation] = useState(null);
  const [providerDismissed, setProviderDismissed] = useState(false);
  const [guideStorage] = useState(() => createGuideStorage(() => globalThis.localStorage));
  const [creationStorage] = useState(() => createGuideStorage(() => globalThis.sessionStorage));
  const draftRecovery = useSyncExternalStore(guideStorage.subscribe, guideStorage.persistent, guideStorage.persistent);
  const creationRecovery = useSyncExternalStore(creationStorage.subscribe, creationStorage.persistent, creationStorage.persistent);
  const browserRecovery = draftRecovery && creationRecovery;
  const [input, setInput] = useState(() => readInput(guideStorage, null));
  const attachments = useGuideAttachments(state?.id, state?.input_surface);
  const [attachmentPreparation, setAttachmentPreparation] = useState(null);
  const attachmentPicker = useRef(null), preparingAttachments = useRef(null);
  const { selected, draft, mode: composeMode, combine_options: combineOptions } = input;
  const setDraft = (value) =>
    setInput((previous) => {
      const next = { ...previous, draft: typeof value === "function" ? value(previous.draft) : value };
      if (!inputStartingHelp(next)) delete next.starting_help;
      return next;
    });
  const setSelected = (value) =>
    setInput((previous) => ({
      ...previous,
      selected: typeof value === "function" ? value(previous.selected) : value,
    }));
  const setComposeMode = (mode) =>
    setInput((previous) => ({ ...previous, mode }));
  const [setups, setSetups] = useState([]),
    [historyOpen, setHistoryOpen] = useState(false);
  const [setupsRead, setSetupsRead] = useState("idle");
  const [setupsPage, setSetupsPage] = useState(0);
  const setupsReading = useRef(false), setupsRequest = useRef(0), setupsMounted = useRef(true);
  useEffect(() => {
    setupsMounted.current = true;
    return () => { setupsMounted.current = false; setupsRequest.current += 1; };
  }, []);
  const [unseenResponse, setUnseenResponse] = useState(null);
  const [questionInView, setQuestionInView] = useState(null);
  const [following, setFollowing] = useState(true);
  const [busy, setBusy] = useState(false),
    [busyAction, setBusyAction] = useState("");
  const [, redrawApplyStatus] = useState(0);
  const [, redrawSendStatus] = useState(0);
  const [creationCheckDetail, setCreationCheckDetail] = useState("");
  const [loadingSetup, setLoadingSetup] = useState(false);
  const [failedSelection, setFailedSelection] = useState(null);
  const [error, setError] = useState(""),
    [manualNote, setManualNote] = useState(null),
    [pendingMessage, setPendingMessage] = useState(null);
  // A product-launched Forge offers no agent team package: Build application is its path.
  const agentTeamAvailable = config?.capabilities?.agent_team?.available !== false;
  const [runtime, setRuntime] = useState(config?.architect ?? null),
    [mobilePanel, setMobilePanel] = useState("chat");
  const latestMobilePanel = useRef(mobilePanel);
  latestMobilePanel.current = mobilePanel;
  const runtimeGeneration = useRef(0);
  const publishRuntime = useCallback((status) => {
    runtimeGeneration.current += 1;
    setRuntime(status);
  }, []);
  async function refreshRuntime(navigationToken, guideId) {
    const generation = ++runtimeGeneration.current;
    try {
      const status = await request("/api/architect/status", { timeoutMs: 30000 });
      if (setupsMounted.current && runtimeGeneration.current === generation &&
          loadGate.current.current(navigationToken) && latestState.current?.id === guideId) publishRuntime(status);
    } catch { /* Guide and installed-app readback remain independent of setup-team status. */ }
  }
  const [validation, setValidation] = useState(null),
    [downloaded, setDownloaded] = useState(false),
    [nativeDownloadedKey, setNativeDownloadedKey] = useState(null),
    [packageBusy, setPackageBusy] = useState(false);
  const [implementationSnapshot, setImplementationSnapshot] = useState(null);
  const latestBuildSnapshot = useRef(implementationSnapshot);
  latestBuildSnapshot.current = implementationSnapshot;
  const [implementationAdmission, setImplementationAdmission] = useState(null);
  const [deploymentSnapshot, setDeploymentSnapshot] = useState(null);
  const [remoteDeploymentSnapshot, setRemoteDeploymentSnapshot] = useState(null);
  const [rehearsal, setRehearsal] = useState(null);
  const [actionsExpansion, setActionsExpansion] = useState(null);
  const [savedBuildReturn, setSavedBuildReturn] = useState(null);
  const savedBuildReturnRef = useRef(null);
  const [trialNavigation, setTrialNavigation] = useState(null);
  const [rehearsalActivity, setRehearsalActivity] = useState(null);
  const [configOpen, setConfigOpen] = useState(false),
    [configDraft, setConfigDraft] = useState(""),
    [configError, setConfigError] = useState(""),
    [configChecking, setConfigChecking] = useState(false);
  const configEditor = useRef(null), configReview = useRef(null), latestManualNote = useRef(manualNote);
  latestManualNote.current = manualNote;
  const [filesOpen, setFilesOpen] = useState(false),
    [selectedFile, setSelectedFile] = useState("");
  const editorBase = useRef(null),
    initialBlueprint = useRef(clone(bp)),
    handledReturn = useRef(null),
    latestState = useRef(state),
    processing = useRef(false),
    applyRequests = useRef(new Map()),
    sendRequests = useRef(new Map()),
    sendingToken = useRef(null),
    packagePreparing = useRef(false),
    countPreparing = useRef(null);
  const chatScroll = useRef(null),
    conversationRef = useRef(null),
    questionRef = useRef(null),
    trialRef = useRef(null),
    teamPackageRef = useRef(null),
    implementationRef = useRef(null),
    deploymentRef = useRef(null),
    remoteDeploymentRef = useRef(null),
    responseRef = useRef(null),
    operationFailureRef = useRef(null),
    inputRef = useRef(null),
    latestInput = useRef(input),
    forceFollow = useRef(false),
    responseSignal = useRef("");
  latestInput.current = input;
  latestState.current = state;
  const loadGate = useRef(null), latestLifecycle = useRef(null), reconnectError = useRef(null), responseFollow = useRef(null), publicationUnread = useRef(null);
  if (!loadGate.current) loadGate.current = createGuideLoadGate();
  const lifecycleSelection = useRef(null);
  if (!lifecycleSelection.current) lifecycleSelection.current = {
    token: () => loadGate.current.token(),
    capture(guide, expectedToken) {
      const token = loadGate.current.token(), identity = guide?.id;
      if (loadGate.current.pending() || expectedToken !== undefined && !loadGate.current.current(expectedToken) ||
          latestState.current?.id !== identity) return null;
      return () => !loadGate.current.pending() && loadGate.current.current(token) && latestState.current?.id === identity;
    },
  };
  const lifecycleToken = loadGate.current.token();
  const buildObservationCurrent = implementationSnapshot?.guideId === state?.id &&
    implementationSnapshot?.selectionToken === lifecycleToken && !loadGate.current.pending();
  // Retain saved source for its existing consumers while observation is rechecked.
  const implementation = implementationSnapshot?.receipt ?? null;
  const buildObservation = buildObservationCurrent ? implementationSnapshot.observation : "loading";
  const buildAdmissionPending = buildObservationCurrent &&
    implementationSnapshot.pendingAdmissionVersion === state?.version;
  const deploymentObservationCurrent = deploymentSnapshot?.guideId === state?.id &&
    deploymentSnapshot?.selectionToken === lifecycleToken && !loadGate.current.pending();
  const deployment = deploymentSnapshot?.receipt ?? null;
  const deploymentObservation = deploymentObservationCurrent ? deploymentSnapshot.observation : "loading";
  const remoteObservationCurrent = remoteDeploymentSnapshot?.guideId === state?.id &&
    remoteDeploymentSnapshot?.selectionToken === lifecycleToken && !loadGate.current.pending();
  const remoteDeployment = state?.id && remoteDeploymentSnapshot?.guideId === state.id ? remoteDeploymentSnapshot.receipt : null;
  const remoteObservation = remoteObservationCurrent ? remoteDeploymentSnapshot.observation : "loading";
  const latestImplementation = useRef(implementation);
  latestImplementation.current = implementation;
  const publishLifecycle = useCallback((setter, value) => {
    if (!loadGate.current.pending() && loadGate.current.current(lifecycleToken) && latestState.current?.id === state?.id)
      setter(value);
  }, [lifecycleToken, state?.id]);
  const acceptImplementationReceipt = useCallback((receipt, snapshot) => {
    if (snapshot?.guideId === state?.id && snapshot.selectionToken === lifecycleToken)
      publishLifecycle(setImplementationSnapshot, { ...snapshot, receipt });
  }, [publishLifecycle, state?.id, lifecycleToken]);
  const acceptImplementationAdmission = useCallback(value => publishLifecycle(setImplementationAdmission, value), [publishLifecycle]);
  const acceptDeploymentReceipt = useCallback((receipt, snapshot) => {
    if (snapshot?.guideId === state?.id && snapshot.selectionToken === lifecycleToken)
      publishLifecycle(setDeploymentSnapshot, { ...snapshot, receipt });
  }, [publishLifecycle, state?.id, lifecycleToken]);
  const acceptRemoteDeploymentReceipt = useCallback((receipt, snapshot) => {
    if (snapshot?.guideId === state?.id && snapshot.selectionToken === lifecycleToken)
      publishLifecycle(setRemoteDeploymentSnapshot, previous => previous?.guideId === snapshot.guideId &&
        previous.selectionToken === snapshot.selectionToken && previous.observation === snapshot.observation && previous.receipt === receipt
        ? previous : { ...snapshot, receipt });
  }, [publishLifecycle, state?.id, lifecycleToken]);
  const acceptRehearsalReceipt = useCallback(value => publishLifecycle(setRehearsal, value), [publishLifecycle]);
  const acceptApplicationDownloaded = useCallback(() => publishLifecycle(setDownloaded, true), [publishLifecycle]);
  const acceptRehearsalActivity = useCallback(activity => publishLifecycle(setRehearsalActivity,
    { guideId: state?.id, guideVersion: state?.version, activity }), [publishLifecycle, state?.version]);
  if (!responseFollow.current) responseFollow.current = createGuideResponseFollow();
  if (!publicationUnread.current) publicationUnread.current = createGuidePublicationUnread();
  const attachmentPreparing = attachmentPreparation !== null && attachmentPreparation === loadGate.current.token();
  const applyingPlan = applyRequests.current.get(state?.id);
  const applyUnconfirmed = !!applyingPlan;
  const sendingMessage = sendRequests.current.get(state?.id || "new");
  const selectedCreationRequest = creationStorage.getItem(GUIDE_CREATION_KEY);
  const creationUnconfirmed = !state?.id && selectedCreationRequest !== null &&
    (!sendingMessage || sendingMessage.status === "unconfirmed");
  const sendUnconfirmed = sendingMessage?.status === "unconfirmed" || creationUnconfirmed;
  latestLifecycle.current = { receipts: { build: implementation, trial: rehearsal, deployment, remoteDeployment },
    flags: { active, pendingChanges: !!manualNote, designBusy: busy || applyUnconfirmed || sendUnconfirmed, packageBusy, buildObservation, buildAdmissionPending, deploymentObservation, remoteObservation, agentTeamAvailable } };
  const timeline = useGuideTimeline(active ? state : null);

  function savedManual(id) {
    try {
      return (
        JSON.parse(guideStorage.getItem(manualKey(id))) ||
        pendingManual(guideStorage, id)
      );
    } catch {
      return null;
    }
  }
  function storeManual(id, feedback) {
    if (feedback) guideStorage.setItem(manualKey(id), JSON.stringify(feedback));
    else guideStorage.removeItem(manualKey(id));
    const legacy = pendingManual(guideStorage, id);
    if (legacy) guideStorage.removeItem(GUIDE_MANUAL_KEY);
  }
  function manualRequest(note) {
    return note.manual_intent
      ? { manual_intent: note.manual_intent, text: note.text }
      : { blueprint: note.blueprint, text: note.text };
  }
  function matchesManual(note, submitted, requestId) {
    if (!note || submitted?.action !== "manual") return false;
    const extra = manualRequest(note);
    if (note.submitted_request_id && requestId)
      return note.submitted_request_id === requestId && submitted.text === extra.text;
    return submitted.text === extra.text && canonicalJson(submitted.manual_intent ?? null) === canonicalJson(extra.manual_intent ?? null) &&
      canonicalJson(submitted.blueprint ?? null) === canonicalJson(extra.blueprint ?? null);
  }
  function bindAdmittedManual(current, snapshot, submitted, next) {
    const operation = next.operation;
    if (!matchesManual(snapshot, submitted) || next.id !== current.id ||
        latestState.current?.id !== current.id || latestState.current?.version !== current.version ||
        operation?.status !== "running" || operation.version !== current.version ||
        operation.request?.version !== current.version || operation.request?.action !== "manual" ||
        operation.request.text !== submitted.text || !/^[a-f0-9]{64}$/.test(operation.request_id) ||
        (!!operation.request.manual_intent !== !!submitted.manual_intent) ||
        (!!operation.request.blueprint !== !!submitted.blueprint)) return;
    const pending = savedManual(current.id);
    if (canonicalJson(pending) !== canonicalJson(snapshot) || canonicalJson(latestManualNote.current) !== canonicalJson(snapshot)) return;
    const bound = { ...pending, submitted_request_id: operation.request_id };
    storeManual(current.id, bound);
    latestManualNote.current = bound;
    setManualNote(bound);
  }
  function clearSubmittedManual(id, submitted, requestId) {
    const pending = savedManual(id);
    if (!matchesManual(pending, submitted, requestId)) return false;
    storeManual(id, null);
    if (latestState.current?.id === id && canonicalJson(latestManualNote.current) === canonicalJson(pending)) {
      latestManualNote.current = null;
      setManualNote(null);
    }
    return true;
  }
  function recoveredManual(note, current) {
    if (!note || note.manual_intent || note.review_unavailable) return note;
    const operation = current?.operation;
    if (matchesManual(note, operation?.request, operation?.request_id)) return note;
    const origin = note.review_guide;
    if (note.review_base || origin) {
      if (!note.review_base || !origin || !Number.isInteger(origin.version) || origin.version < 0 ||
          (origin.id !== current?.id && !(origin.id === null && origin.version === 0)))
        return { ...note, review_unavailable: true };
      if (origin.version !== current?.version || canonicalJson(note.review_base) !== canonicalJson(guidePlan(current))) {
        const retained = { ...note, manual_intent: { base: clone(note.review_base), draft: clone(note.blueprint) },
          proposal_changed: true };
        delete retained.submitted_request_id;
        return retained;
      }
    } else if (operation?.status === "completed") {
      // An unmatched older snapshot has no exact edit base for a full-plan replay.
      return { ...note, review_unavailable: true };
    }
    return note;
  }
  function proposalPlan(note, current) {
    return (note?.manual_intent || note?.review_unavailable ? null : note?.blueprint) || guidePlan(current);
  }
  function visibleManualPlan(note, current) {
    // Manual editing keeps its internal starter even before a proposal exists.
    return proposalPlan(note, current) || initialBlueprint.current;
  }
  async function refreshSetups({ ifIdle = false } = {}) {
    if (!setupsMounted.current || ifIdle && setupsReading.current) return;
    const ticket = ++setupsRequest.current;
    setupsReading.current = true;
    setSetupsRead("loading");
    try {
      const result = await request("/api/guide/list", { timeoutMs: 30000 });
      if (!setupsMounted.current || ticket !== setupsRequest.current) return;
      if (!Array.isArray(result?.setups)) throw new Error("Saved setup list is unavailable.");
      setSetups(result.setups);
      setSetupsPage(value => Math.min(value, Math.max(0, Math.ceil(result.setups.length / 8) - 1)));
      setSetupsRead("ready");
    } catch {
      if (setupsMounted.current && ticket === setupsRequest.current) setSetupsRead("failed");
    } finally {
      if (setupsMounted.current && ticket === setupsRequest.current) setupsReading.current = false;
    }
  }
  function acknowledgeOperation(next) {
    const operation = next.operation;
    if (operation?.status !== "completed") return false;
    clearSubmittedManual(next.id, operation.request, operation.request_id);
    const key = `mobkit-setup-completed-v1:${next.id}`;
    if (guideStorage.getItem(key) === operation.request_id) return false;
    if (!preservedGuideResume(guideStorage, next)) attachments.completed(next.id, operation);
    const restored = operation.request?.action === "manual"
      ? readInput(guideStorage, next) : recoverCompletedInput(guideStorage, next);
    writeInput(guideStorage, restored);
    guideStorage.setItem(key, operation.request_id);
    return true;
  }
  function releaseApply(owner) {
    if (applyRequests.current.get(owner.id) !== owner) return false;
    applyRequests.current.delete(owner.id);
    if (processing.current === owner) processing.current = false;
    if (setupsMounted.current) redrawApplyStatus(value => value + 1);
    return true;
  }
  function releaseSend(owner) {
    if (sendRequests.current.get(owner.id || "new") !== owner) return false;
    sendRequests.current.delete(owner.id || "new");
    if (processing.current === owner) processing.current = false;
    if (setupsMounted.current) redrawSendStatus(value => value + 1);
    return true;
  }
  function reconcileSend(next) {
    const owner = sendRequests.current.get(next.id);
    if (!owner?.request) return false;
    if (guideAdmissionMatches(owner, next)) {
      if (!owner.preserveInput && next.operation.status === "completed") {
        // The saved receipt owns only the submitted writing, choices and files.
        // Later edits remain under the original question's draft key.
        const source = latestInput.current.key === inputKey(owner.state)
          ? latestInput.current : readInput(guideStorage, owner.state);
        const admitted = admitSubmittedInput(source, owner.request);
        writeInput(guideStorage, admitted);
        if (latestInput.current.key === admitted.key) {
          latestInput.current = admitted;
          setInput(admitted);
        }
        if (owner.request.attachments?.length)
          attachments.admitted(next.id, owner.request.attachments, owner.request);
        if (owner.request.action === "manual")
          bindAdmittedManual(owner.state, owner.submittedManual, owner.request, next);
      }
      releaseSend(owner);
      return false;
    }
    if (Number.isSafeInteger(next.version) && next.version > owner.request.version) {
      // The original version can no longer admit a write. This does not prove
      // that our Send was admitted, so leave its draft intact.
      releaseSend(owner);
      if (next.operation?.status === "completed" && typeof next.operation.request_id === "string")
        guideStorage.setItem(`mobkit-setup-completed-v1:${next.id}`, next.operation.request_id);
      return true;
    }
    owner.status = "unconfirmed";
    if (processing.current === owner) processing.current = false;
    redrawSendStatus(value => value + 1);
    return true;
  }
  function acceptState(next) {
    const applying = applyRequests.current.get(next.id);
    // A newer saved version excludes the original expected-version write.
    if (applying && Number.isInteger(next.version) && next.version > applying.version) releaseApply(applying);
    else if (applying) applying.checked = next.version === applying.version && setupProposalReviewable(next) &&
      canonicalJson(next.proposal) === applying.proposal;
    const unmatchedSend = reconcileSend(next);
    const newlyCompleted = unmatchedSend ? false : acknowledgeOperation(next);
    latestState.current = next;
    setState(next);
    const saved = savedManual(next.id), pending = recoveredManual(saved, next);
    if (canonicalJson(saved) !== canonicalJson(pending)) storeManual(next.id, pending);
    latestManualNote.current = pending;
    setManualNote(pending);
    onBlueprint(visibleManualPlan(pending, next));
    if (next.operation?.status === "running") {
      setBusy(true);
      setBusyAction(next.operation.request?.action || "answer");
    } else {
      setBusy(false);
      setBusyAction("");
    }
    return newlyCompleted;
  }
  async function load(id, { refreshRuntimeStatus = true } = {}) {
    savedBuildReturnRef.current = null;
    setSavedBuildReturn(null);
    writeInput(guideStorage, latestInput.current);
    closeConfig();
    setStartingHelpEdit(null);
    const token = loadGate.current.begin();
    const applying = processing.current;
    if (applying && (applyRequests.current.get(applying.id) === applying ||
        sendRequests.current.get(applying.id || "new") === applying)) {
      processing.current = false;
      if (sendRequests.current.get(applying.id || "new") === applying) applying.status = "unconfirmed";
    }
    setLoadingSetup(true);
    setLoadError("");
    setFailedSelection(null);
    const requested = typeof id === "string" ? id : guideStorage.getItem(GUIDE_KEY);
    const unconfirmed = applyRequests.current.get(requested);
    if (unconfirmed) unconfirmed.checked = false;
    try {
      let next;
      const saved = requested;
      if (saved) {
        try {
          next = await request(`/api/guide/${encodeURIComponent(saved)}`, { timeoutMs: 30000 });
        } catch (e) {
          if (e.status !== 404) throw e;
          if (!recoverMissingGuideSelection(guideStorage, GUIDE_KEY, saved, loadGate.current, token)) return;
        }
      }
      if (!loadGate.current.current(token)) return;
      next ||= await request("/api/guide/new", { timeoutMs: 30000 });
      if (!loadGate.current.current(token)) return;
      setError("");
      const newlyCompleted = acceptState(next);
      setInput(previous => guideInputForState(guideStorage, next, previous, { newlyCompleted }));
      if (refreshRuntimeStatus) void refreshRuntime(token, next.id);
      refreshSetups();
      return next;
    } catch (e) {
      if (!loadGate.current.current(token)) return;
      setLoadError(e.message);
      setFailedSelection({ id: requested, token });
      if (latestState.current) setError(e.message);
    } finally {
      if (loadGate.current.finish(token)) setLoadingSetup(false);
    }
  }
  useEffect(() => {
    load(undefined, { refreshRuntimeStatus: false });
  }, []);
  useEffect(() => {
    if (state?.id) guideStorage.setItem(GUIDE_KEY, state.id);
  }, [state?.id]);
  useEffect(() => {
    if (loadingSetup || loadGate.current.pending() || !state || input.key === inputKey(state)) return;
    setInput(previous => guideInputForState(guideStorage, state, previous));
  }, [loadingSetup, state?.id, state?.question, input.key]);
  useEffect(() => {
    if (!state || input.key === inputKey(state))
      writeInput(guideStorage, input);
  }, [input, state?.id, state?.question]);
  useEffect(() => {
    if (loadingSetup || state?.operation?.status !== "running" || !state.id) return;
    let stopped = false, reading = false;
    const navigationToken = loadGate.current.token();
    const watched = { guide_id: state.id, guide_version: state.version,
      request_id: state.operation.request_id, operation_version: state.operation.version };
    const timer = setInterval(async () => {
      if (stopped || reading) return;
      reading = true;
      try {
        const next = await request(
          `/api/guide/${encodeURIComponent(state.id)}`,
          { timeoutMs: 30000 },
        );
        if (stopped || loadGate.current.pending() || !loadGate.current.current(navigationToken) ||
            !guideOperationReadMatches(watched, latestState.current, next)) return;
        const transient = reconnectError.current;
        if (transient?.guide_id === state.id && transient.request_id === watched.request_id) {
          setError(previous => previous === transient.message ? "" : previous);
          reconnectError.current = null;
        }
        if (next.operation?.status === "running") {
          acceptState(next);
          return;
        }
        const submitted = next.operation?.request;
        if (next.operation?.status === "completed") {
          if (preservedGuideResume(guideStorage, next)) {
            setInput(previous => recoverGuideResumeInput(guideStorage, next, previous) || previous);
          } else {
            if (submitted?.action !== "manual") guideStorage.removeItem(inputKey(latestState.current));
            setInput((previous) => admitSubmittedInput(previous, submitted || {}));
          }
        }
        setPendingMessage(null);
        acceptState(next);
        refreshSetups();
      } catch (failure) {
        if (!stopped && !loadGate.current.pending() && loadGate.current.current(navigationToken) &&
            guideOperationReadMatches(watched, latestState.current, latestState.current)) {
          const message = `Connection interrupted. Your answer is still saved on the setup server. ${failure.message}`;
          reconnectError.current = { guide_id: state.id, request_id: watched.request_id, message };
          setError(message);
        }
      } finally {
        reading = false;
      }
    }, 2500);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [loadingSetup, state?.id, state?.version, state?.operation?.request_id, state?.operation?.status]);
  useEffect(() => {
    if (!historyOpen) return;
    const dismiss = (event) => {
      if (event.type === "keydown" && event.key === "Escape") {
        setHistoryOpen(false);
        document.querySelector(".setup-history-control > button")?.focus();
      } else if (
        event.type === "pointerdown" &&
        !event.target.closest(".setup-history-control")
      )
        setHistoryOpen(false);
    };
    document.addEventListener("keydown", dismiss);
    document.addEventListener("pointerdown", dismiss);
    return () => {
      document.removeEventListener("keydown", dismiss);
      document.removeEventListener("pointerdown", dismiss);
    };
  }, [historyOpen]);
  useEffect(() => {
    if (!configOpen && !filesOpen) return;
    const previousFocus = document.activeElement;
    const dialog = document.querySelector(".setup-config-modal");
    const focusable = () => [
      ...dialog.querySelectorAll(
        "button:not(:disabled), textarea, [tabindex='0']",
      ),
    ];
    focusable()[0]?.focus();
    const keydown = (event) => {
      if (event.key === "Escape") {
        closeConfig();
        setFilesOpen(false);
      } else if (event.key === "Tab") {
        const nodes = focusable(),
          first = nodes[0],
          last = nodes[nodes.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last?.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first?.focus();
        }
      }
    };
    document.addEventListener("keydown", keydown);
    return () => {
      document.removeEventListener("keydown", keydown);
      previousFocus?.focus();
    };
  }, [configOpen, filesOpen]);
  useEffect(() => {
    if (
      !active ||
      !manualReturn ||
      handledReturn.current === manualReturn.id ||
      !editorBase.current
    )
      return;
    handledReturn.current = manualReturn.id;
    const editor = editorBase.current;
    editorBase.current = null;
    if (manualReturn.editor_id !== editor.id || !loadGate.current.current(editor.navigationToken) ||
        loadGate.current.pending() || latestState.current?.id !== editor.guide.id) return;
    const feedback = manualFeedback(editor.base, manualReturn.blueprint);
    if (!feedback) return;
    const current = latestState.current;
    const drifted = current.version !== editor.guide.version ||
      canonicalJson({ plan: guidePlan(current), brief: current.brief }) !== editor.inputs ||
      canonicalJson(latestManualNote.current) !== editor.manual;
    if (drifted || current.operation?.status === "running" || processing.current || packageBusy || editor.intentBase || editor.recoveryOnly) {
      const base = editor.intentBase || editor.base;
      const retained = manualFeedback(base, manualReturn.blueprint);
      if (!retained) {
        if (canonicalJson(latestManualNote.current) === editor.manual && current.operation?.status !== "running") discardManual();
        return;
      }
      retained.manual_intent = { base: clone(base), draft: clone(manualReturn.blueprint) };
      retained.proposal_changed = drifted;
      storeManual(current.id, retained);
      latestManualNote.current = retained;
      setManualNote(retained);
      onBlueprint(visibleManualPlan(retained, current));
      return;
    }
    queueManual(feedback, { expectedGuide: editor.guide });
  }, [active, manualReturn]);
  useEffect(() => {
    publishRuntime(config?.architect ?? null);
  }, [config?.architect]);
  useEffect(() => {
    const available = runtime?.available ?? state?.capabilities?.available ?? config?.architect?.available;
    // A fresh idea comes first. Existing conversations can still recover access.
    // Once opened, settings stay mounted through save/start observations.
    if (state && !loadingSetup && !isForgeOpening(state, proposalPlan(manualNote, state), { pending: !!manualNote }) &&
        !providerDismissed && available === false && !runtime?.available) setProviderOpen(true);
  }, [providerDismissed, runtime?.available, state, manualNote, loadingSetup, config?.architect?.available]);
  useEffect(() => {
    if (!busy) return;
    // Parent owner updates invalidate status reads started under the older owner.
    let stopped = false, reading = false;
    const timer = setInterval(async () => {
      if (stopped || reading) return;
      reading = true;
      const generation = runtimeGeneration.current;
      try {
        const status = await request("/api/architect/status", { timeoutMs: 30000 });
        if (!stopped && runtimeGeneration.current === generation) publishRuntime(status);
      } catch {} finally {
        reading = false;
      }
    }, 4000);
    return () => { stopped = true; clearInterval(timer); };
  }, [busy, config?.architect]);
  function revealResponse(explicit = false, expectedPublication = null) {
    if (expectedPublication && expectedPublication !== guidePublicationKey(latestState.current)) return;
    const scroll = chatScroll.current;
    const targets = { scroll, failure: operationFailureRef.current, question: questionRef.current, response: responseRef.current,
      preferQuestion: explicit !== true && !setupProgressHasLifecycle(latestState.current, latestLifecycle.current.receipts) };
    const focus = guideResponseTarget(latestState.current, { ...targets, busy });
    const revealingResponse = !busy && focus && savedGuideResponse(latestState.current);
    if (revealingResponse) {
      if (!responseFollow.current.select(latestState.current, targets, explicit === true)) return;
    } else responseFollow.current.release();
    if (scroll && busy) {
      const top = guideActivityScrollTop(scroll);
      if (top === null) return;
      // An explicit return must not release following during smooth-scroll
      // intermediate positions while new native activity is still arriving.
      scroll.scrollTo({ top, behavior: explicit === true ? "instant" : "smooth" });
      setFollowing(true);
      return;
    }
    if (scroll && !revealingResponse && (latestState.current?.messages?.length || 0) <= 1) {
      scroll.scrollTo({ top: 0, behavior: "smooth" });
      return;
    }
    if (scroll && focus)
      scroll.scrollTo({
        top: guideResponseScrollTop(latestState.current, targets),
        behavior: revealingResponse ? "auto" : "smooth",
      });
    setFollowing(true);
    if (guidePublicationVisible(latestState.current, targets))
      publicationUnread.current.markSeen(latestState.current, guidePublicationKey(latestState.current));
    setUnseenResponse(publicationUnread.current.observe(latestState.current));
    if (document.activeElement === document.body && focus) {
      focus.tabIndex = -1;
      focus.focus({ preventScroll: true });
    }
  }
  function revealMobileResponse(publicationKey) {
    if (publicationKey !== guidePublicationKey(latestState.current)) return;
    const ownsSelection = lifecycleSelection.current.capture(latestState.current);
    const conversation = conversationRef.current, scroll = chatScroll.current;
    if (!ownsSelection || !latestLifecycle.current?.flags.active) return;
    const focusOwner = conversation?.ownerDocument.activeElement;
    setMobilePanel("chat");
    requestAnimationFrame(() => {
      if (!setupsMounted.current || !latestLifecycle.current?.flags.active || !ownsSelection() ||
          latestMobilePanel.current !== "chat" ||
          publicationKey !== guidePublicationKey(latestState.current) ||
          conversationRef.current !== conversation || !conversation?.isConnected ||
          chatScroll.current !== scroll || !scroll?.isConnected) return;
      const currentFocus = conversation.ownerDocument.activeElement;
      if (currentFocus !== focusOwner && !(currentFocus === conversation.ownerDocument.body &&
          focusOwner && (!focusOwner.isConnected || focusOwner.closest("[hidden]")))) return;
      conversation.focus({ preventScroll: true });
      revealResponse(true, publicationKey);
    });
  }
  useEffect(() => {
    if (!active || !chatScroll.current) return;
    const targets = { scroll: chatScroll.current, question: questionRef.current, response: responseRef.current };
    if (guidePublicationVisible(state, targets)) publicationUnread.current.markSeen(state, guidePublicationKey(state));
    setUnseenResponse(publicationUnread.current.observe(state));
    const signal = guideResponseSignal(state, busy, error);
    if (!error && !busy && responseSignal.current === signal) return;
    responseSignal.current = signal;
    if (forceFollow.current || following || !(state?.messages?.length > 1)) {
      const frame = requestAnimationFrame(revealResponse);
      forceFollow.current = false;
      return () => cancelAnimationFrame(frame);
    }
  }, [state?.id, state?.version, state?.operation?.request_id, state?.operation?.status, state?.operation?.started_at,
    state?.messages?.at(-1)?.id, busy, error, active, mobilePanel]);
  useEffect(() => {
    const scroll = chatScroll.current;
    if (!active || !scroll || !busy) return;
    if (following || forceFollow.current) {
      const top = guideActivityScrollTop(scroll);
      if (top !== null) scroll.scrollTop = top;
    }
  }, [timeline.frames, busy, active]);
  useEffect(() => {
    if (!active || busy || !savedGuideResponse(state)) return;
    let frame;
    const maintain = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => responseFollow.current.maintain(latestState.current,
        { scroll: chatScroll.current, failure: operationFailureRef.current,
          question: questionRef.current, response: responseRef.current,
          preferQuestion: !setupProgressHasLifecycle(latestState.current, latestLifecycle.current.receipts) }));
    };
    const transcript = primaryGuideTranscript(chatScroll.current);
    const observer = transcript && typeof ResizeObserver === "function" ? new ResizeObserver(maintain) : null;
    observer?.observe(transcript);
    maintain();
    return () => {
      observer?.disconnect();
      cancelAnimationFrame(frame);
    };
  }, [active, busy, state?.id, state?.version, state?.operation?.request_id, state?.operation?.status,
    state?.operation?.started_at, timeline.frames]);
  function releaseResponseFollow() {
    responseFollow.current.release(latestState.current);
    forceFollow.current = false;
    setFollowing(false);
  }

  function recordQuestionVisibility() {
    const current = latestState.current;
    const next = { id: current?.id, version: current?.version,
      visible: guideQuestionVisible(current, { scroll: chatScroll.current, question: questionRef.current }) };
    setQuestionInView(previous => previous && previous.id === next.id && previous.version === next.version &&
      previous.visible === next.visible ? previous : next);
  }

  useEffect(() => {
    if (!active) return;
    let frame;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(recordQuestionVisibility);
    };
    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(update) : null;
    for (const node of [chatScroll.current, questionRef.current, primaryGuideTranscript(chatScroll.current)]) {
      if (node) observer?.observe(node);
    }
    update();
    return () => { observer?.disconnect(); cancelAnimationFrame(frame); };
  }, [active, state?.id, state?.version, state?.question, busy, mobilePanel, timeline.frames,
    implementation, rehearsal, deployment, manualNote, error, appError, providerOpen, providerDismissed, runtime?.available, savedToolsGuide]);

  function revealCurrentQuestion() {
    const current = latestState.current;
    if (!current?.question) return;
    releaseResponseFollow();
    setMobilePanel("chat");
    requestAnimationFrame(() => {
      const scroll = chatScroll.current, target = questionRef.current;
      if (latestState.current?.id !== current.id || latestState.current.version !== current.version ||
          !scroll || !target?.isConnected) return;
      scroll.scrollTo({ top: Math.max(0, scroll.scrollTop + target.getBoundingClientRect().top -
        scroll.getBoundingClientRect().top - 16), behavior: "auto" });
      target.tabIndex = -1;
      target.focus({ preventScroll: true });
    });
  }

  function admitCreatedGuide(started, startRequest) {
    const admittedInput = latestInput.current;
    latestState.current = started;
    setState(started);
    guideStorage.setItem(GUIDE_KEY, started.id);
    if (creationStorage.getItem(GUIDE_CREATION_KEY) === startRequest)
      creationStorage.removeItem(GUIDE_CREATION_KEY);
    const pending = savedManual(null);
    if (pending) {
      storeManual(started.id, pending);
      storeManual(null, null);
    }
    const startedInput = admitNewGuideInput(guideStorage, started, admittedInput);
    guideStorage.removeItem(admittedInput.key);
    latestInput.current = startedInput;
    writeInput(guideStorage, startedInput);
    setInput(startedInput);
  }
  async function checkCreatedGuide() {
    const startRequest = creationStorage.getItem(GUIDE_CREATION_KEY);
    if (latestState.current?.id || !startRequest || loadGate.current.pending()) return;
    writeInput(guideStorage, latestInput.current);
    const token = loadGate.current.begin();
    const owner = sendRequests.current.get("new");
    if (processing.current === owner) processing.current = false;
    if (owner) owner.status = "unconfirmed";
    setLoadingSetup(true);
    setCreationCheckDetail("");
    try {
      const next = await request(`/api/guide/creation/${encodeURIComponent(startRequest)}`, { timeoutMs: 30000 });
      if (!loadGate.current.current(token) || latestState.current?.id ||
          creationStorage.getItem(GUIDE_CREATION_KEY) !== startRequest) return;
      if (typeof next?.id !== "string" || !Number.isSafeInteger(next.version) || next.version < 1)
        throw new Error("The saved conversation could not be confirmed.");
      if (owner) releaseSend(owner);
      admitCreatedGuide(next, startRequest);
      acceptState(next);
      setBusy(next.operation?.status === "running");
      setPendingMessage(null);
      setError("");
      refreshSetups();
    } catch (failure) {
      if (loadGate.current.current(token) && !latestState.current?.id)
        setCreationCheckDetail(failure.status === 404
          ? "No saved conversation is confirmed yet. Your writing is kept. You can check again."
          : "The saved conversation could not be checked. Your writing is kept. You can check again.");
    } finally {
      if (loadGate.current.finish(token)) {
        setLoadingSetup(false);
        if (!latestState.current?.operation || latestState.current.operation.status !== "running") {
          setBusy(false);
          setBusyAction("");
        }
      }
    }
  }
  async function ensureStarted(navigationToken, owner) {
    if (!loadGate.current.current(navigationToken) || loadGate.current.pending()) return null;
    const current = latestState.current;
    if (current?.id) return current;
    const startRequest = guideCreationRequest(creationStorage);
    owner.startRequest = startRequest;
    owner.postEntered = true;
    const started = await request("/api/guide/start", {
      method: "POST",
      timeoutMs: 30000,
      body: { start_request_id: startRequest },
    });
    // A delayed creation belongs to the anonymous setup that requested it.
    // Navigation must fence every publication and draft removal below.
    if (!loadGate.current.current(navigationToken) || loadGate.current.pending() ||
        latestState.current !== current || sendRequests.current.get("new") !== owner) return null;
    // The first answer moves from the unsaved draft into its newly durable setup.
    admitCreatedGuide(started, startRequest);
    sendRequests.current.delete("new");
    owner.id = started.id;
    owner.postEntered = false;
    sendRequests.current.set(started.id, owner);
    return started;
  }
  async function respond(action = "answer", extra = {}, options = {}) {
    const navigationToken = loadGate.current.token();
    if (processing.current || sendRequests.current.has(latestState.current?.id || "new") ||
        !latestState.current?.id && creationStorage.getItem(GUIDE_CREATION_KEY) !== null ||
        applyRequests.current.has(latestState.current?.id) || packageBusy || loadGate.current.pending()) return;
    const expected = options.expectedGuide;
    if (expected && (latestState.current?.id !== expected.id || latestState.current.version !== expected.version ||
        latestInput.current.key !== inputKey(latestState.current))) return;
    if (latestState.current?.operation?.status === "running") return;
    if (action === "manual" && latestManualNote.current?.review_unavailable) {
      setError("Your earlier draft is saved. Adjust it to review new edits against the current plan, or discard the pending changes.");
      return;
    }
    if (action === "manual" && !options.preserveInput && latestManualNote.current &&
        !matchesManual(latestManualNote.current, { action, ...extra })) {
      setError("Your saved adjustments changed. Review the current adjustments before sending.");
      return;
    }
    if (["answer", "chat"].includes(action) && (preparingAttachments.current !== null && preparingAttachments.current === navigationToken || attachments.uploading || attachments.pending.length)) {
      attachments.fail("Finish uploading your files, or remove them before sending. Your draft and choices are kept.");
      return;
    }
    if (manualNote && action !== "manual") {
      setError("Review your pending changes with the guide before continuing.");
      return;
    }
    const choices = extra.selected ?? (action === "answer" ? selected : []);
    const submittedAttachments = ["answer", "chat"].includes(action)
      ? extra.attachments ?? attachments.read(latestState.current?.id) : [];
    const messageText = extra.text ?? (["answer", "chat"].includes(action) ? inputSubmissionText(input) : "");
    const text = submittedAttachments.length && !messageText.trim() && !choices.length
      ? "Consider the attached evidence." : messageText;
    const { combine_options: explicitCombination, ...requestExtra } = extra;
    const submittedManual = action === "manual" ? clone(latestManualNote.current) : null;
    const combined = action === "answer" && (explicitCombination ?? combineOptions) === true;
    const question = latestState.current?.question;
    const labels =
      action === "answer"
        ? (question?.options || [])
            .filter((o) => choices.includes(o.id))
            .map((o) => o.label)
        : [];
    setPendingMessage(
      ["back", "organization"].includes(action)
        ? null
        : { text: `${combined ? "More than one fits: " : ""}${[...labels, text].filter(Boolean).join("; ")}`, role: "user", created_at: new Date().toISOString(),
            ...(submittedAttachments.length ? { attachments: submittedAttachments } : {}) },
    );
    if (hasForgeOpeningContext(latestState.current, proposalPlan(latestManualNote.current, latestState.current),
        { pending: !!latestManualNote.current }) && questionRef.current?.contains(document.activeElement)) {
      inputRef.current?.focus({ preventScroll: true });
    }
    forceFollow.current = true;
    const owner = { id: latestState.current?.id || null, status: "sending", request: null,
      postEntered: false, preserveInput: options.preserveInput === true, submittedManual };
    sendRequests.current.set(owner.id || "new", owner);
    processing.current = owner;
    sendingToken.current = navigationToken;
    setBusy(true);
    setBusyAction(action);
    setError("");
    let requestGuideId;
    try {
      const current = await ensureStarted(navigationToken, owner);
      if (!current) return;
      requestGuideId = current.id;
      if (!loadGate.current.current(navigationToken) || loadGate.current.pending() ||
          latestState.current?.id !== current.id || expected && (current.id !== expected.id || current.version !== expected.version ||
            latestState.current.version !== expected.version || latestInput.current.key !== inputKey(current))) return;
      if (options.preserveInput && !captureGuideResumeInput(guideStorage, current, latestInput.current, text,
          action === "organization" ? { action, organization_fingerprint: extra.organization_fingerprint } : undefined)) {
        throw new Error("Your pending answer could not be saved. Keep it in the composer and try again after local storage is available.");
      }
      const submittedRequest = {
        version: current.version,
        action,
        selected: choices,
        text,
        ...requestExtra,
        ...(combined ? { combine_options: true } : {}),
        ...(submittedAttachments.length ? { attachments: submittedAttachments } : {}),
      };
      owner.state = current;
      owner.request = submittedRequest;
      if (!options.preserveInput && submittedAttachments.length) attachments.capture(current.id, submittedRequest);
      owner.postEntered = true;
      let next = await request(`/api/guide/${current.id}/respond`, {
        method: "POST",
        timeoutMs: 30000,
        body: submittedRequest,
      });
      if (!loadGate.current.current(navigationToken) || loadGate.current.pending() ||
          latestState.current?.id !== current.id || sendRequests.current.get(current.id) !== owner) return;
      // Consume only the submitted references after admission, retaining files added during the request.
      if (!options.preserveInput && submittedAttachments.length) attachments.admitted(current.id, submittedAttachments, submittedRequest);
      if (!loadGate.current.current(navigationToken) || loadGate.current.pending() || latestState.current?.id !== current.id) return;
      if (options.preserveInput) {
        // Immediate completed responses do not carry the durable operation ID.
        if (!next.operation?.request_id) next = await request(`/api/guide/${current.id}`);
        if (!loadGate.current.current(navigationToken) || loadGate.current.pending() || latestState.current?.id !== current.id) return;
        if (!preservedGuideResume(guideStorage, next)) {
          throw new Error("The guide request could not be matched to its saved response. Your draft and choices are preserved; reload the conversation before retrying.");
        }
        releaseSend(owner);
        setInput(previous => recoverGuideResumeInput(guideStorage, next, previous) || previous);
        acceptState(next);
        refreshSetups();
        setValidation(null);
        setDownloaded(false);
        return;
      }
      releaseSend(owner);
      setInput((previous) => {
        const admitted = admitSubmittedInput(previous, { action, text, selected: choices,
          ...(combined ? { combine_options: true } : {}) });
        writeInput(guideStorage, admitted);
        return admitted;
      });
      if (next.operation?.status === "running") {
        if (action === "manual") bindAdmittedManual(current, submittedManual, { action, text, ...requestExtra }, next);
        acceptState(next);
        return;
      }
      if (action !== "manual") guideStorage.removeItem(inputKey(current));
      latestState.current = next;
      setState(next);
      refreshSetups();
      if (next.proposal || next.blueprint)
        onBlueprint(next.proposal || next.blueprint);
      else onBlueprint(initialBlueprint.current);
      if (
        action === "back" ||
        (["answer", "chat"].includes(action) &&
          (extra.text === undefined || extra.text === draft))
      )
        setDraft((value) => (value === text ? "" : value));
      if (action === "manual") {
        if (submittedManual && canonicalJson(savedManual(current.id)) === canonicalJson(submittedManual) &&
            canonicalJson(latestManualNote.current) === canonicalJson(submittedManual))
          clearSubmittedManual(current.id, { action, ...requestExtra, text });
      }
      setValidation(null);
      setDownloaded(false);
    } catch (e) {
      const refused = Number.isInteger(e.status) && e.status >= 400 && e.status < 500;
      const notEntered = !owner.postEntered;
      if (refused || notEntered) releaseSend(owner);
      if (loadGate.current.current(navigationToken) && (!requestGuideId || latestState.current?.id === requestGuideId))
        setError(refused || notEntered ? e.message : "");
    } finally {
      if (sendRequests.current.get(owner.id || "new") === owner) {
        owner.status = "unconfirmed";
        if (setupsMounted.current) redrawSendStatus(value => value + 1);
      }
      if (processing.current === owner) processing.current = false;
      if (
        loadGate.current.current(navigationToken) && (!requestGuideId || latestState.current?.id === requestGuideId) &&
        latestState.current?.operation?.status !== "running"
      ) {
        setBusy(false);
        setBusyAction("");
        setPendingMessage(null);
      }
    }
  }
  function queueManual(feedback, options = {}) {
    if (processing.current || sendRequests.current.has(latestState.current?.id || "new") ||
        applyRequests.current.has(latestState.current?.id) || packageBusy || options.isCurrent && !options.isCurrent()) return false;
    const current = latestState.current, expected = options.expectedGuide;
    if (expected && (current?.id !== expected.id || current?.version !== expected.version)) return false;
    if (latestManualNote.current?.manual_intent || latestManualNote.current?.review_unavailable) {
      const base = latestManualNote.current.manual_intent?.base || latestManualNote.current.blueprint;
      const retained = manualFeedback(base, feedback.blueprint);
      if (!retained) {
        if (!latestManualNote.current.review_unavailable) discardManual();
        return true;
      }
      retained.manual_intent = { base: clone(base), draft: clone(feedback.blueprint) };
      retained.proposal_changed = latestManualNote.current.proposal_changed === true;
      storeManual(current?.id, retained);
      latestManualNote.current = retained;
      setManualNote(retained);
      onBlueprint(visibleManualPlan(retained, current));
      return true;
    }
    const retained = { ...feedback,
      review_base: clone(latestManualNote.current?.review_base || guidePlan(current) || initialBlueprint.current),
      review_guide: latestManualNote.current?.review_guide || { id: current?.id ?? null, version: current?.version } };
    storeManual(options.expectedGuide?.id || latestState.current?.id || null, retained);
    latestManualNote.current = retained;
    setManualNote(retained);
    onBlueprint(retained.blueprint);
    respond("manual", { blueprint: retained.blueprint, text: retained.text },
      options.expectedGuide?.id ? { expectedGuide: options.expectedGuide } : {});
    return true;
  }
  async function changeCount(classId, count) {
    const guide = latestState.current, flags = latestLifecycle.current?.flags;
    const plan = latestManualNote.current?.blueprint || guide?.proposal || guide?.blueprint;
    if (!plan || countPreparing.current || processing.current || packagePreparing.current || loadGate.current.pending() ||
        !flags?.active || flags?.designBusy || flags?.packageBusy || latestManualNote.current ||
        guide?.operation?.status === "running" || latestInput.current.key !== inputKey(guide)) return;
    const pending = { navigationToken: loadGate.current.token(), guide: { id: guide?.id, version: guide?.version },
      inputs: canonicalJson({ plan, brief: guide?.brief }), manual: canonicalJson(latestManualNote.current), base: clone(plan) };
    countPreparing.current = pending;
    const current = () => {
      const selected = latestState.current, lifecycle = latestLifecycle.current?.flags;
      const selectedPlan = latestManualNote.current?.blueprint || selected?.proposal || selected?.blueprint;
      return countPreparing.current === pending && loadGate.current.current(pending.navigationToken) && !loadGate.current.pending() &&
        selected?.id === pending.guide.id && selected?.version === pending.guide.version &&
        latestInput.current.key === inputKey(selected) && !!selectedPlan &&
        pending.inputs === canonicalJson({ plan: selectedPlan, brief: selected?.brief }) &&
        pending.manual === canonicalJson(latestManualNote.current) && lifecycle?.active && !lifecycle?.designBusy &&
        !processing.current && !packagePreparing.current && selected?.operation?.status !== "running";
    };
    setPackageBusy(true);
    try {
      const checked = await request("/api/blueprint/import", {
        method: "POST",
        body: resizeClass(pending.base, classId, count),
      });
      if (!current()) return;
      const feedback = manualFeedback(pending.base, checked.blueprint);
      if (feedback) queueManual(feedback, { expectedGuide: pending.guide, isCurrent: current });
    } catch (e) {
      if (current()) setError(e.message);
    } finally {
      if (countPreparing.current === pending) {
        countPreparing.current = null;
        setPackageBusy(false);
      }
    }
  }
  async function chooseCloudProject(expected) {
    if (processing.current || packageBusy || manualNote || loadGate.current.pending()) return;
    const flags = () => latestLifecycle.current?.flags;
    const blocked = () => processing.current || !!flags()?.designBusy || !!flags()?.pendingChanges || loadGate.current.pending();
    const current = () => typeof expected?.isCurrent === "function" && expected.isCurrent() &&
      gcpProjectFeedbackCurrent(expected, latestState.current, latestImplementation.current, blocked());
    if (!current()) return;
    const base = latestState.current.proposal || latestState.current.blueprint;
    const candidate = gcpProjectBlueprint(base, expected.project);
    if (!candidate) return;
    setPackageBusy(true);
    let feedback;
    try {
      const checked = await request("/api/blueprint/import", { method: "POST", body: candidate });
      if (!current()) return;
      if (!gcpProjectBlueprintMatches(base, checked.blueprint, expected.project))
        throw new Error("The project choice changed other proposal details. Ask the guide to review the cloud destination.");
      feedback = manualFeedback(base, checked.blueprint);
    } finally {
      setPackageBusy(false);
    }
    if (feedback && current()) queueManual(feedback);
  }
  function discardManual() {
    if (busy || packageBusy) return;
    storeManual(state?.id, null);
    latestManualNote.current = null;
    setManualNote(null);
    setError("");
    if (state.proposal || state.blueprint)
      onBlueprint(state.proposal || state.blueprint);
  }
  function openDetails() {
    if (loadGate.current.pending() || packagePreparing.current) return;
    const plan =
      manualNote?.blueprint ||
      state?.proposal ||
      state?.blueprint ||
      initialBlueprint.current;
    const current = latestState.current, pending = latestManualNote.current;
    const alreadySubmitted = current?.operation?.status === "running" && matchesManual(pending, current.operation.request, current.operation.request_id);
    const editor = { id: crypto.randomUUID(), base: clone(plan),
      intentBase: !alreadySubmitted && pending?.manual_intent?.base && clone(pending.manual_intent.base),
      recoveryOnly: pending?.review_unavailable === true,
      navigationToken: loadGate.current.token(), guide: { id: current?.id, version: current?.version },
      inputs: canonicalJson({ plan: guidePlan(current), brief: current?.brief }), manual: canonicalJson(pending) };
    editorBase.current = editor;
    onAdvanced(plan, editor.id);
  }
  function openConfig() {
    const current = latestState.current, flags = latestLifecycle.current?.flags;
    const plan = latestManualNote.current?.blueprint || current?.proposal || current?.blueprint;
    if (!plan || processing.current || packagePreparing.current || loadGate.current.pending() ||
        flags?.designBusy || flags?.packageBusy || current?.operation?.status === "running") return;
    if (ownsConfigEditor(configEditor.current)) return;
    const draft = JSON.stringify(plan, null, 2);
    configEditor.current = { navigationToken: loadGate.current.token(), guide: { id: current?.id, version: current?.version },
      inputs: canonicalJson({ plan, brief: current?.brief }), manual: canonicalJson(latestManualNote.current),
      base: clone(plan), draft };
    configReview.current = null;
    setConfigChecking(false);
    setConfigDraft(draft);
    setConfigError("");
    setConfigOpen(true);
  }
  function closeConfig(editor) {
    if (editor && configEditor.current !== editor) return false;
    configEditor.current = null;
    configReview.current = null;
    setConfigChecking(false);
    setConfigOpen(false);
    return true;
  }
  function editConfig(draft) {
    if (!configEditor.current) return;
    configEditor.current = { ...configEditor.current, draft };
    configReview.current = null;
    setConfigChecking(false);
    setConfigDraft(draft);
    setConfigError("");
  }
  function ownsConfigEditor(editor) {
    const current = latestState.current;
    const plan = latestManualNote.current?.blueprint || current?.proposal || current?.blueprint;
    return !!editor && configEditor.current === editor && loadGate.current.current(editor.navigationToken) &&
      !loadGate.current.pending() && current?.id === editor.guide.id && current?.version === editor.guide.version &&
      latestInput.current.key === inputKey(current) && !!plan &&
      editor.inputs === canonicalJson({ plan, brief: current?.brief }) &&
      editor.manual === canonicalJson(latestManualNote.current);
  }
  async function reviewConfig() {
    const editor = configEditor.current, flags = latestLifecycle.current?.flags;
    if (!ownsConfigEditor(editor) || configReview.current || processing.current || packagePreparing.current ||
        flags?.designBusy || flags?.packageBusy || latestState.current?.operation?.status === "running") return;
    const review = { editor };
    configReview.current = review;
    setConfigChecking(true);
    const current = () => configReview.current === review && ownsConfigEditor(editor) && !processing.current &&
      !packagePreparing.current && !latestLifecycle.current?.flags?.designBusy &&
      !latestLifecycle.current?.flags?.packageBusy && latestState.current?.operation?.status !== "running";
    setConfigError("");
    try {
      const candidate = JSON.parse(editor.draft);
      const checked = await request("/api/blueprint/import", {
        method: "POST",
        body: candidate,
      });
      if (!current()) return;
      const feedback = manualFeedback(editor.base, checked.blueprint);
      if (!feedback || queueManual(feedback, { expectedGuide: editor.guide, isCurrent: current })) closeConfig(editor);
    } catch (e) {
      if (current()) setConfigError(e.message);
    } finally {
      if (configReview.current === review) {
        configReview.current = null;
        setConfigChecking(false);
      }
    }
  }
  async function applyPlan(displayed, retry) {
    const current = latestState.current, flags = latestLifecycle.current?.flags;
    const retrying = retry && applyRequests.current.get(current?.id) === retry && retry.checked &&
      current.version === retry.version && canonicalJson(current.proposal) === retry.proposal;
    if (retry && !retrying) return;
    if (!setupProposalReviewable(current) || current.id !== displayed?.id || current.version !== displayed?.version ||
        processing.current || (!retrying && applyRequests.current.has(current.id)) || packagePreparing.current || loadGate.current.pending() ||
        busy || (flags?.designBusy && !retrying) || flags?.pendingChanges || flags?.packageBusy || current.operation?.status === "running") return;
    const navigationToken = loadGate.current.token();
    const ownsVersion = (version) => setupsMounted.current && loadGate.current.current(navigationToken) && !loadGate.current.pending() &&
      latestState.current?.id === current.id && latestState.current?.version === version;
    const owner = { id: current.id, version: current.version, proposal: canonicalJson(current.proposal), checked: false };
    let settledVersion = current.version;
    applyRequests.current.set(current.id, owner);
    processing.current = owner;
    sendingToken.current = navigationToken;
    setBusy(true);
    setBusyAction("apply");
    setError("");
    try {
      const next = await request(`/api/guide/${current.id}/apply`, {
        method: "POST", timeoutMs: 30000,
        body: { version: current.version },
      });
      if (!ownsVersion(current.version)) return;
      if (next.id !== current.id || !Number.isInteger(next.version) || next.version <= current.version ||
          next.status !== "ready" || !next.blueprint) throw new Error("Plan confirmation was unavailable.");
      releaseApply(owner);
      settledVersion = next.version;
      latestState.current = next;
      setState(next);
      onBlueprint(next.blueprint);
      setValidation(null);
    } catch (e) {
      const refused = Number.isInteger(e.status) && e.status >= 400 && e.status < 500;
      if (refused) releaseApply(owner);
      if (ownsVersion(current.version)) setError(refused ? e.message
        : "Plan saving was not confirmed. Check the saved plan before continuing. Your draft is kept.");
    } finally {
      if (processing.current === owner) processing.current = false;
      if (ownsVersion(settledVersion) && latestState.current?.operation?.status !== "running") {
        setBusy(false);
        setBusyAction("");
      }
    }
  }
  async function inspectPackage(download = false, scope = "selected", displayed = state) {
    const current = latestState.current, flags = latestLifecycle.current?.flags;
    const plan = scope === "native" ? nativeTeamPackagePlan(current) : current?.blueprint;
    if (!plan || scope !== "native" && current.status !== "ready" ||
        current.id !== displayed?.id || current.version !== displayed?.version ||
        current.operation?.status === "running" || processing.current || packagePreparing.current || loadGate.current.pending() ||
        flags?.designBusy || flags?.pendingChanges || flags?.packageBusy)
      return;
    const navigationToken = loadGate.current.token();
    const inputs = canonicalJson({ plan, brief: current.brief });
    const selectedImplementation = latestImplementation.current;
    const checkedApplication = scope !== "native" && download && implementationCanDownload(selectedImplementation, current);
    const packageReason = packagePreparationReason(current, plan);
    if (packageReason && !checkedApplication) { setError(packageReason); return; }
    const buildKey = (receipt) => canonicalJson({ build_id: receipt?.build_id, source_digest: receipt?.source_digest,
      package_sha256: receipt?.package_sha256 });
    const selectedBuildKey = checkedApplication ? buildKey(selectedImplementation) : null;
    const ownsSelection = () => {
      const latest = latestState.current, flags = latestLifecycle.current?.flags;
      const nextPlan = scope === "native" ? nativeTeamPackagePlan(latest) : latest?.status === "ready" && latest.blueprint;
      return loadGate.current.current(navigationToken) && !loadGate.current.pending() &&
        latest?.id === current.id && latest.version === current.version && !!nextPlan &&
        latest.operation?.status !== "running" && !processing.current && !flags?.designBusy && !flags?.pendingChanges &&
        inputs === canonicalJson({ plan: nextPlan, brief: latest.brief }) &&
        (!checkedApplication || implementationCanDownload(latestImplementation.current, latest) &&
          buildKey(latestImplementation.current) === selectedBuildKey);
    };
    packagePreparing.current = true;
    setPackageBusy(true);
    setError("");
    try {
      const snapshot = await fingerprint({ plan, brief: current.brief });
      if (!ownsSelection()) return;
      const displayedPlan = scope === "native" ? nativeTeamPackagePlan(displayed) : displayed?.blueprint;
      const displayedSnapshot = await fingerprint({ plan: displayedPlan, brief: displayed.brief });
      if (!ownsSelection()) return;
      if (snapshot !== displayedSnapshot)
        throw new Error("The setup changed. Review its current package again.");
      const stillCurrent = async () => {
        if (!ownsSelection()) return false;
        const latest = latestState.current;
        const nextPlan = scope === "native" ? nativeTeamPackagePlan(latest) : latest?.status === "ready" && latest.blueprint;
        const nextSnapshot = await fingerprint({ plan: nextPlan, brief: latest.brief });
        return ownsSelection() && snapshot === nextSnapshot;
      };
      if (!await stillCurrent()) return;
      if (checkedApplication) {
        const blob = await requestBlob(implementationPackageUrl(selectedImplementation, current), { timeoutMs: 60000, stillCurrent });
        if (!ownsSelection()) return;
        if (!await stillCurrent()) return;
        const url = URL.createObjectURL(blob), link = document.createElement("a");
        link.href = url;
        link.download = `${plan.project.slug}-application.zip`;
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 10000);
        setDownloaded(true);
        return;
      }
      const result = await request("/api/blueprint/validate", {
        method: "POST",
        body: plan,
        timeoutMs: 60000,
        stillCurrent,
      });
      if (!await stillCurrent()) return;
      setValidation(result);
      if (result?.valid !== true) throw new Error("This plan could not be compiled. Review its validation before downloading.");
      if (result.package_availability?.available === false) throw new Error(result.package_availability.reason);
      if (download) {
        const blob = await requestBlob("/api/blueprint/package", {
          method: "POST",
          body: plan,
          timeoutMs: 60000,
          stillCurrent,
        });
        if (!ownsSelection()) return;
        if (!await stillCurrent()) return;
        const url = URL.createObjectURL(blob),
          link = document.createElement("a");
        link.href = url;
        link.download = `${plan.project.slug}.zip`;
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 10000);
        if (scope === "native") setNativeDownloadedKey(nativeTeamPackageKey(current));
        else setDownloaded(true);
      } else setFilesOpen(true);
    } catch (e) {
      if (ownsSelection()) setError(e.message);
    } finally {
      // This global latch excludes a second package transaction across navigation.
      packagePreparing.current = false;
      setPackageBusy(false);
    }
  }
  function downloadHandoff() {
    const current = latestState.current, flags = latestLifecycle.current?.flags;
    const ownerCurrent = lifecycleSelection.current.capture(state, lifecycleToken);
    if (!ownerCurrent || current?.id !== state?.id || current?.version !== state?.version ||
        !["ready", "handoff"].includes(current?.status) || !flags?.active ||
        current.operation?.status === "running" || processing.current || packagePreparing.current ||
        flags?.designBusy || flags?.pendingChanges || flags?.packageBusy || latestManualNote.current) return;
    const exported = guide => ({ content: implementationHandoff(guide),
      slug: (guide.proposal || guide.blueprint)?.project.slug || "mobkit" });
    const selected = exported(current);
    if (canonicalJson(selected) !== canonicalJson(exported(state)) || !ownerCurrent()) return;
    const url = URL.createObjectURL(new Blob([selected.content], { type: "text/markdown;charset=utf-8" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = `${selected.slug}-implementation-handoff.md`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 10000);
  }
  function newSetup() {
    setMobileMenuOpen(false);
    setPendingMessage(null);
    writeInput(guideStorage, latestInput.current);
    guideStorage.removeItem(GUIDE_KEY);
    creationStorage.removeItem(GUIDE_CREATION_KEY);
    const abandoned = sendRequests.current.get("new");
    if (abandoned) releaseSend(abandoned);
    setCreationCheckDetail("");
    setState(null);
    latestState.current = null;
    setInput(readInput(guideStorage, null));
    setError("");
    setManualNote(null);
    setValidation(null);
    setDownloaded(false);
    editorBase.current = null;
    setHistoryOpen(false);
    load();
  }
  async function startNewAfterReadFailure(failure) {
    if (!setupsMounted.current || !latestLifecycle.current?.flags.active || latestState.current ||
        typeof failure?.id !== "string" || !failure.id || loadGate.current.pending() ||
        !loadGate.current.current(failure.token) || guideStorage.getItem(GUIDE_KEY) !== failure.id) return;
    const next = await load("", { refreshRuntimeStatus: false });
    if (next && setupsMounted.current && latestLifecycle.current?.flags.active &&
        latestState.current === next && !loadGate.current.pending() &&
        guideStorage.getItem(GUIDE_KEY) === failure.id) guideStorage.removeItem(GUIDE_KEY);
  }
  async function switchSetup(id, buildReturn = null) {
    const focusOwner = buildReturn ? document.activeElement : null;
    setMobileMenuOpen(false);
    setHistoryOpen(false);
    setPendingMessage(null);
    setError("");
    setValidation(null);
    setDownloaded(false);
    const loading = load(id);
    const token = loadGate.current.token();
    const next = await loading;
    if (!next || !setupsMounted.current || !loadGate.current.current(token) ||
        loadGate.current.pending() || latestState.current?.id !== id) return;
    if (buildReturn) {
      const binding = captureSavedBuildNavigation(buildReturn, token, next);
      const intent = binding ? { ...binding, focusOwner } : null;
      savedBuildReturnRef.current = intent;
      setSavedBuildReturn(intent);
    } else forceFollow.current = true;
  }
  function retryOperation() {
    const saved = state?.operation?.request;
    if (!saved) return;
    const { action, version, ...body } = saved;
    // An omitted original file list means no files, not the next message's files.
    respond(action, { combine_options: false, ...body, attachments: body.attachments ?? [] }, preservedGuideResume(guideStorage, state)
      ? { preserveInput: true, expectedGuide: { id: state.id, version: state.version } } : {});
  }
  function reviewCurrentDraft(event) {
    const trigger = event?.currentTarget, editor = inputRef.current;
    const ownsSelection = lifecycleSelection.current.capture(state, lifecycleToken);
    const failure = savedGuideFailure(state), key = inputKey(state), writing = latestInput.current;
    const binding = operation => canonicalJson({ request_id: operation?.request_id, status: operation?.status,
      version: operation?.version, started_at: operation?.started_at, request: operation?.request });
    const expected = binding(failure);
    const current = () => {
      const guide = latestState.current, flags = latestLifecycle.current?.flags;
      return failure && trigger?.isConnected && editor?.isConnected && !editor.disabled &&
        trigger.closest(".setup-error") === operationFailureRef.current && inputRef.current === editor &&
        setupsMounted.current && flags?.active && ownsSelection && ownsSelection() &&
        guide?.id === state?.id && guide?.version === state?.version && inputKey(guide) === key &&
        latestInput.current === writing && writing.key === key &&
        binding(savedGuideFailure(guide)) === expected && !processing.current &&
        !sendRequests.current.has(guide.id) && !applyRequests.current.has(guide.id) &&
        !packagePreparing.current && !flags.designBusy && !flags.pendingChanges && !flags.packageBusy &&
        !latestManualNote.current;
    };
    if (!current()) return;
    const focusOwner = editor.ownerDocument.activeElement;
    if (narrow) setMobilePanel("chat");
    requestAnimationFrame(() => {
      if (!current()) return;
      const currentFocus = editor.ownerDocument.activeElement;
      if (currentFocus !== focusOwner && !(currentFocus === editor.ownerDocument.body &&
          focusOwner && (!focusOwner.isConnected || focusOwner.closest("[hidden]")))) return;
      editor.focus();
    });
  }
  function submitComposer() {
    if (processing.current || packageBusy || loadGate.current.pending() || composer.submitDisabled) return;
    if (available === false) {
      setProviderOpen(true);
      setMobilePanel("chat");
      return;
    }
    const filesOnly = attachments.refs.length > 0 && !inputSubmissionText(input).trim() && !selected.length;
    respond(state?.question && composeMode === "answer" && !filesOnly ? "answer" : "chat");
  }
  async function chooseAttachments(files) {
    const token = loadGate.current.token();
    if (preparingAttachments.current !== null && preparingAttachments.current === token ||
        attachments.uploading || attachments.pending.length ||
        !latestState.current?.id && (processing.current || sendRequests.current.has("new") ||
          creationStorage.getItem(GUIDE_CREATION_KEY) !== null) ||
        loadGate.current.pending()) return;
    const failure = guideAttachmentSelectionError(files, attachments.read(latestState.current?.id), latestState.current?.input_surface);
    if (failure) { attachments.fail(failure); return; }
    preparingAttachments.current = token;
    setAttachmentPreparation(token);
    const owner = latestState.current?.id ? null
      : { id: null, status: "sending", request: null, postEntered: false };
    if (owner) {
      sendRequests.current.set("new", owner);
      processing.current = owner;
    }
    try {
      const current = await ensureStarted(token, owner);
      if (!current || !loadGate.current.current(token) || loadGate.current.pending() || latestState.current?.id !== current.id) return;
      if (owner) releaseSend(owner);
      // Upload owns its guide status; creation no longer owns the composer wait.
      if (preparingAttachments.current === token) {
        preparingAttachments.current = null;
        setAttachmentPreparation(null);
      }
      await attachments.upload(current.id, files);
    } catch (failure) {
      const refused = Number.isInteger(failure.status) && failure.status >= 400 && failure.status < 500;
      if (owner && (refused || !owner.postEntered)) releaseSend(owner);
      if (loadGate.current.current(token)) attachments.fail(owner && owner.postEntered && !refused
        ? "Conversation creation is not confirmed. Check the saved conversation before attaching files again."
        : failure.message);
    } finally {
      if (owner && sendRequests.current.get(owner.id || "new") === owner) {
        owner.status = "unconfirmed";
        if (processing.current === owner) processing.current = false;
        if (setupsMounted.current) redrawSendStatus(value => value + 1);
      }
      if (preparingAttachments.current === token) {
        preparingAttachments.current = null;
        setAttachmentPreparation(null);
      }
    }
  }
  function revisePackagePlan(text, expected) {
    const current = latestState.current;
    if (current?.id !== expected?.id || current.version !== expected?.version ||
        !packagePreparationReason(current) || processing.current || packagePreparing.current || loadGate.current.pending()) return;
    setMobilePanel("chat");
    respond("chat", { text }, { preserveInput: true, expectedGuide: expected });
  }
  function ask(text) {
    setMobilePanel("chat");
    if ([...text].length > 6000) {
      setError("Your note and selected option details are too long to send together. Shorten your note and try again. Your draft and choices are kept.");
      return;
    }
    respond("chat", { text }, { preserveInput: true });
  }
  useEffect(() => {
    const intent = savedBuildReturn;
    if (!intent || savedBuildReturnRef.current !== intent) return;
    const cancel = () => {
      if (savedBuildReturnRef.current === intent) {
        savedBuildReturnRef.current = null;
        setSavedBuildReturn(null);
      }
    };
    if (!active || loadGate.current.pending() || !loadGate.current.current(intent.token) ||
        state?.id !== intent.guideId || state.version !== intent.selectedVersion) { cancel(); return; }
    if (implementationSnapshot?.guideId !== intent.guideId ||
        implementationSnapshot.selectionToken !== intent.token || implementationSnapshot.observation === "loading") return;
    if (!savedBuildNavigationMatches(intent, state, implementationSnapshot, loadGate.current.token())) { cancel(); return; }
    const focusAvailable = () => {
      const document = implementationRef.current?.ownerDocument;
      const focused = document?.activeElement;
      return !!document && (focused === intent.focusOwner || focused === document.body && !intent.focusOwner?.isConnected);
    };
    if (!focusAvailable()) { cancel(); return; }
    releaseResponseFollow();
    forceFollow.current = false;
    setMobilePanel("chat");
    setActionsExpansion({ guideId: intent.guideId, open: true });
    const frame = requestAnimationFrame(() => {
      if (savedBuildReturnRef.current === intent && setupsMounted.current &&
          latestLifecycle.current?.flags.active && !loadGate.current.pending() &&
          savedBuildNavigationMatches(intent, latestState.current, latestBuildSnapshot.current, loadGate.current.token()) &&
          implementationRef.current?.isConnected && focusAvailable()) {
        const target = implementationRef.current;
        target.scrollIntoView({ behavior: "smooth", block: "start" });
        target.focus({ preventScroll: true });
      }
      cancel();
    });
    return () => cancelAnimationFrame(frame);
  }, [savedBuildReturn, active, state?.id, state?.version, implementationSnapshot]);
  const plan = proposalPlan(manualNote, state);
  const proposalUpdated = useForgeProposalNotice({ guideId: state?.id, displayedProposal: plan,
    proposal: state?.proposal || state?.blueprint || null, active,
    current: !loadingSetup && !loadGate.current.pending(), narrow, panel: mobilePanel });
  if (!active) return null;
  if (!state)
    return (
      <div className="setup-loading">
        <GuideAvatar />
        <h1>Meta Forge setup</h1>
        <p role={loadError ? "alert" : undefined}>{loadError || "Opening your conversation..."}</p>
        {loadError && <div className="setup-loading-actions">
          <button disabled={loadingSetup} onClick={load}>Try again</button>
          {failedSelection?.id && <button disabled={loadingSetup}
            onClick={() => startNewAfterReadFailure(failedSelection)}>Start a new setup</button>}
        </div>}
        {loadError && failedSelection?.id && <p>Your saved setup and its drafts stay saved.</p>}
      </div>
    );
  const question = state.question,
    ready = state.status === "ready",
    review = setupProposalReviewable(state);
  const available =
    runtime?.available ?? state.capabilities?.available ?? config?.architect?.available;
  const completed = [...new Set(state.completed_stages || [])];
  const blocked = busy || applyUnconfirmed || sendUnconfirmed || packageBusy || loadingSetup || !!manualNote;
  const hasSavedToolsGuide = typeof state.id === "string" && state.id.trim().length > 0;
  const savedToolsOpen = hasSavedToolsGuide && savedToolsGuide === state.id;
  const consoleUrl = runtime?.browser_console_url ?? null;
  const messages = state.messages || [];
  const opening = !plan && !messages.some(message => message.role === "user") && !question?.options?.length;
  const welcome = isForgeOpening(state, plan, { pending: !!manualNote, busy, loading: loadingSetup });
  const localSending = busy && !loadingSetup && loadGate.current.current(sendingToken.current);
  const firstTurnWaiting = hasForgeOpeningContext(state, plan, { pending: !!manualNote }) && !loadingSetup &&
    !!(localSending || guideWaitBinding(state));
  const firstIdeaRunning = firstTurnWaiting && !!guideWaitBinding(state) && !guideStartupPresentation(state, timeline);
  const resumeSetup = welcome && !providerOpen && setupsRead === "ready"
    ? setups.find(setup => typeof setup?.id === "string" && setup.id.trim() && setup.id !== state.id) : null;
  const requirements = unresolvedRequirements(state, plan);
  const checkedApplication = implementationCanDownload(implementation, state);
  const progress = setupProgress(state, { build: implementation, trial: rehearsal, deployment, remoteDeployment },
    { pendingChanges: !!manualNote, designBusy: busy || applyUnconfirmed || sendUnconfirmed || state.operation?.status === "running", buildObservation, buildAdmissionPending, deploymentObservation, remoteObservation, agentTeamAvailable });
  const hasLifecycle = setupProgressHasLifecycle(state, { build: implementation, trial: rehearsal, deployment, remoteDeployment });
  const applicationBuildRequired = setupNeedsApplicationBuild(state, implementation, { buildObservation });
  const optionalBuildProgress = applicationBuildRequired && !hasLifecycle && state.status !== "ready";
  const designConversationActive = !hasLifecycle && (!!question || optionalBuildProgress);
  const journeyPhase = !firstTurnWaiting && (plan || messages.some(message => message.role === "user") ||
    state.answers?.length || Object.values(state.brief?.facts || {}).some(fact => fact?.summary?.trim()))
    ? "shape" : "idea";
  const actionsOpen = actionsExpansion?.guideId === state.id
    ? actionsExpansion.open : hasLifecycle || !question && !optionalBuildProgress;
  const primaryProgress = progress && !designConversationActive &&
    (hasLifecycle || applicationBuildRequired || progress.kind === "package-pending" ||
      buildObservation !== "known" || deploymentObservation !== "known" || remoteObservation !== "known") ? progress : null;
  const guideResume = setupGuideResume(state, latestLifecycle.current.receipts,
    { ...latestLifecycle.current.flags, setupLoading: loadingSetup });
  const resumeGuide = (resume) => {
    const current = latestLifecycle.current;
    const admitted = admitSetupGuideResume(resume, latestState.current, current.receipts,
      { ...current.flags, setupLoading: loadGate.current.pending(), processing: processing.current }, latestInput.current, respond);
    if (admitted) setMobilePanel("chat");
  };
  const navigateTrial = (mode) => {
    const intent = rehearsalNavigationIntent(mode, state, implementation);
    if (intent) setTrialNavigation(intent);
  };
  const revealActionsTarget = (target) => {
    const guideId = state.id;
    releaseResponseFollow();
    setActionsExpansion({ guideId, open: true });
    requestAnimationFrame(() => {
      if (latestState.current?.id === guideId && target?.isConnected)
        target.scrollIntoView({ behavior: "smooth", block: "start" });
    });
  };
  const navigateInstallation = (sourceKind, displayed = state) => {
    const current = latestState.current, currentBuild = latestImplementation.current, flags = latestLifecycle.current?.flags;
    if (processing.current || packagePreparing.current || loadGate.current.pending() || flags?.designBusy || flags?.pendingChanges || flags?.packageBusy ||
        current?.id !== displayed?.id || current.version !== displayed.version ||
        deploymentSourceKey(currentBuild, current, sourceKind) !== deploymentSourceKey(implementation, displayed, sourceKind) ||
        !deploymentCanPrepare(currentBuild, current, sourceKind)) return false;
    setDeploymentNavigation({ sourceKind, guideId: current.id, version: current.version,
      key: deploymentSourceKey(currentBuild, current, sourceKind) });
    revealActionsTarget(deploymentRef.current);
    return true;
  };
  const navigateProgressAction = (action) => {
    return navigateSetupProgress(action, progress, { onTrial: navigateTrial, onInstall: navigateInstallation, revealTarget: revealActionsTarget,
      targets: { package: teamPackageRef, trial: trialRef, build: implementationRef, deployment: deploymentRef, remote: remoteDeploymentRef } });
  };
  const navigateCurrentStatusAction = (action) => {
    const current = latestState.current, flags = latestLifecycle.current?.flags;
    const target = { package: teamPackageRef, trial: trialRef, build: implementationRef,
      deployment: deploymentRef, remote: remoteDeploymentRef }[action.target]?.current;
    if (!setupsMounted.current || !flags?.active || blocked || loadingSetup || loadGate.current.pending() ||
        processing.current || flags.designBusy || flags.pendingChanges || flags.packageBusy ||
        current?.id !== state.id || current.version !== state.version || !primaryProgress?.actions.includes(action) ||
        action.target !== "console" && !target?.isConnected) return false;
    if (action.target === "console") return true;
    return navigateProgressAction(action);
  };
  const implementationNavigation = progress && implementation ? {
    guide_id: state.id, guide_version: state.version,
    build_id: implementation.build_id, source_digest: implementation.source_digest,
    actions: progress.actions,
  } : null;
  const statusText = guideStatusText({ state, error, appError, busy, ready, review, question, progress: primaryProgress, agentTeamAvailable });
  const file =
    validation?.files?.find((f) => f.path === selectedFile) ||
    validation?.files?.[0];
  const startingContext = welcome && !state?.organization ? startingContextDetails(inputStartingHelp(input)) : null;
  const startingChoicesOnly = !!startingContext && !draft.trim() && !attachments.refs.length;
  const submissionText = inputSubmissionText(input);
  const filesOnly = attachments.refs.length > 0 && !submissionText.trim() && !selected.length;
  const selectedAnswers = composeMode === "answer" && input.key === inputKey(state)
    ? selected.map(id => question?.options?.find(option => option.id === id)).filter(Boolean) : [];
  const optionalAnswerContext = selectedAnswers.length > 0 && question.allow_text !== false;
  const composer = {
    value: draft,
    placeholder:
      firstIdeaRunning ? "Write your next message - you can send it after the reply." : startingContext
        ? `For example: ${startingContext.kind.example}` : optionalAnswerContext ? "Add context (optional)..." : !composerFocused && !draft
        ? opening ? "Describe a task or idea. A sentence is enough to start..." : question && composeMode === "answer" ? "Your answer..." : "Ask or suggest a change..."
        : opening
        ? "Describe a task or idea. A sentence is enough to start..."
        : question && composeMode === "answer"
        ? "Answer in your own words, or add a little context..."
        : "Ask a question or tell your guide what to change...",
    disabled: packageBusy,
    submitDisabled:
      blocked ||
      welcome && providerOpen ||
      attachmentPreparing || attachments.uploading || attachments.pending.length > 0 ||
      (!(welcome && available === false) && !(question && composeMode === "answer"
        ? canSubmitAnswer(question, selected, submissionText, combineOptions) || filesOnly
        : submissionText.trim() || attachments.refs.length)),
    submitLabel:
      firstIdeaRunning ? "Waiting for guide" : question && composeMode === "answer" && !filesOnly ? "Send answer" : "Send message",
    mainRowItems: [{ id: "guide-attach", kind: narrow ? "pill-icon" : "pill",
      label: narrow ? undefined : "Attach files", iconName: narrow ? "plus" : undefined,
      disabled: loadingSetup || packageBusy || attachmentPreparing || attachments.uploading ||
        attachments.pending.length > 0 || attachments.refs.length >= GUIDE_ATTACHMENT_LIMIT }],
    footerLeftItems: [],
    footerRightItems: [],
  };
  return (
    <ForgeIllustrationMotionProvider><div
      className={`setup-shell setup-mobile-${mobilePanel}${designConversationActive && !welcome && !firstTurnWaiting && !plan ? " setup-first-conversation" : ""}${welcome ? " setup-welcome" : ""}${firstTurnWaiting ? " setup-first-turn" : ""}${welcome && providerOpen ? " setup-welcome-provider" : ""}${composerFocused ? " setup-composer-focused" : ""}${draft ? " setup-has-draft" : ""}`}
      data-cc-theme="light"
    >
      <header className="setup-header" onKeyDown={(event) => {
        if (narrow && mobileMenuOpen && event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          setMobileMenuOpen(false);
          setHistoryOpen(false);
          mobileMenuButton.current?.focus();
        }
      }}>
        <div className="setup-brand">
          <img src="/assets/meerkat-transparent.png" alt="" />
          <strong>Meta Forge</strong>
        </div>
        {welcome || firstTurnWaiting || designConversationActive ? <ForgeWelcomeOrientation
          phase={journeyPhase} /> : <span className="setup-header-caption">
          An idea. A conversation. Your own agents.
        </span>}
        <button type="button" className="setup-mobile-menu-toggle" ref={mobileMenuButton}
          aria-expanded={mobileMenuOpen} aria-controls="setup-header-menu"
          onClick={() => setMobileMenuOpen(value => !value)}>
          Menu <Icon name="chevron" size={13} />
        </button>
        <div id="setup-header-menu" className={`setup-header-actions${mobileMenuOpen ? " is-open" : ""}`}>
          <button onClick={() => { setProviderOpen(true); setMobilePanel("chat"); setMobileMenuOpen(false); }} aria-expanded={providerOpen}>
            Guide settings
            <small className="setup-provider-model">{available === false ? "Provider setup needed"
              : runtime?.model || state.capabilities?.model || config?.architect?.model || "Meerkat + MobKit"}</small>
          </button>
          <button aria-expanded={savedToolsOpen} disabled={!hasSavedToolsGuide || blocked}
            onClick={() => { releaseResponseFollow();
              setMobileMenuOpen(false);
              const guideId = state.id;
              setSavedToolsGuide(value => value === guideId ? null : guideId);
              setMobilePanel("chat");
              requestAnimationFrame(() => {
                if (latestState.current?.id === guideId)
                  chatScroll.current?.scrollTo?.({ top: 0, behavior: "smooth" });
              }); }}>
            Saved tools
          </button>
          <div className="setup-history-control">
            <button
              aria-expanded={historyOpen}
              onClick={() => {
                setHistoryOpen(!historyOpen);
                if (!historyOpen) refreshSetups({ ifIdle: true });
              }}
            >
              Your setups <Icon name="chevron" size={13} />
            </button>
            {historyOpen && (
              <div className="setup-history-menu" aria-busy={setupsRead === "loading"}>
                <strong>Recent setups</strong>
                {setupsRead === "loading" && <p role="status">{setups.length
                  ? "Updating saved setups..." : "Reading saved setups..."}</p>}
                {setupsRead === "failed" && <>
                  <p role="alert">{setups.length
                    ? "Saved setups could not be refreshed. The list below is from the last successful check."
                    : "Saved setups could not be read. Your current setup is unchanged."}</p>
                  <button type="button" onClick={() => refreshSetups({ ifIdle: true })}>Retry saved setups</button>
                </>}
                {setups.slice(setupsPage * 8, setupsPage * 8 + 8).map((setup) => (
                  <div className="setup-history-entry" key={setup.id}>
                    <button className="setup-history-conversation"
                      onClick={() => switchSetup(setup.id)}
                      aria-current={setup.id === state.id ? "true" : undefined}>
                      <span>{setup.name || "Untitled setup"}</span>
                      <small>
                      {setup.status === "ready"
                        ? "Accepted plan"
                        : setup.status === "review"
                          ? "Design ready to review"
                          : "Design in progress"}
                      </small>
                    </button>
                    <SavedSetupBuildStatus setup={setup} compact active={active && setupsRead === "ready"}
                      onOpenBuild={observed => switchSetup(setup.id, observed)} />
                  </div>
                ))}
                {setups.length > 8 && <nav className="setup-history-pages" aria-label="Saved setup pages">
                  <button type="button" disabled={setupsPage === 0}
                    onClick={() => setSetupsPage(value => Math.max(0, value - 1))}>Newer setups</button>
                  <span>{setupsPage * 8 + 1}-{Math.min(setupsPage * 8 + 8, setups.length)} of {setups.length}</span>
                  <button type="button" disabled={(setupsPage + 1) * 8 >= setups.length}
                    onClick={() => setSetupsPage(value => value + 1)}>Older setups</button>
                </nav>}
                {!setups.length && setupsRead === "ready" && (
                  <p>Your conversations are saved here as you go.</p>
                )}
                <button className="setup-history-new" onClick={newSetup}>
                  + Start a new setup
                </button>
              </div>
            )}
          </div>
          <button
            className={configOpen ? "selected" : ""}
            onClick={() => { setMobileMenuOpen(false); openConfig(); }}
            disabled={!plan || busy || packageBusy}
          >
            <Icon name="code" size={15} /> Configuration
          </button>
          {narrow && consoleUrl && <a href={consoleUrl} target="_blank" rel="noreferrer"
            aria-label="Open the Forge setup guide console in a new tab">Guide console <Icon name="arrow" size={12} /></a>}
        </div>
      </header>
      {narrow && !welcome && (firstTurnWaiting || designConversationActive) && <div className="forge-mobile-journey">
        <ForgeWelcomeOrientation phase={journeyPhase} />
      </div>}
      <nav className="setup-mobile-tabs" aria-label="Setup views">
        <button
          aria-pressed={mobilePanel === "chat"}
          className={mobilePanel === "chat" ? "active" : ""}
          onClick={() => setMobilePanel("chat")}
        >
          Conversation
        </button>
        <button
          aria-pressed={mobilePanel === "plan"}
          aria-label={proposalUpdated ? "Your team - proposal updated" : undefined}
          title={proposalUpdated ? "Your saved proposal has changed" : undefined}
          className={mobilePanel === "plan" ? "active" : ""}
          onClick={() => setMobilePanel("plan")}
        >
          Your team {plan && <span>{identities(plan).length}</span>}
          {proposalUpdated && <i className="setup-team-update-dot" aria-hidden="true" />}
        </button>
      </nav>
      {narrow && mobilePanel === "plan" && <div className="setup-mobile-response" aria-label="Guide response available">
        <GuideResponseNotice state={state} publicationKey={unseenResponse} busy={busy}
          onReveal={revealMobileResponse} Icon={Icon} />
      </div>}
      {appToast && (
        <div className="setup-toast" role="status">
          {appToast}
        </div>
      )}
      <div
        className="setup-visually-hidden"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {statusText}
      </div>
      <div className="setup-columns">
        <section ref={conversationRef} tabIndex={-1} className="setup-conversation" aria-label="Setup conversation">
          <div className={`setup-chat-top ${hasLifecycle ? "has-lifecycle" : ""}`}>
            <span>
              <i /> Your setup guide
            </span>
            <SetupGuideStatusCaption lifecycle={!!primaryProgress} statusText={statusText} ready={ready} review={review} />
            {(state.proposal || state.blueprint) && (
              <nav className="setup-stage-shortcuts" aria-label="Try, build and run this proposal">
                {designConversationActive ? <a href="#setup-trial" title="Optional ways to try, build and run this proposal"
                  onClick={(event) => {
                    event.preventDefault();
                    revealActionsTarget(trialRef.current);
                  }}>Try, build and run</a> : <>
                <SetupTrialShortcut progress={progress} onNavigate={navigateProgressAction}
                  onDesign={() => {
                    navigateTrial("design");
                    revealActionsTarget(trialRef.current);
                  }} />
                {progress?.kind !== "package-pending" && <a href="#setup-implementation" title="Build the application from this proposal"
                  onClick={(event) => {
                    event.preventDefault();
                    revealActionsTarget(implementationRef.current);
                  }}>Build</a>}
                {(deploymentCanPrepare(implementation, state) || deployment?.guide_id === state.id) &&
                  <a href="#setup-deployment" title="Install or manage this application"
                    onClick={(event) => {
                      event.preventDefault();
                      revealActionsTarget(deploymentRef.current);
                    }}>Run</a>}
                </>}
              </nav>
            )}
            {consoleUrl && (
              <a
                href={consoleUrl}
                target="_blank"
                rel="noreferrer"
                title="Inspect the live Forge setup team"
                aria-label="Open the Forge setup guide console in a new tab"
              >
                Guide console <Icon name="arrow" size={12} />
              </a>
            )}
          </div>
          {!busy && !error && !appError && !savedGuideFailure(state) &&
            <SetupCurrentStatusAnchor progress={primaryProgress} statusText={statusText}
              disabled={blocked || loadingSetup || !active} onNavigate={navigateCurrentStatusAction} Icon={Icon} />}
          {narrow && <div className="setup-mobile-response">
            <GuideResponseNotice state={state} publicationKey={unseenResponse} busy={busy}
              onReveal={(key) => revealResponse(true, key)} Icon={Icon} />
          </div>}
          <div
            className="setup-chat-scroll"
            ref={chatScroll}
            onWheel={releaseResponseFollow}
            onPointerDown={releaseResponseFollow}
            onTouchMove={releaseResponseFollow}
            onKeyDown={releaseResponseFollow}
            onScroll={(event) => {
              const node = event.currentTarget;
              recordQuestionVisibility();
              const tail = busy ? guideActivityScrollTop(node) : node.scrollHeight - node.clientHeight;
              const nearBottom = tail !== null && Math.abs(tail - node.scrollTop) < 120;
              setFollowing(nearBottom);
              if (guidePublicationVisible(latestState.current,
                { scroll: node, question: questionRef.current, response: responseRef.current })) {
                publicationUnread.current.markSeen(latestState.current, guidePublicationKey(latestState.current));
                setUnseenResponse(publicationUnread.current.observe(latestState.current));
              }
            }}
          >
            <div className="setup-conversation-start">LET'S MAKE IT YOURS</div>
            {state.id && <ForgeOrganizationContext update={state.organization_update} disabled={blocked}
              onAdopt={organization_fingerprint => respond("organization", { organization_fingerprint },
                { preserveInput: true, expectedGuide: { id: state.id, version: state.version } })} />}
            {providerOpen && <ProviderSetup
              firstRun={welcome} idea={draft} startingContext={inputStartingHelp(input)} guideReady={runtime?.status === "ready"}
              onRuntime={publishRuntime} onClose={() => {
                const guideId = state.id;
                setProviderOpen(false); setProviderDismissed(true);
                requestAnimationFrame(() => {
                  if (latestState.current?.id === guideId && inputRef.current?.isConnected)
                    inputRef.current.focus();
                });
              }} />}
            {savedToolsOpen && <ToolkitSavedSetup key={state.id} guideId={state.id} version={state.version}
              disabled={blocked} onDiscuss={text => {
                setMobilePanel("chat");
                respond("chat", { text, attachments: [] }, { preserveInput: true,
                  expectedGuide: { id: state.id, version: state.version } });
              }} onClose={() => setSavedToolsGuide(null)} />}
            <NativeGuideChat
              state={state}
              snapshot={timeline}
              pendingMessage={pendingMessage}
              Icon={Icon}
            />
            {failedSelection && loadError && <div className="setup-error" role="alert">
              <strong>That setup did not open.</strong>
              <p>{plan?.project?.name || "Your previous setup"} is still selected. Its saved work and running operations are kept.</p>
              <button disabled={loadingSetup} onClick={() => load(failedSelection.id)}>Retry selected setup</button>
            </div>}
            <div
              ref={responseRef}
              className="setup-response-anchor"
              data-guide-id={state?.id || ""}
              data-guide-publication={guidePublicationKey(state) || ""}
              tabIndex={-1}
              aria-label="Latest guide response"
            />
            {(error || appError) && (
              <div className="setup-error" role="alert">
                <strong>Let's pick up from here.</strong>
                <p>{error || appError}</p>
                <div>
                  <button
                    onClick={() => {
                      forceFollow.current = true;
                      setError("");
                      onDismissError?.();
                    }}
                  >
                    Dismiss
                  </button>
                  <button onClick={load}>Reload saved conversation</button>
                  {available === false && (
                    <button onClick={openDetails}>Configure manually</button>
                  )}
                </div>
              </div>
            )}
            {manualNote && (
              <div className="setup-manual-note">
                <Icon name="settings" size={16} />
                <div>
                  <strong>Your adjustments</strong>
                  <p>{manualNote.changes.join(" · ")}</p>
                  <small>
                    {busyAction === "manual" && matchesManual(manualNote, state.operation?.request, state.operation?.request_id)
                      ? "The guide is reviewing these changes."
                      : manualNote.manual_intent
                      ? `${manualNote.proposal_changed ? "The proposal changed while you edited. " : ""}Your edits are saved. Ask the guide to reconcile them with the latest plan.`
                      : manualNote.review_unavailable
                      ? "Your earlier draft is saved, but its original plan could not be matched. Adjust it to review new edits against the current plan, or discard the pending changes."
                      : "Saved on this device. Send them to your guide to continue."}
                  </small>
                  {!busy && (
                    <div className="setup-manual-actions">
                      <button
                        disabled={manualNote.review_unavailable === true}
                        onClick={() =>
                          respond("manual", manualRequest(manualNote), {
                            expectedGuide: { id: state.id, version: state.version },
                          })
                        }
                      >
                        {manualNote.manual_intent ? "Review changes against current plan" : "Review changes"} <Icon name="arrow" size={14} />
                      </button>
                      <button onClick={openDetails}>Adjust changes</button>
                      <button onClick={discardManual}>
                        Discard pending changes
                      </button>
                    </div>
                  )}
                </div>
              </div>
            )}
            <SetupOperationFailure state={state} busy={busy} failureRef={operationFailureRef} onRetry={retryOperation}
              onReviewDraft={reviewCurrentDraft} reviewDraftDisabled={blocked || !active} />
            {progress && !busy && !designConversationActive && <SetupProgressCard progress={progress} primary={!!primaryProgress} resume={guideResume}
              onResume={resumeGuide} onNavigate={navigateProgressAction} disabled={blocked || loadingSetup} Icon={Icon} />}
            {question && !firstTurnWaiting && (
              <div className={`setup-current-question ${opening ? "setup-opening" : ""}`} ref={questionRef}
                data-guide-id={state.id} data-guide-version={state.version}>
                <AgentMessage>
                  <div className="setup-bubble setup-question-bubble">
                    {welcome ? <ForgeWelcomeIntro /> : <SetupGuideQuestionHeading question={question} lifecycle={!!primaryProgress} waiting={busy} />}
                    <div className="setup-interactive-answer">
                      {opening && !welcome && <SetupStartingPoints organization={state.organization} draft={draft} disabled={blocked}
                        onReturnToTyping={() => inputRef.current?.focus()} onChoose={(text) => {
                        setDraft(text); setComposeMode("answer"); inputRef.current?.focus();
                      }} />}
                      {!welcome && <SetupGuideChoices question={question} selected={selected}
                        helpDraft={draft} combineOptions={combineOptions}
                        disabled={blocked} Icon={Icon} showCaption={!opening} onSelect={(choices) => {
                          setSelected(choices);
                          setComposeMode("answer");
                        }}
                        onCombine={(combine) => setInput(previous => {
                          const next = { ...previous, selected: combine || previous.selected.length <= 1 ? previous.selected : [] };
                          if (combine) next.combine_options = true;
                          else delete next.combine_options;
                          return next;
                        })}
                        onAsk={ask} />}
                    </div>
                  </div>
                </AgentMessage>
              </div>
            )}
            {agentTeamAvailable && (state.proposal || state.blueprint) && !busy && <NativeTeamPackage state={state} packageBusy={packageBusy} blocked={blocked}
              deploymentObservation={deploymentObservation}
              onInstall={(displayed) => navigateInstallation("native-team", displayed)}
              onPackage={inspectPackage} Icon={Icon} targetRef={teamPackageRef} downloadedKey={nativeDownloadedKey} onRevise={revisePackagePlan} />}
            {(state.proposal || state.blueprint) && <details className="setup-proposal-actions" open={actionsOpen}>
              <summary onClick={(event) => {
                event.preventDefault();
                setActionsExpansion({ guideId: state.id, open: !actionsOpen });
              }}>
                <strong>Try, build and run</strong>
                <span>Explore this proposal or prepare it to use.</span>
              </summary>
              {designConversationActive && progress && !busy && <SetupProgressCard progress={progress} resume={guideResume}
                onResume={resumeGuide} onNavigate={navigateProgressAction} disabled={blocked || loadingSetup} Icon={Icon} />}
              <div id="setup-trial" ref={trialRef}>
              <RehearsalPanel state={state} plan={plan} build={implementation} busy={busy || applyUnconfirmed || sendUnconfirmed || state.operation?.status === "running"}
                selectionScope={lifecycleSelection.current} pendingManual={!!manualNote} navigation={trialNavigation}
                onActivity={acceptRehearsalActivity} onRepairReceipt={acceptImplementationAdmission} onReceipt={acceptRehearsalReceipt} />
              </div>
              <div ref={implementationRef} role="region" aria-label="Application build" tabIndex={-1}>
              <p className="setup-custom-application-intro"><strong>Custom application</strong>
                <span>{agentTeamAvailable ? "Use the builder when your requested outcomes need custom screens or application code. The agent team package does not implement remaining application work."
                  : "Build application turns your proposal into an application and installs your agents with it."}</span></p>
              <ImplementationPanel state={state} plan={plan} designBusy={busy || applyUnconfirmed || sendUnconfirmed || state.operation?.status === "running"}
                selectionScope={lifecycleSelection.current} pendingManual={!!manualNote} Icon={Icon} admittedReceipt={implementationAdmission}
                navigation={implementationNavigation} onNavigate={navigateProgressAction} onRevise={revisePackagePlan}
                onReceipt={acceptImplementationReceipt} onDownloaded={acceptApplicationDownloaded} />
              </div>
              <div id="setup-deployment" ref={deploymentRef}>
              <DeploymentPanel state={state} plan={plan} build={implementation}
                initialSourceKind={!agentTeamAvailable || deploymentCanPrepare(implementation, state) ? "application" : "native-team"}
                agentTeamAvailable={agentTeamAvailable}
                sourceNavigation={deploymentNavigation}
                selectionScope={lifecycleSelection.current} designBusy={busy || applyUnconfirmed || sendUnconfirmed || state.operation?.status === "running"} pendingManual={!!manualNote} Icon={Icon}
                onReceipt={acceptDeploymentReceipt} onRemoteReceipt={acceptRemoteDeploymentReceipt} remoteTargetRef={remoteDeploymentRef}
                onRepairReceipt={acceptImplementationAdmission} onChooseCloudProject={chooseCloudProject} />
              </div>
            </details>}
            {review && !busy && plan && (
              <AgentMessage>
                <div className="setup-review-card">
                  <span className="setup-eyebrow">PROPOSAL TO REVIEW</span>
                  <h2>{plan.project.name}</h2>
                  <p>{plan.project.description || state.summary}</p>
                  <div className="setup-package-tags">
                    <span>{identities(plan).length} agents</span>
                    <span>{plan.connections.length} {plan.connections.length === 1 ? "connection" : "connections"}</span>
                    <span>
                      {plan.target === "gcp"
                        ? "Google Cloud"
                        : plan.target === "mac"
                          ? "macOS"
                          : "Linux"}
                    </span>
                  </div>
                  {requirements.length > 0 && (
                    <div className="setup-readiness">
                      <strong>Before this can run</strong>
                      <SetupRequirementGroups state={state} />
                    </div>
                  )}
                  <ScopeDecisions state={state} />
                  <p className="setup-review-expectation">
                    {packagePreparationReason(state, plan)
                      ? "Accepting saves this design and its choices. Package preparation is pending; you can keep refining the plan."
                      : "Accepting prepares the native team and configuration. Use Build for custom application code. Enter declared private settings during installation, then verify the actual connections separately."}
                  </p>
                  {packagePreparationReason(state, plan) && <div className="setup-readiness" role="status">
                    <p>{packagePreparationReason(state, plan)}</p>
                    <button className="setup-action" disabled={blocked}
                      onClick={() => revisePackagePlan(PACKAGE_REVISION_PROMPT, { id: state.id, version: state.version })}>Revise with guide <Icon name="arrow" size={16} /></button>
                  </div>}
                  <button
                    className="setup-action primary"
                    onClick={() => applyPlan(state)}
                    disabled={blocked}
                  >
                    Use this plan <Icon name="arrow" size={16} />
                  </button>
                  <small className="setup-card-foot">
                    Or tell me what you would change below.
                  </small>
                </div>
              </AgentMessage>
            )}
            {(ready || state.status === "handoff") && !busy && plan && <ManualSetupExports
              plan={plan} showPackage={ready && checkedApplication} checkedApplication={checkedApplication}
              packageBusy={packageBusy} blocked={blocked} briefBlocked={blocked || loadingSetup} downloaded={downloaded} saving={saving}
              onPackage={inspectPackage} onBrief={downloadHandoff} onSave={onSave} onAdjust={openDetails} Icon={Icon} />}
            {completed.length > 0 && !busy && (
              <details className="setup-earlier">
                <summary>Revisit an earlier decision</summary>
                <div>
                  {GUIDE_STAGES.filter(
                    (s) => completed.includes(s.id) && s.id !== "review",
                  ).map((s) => (
                    <button
                      disabled={blocked}
                      key={s.id}
                      onClick={() => respond("back", { stage: s.id })}
                    >
                      {s.name}
                      <Icon name="arrow" size={13} />
                    </button>
                  ))}
                </div>
              </details>
            )}
          </div>
          <div className="setup-composer-area" onBlurCapture={(event) => {
            if (event.target === inputRef.current) setComposerFocused(false);
          }}>
            <GuideRecoveryNotice persistent={browserRecovery} />
            {sendUnconfirmed && <div className="setup-guide-wait setup-delivery-recovery" aria-label="Message delivery status">
              <div className="setup-guide-wait-heading"><p role="status">
                {creationUnconfirmed ? "Conversation creation is not confirmed" : "Message delivery is not confirmed"}
              </p></div>
              <small>{creationUnconfirmed
                ? "Your writing is kept. Check for the saved conversation, or open a new draft. Neither action sends your writing. The original request may still finish."
                : "Your writing is kept. Check the saved conversation before continuing. Checking does not send your message again."}</small>
              <div className="setup-delivery-actions">
                <button type="button" className="setup-guide-follow" disabled={loadingSetup} onClick={() => {
                  if (loadGate.current.pending()) return;
                  if (latestState.current?.id === state.id && state.id) load(state.id);
                  else if (!latestState.current?.id) checkCreatedGuide();
                }}>Check saved conversation</button>
                {creationUnconfirmed && !state.id && <button type="button" className="setup-delivery-new-draft"
                  disabled={loadingSetup || !active} onClick={(event) => {
                    const ownsSelection = lifecycleSelection.current.capture(state, lifecycleToken);
                    if (!event.currentTarget?.isConnected || !setupsMounted.current ||
                        !latestLifecycle.current?.flags.active || !ownsSelection || !ownsSelection() ||
                        latestState.current !== state || latestState.current?.id ||
                        latestInput.current.key !== inputKey(state) ||
                        creationStorage.getItem(GUIDE_CREATION_KEY) !== selectedCreationRequest ||
                        sendRequests.current.get("new") !== sendingMessage) return;
                    newSetup();
                  }}>Open a new draft</button>}
              </div>
              {creationUnconfirmed && creationCheckDetail && <small role="status">{creationCheckDetail}</small>}
            </div>}
            {applyUnconfirmed && <div className="setup-guide-wait" aria-label="Plan save status"
              data-guide-id={state.id} data-guide-version={applyingPlan.version}>
              <div className="setup-guide-wait-heading"><p role="status">
                {localSending && busyAction === "apply" ? "Saving your plan" : "Plan save is not confirmed"}
              </p></div>
              <small>Your draft is kept. Check the saved plan before continuing. Checking does not accept it again.</small>
              <button type="button" disabled={loadingSetup} onClick={() => {
                if (!loadGate.current.pending() && latestState.current?.id === state.id) load(state.id);
              }}>Check saved plan</button>
              {applyingPlan.checked && <button type="button"
                disabled={busy || loadingSetup || packageBusy || !!manualNote}
                onClick={() => applyPlan(state, applyingPlan)}>Try saving this plan again</button>}
            </div>}
            <GuideWaitStatus state={loadingSetup ? null : state} snapshot={timeline}
              submitting={localSending && busyAction !== "apply"} firstIdea={firstTurnWaiting}
              following={following} onFollow={() => revealResponse(true)} />
            {!narrow && <GuideResponseNotice state={state} publicationKey={unseenResponse} busy={busy}
              onReveal={(key) => revealResponse(true, key)} Icon={Icon} />}
            {question && !firstTurnWaiting && !opening && !(questionInView?.id === state.id && questionInView.version === state.version && questionInView.visible) &&
              <button type="button" className="setup-question-return"
              onClick={revealCurrentQuestion} aria-label={`Show current question: ${question.title}`}>
              <span>Current question</span>
              <strong>{question.title}</strong>
              <Icon name="arrow" size={14} />
            </button>}
            {question && !welcome && !firstTurnWaiting && (
              <div
                className="setup-composer-mode"
                role="group"
                aria-label="Message purpose"
              >
                <button
                  aria-pressed={composeMode === "answer"}
                  className={composeMode === "answer" ? "active" : ""}
                  onClick={() => {
                    setComposeMode("answer");
                    inputRef.current?.focus();
                  }}
                >
                  Answer this question
                </button>
                <button
                  aria-pressed={composeMode === "chat"}
                  className={composeMode === "chat" ? "active" : ""}
                  onClick={() => {
                    setComposeMode("chat");
                    inputRef.current?.focus();
                  }}
                >
                  Ask or suggest a change
                </button>
              </div>
            )}
            {(!welcome || providerOpen) && <label className="setup-visually-hidden" htmlFor="setup-message">
              {firstIdeaRunning ? "Your next message" : startingContext ? startingContext.kind.prompt : welcome ? "Describe your idea" : composeMode === "answer" && question
                ? "Your answer to the setup guide"
                : "Message your setup guide"}
            </label>}
            {welcome && <ForgeStartingHelp organization={state.organization} key={lifecycleToken} inlineStart separateContext draft={draft} disabled={blocked} composerId="setup-message"
              appliedContext={inputStartingHelp(input)}
              editRequest={startingHelpEdit?.key === input.key ? startingHelpEdit : null}
              onChooseContext={starting_help => {
                const guideId = state.id, key = input.key, editor = inputRef.current;
                const editTrigger = startingHelpEdit?.key === key ? startingHelpEdit.trigger : null;
                setStartingHelpEdit(null);
                setInput(previous => {
                  if (previous.key !== key || previous.draft !== draft || inputKey(latestState.current) !== key) return previous;
                  const next = { ...previous, mode: "answer", starting_help };
                  if (!inputStartingHelp(next)) delete next.starting_help;
                  return next;
                });
                const focusOwner = editor?.ownerDocument.activeElement;
                requestAnimationFrame(() => {
                  if (latestState.current?.id !== guideId || latestInput.current.key !== key ||
                      inputKey(latestState.current) !== key || latestInput.current.draft !== draft ||
                      inputRef.current !== editor || !editor?.isConnected) return;
                  const committed = inputStartingHelp(latestInput.current);
                  if (committed?.version !== starting_help.version || committed.audience_id !== starting_help.audience_id ||
                      committed.kind_id !== starting_help.kind_id) return;
                  const currentFocus = editor.ownerDocument.activeElement;
                  if (currentFocus !== focusOwner && !(currentFocus === editor.ownerDocument.body &&
                      focusOwner && (!focusOwner.isConnected || focusOwner.closest("[hidden]")))) return;
                  // A choices-only start should reveal its handoff without opening a mobile keyboard.
                  // Returning from a single-field edit keeps focus on the original review control.
                  const reviewControl = editTrigger?.isConnected &&
                    editTrigger.closest(".setup-composer-area") === editor.closest(".setup-composer-area")
                    ? editTrigger : null;
                  const target = reviewControl || (!draft.trim() ? editor.ownerDocument.getElementById("setup-send") : editor);
                  target?.focus();
                });
              }}
              onReturnToTyping={() => {
                const guideId = state.id, key = input.key, editor = inputRef.current;
                // Organization examples never hide or replace the composer.
                if (state.organization) { editor?.focus(); return; }
                const focusOwner = editor?.ownerDocument.activeElement;
                setStartingHelpEdit(null);
                requestAnimationFrame(() => {
                  if (latestState.current?.id !== guideId || latestInput.current.key !== key ||
                      inputKey(latestState.current) !== key || latestInput.current.draft !== draft ||
                      inputRef.current !== editor || !editor?.isConnected) return;
                  const currentFocus = editor.ownerDocument.activeElement;
                  if (currentFocus !== focusOwner && !(currentFocus === editor.ownerDocument.body &&
                      focusOwner && (!focusOwner.isConnected || focusOwner.closest("[hidden]")))) return;
                  editor.focus();
                });
              }} onChoose={(text, starting_help) => {
                const guideId = state.id, key = input.key, editor = inputRef.current;
                setInput(previous => {
                  if (previous.key !== input.key || previous.draft !== draft) return previous;
                  const next = { ...previous, draft: text, mode: "answer", starting_help };
                  if (!inputStartingHelp(next)) delete next.starting_help;
                  return next;
                });
                editor?.focus();
                requestAnimationFrame(() => {
                  if (latestState.current?.id !== guideId || latestInput.current.key !== key ||
                      inputKey(latestState.current) !== key || latestInput.current.draft !== text ||
                      inputRef.current !== editor || !editor?.isConnected ||
                      editor.ownerDocument.activeElement !== editor) return;
                  editor.setSelectionRange(editor.value.length, editor.value.length);
                });
              }} />}
            {welcome && startingContext && <ForgeStartingContext context={inputStartingHelp(input)} composerId="setup-message" disabled={blocked}
              onEdit={(step, trigger) => setStartingHelpEdit(previous => ({ key: input.key, step, trigger, sequence: (previous?.sequence || 0) + 1 }))}
              onRemove={() => {
                setStartingHelpEdit(null);
                setInput(previous => {
                  if (previous.key !== input.key || previous.draft !== draft) return previous;
                  const next = { ...previous }; delete next.starting_help; return next;
                });
                inputRef.current?.focus();
              }} />}
            {welcome && !providerOpen && <div className="forge-welcome-input-heading" data-context={Boolean(startingContext)}>
              <label htmlFor="setup-message">{startingContext ? startingContext.kind.prompt : "Your idea"}</label><small id="setup-idea-optional">{startingContext ? "Optional" : "A sentence is enough."}</small>
            </div>}
            {welcome && !providerOpen && startingContext && <ForgeStartingExample context={inputStartingHelp(input)} composerId="setup-message" />}
            {selectedAnswers.length > 0 && <div className="setup-selected-answer" role="group" aria-label="Selected answer, not sent">
              <span>Your answer</span>
              <div className="setup-selected-answer-choices">
                {selectedAnswers.map(option => <button type="button" key={option.id} disabled={blocked}
                  aria-label={`Remove ${option.label} from answer`} onClick={() => setInput(previous => {
                    if (previous.key !== input.key || previous.key !== inputKey(latestState.current)) return previous;
                    return { ...previous, selected: previous.selected.filter(id => id !== option.id) };
                  })}>
                  <span>{option.label}</span><span aria-hidden="true">&#215;</span>
                </button>)}
              </div>
            </div>}
            <ConsoleComposer
              viewState={composer}
              Icon={Icon}
              inputRef={inputRef}
              inputId="setup-message"
              inputDescriptionId={welcome && !providerOpen ? [
                startingContext && "setup-idea-optional",
                startingContext && "setup-message-starting-example",
                "setup-after-send",
                !narrow && "setup-idea-shortcuts",
              ].filter(Boolean).join(" ") : undefined}
              submitButtonId="setup-send"
              onChange={setDraft}
              onFocus={() => { setComposerFocused(true); setMobileMenuOpen(false); }}
              onSubmit={submitComposer}
              renderMainRow={(welcome && !providerOpen || firstIdeaRunning) ? () => <div className="forge-welcome-composer-actions">
                <button type="button" className="forge-welcome-attach" disabled={composer.mainRowItems[0].disabled}
                  aria-label="Attach files" title="Attach files" aria-controls="setup-attachments-picker"
                  onClick={() => attachmentPicker.current?.click()}><Icon name="plus" size={16} /><span>Attach files</span></button>
                <button type="button" id="setup-send" className="setup-welcome-continue"
                  aria-describedby={welcome && !providerOpen && available !== false ? "setup-after-send" : undefined}
                  title={firstIdeaRunning ? "Your guide is working. You can send this draft after the reply." : available === false ? "Connect your guide, keeping your idea unsent" : narrow
                    ? "Send to guide. Return adds a new line."
                    : "Send to guide (Enter). Shift + Enter adds a new line."}
                  disabled={composer.submitDisabled} onClick={submitComposer}>{firstIdeaRunning ? "Waiting for guide" : available === false ? "Connect your guide" : "Send to guide"} <Icon name="arrow" size={16} /></button>
              </div> : undefined}
              getToolbarButtonProps={({ item }) => item.id === "guide-attach" ? {
                onClick: () => attachmentPicker.current?.click(), "aria-controls": "setup-attachments-picker",
                "aria-label": "Attach files", title: "Attach files",
              } : {}}
              onKeyDown={(e) => {
                if (
                  !narrow && e.key === "Enter" &&
                  !e.shiftKey &&
                  !e.nativeEvent.isComposing
                ) {
                  e.preventDefault();
                  if (!composer.submitDisabled) submitComposer();
                }
              }}
            />
            {firstIdeaRunning && <p className="forge-next-draft-hint">Your next draft stays here. Send it when your guide replies.</p>}
            {welcome && !providerOpen && !narrow && <small id="setup-idea-shortcuts" className="forge-welcome-shortcuts">{available === false ? "Enter to connect your guide" : "Enter to send"} <span aria-hidden="true">-</span> Shift + Enter for a new line</small>}
            <input ref={attachmentPicker} id="setup-attachments-picker" type="file" multiple
              className="setup-visually-hidden" tabIndex={-1} accept={guideAttachmentAccept(state.input_surface)}
              aria-label="Choose files for your setup guide"
              onChange={(event) => {
                const files = [...event.currentTarget.files];
                event.currentTarget.value = "";
                if (files.length) chooseAttachments(files);
              }} />
            <GuideAttachmentChips attachments={attachments} preparing={attachmentPreparing} inputSurface={state.input_surface} />
            {welcome && !providerOpen && <ForgeWelcomeNext needsConnection={available === false} choicesOnly={startingChoicesOnly} />}
            {welcome && <>
              {resumeSetup && <section className="forge-welcome-resume" aria-label="Pick up where you left off">
                <div className="forge-welcome-resume-content"><span>Pick up where you left off</span>
                  <strong>{resumeSetup.name || "Untitled setup"}</strong>
                  <small>{resumeSetup.status === "ready" ? "Accepted plan"
                    : resumeSetup.status === "review" ? "Design ready to review" : "Design in progress"}</small>
                  <SavedSetupBuildStatus setup={resumeSetup} active={active && !loadingSetup} disabled={blocked}
                    onOpenBuild={observed => { if (!blocked && !loadGate.current.pending()) switchSetup(resumeSetup.id, observed); }} />
                </div>
                <button type="button" className="forge-welcome-resume-conversation" disabled={blocked}
                  aria-label={`Continue ${resumeSetup.name || "Untitled setup"}`}
                  onClick={() => { if (!blocked && !loadGate.current.pending()) switchSetup(resumeSetup.id); }}>Open conversation <Icon name="arrow" size={14} /></button>
              </section>}
              {!state.organization && <details className="forge-example-disclosure">
                <summary>See example ideas</summary>
                <SetupStartingPoints organization={state.organization} draft={draft} disabled={blocked} showHelp={false}
                  onReturnToTyping={() => inputRef.current?.focus()} onChoose={text => {
                  setDraft(text); setComposeMode("answer"); inputRef.current?.focus();
                }} />
              </details>}
              <button type="button" className="forge-welcome-mobile-editor" disabled={blocked || !onAdvanced}
                onClick={openDetails}>Advanced editor <Icon name="arrow" size={14} /></button>
            </>}
            <div className="setup-composer-foot">
              <span>Powered by a real MobKit team</span>
              <span>
                {available === false
                  ? "Provider setup needed"
                  : runtime?.model || state.capabilities?.model ||
                    config?.architect?.model ||
                    "Meerkat + MobKit"}
              </span>
            </div>
          </div>
        </section>
        <aside className="setup-plan-pane" aria-label="Your evolving system">
          <SetupPlan
            welcome={welcome}
            welcomeDraft={welcome ? draft : ""}
            guideTimeline={timeline}
            onConversation={() => {
              const ownsSelection = lifecycleSelection.current.capture(state);
              const conversation = conversationRef.current, scroll = chatScroll.current;
              if (!ownsSelection || !latestLifecycle.current?.flags.active) return;
              setMobilePanel("chat");
              requestAnimationFrame(() => {
                if (!setupsMounted.current || !latestLifecycle.current?.flags.active || !ownsSelection() ||
                    conversationRef.current !== conversation || !conversation?.isConnected ||
                    chatScroll.current !== scroll || !scroll?.isConnected) return;
                conversation.focus({ preventScroll: true });
                scroll.scrollTo?.({ top: 0, behavior: "smooth" });
              });
            }}
            rehearsalActivity={currentGuideActivity(rehearsalActivity, state)}
            applicationProgress={designConversationActive ? null : progress}
            plan={plan}
            state={state}
            pending={!!manualNote}
            busy={blocked}
            onResize={changeCount}
            onDetails={openDetails}
            onAsk={ask}
            onExpand={() => setMobilePanel("plan")}
            onConfigure={openConfig}
          />
        </aside>
      </div>
      {configOpen && (
        <div
          className="setup-modal-backdrop"
          onClick={() => closeConfig()}
        >
          <section
            className="setup-config-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="setup-config-title"
            onClick={(e) => e.stopPropagation()}
          >
            <header>
              <div>
                <span className="setup-eyebrow">OPTIONAL MANUAL OVERRIDE</span>
                <h2 id="setup-config-title">Your configuration</h2>
              </div>
              <button
                aria-label="Close configuration"
                onClick={() => closeConfig()}
              >
                <Icon name="close" />
              </button>
            </header>
            <p>
              Make a precise change. Your guide will receive the full
              configuration and explain how it affects the plan.
            </p>
            <label htmlFor="setup-config-source">
              Deployment blueprint (JSON)
            </label>
            <textarea
              id="setup-config-source"
              spellCheck="false"
              value={configDraft}
              onChange={(e) => editConfig(e.target.value)}
            />
            {(configError || !ownsConfigEditor(configEditor.current)) && (
              <div className="setup-error" role="alert">
                {configError || "The setup changed. Close and reopen configuration to review the current plan."}
              </div>
            )}
            <footer>
              <button onClick={() => closeConfig()}>Cancel</button>
              <button
                className="setup-action primary"
                disabled={busy || packageBusy || configChecking || !ownsConfigEditor(configEditor.current)}
                onClick={reviewConfig}
              >
                Send changes to guide <Icon name="arrow" size={15} />
              </button>
            </footer>
          </section>
        </div>
      )}
      {filesOpen && validation && (
        <div
          className="setup-modal-backdrop"
          onClick={() => setFilesOpen(false)}
        >
          <section
            className="setup-config-modal setup-file-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="setup-files-title"
            onClick={(e) => e.stopPropagation()}
          >
            <header>
              <div>
                <span className="setup-eyebrow">GENERATED FROM YOUR PLAN</span>
                <h2 id="setup-files-title">Your package files</h2>
              </div>
              <button
                aria-label="Close package files"
                onClick={() => setFilesOpen(false)}
              >
                <Icon name="close" />
              </button>
            </header>
            <div className="setup-files-layout">
              <nav>
                {validation.files.map((f) => (
                  <button
                    className={file?.path === f.path ? "active" : ""}
                    key={f.path}
                    onClick={() => setSelectedFile(f.path)}
                  >
                    {f.path}
                  </button>
                ))}
              </nav>
              <pre>{file?.content}</pre>
            </div>
          </section>
        </div>
      )}
    </div></ForgeIllustrationMotionProvider>
  );
}
