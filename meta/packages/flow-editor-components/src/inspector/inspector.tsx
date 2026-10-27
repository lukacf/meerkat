// Inspector — context-sensitive panel.
//
// Selection modes:
//   { kind: null }              → TemplateInspector
//   { kind: "instance", id }    → InstanceInspector  (template-only fields + member summary)
//   { kind: "edge",     id }    → EdgeInspector
//
// AddNodeMenu lets the user place existing real members. Control-flow rows are
// projected from the Basic Editor's deployable flow model.

import { EchoInput } from "../shared/echo-text";

export function Inspector({ studio, selection, selectMember, selectInstance, clearSelection, editGraphNode = null, editGraphEdge = null, deleteGraphNode = null, deleteGraphEdge = null, template, templateSeed, templateView, launchView = null, graphView = null, conditionView = null, flow, contract }) {
  const selectionState = window.MetaFlowController.graphSelectionState({
    selection,
    instances: studio.instances,
    edges: studio.edges,
  });
  if (selectionState.kind === "instance") {
    if (!selectionState.instance) return <TemplateInspector studio={studio} template={template} templateSeed={templateSeed} templateView={templateView} />;
    return <InstanceInspector studio={studio} flow={flow} inst={selectionState.instance} selectMember={selectMember} clearSelection={clearSelection} editGraphNode={editGraphNode} editGraphEdge={editGraphEdge} deleteGraphNode={deleteGraphNode} contract={contract} launchView={launchView} graphView={graphView} conditionView={conditionView} />;
  }
  if (selectionState.kind === "edge") {
    if (!selectionState.edge) return <TemplateInspector studio={studio} template={template} templateSeed={templateSeed} templateView={templateView} />;
    return <EdgeInspector studio={studio} flow={flow} edge={selectionState.edge} clearSelection={clearSelection} editGraphEdge={editGraphEdge} deleteGraphEdge={deleteGraphEdge} contract={contract} graphView={graphView} conditionView={conditionView} />;
  }
  return <TemplateInspector studio={studio} template={template} templateSeed={templateSeed} templateView={templateView} />;
}

function clearSelectionAfterOperation(result, clearSelection) {
  if (!result) return;
  Promise.resolve(result).then((operationResult) => {
    if (operationResult?.ok === false) return;
    clearSelection(operationResult?.selection);
  }).catch(() => {});
}

// ── Template (no selection) ──────────────────────────────────────
function TemplateInspector({ studio, template, templateSeed, templateView }) {
  const templateState = window.MetaFlowController.graphTemplateInspectorState({ studio, template, templateSeed, templateView });
  return (
    <>
      <div className="inspector__head">
        <div className="inspector__eyebrow">{templateState.templateEyebrow}</div>
        <div className="inspector__title">{templateState.name}</div>
        <div className="inspector__id">{templateState.repo} · {templateState.version}</div>
      </div>
      <div className="inspector__body">
        <div className="section">
          <div className="section__title">{templateState.summaryTitle}</div>
          <dl className="kv">
            {templateState.summaryRows.map(row => (
              <React.Fragment key={row.key}>
                <dt>{row.label}</dt><dd>{row.value}</dd>
              </React.Fragment>
            ))}
          </dl>
        </div>
        <div className="section">
          <div className="section__title">{templateState.triggersTitle}</div>
          <dl className="kv">
            {templateState.triggerRows.map(row => (
              <React.Fragment key={row.key}>
                <dt>{row.label}</dt><dd>{row.value}</dd>
              </React.Fragment>
            ))}
          </dl>
        </div>
        <div className="section section--hint">
          <div className="hint__title">{templateState.quickStartTitle}</div>
          {templateState.quickStartRows.map(row => (
            <div className="hint__line" key={row.key}>
              {row.parts.map(part => {
                if (part.kind === "strong") return <strong key={part.key}>{part.text}</strong>;
                if (part.kind === "code") return <code key={part.key}>{part.text}</code>;
                return <React.Fragment key={part.key}>{part.text}</React.Fragment>;
              })}
            </div>
          ))}
        </div>
      </div>
    </>
  );
}

// ── Gate (fork / join / branch) ───────────────────────────────────
function GateInspector({ studio, flow, inst, clearSelection, editGraphNode = null, editGraphEdge = null, deleteGraphNode = null, contract, graphView = null, conditionView = null }) {
  const change = (action, payload = {}) => editGraphNode?.(inst.id, action, payload);
  const kind = inst.gateKind;
  const gateState = window.MetaFlowController.graphGateControlState(inst, {
    edges: studio.edges,
    members: studio.members,
    contract,
    graphView,
  });
  const branchRows = kind === "branch"
    ? window.MetaFlowController.graphBranchConditionRows({
      inst,
      edges: studio.edges,
      instances: studio.instances,
      members: studio.members,
      schemas: studio.schemas,
      flow,
      contract,
      graphView,
    })
    : [];

  return (
    <>
      <div className="inspector__head">
        <div className="row row--between">
          <div>
            <div className="inspector__eyebrow">{gateState.eyebrow}</div>
            <div className="inspector__title">{gateState.title}</div>
            <div className="inspector__id">{gateState.idLine}</div>
          </div>
          <button className="btn btn--ghost btn--sm" onClick={() => clearSelectionAfterOperation(deleteGraphNode?.(inst.id), clearSelection)}>{gateState.deleteLabel}</button>
        </div>
      </div>
      <div className="inspector__body">
        <div className="section">
          <div className="section__title">{gateState.labelTitle}</div>
          <EchoInput key={inst.id} className="field__input" value={inst.label} onChangeText={label => change("set_label", { label })} />
        </div>
        <div className="section">
          <div className="section__title">{gateState.kindTitle}</div>
          <select className="field__select" value={gateState.gateKind} onChange={e => change("set_gate_kind", { gate_kind: e.target.value })}>
            {gateState.gateKindOptions.map(option => (
              <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
            ))}
          </select>
          {gateState.selectedGateKind?.reason && <div className="kv__hint" style={{ color: "var(--warn)" }}>{gateState.selectedGateKind.reason}</div>}
        </div>
        {kind === "join" && (
          <div className="section">
            <div className="section__title">{gateState.collectionTitle}</div>
            <select className="field__select" value={gateState.collection} onChange={e => change("set_join_collection", { collection: e.target.value })}>
              {gateState.collectionOptions.map(option => (
                <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
              ))}
            </select>
            {gateState.selectedCollection?.reason && <div className="kv__hint" style={{ color: "var(--warn)" }}>{gateState.selectedCollection.reason}</div>}
            {gateState.collection === "quorum" && (
              <div className="row" style={{ marginTop: 8 }}>
                <input className="field__input field__input--num" type="number" min="1"
                  value={inst.quorum?.n || gateState.incoming.length || 1}
                  onChange={e => change("set_join_quorum", { n: e.target.value })} />
                <span className="kv__hint">{gateState.quorumIncomingLabel}</span>
              </div>
            )}
            {gateState.collection && gateState.collection !== "all" && (
              <div className="field" style={{ marginTop: 8 }}>
                <label className="field__label">{gateState.joinMemberLabel}</label>
                <select className="field__select" value={inst.controllerRole || ""} onChange={e => change("set_join_controller_role", { controller_role: e.target.value })}>
                  <option value={gateState.joinMemberPlaceholderOption.value}>{gateState.joinMemberPlaceholderOption.label}</option>
                  {gateState.memberOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
                </select>
                <div className="kv__hint">{gateState.joinMemberHint}</div>
              </div>
            )}
          </div>
        )}
        {kind === "fork" && (
          <div className="section">
            <div className="section__title">{gateState.dispatchTitle}</div>
            <select className="field__select" value={gateState.dispatch} onChange={e => change("set_fork_dispatch", { dispatch: e.target.value })}>
              {gateState.dispatchOptions.map(option => (
                <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
              ))}
            </select>
            {gateState.selectedDispatch?.reason && <div className="kv__hint" style={{ color: "var(--warn)" }}>{gateState.selectedDispatch.reason}</div>}
            <div className="kv__hint">{gateState.dispatchHint}</div>
          </div>
        )}
        {kind === "branch" && (
          <div className="section">
            <div className="section__title">{gateState.conditionsTitle}</div>
            {branchRows.length === 0 && <div className="kv__hint">{gateState.emptyBranchHint}</div>}
            {branchRows.map(row => {
              const e = row.edge;
              const setCondOwner = (instanceId) => editGraphEdge?.(e.id, "set_condition_owner", { owner_instance_id: instanceId });
              const setCondField = (field) => editGraphEdge?.(e.id, "set_condition_field", { field_name: field });
              return (
                <div key={e.id} className="branch-cond-row">
                  <div className="row row--gap">
                    <select className="field__select" value={row.modeValue} onChange={ev => {
                      editGraphEdge?.(e.id, "set_condition_mode", { mode: ev.target.value, owner_instance_id: row.firstOwnerId });
                    }}>
                      {row.modeOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
                    </select>
                    <span className="kv__hint">{row.targetPrefix} {row.targetLabel}</span>
                  </div>
                  {row.isCondition && (
                    !row.hasConditionOptions ? (
                      <div className="kv__hint" style={{ color: "var(--warn)" }}>{row.noConditionOptionsHint}</div>
                    ) : (
                      <div className="bld-cond" style={{ marginTop: 8 }}>
                        <select className="field__select" value={row.ownerValue} onChange={ev => setCondOwner(ev.target.value)}>
                          {row.ownerOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
                        </select>
                        <select className="field__select" value={row.fieldValue} onChange={ev => setCondField(ev.target.value)}>
                          <option value={row.fieldPlaceholderOption.value}>{row.fieldPlaceholderOption.label}</option>
                          {row.fieldOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
                        </select>
                        <select className="field__select bld-cond__op" value={row.operatorValue} onChange={ev => editGraphEdge?.(e.id, "set_condition_operator", { operator: ev.target.value })}>
                          {row.operatorOptions.map(option => (
                            <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
                          ))}
                        </select>
                        <GraphCondValue field={row.condField} value={e.cond?.val} conditionView={conditionView} onChange={val => editGraphEdge?.(e.id, "set_condition_value", { value: val })} />
                      </div>
                    )
                  )}
                </div>
              );
            })}
          </div>
        )}
        <div className="section">
          <div className="section__title">{gateState.wiringTitle}</div>
          <dl className="kv">
            <dt>{gateState.incomingLabel}</dt><dd>{gateState.incomingCount}</dd>
            <dt>{gateState.outgoingLabel}</dt><dd>{gateState.outgoingCount}</dd>
          </dl>
        </div>
      </div>
    </>
  );
}

// ── Instance (graph node) — TEMPLATE-ONLY fields + member summary ──
function InstanceInspector({ studio, flow, inst, selectMember, clearSelection, editGraphNode = null, editGraphEdge = null, deleteGraphNode = null, contract, launchView = null, graphView = null, conditionView = null }) {
  const instanceState = window.MetaFlowController.graphInstanceControlState({
    inst,
    instances: studio.instances,
    members: studio.members,
    schemas: studio.schemas,
    graphView,
  });
  const member = instanceState.member;

  if (inst.isGate) {
    return <GateInspector studio={studio} flow={flow} inst={inst} clearSelection={clearSelection} editGraphNode={editGraphNode} editGraphEdge={editGraphEdge} deleteGraphNode={deleteGraphNode} contract={contract} graphView={graphView} conditionView={conditionView} />;
  }

  // Adaptive layer instance — read-only card. The adaptive step is authored
  // in Basic mode only (v1: no graph-side edit ops); the title is the
  // server-projected instance label, and the locked-authoring copy comes from
  // the controller plane (adaptiveLockedTitle/adaptiveLockedHint on
  // graphInstanceControlState, mirroring the terminal authoringLocked
  // pattern). Delete stays live and mirrors the member-node delete path
  // (removing the node removes the adaptive step).
  if (inst.kind === "adaptive" || inst.adaptive) {
    return (
      <>
        <div className="inspector__head">
          <div className="row row--between">
            <div>
              <div className="inspector__eyebrow">{instanceState.eyebrow}</div>
              <div className="inspector__title">{inst.label || ""}</div>
              <div className="inspector__id">{instanceState.idLine}</div>
            </div>
            <button className="btn btn--ghost btn--sm" onClick={() => clearSelectionAfterOperation(deleteGraphNode?.(inst.id), clearSelection)}>{instanceState.deleteLabel}</button>
          </div>
        </div>
        <div className="inspector__body">
          <div className="section section--locked">
            <div className="section__title">{instanceState.adaptiveLockedTitle}</div>
            <div className="hint__line">{instanceState.adaptiveLockedHint}</div>
          </div>
        </div>
      </>
    );
  }

  if (inst.isTerminal) {
    const terminalState = window.MetaFlowController.graphTerminalControlState(inst, contract, graphView);
    return (
      <>
        <div className="inspector__head">
          <div className="row row--between">
            <div>
              <div className="inspector__eyebrow">{terminalState.eyebrow}</div>
              <div className="inspector__title">{terminalState.title}</div>
              <div className="inspector__id">{terminalState.idLine}</div>
            </div>
            <button className="btn btn--ghost btn--sm" onClick={() => clearSelectionAfterOperation(deleteGraphNode?.(inst.id), clearSelection)}>{terminalState.deleteLabel}</button>
          </div>
        </div>
        <div className="inspector__body">
          <div className="section">
            <div className="section__title">{terminalState.labelTitle}</div>
            <input className="field__input" value={terminalState.labelValue} disabled readOnly />
          </div>
          <div className="section">
            <div className="section__title">{terminalState.kindTitle}</div>
            <select className="field__select" value={terminalState.terminalKind} disabled>
              {terminalState.terminalKindOptions.map(option => (
                <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
              ))}
            </select>
            {terminalState.selectedTerminalKind?.reason && <div className="kv__hint" style={{ color: "var(--warn)" }}>{terminalState.selectedTerminalKind.reason}</div>}
          </div>
          <div className="section section--locked">
            <div className="section__title">{terminalState.authoringLockedTitle}</div>
            <div className="hint__line">{terminalState.authoringLockedHint}</div>
          </div>
        </div>
      </>
    );
  }

  const launchState = window.MetaFlowController.launchModeControlState(inst, contract, launchView);

  return (
    <>
      <div className="inspector__head">
        <div className="row row--between">
          <div>
            <div className="inspector__eyebrow">{instanceState.eyebrow}</div>
            <div className="inspector__title">{instanceState.title}</div>
            <div className="inspector__id">{instanceState.idLine}</div>
          </div>
          <button className="btn btn--ghost btn--sm" onClick={() => clearSelectionAfterOperation(deleteGraphNode?.(inst.id), clearSelection)}>{instanceState.deleteLabel}</button>
        </div>
      </div>

      <div className="inspector__body">

        {/* MEMBER SUMMARY (read-only summary, click to edit) */}
        {member && (
          <div className="section section--member-card">
            <div className="member-card">
              <div className="member-card__head">
                <span className="member-card__role">{instanceState.memberRoleLabel}</span>
                <button className="btn btn--ghost btn--sm" onClick={() => selectMember(instanceState.memberId)}>{instanceState.editMemberLabel}</button>
              </div>
              <div className="member-card__name">{instanceState.memberName}</div>
              <dl className="kv kv--small">
                {instanceState.memberSummaryRows.map(row => (
                  <React.Fragment key={row.key}>
                    <dt>{row.label}</dt><dd>{row.value}</dd>
                  </React.Fragment>
                ))}
              </dl>
              <div className="member-card__hint">{instanceState.memberHint}</div>
            </div>
          </div>
        )}

        {/* TEMPLATE-ONLY FIELDS */}
        <div className="section">
          <div className="section__title">{launchState.graphLaunchTitle}</div>
          <select
            className="field__select"
            value={launchState.launchKind}
            onChange={e => {
              editGraphNode?.(inst.id, "set_launch_kind", { kind: e.target.value, first_fork_source_id: instanceState.firstForkSourceId });
            }}
          >
            {launchState.launchOptions.map(option => (
              <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
            ))}
          </select>
          {launchState.selectedLaunchMode?.reason && <div className="hint__line" style={{ color: "var(--warn)" }}>{launchState.selectedLaunchMode.reason}</div>}
          {launchState.launchKind === "Resume" && (
            <div className="field" style={{ marginTop: 8 }}>
              <label className="field__label">{launchState.resumeSessionLabel}</label>
              <input
	                className="field__input"
	                value={launchState.launchMode.sessionId || ""}
	                placeholder={launchState.resumeSessionPlaceholder}
	                onChange={e => editGraphNode?.(inst.id, "set_launch_session", { session_id: e.target.value })}
	              />
            </div>
          )}
          {launchState.launchKind === "Fork" && (
            <>
              <div className="field" style={{ marginTop: 8 }}>
                <label className="field__label">{launchState.forkSourceLabel}</label>
                <select className="field__select" value={launchState.launchMode.from || ""} onChange={e => editGraphNode?.(inst.id, "set_launch_fork_source", { from: e.target.value })}>
                  {instanceState.forkSourceOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
                </select>
              </div>
              <div className="field">
                <label className="field__label">{launchState.graphForkContextLabel}</label>
                <select className="field__select" value={launchState.forkContextValue} onChange={e => editGraphNode?.(inst.id, "set_launch_fork_context", { context: e.target.value })}>
                  {launchState.forkContextOptions.map(option => (
                    <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
                  ))}
                </select>
              </div>
              {launchState.selectedForkContext?.reason && <div className="hint__line" style={{ color: "var(--warn)" }}>{launchState.selectedForkContext.reason}</div>}
            </>
          )}
        </div>

        <div className="section">
          <div className="section__title">{instanceState.positionTitle}</div>
          <dl className="kv kv--small">
            {instanceState.positionRows.map(row => (
              <React.Fragment key={row.key}>
                <dt>{row.label}</dt><dd>{row.value}</dd>
              </React.Fragment>
            ))}
          </dl>
        </div>

        {member && (
          <div className="section">
            <div className="section__title">{instanceState.outputTitle}</div>
            {instanceState.outputSchema && (
              <ul className="schema-fields">
                {instanceState.outputFieldRows.map(f => (
                  <li key={f.id}>
                    <span className="sf__name">{f.name}</span>
                    <span className="sf__type">{f.type}</span>
                    {f.required && <span className="sf__req">{f.requiredLabel}</span>}
                  </li>
                ))}
              </ul>
            )}
            <div className="hint__line" style={{ marginTop: 6 }}>{instanceState.outputHint} <button className="link" onClick={() => selectMember(instanceState.memberId)}>{instanceState.outputOpenMemberLabel}</button></div>
          </div>
        )}
      </div>
    </>
  );
}

function GraphCondValue({ field, value, onChange, conditionView = null }) {
  const control = window.MetaFlowController.conditionValueControl(field, value, conditionView);
  if (control.kind === "enum") {
    return (
      <select className="field__select" value={control.value} onChange={e => onChange(e.target.value)}>
        {control.optionRows.map(row => <option key={row.value || "blank"} value={row.value}>{row.label}</option>)}
      </select>
    );
  }
  if (control.kind === "boolean") {
    return (
      <select className="field__select" value={control.value} onChange={e => onChange(e.target.value)}>
        {control.optionRows.map(row => <option key={row.value || "blank"} value={row.value}>{row.label}</option>)}
      </select>
    );
  }
  return <EchoInput className="field__input" placeholder={control.placeholder} value={control.value} onChangeText={onChange} />;
}

// ── Edge ─────────────────────────────────────────────────────────
function EdgeInspector({ studio, flow, edge, clearSelection, editGraphEdge = null, deleteGraphEdge = null, contract, graphView = null, conditionView = null }) {
  const edgeState = window.MetaFlowController.graphEdgeInspectorState({
    edge,
    instances: studio.instances,
    members: studio.members,
    schemas: studio.schemas,
    flow,
    contract,
    graphView,
  });
  const change = (action, payload = {}) => editGraphEdge?.(edge.id, action, payload);
  const setEdgeKind = (kind) => change("set_kind", { edge_kind: kind });
  const setCondOwner = (instanceId) => change("set_condition_owner", { owner_instance_id: instanceId });
  const setCondField = (field) => change("set_condition_field", { field_name: field });
  return (
    <>
      <div className="inspector__head">
        <div className="row row--between">
          <div>
            <div className="inspector__eyebrow">{edgeState.eyebrow}</div>
            <div className="inspector__title">{edgeState.title}</div>
            <div className="inspector__id">{edgeState.idLine}</div>
          </div>
          <button className="btn btn--ghost btn--sm" onClick={() => clearSelectionAfterOperation(deleteGraphEdge?.(edge.id), clearSelection)}>{edgeState.deleteLabel}</button>
        </div>
      </div>
      <div className="inspector__body">
        <div className="section">
          <div className="section__title">{edgeState.kindTitle}</div>
          <select className="field__select" value={edgeState.edgeKind} onChange={e => setEdgeKind(e.target.value)}>
            {edgeState.edgeKindOptions.map(option => (
              <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
            ))}
          </select>
          {edgeState.selectedEdgeKind?.reason && <div className="hint__line" style={{ color: "var(--warn)" }}>{edgeState.selectedEdgeKind.reason}</div>}
        </div>
        <div className="section">
          <div className="section__title">{edgeState.labelTitle}</div>
          <EchoInput key={edge.id} className="field__input" value={edge.label || ""} onChangeText={label => change("set_label", { label })} />
        </div>
        {edgeState.isCondition && (
          <div className="section">
            <div className="section__title">{edgeState.conditionTitle}</div>
            {!edgeState.hasConditionOptions ? (
              <div className="hint__line" style={{ color: "var(--warn)" }}>{edgeState.noConditionOptionsHint}</div>
            ) : (
            <div className="cond-row">
              <select className="field__select" value={edgeState.ownerValue} onChange={e => setCondOwner(e.target.value)}>
                <option value={edgeState.ownerPlaceholderOption.value}>{edgeState.ownerPlaceholderOption.label}</option>
                {edgeState.ownerOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
              </select>
              <select className="field__select" value={edgeState.fieldValue} disabled={!edgeState.condOwner} onChange={e => setCondField(e.target.value)}>
                <option value="">{edgeState.fieldPlaceholder}</option>
                {edgeState.fieldOptions.map(option => <option key={option.field.id || option.value} value={option.value}>{option.label}</option>)}
              </select>
              <select className="field__select" style={{ width: 60 }} value={edgeState.operatorValue} onChange={e => change("set_condition_operator", { operator: e.target.value })}>
                {edgeState.operatorOptions.map(option => (
                  <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
                ))}
              </select>
              <GraphCondValue field={edgeState.condField} value={edge.cond?.val} conditionView={conditionView} onChange={val => change("set_condition_value", { value: val })} />
            </div>
            )}
          </div>
        )}
        <div className="section">
          <div className="section__title">{edgeState.fromTitle}</div>
          <dl className="kv">
            {edgeState.fromRows.map(row => (
              <React.Fragment key={row.key}>
                <dt>{row.label}</dt><dd>{row.value}</dd>
              </React.Fragment>
            ))}
          </dl>
        </div>
        <div className="section">
          <div className="section__title">{edgeState.toTitle}</div>
          <dl className="kv">
            {edgeState.toRows.map(row => (
              <React.Fragment key={row.key}>
                <dt>{row.label}</dt><dd>{row.value}</dd>
              </React.Fragment>
            ))}
          </dl>
        </div>
      </div>
    </>
  );
}

// ── Add-node dialog ────────────────────────────────────────────────
// In Graph view, you can place existing agents and Meta flow gates.
// To define a new agent, jump to the Agents view.
export function AddNodeMenu({ at, members, contract, graphView = null, onPick, onClose, onJumpToAgents }) {
  const [q, setQ] = React.useState("");
  React.useEffect(() => { setQ(""); }, [at]);
  if (!at) return null;

  const menuState = window.MetaFlowController.graphAddNodeMenuState({ members, contract, query: q, graphView });

  return (
    <div className="add-menu" data-own-scroll="" style={{ left: at.x, top: at.y }} onClick={e => e.stopPropagation()} onMouseDown={e => e.stopPropagation()}>
      <div className="add-menu__search">
        <span className="add-menu__search-icon">{menuState.searchIcon}</span>
        <input className="add-menu__search-input" autoFocus placeholder={menuState.searchPlaceholder} value={q} onChange={e => setQ(e.target.value)}
          onKeyDown={e => { if (e.key === "Escape") onClose(); }} />
        <button className="add-menu__x" onClick={onClose} title={menuState.closeTitle}>{menuState.closeLabel}</button>
      </div>

      <div className="add-menu__scroll">
        {menuState.hasMembers && <div className="add-menu__label">{menuState.agentsLabel}</div>}
        {menuState.memberRows.map(row => (
          <button key={row.id} className="add-menu__row" onClick={() => onPick(row.pick)}>
            <span className="add-menu__dot" data-role={row.role} style={row.dotStyle} />
            <span className="add-menu__row-name">{row.name}</span>
            <span className="add-menu__row-meta">{row.model}</span>
          </button>
        ))}

        {menuState.hasControls && <div className="add-menu__label">{menuState.controlsLabel}</div>}
        {menuState.controlRows.map(row => (
          <button key={row.id} className="add-menu__row" onClick={() => onPick(row.pick)}>
            <span className="add-menu__glyph">{row.glyph}</span>
            <span className="add-menu__row-name">{row.label}</span>
            <span className="add-menu__row-meta">{row.meta}</span>
          </button>
        ))}

        {menuState.isEmpty && (
          <div className="add-menu__empty">{menuState.emptyLabel}</div>
        )}
      </div>

      {onJumpToAgents && (
        <button className="add-menu__foot" onClick={() => onJumpToAgents(null)}>{menuState.jumpLabel}</button>
      )}
    </div>
  );
}
