import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";
import { captureGuideSelection } from "./guide-selection.mjs";
import { ApplicationSetupCard } from "./application-setup.jsx";
import { remoteDeploymentUrl, remoteActionAvailable } from "./remote-deployment.mjs";
import { remoteApplicationSetupKey, remoteApplicationSetupView } from "./remote-application-setup.mjs";

export function RemoteApplicationSetup({ receipt, options, build, state, selectionScope, confirmed = false,
  pending = false, disabled = false, onAction, onSettings, onCheck }) {
  const key = remoteApplicationSetupKey(receipt, options, build, state), token = selectionScope?.token();
  const [snapshot, setSnapshot] = useState(null), [retry, setRetry] = useState(0);
  const mounted = useRef(false), latest = useRef(null);
  latest.current = { key, token, receipt, options, build, state, confirmed, pending, disabled };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    const ownerCurrent = captureGuideSelection(selectionScope, state, token);
    if (!key || !confirmed || pending || disabled || receipt.pending_operation || !ownerCurrent) return;
    let cancelled = false;
    setSnapshot({ key, pending: true, value: null });
    const query = new URLSearchParams({ deployment_id: receipt.deployment_id });
    request(`${remoteDeploymentUrl(state.id, "application-setup")}?${query}`).then((value) => {
      if (!cancelled && ownerCurrent() && latest.current.key === key && latest.current.token === token &&
          latest.current.confirmed && !latest.current.disabled) setSnapshot({ key, pending: false, value });
    }).catch(() => {
      if (!cancelled && ownerCurrent() && latest.current.key === key && latest.current.token === token)
        setSnapshot({ key, pending: false, value: null });
    });
    return () => { cancelled = true; };
  }, [key, token, confirmed, pending, disabled, receipt.pending_operation, retry]);
  const view = remoteApplicationSetupView({ receipt, options, build, state, confirmed, pending, disabled, snapshot });
  latest.current.view = view;
  if (!view) return null;
  function current() {
    return mounted.current && !!captureGuideSelection(selectionScope, state, token) && latest.current.key === key &&
      latest.current.token === token && latest.current.view === view && latest.current.confirmed &&
      !latest.current.pending && !latest.current.disabled && !latest.current.receipt.pending_operation;
  }
  function send(action) {
    const now = latest.current;
    if (current() && remoteActionAvailable(now.receipt, action, now.options, now.build, now.state)) onAction?.(action);
  }
  return <ApplicationSetupCard view={view} disabled={disabled || pending}
    onStart={() => send(view.action || "finish-setup")} onBackground={() => send("start-background")}
    onSettings={onSettings ? () => { if (current()) onSettings(); } : null}
    onCheck={() => { if (mounted.current && captureGuideSelection(selectionScope, state, token) && !latest.current.disabled) {
      onCheck?.(token); setRetry((value) => value + 1);
    } }} />;
}
