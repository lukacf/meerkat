// =====================================================================
// Knowledge Base -- Archivist Records + Knowledge Graph
// =====================================================================

// -- Record types --

export interface ArchiveRecord {
  id: string;
  title: string;
  type: "incident" | "person" | "company" | "system" | "policy";
  summary: string;
  entities: Array<{ name: string; type: string; role?: string }>;
  relationships: Array<{ from: string; to: string; type: string }>;
  decisions: Array<{ action: string; outcome: string; by: string }>;
  status: "open" | "resolved" | "monitoring";
  lastUpdated: number;
}

const records = new Map<string, ArchiveRecord>();
let activeTab: "cases" | "graph" = "cases";

// -- Parse archivist messages for record blocks --

export function upsertRecord(data: any): void {
  const existing = records.get(data.id);
  if (existing) {
    // Merge: append new entities/relationships/decisions, update summary
    if (data.summary) existing.summary = data.summary;
    if (data.status) existing.status = data.status;
    if (data.title) existing.title = data.title;
    if (Array.isArray(data.entities)) {
      for (const e of data.entities) {
        if (!existing.entities.some(x => x.name.toLowerCase() === e.name?.toLowerCase())) {
          existing.entities.push(e);
        }
      }
    }
    if (Array.isArray(data.relationships)) {
      for (const r of data.relationships) {
        if (!existing.relationships.some(x => x.from === r.from && x.to === r.to && x.type === r.type)) {
          existing.relationships.push(r);
        }
      }
    }
    if (Array.isArray(data.decisions)) {
      for (const d of data.decisions) {
        existing.decisions.push(d);
      }
    }
    existing.lastUpdated = Date.now();
  } else {
    records.set(data.id, {
      id: data.id,
      title: data.title || data.id,
      type: data.type || "incident",
      summary: data.summary || "",
      entities: Array.isArray(data.entities) ? data.entities : [],
      relationships: Array.isArray(data.relationships) ? data.relationships : [],
      decisions: Array.isArray(data.decisions) ? data.decisions : [],
      status: data.status || "open",
      lastUpdated: Date.now(),
    });
  }
}

// -- Public API --

export function getRecordCount(): number { return records.size; }
export function getRecords(): ArchiveRecord[] { return [...records.values()]; }

export function isKBVisible(): boolean { return activeTab === "cases" || activeTab === "graph"; }
export function getActiveTab(): "cases" | "graph" { return activeTab; }

export function showCaseFiles(contentEl: HTMLElement, footerEl: HTMLElement): void {
  activeTab = "cases";
  renderCaseFiles(contentEl, footerEl);
}

export function showGraph(canvas: HTMLCanvasElement): void {
  activeTab = "graph";
  renderGraph(canvas);
}

export function hideKnowledgeBase(): void {
  activeTab = "cases"; // reset but don't render
  stopGraphAnimation();
}

// =====================================================================
// Case Files Renderer
// =====================================================================

function renderCaseFiles(contentEl: HTMLElement, footerEl: HTMLElement): void {
  const recs = [...records.values()].sort((a, b) => b.lastUpdated - a.lastUpdated);

  if (recs.length === 0) {
    contentEl.innerHTML = `<div class="kb-empty">NO RECORDS YET.<br><br>THE ARCHIVIST WILL CREATE<br>STRUCTURED RECORDS AS EVENTS<br>ARE PROCESSED.</div>`;
    footerEl.textContent = "0 RECORDS";
    return;
  }

  let html = "";
  for (const rec of recs) {
    const statusClass = rec.status === "open" ? "case-active" :
      rec.status === "resolved" ? "case-closed" : "case-monitoring";
    const statusLabel = rec.status.toUpperCase();

    html += `<div class="filing-card">`;
    html += `<div class="case-header">`;
    html += `<span class="card-entity">${esc(rec.title)}</span>`;
    html += `<span class="case-status ${statusClass}">${statusLabel}</span>`;
    html += `</div>`;

    if (rec.summary) {
      html += `<div class="case-summary">${esc(rec.summary)}</div>`;
    }

    if (rec.entities.length > 0) {
      html += `<div class="case-section-title">ENTITIES</div>`;
      html += `<div class="case-entities">`;
      for (const e of rec.entities) {
        const role = e.role ? ` — ${esc(e.role)}` : "";
        html += `<div class="case-entity"><span class="entity-type">${esc(e.type)}</span> ${esc(e.name)}${role}</div>`;
      }
      html += `</div>`;
    }

    if (rec.relationships.length > 0) {
      html += `<div class="case-section-title">RELATIONSHIPS</div>`;
      for (const r of rec.relationships) {
        html += `<div class="case-rel">${esc(r.from)} <span class="rel-arrow">\u2192</span> ${esc(r.to)} <span class="rel-type">(${esc(r.type)})</span></div>`;
      }
    }

    if (rec.decisions.length > 0) {
      html += `<div class="case-section-title">DECISIONS</div>`;
      for (const d of rec.decisions) {
        const isApproved = d.outcome?.toLowerCase().includes("approved");
        const cls = isApproved ? "decision-approved" : "decision-denied";
        html += `<div class="case-decision ${cls}">${esc(d.action)} \u2014 ${esc(d.outcome)} (by ${esc(d.by)})</div>`;
      }
    }

    html += `</div>`;
  }

  contentEl.innerHTML = html;
  footerEl.textContent = `${recs.length} RECORD${recs.length !== 1 ? "S" : ""} / ${countTotalEntities()} ENTITIES / ${countTotalRels()} RELATIONSHIPS`;
}

function countTotalEntities(): number {
  const names = new Set<string>();
  for (const [, r] of records) for (const e of r.entities) names.add(e.name.toLowerCase());
  return names.size;
}

function countTotalRels(): number {
  return [...records.values()].reduce((sum, r) => sum + r.relationships.length, 0);
}

// =====================================================================
// Knowledge Graph Renderer (deterministic layout, ink-on-paper)
// =====================================================================

interface GraphNode {
  id: string;
  label: string;
  type: string;
  x: number;
  y: number;
}

interface GraphEdge {
  from: string;
  to: string;
  label: string;
}

let graphCanvas: HTMLCanvasElement | null = null;
let graphRafId = 0;
let graphNodes: GraphNode[] = [];
let graphEdges: GraphEdge[] = [];
let nodeIndex: Map<string, GraphNode> = new Map();

function buildGraph(): void {
  const nodeMap = new Map<string, GraphNode>();
  const edgeList: GraphEdge[] = [];

  for (const [, rec] of records) {
    for (const e of rec.entities) {
      const key = e.name.toLowerCase();
      if (!nodeMap.has(key)) {
        nodeMap.set(key, { id: key, label: e.name, type: e.type, x: 0, y: 0 });
      }
    }
    for (const r of rec.relationships) {
      const fk = r.from.toLowerCase();
      const tk = r.to.toLowerCase();
      if (!nodeMap.has(fk)) nodeMap.set(fk, { id: fk, label: r.from, type: "unknown", x: 0, y: 0 });
      if (!nodeMap.has(tk)) nodeMap.set(tk, { id: tk, label: r.to, type: "unknown", x: 0, y: 0 });
      if (!edgeList.some(e => e.from === fk && e.to === tk && e.label === r.type)) {
        edgeList.push({ from: fk, to: tk, label: r.type });
      }
    }
  }

  graphNodes = [...nodeMap.values()];
  graphEdges = edgeList;
  nodeIndex = nodeMap;
}

/** Deterministic layout: place nodes in a circle, connected nodes closer together */
function layoutGraph(w: number, h: number): void {
  const n = graphNodes.length;
  if (n === 0) return;

  // Find connected components via BFS
  const adj = new Map<string, Set<string>>();
  for (const nd of graphNodes) adj.set(nd.id, new Set());
  for (const e of graphEdges) {
    adj.get(e.from)?.add(e.to);
    adj.get(e.to)?.add(e.from);
  }

  const visited = new Set<string>();
  const components: GraphNode[][] = [];
  for (const nd of graphNodes) {
    if (visited.has(nd.id)) continue;
    const comp: GraphNode[] = [];
    const queue = [nd.id];
    visited.add(nd.id);
    while (queue.length > 0) {
      const cur = queue.shift()!;
      const node = nodeIndex.get(cur);
      if (node) comp.push(node);
      for (const nb of adj.get(cur) ?? []) {
        if (!visited.has(nb)) { visited.add(nb); queue.push(nb); }
      }
    }
    components.push(comp);
  }

  // Sort: largest component first
  components.sort((a, b) => b.length - a.length);

  // Layout each component in a circle, then arrange components horizontally
  const padding = 40;
  let cursorX = padding;
  const centerY = h / 2;

  for (const comp of components) {
    if (comp.length === 1) {
      comp[0].x = cursorX;
      comp[0].y = centerY;
      cursorX += 80;
    } else {
      // Circle radius proportional to node count
      const radius = Math.max(40, comp.length * 25);

      // Place nodes in a circle
      for (let i = 0; i < comp.length; i++) {
        const angle = (i / comp.length) * Math.PI * 2 - Math.PI / 2;
        comp[i].x = cursorX + radius + Math.cos(angle) * radius;
        comp[i].y = centerY + Math.sin(angle) * radius;
      }
      cursorX += radius * 2 + 60;
    }
  }

  // Center everything
  let minX = Infinity, maxX = -Infinity;
  for (const nd of graphNodes) {
    minX = Math.min(minX, nd.x);
    maxX = Math.max(maxX, nd.x);
  }
  const totalW = maxX - minX;
  const offsetX = (w - totalW) / 2 - minX;
  for (const nd of graphNodes) nd.x += offsetX;
}

// Node shape by type
const TYPE_SHAPES: Record<string, string> = {
  person: "circle",
  company: "diamond",
  system: "square",
  location: "triangle",
  amount: "circle",
};

const TYPE_COLORS: Record<string, string> = {
  person: "#4466aa",
  company: "#aa6633",
  system: "#448844",
  location: "#886644",
  amount: "#884488",
  unknown: "#666666",
};

function renderGraph(canvas: HTMLCanvasElement): void {
  graphCanvas = canvas;
  canvas.width = canvas.parentElement!.clientWidth;
  canvas.height = canvas.parentElement!.clientHeight;
  buildGraph();
  layoutGraph(canvas.width, canvas.height);
  if (graphRafId) cancelAnimationFrame(graphRafId);
  drawGraph(canvas);
}

function stopGraphAnimation(): void {
  if (graphRafId) { cancelAnimationFrame(graphRafId); graphRafId = 0; }
  graphCanvas = null;
}

function drawGraph(canvas: HTMLCanvasElement): void {
  const ctx = canvas.getContext("2d")!;
  const w = canvas.width, h = canvas.height;

  // Paper background
  ctx.fillStyle = "#f0e8d0";
  ctx.fillRect(0, 0, w, h);

  // Subtle ruled lines
  ctx.strokeStyle = "rgba(180, 170, 150, 0.25)";
  ctx.lineWidth = 0.5;
  for (let y = 20; y < h; y += 16) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(w, y);
    ctx.stroke();
  }

  if (graphNodes.length === 0) {
    ctx.font = "10px 'Press Start 2P', monospace";
    ctx.fillStyle = "rgba(100, 90, 75, 0.5)";
    ctx.textAlign = "center";
    ctx.fillText("NO DATA YET", w / 2, h / 2 - 10);
    ctx.font = "9px 'IBM Plex Mono', monospace";
    ctx.fillText("Records will populate this graph", w / 2, h / 2 + 10);
    ctx.textAlign = "left";
    return;
  }

  // Edges — curved ink lines with labels
  for (let ei = 0; ei < graphEdges.length; ei++) {
    const e = graphEdges[ei];
    const a = nodeIndex.get(e.from);
    const b = nodeIndex.get(e.to);
    if (!a || !b) continue;

    const dx = b.x - a.x, dy = b.y - a.y;
    // Offset curve for multiple edges between same node pair
    const perpX = -dy * 0.15, perpY = dx * 0.15;
    const mx = (a.x + b.x) / 2 + perpX;
    const my = (a.y + b.y) / 2 + perpY;

    ctx.strokeStyle = "rgba(60, 50, 40, 0.35)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.quadraticCurveTo(mx, my, b.x, b.y);
    ctx.stroke();

    // Edge label — offset from midpoint, small italic
    if (e.label) {
      ctx.font = "italic 8px 'IBM Plex Mono', monospace";
      ctx.fillStyle = "rgba(120, 100, 80, 0.7)";
      ctx.textAlign = "center";
      ctx.fillText(e.label, mx, my - 4);
    }
  }

  // Nodes — draw node circles/shapes with colored fill
  const nodeR = 6;
  for (const n of graphNodes) {
    const shape = TYPE_SHAPES[n.type] || "circle";
    const color = TYPE_COLORS[n.type] || TYPE_COLORS.unknown;

    // White background behind node to clear any overlapping edges
    ctx.fillStyle = "#f0e8d0";
    ctx.beginPath();
    ctx.arc(n.x, n.y, nodeR + 3, 0, Math.PI * 2);
    ctx.fill();

    ctx.fillStyle = color;
    ctx.strokeStyle = "#28231e";
    ctx.lineWidth = 1.5;

    if (shape === "diamond") {
      const r = nodeR + 1;
      ctx.beginPath();
      ctx.moveTo(n.x, n.y - r); ctx.lineTo(n.x + r, n.y);
      ctx.lineTo(n.x, n.y + r); ctx.lineTo(n.x - r, n.y);
      ctx.closePath();
      ctx.fill(); ctx.stroke();
    } else if (shape === "square") {
      ctx.fillRect(n.x - nodeR, n.y - nodeR, nodeR * 2, nodeR * 2);
      ctx.strokeRect(n.x - nodeR, n.y - nodeR, nodeR * 2, nodeR * 2);
    } else {
      ctx.beginPath();
      ctx.arc(n.x, n.y, nodeR, 0, Math.PI * 2);
      ctx.fill(); ctx.stroke();
    }

    // Label below node — dark ink, white background for readability
    ctx.font = "10px 'IBM Plex Mono', monospace";
    ctx.textAlign = "center";
    const labelW = ctx.measureText(n.label).width;
    ctx.fillStyle = "rgba(240, 232, 208, 0.85)";
    ctx.fillRect(n.x - labelW / 2 - 2, n.y + nodeR + 2, labelW + 4, 13);
    ctx.fillStyle = "#28231e";
    ctx.fillText(n.label, n.x, n.y + nodeR + 13);
  }

  ctx.textAlign = "left";
}

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
