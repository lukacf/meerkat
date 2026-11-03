import { fingerprint } from './state.mjs';

const PREFIX = 'mobkit-animation-last-displayed-v1:';
const MAX_BYTES = 8_000_000;
const record = value => !!value && typeof value === 'object' && !Array.isArray(value);
const id = value => typeof value === 'string' && value.length > 0 && value.length <= 256
  && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
const hash = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const fields = ['version', 'setup_id', 'stream_id', 'operation_id', 'blueprint', 'model'];
const selectionFields = ['version', 'selection', 'setup_id', 'stream_id', 'operation_id', 'blueprint_hash', 'model_hash', 'code_hash'];
const key = scope => scope && id(scope.setup_id) && id(scope.stream_id)
  ? PREFIX + JSON.stringify([scope.setup_id, scope.stream_id]) : null;
const canonical = value => Array.isArray(value) ? value.map(canonical) : record(value)
  ? Object.fromEntries(Object.keys(value).sort().map(name => [name, canonical(value[name])])) : value;
const equal = (left, right) => JSON.stringify(canonical(left ?? null)) === JSON.stringify(canonical(right ?? null));

function snapshotMatches(value, scope) {
  return record(value) && Object.keys(value).length === fields.length && fields.every(name => Object.hasOwn(value, name))
    && value.version === 1 && hash(value.operation_id) && value.setup_id === scope?.setup_id
    && value.stream_id === scope?.stream_id && record(value.blueprint) && record(value.model);
}

function selectionMatches(value, scope) {
  return !!key(scope) && record(value) && Object.keys(value).length === selectionFields.length
    && selectionFields.every(field => Object.hasOwn(value, field)) && value.version === 1 && value.selection === 'saved'
    && value.setup_id === scope.setup_id && value.stream_id === scope.stream_id
    && ['operation_id', 'blueprint_hash', 'model_hash', 'code_hash'].every(field => hash(value[field]));
}

export function readAnimationRecovery(storage, scope) {
  const name = key(scope);
  if (!name) return null;
  try {
    const raw = storage?.getItem(name);
    if (typeof raw !== 'string' || raw.length > MAX_BYTES || new TextEncoder().encode(raw).length > MAX_BYTES) return null;
    const value = JSON.parse(raw);
    return snapshotMatches(value, scope) ? value : null;
  } catch { return null; }
}

/** A prior explicit source choice, never source code or invented author feedback. */
export function readAnimationSelection(storage, scope) {
  const name=key(scope);
  if(!name)return null;
  try{
    const raw=storage?.getItem(name);
    if(typeof raw!=='string' || raw.length>2000)return null;
    const value=JSON.parse(raw);
    return selectionMatches(value, scope) ? value : null;
  }catch{return null;}
}

/** A displayed scene reference and its inputs only. Animation code comes from the host. */
export function saveAnimationRecovery(storage, scope, candidate) {
  const name = key(scope), receipt = candidate?.receipt;
  if (!name || !record(receipt) || receipt.setup_id !== scope.setup_id || receipt.stream_id !== scope.stream_id
      || candidate.streamId !== scope.stream_id || !hash(receipt.id)) return false;
  try {
    if(candidate.adopted && !candidate.resumed){
      const value={version:1,selection:'saved',...scope,operation_id:receipt.id,
        blueprint_hash:receipt.blueprint_hash,model_hash:receipt.model_hash,code_hash:candidate.artifact?.code_hash};
      if(!['blueprint_hash','model_hash','code_hash'].every(field=>hash(value[field])))return false;
      const raw=JSON.stringify(value);storage?.setItem(name,raw);
      return storage?.getItem(name)===raw;
    }
    const value = {version: 1, ...scope, operation_id: receipt.id, blueprint: candidate.plan, model: candidate.model};
    if (!snapshotMatches(value, scope)) return false;
    const raw = JSON.stringify(value);
    if (new TextEncoder().encode(raw).length > MAX_BYTES) return false;
    storage?.setItem(name, raw);
    return storage?.getItem(name) === raw;
  } catch { return false; }
}

export function forgetAnimationRecovery(storage, scope, operationId) {
  const saved = readAnimationRecovery(storage, scope) || readAnimationSelection(storage, scope);
  if (saved?.operation_id !== operationId) return false;
  try { storage.removeItem(key(scope)); return true; } catch { return false; }
}

function admission(value) {
  if (!record(value) || !id(value.interaction_id)) return false;
  const cursor = typeof value.cursor === 'string' && /^console:\d+$/.test(value.cursor)
    ? Number(value.cursor.slice(8)) : value.cursor;
  return Number.isSafeInteger(cursor) && cursor >= 0
    && ['session_id', 'input_frame_id', 'conversation_id'].every(name => value[name] == null || id(value[name]));
}

/** Validate a historical host publication without admitting work or claiming current desire. */
export async function recoveredAnimation(snapshot, receipt, scope) {
  if (!snapshotMatches(snapshot, scope) || !record(receipt) || receipt.id !== snapshot.operation_id
      || receipt.setup_id !== scope.setup_id || receipt.stream_id !== scope.stream_id
      || !['published', 'completed'].includes(receipt.status)
      || !Number.isSafeInteger(receipt.source_generation) || receipt.source_generation < 1
      || !hash(receipt.request_fingerprint) || !hash(receipt.native_request_key)
      || !admission(receipt.native_admission)) return null;
  const artifact = receipt.artifact, publication = artifact?.publication_receipt;
  if (!record(artifact) || artifact.contract !== 'freeform-animation-v3' || !record(publication)
      || !['publish_visualization', 'native_publish_acknowledgment'].includes(publication.authority)
      || artifact.key !== receipt.native_request_key || publication.request_key !== artifact.key
      || typeof artifact.code !== 'string' || artifact.code.length < 20 || artifact.code.length > 48000
      || !equal(receipt.revision_binding, artifact.revision_binding)
      || !equal(publication.revision_binding, artifact.revision_binding)
      || artifact.native_admission != null && !equal(artifact.native_admission, receipt.native_admission)) return null;
  for (const name of ['blueprint_hash', 'model_hash', 'reference_hash', 'code_hash']) {
    if (!hash(artifact[name]) || publication[name] !== artifact[name]) return null;
  }
  if (artifact.blueprint_hash !== receipt.blueprint_hash || artifact.model_hash !== receipt.model_hash) return null;
  if (receipt.revision_binding != null && (!record(receipt.revision_binding)
      || receipt.revision_binding.version !== 'prior-program-v1'
      || receipt.revision_binding.setup_id !== scope.setup_id || receipt.revision_binding.stream_id !== scope.stream_id
      || !hash(receipt.revision_binding.context_hash))) return null;
  try {
    const [blueprintHash, modelHash, codeBytes] = await Promise.all([
      fingerprint(snapshot.blueprint), fingerprint(snapshot.model),
      crypto.subtle.digest('SHA-256', new TextEncoder().encode(artifact.code)),
    ]);
    const codeHash = [...new Uint8Array(codeBytes)].map(value => value.toString(16).padStart(2, '0')).join('');
    if (blueprintHash !== receipt.blueprint_hash || modelHash !== receipt.model_hash || codeHash !== artifact.code_hash) return null;
    return {artifact: structuredClone(artifact), receipt: structuredClone(receipt),
      plan: structuredClone(snapshot.blueprint), model: structuredClone(snapshot.model)};
  } catch { return null; }
}

/** Recover a prior selected source only for the current exact proposal and view. */
export async function recoveredAnimationSelection(selection, receipt, scope, blueprint, model) {
  if (!selectionMatches(selection, scope) || !record(blueprint) || !record(model) || !record(receipt)
      || receipt.id !== selection.operation_id || receipt.blueprint_hash !== selection.blueprint_hash
      || receipt.model_hash !== selection.model_hash || receipt.artifact?.code_hash !== selection.code_hash) return null;
  try {
    const snapshot = {version: 1, setup_id: scope.setup_id, stream_id: scope.stream_id,
      operation_id: selection.operation_id, blueprint: structuredClone(blueprint), model: structuredClone(model)};
    const recovered = await recoveredAnimation(snapshot, receipt, scope);
    return recovered?.artifact.code_hash === selection.code_hash ? recovered : null;
  } catch { return null; }
}
