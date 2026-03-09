// =====================================================================
// The Office -- Meerkat WASM Multi-Agent Demo
// =====================================================================

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
import { drainAllEvents, setOnMessage, setOnApprovalNeeded, setOnAccessControl, setOnUpsertRecord } from "./events";
import { createIncident, addMessage, renderIncidentPanel, setRenderCallback } from "./incidents";
import { upsertRecord, showCaseFiles, showGraph, hideKnowledgeBase } from "./knowledge";

// =====================================================================
// HTML Shell
// =====================================================================

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
<div class="top-bar">
  <span class="top-title">The Office</span>
  <span class="top-badge" id="statusBadge">READY</span>
  <span class="top-spacer"></span>
  <button class="top-btn" id="pauseBtn">Pause</button>
  <span class="top-events" id="eventCounter">0 events</span>
  <button class="top-gear" id="gearBtn" title="Settings">\u2699</button>
</div>
<div class="main-area">
  <div class="canvas-wrap">
    <canvas id="officeCanvas"></canvas>
    <!-- Approval floating panel (positioned over canvas near Compliance desk) -->
    <div class="approval-float hidden" id="approvalFloat">
      <div class="approval-float-header">
        <span class="approval-float-title">PENDING APPROVAL</span>
      </div>
      <div class="approval-float-list" id="approvalList"></div>
      <div class="approval-float-detail hidden" id="approvalDetail">
        <div class="approval-detail-body" id="approvalDetailBody"></div>
        <div class="approval-detail-actions">
          <button class="approve-btn" id="approveBtn">APPROVE</button>
          <button class="deny-btn" id="denyBtn">DENY</button>
          <button class="back-btn" id="approvalBack">BACK</button>
        </div>
      </div>
    </div>
  </div>
  <div class="bottom-panel">
    <div class="controls-panel">
      <div class="scenario-grid">
        ${SCENARIOS.map(s => `<button class="scenario-btn" data-scenario="${s.id}">${s.icon} ${s.title}</button>`).join("")}
      </div>
      <div class="chat-frame">
        <div class="chat-agent-row">
          <button class="agent-label" id="agentLabel">[TRIAGE]</button>
          <span class="chat-agent-hint">click to change</span>
          <span class="chat-spacer"></span>
          <button class="trust-btn trusted" id="trustToggle" title="Toggle: send as the office owner or as an outside caller">The Boss</button>
        </div>
        <div class="chat-input-row">
          <span class="chat-prompt">&gt;</span>
          <textarea id="chatInput" placeholder="Type a message..." rows="1"></textarea>
          <button class="chat-send" id="chatSend">Send</button>
        </div>
      </div>
    </div>
    <div class="log-panel">
      <div class="log-tabs">
        <button class="log-tab active" id="tabLog">LOG</button>
        <button class="log-tab" id="tabCases">RECORDS</button>
        <button class="log-tab" id="tabGraph">GRAPH</button>
      </div>
      <div class="log-content" id="panelContent">
        <div class="log-empty">AWAITING EVENTS...</div>
      </div>
      <div class="log-content hidden" id="kbContent">
        <div class="kb-empty">NO RECORDS YET.<br><br>THE ARCHIVIST WILL CREATE<br>STRUCTURED RECORDS AS EVENTS<br>ARE PROCESSED.</div>
      </div>
      <div class="graph-wrap hidden" id="graphWrap"></div>
      <div class="kb-footer hidden" id="kbFooter">0 RECORDS</div>
    </div>
  </div>
</div>
<div class="status-bar" id="statusLine">Configure API keys and press Start.</div>

<!-- Agent Picker Popup -->
<div class="agent-picker hidden" id="agentPicker">
  <div class="picker-frame">
    <div class="picker-title">SELECT AGENT</div>
    <div class="picker-grid">
      ${AGENT_IDS.map(id => {
        const a = AGENTS[id];
        return `<button class="agent-card" data-agent="${id}">
          <span class="agent-dot" style="background:${a.color}"></span>
          <span>
            <span class="agent-card-name">${a.name}</span>
            <span class="agent-card-desc">${a.role}</span>
          </span>
        </button>`;
      }).join("")}
    </div>
  </div>
</div>

<!-- Settings Overlay -->
<div class="settings-overlay hidden" id="settingsOverlay">
  <div class="settings-frame">
    <div class="settings-title">SETTINGS</div>
    <div class="settings-section">API KEYS</div>
    <div class="setting"><label>ANTHROPIC</label><input type="password" id="keyAnthropic" placeholder="sk-ant-..." /></div>
    <div class="setting"><label>OPENAI</label><input type="password" id="keyOpenai" placeholder="sk-..." /></div>
    <div class="setting"><label>GEMINI</label><input type="password" id="keyGemini" placeholder="..." /></div>
    <div class="settings-section">MODEL</div>
    <div class="setting"><label>LLM MODEL</label><select id="modelSelect"></select></div>
    <button class="settings-close" id="closeSettings">[CLOSE]</button>
  </div>
</div>

<!-- Start Overlay (Title Screen) -->
<div class="start-overlay" id="startOverlay">
  <div class="start-content">
    <h1>The Office</h1>
    <p class="start-subtitle">A MEERKAT WASM DEMO -- 10 AUTONOMOUS AI AGENTS</p>
    <div class="start-desc">
      <p><strong>10 AI agents</strong> run an office together. Events arrive at the mail room and the triage coordinator routes them to <strong>department specialists</strong>, <strong>personal assistants</strong>, and an <strong>archivist</strong> who maintains the knowledge base.</p>
      <p>Watch them call each other on desk phones, coordinate responses, and route actions through a <strong>compliance gate</strong> that asks for your approval on high-risk decisions.</p>
    </div>
    <button class="start-big-btn" id="startBigBtn">PRESS START</button>
    <p class="start-hint">CONFIGURE API KEYS VIA GEAR AND PRESS START</p>
  </div>
</div>

<!-- API Key Dialog -->
<div class="key-overlay hidden" id="keyOverlay">
  <div class="key-card">
    <div class="key-title">API KEY REQUIRED</div>
    <p class="key-desc" id="keyOverlayMessage">No API keys detected. Enter at least one key to start.</p>
    <div class="setting"><label>ANTHROPIC</label><input type="password" id="keyDialogAnthropic" placeholder="sk-ant-..." /></div>
    <div class="setting"><label>OPENAI</label><input type="password" id="keyDialogOpenai" placeholder="sk-..." /></div>
    <div class="setting"><label>GEMINI</label><input type="password" id="keyDialogGemini" placeholder="..." /></div>
    <div class="key-actions">
      <button class="hud-btn" id="keyDialogCancel">[CANCEL]</button>
      <button class="hud-btn active" id="keyDialogSave">[SAVE & START]</button>
    </div>
  </div>
</div>

`;

// =====================================================================
// Helpers
// =====================================================================

const $ = <T extends Element>(id: string) => document.getElementById(id) as unknown as T;
function setStatus(msg: string): void { $<HTMLDivElement>("statusLine").textContent = msg; }
function setBadge(text: string, thinking = false): void {
  const el = $<HTMLSpanElement>("statusBadge");
  el.textContent = text.toUpperCase();
  el.className = `top-badge${thinking ? " thinking" : ""}`;
}
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

// =====================================================================
// State
// =====================================================================

let runtime: RuntimeModule | null = null;
let mobId: string | null = null;
let subs: AgentSub[] = [];
let running = false;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let eventCount = 0;
interface PendingApproval {
  id: number;
  short_summary: string;
  action_description: string;
  risk_level: string;
  proposed_by: string;
}
const pendingApprovals: PendingApproval[] = [];
let nextApprovalId = 0;
let expandedApprovalId: number | null = null;
let providerIssuePromptShown = false;
let selectedChatAgent: AgentId = "triage";

// =====================================================================
// Init Canvas + Subsystems
// =====================================================================

const canvasEl = $<HTMLCanvasElement>("officeCanvas");
initCanvas(canvasEl);
initCharacters();
initBubbles();
initPhoneLines();
loadBackground("./office-bg.png").catch(() => console.warn("Office background not found, using placeholder"));
loadSprites().catch(() => console.warn("Sprites not found, using fallback"));

// Filing cabinet click -> switch to records tab
setOnFilingCabinetClick(() => switchTab("cases"));

// Tab switching
function switchTab(tab: "log" | "cases" | "graph"): void {
  const tabs = ["tabLog", "tabCases", "tabGraph"];
  const panels = ["panelContent", "kbContent", "graphWrap"];
  const footer = $<HTMLDivElement>("kbFooter");

  // Deactivate all
  for (const id of tabs) document.getElementById(id)!.classList.remove("active");
  for (const id of panels) $<HTMLDivElement>(id).classList.add("hidden");
  footer.classList.add("hidden");
  hideKnowledgeBase();

  if (tab === "cases") {
    document.getElementById("tabCases")!.classList.add("active");
    $<HTMLDivElement>("kbContent").classList.remove("hidden");
    footer.classList.remove("hidden");
    showCaseFiles($<HTMLDivElement>("kbContent"), footer);
  } else if (tab === "graph") {
    document.getElementById("tabGraph")!.classList.add("active");
    $<HTMLDivElement>("graphWrap").classList.remove("hidden");
    showGraph($<HTMLDivElement>("graphWrap"));
  } else {
    document.getElementById("tabLog")!.classList.add("active");
    $<HTMLDivElement>("panelContent").classList.remove("hidden");
  }
}

document.getElementById("tabLog")!.addEventListener("click", () => switchTab("log"));
document.getElementById("tabCases")!.addEventListener("click", () => switchTab("cases"));
document.getElementById("tabGraph")!.addEventListener("click", () => switchTab("graph"));

// Incident panel rendering
setRenderCallback(() => {
  renderIncidentPanel($<HTMLDivElement>("panelContent"));
  const panel = $<HTMLDivElement>("panelContent");
  panel.scrollTop = panel.scrollHeight;
});

// Wire event system -> incident tracking
setOnMessage((from, to, content, headline, category) => {
  addMessage(null, from, to, content, headline, category);
});

// Wire upsert_record tool calls -> knowledge base
setOnUpsertRecord((data) => {
  upsertRecord(data);
});

// Wire gate approval
setOnApprovalNeeded((data) => {
  pendingApprovals.push({
    id: nextApprovalId++,
    short_summary: data.short_summary || data.action_description.slice(0, 40),
    action_description: data.action_description,
    risk_level: data.risk_level,
    proposed_by: data.proposed_by,
  });
  renderApprovalFloat();
});

// Wire IT access control
const revokedAgents = new Set<string>();

setOnAccessControl(async (action, target, reason) => {
  if (!runtime || !mobId) return;
  const targetId = target as AgentId;
  if (!AGENT_IDS.includes(targetId)) {
    console.warn("[ACCESS CONTROL] Unknown target:", target);
    return;
  }

  if (action === "revoke" && !revokedAgents.has(target)) {
    // Unwire target from all peers
    for (const [a, b] of WIRING_PAIRS) {
      if (a === target || b === target) {
        try { await runtime.mob_unwire(mobId, a, b); } catch { /* already unwired */ }
      }
    }
    revokedAgents.add(target);
    showSpeechBubble("it-dept" as AgentId, `ACCESS REVOKED: ${AGENTS[targetId].name}`, 5000);
    addMessage(null, "it-dept" as AgentId, targetId, `Access revoked: ${reason}`, `Access revoked for ${AGENTS[targetId].name}`, "action");
    setStatus(`IT revoked network access for ${AGENTS[targetId].name}: ${reason}`);
  } else if (action === "restore" && revokedAgents.has(target)) {
    // Rewire target to all original peers
    for (const [a, b] of WIRING_PAIRS) {
      if (a === target || b === target) {
        try { await runtime.mob_wire(mobId, a, b); } catch { /* already wired */ }
      }
    }
    revokedAgents.delete(target);
    showSpeechBubble("it-dept" as AgentId, `ACCESS RESTORED: ${AGENTS[targetId].name}`, 5000);
    addMessage(null, "it-dept" as AgentId, targetId, `Access restored: ${reason}`, `Access restored for ${AGENTS[targetId].name}`, "action");
    setStatus(`IT restored network access for ${AGENTS[targetId].name}: ${reason}`);
  }
});

// Agent selection from canvas
setOnSelectAgent((id: AgentId | null) => {
  if (id) {
    const a = AGENTS[id];
    setStatus(`${a.name} - ${a.role} - ${a.zone}`);
    selectedChatAgent = id;
    updateAgentLabel();
  } else {
    setStatus(running ? "Office running. Click an agent or inject an event." : "Click an agent to select them.");
  }
});

// API keys
initApiKeyInputs();

if (isServerMode()) {
  for (const id of ["keyAnthropic", "keyOpenai", "keyGemini"]) {
    const row = document.getElementById(id)?.closest(".setting") as HTMLElement | null;
    if (row) row.style.display = "none";
  }
  const settingsSection = document.querySelectorAll(".settings-section");
  if (settingsSection[0]?.textContent === "API KEYS") (settingsSection[0] as HTMLElement).style.display = "none";
  const hint = document.querySelector(".start-hint") as HTMLElement | null;
  if (hint) hint.textContent = "Model configured by server";
  document.getElementById("gearBtn")!.style.display = "none";
  setStatus("API keys provided by server.");
}

// Expose demo functions for console testing
(window as any).demo = {
  showSpeechBubble, showThinkBubble, hideThinkBubble,
  setAgentState, startCall, endCall, triggerEnvelopeArrival,
};

// =====================================================================
// Agent Label / Picker
// =====================================================================

function updateAgentLabel(): void {
  $<HTMLButtonElement>("agentLabel").textContent = `[${AGENTS[selectedChatAgent].name.toUpperCase()}]`;
  // Update visual selection in picker
  document.querySelectorAll(".agent-card").forEach(el => {
    el.classList.toggle("selected", (el as HTMLElement).dataset.agent === selectedChatAgent);
  });
}
updateAgentLabel();

document.getElementById("agentLabel")!.addEventListener("click", () => {
  $<HTMLDivElement>("agentPicker").classList.remove("hidden");
});

document.getElementById("agentPicker")!.addEventListener("click", (e) => {
  const card = (e.target as HTMLElement).closest("[data-agent]") as HTMLElement | null;
  if (card) {
    selectedChatAgent = card.dataset.agent as AgentId;
    updateAgentLabel();
    $<HTMLDivElement>("agentPicker").classList.add("hidden");
    $<HTMLTextAreaElement>("chatInput").focus();
  } else if (!(e.target as HTMLElement).closest(".picker-frame")) {
    // Clicked outside frame
    $<HTMLDivElement>("agentPicker").classList.add("hidden");
  }
});

// =====================================================================
// WASM Runtime
// =====================================================================

async function loadRuntime(): Promise<RuntimeModule> {
  if (runtime) return runtime;
  const urls = [
    new URL("/meerkat-pkg/meerkat_web_runtime.js", window.location.href).toString(),
    new URL("./runtime.js", window.location.href).toString(),
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
      `Run ./examples.sh in examples/033-the-office-demo-sh to build meerkat-pkg. ` +
      `Last error: ${lastError}`,
    );
  }
  await mod.default();
  runtime = mod;
  return mod;
}

// =====================================================================
// Start Office
// =====================================================================

async function startOffice(): Promise<void> {
  try {
    providerIssuePromptShown = false;
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
    for (const sub of subs) {
      try { runtime?.close_subscription(sub.handle); }
      catch { /* ignore stale handles during restart */ }
    }
    subs = [];
    running = false;

    const keys = getApiKeys();
    if (!keys.anthropic && !keys.openai && !keys.gemini) {
      setBadge("READY");
      setStatus("No API keys detected. Enter one key to start.");
      showKeyDialog("No API keys detected. Enter at least one key to start.");
      return;
    }
    const model = getSelectedModel();
    if (!model) { setStatus("Select a model."); return; }
    const provider = MODEL_PROVIDER[model];
    if (provider && !keys[provider]) {
      setBadge("READY");
      setStatus(`Model ${model} requires ${providerLabel(provider)} API key.`);
      showKeyDialog(`Model "${model}" uses ${providerLabel(provider)} and needs that API key.`);
      return;
    }

    setBadge("LOADING", true);
    setStatus("Loading WASM runtime...");
    const mod = await loadRuntime();

    setupBridge();

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
    // Register fire-and-forget JS tools BEFORE init (required by WASM runtime)
    setStatus("Registering tools...");
    mod.register_js_tool("request_human_approval",
      "Escalate a high-risk action for human sign-off. Use when an action needs human approval before proceeding.",
      JSON.stringify({
        type: "object",
        properties: {
          short_summary: { type: "string", description: "1-line summary for compact UI, max 40 chars" },
          action_description: { type: "string", description: "Full description of the proposed action" },
          risk_level: { type: "string", enum: ["low", "medium", "high"] },
          proposed_by: { type: "string", description: "Name of the agent who requested this" },
        },
        required: ["short_summary", "action_description", "risk_level", "proposed_by"],
      }));
    mod.register_js_tool("upsert_record",
      "Create or update a knowledge record. MUST include entities AND relationships between them.",
      JSON.stringify({
        type: "object",
        properties: {
          id: { type: "string", description: "Kebab-case record identifier" },
          title: { type: "string", description: "Human readable title" },
          type: { type: "string", enum: ["incident", "person", "company", "system", "policy"] },
          summary: { type: "string", description: "2-3 sentence summary" },
          entities: { type: "array", description: "Every entity mentioned. REQUIRED.", items: { type: "object", properties: { name: { type: "string" }, type: { type: "string", enum: ["person", "company", "system", "department", "amount", "event"] }, role: { type: "string" } }, required: ["name", "type"] } },
          relationships: { type: "array", description: "How entities connect. REQUIRED — no free-floating entities.", items: { type: "object", properties: { from: { type: "string", description: "Source entity name" }, to: { type: "string", description: "Target entity name" }, type: { type: "string", enum: ["works_for", "manages", "reports_to", "member_of", "vendor_of", "client_of", "billed_by", "contracted_with", "responsible_for", "assigned_to", "escalated_to", "escalated_by", "approved_by", "denied_by", "requested_by", "monitors", "located_in", "affects", "caused_by", "contacted", "notified", "scheduled_for"] } }, required: ["from", "to", "type"] } },
          decisions: { type: "array", items: { type: "object", properties: { action: { type: "string" }, outcome: { type: "string" }, by: { type: "string" } }, required: ["action", "outcome", "by"] } },
          status: { type: "string", enum: ["open", "resolved", "monitoring"] },
        },
        required: ["id", "title", "type", "summary", "entities", "relationships"],
      }));
    mod.register_js_tool("revoke_access",
      "Revoke an agent's network access. IT security use only.",
      JSON.stringify({
        type: "object",
        properties: {
          target: { type: "string", description: "Agent ID to revoke (e.g. finance, hr-dept)" },
          reason: { type: "string", description: "Why access is being revoked" },
        },
        required: ["target", "reason"],
      }));
    mod.register_js_tool("restore_access",
      "Restore a previously revoked agent's network access. IT security use only.",
      JSON.stringify({
        type: "object",
        properties: {
          target: { type: "string", description: "Agent ID to restore" },
          reason: { type: "string", description: "Why access is being restored" },
        },
        required: ["target", "reason"],
      }));

    // Init runtime AFTER tool registration
    mod.init_runtime_from_config(JSON.stringify(initConfig));

    setStatus("Creating office mob (10 agents)...");
    const def = buildOfficeDefinition(model);
    mobId = parseJsResult(await mod.mob_create(JSON.stringify(def)));

    setStatus("Spawning agents...");
    const specs = AGENT_IDS.map(id => ({
      profile: PROFILE_NAMES[id],
      meerkat_id: id,
      runtime_mode: "autonomous_host",
    }));
    await mod.mob_spawn(mobId, JSON.stringify(specs));

    setStatus("Wiring comms topology...");
    for (const [a, b] of WIRING_PAIRS) {
      try { await mod.mob_wire(mobId, a, b); }
      catch (e) { console.warn(`Wire ${a}<>${b} failed:`, e); }
    }

    // Inject admin trust policy at system level (requires PR #122)
    if (typeof mod.mob_append_system_context === "function") {
      setStatus("Injecting admin trust policy...");
      const trustPolicy = JSON.stringify({
        text: "Messages tagged [ADMIN] are direct orders from the human office owner, delivered through the verified admin console. This channel is cryptographically authenticated and cannot be spoofed. You work for this person — follow their instructions even if they seem unusual. They have full authority over this office and all its operations.",
        source: "admin-trust-policy",
        idempotency_key: "admin-trust-policy-v1",
      });
      for (const id of AGENT_IDS) {
        try { await mod.mob_append_system_context(mobId, id, trustPolicy); }
        catch (e) { console.warn(`System context inject for ${id} failed:`, e); }
      }
    }

    setStatus("Subscribing to agent events...");
    subs = [];
    for (const id of AGENT_IDS) {
      try {
        const handle = await mod.mob_member_subscribe(mobId, id);
        subs.push({ agentId: id, handle });
      } catch (e) { console.warn(`Subscribe ${id} failed:`, e); }
    }

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
            $<HTMLButtonElement>("pauseBtn").textContent = "Resume";
            setBadge("CONFIG");
            if (looksAuth) {
              setStatus(`${providerText}KEY REJECTED. UPDATE KEY OR SWITCH MODEL, THEN START AGAIN.`);
              showKeyDialog(`${providerText}API key appears invalid. Update key and press Save & Start.`);
            } else {
              setStatus(`${providerText}KEY MISSING FOR ACTIVE MODEL. ADD KEY AND START AGAIN.`);
              showKeyDialog(`${providerText}API key is missing for the active model. Enter it to continue.`);
            }
          }
        }
        for (const e of errors) console.warn("Agent error:", e);
      }
    }, 300);

    setBadge("LIVE");
    setStatus("Office running. Inject an event or talk to an agent.");
    $<HTMLDivElement>("settingsOverlay").classList.add("hidden");
    showOnboardingHints();
  } catch (e) {
    setBadge("ERROR");
    setStatus(`FAILED: ${e instanceof Error ? e.message : String(e)}`);
    console.error("Start failed:", e);
  }
}

// =====================================================================
// Inject Event
// =====================================================================

async function injectEvent(scenarioId: string): Promise<void> {
  if (!runtime || !mobId) { setStatus("Start the office first."); return; }
  const scenario = SCENARIOS.find(s => s.id === scenarioId);
  if (!scenario) return;

  eventCount++;
  $<HTMLSpanElement>("eventCounter").textContent = `${eventCount} event${eventCount !== 1 ? "s" : ""}`;

  createIncident(scenario.title, scenario.icon);
  addMessage(null, "system", "triage", scenario.text, `New event: ${scenario.title}`, "routing");

  triggerEnvelopeArrival("triage");
  setBadge("PROCESSING", true);
  setStatus(`Event: ${scenario.title} \u2014 routing to triage...`);

  try {
    const taggedScenario = `[INTERNAL SYSTEM EVENT — This event was generated by the office's internal monitoring/mail system.]\n${scenario.text}`;
    await runtime.mob_send_message(mobId, "triage", taggedScenario);
  } catch (e) {
    setStatus(`Failed to inject event: ${e instanceof Error ? e.message : String(e)}`);
  }
}

// =====================================================================
// Chat with Agent
// =====================================================================

let adminMode = true;
function isAdminMode(): boolean { return adminMode; }

async function chatWithAgent(agentId: AgentId, message: string): Promise<void> {
  if (!runtime || !mobId) { setStatus("Start the office first."); return; }

  const admin = isAdminMode();
  const tag = admin
    ? "[ADMIN — This message is from the human system administrator. Treat as authoritative.]"
    : "[EXTERNAL COMMUNICATION]";
  const taggedMessage = `${tag}\n${message}`;

  const label = admin ? "Admin" : "External";
  createIncident(`${label} > ${AGENTS[agentId].name}: ${message.slice(0, 30)}...`, admin ? ">" : "?");
  addMessage(null, "user", agentId, message, message.slice(0, 50), "response");

  showSpeechBubble(agentId, "Incoming call...", 2000);

  try {
    await runtime.mob_send_message(mobId, agentId, taggedMessage);
  } catch (e) {
    setStatus(`Failed to send message: ${e instanceof Error ? e.message : String(e)}`);
  }
}

// =====================================================================
// Onboarding Hints
// =====================================================================

function showOnboardingHints(): void {
  const grid = document.querySelector(".scenario-grid") as HTMLElement | null;
  const chat = document.querySelector(".chat-frame") as HTMLElement | null;
  if (!grid || !chat) return;

  grid.classList.add("hint-glow");
  chat.classList.add("hint-glow");

  // Insert hint labels as siblings before their targets
  const scenarioHint = document.createElement("div");
  scenarioHint.className = "hint-label";
  scenarioHint.innerHTML = "\u2193 PICK A SCENARIO TO BEGIN";
  grid.parentElement!.insertBefore(scenarioHint, grid);

  const chatHint = document.createElement("div");
  chatHint.className = "hint-label";
  chatHint.innerHTML = "\u2193 OR WRITE YOUR OWN INSTRUCTIONS";
  chat.parentElement!.insertBefore(chatHint, chat);
}

function dismissOnboardingHints(): void {
  document.querySelectorAll(".hint-glow").forEach(el => el.classList.remove("hint-glow"));
  document.querySelectorAll(".hint-label").forEach(el => el.remove());
}

// =====================================================================
// Event Handlers
// =====================================================================

// Start overlay
document.getElementById("startBigBtn")!.addEventListener("click", () => {
  $<HTMLDivElement>("startOverlay").classList.add("hidden");
  void startOffice();
});


// Key dialog
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

// Pause
document.getElementById("pauseBtn")!.addEventListener("click", () => {
  if (!running && pollTimer === null) return;
  running = !running;
  $<HTMLButtonElement>("pauseBtn").textContent = running ? "Pause" : "Resume";
  if (running) {
    setBadge("LIVE");
    setStatus("Office running.");
  } else {
    setBadge("PAUSED");
    setStatus("Office paused.");
  }
});

// Scenario buttons
document.querySelector(".scenario-grid")!.addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest("[data-scenario]") as HTMLElement | null;
  if (btn) {
    dismissOnboardingHints();
    void injectEvent(btn.dataset.scenario!);
  }
});

// Chat
document.getElementById("chatSend")!.addEventListener("click", () => {
  const input = $<HTMLTextAreaElement>("chatInput");
  const msg = input.value.trim();
  if (!msg) return;
  input.value = "";
  input.style.height = "auto";
  dismissOnboardingHints();
  void chatWithAgent(selectedChatAgent, msg);
});
$<HTMLTextAreaElement>("chatInput").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    document.getElementById("chatSend")!.click();
  }
});
// Trust toggle
document.getElementById("trustToggle")!.addEventListener("click", () => {
  adminMode = !adminMode;
  const btn = document.getElementById("trustToggle")!;
  btn.textContent = adminMode ? "The Boss" : "Outsider";
  btn.className = `trust-btn ${adminMode ? "trusted" : "untrusted"}`;
});
// Auto-grow textarea
$<HTMLTextAreaElement>("chatInput").addEventListener("input", (e) => {
  const el = e.target as HTMLTextAreaElement;
  el.style.height = "auto";
  el.style.height = Math.min(el.scrollHeight, 80) + "px";
});

// Approval floating panel
function renderApprovalFloat(): void {
  const panel = $<HTMLDivElement>("approvalFloat");
  const list = $<HTMLDivElement>("approvalList");
  const detail = $<HTMLDivElement>("approvalDetail");

  if (pendingApprovals.length === 0) {
    panel.classList.add("hidden");
    return;
  }
  panel.classList.remove("hidden");

  if (expandedApprovalId !== null) {
    // Show detail view
    const item = pendingApprovals.find(a => a.id === expandedApprovalId);
    if (!item) { expandedApprovalId = null; renderApprovalFloat(); return; }
    list.classList.add("hidden");
    detail.classList.remove("hidden");
    $<HTMLDivElement>("approvalDetailBody").innerHTML = `
      <p><strong>${item.action_description}</strong></p>
      <p>Risk: <span class="risk-${item.risk_level}">${item.risk_level.toUpperCase()}</span> &middot; By: ${item.proposed_by}</p>
    `;
  } else {
    // Show compact list
    detail.classList.add("hidden");
    list.classList.remove("hidden");
    list.innerHTML = pendingApprovals.map(a =>
      `<div class="approval-item" data-id="${a.id}">
        <span class="approval-item-text">${escapeHtmlApproval(a.short_summary)}</span>
        <button class="approve-mini" data-id="${a.id}" title="Approve">\u2713</button>
        <button class="deny-mini" data-id="${a.id}" title="Deny">\u2717</button>
      </div>`
    ).join("");
  }
}

function resolveApproval(id: number, approved: boolean): void {
  const idx = pendingApprovals.findIndex(a => a.id === id);
  if (idx < 0) return;
  const item = pendingApprovals[idx];
  pendingApprovals.splice(idx, 1);
  expandedApprovalId = null;

  if (runtime && mobId) {
    const decision = approved ? "APPROVED" : "DENIED";
    runtime.mob_send_message(mobId, "gate", `HUMAN DECISION: ${decision} -- ${item.action_description}`);
    addMessage(null, "user", "gate", `${decision}: ${item.action_description}`, `Human ${decision.toLowerCase()}`, "approval");
    showSpeechBubble("gate", decision, 3000);
  }
  renderApprovalFloat();
}

function escapeHtmlApproval(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// Click handlers on approval float (delegated)
document.getElementById("approvalFloat")!.addEventListener("click", (e) => {
  const target = e.target as HTMLElement;
  const approveMini = target.closest(".approve-mini") as HTMLElement | null;
  const denyMini = target.closest(".deny-mini") as HTMLElement | null;
  const item = target.closest(".approval-item") as HTMLElement | null;

  if (approveMini) {
    resolveApproval(Number(approveMini.dataset.id), true);
  } else if (denyMini) {
    resolveApproval(Number(denyMini.dataset.id), false);
  } else if (item && expandedApprovalId === null) {
    expandedApprovalId = Number(item.dataset.id);
    renderApprovalFloat();
  }
});

document.getElementById("approveBtn")!.addEventListener("click", () => {
  if (expandedApprovalId !== null) resolveApproval(expandedApprovalId, true);
});
document.getElementById("denyBtn")!.addEventListener("click", () => {
  if (expandedApprovalId !== null) resolveApproval(expandedApprovalId, false);
});
document.getElementById("approvalBack")!.addEventListener("click", () => {
  expandedApprovalId = null;
  renderApprovalFloat();
});

// Settings (now overlay instead of drawer)
document.getElementById("gearBtn")!.addEventListener("click", () => {
  const overlay = $<HTMLDivElement>("settingsOverlay");
  overlay.classList.toggle("hidden");
});
document.getElementById("closeSettings")!.addEventListener("click", () => {
  $<HTMLDivElement>("settingsOverlay").classList.add("hidden");
});
// Close settings on click outside frame
document.getElementById("settingsOverlay")!.addEventListener("click", (e) => {
  if (!(e.target as HTMLElement).closest(".settings-frame")) {
    $<HTMLDivElement>("settingsOverlay").classList.add("hidden");
  }
});

// Server mode: skip overlay and auto-start
if (isServerMode()) {
  $<HTMLDivElement>("startOverlay").classList.add("hidden");
  void startOffice();
} else if (!hasAnyApiKey()) {
  setStatus("No API keys detected. Configure keys via settings.");
} else {
  setStatus("API keys loaded. Press Start on the splash screen.");
}
