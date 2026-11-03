import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";
import { deploymentUrl } from "./deployment-panel.mjs";
import { captureGuideSelection } from "./guide-selection.mjs";
import { applicationSetupKey, applicationSetupView } from "./application-setup.mjs";

/** The existing deployment command owner handles effects and lost replies. */
export function ApplicationSetup({ receipt, state, selectionScope, confirmed = false,
  pending = false, disabled = false, onAction, onSettings, onCheck }) {
  const key = applicationSetupKey(receipt, state), token = selectionScope?.token();
  const [snapshot, setSnapshot] = useState(null), [retry, setRetry] = useState(0);
  const mounted = useRef(false);
  const latest = useRef(null);
  latest.current = { key, confirmed, pending, disabled, token };
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    const ownerCurrent = captureGuideSelection(selectionScope, state, token);
    if (!key || !confirmed || pending || disabled || !ownerCurrent) return;
    let cancelled = false;
    setSnapshot({ key, pending: true, value: null });
    request(`${deploymentUrl(state.id)}/application-setup?${new URLSearchParams({ deployment_id: receipt.deployment_id })}`)
      .then((value) => {
        if (!cancelled && ownerCurrent() && latest.current.key === key && latest.current.token === token &&
            latest.current.confirmed && !latest.current.disabled) setSnapshot({ key, pending: false, value });
      }).catch(() => {
        if (!cancelled && ownerCurrent() && latest.current.key === key && latest.current.token === token &&
            latest.current.confirmed && !latest.current.disabled) setSnapshot({ key, pending: false, value: null });
      });
    return () => { cancelled = true; };
  }, [key, confirmed, pending, disabled, token, retry]);
  const view = applicationSetupView({ receipt, state, confirmed, pending, snapshot });
  latest.current.view = view;
  if (!view) return null;
  const current = () => mounted.current && latest.current.key === key && latest.current.confirmed && !latest.current.pending && !latest.current.disabled &&
    latest.current.view === view && !!captureGuideSelection(selectionScope, state, token);
  function check() {
    if (!mounted.current || !captureGuideSelection(selectionScope, state, token) || latest.current.disabled) return;
    onCheck?.();
    setRetry((value) => value + 1);
  }
  return <ApplicationSetupCard view={view} disabled={disabled}
    onStart={() => { if (current() && ["ready", "reconnect"].includes(view.phase)) onAction?.(view.action || "finish-setup"); }}
    onBackground={() => { if (current() && view.phase === "ready" && view.kind === "restore" &&
      view.background?.available === true) onAction?.("start-background"); }}
    onSettings={onSettings ? () => { if (current()) onSettings(); } : null} onCheck={check} />;
}

export function ApplicationSetupCard({ view, disabled = false, onStart, onBackground, onSettings, onCheck }) {
  const ready = view.phase === "ready", missing = view.phase === "settings", waiting = view.phase === "unconfirmed";
  const checking = view.phase === "checking", restore = view.kind === "restore";
  const installed = restore || view.kind === "ordinary";
  const reconnect = view.phase === "reconnect";
  const prepare = ready && view.action === "prepare-setup";
  const hostUnavailable = view.phase === "unavailable" && view.reason === "host-unavailable";
  const previousAttempt = view.phase === "unavailable" && view.reason === "previous-attempt";
  const sourceUnavailable = view.phase === "unavailable" && view.reason === "source-unavailable";
  const recovery = previousAttempt || sourceUnavailable;
  const title = reconnect ? `Continue setting up ${view.name}` : ready ? `${prepare ? "Prepare" : installed ? "Start" : "Set up"} ${view.name}` : missing ? "One step before setup" : waiting ? "Checking application setup"
    : previousAttempt ? "Previous setup needs recovery" : sourceUnavailable ? "Saved setup needs recovery" : hostUnavailable ? "Setup is not available yet" : "Application setup is next";
  return <section className="deployment-setup-handoff" aria-label="Application setup" aria-busy={checking || waiting}>
    <p className="deployment-setup-eyebrow"><span aria-hidden="true" />
      {reconnect ? "Saved setup found" : ready ? prepare ? "Ready to prepare setup" : installed ? "Ready to start" : "Ready to set up" : missing ? "Private settings needed" : waiting ? "Result not confirmed" : checking ? "Checking setup" : recovery ? "Recovery needed" : "Setup pending"}</p>
    <h3>{title}</h3>
    <p>{reconnect ? "Forge will check this installation's saved setup and reconnect to it. If setup has not started, Forge will prepare it. The application stays stopped until you choose to start it."
      : ready ? prepare ? "Forge will prepare this installation's private setup inputs and keep the application stopped. You can then finish setup and start its agents."
      : restore ? "Forge will check this installation again, then start its agents with your saved settings." : "Forge will complete the application's setup, then start its agents."
      : missing ? "Save the required private settings, then come back here to start your application."
      : waiting ? "Forge is checking this installation's status. Setup will not be sent again automatically."
      : checking ? "Checking the installation and its saved settings before continuing."
      : hostUnavailable ? "Application setup is not enabled on this Forge server yet. Keep this installation and its saved settings. Connecting the guide or changing private settings will not enable it."
      : previousAttempt ? "Setup was already attempted. Forge needs the result of that attempt before it can start the application."
      : sourceUnavailable ? "Forge needs this installation's original saved setup inputs before it can continue. Keep its files and saved settings. Changing private settings cannot repair these inputs."
      : "Forge cannot finish setup for this installation yet. Your files and saved settings are preserved."}</p>
    {!waiting && <dl className="deployment-setup-checks">
      <div><dt>Installation</dt><dd>Checked</dd></div>
      <div><dt>Private settings</dt><dd>{ready ? "Saved" : missing ? "Needs attention" : checking ? "Checking" : "Not checked"}</dd></div>
      <div><dt>Application setup</dt><dd>{reconnect ? "Saved setup to check" : ready ? "Ready to continue" : recovery ? "Needs recovery" : hostUnavailable ? "Not available yet" : "Waiting"}</dd></div>
    </dl>}
    {missing && <p className="deployment-setup-missing">Needed: {view.missing.join(", ")}.</p>}
    <div className="deployment-setup-controls">
      {reconnect ? <button className="setup-action primary" disabled={disabled || !onStart} onClick={onStart}>Continue setup</button>
        : ready ? <button className="setup-action primary" disabled={disabled || !onStart} onClick={onStart}>{prepare ? "Prepare application setup" : installed ? "Start installed application" : "Set up and start application"}</button>
        : missing && onSettings ? <button className="setup-action primary" disabled={disabled} onClick={onSettings}>Review private settings</button>
        : <button className="setup-action" disabled={disabled || checking || !onCheck} onClick={onCheck}>{checking ? "Checking setup..." : "Check setup status"}</button>}
      {ready && restore && view.background?.available === true && <button className="setup-action"
        disabled={disabled || !onBackground} onClick={onBackground}>Run in background</button>}
      {(ready || view.phase === "unavailable" && !hostUnavailable && !recovery) && onSettings && <button className="deployment-setup-settings" disabled={disabled} onClick={onSettings}>Change private settings</button>}
      {onSettings && !waiting && !checking && !hostUnavailable && !recovery && <span className="deployment-setup-preserved">Saving settings keeps the application stopped.</span>}
    </div>
    {ready && restore && view.background?.available === true && <p className="deployment-setup-preserved">
      {view.background.persistence || "Background mode uses this computer's user service manager."}
    </p>}
    {ready && restore && view.background?.available === false && view.background.detail && <p className="deployment-setup-preserved">
      {view.background.detail}
    </p>}
    <p className="deployment-setup-preserved">{reconnect ? "Your files and saved settings stay in the folder you chose. Continuing setup does not start the application."
      : recovery ? "Your files and saved settings stay in the folder you chose. Check setup status only reads this installation."
      : waiting ? "Your files stay in the folder you chose." : "Nothing is running yet. Your files and saved settings stay in the folder you chose."}</p>
  </section>;
}
