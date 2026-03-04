// ═══════════════════════════════════════════════════════════
// Incidents — Event tracking with message trees
// ═══════════════════════════════════════════════════════════

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

export function createIncident(title: string, icon = "\u26A1"): string {
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
  // If no incident specified, add to the most recent active one
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

// ── Render incident panel HTML ──

export function renderIncidentPanel(container: HTMLElement): void {
  if (incidents.length === 0) {
    container.innerHTML = `<div class="panel-empty">Events will appear here as agents process them...</div>`;
    return;
  }

  container.innerHTML = incidents.map(inc => {
    const timeAgo = formatTimeAgo(inc.timestamp);
    const isActive = inc.status === "active";
    const msgCount = inc.messages.length;

    const messagesHtml = inc.messages.map(m => {
      const fromName = m.from === "user" ? "You" : m.from === "system" ? "System" : AGENTS[m.from as AgentId]?.name ?? m.from;
      const toName = m.to && m.to !== "system" ? AGENTS[m.to as AgentId]?.name ?? m.to : "";
      const fromColor = m.from !== "user" && m.from !== "system" ? AGENTS[m.from]?.color ?? "#888" : "#888";
      const arrow = toName ? ` \u2192 ${toName}` : "";
      const categoryClass = m.category || "routing";

      return `<div class="incident-msg">
        <span class="msg-dot" style="background:${fromColor}"></span>
        <span class="msg-from">${fromName}${arrow}</span>
        <span class="msg-category ${categoryClass}">${m.category}</span>
        <div class="msg-headline">${escapeHtml(m.headline)}</div>
      </div>`;
    }).join("");

    return `<div class="incident-card ${isActive ? "active" : "resolved"}">
      <div class="incident-header">
        <span class="incident-icon">${inc.icon}</span>
        <span class="incident-title">${escapeHtml(inc.title)}</span>
        <span class="incident-meta">${timeAgo} \u2022 ${msgCount} msg${msgCount !== 1 ? "s" : ""}</span>
      </div>
      <div class="incident-messages">${messagesHtml}</div>
    </div>`;
  }).join("");
}

function formatTimeAgo(ts: number): string {
  const sec = Math.floor((Date.now() - ts) / 1000);
  if (sec < 60) return `${sec}s ago`;
  if (sec < 3600) return `${Math.floor(sec / 60)}m ago`;
  return `${Math.floor(sec / 3600)}h ago`;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
