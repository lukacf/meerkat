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
interface OrderSet { team: Team; aggression: number; fortify: number; target_region: string }
interface TurnDecision { order: OrderSet; reasoning: string }

// ── DM Channels ──
type ChannelId =
  | "n-plan-op" | "s-plan-op" | "e-plan-op"       // planner↔operator
  | "n-plan-amb" | "s-plan-amb" | "e-plan-amb"     // planner↔ambassador
  | "e-n-diplo" | "e-s-diplo" | "n-s-diplo"        // ambassador↔ambassador (alphabetical sort)
  | "narrator";
type MessageRole = "planner" | "operator" | "ambassador" | "narrator" | "system";
interface ChatMessage { channel: ChannelId; role: MessageRole; faction: Team | "neutral"; content: string; turn: number }

const CHANNELS: { id: ChannelId; label: string; icon: string }[] = [
  { id: "n-plan-op",  label: "N: Plan\u2194Op",  icon: "\u{1F9ED}" },
  { id: "s-plan-op",  label: "S: Plan\u2194Op",  icon: "\u{1F9ED}" },
  { id: "e-plan-op",  label: "E: Plan\u2194Op",  icon: "\u{1F9ED}" },
  { id: "n-plan-amb", label: "N: Plan\u2194Amb", icon: "\u{1F4E8}" },
  { id: "s-plan-amb", label: "S: Plan\u2194Amb", icon: "\u{1F4E8}" },
  { id: "e-plan-amb", label: "E: Plan\u2194Amb", icon: "\u{1F4E8}" },
  { id: "e-n-diplo",  label: "N\u2194E Diplo",   icon: "\u{1F91D}" },
  { id: "e-s-diplo",  label: "S\u2194E Diplo",   icon: "\u{1F91D}" },
  { id: "n-s-diplo",  label: "N\u2194S Diplo",   icon: "\u{1F91D}" },
  { id: "narrator",   label: "#narrator",         icon: "\u{1F4DC}" },
];

// ── WASM Runtime ──
interface RuntimeModule {
  default: () => Promise<unknown>;
  init_runtime_from_config: (configJson: string) => unknown;
  mob_create: (definitionJson: string) => Promise<unknown>;
  mob_spawn: (mobId: string, specsJson: string) => Promise<unknown>;
  mob_wire: (mobId: string, a: string, b: string) => Promise<void>;
  wire_cross_mob: (mobA: string, meerkatA: string, mobB: string, meerkatB: string) => Promise<void>;
  mob_send_message: (mobId: string, meerkatId: string, message: string) => Promise<void>;
  mob_run_flow: (mobId: string, flowId: string, paramsJson: string) => Promise<unknown>;
  mob_flow_status: (mobId: string, runId: string) => Promise<unknown>;
  mob_list_members: (mobId: string) => Promise<unknown>;
  mob_member_subscribe: (mobId: string, meerkatId: string) => Promise<number>;
  poll_subscription: (handle: number) => string;
  close_subscription: (handle: number) => void;
}

interface AgentSub { meerkatId: string; handle: number; role: MessageRole; team: Team }
interface FactionMob { team: Team; mobId: string }
interface MatchSession {
  factions: FactionMob[];
  narratorMobId: string | null;
  subs: AgentSub[];  // event subscriptions for all 9 agents
  state: ArenaState;
  messages: ChatMessage[];
  running: boolean;
  prevControllers: Map<string, Team>;
}

// ═══════════════════════════════════════════════════════════
// Map Data
// ═══════════════════════════════════════════════════════════

interface TerritoryInfo { path: string; center: { x: number; y: number }; label: string }

const MAP: Record<string, TerritoryInfo> = {
  "north-capital":  { path: "M350,15 L390,10 L460,8 L520,14 L550,25 L540,60 L510,95 L450,110 L390,105 L355,80 L340,50 Z", center: { x: 450, y: 58 }, label: "North\nCapital" },
  "north-harbor":   { path: "M140,75 L195,48 L250,35 L310,22 L350,15 L340,50 L355,80 L390,105 L340,130 L280,145 L220,150 L165,135 L130,110 Z", center: { x: 255, y: 88 }, label: "North\nHarbor" },
  "north-ridge":    { path: "M550,25 L610,30 L680,50 L740,78 L760,110 L730,140 L670,150 L610,145 L560,130 L510,95 L540,60 Z", center: { x: 640, y: 95 }, label: "North\nRidge" },
  "obsidian-gate":  { path: "M130,110 L165,135 L220,150 L280,145 L340,130 L390,105 L450,110 L440,155 L410,200 L350,225 L280,230 L210,220 L155,195 L120,160 Z", center: { x: 290, y: 170 }, label: "Obsidian\nGate" },
  "crimson-pass":   { path: "M450,110 L510,95 L560,130 L610,145 L670,150 L660,195 L630,230 L570,250 L500,245 L440,230 L410,200 L440,155 Z", center: { x: 545, y: 180 }, label: "Crimson\nPass" },
  "glass-frontier": { path: "M120,160 L155,195 L210,220 L280,230 L350,225 L410,200 L440,230 L430,280 L390,320 L320,340 L240,330 L170,305 L120,265 L100,220 Z", center: { x: 270, y: 270 }, label: "Glass\nFrontier" },
  "ember-crossing": { path: "M670,150 L730,140 L760,110 L800,145 L830,195 L840,250 L820,300 L770,330 L710,335 L650,320 L610,285 L590,250 L630,230 L660,195 Z", center: { x: 720, y: 240 }, label: "Ember\nCrossing" },
  "saffron-fields": { path: "M440,230 L500,245 L570,250 L590,250 L610,285 L650,320 L630,360 L570,385 L500,390 L430,380 L380,355 L390,320 L430,280 Z", center: { x: 510, y: 315 }, label: "Saffron\nFields" },
  "southern-rail":  { path: "M100,220 L120,265 L170,305 L240,330 L320,340 L300,385 L260,420 L200,440 L140,430 L90,400 L65,350 L70,290 Z", center: { x: 175, y: 355 }, label: "Southern\nRail" },
  "ash-basin":      { path: "M650,320 L710,335 L770,330 L820,300 L850,340 L855,395 L830,440 L780,465 L720,470 L660,455 L620,420 L600,385 L630,360 Z", center: { x: 740, y: 395 }, label: "Ash\nBasin" },
  "mercury-delta":  { path: "M320,340 L390,320 L380,355 L430,380 L500,390 L480,430 L440,465 L380,485 L310,490 L250,475 L200,440 L260,420 L300,385 Z", center: { x: 355, y: 425 }, label: "Mercury\nDelta" },
  "south-capital":  { path: "M500,390 L570,385 L630,360 L600,385 L620,420 L660,455 L640,490 L590,520 L530,535 L460,530 L400,510 L380,485 L440,465 L480,430 Z", center: { x: 530, y: 465 }, label: "South\nCapital" },
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
// Game Engine
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

// ═══════════════════════════════════════════════════════════
// Mob Definitions — autonomous agents with comms
// ═══════════════════════════════════════════════════════════

const GAME_RULES = `GAME: 3-faction territory war (North, South, East). 12 territories with defense (0-100) and value (2-4 pts).
COMBAT: aggression + rand(0-12) vs defense + rand(0-8). Capture if greater. Fortify = 100 - aggression.
SCORING: Each turn, factions earn sum of their territories' values. 10 turns total. Highest cumulative score wins.
DIPLOMACY: 2v1 is decisive. Alliances, betrayals, and threats are critical to winning.`;

const NARRATOR_SCHEMA = {
  type: "object",
  properties: { narrative: { type: "string", description: "2-3 sentences of dramatic war narrative" } },
  required: ["narrative"],
  additionalProperties: false,
};

function buildFactionDefinition(team: Team, model: string): object {
  return {
    id: `diplomacy-${team}`,
    profiles: {
      planner: {
        model, runtime_mode: "autonomous_host",
        tools: { comms: true },
        peer_description: `${team} strategic planner — analyzes territory, proposes attacks, coordinates with operator and ambassador`,
        external_addressable: true,
      },
      operator: {
        model, runtime_mode: "autonomous_host",
        tools: { comms: true },
        peer_description: `${team} military operator — validates orders, challenges assumptions, executes final commands`,
        external_addressable: true,
      },
      ambassador: {
        model, runtime_mode: "autonomous_host",
        tools: { comms: true },
        peer_description: `${team} diplomatic ambassador — negotiates alliances and truces with foreign ambassadors`,
        external_addressable: true,
      },
    },
    // No role_wiring — we wire explicitly after spawn so planner↔operator
    // and planner↔ambassador are paired, but operator↔ambassador are not
    // (ambassador reports to planner, not directly to operator)
    wiring: {},
    // Narrator flow only — faction agents converse via comms, not flows
    flows: {},
  };
}

function buildNarratorDefinition(model: string): object {
  return {
    id: "diplomacy-narrator",
    profiles: {
      narrator: {
        model, runtime_mode: "turn_driven",
        peer_description: "War correspondent narrator",
        external_addressable: false,
        output_schema: NARRATOR_SCHEMA,
      },
    },
    flows: {
      narrate: {
        steps: {
          summarize: {
            role: "narrator",
            message: `You are a dramatic war correspondent narrating an epic 3-faction territorial conflict.

Turn summary:
{{params.summary}}

Write 2-3 sentences of vivid, gripping war narrative. Reference specific territories by name. Describe the drama of alliances forming and breaking, the clash of armies, the cunning of diplomats. Make the reader feel the tension.

Respond with ONLY a JSON object.`,
            dispatch_mode: "one_to_one",
          },
        },
      },
    },
  };
}

function serializeState(team: Team, state: ArenaState): string {
  const ours = state.regions.filter(r => r.controller === team);
  const enemies = state.regions.filter(r => r.controller !== team);
  return JSON.stringify({
    turn: state.turn, max_turns: state.max_turns, team,
    scores: state.scores,
    your_territories: ours.map(r => ({ id: r.id, defense: r.defense, value: r.value })),
    enemy_territories: enemies.map(r => ({ id: r.id, controller: r.controller, defense: r.defense, value: r.value })),
  });
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
  <div class="setting"><label>Model</label><select id="modelSelect"><option value="claude-sonnet-4-5" selected>claude-sonnet-4-5</option><option value="claude-opus-4-6">claude-opus-4-6</option><option value="gpt-5.2">gpt-5.2</option><option value="gemini-3-flash-preview">gemini-3-flash-preview</option></select></div>
  <p class="settings-note">Each faction is a mob of 3 autonomous agents (planner, operator, ambassador) communicating via comms. Ambassadors negotiate across factions. Sessions persist across turns.</p>
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
const unreadCounts: Record<ChannelId, number> = Object.fromEntries(CHANNELS.map(c => [c.id, 0])) as Record<ChannelId, number>;

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
  for (const [a, b] of EDGES) {
    if (ctrl.get(a) !== ctrl.get(b)) {
      const pa = MAP[a], pb = MAP[b];
      svg += `<line x1="${pa.center.x}" y1="${pa.center.y}" x2="${pb.center.x}" y2="${pb.center.y}" class="edge-front"/>`;
    }
  }
  if (captures) for (const id of captures) {
    const t = MAP[id], team = ctrl.get(id);
    if (t && team) svg += `<path d="${t.path}" class="captured-flash to-${team}" filter="url(#glow)"/>`;
  }
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

const ROLE_ICONS: Record<MessageRole, string> = {
  planner: "\u{1F9ED}", operator: "\u{1F6E1}\uFE0F", ambassador: "\u{1F3F3}\uFE0F",
  narrator: "\u{1F4DC}", system: "\u2699\uFE0F",
};

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

// ═══════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════

function parseJsResult(val: unknown): string { return typeof val === "string" ? val : String(val); }
function sleep(ms: number): Promise<void> { return new Promise(r => setTimeout(r, ms)); }

async function loadRuntime(): Promise<RuntimeModule> {
  if (runtime) return runtime;
  const url = new URL("./runtime.js", window.location.href).toString();
  const mod = (await import(/* @vite-ignore */ url)) as RuntimeModule;
  await mod.default();
  runtime = mod;
  return mod;
}

/** Extract meerkat_id from a full peer address like "diplomacy-north/ambassador/north-ambassador" */
function extractMeerkatId(peerAddress: string): string {
  const parts = peerAddress.split("/");
  return parts[parts.length - 1] || peerAddress;
}

/** Resolve meerkat_id or peer address → team. */
function meerkatTeam(id: string): Team | null {
  const mid = extractMeerkatId(id);
  for (const t of TEAMS) if (mid.startsWith(t)) return t;
  return null;
}

/** Resolve meerkat_id or peer address → role. */
function meerkatRole(id: string): MessageRole {
  const mid = extractMeerkatId(id);
  if (mid.includes("planner")) return "planner";
  if (mid.includes("operator")) return "operator";
  if (mid.includes("ambassador")) return "ambassador";
  return "system";
}

/** Map a (sender, receiver) pair to the correct DM channel. */
function dmChannel(senderId: string, receiverId: string): ChannelId | null {
  const sTeam = meerkatTeam(senderId);
  const rTeam = meerkatTeam(receiverId);
  if (!sTeam || !rTeam) return null;

  const sRole = meerkatRole(senderId);
  const rRole = meerkatRole(receiverId);

  // Cross-faction: ambassador↔ambassador
  if (sTeam !== rTeam) {
    const sorted = [sTeam, rTeam].sort();
    return `${sorted[0][0]}-${sorted[1][0]}-diplo` as ChannelId;
  }

  // Same faction: planner↔operator or planner↔ambassador
  if ((sRole === "planner" && rRole === "operator") || (sRole === "operator" && rRole === "planner"))
    return `${sTeam[0]}-plan-op` as ChannelId;
  if ((sRole === "planner" && rRole === "ambassador") || (sRole === "ambassador" && rRole === "planner"))
    return `${sTeam[0]}-plan-amb` as ChannelId;

  return null;
}

// ═══════════════════════════════════════════════════════════
// Event Streaming → DM Channels
// ═══════════════════════════════════════════════════════════

/** Poll all agent subscriptions and push events to DM channels. Returns count of new messages. */
function drainAllEvents(mod: RuntimeModule, turn: number): number {
  if (!session) return 0;
  let count = 0;
  for (const sub of session.subs) {
    try {
      const raw = mod.poll_subscription(sub.handle);
      const events: any[] = JSON.parse(raw);
      // Uncomment for debugging: if (events.length > 0) console.log(`[${sub.meerkatId}] ${events.length} events`);
      for (const event of events) {
        // Outgoing comms: agent used "send" tool (not "send_message")
        // AgentEvent::ToolCallRequested { id, name, args: { kind, to, body } }
        if (event.type === "tool_call_requested" && event.name === "send") {
          try {
            const args = typeof event.args === "string" ? JSON.parse(event.args) : event.args;
            const to = args.to || "";
            const body = args.body || "";
            if (to && body) {
              const ch = dmChannel(sub.meerkatId, to);
              if (ch) {
                const targetRole = meerkatRole(to);
                pushMessage({ channel: ch, role: sub.role, faction: sub.team, content: body, turn });
              }
            }
          } catch { /* skip parse errors */ }
        }
        // Agent's text output — AgentEvent::TextComplete { content }
        if (event.type === "text_complete" && event.content) {
          const ch = `${sub.team[0]}-plan-op` as ChannelId;
          if (sub.role === "planner" || sub.role === "operator") {
            pushMessage({ channel: ch, role: sub.role, faction: sub.team, content: event.content.slice(0, 500), turn });
            count++;
          }
        }
        // Count ANY event for quiescence detection (even text_delta)
        count++;
      }
    } catch { /* poll error */ }
  }
  return count;
}

// ═══════════════════════════════════════════════════════════
// Game Loop — Autonomous Agents
// ═══════════════════════════════════════════════════════════

async function tick(): Promise<void> {
  if (!runtime || !session || !session.running) return;
  if (session.state.winner) { session.running = false; showVictory(session.state.winner, session.state); setBadge("Complete"); return; }

  const mod = runtime;
  const turn = session.state.turn;

  try {
    setBadge("Agents working...", true);
    setStatus(`Round ${turn}: Triggering planners — agents will deliberate, negotiate, and finalize autonomously...`);
    showBanner(`Round ${turn}`, "Agents are deliberating and negotiating...", 8000);

    // Trigger each planner with the game state.
    // The planner's system prompt tells it to:
    //   1. Discuss with operator (comms)
    //   2. Brief ambassador with diplomatic intent (comms)
    //   3. Ambassador negotiates with foreign ambassadors (comms, cross-mob)
    //   4. Ambassador reports back to planner (comms)
    //   5. Planner tells operator to finalize
    //   6. Operator produces final order
    for (const f of session.factions) {
      const stateStr = serializeState(f.team, session.state);
      const prompt = `=== TURN ${turn} ===
${GAME_RULES}

You are the PLANNER for ${f.team.toUpperCase()}.

CURRENT STATE:
${stateStr}

PROTOCOL:
1. Use send_message to your OPERATOR (${f.team}-operator) to discuss strategy. Debate which territory to attack, what aggression level to use. Challenge each other's ideas. Agree on a plan.
2. Use send_message to your AMBASSADOR (${f.team}-ambassador) with your diplomatic intent — what alliances or threats should they pursue.
3. Wait for your ambassador to report back with negotiation results.
4. Based on diplomatic outcomes, send your OPERATOR the final instruction. Tell them to produce the order as: FINAL ORDER: target=<region-id> aggression=<0-100>
5. The operator will output the final order.

Start by messaging your operator to discuss the situation.`;

      try {
        await mod.mob_send_message(f.mobId, `${f.team}-planner`, prompt);
      } catch (e) {
        console.warn(`Failed to trigger ${f.team} planner:`, e);
      }
    }

    // Poll events until quiescence (agents done talking)
    const MAX_WAIT_MS = 120_000; // 2 minutes max per turn
    const QUIET_THRESHOLD = 8_000; // 8 seconds of silence = done
    const deadline = Date.now() + MAX_WAIT_MS;
    let lastEventTime = Date.now();

    while (Date.now() < deadline && session.running) {
      await sleep(300);
      const newMessages = drainAllEvents(mod, turn);
      if (newMessages > 0) {
        lastEventTime = Date.now();
        // Update status with activity indicator
        const totalDMs = session.messages.filter(m => m.turn === turn).length;
        setStatus(`Round ${turn}: ${totalDMs} messages exchanged...`);
      }
      if (Date.now() - lastEventTime > QUIET_THRESHOLD) break;
    }
    if (!session.running) return;

    // Extract final orders from operator text outputs this turn.
    // Look for "FINAL ORDER: target=X aggression=Y" pattern in operator messages.
    const decisions: TurnDecision[] = [];
    for (const f of session.factions) {
      const opMsgs = session.messages.filter(
        m => m.turn === turn && m.faction === f.team && m.role === "operator"
      );
      // Search backwards for the most recent message containing FINAL ORDER
      let order: OrderSet | null = null;
      let reasoning = "";
      for (let i = opMsgs.length - 1; i >= 0; i--) {
        const text = opMsgs[i].content;
        const match = text.match(/FINAL\s*ORDER\s*:?\s*target\s*=\s*([\w-]+)\s*aggression\s*=\s*(\d+)/i);
        if (match) {
          const targetRegion = match[1];
          const aggression = Math.max(0, Math.min(100, parseInt(match[2], 10)));
          const validTargets = session.state.regions.filter(r => r.controller !== f.team).map(r => r.id);
          order = {
            team: f.team, aggression, fortify: 100 - aggression,
            target_region: validTargets.includes(targetRegion) ? targetRegion : validTargets[0],
          };
          reasoning = text;
          break;
        }
      }
      // Fallback: try JSON extraction from any operator message
      if (!order) {
        for (let i = opMsgs.length - 1; i >= 0; i--) {
          try {
            const jsonMatch = opMsgs[i].content.match(/\{[\s\S]*\}/);
            if (jsonMatch) {
              const parsed = JSON.parse(jsonMatch[0]);
              const o = parsed.order || parsed;
              if (o.target_region && o.aggression != null) {
                const validTargets = session.state.regions.filter(r => r.controller !== f.team).map(r => r.id);
                const aggression = Math.max(0, Math.min(100, Number(o.aggression) || 50));
                order = {
                  team: f.team, aggression, fortify: 100 - aggression,
                  target_region: validTargets.includes(o.target_region) ? o.target_region : validTargets[0],
                };
                reasoning = parsed.reasoning || opMsgs[i].content;
                break;
              }
            }
          } catch { /* skip parse errors */ }
        }
      }
      // Last resort fallback
      if (!order) {
        const validTargets = session.state.regions.filter(r => r.controller !== f.team).map(r => r.id);
        order = { team: f.team, aggression: 50, fortify: 50, target_region: validTargets[0] };
        reasoning = "Fallback: no clear order from operator.";
      }
      decisions.push({ order, reasoning });
    }

    // Resolve & Render
    const targets = Object.fromEntries(decisions.map(d => [d.order.team, d.order.target_region])) as Record<Team, string>;
    showBanner(`Round ${turn}`, decisions.map(d => `${d.order.team}\u2192${d.order.target_region.replace(/-/g," ")}`).join(" \u2022 "), 2500);
    renderMap(session.state, targets);
    await sleep(1500);
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

    // Narrator
    if (session.narratorMobId) {
      try {
        const summary = [
          ...decisions.map(d => `${d.order.team.toUpperCase()} attacked ${d.order.target_region.replace(/-/g," ")} (aggression=${d.order.aggression}). ${d.reasoning.slice(0, 100)}`),
          captures.size > 0 ? `Territories changed: ${[...captures].map(id => id.replace(/-/g, " ")).join(", ")}` : "No territories changed.",
          `Scores: ${TEAMS.map(t => `${t}=${newState.scores[t]}`).join(", ")}`,
          // Include diplomatic context from DM channels
          ...session.messages.filter(m => m.turn === turn && m.role === "ambassador").slice(-3)
            .map(m => `${m.faction} ambassador said: "${m.content.slice(0, 80)}"`),
        ].join("\n");
        const runId = parseJsResult(await mod.mob_run_flow(session.narratorMobId, "narrate", JSON.stringify({ summary })));
        for (let i = 0; i < 30; i++) {
          await sleep(500);
          const raw = parseJsResult(await mod.mob_flow_status(session.narratorMobId, runId));
          if (raw === "null") continue;
          const result = JSON.parse(raw);
          if (result.status === "completed") {
            const step = result.step_ledger?.find((s: any) => s.step_id === "summarize" && s.status === "completed");
            if (step?.output?.narrative) {
              pushMessage({ channel: "narrator", role: "narrator", faction: "neutral", content: step.output.narrative, turn });
            }
            break;
          }
          if (result.status !== "running" && result.status !== "pending") break;
        }
      } catch { /* narrator non-fatal */ }
    }

    setBadge("Live"); setStatus(`Round ${turn} resolved.`);
    if (newState.winner) {
      session.running = false;
      await sleep(800);
      const lastNarrator = session.messages.filter(m => m.channel === "narrator" && m.role === "narrator").pop();
      showVictory(newState.winner, newState, lastNarrator?.content);
      setBadge("Complete");
      return;
    }
    await sleep(2000);
    if (session.running) void tick();
  } catch (error) {
    session.running = false; setBadge("Error");
    setStatus(`Error: ${error instanceof Error ? error.message : String(error)}`);
  }
}

// ═══════════════════════════════════════════════════════════
// Start Match
// ═══════════════════════════════════════════════════════════

async function startMatch(): Promise<void> {
  try {
    ($<HTMLDivElement>("victory")).classList.remove("visible");
    const apiKey = apiKeyInput.value.trim();
    if (!apiKey) { setStatus("Enter API key in settings."); return; }
    const model = ($<HTMLSelectElement>("modelSelect")).value;
    setBadge("Loading...", true);
    setStatus("Loading WASM runtime...");
    const mod = await loadRuntime();

    setStatus("Initializing runtime...");
    mod.init_runtime_from_config(JSON.stringify({ api_key: apiKey, model }));

    const factions: FactionMob[] = [];
    const subs: AgentSub[] = [];

    for (const team of TEAMS) {
      setStatus(`Creating ${team} faction mob (3 autonomous agents)...`);
      const def = buildFactionDefinition(team, model);
      const mobId = parseJsResult(await mod.mob_create(JSON.stringify(def)));

      // Spawn all 3 agents as autonomous_host
      await mod.mob_spawn(mobId, JSON.stringify([
        { profile: "planner", meerkat_id: `${team}-planner`, runtime_mode: "autonomous_host" },
        { profile: "operator", meerkat_id: `${team}-operator`, runtime_mode: "autonomous_host" },
        { profile: "ambassador", meerkat_id: `${team}-ambassador`, runtime_mode: "autonomous_host" },
      ]));

      // Explicit wiring: planner↔operator, planner↔ambassador
      // (NOT operator↔ambassador — ambassador reports through planner)
      await mod.mob_wire(mobId, `${team}-planner`, `${team}-operator`);
      await mod.mob_wire(mobId, `${team}-planner`, `${team}-ambassador`);

      // Subscribe to all 3 agents' event streams
      for (const role of ["planner", "operator", "ambassador"] as const) {
        try {
          const handle = await mod.mob_member_subscribe(mobId, `${team}-${role}`);
          subs.push({ meerkatId: `${team}-${role}`, handle, role, team });
        } catch (e) {
          console.warn(`Failed to subscribe to ${team}-${role}:`, e);
        }
      }

      factions.push({ team, mobId });
    }

    // Wire ambassadors across mobs so they can discover each other via peers()
    setStatus("Wiring cross-mob ambassador trust...");
    for (let i = 0; i < factions.length; i++) {
      for (let j = i + 1; j < factions.length; j++) {
        const a = factions[i], b = factions[j];
        try {
          await mod.wire_cross_mob(
            a.mobId, `${a.team}-ambassador`,
            b.mobId, `${b.team}-ambassador`,
          );
        } catch (e) {
          console.warn(`Cross-mob wire ${a.team}↔${b.team} failed:`, e);
        }
      }
    }

    // Narrator mob (turn_driven, flow-based — just summarizes)
    let narratorMobId: string | null = null;
    try {
      setStatus("Creating narrator...");
      const narratorDef = buildNarratorDefinition(model);
      narratorMobId = parseJsResult(await mod.mob_create(JSON.stringify(narratorDef)));
      await mod.mob_spawn(narratorMobId, JSON.stringify([
        { profile: "narrator", meerkat_id: "narrator", runtime_mode: "turn_driven" },
      ]));
    } catch { /* narrator optional */ }

    const state = defaultState();
    session = {
      factions, narratorMobId, subs, state, messages: [], running: true,
      prevControllers: new Map(state.regions.map(r => [r.id, r.controller])),
    };
    renderMap(state); renderScore(state);
    showBanner("Campaign Begins", "9 autonomous agents across 3 factions", 3000);
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
document.getElementById("exportBtn")!.addEventListener("click", () => { if (!session) return; const b = new Blob([JSON.stringify({state:session.state,messages:session.messages},null,2)],{type:"application/json"}); const a = document.createElement("a"); a.href = URL.createObjectURL(b); a.download = "replay.json"; a.click(); });
document.getElementById("gearBtn")!.addEventListener("click", () => ($<HTMLDivElement>("drawer")).classList.toggle("open"));
document.getElementById("closeDrawer")!.addEventListener("click", () => ($<HTMLDivElement>("drawer")).classList.remove("open"));

renderMap(defaultState());
renderScore(defaultState());
renderChannelSidebar();
renderMessages();
