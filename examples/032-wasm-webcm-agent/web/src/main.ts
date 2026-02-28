/**
 * 032 — Meerkat WebCM Agent
 *
 * A coding agent that runs entirely in the browser:
 * - LLM calls go directly to Anthropic's API (browser fetch)
 * - Shell commands execute in a RISC-V Linux VM (WebCM via WASM)
 * - No server, no proxy, just a browser tab
 */

import { WebCMHost } from "./webcm-host";
import { Agent, type AgentEvent } from "./agent";

// ── DOM refs ────────────────────────────────────────────────────────────────

const apiKeyInput = document.getElementById("api-key") as HTMLInputElement;
const bootBtn = document.getElementById("boot-btn") as HTMLButtonElement;
const status = document.getElementById("status") as HTMLSpanElement;
const setup = document.getElementById("setup") as HTMLDivElement;
const workspace = document.getElementById("workspace") as HTMLDivElement;
const terminalEl = document.getElementById("terminal") as HTMLDivElement;
const messages = document.getElementById("messages") as HTMLDivElement;
const promptInput = document.getElementById("prompt") as HTMLInputElement;
const sendBtn = document.getElementById("send-btn") as HTMLButtonElement;

// ── State ───────────────────────────────────────────────────────────────────

const vm = new WebCMHost();
let agent: Agent | null = null;
let running = false;

// ── UI helpers ──────────────────────────────────────────────────────────────

function addMessage(type: string, label: string, content: string): HTMLDivElement {
  const el = document.createElement("div");
  el.className = `msg ${type}`;
  el.innerHTML = `<span class="label">${label}</span>${escapeHtml(content)}`;
  messages.appendChild(el);
  messages.scrollTop = messages.scrollHeight;
  return el;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function setRunning(v: boolean) {
  running = v;
  sendBtn.disabled = v;
  promptInput.disabled = v;
  if (v) {
    status.textContent = "Agent working...";
    status.className = "spinner";
  } else {
    status.textContent = "Ready";
    status.className = "";
  }
}

// ── Boot ────────────────────────────────────────────────────────────────────

bootBtn.addEventListener("click", async () => {
  const key = apiKeyInput.value.trim();
  if (!key) {
    status.textContent = "Enter an API key";
    return;
  }

  bootBtn.disabled = true;

  try {
    await vm.boot(terminalEl, (msg) => {
      status.textContent = msg;
    });

    // Debug: expose VM on window for console inspection
    (window as any).__vm = vm;
    agent = new Agent(key, vm, handleAgentEvent);

    setup.classList.add("hidden");
    workspace.classList.remove("hidden");
    vm.fit();
    status.textContent = "VM ready";
    promptInput.focus();
  } catch (err: any) {
    status.textContent = `Boot failed: ${err.message}`;
    bootBtn.disabled = false;
  }
});

// ── Resize ──────────────────────────────────────────────────────────────────

window.addEventListener("resize", () => vm.fit());

// ── Agent event handler ─────────────────────────────────────────────────────

let currentAssistantEl: HTMLDivElement | null = null;

function handleAgentEvent(e: AgentEvent) {
  switch (e.type) {
    case "text":
      currentAssistantEl = addMessage("assistant", "Assistant", e.content);
      break;
    case "tool_call":
      addMessage("tool", `Tool: ${e.toolName}`, e.content);
      break;
    case "tool_result":
      addMessage("tool", `Result: ${e.toolName}`, e.content);
      break;
    case "error":
      addMessage("error", "Error", e.content);
      break;
    case "done":
      break;
  }
}

// ── Send ────────────────────────────────────────────────────────────────────

async function send() {
  if (running || !agent) return;
  const text = promptInput.value.trim();
  if (!text) return;

  promptInput.value = "";
  addMessage("user", "You", text);
  currentAssistantEl = null;

  setRunning(true);
  try {
    await agent.run(text);
  } catch (err: any) {
    addMessage("error", "Error", err.message);
  }
  setRunning(false);
}

sendBtn.addEventListener("click", send);
promptInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    send();
  }
});
