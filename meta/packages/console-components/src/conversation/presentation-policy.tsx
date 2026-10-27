import { createContext, useCallback, useContext, useEffect, useMemo, useReducer, useRef, useState, type ReactNode } from "react";
import type React from "react";
import type { ConversationRichBlock, ConversationRichToolCallBlock } from "@console-core";
import type { ConversationViewportKey } from "./scroll-controller";

export type ConversationDisplayLabels = {
  tools?: ReadonlyMap<string, string>;
  /** Populate only with already-authorized roster entries, keyed by exact ID. */
  peers?: ReadonlyMap<string, string>;
};
export function explicitDisplayLabel(id: string, labels?: ReadonlyMap<string, string>): string {
  return labels?.get(id)?.trim() || id;
}
export function peerDisplayLabel(block: ConversationRichToolCallBlock, labels?: ReadonlyMap<string, string>): string {
  const hostLabel = block.peerIdentity ? labels?.get(block.peerIdentity)?.trim() : undefined;
  return hostLabel || block.peerDisplayLabel?.trim() || block.peerIdentity || block.peerTarget || "Unknown peer";
}
export const ROUTINE_TOOL_NAMES: ReadonlySet<string> = new Set(["read_file", "list_files", "search", "glob", "grep", "ls", "read", "file_read", "directory_list"]);
export function isCompletedRoutineTool(block: ConversationRichBlock): block is ConversationRichToolCallBlock {
  return block.type === "tool-call"
    && ROUTINE_TOOL_NAMES.has(block.name)
    && !block.peerIncoming && !block.peerTarget && !block.peerIdentity && !block.peerDisplayLabel && !block.peerBody
    && block.status === "success"
    && block.completionEvidence?.outcome === "success"
    && block.completionEvidence.toolCallId === block.toolCallId
    && (block.completionEvidence.source === "runtime-result" || block.completionEvidence.source === "session-history");
}
export function canFoldCompletedTools(blocks: readonly ConversationRichBlock[]): blocks is ConversationRichToolCallBlock[] {
  return blocks.length >= 2 && blocks.every(isCompletedRoutineTool);
}

/** Group only adjacent eligible records, without rewriting their contents or IDs. */
export function groupRoutineToolRows<T>(rows: readonly T[], blocksFor: (row: T) => readonly ConversationRichBlock[] | undefined): { rows: T[]; tools: ConversationRichToolCallBlock[] }[] {
  const groups: { rows: T[]; tools: ConversationRichToolCallBlock[] }[] = [];
  for (const row of rows) {
    const blocks = blocksFor(row);
    const eligible = Boolean(blocks?.length && blocks.every(isCompletedRoutineTool));
    const previous = groups.at(-1);
    if (eligible && previous?.tools.length) {
      previous.rows.push(row); previous.tools.push(...blocks as ConversationRichToolCallBlock[]);
    } else {
      groups.push({ rows: [row], tools: eligible ? [...blocks!] as ConversationRichToolCallBlock[] : [] });
    }
  }
  return groups;
}

const scopes = new Map<string, Map<string, unknown>>();
function scopeState(key: string): Map<string, unknown> {
  const state = scopes.get(key) ?? new Map<string, unknown>();
  scopes.delete(key); scopes.set(key, state);
  if (scopes.size > 100) scopes.delete(scopes.keys().next().value!);
  return state;
}
/// Reader UI state per pane: open disclosures, expanded cards. A row can
/// unmount and mount again (a windowed transcript) without changing what it
/// shows, because its parts keep their state here instead of in the DOM or
/// in component state. Bounded, least recently used first out.
const ROW_STATE_LIMIT = 2_000;
function rememberRowState(store: Map<string, unknown>, key: string, value: unknown): void {
  store.delete(key);
  store.set(key, value);
  if (store.size > ROW_STATE_LIMIT) store.delete(store.keys().next().value!);
}
const PresentationContext = createContext<{ labels?: ConversationDisplayLabels; disclosures: Map<string, unknown>; autoFold: boolean } | null>(null);
const RowScopeContext = createContext<string | null>(null);

/** Names the row whose parts call `useRowState`: its stable row id. */
export function ConversationRowStateScope({ rowId, children }: { rowId: string; children: ReactNode }) {
  return <RowScopeContext.Provider value={rowId}>{children}</RowScopeContext.Provider>;
}

/**
 * State for one part of a rendered row, such as a disclosure. Inside a row
 * scope and a presentation provider it lives in the pane's store, keyed by
 * row id and `part`, and the first render records `initial()`, so the part
 * shows the same thing when its row mounts again. Elsewhere it is
 * component-local, as before.
 */
export function useRowState<T>(part: string, initial: () => T): [T, (next: T) => void] {
  const context = useContext(PresentationContext);
  const row = useContext(RowScopeContext);
  const [, rerender] = useReducer((count: number) => count + 1, 0);
  const local = useRef<{ value: T } | null>(null);
  const store = context?.disclosures;
  const key = store && row !== null ? JSON.stringify(["row", row, part]) : null;
  let value: T;
  if (store && key !== null) {
    if (!store.has(key)) rememberRowState(store, key, initial());
    value = store.get(key) as T;
  } else {
    if (!local.current) local.current = { value: initial() };
    value = local.current.value;
  }
  const set = useCallback((next: T) => {
    if (store && key !== null) rememberRowState(store, key, next);
    else local.current = { value: next };
    rerender();
  }, [store, key]);
  return [value, set];
}
const FoldedToolsContext = createContext(false);
export function useInsideCompletedToolDisclosure() { return useContext(FoldedToolsContext); }
export function ConversationPresentationProvider({ labels, viewportKey, autoFold = true, children }: { labels?: ConversationDisplayLabels; viewportKey?: ConversationViewportKey; autoFold?: boolean; children: ReactNode }) {
  const local = useRef(new Map<string, unknown>());
  const oldAuthority = useRef(viewportKey?.authority);
  const key = viewportKey ? JSON.stringify([viewportKey.authority, viewportKey.identity, viewportKey.conversation, viewportKey.pane]) : null;
  const disclosures = useMemo(() => key ? scopeState(key) : local.current, [key]);
  useEffect(() => {
    if (oldAuthority.current !== viewportKey?.authority) {
      for (const existing of scopes.keys()) if (JSON.parse(existing)[0] === oldAuthority.current) scopes.delete(existing);
      local.current.clear();
      oldAuthority.current = viewportKey?.authority;
    }
  }, [viewportKey?.authority]);
  // A fresh value object would re-render every consumer row on every render.
  const value = useMemo(() => ({ labels, disclosures, autoFold }), [labels, disclosures, autoFold]);
  return <PresentationContext.Provider value={value}>{children}</PresentationContext.Provider>;
}
export function useConversationDisplayLabels() { return useContext(PresentationContext)?.labels; }

/** A `<details>` whose open state is row state (see `useRowState`). */
export function RowDetails({ part, initiallyOpen = false, children, ...props }: {
  part: string;
  initiallyOpen?: boolean;
  children: ReactNode;
} & Omit<React.DetailsHTMLAttributes<HTMLDetailsElement>, "open" | "onToggle" | "children">) {
  const [open, setOpen] = useRowState(part, () => initiallyOpen);
  return <details {...props} open={open} onToggle={(event) => {
    if (event.currentTarget.open !== open) setOpen(event.currentTarget.open);
  }}>{children}</details>;
}

/** A layout disclosure only: the full underlying records and copy actions remain. */
export function CompletedToolDisclosure({ blocks, children }: { blocks: ConversationRichToolCallBlock[]; children: ReactNode }) {
  const context = useContext(PresentationContext);
  const local = useRef(new Map<string, unknown>());
  const disclosures = context?.disclosures ?? local.current;
  const key = JSON.stringify(blocks.map((block) => block.toolCallId));
  const initiallyOpen = (disclosures.get(key) as boolean | undefined) ?? context?.autoFold === false;
  // Whether it starts folded depends on the scroll mode when it first
  // renders; record that, so mounting it again later shows the same.
  if (context && !disclosures.has(key)) rememberRowState(disclosures, key, initiallyOpen);
  const [state, setState] = useState(() => ({ disclosures, key, open: initiallyOpen }));
  const open = state.disclosures === disclosures && state.key === key ? state.open : initiallyOpen;
  return <details className="cc-completed-tools" open={open} onToggle={(event) => {
    const next = event.currentTarget.open;
    setState({ disclosures, key, open: next });
    rememberRowState(disclosures, key, next);
  }}>
    <summary>{blocks.length} completed tool calls</summary>
    <FoldedToolsContext.Provider value={true}><div className="cc-completed-tools__body">{children}</div></FoldedToolsContext.Provider>
  </details>;
}

export type ConversationHeaderProps = { title: string; identity: string; detail?: string | null; actions?: ReactNode; variant?: "full" | "compact" };
export function ConversationHeader({ title, identity, detail, actions, variant = "full" }: ConversationHeaderProps) {
  return <header className={`cc-conversation-header cc-conversation-header--${variant}`}>
    <div className="cc-conversation-header__target" title={identity}>
      <span className="cc-conversation-header__title">{title}</span>
      {variant === "full" ? <span className="cc-conversation-header__identity">{identity}{detail ? ` · ${detail}` : ""}</span> : null}
    </div>
    {actions ? <div className="cc-conversation-header__actions">{actions}</div> : null}
  </header>;
}
