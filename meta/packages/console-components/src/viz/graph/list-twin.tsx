// The list twin of a graph (design spec A3.9): every node as a row with its
// relations in words, sortable by name, group, connections and messages. It
// is the screen reader path and the panels' List view; above 200 rows it is
// windowed (A4.9).

import React from "react";

import { isCardKind } from "../layout/geometry";
import { compareLabels } from "../layout/types";
import { MarkShape, StatusSquareShape, itemStatusOf } from "./marks";
import type { VizEdge, VizLane, VizNode, VizRelation } from "./types";
import { nodeStatusWord, vizRelationWord } from "./words";

export interface GraphListConnection {
  peer: VizNode;
  edge: VizEdge;
  relation: VizRelation;
  direction: "out" | "in" | "both";
}

export interface GraphListRow {
  node: VizNode;
  laneLabel?: string;
  /** Edges at this node. */
  degree: number;
  /** Messages in the window: the traffic weights at this node. */
  messages: number;
  selected: boolean;
  connections: GraphListConnection[];
}

export type GraphListSort = "name" | "group" | "degree" | "messages";

export interface GraphListProps {
  nodes: readonly VizNode[];
  edges: readonly VizEdge[];
  lanes?: readonly VizLane[];
  selection?: readonly string[];
  onSelect?: (ids: string[]) => void;
  onOpen?: (id: string) => void;
  sortBy?: GraphListSort;
  onSortChange?: (sort: GraphListSort) => void;
  showRetired?: boolean;
  /** Renders a row's content (K1's EntityRow); the built-in row otherwise. */
  renderRow?: (row: GraphListRow) => React.ReactNode;
  ariaLabel: string;
  testIds?: { root?: string; row?: string | ((node: VizNode) => string) };
  /** Derives <prefix>-list and <prefix>-list-row:<id>. */
  testIdPrefix?: string;
  className?: string;
  /** Row height in px for windowing (comfortable two-line rows). */
  rowHeight?: number;
}

export const LIST_WINDOW_THRESHOLD = 200;
const OVERSCAN = 8;

function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

/** The rows of the list twin, sorted. Pure, for tests and custom lists. */
export function graphListRows(
  nodes: readonly VizNode[],
  edges: readonly VizEdge[],
  options: { lanes?: readonly VizLane[]; selection?: readonly string[]; sortBy?: GraphListSort; showRetired?: boolean } = {},
): GraphListRow[] {
  const visible = nodes.filter((node) => options.showRetired || !node.retired);
  const byId = new Map(visible.map((node) => [node.id, node]));
  const laneIndex = new Map((options.lanes ?? []).map((lane, index) => [lane.id, index]));
  const laneLabel = new Map((options.lanes ?? []).map((lane) => [lane.id, lane.label]));
  const selected = new Set(options.selection ?? []);
  const connections = new Map<string, GraphListConnection[]>();
  const messages = new Map<string, number>();
  for (const node of visible) {
    connections.set(node.id, []);
    messages.set(node.id, 0);
  }
  for (const edge of edges) {
    const from = byId.get(edge.from);
    const to = byId.get(edge.to);
    if (!from || !to || edge.from === edge.to) continue;
    const undirected = edge.relation === "wired" || edge.relation === "cross" || edge.relation === "related";
    connections.get(from.id)!.push({ peer: to, edge, relation: edge.relation, direction: undirected ? "both" : "out" });
    connections.get(to.id)!.push({ peer: from, edge, relation: edge.relation, direction: undirected ? "both" : "in" });
    if (edge.relation === "traffic") {
      const weight = edge.weight ?? 1;
      messages.set(from.id, (messages.get(from.id) ?? 0) + weight);
      messages.set(to.id, (messages.get(to.id) ?? 0) + weight);
    }
  }
  const rows: GraphListRow[] = visible.map((node) => ({
    node,
    laneLabel: node.lane !== undefined ? laneLabel.get(node.lane) : undefined,
    degree: connections.get(node.id)!.length,
    messages: messages.get(node.id) ?? 0,
    selected: selected.has(node.id),
    connections: connections.get(node.id)!,
  }));
  const byName = (a: GraphListRow, b: GraphListRow) => compareLabels(a.node.label, b.node.label) || (a.node.id < b.node.id ? -1 : 1);
  const sort = options.sortBy ?? "group";
  rows.sort((a, b) => {
    if (sort === "degree") return b.degree - a.degree || byName(a, b);
    if (sort === "messages") return b.messages - a.messages || byName(a, b);
    if (sort === "group") {
      const ga = a.node.lane !== undefined ? laneIndex.get(a.node.lane) ?? Number.MAX_SAFE_INTEGER : Number.MAX_SAFE_INTEGER;
      const gb = b.node.lane !== undefined ? laneIndex.get(b.node.lane) ?? Number.MAX_SAFE_INTEGER : Number.MAX_SAFE_INTEGER;
      if (ga !== gb) return ga - gb;
      const oa = a.node.order ?? Number.POSITIVE_INFINITY;
      const ob = b.node.order ?? Number.POSITIVE_INFINITY;
      if (oa !== ob) return oa < ob ? -1 : 1;
    }
    return byName(a, b);
  });
  return rows;
}

/** "Router · wired, Support · 12 messages, +2": the relations of a row in words. */
export function connectionsSummary(row: GraphListRow, limit = 3): string {
  if (row.connections.length === 0) return "No connections";
  const parts = row.connections.slice(0, limit).map((connection) => {
    const word = connection.relation === "traffic" && connection.edge.weight
      ? `${connection.edge.weight} ${connection.edge.weight === 1 ? "message" : "messages"}`
      : vizRelationWord(connection.relation);
    return `${connection.peer.label} · ${word}`;
  });
  const more = row.connections.length - limit;
  return more > 0 ? `${parts.join(", ")}, +${more}` : parts.join(", ");
}

function RowMark({ node }: { node: VizNode }): React.JSX.Element {
  if (isCardKind(node.kind)) {
    return (
      <svg className="cc-viz-list__mark" viewBox="-9 -9 18 18" width="18" height="18" style={{ width: 18, height: 18 }} aria-hidden="true" focusable="false">
        <StatusSquareShape status={itemStatusOf(node.status)} live={Boolean(node.live)} />
      </svg>
    );
  }
  return (
    <svg className="cc-viz-list__mark" viewBox="-9 -9 18 18" width="18" height="18" style={{ width: 18, height: 18 }} aria-hidden="true" focusable="false">
      <g className={cx("cc-viz-node", `cc-viz-node--${node.kind}`, (node.live || node.tone === "live") && "is-live is-hot", node.retired && "is-retired")}>
        <MarkShape kind={node.kind} r={node.kind === "store" ? 6 : node.kind === "coordinator" ? 4.5 : 5} />
      </g>
    </svg>
  );
}

const SORT_LABELS: Record<GraphListSort, string> = { name: "Name", group: "Group", degree: "Connections", messages: "Messages" };

export function GraphList(props: GraphListProps): React.JSX.Element {
  const [ownSort, setOwnSort] = React.useState<GraphListSort>("group");
  const sortBy = props.sortBy ?? ownSort;
  const rows = React.useMemo(
    () => graphListRows(props.nodes, props.edges, { lanes: props.lanes, selection: props.selection, sortBy, showRetired: props.showRetired }),
    [props.nodes, props.edges, props.lanes, props.selection, sortBy, props.showRetired],
  );
  const rowHeight = props.rowHeight ?? 48;
  const windowed = rows.length > LIST_WINDOW_THRESHOLD;
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  const [range, setRange] = React.useState({ start: 0, end: Math.min(rows.length, 40) });
  React.useLayoutEffect(() => {
    if (!windowed) return undefined;
    const element = scrollRef.current;
    if (!element) return undefined;
    const update = () => {
      const top = element.scrollTop;
      const height = element.clientHeight || 600;
      const start = Math.max(0, Math.floor(top / rowHeight) - OVERSCAN);
      const end = Math.min(rows.length, Math.ceil((top + height) / rowHeight) + OVERSCAN);
      setRange((current) => (current.start === start && current.end === end ? current : { start, end }));
    };
    update();
    element.addEventListener("scroll", update, { passive: true });
    return () => element.removeEventListener("scroll", update);
  }, [windowed, rows.length, rowHeight]);
  const start = windowed ? range.start : 0;
  const end = windowed ? Math.min(range.end, rows.length) : rows.length;
  const prefixed = (suffix: string) => (props.testIdPrefix ? `${props.testIdPrefix}-${suffix}` : undefined);
  const rowTestId = (node: VizNode) => {
    const value = props.testIds?.row;
    if (typeof value === "function") return value(node);
    if (typeof value === "string") return value;
    return prefixed(`list-row:${node.id}`);
  };
  const setSort = (sort: GraphListSort) => {
    if (props.sortBy === undefined) setOwnSort(sort);
    props.onSortChange?.(sort);
  };
  const header = (sort: GraphListSort, className: string) => (
    <th scope="col" className={className} aria-sort={sortBy === sort ? (sort === "name" || sort === "group" ? "ascending" : "descending") : "none"}>
      <button type="button" className={cx("cc-viz-list__sort", sortBy === sort && "is-active")} onClick={() => setSort(sort)}>
        {SORT_LABELS[sort]}
      </button>
    </th>
  );
  return (
    <div ref={scrollRef} className={cx("cc-viz-list", props.className)} data-testid={props.testIds?.root ?? prefixed("list")}>
      <table className="cc-viz-list__table" aria-label={props.ariaLabel} aria-rowcount={rows.length + 1}>
        <thead>
          <tr aria-rowindex={1}>
            {header("name", "cc-viz-list__name")}
            {header("group", "cc-viz-list__group")}
            <th scope="col" className="cc-viz-list__status">
              Status
            </th>
            {header("degree", "cc-viz-list__connections")}
          </tr>
        </thead>
        <tbody>
          {start > 0 ? <tr className="cc-viz-list__spacer" style={{ height: start * rowHeight }} aria-hidden="true" /> : null}
          {rows.slice(start, end).map((row, offset) => {
            const index = start + offset;
            const { node } = row;
            return (
              <tr
                key={node.id}
                className={cx("cc-viz-list__row", row.selected && "is-selected", (node.live || node.tone === "live") && "is-live", node.retired && "is-retired")}
                aria-rowindex={index + 2}
                aria-selected={row.selected}
                data-testid={rowTestId(node)}
                data-node-id={node.id}
                style={windowed ? { height: rowHeight } : undefined}
                onDoubleClick={() => props.onOpen?.(node.id)}
              >
                {props.renderRow ? (
                  <td colSpan={4}>{props.renderRow(row)}</td>
                ) : (
                  <>
                    <td className="cc-viz-list__name">
                      <div className="cc-viz-list__cell">
                      <RowMark node={node} />
                      <span className="cc-viz-list__titles">
                        <button type="button" className="cc-viz-list__select" aria-pressed={row.selected} onClick={() => props.onSelect?.(row.selected ? [] : [node.id])}>
                          {node.label}
                        </button>
                        {node.sublabel ? <span className="cc-viz-list__sub">{node.sublabel}</span> : null}
                      </span>
                      {props.onOpen ? (
                        <button type="button" className="cc-viz-list__open" aria-label={`Open ${node.label}`} onClick={() => props.onOpen?.(node.id)}>
                          Open
                        </button>
                      ) : null}
                      </div>
                    </td>
                    <td className="cc-viz-list__group">{row.laneLabel ?? ""}</td>
                    <td className="cc-viz-list__status">
                      {nodeStatusWord(node) ? <span className="cc-viz-tone" data-tone={node.live ? "live" : node.tone ?? "neutral"} aria-hidden="true" /> : null}
                      {nodeStatusWord(node)}
                    </td>
                    <td className="cc-viz-list__connections">
                      <span className="cc-viz-list__count">{row.degree}</span>
                      <span className="cc-viz-list__peers">{connectionsSummary(row)}</span>
                    </td>
                  </>
                )}
              </tr>
            );
          })}
          {end < rows.length ? <tr className="cc-viz-list__spacer" style={{ height: (rows.length - end) * rowHeight }} aria-hidden="true" /> : null}
        </tbody>
      </table>
    </div>
  );
}
