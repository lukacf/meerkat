// Deploy plan trace, validate sheet, source drawer.

export function DeployPlanTrace({ open, onClose, onActiveStep, runKey, document, plan, deployView = null }) {
  const traceState = React.useMemo(() =>
    window.MetaFlowController.deployPlanTraceState(document, plan, { deployView }),
    [document, plan, deployView]);
  const [idx, setIdx] = React.useState(0);
  const bodyRef = React.useRef(null);

  React.useEffect(() => {
    if (!open) return;
    setIdx(0);
  }, [open, runKey]);

  React.useEffect(() => {
    if (!open) { onActiveStep(null); return; }
    onActiveStep(traceState.steps[idx]?.node || null);
    if (bodyRef.current) {
      const el = bodyRef.current.querySelector(`[data-step="${idx}"]`);
      if (el) el.scrollIntoView({ block: "nearest", behavior: "smooth" });
    }
  }, [idx, open, traceState.steps]);

  if (!open) return null;

  return (
    <div className="deploy-plan">
      <div className="deploy-plan__head">
        <div>
          <div className="deploy-plan__title"><span className="accent">{traceState.eyebrow}</span> · {traceState.title}</div>
          <div className="deploy-plan__sub">{traceState.subtitle}</div>
        </div>
        <div className="row">
          <button className="btn btn--sm" onClick={() => setIdx(0)}>{traceState.firstLabel}</button>
          <button className="btn btn--ghost btn--sm" onClick={onClose}>{traceState.closeLabel}</button>
        </div>
      </div>
      <div className="deploy-plan__body" ref={bodyRef}>
        {traceState.steps.map((s, i) => (
          <div
            key={i}
            data-step={i}
            className={"deploy-plan__step" + (i === idx ? " is-current" : "") + (i > idx ? " is-pending" : "")}
          >
            <div className="g" />
            <div>
              <div className="head">{s.head}</div>
              <div className="body">{s.body}</div>
            </div>
          </div>
        ))}
      </div>
      <div className="deploy-plan__foot">
        <div className="row row--between" style={{ width: "100%" }}>
          <span className="muted">{traceState.packLabel ? `${traceState.packLabel} · ` : ""}{traceState.stepLabel} {idx + 1} / {traceState.steps.length}</span>
          <div className="row">
            <button className="btn btn--sm" onClick={() => setIdx(i => Math.max(0, i - 1))}>{traceState.previousLabel}</button>
            <button className="btn btn--sm" onClick={() => setIdx(i => Math.min(traceState.steps.length - 1, i + 1))}>{traceState.nextLabel}</button>
          </div>
        </div>
      </div>
    </div>
  );
}

export function ValidateSheet({ open, onClose, onPublish, onDeployPlan, onDeployRun, results, stage, deployView = null, capabilities = null }) {
  if (!open) return null;
  const sheetState = window.MetaFlowController.validationSheetState(results, { stage, deployView, capabilities });
  return (
    <div className="validate">
      <div className="validate__head">
        <div>
          <div className="inspector__eyebrow">{sheetState.eyebrow}</div>
          <div className="inspector__title">{sheetState.title}</div>
        </div>
        <div className="row">
          <button className="btn btn--primary btn--sm" onClick={onPublish} disabled={sheetState.publishDisabled}>{sheetState.publishLabel}</button>
          <button className="btn btn--ghost btn--sm" onClick={onDeployPlan} disabled={sheetState.deployPlanDisabled}>{sheetState.deployPlanLabel}</button>
          <button className="btn btn--primary btn--sm" onClick={onDeployRun} disabled={sheetState.deployRunDisabled}>{sheetState.deployLabel}</button>
          <button className="btn btn--ghost btn--sm" onClick={onClose}>{sheetState.closeLabel}</button>
        </div>
      </div>
      <div className="validate__body">
        {sheetState.rows.map((r, i) => (
          <div key={i} className={"validate__row is-" + r.kind}>
            <span className="glyph">{r.glyph}</span>
            <div>
              <div className="head">{r.head}</div>
              <div className="sub">{r.sub}</div>
            </div>
            <span className="meta">{r.meta}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function SourceCodePanel({ state, busy = false, compact = false, sourceView = null, sourcePath = "" }) {
  const editorState = window.MetaFlowController.sourceEditorState(state, { busy, compact, sourceView, sourcePath });
  if (editorState.showLoading) {
    return <pre className={editorState.bodyClass} role="textbox" aria-readonly="true">{editorState.loadingText}</pre>;
  }
  return (
    <pre
      className={editorState.bodyClass}
      role="textbox"
      aria-readonly="true"
      dangerouslySetInnerHTML={{ __html: editorState.sourceHtml }}
    />
  );
}

export function SourceDrawer({ open, onClose, state, sourceView = null }) {
  const [sourcePath, setSourcePath] = React.useState("");
  const selectSourcePath = (path) => {
    const result = window.MetaFlowController.sourceFileSelectionTransition(state, path, sourcePath);
    setSourcePath(result.sourcePath);
  };
  React.useEffect(() => {
    setSourcePath("");
  }, [state]);
  if (!open) return null;
  const editorState = window.MetaFlowController.sourceEditorState(state, { sourceView, sourcePath });
  return (
    <div className="source-drawer" data-own-scroll="">
      <div className="source-drawer__head">
        <div>
          <div className="inspector__eyebrow">{editorState.drawerEyebrow}</div>
          <div className="inspector__id">{editorState.sourceLabel}</div>
          {editorState.validationSource && <div className="inspector__id">{editorState.validationSource}</div>}
        </div>
        <div className="row">
          <button className="btn btn--sm" onClick={() => navigator.clipboard?.writeText(editorState.source)} disabled={editorState.copyDisabled}>{editorState.copyLabel}</button>
          <button className="btn btn--ghost btn--sm" onClick={onClose}>{editorState.closeLabel}</button>
        </div>
      </div>
      {editorState.fileRows.length > 1 && (
        <div className="source-file-list">
          {editorState.fileRows.map(row => (
            <button key={row.path} className={row.className} onClick={() => selectSourcePath(row.path)}>
              <span>{row.label}</span>
              <em>{row.meta}</em>
            </button>
          ))}
        </div>
      )}
      <SourceCodePanel state={state} sourceView={sourceView} sourcePath={sourcePath} />
    </div>
  );
}

export function InlineSourceEditor({ open, onClose, state, busy = false, surface = "basic", sourceView = null }) {
  const [sourcePath, setSourcePath] = React.useState("");
  const selectSourcePath = (path) => {
    const result = window.MetaFlowController.sourceFileSelectionTransition(state, path, sourcePath);
    setSourcePath(result.sourcePath);
  };
  React.useEffect(() => {
    setSourcePath("");
  }, [state]);
  if (!open) return null;
  const editorState = window.MetaFlowController.sourceEditorState(state, { busy, compact: true, sourceView, sourcePath });
  return (
    <div className={"bld-toml bld-toml--" + surface} data-own-scroll="" onMouseDown={e => e.stopPropagation()}>
      <div className="bld-toml__head">
        <div>
          <div>{editorState.inlineTitle}</div>
          <div className="bld-toml__hint">{editorState.sourceLabel}</div>
          {editorState.validationSource && <div className="bld-toml__hint">{editorState.validationSource}</div>}
        </div>
        <div className="row">
          <button className="btn btn--sm" onClick={() => navigator.clipboard?.writeText(editorState.source)} disabled={editorState.copyDisabled}>{editorState.copyLabel}</button>
          <button className="btn btn--ghost btn--sm" onClick={onClose}>{editorState.closeLabel}</button>
        </div>
      </div>
      {editorState.fileRows.length > 1 && (
        <div className="source-file-list source-file-list--inline">
          {editorState.fileRows.map(row => (
            <button key={row.path} className={row.className} onClick={() => selectSourcePath(row.path)}>
              <span>{row.label}</span>
              <em>{row.meta}</em>
            </button>
          ))}
        </div>
      )}
      <SourceCodePanel state={state} busy={busy} compact sourceView={sourceView} sourcePath={sourcePath} />
    </div>
  );
}
