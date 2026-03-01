/**
 * Monaco editor with tabs for viewing files from the VM.
 */

import * as monaco from "monaco-editor";

// Configure Monaco workers for Vite.
// Language-specific workers (typescript, json, css, html) are optional.
// We only load the base editor worker to avoid module resolution errors.
self.MonacoEnvironment = {
  getWorker() {
    return new Worker(
      new URL("monaco-editor/esm/vs/editor/editor.worker.js", import.meta.url),
      { type: "module" },
    );
  },
};

interface Tab {
  path: string;
  model: monaco.editor.ITextModel;
}

export class Editor {
  private editor: monaco.editor.IStandaloneCodeEditor;
  private tabsContainer: HTMLElement;
  private emptyEl: HTMLElement;
  private tabs: Tab[] = [];
  private activeTab: Tab | null = null;

  constructor(container: HTMLElement, tabsContainer: HTMLElement, emptyEl: HTMLElement) {
    this.tabsContainer = tabsContainer;
    this.emptyEl = emptyEl;

    this.editor = monaco.editor.create(container, {
      theme: "vs-dark",
      fontSize: 13,
      fontFamily: '"Berkeley Mono", "SF Mono", "Cascadia Code", monospace',
      minimap: { enabled: false },
      readOnly: true,
      scrollBeyondLastLine: false,
      automaticLayout: true,
      lineNumbers: "on",
      renderLineHighlight: "line",
      padding: { top: 8 },
    });

    // Initially hidden (empty state shown)
    container.style.display = "none";
  }

  openFile(path: string, content: string): void {
    // Check if tab already exists
    let tab = this.tabs.find((t) => t.path === path);
    if (tab) {
      // Update content
      tab.model.setValue(content);
    } else {
      // Create new tab
      const lang = this.inferLanguage(path);
      const model = monaco.editor.createModel(content, lang, monaco.Uri.file(path));
      tab = { path, model };
      this.tabs.push(tab);
    }
    this.activateTab(tab);
    this.renderTabs();
  }

  private activateTab(tab: Tab): void {
    this.activeTab = tab;
    this.editor.setModel(tab.model);

    // Show editor, hide empty state
    (this.editor.getDomNode()?.parentElement as HTMLElement).style.display = "";
    this.emptyEl.style.display = "none";
  }

  closeTab(path: string): void {
    const idx = this.tabs.findIndex((t) => t.path === path);
    if (idx < 0) return;

    const tab = this.tabs[idx];
    tab.model.dispose();
    this.tabs.splice(idx, 1);

    if (this.activeTab === tab) {
      if (this.tabs.length > 0) {
        this.activateTab(this.tabs[Math.min(idx, this.tabs.length - 1)]);
      } else {
        this.activeTab = null;
        this.editor.setModel(null);
        (this.editor.getDomNode()?.parentElement as HTMLElement).style.display = "none";
        this.emptyEl.style.display = "";
      }
    }
    this.renderTabs();
  }

  private renderTabs(): void {
    this.tabsContainer.innerHTML = "";
    for (const tab of this.tabs) {
      const el = document.createElement("div");
      el.className = `editor-tab${tab === this.activeTab ? " active" : ""}`;

      const name = document.createElement("span");
      name.textContent = tab.path.split("/").pop() || tab.path;
      name.title = tab.path;
      el.appendChild(name);

      const close = document.createElement("span");
      close.className = "tab-close";
      close.textContent = "\u00d7";
      close.addEventListener("click", (e) => {
        e.stopPropagation();
        this.closeTab(tab.path);
      });
      el.appendChild(close);

      el.addEventListener("click", () => {
        this.activateTab(tab);
        this.renderTabs();
      });

      this.tabsContainer.appendChild(el);
    }
  }

  private inferLanguage(path: string): string {
    const ext = path.split(".").pop()?.toLowerCase();
    const map: Record<string, string> = {
      js: "javascript",
      ts: "typescript",
      py: "python",
      lua: "lua",
      c: "c",
      h: "c",
      rs: "rust",
      sh: "shell",
      bash: "shell",
      json: "json",
      toml: "ini",
      yaml: "yaml",
      yml: "yaml",
      md: "markdown",
      html: "html",
      css: "css",
      rb: "ruby",
      sql: "sql",
      xml: "xml",
    };
    return map[ext || ""] || "plaintext";
  }
}
