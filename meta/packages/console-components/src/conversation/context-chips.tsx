import { useEffect, useId, useRef, useState } from "react";
import type { ConsoleContextRecord } from "../../../console-core/src/context-record";
import { editConsoleContextQuote } from "../../../console-core/src/context-edit";
import { RowDetails } from "./presentation-policy";

export interface QuoteContextChipsProps {
  records: readonly ConsoleContextRecord[];
  destinationLabel: string;
  onRemove?: (id: string) => void;
  onReorder?: (id: string, direction: "up" | "down") => void;
  onEdit?: (id: string, quote: string) => void | Promise<void>;
}

function QuoteActionIcon({ action }: { action: "up" | "down" | "edit" | "remove" }) {
  return <svg viewBox="0 0 20 20" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {action === "edit" ? <><path d="m12.8 3.2 4 4-9 9-5 1 1-5z" /><path d="m10.8 5.2 4 4" /></>
      : action === "remove" ? <path d="m5 5 10 10M15 5 5 15" />
        : action === "up" ? <path d="M10 16V4m-5 5 5-5 5 5" /> : <path d="M10 4v12m-5-5 5 5 5-5" />}
  </svg>;
}

function QuoteContextChip({ record, index, records, onEdit, onRemove, onReorder }: Omit<QuoteContextChipsProps, "destinationLabel"> & { record: ConsoleContextRecord; index: number }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(record.quote);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const editButtonRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef(false);
  const errorId = useId();
  useEffect(() => { if (!onEdit) { setEditing(false); setError(""); } }, [onEdit]);
  useEffect(() => {
    if (editing) editorRef.current?.focus();
    else if (restoreFocusRef.current) { restoreFocusRef.current = false; editButtonRef.current?.focus(); }
  }, [editing]);
  function cancel() { restoreFocusRef.current = true; setEditing(false); setError(""); }
  async function save() {
    if (!onEdit || saving) return;
    try {
      editConsoleContextQuote(records, record.id, draft);
      setSaving(true); setError("");
      await onEdit(record.id, draft);
      cancel();
    } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setSaving(false); }
  }
  return <li className={`cc-context-chip${editing && onEdit ? " cc-context-chip--editing" : ""}`}>
    {editing && onEdit ? <div className="cc-context-chip__editor">
      <label>{record.label}<textarea ref={editorRef} aria-label={`Quote from ${record.label}`} value={draft} rows={4}
        aria-invalid={!!error} aria-describedby={error ? errorId : undefined} disabled={saving}
        onChange={event => { setDraft(event.target.value); setError(""); }}
        onKeyDown={event => { event.stopPropagation(); if (event.key === "Escape" && !saving) { event.preventDefault(); cancel(); } }} /></label>
      {error ? <p id={errorId} role="alert">{error}</p> : null}
      <div className="cc-context-chip__edit-actions">
        <button type="button" onClick={() => void save()} disabled={saving}>Save quote</button>
        <button type="button" onClick={cancel} disabled={saving} aria-label="Cancel quote edit">Cancel</button>
      </div>
    </div> : <>
      <RowDetails part={`quote:${record.id}`}>
        <summary>{record.label}</summary>
        <blockquote>{record.quote}</blockquote>
        <small>User-provided snapshot{record.sourceRange ? "" : "; original source range unavailable"}</small>
      </RowDetails>
      <div className="cc-context-chip__actions">
        {onReorder ? <>
          <button type="button" disabled={index === 0} onClick={() => onReorder(record.id, "up")} aria-label={`Move quote from ${record.label} earlier`} title="Move quote earlier"><QuoteActionIcon action="up" /></button>
          <button type="button" disabled={index === records.length - 1} onClick={() => onReorder(record.id, "down")} aria-label={`Move quote from ${record.label} later`} title="Move quote later"><QuoteActionIcon action="down" /></button>
        </> : null}
        {onEdit ? <button type="button" ref={editButtonRef} onClick={() => { setDraft(record.quote); setError(""); setEditing(true); }} aria-label={`Edit quote from ${record.label}`} title="Edit quote"><QuoteActionIcon action="edit" /></button> : null}
        {onRemove ? <button type="button" onClick={() => onRemove(record.id)} aria-label={`Remove quote from ${record.label}`} title="Remove quote"><QuoteActionIcon action="remove" /></button> : null}
      </div>
    </>}
  </li>;
}

export function QuoteContextChips({ records, destinationLabel, ...actions }: QuoteContextChipsProps) {
  if (!records.length) return null;
  return <section className="cc-context-chips" aria-label={`Quoted context for ${destinationLabel}`}>
    <span className="cc-context-chips__destination">Quoted context for {destinationLabel}</span>
    <ul>{records.map((record, index) => <QuoteContextChip key={record.id} record={record} index={index} records={records} {...actions} />)}</ul>
  </section>;
}
