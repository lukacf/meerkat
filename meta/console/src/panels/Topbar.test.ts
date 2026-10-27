import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { Topbar } from "./Topbar";

function renderTopbar(mobStatus: string): string {
  return renderToStaticMarkup(
    React.createElement(Topbar, {
      mobName: "ops",
      mobStatus,
      theme: "dark",
      onToggleTheme: () => {},
      sidebarCollapsed: false,
      railCollapsed: false,
      onToggleSidebar: () => {},
      onToggleRail: () => {},
    }),
  );
}

test("the topbar dot carries the mob status it shows", () => {
  for (const status of ["running", "stopped", "unknown"]) {
    const html = renderTopbar(status);
    assert.match(html, new RegExp(`class="meta-topbar__mob-status" data-status="${status}"`), html);
  }
});

test("only a running mob's dot is green", () => {
  // Run from console/ (npm --prefix console).
  const css = readFileSync("src/console-host.css", "utf8");
  const rule = (selector: string) => {
    const start = css.indexOf(`${selector} {`);
    assert.notEqual(start, -1, `${selector} has a rule`);
    return css.slice(start, css.indexOf("}", start));
  };
  assert.match(rule(".meta-topbar__mob-status"), /background: var\(--ok\)/);
  assert.match(rule('.meta-topbar__mob-status[data-status="stopped"]'), /background: var\(--crit\)/);
  assert.match(rule('.meta-topbar__mob-status[data-status="unknown"]'), /background: var\(--warn\)/);
});
