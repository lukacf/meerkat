import React, { useEffect, useRef, useState } from 'react';
import { request } from './state.mjs';
import { guidePlan } from './evolving-plan-state.mjs';
import { DeploymentPanel } from './deployment-panel.jsx';
import { deploymentSetups, readDeploymentSummaries } from './my-deployments.mjs';
import './my-deployments.css';

export function MyDeployments({ onClose, onOpenSetup, Icon }) {
  const dialog = useRef(null), heading = useRef(null), lifetime = useRef(null);
  const [revision, setRevision] = useState(0), [selected, setSelected] = useState(null);
  const [view, setView] = useState({ loading: true, setups: [], rows: {}, error: '' });
  const [detail, setDetail] = useState(null);
  // Retire action closures synchronously, before React unmounts their controls.
  const retire = () => { lifetime.current = null; };
  const close = () => { retire(); onClose(); };
  const choose = setup => { retire(); setDetail(null); setSelected(setup); };
  const back = () => { retire(); setSelected(null); setDetail(null); setRevision(value => value + 1); };
  useEffect(() => {
    const node = dialog.current, previous = document.activeElement;
    node.showModal();
    return () => { retire(); node.close(); if (previous?.isConnected) previous.focus(); };
  }, []);
  useEffect(() => { heading.current?.focus(); }, [selected?.id]);
  useEffect(() => {
    if (selected) return;
    const controller = new AbortController();
    setView({ loading: true, setups: [], rows: {}, error: '' });
    (async () => {
      try {
        const setups = deploymentSetups(await request('/api/guide/list?include_all=true', { signal: controller.signal, timeoutMs: 30000 }));
        if (controller.signal.aborted) return;
        setView({ loading: true, setups, rows: {}, error: '' });
        await readDeploymentSummaries(setups, request, controller.signal, (setup, entries) => {
          setView(previous => ({ ...previous, rows: { ...previous.rows, [setup.id]: entries } }));
        });
        if (!controller.signal.aborted) setView(previous => ({ ...previous, loading: false }));
      } catch {
        if (!controller.signal.aborted) setView(previous => ({ ...previous, loading: false,
          error: 'Your deployments could not be listed. This does not mean they are absent. Retry to check again.' }));
      }
    })();
    return () => controller.abort();
  }, [revision, selected?.id]);
  useEffect(() => {
    if (!selected) return;
    const controller = new AbortController(), owner = { id: selected.id };
    lifetime.current = owner;
    const current = () => lifetime.current === owner && !controller.signal.aborted;
    (async () => {
      try {
        const state = await request(`/api/guide/${encodeURIComponent(selected.id)}`, { signal: controller.signal, timeoutMs: 30000 });
        if (!current()) return;
        if (state?.id !== owner.id || !Number.isSafeInteger(state.version) || state.version < 0)
          throw new Error('The saved setup identity could not be verified.');
        const selectionScope = { token: () => owner, capture: (guide, token) => {
          if (!current() || guide?.id !== owner.id || token !== undefined && token !== owner) return null;
          return current;
        } };
        setDetail({ state, selectionScope });
      } catch {
        if (current()) setDetail({ error: 'This setup could not be verified. Return to the list and retry; no action was sent.' });
      }
    })();
    return () => { controller.abort(); if (lifetime.current === owner) retire(); };
  }, [selected?.id]);

  const shown = view.setups.filter(setup => view.rows[setup.id]?.length);
  return <dialog ref={dialog} className="my-deployments" aria-labelledby="my-deployments-title"
    onCancel={event => { event.preventDefault(); close(); }}>
    <header className="my-deployments-header">
      <div><span className="setup-eyebrow">YOUR WORKSPACE</span>
        <h1 id="my-deployments-title" tabIndex={-1} ref={heading}>{selected ? selected.name : 'My deployments'}</h1></div>
      <button type="button" className="setup-action" onClick={close}>Close</button>
    </header>
    {selected ? <>
      <nav className="deployment-actions" aria-label="Deployment navigation">
        <button type="button" className="setup-action" onClick={back}>All my deployments</button>
        <button type="button" className="setup-action" onClick={() => { retire(); onOpenSetup(selected.id); }}>Open setup conversation</button>
      </nav>
      <p>Use the checked controls below to open, stop or restart this deployment where supported. Stop keeps files and cloud resources.</p>
      <p>Local and remote folders have no generic uninstall. Remove cloud computer only removes this setup's recorded cloud resources after you review the resources and accept permanent disk data loss.</p>
      {!detail && <p role="status">Reading this setup's deployment controls...</p>}
      {detail?.error && <p role="alert">{detail.error}</p>}
      {detail?.state && <DeploymentPanel key={selected.id} state={detail.state}
        plan={guidePlan(detail.state) || { project: { name: selected.name } }} build={null}
        designBusy={detail.state.operation?.status === 'running'} pendingManual={false} Icon={Icon}
        selectionScope={detail.selectionScope} managementOnly />}
    </> : <>
      <p>Deployments recorded by Forge in your workspace, across all saved setups. This is not a search of your cloud projects or other users' deployments.</p>
      <div className="deployment-actions"><button type="button" className="setup-action" disabled={view.loading}
        onClick={() => setRevision(value => value + 1)}>Refresh deployments</button>
        {view.loading && <span role="status">Checking recorded deployments...</span>}</div>
      {view.error && <p role="alert">{view.error}</p>}
      {!view.loading && !view.error && !shown.length && <div className="my-deployments-empty">
        <h2>No recorded deployments</h2><p>Your saved setups remain available in Your setups. Once deployed through Forge, their receipts appear here.</p></div>}
      {shown.map(setup => <article className="my-deployment-card" key={setup.id}>
        <h2>{setup.name}</h2>
        <ul>{view.rows[setup.id].map(entry => <li key={entry.kind}>
          <strong>{entry.label}</strong><span className={entry.unknown ? 'deployment-warning' : 'my-deployment-status'}>{entry.status}</span>
          <code>{entry.destination}</code>{entry.removal && <p>{entry.removal}</p>}
        </li>)}</ul>
        <button type="button" className="setup-action primary" onClick={() => choose(setup)}>Open and manage</button>
      </article>)}
      {shown.length > 0 && <p className="deployment-note">Status reflects the latest check in this view. Open and manage rechecks the original receipts before offering actions.</p>}
    </>}
  </dialog>;
}
