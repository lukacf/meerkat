import assert from "node:assert/strict";
import test from "node:test";

import { findPaneResizeRoot } from "./pane-resize";

class TestElement {
  className = "";
  parent: TestElement | null = null;
  private attrs = new Map<string, string>();

  constructor(options: { className?: string; attrs?: Record<string, string>; parent?: TestElement | null } = {}) {
    this.className = options.className || "";
    this.parent = options.parent || null;
    for (const [key, value] of Object.entries(options.attrs || {})) {
      this.attrs.set(key, value);
    }
  }

  closest(selector: string): TestElement | null {
    let current: TestElement | null = this;
    while (current) {
      if (selector === ".shell" && current.className.split(/\s+/).includes("shell")) return current;
      if (selector === "[data-console-workbench]" && current.attrs.has("data-console-workbench")) return current;
      current = current.parent;
    }
    return null;
  }

  getAttribute(name: string): string | null {
    return this.attrs.get(name) ?? null;
  }
}

globalThis.HTMLElement = TestElement as unknown as typeof HTMLElement;

test("pane resize root resolves the console shell grid", () => {
  const shell = new TestElement({ className: "shell" });
  const handle = new TestElement({ parent: shell });

  assert.equal(findPaneResizeRoot(handle as unknown as HTMLElement)?.className, "shell");
});

test("pane resize root prefers explicit workbench roots", () => {
  const shell = new TestElement({ className: "shell" });
  const workbench = new TestElement({ attrs: { "data-console-workbench": "root" }, parent: shell });
  const handle = new TestElement({ parent: workbench });

  assert.equal(
    findPaneResizeRoot(handle as unknown as HTMLElement)?.getAttribute("data-console-workbench"),
    "root",
  );
});
