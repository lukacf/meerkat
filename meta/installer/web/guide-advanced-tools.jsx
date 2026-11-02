import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";

export function GuideAdvancedTools({ guideId, active, disabled, savedToolsOpen, onSavedTools, children }) {
  const [capability, setCapability] = useState(null), [revision, setRevision] = useState(0);
  const summary = useRef(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!guideId || !active || !open) return;
    const controller = new AbortController();
    setCapability(null);
    request(`/api/guide/${encodeURIComponent(guideId)}/toolkit-saved`, { signal: controller.signal, timeoutMs: 30000 })
      .then(value => { if (!controller.signal.aborted) setCapability({ guideId, available: value?.available === true }); })
      .catch(() => { if (!controller.signal.aborted) setCapability({ guideId, failed: true }); });
    return () => controller.abort();
  }, [guideId, active, open, revision]);
  return <details className="setup-advanced-tools" onToggle={event => setOpen(event.currentTarget.open)} onKeyDown={event => {
    if (event.key === "Escape") {
      event.preventDefault(); event.stopPropagation(); event.currentTarget.open = false; summary.current?.focus();
    }
  }}>
    <summary ref={summary}>Advanced</summary>
    <div className="setup-advanced-tools-menu">
      {capability?.guideId === guideId && capability.available && <button type="button"
        disabled={disabled || !active} aria-expanded={savedToolsOpen} onClick={onSavedTools}>Import saved tool</button>}
      {capability?.guideId === guideId && capability.failed && <button type="button"
        disabled={!active} onClick={() => setRevision(value => value + 1)}>Check saved tool availability</button>}
      {children}
    </div>
  </details>;
}
