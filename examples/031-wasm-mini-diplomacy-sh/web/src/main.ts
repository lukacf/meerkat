// ═══════════════════════════════════════════════════════════
// Mini Diplomacy Arena — 19th Century European Theatre
// ═══════════════════════════════════════════════════════════

import "./styles.css";

import type { FactionMob, MatchSession, RuntimeModule, Team } from "./types";
import { TEAMS, TEAM_LABELS } from "./types";
import { getApiKeys, initApiKeyInputs } from "./config";
import { defaultState, resolveOrders } from "./game";
import { renderMap } from "./map";
import { buildFactionDefinition, buildNarratorDefinition, serializeState } from "./agents";
import { drainAllEvents, buildNarratorSummary } from "./events";
import {
  $, setSessionRef, setPhase, setStatus, setBadge, showBanner, showVictory,
  renderScore, renderGrid, pushMessage, pushNarrator, parseJsResult, sleep,
  initMapResize,
} from "./ui";

// ═══════════════════════════════════════════════════════════
// HTML Shell
// ═══════════════════════════════════════════════════════════

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
<div class="score-strip">
  <div class="faction france"><span class="name">France</span><span class="pts" id="fPts">0</span></div>
  <div class="faction prussia"><span class="name">Prussia</span><span class="pts" id="pPts">0</span></div>
  <div class="strip-center">
    <span class="turn-pill" id="turnPill">Turn 1 / 10</span>
    <span class="status-badge" id="statusBadge">Ready</span>
    <button class="ctrl-btn primary" id="startBtn">Start</button>
    <button class="ctrl-btn" id="pauseBtn">Pause</button>
    <button class="ctrl-btn" id="stepBtn">Step</button>
    <button class="ctrl-btn" id="exportBtn">Export</button>
  </div>
  <div class="faction russia"><span class="pts" id="rPts">0</span><span class="name">Russia</span></div>
  <button class="gear-btn" id="gearBtn" title="Settings">\u2699</button>
</div>
<div class="control-bar-wrap"><div class="f-fill" id="fFill" style="flex:1"></div><div class="p-fill" id="pFill" style="flex:1"></div><div class="r-fill" id="rFill" style="flex:1"></div></div>
<div class="arena-layout" id="arenaLayout" data-phase="idle">
  <div class="left-col">
    <div class="map-stage" id="mapStage">
      <svg id="mapSvg" viewBox="0 0 960 600" preserveAspectRatio="xMidYMid meet" xmlns="http://www.w3.org/2000/svg"></svg>
      <div class="turn-banner" id="banner"><div class="phase" id="bannerPhase"></div><div class="detail" id="bannerDetail"></div></div>
      <div class="victory-overlay" id="victory"><div class="victory-content" id="victoryInner"></div></div>
    </div>
    <div class="narrator-panel" id="narratorPanel">
      <div class="narrator-header">\u{1F4DC} War Correspondent</div>
      <div class="narrator-feed" id="narratorFeed">
        <div class="narrator-entry"><em>Awaiting the first dispatches from the field...</em></div>
      </div>
    </div>
  </div>
  <div class="right-col">
    <div class="channel-grid" id="channelGrid"></div>
  </div>
</div>
<div class="settings-drawer" id="drawer">
  <button class="close-btn" id="closeDrawer">\u2715 Close</button>
  <h3>API Keys</h3>
  <div class="setting"><label>Anthropic</label><input type="password" id="keyAnthropic" placeholder="sk-ant-..." /></div>
  <div class="setting"><label>OpenAI</label><input type="password" id="keyOpenai" placeholder="sk-..." /></div>
  <div class="setting"><label>Gemini</label><input type="password" id="keyGemini" placeholder="..." /></div>
  <h3>Faction Models</h3>
  <div class="setting"><label>France</label><select id="modelFrance"></select></div>
  <div class="setting"><label>Prussia</label><select id="modelPrussia"></select></div>
  <div class="setting"><label>Russia</label><select id="modelRussia"></select></div>
  <div class="setting"><label>Narrator</label><select id="modelNarrator"></select></div>
  <p class="settings-note">Each faction is a mob of 3 autonomous agents (planner, operator, ambassador). Enter API keys to unlock models for that provider. Per-faction model selection allows mixed-provider games.</p>
</div>
<div class="start-overlay" id="startOverlay">
  <div class="start-content">
    <h1>The Congress of Europe</h1>
    <p class="start-subtitle">A Meerkat WASM Demo \u2014 9 Autonomous AI Agents</p>
    <div class="start-desc">
      <p>Three great European powers \u2014 <strong>France</strong>, <strong>Prussia</strong>, and <strong>Russia</strong> \u2014 contest 12 territories across the continent. Each faction deploys 3 autonomous agents (planner, operator, ambassador) that strategize, negotiate, deceive, and betray through real-time comms.</p>
      <p>Powered by <strong>Meerkat</strong>'s WASM runtime: 4 mobs, 9 autonomous_host agents, cross-mob comms, structured output extraction, all running in your browser.</p>
    </div>
    <button class="start-big-btn" id="startBigBtn">Enter the War Room</button>
    <p class="start-hint">Then configure API keys via \u2699 and press Start</p>
  </div>
</div>
<p class="status-bar" id="statusLine">Open settings (\u2699) to configure API keys, then start.</p>`;

// ═══════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════

let runtime: RuntimeModule | null = null;
let session: MatchSession | null = null;

async function loadRuntime(): Promise<RuntimeModule> {
  if (runtime) return runtime;
  const url = new URL("./runtime.js", window.location.href).toString();
  const mod = (await import(/* @vite-ignore */ url)) as RuntimeModule;
  await mod.default();
  runtime = mod;
  return mod;
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
    setStatus(`Round ${turn}: Triggering planners \u2014 agents deliberate, negotiate, finalize...`);
    setPhase("deliberation");
    showBanner(`Round ${turn}`, "The powers deliberate and negotiate...", 8000);

    for (const f of session.factions) {
      const stateStr = serializeState(f.team, session.state);
      const prompt = `=== TURN ${turn} GAME STATE ===\n${stateStr}\n\nBegin: message your operator to discuss strategy, then brief your ambassador.`;
      try { await mod.mob_send_message(f.mobId, `${f.team}-planner`, prompt); }
      catch (e) { console.warn(`Failed to trigger ${f.team} planner:`, e); }
    }

    const MAX_WAIT_MS = 120_000;
    const QUIET_THRESHOLD = 8_000;
    const deadline = Date.now() + MAX_WAIT_MS;
    let lastEventTime = Date.now();
    const allErrors: string[] = [];

    while (Date.now() < deadline && session.running) {
      await sleep(300);
      const { events: newEvents, errors } = drainAllEvents(mod, session, turn);
      if (errors.length > 0) allErrors.push(...errors);
      if (newEvents > 0) {
        lastEventTime = Date.now();
        const totalDMs = session.messages.filter(m => m.turn === turn).length;
        setStatus(`Round ${turn}: ${totalDMs} messages exchanged...`);
      }
      if (Date.now() - lastEventTime > QUIET_THRESHOLD) break;
    }
    if (!session.running) return;

    const turnMsgs = session.messages.filter(m => m.turn === turn).length;
    if (allErrors.length > 0) {
      // Detect which factions had errors
      const failedFactions = new Set<string>();
      for (const err of allErrors) {
        for (const t of TEAMS) if (err.startsWith(`${t}-`)) failedFactions.add(TEAM_LABELS[t]);
      }
      const failedStr = failedFactions.size > 0 ? [...failedFactions].join(", ") : "Some agents";

      if (turnMsgs === 0) {
        // Total failure — no messages at all
        session.running = false; setBadge("Error");
        const first = allErrors[0];
        const hint = first.includes("401") || first.toLowerCase().includes("auth") ? " Check API keys." : "";
        setStatus(`All agents failed: ${first}${hint}`);
        showBanner("Error", "LLM calls failed \u2014 check API keys", 10000);
        return;
      } else {
        // Partial failure — some factions working, others not
        setStatus(`Round ${turn}: ${turnMsgs} messages. WARNING: ${failedStr} had errors \u2014 ${allErrors[0].split(":").slice(1).join(":").trim()}`);
      }
    }

    // ── Post-quiescence order extraction ──
    setStatus(`Round ${turn}: Extracting final orders...`);
    for (const f of session.factions) {
      const validTargets = session.state.regions.filter(r => r.controller !== f.team).map(r => r.id);
      const plannerAddr = `diplomacy-${f.team}/planner/${f.team}-planner`;
      try {
        await mod.mob_send_message(f.mobId, `${f.team}-operator`,
          `TIME IS UP. Send your final order to your planner NOW using send_message. ` +
          `Valid targets: ${validTargets.join(", ")}. ` +
          `Your message MUST contain: FINAL ORDER: target=<region-id> aggression=<0-100>. ` +
          `Send it to: ${plannerAddr}`);
      } catch (e) { console.warn(`Failed to prompt ${f.team} operator for order:`, e); }
    }
    const extractDeadline = Date.now() + 20_000;
    while (Date.now() < extractDeadline && session.running) {
      await sleep(300);
      const { events: n } = drainAllEvents(mod, session, turn);
      if (n > 0) lastEventTime = Date.now();
      const allHaveOrders = session.factions.every(f =>
        session!.messages.some(m => m.turn === turn && m.faction === f.team && m.role === "operator" && /FINAL\s*ORDER/i.test(m.content))
      );
      if (allHaveOrders) break;
      if (Date.now() - lastEventTime > 6_000) break;
    }
    if (!session.running) return;

    // Parse orders
    const decisions = session.factions.map(f => {
      const opMsgs = session!.messages.filter(m => m.turn === turn && m.faction === f.team && m.role === "operator");
      for (let i = opMsgs.length - 1; i >= 0; i--) {
        const match = opMsgs[i].content.match(/FINAL\s*ORDER\s*:?\s*target\s*=\s*([\w-]+)\s*aggression\s*=\s*(\d+)/i);
        if (match) {
          const validTargets = session!.state.regions.filter(r => r.controller !== f.team).map(r => r.id);
          const aggression = Math.max(0, Math.min(100, parseInt(match[2], 10)));
          return { order: { team: f.team, aggression, fortify: 100 - aggression,
            target_region: validTargets.includes(match[1]) ? match[1] : validTargets[0] }, reasoning: opMsgs[i].content };
        }
      }
      for (let i = opMsgs.length - 1; i >= 0; i--) {
        try {
          const jsonMatch = opMsgs[i].content.match(/\{[\s\S]*\}/);
          if (jsonMatch) {
            const parsed = JSON.parse(jsonMatch[0]);
            const o = parsed.order || parsed;
            if (o.target_region && o.aggression != null) {
              const validTargets = session!.state.regions.filter(r => r.controller !== f.team).map(r => r.id);
              return { order: { team: f.team, aggression: Math.max(0, Math.min(100, Number(o.aggression) || 50)), fortify: 100 - (Number(o.aggression) || 50),
                target_region: validTargets.includes(o.target_region) ? o.target_region : validTargets[0] }, reasoning: opMsgs[i].content };
            }
          }
        } catch { /* skip */ }
      }
      const validTargets = session!.state.regions.filter(r => r.controller !== f.team).map(r => r.id);
      return { order: { team: f.team, aggression: 50, fortify: 50, target_region: validTargets[0] }, reasoning: "Fallback." };
    });

    // Resolve & Render
    setPhase("resolution");
    const targets = Object.fromEntries(decisions.map(d => [d.order.team, d.order.target_region])) as Record<Team, string>;
    showBanner(`Round ${turn}`, decisions.map(d => `${TEAM_LABELS[d.order.team]}\u2192${d.order.target_region.replace(/-/g," ")}`).join(" \u2022 "), 2500);
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
        const summary = buildNarratorSummary(session, turn, decisions, captures, newState);
        const runId = parseJsResult(await mod.mob_run_flow(session.narratorMobId, "narrate", JSON.stringify({ summary })));
        for (let i = 0; i < 40; i++) {
          await sleep(500);
          const raw = parseJsResult(await mod.mob_flow_status(session.narratorMobId, runId));
          if (raw === "null") continue;
          try {
            const result = JSON.parse(raw);
            if (result.status === "completed") {
              const step = result.step_ledger?.find((s: any) => s.step_id === "summarize" && s.status === "completed");
              const narrative = step?.output?.narrative ?? (typeof step?.output === "string" ? step.output : null);
              if (narrative) {
                pushMessage({ channel: "narrator", role: "narrator", faction: "neutral", content: narrative, turn });
                pushNarrator(narrative, turn);
              }
              break;
            }
            if (result.status !== "running" && result.status !== "pending") break;
          } catch { /* parse error */ }
        }
      } catch (e) { console.warn("Narrator error:", e); }
    }

    setBadge("Live"); setStatus(`Round ${turn} resolved.`); setPhase("idle");
    if (newState.winner) {
      session.running = false; await sleep(800);
      const lastNarrator = session.messages.filter(m => m.channel === "narrator" && m.role === "narrator").pop();
      showVictory(newState.winner, newState, lastNarrator?.content);
      setBadge("Complete"); return;
    }
    await sleep(2000);
    if (session.running) void tick();
  } catch (error) {
    if (session) session.running = false;
    setBadge("Error"); setStatus(`Error: ${error instanceof Error ? error.message : String(error)}`);
  }
}

// ═══════════════════════════════════════════════════════════
// Start Match
// ═══════════════════════════════════════════════════════════

async function startMatch(): Promise<void> {
  try {
    $<HTMLDivElement>("victory").classList.remove("visible");
    $<HTMLDivElement>("startOverlay").classList.add("hidden");
    const keys = getApiKeys();
    if (!keys.anthropic && !keys.openai && !keys.gemini) { setStatus("Enter at least one API key in settings."); return; }
    const models: Record<Team | "narrator", string> = {
      france: $<HTMLSelectElement>("modelFrance").value,
      prussia: $<HTMLSelectElement>("modelPrussia").value,
      russia: $<HTMLSelectElement>("modelRussia").value,
      narrator: $<HTMLSelectElement>("modelNarrator").value,
    };
    if (!models.france || !models.prussia || !models.russia) { setStatus("Select models for all factions."); return; }

    setBadge("Loading...", true); setStatus("Loading WASM runtime...");
    const mod = await loadRuntime();

    setStatus("Initializing runtime with provider keys...");
    const initConfig: Record<string, unknown> = {};
    if (keys.anthropic) initConfig.anthropic_api_key = keys.anthropic;
    if (keys.openai) initConfig.openai_api_key = keys.openai;
    if (keys.gemini) initConfig.gemini_api_key = keys.gemini;
    initConfig.model = models.france;
    mod.init_runtime_from_config(JSON.stringify(initConfig));

    const factions: FactionMob[] = [];
    const subs: MatchSession["subs"] = [];

    for (const team of TEAMS) {
      setStatus(`Creating ${TEAM_LABELS[team]} faction (3 autonomous agents)...`);
      const def = buildFactionDefinition(team, models[team]);
      const mobId = parseJsResult(await mod.mob_create(JSON.stringify(def)));

      await mod.mob_spawn(mobId, JSON.stringify([
        { profile: "planner", meerkat_id: `${team}-planner`, runtime_mode: "autonomous_host" },
        { profile: "operator", meerkat_id: `${team}-operator`, runtime_mode: "autonomous_host" },
        { profile: "ambassador", meerkat_id: `${team}-ambassador`, runtime_mode: "autonomous_host" },
      ]));
      await mod.mob_wire(mobId, `${team}-planner`, `${team}-operator`);
      await mod.mob_wire(mobId, `${team}-planner`, `${team}-ambassador`);

      for (const role of ["planner", "operator", "ambassador"] as const) {
        try { subs.push({ meerkatId: `${team}-${role}`, handle: await mod.mob_member_subscribe(mobId, `${team}-${role}`), role, team }); }
        catch (e) { console.warn(`Failed to subscribe to ${team}-${role}:`, e); }
      }
      factions.push({ team, mobId });
    }

    setStatus("Wiring cross-faction ambassador trust...");
    for (let i = 0; i < factions.length; i++) {
      for (let j = i + 1; j < factions.length; j++) {
        const a = factions[i], b = factions[j];
        try { await mod.wire_cross_mob(a.mobId, `${a.team}-ambassador`, b.mobId, `${b.team}-ambassador`); }
        catch (e) { console.warn(`Cross-mob wire ${a.team}\u2194${b.team} failed:`, e); }
      }
    }

    let narratorMobId: string | null = null;
    try {
      setStatus("Creating war correspondent...");
      narratorMobId = parseJsResult(await mod.mob_create(JSON.stringify(buildNarratorDefinition(models.narrator))));
      await mod.mob_spawn(narratorMobId, JSON.stringify([{ profile: "narrator", meerkat_id: "narrator", runtime_mode: "turn_driven" }]));
    } catch { /* narrator optional */ }

    const state = defaultState();
    session = { factions, narratorMobId, subs, state, messages: [], running: true,
      prevControllers: new Map(state.regions.map(r => [r.id, r.controller])), seenToolCallIds: new Set() };
    setSessionRef(session);
    renderMap(state); renderScore(state);
    showBanner("The Campaign Begins", `${TEAMS.map(t => TEAM_LABELS[t]).join(", ")} \u2014 9 agents, 3 powers`, 3000);
    setBadge("Live"); setStatus("Campaign started.");
    $<HTMLDivElement>("drawer").classList.remove("open");
    await tick();
  } catch (e) { setBadge("Error"); setStatus(`Failed: ${e instanceof Error ? e.message : String(e)}`); }
}

// ═══════════════════════════════════════════════════════════
// Event Handlers + Init
// ═══════════════════════════════════════════════════════════

initApiKeyInputs();

document.getElementById("startBtn")!.addEventListener("click", () => void startMatch());
document.getElementById("startBigBtn")!.addEventListener("click", () => {
  $<HTMLDivElement>("startOverlay").classList.add("hidden");
});
document.getElementById("pauseBtn")!.addEventListener("click", () => { if (!session) return; session.running = !session.running; (document.getElementById("pauseBtn") as HTMLButtonElement).textContent = session.running ? "Pause" : "Resume"; if (session.running) void tick(); });
document.getElementById("stepBtn")!.addEventListener("click", () => { if (!session) return; session.running = true; (document.getElementById("pauseBtn") as HTMLButtonElement).textContent = "Resume"; const doStep = async () => { await tick(); if (session) session.running = false; (document.getElementById("pauseBtn") as HTMLButtonElement).textContent = "Resume"; }; void doStep(); });
document.getElementById("exportBtn")!.addEventListener("click", () => { if (!session) return; const b = new Blob([JSON.stringify({state:session.state,messages:session.messages},null,2)],{type:"application/json"}); const a = document.createElement("a"); a.href = URL.createObjectURL(b); a.download = "replay.json"; a.click(); });
document.getElementById("gearBtn")!.addEventListener("click", () => $<HTMLDivElement>("drawer").classList.toggle("open"));
document.getElementById("closeDrawer")!.addEventListener("click", () => $<HTMLDivElement>("drawer").classList.remove("open"));

renderMap(defaultState());
renderScore(defaultState());
renderGrid();
initMapResize();
