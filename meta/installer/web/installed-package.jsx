import React, { useEffect, useRef, useState } from "react";
import { captureGuideSelection, guideSelectionAvailable } from "./guide-selection.mjs";
import { requestBlob } from "./state.mjs";
import { installedPackageBinding, installedPackageUrl, verifyInstalledPackage } from "./installed-package.mjs";

export function InstalledPackageDownload({ receipt, state, selectionScope, blocked = false }) {
  const binding = installedPackageBinding(receipt, state);
  const token = selectionScope?.token();
  const unavailable = blocked || !guideSelectionAvailable(selectionScope, state, token);
  const key = JSON.stringify([state?.id, state?.version, binding, unavailable, token]);
  const selection = useRef(null), operation = useRef(null);
  if (selection.current?.key !== key) selection.current = { key };
  const owner = selection.current;
  const [pending, setPending] = useState(null), [failure, setFailure] = useState(null);
  useEffect(() => () => {
    if (operation.current?.owner === owner) {
      operation.current.controller.abort();
      operation.current = null;
    }
  }, [owner]);

  async function download() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, token);
    if (!binding || unavailable || !ownerCurrent || selection.current !== owner || operation.current?.owner === owner) return;
    const request = { owner, controller: new AbortController() };
    operation.current = request;
    const current = () => operation.current === request && selection.current === owner && ownerCurrent();
    setPending(owner); setFailure(null);
    try {
      const blob = await requestBlob(installedPackageUrl(state.id, binding), {
        timeoutMs: 60000, signal: request.controller.signal,
      });
      if (!current()) return;
      await verifyInstalledPackage(blob, binding.package_sha256);
      if (!current()) return;
      const url = URL.createObjectURL(blob), link = document.createElement("a");
      try {
        link.href = url;
        link.download = `forge-installed-application-${binding.build_id.slice(0, 8).replace(/[^a-zA-Z0-9_-]/g, "") || "source"}.zip`;
        link.click();
      } finally { setTimeout(() => URL.revokeObjectURL(url), 1000); }
    } catch (error) {
      if (current()) setFailure({ owner, message: error.message || "The installed package could not be downloaded. Try again." });
    } finally {
      if (operation.current === request) operation.current = null;
      setPending(previous => previous === owner ? null : previous);
    }
  }

  if (!binding) return null;
  return <details className="deployment-evidence deployment-installed-package">
    <summary>Keep a copy of this installed version</summary>
    <p>Download the checked application package saved with this installation, even if a newer build needs repair.</p>
    <button className="setup-action" type="button" disabled={unavailable || pending === owner} onClick={download}>
      {pending === owner ? "Checking installed package..." : "Download installed package"}
    </button>
    {failure?.owner === owner && <p className="deployment-error" role="alert">{failure.message}</p>}
    <p className="deployment-note">Application source only. Private settings and saved records are not included. Your selected build and installation stay unchanged.</p>
  </details>;
}
