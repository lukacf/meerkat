import { canonicalJson } from "./state.mjs";

export const GUIDE_ATTACHMENT_ACCEPT = "image/png,image/jpeg,image/webp,image/gif,.txt,.md,.csv,.json";
export const GUIDE_ATTACHMENT_LIMIT = 4;
const FILE_LIMIT = 8 * 1024 * 1024;
const TOTAL_LIMIT = 16 * 1024 * 1024;
const TYPES = new Set(["image/png", "image/jpeg", "image/webp", "image/gif",
  "text/plain", "text/markdown", "text/csv", "application/json", "application/pdf"]);
const HEX = /^[a-f0-9]{64}$/;
const REF_KEYS = ["filename", "id", "media_type", "sha256", "size", "url"];

export function checkedGuideAttachment(guideId, value) {
  if (!guideId || !value || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).sort().join() !== REF_KEYS.join() || !HEX.test(value.id) || !HEX.test(value.sha256) ||
      typeof value.filename !== "string" || !value.filename.trim() || value.filename.length > 200 ||
      /[\x00-\x1f\x7f/\\]/.test(value.filename) ||
      !TYPES.has(value.media_type) || !Number.isInteger(value.size) || value.size <= 0 || value.size > FILE_LIMIT ||
      value.url !== `/api/guide/${encodeURIComponent(guideId)}/attachments/${value.id}`) return null;
  return { ...value };
}

function checkedRefs(guideId, values) {
  if (!Array.isArray(values) || values.length > GUIDE_ATTACHMENT_LIMIT) return null;
  const refs = values.map(value => checkedGuideAttachment(guideId, value));
  if (refs.some(value => !value) || new Set(refs.map(value => value.id)).size !== refs.length ||
      refs.reduce((size, value) => size + value.size, 0) > TOTAL_LIMIT) return null;
  return refs;
}

function attachmentRequest(guideId, request) {
  const refs = checkedRefs(guideId, request?.attachments);
  if (!request || !Number.isSafeInteger(request.version) || request.version < 1 ||
      !["answer", "chat"].includes(request.action) || typeof request.text !== "string" ||
      !Array.isArray(request.selected) || request.selected.some(value => typeof value !== "string") ||
      request.combine_options !== undefined && typeof request.combine_options !== "boolean" || !refs?.length) return null;
  // Match the ordinary request, including the defaults added by GuideResponse.
  // This is an attachment-consumption marker, never an operation-status receipt.
  return { version: request.version, action: request.action, selected: [...request.selected], text: request.text,
    attachments: refs, combine_options: request.combine_options === true, stage: request.stage ?? null,
    blueprint: request.blueprint ?? null, manual_intent: request.manual_intent ?? null };
}

export function guideAttachmentAccept(inputSurface) {
  return GUIDE_ATTACHMENT_ACCEPT + (inputSurface?.media_types?.includes("application/pdf") ? ",.pdf" : "");
}

export function guideAttachmentSelectionError(files, existing = [], inputSurface) {
  if (!files.length) return "Choose a file to attach.";
  if (files.length + existing.length > GUIDE_ATTACHMENT_LIMIT) return "Attach up to four files at a time. Remove a file before adding another.";
  for (const file of files) {
    if (!Number.isInteger(file.size) || file.size <= 0) return "Empty files cannot be attached.";
    if (file.size > FILE_LIMIT) return "Each attachment must be 8 MiB or smaller.";
    const pdf = file.type === "application/pdf" || /\.pdf$/i.test(file.name || "");
    if (pdf && !inputSurface?.media_types?.includes("application/pdf"))
      return "PDF upload is unavailable on this setup server. Use a text or image copy instead.";
    if (!TYPES.has(file.type) && !/\.(png|jpe?g|webp|gif|txt|md|markdown|csv|json|pdf)$/i.test(file.name || ""))
      return "Choose PNG, JPEG, WebP, GIF, text, Markdown, CSV or JSON files.";
  }
  if ([...files, ...existing].reduce((size, file) => size + file.size, 0) > TOTAL_LIMIT)
    return "Attachments must total 16 MiB or less. Remove a file before adding another.";
  return "";
}

/** Recover checked references and their send ownership in this tab; never raw files or preview URLs. */
export function createGuideAttachmentStore(storageFactory = () => globalThis.sessionStorage) {
  const memory = new Map();
  const sends = new Map();
  let persistent = true;
  const key = id => `mobkit-guide-attachments-v1:${id}`;
  const sendKey = id => `mobkit-guide-attachment-send-v1:${id}`;
  const storage = () => { try { return storageFactory(); } catch { persistent = false; return null; } };
  function read(id) {
    if (!id) return [];
    if (!memory.has(id)) {
      let refs = [];
      try {
        const raw = storage()?.getItem(key(id));
        if (raw) {
          const saved = JSON.parse(raw);
          if (saved?.guide_id === id && Object.keys(saved).sort().join() === "attachments,guide_id")
            refs = checkedRefs(id, saved.attachments) || [];
        }
      } catch { persistent = false; }
      memory.set(id, refs);
    }
    return memory.get(id).map(ref => ({ ...ref }));
  }
  function write(id, refs) {
    const checked = checkedRefs(id, refs);
    if (!checked) throw new Error("The uploaded files could not be matched to this setup. Choose them again.");
    memory.set(id, checked);
    try {
      const target = storage();
      if (!target) persistent = false;
      else if (checked.length) target.setItem(key(id), JSON.stringify({ guide_id: id, attachments: checked }));
      else target.removeItem(key(id));
    } catch { persistent = false; }
    return read(id);
  }
  function readSend(id) {
    if (!sends.has(id)) {
      let value = null;
      try {
        const saved = JSON.parse(storage()?.getItem(sendKey(id)) || "null");
        const request = attachmentRequest(id, saved?.request);
        if (saved?.guide_id === id && request && typeof saved.consumed === "boolean" &&
            Array.isArray(saved.newer) && saved.newer.every(file => request.attachments.some(ref => ref.id === file)))
          value = { guide_id: id, request, consumed: saved.consumed, newer: [...new Set(saved.newer)] };
      } catch { persistent = false; }
      sends.set(id, value);
    }
    return sends.get(id);
  }
  function writeSend(id, value) {
    sends.set(id, value);
    try {
      const target = storage();
      if (!target) persistent = false;
      else target.setItem(sendKey(id), JSON.stringify(value));
    } catch { persistent = false; }
  }
  function changed(id, files) {
    const send = readSend(id);
    if (!send || send.consumed) return;
    const original = new Set(send.request.attachments.map(ref => ref.id));
    const newer = [...new Set([...send.newer, ...files.filter(file => original.has(file))])];
    if (newer.length !== send.newer.length) writeSend(id, { ...send, newer });
  }
  function consume(id, request) {
    const expected = attachmentRequest(id, request), send = readSend(id);
    if (!expected || !send || send.consumed || canonicalJson(send.request) !== canonicalJson(expected)) return read(id);
    const original = new Set(send.request.attachments.filter(ref => !send.newer.includes(ref.id)).map(ref => ref.id));
    const result = write(id, read(id).filter(ref => !original.has(ref.id)));
    writeSend(id, { ...send, consumed: true });
    return result;
  }
  return {
    read, persistent: () => persistent,
    add(id, refs) {
      const result = write(id, [...new Map([...read(id), ...refs].map(ref => [ref.id, ref])).values()]);
      changed(id, refs.map(ref => ref.id));
      return result;
    },
    remove(id, attachmentId) {
      const result = write(id, read(id).filter(ref => ref.id !== attachmentId));
      changed(id, [attachmentId]);
      return result;
    },
    capture(id, request) {
      const expected = attachmentRequest(id, request);
      if (expected) writeSend(id, { guide_id: id, request: expected, consumed: false, newer: [] });
    },
    admitted(id, refs, request) {
      if (request) return consume(id, request);
      const sent = new Set(refs.map(ref => ref.id));
      return write(id, read(id).filter(ref => !sent.has(ref.id)));
    },
    completed(id, operation) {
      if (operation?.status !== "completed" || !HEX.test(operation.request_id || "") ||
          operation.version !== operation.request?.version) return read(id);
      return consume(id, operation.request);
    },
  };
}

export async function uploadGuideAttachments(guideId, files, { fetch: send = globalThis.fetch, signal } = {}) {
  const body = new FormData();
  for (const file of files) body.append("files", file, file.name);
  const response = await send(`/api/guide/${encodeURIComponent(guideId)}/attachments`, { method: "POST", body, signal });
  let result;
  try { result = await response.json(); } catch {}
  if (!response.ok) throw new Error(typeof result?.detail === "string" ? result.detail : "The files could not be uploaded. Try again. Your draft is kept.");
  const refs = checkedRefs(guideId, result?.attachments);
  if (!refs || refs.length !== files.length) throw new Error("The uploaded files could not be matched to this setup. Choose them again.");
  return refs;
}
