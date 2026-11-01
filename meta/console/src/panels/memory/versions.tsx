// The VERSIONS picture of a record biography: earlier versions to the open
// one, left to right, with the records merged into it (`derived_from`)
// joining in. Drawn by the kit's GraphView (layered, A3.9 grammar): an ink
// mark for the active version, hollow marks for replaced ones, hollow
// squares for merged sources, the accent ring on the version the drawer
// shows. The list twin is the version rows under it.

import React from "react";

import type { MemoryPanelRecord } from "../../types";
import { GraphView } from "../../../../packages/console-components/src/viz";
import type { VizEdge, VizNode } from "../../../../packages/console-components/src/viz";
import { formatRelativeTime, joinMeta, plural } from "./kit";
import { statusWord, trustDisplay } from "./words";

export interface VersionsModel {
  nodes: VizNode[];
  edges: VizEdge[];
  /** Versions in the chain. */
  versions: number;
  /** Records merged into the open one that are not in the chain. */
  sources: number;
  /** Rows the layout needs (the chain, then one per source). */
  rows: number;
}

/** Where a version node opens: the drawer reads a record by realm and id. */
interface VersionData {
  realm: string;
}

const SOURCE_LABEL_MAX = 22;

function shortTitle(title: string): string {
  return title.length > SOURCE_LABEL_MAX ? `${title.slice(0, SOURCE_LABEL_MAX - 1).trimEnd()}...` : title;
}

/**
 * The versions graph's model: the chain (oldest first) on one row, a column
 * per version, and the open record's `derived_from` sources in the column
 * before it, on the rows below, each joining the open record.
 */
export function versionsModel(
  chain: ReadonlyArray<MemoryPanelRecord>,
  openId: string,
  derivedFrom: ReadonlyArray<string>,
  titles: ReadonlyMap<string, string>,
  now: number,
): VersionsModel {
  const nodes: VizNode[] = [];
  const edges: VizEdge[] = [];
  const sources = derivedFrom.filter((id, index) => id !== openId && derivedFrom.indexOf(id) === index && !chain.some((record) => record.id === id));
  const openIndex = Math.max(0, chain.findIndex((record) => record.id === openId));
  // Sources sit one column before the open record; the chain moves right
  // when the open record is the first version.
  const offset = sources.length > 0 && openIndex === 0 ? 1 : 0;
  const realm = chain[openIndex]?.scope.realm ?? chain[0]?.scope.realm ?? "";
  chain.forEach((record, index) => {
    const status = record.status?.status ?? "unknown";
    const open = record.id === openId;
    const version = `v${index + 1}`;
    const word = statusWord(record.status);
    const title = record.title || "Untitled record";
    const at = record.updated_at_ms || record.created_at_ms;
    const when = at ? formatRelativeTime(at, now) : null;
    nodes.push({
      id: record.id,
      kind: status === "active" ? "agent" : "helper",
      // The mark carries the state (ink active, hollow replaced, the accent
      // ring on the open one); the rows under the graph say it in words.
      label: version,
      rank: index + offset,
      order: 0,
      tone: status === "quarantined" ? "warn" : undefined,
      badge: status === "quarantined" ? "warn" : null,
      dim: status === "tombstoned",
      hub: true,
      ariaLabel: joinMeta(`${version}: ${title}`, word, trustDisplay(record.trust), when, open ? "open now" : null),
      title: `${version}: ${title}\n${joinMeta(word, trustDisplay(record.trust), when)}`,
      attributes: { "data-version": version, "data-current": open ? "true" : undefined },
      data: { realm: record.scope.realm } satisfies VersionData,
    });
    if (index > 0) {
      const previous = chain[index - 1];
      edges.push({
        id: `${previous.id}>${record.id}`,
        from: previous.id,
        to: record.id,
        relation: "related",
        directed: true,
        title: `v${index} replaced by ${version}`,
      });
    }
  });
  const sourceRank = Math.max(0, openIndex + offset - 1);
  sources.forEach((id, index) => {
    const title = titles.get(id) ?? "An earlier record";
    nodes.push({
      id,
      kind: "source",
      label: shortTitle(title),
      rank: sourceRank,
      order: index + 1,
      hub: true,
      ariaLabel: `${title}, merged into this memory`,
      title: `${title}\nMerged into this memory`,
      attributes: { "data-version": "source" },
      data: { realm } satisfies VersionData,
    });
    edges.push({
      id: `${id}>${openId}`,
      from: id,
      to: openId,
      relation: "related",
      directed: true,
      title: `${title} merged into this memory`,
    });
  });
  // Sources alone in the first column stack beside the chain's row; sources
  // sharing a version's column stack under it.
  const rows = sources.length === 0 ? 1 : offset === 1 ? sources.length : sources.length + 1;
  return { nodes, edges, versions: chain.length, sources: sources.length, rows };
}

/**
 * The graph's height for its rows: a row of marks is 24 px, rows 12 px
 * apart, and the fit keeps FIT_PADDING clear on each side (mark labels sit
 * to the right of their marks, so no label line hangs below).
 */
const FIT_PADDING = 8;
function graphHeight(rows: number): number {
  return 24 * rows + 12 * Math.max(0, rows - 1) + 2 * FIT_PADDING + 4;
}

/** The small left-to-right versions graph. A click or Enter opens a version. */
export function VersionsGraph({
  model,
  openId,
  onOpen,
  testId,
}: {
  model: VersionsModel;
  openId: string;
  onOpen: (realm: string, id: string) => void;
  testId?: string;
}): React.JSX.Element {
  const realms = React.useMemo(
    () => new Map(model.nodes.map((node) => [node.id, (node.data as VersionData | undefined)?.realm ?? ""])),
    [model.nodes],
  );
  const open = (id: string | undefined) => {
    if (id && id !== openId && realms.has(id)) onOpen(realms.get(id) ?? "", id);
  };
  const selection = React.useMemo(() => [openId], [openId]);
  const summary = joinMeta(
    plural(model.versions, "version"),
    model.sources > 0 ? `${plural(model.sources, "record")} merged in` : null,
  );
  return (
    <div className="memory-versions" data-testid={testId}>
      <GraphView
        nodes={model.nodes}
        edges={model.edges}
        layout="layered"
        ariaLabel={`Versions: ${summary}`}
        testIdPrefix="memory-versions"
        selection={selection}
        onSelect={(ids) => open(ids.find((id) => id !== openId))}
        onOpen={(id) => open(id)}
        legend="off"
        minimap="off"
        controls={false}
        labels="all"
        // The drawer keeps the wheel; a pinch still zooms the graph.
        wheelZoom="pinch"
        fitPadding={FIT_PADDING}
        height={graphHeight(model.rows)}
        className="memory-versions__graph"
      />
    </div>
  );
}
