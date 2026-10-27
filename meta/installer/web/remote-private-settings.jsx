import React, { useEffect, useRef, useState } from "react";
import { captureGuideSelection, guideSelectionAvailable } from "./guide-selection.mjs";
import { request } from "./state.mjs";
import { remoteDeploymentUrl, remoteReceiptBinding, remoteSelectionKey, remoteObservationAvailable, remoteInstallAdmitted, remoteProductSelected, remoteControlBindingKey, remoteProductActionSettled } from "./remote-deployment.mjs";
import { remoteInstalledCanConfigure, remoteInstalledSettingsOptions, remoteInstalledSettingsBody, remoteInstalledSettingsSaved, remoteInstalledSettingsPacket } from "./remote-private-settings.mjs";

export function RemotePrivateSettings({ state, build, options, receipt, blocked, observationUnavailable,
  selectionScope, renderConfiguration, onBusy, onReceipt, onRecheck, onEditing, openRequest }) {
  const selectionToken = selectionScope?.token();
  const selected = remoteSelectionKey(state, build);
  const binding = remoteReceiptBinding(receipt);
  const controlBinding = remoteControlBindingKey(receipt?.product_control?.binding);
  const available = guideSelectionAvailable(selectionScope, state, selectionToken);
  const canConfigure = !observationUnavailable && available && remoteInstalledCanConfigure(receipt, options, build, state, blocked);
  const [open, setOpen] = useState(false), [metadata, setMetadata] = useState(null);
  const [values, setValues] = useState({}), [removed, setRemoved] = useState([]);
  const [phase, setPhase] = useState(""), [notice, setNotice] = useState("");
  const [needsReload, setNeedsReload] = useState(false), [unconfirmed, setUnconfirmed] = useState(false);
  const requestedOpen = useRef(null), pendingSave = useRef(null);
  const lifetime = useRef({ revision: 0, mounted: true }), operation = useRef(null), latest = useRef(null), heading = useRef(null);
  latest.current = { state, build, options, receipt, binding, controlBinding, selected, selectionToken, canConfigure, onBusy, onReceipt, onEditing };

  function release(job) {
    if (operation.current !== job) return;
    operation.current = null;
    latest.current.onBusy?.("", job);
    if (lifetime.current.mounted) setPhase("");
  }
  useEffect(() => {
    lifetime.current.revision += 1;
    if (operation.current) release(operation.current);
    setOpen(false); setMetadata(null); setValues({}); setRemoved([]);
    setNotice(""); setNeedsReload(false); setUnconfirmed(false); pendingSave.current = null;
  }, [binding, controlBinding]);
  useEffect(() => {
    lifetime.current.revision += 1;
    if (operation.current) release(operation.current);
    setNeedsReload(true);
  }, [selected, selectionToken]);
  useEffect(() => {
    lifetime.current.mounted = true;
    return () => { lifetime.current.mounted = false; lifetime.current.revision += 1;
      if (operation.current) release(operation.current); };
  }, []);
  useEffect(() => { if (open || notice) heading.current?.focus({ preventScroll: true }); }, [open, notice]);
  useEffect(() => {
    const owner = { guideId: state.id, binding, editor: {} };
    latest.current.onEditing?.(open && receipt?.private_settings_supported !== false, owner);
    return () => latest.current.onEditing?.(false, owner);
  }, [open, receipt?.private_settings_supported, state.id, binding]);

  function begin(kind) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !canConfigure || operation.current) return null;
    const job = { guideId: state.id, revision: ++lifetime.current.revision, selected, binding, controlBinding, ownerCurrent };
    if (onBusy?.(kind, job) === false) return null;
    operation.current = job; setPhase(kind);
    return job;
  }
  function current(job) {
    return lifetime.current.mounted && job.ownerCurrent() && operation.current === job &&
      lifetime.current.revision === job.revision && latest.current.selected === job.selected && latest.current.binding === job.binding && latest.current.controlBinding === job.controlBinding;
  }
  async function load() {
    const job = begin("read");
    if (!job) return;
    setOpen(true); setNotice("");
    try {
      const query = new URLSearchParams({ deployment_id: receipt.deployment_id });
      const value = await request(`${remoteDeploymentUrl(state.id, "settings")}?${query}`);
      if (!current(job)) return;
      const now = latest.current;
      const checked = remoteInstalledSettingsOptions(value, now.receipt, now.options, now.build, now.state, !now.canConfigure);
      if (!checked) throw new Error("Settings need a fresh installation check.");
      setMetadata(checked); setNeedsReload(false);
    } catch {
      if (current(job)) { setNeedsReload(true); setNotice("Private settings could not be checked. Your unsent changes are kept here. Recheck remote status, then try loading them again."); }
    } finally { release(job); }
  }
  useEffect(() => {
    if (!openRequest || requestedOpen.current === openRequest || !canConfigure || phase) return;
    requestedOpen.current = openRequest;
    load();
  }, [openRequest, canConfigure, phase]);
  useEffect(() => {
    const identity = pendingSave.current;
    if (!identity || !unconfirmed) return;
    const settled = remoteProductActionSettled(receipt, state, identity, "configure");
    if (!settled) return;
    pendingSave.current = null; setUnconfirmed(false); setNeedsReload(true);
    if (settled.outcome === "returned") {
      setValues({}); setRemoved([]); setMetadata(null); setOpen(false);
      setNotice("The original private settings save is confirmed. Your application is still stopped.");
    } else setNotice("The original save ended without confirmation. Your unsent changes are kept here. Reload the checked settings before another save.");
  }, [receipt, state.id, unconfirmed]);
  const body = (!remoteProductSelected(receipt) || !unconfirmed) && !needsReload && remoteInstalledSettingsBody(metadata, receipt, options, build, state, values, removed, !canConfigure);
  async function save(event) {
    event.preventDefault();
    if (!body) return;
    const job = begin("save");
    if (!job) return;
    const identity = remoteProductSelected(receipt) ? crypto.randomUUID() : undefined;
    const packet = remoteInstalledSettingsPacket(body, receipt, state, identity);
    if (!packet) { release(job); return; }
    pendingSave.current = identity || null;
    setNotice("");
    try {
      const next = await request(remoteDeploymentUrl(state.id, "settings"), { method: "POST", body: packet });
      if (!current(job)) return;
      const now = latest.current;
      if (remoteInstalledSettingsSaved(next, now.receipt, now.options, now.build, now.state, identity)) {
        now.onReceipt?.(next, state.id); setValues({}); setRemoved([]); setMetadata(null); setOpen(false);
        setUnconfirmed(false); setNeedsReload(false); pendingSave.current = null;
        setNotice("Private settings saved. Your application is still stopped. Start it again when you are ready.");
      } else {
        if (remoteReceiptBinding(next) === job.binding) now.onReceipt?.(next, state.id);
        setUnconfirmed(true); setNeedsReload(true);
      }
    } catch (error) {
      if (current(job)) {
        const notSent = identity && error.status === 409 && error.code === "remote_product_action_not_sent" &&
          error.action === "configure" && error.action_request_id === identity;
        if (notSent) {
          pendingSave.current = null; setUnconfirmed(false);
          setNotice("This save was not sent. Your changes are kept here. Reload the checked settings before saving again.");
        } else setUnconfirmed(true);
        setNeedsReload(true);
      }
    } finally { release(job); }
  }
  if (!receipt) return null;
  if (receipt.private_settings_supported === false && remoteInstallAdmitted(receipt) && remoteObservationAvailable(receipt) && !observationUnavailable)
    return <details className="deployment-evidence remote-settings-legacy"><summary>Changing private settings on this installation</summary>
      <p>This installation does not support settings changes in Forge. Stop and restart keep its existing settings. Recover a password with help from the computer's administrator while preserving this project folder and its saved records.</p>
    </details>;
  if (!open && !notice && !unconfirmed && !canConfigure) return null;
  return <section className="remote-settings" aria-label="Installed remote private settings">
    {notice && <p className="deployment-note" role="status" ref={heading} tabIndex={-1}>{notice}</p>}
    {unconfirmed && <p className="deployment-warning" role="status">Forge could not confirm this save. The original request may still finish. Your changes are kept here; {remoteProductSelected(receipt)
      ? "Forge checks the original request without sending the save again."
      : "checking status does not confirm that these exact values were saved."}</p>}
    {!open && canConfigure && <button className="setup-action" type="button" onClick={load} disabled={!!phase}>Change private settings</button>}
    {open && <form className="deployment-form remote-private-form" onSubmit={save} autoComplete="off">
      <h4 ref={heading} tabIndex={-1}>Change private settings</h4>
      <p>Leave a field blank to keep its saved value. Saving keeps this application stopped and preserves its saved records.</p>
      {metadata && renderConfiguration({ options: metadata, values, removed, useHost: false,
        guideId: `remote-installed-${state.id}`, busy: !!phase || !canConfigure || needsReload,
        onChange: (key, value) => setValues((previous) => ({ ...previous, [key]: value })),
        onRemove: (key, checked) => setRemoved((previous) => checked ? [...new Set([...previous, key])] : previous.filter((item) => item !== key)) })}
      {phase === "read" && <p role="status">Checking saved settings without showing private values...</p>}
      {phase === "save" && <p role="status">Waiting for the save response. If the wait continues, open <strong>Your setups</strong> and select this setup again. The original request may still finish; reopening does not send it again.</p>}
      {!canConfigure && !phase && <p className="deployment-note" role="status">Settings changes need the same accepted application and a confirmed stopped service. Your unsent changes are kept here.</p>}
      <div className="deployment-actions">
        {!needsReload && metadata && <button className="setup-action primary" type="submit" disabled={!!phase || !body}>{phase === "save" ? "Saving private settings..." : "Save private settings"}</button>}
        {(needsReload || !metadata) && <button className="setup-action primary" type="button" onClick={load} disabled={!!phase || !canConfigure}>{phase === "read" ? "Loading private settings..." : "Reload private settings"}</button>}
        <button className="setup-action" type="button" onClick={() => onRecheck?.(selectionToken)} disabled={!!phase}>Recheck remote status</button>
        <button className="setup-action" type="button" disabled={!!phase || !available} onClick={() => { setOpen(false); setValues({}); setRemoved([]); setMetadata(null); }}>Cancel</button>
      </div>
    </form>}
  </section>;
}
