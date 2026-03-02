/**
 * 032 — Meerkat WebCM Agent (TUI)
 *
 * Claude Code-inspired coding agent mob running entirely in the browser.
 * Left: orchestrator stream. Right: three sub-agent panels (planner/coder/reviewer).
 * All agents communicate via comms (send/peers). No custom delegation tools.
 */

import { WebCMHost } from "./webcm-host";
import { StreamRenderer } from "./stream";
import { MobOrchestrator, resolveModels, type MobRuntime } from "./mob";
import { registerWebCMTools, type ToolRuntime } from "./tools";

// ── DOM refs ────────────────────────────────────────────────────────────────

// Compile-time env injection (Vite `define`)
declare const __ANTHROPIC_API_KEY__: string;
declare const __OPENAI_API_KEY__: string;
declare const __GEMINI_API_KEY__: string;

const setupOverlay = document.getElementById("setup-overlay") as HTMLDivElement;
const anthropicKeyInput = document.getElementById("anthropic-key") as HTMLInputElement;
const openaiKeyInput = document.getElementById("openai-key") as HTMLInputElement;
const geminiKeyInput = document.getElementById("gemini-key") as HTMLInputElement;

// Pre-fill from environment variables (baked in at build/dev time)
if (__ANTHROPIC_API_KEY__) anthropicKeyInput.value = __ANTHROPIC_API_KEY__;
if (__OPENAI_API_KEY__) openaiKeyInput.value = __OPENAI_API_KEY__;
if (__GEMINI_API_KEY__) geminiKeyInput.value = __GEMINI_API_KEY__;
const bootBtn = document.getElementById("boot-btn") as HTMLButtonElement;
const bootStatus = document.getElementById("boot-status") as HTMLDivElement;
const workspace = document.getElementById("workspace") as HTMLDivElement;
const terminalEl = document.getElementById("terminal") as HTMLDivElement;
const mainStreamEl = document.getElementById("main-stream") as HTMLDivElement;
const promptInput = document.getElementById("prompt") as HTMLInputElement;
const promptSigil = document.getElementById("prompt-sigil") as HTMLSpanElement;

// Sub-agent panel elements
const panelEls = {
  planner: {
    stream: document.getElementById("stream-planner") as HTMLDivElement,
    status: document.getElementById("status-planner") as HTMLSpanElement,
    model: document.getElementById("model-planner") as HTMLSpanElement,
  },
  coder: {
    stream: document.getElementById("stream-coder") as HTMLDivElement,
    status: document.getElementById("status-coder") as HTMLSpanElement,
    model: document.getElementById("model-coder") as HTMLSpanElement,
  },
  reviewer: {
    stream: document.getElementById("stream-reviewer") as HTMLDivElement,
    status: document.getElementById("status-reviewer") as HTMLSpanElement,
    model: document.getElementById("model-reviewer") as HTMLSpanElement,
  },
};

// ── State ───────────────────────────────────────────────────────────────────

const vm = new WebCMHost();
let mob: MobOrchestrator | null = null;
let mainStream: StreamRenderer | null = null;
let running = false;

// ── Boot ────────────────────────────────────────────────────────────────────

bootBtn.addEventListener("click", boot);

async function boot() {
  const keys = {
    anthropic: anthropicKeyInput.value.trim() || undefined,
    openai: openaiKeyInput.value.trim() || undefined,
    gemini: geminiKeyInput.value.trim() || undefined,
  };

  if (!keys.anthropic && !keys.openai && !keys.gemini) {
    bootStatus.textContent = "Enter at least one API key";
    return;
  }

  bootBtn.disabled = true;
  const models = resolveModels(keys);

  try {
    // Step 1: Boot WebCM VM
    bootStatus.textContent = "Booting Alpine Linux VM...";
    await vm.boot(terminalEl, (msg) => { bootStatus.textContent = msg; });

    // Step 2: Load WASM runtime
    bootStatus.textContent = "Loading Meerkat WASM runtime...";
    const runtime = await loadWasmRuntime();

    // Step 3: Register WebCM tools (before init_runtime_from_config)
    registerWebCMTools(runtime as ToolRuntime, vm);

    // Step 4: Init mob runtime
    bootStatus.textContent = "Initializing mob runtime...";
    const mobRuntime = runtime as unknown as MobRuntime;

    // Build panel state for sub-agents (orchestrator goes to main stream)
    const panelStates = new Map<string, any>();

    // Orchestrator events → main stream
    mainStream = new StreamRenderer(mainStreamEl);
    const orchestratorStatusEl = document.getElementById("status-orchestrator") as HTMLSpanElement;
    const orchestratorModelEl = document.getElementById("model-orchestrator") as HTMLSpanElement;
    orchestratorModelEl.textContent = models.main;
    panelStates.set("orchestrator", {
      stream: mainStream,
      statusEl: orchestratorStatusEl,
      subHandle: null,
      currentCard: null,
    });

    // Sub-agent panels
    for (const [name, els] of Object.entries(panelEls)) {
      els.model.textContent = models[name as keyof typeof models] ?? "";
      panelStates.set(name, {
        stream: new StreamRenderer(els.stream),
        statusEl: els.status,
        subHandle: null,
        currentCard: null,
      });
    }

    // Step 5: Create mob (orchestrator + planner + coder + reviewer)
    mob = new MobOrchestrator(mobRuntime, panelStates);
    await mob.init(keys, models);

    // Step 6: Show workspace
    setupOverlay.classList.add("hidden");
    workspace.classList.remove("hidden");
    mainStream.appendBanner(models);

    // Step 7: Start event polling for all agents
    mob.startPolling();

    // Step 8: Focus prompt
    promptInput.focus();

  } catch (err: any) {
    const msg = err instanceof Error ? err.message : String(err);
    console.error("Boot failed:", err);
    bootStatus.textContent = `Boot failed: ${msg}`;
    bootBtn.disabled = false;
  }
}

// ── WASM runtime loader ─────────────────────────────────────────────────────

async function loadWasmRuntime(): Promise<any> {
  const url = new URL("/meerkat-pkg/meerkat_web_runtime.js", window.location.href).toString();
  let mod: any;
  try {
    mod = await import(/* @vite-ignore */ url);
  } catch (err: any) {
    throw new Error(
      `Failed to load WASM runtime from ${url}. ` +
      `Run ./examples.sh to build the WASM bundle. Error: ${err.message}`,
    );
  }
  await mod.default();
  return mod;
}

// ── Input handling ──────────────────────────────────────────────────────────

promptInput.addEventListener("keydown", async (e) => {
  if (e.key === "Enter" && !e.shiftKey && !running) {
    e.preventDefault();
    await send();
  }
});

async function send() {
  if (running || !mob) return;
  const text = promptInput.value.trim();
  if (!text) return;

  promptInput.value = "";
  mainStream?.appendUserLine(text);
  setRunning(true);

  try {
    // Inject user message into the orchestrator mob member via mob_send_message
    await mob.sendToOrchestrator(text);
  } catch (err: any) {
    mainStream?.appendError(err.message);
  } finally {
    setRunning(false);
  }
}

function setRunning(v: boolean) {
  running = v;
  promptInput.disabled = v;
  promptSigil.classList.toggle("running", v);
  if (!v) promptInput.focus();
}
