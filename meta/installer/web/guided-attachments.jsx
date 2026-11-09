import React, { useEffect, useRef, useState } from "react";
import { createGuideAttachmentStore, guideAttachmentSelectionError, uploadGuideAttachments } from "./guided-attachments.mjs";

export function useGuideAttachments(guideId, inputSurface) {
  const [store] = useState(() => createGuideAttachmentStore());
  const [, refresh] = useState(0);
  const [pendingByGuide, setPendingByGuide] = useState(() => new Map());
  const [uploadingGuide, setUploadingGuide] = useState(null);
  const [stoppedUploadGuide, setStoppedUploadGuide] = useState(null);
  const [errorsByGuide, setErrorsByGuide] = useState(() => new Map());
  const owner = useRef(null), mounted = useRef(true), previews = useRef(new Set());
  const pending = pendingByGuide.get(guideId) || [];
  const redraw = () => { if (mounted.current) refresh(value => value + 1); };
  function errorFor(id, message) {
    setErrorsByGuide(current => {
      const next = new Map(current);
      if (message) next.set(id, message); else next.delete(id);
      return next;
    });
  }
  function replacePending(id, items) {
    setPendingByGuide(current => {
      const next = new Map(current);
      if (items.length) next.set(id, items); else next.delete(id);
      return next;
    });
  }
  function discardPending(id, items) {
    setPendingByGuide(current => {
      const next = new Map(current);
      const kept = (current.get(id) || []).filter(item => !items.includes(item));
      if (kept.length) next.set(id, kept); else next.delete(id);
      return next;
    });
  }
  function release(items) {
    for (const item of items) if (item.preview) {
      URL.revokeObjectURL?.(item.preview);
      previews.current.delete(item.preview);
    }
  }
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      owner.current?.controller.abort();
      for (const preview of previews.current) URL.revokeObjectURL?.(preview);
    };
  }, []);
  useEffect(() => {
    const upload = owner.current;
    if (upload && upload.guideId !== guideId) {
      owner.current = null;
      upload.controller.abort();
      release(upload.items);
      discardPending(upload.guideId, upload.items);
      setUploadingGuide(current => current === upload.guideId ? null : current);
    }
  }, [guideId]);
  async function upload(id, files, retry = false) {
    if (owner.current) return;
    const failure = guideAttachmentSelectionError(files, store.read(id), inputSurface);
    if (failure) { errorFor(id, failure); return; }
    const previous = pendingByGuide.get(id) || [];
    if (!retry) release(previous);
    const items = retry ? previous : files.map((file, index) => {
      const preview = file.type.startsWith("image/") && URL.createObjectURL ? URL.createObjectURL(file) : null;
      if (preview) previews.current.add(preview);
      return { id: `${index}:${file.name}`, guideId: id, file, preview };
    });
    const controller = new AbortController();
    const upload = { guideId: id, controller, items };
    owner.current = upload;
    replacePending(id, items);
    setUploadingGuide(id);
    setStoppedUploadGuide(current => current === id ? null : current);
    errorFor(id, null);
    try {
      const refs = await uploadGuideAttachments(id, files, { signal: controller.signal });
      if (!mounted.current || owner.current !== upload || controller.signal.aborted) return;
      store.add(id, refs);
      release(items);
      if (mounted.current && owner.current === upload) discardPending(id, items);
      redraw();
    } catch (failure) {
      if (mounted.current && owner.current === upload && failure.name !== "AbortError")
        errorFor(id, failure.message);
    } finally {
      if (owner.current === upload) {
        owner.current = null;
        if (mounted.current) setUploadingGuide(null);
      }
    }
  }
  return {
    refs: store.read(guideId), pending,
    uploading: uploadingGuide !== null && uploadingGuide === guideId,
    stopped: stoppedUploadGuide === guideId && pending.length > 0,
    error: errorsByGuide.get(guideId) || "", persistent: store.persistent(), upload,
    read: store.read,
    stopWaiting() {
      const upload = owner.current;
      if (!upload || upload.guideId !== guideId) return;
      owner.current = null;
      upload.controller.abort();
      setUploadingGuide(current => current === guideId ? null : current);
      setStoppedUploadGuide(guideId);
      errorFor(guideId, null);
    },
    fail(message) { errorFor(guideId, message); },
    remove(id) { store.remove(guideId, id); redraw(); },
    removePending(id) {
      const item = pending.find(item => item.id === id);
      release(item ? [item] : []);
      if (item) discardPending(guideId, [item]);
      errorFor(guideId, null);
    },
    retry() { return upload(guideId, pending.map(item => item.file), true); },
    capture: store.capture,
    admitted(id, refs, request) { store.admitted(id, refs, request); redraw(); },
    completed(id, operation) { store.completed(id, operation); redraw(); },
  };
}

export function GuideAttachmentChips({ attachments, preparing = false, inputSurface }) {
  const hasImage = attachments.refs.some(ref => ref.media_type.startsWith("image/")) ||
    attachments.pending.some(item => item.file.type.startsWith("image/"));
  const imageAnalysisDetail = inputSurface?.image_analysis_detail ||
    (inputSurface?.image_analysis === false
      ? "Your image is kept, but pixel analysis is unavailable for this guide."
      : "Your image is kept. Pixel analysis has not been verified for this guide.");
  return <div className="setup-attachments" aria-label="Files for your next message">
    {(attachments.refs.length > 0 || attachments.pending.length > 0) && <ul>
      {attachments.refs.map(ref => <li key={ref.id}>
        {ref.media_type.startsWith("image/") && <img src={ref.url} alt="" />}
        <a href={ref.url} target="_blank" rel="noreferrer">{ref.filename}</a>
        <button type="button" aria-label={`Remove attachment ${ref.filename}`} onClick={() => attachments.remove(ref.id)}>Remove</button>
      </li>)}
      {attachments.pending.map(item => <li key={item.id}>
        {item.preview && <img src={item.preview} alt="" />}
        <span>{item.file.name}</span>
        <button type="button" disabled={attachments.uploading} aria-label={`Remove attachment ${item.file.name}`}
          onClick={() => attachments.removePending(item.id)}>Remove</button>
      </li>)}
    </ul>}
    {(preparing || attachments.uploading) && <div className="setup-attachment-wait">
      <p role="status">Uploading files. Your draft and choices are kept.</p>
      {attachments.uploading && <button className="setup-attachment-stop" type="button" onClick={() => attachments.stopWaiting()}>Stop waiting</button>}
    </div>}
    {attachments.stopped && <p className="setup-attachment-recovery" role="status">Upload not confirmed. Your draft and files stay here. The server may still finish the upload. Retry the upload or remove the files before sending.</p>}
    {hasImage && inputSurface?.image_analysis !== true && <p className="setup-attachment-analysis-note">{imageAnalysisDetail}</p>}
    {attachments.error && <div role="alert"><p>{attachments.error}</p></div>}
    {attachments.pending.length > 0 && !attachments.uploading &&
      <button type="button" onClick={() => attachments.retry()}>Retry upload</button>}
    {!attachments.persistent && attachments.refs.length > 0 && <p role="status">Uploaded files stay available in this page, but attachment recovery after reload is unavailable.</p>}
  </div>;
}
