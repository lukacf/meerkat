import React, { useCallback, useEffect, useRef, useState } from "react";
import { captureGuideSelection, guideSelectionEvents } from "./guide-selection.mjs";
import { request } from "./state.mjs";
import { createDeploymentPollingGate } from "./deployment-panel.mjs";
import { remoteBrowserQualified, remoteBrowserBinding, remoteBrowserOwnedBinding, remoteBrowserObservedTime } from "./remote-browser-access.mjs";
import { remoteApplicationExperiences, remoteApplicationEntryId, remoteApplicationUrl, remoteApplicationBinding,
  remoteApplicationOwnedBinding, remoteApplicationReplyOwned, remoteApplicationReplyCurrent, remoteApplicationLink, remoteApplicationCanOpen, remoteApplicationCanClose } from "./remote-application-access.mjs";
import { remoteInstalledUseReadKey, remoteInstalledUseBinding } from "./remote-installed-use.mjs";
import { remoteProductSelected } from "./remote-deployment.mjs";
import { installedTaskHandoffUrl } from "./installed-task-handoff.mjs";

export function RemoteApplicationAccess({ state, build, options, receipt, blocked = false,
  observationUnavailable = false, onRecheck, taskHandoff = null, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !captureGuideSelection(selectionScope, state, selectionToken);
  const qualified = remoteBrowserQualified(receipt, options, build, state, blocked, observationUnavailable);
  const observedExperiences = remoteApplicationExperiences(receipt), product = remoteProductSelected(receipt);
  const ownerKey = product ? remoteBrowserOwnedBinding(receipt, state) : "";
  const heldEntries = useRef({ key: "", value: null });
  if (heldEntries.current.key !== ownerKey) heldEntries.current = { key: ownerKey, value: null };
  if (product && ownerKey && observedExperiences.status === "resolved") heldEntries.current.value = observedExperiences;
  // A saved entry can identify only its admitted forward while observation is
  // unavailable. It cannot enable Connect, links or task handoff.
  const experiences = product && observedExperiences.status !== "resolved"
    ? heldEntries.current.value || observedExperiences : observedExperiences;
  const canOpen = qualified && observedExperiences.status === "resolved";
  const currentTaskHandoff = taskHandoff?.readKey === remoteInstalledUseReadKey(receipt, state, build) &&
    taskHandoff?.binding === remoteInstalledUseBinding(receipt, state) ? taskHandoff : null;
  const key = product ? ownerKey && experiences.status === "resolved" ? JSON.stringify([
    ownerKey, experiences.metadata_sha256, experiences.entries.map((entry) => [entry.id, entry.path])]) : ""
    : qualified && experiences.status === "resolved" ? JSON.stringify([
      remoteBrowserBinding(receipt, state, build), experiences.metadata_sha256,
      experiences.entries.map((entry) => [entry.id, entry.path, entry.destination_port])]) : "";
  const [, redraw] = useState(0), ports = useRef({ key: "", groups: new Map() }), latest = useRef(null);
  if (ports.current.key !== key) ports.current = { key, groups: new Map() };
  latest.current = { key, entries: experiences.entries };
  const getPortRevision = useCallback((port) => ports.current.groups.get(port)?.revision || 0, []);
  const onCloseStart = useCallback((port, selectedKey) => {
    if (selectedKey !== latest.current.key) return null;
    const previous = ports.current.groups.get(port);
    const group = { revision: (previous?.revision || 0) + 1, closing: true,
      refresh: (previous?.refresh || 0) + 1, confirmed: new Set() };
    ports.current.groups.set(port, group); redraw((revision) => revision + 1);
    return group.revision;
  }, []);
  const onObserved = useCallback((entry, value, portRevision, selectedKey) => {
    if (selectedKey !== latest.current.key) return;
    const observedPort = value.destination_port ?? entry.destination_port;
    const group = ports.current.groups.get(observedPort);
    if (!group?.closing || group.revision !== portRevision || value.cleanup_confirmed !== true ||
        !["closed", "blocked", "unavailable"].includes(value.status)) return;
    group.confirmed.add(entry.id);
    if (latest.current.entries.filter((row) => row.destination_port === observedPort)
      .every((row) => group.confirmed.has(row.id))) {
      group.closing = false; redraw((revision) => revision + 1);
    }
  }, []);
  const onSettled = useCallback((port, selectedKey) => {
    if (selectedKey !== latest.current.key) return;
    const group = ports.current.groups.get(port) || { revision: 0, closing: false, refresh: 0, confirmed: new Set() };
    group.refresh += 1; ports.current.groups.set(port, group); redraw((revision) => revision + 1);
  }, []);

  if ((!product && !qualified || product && !ownerKey) || !experiences.supported || experiences.status === "none") return null;
  return <section className="remote-application-access" aria-label="Custom application browser access"
    aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)}
    data-application-status={experiences.status}>
    <h4>Open my app</h4>
    {experiences.status !== "resolved" ? <>
      <p role="status">Forge could not check this installation's application links. Native Console access remains separate.</p>
      {onRecheck && <button className="setup-action" type="button" disabled={blocked || selectionBlocked} onClick={() => {
        if (captureGuideSelection(selectionScope, state, selectionToken)) onRecheck();
      }}>Recheck application links</button>}
    </> : <>
      {!canOpen && <p role="status">Saved app connections can be rechecked or closed. Opening the app requires current destination and listener checks.</p>}
      <div className="remote-application-entries">{experiences.entries.map((entry) => {
        const group = ports.current.groups.get(entry.destination_port);
        return <RemoteApplicationEntry key={remoteApplicationOwnedBinding(receipt, state, build, experiences, entry)}
          state={state} build={build} receipt={receipt} experiences={experiences} entry={entry} panelKey={key} selectionScope={selectionScope} qualified={canOpen}
          taskHandoff={currentTaskHandoff?.entryId === entry.id ? currentTaskHandoff : null}
          closing={group?.closing === true} portRevision={group?.revision || 0} refreshRevision={group?.refresh || 0}
          getPortRevision={getPortRevision} onCloseStart={onCloseStart} onObserved={onObserved} onSettled={onSettled} />;
      })}</div>
      <p>Open the app, sign in if it asks, then try one real task. A responding interface does not verify sign-in or a useful task result.</p>
      <p>Keep Forge open while using the app.</p>
      <details className="deployment-evidence"><summary>About application access</summary>
        <p>These entries come from this installation's accepted source. Forge checks the owned application, declared listener and browser connection. It has not observed a completed application task.</p>
        <p>Some pages share one connection. Closing it also closes access to those pages; the Linux application keeps running.</p>
      </details>
    </>}
  </section>;
}

function RemoteApplicationEntry({ state, build, receipt, experiences, entry, panelKey, closing, qualified,
  portRevision, refreshRevision, getPortRevision, onCloseStart, onObserved, onSettled, taskHandoff, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !captureGuideSelection(selectionScope, state, selectionToken);
  const key = remoteApplicationOwnedBinding(receipt, state, build, experiences, entry);
  const openingScope = qualified ? remoteApplicationBinding(receipt, state, build, experiences, entry) : "";
  const openingLifetime = useRef({ scope: "", revision: 0 });
  if (openingLifetime.current.scope !== openingScope)
    openingLifetime.current = { scope: openingScope, revision: openingLifetime.current.revision + 1 };
  const [snapshot, setSnapshot] = useState({ key: "", value: null, unavailable: false, loaded: false });
  const [busy, setBusy] = useState(""), [error, setError] = useState(null), [clock, setClock] = useState(Date.now);
  const gate = useRef(null), sequence = useRef(0), latest = useRef(null);
  if (!gate.current) gate.current = createDeploymentPollingGate();
  const selected = snapshot.key === key ? snapshot : { value: null, unavailable: false, loaded: false };
  latest.current = { key, state, build, receipt, experiences, entry, qualified,
    openingRevision: openingLifetime.current.revision, value: selected.value };

  function publish(value, selectedKey, selectedRevision, ownerCurrent) {
    const current = latest.current;
    const observedPort = current.value?.destination_port ?? current.entry.destination_port;
    if (!ownerCurrent?.() || selectedKey !== current.key || getPortRevision(observedPort) !== selectedRevision) return false;
    if (!remoteApplicationReplyOwned(value, current.receipt, current.state, current.build, current.experiences, current.entry)) {
      setSnapshot((previous) => ({ key: selectedKey, value: previous.key === selectedKey ? previous.value : null,
        unavailable: true, loaded: true }));
      return false;
    }
    if (current.value && remoteBrowserObservedTime(value.observed_at) < remoteBrowserObservedTime(current.value.observed_at)) return false;
    setSnapshot({ key: selectedKey, value, unavailable: false, loaded: true });
    if (["closed", "ready"].includes(value.status)) setError(null);
    onObserved(current.entry, value, selectedRevision, panelKey);
    return true;
  }

  async function refresh(expectedToken) {
    const current = latest.current, observedPort = current.value?.destination_port ?? current.entry.destination_port;
    const selectedRevision = getPortRevision(observedPort);
    const ownerCurrent = captureGuideSelection(selectionScope, current.state, expectedToken);
    if (!ownerCurrent) return;
    const selectedSequence = ++sequence.current;
    const observed = await gate.current.observe(current.key, () => request(remoteApplicationUrl(
      current.state.id, current.receipt.deployment_id, current.entry)));
    if (!ownerCurrent() || selectedSequence !== sequence.current || !observed ||
        !gate.current.current(observed.token, latest.current.key) ||
        selectedRevision !== getPortRevision(observedPort)) return;
    if (observed.failure) setSnapshot((previous) => ({ key: current.key,
      value: previous.key === current.key ? previous.value : null, unavailable: true, loaded: true }));
    else publish(observed.receipt, current.key, selectedRevision, ownerCurrent);
  }

  useEffect(() => {
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
  useEffect(() => { if (refreshRevision > 0) refresh(); }, [refreshRevision]);

  async function submit(action) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || busy || !selected.loaded || action === "open" &&
        (!latest.current.qualified || closing || selected.unavailable ||
          !remoteApplicationReplyCurrent(selected.value, receipt, state, build, experiences, entry) || !remoteApplicationCanOpen(selected.value, entry)) ||
        action === "close" && !remoteApplicationCanClose(selected.value)) return;
    const ownedPort = selected.value?.destination_port ?? entry.destination_port;
    const captured = { key, token: gate.current.capture(key), accessId: selected.value?.access_id, port: ownedPort,
      openingRevision: latest.current.openingRevision,
      revision: action === "close" ? onCloseStart(ownedPort, panelKey) : getPortRevision(ownedPort) };
    if (captured.revision === null) return;
    ++sequence.current; setBusy(action); setError(null);
    try {
      const value = await request(remoteApplicationUrl(state.id, receipt.deployment_id, entry, action), {
        method: "POST", body: action === "open"
          ? { version: state.version, build_id: build.build_id, deployment_id: receipt.deployment_id, entry_id: remoteApplicationEntryId(entry) }
          : { deployment_id: receipt.deployment_id, access_id: captured.accessId } });
      if (!ownerCurrent() || captured.key !== latest.current.key || captured.revision !== getPortRevision(captured.port) ||
          action === "open" && (!latest.current.qualified || captured.openingRevision !== latest.current.openingRevision ||
            !remoteApplicationReplyCurrent(value, latest.current.receipt, latest.current.state, latest.current.build,
              latest.current.experiences, latest.current.entry)) ||
          action === "close" && value.access_id !== null && value.access_id !== captured.accessId ||
          latest.current.value?.access_id !== captured.accessId && latest.current.value?.access_id !== value.access_id ||
          !gate.current.admit(captured.token, latest.current.key)) return;
      publish(value, captured.key, captured.revision, ownerCurrent);
    } catch {
      if (ownerCurrent() && gate.current.current(captured.token, latest.current.key) &&
          captured.revision === getPortRevision(captured.port)) {
        setError(action === "open" ? "Forge could not confirm the connection response. Recheck this app connection; it will not reconnect automatically."
          : "Closing this app connection has not been confirmed. Recheck or retry closing it.");
        await refresh();
      }
    } finally {
      if (gate.current.sameScope(captured.token, latest.current.key)) {
        setBusy(""); if (ownerCurrent()) onSettled(captured.port, panelKey);
      }
    }
  }

  const access = selected.value;
  const url = !selectionBlocked && qualified && !selected.unavailable && !busy && !closing ? remoteApplicationLink(
    access, receipt, state, build, experiences, entry, Math.max(clock, Date.now())) : null;
  const expired = !selectionBlocked && access?.status === "ready" && !url && !closing && !busy && !selected.unavailable;
  const taskUrl = url && taskHandoff ? installedTaskHandoffUrl(url, taskHandoff.task) : null;
  return <div className="remote-application-entry" data-entry-id={entry.id}
    data-entry-status={selected.unavailable ? "unavailable" : access?.status || "checking"}>
    <strong>{entry.label}</strong>
    <div className="deployment-actions">
      {url && <a className="setup-action primary" href={url} target="_blank" rel="noopener noreferrer">Open {entry.label}</a>}
      {taskUrl && <a className="setup-action primary" href={taskUrl} target="_blank" rel="noopener noreferrer"
        referrerPolicy="no-referrer">Continue with this task</a>}
      {qualified && !url && !closing && !selected.unavailable && remoteApplicationReplyCurrent(access, receipt, state, build, experiences, entry) && remoteApplicationCanOpen(access, entry) &&
        <button className="setup-action primary" type="button" disabled={selectionBlocked || !!busy}
          onClick={() => submit("open")}>{busy === "open" ? "Connecting browser..." : `Connect to ${entry.label}`}</button>}
      <button className="setup-action" type="button" disabled={selectionBlocked || !!busy}
        onClick={() => refresh(selectionToken)}>{selected.loaded ? "Recheck connection" : "Checking connection..."}</button>
      {remoteApplicationCanClose(access) && <button className="setup-action" type="button" disabled={selectionBlocked || !!busy}
        onClick={() => submit("close")}>{busy === "close" ? "Closing connection..." : "Close connection"}</button>}
    </div>
    {taskUrl && <p className="deployment-use-help">Your task opens as an editable draft. Sign in if asked; use the app's send and confirmation controls. Your draft stays in Forge.</p>}
    {entry.listener_owned !== true && <p className="deployment-note" role="status">{entry.reason || "This application's declared listener could not be verified. Recheck remote status before connecting."}</p>}
    {closing && <p className="deployment-note" role="status">Links sharing this connection stay hidden until closing is confirmed.</p>}
    {!closing && access?.status === "connecting" && <p role="status">Connecting your browser to this app...</p>}
    {(selected.unavailable || access?.status === "unavailable") && <p className="deployment-warning" role="status">This browser connection could not be verified. Its link is hidden; the Linux application may still be running.</p>}
    {access?.status === "blocked" && <p className="deployment-warning" role="status">This app's browser port may already be in use on this computer. Close the other app or connection using it, then try again.</p>}
    {access?.status === "cleanup-needed" && <p className="deployment-warning" role="status">Connection cleanup is unconfirmed. Retry closing it before connecting again.</p>}
    {expired && <p className="deployment-note" role="status">Recheck this connection before opening the app. Its readiness needs another check.</p>}
    {error && <p className="deployment-error" role="alert">{error}</p>}
  </div>;
}
