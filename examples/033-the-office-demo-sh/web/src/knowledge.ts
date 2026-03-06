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

export function showGraph(container: HTMLElement): void {
  activeTab = "graph";
  renderGraph(container);
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
// Knowledge Graph Renderer (Cytoscape.js)
// =====================================================================

import cytoscape from "cytoscape";

const TYPE_COLORS: Record<string, string> = {
  person: "#4466aa",
  company: "#aa6633",
  system: "#448844",
  location: "#886644",
  amount: "#884488",
  unknown: "#666666",
};

const TYPE_SHAPES: Record<string, cytoscape.Css.NodeShape> = {
  person: "ellipse",
  company: "diamond",
  system: "rectangle",
  location: "triangle",
  amount: "ellipse",
  unknown: "ellipse",
};

let cyInstance: cytoscape.Core | null = null;

function buildCyElements(): cytoscape.ElementDefinition[] {
  const elements: cytoscape.ElementDefinition[] = [];
  const nodeIds = new Set<string>();

  for (const [, rec] of records) {
    for (const e of rec.entities) {
      const key = e.name.toLowerCase();
      if (!nodeIds.has(key)) {
        nodeIds.add(key);
        elements.push({
          data: { id: key, label: e.name, entityType: e.type },
        });
      }
    }
    for (const r of rec.relationships) {
      const fk = r.from.toLowerCase();
      const tk = r.to.toLowerCase();
      if (!nodeIds.has(fk)) {
        nodeIds.add(fk);
        elements.push({ data: { id: fk, label: r.from, entityType: "unknown" } });
      }
      if (!nodeIds.has(tk)) {
        nodeIds.add(tk);
        elements.push({ data: { id: tk, label: r.to, entityType: "unknown" } });
      }
      elements.push({
        data: { id: `${fk}-${r.type}-${tk}`, source: fk, target: tk, label: r.type },
      });
    }
  }

  return elements;
}

function renderGraph(container: HTMLElement): void {
  destroyGraph();

  const elements = buildCyElements();

  if (elements.length === 0) {
    container.innerHTML = `<div class="kb-empty" style="display:flex;align-items:center;justify-content:center;height:100%">NO DATA YET</div>`;
    return;
  }

  // Clear any leftover HTML
  container.innerHTML = "";

  cyInstance = cytoscape({
    container,
    elements,
    style: [
      {
        selector: "node",
        style: {
          label: "data(label)",
          "background-color": (ele: cytoscape.NodeSingular) =>
            TYPE_COLORS[ele.data("entityType")] || TYPE_COLORS.unknown,
          shape: (ele: cytoscape.NodeSingular) =>
            TYPE_SHAPES[ele.data("entityType")] || "ellipse",
          width: 20,
          height: 20,
          "border-width": 1.5,
          "border-color": "#28231e",
          "font-family": "'IBM Plex Mono', monospace",
          "font-size": "9px",
          color: "#28231e",
          "text-margin-y": 4,
          "text-valign": "bottom",
          "text-halign": "center",
          "text-background-color": "#f0e8d0",
          "text-background-opacity": 0.85,
          "text-background-padding": "2px",
          "text-background-shape": "roundrectangle",
        } as any,
      },
      {
        selector: "edge",
        style: {
          label: "data(label)",
          width: 1,
          "line-color": "rgba(60, 50, 40, 0.4)",
          "curve-style": "bezier",
          "target-arrow-shape": "triangle",
          "target-arrow-color": "rgba(60, 50, 40, 0.4)",
          "arrow-scale": 0.6,
          "font-family": "'IBM Plex Mono', monospace",
          "font-size": "7px",
          "font-style": "italic",
          color: "rgba(120, 100, 80, 0.7)",
          "text-rotation": "autorotate",
          "text-margin-y": -6,
          "text-background-color": "#f0e8d0",
          "text-background-opacity": 0.8,
          "text-background-padding": "1px",
          "text-background-shape": "roundrectangle",
        } as any,
      },
    ],
    layout: {
      name: "cose",
      animate: false,
      nodeDimensionsIncludeLabels: true,
      nodeRepulsion: () => 8000,
      idealEdgeLength: () => 80,
      edgeElasticity: () => 100,
      gravity: 0.3,
      padding: 20,
      fit: true,
    } as any,
    userZoomingEnabled: true,
    userPanningEnabled: true,
    boxSelectionEnabled: false,
  });
}

function destroyGraph(): void {
  if (cyInstance) {
    cyInstance.destroy();
    cyInstance = null;
  }
}

function stopGraphAnimation(): void {
  destroyGraph();
}


function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
