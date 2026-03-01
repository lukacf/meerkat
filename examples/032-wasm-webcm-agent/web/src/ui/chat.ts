/**
 * Chat panel — markdown rendering, streaming text, tool call cards.
 */

import { marked, type MarkedExtension } from "marked";
import hljs from "highlight.js";

// Configure marked with a custom renderer for syntax highlighting
const renderer: MarkedExtension = {
  renderer: {
    code({ text, lang }: { text: string; lang?: string }) {
      const language = lang && hljs.getLanguage(lang) ? lang : undefined;
      const highlighted = language
        ? hljs.highlight(text, { language }).value
        : hljs.highlightAuto(text).value;
      return `<pre><code class="hljs${language ? ` language-${language}` : ""}">${highlighted}</code></pre>`;
    },
  },
};
marked.use(renderer);

export type ChatEventType = "text" | "tool_call" | "tool_result" | "error" | "done";

export interface ChatEvent {
  type: ChatEventType;
  content: string;
  toolName?: string;
}

export class Chat {
  private container: HTMLElement;
  private currentToolCard: HTMLElement | null = null;

  constructor(container: HTMLElement) {
    this.container = container;
  }

  addUserMessage(text: string): void {
    const msg = this.createMsg("user");
    const label = this.createLabel("You");
    const body = document.createElement("div");
    body.className = "msg-body";
    body.textContent = text;
    msg.appendChild(label);
    msg.appendChild(body);
    this.container.appendChild(msg);
    this.scrollBottom();
  }

  addAssistantText(text: string): void {
    const msg = this.createMsg("assistant");
    const label = this.createLabel("Assistant");
    const body = document.createElement("div");
    body.className = "msg-body";
    body.innerHTML = marked.parse(text) as string;
    msg.appendChild(label);
    msg.appendChild(body);
    this.container.appendChild(msg);
    this.scrollBottom();
  }

  addToolCall(toolName: string, content: string): HTMLElement {
    const card = document.createElement("div");
    card.className = "tool-card";

    const header = document.createElement("div");
    header.className = "tool-card-header";
    header.innerHTML = `<span class="tool-card-chevron">&#9654;</span> <span class="tool-card-name">${this.esc(toolName)}</span> <span class="tool-spinner"></span>`;
    header.addEventListener("click", () => card.classList.toggle("expanded"));

    const body = document.createElement("div");
    body.className = "tool-card-body";
    body.textContent = content.length > 500 ? content.slice(0, 500) + "..." : content;

    card.appendChild(header);
    card.appendChild(body);
    this.container.appendChild(card);
    this.currentToolCard = card;
    this.scrollBottom();
    return card;
  }

  resolveToolCard(card: HTMLElement, result: string, isError: boolean): void {
    // Remove spinner
    const spinner = card.querySelector(".tool-spinner");
    if (spinner) spinner.remove();

    // Add result to body
    const body = card.querySelector(".tool-card-body") as HTMLElement;
    if (body) {
      const resultEl = document.createElement("div");
      resultEl.className = isError ? "tool-error" : "tool-result";
      resultEl.textContent = result.length > 1000 ? result.slice(0, 1000) + "\n...(truncated)" : result;
      body.appendChild(document.createElement("hr"));
      body.appendChild(resultEl);
    }

    // Auto-expand on error
    if (isError) card.classList.add("expanded");
    this.scrollBottom();
  }

  addError(text: string): void {
    const msg = this.createMsg("error");
    const body = document.createElement("div");
    body.className = "msg-body";
    body.textContent = text;
    msg.appendChild(body);
    this.container.appendChild(msg);
    this.scrollBottom();
  }

  clear(): void {
    this.container.innerHTML = "";
  }

  private createMsg(type: string): HTMLElement {
    const el = document.createElement("div");
    el.className = `msg ${type}`;
    return el;
  }

  private createLabel(text: string): HTMLElement {
    const el = document.createElement("div");
    el.className = "msg-label";
    el.textContent = text;
    return el;
  }

  private scrollBottom(): void {
    this.container.scrollTop = this.container.scrollHeight;
  }

  private esc(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }
}
