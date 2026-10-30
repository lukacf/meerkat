import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

const root = path.basename(process.cwd()) === "console"
  ? path.resolve(process.cwd(), "..")
  : process.cwd();
const componentsSrc = path.join(root, "packages", "console-components", "src");
const coreActivitySource = path.join(root, "packages", "console-core", "src", "activity.ts");
const activityStyles = path.join(componentsSrc, "styles", "activity.css");
const componentIndex = path.join(componentsSrc, "index.ts");
const coreIndex = path.join(root, "packages", "console-core", "src", "index.ts");
const stockConsoleAppSource = path.join(root, "console", "src", "ConsoleApp.tsx");

const forbiddenImportPatterns = [
  /from\s+["'][^"']*console\/src\//,
  /from\s+["'][^"']*ConsoleApp/,
  /from\s+["'][^"']*headless/,
  /from\s+["'][^"']*network/,
  /from\s+["']electron["']/,
  /createMetaConsoleController/,
  /useMetaConsoleController/,
];

function sourceFiles(dir: string): string[] {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  return entries.flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(fullPath);
    return /\.(ts|tsx)$/.test(entry.name) ? [fullPath] : [];
  });
}

test("console-components stay model/callback renderers without app, network, or headless imports", () => {
  const violations: string[] = [];

  for (const filePath of sourceFiles(componentsSrc)) {
    if (filePath.endsWith(".test.ts") || filePath.endsWith(".test.tsx")) {
      continue;
    }
    const source = fs.readFileSync(filePath, "utf8");
    for (const pattern of forbiddenImportPatterns) {
      if (pattern.test(source)) {
        violations.push(`${path.relative(root, filePath)} matched ${pattern}`);
      }
    }
  }

  assert.deepEqual(violations, []);
});

test("console-components root barrel exposes reusable composition surfaces", () => {
  const source = fs.readFileSync(componentIndex, "utf8");
  for (const symbol of [
    "ConsoleActivityRail",
    "ConsoleComposer",
    "ConsoleConversationPanel",
    "ConsoleDock",
    "ConsolePendingStack",
    "ConsoleSidebar",
    "ConsoleWorkbench",
    "ConversationPane",
    "ConversationTranscript",
    "TopologyPanel",
    "useConsoleDockController",
  ]) {
    assert.match(source, new RegExp(`\\b${symbol}\\b`));
  }
});

test("console-core root barrel exposes reusable controller and adapter surfaces", () => {
  const source = fs.readFileSync(coreIndex, "utf8");
  for (const symbol of [
    "CONSOLE_RPC_METHODS",
    "createMetaConsoleController",
    "createHttpConsoleTransport",
    "mapFramesToTimelineEntries",
    "buildConversationViewState",
    "buildSidebarViewState",
    "ConsoleFrame",
    "ConsoleAgent",
  ]) {
    assert.match(source, new RegExp(`\\b${symbol}\\b`));
  }
});

test("activity rail roster actions are declared in the shared core model", () => {
  const source = fs.readFileSync(coreActivitySource, "utf8");
  const rosterPanel = source.match(/export interface ConsoleActivityRosterPanel \{(?<body>[\s\S]*?)\n\}/)?.groups?.body || "";

  assert.match(rosterPanel, /\bactions\?:\s*ConsoleActivityAction\[\]/);
});

test("activity feed previews remove the entire transcript action row", () => {
  const source = fs.readFileSync(activityStyles, "utf8");

  assert.match(
    source,
    /\.cc-activity-rail__feed-canvas\s+\.cc-message-group__actions\s*\{\s*display:\s*none;\s*\}/,
  );
  assert.doesNotMatch(
    source,
    /\.cc-activity-rail__feed-canvas\s+\.cc-message-group__copy\s*\{\s*display:\s*none;\s*\}/,
  );
});

/** Every `color-mix(...)` in a stylesheet, as its interpolation space and colour arguments. */
function colorMixes(css: string): { space: string; colors: string[]; text: string }[] {
  const source = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const mixes: { space: string; colors: string[]; text: string }[] = [];
  let from = source.indexOf("color-mix(");
  while (from !== -1) {
    const open = from + "color-mix".length;
    let depth = 0;
    let end = open;
    for (; end < source.length; end += 1) {
      if (source[end] === "(") depth += 1;
      else if (source[end] === ")" && (depth -= 1) === 0) break;
    }
    const inner = source.slice(open + 1, end);
    const parts: string[] = [];
    let level = 0;
    let start = 0;
    for (let index = 0; index < inner.length; index += 1) {
      if (inner[index] === "(") level += 1;
      else if (inner[index] === ")") level -= 1;
      else if (inner[index] === "," && level === 0) {
        parts.push(inner.slice(start, index).trim());
        start = index + 1;
      }
    }
    parts.push(inner.slice(start).trim());
    const [space = "", ...colors] = parts;
    mixes.push({ space: space.replace(/^in\s+/, ""), colors: colors.map((color) => color.replace(/\s+\d+(\.\d+)?%$/, "")), text: source.slice(from, end + 1) });
    from = source.indexOf("color-mix(", end);
  }
  return mixes;
}

test("kit colour mixes against an opaque colour interpolate in srgb", () => {
  // The default variant's surfaces and foreground carry an explicit oklch
  // hue, so an oklch mix with them pulls every tone toward that hue: toned
  // chips all turned one pink (light) or brown (dark). A mix with
  // transparent has no hue of its own and may stay in oklch.
  const mixes = [
    ...colorMixes(fs.readFileSync(path.join(componentsSrc, "styles", "panel-kit.css"), "utf8")),
    ...colorMixes(fs.readFileSync(path.join(componentsSrc, "styles", "tokens.css"), "utf8")),
  ];
  const opaque = mixes.filter((mix) => !mix.colors.includes("transparent"));
  assert.ok(
    opaque.some((mix) => mix.colors.join(" ") === "var(--_cc-tone) var(--_cc-surface-card)"),
    "the toned chip fill is an opaque mix with the card",
  );
  assert.ok(opaque.length >= 8, `expected the chip fill, the inks and the accent mixes, found ${opaque.length}`);
  assert.deepEqual(opaque.filter((mix) => mix.space !== "srgb").map((mix) => mix.text), []);
});

test("stock console runtime path is backed by the headless controller", () => {
  const source = fs.readFileSync(stockConsoleAppSource, "utf8");

  assert.match(source, /\bcreateHttpConsoleTransport\b/);
  assert.match(source, /\bcreateMetaConsoleController\b/);
  assert.match(source, /\bconsoleController\.timeline\.query\b/);
  assert.match(source, /\bconsoleController\.timeline\.subscribeWithBackfill\b/);
  assert.match(source, /\bdispatchController\.commands\.sendMessage\b/);
  assert.match(source, /dispatchController\s*=\s*sendControllerRef\.current/);
  assert.match(source, /\bconsoleController\.commands\.execute\b/);
  assert.match(source, /TopologyPanel[\s\S]*from\s+["']@console-components["']/);
  assert.doesNotMatch(source, /from\s+["']\.\/panels\/TopologyPanel["']/);
  // A transport lifetime decorator may forward source.queryTimeline; the
  // application still queries through the controller, never a raw helper.
  assert.doesNotMatch(source, /(?<![\w.])queryTimeline\s*\(/);
  assert.doesNotMatch(source, /import\s*\{[^}]*\bqueryTimeline\b[^}]*\}\s*from/s);
  for (const rawHelper of [
    "fetchJson",
    "sendConsole",
    "sendConsoleMultipart",
    "subscribeTimelineEvents",
    "callConsoleRpc",
  ]) {
    assert.doesNotMatch(source, new RegExp(`\\b${rawHelper}\\b`));
  }
});
