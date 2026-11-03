import React, { useEffect, useRef, useState } from "react";
import { captureGuideSelection, guideSelectionEvents } from "./guide-selection.mjs";
import { request } from "./state.mjs";
import { createDeploymentPollingGate } from "./deployment-panel.mjs";
import { remoteCloudRemoval } from "./remote-deployment.mjs";
import { remoteBrowserUrl, remoteBrowserQualified, remoteBrowserBinding, remoteBrowserReplyCurrent,
  remoteBrowserOwnedBinding, remoteBrowserReplyOwned,
  remoteBrowserConsole, remoteBrowserCanOpen, remoteBrowserCanClose, remoteBrowserStatus,
  remoteBrowserObservedTime } from "./remote-browser-access.mjs";

export function RemoteBrowserAccess({ state, build, options, receipt, blocked = false, observationUnavailable = false, removalPending = false, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !captureGuideSelection(selectionScope, state, selectionToken);
  const removing = removalPending || !!remoteCloudRemoval(receipt);
  blocked = blocked || removing;
  const qualified = remoteBrowserQualified(receipt, options, build, state, blocked, observationUnavailable);
  const key = remoteBrowserOwnedBinding(receipt, state);
  const openingScope = qualified ? remoteBrowserBinding(receipt, state, build) : "";
  const [snapshot, setSnapshot] = useState({ key: "", value: null, unavailable: false, loaded: false });
  const [busy, setBusy] = useState(""), [error, setError] = useState(null), [closeIntent, setCloseIntent] = useState(false);
  const [clock, setClock] = useState(Date.now);
  const latest = useRef(null), gate = useRef(null), sequence = useRef(0);
  const openingLifetime = useRef({ scope: "", revision: 0 });
  if (openingLifetime.current.scope !== openingScope)
    openingLifetime.current = { scope: openingScope, revision: openingLifetime.current.revision + 1 };
  if (!gate.current) gate.current = createDeploymentPollingGate();
  const selected = snapshot.key === key ? snapshot : { value: null, unavailable: false, loaded: false };
  latest.current = { key, qualified, openingRevision: openingLifetime.current.revision, state, build, receipt, value: selected.value };

  function publish(value, selectedKey, ownerCurrent) {
    if (!ownerCurrent?.() || selectedKey !== latest.current.key || !selectedKey) return false;
    if (!remoteBrowserReplyOwned(value, latest.current.receipt, latest.current.state)) {
      setSnapshot((previous) => ({ key: selectedKey, value: previous.key === selectedKey ? previous.value : null,
        unavailable: true, loaded: true }));
      return false;
    }
    const previous = latest.current.value;
    if (previous && remoteBrowserObservedTime(value.observed_at) < remoteBrowserObservedTime(previous.observed_at)) return false;
    setSnapshot({ key: selectedKey, value, unavailable: false, loaded: true });
    if (["ready", "closed"].includes(value.status)) setError(null);
    if (value.status === "closed") setCloseIntent(false);
    return true;
  }

  async function refresh(expectedToken) {
    const ownerCurrent = captureGuideSelection(selectionScope, latest.current.state, expectedToken);
    const selectedKey = latest.current.key;
    if (!ownerCurrent || !selectedKey) return;
    const identity = latest.current.state.id, deploymentId = latest.current.receipt.deployment_id;
    const currentSequence = ++sequence.current;
    const observed = await gate.current.observe(selectedKey, () => request(remoteBrowserUrl(identity, deploymentId)));
    if (!ownerCurrent() || currentSequence !== sequence.current || selectedKey !== latest.current.key || !observed ||
        !gate.current.current(observed.token, latest.current.key)) return;
    if (observed.failure) setSnapshot((previous) => ({ key: selectedKey,
      value: previous.key === selectedKey ? previous.value : null, unavailable: true, loaded: true }));
    else publish(observed.receipt, selectedKey, ownerCurrent);
  }

  useEffect(() => {
    setSnapshot({ key, value: null, unavailable: false, loaded: false }); setBusy(""); setError(null); setCloseIntent(false);
    if (!key) return;
    const scope = gate.current.open(key);
    let stopped = false, timer;
    const clockTimer = setInterval(() => setClock(Date.now()), 1000);
    async function poll() {
      if (stopped) return;
      await refresh();
      if (!stopped) timer = setTimeout(poll, 2000);
    }
    poll();
    return () => { stopped = true; clearTimeout(timer); clearInterval(clockTimer); gate.current.close(scope); };
  }, [key]);

  async function submit(action) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !key || busy || !selected.loaded ||
        action === "open" && (!latest.current.qualified || selected.unavailable || !remoteBrowserReplyCurrent(selected.value, receipt, state, build) || !remoteBrowserCanOpen(selected.value)) ||
        action === "close" && !remoteBrowserCanClose(selected.value)) return;
    const captured = { key, state, deploymentId: receipt.deployment_id, accessId: selected.value?.access_id,
      openingRevision: latest.current.openingRevision, token: gate.current.capture(key) };
    ++sequence.current;
    setBusy(action); setError(null);
    if (action === "close") setCloseIntent(true);
    try {
      const value = await request(remoteBrowserUrl(state.id, receipt.deployment_id, action), { method: "POST", body: action === "open"
        ? { version: state.version, build_id: build.build_id, deployment_id: receipt.deployment_id, entry_id: "native-console" }
        : { deployment_id: receipt.deployment_id, access_id: captured.accessId } });
      if (!ownerCurrent() || captured.key !== latest.current.key ||
          action === "open" && (captured.openingRevision !== latest.current.openingRevision || !latest.current.qualified ||
            !remoteBrowserReplyCurrent(value, latest.current.receipt, latest.current.state, latest.current.build)) ||
          action === "close" && value.access_id !== null && value.access_id !== captured.accessId ||
          latest.current.value?.access_id !== captured.accessId && latest.current.value?.access_id !== value.access_id ||
          !gate.current.admit(captured.token, latest.current.key)) return;
      publish(value, captured.key, ownerCurrent);
    } catch {
      if (ownerCurrent() && captured.key === latest.current.key && gate.current.current(captured.token, latest.current.key)) {
        setError(action === "open" ? "Forge could not confirm the connection response. Rechecking the owned browser connection; no automatic reconnect will run."
          : "Closing the browser connection has not been confirmed. Recheck its status or retry closing this owned connection.");
        await refresh();
      }
    } finally { if (gate.current.sameScope(captured.token, latest.current.key)) setBusy(""); }
  }

  if (!key) return null;
  const access = selected.value;
  const url = !selectionBlocked && qualified && !selected.unavailable && !busy && !closeIntent
    ? remoteBrowserConsole(access, receipt, state, build, Math.max(clock, Date.now())) : null;
  const expired = !selectionBlocked && qualified && access?.status === "ready" && !url && !busy && !closeIntent && !selected.unavailable;
  const authenticated = access?.scope?.authentication === "local-password";
  return <section className="remote-browser-access" aria-label="Native Console browser connection"
    aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)}
    data-browser-status={selected.unavailable ? "unavailable" : access?.status || "checking"}>
    <h4>{!qualified && !selected.unavailable && !closeIntent ? "Saved browser connection"
      : expired ? "Recheck your browser connection" : remoteBrowserStatus(access, selected.unavailable, closeIntent)}</h4>
    <div className="deployment-actions">
      {url && <a className="setup-action primary" href={url} target="_blank" rel="noopener noreferrer">Open native Console</a>}
      {qualified && !url && !closeIntent && !selected.unavailable && remoteBrowserReplyCurrent(access, receipt, state, build) && remoteBrowserCanOpen(access) && <button className="setup-action primary" type="button"
        disabled={selectionBlocked || !!busy || !selected.loaded} onClick={() => submit("open")}>{busy === "open" ? "Opening browser connection..." : "Connect browser to native Console"}</button>}
      <button className="setup-action" type="button" disabled={selectionBlocked || !!busy} onClick={() => refresh(selectionToken)}>{selected.loaded ? "Recheck browser connection" : "Checking browser connection..."}</button>
      {remoteBrowserCanClose(access) && <button className="setup-action" type="button" disabled={selectionBlocked || !!busy}
        onClick={() => submit("close")}>{busy === "close" ? "Closing browser connection..." : "Close browser connection"}</button>}
    </div>
    {removing ? <p className="deployment-note">Opening Console is withdrawn for this cloud removal request. Recheck or close an existing browser connection here until its local cleanup is confirmed.</p>
      : !qualified && <p className="deployment-note">Recheck or close this installed application's saved connection here. Opening its Console requires the matching accepted application and current destination checks.</p>}
    {access && qualified && <p>{authenticated ? "Sign in with the Console password you chose for this installation. Forge does not copy or fill it in."
      : "This installation's native Console does not ask for a Console password."}</p>}
    {!removing && <p>Keep Forge open while using the Console.</p>}
    {(selected.unavailable || access?.status === "unavailable") && <p className="deployment-warning" role="status">Browser access could not be verified. The open link is withheld; this does not prove the remote application failed or stopped. Recheck before reconnecting.</p>}
    {access?.status === "blocked" && <p className="deployment-warning" role="status">Forge could not open this browser connection. Its Console port may already be in use on this computer. Close the other app or connection using that port, then try again.</p>}
    {access?.status === "cleanup-needed" && <p className="deployment-warning" role="status">The owned browser connection has not confirmed cleanup. Its link remains unavailable. Retry closing this connection; another one will not be opened while cleanup is uncertain.</p>}
    {expired && <p className="deployment-warning" role="status">Recheck this browser connection before opening the Console. Its last readiness observation is no longer fresh.</p>}
    {closeIntent && <p className="deployment-note" role="status">The open link stays hidden until closing this owned browser connection is confirmed. The remote application's status is separate.</p>}
    <details className="deployment-evidence"><summary>About this Console connection</summary>
      <p>This opens the native MobKit chat and admin Console on this computer through the owned SSH connection. Forge will not replace an existing listener or expose the Console publicly.</p>
      <p>{removing ? 'Closing cleans up the connection on this computer. Cloud resource removal status is shown above.' : 'Closing this browser connection does not stop the application on your Linux computer. SSH access and native readiness are checked separately.'}</p>
      <p>Custom application UI, successful sign-in and useful application results still need separate verification.</p>
    </details>
    {error && <p className="deployment-error" role="alert">{error}</p>}
  </section>;
}
