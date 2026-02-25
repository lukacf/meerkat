import "./styles.css";

// ═══════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════

type Team = "north" | "south" | "east";
const TEAMS: Team[] = ["north", "south", "east"];

interface RegionState { id: string; controller: Team; defense: number; value: number }
interface ArenaState {
  turn: number; max_turns: number; regions: RegionState[];
  scores: Record<Team, number>; winner?: Team | "draw";
}
interface OrderSet { team: Team; aggression: number; fortify: number; target_region: string; diplomacy: string }
interface TurnDecision { order: OrderSet; reasoning: string }
interface Dispatch { team: Team; turn: number; diplomacy: string; reasoning: string }

// ── Channel / Message types for Slack-like UI ──
type ChannelId = "north-ops" | "south-ops" | "east-ops" | "negotiations" | "narrator";
type MessageRole = "planner" | "operator" | "narrator" | "system";
interface ChatMessage { channel: ChannelId; role: MessageRole; faction: Team | "neutral"; content: string; turn: number }
const CHANNELS: { id: ChannelId; label: string; icon: string }[] = [
  { id: "north-ops", label: "#north-ops", icon: "\u{1F9ED}" },
  { id: "south-ops", label: "#south-ops", icon: "\u{1F9ED}" },
  { id: "east-ops", label: "#east-ops", icon: "\u{1F9ED}" },
  { id: "negotiations", label: "#negotiations", icon: "\u{1F91D}" },
  { id: "narrator", label: "#narrator", icon: "\u{1F4DC}" },
];

interface RuntimeModule {
  default: () => Promise<unknown>;
  create_session: (bytes: Uint8Array, configJson: string) => number;
  start_turn: (handle: number, prompt: string, optionsJson: string) => Promise<string>;
  poll_events: (handle: number) => string;
  get_session_state: (handle: number) => string;
  inspect_mobpack: (bytes: Uint8Array) => string;
  destroy_session: (handle: number) => void;
}

interface FactionSession { team: Team; handle: number; model: string }
interface MatchSession {
  factions: FactionSession[];
  narratorHandle: number | null;
  state: ArenaState;
  dispatches: Dispatch[];
  messages: ChatMessage[];
  running: boolean;
  prevControllers: Map<string, Team>;
}

// ═══════════════════════════════════════════════════════════
// Map Data — 12 territories, 3 factions (geographic landmass)
// ═══════════════════════════════════════════════════════════

interface TerritoryInfo {
  path: string;          // SVG path string for irregular polygon border
  center: { x: number; y: number };  // label placement
  label: string;
}

// Continental landmass: North = mountainous highlands (top), East = frontier steppe (right),
// South = delta lowlands (bottom-left). Territories are irregular polygons that tile together.
const MAP: Record<string, TerritoryInfo> = {
  // ── North Faction (top of the landmass — highlands) ──
  "north-capital":  {
    path: "M350,15 L390,10 L460,8 L520,14 L550,25 L540,60 L510,95 L450,110 L390,105 L355,80 L340,50 Z",
    center: { x: 450, y: 58 }, label: "North\nCapital"
  },
  "north-harbor":   {
    path: "M140,75 L195,48 L250,35 L310,22 L350,15 L340,50 L355,80 L390,105 L340,130 L280,145 L220,150 L165,135 L130,110 Z",
    center: { x: 255, y: 88 }, label: "North\nHarbor"
  },
  "north-ridge":    {
    path: "M550,25 L610,30 L680,50 L740,78 L760,110 L730,140 L670,150 L610,145 L560,130 L510,95 L540,60 Z",
    center: { x: 640, y: 95 }, label: "North\nRidge"
  },
  // ── Contested Middle ──
  "obsidian-gate":  {
    path: "M130,110 L165,135 L220,150 L280,145 L340,130 L390,105 L450,110 L440,155 L410,200 L350,225 L280,230 L210,220 L155,195 L120,160 Z",
    center: { x: 290, y: 170 }, label: "Obsidian\nGate"
  },
  "crimson-pass":   {
    path: "M450,110 L510,95 L560,130 L610,145 L670,150 L660,195 L630,230 L570,250 L500,245 L440,230 L410,200 L440,155 Z",
    center: { x: 545, y: 180 }, label: "Crimson\nPass"
  },
  // ── East Faction (right side — frontier steppe) ──
  "glass-frontier": {
    path: "M120,160 L155,195 L210,220 L280,230 L350,225 L410,200 L440,230 L430,280 L390,320 L320,340 L240,330 L170,305 L120,265 L100,220 Z",
    center: { x: 270, y: 270 }, label: "Glass\nFrontier"
  },
  "ember-crossing": {
    path: "M670,150 L730,140 L760,110 L800,145 L830,195 L840,250 L820,300 L770,330 L710,335 L650,320 L610,285 L590,250 L630,230 L660,195 Z",
    center: { x: 720, y: 240 }, label: "Ember\nCrossing"
  },
  // ── Central contested ──
  "saffron-fields": {
    path: "M440,230 L500,245 L570,250 L590,250 L610,285 L650,320 L630,360 L570,385 L500,390 L430,380 L380,355 L390,320 L430,280 Z",
    center: { x: 510, y: 315 }, label: "Saffron\nFields"
  },
  // ── South Faction (bottom — delta lowlands) ──
  "southern-rail":  {
    path: "M100,220 L120,265 L170,305 L240,330 L320,340 L300,385 L260,420 L200,440 L140,430 L90,400 L65,350 L70,290 Z",
    center: { x: 175, y: 355 }, label: "Southern\nRail"
  },
  "ash-basin":      {
    path: "M650,320 L710,335 L770,330 L820,300 L850,340 L855,395 L830,440 L780,465 L720,470 L660,455 L620,420 L600,385 L630,360 Z",
    center: { x: 740, y: 395 }, label: "Ash\nBasin"
  },
  "mercury-delta":  {
    path: "M320,340 L390,320 L380,355 L430,380 L500,390 L480,430 L440,465 L380,485 L310,490 L250,475 L200,440 L260,420 L300,385 Z",
    center: { x: 355, y: 425 }, label: "Mercury\nDelta"
  },
  "south-capital":  {
    path: "M500,390 L570,385 L630,360 L600,385 L620,420 L660,455 L640,490 L590,520 L530,535 L460,530 L400,510 L380,485 L440,465 L480,430 Z",
    center: { x: 530, y: 465 }, label: "South\nCapital"
  },
};

const EDGES: [string, string][] = [
  ["north-capital", "north-harbor"], ["north-capital", "north-ridge"],
  ["north-capital", "obsidian-gate"], ["north-capital", "crimson-pass"],
  ["north-harbor", "obsidian-gate"], ["north-ridge", "crimson-pass"],
  ["obsidian-gate", "crimson-pass"], ["obsidian-gate", "glass-frontier"],
  ["crimson-pass", "ember-crossing"], ["glass-frontier", "saffron-fields"],
  ["glass-frontier", "southern-rail"], ["ember-crossing", "saffron-fields"],
  ["ember-crossing", "ash-basin"], ["southern-rail", "mercury-delta"],
  ["saffron-fields", "mercury-delta"], ["saffron-fields", "south-capital"],
  ["ash-basin", "south-capital"], ["mercury-delta", "south-capital"],
  ["saffron-fields", "ash-basin"],
];

// ═══════════════════════════════════════════════════════════
// Game Engine (TypeScript — app layer, NOT in WASM)
// ═══════════════════════════════════════════════════════════

function defaultState(): ArenaState {
  return {
    turn: 1, max_turns: 10,
    scores: { north: 0, south: 0, east: 0 },
    regions: [
      { id: "north-capital",  controller: "north", defense: 58, value: 3 },
      { id: "north-harbor",   controller: "north", defense: 48, value: 2 },
      { id: "north-ridge",    controller: "north", defense: 42, value: 2 },
      { id: "obsidian-gate",  controller: "north", defense: 43, value: 3 },
      { id: "south-capital",  controller: "south", defense: 56, value: 3 },
      { id: "southern-rail",  controller: "south", defense: 47, value: 2 },
      { id: "mercury-delta",  controller: "south", defense: 38, value: 2 },
      { id: "ash-basin",      controller: "south", defense: 45, value: 3 },
      { id: "crimson-pass",   controller: "east",  defense: 40, value: 3 },
      { id: "ember-crossing", controller: "east",  defense: 44, value: 3 },
      { id: "glass-frontier", controller: "east",  defense: 46, value: 4 },
      { id: "saffron-fields", controller: "east",  defense: 40, value: 2 },
    ],
  };
}

function resolveOrders(state: ArenaState, orders: TurnDecision[]): ArenaState {
  const newRegions = state.regions.map(r => ({ ...r }));
  for (const decision of orders) {
    const { order } = decision;
    const target = newRegions.find(r => r.id === order.target_region);
    if (!target || target.controller === order.team) continue;
    const atkNoise = Math.floor(Math.random() * 12);
    const defNoise = Math.floor(Math.random() * 8);
    if (order.aggression + atkNoise > target.defense + defNoise) {
      target.controller = order.team;
      target.defense = Math.max(25, Math.floor(target.defense * 0.5) + Math.floor(order.fortify / 8));
    } else {
      target.defense = Math.min(100, target.defense + 2);
    }
    for (const r of newRegions) {
      if (r.controller === order.team) { r.defense = Math.min(100, r.defense + Math.floor(order.fortify / 15)); break; }
    }
  }
  const scores = { ...state.scores };
  for (const team of TEAMS) scores[team] += newRegions.filter(r => r.controller === team).reduce((s, r) => s + r.value, 0);
  const turn = state.turn + 1;
  let winner: Team | "draw" | undefined;
  if (turn > state.max_turns) {
    const sorted = TEAMS.slice().sort((a, b) => scores[b] - scores[a]);
    winner = scores[sorted[0]] > scores[sorted[1]] ? sorted[0] : "draw";
  }
  return { turn, max_turns: state.max_turns, regions: newRegions, scores, winner };
}

function stateBlock(team: Team, state: ArenaState, signals: Record<Team, string>): string {
  const ours = state.regions.filter(r => r.controller === team);
  const enemies = state.regions.filter(r => r.controller !== team);
  const others = TEAMS.filter(t => t !== team);
  return `Turn ${state.turn}/${state.max_turns}. You are ${team.toUpperCase()}.

SCORES: ${TEAMS.map(t => `${t}=${state.scores[t]}`).join(", ")}

YOUR TERRITORIES (${ours.length}):
${ours.map(r => `  ${r.id}: def=${r.defense}, val=${r.value}`).join("\n")}

ENEMY TERRITORIES (${enemies.length}):
${enemies.map(r => `  ${r.id} [${r.controller}]: def=${r.defense}, val=${r.value}`).join("\n")}

DIPLOMATIC SIGNALS FROM LAST TURN:
${others.map(t => `  ${t}: "${signals[t]}"`).join("\n")}`;
}

function buildPlannerPrompt(team: Team, state: ArenaState, signals: Record<Team, string>): string {
  return `[PLANNER PHASE] ${stateBlock(team, state, signals)}

As the PLANNER for ${team.toUpperCase()}, analyze the strategic situation and propose:
1. Which enemy territory should we target and why?
2. What aggression level (0-100) is appropriate?
3. What diplomatic signal should we send? Remember: diplomacy is PUBLIC. You can propose alliances, threaten, deceive, or negotiate.

Think 2-3 turns ahead. Consider alliances and betrayals.

Respond with ONLY valid JSON:
{"order":{"aggression":<0-100>,"fortify":<0-100>,"target_region":"<id>","diplomacy":"<your public diplomatic message>"},"reasoning":"<your strategic analysis>"}`;
}

function buildOperatorPrompt(team: Team, plannerOutput: string): string {
  return `[OPERATOR PHASE] The planner has proposed the following:

${plannerOutput}

As the OPERATOR for ${team.toUpperCase()}, validate this order:
- Is the target an enemy territory?
- Does aggression + fortify = 100?
- Is the diplomatic signal strategically sound?

If the plan is good, commit it. If not, adjust and explain why.

Respond with ONLY valid JSON (the final committed order):
{"order":{"aggression":<0-100>,"fortify":<0-100>,"target_region":"<id>","diplomacy":"<your public diplomatic message>"},"reasoning":"<brief execution note>"}`;
}

function parseDecision(team: Team, text: string, state: ArenaState): TurnDecision | null {
  try {
    const match = text.match(/\{[\s\S]*\}/);
    if (!match) return null;
    const parsed = JSON.parse(match[0]);
    const order = parsed.order || parsed;
    const validTargets = state.regions.filter(r => r.controller !== team).map(r => r.id);
    if (validTargets.length === 0) return null;
    const aggression = Math.max(0, Math.min(100, Number(order.aggression) || 50));
    return {
      order: {
        team, aggression, fortify: 100 - aggression,
        target_region: validTargets.includes(order.target_region) ? order.target_region : validTargets[0],
        diplomacy: typeof order.diplomacy === "string" ? order.diplomacy : (order.diplomacy ? JSON.stringify(order.diplomacy) : "no comment"),
      },
      reasoning: String(parsed.reasoning || ""),
    };
  } catch { return null; }
}

// ═══════════════════════════════════════════════════════════
// HTML
// ═══════════════════════════════════════════════════════════

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
<div class="score-strip">
  <div class="faction north"><span class="name">North</span><span class="pts" id="nPts">0</span></div>
  <div class="faction east"><span class="name">East</span><span class="pts" id="ePts">0</span></div>
  <div class="strip-center">
    <span class="turn-pill" id="turnPill">Turn 1 / 10</span>
    <span class="status-badge" id="statusBadge">Ready</span>
  </div>
  <div class="faction south"><span class="pts" id="sPts">0</span><span class="name">South</span></div>
  <button class="gear-btn" id="gearBtn" title="Settings">\u2699</button>
</div>
<div class="control-bar-wrap"><div class="n-fill" id="nFill" style="flex:1"></div><div class="e-fill" id="eFill" style="flex:1"></div><div class="s-fill" id="sFill" style="flex:1"></div></div>
<div class="arena-layout">
  <div class="map-stage" id="mapStage">
    <svg id="mapSvg" viewBox="0 0 900 570" preserveAspectRatio="xMidYMid meet" xmlns="http://www.w3.org/2000/svg"></svg>
    <div class="turn-banner" id="banner"><div class="phase" id="bannerPhase"></div><div class="detail" id="bannerDetail"></div></div>
    <div class="victory-overlay" id="victory"><div class="victory-content" id="victoryInner"></div></div>
    <div class="game-controls">
      <button class="primary" id="startBtn">Start Campaign</button>
      <button id="pauseBtn">Pause</button>
      <button id="stepBtn">Step</button>
      <span class="sep"></span>
      <button id="exportBtn">Export</button>
    </div>
  </div>
  <div class="chat-panel" id="chatPanel">
    <div class="channel-sidebar" id="channelSidebar"></div>
    <div class="message-area" id="messageArea">
      <div class="msg-header" id="msgHeader">#narrator</div>
      <div class="msg-feed" id="msgFeed"></div>
    </div>
  </div>
</div>
<div class="settings-drawer" id="drawer">
  <button class="close-btn" id="closeDrawer">\u2715 Close</button>
  <h3>Settings</h3>
  <div class="setting"><label>API Key</label><input type="password" id="apiKey" placeholder="sk-ant-..." /></div>
  <div class="setting"><label>North Model</label><select id="northModel"><option value="claude-sonnet-4-5" selected>claude-sonnet-4-5</option><option value="claude-opus-4-6">claude-opus-4-6</option></select></div>
  <div class="setting"><label>South Model</label><select id="southModel"><option value="claude-sonnet-4-5" selected>claude-sonnet-4-5</option><option value="claude-opus-4-6">claude-opus-4-6</option></select></div>
  <div class="setting"><label>East Model</label><select id="eastModel"><option value="claude-sonnet-4-5" selected>claude-sonnet-4-5</option><option value="claude-opus-4-6">claude-opus-4-6</option></select></div>
  <p class="settings-note">Each faction runs inside the real meerkat agent loop via WASM. The runtime calls Anthropic directly through browser fetch.</p>
</div>
<p class="status-bar" id="statusLine">Open settings (\u2699) to configure API key, then start.</p>`;

// ═══════════════════════════════════════════════════════════
// Refs + State
// ═══════════════════════════════════════════════════════════

const $ = <T extends Element>(id: string) => document.getElementById(id) as unknown as T;
let runtime: RuntimeModule | null = null;
let session: MatchSession | null = null;
let bannerTimer: ReturnType<typeof setTimeout> | null = null;
let activeChannel: ChannelId = "narrator";
const unreadCounts: Record<ChannelId, number> = { "north-ops": 0, "south-ops": 0, "east-ops": 0, negotiations: 0, narrator: 0 };

const apiKeyInput = $<HTMLInputElement>("apiKey");
apiKeyInput.value = sessionStorage.getItem("api_key") ?? "";
apiKeyInput.addEventListener("change", () => sessionStorage.setItem("api_key", apiKeyInput.value));

function setStatus(msg: string): void { ($<HTMLParagraphElement>("statusLine")).textContent = msg; }
function setBadge(text: string, thinking = false): void {
  const el = $<HTMLSpanElement>("statusBadge");
  el.textContent = text;
  el.className = `status-badge${thinking ? " thinking" : ""}`;
}
function showBanner(phase: string, detail: string, ms: number): void {
  if (bannerTimer) clearTimeout(bannerTimer);
  ($<HTMLDivElement>("bannerPhase")).textContent = phase;
  ($<HTMLDivElement>("bannerDetail")).textContent = detail;
  ($<HTMLDivElement>("banner")).classList.add("visible");
  bannerTimer = setTimeout(() => ($<HTMLDivElement>("banner")).classList.remove("visible"), ms);
}
function showVictory(winner: Team | "draw", state: ArenaState, narratorText?: string): void {
  const title = winner === "draw" ? "DRAW" : `${winner.toUpperCase()} WINS`;
  const narration = narratorText ? `<p class="victory-narration">${escapeHtml(narratorText)}</p>` : "";
  ($<HTMLDivElement>("victoryInner")).innerHTML = `<h2 class="${winner}">${title}</h2>${narration}<p>After ${state.turn} turns.</p><div class="final"><span class="north">N:${state.scores.north}</span><span class="east">E:${state.scores.east}</span><span class="south">S:${state.scores.south}</span></div>`;
  ($<HTMLDivElement>("victory")).classList.add("visible");
}

// ═══════════════════════════════════════════════════════════
// Render
// ═══════════════════════════════════════════════════════════

const GRAD: Record<Team, [string, string]> = { north: ["#2a7ac4","#143a6a"], south: ["#c4522a","#6a2414"], east: ["#24a068","#146040"] };

function renderMap(state: ArenaState, targets?: Record<Team, string>, captures?: Set<string>): void {
  const ctrl = new Map(state.regions.map(r => [r.id, r.controller]));
  let svg = `<defs>`;
  for (const [t, [c1, c2]] of Object.entries(GRAD))
    svg += `<linearGradient id="g${t[0].toUpperCase()}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${c1}" stop-opacity="0.85"/><stop offset="1" stop-color="${c2}" stop-opacity="0.7"/></linearGradient>`;
  svg += `<filter id="glow" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="6" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
  <filter id="terrainNoise"><feTurbulence type="fractalNoise" baseFrequency="0.04" numOctaves="3" result="n"/><feColorMatrix type="saturate" values="0" in="n" result="ng"/><feBlend in="SourceGraphic" in2="ng" mode="overlay" result="blended"/><feComposite in="blended" in2="SourceGraphic" operator="in"/></filter>
  <pattern id="grid" width="40" height="40" patternUnits="userSpaceOnUse"><path d="M40 0 L0 0 0 40" fill="none" stroke="rgba(50,70,100,0.03)" stroke-width="0.5"/></pattern></defs>`;
  svg += `<rect width="900" height="570" fill="url(#grid)"/>`;

  // Draw territories as irregular polygons
  for (const region of state.regions) {
    const t = MAP[region.id]; if (!t) continue;
    const lines = t.label.split("\n");
    svg += `<g class="territory" data-region="${region.id}">`;
    svg += `<path d="${t.path}" fill="url(#g${region.controller[0].toUpperCase()})" class="hex-bg" filter="url(#terrainNoise)"/>`;
    svg += `<path d="${t.path}" class="hex-border ${region.controller}"/>`;
    if (lines.length === 2) {
      svg += `<text class="region-label" x="${t.center.x}" y="${t.center.y - 6}">${lines[0]}</text>`;
      svg += `<text class="region-label" x="${t.center.x}" y="${t.center.y + 7}">${lines[1]}</text>`;
    } else {
      svg += `<text class="region-label" x="${t.center.x}" y="${t.center.y + 2}">${t.label}</text>`;
    }
    svg += `<text class="region-stats" x="${t.center.x}" y="${t.center.y + 20}">\u2694${region.defense} \u2605${region.value}</text>`;
    svg += `</g>`;
  }

  // Front lines: shared borders between different factions
  for (const [a, b] of EDGES) {
    if (ctrl.get(a) !== ctrl.get(b)) {
      const pa = MAP[a], pb = MAP[b];
      svg += `<line x1="${pa.center.x}" y1="${pa.center.y}" x2="${pb.center.x}" y2="${pb.center.y}" class="edge-front"/>`;
    }
  }

  // Capture flash animation
  if (captures) for (const id of captures) {
    const t = MAP[id], team = ctrl.get(id);
    if (t && team) svg += `<path d="${t.path}" class="captured-flash to-${team}" filter="url(#glow)"/>`;
  }

  // Target indicators
  if (targets) for (const [team, rid] of Object.entries(targets)) {
    const t = MAP[rid];
    if (t) svg += `<circle cx="${t.center.x}" cy="${t.center.y}" r="28" class="target-ring ${team}"/>`;
  }

  ($<SVGSVGElement>("mapSvg")).innerHTML = svg;
}

function renderScore(state: ArenaState): void {
  ($<HTMLSpanElement>("nPts")).textContent = String(state.scores.north);
  ($<HTMLSpanElement>("sPts")).textContent = String(state.scores.south);
  ($<HTMLSpanElement>("ePts")).textContent = String(state.scores.east);
  ($<HTMLSpanElement>("turnPill")).textContent = `Turn ${state.turn} / ${state.max_turns}`;
  const total = state.scores.north + state.scores.south + state.scores.east || 1;
  ($<HTMLDivElement>("nFill")).style.flex = String(state.scores.north / total);
  ($<HTMLDivElement>("sFill")).style.flex = String(state.scores.south / total);
  ($<HTMLDivElement>("eFill")).style.flex = String(state.scores.east / total);
}

// ── Channel UI ──

const ROLE_ICONS: Record<MessageRole, string> = { planner: "\u{1F9ED}", operator: "\u{1F6E1}\uFE0F", narrator: "\u{1F4DC}", system: "\u2699\uFE0F" };
const TEAM_CHANNEL: Record<Team, ChannelId> = { north: "north-ops", south: "south-ops", east: "east-ops" };

function pushMessage(msg: ChatMessage): void {
  if (!session) return;
  session.messages.push(msg);
  if (msg.channel !== activeChannel) { unreadCounts[msg.channel]++; }
  renderChannelSidebar();
  if (msg.channel === activeChannel) renderMessages();
}

function selectChannel(ch: ChannelId): void {
  activeChannel = ch;
  unreadCounts[ch] = 0;
  renderChannelSidebar();
  renderMessages();
}

function renderChannelSidebar(): void {
  const el = $<HTMLDivElement>("channelSidebar");
  el.innerHTML = CHANNELS.map(ch => {
    const active = ch.id === activeChannel ? " active" : "";
    const unread = unreadCounts[ch.id] > 0 ? `<span class="unread-badge">${unreadCounts[ch.id]}</span>` : "";
    return `<button class="ch-btn${active}" data-ch="${ch.id}">${ch.icon} ${ch.label}${unread}</button>`;
  }).join("");
  el.querySelectorAll(".ch-btn").forEach(btn => {
    btn.addEventListener("click", () => selectChannel(btn.getAttribute("data-ch") as ChannelId));
  });
}

function renderMessages(): void {
  const header = $<HTMLDivElement>("msgHeader");
  const feed = $<HTMLDivElement>("msgFeed");
  const ch = CHANNELS.find(c => c.id === activeChannel);
  header.textContent = ch ? `${ch.icon} ${ch.label}` : activeChannel;

  const msgs = session ? session.messages.filter(m => m.channel === activeChannel) : [];
  feed.innerHTML = msgs.map(m => {
    const fClass = m.faction === "neutral" ? "" : ` ${m.faction}`;
    const icon = ROLE_ICONS[m.role];
    const roleLabel = m.role.charAt(0).toUpperCase() + m.role.slice(1);
    const factionLabel = m.faction === "neutral" ? "" : ` [${m.faction.toUpperCase()}]`;
    return `<div class="chat-msg${fClass}"><div class="msg-meta"><span class="msg-icon">${icon}</span><span class="msg-role">${roleLabel}${factionLabel}</span><span class="msg-turn">T${m.turn}</span></div><div class="msg-body">${escapeHtml(m.content)}</div></div>`;
  }).join("");
  feed.scrollTop = feed.scrollHeight;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// (dispatch messages are now pushed directly in runFactionTurn and tick)

// ═══════════════════════════════════════════════════════════
// Runtime + Game Loop
// ═══════════════════════════════════════════════════════════

async function loadRuntime(): Promise<RuntimeModule> {
  if (runtime) return runtime;
  const url = new URL("./runtime.js", window.location.href).toString();
  const mod = (await import(/* @vite-ignore */ url)) as RuntimeModule;
  await mod.default();
  runtime = mod;
  return mod;
}

async function runFactionTurn(f: FactionSession, signals: Record<Team, string>): Promise<TurnDecision | null> {
  if (!runtime || !session) return null;
  const turn = session.state.turn;
  const ch = TEAM_CHANNEL[f.team];

  // Step 1: Planner phase
  setStatus(`Round ${turn}: ${f.team.toUpperCase()} planner thinking...`);
  const plannerPrompt = buildPlannerPrompt(f.team, session.state, signals);
  const pj = await runtime.start_turn(f.handle, plannerPrompt, "{}");
  const pr = JSON.parse(pj);
  const plannerText = pr.text as string;

  // Post planner message to faction channel
  const plannerDecision = parseDecision(f.team, plannerText, session.state);
  pushMessage({ channel: ch, role: "planner", faction: f.team, content: plannerDecision?.reasoning || plannerText.slice(0, 200), turn });

  if (!session.running) return null;

  // Step 2: Operator phase
  setStatus(`Round ${turn}: ${f.team.toUpperCase()} operator validating...`);
  const operatorPrompt = buildOperatorPrompt(f.team, plannerText);
  const oj = await runtime.start_turn(f.handle, operatorPrompt, "{}");
  const or_ = JSON.parse(oj);
  const operatorText = or_.text as string;

  // Post operator message to faction channel
  const operatorDecision = parseDecision(f.team, operatorText, session.state);
  pushMessage({ channel: ch, role: "operator", faction: f.team, content: operatorDecision?.reasoning || "Order committed.", turn });

  // Use operator's decision (final committed order), fall back to planner's
  return operatorDecision || plannerDecision;
}

async function tick(): Promise<void> {
  if (!runtime || !session || !session.running) return;
  if (session.state.winner) { session.running = false; showVictory(session.state.winner, session.state); setBadge("Complete"); return; }
  try {
    const turn = session.state.turn;
    setBadge("Thinking...", true);
    setStatus(`Round ${turn}: Factions deliberating...`);
    const signals: Record<Team, string> = { north: "opening", south: "opening", east: "opening" };
    for (const d of session.dispatches.slice(-3)) signals[d.team] = d.diplomacy;

    // Run all factions in parallel (each does planner + operator internally)
    const results = await Promise.all(session.factions.map(f => runFactionTurn(f, signals)));
    if (!session.running) return;

    const decisions: TurnDecision[] = [];
    for (let i = 0; i < results.length; i++) {
      const d = results[i];
      const team = session.factions[i].team;
      if (d) {
        decisions.push(d);
        session.dispatches.push({ team, turn, diplomacy: d.order.diplomacy, reasoning: d.reasoning });
        // Post diplomatic signal to negotiations channel
        if (d.order.diplomacy && d.order.diplomacy !== "no comment") {
          pushMessage({ channel: "negotiations", role: "planner", faction: team, content: d.order.diplomacy, turn });
        }
      }
    }
    if (decisions.length === 0) { session.running = false; setBadge("Error"); setStatus("No valid orders."); return; }

    const targets = Object.fromEntries(decisions.map(d => [d.order.team, d.order.target_region])) as Record<Team, string>;
    showBanner(`Round ${turn}`, decisions.map(d => `${d.order.team}\u2192${d.order.target_region.replace(/-/g," ")}`).join(" \u2022 "), 2500);
    renderMap(session.state, targets);
    await new Promise(r => setTimeout(r, 1500));
    if (!session.running) return;

    const newState = resolveOrders(session.state, decisions);
    const captures = new Set<string>();
    const nc = new Map(newState.regions.map(r => [r.id, r.controller]));
    for (const [id, t] of nc) if (session.prevControllers.get(id) !== t) captures.add(id);
    session.prevControllers = nc;
    session.state = newState;
    renderMap(newState, undefined, captures);
    renderScore(newState);
    if (captures.size) showBanner("Territory Changed", [...captures].map(id => id.replace(/-/g," ")).join(", "), 2000);

    // Narrator: summarize the turn
    if (session.narratorHandle && runtime) {
      try {
        const captureList = [...captures].map(id => id.replace(/-/g, " ")).join(", ");
        const narratorPrompt = `Turn ${turn} summary:
${decisions.map(d => `${d.order.team.toUpperCase()} attacked ${d.order.target_region.replace(/-/g," ")} (aggression=${d.order.aggression}). Diplomacy: "${d.order.diplomacy}"`).join("\n")}
${captures.size > 0 ? `Territories changed hands: ${captureList}` : "No territories changed hands."}
Scores: ${TEAMS.map(t => `${t}=${newState.scores[t]}`).join(", ")}
${newState.winner ? `GAME OVER: ${newState.winner === "draw" ? "It's a draw!" : `${newState.winner.toUpperCase()} wins!`}` : ""}

Write 2-3 sentences of dramatic war narrative.`;
        const nj = await runtime.start_turn(session.narratorHandle, narratorPrompt, "{}");
        const nr = JSON.parse(nj);
        if (nr.text) {
          pushMessage({ channel: "narrator", role: "narrator", faction: "neutral", content: nr.text, turn });
        }
      } catch { /* narrator failure is non-fatal */ }
    }

    setBadge("Live");
    setStatus(`Round ${turn} resolved.`);
    if (newState.winner) {
      session.running = false;
      await new Promise(r => setTimeout(r, 800));
      // Show victory with narrator's final words if available
      const lastNarrator = session.messages.filter(m => m.channel === "narrator").pop();
      showVictory(newState.winner, newState, lastNarrator?.content);
      setBadge("Complete");
      return;
    }
    await new Promise(r => setTimeout(r, 2000));
    if (session.running) void tick();
  } catch (error) { session.running = false; setBadge("Error"); setStatus(`Error: ${error instanceof Error ? error.message : String(error)}`); }
}

async function startMatch(): Promise<void> {
  try {
    ($<HTMLDivElement>("victory")).classList.remove("visible");
    const apiKey = apiKeyInput.value.trim();
    if (!apiKey) { setStatus("Enter API key in settings."); return; }
    setBadge("Loading...", true);
    const mod = await loadRuntime();
    const packs = await Promise.all(["/north.mobpack","/south.mobpack","/east.mobpack","/narrator.mobpack"].map(async p => { const r = await fetch(p, {cache:"no-store"}); return new Uint8Array(await r.arrayBuffer()); }));
    const models: Record<Team, string> = { north: ($<HTMLSelectElement>("northModel")).value, south: ($<HTMLSelectElement>("southModel")).value, east: ($<HTMLSelectElement>("eastModel")).value };
    const factions: FactionSession[] = TEAMS.map((team, i) => {
      const h = mod.create_session(packs[i], JSON.stringify({ model: models[team], api_key: apiKey, max_tokens: 1024 }));
      return { team, handle: h, model: models[team] };
    });
    // Create narrator session (4th pack)
    let narratorHandle: number | null = null;
    try {
      narratorHandle = mod.create_session(packs[3], JSON.stringify({ model: models.north, api_key: apiKey, max_tokens: 512 }));
    } catch { /* narrator is optional — game works without it */ }
    const state = defaultState();
    session = { factions, narratorHandle, state, dispatches: [], messages: [], running: true, prevControllers: new Map(state.regions.map(r => [r.id, r.controller])) };
    renderMap(state); renderScore(state);
    showBanner("Campaign Begins", `3 factions, real LLM reasoning`, 2000);
    setBadge("Live"); setStatus("Campaign started.");
    ($<HTMLDivElement>("drawer")).classList.remove("open");
    await tick();
  } catch (e) { setBadge("Error"); setStatus(`Failed: ${e instanceof Error ? e.message : String(e)}`); }
}

// ═══════════════════════════════════════════════════════════
// Event Handlers
// ═══════════════════════════════════════════════════════════

document.getElementById("startBtn")!.addEventListener("click", () => void startMatch());
document.getElementById("pauseBtn")!.addEventListener("click", () => { if (!session) return; session.running = !session.running; (document.getElementById("pauseBtn") as HTMLButtonElement).textContent = session.running ? "Pause" : "Resume"; if (session.running) void tick(); });
document.getElementById("stepBtn")!.addEventListener("click", () => { if (!session) return; session.running = true; (document.getElementById("pauseBtn") as HTMLButtonElement).textContent = "Resume"; const doStep = async () => { await tick(); if (session) session.running = false; (document.getElementById("pauseBtn") as HTMLButtonElement).textContent = "Resume"; }; void doStep(); });
document.getElementById("exportBtn")!.addEventListener("click", () => { if (!session) return; const b = new Blob([JSON.stringify({state:session.state,dispatches:session.dispatches},null,2)],{type:"application/json"}); const a = document.createElement("a"); a.href = URL.createObjectURL(b); a.download = "replay.json"; a.click(); });
document.getElementById("gearBtn")!.addEventListener("click", () => ($<HTMLDivElement>("drawer")).classList.toggle("open"));
document.getElementById("closeDrawer")!.addEventListener("click", () => ($<HTMLDivElement>("drawer")).classList.remove("open"));

renderMap(defaultState());
renderScore(defaultState());
renderChannelSidebar();
renderMessages();
