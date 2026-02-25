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
  state: ArenaState;
  dispatches: Dispatch[];
  running: boolean;
  prevControllers: Map<string, Team>;
}

// ═══════════════════════════════════════════════════════════
// Map Data — 12 territories, 3 factions
// ═══════════════════════════════════════════════════════════

interface HexInfo { cx: number; cy: number; r: number; label: string }

const MAP: Record<string, HexInfo> = {
  "north-capital":  { cx: 450, cy: 55,  r: 48, label: "North\nCapital" },
  "north-harbor":   { cx: 248, cy: 140, r: 42, label: "North\nHarbor" },
  "north-ridge":    { cx: 652, cy: 140, r: 42, label: "North\nRidge" },
  "obsidian-gate":  { cx: 338, cy: 225, r: 46, label: "Obsidian\nGate" },
  "crimson-pass":   { cx: 562, cy: 225, r: 46, label: "Crimson\nPass" },
  "glass-frontier": { cx: 200, cy: 330, r: 48, label: "Glass\nFrontier" },
  "ember-crossing": { cx: 700, cy: 330, r: 46, label: "Ember\nCrossing" },
  "southern-rail":  { cx: 140, cy: 435, r: 42, label: "Southern\nRail" },
  "saffron-fields": { cx: 450, cy: 390, r: 44, label: "Saffron\nFields" },
  "ash-basin":      { cx: 760, cy: 435, r: 46, label: "Ash\nBasin" },
  "mercury-delta":  { cx: 300, cy: 510, r: 42, label: "Mercury\nDelta" },
  "south-capital":  { cx: 600, cy: 510, r: 48, label: "South\nCapital" },
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

function hexPath(cx: number, cy: number, r: number): string {
  return Array.from({ length: 6 }, (_, i) => {
    const a = (Math.PI / 3) * i - Math.PI / 2;
    return `${cx + r * Math.cos(a)},${cy + r * Math.sin(a)}`;
  }).join("L").replace(/^/, "M") + "Z";
}

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

function buildPrompt(team: Team, state: ArenaState, signals: Record<Team, string>): string {
  const ours = state.regions.filter(r => r.controller === team);
  const enemies = state.regions.filter(r => r.controller !== team);
  const others = TEAMS.filter(t => t !== team);
  return `Turn ${state.turn}/${state.max_turns}. You are ${team.toUpperCase()}.
SCORES: ${TEAMS.map(t => `${t}=${state.scores[t]}`).join(", ")}
YOUR TERRITORIES (${ours.length}):
${ours.map(r => `  ${r.id}: def=${r.defense}, val=${r.value}`).join("\n")}
ENEMY TERRITORIES (${enemies.length}):
${enemies.map(r => `  ${r.id} [${r.controller}]: def=${r.defense}, val=${r.value}`).join("\n")}
SIGNALS FROM LAST TURN:
${others.map(t => `  ${t}: "${signals[t]}"`).join("\n")}
Choose your target and set aggression/fortify. With 3 factions, alliances and betrayal are key.`;
}

function parseDecision(team: Team, text: string, state: ArenaState): TurnDecision | null {
  try {
    const match = text.match(/\{[\s\S]*\}/);
    if (!match) return null;
    const parsed = JSON.parse(match[0]);
    const order = parsed.order || parsed;
    const validTargets = state.regions.filter(r => r.controller !== team).map(r => r.id);
    const aggression = Math.max(0, Math.min(100, Number(order.aggression) || 50));
    return {
      order: {
        team, aggression, fortify: 100 - aggression,
        target_region: validTargets.includes(order.target_region) ? order.target_region : validTargets[0],
        diplomacy: String(order.diplomacy || "no comment"),
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
<div class="map-stage" id="mapStage">
  <svg id="mapSvg" viewBox="0 0 900 570" preserveAspectRatio="xMidYMid meet" xmlns="http://www.w3.org/2000/svg"></svg>
  <div class="dispatch-panel" id="dispatchPanel">
    <div class="d-head"><span class="d-title">Dispatches</span><button class="d-toggle" id="dToggle">\u2212</button></div>
    <div class="d-feed" id="dFeed"></div>
  </div>
  <div class="turn-banner" id="banner"><div class="phase" id="bannerPhase"></div><div class="detail" id="bannerDetail"></div></div>
  <div class="victory-overlay" id="victory"><div class="victory-content" id="victoryInner"></div></div>
  <div class="game-controls">
    <button class="primary" id="startBtn">Start Campaign</button>
    <button id="pauseBtn">Pause</button>
    <button id="stepBtn">Step</button>
    <span class="sep"></span>
    <button id="exportBtn">Export</button>
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
</div>
<p class="status-bar" id="statusLine">Open settings (\u2699) to configure API key, then start.</p>`;

// ═══════════════════════════════════════════════════════════
// Refs + State
// ═══════════════════════════════════════════════════════════

const $ = <T extends Element>(id: string) => document.getElementById(id) as unknown as T;
let runtime: RuntimeModule | null = null;
let session: MatchSession | null = null;
let bannerTimer: ReturnType<typeof setTimeout> | null = null;

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
function showVictory(winner: Team | "draw", state: ArenaState): void {
  const title = winner === "draw" ? "DRAW" : `${winner.toUpperCase()} WINS`;
  ($<HTMLDivElement>("victoryInner")).innerHTML = `<h2 class="${winner}">${title}</h2><p>After ${state.turn} turns.</p><div class="final">N:${state.scores.north} E:${state.scores.east} S:${state.scores.south}</div>`;
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
  <pattern id="grid" width="40" height="40" patternUnits="userSpaceOnUse"><path d="M40 0 L0 0 0 40" fill="none" stroke="rgba(50,70,100,0.05)" stroke-width="0.5"/></pattern></defs>`;
  svg += `<rect width="900" height="570" fill="url(#grid)"/>`;
  for (const [a, b] of EDGES) { const pa = MAP[a], pb = MAP[b]; svg += `<line x1="${pa.cx}" y1="${pa.cy}" x2="${pb.cx}" y2="${pb.cy}" class="${ctrl.get(a) !== ctrl.get(b) ? "edge-front" : "edge-line"}"/>`; }
  for (const region of state.regions) {
    const h = MAP[region.id]; if (!h) continue;
    const path = hexPath(h.cx, h.cy, h.r), lines = h.label.split("\n");
    svg += `<g class="territory" data-region="${region.id}"><path d="${path}" fill="url(#g${region.controller[0].toUpperCase()})" class="hex-bg"/><path d="${path}" class="hex-border ${region.controller}"/>`;
    if (lines.length === 2) svg += `<text class="region-label" x="${h.cx}" y="${h.cy-6}">${lines[0]}</text><text class="region-label" x="${h.cx}" y="${h.cy+7}">${lines[1]}</text>`;
    else svg += `<text class="region-label" x="${h.cx}" y="${h.cy+2}">${h.label}</text>`;
    svg += `<text class="region-stats" x="${h.cx}" y="${h.cy+h.r*0.55}">\u2694${region.defense} \u2605${region.value}</text></g>`;
  }
  if (captures) for (const id of captures) { const h = MAP[id], t = ctrl.get(id); if (h && t) svg += `<path d="${hexPath(h.cx, h.cy, h.r+6)}" class="captured-flash to-${t}" filter="url(#glow)"/>`; }
  if (targets) for (const [team, rid] of Object.entries(targets)) { const h = MAP[rid]; if (h) svg += `<circle cx="${h.cx}" cy="${h.cy}" r="${h.r+10}" class="target-ring ${team}"/>`; }
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

function renderDispatches(dispatches: Dispatch[]): void {
  ($<HTMLDivElement>("dFeed")).innerHTML = dispatches.slice(-24).reverse().map(d =>
    `<div class="dispatch ${d.team}"><div class="d-sender">${d.team.toUpperCase()} T${d.turn}</div><div class="d-body">${d.diplomacy}</div></div>`
  ).join("");
}

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

async function tick(): Promise<void> {
  if (!runtime || !session || !session.running) return;
  if (session.state.winner) { session.running = false; showVictory(session.state.winner, session.state); setBadge("Complete"); return; }
  try {
    setBadge("Thinking...", true);
    setStatus(`Round ${session.state.turn}: Factions planning...`);
    const signals: Record<Team, string> = { north: "opening", south: "opening", east: "opening" };
    for (const d of session.dispatches.slice(-3)) signals[d.team] = d.diplomacy;

    const results = await Promise.all(session.factions.map(async f => {
      const prompt = buildPrompt(f.team, session!.state, signals);
      const rj = await runtime!.start_turn(f.handle, prompt, "{}");
      const r = JSON.parse(rj);
      return { team: f.team, text: r.text as string };
    }));
    if (!session.running) return;

    const decisions: TurnDecision[] = [];
    for (const r of results) {
      const d = parseDecision(r.team, r.text, session.state);
      if (d) { decisions.push(d); session.dispatches.push({ team: r.team, turn: session.state.turn, diplomacy: d.order.diplomacy, reasoning: d.reasoning }); }
    }
    if (decisions.length === 0) { session.running = false; setBadge("Error"); setStatus("No valid orders."); return; }

    const targets = Object.fromEntries(decisions.map(d => [d.order.team, d.order.target_region])) as Record<Team, string>;
    showBanner(`Round ${session.state.turn}`, decisions.map(d => `${d.order.team}\u2192${d.order.target_region.replace(/-/g," ")}`).join(" \u2022 "), 2500);
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
    renderDispatches(session.dispatches);
    if (captures.size) showBanner("Territory Changed", [...captures].map(id => id.replace(/-/g," ")).join(", "), 2000);
    setBadge("Live");
    setStatus(`Round ${newState.turn-1} resolved.`);
    if (newState.winner) { session.running = false; await new Promise(r => setTimeout(r, 800)); showVictory(newState.winner, newState); setBadge("Complete"); return; }
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
    const packs = await Promise.all(["/north.mobpack","/south.mobpack","/east.mobpack"].map(async p => { const r = await fetch(p, {cache:"no-store"}); return new Uint8Array(await r.arrayBuffer()); }));
    const models: Record<Team, string> = { north: ($<HTMLSelectElement>("northModel")).value, south: ($<HTMLSelectElement>("southModel")).value, east: ($<HTMLSelectElement>("eastModel")).value };
    const factions: FactionSession[] = TEAMS.map((team, i) => {
      const h = mod.create_session(packs[i], JSON.stringify({ model: models[team], api_key: apiKey, max_tokens: 1024 }));
      return { team, handle: h, model: models[team] };
    });
    const state = defaultState();
    session = { factions, state, dispatches: [], running: true, prevControllers: new Map(state.regions.map(r => [r.id, r.controller])) };
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
document.getElementById("stepBtn")!.addEventListener("click", () => { if (!session) return; session.running = false; (document.getElementById("pauseBtn") as HTMLButtonElement).textContent = "Resume"; void tick(); });
document.getElementById("exportBtn")!.addEventListener("click", () => { if (!session) return; const b = new Blob([JSON.stringify({state:session.state,dispatches:session.dispatches},null,2)],{type:"application/json"}); const a = document.createElement("a"); a.href = URL.createObjectURL(b); a.download = "replay.json"; a.click(); });
document.getElementById("gearBtn")!.addEventListener("click", () => ($<HTMLDivElement>("drawer")).classList.toggle("open"));
document.getElementById("closeDrawer")!.addEventListener("click", () => ($<HTMLDivElement>("drawer")).classList.remove("open"));
document.getElementById("dToggle")!.addEventListener("click", () => { const p = $<HTMLDivElement>("dispatchPanel"); p.classList.toggle("collapsed"); ($<HTMLButtonElement>("dToggle")).textContent = p.classList.contains("collapsed") ? "+" : "\u2212"; });

renderMap(defaultState());
renderScore(defaultState());
