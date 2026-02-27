// ═══════════════════════════════════════════════════════════
// UI — 3x3 Channel Grid with WhatsApp-style Bubbles
// ═══════════════════════════════════════════════════════════

import type { ArenaState, ChannelId, ChatMessage, MatchSession, MessageRole, Team } from "./types";
import { CHANNELS, TEAMS, TEAM_LABELS } from "./types";

export const $ = <T extends Element>(id: string) => document.getElementById(id) as unknown as T;

let _session: MatchSession | null = null;
let _seqCounter = 0;
export function setSessionRef(s: MatchSession | null): void { _session = s; _seqCounter = 0; }
export function getSession(): MatchSession | null { return _session; }

// ── Phase / Status / Banner ──

export function setPhase(phase: "deliberation" | "resolution" | "idle"): void {
  $<HTMLDivElement>("arenaLayout").dataset.phase = phase;
}
export function setStatus(msg: string): void { $<HTMLParagraphElement>("statusLine").textContent = msg; }
export function setBadge(text: string, thinking = false): void {
  const el = $<HTMLSpanElement>("statusBadge");
  el.textContent = text;
  el.className = `status-badge${thinking ? " thinking" : ""}`;
}

let bannerTimer: ReturnType<typeof setTimeout> | null = null;
export function showBanner(phase: string, detail: string, ms: number): void {
  if (bannerTimer) clearTimeout(bannerTimer);
  $<HTMLDivElement>("bannerPhase").textContent = phase;
  $<HTMLDivElement>("bannerDetail").textContent = detail;
  $<HTMLDivElement>("banner").classList.add("visible");
  bannerTimer = setTimeout(() => $<HTMLDivElement>("banner").classList.remove("visible"), ms);
}

export function showVictory(winner: Team | "draw", state: ArenaState, narratorText?: string): void {
  const title = winner === "draw" ? "DRAW" : `${TEAM_LABELS[winner as Team] ?? winner} VICTORIOUS`;
  const narration = narratorText ? `<p class="victory-narration">${escapeHtml(narratorText)}</p>` : "";
  $<HTMLDivElement>("victoryInner").innerHTML = `<h2 class="${winner}">${title}</h2>${narration}<p>After ${state.turn} turns of warfare.</p><div class="final"><span class="france">F:${state.scores.france}</span><span class="prussia">P:${state.scores.prussia}</span><span class="russia">R:${state.scores.russia}</span></div>`;
  $<HTMLDivElement>("victory").classList.add("visible");
}

// ── Score ──

export function renderScore(state: ArenaState): void {
  $<HTMLSpanElement>("fPts").textContent = String(state.scores.france);
  $<HTMLSpanElement>("pPts").textContent = String(state.scores.prussia);
  $<HTMLSpanElement>("rPts").textContent = String(state.scores.russia);
  $<HTMLSpanElement>("turnPill").textContent = `Turn ${state.turn} / ${state.max_turns}`;
  const total = state.scores.france + state.scores.prussia + state.scores.russia || 1;
  $<HTMLDivElement>("fFill").style.flex = String(state.scores.france / total);
  $<HTMLDivElement>("pFill").style.flex = String(state.scores.prussia / total);
  $<HTMLDivElement>("rFill").style.flex = String(state.scores.russia / total);
}

// ── 3x3 Channel Grid ──

const ROLE_ICONS: Record<MessageRole, string> = {
  planner: "\u2694", operator: "\u{1F6E1}\uFE0F", ambassador: "\u{1F3F3}\uFE0F",
  narrator: "\u{1F4DC}", system: "\u2699\uFE0F",
};

// Grid layout: 3 columns x 3 rows = 9 strategy/diplo channels
// Row 1: Strategy (planner↔operator)
// Row 2: Diplomacy (planner↔ambassador)
// Row 3: Cross-faction (ambassador↔ambassador)
const GRID_CHANNELS: ChannelId[][] = [
  ["f-plan-op", "p-plan-op", "r-plan-op"],
  ["f-plan-amb", "p-plan-amb", "r-plan-amb"],
  ["f-p-diplo", "f-r-diplo", "p-r-diplo"],
];

// Track which cells are in verbose mode
const verboseCells = new Set<ChannelId>();

/** Parse summary from agent message content. */
function parseSummary(content: string): { summary: string; body: string } {
  // Skip salutations and honorifics to find the first substantive line
  const lines = content.split('\n').map(l => l.trim()).filter(l => l.length > 0);
  const skipPatterns = /^(\*\*)?(\s)*(your excellency|esteemed|distinguished|greetings|dear|mon ami|salutations|honorable|respected)/i;

  let substantiveLine = "";
  for (const line of lines) {
    const stripped = line.replace(/\*\*/g, "").replace(/[,.:!]+$/, "").trim();
    if (!skipPatterns.test(stripped) && stripped.length > 10) {
      substantiveLine = line;
      break;
    }
  }
  if (!substantiveLine) substantiveLine = lines[0] || content;

  const truncated = substantiveLine.length > 100
    ? substantiveLine.slice(0, 97) + "..."
    : substantiveLine;
  return { summary: truncated, body: content };
}

export function pushMessage(msg: Omit<ChatMessage, "seq">): void {
  if (!_session) return;
  const full: ChatMessage = { ...msg, seq: _seqCounter++ };
  _session.messages.push(full);
  renderCell(full.channel);
}

/** Push a structured summary from extraction turn — shows as highlighted headline bubble. */
export function pushStructuredSummary(msg: Omit<ChatMessage, "seq">): void {
  if (!_session) return;
  const full: ChatMessage = { ...msg, seq: _seqCounter++ };
  _session.messages.push(full);
  renderCell(full.channel);
}

function toggleVerbose(chId: ChannelId): void {
  if (verboseCells.has(chId)) verboseCells.delete(chId);
  else verboseCells.add(chId);
  renderCell(chId);
}

function renderBubble(m: ChatMessage, compact: boolean): string {
  const icon = ROLE_ICONS[m.role];
  // In diplomatic channels, show faction name instead of generic "Ambassador"
  const isCrossFaction = m.channel.endsWith("-diplo");
  const roleLabel = isCrossFaction && m.faction !== "neutral"
    ? TEAM_LABELS[m.faction as Team] ?? m.role
    : m.role.charAt(0).toUpperCase() + m.role.slice(1);
  const fClass = m.faction === "neutral" ? "" : ` ${m.faction}`;
  // Alignment: In same-faction channels, planner=left, others=right.
  // In cross-faction channels (diplo), align by faction alphabetical order.
  let align: "left" | "right";
  if (m.channel.endsWith("-diplo")) {
    // Cross-faction: first faction alphabetically goes left
    const parts = m.channel.replace("-diplo", "").split("-");
    const firstFaction = parts[0]; // e.g. "f" from "f-p-diplo"
    const senderInitial = m.faction === "neutral" ? "x" : (m.faction as string)[0];
    align = senderInitial === firstFaction ? "left" : "right";
  } else {
    align = (m.role === "planner" || m.role === "system") ? "left" : "right";
  }
  // Use structured headline if available, otherwise fallback to parsed summary
  const headline = m.headline || parseSummary(m.content).summary;
  const body = m.details || m.content;

  if (compact) {
    return `<div class="bubble ${align}${fClass}"><span class="bubble-sender">${icon} ${roleLabel}</span>${escapeHtml(headline)}</div>`;
  } else {
    return `<div class="bubble verbose ${align}${fClass}"><div class="bubble-head"><span class="bubble-sender">${icon} ${roleLabel}</span><span class="bubble-turn">T${m.turn}</span></div><div class="bubble-body">${renderMarkdown(body)}</div></div>`;
  }
}

function renderCell(chId: ChannelId): void {
  const el = document.getElementById(`cell-${chId}`);
  if (!el) return;
  const feedEl = el.querySelector('.cell-feed');
  if (!feedEl) return;
  const allMsgs = _session
    ? _session.messages.filter(m => m.channel === chId).sort((a, b) => a.seq - b.seq)
    : [];
  const isVerbose = verboseCells.has(chId);

  // Compact mode: show ONLY structured summaries (headlines from extraction turns).
  // Verbose mode: show ONLY raw comms messages (the actual conversations).
  // They're two views of the same activity, like email subject lines vs full bodies.
  let msgs: typeof allMsgs;
  if (isVerbose) {
    msgs = allMsgs.filter(m => !m.headline); // raw comms only
  } else {
    msgs = allMsgs.filter(m => m.headline); // structured summaries only
    // If no structured summaries yet (agents still working), show raw as fallback
    if (msgs.length === 0) msgs = allMsgs;
  }

  feedEl.innerHTML = msgs.map(m => renderBubble(m, !isVerbose)).join("");
  feedEl.scrollTop = feedEl.scrollHeight;
}

// Map channel IDs to faction for cell coloring
const CHANNEL_FACTION: Record<string, string> = {
  "f-plan-op": "france", "f-plan-amb": "france",
  "p-plan-op": "prussia", "p-plan-amb": "prussia",
  "r-plan-op": "russia", "r-plan-amb": "russia",
  "f-p-diplo": "cross", "f-r-diplo": "cross", "p-r-diplo": "cross",
};

/** Build the 3x3 grid. Called once at init. */
export function renderGrid(): void {
  const grid = $<HTMLDivElement>("channelGrid");
  const rows = GRID_CHANNELS.map((row, ri) => {
    const rowLabel = ["Strategy", "Briefings", "Negotiations"][ri];
    const cells = row.map(chId => {
      const ch = CHANNELS.find(c => c.id === chId)!;
      return `<div class="grid-cell" id="cell-${chId}">
        <div class="cell-header">
          <span class="cell-title">${ch.icon} ${ch.label}</span>
          <span class="cell-subtitle">${ch.subtitle}</span>
          <button class="cell-toggle" data-ch="${chId}" title="Toggle compact/full">\u25B6</button>
        </div>
        <div class="cell-feed"></div>
      </div>`;
    }).join("");
    return `<div class="grid-row"><div class="row-label">${rowLabel}</div>${cells}</div>`;
  }).join("");
  grid.innerHTML = rows;

  // Click header to toggle verbose
  grid.querySelectorAll('.cell-toggle').forEach(btn => {
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      const chId = btn.getAttribute('data-ch') as ChannelId;
      toggleVerbose(chId);
      btn.textContent = verboseCells.has(chId) ? '\u25BC' : '\u25B6';
    });
  });
}

/** Append narrator entry to scrollable feed. */
export function pushNarrator(text: string, turn: number): void {
  const feed = $<HTMLDivElement>("narratorFeed");
  const entry = document.createElement("div");
  entry.className = "narrator-entry";
  entry.innerHTML = `<span class="narrator-turn">Turn ${turn}</span>${renderMarkdown(text)}`;
  feed.appendChild(entry);
  feed.scrollTop = feed.scrollHeight;
}

// Legacy alias
export function renderChannelTabs(): void { /* no-op, grid replaces tabs */ }
export function renderAllColumns(): void { renderGrid(); }

// ── Text Rendering ──

export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function renderMarkdown(raw: string): string {
  let html = escapeHtml(raw);
  html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
  html = html.replace(/__(.+?)__/g, '<strong>$1</strong>');
  html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, '<em>$1</em>');
  html = html.replace(/^[\-\*]\s+(.+)$/gm, '<li>$1</li>');
  html = html.replace(/(<li>.*<\/li>\n?)+/g, (m) => `<ul>${m}</ul>`);
  html = html.replace(/^\d+\.\s+(.+)$/gm, '<li>$1</li>');
  html = html.replace(/\n/g, '<br>');
  html = html.replace(/<br><ul>/g, '<ul>');
  html = html.replace(/<\/ul><br>/g, '</ul>');
  html = html.replace(/<\/li><br><li>/g, '</li><li>');
  return html;
}

// ── Routing helpers ──

export function extractMeerkatId(peerAddress: string): string {
  const parts = peerAddress.split("/");
  return parts[parts.length - 1] || peerAddress;
}
export function meerkatTeam(id: string): Team | null {
  const mid = extractMeerkatId(id);
  for (const t of TEAMS) if (mid.startsWith(t)) return t;
  return null;
}
export function meerkatRole(id: string): MessageRole {
  const mid = extractMeerkatId(id);
  if (mid.includes("planner")) return "planner";
  if (mid.includes("operator")) return "operator";
  if (mid.includes("ambassador")) return "ambassador";
  return "system";
}
export function dmChannel(senderId: string, receiverId: string): ChannelId | null {
  const sTeam = meerkatTeam(senderId);
  const rTeam = meerkatTeam(receiverId);
  if (!sTeam || !rTeam) return null;
  const sRole = meerkatRole(senderId);
  const rRole = meerkatRole(receiverId);
  if (sTeam !== rTeam) {
    const sorted = [sTeam, rTeam].sort();
    return `${sorted[0][0]}-${sorted[1][0]}-diplo` as ChannelId;
  }
  if ((sRole === "planner" && rRole === "operator") || (sRole === "operator" && rRole === "planner"))
    return `${sTeam[0]}-plan-op` as ChannelId;
  if ((sRole === "planner" && rRole === "ambassador") || (sRole === "ambassador" && rRole === "planner"))
    return `${sTeam[0]}-plan-amb` as ChannelId;
  return null;
}

// ── Map aspect ratio enforcement ──

const MAP_ASPECT = 960 / 600; // viewBox ratio

function resizeMap(): void {
  const mapEl = document.querySelector('.map-stage') as HTMLElement | null;
  if (!mapEl) return;
  const w = mapEl.clientWidth;
  mapEl.style.height = `${Math.round(w / MAP_ASPECT)}px`;
}

// Resize on load and window resize
export function initMapResize(): void {
  resizeMap();
  window.addEventListener('resize', resizeMap);
  // Also observe left-col width changes from phase transitions
  const observer = new ResizeObserver(resizeMap);
  const leftCol = document.querySelector('.left-col');
  if (leftCol) observer.observe(leftCol);
}

// ── Misc ──

export function parseJsResult(val: unknown): string { return typeof val === "string" ? val : String(val); }
export function sleep(ms: number): Promise<void> { return new Promise(r => setTimeout(r, ms)); }
