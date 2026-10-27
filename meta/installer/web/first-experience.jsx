import React, { createContext, useContext, useEffect, useId, useMemo, useRef, useState } from "react";
import { makeOrbitalIllustration } from "./forge-orbital-illustration.mjs";
import { canonicalJson } from "./state.mjs";

const OrbitalIllustration = makeOrbitalIllustration(React);
const IllustrationMotion = createContext(null);

// This notice describes saved proposal content, never animation generation or rendering.
export function useForgeProposalNotice({ guideId, proposal, displayedProposal, active, current, narrow, panel }) {
  const publication = useMemo(() => proposal ? canonicalJson(proposal) : null, [proposal]);
  const displayedPublication = useMemo(() => displayedProposal === proposal ? publication
    : displayedProposal ? canonicalJson(displayedProposal) : null, [displayedProposal, proposal, publication]);
  const observed = useRef(null);
  const [, redraw] = useState(0);
  useEffect(() => {
    if (!active || !current || !guideId) return;
    const previous = observed.current;
    // A selected saved setup starts with its own baseline, not another setup's unread state.
    const visible = (!narrow || panel === "plan") && displayedPublication === publication;
    const seen = previous?.guideId !== guideId || !publication || visible
      ? publication : previous.seen;
    if (previous?.guideId === guideId && previous.publication === publication && previous.seen === seen) return;
    observed.current = { guideId, publication, seen };
    redraw(value => value + 1);
  }, [active, current, guideId, publication, displayedPublication, narrow, panel]);
  return !!(active && current && narrow && publication &&
    observed.current?.guideId === guideId && observed.current.publication === publication &&
    observed.current.seen !== publication);
}

export function ForgeIllustrationMotionProvider({ children }) {
  const [paused, setPaused] = useState(false);
  return <IllustrationMotion.Provider value={{ paused, toggle: () => setPaused(value => !value) }}>{children}</IllustrationMotion.Provider>;
}

function useIllustrationMotion() {
  const shared = useContext(IllustrationMotion);
  const [paused, setPaused] = useState(false);
  return shared || { paused, toggle: () => setPaused(value => !value) };
}

function MotionButton({ paused, toggle }) {
  return <button type="button" className="forge-motion-toggle" aria-pressed={paused} onClick={toggle}>
    {paused ? "Resume motion" : "Pause motion"}
  </button>;
}

// Presentation only: an opening is never a substitute for saved guide context.
export function hasForgeOpeningContext(state, plan, { pending = false } = {}) {
  return !!state && !plan && !pending && state.status === "question" &&
    !!state.question && !state.question.options?.length &&
    !(state.messages || []).some(message => message.role === "user") &&
    !(state.answers || []).length && !(state.completed_stages || []).length &&
    !Object.values(state.brief?.facts || {}).some(fact => fact?.summary?.trim()) &&
    ![state.brief?.requirements, state.feature_gaps, state.warnings].some(items => items?.length);
}

export function isForgeOpening(state, plan, { pending = false, busy = false, loading = false } = {}) {
  return hasForgeOpeningContext(state, plan, { pending }) && !busy && !loading && !state.operation;
}

export function ForgeWelcomeIntro() {
  const previewId = useId(), preview = useRef(null), previewButton = useRef(null), heading = useRef(null);
  const motion = useIllustrationMotion();
  return <div className="setup-welcome-intro">
    <div className="forge-welcome-mobile-art">
      <button ref={previewButton} type="button" className="forge-welcome-preview-button"
        aria-label="Explore the Forge illustration" aria-haspopup="dialog" aria-controls={previewId}
        onClick={() => preview.current?.showModal()}>
        <span aria-hidden="true"><OrbitalIllustration mini still={motion.paused} /></span><span>View</span>
      </button>
      <div className="forge-welcome-mobile-caption"><ForgeWelcomeOrientation /><span>Start with what you need.</span></div>
    </div>
    <span className="forge-welcome-purpose"><i aria-hidden="true" />YOUR FORGE GUIDE</span>
    <h1 ref={heading} tabIndex={-1}>What would you like to get done?</h1>
    <p>Turn a task, a frustration, or an idea into your own team of AI agents. Your guide will work out what you need, one question at a time.</p>
    <dialog ref={preview} id={previewId} className="forge-welcome-dialog"
      aria-label="Explore the Forge illustration"
      onClose={() => {
        const intro = heading.current?.closest(".setup-conversation");
        const review = intro?.querySelector(".forge-starting-context__sentence");
        const question = intro?.querySelector(".forge-starting-conversation[data-open='true'] h3");
        const target = previewButton.current?.getClientRects().length ? previewButton.current
          : question?.getClientRects().length ? question : review?.getClientRects().length ? review : heading.current;
        target?.focus({ preventScroll: true });
      }}
      onClick={event => { if (event.target === event.currentTarget) preview.current?.close(); }}>
      <div className="forge-welcome-dialog-header"><span>YOUR IDEA, TAKING SHAPE</span>
        <button type="button" aria-label="Close illustration" onClick={() => preview.current?.close()}>Close <span aria-hidden="true">&#215;</span></button>
      </div>
      <ForgeWelcomePortrait />
    </dialog>
  </div>;
}

export function ForgeWelcomeNext({ needsConnection = false, choicesOnly = false }) {
  return <div id="setup-after-send" className="forge-welcome-next" aria-label={needsConnection ? "What happens next" : "What happens after you send"}>
    {needsConnection && <span>First, connect your guide</span>}
    <p>{needsConnection ? "Connect a model for your guide next. You may need an API key from a model provider. Your idea stays here. Nothing is sent yet."
      : choicesOnly ? "Your two answers stay here until you send."
      : "Nothing is sent until you choose Send."}</p>
  </div>;
}

export function ForgeWelcomeOrientation({ phase = "idea" }) {
  return <ol className="forge-welcome-orientation" aria-label="From idea to your own agent team">
    <li aria-current={phase === "idea" ? "step" : undefined}>Idea</li>
    <li aria-current={phase === "shape" ? "step" : undefined}>Shape</li>
    <li aria-current={phase === "build" ? "step" : undefined}>Build &amp; run</li>
  </ol>;
}

// An ambient illustration, deliberately independent of proposal identities.
export function ForgeWelcomePortrait({ draft = "", caption = "Preview illustration. Your proposal appears as we talk." }) {
  const motion = useIllustrationMotion();
  const hasIdea = typeof draft === "string" && !!draft.trim();
  const energy = hasIdea ? 0.7 + Math.min(draft.trim().length / 240, 1) * 0.25 : 0.7;
  return <section className="forge-welcome-portrait" aria-label="Illustration of a system taking shape">
    <span className="forge-portrait-eyebrow">YOUR IDEA, TAKING SHAPE</span>
    <div className="forge-portrait-art"><OrbitalIllustration energy={energy} still={motion.paused} /></div>
    <div className="forge-portrait-caption"><h2>Your team takes shape here.</h2>
      <p>Watch your agents and the work between them<br />take shape as we talk.</p>
      <small>{caption}</small>
      <MotionButton {...motion} />
    </div>
  </section>;
}
