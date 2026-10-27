// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the rpc-doc-ops functions move byte-verbatim as plain JS, and
// their `options = {}` / `params = {}` / `row = {}` parameter defaults plus
// destructured request options raise TS2339/TS2345 under .ts semantics.
// Source-contract pins this exact text (e.g. saveDocument's guard threading
// and applyAuthoringOperationDocument's snapshot id), so suppression must
// live at file level, not in the moved bodies. Resolution/linkage stays
// guarded behaviorally: the projection suite and export-keys test load the
// bundle and exercise these functions, so a missed import or re-export
// still fails the gate as a ReferenceError.
//
// Document-operation RPC wrappers for the Flow Editor controller plane.
// Moved verbatim from the controller.js rpc-doc-ops range: schema/
// capability/catalog loads, document validate/source/export/deploy/import/
// list/get/create/save/delete, Meta-owned undo/redo history steps, the
// authoring operation apply path with draft and catalog snapshot guards,
// createAuthoringOperationRunner (a stateful per-runner promise queue —
// moved intact as a closure factory), graph projection RPCs, and decoded
// file import params projection.
import { authoringProjectionFromOperationResult } from "../catalogs/hydration";
import { normalizeDeploySettings } from "../document/build-projection";
import { flowRegistryRowIsRuntimeProjection } from "../registry/flow-registry";
import {
  authoringOperationAvailability,
  authoringOperationFromIntent,
  callRpc,
  rpcMethod,
} from "./client";

export async function loadSchema(options = {}) {
  return callRpc(rpcMethod("schema"), {}, options);
}

export async function loadCapabilities(options = {}) {
  return callRpc("meta/capabilities", {}, options);
}

export async function loadCatalogs(options = {}) {
  return callRpc(rpcMethod("catalogs"), {}, options);
}

export async function validateDocument(document, options = {}) {
  const { signal, rkatValidate, rkat_validate, ...requestOptions } = options || {};
  return callRenderRpc(rpcMethod("validate"), {
    document,
    rkat_validate: rkatValidate ?? rkat_validate ?? true,
  }, requestOptions, signal);
}

export async function sourceDocument(document, options = {}) {
  const { signal, ...requestOptions } = options || {};
  return callRenderRpc(rpcMethod("source"), { document }, requestOptions, signal);
}

export async function exportDocument(document, options = {}) {
  const { signal, ...requestOptions } = options || {};
  return callRpc(rpcMethod("export"), { document, ...requestOptions }, { signal });
}

export async function deployDocument(document, options = {}) {
  const { signal, ...requestOptions } = options || {};
  // Only the plan (`execute: false`) is a read-only render. A real deploy
  // keeps the optimistic guard as-is: deploying a draft whose store revision
  // moved underneath the caller is exactly what the guard is there to refuse.
  if (requestOptions.execute === false) {
    return callRenderRpc(rpcMethod("deploy"), { document }, requestOptions, signal);
  }
  return callRpc(rpcMethod("deploy"), { document, ...requestOptions }, { signal });
}

const DRAFT_GUARD_KEYS = ["expected_revision", "expected_etag"];

function hasDraftGuard(requestOptions) {
  return DRAFT_GUARD_KEYS.some((key) => requestOptions?.[key] !== undefined && requestOptions?.[key] !== null);
}

// Read-only renders of the CLIENT's document: source preview, validation and
// the deploy plan. They carry the optimistic draft store guard of the registry
// row the caller last saw (`flowRegistryDraftGuard`), and the server refuses
// the render with a draft revision conflict when the store has moved on.
//
// The store moving on is routinely OUR OWN autosave: the projection applied by
// an authoring operation persists the row, the save reaches the server and
// bumps the revision, and until its response has been applied to the row the
// guard the UI hands out is one revision behind. A render issued inside that
// window was refused even though the submitted document IS the freshest
// authoring state; the inline source panel opened in Graph mode then stayed
// empty behind the source-failed validation sheet for as long as the caller
// waited (#398). `createAuthoringOperationRunner` already handles this exact
// race for mutations with a one-shot retry without the store guard. Same
// remedy, same scope here: the retry drops only the guard keys and re-submits
// the same document, and it fires only when a guard was present and the
// refusal is the guard's. Nothing here writes the store, so save-time
// concurrency control is unaffected.
async function callRenderRpc(method, params, requestOptions, signal) {
  const guardedOptions = requestOptions && typeof requestOptions === "object" ? requestOptions : {};
  try {
    return await callRpc(method, { ...params, ...guardedOptions }, { signal });
  } catch (error) {
    if (!hasDraftGuard(guardedOptions) || !isDraftGuardConflictError(error)) throw error;
    const unguardedOptions = Object.fromEntries(
      Object.entries(guardedOptions).filter(([key]) => !DRAFT_GUARD_KEYS.includes(key)),
    );
    return callRpc(method, { ...params, ...unguardedOptions }, { signal });
  }
}

export async function deployCommandPreviewForDocument(document, options = {}) {
  const { signal, packPath, prompt: optionPrompt, deploySettings, ...requestOptions } = options || {};
  const sourceDocument = document && typeof document === "object" ? document : {};
  const deploy = normalizeDeploySettings(sourceDocument.deploy || deploySettings);
  const prompt = String(optionPrompt || deploy.prompt || "").trim();
  const request = {
    document: {
      ...sourceDocument,
      deploy,
    },
    ...requestOptions,
  };
  if (String(packPath || "").trim()) request.pack_path = String(packPath).trim();
  if (prompt) request.prompt = prompt;
  return callRpc(rpcMethod("deployCommand"), request, { signal });
}

export async function importDocument(params, options = {}) {
  return callRpc(rpcMethod("import"), params || {}, options);
}

export async function listDocuments(params = {}, options = {}) {
  return callRpc(rpcMethod("list"), params || {}, options);
}

export async function getDocument(id, params = {}, options = {}) {
  return callRpc(rpcMethod("get"), { ...(params || {}), id }, options);
}

export async function createDocument(spec = {}, options = {}) {
  return callRpc(rpcMethod("create"), spec || {}, options);
}

// Meta-owned history steps over the draft store: the server restores a
// snapshot it recorded itself, so the browser never authors restore state.
export async function undoDocument(params = {}, options = {}) {
  return historyStepDocument("undo", params, options);
}

export async function redoDocument(params = {}, options = {}) {
  return historyStepDocument("redo", params, options);
}

export async function historyStepDocument(direction, params = {}, options = {}) {
  const { signal } = options || {};
  const request = { id: String(params.id || "").trim() };
  const expectedRevision = params.expected_revision ?? params.expectedRevision;
  if (expectedRevision !== undefined && expectedRevision !== null && expectedRevision !== "") {
    request.expected_revision = Number(expectedRevision);
  }
  const expectedEtag = String(params.expected_etag ?? params.expectedEtag ?? "").trim();
  if (expectedEtag) request.expected_etag = expectedEtag;
  return callRpc(rpcMethod(direction), request, { signal });
}

export async function saveDocument(row = {}, options = {}) {
  if (flowRegistryRowIsRuntimeProjection(row)) {
    return {
      ok: false,
      error: "runtime_projection_read_only",
      row: null,
      reason: "Runtime flow projections must be forked into a Meta draft before saving.",
    };
  }
  const document = row.document;
  const request = {
    id: row.id || row.currentFlowId,
    document,
    validation: row.validation ?? null,
    stage: row.stage,
    trigger: row.trigger,
    source: row.source,
  };
  const expectedRevision = row.expectedRevision ?? row.expected_revision ?? row.baseRevision ?? row.base_revision ?? row.revision ?? row.draft_revision;
  if (expectedRevision !== undefined && expectedRevision !== null && expectedRevision !== "") {
    request.expected_revision = Number(expectedRevision);
  }
  const expectedEtag = row.expectedEtag ?? row.expected_etag ?? row.draft_etag ?? row.etag;
  if (expectedEtag) {
    request.expected_etag = String(expectedEtag);
  }
  return callRpc(rpcMethod("save"), request, options);
}

export async function deleteDocument(id, params = {}, options = {}) {
  return callRpc(rpcMethod("delete"), { ...(params || {}), id }, options);
}

export async function applyAuthoringOperationDocument(document, operation, options = {}) {
  const {
    signal,
    catalogSnapshot,
    catalog_snapshot,
    expectedCatalogSnapshotId,
    expected_catalog_snapshot_id,
    ...requestOptions
  } = options || {};
  const expectedSnapshotId = String(
    expectedCatalogSnapshotId
    ?? expected_catalog_snapshot_id
    ?? catalogSnapshot?.id
    ?? catalog_snapshot?.id
    ?? catalogSnapshot
    ?? catalog_snapshot
    ?? "",
  ).trim();
  return callRpc(rpcMethod("applyOperation"), {
    document,
    operation,
    ...(expectedSnapshotId ? { expected_catalog_snapshot_id: expectedSnapshotId } : {}),
    ...requestOptions,
  }, { signal });
}

export function isDraftGuardConflictError(error) {
  const message = String(error?.message || error || "");
  return message.includes("draft revision conflict") || message.includes("draft etag conflict");
}

export function createAuthoringOperationRunner(options = {}) {
  const hooks = options && typeof options === "object" ? options : {};
  let queue = Promise.resolve();
  const runOperation = async (operation, enqueuedRevision) => {
    if (hooks.isRevisionCurrent && !hooks.isRevisionCurrent(enqueuedRevision)) {
      return {
        ok: false,
        error: hooks.getStaleError?.() || "Meta authoring operation result is stale",
      };
    }
    const translatedOperation = authoringOperationFromIntent(operation);
    const availability = authoringOperationAvailability(
      hooks.getAuthoringOperations?.() || hooks.authoringOperations || {},
      translatedOperation?.type,
    );
    if (!availability.supported) return { ok: false, error: availability.error };
    const requestToken = hooks.getCurrentRevision?.();
    let document;
    try {
      document = hooks.getCurrentDocument?.();
    } catch (error) {
      return { ok: false, error: error?.message || String(error) };
    }
    let result;
    try {
      result = await applyAuthoringOperationDocument(document, translatedOperation, {
        ...(hooks.getDraftGuard?.() || {}),
        catalogSnapshot: hooks.getCatalogSnapshot?.(),
      });
    } catch (error) {
      if (!isDraftGuardConflictError(error)) throw error;
      // Our own autosave raced this operation and bumped the draft store
      // revision. The submitted document is still the freshest authoring
      // state, so retry once without the optimistic store guard; save-time
      // concurrency control is unaffected.
      result = await applyAuthoringOperationDocument(document, translatedOperation, {
        catalogSnapshot: hooks.getCatalogSnapshot?.(),
      });
    }
    if (hooks.isRevisionCurrent && !hooks.isRevisionCurrent(requestToken)) {
      return {
        ok: false,
        error: hooks.getStaleError?.() || "Meta authoring operation result is stale",
      };
    }
    const projection = authoringProjectionFromOperationResult(result, hooks.getProjectionDefaults?.() || {});
    if (!projection) {
      return {
        ok: false,
        error: hooks.getMissingDocumentError?.() || "Meta authoring operation did not return a document",
      };
    }
    hooks.beginProjectionSync?.();
    hooks.applyProjection?.(projection);
    hooks.markDraft?.();
    return result;
  };
  return (operation) => {
    const enqueuedRevision = hooks.getCurrentRevision?.();
    const run = queue.catch(() => null).then(() => runOperation(operation, enqueuedRevision));
    queue = run.catch(() => null);
    return run;
  };
}

export async function graphProjectionDocument(document, options = {}) {
  const { signal, ...requestOptions } = options || {};
  return callRpc(rpcMethod("graphProjection"), { document, ...requestOptions }, { signal });
}

export async function graphToFlowDocument(document, options = {}) {
  const { signal, ...requestOptions } = options || {};
  return callRpc(rpcMethod("graphToFlow"), { document, ...requestOptions }, { signal });
}

export function importParamsFromDecodedFile(input = {}) {
  const {
    filename = "",
    mediaType = "",
    kind = "",
    text = "",
    parsedJson,
    contentBase64 = "",
  } = input;
  const sourceMeta = {
    source_name: String(filename || ""),
    source_media_type: String(mediaType || ""),
  };
  const filenameText = String(filename || "");
  const mediaTypeText = String(mediaType || "");
  const sourceKind = String(kind || inferDecodedFileKind(filenameText, mediaTypeText)).toLowerCase();
  if (sourceKind === "toml") {
    return { ...sourceMeta, mob_toml: String(text || "") };
  }
  if (sourceKind === "json") {
    const parsed = Object.prototype.hasOwnProperty.call(input, "parsedJson")
      ? parsedJson
      : parseDecodedJsonImport(text, filenameText);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? { ...parsed, ...sourceMeta }
      : { ...sourceMeta, document: parsed };
  }
  return { ...sourceMeta, content_base64: String(contentBase64 || "") };
}

export function inferDecodedFileKind(filename, mediaType) {
  const name = String(filename || "");
  const type = String(mediaType || "").toLowerCase();
  if (/\.toml$/i.test(name) || type.includes("toml")) return "toml";
  if (/\.json$/i.test(name) || type.includes("json")) return "json";
  return "binary";
}

export function parseDecodedJsonImport(text, filename = "") {
  try {
    return JSON.parse(String(text || ""));
  } catch (error) {
    const label = String(filename || "JSON import");
    throw new Error(`${label} is not valid JSON: ${error?.message || error}`);
  }
}
