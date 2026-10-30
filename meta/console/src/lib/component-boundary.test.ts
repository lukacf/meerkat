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

/** A painted colour: gamma-encoded sRGB channels in 0..1 and an alpha. */
type Paint = { rgb: [number, number, number]; alpha: number };

function parsePaint(value: string): Paint {
  const text = value.trim();
  const oklch = /^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*\)$/.exec(text);
  if (oklch) {
    const [L, C, H] = oklch.slice(1).map(Number);
    const h = (H * Math.PI) / 180;
    const a = C * Math.cos(h);
    const b = C * Math.sin(h);
    const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
    const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
    const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
    const linear = [
      4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
      -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
      -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ];
    const encode = (x: number) => Math.min(1, Math.max(0, x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055));
    return { rgb: linear.map(encode) as [number, number, number], alpha: 1 };
  }
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(text);
  if (hex) return { rgb: hex.slice(1).map((pair) => parseInt(pair, 16) / 255) as [number, number, number], alpha: 1 };
  const rgba = /^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+)\s*)?\)$/.exec(text);
  if (rgba) {
    return { rgb: rgba.slice(1, 4).map((part) => Number(part) / 255) as [number, number, number], alpha: rgba[4] === undefined ? 1 : Number(rgba[4]) };
  }
  throw new Error(`Unsupported colour in the contrast guard: ${value}`);
}

/** color-mix(in srgb, first p, second): premultiplied interpolation. */
function mixSrgb(first: Paint, p: number, second: Paint): Paint {
  const alpha = p * first.alpha + (1 - p) * second.alpha;
  const rgb = first.rgb.map((channel, index) => (
    (p * first.alpha * channel + (1 - p) * second.alpha * second.rgb[index]) / alpha
  )) as [number, number, number];
  return { rgb, alpha };
}

/** Paint `top` over an opaque `bottom`, as the browser does, in 8-bit channels. */
function paintOver(top: Paint, bottom: Paint): Paint {
  const quantize = (channel: number) => Math.round(channel * 255) / 255;
  return {
    rgb: top.rgb.map((channel, index) => quantize(top.alpha * channel + (1 - top.alpha) * bottom.rgb[index])) as [number, number, number],
    alpha: 1,
  };
}

function contrastRatio(first: Paint, second: Paint): number {
  const luminance = (paint: Paint) => {
    const [r, g, b] = paint.rgb.map((channel) => (channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4));
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const [light, dark] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (light + 0.05) / (dark + 0.05);
}

test("warning text keeps 4.5:1 on toned chips and on the Logs warning row in every variant", () => {
  const styles = path.join(componentsSrc, "styles");
  const themes = fs.readFileSync(path.join(styles, "themes.css"), "utf8");
  const tokens = fs.readFileSync(path.join(styles, "tokens.css"), "utf8");
  const kit = fs.readFileSync(path.join(styles, "panel-kit.css"), "utf8");
  const host = fs.readFileSync(path.join(root, "console", "src", "console-host.css"), "utf8");

  // The formulas come from the stylesheets, so a change there is measured here.
  const inkShare = Number(/--_cc-warning-ink:\s*var\(--cc-warning-ink,\s*color-mix\(in srgb, var\(--_cc-warning\) (\d+)%, var\(--_cc-foreground\)\)\)/.exec(tokens)?.[1]) / 100;
  const chipShare = Number(/\.cc-chip:not\(\[data-tone="neutral"\]\):not\(\[data-tone="locked"\]\) \{\s*background: color-mix\(in srgb, var\(--_cc-tone\) (\d+)%, var\(--_cc-surface-card\)\);/.exec(kit)?.[1]) / 100;
  const rowShare = Number(/\.logline--warn \{ background: color-mix\(in oklch, var\(--warn\) (\d+)%, transparent\); \}/.exec(host)?.[1]) / 100;
  assert.ok(inkShare > 0 && chipShare > 0 && rowShare > 0, "the guard reads the warning ink, the chip fill and the Logs row tint");
  // The Logs stream sits on the sunken surface.
  assert.match(host, /\.logs__body \{[^}]*background: var\(--bg-sunken\);/);
  assert.match(host, /--bg-sunken:\s+var\(--_cc-surface-soft\);/);

  const pairs: string[] = [];
  for (const block of themes.matchAll(/\.cc-theme-scope\[data-cc-variant="(\w+)"\]\[data-cc-theme="(\w+)"\] \{([^}]*)\}/g)) {
    const [, variant, theme, body] = block;
    const token = (name: string) => {
      const value = new RegExp(`--_cc-${name}-default:\\s*([^;]+);`).exec(body)?.[1];
      assert.ok(value, `${variant} ${theme} defines ${name}`);
      return parsePaint(value);
    };
    const warning = token("warning");
    const ink = mixSrgb(warning, inkShare, token("foreground"));
    const chip = paintOver(mixSrgb(warning, chipShare, token("surface-card")), token("surface-card"));
    const logsRow = paintOver({ rgb: warning.rgb, alpha: rowShare }, token("surface-soft"));
    for (const [where, background] of [["warn chip", chip], ["Logs warning row", logsRow]] as const) {
      const ratio = contrastRatio(paintOver(ink, background), background);
      pairs.push(`${variant} ${theme} ${where} ${ratio.toFixed(2)}`);
      assert.ok(ratio >= 4.5, `${variant} ${theme}: warning text on the ${where} is ${ratio.toFixed(2)}:1`);
    }
  }
  assert.equal(pairs.length, 12, `six variant and theme pairs, two places each: ${pairs.join("; ")}`);
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
