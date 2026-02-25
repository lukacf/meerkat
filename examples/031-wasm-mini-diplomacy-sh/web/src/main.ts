import "./styles.css";

type Team = "north" | "south";

type ModelPreset = "claude-opus-4-6" | "gpt-5.2" | "gemini-3.1-pro-preview";

type AuthMode = "browser_byok" | "proxy";

interface RegionState {
  id: string;
  controller: Team;
  defense: number;
  value: number;
}

interface ArenaState {
  turn: number;
  max_turns: number;
  regions: RegionState[];
  north_score: number;
  south_score: number;
  winner?: Team | "draw";
  ruleset_version: string;
}

interface RuntimeEvent {
  seq: number;
  kind: string;
  from: string;
  to?: string;
  team: Team;
  payload: string;
}

interface OrderSet {
  team: Team;
  turn: number;
  aggression: number;
  fortify: number;
  diplomacy: string;
  target_region: string;
  model: string;
}

interface TurnDecision {
  order: OrderSet;
  planner_note: string;
  operator_note: string;
}

interface ResolveOutput {
  state: ArenaState;
  summary: string;
}

interface ReplayFrame {
  turn: number;
  state: ArenaState;
  north: TurnDecision;
  south: TurnDecision;
  summary: string;
  events: RuntimeEvent[];
}

interface ReplayEnvelope {
  version: string;
  created_at: string;
  config: {
    north_model: ModelPreset;
    south_model: ModelPreset;
    auth_mode: AuthMode;
  };
  frames: ReplayFrame[];
}

interface RuntimeModule {
  default: () => Promise<unknown>;
  init_mobpack: (bytes: Uint8Array, optionsJson: string) => number;
  start_match: (handle: number, initialStateJson: string) => string;
  submit_turn_input: (handle: number, turnInputJson: string) => string;
  poll_events: (handle: number) => string;
  snapshot_state: (handle: number) => string;
  restore_state: (handle: number, snapshotJson: string) => void;
  resolve_turn: (resolveInputJson: string) => string;
}

interface MatchSession {
  northHandle: number;
  southHandle: number;
  northModel: ModelPreset;
  southModel: ModelPreset;
  authMode: AuthMode;
  state: ArenaState;
  frames: ReplayFrame[];
  chatter: RuntimeEvent[];
  edges: Map<string, number>;
  running: boolean;
  speedMs: number;
}

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) {
  throw new Error("#app element missing");
}

app.innerHTML = `
  <div class="shell">
    <header class="hero">
      <div class="hero-text">
        <p class="kicker">Meerkat WASM Arena</p>
        <h1>Mini Diplomacy Territory Control</h1>
        <p class="subtitle">Two autonomous mobs negotiate, commit hidden orders, and clash in a deterministic board resolver.</p>
      </div>
      <div class="hero-crests">
        <img src="/assets/crest-north.svg" alt="North crest" />
        <img src="/assets/crest-south.svg" alt="South crest" />
      </div>
    </header>

    <section class="controls card">
      <div class="control-grid">
        <label>North model
          <select id="northModel">
            <option value="claude-opus-4-6">claude-opus-4-6</option>
            <option value="gpt-5.2">gpt-5.2</option>
            <option value="gemini-3.1-pro-preview">gemini-3.1-pro-preview</option>
          </select>
        </label>
        <label>South model
          <select id="southModel">
            <option value="gpt-5.2">gpt-5.2</option>
            <option value="claude-opus-4-6">claude-opus-4-6</option>
            <option value="gemini-3.1-pro-preview">gemini-3.1-pro-preview</option>
          </select>
        </label>
        <label>Auth mode
          <select id="authMode">
            <option value="browser_byok">browser_byok</option>
            <option value="proxy">proxy</option>
          </select>
        </label>
        <label>Tick speed
          <select id="speedMs">
            <option value="1700">cinematic</option>
            <option value="1000" selected>normal</option>
            <option value="350">fast</option>
          </select>
        </label>
      </div>
      <div class="button-row">
        <button id="startBtn">Start Match</button>
        <button id="pauseBtn">Pause</button>
        <button id="stepBtn">Step</button>
        <button id="backBtn">Back</button>
        <button id="nextBtn">Forward</button>
        <button id="exportBtn">Export Replay</button>
        <label class="import-label">Import Replay
          <input id="importInput" type="file" accept="application/json" />
        </label>
      </div>
      <p id="statusLine" class="status">Idle</p>
    </section>

    <section class="layout">
      <article class="board card">
        <div class="board-header">
          <h2>Board + Score</h2>
          <span id="winnerBadge" class="badge">Winner: pending</span>
        </div>
        <div id="scoreStrip" class="score-strip"></div>
        <div id="regionGrid" class="region-grid"></div>
      </article>

      <article class="network card">
        <h2>Mob Network</h2>
        <div id="networkGraph"></div>
      </article>

      <article class="chatter card">
        <h2>Chatter Timeline</h2>
        <div id="chatterFeed" class="chatter-feed"></div>
      </article>
    </section>
  </div>
`;

const northModelSelect = document.querySelector<HTMLSelectElement>("#northModel");
const southModelSelect = document.querySelector<HTMLSelectElement>("#southModel");
const authModeSelect = document.querySelector<HTMLSelectElement>("#authMode");
const speedSelect = document.querySelector<HTMLSelectElement>("#speedMs");
const startBtn = document.querySelector<HTMLButtonElement>("#startBtn");
const pauseBtn = document.querySelector<HTMLButtonElement>("#pauseBtn");
const stepBtn = document.querySelector<HTMLButtonElement>("#stepBtn");
const backBtn = document.querySelector<HTMLButtonElement>("#backBtn");
const nextBtn = document.querySelector<HTMLButtonElement>("#nextBtn");
const exportBtn = document.querySelector<HTMLButtonElement>("#exportBtn");
const importInput = document.querySelector<HTMLInputElement>("#importInput");
const statusLine = document.querySelector<HTMLParagraphElement>("#statusLine");
const winnerBadge = document.querySelector<HTMLSpanElement>("#winnerBadge");
const scoreStrip = document.querySelector<HTMLDivElement>("#scoreStrip");
const regionGrid = document.querySelector<HTMLDivElement>("#regionGrid");
const networkGraph = document.querySelector<HTMLDivElement>("#networkGraph");
const chatterFeed = document.querySelector<HTMLDivElement>("#chatterFeed");

if (
  !northModelSelect ||
  !southModelSelect ||
  !authModeSelect ||
  !speedSelect ||
  !startBtn ||
  !pauseBtn ||
  !stepBtn ||
  !backBtn ||
  !nextBtn ||
  !exportBtn ||
  !importInput ||
  !statusLine ||
  !winnerBadge ||
  !scoreStrip ||
  !regionGrid ||
  !networkGraph ||
  !chatterFeed
) {
  throw new Error("UI controls not found");
}

let runtime: RuntimeModule | null = null;
let session: MatchSession | null = null;
let frameCursor = -1;

function setStatus(message: string): void {
  statusLine.textContent = message;
}

function defaultState(): ArenaState {
  return {
    turn: 1,
    max_turns: 14,
    north_score: 0,
    south_score: 0,
    ruleset_version: "mini-diplomacy-v1",
    regions: [
      { id: "north-capital", controller: "north", defense: 58, value: 3 },
      { id: "north-harbor", controller: "north", defense: 48, value: 2 },
      { id: "north-ridge", controller: "north", defense: 42, value: 2 },
      { id: "glass-frontier", controller: "south", defense: 46, value: 4 },
      { id: "ember-crossing", controller: "south", defense: 44, value: 3 },
      { id: "southern-rail", controller: "south", defense: 47, value: 2 },
      { id: "saffron-fields", controller: "south", defense: 40, value: 2 },
      { id: "ash-basin", controller: "south", defense: 45, value: 3 },
      { id: "obsidian-gate", controller: "north", defense: 43, value: 3 },
      { id: "mercury-delta", controller: "south", defense: 38, value: 2 },
      { id: "crimson-pass", controller: "north", defense: 40, value: 3 },
      { id: "south-capital", controller: "south", defense: 56, value: 3 }
    ]
  };
}

function parseJson<T>(raw: string): T {
  return JSON.parse(raw) as T;
}

function formatWinner(state: ArenaState): string {
  return state.winner ?? "pending";
}

function pushSyntheticEvent(events: RuntimeEvent[], team: Team, payload: string): void {
  events.push({
    seq: Date.now(),
    kind: "RESOLVE",
    from: "resolver",
    to: "board",
    team,
    payload
  });
}

function render(sessionState: MatchSession): void {
  const state = sessionState.state;
  winnerBadge.textContent = `Winner: ${formatWinner(state)}`;
  scoreStrip.innerHTML = `
    <div class="score north">North: ${state.north_score}</div>
    <div class="score meta">Turn ${state.turn}/${state.max_turns}</div>
    <div class="score south">South: ${state.south_score}</div>
  `;

  regionGrid.innerHTML = "";
  for (const region of state.regions) {
    const regionEl = document.createElement("div");
    regionEl.className = `region ${region.controller}`;
    regionEl.innerHTML = `
      <h3>${region.id}</h3>
      <p>control: ${region.controller}</p>
      <p>defense: ${region.defense}</p>
      <p>value: ${region.value}</p>
    `;
    regionGrid.append(regionEl);
  }

  const latest = sessionState.chatter.slice(-120).reverse();
  chatterFeed.innerHTML = latest
    .map((event) => {
      const edgeKey = `${event.team}.${event.from}->${event.to ?? "unknown"}`;
      return `<div class="event ${event.team}">
        <span class="tag">${event.kind}</span>
        <span class="edge">${edgeKey}</span>
        <p>${event.payload}</p>
      </div>`;
    })
    .join("");

  const nodes = [
    { id: "north.planner", x: 90, y: 70 },
    { id: "north.operator", x: 90, y: 190 },
    { id: "resolver.board", x: 280, y: 130 },
    { id: "south.planner", x: 470, y: 70 },
    { id: "south.operator", x: 470, y: 190 }
  ];

  const edgeSvg: string[] = [];
  for (const [key, weight] of sessionState.edges.entries()) {
    const [from, to] = key.split("->");
    const src = nodes.find((node) => node.id === from);
    const dst = nodes.find((node) => node.id === to);
    if (!src || !dst) {
      continue;
    }
    const w = Math.min(11, 1 + weight / 2);
    edgeSvg.push(
      `<line x1="${src.x}" y1="${src.y}" x2="${dst.x}" y2="${dst.y}" stroke-width="${w}" class="edge-line" />`
    );
  }

  const nodeSvg = nodes
    .map((node) => {
      const label = node.id.replace(".", " ");
      return `<g>
        <circle cx="${node.x}" cy="${node.y}" r="28" class="node-dot" />
        <text x="${node.x}" y="${node.y + 4}" text-anchor="middle">${label}</text>
      </g>`;
    })
    .join("");

  networkGraph.innerHTML = `
    <svg viewBox="0 0 560 260" role="img" aria-label="live network graph">
      ${edgeSvg.join("\n")}
      ${nodeSvg}
    </svg>
  `;
}

async function loadRuntime(): Promise<RuntimeModule> {
  if (runtime) {
    return runtime;
  }
  const runtimeUrl = new URL("./runtime.js", window.location.href).toString();
  const mod = (await import(/* @vite-ignore */ runtimeUrl)) as RuntimeModule;
  await mod.default();
  runtime = mod;
  return mod;
}

async function loadMobpack(path: string): Promise<Uint8Array> {
  const response = await fetch(path, { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`failed to fetch ${path}: ${response.status}`);
  }
  const buffer = await response.arrayBuffer();
  return new Uint8Array(buffer);
}

function addEdges(events: RuntimeEvent[], edges: Map<string, number>): void {
  for (const event of events) {
    const from = `${event.team}.${event.from}`;
    const to = event.to === "board" ? "resolver.board" : `${event.team}.${event.to ?? "unknown"}`;
    const key = `${from}->${to}`;
    edges.set(key, (edges.get(key) ?? 0) + 1);
  }
}

async function tick(): Promise<void> {
  if (!runtime || !session || !session.running) {
    return;
  }
  if (session.state.winner) {
    session.running = false;
    setStatus(`Match complete. Winner=${session.state.winner}`);
    render(session);
    return;
  }

  try {
    const northIn = JSON.stringify({
      state: session.state,
      opponent_signal: session.frames.at(-1)?.south.order.diplomacy ?? "opening"
    });
    const southIn = JSON.stringify({
      state: session.state,
      opponent_signal: session.frames.at(-1)?.north.order.diplomacy ?? "opening"
    });

    const northDecision = parseJson<TurnDecision>(runtime.submit_turn_input(session.northHandle, northIn));
    const southDecision = parseJson<TurnDecision>(runtime.submit_turn_input(session.southHandle, southIn));

    const resolved = parseJson<ResolveOutput>(
      runtime.resolve_turn(
        JSON.stringify({
          state: session.state,
          north_order: northDecision.order,
          south_order: southDecision.order
        })
      )
    );

    const northEvents = parseJson<RuntimeEvent[]>(runtime.poll_events(session.northHandle));
    const southEvents = parseJson<RuntimeEvent[]>(runtime.poll_events(session.southHandle));
    const events = [...northEvents, ...southEvents];
    pushSyntheticEvent(events, "north", resolved.summary);
    addEdges(events, session.edges);
    session.chatter.push(...events);
    session.state = resolved.state;

    const frame: ReplayFrame = {
      turn: session.state.turn,
      state: JSON.parse(JSON.stringify(session.state)) as ArenaState,
      north: northDecision,
      south: southDecision,
      summary: resolved.summary,
      events
    };
    session.frames.push(frame);
    frameCursor = session.frames.length - 1;

    render(session);
    setStatus(
      `Turn ${session.state.turn - 1} resolved. North=${session.state.north_score} South=${session.state.south_score}`
    );

    if (session.running && !session.state.winner) {
      window.setTimeout(() => {
        void tick();
      }, session.speedMs);
    }
  } catch (error) {
    session.running = false;
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`Runtime error: ${message}`);
  }
}

async function startMatch(): Promise<void> {
  try {
    setStatus("Loading runtime and mobpacks...");
    const mod = await loadRuntime();
    const [northPack, southPack] = await Promise.all([
      loadMobpack("/north.mobpack"),
      loadMobpack("/south.mobpack")
    ]);

    const northModel = northModelSelect.value as ModelPreset;
    const southModel = southModelSelect.value as ModelPreset;
    const authMode = authModeSelect.value as AuthMode;
    const speedMs = Number(speedSelect.value);

    const northHandle = mod.init_mobpack(
      northPack,
      JSON.stringify({ team: "north", model: northModel, seed: 101, auth_mode: authMode })
    );
    const southHandle = mod.init_mobpack(
      southPack,
      JSON.stringify({ team: "south", model: southModel, seed: 202, auth_mode: authMode })
    );

    const state = defaultState();
    const stateJson = JSON.stringify(state);
    mod.start_match(northHandle, stateJson);
    mod.start_match(southHandle, stateJson);

    session = {
      northHandle,
      southHandle,
      northModel,
      southModel,
      authMode,
      state,
      frames: [],
      chatter: [],
      edges: new Map<string, number>(),
      running: true,
      speedMs
    };
    frameCursor = -1;

    render(session);
    setStatus(`Match started (${northModel} vs ${southModel}; mode=${authMode})`);
    await tick();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`Failed to start match: ${message}`);
  }
}

function moveFrame(delta: number): void {
  if (!session || session.frames.length === 0) {
    return;
  }
  frameCursor = Math.max(0, Math.min(session.frames.length - 1, frameCursor + delta));
  const frame = session.frames[frameCursor];
  session.state = JSON.parse(JSON.stringify(frame.state)) as ArenaState;
  render(session);
  setStatus(`Replay frame ${frameCursor + 1}/${session.frames.length}: ${frame.summary}`);
}

function exportReplay(): void {
  if (!session || session.frames.length === 0) {
    setStatus("No replay data to export yet.");
    return;
  }

  const envelope: ReplayEnvelope = {
    version: "mini-diplomacy-replay-v1",
    created_at: new Date().toISOString(),
    config: {
      north_model: session.northModel,
      south_model: session.southModel,
      auth_mode: session.authMode
    },
    frames: session.frames
  };

  const blob = new Blob([JSON.stringify(envelope, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = "mini-diplomacy-replay.json";
  anchor.click();
  URL.revokeObjectURL(url);
  setStatus("Replay exported.");
}

async function importReplay(file: File): Promise<void> {
  const text = await file.text();
  const replay = parseJson<ReplayEnvelope>(text);
  if (!replay.frames.length) {
    throw new Error("Replay file has no frames");
  }

  const lastFrame = replay.frames[replay.frames.length - 1];
  session = {
    northHandle: 0,
    southHandle: 0,
    northModel: replay.config.north_model,
    southModel: replay.config.south_model,
    authMode: replay.config.auth_mode,
    state: JSON.parse(JSON.stringify(lastFrame.state)) as ArenaState,
    frames: replay.frames,
    chatter: replay.frames.flatMap((frame) => frame.events),
    edges: new Map<string, number>(),
    running: false,
    speedMs: 1000
  };
  addEdges(session.chatter, session.edges);
  frameCursor = replay.frames.length - 1;
  render(session);
  setStatus(`Replay imported (${replay.frames.length} frames).`);
}

startBtn.addEventListener("click", () => {
  void startMatch();
});

pauseBtn.addEventListener("click", () => {
  if (!session) {
    return;
  }
  session.running = !session.running;
  if (session.running) {
    pauseBtn.textContent = "Pause";
    void tick();
  } else {
    pauseBtn.textContent = "Resume";
  }
});

stepBtn.addEventListener("click", () => {
  if (!session) {
    return;
  }
  session.running = false;
  pauseBtn.textContent = "Resume";
  void tick();
});

backBtn.addEventListener("click", () => {
  moveFrame(-1);
});

nextBtn.addEventListener("click", () => {
  moveFrame(1);
});

exportBtn.addEventListener("click", () => {
  exportReplay();
});

importInput.addEventListener("change", (event) => {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (!file) {
    return;
  }
  void importReplay(file).catch((error) => {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`Import failed: ${message}`);
  });
});

setStatus("Ready. Build runtime artifacts with ../examples.sh, then Start Match.");
