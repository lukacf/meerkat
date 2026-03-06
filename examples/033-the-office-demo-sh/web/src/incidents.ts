// =====================================================================
// Incidents -- RPG Battle Log format
// =====================================================================

import type { AgentId, Incident } from "./types";
import { AGENTS } from "./types";

let incidents: Incident[] = [];
let nextId = 0;
let renderCallback: (() => void) | null = null;

export function setRenderCallback(cb: () => void): void {
  renderCallback = cb;
}

export function getIncidents(): Incident[] {
  return incidents;
}

export function getActiveIncident(): Incident | null {
  return incidents.find(i => i.status === "active") ?? null;
}

export function createIncident(title: string, icon = "!"): string {
  const id = `incident-${nextId++}`;
  incidents.unshift({
    id,
    title,
    icon,
    timestamp: Date.now(),
    status: "active",
    messages: [],
  });
  renderCallback?.();
  return id;
}

export function addMessage(
  incidentId: string | null,
  from: AgentId | "user" | "system",
  to: AgentId | "system" | null,
  content: string,
  headline: string,
  category: string,
): void {
  const target = incidentId
    ? incidents.find(i => i.id === incidentId)
    : incidents.find(i => i.status === "active");

  if (!target) return;

  target.messages.push({
    from,
    to: to ?? "system",
    content,
    headline,
    timestamp: Date.now(),
    category,
  });
  renderCallback?.();
}

export function resolveIncident(incidentId: string): void {
  const inc = incidents.find(i => i.id === incidentId);
  if (inc) {
    inc.status = "resolved";
    renderCallback?.();
  }
}

// -- Render as RPG battle log --

const AGENT_COLORS: Record<string, string> = {};
for (const id of Object.keys(AGENTS) as AgentId[]) {
  AGENT_COLORS[id] = AGENTS[id].color;
}
AGENT_COLORS["user"] = "#dddddd";
AGENT_COLORS["system"] = "#888888";

function agentName(id: string): string {
  if (id === "user") return "YOU";
  if (id === "system") return "SYSTEM";
  const a = AGENTS[id as AgentId];
  return a ? a.name.toUpperCase() : id.toUpperCase();
}

function agentColor(id: string): string {
  return AGENT_COLORS[id] ?? "#888888";
}

export function renderIncidentPanel(container: HTMLElement): void {
  if (incidents.length === 0) {
    container.innerHTML = `<div class="log-empty">AWAITING EVENTS...</div>`;
    return;
  }

  let html = "";
  // Render oldest first so newest activity is at the bottom (scroll target)
  const ordered = [...incidents].reverse();
  for (const inc of ordered) {
    const timeAgo = formatTimeAgo(inc.timestamp);
    html += `<div class="log-incident-header">${escapeHtml(inc.icon)} ${escapeHtml(inc.title.toUpperCase())} [${timeAgo}]</div>`;

    for (const m of inc.messages) {
      const fromColor = agentColor(m.from);
      const fromName = agentName(m.from);
      const toName = m.to && m.to !== "system" ? agentName(m.to) : "";
      const arrow = toName ? ` <span class="log-arrow">&gt;</span> <span style="color:${agentColor(m.to)}">${toName}</span>` : "";
      const headlineText = escapeHtml(m.headline.length > 70 ? m.headline.slice(0, 67) + "..." : m.headline);

      html += `<div class="log-entry">` +
        `<span class="log-sender" style="color:${fromColor}">${fromName}</span>${arrow}` +
        `<span class="log-headline">${headlineText}</span>` +
        `</div>`;
    }
  }

  container.innerHTML = html;
}

function formatTimeAgo(ts: number): string {
  const sec = Math.floor((Date.now() - ts) / 1000);
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.floor(sec / 60)}m`;
  return `${Math.floor(sec / 3600)}h`;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
