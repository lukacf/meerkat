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

export function parseArchivistMessage(content: string): void {
  // Look for ```record ... ``` blocks
  const pattern = /```record\s*\n?([\s\S]*?)```/g;
  let match;
  while ((match = pattern.exec(content)) !== null) {
    try {
      const json = JSON.parse(match[1].trim());
      if (json.op === "upsert" && json.id) {
        upsertRecord(json);
      }
    } catch {
      // Malformed JSON — try to salvage partial data
      tryPartialParse(match[1].trim());
    }
  }
}

function upsertRecord(data: any): void {
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

function tryPartialParse(text: string): void {
  // Try to extract at least some structured data from malformed JSON
  const idMatch = text.match(/"id"\s*:\s*"([^"]+)"/);
  const titleMatch = text.match(/"title"\s*:\s*"([^"]+)"/);
  const summaryMatch = text.match(/"summary"\s*:\s*"([^"]+)"/);
  if (idMatch) {
    upsertRecord({
      op: "upsert",
      id: idMatch[1],
      title: titleMatch?.[1] || idMatch[1],
      summary: summaryMatch?.[1] || "",
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
// Knowledge Graph Renderer (ink-on-paper)
// =====================================================================

interface GraphNode {
  id: string;
  label: string;
  type: string;
  x: number;
  y: number;
  vx: number;
  vy: number;
}

interface GraphEdge {
  from: string;
  to: string;
  label: string;
}

let graphNodes: GraphNode[] = [];
let graphEdges: GraphEdge[] = [];
let graphCanvas: HTMLCanvasElement | null = null;
let graphRafId = 0;
let graphLastTime = 0;
let graphAge = 0; // seconds since graph opened — for cooling

function buildGraph(): void {
  const nodeMap = new Map<string, GraphNode>();
  const edgeList: GraphEdge[] = [];

  for (const [, rec] of records) {
    for (const e of rec.entities) {
      const key = e.name.toLowerCase();
      if (!nodeMap.has(key)) {
        // Spread nodes in a circle around origin
        const idx = nodeMap.size;
        const angle = (idx / Math.max(1, idx + 5)) * Math.PI * 2 + Math.random() * 0.3;
        const r = 40 + Math.random() * 120;
        nodeMap.set(key, {
          id: key, label: e.name, type: e.type,
          x: Math.cos(angle) * r,
          y: Math.sin(angle) * r,
          vx: 0, vy: 0,
        });
      }
    }
    for (const r of rec.relationships) {
      const fk = r.from.toLowerCase();
      const tk = r.to.toLowerCase();
      if (!nodeMap.has(fk)) {
        nodeMap.set(fk, { id: fk, label: r.from, type: "unknown", x: (Math.random() - 0.5) * 100, y: (Math.random() - 0.5) * 100, vx: 0, vy: 0 });
      }
      if (!nodeMap.has(tk)) {
        nodeMap.set(tk, { id: tk, label: r.to, type: "unknown", x: (Math.random() - 0.5) * 100, y: (Math.random() - 0.5) * 100, vx: 0, vy: 0 });
      }
      if (!edgeList.some(e => e.from === fk && e.to === tk && e.label === r.type)) {
        edgeList.push({ from: fk, to: tk, label: r.type });
      }
    }
  }

  graphNodes = [...nodeMap.values()];
  graphEdges = edgeList;
}

// Build a fast lookup for edge endpoints
let edgeIndex: Map<string, GraphNode> = new Map();
function rebuildEdgeIndex(): void {
  edgeIndex = new Map();
  for (const n of graphNodes) edgeIndex.set(n.id, n);
}

function renderGraph(canvas: HTMLCanvasElement): void {
  graphCanvas = canvas;
  canvas.width = canvas.parentElement!.clientWidth;
  canvas.height = canvas.parentElement!.clientHeight;
  buildGraph();
  rebuildEdgeIndex();
  graphAge = 0;
  graphLastTime = 0;

  // Pre-simulate 500 steps to settle before first draw
  for (let i = 0; i < 500; i++) simulateGraph(0.016, Math.max(0.05, 1.0 - i * 0.002));

  if (graphRafId) cancelAnimationFrame(graphRafId);
  graphRafId = requestAnimationFrame(graphFrame);
}

function stopGraphAnimation(): void {
  if (graphRafId) {
    cancelAnimationFrame(graphRafId);
    graphRafId = 0;
  }
  graphCanvas = null;
}

function graphFrame(time: number): void {
  if (!graphCanvas) return;
  const dt = graphLastTime === 0 ? 0 : Math.min((time - graphLastTime) / 1000, 0.05);
  graphLastTime = time;
  graphAge += dt;

  // Cooling: simulation strength decays over time, settling after ~3 seconds
  const alpha = Math.max(0.01, 1.0 - graphAge * 0.3);
  if (alpha > 0.02) {
    simulateGraph(dt, alpha);
  }
  drawGraph(graphCanvas);
  graphRafId = requestAnimationFrame(graphFrame);
}

function simulateGraph(dt: number, alpha: number): void {
  const n = graphNodes.length;
  if (n === 0) return;

  // Minimum distance between any two nodes — generous to avoid label overlap
  const minSep = 120 + n * 5;

  // Repulsion — very strong, keeps nodes well separated
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      const a = graphNodes[i], b = graphNodes[j];
      let dx = b.x - a.x, dy = b.y - a.y;
      let dist = Math.sqrt(dx * dx + dy * dy);
      if (dist < 1) { dx = Math.random() * 2 - 1; dy = Math.random() * 2 - 1; dist = 1; }
      // Strong repulsion that falls off with distance
      const force = (minSep * minSep) / (dist * dist) * 50 * alpha;
      const fx = (dx / dist) * force;
      const fy = (dy / dist) * force;
      a.vx -= fx; a.vy -= fy;
      b.vx += fx; b.vy += fy;
    }
  }

  // Spring attraction along edges — pull connected nodes together
  const springLen = minSep * 0.8;
  for (const e of graphEdges) {
    const a = edgeIndex.get(e.from);
    const b = edgeIndex.get(e.to);
    if (!a || !b) continue;
    const dx = b.x - a.x, dy = b.y - a.y;
    const dist = Math.sqrt(dx * dx + dy * dy);
    if (dist < 1) continue;
    const force = (dist - springLen) * 0.03 * alpha;
    const fx = (dx / dist) * force;
    const fy = (dy / dist) * force;
    a.vx += fx; a.vy += fy;
    b.vx -= fx; b.vy -= fy;
  }

  // Gentle gravity toward origin
  for (const nd of graphNodes) {
    nd.vx += (0 - nd.x) * 0.001 * alpha;
    nd.vy += (0 - nd.y) * 0.001 * alpha;
  }

  // Apply velocity with damping
  for (const nd of graphNodes) {
    nd.vx *= 0.6;
    nd.vy *= 0.6;
    nd.x += nd.vx * dt * 20;
    nd.y += nd.vy * dt * 20;
  }
}

/** Compute viewport transform that fits all nodes with padding */
function computeViewport(canvasW: number, canvasH: number): { offsetX: number; offsetY: number; scale: number } {
  if (graphNodes.length === 0) return { offsetX: canvasW / 2, offsetY: canvasH / 2, scale: 1 };

  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  for (const n of graphNodes) {
    minX = Math.min(minX, n.x);
    maxX = Math.max(maxX, n.x);
    minY = Math.min(minY, n.y);
    maxY = Math.max(maxY, n.y);
  }

  // Generous padding for labels and breathing room
  const pad = 80;
  minX -= pad; maxX += pad; minY -= pad; maxY += pad + 20;

  const graphW = maxX - minX || 1;
  const graphH = maxY - minY || 1;
  const scale = Math.min(canvasW / graphW, canvasH / graphH, 1.5); // cap at 1.5x zoom
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;

  return {
    offsetX: canvasW / 2 - cx * scale,
    offsetY: canvasH / 2 - cy * scale,
    scale,
  };
}

// Node shape by type
const TYPE_SHAPES: Record<string, string> = {
  person: "circle",
  company: "diamond",
  system: "square",
  location: "triangle",
  amount: "circle",
};

function drawGraph(canvas: HTMLCanvasElement): void {
  const ctx = canvas.getContext("2d")!;
  const w = canvas.width, h = canvas.height;

  // Paper background
  ctx.fillStyle = "#f0e8d0";
  ctx.fillRect(0, 0, w, h);

  // Subtle ruled lines (typewriter paper)
  ctx.strokeStyle = "rgba(180, 170, 150, 0.3)";
  ctx.lineWidth = 0.5;
  for (let y = 20; y < h; y += 18) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(w, y);
    ctx.stroke();
  }

  // Empty state
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

  // Compute viewport transform to fit all nodes
  const vp = computeViewport(w, h);

  ctx.save();
  ctx.translate(vp.offsetX, vp.offsetY);
  ctx.scale(vp.scale, vp.scale);

  // Edges — thin ink lines
  ctx.lineWidth = 1 / vp.scale;
  for (const e of graphEdges) {
    const a = edgeIndex.get(e.from);
    const b = edgeIndex.get(e.to);
    if (!a || !b) continue;

    ctx.strokeStyle = "rgba(40, 35, 30, 0.5)";
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.stroke();

    // Edge label at midpoint
    if (e.label) {
      const mx = (a.x + b.x) / 2;
      const my = (a.y + b.y) / 2;
      ctx.font = `${8 / vp.scale}px 'IBM Plex Mono', monospace`;
      ctx.fillStyle = "rgba(100, 90, 75, 0.7)";
      ctx.textAlign = "center";
      ctx.fillText(e.label, mx, my - 3 / vp.scale);
    }
  }

  // Nodes
  const nodeR = 5 / vp.scale;
  for (const n of graphNodes) {
    const shape = TYPE_SHAPES[n.type] || "circle";

    ctx.fillStyle = "#f0e8d0";
    ctx.strokeStyle = "#28231e";
    ctx.lineWidth = 1.5 / vp.scale;

    if (shape === "diamond") {
      ctx.beginPath();
      ctx.moveTo(n.x, n.y - nodeR - 1);
      ctx.lineTo(n.x + nodeR + 1, n.y);
      ctx.lineTo(n.x, n.y + nodeR + 1);
      ctx.lineTo(n.x - nodeR - 1, n.y);
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

    // Label
    ctx.font = `${9 / vp.scale}px 'IBM Plex Mono', monospace`;
    ctx.fillStyle = "#28231e";
    ctx.textAlign = "center";
    ctx.fillText(n.label, n.x, n.y + nodeR + 12 / vp.scale);
  }

  ctx.restore();
  ctx.textAlign = "left";
}

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
