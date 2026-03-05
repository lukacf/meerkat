// ═══════════════════════════════════════════════════════════
// The Office — Meerkat WASM Multi-Agent Demo
// ═══════════════════════════════════════════════════════════

import "./styles.css";
import { AGENTS, AGENT_IDS } from "./types";
import type { AgentId, RuntimeModule, AgentSub } from "./types";
import {
  getApiKeys,
  getSelectedModel,
  initApiKeyInputs,
  isServerMode,
  getServerProxyConfig,
  setApiKeys,
  hasAnyApiKey,
  MODEL_PROVIDER,
} from "./config";
import type { Provider } from "./config";
import { setupBridge } from "./llm-bridge";
import { initCanvas, setOnSelectAgent, loadBackground, setOnFilingCabinetClick } from "./office/canvas";
import { initCharacters, setAgentState } from "./office/characters";
import { loadSprites } from "./office/sprites";
import { initBubbles, showSpeechBubble, showThinkBubble, hideThinkBubble } from "./office/bubbles";
import { initPhoneLines, startCall, endCall, triggerEnvelopeArrival } from "./office/phonelines";
import { buildOfficeDefinition, WIRING_PAIRS, PROFILE_NAMES } from "./agents";
import { SCENARIOS } from "./scenarios";
import { drainAllEvents, setOnMessage, setOnApprovalNeeded } from "./events";
import { createIncident, addMessage, renderIncidentPanel, setRenderCallback } from "./incidents";
import { extractFacts, toggleKnowledgeGraph, isGraphVisible } from "./knowledge";

// ═══════════════════════════════════════════════════════════
// HTML Shell
// ═══════════════════════════════════════════════════════════

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
<div class="control-bar">
  <span class="title">THE OFFICE</span>
  <span class="status-badge" id="statusBadge">Ready</span>
  <span class="spacer"></span>
  <button class="ctrl-btn primary" id="startBtn">Start</button>
  <button class="ctrl-btn" id="pauseBtn">Pause</button>
  <div class="event-dropdown" id="eventDropdown">
    <button class="ctrl-btn" id="eventBtn">+ Event</button>
    <div class="dropdown-menu hidden" id="eventMenu">
      ${SCENARIOS.map(s => `<button class="dropdown-item" data-scenario="${s.id}">${s.icon} ${s.title}</button>`).join("")}
    </div>
  </div>
  <span class="event-counter" id="eventCounter">0 events</span>
  <button class="gear-btn" id="gearBtn" title="Settings">\u2699</button>
</div>
<div class="arena-layout">
  <div class="canvas-col">
    <canvas id="officeCanvas"></canvas>
  </div>
  <div class="panel-col">
    <div class="panel-header">Incidents</div>
    <div class="panel-content" id="panelContent">
      <div class="panel-empty">Events will appear here as agents process them...</div>
    </div>
  </div>
</div>
<div class="chat-bar">
  <select id="chatAgent">
    ${AGENT_IDS.map(id => `<option value="${id}">${AGENTS[id].name} (${AGENTS[id].role})</option>`).join("")}
  </select>
  <input type="text" id="chatInput" placeholder="Talk to an agent..." />
  <button id="chatSend">Send</button>
</div>
<p class="status-bar" id="statusLine">Configure API keys via \u2699, then press Start.</p>
<div class="settings-drawer" id="drawer">
  <button class="close-btn" id="closeDrawer">\u2715 Close</button>
  <h3>API Keys</h3>
  <div class="setting"><label>Anthropic</label><input type="password" id="keyAnthropic" placeholder="sk-ant-..." /></div>
  <div class="setting"><label>OpenAI</label><input type="password" id="keyOpenai" placeholder="sk-..." /></div>
  <div class="setting"><label>Gemini</label><input type="password" id="keyGemini" placeholder="..." /></div>
  <h3>Model</h3>
  <div class="setting"><label>LLM Model</label><select id="modelSelect"></select></div>
</div>
<div class="start-overlay" id="startOverlay">
  <div class="start-content">
    <h1>The Office</h1>
    <p class="start-subtitle">A Meerkat WASM Demo \u2014 10 Autonomous AI Agents</p>
    <div class="start-desc">
      <p><strong>10 AI agents</strong> run an office together. Events arrive at the mail room \u2014 emails, alerts, calendar conflicts \u2014 and the triage coordinator routes them to <strong>department specialists</strong> (IT, HR, Facilities, Finance), <strong>personal assistants</strong>, and an <strong>archivist</strong> who maintains the institutional knowledge base.</p>
      <p>Watch them call each other on desk phones, coordinate responses, store knowledge, and route actions through a <strong>compliance gate</strong> that asks for your approval on high-risk decisions.</p>
    </div>
    <button class="start-big-btn" id="startBigBtn">Enter the Office</button>
    <p class="start-hint">Configure API keys via \u2699 and press Start</p>
  </div>
</div>
<div class="key-overlay hidden" id="keyOverlay">
  <div class="key-card">
    <div class="key-title">API Key Required</div>
    <p class="key-desc" id="keyOverlayMessage">No API keys detected. Enter at least one key to start.</p>
    <div class="setting"><label>Anthropic</label><input type="password" id="keyDialogAnthropic" placeholder="sk-ant-..." /></div>
    <div class="setting"><label>OpenAI</label><input type="password" id="keyDialogOpenai" placeholder="sk-..." /></div>
    <div class="setting"><label>Gemini</label><input type="password" id="keyDialogGemini" placeholder="..." /></div>
    <div class="key-actions">
      <button class="ctrl-btn" id="keyDialogCancel">Cancel</button>
      <button class="ctrl-btn primary" id="keyDialogSave">Save & Start</button>
    </div>
  </div>
</div>
<div class="approval-overlay hidden" id="approvalOverlay">
  <div class="approval-card">
    <div class="approval-header">APPROVAL REQUIRED</div>
    <div class="approval-body" id="approvalBody"></div>
    <div class="approval-actions">
      <button class="ctrl-btn approve-btn" id="approveBtn">Approve</button>
      <button class="ctrl-btn deny-btn" id="denyBtn">Deny</button>
    </div>
  </div>
</div>`;

// ═══════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════

const $ = <T extends Element>(id: string) => document.getElementById(id) as unknown as T;
function setStatus(msg: string): void { $<HTMLParagraphElement>("statusLine").textContent = msg; }
function setBadge(text: string, thinking = false): void {
  const el = $<HTMLSpanElement>("statusBadge");
  el.textContent = text;
  el.className = `status-badge${thinking ? " thinking" : ""}`;
}
function sleep(ms: number): Promise<void> { return new Promise(r => setTimeout(r, ms)); }
function parseJsResult(val: unknown): string { return typeof val === "string" ? val : String(val); }
function providerLabel(provider: Provider): string {
  if (provider === "anthropic") return "Anthropic";
  if (provider === "openai") return "OpenAI";
  return "Gemini";
}
function extractProviderFromError(err: string): Provider | null {
  const match = err.match(/\b(anthropic|openai|gemini)\b/i);
  if (!match) return null;
  return match[1].toLowerCase() as Provider;
}

function showKeyDialog(message: string): void {
  const overlay = $<HTMLDivElement>("keyOverlay");
  $<HTMLParagraphElement>("keyOverlayMessage").textContent = message;
  $<HTMLInputElement>("keyDialogAnthropic").value = $<HTMLInputElement>("keyAnthropic").value.trim();
  $<HTMLInputElement>("keyDialogOpenai").value = $<HTMLInputElement>("keyOpenai").value.trim();
  $<HTMLInputElement>("keyDialogGemini").value = $<HTMLInputElement>("keyGemini").value.trim();
  overlay.classList.remove("hidden");
  const first = [
    "keyDialogAnthropic",
    "keyDialogOpenai",
    "keyDialogGemini",
  ].map((id) => $<HTMLInputElement>(id)).find((el) => !el.value.trim());
  (first ?? $<HTMLInputElement>("keyDialogAnthropic")).focus();
}

function hideKeyDialog(): void {
  $<HTMLDivElement>("keyOverlay").classList.add("hidden");
}

function applyDialogKeys(): boolean {
  const anthropic = $<HTMLInputElement>("keyDialogAnthropic").value.trim();
  const openai = $<HTMLInputElement>("keyDialogOpenai").value.trim();
  const gemini = $<HTMLInputElement>("keyDialogGemini").value.trim();
  setApiKeys({ anthropic, openai, gemini });
  if (!hasAnyApiKey()) {
    $<HTMLParagraphElement>("keyOverlayMessage").textContent = "Please enter at least one key.";
    return false;
  }
  hideKeyDialog();
  return true;
}

// ═══════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════

let runtime: RuntimeModule | null = null;
let mobId: string | null = null;
let subs: AgentSub[] = [];
let running = false;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let eventCount = 0;
let pendingApproval: { action_description: string; risk_level: string; proposed_by: string } | null = null;
let providerIssuePromptShown = false;

// ═══════════════════════════════════════════════════════════
// Init Canvas + Subsystems
// ═══════════════════════════════════════════════════════════

const canvasEl = $<HTMLCanvasElement>("officeCanvas");
initCanvas(canvasEl);
initCharacters();
initBubbles();
initPhoneLines();
loadBackground("./office-bg.png").catch(() => console.warn("Office background not found, using placeholder"));
loadSprites().catch(() => console.warn("Sprites not found, using fallback"));

// Filing cabinet click → toggle knowledge graph
setOnFilingCabinetClick(() => {
  const panel = $<HTMLDivElement>("panelContent");
  if (isGraphVisible()) {
    toggleKnowledgeGraph(panel);
    renderIncidentPanel(panel);
    $<HTMLDivElement>("panelContent").closest(".panel-col")!.querySelector(".panel-header")!.textContent = "Incidents";
  } else {
    toggleKnowledgeGraph(panel);
    $<HTMLDivElement>("panelContent").closest(".panel-col")!.querySelector(".panel-header")!.textContent = "Knowledge Graph";
  }
});

// Incident panel rendering
setRenderCallback(() => {
  renderIncidentPanel($<HTMLDivElement>("panelContent"));
  // Scroll to bottom
  const panel = $<HTMLDivElement>("panelContent");
  panel.scrollTop = panel.scrollHeight;
});

// Wire event system → incident tracking + knowledge extraction
setOnMessage((from, to, content, headline, category) => {
  addMessage(null, from, to, content, headline, category);
  // If archivist is involved, extract knowledge
  if (from === "archivist" || to === "archivist") {
    extractFacts(content);
  }
});

// Wire gate approval
setOnApprovalNeeded((data) => {
  pendingApproval = data;
  const overlay = $<HTMLDivElement>("approvalOverlay");
  $<HTMLDivElement>("approvalBody").innerHTML = `
    <p><strong>Action:</strong> ${data.action_description}</p>
    <p><strong>Risk:</strong> <span class="risk-${data.risk_level}">${data.risk_level.toUpperCase()}</span></p>
    <p><strong>Proposed by:</strong> ${data.proposed_by}</p>
  `;
  overlay.classList.remove("hidden");
});

// Agent selection
setOnSelectAgent((id: AgentId | null) => {
  if (id) {
    const a = AGENTS[id];
    setStatus(`Selected: ${a.name} (${a.role}) \u2014 ${a.zone}`);
    $<HTMLSelectElement>("chatAgent").value = id;
  } else {
    setStatus(running ? "Office running. Click an agent or inject an event." : "Click an agent to select them, or inject an event.");
  }
});

// API keys
initApiKeyInputs();

if (isServerMode()) {
  for (const id of ["keyAnthropic", "keyOpenai", "keyGemini"]) {
    const row = document.getElementById(id)?.closest(".setting") as HTMLElement | null;
    if (row) row.style.display = "none";
  }
  const drawer = document.getElementById("drawer");
  const h3s = drawer?.querySelectorAll("h3");
  if (h3s?.[0]?.textContent === "API Keys") h3s[0].style.display = "none";
  const hint = document.querySelector(".start-hint") as HTMLElement | null;
  if (hint) hint.textContent = "Model configured by server";
  setStatus("API keys provided by server.");
}

// Expose demo functions for console testing
(window as any).demo = {
  showSpeechBubble, showThinkBubble, hideThinkBubble,
  setAgentState, startCall, endCall, triggerEnvelopeArrival,
};

// ═══════════════════════════════════════════════════════════
// WASM Runtime
// ═══════════════════════════════════════════════════════════

async function loadRuntime(): Promise<RuntimeModule> {
  if (runtime) return runtime;
  const urls = [
    new URL("/meerkat-pkg/meerkat_web_runtime.js", window.location.href).toString(),
    new URL("./runtime.js", window.location.href).toString(), // legacy fallback
  ];
  let mod: RuntimeModule | null = null;
  let lastError = "";
  for (const url of urls) {
    try {
      mod = (await import(/* @vite-ignore */ url)) as RuntimeModule;
      break;
    } catch (err) {
      lastError = err instanceof Error ? err.message : String(err);
    }
  }
  if (!mod) {
    throw new Error(
      `Failed to load WASM runtime bundle. Tried: ${urls.join(", ")}. ` +
      `Run ./examples.sh in examples/034-the-office-demo-sh to build meerkat-pkg. ` +
      `Last error: ${lastError}`,
    );
  }
  await mod.default();
  runtime = mod;
  return mod;
}

// ═══════════════════════════════════════════════════════════
// Start Office
// ═══════════════════════════════════════════════════════════

async function startOffice(): Promise<void> {
  try {
    providerIssuePromptShown = false;
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
    running = false;

    const keys = getApiKeys();
    if (!keys.anthropic && !keys.openai && !keys.gemini) {
      setBadge("Ready");
      setStatus("No API keys detected. Enter one key to start.");
      showKeyDialog("No API keys detected. Enter at least one key to start.");
      return;
    }
    const model = getSelectedModel();
    if (!model) { setStatus("Select a model."); return; }
    const provider = MODEL_PROVIDER[model];
    if (provider && !keys[provider]) {
      setBadge("Ready");
      setStatus(`Model ${model} requires ${providerLabel(provider)} API key.`);
      showKeyDialog(`Model "${model}" uses ${providerLabel(provider)} and needs that API key.`);
      return;
    }

    setBadge("Loading...", true);
    setStatus("Loading WASM runtime...");
    const mod = await loadRuntime();

    // Setup network bridge
    setupBridge();

    // Init runtime
    setStatus("Initializing runtime...");
    const initConfig: Record<string, unknown> = { model };
    if (keys.anthropic) initConfig.anthropic_api_key = keys.anthropic;
    if (keys.openai) initConfig.openai_api_key = keys.openai;
    if (keys.gemini) initConfig.gemini_api_key = keys.gemini;
    const proxy = getServerProxyConfig();
    if (proxy) {
      initConfig.anthropic_base_url = `${proxy.proxyUrl}/anthropic`;
      initConfig.openai_base_url = `${proxy.proxyUrl}/openai`;
      initConfig.gemini_base_url = `${proxy.proxyUrl}/gemini`;
    }
    mod.init_runtime_from_config(JSON.stringify(initConfig));

    // Create mob
    setStatus("Creating office mob (10 agents)...");
    const def = buildOfficeDefinition(model);
    mobId = parseJsResult(await mod.mob_create(JSON.stringify(def)));

    // Spawn all agents
    setStatus("Spawning agents...");
    const specs = AGENT_IDS.map(id => ({
      profile: PROFILE_NAMES[id],
      meerkat_id: id,
      runtime_mode: "autonomous_host",
    }));
    await mod.mob_spawn(mobId, JSON.stringify(specs));

    // Wire comms
    setStatus("Wiring comms topology...");
    for (const [a, b] of WIRING_PAIRS) {
      try { await mod.mob_wire(mobId, a, b); }
      catch (e) { console.warn(`Wire ${a}\u2194${b} failed:`, e); }
    }

    // Subscribe to events
    setStatus("Subscribing to agent events...");
    subs = [];
    for (const id of AGENT_IDS) {
      try {
        const handle = await mod.mob_member_subscribe(mobId, id);
        subs.push({ agentId: id, handle });
      } catch (e) { console.warn(`Subscribe ${id} failed:`, e); }
    }

    // Start polling
    running = true;
    pollTimer = setInterval(() => {
      if (!running || !runtime) return;
      const { errors } = drainAllEvents(runtime, subs);
      if (errors.length > 0) {
        if (!providerIssuePromptShown) {
          const missingKeyError = errors.find((err) => {
            const lower = err.toLowerCase();
            return lower.includes("api key")
              && (lower.includes("missing")
                || lower.includes("not configured")
                || lower.includes("no provider api key")
                || lower.includes("must be provided"));
          });
          const authError = errors.find((err) => {
            const lower = err.toLowerCase();
            return lower.includes(" 401")
              || lower.includes("authentication")
              || lower.includes("unauthorized")
              || lower.includes("invalid x-api-key")
              || lower.includes("invalid api key");
          });
          const keyIssue = missingKeyError ?? authError;
          if (keyIssue) {
            providerIssuePromptShown = true;
            const provider = extractProviderFromError(keyIssue);
            const providerText = provider ? `${providerLabel(provider)} ` : "";
            const looksAuth = keyIssue === authError;
            running = false;
            const pauseBtn = document.getElementById("pauseBtn") as HTMLButtonElement | null;
            if (pauseBtn) pauseBtn.textContent = "Resume";
            setBadge("Config");
            if (looksAuth) {
              setStatus(`${providerText}key rejected. Update key or switch model, then Start again.`);
              showKeyDialog(`${providerText}API key appears invalid. Update key and press Save & Start.`);
            } else {
              setStatus(`${providerText}key missing for active model. Add key and Start again.`);
              showKeyDialog(`${providerText}API key is missing for the active model. Enter it to continue.`);
            }
          }
        }
        for (const e of errors) console.warn("Agent error:", e);
      }
    }, 300);

    setBadge("Live");
    setStatus("Office running. Inject an event with + Event, or talk to an agent.");
    $<HTMLDivElement>("drawer").classList.remove("open");
  } catch (e) {
    setBadge("Error");
    setStatus(`Failed: ${e instanceof Error ? e.message : String(e)}`);
    console.error("Start failed:", e);
  }
}

// ═══════════════════════════════════════════════════════════
// Inject Event
// ═══════════════════════════════════════════════════════════

async function injectEvent(scenarioId: string): Promise<void> {
  if (!runtime || !mobId) { setStatus("Start the office first."); return; }
  const scenario = SCENARIOS.find(s => s.id === scenarioId);
  if (!scenario) return;

  eventCount++;
  $<HTMLSpanElement>("eventCounter").textContent = `${eventCount} event${eventCount !== 1 ? "s" : ""}`;

  // Create incident
  createIncident(scenario.title, scenario.icon);
  addMessage(null, "system", "triage", scenario.text, `New event: ${scenario.title}`, "routing");

  // Visual: envelope arrival
  triggerEnvelopeArrival("triage");
  setBadge("Processing...", true);
  setStatus(`Event: ${scenario.title} \u2014 routing to triage...`);

  // Inject into triage agent
  try {
    await runtime.mob_send_message(mobId, "triage", scenario.text);
  } catch (e) {
    setStatus(`Failed to inject event: ${e instanceof Error ? e.message : String(e)}`);
  }
}

// ═══════════════════════════════════════════════════════════
// Chat with Agent
// ═══════════════════════════════════════════════════════════

async function chatWithAgent(agentId: AgentId, message: string): Promise<void> {
  if (!runtime || !mobId) { setStatus("Start the office first."); return;  }

  createIncident(`You \u2192 ${AGENTS[agentId].name}: ${message.slice(0, 30)}...`, "\u{1F4AC}");
  addMessage(null, "user", agentId, message, message.slice(0, 50), "response");

  // Visual: phone call from bottom of screen to agent
  showSpeechBubble(agentId, "Incoming call...", 2000);

  try {
    await runtime.mob_send_message(mobId, agentId, message);
  } catch (e) {
    setStatus(`Failed to send message: ${e instanceof Error ? e.message : String(e)}`);
  }
}

// ═══════════════════════════════════════════════════════════
// Event Handlers
// ═══════════════════════════════════════════════════════════

document.getElementById("startBigBtn")!.addEventListener("click", () => {
  $<HTMLDivElement>("startOverlay").classList.add("hidden");
});

document.getElementById("startBtn")!.addEventListener("click", () => void startOffice());
document.getElementById("keyDialogCancel")!.addEventListener("click", () => {
  hideKeyDialog();
  setStatus("Start is blocked until at least one API key is provided.");
});
document.getElementById("keyDialogSave")!.addEventListener("click", () => {
  if (applyDialogKeys()) {
    setStatus("API key saved. Starting office...");
    void startOffice();
  }
});
for (const id of ["keyDialogAnthropic", "keyDialogOpenai", "keyDialogGemini"] as const) {
  $<HTMLInputElement>(id).addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (applyDialogKeys()) {
        setStatus("API key saved. Starting office...");
        void startOffice();
      }
    }
  });
}

document.getElementById("pauseBtn")!.addEventListener("click", () => {
  if (!running && pollTimer === null) return;
  running = !running;
  (document.getElementById("pauseBtn") as HTMLButtonElement).textContent = running ? "Pause" : "Resume";
  if (running) {
    setBadge("Live");
    setStatus("Office running.");
  } else {
    setBadge("Paused");
    setStatus("Office paused.");
  }
});

// Event dropdown
document.getElementById("eventBtn")!.addEventListener("click", () => {
  $<HTMLDivElement>("eventMenu").classList.toggle("hidden");
});
document.getElementById("eventMenu")!.addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest("[data-scenario]") as HTMLElement | null;
  if (!btn) return;
  $<HTMLDivElement>("eventMenu").classList.add("hidden");
  void injectEvent(btn.dataset.scenario!);
});
// Close dropdown on outside click
document.addEventListener("click", (e) => {
  if (!(e.target as HTMLElement).closest("#eventDropdown")) {
    $<HTMLDivElement>("eventMenu").classList.add("hidden");
  }
});

// Chat
document.getElementById("chatSend")!.addEventListener("click", () => {
  const input = $<HTMLInputElement>("chatInput");
  const agentId = $<HTMLSelectElement>("chatAgent").value as AgentId;
  const msg = input.value.trim();
  if (!msg) return;
  input.value = "";
  void chatWithAgent(agentId, msg);
});
$<HTMLInputElement>("chatInput").addEventListener("keydown", (e) => {
  if (e.key === "Enter") document.getElementById("chatSend")!.click();
});

// Approval
document.getElementById("approveBtn")!.addEventListener("click", () => {
  if (pendingApproval && runtime && mobId) {
    runtime.mob_send_message(mobId, "gate", `HUMAN DECISION: APPROVED \u2014 ${pendingApproval.action_description}`);
    addMessage(null, "user", "gate", `APPROVED: ${pendingApproval.action_description}`, "Human approved", "approval");
    showSpeechBubble("gate", "APPROVED \u2714", 3000);
  }
  pendingApproval = null;
  $<HTMLDivElement>("approvalOverlay").classList.add("hidden");
});
document.getElementById("denyBtn")!.addEventListener("click", () => {
  if (pendingApproval && runtime && mobId) {
    runtime.mob_send_message(mobId, "gate", `HUMAN DECISION: DENIED \u2014 ${pendingApproval.action_description}`);
    addMessage(null, "user", "gate", `DENIED: ${pendingApproval.action_description}`, "Human denied", "approval");
    showSpeechBubble("gate", "DENIED \u2718", 3000);
  }
  pendingApproval = null;
  $<HTMLDivElement>("approvalOverlay").classList.add("hidden");
});

// Settings
document.getElementById("gearBtn")!.addEventListener("click", () =>
  $<HTMLDivElement>("drawer").classList.toggle("open"));
document.getElementById("closeDrawer")!.addEventListener("click", () =>
  $<HTMLDivElement>("drawer").classList.remove("open"));

// Server mode: skip overlay and auto-start
if (isServerMode()) {
  $<HTMLDivElement>("startOverlay").classList.add("hidden");
  document.getElementById("startBtn")!.style.display = "none";
  void startOffice();
} else if (!hasAnyApiKey()) {
  setStatus("No API keys detected. Press Start to open key entry dialog.");
} else {
  setStatus("API keys loaded. Press Start.");
}
