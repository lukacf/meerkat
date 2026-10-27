import { captureGuideSelection, guideSelectionAvailable, guideSelectionEvents } from './guide-selection.mjs';
import React, { useEffect, useRef, useState } from 'react';
import { request } from './state.mjs';
import { remoteReceiptOwned } from './remote-deployment.mjs';
import { artifactVersionCurrent } from './artifact-current.mjs';
import { gcpPreflightScope } from './gcp-preflight.mjs';
import { GcpPreflight } from './gcp-preflight.jsx';
import { CloudResourceGuidance } from './gcp-resource-guidance.jsx';
import { gcpDeploymentUrl, gcpCloudOwned, gcpCloudCurrent, gcpCanCreate, gcpMakeIntent,
  gcpLoadIntent, gcpStoreIntent, gcpIntentMatchesPlan, gcpCloudMatchesIntent } from './gcp-guided.mjs';
import { gcpRemovalLifecycle, gcpRemovalResourceRows, gcpLoadRemovalIntent, gcpStoreRemovalIntent, gcpCloudSameGeneration,
  gcpMakeRemovalIntent } from './gcp-guided.mjs';

const PREPARATION_STEPS = new Map([
  ['checking', 'Checking your cloud destination'],
  ['creating-firewall', 'Creating and checking the SSH access rule'],
  ['creating-vm', 'Creating and checking your cloud computer'],
  ['vm-ready', 'Cloud computer preparation verified'],
]);

export function GcpGuided({ state, build, blocked = false, allowPreparation = true, onChooseProject, onDownload, downloading = false, onCloudRemoval, selectionScope }) {
  blocked = blocked || !allowPreparation;
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const [view, setView] = useState({ guideId: '', cloud: null, intent: null, removalIntent: null, loaded: false, unavailable: true, storageError: '' });
  const [busy, setBusy] = useState(''), [error, setError] = useState('');
  const [observationError, setObservationError] = useState('');
  const [connection, setConnection] = useState(null);
  const [removeOpen, setRemoveOpen] = useState(false), [dataLoss, setDataLoss] = useState(false);
  const latest = useRef(null), lifetime = useRef(0), sequence = useRef(0), operation = useRef(null), requestIntent = useRef(null), active = useRef(new Map());
  const selected = view.guideId === state.id ? view : { cloud: null, intent: null, removalIntent: null, loaded: false, unavailable: true, storageError: '' };
  latest.current = { state, build, blocked, selected };
  const cloud = selected.cloud;
  const removalStarted = !!selected.removalIntent || gcpRemovalLifecycle(cloud?.status);
  const current = !removalStarted && gcpCloudCurrent(cloud, state, build, blocked);
  const preparationStep = !removalStarted && PREPARATION_STEPS.get(cloud?.phase);
  const stepCaption = selected.unavailable ? 'Last observed step' : cloud?.status === 'provisioning' ? 'Current step'
    : cloud?.status === 'vm-ready' ? 'Completed step' : 'Last recorded step';

  useEffect(() => {
    if (selectionBlocked || !cloud && !selected.removalIntent) return;
    onCloudRemoval?.({ guideId: state.id, operationId: cloud?.operation_id || selected.removalIntent.operation_id,
      available: !!cloud, pending: removalStarted,
      status: gcpRemovalLifecycle(cloud?.status) ? cloud.status : null,
      requestId: cloud?.removal?.request_id || selected.removalIntent?.request_id || null });
  }, [state.id, cloud?.operation_id, cloud?.status, cloud?.removal?.request_id, selected.removalIntent, onCloudRemoval, selectionToken, selectionBlocked]);

  async function refresh(expectedLifetime = lifetime.current, expectedSelectionToken) {
    const ownerCurrent = captureGuideSelection(selectionScope, latest.current.state, expectedSelectionToken);
    if (!ownerCurrent) return;
    const id = latest.current.state.id, serial = ++sequence.current;
    const suffix = operation.current ? `?${new URLSearchParams({ operation_id: operation.current })}` : '';
    try {
      const value = await request(gcpDeploymentUrl(id) + suffix);
      if (!ownerCurrent() || expectedLifetime !== lifetime.current || id !== latest.current.state.id || serial !== sequence.current) return;
      if (value !== null && (!gcpCloudOwned(value, id, operation.current) || requestIntent.current && !gcpCloudMatchesIntent(value, requestIntent.current))) throw new Error('Saved cloud status does not identify this setup. Recheck before continuing.');
      if (value && latest.current.selected.cloud && !gcpCloudSameGeneration(value, latest.current.selected.cloud)) throw new Error('Cloud status does not match the original saved resource generation.');
      if (value && latest.current.selected.removalIntent && (value.operation_id !== latest.current.selected.removalIntent.operation_id ||
          value.removal && value.removal.request_id !== latest.current.selected.removalIntent.request_id)) throw new Error('Cloud status does not match the saved removal request.');
      if (latest.current.selected.cloud?.status === 'removed' && value?.status !== 'removed') throw new Error('The completed removal receipt could not be rechecked. Its last confirmation is preserved.');
      if (value) operation.current = value.operation_id;
      setView((old) => ({ ...old, guideId: id, cloud: value, loaded: true, unavailable: false }));
      setObservationError('');
    } catch (failure) {
      if (ownerCurrent() && expectedLifetime === lifetime.current && id === latest.current.state.id && serial === sequence.current) {
        setView((old) => ({ ...old, guideId: id, loaded: true, unavailable: true }));
        setObservationError(failure.message || 'Saved cloud status could not be checked. The original setup request is preserved.');
      }
    }
  }

  useEffect(() => {
    const epoch = ++lifetime.current;
    let intent = null, removalIntent = null, storageError = '';
    try { intent = gcpLoadIntent(localStorage, state.id); }
    catch (failure) { storageError = failure.message || 'The setup request cannot be saved on this browser.'; }
    try { removalIntent = gcpLoadRemovalIntent(localStorage, state.id); }
    catch (failure) { storageError = failure.message || 'The removal request cannot be read on this browser.'; }
    operation.current = removalIntent?.operation_id || intent?.request_id || null; requestIntent.current = intent;
    setView({ guideId: state.id, cloud: null, intent, removalIntent, loaded: false, unavailable: true, storageError });
    setBusy(''); setError(''); setObservationError(''); setConnection(null); setRemoveOpen(false); setDataLoss(false);
    let timer, stopped = false;
    async function poll() {
      if (stopped) return;
      await refresh(epoch);
      if (!stopped) timer = setTimeout(poll, 2000);
    }
    poll();
    return () => { stopped = true; clearTimeout(timer); lifetime.current += 1; };
  }, [state.id]);

  function publish(value, id, epoch, ownerCurrent) {
    if (!ownerCurrent() || epoch !== lifetime.current || id !== latest.current.state.id) return false;
    if (!gcpCloudOwned(value, id, operation.current) || requestIntent.current && !gcpCloudMatchesIntent(value, requestIntent.current)) throw new Error('The cloud response does not match the original setup request.');
    if (latest.current.selected.cloud && !gcpCloudSameGeneration(value, latest.current.selected.cloud)) throw new Error('The cloud response does not match the original saved resource generation.');
    const removalIntent = latest.current.selected.removalIntent;
    if (removalIntent && (value.operation_id !== removalIntent.operation_id || value.removal && value.removal.request_id !== removalIntent.request_id)) throw new Error('The cloud response does not match the original removal request.');
    sequence.current += 1;
    setView((old) => ({ ...old, guideId: id, cloud: value, loaded: true, unavailable: false }));
    return true;
  }

  async function create(checked) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const now = latest.current;
    if (active.current.has(state.id) || !checked?.isCurrent?.() || !gcpCanCreate(checked.value, now.state, now.build, now.blocked) ||
        !now.selected.loaded || now.selected.unavailable || now.selected.storageError || now.selected.cloud || now.selected.removalIntent) return;
    let intent = now.selected.intent;
    if (intent && !gcpIntentMatchesPlan(intent, checked.value, now.state, now.build, now.blocked)) {
      setError('The original setup request belongs to an earlier source or account. Preserve it and return to that accepted setup before retrying.'); return;
    }
    if (!intent) intent = gcpMakeIntent(checked.value, now.state, now.build, crypto.randomUUID());
    const epoch = lifetime.current, id = now.state.id;
    const activeRequest = { guideId: id };
    active.current.set(id, activeRequest); setBusy('create'); setError('');
    try {
      // Save public request identity before sending any cloud mutation. A lost
      // response can be observed or deliberately retried with this exact ID.
      gcpStoreIntent(localStorage, intent);
      operation.current = intent.request_id; requestIntent.current = intent; sequence.current += 1;
      setView((old) => ({ ...old, guideId: id, intent }));
      const { version, build_id, account, project, request_id } = intent;
      const value = await request(gcpDeploymentUrl(id, 'create'), { method: 'POST', body: { version, build_id, account, project, request_id } });
      publish(value, id, epoch, ownerCurrent);
    } catch (failure) {
      if (ownerCurrent() && epoch === lifetime.current) { setError(failure.message || 'Setup admission is uncertain. Recheck the original request before continuing.'); await refresh(epoch); }
    } finally { if (active.current.get(id) === activeRequest) active.current.delete(id); if (epoch === lifetime.current) setBusy(''); }
  }

  async function act(action) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const now = latest.current, chosen = now.selected.cloud, epoch = lifetime.current, id = now.state.id;
    if (active.current.has(state.id) || now.selected.removalIntent || now.selected.unavailable || !gcpCloudCurrent(chosen, now.state, now.build, now.blocked) ||
        action === 'connect' && chosen.status !== 'vm-ready' || action === 'resume' && !['blocked', 'resume-required'].includes(chosen.status)) return;
    const activeRequest = { guideId: id };
    active.current.set(id, activeRequest); setBusy(action); setError('');
    const requestScope = gcpPreflightScope(now.state, now.build, now.blocked);
    try {
      const body = { operation_id: chosen.operation_id, ...(action === 'connect' ? { version: now.state.version, build_id: now.build.build_id } : {}) };
      const value = await request(gcpDeploymentUrl(id, action), { method: 'POST', body });
      if (!ownerCurrent() || epoch !== lifetime.current || id !== latest.current.state.id ||
          requestScope !== gcpPreflightScope(latest.current.state, latest.current.build, latest.current.blocked)) return;
      if (action === 'resume') publish(value, id, epoch, ownerCurrent);
      else if (!remoteReceiptOwned(value, now.state) || value.target !== 'gcp' || value.cloud_operation_id !== chosen.operation_id || value.source_binding_current !== true ||
          !artifactVersionCurrent(value, now.state) || value.build_id !== chosen.binding.build_id ||
          value.accepted_source_digest !== chosen.binding.source_digest || value.package_sha256 !== chosen.binding.package_sha256 ||
          !Object.keys(chosen.endpoint).every((key) => value.endpoint[key] === chosen.endpoint[key])) throw new Error('The installation connection does not match the verified cloud computer.');
      else setConnection({ guideId: id, operationId: chosen.operation_id, deploymentId: value.deployment_id, sourceDigest: now.build.source_digest });
    } catch (failure) {
      if (ownerCurrent() && epoch === lifetime.current) { setError(failure.message || 'This owned cloud step could not be confirmed.'); await refresh(epoch); }
    } finally { if (active.current.get(id) === activeRequest) active.current.delete(id); if (epoch === lifetime.current) setBusy(''); }
  }

  async function remove() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const now = latest.current, chosen = now.selected.cloud, epoch = lifetime.current, id = now.state.id;
    if (active.current.has(state.id) || !chosen || !gcpCloudOwned(chosen, id) || now.selected.unavailable || now.selected.storageError ||
        chosen.status === 'removed' || chosen.status === 'removing' ||
        !now.selected.removalIntent && !chosen.removal && (!removeOpen || !dataLoss)) return;
    let intent = now.selected.removalIntent;
    if (intent && intent.operation_id !== chosen.operation_id) { setError('The saved removal request belongs to another cloud computer. Recheck its original receipt.'); return; }
    if (!intent) intent = gcpMakeRemovalIntent(chosen, chosen.removal?.request_id || crypto.randomUUID());
    const activeRequest = { guideId: id };
    active.current.set(id, activeRequest); setBusy('remove'); setError('');
    try {
      gcpStoreRemovalIntent(localStorage, intent);
      operation.current = chosen.operation_id; sequence.current += 1;
      setView((old) => ({ ...old, guideId: id, removalIntent: intent }));
      onCloudRemoval?.({ guideId: id, operationId: chosen.operation_id, available: true, pending: true,
        status: gcpRemovalLifecycle(chosen.status) ? chosen.status : null, requestId: intent.request_id });
      setRemoveOpen(false); setDataLoss(false); setConnection(null);
      const { operation_id, request_id, acknowledge_data_loss } = intent;
      const value = await request(gcpDeploymentUrl(id, 'remove'), { method: 'POST', body: { operation_id, request_id, acknowledge_data_loss } });
      if (!gcpRemovalLifecycle(value?.status) || value.removal?.request_id !== intent.request_id) throw new Error('Removal admission was not confirmed. Recheck the saved removal request before continuing.');
      publish(value, id, epoch, ownerCurrent);
    } catch (failure) {
      if (ownerCurrent() && epoch === lifetime.current) { setError(failure.message || 'Removal is unconfirmed. Recheck the original resources before continuing.'); await refresh(epoch); }
    } finally { if (active.current.get(id) === activeRequest) active.current.delete(id); if (epoch === lifetime.current) setBusy(''); }
  }

  const canOfferCreate = selected.loaded && !selected.unavailable && !selected.storageError && !cloud && !selected.removalIntent && !busy;
  if (!allowPreparation && !cloud && !selected.intent && !selected.removalIntent && !selected.storageError && (!selected.loaded || !selected.unavailable)) return null;
  return <div aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)} className="gcp-guided" aria-busy={!!busy}>
    {allowPreparation && !removalStarted && !removeOpen && <GcpPreflight selectionScope={selectionScope} state={state} build={build} blocked={blocked || !!busy} onChooseProject={onChooseProject}
      onProvision={canOfferCreate ? create : undefined}
      hasSavedRequest={!!selected.intent} />}
    {!selected.loaded && <p role="status">Checking the original cloud setup request...</p>}
    {selected.storageError && <p className="deployment-error" role="alert">{selected.storageError}</p>}
    {selected.intent && !cloud && !removalStarted && <p role="status">Your original setup request is preserved. Recheck its status; if it was not received, refresh the same destination plan and retry this request.</p>}
    {selected.removalIntent && !cloud && <p role="status">Your removal request is saved. Its original cloud resources have not been confirmed absent. Recheck this request; cloud charges may continue.</p>}
    {cloud && <section className="gcp-preflight gcp-provisioning" aria-label={removeOpen || removalStarted ? 'Cloud resource removal' : 'Cloud computer preparation'} data-cloud-status={cloud.status}>
      <span className="setup-eyebrow">YOUR CLOUD COMPUTER</span>
      <h3>{removeOpen ? 'Remove this cloud computer?' : removalStarted ? cloud.status === 'removed' ? 'Original cloud resources confirmed absent' : cloud.status === 'removing' ? 'Removing your cloud computer' : 'Cloud removal needs confirmation'
        : cloud.status === 'vm-ready' ? 'Your cloud computer is prepared' : cloud.status === 'provisioning' ? 'Preparing your cloud computer' : 'Continue the original cloud setup'}</h3>
      {preparationStep && !removeOpen && <p className="gcp-provisioning-step" role="status" data-preparation-phase={cloud.phase}>
        <strong>{stepCaption}:</strong> {preparationStep}
      </p>}
      {!removeOpen && <p>{removalStarted ? cloud.status === 'removed' ? 'Forge confirmed that this original VM, boot disk and SSH access rule are absent. This does not cover other resources or charges in your project.'
        : cloud.status === 'removing' ? 'The saved removal request is in progress. Remaining or unverified resources may continue to incur charges.'
        : 'Removal is incomplete or has not been confirmed. Recheck the original resources, then continue the same saved request. Cloud charges may continue.'
        : cloud.status === 'vm-ready' ? 'The VM and SSH access rule passed preparation checks.' : cloud.status === 'provisioning' ? 'Preparing the VM and SSH access rule.' : cloud.detail}</p>}
      <p className="gcp-resource-location"><strong>{cloud.destination.project}</strong> / {cloud.destination.zone} / {cloud.destination.instance}</p>
      {!removalStarted && !removeOpen && <p className="deployment-note">This card describes computer preparation. Application setup and current application status are shown below.</p>}
      {!removeOpen && cloud.status !== 'removed' && <CloudResourceGuidance destination={cloud.destination} removalAvailable removalInProgress={removalStarted} />}
      {!current && !removalStarted && !removeOpen && <p role="status">This cloud computer belongs to an earlier accepted setup. Keep its receipt; return to that source before continuing installation.</p>}
      {current && !removeOpen && cloud.status === 'vm-ready' && <button type="button" className="setup-action primary" disabled={selectionBlocked || !!busy || selected.unavailable}
        onClick={() => act('connect')}>{busy === 'connect' ? 'Checking private setup access...' : 'Continue to application setup'}</button>}
      {current && !removeOpen && ['resume-required', 'blocked'].includes(cloud.status) && <button type="button" className="setup-action" disabled={selectionBlocked || !!busy || selected.unavailable}
        onClick={() => act('resume')}>{busy === 'resume' ? 'Checking original resources...' : 'Recheck and continue this setup'}</button>}
      {(removeOpen || removalStarted) && <div className="gcp-removal" aria-label="Remove original cloud resources">
        <ul>{gcpRemovalResourceRows(cloud).map((row) => <li key={row.kind} data-removal-resource={row.kind}>
          <strong>{row.label}: {row.name}</strong>{!row.id && <span> - Resource identity not yet verified</span>}
          {removalStarted && <span className="gcp-removal-state" data-removal-state={row.status}>{selected.unavailable ? 'Last observed: ' : ''}{({ waiting: 'Waiting for removal checks', checking: 'Checking original resource', removing: 'Removal requested; checking absence', absent: 'Confirmed absent' })[row.status]}</span>}
        </li>)}</ul>
        {!removalStarted && <>
          <p>Removing this computer ends access to its application. Deleting the boot disk permanently deletes its files and data, including application records, private settings and history. Back up anything you need first. Forge does not make a backup.</p>
          <label><input type="checkbox" checked={dataLoss} disabled={selectionBlocked || !!busy} onChange={(event) => setDataLoss(event.target.checked)} /> I have saved what I need, or accept losing this disk's data.</label>
          <div className="deployment-actions"><button type="button" className="setup-action" disabled={selectionBlocked || !!busy} onClick={() => { setRemoveOpen(false); setDataLoss(false); }}>Keep cloud resources</button>
            <button type="button" className="setup-action primary" disabled={selectionBlocked || !!busy || selected.unavailable || !!selected.storageError || !dataLoss} onClick={remove}>Remove VM, disk and access rule</button></div>
        </>}
        {removalStarted && cloud.status !== 'removed' && cloud.status !== 'removing' && <button type="button" className="setup-action" disabled={selectionBlocked || !!busy || selected.unavailable || !!selected.storageError} onClick={remove}>{busy === 'remove' ? 'Sending saved removal request...' : 'Continue removal'}</button>}
        <details className="deployment-evidence"><summary>Original resource details</summary><ul>{gcpRemovalResourceRows(cloud).map((row) => <li key={row.kind}>{row.label}: {row.name} - {row.id ? `Saved resource ID: ${row.id}` : 'Resource identity not yet verified'}</li>)}</ul></details>
      </div>}
      {!removeOpen && !removalStarted && <button type="button" className="setup-action" disabled={selectionBlocked || !!busy || selected.unavailable || !!selected.storageError} onClick={() => { setDataLoss(false); setRemoveOpen(true); }}>Review cloud removal</button>}
    </section>}
    {connection?.guideId === state.id && connection.operationId === cloud?.operation_id && connection.sourceDigest === build?.source_digest && current && !removeOpen && <p role="status" className="gcp-application-handoff">Application access checks have started. Continue below when its destination checks pass. <a href={`#remote-cloud-${state.id}`} onClick={() => document.getElementById(`remote-cloud-${state.id}`)?.focus()}>Continue to private application setup</a></p>}
    {selected.unavailable && selected.loaded && <p className="deployment-warning" role="status">Cloud observation is unavailable. {cloud?.status === 'removed' ? 'The last confirmed removal receipt is preserved.'
      : removalStarted ? 'This does not prove removal finished; unverified resources may remain and cloud charges may continue.' : 'This does not prove creation failed or resources are absent.'}</p>}
    {!removeOpen && <button type="button" className="setup-action" disabled={selectionBlocked || !!busy} onClick={() => refresh(lifetime.current, selectionToken)}>{removalStarted ? 'Recheck removal' : 'Recheck original cloud setup'}</button>}
    {busy === 'create' && <p role="status">Sending your saved setup request. Its original identity is kept for recovery.</p>}
    {observationError && <p className="deployment-error" role="alert">{observationError}</p>}
    {error && <p className="deployment-error" role="alert">{error}</p>}
  </div>;
}
