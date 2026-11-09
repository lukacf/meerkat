const PREFIX = 'mobkit-animation-draft-v1:';
const MAX_RECORD_BYTES = 12000;
const id = value => typeof value === 'string' && value.length > 0 && value.length <= 256 && !/[\u0000-\u001f\u007f]/.test(value);
const hash = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const text = value => typeof value === 'string' && value.length <= 1000;

export function animationDraftSelection(setupId, proposalKey, view) {
  return id(setupId) && typeof proposalKey === 'string' && proposalKey && id(view)
    ? { setupId, proposalKey, view } : null;
}

const selectionKey = selection => selection && animationDraftSelection(selection.setupId, selection.proposalKey, selection.view)
  ? JSON.stringify([selection.setupId, selection.proposalKey, selection.view]) : null;

/** Raw human text only. Proposal content stays in memory; storage uses its hash. */
export function createAnimationDraftStore(storage) {
  const entries = new Map();
  const empty = () => ({ text: '', revision: 0, owner: null, persistent: false });
  const entry = selection => {
    const key = selectionKey(selection);
    if (!key) return empty();
    if (!entries.has(key)) entries.set(key, empty());
    return entries.get(key);
  };
  const persist = value => {
    value.persistent = false;
    if (!value.owner || value.localOnly) return;
    const key = PREFIX + JSON.stringify(value.owner);
    try {
      if (!storage?.getItem || !storage?.setItem || !storage?.removeItem) return;
      if (value.text) {
        const raw = JSON.stringify({ owner: value.owner, text: value.text, revision: value.revision });
        storage.setItem(key, raw);
        value.persistent = storage.getItem(key) === raw;
      } else {
        storage.removeItem(key);
        value.persistent = storage.getItem(key) === null;
      }
    } catch { /* The latest editable draft remains in this open page. */ }
  };
  return {
    read(selection) {
      const value = entry(selection);
      return { text: value.text, revision: value.revision, ready: !!value.owner, persistent: value.persistent };
    },
    attach(selection, proposalHash, persistToTab = true) {
      if (!selectionKey(selection) || !hash(proposalHash)) return false;
      const value = entry(selection), owner = { setup_id: selection.setupId, proposal_hash: proposalHash, view: selection.view };
      if (value.owner) return JSON.stringify(value.owner) === JSON.stringify(owner);
      value.localOnly = !persistToTab;
      if (value.localOnly) { value.owner = owner; return true; }
      let saved = null, persistent = false;
      try {
        if (storage?.getItem && storage?.setItem && storage?.removeItem) {
          const raw = storage.getItem(PREFIX + JSON.stringify(owner));
          persistent = true;
          if (typeof raw === 'string' && new TextEncoder().encode(raw).byteLength <= MAX_RECORD_BYTES) {
            const item = JSON.parse(raw);
            if (item && Object.keys(item).every(key => ['owner', 'text', 'revision'].includes(key)) &&
                JSON.stringify(item.owner) === JSON.stringify(owner) && text(item.text) &&
                Number.isSafeInteger(item.revision) && item.revision >= 0) saved = item;
          }
        }
      } catch { persistent = false; }
      value.owner = owner;
      if (value.revision === 0) {
        value.text = saved?.text || '';
        value.revision = saved?.revision || 0;
        value.persistent = persistent;
      } else persist(value); // A delayed read cannot replace an edit made while hashing.
      return true;
    },
    update(selection, rawText) {
      if (!selectionKey(selection) || !text(rawText)) return false;
      const value = entry(selection);
      value.text = rawText;
      value.revision = Math.min(Number.MAX_SAFE_INTEGER, value.revision + 1);
      persist(value);
      return true;
    },
    capture(selection) {
      const value = entry(selection), key = selectionKey(selection);
      return key && value.owner ? { key, owner: { ...value.owner }, text: value.text, revision: value.revision } : null;
    },
    clearSubmitted(selection, submitted) {
      const value = entry(selection);
      if (!submitted || submitted.key !== selectionKey(selection) ||
          JSON.stringify(submitted.owner) !== JSON.stringify(value.owner) ||
          value.text !== submitted.text || value.revision !== submitted.revision) return false;
      value.text = '';
      value.revision = Math.min(Number.MAX_SAFE_INTEGER, value.revision + 1);
      persist(value);
      return true;
    },
  };
}

export function animationSessionStorage() {
  try { return globalThis.sessionStorage; } catch { return null; }
}
