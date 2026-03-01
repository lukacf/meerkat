/**
 * 032 — Meerkat WebCM Agent
 *
 * Claude Code-inspired coding agent running entirely in the browser.
 * Split-pane layout: file tree + Monaco editor + chat + terminal.
 */

import { WebCMHost } from "./webcm-host";
import { Agent, type AgentEvent } from "./agent";
import { MobOrchestrator, type MobEvent } from "./mob";
import { FileTree } from "./ui/file-tree";
import { Editor } from "./ui/editor";
import { Chat } from "./ui/chat";

// ── DOM refs ────────────────────────────────────────────────────────────────

const setupOverlay = document.getElementById("setup-overlay") as HTMLDivElement;
const apiKeyInput = document.getElementById("api-key") as HTMLInputElement;
const modelSelect = document.getElementById("model-select") as HTMLSelectElement;
const bootBtn = document.getElementById("boot-btn") as HTMLButtonElement;
const bootStatus = document.getElementById("boot-status") as HTMLDivElement;
const workspace = document.getElementById("workspace") as HTMLDivElement;
const terminalEl = document.getElementById("terminal") as HTMLDivElement;
const terminalPane = document.getElementById("terminal-pane") as HTMLDivElement;
const terminalToggle = document.getElementById("terminal-toggle") as HTMLDivElement;
const fileTreeEl = document.getElementById("file-tree") as HTMLDivElement;
const editorTabsEl = document.getElementById("editor-tabs") as HTMLDivElement;
const editorContainerEl = document.getElementById("editor-container") as HTMLDivElement;
const editorEmptyEl = document.getElementById("editor-empty") as HTMLDivElement;
const messagesEl = document.getElementById("messages") as HTMLDivElement;
const promptInput = document.getElementById("prompt") as HTMLTextAreaElement;
const sendBtn = document.getElementById("send-btn") as HTMLButtonElement;
const sessionInfo = document.getElementById("session-info") as HTMLSpanElement;
const modelSwitch = document.getElementById("model-switch") as HTMLSelectElement;
const newSessionBtn = document.getElementById("new-session-btn") as HTMLButtonElement;

// ── State ───────────────────────────────────────────────────────────────────

const vm = new WebCMHost();
let agent: Agent | null = null;
let mob: MobOrchestrator | null = null;
let running = false;
let chat: Chat;
let editor: Editor;
let fileTree: FileTree;
let turnCount = 0;
let selectedModel = "claude-sonnet-4-5";
let isMobMode = false;

// ── UI Components ───────────────────────────────────────────────────────────

chat = new Chat(messagesEl);

// ── Boot ────────────────────────────────────────────────────────────────────

bootBtn.addEventListener("click", async () => {
  const key = apiKeyInput.value.trim();
  if (!key) {
    bootStatus.textContent = "Enter an API key";
    return;
  }
  selectedModel = modelSelect.value;
  isMobMode = (document.querySelector('input[name="mode"]:checked') as HTMLInputElement)?.value === "mob";
  bootBtn.disabled = true;

  try {
    await vm.boot(terminalEl, (msg) => {
      bootStatus.textContent = msg;
    });

    // Init UI components that need the VM
    editor = new Editor(editorContainerEl, editorTabsEl, editorEmptyEl);
    fileTree = new FileTree(fileTreeEl, vm, {
      onFileSelect: async (path) => {
        try {
          const content = await vm.readFile(path);
          editor.openFile(path, content);
          fileTree.setActive(path);
        } catch (e: any) {
          chat.addError(`Failed to open ${path}: ${e.message}`);
        }
      },
    });

    // Debug: expose VM for console testing
    (window as any).__vm = vm;

    if (isMobMode) {
      // Mob mode: init meerkat WASM runtime + create mob
      mob = new MobOrchestrator(vm, handleMobEvent);
      await mob.init(key, selectedModel);
      await mob.createMob(selectedModel);
      sessionInfo.textContent = `Mob mode · ${selectedModel}`;
    } else {
      // Solo mode: direct JS agent loop
      agent = new Agent(key, vm, handleAgentEvent, selectedModel);
      sessionInfo.textContent = `Model: ${selectedModel}`;
    }

    // Transition to workspace
    setupOverlay.classList.add("hidden");
    workspace.classList.remove("hidden");
    vm.fit();
    modelSwitch.value = selectedModel;
    promptInput.focus();

    // Initial file tree scan
    await fileTree.refresh();
  } catch (err: any) {
    bootStatus.textContent = `Boot failed: ${err.message}`;
    bootBtn.disabled = false;
  }
});

// ── Terminal toggle ─────────────────────────────────────────────────────────

let terminalCollapsed = false;
terminalToggle.addEventListener("click", () => {
  terminalCollapsed = !terminalCollapsed;
  terminalPane.classList.toggle("collapsed", terminalCollapsed);
  document.getElementById("terminal-chevron")!.style.transform = terminalCollapsed ? "rotate(180deg)" : "";
  setTimeout(() => vm.fit(), 50);
});

// ── Resize ──────────────────────────────────────────────────────────────────

window.addEventListener("resize", () => vm.fit());

// ── Agent events ────────────────────────────────────────────────────────────

let currentToolCard: HTMLElement | null = null;

// ── Mob event handler ───────────────────────────────────────────────────────

function handleMobEvent(e: MobEvent) {
  // Add agent label before content
  const agentLabel = document.createElement("span");
  agentLabel.className = `mob-agent-label ${e.agent}`;
  agentLabel.textContent = e.agent;

  switch (e.type) {
    case "status":
      chat.addAssistantText(`**[${e.agent}]** ${e.content}`);
      break;
    case "text":
      chat.addAssistantText(`**[${e.agent}]** ${e.content}`);
      break;
    case "tool_call":
      currentToolCard = chat.addToolCall(
        `${e.agent}/${e.toolName || "tool"}`,
        e.content.slice(0, 300),
      );
      break;
    case "tool_result":
      if (currentToolCard) {
        chat.resolveToolCard(currentToolCard, e.content, false);
        currentToolCard = null;
      }
      fileTree?.refresh();
      break;
  }
}

// ── Agent event handler ─────────────────────────────────────────────────────

function handleAgentEvent(e: AgentEvent) {
  switch (e.type) {
    case "text":
      chat.addAssistantText(e.content);
      break;
    case "tool_call":
      currentToolCard = chat.addToolCall(
        e.toolName || "tool",
        e.toolName === "shell" ? e.content : e.content.slice(0, 200),
      );
      break;
    case "tool_result":
      if (currentToolCard) {
        chat.resolveToolCard(currentToolCard, e.content, false);
        currentToolCard = null;
      }
      // Auto-refresh file tree after write operations
      if (e.toolName === "write_file" || e.toolName === "shell") {
        fileTree?.refresh();
        // If a file was written, try to open it in editor
        if (e.toolName === "write_file" && e.content.startsWith("Wrote ")) {
          const path = e.content.slice(6).trim();
          vm.readFile(path)
            .then((content) => {
              editor?.openFile(path, content);
              fileTree?.setActive(path);
            })
            .catch(() => {});
        }
      }
      break;
    case "error":
      if (currentToolCard) {
        chat.resolveToolCard(currentToolCard, e.content, true);
        currentToolCard = null;
      } else {
        chat.addError(e.content);
      }
      break;
    case "done":
      turnCount++;
      sessionInfo.textContent = `Model: ${selectedModel} · Turn ${turnCount}`;
      break;
  }
}

// ── Send ────────────────────────────────────────────────────────────────────

async function send() {
  if (running) return;
  const text = promptInput.value.trim();
  if (!text) return;

  promptInput.value = "";
  promptInput.style.height = "36px";
  chat.addUserMessage(text);
  currentToolCard = null;

  setRunning(true);
  try {
    if (isMobMode && mob) {
      // Mob mode: run the implement flow
      await mob.runFlow(text);
      // Refresh file tree after flow
      await fileTree?.refresh();
    } else if (agent) {
      await agent.run(text);
    }
  } catch (err: any) {
    chat.addError(err.message);
  }
  setRunning(false);
}

function setRunning(v: boolean) {
  running = v;
  sendBtn.disabled = v;
  promptInput.disabled = v;
}

sendBtn.addEventListener("click", send);

// Cmd+Enter or Ctrl+Enter to send
promptInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
    e.preventDefault();
    send();
  }
});

// Auto-resize textarea
promptInput.addEventListener("input", () => {
  promptInput.style.height = "36px";
  promptInput.style.height = Math.min(promptInput.scrollHeight, 120) + "px";
});

// ── Model switch ────────────────────────────────────────────────────────────

modelSwitch.addEventListener("change", () => {
  selectedModel = modelSwitch.value;
  if (agent) {
    agent = new Agent(apiKeyInput.value.trim(), vm, handleAgentEvent, selectedModel);
    turnCount = 0;
    sessionInfo.textContent = `Model: ${selectedModel} (new session)`;
  }
});

// ── New session ─────────────────────────────────────────────────────────────

newSessionBtn.addEventListener("click", () => {
  if (agent) {
    agent = new Agent(apiKeyInput.value.trim(), vm, handleAgentEvent, selectedModel);
    chat.clear();
    turnCount = 0;
    sessionInfo.textContent = `Model: ${selectedModel}`;
    promptInput.focus();
  }
});

// ── Resize handles ──────────────────────────────────────────────────────────

document.querySelectorAll(".resize-handle").forEach((handle) => {
  let startX = 0;
  let startWidth = 0;
  let target: HTMLElement | null = null;

  const onMouseMove = (e: MouseEvent) => {
    if (!target) return;
    const dx = e.clientX - startX;
    target.style.width = Math.max(120, startWidth + dx) + "px";
  };

  const onMouseUp = () => {
    handle.classList.remove("active");
    document.removeEventListener("mousemove", onMouseMove as any);
    document.removeEventListener("mouseup", onMouseUp);
  };

  handle.addEventListener("mousedown", (e: Event) => {
    const me = e as MouseEvent;
    handle.classList.add("active");
    startX = me.clientX;
    const resizeType = (handle as HTMLElement).getAttribute("data-resize");
    if (resizeType === "tree-editor") {
      target = document.getElementById("file-tree-pane");
    } else if (resizeType === "editor-chat") {
      target = document.getElementById("chat-pane");
      // For chat, resize is from the left edge, so invert
      startWidth = target?.offsetWidth || 400;
      const origStartX = startX;
      const origOnMouseMove = (ev: MouseEvent) => {
        if (!target) return;
        const dx = origStartX - ev.clientX;
        target.style.width = Math.max(250, startWidth + dx) + "px";
      };
      document.addEventListener("mousemove", origOnMouseMove as any);
      document.addEventListener("mouseup", () => {
        handle.classList.remove("active");
        document.removeEventListener("mousemove", origOnMouseMove as any);
      }, { once: true });
      return;
    }
    if (target) startWidth = target.offsetWidth;
    document.addEventListener("mousemove", onMouseMove as any);
    document.addEventListener("mouseup", onMouseUp);
  });
});
