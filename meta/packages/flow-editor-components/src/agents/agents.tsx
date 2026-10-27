// AgentsView — full-pane Agents catalog editor.
//
// Two side-by-side stacks:
//   ┌─ AGENTS ────┐ ┌─ EDITOR ─────────────────────────────┐
//   │ planner     │ │ identity, prompt, tools, schema ref, │
//   │ coder       │ │ used in (instances)                  │
//   │ reviewer ●  │ │                                       │
//   │ + new       │ │ ── SCHEMA LIBRARY (collapsible) ──    │
//   ├─ SCHEMAS ───┤ │ visual field editor for selected     │
//   │ PlanArtifact│ │ schema (no JSON)                      │
//   │ ReviewGate  │ └───────────────────────────────────────┘
//
// The right pane swaps based on what's selected:
//   { kind: "agent", id }   → AgentEditor
//   { kind: "schema", id }  → SchemaEditor (visual, field-by-field)
//   null                    → empty hint

import { EchoInput, EchoTextArea } from "../shared/echo-text";

export function AgentsView({ studio, agentSel, setAgentSel, contract, deploySettings, flow, setFlow, mobSettings, setMobSettings, toolCatalog = [], modelCatalog = [], agentDefinitions = [], applyAgentIntent = null, agentView = null, agentDetailView = null, agentAccessView = null, schemaView = null }) {
  return (
    <div className="agents-view">
      <AgentsList studio={studio} agentSel={agentSel} setAgentSel={setAgentSel} contract={contract} deploySettings={deploySettings} agentDefinitions={agentDefinitions} applyAgentIntent={applyAgentIntent} toolCatalog={toolCatalog} modelCatalog={modelCatalog} agentView={agentView} />
      <div className="agents-view__main">
        <AgentsMain studio={studio} agentSel={agentSel} setAgentSel={setAgentSel} contract={contract} deploySettings={deploySettings} flow={flow} setFlow={setFlow} mobSettings={mobSettings} setMobSettings={setMobSettings} toolCatalog={toolCatalog} modelCatalog={modelCatalog} applyAgentIntent={applyAgentIntent} agentView={agentView} agentDetailView={agentDetailView} agentAccessView={agentAccessView} schemaView={schemaView} />
      </div>
    </div>
  );
}

function AgentsList({ studio, agentSel, setAgentSel, contract, deploySettings, agentDefinitions, applyAgentIntent = null, toolCatalog = [], modelCatalog = [], agentView = null }) {
  const [schemaAddResult, setSchemaAddResult] = React.useState(null);
  const listState = window.MetaFlowController.agentListState({
    members: studio.members,
    instances: studio.instances,
    schemas: studio.schemas,
    selection: agentSel,
    agentView,
  });
  const schemaAddErrorState = window.MetaFlowController.schemaDefinitionAddErrorState(schemaAddResult, listState.schemaAddFallbackError);
  return (
    <aside className="agents-list">
      <div className="agents-list__head">
        <span className="agents-list__title">{listState.agentsHeading}</span>
        <span className="agents-list__count">{listState.memberCount}</span>
      </div>
      <div className="agents-list__scroll">
        {listState.memberRows.map(row => {
          return (
            <button
              key={row.id}
              className={row.itemClass}
              onClick={() => setAgentSel(window.MetaFlowController.agentListSelectionProjection("agent", row.id))}
            >
              <span className="agents-list__bullet" data-role={row.bulletRole} style={row.bulletStyle}>●</span>
              <div className="agents-list__col">
                <span className="agents-list__name">{row.name}</span>
                <span className="agents-list__sub">{row.subLabel}</span>
              </div>
              <span className={row.placedClass}>
                {row.placedLabel}
              </span>
            </button>
          );
        })}
        <AddAgentControl studio={studio} setAgentSel={setAgentSel} agentDefinitions={agentDefinitions} applyAgentIntent={applyAgentIntent} contract={contract} deploySettings={deploySettings} toolCatalog={toolCatalog} modelCatalog={modelCatalog} agentView={agentView} />
      </div>

      <div className="agents-list__head agents-list__head--sub">
        <span className="agents-list__title">{listState.schemasHeading}</span>
        <span className="agents-list__count">{listState.schemaCount}</span>
      </div>
      <div className="agents-list__scroll">
        {listState.schemaRows.map(row => {
          return (
            <button
              key={row.id}
              className={row.itemClass}
              onClick={() => setAgentSel(window.MetaFlowController.agentListSelectionProjection("schema", row.id))}
            >
              <span className="agents-list__bullet" data-role={row.bulletRole} style={row.bulletStyle}>▢</span>
              <div className="agents-list__col">
                <span className="agents-list__name">{row.id}</span>
                <span className="agents-list__sub">{row.subLabel}</span>
              </div>
            </button>
          );
        })}
        <button
          className="agents-list__add"
          onClick={() => {
            if (!applyAgentIntent) {
              setSchemaAddResult({ ok: false, error: listState.authoringOperationUnavailableError });
              return;
            }
            setSchemaAddResult(null);
            applyAgentIntent({ intent: "schema.add" }).then((result) => {
              if (result?.ok === false) {
                setSchemaAddResult(result);
                return;
              }
              const selection = result?.selection;
              setSchemaAddResult(null);
              if (selection?.kind) setAgentSel(selection);
            }).catch((error) => {
              setSchemaAddResult({
                ok: false,
                error: error?.message || String(error || listState.schemaAddFallbackError),
              });
            });
          }}
        >{listState.addSchemaLabel}</button>
        {schemaAddErrorState.hasError && <div className="hint__line">{schemaAddErrorState.text}</div>}
      </div>
    </aside>
  );
}

function AddAgentControl({ studio, setAgentSel, agentDefinitions = [], applyAgentIntent = null, contract = null, deploySettings = null, toolCatalog = [], modelCatalog = [], agentView = null }) {
  const [lastAddResult, setLastAddResult] = React.useState(null);
  const definitionState = window.MetaFlowController.agentDefinitionAddControlState(agentDefinitions, agentView);
  const definitionErrorState = window.MetaFlowController.agentDefinitionAddErrorState(lastAddResult, agentView);
  const createFromDefinition = async (definitionId) => {
    if (!applyAgentIntent) {
      setLastAddResult({ ok: false, error: definitionState.authoringOperationUnavailableError });
      return;
    }
    // No client-side history snapshot before a server-authoritative op: it
    // bumps authoringRevision (via markDraft) and the runner then drops any
    // in-flight op whose enqueued revision is now stale, silently discarding
    // the prior agent edit. Undo/redo are server-backed (handleHistoryStep),
    // so the studio history snap maintained here is dead for undo anyway.
    const result = await applyAgentIntent({ intent: "agent.addDefinition", definitionId });
    setLastAddResult(result);
    if (!result.ok) return;
    setAgentSel(result.selection);
  };
  if (!definitionState.hasDefinitions) {
    return (
      <>
        <button
          className={definitionState.controlClass}
          disabled
          title={definitionState.title}
        >{definitionState.unavailableLabel}</button>
        {definitionErrorState.hasError && <div className="hint__line">{definitionErrorState.text}</div>}
      </>
    );
  }
  return (
    <>
      <select
        className={definitionState.controlClass}
        value={definitionState.value}
        title={definitionState.title}
        onChange={e => {
          const id = e.target.value;
          if (!id) return;
          createFromDefinition(id);
          e.target.value = "";
        }}
      >
        <option value={definitionState.placeholderOption.value}>{definitionState.placeholderOption.label}</option>
        {definitionState.optionRows.map(option => (
          <option key={option.value} value={option.value}>{option.label}</option>
        ))}
      </select>
      {definitionErrorState.hasError && <div className="hint__line">{definitionErrorState.text}</div>}
    </>
  );
}

function AgentsMain({ studio, agentSel, setAgentSel, contract, deploySettings, flow, setFlow, mobSettings, setMobSettings, toolCatalog, modelCatalog, applyAgentIntent = null, agentView = null, agentDetailView = null, agentAccessView = null, schemaView = null }) {
  const selectionState = window.MetaFlowController.agentSelectionState({
    selection: agentSel,
    members: studio.members,
    schemas: studio.schemas,
    agentView,
  });
  if (selectionState.kind === "empty") {
    return (
      <div className="agents-empty">
        <div className="agents-empty__head">{selectionState.emptyState.title}</div>
        {selectionState.emptyState.lines.map((line, index) => (
          <div className="agents-empty__line" key={index}>{line}</div>
        ))}
      </div>
    );
  }
  if (selectionState.kind === "schema") {
    if (!selectionState.schema) return <div className="agents-empty">{selectionState.missingSchemaLabel}</div>;
    return <SchemaEditor studio={studio} schema={selectionState.schema} setAgentSel={setAgentSel} contract={contract} flow={flow} setFlow={setFlow} schemaView={schemaView} applyAgentIntent={applyAgentIntent} />;
  }
  if (!selectionState.member) return <div className="agents-empty">{selectionState.missingAgentLabel}</div>;
  return <AgentEditor studio={studio} member={selectionState.member} setAgentSel={setAgentSel} contract={contract} deploySettings={deploySettings} flow={flow} setFlow={setFlow} mobSettings={mobSettings} setMobSettings={setMobSettings} toolCatalog={toolCatalog} modelCatalog={modelCatalog} applyAgentIntent={applyAgentIntent} agentView={agentView} agentDetailView={agentDetailView} agentAccessView={agentAccessView} />;
}

// ── Agent editor ────────────────────────────────────────────────────
function AgentEditor({ studio, member, setAgentSel, contract, deploySettings, flow, setFlow, mobSettings, setMobSettings, toolCatalog = [], modelCatalog = [], applyAgentIntent = null, agentView = null, agentDetailView = null, agentAccessView = null }) {
  const [memberEditError, setMemberEditError] = React.useState("");
  const [deleteConfirmOpen, setDeleteConfirmOpen] = React.useState(false);
  React.useEffect(() => {
    setDeleteConfirmOpen(false);
  }, [member.id]);
  const metaOperationError = (result, fallback) => {
    return window.MetaFlowController.operationErrorText(result, fallback);
  };
  const editorState = window.MetaFlowController.agentEditorControlState({
    member,
    instances: studio.instances,
    schemas: studio.schemas,
    contract,
    deploySettings,
    modelCatalog,
    agentDetailView,
    agentView,
  });
  const unavailableError = editorState.authoringOperationUnavailableError;
  const change = async (patch) => {
    if (!patch || typeof patch !== "object" || !Object.keys(patch).length) return;
    if (!applyAgentIntent) {
      setMemberEditError(unavailableError);
      return;
    }
    try {
      const result = await applyAgentIntent({ intent: "agent.updateMember", memberId: member.id, patch });
      if (!result?.ok) {
        setMemberEditError(metaOperationError(result, editorState.memberUpdateFallbackError));
        return;
      }
      setMemberEditError("");
    } catch (error) {
      setMemberEditError(error?.message || editorState.memberUpdateFallbackError);
    }
  };
  const [toolDraft, setToolDraft] = React.useState("");
  const [toolDraftError, setToolDraftError] = React.useState("");
  const [schemaChangeResult, setSchemaChangeResult] = React.useState(null);
  const toolAccessState = window.MetaFlowController.memberToolAccessState(member, toolCatalog, agentAccessView);
  const schemaErrorState = window.MetaFlowController.memberSchemaChangeErrorState(schemaChangeResult);
  const addToolAccess = async (raw) => {
    const toolId = String(raw || "").trim();
    if (!toolId) {
      setToolDraftError(toolAccessState.emptyToolError);
      return;
    }
    if (!applyAgentIntent) {
      setToolDraftError(toolAccessState.authoringOperationUnavailableError);
      return;
    }
    try {
      const result = await applyAgentIntent({ intent: "agent.addTool", memberId: member.id, toolId });
      if (!result?.ok) {
        setToolDraftError(metaOperationError(result, editorState.toolUpdateFallbackError));
        return;
      }
      setToolDraft("");
      setToolDraftError("");
    } catch (error) {
      setToolDraftError(error?.message || editorState.toolUpdateFallbackError);
    }
  };
  const removeToolAccess = async (toolId) => {
    if (!applyAgentIntent) {
      setToolDraftError(toolAccessState.authoringOperationUnavailableError);
      return;
    }
    try {
      const result = await applyAgentIntent({ intent: "agent.removeTool", memberId: member.id, toolId });
      if (!result?.ok) {
        setToolDraftError(metaOperationError(result, editorState.toolUpdateFallbackError));
        return;
      }
      setToolDraftError("");
    } catch (error) {
      setToolDraftError(error?.message || editorState.toolUpdateFallbackError);
    }
  };
  const changeSchema = (rawSchema) => {
    if (!applyAgentIntent) {
      setSchemaChangeResult({ ok: false, error: unavailableError });
      return;
    }
    applyAgentIntent({
      intent: "agent.assignSchema",
      memberId: member.id,
      schemaId: rawSchema,
      selection: { kind: "agent", id: member.id },
    }).then((result) => {
      setSchemaChangeResult(result?.ok === false ? result : null);
    }).catch((error) => {
      setSchemaChangeResult({ ok: false, error: error?.message || editorState.schemaAssignmentFallbackError });
    });
  };
  const deleteConfirmState = window.MetaFlowController.agentDeleteConfirmationState(editorState, deleteConfirmOpen);
  const deleteMember = () => {
    if (!applyAgentIntent) return;
    const selection = null;
    applyAgentIntent({
      intent: "agent.deleteMember",
      memberId: member.id,
      selection,
    }).then((result) => {
      if (result?.ok === false) return;
      setAgentSel(selection);
      setDeleteConfirmOpen(false);
    }).catch(() => {
      setDeleteConfirmOpen(false);
    });
  };

  return (
    <div className="agent-editor">
      <div className="agent-editor__head">
        <div className="row row--between">
          <div>
            <div className="inspector__eyebrow">{editorState.eyebrow}</div>
            <EchoInput
              key={member.id}
              className="agent-editor__title-input"
              value={member.name}
              onChangeText={name => change(window.MetaFlowController.memberNamePatch(name))}
            />
            <div className="inspector__id">{editorState.idLine}</div>
            {memberEditError && <div className="hint__line" style={{ color: "var(--danger)" }}>{memberEditError}</div>}
          </div>
          <button className="btn btn--ghost btn--sm" onClick={() => {
            if (deleteConfirmState.needsConfirmation) {
              setDeleteConfirmOpen(true);
              return;
            }
            deleteMember();
          }}>{editorState.deleteLabel}</button>
        </div>
        {deleteConfirmState.open && (
          <div className="agent-editor__confirm">
            <span>{deleteConfirmState.message}</span>
            <button className="btn btn--ghost btn--sm" onClick={() => setDeleteConfirmOpen(false)}>{deleteConfirmState.cancelLabel}</button>
            <button className="btn btn--primary btn--sm" onClick={deleteMember}>{deleteConfirmState.confirmLabel}</button>
          </div>
        )}
      </div>

      <div className="agent-editor__body">
        <div className="agent-editor__cols">
          {/* LEFT COLUMN — identity, prompt */}
          <div className="agent-editor__col">

            <div className="section">
              <div className="section__title">{editorState.identityTitle}</div>
              {editorState.isRealmProfile ? (
                <div className="field">
                  <label className="field__label">{editorState.realmProfileLabel}</label>
                  <input
                    className="field__input field__input--mono"
                    value={member.realmProfile || ""}
                    placeholder={editorState.realmProfilePlaceholder}
                    onChange={e => change(window.MetaFlowController.memberRealmProfilePatch(e.target.value))}
                  />
                  <div className="hint__line">{editorState.realmProfileImportHint}</div>
                </div>
              ) : (
                <>
              <div className="field">
                <label className="field__label">{editorState.modelLabel}</label>
                <select className="field__select" value={member.model} onChange={e => change(window.MetaFlowController.memberModelPatch(e.target.value, modelCatalog))}>
                  {editorState.modelOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
                </select>
              </div>
                </>
              )}
            </div>

            {!editorState.isRealmProfile && (
              <div className="section">
                <div className="section__title section__title--row">
                  <span>{editorState.systemPromptTitle}</span>
                  <button className="ghost-btn" onClick={() => change(window.MetaFlowController.memberSystemPromptPatch(window.MetaFlowController.memberPromptSkeleton(member)))} title={editorState.applySkeletonTitle}>{editorState.applySkeletonLabel}</button>
                </div>
                <EchoTextArea
                  key={member.id}
                  className="field__textarea"
                  rows={8}
                  value={member.systemPrompt || ""}
                  onChangeText={text => change(window.MetaFlowController.memberSystemPromptPatch(text))}
                  placeholder={editorState.systemPromptPlaceholder}
                />
              </div>
            )}

            <details className="section agent-runtime">
              <summary className="section__title agent-runtime__summary">
                <span>{editorState.runtimeSectionTitle}</span>
              </summary>
              <div className="agent-runtime__body">
                <div className="field">
                  <label className="field__label">{editorState.profileBindingLabel}</label>
                  <select
                    className="field__select"
                    value={editorState.profileBinding}
                    onChange={e => change(window.MetaFlowController.memberProfileBindingPatch(member, e.target.value, contract))}
                  >
                    {editorState.bindingOptions.map(option => (
                      <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
                    ))}
                  </select>
                  {editorState.selectedBinding?.reason && <div className="hint__line" style={{ color: "var(--warn)" }}>{editorState.selectedBinding.reason}</div>}
                </div>
                {!editorState.isRealmProfile && (
                  <>
                    <div className="field">
                      <label className="field__label">{editorState.runtimeModeLabel}</label>
                      <select className="field__select" value={editorState.runtimeMode} onChange={e => change(window.MetaFlowController.memberRuntimeModePatch(e.target.value, contract, deploySettings))}>
                        {editorState.runtimeOptions.map(option => (
                          <option key={option.value} value={option.value} disabled={option.disabled}>
                            {option.label}
                          </option>
                        ))}
                      </select>
                      {editorState.selectedRuntime?.reason && <div className="hint__line" style={{ color: "var(--warn)" }}>{editorState.selectedRuntime.reason}</div>}
                    </div>
                    <div className="field">
                      <label className="field__label">{editorState.backendLabel}</label>
                      <select className="field__select" value={editorState.backendValue} onChange={e => change(window.MetaFlowController.memberBackendPatch(e.target.value, contract))}>
                        {editorState.backendOptions.map(option => (
                          <option key={option.value || "default"} value={option.value} disabled={option.disabled}>{option.label}</option>
                        ))}
                      </select>
                      {editorState.selectedBackend?.reason && <div className="hint__line" style={{ color: "var(--warn)" }}>{editorState.selectedBackend.reason}</div>}
                    </div>
                    <div className="field">
                      <label className="field__label">{editorState.inlinePeerNotificationsLabel}</label>
                      <input
                        className="field__input"
                        type="number"
                        min="-1"
                        step="1"
                        value={member.maxInlinePeerNotifications ?? ""}
                        placeholder={editorState.inlinePeerNotificationsPlaceholder}
                        onChange={e => change(window.MetaFlowController.memberMaxInlinePeerNotificationsPatch(e.target.value))}
                      />
                    </div>
                    <ProviderParamsEditor member={member} change={change} agentDetailView={agentDetailView} />
                  </>
                )}
              </div>
            </details>

            <div className="section">
              <div className="section__title">{editorState.sourceProvenance.title}</div>
              {editorState.sourceProvenance.hasRows ? (
                <dl className="kv kv--small">
                  {editorState.sourceProvenance.rows.map(row => (
                    <React.Fragment key={row.label}>
                      <dt>{row.label}</dt>
                      <dd>{row.value}</dd>
                    </React.Fragment>
                  ))}
                </dl>
              ) : (
                <div className="hint__line">{editorState.sourceProvenance.emptyHint}</div>
              )}
            </div>

          </div>

          {/* RIGHT COLUMN — tools, schema, usage */}
          <div className="agent-editor__col">

            {editorState.isRealmProfile ? (
              <div className="section">
                <div className="section__title">{editorState.realmProfileTitle}</div>
                <div className="hint__line">
                  {editorState.realmProfileReferenceHintBefore} <code>{editorState.realmProfileReferenceLabel}</code> {editorState.realmProfileReferenceHintAfter}
                </div>
              </div>
            ) : (
              <>
            <div className="section">
              <div className="section__title">{toolAccessState.title}</div>
              <div className="hint__line" style={{ marginBottom: 8 }}>
                {toolAccessState.hint}
              </div>
              {toolAccessState.rows.map(row => {
                return (
                  <div key={row.id} className={row.className}>
                    <div>
                      <div className="name">{row.name}</div>
                      <div className="auth">{row.description}</div>
                    </div>
                    <button onClick={() => removeToolAccess(row.id)}>{row.removeLabel}</button>
                  </div>
                );
              })}
              <select
                className="field__select"
                value={toolAccessState.addSelectValue}
                onChange={e => {
                  const id = e.target.value; if (!id) return;
                  addToolAccess(id);
                }}
              >
                <option value={toolAccessState.addSelectValue}>{toolAccessState.addSelectPlaceholder}</option>
                {toolAccessState.addableRows.map(row => (
                  <option key={row.id} value={row.value} disabled={row.disabled}>{row.optionLabel}</option>
                ))}
              </select>
              <div className="field" style={{ marginTop: 8 }}>
                <label className="field__label">{toolAccessState.sourceLabel}</label>
                <div className="row row--gap">
                  <input
                    className="field__input field__input--mono"
                    value={toolDraft}
                    placeholder={toolAccessState.sourcePlaceholder}
                    onChange={e => { setToolDraft(e.target.value); setToolDraftError(""); }}
                    onKeyDown={e => { if (e.key === "Enter") addToolAccess(toolDraft); }}
                  />
                  <button className="btn btn--ghost btn--sm" onClick={() => addToolAccess(toolDraft)}>{toolAccessState.addButtonLabel}</button>
                </div>
                {toolDraftError && <div className="hint__line">{toolDraftError}</div>}
              </div>
            </div>

            <div className="section">
              <div className="section__title">{editorState.outputSchemaTitle}</div>
              <select
                className="field__select"
                value={member.schema || ""}
                onChange={e => changeSchema(e.target.value)}
              >
                {editorState.schemaOptions.map(option => <option key={option.value || "none"} value={option.value}>{option.label}</option>)}
              </select>
              {schemaErrorState.hasError && <div className="hint__line">{schemaErrorState.text}</div>}
              {editorState.hasOutputSchema ? (
                <>
                  <ul className="schema-fields schema-fields--preview">
                    {editorState.schemaPreviewRows.map(f => (
                      <li key={f.id}>
                        <span className="sf__name">{f.name}</span>
                        <span className="sf__type">{f.type}</span>
                        {f.required && <span className="sf__req">{f.requiredLabel}</span>}
                      </li>
                    ))}
                  </ul>
                  <button className="link" onClick={() => setAgentSel(editorState.editSchemaSelection)}>
                    {editorState.editSchemaLabel}
                  </button>
                </>
              ) : (
                <div className="hint__line" style={{ marginTop: 6 }}>
                  {editorState.emptySchemaHint}
                </div>
              )}
            </div>

            <div className="section">
              <SkillAccess studio={studio} member={member} agentAccessView={agentAccessView} applyAgentIntent={applyAgentIntent} />
            </div>
              </>
            )}

            <div className="section">
              <div className="section__title">{editorState.usageTitle}</div>
              {editorState.placedCount === 0 && (
                <div className="hint__line">{editorState.emptyUsageHint}</div>
              )}
              {editorState.usageRows.map(row => (
                <div key={row.id} className="usage-row usage-row--ro">
                  <span className="usage-row__label">{row.id}</span>
                  <span className="usage-row__cell">{row.cellLabel}</span>
                  <span className="usage-row__lane">{row.laneLabel}</span>
                </div>
              ))}
            </div>

          </div>
        </div>
      </div>
    </div>
  );
}

// ── Schema editor (visual, field-by-field) ──────────────────────────
function SchemaEditor({ studio, schema, setAgentSel, contract, flow, setFlow, schemaView = null, applyAgentIntent = null }) {
  const [fieldAddResult, setFieldAddResult] = React.useState(null);
  const [schemaOperationError, setSchemaOperationError] = React.useState("");
  React.useEffect(() => setFieldAddResult(null), [schema?.id]);
  React.useEffect(() => setSchemaOperationError(""), [schema?.id]);
  const schemaState = window.MetaFlowController.schemaEditorControlState({
    schema,
    members: studio.members,
    schemaView,
  });
  const fieldAddErrorState = window.MetaFlowController.schemaFieldAddErrorState(fieldAddResult, schemaState.fieldAddFallbackError);
  const applySchemaIntent = async (intentRequest, selection = { kind: "schema", id: schema.id }) => {
    const fallback = schemaState.schemaOperationFallbackError;
    if (!applyAgentIntent) {
      const result = { ok: false, error: schemaState.authoringOperationUnavailableError };
      setSchemaOperationError(window.MetaFlowController.operationErrorText(result, fallback));
      return result;
    }
    try {
      const result = await applyAgentIntent({ ...intentRequest, selection });
      if (result?.ok === false) {
        setSchemaOperationError(window.MetaFlowController.operationErrorText(result, fallback));
      } else {
        setSchemaOperationError("");
      }
      return result;
    } catch (error) {
      const result = { ok: false, error: error?.message || String(error || fallback) };
      setSchemaOperationError(window.MetaFlowController.operationErrorText(result, fallback));
      return result;
    }
  };

  const change = (patch) => {
    applySchemaIntent({ intent: "schema.update", schemaId: schema.id, patch });
  };

  const renameField = (fieldId, oldName, newName) => {
    applySchemaIntent({ intent: "schema.renameField", schemaId: schema.id, fieldId, oldName, newName });
  };

  const updateField = (fieldId, patch) => {
    applySchemaIntent({ intent: "schema.updateField", schemaId: schema.id, fieldId, patch });
  };

  const deleteField = (fieldId) => {
    applySchemaIntent({ intent: "schema.deleteField", schemaId: schema.id, fieldId });
  };

  const addField = () => {
    if (!applyAgentIntent) {
      setFieldAddResult({ ok: false, error: schemaState.authoringOperationUnavailableError });
      return;
    }
    setFieldAddResult(null);
    applyAgentIntent({
      intent: "schema.addField",
      schemaId: schema.id,
      selection: { kind: "schema", id: schema.id },
    }).then((result) => {
      if (result?.ok === false) {
        setFieldAddResult(result);
        return;
      }
      setFieldAddResult(null);
    }).catch((error) => {
      setFieldAddResult({
        ok: false,
        error: error?.message || String(error || schemaState.fieldAddFallbackError),
      });
    });
  };

  const deleteSchema = async () => {
    const selection = { kind: null, id: null };
    const result = await applySchemaIntent({ intent: "schema.delete", schemaId: schema.id }, selection);
    if (result?.ok === false) return;
    setAgentSel(result?.selection || selection);
  };

  const renameSchema = async (newId) => {
    const result = window.MetaFlowController.renameSchemaDefinition({
      schemas: studio.schemas,
      members: studio.members,
      flow,
    }, schema.id, newId);
    if (!result.renamed) return;
    const operationResult = await applySchemaIntent({ intent: "schema.rename", schemaId: schema.id, newId }, result.selection);
    if (operationResult?.ok === false) return;
    setAgentSel(operationResult?.selection || result.selection);
  };

  return (
    <div className="agent-editor">
      <div className="agent-editor__head">
        <div className="row row--between">
          <div>
            <div className="inspector__eyebrow">{schemaState.eyebrow}</div>
            <input
              className="agent-editor__title-input"
              defaultValue={schema.id}
              onBlur={e => renameSchema(e.target.value)}
              onKeyDown={e => { if (e.key === "Enter") e.target.blur(); }}
            />
            <div className="inspector__id">{schemaState.usageLabel}</div>
            {schemaOperationError && <div className="hint__line" style={{ color: "var(--danger)" }}>{schemaOperationError}</div>}
          </div>
          <button
            className="btn btn--ghost btn--sm"
            disabled={!schemaState.canDelete}
            title={schemaState.deleteTitle}
            onClick={deleteSchema}
          >{schemaState.deleteLabel}</button>
        </div>
      </div>

      <div className="agent-editor__body">
        <div className="section">
          <div className="section__title">{schemaState.descriptionTitle}</div>
          <textarea
            className="field__textarea"
            rows={2}
            value={schema.description || ""}
            placeholder={schemaState.descriptionPlaceholder}
            onChange={e => change(window.MetaFlowController.schemaDescriptionPatch(e.target.value))}
          />
        </div>

        <div className="section">
          <div className="row row--between" style={{ marginBottom: 6 }}>
            <div className="section__title">{schemaState.fieldsTitle}</div>
            <button className="btn btn--ghost btn--sm" onClick={addField}>{schemaState.addFieldLabel}</button>
          </div>
          {fieldAddErrorState.hasError && <div className="hint__line">{fieldAddErrorState.text}</div>}

          <div className="schema-builder">
            <div className="schema-builder__header">
              <span className="sb-col sb-col--name">{schemaState.headerLabels.name}</span>
              <span className="sb-col sb-col--type">{schemaState.headerLabels.type}</span>
              <span className="sb-col sb-col--req">{schemaState.headerLabels.required}</span>
              <span className="sb-col sb-col--desc">{schemaState.headerLabels.description}</span>
              <span className="sb-col sb-col--act">{schemaState.headerLabels.action}</span>
            </div>
            {schemaState.fieldRows.map(({ field: f }) => (
              <SchemaField
                key={f.id}
                field={f}
                normalizeName={(raw) => window.MetaFlowController.uniqueSchemaFieldName(schema.fields, raw, f.id)}
                onChange={(patch) => updateField(f.id, patch)}
                onRename={(oldName, newName) => renameField(f.id, oldName, newName)}
                onDelete={() => deleteField(f.id)}
                contract={contract}
                schemaView={schemaView}
              />
            ))}
            {schemaState.fieldRows.length === 0 && (
              <div className="schema-builder__empty">{schemaState.emptyFieldsHint}</div>
            )}
          </div>
        </div>

        <div className="section">
          <div className="section__title">{schemaState.usedByTitle}</div>
          {schemaState.usedCount === 0 && <div className="hint__line">{schemaState.emptyUsedByHint}</div>}
          {schemaState.usedBy.map(row => (
            <button
              key={row.id}
              className="usage-row"
              onClick={() => setAgentSel(row.selection)}
            >
              <span className="usage-row__label">{row.name}</span>
              <span className="usage-row__cell">{row.role}</span>
              <span className="usage-row__lane">{row.model}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

function SchemaEnumValueChip({ field, value, index, onChange }: any) {
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
      <button
        className="chip__x"
        onClick={() => onChange(window.MetaFlowController.enumValueDeletePatch(field, index))}
      >×</button>
    </span>
  );
}

function SchemaField({ field, normalizeName, onChange, onRename, onDelete, contract, schemaView = null }: any) {
  const nameBeforeEdit = React.useRef(field.name);
  const [draftName, setDraftName] = React.useState(field.name || "");
  React.useEffect(() => {
    setDraftName(field.name || "");
  }, [field.id, field.name]);
  const fieldState = window.MetaFlowController.schemaFieldRowControlState(field, contract, schemaView);
  const typeState = fieldState.typeState;
  const values = fieldState.enumValues;

  return (
    <div className="schema-field">
      <input
        className="sb-input sb-col--name"
        value={draftName}
        onFocus={() => { nameBeforeEdit.current = field.name; }}
        onChange={e => setDraftName(e.target.value)}
        onBlur={e => {
          const normalized = normalizeName(e.target.value);
          const previous = String(nameBeforeEdit.current || "").trim();
          setDraftName(normalized);
          if (previous && previous !== normalized && onRename) {
            onRename(previous, normalized);
            return;
          }
          if (!onRename && previous !== normalized) onChange({ name: normalized });
        }}
        placeholder={fieldState.namePlaceholder}
      />
      <select
        className="sb-select sb-col--type"
        value={typeState.type}
        onChange={e => {
          onChange(window.MetaFlowController.schemaLikeFieldTypePatch(field, e.target.value, contract));
        }}
      >
        {typeState.typeOptions.map(option => (
          <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
        ))}
      </select>
      {typeState.selectedType?.reason && <div className="hint__line" style={{ color: "var(--warn)" }}>{typeState.selectedType.reason}</div>}
      <label className="sb-col--req sb-checkbox">
        <input
          type="checkbox"
          checked={!!field.required}
          onChange={e => onChange(window.MetaFlowController.schemaLikeFieldRequiredPatch(e.target.checked))}
        />
      </label>
      <input
        className="sb-input sb-col--desc"
        value={field.description || ""}
        onChange={e => onChange(window.MetaFlowController.schemaLikeFieldDescriptionPatch(e.target.value))}
        placeholder={fieldState.descriptionPlaceholder}
      />
      <button className="sb-del" onClick={onDelete} title={fieldState.removeTitle}>×</button>
      {field.type === "enum" && (
        <div className="sb-enum">
          <span className="sb-enum__label">{fieldState.enumLabel}</span>
          <div className="sb-enum__chips">
            {values.map((v, i) => (
              <SchemaEnumValueChip key={i} field={field} value={v} index={i} onChange={onChange} />
            ))}
            <button
              className="chip chip--add"
              onClick={() => onChange(window.MetaFlowController.enumValueAddPatch(field, fieldState.enumAddValue))}
            >{fieldState.enumAddLabel}</button>
          </div>
        </div>
      )}
    </div>
  );
}

function ProviderParamsEditor({ member, change, agentDetailView = null }) {
  const paramsState = window.MetaFlowController.memberProviderParamsEditorState(member, agentDetailView);
  const [draft, setDraft] = React.useState(paramsState.text);
  const [error, setError] = React.useState("");
  React.useEffect(() => {
    setDraft(paramsState.text);
    setError("");
  }, [member.id, paramsState.text]);
  const commit = (next) => {
    setDraft(next);
    const result = window.MetaFlowController.memberProviderParamsPatch(next, agentDetailView);
    if (!result.ok) {
      setError(result.error || paramsState.invalidJsonLabel);
      return;
    }
    setError("");
    change(result.patch);
  };
  return (
    <div className="field">
      <label className="field__label">{paramsState.label}</label>
      <textarea
        className="field__textarea field__textarea--mono"
        rows={paramsState.rows}
        value={draft}
        placeholder={paramsState.placeholder}
        onChange={e => commit(e.target.value)}
      />
      {error && <div className="hint__line" style={{ color: "var(--danger)" }}>{error}</div>}
    </div>
  );
}

// ── Skill access (realm picker + per-skill toggles, baked into the pack) ──
function SkillAccess({ studio, member, agentAccessView = null, applyAgentIntent = null }) {
  const realms = studio.skillRealms || [];
  const initialSkillState = window.MetaFlowController.memberSkillAccessState({ member, skillRealms: realms, accessView: agentAccessView });
  const [realmId, setRealmId] = React.useState(initialSkillState.realmId);
  const [inlineOpen, setInlineOpen] = React.useState(false);
  const [inlineLabel, setInlineLabel] = React.useState("");
  const [inlineContent, setInlineContent] = React.useState("");
  const [inlineError, setInlineError] = React.useState("");
  const skillState = window.MetaFlowController.memberSkillAccessState({ member, skillRealms: realms, realmId, inlineOpen, accessView: agentAccessView });
  React.useEffect(() => {
    if (skillState.realmId !== realmId) setRealmId(skillState.realmId);
  }, [skillState.realmId, realmId]);
  const applySkillIntent = async (intentRequest, fallback = skillState.inlineErrorFallback) => {
    if (!applyAgentIntent) {
      setInlineError(fallback);
      return null;
    }
    try {
      const result = await applyAgentIntent({ memberId: member.id, ...intentRequest });
      if (!result?.ok) {
        setInlineError(window.MetaFlowController.operationErrorText(result, fallback));
        return null;
      }
      setInlineError("");
      return result;
    } catch (err) {
      setInlineError(err?.message || fallback);
      return null;
    }
  };
  const toggle = (sid) => {
    applySkillIntent({ intent: "agent.toggleSkill", skillId: sid });
  };
  const removeSkill = (sid) => {
    applySkillIntent({ intent: "agent.removeSkill", skillId: sid });
  };
  const addInlineSkill = async () => {
    const result = await applySkillIntent({
      intent: "agent.createInlineSkill",
      label: inlineLabel,
      content: inlineContent,
    }, skillState.inlineErrorFallback);
    if (result) {
      const nextRealmId = window.MetaFlowController.inlineSkillRealmIdFromOperationResult(result);
      if (nextRealmId) setRealmId(nextRealmId);
      setInlineLabel("");
      setInlineContent("");
      setInlineError("");
      setInlineOpen(false);
    }
  };
  return (
    <>
      <div className="section__title section__title--row">
        <span>{skillState.sectionTitle}</span>
        <button className="ghost-btn" onClick={() => setInlineOpen(open => !open)}>
          {skillState.inlineToggleLabel}
        </button>
      </div>
      <div className="hint__line" style={{ marginBottom: 8 }}>
        {skillState.hint}
      </div>
      {inlineOpen && (
        <div className="inline-skill">
          <input
            className="field__input"
            value={inlineLabel}
            placeholder={skillState.inlineLabelPlaceholder}
            onChange={e => { setInlineLabel(e.target.value); setInlineError(""); }}
          />
          <textarea
            className="field__textarea field__textarea--mono"
            rows={skillState.inlineContentRows}
            value={inlineContent}
            placeholder={skillState.inlineContentPlaceholder}
            onChange={e => { setInlineContent(e.target.value); setInlineError(""); }}
          />
          <div className="row row--between">
            <span className="hint__line">{skillState.inlineCreateHint}</span>
            <button className="btn btn--ghost btn--sm" onClick={addInlineSkill}>{skillState.inlineAddLabel}</button>
          </div>
          {inlineError && <div className="hint__line" style={{ color: "var(--danger)" }}>{inlineError}</div>}
        </div>
      )}
      {!skillState.hasRealms ? (
        <div className="hint__line" style={{ color: "var(--warn)" }}>
          {skillState.noRealmsMessage}
        </div>
      ) : (
        <>
          <div className="field">
            <label className="field__label">{skillState.realmLabel}</label>
            <select className="field__select" value={skillState.realmId} onChange={e => setRealmId(e.target.value)}>
              {skillState.realmOptions.map(realm => <option key={realm.id} value={realm.id}>{realm.label}</option>)}
            </select>
          </div>
          <div className="skill-list">
            {skillState.skillRows.map(row => {
              return (
                <button key={row.id} className={row.className} onClick={() => toggle(row.id)}>
                  <span className="skill-row__check">{row.checkLabel}</span>
                  <span className="skill-row__text">
                    <span className="skill-row__name">{row.name}</span>
                    <span className="skill-row__desc">{row.desc}</span>
                  </span>
                </button>
              );
            })}
          </div>
        </>
      )}
      {skillState.selectedOutsideRealm.length > 0 && (
        <div className="skill-other">
          <span className="hint__line">{skillState.outsideRealmHeading}</span>
          {skillState.selectedOutsideRealm.map(skill => (
            <span key={skill.id} className={skill.className} title={skill.title}>
              {skill.label}
              <em>{skill.detail}</em>
              <button onClick={() => removeSkill(skill.id)}>{skill.removeLabel}</button>
            </span>
          ))}
        </div>
      )}
      {skillState.unavailableSelected.length > 0 && (
        <div className="skill-other">
          <span className="hint__line" style={{ color: "var(--warn)" }}>{skillState.unavailableHeading}</span>
          {skillState.unavailableSelected.map(sid => (
            <span key={sid.id} className={sid.className}>
              {sid.label}
              <button onClick={() => removeSkill(sid.id)}>{sid.removeLabel}</button>
            </span>
          ))}
        </div>
      )}
    </>
  );
}
