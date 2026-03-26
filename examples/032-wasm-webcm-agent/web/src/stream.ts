/**
 * Stream renderer — shared by the main agent column and specialist panels.
 *
 * Renders agent events as TUI-style lines: user prompts, markdown text,
 * collapsible tool cards, status indicators, and errors.
 */

import { marked, type Renderer } from "marked";
import hljs from "highlight.js";

// Custom renderer for code highlighting (marked v15+ API)
const renderer: Partial<Renderer> = {
  code({ text, lang }: { text: string; lang?: string }) {
    const language = lang && hljs.getLanguage(lang) ? lang : undefined;
    const highlighted = language
      ? hljs.highlight(text, { language }).value
      : hljs.highlightAuto(text).value;
    return `<pre><code class="hljs${language ? ` language-${language}` : ""}">${highlighted}</code></pre>`;
  },
};

marked.use({ renderer });

export class StreamRenderer {
  private container: HTMLElement;
  private statusEl: HTMLElement | null = null;
  private pendingTextEl: HTMLElement | null = null;

  constructor(container: HTMLElement) {
    this.container = container;
  }

  /** Render a user prompt line: ❯ text */
  appendUserLine(text: string): void {
    const el = document.createElement("div");
    el.className = "stream-entry stream-user";
    el.textContent = text;
    this.append(el);
  }

  /** Render agent text with light markdown (used internally by finalizeText). */
  private appendText(markdown: string): void {
    const el = document.createElement("div");
    el.className = "stream-entry stream-text";
    el.innerHTML = marked.parse(markdown) as string;
    this.append(el);
  }

  /** Append a streaming text delta (raw text, no markdown yet). */
  appendTextDelta(delta: string): void {
    this.clearStatus();
    if (!this.pendingTextEl) {
      this.pendingTextEl = document.createElement("div");
      this.pendingTextEl.className = "stream-entry stream-text";
      this.append(this.pendingTextEl);
    }
    this.pendingTextEl.textContent += delta;
    this.scrollBottom();
  }

  /** Finalize streaming text: replace raw deltas with rendered markdown. */
  finalizeText(markdown: string): void {
    if (this.pendingTextEl) {
      this.pendingTextEl.innerHTML = marked.parse(markdown) as string;
      this.pendingTextEl = null;
    } else {
      this.appendText(markdown);
    }
  }

  /** Begin a tool call card. Returns the card element for later resolution. */
  beginToolCall(toolName: string, args: any): HTMLElement {
    // Finalize any pending streaming text
    if (this.pendingTextEl) {
      this.pendingTextEl = null;
    }
    this.clearStatus();

    const card = document.createElement("div");
    card.className = "stream-entry stream-tool";

    const header = document.createElement("div");
    header.className = "stream-tool-header";

    const name = document.createElement("span");
    name.className = "stream-tool-name";
    name.textContent = toolName;

    const argsSummary = document.createElement("span");
    argsSummary.className = "stream-tool-args";
    argsSummary.textContent = this.summarizeArgs(toolName, args);

    const spinner = document.createElement("span");
    spinner.className = "stream-tool-spinner";

    header.appendChild(name);
    header.appendChild(argsSummary);
    header.appendChild(spinner);

    const body = document.createElement("div");
    body.className = "stream-tool-body";

    card.appendChild(header);
    card.appendChild(body);

    // Toggle expand on header click
    header.addEventListener("click", () => {
      card.classList.toggle("expanded");
    });

    this.append(card);
    return card;
  }

  /** Resolve a pending tool call with output. */
  resolveToolCall(card: HTMLElement, output: string, isError: boolean): void {
    const spinner = card.querySelector(".stream-tool-spinner");
    if (spinner) spinner.remove();

    const body = card.querySelector(".stream-tool-body") as HTMLElement;
    if (body) {
      body.textContent = output;
      if (isError) body.classList.add("is-error");
    }

    // Auto-expand on error
    if (isError) card.classList.add("expanded");
  }

  /** Show or update a status line (replaces previous status in-place). */
  setStatus(text: string): void {
    if (!this.statusEl) {
      this.statusEl = document.createElement("div");
      this.statusEl.className = "stream-entry stream-status";
      this.container.appendChild(this.statusEl);
    }
    this.statusEl.textContent = text;
    this.scrollBottom();
  }

  /** Remove the current status line. */
  clearStatus(): void {
    if (this.statusEl) {
      this.statusEl.remove();
      this.statusEl = null;
    }
  }

  /** Render an error line. */
  appendError(text: string): void {
    this.clearStatus();
    const el = document.createElement("div");
    el.className = "stream-entry stream-error";
    el.textContent = text;
    this.append(el);
  }

  /** Clear all content. */
  clear(): void {
    this.container.innerHTML = "";
    this.statusEl = null;
  }

  /** Inject the ASCII banner and intro text (main stream only). */
  appendBanner(models: { main: string; planner: string; coder: string; reviewer: string }): void {
    const banner = document.createElement("pre");
    banner.className = "stream-banner";
    banner.textContent = [
      `  .~.  .~.  .~.       MEERKAT`,
      ` (o.o)(o.o)(o.o)      ───────`,
      `  /|\\  /|\\  /|\\       Agent mob running`,
      `  | |  | |  | |       in your browser`,
      ` _/ \\__/ \\__/ \\_`,
    ].join("\n");

    const intro = document.createElement("div");
    intro.className = "stream-intro";
    intro.innerHTML =
      `Collaborative multi-agent system (Meerkat Mob) entirely in your browser — no backend, no server. ` +
      `The Alpha Meerkat (<span class="model-tag">${models.main}</span>) coordinates a ` +
      `planner (<span class="model-tag">${models.planner}</span>), ` +
      `coder (<span class="model-tag">${models.coder}</span>), and ` +
      `reviewer (<span class="model-tag">${models.reviewer}</span>) ` +
      `with access to a sandboxed Alpine Linux VM.<br><br>` +
      `Try: <em>"Write an ASCII meerkat generator in micropython with randomized poses"</em> ` +
      `or <em>"Build a markdown-to-HTML converter with tests in Lua"</em>`;

    this.container.appendChild(banner);
    this.container.appendChild(intro);
  }

  private append(el: HTMLElement): void {
    // Insert before the status line if it exists, otherwise append
    if (this.statusEl && this.statusEl.parentNode === this.container) {
      this.container.insertBefore(el, this.statusEl);
    } else {
      this.container.appendChild(el);
    }
    this.scrollBottom();
  }

  private scrollBottom(): void {
    this.container.scrollTop = this.container.scrollHeight;
  }

  private summarizeArgs(toolName: string, args: any): string {
    if (!args) return "";
    if (typeof args === "string") {
      try { args = JSON.parse(args); } catch { return args.slice(0, 80); }
    }
    if (toolName === "shell" && args.command) return args.command;
    if (toolName === "write_file" && args.path) return args.path;
    if (toolName === "read_file" && args.path) return args.path;
    if (args.task) return args.task.slice(0, 80);
    return JSON.stringify(args).slice(0, 80);
  }
}
