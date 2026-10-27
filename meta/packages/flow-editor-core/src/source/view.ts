// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the source-view functions move byte-verbatim as plain JS, and
// their destructured `= {}` parameter defaults (inlineSourceToggleTransition,
// inlineSourceToggleButtonState) plus `options = {}` defaults raise TS2339
// under .ts semantics. Source-contract pins this exact text, so suppression
// must live at file level, not in the moved bodies. Resolution/linkage stays
// guarded behaviorally: the projection suite and export-keys test load the
// bundle and exercise these functions, so a missed import or re-export still
// fails the gate as a ReferenceError.
//
// Source-view plane for the Flow Editor controller: source file metadata
// validation and rows, the source API result to source-document projection,
// export download payloads, source drawer / inline source transitions,
// TOML highlighting, and the schema-backed source-view contract
// (sourceViewFromSchema/sourceViewForState). Moved verbatim from the
// controller.js source-view range, minus escapeHtml (re-homed to
// shared/normalize.ts in S1).
import { basicEditorViewState } from "../editors/basic-editor";
import { graphCanvasViewState } from "../editors/graph-editor";
import { escapeHtml } from "../shared/normalize";
import { diagnosticsToRows } from "../shell/outcomes";

export function sourceFileRequiresText(file) {
  const path = String(file?.path || "");
  const mediaType = String(file?.media_type || "");
  return /\.toml$/i.test(path)
    || /\.json$/i.test(path)
    || /^text\//i.test(mediaType)
    || mediaType === "application/json";
}

export function validateSourceFileMetadata(apiSource, file, index) {
  const prefix = `${apiSource} source_files[${index}]`;
  if (!String(file?.path || "").trim()) throw new Error(`${prefix} did not return path`);
  if (!String(file?.media_type || "").trim()) throw new Error(`${prefix} did not return media_type`);
  if (!String(file?.content_base64 || "").trim()) throw new Error(`${prefix} did not return content_base64`);
  if (!String(file?.sha256 || "").trim()) throw new Error(`${prefix} did not return sha256`);
  const size = Number(file?.size_bytes);
  if (!Number.isFinite(size) || size < 0) throw new Error(`${prefix} did not return size_bytes`);
  if (sourceFileRequiresText(file) && typeof file?.text !== "string") {
    throw new Error(`${prefix} did not return text`);
  }
}

export function sourceDocumentFromSourceResult(document, result, options = {}) {
  const apiSource = String(result?.source || "").trim();
  if (apiSource !== "meta/mobpacks/source") {
    throw new Error(`source preview expected meta/mobpacks/source but received ${apiSource}`);
  }
  const sourceView = sourceViewForState(null, options.sourceView);
  const primarySourcePath = sourceView.primarySourcePath;
  if (!primarySourcePath) throw new Error(`${apiSource} did not receive primary source path from Meta schema`);
  const files = Array.isArray(result?.source_files) ? result.source_files : [];
  if (!files.length) throw new Error(`${apiSource} did not return source_files`);
  const primarySourceFile = files.find((file) => String(file?.path || "") === primarySourcePath);
  if (!primarySourceFile) throw new Error(`${apiSource} did not return primary source file ${primarySourcePath}`);
  const exportedSource = String(primarySourceFile.text || "").trim();
  if (!exportedSource) throw new Error(`${apiSource} did not return primary source text ${primarySourcePath}`);
  const filename = String(result?.filename || "").trim();
  if (!filename) throw new Error(`${apiSource} did not return filename`);
  const mediaType = String(result?.media_type || "").trim();
  if (!mediaType) throw new Error(`${apiSource} did not return media_type`);
  const sourceDigest = String(primarySourceFile.sha256 || "").trim();
  if (!sourceDigest) throw new Error(`${apiSource} did not return primary source sha256 ${primarySourcePath}`);
  files.forEach((file, index) => validateSourceFileMetadata(apiSource, file, index));
  const authoringDocument = document && typeof document === "object" ? document : {};
  const validation = result?.validation || null;
  const stage = validation?.ok ? "valid" : "draft";
  return {
    document: authoringDocument,
    sourceDocument: {
      ...authoringDocument,
      validation,
      filename,
      media_type: mediaType,
      sourcePath: primarySourceFile.path,
      sourceFile: primarySourceFile,
      sourceFiles: files,
      sourceDigest,
      source: apiSource,
      sourceView,
    },
    validation,
    validationRows: diagnosticsToRows(validation),
    stage,
  };
}

export function exportDownloadPayload(result) {
  const contentBase64 = String(result?.content_base64 || "").trim();
  if (!contentBase64) throw new Error("meta/mobpacks/export did not return content_base64");
  const mediaType = String(result?.media_type || "").trim();
  if (!mediaType) throw new Error("meta/mobpacks/export did not return media_type");
  const filename = String(result?.filename || "").trim();
  if (!filename) throw new Error("meta/mobpacks/export did not return filename");
  return {
    contentBase64,
    mediaType,
    filename,
  };
}

export function sourceProjectionClearTransition() {
  return {
    sourceOpen: false,
    sourceDocument: null,
    inlineSourceOpen: false,
    inlineSourceSurface: null,
    inlineSourceDocument: null,
    inlineSourceBusy: false,
  };
}

export function sourceDrawerReadyTransition(sourceDocument) {
  return {
    sourceOpen: !!sourceDocument,
    sourceDocument: sourceDocument || null,
  };
}

export function inlineSourcePendingTransition(surface = "basic") {
  return {
    inlineSourceOpen: true,
    inlineSourceSurface: String(surface || "basic"),
    inlineSourceBusy: true,
  };
}

export function inlineSourceReadyTransition(sourceDocument) {
  return {
    inlineSourceDocument: sourceDocument || null,
    inlineSourceBusy: false,
  };
}

export function inlineSourceBusyTransition(busy) {
  return { inlineSourceBusy: !!busy };
}

export function inlineSourceToggleTransition({
  open = false,
  currentSurface = "",
  targetSurface = "basic",
} = {}) {
  const target = String(targetSurface || "basic");
  const active = !!open && String(currentSurface || "") === target;
  return active
    ? { shouldOpen: false, patch: sourceProjectionClearTransition() }
    : { shouldOpen: true, patch: inlineSourcePendingTransition(target) };
}

export function inlineSourceToggleButtonState({
  open = false,
  currentSurface = "",
  targetSurface = "basic",
  basicView = null,
  sourceView = null,
} = {}) {
  const target = String(targetSurface || "basic");
  const active = !!open && String(currentSurface || "") === target;
  const basic = basicEditorViewState(basicView);
  const source = sourceViewForState(null, sourceView);
  return {
    active,
    label: active ? (source.closeLabel || basic.sourceToggleLabel) : basic.sourceToggleLabel,
  };
}

export function inlineSourceRequestPath(request = null, options = {}) {
  const explicitPath = String(request?.sourcePath || request?.path || "").trim();
  if (explicitPath) return explicitPath;
  const graphView = graphCanvasViewState(options.graphView);
  const sourceView = sourceViewForState(null, options.sourceView);
  const requestedId = String(request?.id || "").trim();
  const requestedKind = String(request?.kind || "").trim();
  if (
    requestedId === graphView.sourceFileNodeId
    || requestedKind === graphView.sourceFileNodeKind
    || request?.isSourceFile
  ) {
    return sourceView.primarySourcePath || "meta/mob.toml";
  }
  return "";
}

export function sourceFileForPath(sourceDocument, path) {
  const files = Array.isArray(sourceDocument?.sourceFiles) ? sourceDocument.sourceFiles : [];
  const selectedPath = String(path || sourceDocument?.sourcePath || sourceViewForState(sourceDocument).primarySourcePath || "").trim();
  return files.find((file) => String(file?.path || "") === selectedPath)
    || sourceDocument?.sourceFile
    || files[0]
    || null;
}

export function sourceFileSelectionTransition(sourceDocument, path, currentPath = "") {
  const files = Array.isArray(sourceDocument?.sourceFiles) ? sourceDocument.sourceFiles : [];
  const requestedPath = String(path || "").trim();
  const requestedFile = files.find((file) => String(file?.path || "") === requestedPath) || null;
  if (requestedFile) return { sourcePath: String(requestedFile.path || "") };
  const currentFile = sourceFileForPath(sourceDocument, currentPath);
  return { sourcePath: String(currentFile?.path || "") };
}

export function sourceFileContent(file) {
  return typeof file?.text === "string" ? file.text : "";
}

export function sourceFileRows(sourceDocument, selectedPath) {
  const files = Array.isArray(sourceDocument?.sourceFiles) ? sourceDocument.sourceFiles : [];
  const activePath = String(selectedPath || sourceDocument?.sourcePath || "").trim();
  return files
    .filter((file) => String(file?.path || "").trim())
    .map((file) => {
      const path = String(file.path || "").trim();
      const size = Number(file.size_bytes || 0);
      const mediaType = String(file.media_type || "").trim();
      return {
        path,
        label: path,
        value: path,
        selected: path === activePath,
        className: `source-file-row${path === activePath ? " is-selected" : ""}`,
        meta: [mediaType, size > 0 ? `${size}b` : ""].filter(Boolean).join(" · "),
        file,
      };
    });
}

export function highlightSourceFile(file) {
  const source = sourceFileContent(file);
  const path = String(file?.path || "");
  const mediaType = String(file?.media_type || "");
  if (/\.toml$/i.test(path) || mediaType === "text/toml") return highlightTomlSource(source);
  return escapeHtml(source);
}

export function sourceEditorState(sourceDocument, options = {}) {
  const selectedFile = sourceFileForPath(sourceDocument, options.sourcePath);
  const source = selectedFile ? sourceFileContent(selectedFile) : "";
  const view = sourceViewForState(sourceDocument, options.sourceView);
  const sourcePath = String(selectedFile?.path || sourceDocument?.sourcePath || "").trim();
  const sourceLabel = [
    sourceDocument?.source || "",
    sourcePath,
    sourceDocument?.filename || "",
    sourceDocument?.media_type || "",
  ].filter(Boolean).join(" · ");
  const validationSource = sourceDocument?.validation?.validation_source || "";
  const bodyClass = options.compact ? "bld-toml__body" : "source-drawer__body";
  return {
    source,
    sourceHtml: selectedFile ? highlightSourceFile(selectedFile) : "",
    drawerEyebrow: view.drawerEyebrow,
    inlineTitle: view.inlineTitle,
    sourceLabel,
    validationSource,
    bodyClass,
    selectedPath: sourcePath,
    fileRows: sourceFileRows(sourceDocument, sourcePath),
    showLoading: !!options.busy && !source,
    loadingText: view.loadingText,
    copyLabel: view.copyLabel,
    closeLabel: view.closeLabel,
    copyDisabled: !!options.busy || !source,
  };
}

export function highlightTomlSource(source) {
  return escapeHtml(String(source || ""))
    .replace(/^(\s*#.*)$/gm, '<span class="toml-comment">$1</span>')
    .replace(/^(\s*)(\[[^\]]+\])/gm, '$1<span class="toml-table">$2</span>')
    .replace(/^(\s*)([A-Za-z_][\w-]*)(\s*=)/gm, '$1<span class="toml-key">$2</span>$3');
}

export function sourceViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_source_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    drawerEyebrow: String(view.drawer_eyebrow || "").trim(),
    inlineTitle: String(view.inline_title || "").trim(),
    primarySourcePath: String(view.primary_source_path || "").trim(),
    loadingText: String(view.loading_text || "").trim(),
    copyLabel: String(view.copy_label || "").trim(),
    closeLabel: String(view.close_label || "").trim(),
  };
  return out.drawerEyebrow && out.inlineTitle && out.primarySourcePath && out.loadingText && out.copyLabel && out.closeLabel
    ? out
    : null;
}

export function sourceViewForState(sourceDocument, sourceView) {
  const view = sourceView && typeof sourceView === "object"
    ? sourceView
    : sourceDocument?.sourceView;
  return {
    drawerEyebrow: String(view?.drawerEyebrow || ""),
    inlineTitle: String(view?.inlineTitle || ""),
    primarySourcePath: String(view?.primarySourcePath || ""),
    loadingText: String(view?.loadingText || ""),
    copyLabel: String(view?.copyLabel || ""),
    closeLabel: String(view?.closeLabel || ""),
  };
}
