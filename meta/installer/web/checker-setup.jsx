import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";
import "./checker-setup.css";

const STATUSES = new Set(["ready", "needs_preparation", "preparing", "unavailable", "failed"]);
const POLL_MS = 2000;

function checkedStatus(value) {
  if (!value || !STATUSES.has(value.status) || typeof value.can_prepare !== "boolean"
      || typeof value.detail !== "string") throw new Error("Invalid checker status");
  return { status: value.status, can_prepare: value.can_prepare, detail: value.detail };
}

export function CheckerSetup({ onReadiness, onClose }) {
  const [settings, setSettings] = useState(null);
  const [reading, setReading] = useState(false);
  const [posting, setPosting] = useState(false);
  const [unknown, setUnknown] = useState(false);
  const [error, setError] = useState("");
  const [closed, setClosed] = useState(false);
  const lifetime = useRef(0);
  const active = useRef(false);
  const busy = useRef(false);
  const preparation = useRef(null);
  const current = useRef(null);
  const unconfirmed = useRef(false);
  const notify = useRef(onReadiness);
  notify.current = onReadiness;

  function report(value) {
    // A parent UI callback cannot change whether the host response was received.
    try { notify.current?.(value); } catch {}
  }

  function apply(value) {
    current.current = value;
    setSettings(value);
    report(value);
  }

  async function refresh(explicit = false) {
    if (!active.current || busy.current) return;
    const generation = lifetime.current;
    busy.current = true;
    // A newer read owns status presentation without abandoning the original POST.
    if (preparation.current) preparation.current.observed = true;
    setReading(true);
    try {
      const next = checkedStatus(await request("/api/checker-setup"));
      if (!active.current || lifetime.current !== generation) return;
      apply(next);
      setError("");
      if (explicit) {
        const unresolved = !!preparation.current && !["preparing", "ready"].includes(next.status);
        unconfirmed.current = unresolved;
        setUnknown(unresolved);
      }
    } catch {
      if (active.current && lifetime.current === generation) {
        report(null);
        setError("App check status could not be read. Check status again; no preparation request was sent by this check.");
      }
    } finally {
      if (active.current && lifetime.current === generation) {
        busy.current = false;
        setReading(false);
      }
    }
  }

  useEffect(() => {
    const generation = ++lifetime.current;
    active.current = true;
    busy.current = false;
    preparation.current = null;
    report(null);
    refresh();
    return () => {
      if (lifetime.current === generation) {
        active.current = false;
        ++lifetime.current;
      }
    };
  }, []);

  useEffect(() => {
    if (closed || unknown || settings?.status !== "preparing") return;
    let stopped = false;
    let timer;
    async function poll() {
      if (stopped || !active.current) return;
      await refresh();
      if (!stopped && active.current) timer = setTimeout(poll, POLL_MS);
    }
    timer = setTimeout(poll, POLL_MS);
    return () => { stopped = true; clearTimeout(timer); };
  }, [closed, unknown, settings]);

  async function prepare() {
    const value = current.current;
    if (!active.current || busy.current || preparation.current || unconfirmed.current || !value?.can_prepare
        || !["needs_preparation", "failed"].includes(value.status)) return;
    const generation = lifetime.current;
    const pending = { observed: false };
    preparation.current = pending;
    unconfirmed.current = true;
    setUnknown(true);
    setPosting(true);
    setError("");
    try {
      const next = checkedStatus(await request("/api/checker-setup", { method: "POST" }));
      if (!active.current || lifetime.current !== generation || preparation.current !== pending) return;
      if (!pending.observed) {
        apply(next);
        unconfirmed.current = false;
        setUnknown(false);
      }
    } catch {
      if (active.current && lifetime.current === generation && preparation.current === pending) {
        if (!pending.observed || !["preparing", "ready"].includes(current.current?.status)) {
          unconfirmed.current = true;
          setUnknown(true);
        }
        if (!pending.observed)
          setError("Preparation was not confirmed. It may already be running. Check status before sending another request.");
      }
    } finally {
      if (active.current && lifetime.current === generation && preparation.current === pending) {
        preparation.current = null;
        setPosting(false);
      }
    }
  }

  function close() {
    active.current = false;
    ++lifetime.current;
    setClosed(true);
    onClose?.();
  }

  if (closed) return null;
  const canPrepare = settings?.can_prepare && ["needs_preparation", "failed"].includes(settings.status);
  return <section className="setup-checker" aria-label="Application checks setup">
    <div className="setup-checker-heading"><h3>App checks</h3>
      {onClose && <button type="button" onClick={close} aria-label="Close app checks setup">Close</button>}
    </div>
    {(posting && settings?.status !== "ready" || unknown || ["needs_preparation", "preparing", "failed"].includes(settings?.status)) &&
      <p>Preparation downloads a private Node runtime only into this installation. No administrator access or system Node replacement is needed.</p>}
    <div aria-live="polite" aria-atomic="true">
      {!settings && !error && <p>Reading app check status...</p>}
      {settings?.status === "needs_preparation" && <p>Prepare the local checker before building an application.</p>}
      {(posting && settings?.status !== "ready" || settings?.status === "preparing") && <p>{posting
        ? "The preparation response is still pending. Check its saved status without starting another preparation."
        : "Preparing app checks."} The download may continue if you close this card.</p>}
      {settings?.status === "ready" && <p>App checks ready.</p>}
      {settings?.status === "unavailable" && <p>This computer cannot build applications yet. You can continue designing and deploy a package built on a supported Mac.</p>}
      {settings?.status === "failed" && <p>App checks could not be prepared. Review the status before trying again.</p>}
      {settings?.detail && ["unavailable", "failed"].includes(settings.status) &&
        <p className="setup-checker-detail">{settings.detail}</p>}
    </div>
    {error && <p role="alert">{error}</p>}
    {(settings?.status !== "ready" || error) && <div className="setup-checker-actions">
      {canPrepare && <button type="button" disabled={reading || posting || unknown} onClick={prepare}>
        {posting ? "Preparing..." : "Prepare app checks"}</button>}
      <button type="button" disabled={reading} onClick={() => refresh(true)}>
        {reading ? "Checking status..." : posting ? "Check preparation status" : "Check status"}</button>
    </div>}
  </section>;
}
