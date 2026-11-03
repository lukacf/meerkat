// BuilderView — linear top→bottom mob flow builder. Emits a mob flow
// (the [flows.<id>.root] FrameSpec of a mobpack's mob.toml).
//
// Real meerkat model, not generic SaaS actions:
//   - A flow node is a MEMBER TURN: a profile/role runs with an instruction
//     `message`, gated by `depends_on`.
//   - Flow primitives: repeat-until (loop_id/body/until/max_iterations),
//     branch (condition + collection over downstream), parallel fan-out/
//     fan-in (dispatch_mode + collection_policy).
//   - "Input" is the mob's ingress: the task the run is started with
//     (`rkat mob run <pack> --prompt "task…"` / `run_flow(input)`), plus any typed
//     input fields. It is NOT an event source — schedulers/event sources live
//     outside the mobpack.
//
// Entry nodes of the flow are simply those with depends_on = [] (the steps
// right after Input).
// The vertical START → step → step layout mirrors how a FrameSpec reads
// top-to-bottom along its depends_on chain.

import { InlineSourceEditor } from "../overlays/overlays";
import { EchoInput, EchoTextArea } from "../shared/echo-text";

function CondValue({ field, value, onChange, conditionView = null }) {
  const control = window.MetaFlowController.conditionValueControl(field, value, conditionView);
  if (control.kind === "enum") {
    return (
      <select className="field__select bld-cond__val" value={control.value} onChange={e => onChange(e.target.value)}>
        {control.optionRows.map(row => <option key={row.value || "blank"} value={row.value}>{row.label}</option>)}
      </select>
    );
  }
  if (control.kind === "boolean") {
    return (
      <select className="field__select bld-cond__val" value={control.value} onChange={e => onChange(e.target.value)}>
        {control.optionRows.map(row => <option key={row.value || "blank"} value={row.value}>{row.label}</option>)}
      </select>
    );
  }
  return <EchoInput className="field__input bld-cond__val" placeholder={control.placeholder} value={control.value} onChangeText={onChange} />;
}

function InputEnumValueChip({ field, value, index, onChange }: any) {
  const [draftValue, setDraftValue] = React.useState(value || "");
  React.useEffect(() => {
    setDraftValue(value || "");
  }, [index, value]);
  return (
    <span className="chip">
      <input
        className="chip__input"
        value={draftValue}
        onChange={e => setDraftValue(e.target.value)}
        onBlur={e => {
          const patch = window.MetaFlowController.enumValueCommitPatch(field, index, e.target.value);
          setDraftValue(patch.enumValues?.[index] || "");
          onChange(patch);
        }}
      />
      <button className="chip__x" onClick={() => onChange(window.MetaFlowController.enumValueDeletePatch(field, index))}>×</button>
    </span>
  );
}

function InputParamField({ param, normalizeName, onRename, onChange, onDelete, contract, basicView = null }: any) {
  const fieldState = window.MetaFlowController.inputParamFieldControlState(param, contract, basicView);
  const values = fieldState.enumValues;
  const previousNameRef = React.useRef(null);
  const [draftName, setDraftName] = React.useState(param.name || "");
  React.useEffect(() => {
    setDraftName(param.name || "");
  }, [param.id, param.name]);
  const typeState = fieldState.typeState;
  return (
    <div className="schema-field">
      <input
        className="sb-input sb-col--name"
        value={draftName}
        onFocus={() => { previousNameRef.current = param.name || ""; }}
        onChange={e => setDraftName(e.target.value)}
        onBlur={e => {
          const previousName = previousNameRef.current ?? param.name;
          previousNameRef.current = null;
          const normalized = normalizeName(e.target.value);
          setDraftName(normalized);
          if (String(previousName || "").trim() !== normalized) onRename?.(normalized, previousName);
        }}
        placeholder={fieldState.namePlaceholder}
      />
      <select
        className="sb-select sb-col--type"
        value={typeState.type}
        onChange={e => {
          onChange(window.MetaFlowController.schemaLikeFieldTypePatch(param, e.target.value, contract));
        }}
      >
        {typeState.typeOptions.map(option => (
          <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
        ))}
      </select>
      {typeState.selectedType?.reason && <div className="hint__line" style={{ color: "var(--warn)" }}>{typeState.selectedType.reason}</div>}
      <label className="sb-col--req sb-checkbox">
        <input type="checkbox" checked={param.required !== false} onChange={e => onChange(window.MetaFlowController.schemaLikeFieldRequiredPatch(e.target.checked))} />
      </label>
      <input
        className="sb-input sb-col--desc"
        value={param.description || ""}
        onChange={e => onChange(window.MetaFlowController.schemaLikeFieldDescriptionPatch(e.target.value))}
        placeholder={fieldState.descriptionPlaceholder}
      />
      <button className="sb-del" onClick={onDelete} title={fieldState.removeTitle}>×</button>
      {param.type === "enum" && (
        <div className="sb-enum">
          <span className="sb-enum__label">{fieldState.enumLabel}</span>
          <div className="sb-enum__chips">
            {values.map((value, index) => (
              <InputEnumValueChip key={index} field={param} value={value} index={index} onChange={onChange} />
            ))}
            <button className="chip chip--add" onClick={() => onChange(window.MetaFlowController.enumValueAddPatch(param, fieldState.enumAddValue))}>{fieldState.enumAddLabel}</button>
          </div>
        </div>
      )}
    </div>
  );
}

function BranchConditionEditor({ index, branch, options, schemas, onChange, contract, basicView = null, conditionView = null }: any) {
  const conditionState = window.MetaFlowController.basicBranchConditionControlState({
    branch: { ...branch, index },
    options,
    schemas,
    contract,
    basicView,
  });
  return (
    <div className="bld-branch-card">
      <div className="bld-branch-card__head">{conditionState.rowTitle}</div>
      {!conditionState.hasConditionOptions ? (
        <div className="bld-hint" style={{ color: "var(--warn)" }}>{conditionState.emptyHint}</div>
      ) : (
        <>
          <div className="bld-cond">
            <select className="field__select" value={conditionState.cond.stepId || ""} onChange={e => onChange(window.MetaFlowController.basicConditionSourcePatch(options, e.target.value, { includeNamespace: true }))}>
              <option value="">{conditionState.sourcePlaceholder}</option>
              {conditionState.sourceOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
            <select className="field__select" value={conditionState.cond.field || ""} onChange={e => onChange(window.MetaFlowController.basicConditionFieldPatch(e.target.value, conditionState.fieldOptions))} disabled={!conditionState.fields.length}>
              <option value="">{conditionState.fieldPlaceholder}</option>
              {conditionState.fieldOptions.map(option => <option key={option.field.id || option.value} value={option.value}>{option.label}</option>)}
            </select>
            <select className="field__select bld-cond__op" value={conditionState.operatorValue} onChange={e => onChange(window.MetaFlowController.basicConditionOperatorPatch(e.target.value, contract))}>
              {conditionState.operatorOptions.map(option => (
                <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
              ))}
            </select>
            <CondValue field={conditionState.field} value={conditionState.cond.val} conditionView={conditionView} onChange={v => onChange(window.MetaFlowController.basicConditionValuePatch(v))} />
          </div>
          <div className="bld-cond__preview">{conditionState.previewPrefix} <code>{conditionState.previewLabel}</code></div>
        </>
      )}
    </div>
  );
}

export function BuilderView({ studio, mode = "build", flow: flowProp, setFlow: setFlowProp, sel: selProp, setSel: setSelProp, onShowSource, sourceOpen = false, sourceDocument = null, sourceBusy = false, sourceToggleLabel = "", onCloseSource, contract, toolCatalog = [], sourceView = null, basicView = null, launchView = null, conditionView = null, applyAuthoringIntent = null }) {
  const members = studio?.members || [];
  const [flowLocal, setFlowLocal] = React.useState(() => window.MetaFlowController.emptyAuthoringFlowState());
  const [selLocal, setSelLocal] = React.useState(null);
  const flow = flowProp || flowLocal;
  const setFlow = setFlowProp || setFlowLocal;
  const sel = selProp !== undefined ? selProp : selLocal;
  const setSel = setSelProp || setSelLocal;
  const [picker, setPicker] = React.useState({ open: false });
  const [view, setView] = React.useState({ scale: 1, tx: 0, ty: 0 });
  const [operationError, setOperationError] = React.useState("");
  const hostRef = React.useRef(null);
  const panRef = React.useRef(null);
  const isFlow = mode === "flow";
  const viewState = window.MetaFlowController.basicEditorViewState(basicView);
  const canvasView = Math.abs(view.ty) > 1200
    ? { ...view, ty: 0 }
    : view;

  const commitFlow = async (intentRequest) => {
    const fallback = viewState.authoringOperationFallbackError;
    if (!applyAuthoringIntent) {
      const result = { ok: false, error: viewState.authoringOperationUnavailableError };
      setOperationError(window.MetaFlowController.operationErrorText(result, fallback));
      return result;
    }
    try {
      const result = await applyAuthoringIntent(intentRequest);
      if (result?.ok === false) {
        setOperationError(window.MetaFlowController.operationErrorText(result, fallback));
      } else {
        setOperationError("");
      }
      return result;
    } catch (error) {
      const result = {
        ok: false,
        error: error?.message || String(error || fallback),
      };
      setOperationError(window.MetaFlowController.operationErrorText(result, fallback));
      return result;
    }
  };
  const update = (requestOrId, patch = {}) => {
    if (requestOrId && typeof requestOrId === "object") return commitFlow(requestOrId);
    return commitFlow({ intent: "basic.updateStep", stepId: requestOrId, patch });
  };
  const editStep = (id, action, payload = {}) => {
    return commitFlow({ intent: "basic.editStep", stepId: id, action, payload });
  };
  const selStep = findStep(flow.steps, sel);
  const applyBasicInteraction = (result) => {
    if (!result) return;
    if ("selection" in result) setSel(result.selection);
    if ("picker" in result) setPicker(result.picker);
  };

  const insertAt = (laneRef, pick) => {
    applyBasicInteraction(window.MetaFlowController.basicStepPickerCloseTransition());
    commitFlow({ intent: "basic.insertStep", pick, laneRef }).then((result) => {
      if (result?.ok === false) return;
      const id = result?.selection?.id;
      if (id) setSel(id);
    });
  };
  const removeStep = (id) => {
    const result = window.MetaFlowController.flowStepDeleteTransition(flow, id);
    commitFlow({ intent: "basic.deleteStep", stepId: id }).then((operationResult) => {
      if (operationResult?.ok === false) return;
      setSel(result.selection);
      setPicker(result.picker);
    });
  };
  const openPicker = (laneRef) => applyBasicInteraction(window.MetaFlowController.basicStepPickerOpenTransition(laneRef));

  // pan / zoom
  const onWheel = (e) => {
    // Overlays with their own scrolling (e.g. the mob.toml drawer) consume
    // the wheel — preventDefault here would pan the canvas behind them.
    if (e.target instanceof Element && e.target.closest("[data-own-scroll]")) return;
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      const fz = Math.exp(-e.deltaY * 0.0015);
      setView(v => {
        const r = hostRef.current.getBoundingClientRect();
        const cx = e.clientX - r.left, cy = e.clientY - r.top;
        const next = Math.max(0.4, Math.min(2, v.scale * fz));
        const k = next / v.scale;
        return { scale: next, tx: cx - (cx - v.tx) * k, ty: cy - (cy - v.ty) * k };
      });
    } else {
      e.preventDefault();
      setView(v => ({ ...v, tx: v.tx - e.deltaX, ty: v.ty - e.deltaY }));
    }
  };
  React.useEffect(() => {
    const el = hostRef.current; if (!el) return;
    const h = (e) => onWheel(e);
    el.addEventListener("wheel", h, { passive: false });
    return () => el.removeEventListener("wheel", h);
  });
  const onHostDown = (e) => {
    if (e.target === hostRef.current || e.target.classList?.contains("bld-canvas")) {
      panRef.current = { sx: e.clientX, sy: e.clientY, tx: view.tx, ty: view.ty };
      const move = (ev) => setView(v => ({ ...v, tx: panRef.current.tx + (ev.clientX - panRef.current.sx), ty: panRef.current.ty + (ev.clientY - panRef.current.sy) }));
      const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
      window.addEventListener("mousemove", move); window.addEventListener("mouseup", up);
      applyBasicInteraction(window.MetaFlowController.basicCanvasClearTransition());
    }
  };

  return (
    <div className={"builder" + (isFlow ? " builder--flow" : "")}>
      <div className="bld-stage" ref={hostRef} onMouseDown={onHostDown}>
        <div className="bld-canvas" style={{ transform: `translate(calc(-50% + ${canvasView.tx}px), ${canvasView.ty}px) scale(${canvasView.scale})` }}>
          <div className="bld-start">{viewState.startLabel}</div>
          <Lane studio={studio} mode={mode} steps={flow.steps} laneRef={{ lane: "main" }} sel={sel}
            contract={contract}
            basicView={basicView}
            setSel={(id) => applyBasicInteraction(window.MetaFlowController.basicStepSelectionTransition(id))}
            openPicker={openPicker} />
        </div>

        <button className="bld-toml-toggle" onMouseDown={e => e.stopPropagation()} onClick={() => onShowSource && onShowSource()}>
          {sourceToggleLabel || viewState.sourceToggleLabel}
        </button>
        <InlineSourceEditor
          open={sourceOpen}
          onClose={() => onCloseSource && onCloseSource()}
          state={sourceDocument}
          busy={sourceBusy}
          sourceView={sourceView}
        />

        <div className="zoom-controls" onMouseDown={e => e.stopPropagation()}>
          <button className="zoom-btn" onClick={() => setView(v => ({ ...v, scale: Math.max(0.4, v.scale / 1.2) }))}>−</button>
          <button className="zoom-btn zoom-btn--pct" onClick={() => setView({ scale: 1, tx: 0, ty: 0 })}>{Math.round(view.scale * 100)}%</button>
          <button className="zoom-btn" onClick={() => setView(v => ({ ...v, scale: Math.min(2, v.scale * 1.2) }))}>+</button>
        </div>
      </div>

      <aside className="bld-panel">
        {picker.open ? (
          <StepPicker
            members={members}
            isKickoff={picker.at?.lane === "main" && picker.at?.index === 0 && kickoffSlotEmpty(flow)}
            contract={contract}
            basicView={basicView}
            onPick={(pick) => insertAt(picker.at, pick)}
            onClose={() => applyBasicInteraction(window.MetaFlowController.basicStepPickerCloseTransition())}
          />
        ) : selStep ? (
          <>
            {operationError && <div className="hint__line" style={{ color: "var(--danger)", padding: "0 16px 8px" }}>{operationError}</div>}
            <StepInspector studio={studio} members={members} flow={flow} setFlow={setFlow} step={selStep} update={update} editStep={editStep} onDelete={() => removeStep(selStep.id)} contract={contract} toolCatalog={toolCatalog} basicView={basicView} launchView={launchView} conditionView={conditionView} />
          </>
        ) : (
          <EmptyPanel state={viewState} />
        )}
      </aside>
    </div>
  );
}

function Lane({ studio, mode, steps, laneRef, sel, setSel, openPicker, contract, basicView = null }) {
  const viewState = window.MetaFlowController.basicEditorViewState(basicView);
  return (
    <div className="bld-lane">
      {steps.map((step, i) => (
        <React.Fragment key={step.id}>
        <StepCard studio={studio} step={step} index={i} selected={sel === step.id} onSelect={() => setSel(step.id)} contract={contract} basicView={basicView} />
          {step.type === "branch" || step.type === "parallel" ? (
            <>
              <Fork studio={studio} mode={mode} step={step} sel={sel} setSel={setSel} openPicker={openPicker} contract={contract} basicView={basicView} />
              <InsertBtn mode={mode} mid={i < steps.length - 1} title={viewState.addStepTitle} onClick={() => openPicker({ ...laneRef, index: i + 1 })} />
            </>
          ) : step.type === "repeat" ? (
            <>
              <RepeatBody studio={studio} mode={mode} step={step} sel={sel} setSel={setSel} openPicker={openPicker} contract={contract} basicView={basicView} />
              <InsertBtn mode={mode} mid={i < steps.length - 1} title={viewState.addStepTitle} onClick={() => openPicker({ ...laneRef, index: i + 1 })} />
            </>
          ) : step.type === "adaptive" ? (
            <>
              <AdaptiveBody studio={studio} step={step} selected={sel === step.id} onSelect={() => setSel(step.id)} contract={contract} basicView={basicView} />
              <InsertBtn mode={mode} mid={i < steps.length - 1} title={viewState.addStepTitle} onClick={() => openPicker({ ...laneRef, index: i + 1 })} />
            </>
          ) : (
            <InsertBtn mode={mode} mid={i < steps.length - 1} title={viewState.addStepTitle} onClick={() => openPicker({ ...laneRef, index: i + 1 })} />
          )}
        </React.Fragment>
      ))}
      {steps.length === 0 && <InsertBtn mode={mode} title={viewState.addStepTitle} onClick={() => openPicker({ ...laneRef, index: 0 })} />}
    </div>
  );
}

// Adaptive layer — expanded inline block on the canvas. Everything rendered
// here (head line, node titles/kickers, connector texts, chips, loop-back
// copy) is projected by the controller plane from the server view contract;
// the block composes no copy locally.
function AdaptiveBody({ studio, step, selected, onSelect, contract, basicView = null }) {
  const adaptiveState = window.MetaFlowController.adaptiveStepState(studio, step, contract, basicView);
  const block = adaptiveState.block || {};
  const chips = Array.isArray(block.chips) ? block.chips : [];
  return (
    <div className="bld-adaptive" onMouseDown={(e) => { e.stopPropagation(); onSelect(); }}>
      <div className="bld-fork__bar" />
      <div className={"bld-aframe" + (selected ? " is-selected" : "")}>
        <div className="bld-aframe__rail"><span>{adaptiveState.panelIcon}</span></div>
        <div className="bld-aframe__body">
          <div className="bld-aframe__head">{block.headText}</div>
          <div className="bld-anode bld-anode--fm">
            <div className="bld-anode__kicker">{block.flowmasterKicker}</div>
            <div className="bld-anode__title">{block.flowmasterTitle}</div>
            <div className="bld-anode__desc">{block.flowmasterDesc}</div>
          </div>
          <div className="bld-aconn">{block.planConnector}</div>
          <div className="bld-anode bld-anode--layer">
            <div className="bld-anode__kicker">{block.layerKicker}</div>
            <div className="bld-afan">
              {chips.length === 0 && <span className="bld-afan__empty">{block.emptyProfilesLabel}</span>}
              {chips.map(chip => <span key={chip.id} className="bld-afan__chip">{chip.label}<em>{chip.suffix}</em></span>)}
            </div>
          </div>
          <div className="bld-aconn">{block.collectConnector}</div>
          <div className="bld-aframe__back">{block.loopBackText}</div>
        </div>
      </div>
    </div>
  );
}

function Fork({ studio, mode, step, sel, setSel, openPicker, contract, basicView = null }) {
  const forkState = window.MetaFlowController.basicForkCanvasState({ step, contract, basicView });
  const viewState = window.MetaFlowController.basicEditorViewState(basicView);
  return (
    <div className={forkState.className}>
      <div className="bld-fork__bar" />
      {forkState.showRail && <div className="bld-fork__rail" />}
      <div className="bld-fork__lanes">
        {forkState.lanes.map(l => (
          <div className="bld-fork__lane" key={l.id}>
            <div className="bld-fork__drop" />
            <div className="bld-fork__label">{l.label}</div>
            <div className="bld-fork__drop" />
            {l.steps.length === 0
              ? <InsertBtn mode={mode} title={viewState.addStepTitle} onClick={() => openPicker({ lane: "branch", parentId: step.id, branchId: l.id, index: 0 })} />
              : <Lane studio={studio} mode={mode} steps={l.steps} laneRef={{ lane: "branch", parentId: step.id, branchId: l.id }} sel={sel} setSel={setSel} openPicker={openPicker} contract={contract} basicView={basicView} />}
            {forkState.isParallel && <div className="bld-fork__drop" />}
          </div>
        ))}
      </div>
      {forkState.isParallel ? (
        <>
          {forkState.showRail && <div className="bld-fork__rail bld-fork__rail--join" />}
          <div className="bld-fork__bar" />
          <div className="bld-join">{forkState.joinLabel}</div>
        </>
      ) : (
        // Branch paths reconverge to a single downstream column so the
        // following main-lane step connects cleanly (no diagonal jump).
        forkState.showRail && <div className="bld-fork__rail bld-fork__rail--join" />
      )}
    </div>
  );
}

function RepeatBody({ studio, mode, step, sel, setSel, openPicker, contract, basicView = null }) {
  const repeatState = window.MetaFlowController.basicRepeatCanvasState({ step, members: studio?.members || [], contract, basicView });
  const viewState = window.MetaFlowController.basicEditorViewState(basicView);
  return (
    <div className="bld-repeat">
      <div className="bld-fork__bar" />
      <div className="bld-loop">
        <div className="bld-loop__rail">
          <span className="bld-loop__rail-glyph">↻</span>
        </div>
        <div className="bld-loop__frame">
          <div className="bld-loop__head">
            <span className="bld-loop__badge">{viewState.loopBadge}</span>
            <span className="bld-loop__meta">{repeatState.whileLabel} <strong>{repeatState.notLabel}</strong> ({repeatState.conditionLabel}) · {repeatState.maxIterationsLabel}</span>
          </div>
          {step.steps.length === 0
            ? <InsertBtn mode={mode} title={viewState.addStepTitle} onClick={() => openPicker({ lane: "branch", parentId: step.id, branchId: "body", index: 0 })} />
            : <Lane studio={studio} mode={mode} steps={step.steps} laneRef={{ lane: "branch", parentId: step.id, branchId: "body" }} sel={sel} setSel={setSel} openPicker={openPicker} contract={contract} basicView={basicView} />}
          <div className="bld-loop__back">{repeatState.loopBackLabel}</div>
        </div>
      </div>
      <div className="bld-loop__exit">{repeatState.exitLabel}</div>
    </div>
  );
}

function StepCard({ studio, step, index, selected, onSelect, contract, basicView = null }) {
  if (step.type === "adaptive") return null; // rendered as an expanded block by AdaptiveBody
  const cardState = window.MetaFlowController.basicStepCardState({ step, members: studio?.members || [], contract, basicView });

  return (
    <div
      className={"bld-card" + (selected ? " is-selected" : "") + (!cardState.configured ? " is-empty" : "") + (cardState.isFlowCard ? " bld-card--flow" : "")}
      onMouseDown={(e) => { e.stopPropagation(); onSelect(); }}
    >
      <div className="bld-card__head">
        <span className="bld-card__index">{index}.</span>
        {cardState.icon && <span className={"bld-card__icon tint--" + cardState.iconTint}>{cardState.icon}</span>}
        <span className="bld-card__title">{cardState.title}</span>
      </div>
      {cardState.configured ? (
        <div className="bld-card__body"><span className="bld-card__desc">{cardState.desc}</span></div>
      ) : (
        <div className="bld-card__skeleton"><span /><span /></div>
      )}
    </div>
  );
}

function InsertBtn({ onClick, mid, mode, title = "" }: any) {
  if (mode === "flow") {
    return (
      <div className={"bld-insert bld-insert--conn" + (mid ? " bld-insert--mid" : "")}>
        <div className="bld-insert__line" />
        <span className="bld-insert__dot" />
        {mid && <div className="bld-insert__line" />}
      </div>
    );
  }
  return (
    <div className={"bld-insert" + (mid ? " bld-insert--mid" : "")}>
      <div className="bld-insert__line" />
      <button className="bld-insert__btn" onMouseDown={(e) => { e.stopPropagation(); onClick(); }} title={title}>+</button>
      {mid && <div className="bld-insert__line" />}
    </div>
  );
}

// ── Picker ──
function StepPicker({ members, isKickoff, contract, onPick, onClose, basicView = null }) {
  const [q, setQ] = React.useState("");
  const pickerState = window.MetaFlowController.basicStepPickerState({ members, contract, query: q, isKickoff, basicView });
  if (pickerState.mode === "kickoff") {
    return (
      <div className="bld-panel__inner">
        <PanelHead title={pickerState.title} sub={pickerState.sub} onClose={onClose} />
        <div className="bld-hint">{pickerState.kickoffHint}</div>
      </div>
    );
  }
  return (
    <div className="bld-panel__inner">
      <PanelHead title={pickerState.title} sub={pickerState.sub} onClose={onClose} />
      <div className="bld-search">
        <span className="bld-search__icon">{pickerState.searchIcon}</span>
        <input className="bld-search__input" placeholder={pickerState.searchPlaceholder} value={q} onChange={e => setQ(e.target.value)} autoFocus />
      </div>

      <div className="bld-opts__group">{pickerState.membersLabel}</div>
      <div className="bld-opts">
        {pickerState.memberRows.map(row => (
          <button key={row.id} className="bld-opt" onClick={() => onPick(row.pick)}>
            <span className={"bld-opt__icon tint--" + row.iconTint}>{row.icon}</span>
            <span className="bld-opt__text">
              <span className="bld-opt__label">{row.name}</span>
              <span className="bld-opt__sub">{row.sub}</span>
            </span>
          </button>
        ))}
        {!pickerState.hasConfiguredMembers && <div className="bld-hint" style={{ padding: "4px 8px" }}>{pickerState.emptyMembersHint}</div>}
      </div>

      <div className="bld-opts__group">{pickerState.flowLabel}</div>
      <div className="bld-opts">
        {pickerState.primitiveRows.map(row => (
          <button
            key={row.id}
            className="bld-opt"
            disabled={row.disabled}
            title={row.disabledReason || undefined}
            onClick={() => !row.disabled && row.pick && onPick(row.pick)}
          >
            <span className={"bld-opt__icon tint--" + row.tint}>{row.glyph}</span>
            <span className="bld-opt__text">
              <span className="bld-opt__label">{row.label}{row.isNew && <span className="bld-opt__new">{pickerState.newBadgeLabel}</span>}</span>
              <span className="bld-opt__sub">{row.sub}</span>
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

// ── Inspector ──
function StepInspector({ studio, members, flow, setFlow, step, update, editStep, onDelete, contract, toolCatalog, basicView = null, launchView = null, conditionView = null }) {
  const [paramAddResult, setParamAddResult] = React.useState(null);
  React.useEffect(() => setParamAddResult(null), [step?.id]);
  const viewState = window.MetaFlowController.basicEditorViewState(basicView);
  if (step.type === "input") {
    const inputState = window.MetaFlowController.basicInputControlState(step, contract, basicView);
    const params = inputState.params;
    const paramAddErrorState = window.MetaFlowController.inputParamAddErrorState(paramAddResult, viewState.authoringOperationFallbackError);
    const applyInputIntent = (intentRequest) => {
      if (!update) return Promise.resolve({ ok: false, error: viewState.authoringOperationUnavailableError });
      return update(intentRequest);
    };
    const updateParam = (id, patch) => {
      return applyInputIntent({ intent: "basic.updateInputParam", stepId: step.id, paramId: id, patch });
    };
    const deleteParam = (id) => {
      return applyInputIntent({ intent: "basic.deleteInputParam", stepId: step.id, paramId: id });
    };
    const renameParam = (id, rawName, previousName) => {
      return applyInputIntent({ intent: "basic.renameInputParam", stepId: step.id, paramId: id, newName: rawName, previousName });
    };
    const addParam = () => {
      setParamAddResult(null);
      applyInputIntent({ intent: "basic.addInputParam", stepId: step.id }).then((result) => {
        if (result?.ok === false) {
          setParamAddResult(result);
          return;
        }
        setParamAddResult(null);
      }).catch((error) => {
        setParamAddResult({
          ok: false,
          error: error?.message || String(error || viewState.authoringOperationFallbackError),
        });
      });
    };
    return (
      <div className="bld-panel__inner">
        <PanelHead icon={inputState.panelIcon} iconTint="member" title={inputState.panelTitle} sub={inputState.panelSub} onClose={onDelete} deleteMode />
        <Field label={inputState.taskLabel}><EchoTextArea key={step.id} className="field__textarea" rows={3} placeholder={inputState.taskPlaceholder} value={step.task || ""} onChangeText={value => editStep(step.id, "set_task", { value })} /></Field>
        <div className="section">
          <div className="row row--between" style={{ marginBottom: 6 }}>
            <div className="section__title">{inputState.paramsTitle}</div>
            <button className="btn btn--ghost btn--sm" onClick={addParam}>{inputState.addParamLabel}</button>
          </div>
          {paramAddErrorState.hasError && <div className="hint__line">{paramAddErrorState.text}</div>}
          <div className="schema-builder">
            <div className="schema-builder__header">
              {inputState.headerRows.map(row => <span key={row.key} className={row.className}>{row.label}</span>)}
            </div>
            {params.map(param => (
              <InputParamField
                key={param.id}
                param={param}
                normalizeName={(raw) => window.MetaFlowController.uniqueInputParamName(params, raw, param.id)}
                onRename={(raw, previousName) => renameParam(param.id, raw, previousName)}
                onChange={(patch) => updateParam(param.id, patch)}
                onDelete={() => deleteParam(param.id)}
                contract={contract}
                basicView={basicView}
              />
            ))}
            {params.length === 0 && (
              <div className="schema-builder__empty">
                {inputState.emptyParamsParts.map(part => (
                  part.kind === "code"
                    ? <code key={part.key}>{part.text}</code>
                    : <React.Fragment key={part.key}>{part.text}</React.Fragment>
                ))}
              </div>
            )}
          </div>
        </div>
        <PanelTips title={viewState.tipsTitle} items={inputState.tips} />
      </div>
    );
  }

  if (step.type === "branch" || step.type === "parallel") {
    const branchState = window.MetaFlowController.basicBranchParallelControlState({
      step,
      flow,
      members: studio?.members || [],
      contract,
      basicView,
    });
    const setBranchCondition = (branch, patch) => {
      editStep(step.id, "set_branch_condition", { branch_id: branch.id, patch });
    };
    const addBranch = () => editStep(step.id, "add_branch");
    return (
      <div className="bld-panel__inner">
        <PanelHead icon={branchState.panelIcon} iconTint="member" title={branchState.panelTitle} sub={branchState.panelSub} onClose={onDelete} deleteMode />
        <Field label={branchState.controllerLabel}>
          <select className="field__select" value={branchState.controllerRole} onChange={e => editStep(step.id, "set_controller_role", { role: e.target.value })}>
            <option value="">{branchState.controllerPlaceholderLabel}</option>
            {branchState.memberOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
          </select>
        </Field>
        {!branchState.controllerRole && <div className="bld-hint" style={{ marginTop: 8 }}>{branchState.emptyControllerHint}</div>}
        {!branchState.isParallel && <>
          <div className="bld-section-label">{branchState.branchConditionTitle}</div>
          <div className="bld-hint">{branchState.branchConditionIntro}</div>
          {step.branches.map((b, i) => (
            <BranchConditionEditor
              key={b.id}
              index={i}
              branch={b}
              options={branchState.conditionOptions}
              schemas={studio?.schemas || []}
              onChange={(patch) => setBranchCondition(b, patch)}
              contract={contract}
              basicView={basicView}
              conditionView={conditionView}
            />
          ))}
          <button className="bld-add-row" onClick={addBranch}>{branchState.addBranchLabel}</button>
          <div className="bld-branch-card bld-branch-card--fallback"><div className="bld-branch-card__head">{branchState.fallbackTitle}</div><div className="bld-hint">{branchState.fallbackHint}</div></div>
        </>}
        {branchState.isParallel && <>
          <Field label={branchState.dispatchLabel}><select className="field__select" value={branchState.dispatchValue} onChange={e => editStep(step.id, "set_parallel_dispatch", { dispatch: e.target.value })}>
            {branchState.dispatchOptions.map(option => (
              <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
            ))}
          </select></Field>
          {branchState.selectedDispatch?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{branchState.selectedDispatch.reason}</div>}
          <Field label={branchState.collectionLabel}><select className="field__select" value={branchState.collectionValue} onChange={e => editStep(step.id, "set_collection", { collection: e.target.value })}>
            {branchState.collectionOptions.map(option => (
              <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
            ))}
          </select></Field>
          {branchState.selectedCollection?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{branchState.selectedCollection.reason}</div>}
          {branchState.showQuorum && (
            <Field label={branchState.quorumLabel}><input className="field__input" type="number" min="1" value={step.quorum ?? ""} placeholder={branchState.quorumPlaceholder} onChange={e => editStep(step.id, "set_quorum", { value: e.target.value })} /></Field>
          )}
          <button className="bld-add-row" onClick={addBranch}>{branchState.addBranchLabel}</button>
        </>}
        <Field label={branchState.dependencyLabel}><select className="field__select" value={branchState.dependencyValue} onChange={e => editStep(step.id, "set_dependency_mode", { value: e.target.value })}>
          {branchState.dependencyOptions.map(option => (
            <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
          ))}
        </select></Field>
        {branchState.selectedDependency?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{branchState.selectedDependency.reason}</div>}
      </div>
    );
  }

  if (step.type === "repeat") {
    const repeatState = window.MetaFlowController.basicRepeatControlState({
      step,
      members: studio?.members || [],
      schemas: studio?.schemas || [],
      contract,
      basicView,
    });
    const setCond = (patch) => editStep(step.id, "set_repeat_condition", window.MetaFlowController.flowStepRepeatConditionPatch(step, patch));
    return (
      <div className="bld-panel__inner">
        <PanelHead icon={repeatState.panelIcon} iconTint="member" title={repeatState.panelTitle} sub={repeatState.panelSub} onClose={onDelete} deleteMode />
        <Field label={repeatState.loopIdLabel}><input className="field__input field__input--mono" value={step.loopId || ""} placeholder={repeatState.loopIdPlaceholder} onChange={e => editStep(step.id, "set_loop_id", { value: e.target.value })} /></Field>

        <div className="bld-section-label" style={{ marginTop: 16 }}>{repeatState.conditionTitle}</div>
        <div className="bld-hint">{repeatState.conditionIntro}</div>
        {!repeatState.hasBodyMembers ? (
          <div className="bld-hint" style={{ marginTop: 10, color: "var(--warn)" }}>{repeatState.emptyBodyHint}</div>
        ) : (
          <div className="bld-cond">
            <select className="field__select" value={repeatState.cond.stepId || ""} onChange={e => setCond(window.MetaFlowController.basicConditionSourcePatch(repeatState.bodyMembers, e.target.value))}>
              <option value="">{repeatState.memberPlaceholderLabel}</option>
              {repeatState.bodyMemberOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
            <select className="field__select" value={repeatState.cond.field || ""} onChange={e => setCond(window.MetaFlowController.basicConditionFieldPatch(e.target.value, repeatState.fieldOptions))} disabled={!repeatState.condSchema}>
              <option value="">{repeatState.fieldPlaceholder}</option>
              {repeatState.fieldOptions.map(option => <option key={option.field.id || option.value} value={option.value}>{option.label}</option>)}
            </select>
            <select className="field__select bld-cond__op" value={repeatState.operatorValue} onChange={e => setCond(window.MetaFlowController.basicConditionOperatorPatch(e.target.value, contract))}>
              {repeatState.operatorOptions.map(option => (
                <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
              ))}
            </select>
            <CondValue field={repeatState.condField} value={repeatState.cond.val} conditionView={conditionView} onChange={v => setCond(window.MetaFlowController.basicConditionValuePatch(v))} />
          </div>
        )}
        <div className="bld-cond__preview">{repeatState.previewLabel} <code>{repeatState.repeatUntilExpression || repeatState.previewFallback}</code></div>

        <Field label={repeatState.iterationInputLabel}>
          <select className="field__select" value={repeatState.iterationInputValue} onChange={e => editStep(step.id, "set_iteration_input", { value: e.target.value })}>
            {repeatState.iterationInputOptions.map(option => (
              <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
            ))}
          </select>
        </Field>
        {repeatState.selectedIterationInput?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{repeatState.selectedIterationInput.reason}</div>}

        <Field label={repeatState.maxIterationsLabel}><input className="field__input" type="number" min="1" placeholder={repeatState.maxIterationsPlaceholder} value={step.maxIterations ?? ""} onChange={e => editStep(step.id, "set_max_iterations", { value: e.target.value })} /></Field>
        <PanelTips title={viewState.tipsTitle} items={repeatState.tips} />
      </div>
    );
  }

  if (step.type === "adaptive") {
    return <AdaptivePanel studio={studio} step={step} update={update} onDelete={onDelete} contract={contract} basicView={basicView} />;
  }

  // member step
  const memberStepState = window.MetaFlowController.basicMemberStepControlState({
    step,
    flow,
    members,
    contract,
    basicView,
    launchView,
  });
  const m = memberStepState.member;
  const launchState = memberStepState.launchState;
  return (
    <div className="bld-panel__inner">
      <PanelHead icon="◆" iconTint="accent" title={memberStepState.panelTitle} sub={memberStepState.panelSub} onClose={onDelete} deleteMode />
      <Field label={memberStepState.memberFieldLabel}><select className="field__select" value={step.role || ""} onChange={e => editStep(step.id, "set_member_role", { role: e.target.value })}>
        <option value="">{memberStepState.memberPlaceholderLabel}</option>
        {memberStepState.memberOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
      </select></Field>
      <Field label={launchState.launchTitle}>
        <select className="field__select" value={launchState.launchKind} onChange={e => {
          editStep(step.id, "set_launch_kind", { kind: e.target.value, first_fork_source_id: memberStepState.firstLaunchSourceId });
        }}>
          {launchState.launchOptions.map(option => (
            <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
          ))}
        </select>
      </Field>
      {launchState.selectedLaunchMode?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{launchState.selectedLaunchMode.reason}</div>}
      {launchState.launchKind === "Resume" && (
        <Field label={launchState.resumeSessionLabel}>
          <input className="field__input" value={launchState.launchMode.sessionId || ""} placeholder={launchState.resumeSessionPlaceholder} onChange={e => editStep(step.id, "set_launch_session", { session_id: e.target.value })} />
        </Field>
      )}
      {launchState.launchKind === "Fork" && (
        <>
          <Field label={launchState.forkSourceLabel}>
            <select className="field__select" value={launchState.launchMode.from || ""} onChange={e => editStep(step.id, "set_launch_fork_source", { from: e.target.value })}>
              {memberStepState.launchSourceOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
          </Field>
          <Field label={launchState.forkContextLabel}>
            <select className="field__select" value={launchState.forkContextValue} onChange={e => editStep(step.id, "set_launch_fork_context", { context: e.target.value })}>
              {launchState.forkContextOptions.map(option => (
                <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
              ))}
            </select>
          </Field>
          {launchState.selectedForkContext?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{launchState.selectedForkContext.reason}</div>}
        </>
      )}
      <Field label={memberStepState.instructionLabel}><EchoTextArea key={step.id} className="field__textarea" rows={4} placeholder={memberStepState.instructionPlaceholder} value={step.instruction || ""} onChangeText={value => editStep(step.id, "set_instruction", { value })} /></Field>
      <Field label={memberStepState.dispatchLabel}>
        <select className="field__select" value={memberStepState.dispatchValue} onChange={e => editStep(step.id, "set_dispatch_mode", { dispatch: e.target.value })}>
          {memberStepState.dispatchOptions.map(option => (
            <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
          ))}
        </select>
      </Field>
      {memberStepState.selectedDispatch?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{memberStepState.selectedDispatch.reason}</div>}
      <Field label={memberStepState.collectionLabel}>
        <select className="field__select" value={memberStepState.collectionValue} onChange={e => editStep(step.id, "set_collection", { collection: e.target.value })}>
          {memberStepState.collectionOptions.map(option => (
            <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
          ))}
        </select>
      </Field>
      {memberStepState.selectedCollection?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{memberStepState.selectedCollection.reason}</div>}
      {memberStepState.showQuorum && (
        <Field label={memberStepState.quorumLabel}>
          <input className="field__input" type="number" min="1" step="1" value={step.quorum ?? ""} placeholder={memberStepState.quorumPlaceholder} onChange={e => editStep(step.id, "set_quorum", { value: e.target.value })} />
        </Field>
      )}
      <Field label={memberStepState.timeoutLabel}>
        <input className="field__input" type="number" min="1" step="1" placeholder={memberStepState.timeoutPlaceholder} value={step.timeoutMs ?? ""} onChange={e => editStep(step.id, "set_timeout_ms", { value: e.target.value })} />
      </Field>
      <Field label={memberStepState.outputFormatLabel}>
        <select className="field__select" value={memberStepState.outputValue} onChange={e => editStep(step.id, "set_output_format", { value: e.target.value })}>
          {memberStepState.outputOptions.map(option => (
            <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
          ))}
        </select>
      </Field>
      {memberStepState.selectedOutput?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{memberStepState.selectedOutput.reason}</div>}
      <ToolScopeEditor
        label={memberStepState.allowedToolsLabel}
        emptyLabel={memberStepState.allowedToolsEmptyLabel}
        member={m}
        selected={step.allowedTools || []}
        onChange={tools => editStep(step.id, "set_allowed_tools", { tools })}
        mode="member"
        toolCatalog={toolCatalog}
        basicView={basicView}
      />
      <ToolScopeEditor
        label={memberStepState.blockedToolsLabel}
        emptyLabel={memberStepState.blockedToolsEmptyLabel}
        member={m}
        selected={step.blockedTools || []}
        onChange={tools => editStep(step.id, "set_blocked_tools", { tools })}
        mode="catalog"
        toolCatalog={toolCatalog}
        basicView={basicView}
      />
      {memberStepState.schemaHint && (
        <div className="bld-hint" style={{ marginTop: 10 }}>
          {memberStepState.schemaHint.parts.map(part => (
            part.kind === "code"
              ? <code key={part.key}>{part.text}</code>
              : <React.Fragment key={part.key}>{part.text}</React.Fragment>
          ))}
        </div>
      )}
      <Field label={memberStepState.dependencyLabel}><select className="field__select" value={memberStepState.dependencyValue} onChange={e => editStep(step.id, "set_dependency_mode", { value: e.target.value })}>
        {memberStepState.dependencyOptions.map(option => (
          <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
        ))}
      </select></Field>
      {memberStepState.selectedDependency?.reason && <div className="bld-hint" style={{ color: "var(--warn)" }}>{memberStepState.selectedDependency.reason}</div>}
    </div>
  );
}

function ToolScopeEditor({ label, emptyLabel, member, selected, onChange, mode = "member", toolCatalog = [], basicView = null }) {
  const field = mode === "catalog" ? "blockedTools" : "allowedTools";
  const scope = window.MetaFlowController.stepToolScopeState({ member, selected, mode, toolCatalog, basicView });
  const remove = (id) => {
    const result = window.MetaFlowController.stepToolScopeRemovePatch(selected, id, { field });
    if (result.patch) onChange(result.patch[field] || []);
  };
  const add = (id) => {
    const result = window.MetaFlowController.stepToolScopeAddPatch(selected, id, { member, mode, toolCatalog, field, basicView });
    if (result.patch) onChange(result.patch[field] || []);
  };
  return (
    <Field label={label}>
      {scope.selectedTools.length === 0 ? (
        <div className="bld-hint">{emptyLabel}</div>
      ) : (
        scope.rows.map(row => {
          return (
            <div key={row.id} className={row.className}>
              <div>
                <div className="name">{row.name}</div>
                <div className="auth">{row.description}</div>
              </div>
              <button onClick={() => remove(row.id)}>{row.removeLabel}</button>
            </div>
          );
        })
      )}
      <select className="field__select" value={scope.addSelectValue} disabled={scope.disabled} onChange={e => { add(e.target.value); e.target.value = ""; }}>
        <option value={scope.addSelectValue}>{scope.addSelectPlaceholder}</option>
        {scope.addableRows.map(row => (
          <option key={row.id} value={row.value}>{row.optionLabel}</option>
        ))}
      </select>
    </Field>
  );
}

// Adaptive layer panel — configures the single adaptive step. All labels,
// placeholders, option rows, template toggle rows, surface toggles, limit
// rows, and tips are projected by adaptiveStepState from the server view
// contract; the panel composes no copy and no defaults locally. Free-text
// fields (prompt, objectiveClass) go through EchoTextArea keyed by step.id so
// per-keystroke server echoes never move the caret, and their values are
// committed verbatim (never trimmed). Profile templates are toggles over
// EXISTING members — member CRUD lives in the AGENTS tab (the hint below the
// toggle list points there).
function AdaptivePanel({ studio, step, update, onDelete, contract, basicView = null }) {
  const viewState = window.MetaFlowController.basicEditorViewState(basicView);
  const adaptiveState = window.MetaFlowController.adaptiveStepState(studio, step, contract, basicView);
  const [advancedLimitsOpen, setAdvancedLimitsOpen] = React.useState(false);
  React.useEffect(() => setAdvancedLimitsOpen(false), [step.id]);
  const flowmasterOptions = Array.isArray(adaptiveState.flowmasterOptions) ? adaptiveState.flowmasterOptions : [];
  const schemaOptions = Array.isArray(adaptiveState.schemaOptions) ? adaptiveState.schemaOptions : [];
  const templateRows = Array.isArray(adaptiveState.templateRows) ? adaptiveState.templateRows : [];
  const surfaceToggles = Array.isArray(adaptiveState.surfaceToggles) ? adaptiveState.surfaceToggles : [];
  const primaryLimitRows = Array.isArray(adaptiveState.primaryLimitRows) ? adaptiveState.primaryLimitRows : [];
  const advancedLimitRows = Array.isArray(adaptiveState.advancedLimitRows) ? adaptiveState.advancedLimitRows : [];
  const toggleTemplate = (row) => {
    const next = templateRows.filter(r => (r.id === row.id ? !row.on : r.on)).map(r => r.id);
    update(step.id, { profileTemplateIds: next });
  };
  const toggleSurface = (toggle) => {
    const next = surfaceToggles.filter(t => (t.id === toggle.id ? !toggle.on : t.on)).map(t => t.id);
    update(step.id, { targetSurfaces: next });
  };
  const commitLimit = (row, raw) => {
    // The limits patch arm takes positive integers and merges partially over
    // current values server-side; transient non-numeric drafts stay local to
    // the EchoInput and are never committed.
    const value = Number(raw);
    if (!Number.isInteger(value) || value <= 0) return;
    update(step.id, { limits: { [row.key]: value } });
  };
  const renderLimitRow = (row) => (
    <label key={row.key} className="ap-num">
      <span className="ap-num__label">{row.label}</span>
      <EchoInput key={step.id + ":" + row.key} className="field__input" type="number" min="1" step="1" value={row.value ?? ""} onChangeText={value => commitLimit(row, value)} />
    </label>
  );
  return (
    <div className="bld-panel__inner">
      <PanelHead icon={adaptiveState.panelIcon} iconTint="member" title={adaptiveState.panelTitle} sub={adaptiveState.panelSub} onClose={onDelete} deleteMode />
      <Field label={adaptiveState.flowmasterLabel}>
        <select className="field__select" value={adaptiveState.flowmasterId || ""} onChange={e => update(step.id, { flowmasterId: e.target.value })}>
          <option value="">{adaptiveState.flowmasterPlaceholderLabel}</option>
          {flowmasterOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
        </select>
      </Field>
      <Field label={adaptiveState.promptLabel}>
        <EchoTextArea key={step.id} className="field__textarea" rows={4} placeholder={adaptiveState.promptPlaceholder} value={adaptiveState.promptValue || ""} onChangeText={value => update(step.id, { prompt: value })} />
      </Field>
      <Field label={adaptiveState.objectiveLabel}>
        <EchoTextArea key={step.id} className="field__textarea" rows={2} placeholder={adaptiveState.objectivePlaceholder} value={adaptiveState.objectiveValue || ""} onChangeText={value => update(step.id, { objectiveClass: value })} />
      </Field>
      <Field label={adaptiveState.resultSchemaLabel}>
        <select className="field__select" value={adaptiveState.resultSchemaValue || ""} onChange={e => update(step.id, { resultSchema: e.target.value })}>
          <option value="">{adaptiveState.resultSchemaPlaceholderLabel}</option>
          {schemaOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
        </select>
      </Field>
      <div className="bld-section-label">{adaptiveState.profilesTitle}</div>
      <div className="bld-hint">{adaptiveState.profilesHint}</div>
      <div className="ap-profiles">
        {templateRows.map(row => (
          <button key={row.id} className={"ap-profile" + (row.on ? " is-on" : "")} onClick={() => toggleTemplate(row)}>
            <span className="ap-profile__name">{row.label}</span>
            <span className="ap-profile__chiprow">
              {row.role && <span className="ap-tool">{row.role}</span>}
              {row.model && <span className="ap-tool">{row.model}</span>}
            </span>
          </button>
        ))}
        {templateRows.length === 0 && <div className="bld-hint">{adaptiveState.profilesEmptyHint}</div>}
      </div>
      <div className="bld-hint">{adaptiveState.profilesAgentsHint}</div>
      <Field label={adaptiveState.surfacesLabel}>
        <div className="ap-profile__toolrow">
          {surfaceToggles.map(toggle => (
            <button key={toggle.id} className={"ap-tool" + (toggle.on ? " is-on" : "")} onClick={() => toggleSurface(toggle)}>{toggle.label}</button>
          ))}
        </div>
      </Field>
      <label className="ap-inline">
        <input type="checkbox" checked={!!adaptiveState.allowInlineProfiles} onChange={e => update(step.id, { allowInlineProfiles: e.target.checked })} />
        <span>{adaptiveState.inlineProfilesLabel}</span>
      </label>
      <div className="bld-section-label">{adaptiveState.limitsTitle}</div>
      <div className="ap-grid">
        {primaryLimitRows.map(renderLimitRow)}
      </div>
      <button className={"ap-advanced" + (advancedLimitsOpen ? " is-open" : "")} onClick={() => setAdvancedLimitsOpen(open => !open)}>{adaptiveState.limitsAdvancedLabel}</button>
      {advancedLimitsOpen && (
        <div className="ap-grid">
          {advancedLimitRows.map(renderLimitRow)}
        </div>
      )}
      <PanelTips title={viewState.tipsTitle} items={Array.isArray(adaptiveState.tips) ? adaptiveState.tips : []} />
    </div>
  );
}

function PanelHead({ icon, iconTint, title, sub, onClose, deleteMode }: any) {
  return (
    <div className="bld-panel__head">
      <div className="bld-panel__head-main">
        {icon && <span className={"bld-panel__icon tint--" + (iconTint || "muted")}>{icon}</span>}
        <div><div className="bld-panel__title">{title}</div>{sub && <div className="bld-panel__sub">{sub}</div>}</div>
      </div>
      <button className="bld-panel__close" onClick={onClose} title={deleteMode ? "Delete step" : "Close"}>{deleteMode ? "🗑" : "✕"}</button>
    </div>
  );
}
function Field({ label, children }: any) {
  return <div className="field" style={{ marginTop: 14 }}><label className="field__label">{label}</label>{children}</div>;
}
function PanelTips({ title, items }) {
  return <div className="bld-tips"><div className="bld-tips__head">{title}</div><ul>{items.map((t, i) => <li key={i}>{t}</li>)}</ul></div>;
}
function EmptyPanel({ state }) {
  return (
    <div className="bld-panel__inner bld-panel__empty">
      <div className="bld-panel__title">{state.emptyPanelTitle}</div>
      <div className="bld-panel__sub">
        {state.emptyPanelSubtitleParts.map(part => {
          if (part.kind === "code") return <code key={part.key}>{part.text}</code>;
          if (part.kind === "strong") return <strong key={part.key}>{part.text}</strong>;
          return <React.Fragment key={part.key}>{part.text}</React.Fragment>;
        })}
      </div>
    </div>
  );
}

// ── step model ──
function kickoffSlotEmpty(flow) {
  // The first node is always the (fixed) Input ingress; clicking it shows the
  // Input explainer rather than the step picker.
  const first = flow.steps[0];
  return !!first && first.type === "input";
}
function childLanes(s) {
  if (s.type === "branch") return [...s.branches, { id: "fallback", steps: s.fallback }];
  if (s.type === "parallel") return s.branches;
  if (s.type === "repeat") return [{ id: "body", steps: s.steps, _direct: true }];
  return [];
}
function findStep(steps, id) {
  for (const s of steps) {
    if (s.id === id) return s;
    for (const l of childLanes(s)) { const r = findStep(l.steps, id); if (r) return r; }
  }
  return null;
}
