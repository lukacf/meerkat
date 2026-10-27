#!/usr/bin/env node
"use strict";

// Real-browser typing-lag benchmark.
//
// jsdom does no style, layout or paint, so it cannot see keystroke latency.
// This builds src/perf/typing-lag-harness.tsx with the production console
// build options (the real ConsoleApp, React production build and CSS, fed by
// real-shaped gateway frames paged like the gateway) and drives Chromium:
// for each server history size it opens the docked chat, pages back through
// history like an operator scrolling up until the console's identity log is
// full, returns to the live edge, then types into the composer with real key
// events, first idle and then while a reply streams in.
//
// Latency per keystroke is keydown event.timeStamp (the hardware timestamp)
// to the first task after the next animation frame callback, which runs after
// that frame has been produced. Event Timing entries (>= 16 ms) and long
// tasks are recorded alongside. With --trace a CDP trace is recorded per size
// and renderer main-thread time inside keystroke windows is attributed to
// scripting, style, layout, paint and compositing.
//
// Usage: node typing-lag-browser.cjs [--turns 100,1000,5000] [--keys 40]
//        [--scenarios idle,streaming] [--budget-p95 idle=16,streaming=40]
//        [--trace] [--profile]
//        [--src ../other-tree/console] [--no-fill] [--json out.json]
//        [--session flowforensics|path/to/session.json] [--expand] [--restarted]
//        [--max-layout-objects 500] [--max-full-derivations-per-token 0.1]
//        [--max-turn-renders-per-token 2] [--max-rect-reads-per-token 20]
//        [--enforce-timing]
//
// Scenarios: idle (no stream), streaming (a reply streams into the open chat)
// and send-streaming (the operator sends a message, then keeps typing while
// its reply streams: the console holds the submitted turn in place instead
// of following the live edge).

const crypto = require("node:crypto");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { build } = require("esbuild");
const { chromium } = require("playwright");
const { fontLoaders } = require("./build.cjs");

function arg(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  if (index === -1) return fallback;
  const value = process.argv[index + 1];
  return value === undefined || value.startsWith("--") ? true : value;
}

const TURNS = String(arg("turns", "100,1000,5000")).split(",").map(Number);
// Render one persisted session instead of generated turns: "flowforensics"
// (a generated session shaped like a real slow one) or a Session JSON path.
const SESSION = arg("session", null);
const KEYS = Number(arg("keys", "40"));
// History published by a previous gateway process: live replies restart their
// source sequence in a new stream epoch, as after any gateway restart.
const RESTARTED = Boolean(arg("restarted", false));
const SCENARIOS = String(arg("scenarios", "idle,streaming")).split(",");
const TRACE = Boolean(arg("trace", false));
// p95 budget in ms, one number for every scenario or per scenario
// ("idle=16,streaming=40"). META_TYPING_LAG_BUDGET overrides the flag.
// Wall-clock budgets vary with the runner, so by default they are reported,
// not enforced; META_TYPING_LAG_ENFORCE_TIMING=1 or --enforce-timing makes
// them failures (local perf work). Structural limits always fail.
const ENFORCE_TIMING = process.env.META_TYPING_LAG_ENFORCE_TIMING === "1" || Boolean(arg("enforce-timing", false));
// Transcript entries presented per streamed token. Every entry was presented
// again on each token; a continued derivation presents only the open text.
const MAX_PRESENTED_ENTRIES_PER_TOKEN = arg("max-presented-entries-per-token", null) === null ? null : Number(arg("max-presented-entries-per-token"));
const MAX_FULL_DERIVATIONS_PER_TOKEN = arg("max-full-derivations-per-token", null) === null ? null : Number(arg("max-full-derivations-per-token"));
const MAX_TURN_RENDERS_PER_TOKEN = arg("max-turn-renders-per-token", null) === null ? null : Number(arg("max-turn-renders-per-token"));
// Element rect reads per streamed token. Each read forces layout; holding a
// submitted turn used to read every mounted row's rect on every token.
const MAX_RECT_READS_PER_TOKEN = arg("max-rect-reads-per-token", null) === null ? null : Number(arg("max-rect-reads-per-token"));
// Layouts forced by script (geometry read while layout was dirty) per
// streamed token (needs --trace). Geometry belongs in resize observer
// callbacks, which run after the frame's layout. A one-off commit, such as a
// send mounting its row, may still force one.
const MAX_FORCED_LAYOUTS_PER_TOKEN = arg("max-forced-layouts-per-token", null) === null ? null : Number(arg("max-forced-layouts-per-token"));
// Markdown source characters parsed per streamed token. Re-parsing the whole
// reply on every token grew with its length; only the open block should.
const MAX_MARKDOWN_CHARS_PER_TOKEN = arg("max-markdown-chars-per-token", null) === null ? null : Number(arg("max-markdown-chars-per-token"));
// Day-separator date formats per streamed token. A streaming turn renders its
// separators on every token, and locale formatting is costly; a day key's
// label is formatted once.
const MAX_DAY_LABEL_FORMATS_PER_TOKEN = arg("max-day-label-formats-per-token", null) === null ? null : Number(arg("max-day-label-formats-per-token"));
// Console renders (live-frame flushes) per second while a reply streams.
// Streamed text alone renders on every third animation frame, so at 60 Hz
// at most about 20 per second; every token used to render.
const MAX_STREAM_COMMITS_PER_SECOND = arg("max-stream-commits-per-second", null) === null ? null : Number(arg("max-stream-commits-per-second"));
const BUDGET_SPEC = process.env.META_TYPING_LAG_BUDGET || arg("budget-p95", null);
const BUDGET_P95 = BUDGET_SPEC === null || BUDGET_SPEC === true ? null : parseBudget(String(BUDGET_SPEC));

function parseBudget(spec) {
  if (/^[0-9.]+$/.test(spec)) return { default: Number(spec) };
  return Object.fromEntries(spec.split(",").map((part) => {
    const [scenario, ms] = part.split("=");
    return [scenario.trim(), Number(ms)];
  }));
}

function budgetFor(scenario) {
  return BUDGET_P95[scenario] ?? BUDGET_P95.default ?? Infinity;
}
const JSON_OUT = arg("json", null);
// Windowed transcript equivalence (#544): the same filled history rendered
// windowed and unwindowed must look identical at every scroll position.
const EQUIVALENCE = Boolean(arg("equivalence", false));
const MAX_MOUNTED_ELEMENTS = arg("max-mounted-elements", null) === null ? null : Number(arg("max-mounted-elements"));
const HEADED = Boolean(arg("headed", false));
const FILL = !arg("no-fill", false);
// Open every tool card and disclosure, as an operator reading tool output does.
const EXPAND = Boolean(arg("expand", false));
// Fail when an idle keystroke lays out more than this many layout objects
// (requires --trace). Hardware-independent: a keystroke that reaches the
// transcript lays out tens of thousands of objects, one that stays in the
// composer a handful.
const MAX_LAYOUT_OBJECTS = arg("max-layout-objects", null) === null ? null : Number(arg("max-layout-objects"));
const DPR = Number(arg("dpr", "1"));
// One streamed text_delta frame every STREAM_MS while typing (real token rate).
const STREAM_MS = Number(arg("stream-ms", "25"));
// Console tree to build the harness from (default: this one). Lets the same
// benchmark measure a baseline checkout.
const SRC = path.resolve(String(arg("src", __dirname)));
const PROFILE = Boolean(arg("profile", false));
const MINIFY = !arg("no-minify", false) && !PROFILE;
const TRACE_INVALIDATIONS = Boolean(arg("trace-invalidations", false));
const outDir = path.join(__dirname, ".tmp/typing-lag", crypto.createHash("sha1").update(SRC).digest("hex").slice(0, 8));

async function buildHarness() {
  await fs.mkdir(outDir, { recursive: true });
  if (SRC !== __dirname) {
    // The harness and generator are benchmark files; run the same ones there.
    for (const file of ["typing-lag-harness.tsx", "realistic-transcript.ts", "session-projection.ts"]) {
      await fs.copyFile(path.join(__dirname, "src/perf", file), path.join(SRC, "src/perf", file));
    }
  }
  await build({
    absWorkingDir: SRC,
    entryPoints: [path.join(SRC, "src/perf/typing-lag-harness.tsx")],
    outfile: path.join(outDir, "harness.js"),
    bundle: true,
    format: "iife",
    platform: "browser",
    target: ["es2020"],
    define: { "process.env.NODE_ENV": '"production"', NODE_ENV: '"production"' },
    alias: {
      "@console-core": path.resolve(SRC, "../packages/console-core/src/index.ts"),
      "@console-components": path.resolve(SRC, "../packages/console-components/src/index.ts"),
      "@console-components/styles": path.resolve(SRC, "../packages/console-components/src/styles/index.ts"),
    },
    nodePaths: [path.resolve(SRC, "node_modules")],
    loader: fontLoaders,
    jsx: "automatic",
    keepNames: true,
    minify: MINIFY,
    logLevel: "error",
  });
  await fs.writeFile(
    path.join(outDir, "index.html"),
    `<!doctype html><html lang="en"><head><meta charset="utf-8" /><title>typing lag</title>
<link rel="stylesheet" href="/harness.css" /></head><body><div id="root"></div>
<script src="/harness.js"></script></body></html>`,
  );
}

function serve() {
  const types = { ".html": "text/html", ".js": "application/javascript", ".css": "text/css" };
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, "http://x");
    const file = url.pathname === "/" ? "index.html" : path.basename(url.pathname);
    try {
      const bytes = file === "session.json" && SESSION && SESSION !== "flowforensics"
        ? await fs.readFile(path.resolve(String(SESSION)))
        : await fs.readFile(path.join(outDir, file));
      res.writeHead(200, { "content-type": types[path.extname(file)] || "application/octet-stream" });
      res.end(bytes);
    } catch {
      res.writeHead(404);
      res.end();
    }
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server)));
}

// Installed before any page script: Event Timing, long tasks, and the
// keydown-to-after-next-frame probe for composer keystrokes. Trace markers
// (console.timeStamp) bracket each keystroke window for --trace.
function installProbe() {
  // Sink for the console's countRender() calls (a no-op without it).
  globalThis.__consoleRenderCounts = {};
  window.__lag = { probe: [], events: [], longtasks: [], index: 0, rects: 0 };
  const rectOf = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
    window.__lag.rects += 1;
    return rectOf.call(this);
  };
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) {
      window.__lag.events.push({ name: entry.name, duration: entry.duration, interactionId: entry.interactionId, start: entry.startTime });
    }
  }).observe({ type: "event", durationThreshold: 16, buffered: true });
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) window.__lag.longtasks.push(entry.duration);
  }).observe({ type: "longtask", buffered: true });
  document.addEventListener(
    "keydown",
    (event) => {
      if (!(event.target instanceof HTMLTextAreaElement)) return;
      const start = event.timeStamp;
      const index = window.__lag.index++;
      console.timeStamp(`lag:start:${index}`);
      requestAnimationFrame(() => {
        const channel = new MessageChannel();
        channel.port1.onmessage = () => {
          window.__lag.probe.push(performance.now() - start);
          console.timeStamp(`lag:end:${index}`);
        };
        channel.port2.postMessage(0);
      });
    },
    true,
  );
}

function percentile(sorted, p) {
  if (sorted.length === 0) return NaN;
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * p) - 1))];
}

function summarize(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return { n: sorted.length, p50: percentile(sorted, 0.5), p95: percentile(sorted, 0.95), max: sorted[sorted.length - 1] };
}

// Script slices: a Layout nested inside one was forced by a geometry read.
// A native layout, such as the editor's own text insertion under
// EventDispatch, is not counted.
const FORCING_SCRIPT = new Set(["FunctionCall", "v8.callFunction", "RunMicrotasks", "v8.run", "EvaluateScript", "V8.Execute"]);

// Renderer main-thread slices, attributed by name (self time).
const CATEGORY = {
  scripting: ["FunctionCall", "EventDispatch", "TimerFire", "FireAnimationFrame", "RunMicrotasks", "v8.callFunction", "EvaluateScript", "V8.GC_SCAVENGER_SCAVENGE_PARALLEL", "MinorGC", "MajorGC", "V8.GCScavenger", "BlinkGC.AtomicPhase", "v8.run", "V8.Execute"],
  style: ["UpdateLayoutTree", "RecalculateStyles", "ParseAuthorStyleSheet", "Document::UpdateStyleAndLayoutTree"],
  layout: ["Layout", "LocalFrameView::performLayout", "UpdateLayerTree", "Blink.ForcedStyleAndLayout.UpdateTime", "Document::UpdateStyleAndLayout", "LocalFrameView::UpdateStyleAndLayout", "ResizeObserverController::handleObservations", "IntersectionObserverController::computeIntersections"],
  paint: ["Paint", "PrePaint", "Blink.PrePaint.UpdateTime", "Blink.Paint.UpdateTime", "PaintImage", "LocalFrameView::RunPaintLifecyclePhase"],
  composite: ["Layerize", "Commit", "CompositeLayers", "PaintArtifactCompositor::Update", "Blink.CompositingInputs.UpdateTime", "UpdateLayer"],
};

function analyzeTrace(trace) {
  const events = trace.traceEvents || trace;
  const names = new Map();
  for (const e of events) if (e.ph === "M" && e.name === "thread_name") names.set(`${e.pid}:${e.tid}`, e.args.name);
  const counts = new Map();
  for (const e of events) {
    const key = `${e.pid}:${e.tid}`;
    if (names.get(key) === "CrRendererMain" && e.ph === "X") counts.set(key, (counts.get(key) || 0) + 1);
  }
  const main = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
  if (!main) return null;
  const slices = events
    .filter((e) => `${e.pid}:${e.tid}` === main && e.ph === "X" && typeof e.dur === "number")
    .sort((a, b) => a.ts - b.ts || b.dur - a.dur);
  const records = [];
  const stack = [];
  for (const e of slices) {
    while (stack.length && stack[stack.length - 1].e.ts + stack[stack.length - 1].e.dur <= e.ts) stack.pop();
    // A layout nested inside script ran because script read geometry with
    // layout dirty (a forced, synchronous layout), not as the frame's own.
    const record = { e, self: e.dur, inScript: stack.some((r) => FORCING_SCRIPT.has(r.e.name)), parents: stack.map((r) => r.e.name) };
    if (stack.length) stack[stack.length - 1].self -= e.dur;
    stack.push(record);
    records.push(record);
  }
  const ranges = [];
  for (const m of events) {
    const message = m.name === "TimeStamp" ? m.args?.data?.message : undefined;
    if (!message || !message.startsWith("lag:")) continue;
    const [, kind, idx] = message.split(":");
    ranges[idx] = ranges[idx] || {};
    ranges[idx][kind] = m.ts;
  }
  const windows = ranges.filter((r) => r && r.start !== undefined && r.end !== undefined);
  // Any main-thread work overlapping a keystroke window delays that
  // keystroke's frame, including a task that started before the keydown.
  const byName = new Map();
  for (const rec of records) {
    const start = rec.e.ts;
    const end = rec.e.ts + rec.e.dur;
    let overlap = 0;
    for (const w of windows) overlap += Math.max(0, Math.min(end, w.end) - Math.max(start, w.start));
    if (overlap <= 0 || rec.self <= 0) continue;
    byName.set(rec.e.name, (byName.get(rec.e.name) || 0) + (rec.self * Math.min(1, overlap / rec.e.dur)) / 1000);
  }
  const totals = { scripting: 0, style: 0, layout: 0, paint: 0, composite: 0, other: 0 };
  for (const [name, ms] of byName) {
    const category = Object.keys(CATEGORY).find((c) => CATEGORY[c].includes(name)) || "other";
    totals[category] += ms;
  }
  const keystrokes = windows.length || 1;
  const span = windows.length ? { start: Math.min(...windows.map((w) => w.start)), end: Math.max(...windows.map((w) => w.end)) } : null;
  const forced = span ? records.filter((r) => r.e.name === "Layout" && r.inScript && r.e.ts >= span.start && r.e.ts <= span.end) : [];
  // With --trace-invalidations Layout events carry the script stack that
  // forced them; name the sources so a regression points at its reader.
  const forcedSources = new Map();
  for (const r of forced) {
    const frames = r.e.args?.beginData?.stackTrace ?? [];
    const source = frames.length
      ? frames.slice(0, 3).map((f) => `${f.functionName || "(anonymous)"}@${String(f.url || "").split("/").pop()}:${f.lineNumber}`).join(" < ")
      : r.parents.join(" > ");
    forcedSources.set(source, (forcedSources.get(source) || 0) + 1);
  }
  let maxLayoutObjects = 0;
  for (const e of slices) {
    if (e.name !== "Layout") continue;
    const inside = windows.some((w) => e.ts >= w.start && e.ts <= w.end);
    if (inside) maxLayoutObjects = Math.max(maxLayoutObjects, e.args?.beginData?.totalObjects ?? 0);
  }
  return {
    keystrokes,
    maxLayoutObjects,
    forcedLayouts: forced.length,
    forcedSources: [...forcedSources.entries()].sort((x, y) => y[1] - x[1]).slice(0, 5),
    perKey: Object.fromEntries(Object.entries(totals).map(([k, v]) => [k, v / keystrokes])),
    top: [...byName.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12).map(([name, ms]) => ({ name, msPerKey: ms / keystrokes })),
  };
}

async function fillHistory(page) {
  // Page back like an operator scrolling up: reveal mounted-but-windowed turns
  // and load older pages until history is exhausted or the log is trimmed.
  for (let step = 0; step < 400; step += 1) {
    const reveal = page.locator('[data-testid^="chat-reveal-earlier"]');
    if (await reveal.count()) {
      await reveal.first().click();
      await page.waitForTimeout(60);
      continue;
    }
    const older = page.locator("button.conv__history:not([disabled])", { hasText: "Load older history" });
    if (!(await older.count())) {
      const loading = page.locator("button.conv__history[disabled]");
      if (await loading.count()) {
        await page.waitForTimeout(100);
        continue;
      }
      break;
    }
    await older.first().click();
    await page.waitForTimeout(120);
  }
}

// Deterministic work counters: renders by component, transcript
// derivations, and streamed tokens delivered.
function workCounters(page) {
  return page.evaluate(() => ({
    renders: { ...(globalThis.__consoleRenderCounts || {}) },
    derivations: window.__perf?.derivations?.() ?? null,
    streamed: window.__perf?.streamed ?? 0,
    rects: window.__lag?.rects ?? 0,
    at: performance.now(),
  }));
}

function workDelta(before, after, keystrokes) {
  const renders = (name) => (after.renders[name] || 0) - (before.renders[name] || 0);
  const tokens = after.streamed - before.streamed;
  const per = (value, n) => (n > 0 ? value / n : 0);
  const full = after.derivations && before.derivations ? after.derivations.full - before.derivations.full : null;
  const presented = after.derivations?.presented !== undefined && before.derivations?.presented !== undefined
    ? after.derivations.presented - before.derivations.presented : null;
  return {
    tokens,
    fullDerivations: full,
    fullDerivationsPerToken: full === null ? null : per(full, tokens),
    presentedEntriesPerToken: presented === null ? null : per(presented, tokens),
    turnRendersPerToken: per(renders("TranscriptTurn"), tokens),
    rowRendersPerToken: per(renders("MessageRow"), tokens),
    markdownCharsPerToken: per(renders("MarkdownSourceChars"), tokens),
    dayLabelFormatsPerToken: per(renders("DayLabelFormats"), tokens),
    activeRunFramesPerToken: per(renders("ActiveRunFramesRead"), tokens),
    commitsPerSecond: per(renders("LiveRenderFlush"), (after.at - before.at) / 1000),
    transcriptRendersPerKeystroke: per(renders("TranscriptView"), keystrokes),
    rowRendersPerKeystroke: per(renders("MessageRow"), keystrokes),
    rectReadsPerToken: per(after.rects - before.rects, tokens),
  };
}

async function typeKeys(page, textarea) {
  await textarea.click();
  await page.evaluate(() => {
    window.__lag.probe.length = 0;
    window.__lag.events.length = 0;
    window.__lag.longtasks.length = 0;
  });
  const before = await workCounters(page);
  const text = "the quick brown fox jumps over the lazy dog and keeps typing ";
  let seed = 7;
  const rand = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
  // Words typed at 60-120 ms per key with 450-900 ms pauses between words,
  // so the composer's debounced draft publish also lands mid-sentence.
  for (let i = 0; i < KEYS; i += 1) {
    const ch = text[i % text.length];
    await page.keyboard.press(ch === " " ? "Space" : ch);
    await page.waitForTimeout(ch === " " ? 450 + rand() * 450 : 60 + rand() * 60);
  }
  await page.waitForTimeout(600);
  const work = workDelta(before, await workCounters(page), KEYS);
  const lag = await page.evaluate(() => window.__lag);
  const typed = await textarea.inputValue();
  await textarea.fill("");
  const keyEvents = lag.events.filter((e) => ["keydown", "keypress", "input", "keyup"].includes(e.name));
  const perInteraction = new Map();
  for (const e of keyEvents) {
    const key = e.interactionId || `${e.name}:${e.start}`;
    perInteraction.set(key, Math.max(perInteraction.get(key) || 0, e.duration));
  }
  return {
    work,
    typedOk: typed.length >= KEYS,
    latency: summarize(lag.probe),
    samples: lag.probe.map((ms) => Math.round(ms * 10) / 10),
    eventTimingOver16: perInteraction.size,
    eventTimingMax: Math.max(0, ...perInteraction.values()),
    longtasks: summarize(lag.longtasks),
  };
}

async function measureSize(browser, baseUrl, turns) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: DPR });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(installProbe);
  const session = SESSION ? `&session=${SESSION === "flowforensics" ? "flowforensics" : "/session.json"}` : "";
  await page.goto(`${baseUrl}/?turns=${turns}${session}${RESTARTED ? "&restarted=1" : ""}`);
  await page.waitForFunction(() => document.querySelectorAll("[data-chat-turn-index]").length > 0, null, { timeout: 120_000 });
  if (FILL) await fillHistory(page);
  if (EXPAND) {
    for (let pass = 0; pass < 4; pass += 1) {
      const opened = await page.evaluate(() => {
        let count = 0;
        for (const details of document.querySelectorAll(".conv__body details:not([open])")) {
          details.open = true;
          count += 1;
        }
        for (const header of document.querySelectorAll('.conv__body [role="button"][aria-expanded="false"]')) {
          header.click();
          count += 1;
        }
        return count;
      });
      await page.waitForTimeout(400);
      if (!opened) break;
    }
  }
  const textarea = page.locator('textarea[data-testid^="chat-composer"]').first();
  // Back at the live edge, where an operator types.
  await page.evaluate(() => {
    const body = document.querySelector(".conv__body");
    if (body) body.scrollTop = body.scrollHeight;
  });
  await page.waitForTimeout(1500);
  const dom = await page.evaluate(() => {
    const turnNodes = document.querySelectorAll("[data-chat-turn-index]");
    return { elements: document.getElementsByTagName("*").length, mountedTurns: turnNodes.length };
  });
  await textarea.click();
  await page.keyboard.type("warm", { delay: 80 });
  await textarea.fill("");
  await page.waitForTimeout(300);
  const result = { turns, dom, scenarios: {}, errors };
  for (const scenario of SCENARIOS) {
    let stop = null;
    if (scenario === "send-streaming") {
      // Send like an operator, wait for the accepted turn to render, then
      // stream its reply while typing the next message.
      const message = "Write the long report now.";
      await textarea.click();
      await page.keyboard.type(message, { delay: 20 });
      await page.keyboard.press("Enter");
      await page.waitForFunction((text) => [...document.querySelectorAll("[data-chat-turn-index]")].at(-1)?.textContent?.includes(text), message, { timeout: 30_000 });
      await page.waitForTimeout(500);
    }
    if (scenario === "streaming" || scenario === "send-streaming") {
      await page.evaluate((everyMs) => {
        window.__stopStream = window.__perf.streamReply(everyMs);
      }, STREAM_MS);
      await page.waitForTimeout(1000);
      stop = () => page.evaluate(() => window.__stopStream?.());
    }
    if (TRACE) {
      await browser.startTracing(page, {
        categories: ["devtools.timeline", "disabled-by-default-devtools.timeline", "blink", "v8.execute", "blink.user_timing",
          ...(TRACE_INVALIDATIONS ? ["disabled-by-default-devtools.timeline.invalidationTracking", "disabled-by-default-devtools.timeline.stack"] : [])],
      });
    }
    let cdp = null;
    if (PROFILE) {
      cdp = await context.newCDPSession(page);
      await cdp.send("Profiler.enable");
      await cdp.send("Profiler.setSamplingInterval", { interval: 250 });
      await cdp.send("Profiler.start");
    }
    const measured = await typeKeys(page, textarea);
    if (cdp) {
      const { profile } = await cdp.send("Profiler.stop");
      measured.profile = summarizeProfile(profile);
      await fs.writeFile(path.join(outDir, `profile-${turns}-${scenario}.cpuprofile`), JSON.stringify(profile));
      await cdp.detach();
    }
    if (TRACE) {
      const buffer = await browser.stopTracing();
      const tracePath = path.join(outDir, `trace-${turns}-${scenario}.json`);
      await fs.writeFile(tracePath, buffer);
      const traceJson = JSON.parse(buffer.toString("utf8"));
      measured.breakdown = analyzeTrace(traceJson);
      measured.work.forcedLayoutsPerToken = measured.work.tokens ? (measured.breakdown?.forcedLayouts ?? 0) / measured.work.tokens : 0;
      if (TRACE_INVALIDATIONS) {
        // Big layouts and the invalidations recorded just before each.
        const events = traceJson.traceEvents || traceJson;
        for (const big of events.filter((e) => e.ph === "X" && e.name === "Layout" && e.dur > 20_000)) {
          const counts = {};
          let stack;
          for (const e of events) {
            if (!/InvalidationTracking/.test(e.name) || e.ts > big.ts || e.ts < big.ts - 300_000) continue;
            const d = e.args?.data || {};
            const key = `${e.name}|${d.reason || ""}|${d.nodeName || ""}`;
            counts[key] = (counts[key] || 0) + 1;
            if (d.stackTrace && !stack) stack = d.stackTrace.slice(0, 8).map((f) => `${f.functionName}:${f.lineNumber}`);
          }
          process.stdout.write(`  big layout ${Math.round(big.dur / 1000)} ms objects=${big.args?.beginData?.totalObjects} dirty=${big.args?.beginData?.dirtyObjects} ${JSON.stringify(counts)} stack=${JSON.stringify(stack)}\n`);
        }
      }
      measured.breakdown.tracePath = tracePath;
    }
    if (stop) await stop();
    result.scenarios[scenario] = measured;
    await page.waitForTimeout(500);
  }
  await context.close();
  return result;
}

// Self and inclusive time per function from a CDP CPU profile.
function summarizeProfile(profile) {
  const byId = new Map(profile.nodes.map((node) => [node.id, node]));
  const parent = new Map();
  for (const node of profile.nodes) for (const child of node.children || []) parent.set(child, node.id);
  const label = (node) => `${node.callFrame.functionName || "(anonymous)"}${node.callFrame.url ? `:${node.callFrame.lineNumber}` : ""}`;
  const self = new Map();
  const inclusive = new Map();
  profile.samples.forEach((id, index) => {
    const dt = (profile.timeDeltas[index] || 0) / 1000;
    const node = byId.get(id);
    self.set(label(node), (self.get(label(node)) || 0) + dt);
    const seen = new Set();
    for (let cursor = id; cursor !== undefined; cursor = parent.get(cursor)) {
      const key = label(byId.get(cursor));
      if (seen.has(key)) continue;
      seen.add(key);
      inclusive.set(key, (inclusive.get(key) || 0) + dt);
    }
  });
  const top = (map, n) => [...map.entries()].sort((a, b) => b[1] - a[1]).slice(0, n).map(([name, ms]) => ({ name, ms: Math.round(ms) }));
  // Who calls the hottest native DOM reads (forced layout, selector scans).
  const callers = new Map();
  profile.samples.forEach((id, index) => {
    const node = byId.get(id);
    const name = node.callFrame.functionName;
    if (!["querySelectorAll", "getBoundingClientRect", "get scrollHeight", "closest", "querySelector"].includes(name)) return;
    const caller = parent.has(id) ? label(byId.get(parent.get(id))) : "(root)";
    const grand = parent.has(parent.get(id)) ? label(byId.get(parent.get(parent.get(id)))) : "";
    const key = `${name} <- ${caller} <- ${grand}`;
    callers.set(key, (callers.get(key) || 0) + (profile.timeDeltas[index] || 0) / 1000);
  });
  return { self: top(self, 25), inclusive: top(inclusive, 45), callers: top(callers, 8) };
}

function fmt2(n) {
  return Number.isFinite(n) ? n.toFixed(2) : "-";
}

function fmt(n) {
  return Number.isFinite(n) ? n.toFixed(1) : "-";
}

/// Decode a non-interlaced 8-bit RGB(A) PNG (what Chromium screenshots are)
/// to count differing pixels when two screenshots are not byte-identical.
function decodePng(buffer) {
  const zlib = require("node:zlib");
  let offset = 8;
  let width = 0, height = 0, channels = 0;
  const idat = [];
  while (offset < buffer.length) {
    const length = buffer.readUInt32BE(offset);
    const type = buffer.toString("ascii", offset + 4, offset + 8);
    const data = buffer.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      width = data.readUInt32BE(0); height = data.readUInt32BE(4);
      channels = data[9] === 6 ? 4 : data[9] === 2 ? 3 : 0;
      if (!channels || data[8] !== 8 || data[12] !== 0) throw new Error("unsupported png");
    } else if (type === "IDAT") idat.push(data);
    offset += 12 + length;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const pixels = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x += 1) {
      const a = x >= channels ? pixels[y * stride + x - channels] : 0;
      const b = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const c = x >= channels && y > 0 ? pixels[(y - 1) * stride + x - channels] : 0;
      const predictor = filter === 1 ? a : filter === 2 ? b : filter === 3 ? (a + b) >> 1
        : filter === 4 ? (() => { const p = a + b - c; const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c); return pa <= pb && pa <= pc ? a : pb <= pc ? b : c; })() : 0;
      pixels[y * stride + x] = (line[x] + predictor) & 0xff;
    }
  }
  return { width, height, channels, pixels };
}

/// Pixels that differ at all and the largest channel difference. Compared
/// pages must be pixel-identical: with the console's web fonts blocked
/// (openFilled) and text rastered as below, two identical pages measure 0
/// pixels. Every equivalence run measures that noise floor again
/// (noiseFloor), so a renderer that stops being pixel-stable fails there,
/// by name, rather than in the windowed comparison. With network fonts, LCD
/// text and subpixel glyph positions, identical pages differed by about ten
/// pixels at one level, and by hundreds when a font swap raced the capture.
const PIXEL_STABLE_TEXT = ["--disable-lcd-text", "--disable-font-subpixel-positioning"];
function differingPixels(left, right) {
  const a = decodePng(left), b = decodePng(right);
  if (a.width !== b.width || a.height !== b.height) return { any: Infinity, maxDelta: 255 };
  let any = 0, maxDelta = 0;
  for (let i = 0; i < a.width * a.height; i += 1) {
    let delta = 0;
    for (let k = 0; k < Math.min(a.channels, b.channels, 3); k += 1) delta = Math.max(delta, Math.abs(a.pixels[i * a.channels + k] - b.pixels[i * b.channels + k]));
    if (delta > 0) any += 1;
    maxDelta = Math.max(maxDelta, delta);
  }
  return { any, maxDelta };
}

async function openFilled(browser, baseUrl, turns, windowed) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: DPR });
  // Every compared page renders the same local fonts, not the console's web
  // fonts: pages swapped those in (or kept the fallback) on their own timing,
  // and their fractional text heights put rounded borders at fractional
  // offsets, which the windowed and unwindowed layer trees anti-alias up to
  // two levels apart although every box sits at the same position. Fonts
  // imported from the network (a --src tree from before the console carried
  // its own) are aborted; the console's own fonts are inline data: URLs, so
  // the stylesheet these pages load drops its @font-face rules.
  await context.route(/^https:\/\/fonts\.(googleapis|gstatic)\.com\//, (route) => route.abort());
  await context.route((url) => url.pathname === "/harness.css", async (route) => {
    const response = await route.fetch();
    const css = (await response.text()).replace(/@font-face\s*\{[^}]*\}/g, "");
    // Fresh headers: the fetched ones describe the original body (chunked).
    await route.fulfill({ status: response.status(), headers: { "content-type": "text/css" }, body: css });
  });
  const page = await context.newPage();
  if (!windowed) await page.addInitScript(() => { globalThis.__consoleTranscriptWindowing = false; });
  // The same wall clock on every page: time labels ("Today", clock times)
  // would otherwise differ between pages loaded in different minutes.
  await page.clock.setFixedTime(new Date("2026-09-22T12:00:00Z"));
  await page.goto(`${baseUrl}/?turns=${turns}`);
  await page.waitForFunction(() => document.querySelectorAll("[data-chat-turn-index]").length > 0, null, { timeout: 120_000 });
  // Font-free must not mean unstyled: two pages without the stylesheet also
  // compare equal. The transcript scroller's layout comes from it alone.
  const scroller = await page.evaluate(() => { const style = getComputedStyle(document.querySelector(".conv__body")); return `${style.display} ${style.overflowY}`; });
  if (scroller !== "flex auto") throw new Error(`compared pages must load the harness stylesheet, but the transcript scroller is "${scroller}", not "flex auto"`);
  const webFonts = await page.evaluate(() => [...document.fonts].map((face) => face.family));
  if (webFonts.length) throw new Error(`compared pages must render local fonts, but ${webFonts.length} web font faces remain (${[...new Set(webFonts)].join(", ")})`);
  await fillHistory(page);
  await page.evaluate(() => { const body = document.querySelector(".conv__body"); body.scrollTop = body.scrollHeight; });
  await page.waitForTimeout(1500);
  return page;
}

async function settled(page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(resolve)))));
  await page.waitForTimeout(150);
}

function transcriptState(page) {
  return page.evaluate(() => {
    const body = document.querySelector(".conv__body");
    const box = body.getBoundingClientRect();
    const rows = [...body.querySelectorAll("[data-conversation-row-id]")].map((element) => {
      const rect = element.getBoundingClientRect();
      return { id: element.dataset.conversationRowId, top: Math.round((rect.top - box.top) * 10) / 10, height: Math.round(rect.height * 10) / 10 };
    }).filter((row) => row.top + row.height > 0 && row.top < box.height);
    return {
      scrollTop: body.scrollTop, scrollHeight: body.scrollHeight, clientHeight: body.clientHeight, rows,
      mountedTurns: body.querySelectorAll("[data-chat-turn-index]").length, elements: document.getElementsByTagName("*").length,
      box: { x: box.x, y: box.y, width: box.width, height: box.height },
      // Content starts after the left padding (the turn rail's gutter).
      contentLeft: parseFloat(getComputedStyle(body).paddingLeft),
      railActive: [...document.querySelectorAll(".conv-turn-rail__button")].map((button) => `${button.getAttribute("aria-label")}:${button.classList.contains("is-active") || button.getAttribute("aria-current") === "true"}`).join("|"),
    };
  });
}

/// Windowed versus unwindowed at a spread of scroll positions: scroll height,
/// visible rows and their offsets, and a pixel-identical transcript.
const contentClip = (state) => ({ x: state.box.x + state.contentLeft + 1, y: state.box.y, width: state.box.width - state.contentLeft - 1, height: state.box.height });

/// Two identical unwindowed pages compared the same way: the renderer must be
/// pixel-stable (0 differing pixels) before the windowed comparison means
/// anything.
async function noiseFloor(left, right, turns, clipOf, positions) {
  const failures = [];
  let worst = { any: 0, maxDelta: 0 };
  let total = 0;
  for (const y of positions) {
    for (const page of [left, right]) await page.evaluate((top) => { document.querySelector(".conv__body").scrollTop = top; }, y);
    await settled(left); await settled(right);
    const clip = clipOf(await transcriptState(right));
    const [a, b] = [await left.screenshot({ clip }), await right.screenshot({ clip })];
    const diff = a.equals(b) ? { any: 0, maxDelta: 0 } : differingPixels(a, b);
    worst = { any: Math.max(worst.any, diff.any), maxDelta: Math.max(worst.maxDelta, diff.maxDelta) };
    total += diff.any;
    if (diff.any > 0) {
      await fs.writeFile(path.join(outDir, `noise-${turns}-${y}-a.png`), a);
      await fs.writeFile(path.join(outDir, `noise-${turns}-${y}-b.png`), b);
      failures.push(`turns=${turns} scrollTop=${y}: two identical unwindowed pages differ (${diff.any} pixels, max channel delta ${diff.maxDelta}); the renderer is no longer pixel-stable (screenshots in ${outDir})`);
    }
  }
  process.stdout.write(`[typing-lag-browser] noise floor turns=${turns}: identical pages differ by ${total} pixels over ${positions.length} positions (at most ${worst.any} per position), max channel delta ${worst.maxDelta}\n`);
  return failures;
}

async function equivalence(browser, baseUrl, turns) {
  const failures = [];
  const windowed = await openFilled(browser, baseUrl, turns, true);
  const oracle = await openFilled(browser, baseUrl, turns, false);
  const base = await transcriptState(oracle);
  const range = base.scrollHeight - base.clientHeight;
  // A pane resize invalidates every measured height: compare after one too.
  const positions = [1, 0.97, 0.9, 0.75, 0.6, 0.5, 0.4, 0.33, 0.25, 0.1, 0.05, 0, 0.5, 0.99, "resize", 0.8, 0.3, 1]
    .map((f) => (f === "resize" ? f : Math.round(range * f)));
  const twin = await openFilled(browser, baseUrl, turns, false);
  failures.push(...await noiseFloor(oracle, twin, turns, contentClip, positions.filter((y) => y !== "resize")));
  await twin.context().close();
  let checked = 0;
  let maxElements = 0;
  const differing = { any: 0, maxDelta: 0 };
  for (const y of positions) {
    if (y === "resize") {
      for (const page of [windowed, oracle]) await page.setViewportSize({ width: 1180, height: 820 });
      await windowed.waitForTimeout(1000);
      continue;
    }
    for (const page of [windowed, oracle]) await page.evaluate((top) => { document.querySelector(".conv__body").scrollTop = top; }, y);
    await settled(windowed); await settled(oracle);
    const [a, b] = [await transcriptState(windowed), await transcriptState(oracle)];
    maxElements = Math.max(maxElements, a.elements);
    const where = `turns=${turns} scrollTop=${y}`;
    if (Math.abs(a.scrollHeight - b.scrollHeight) > 0.5) failures.push(`${where}: scrollHeight ${a.scrollHeight} != ${b.scrollHeight}`);
    if (process.env.EQUIVALENCE_DEBUG && Math.abs(a.scrollHeight - b.scrollHeight) > 0.5) {
      const spacers = await windowed.evaluate(() => [...document.querySelectorAll("[data-conversation-spacer]")].map((el) => ({ range: el.dataset.conversationSpacer, height: el.getBoundingClientRect().height })));
      const real = await oracle.evaluate(() => { const gap = parseFloat(getComputedStyle(document.querySelector(".conv__body")).rowGap); return { gap, turns: Object.fromEntries([...document.querySelectorAll("[data-chat-turn-index]")].map((el) => [el.dataset.chatTurnIndex, el.getBoundingClientRect().height])) }; });
      const mountedHeights = await windowed.evaluate(() => Object.fromEntries([...document.querySelectorAll("[data-chat-turn-index]")].map((el) => [el.dataset.chatTurnIndex, el.getBoundingClientRect().height])));
      for (const sp of spacers) {
        const [from, to] = sp.range.split("-").map(Number);
        let expected = real.gap * (to - from - 1);
        for (let i = from; i < to; i += 1) expected += real.turns[i] ?? NaN;
        if (Math.abs(expected - sp.height) > 0.5) process.stdout.write(`  spacer ${sp.range}: ${sp.height} vs real ${expected.toFixed(1)}\n`);
      }
      for (const [i, h] of Object.entries(mountedHeights)) if (Math.abs((real.turns[i] ?? NaN) - h) > 0.5) process.stdout.write(`  mounted turn ${i}: ${h} vs real ${real.turns[i]}\n`);
    }
    if (Math.abs(a.scrollTop - b.scrollTop) > 0.5) failures.push(`${where}: scrollTop ${a.scrollTop} != ${b.scrollTop}`);
    if (JSON.stringify(a.rows) !== JSON.stringify(b.rows)) failures.push(`${where}: visible rows differ: ${JSON.stringify(a.rows).slice(0, 300)} vs ${JSON.stringify(b.rows).slice(0, 300)}`);
    if (a.railActive !== b.railActive) failures.push(`${where}: turn rail differs`);
    // Glyph anti-aliasing bleeds one column into the gutter and is not
    // deterministic there even between two identical unwindowed pages, so the
    // pixels compared start at the content edge; the gutter's rail is
    // compared above as state.
    const clip = contentClip(b);
    const [shotA, shotB] = [await windowed.screenshot({ clip }), await oracle.screenshot({ clip })];
    if (!shotA.equals(shotB)) {
      const diff = differingPixels(shotA, shotB);
      differing.any += diff.any;
      differing.maxDelta = Math.max(differing.maxDelta, diff.maxDelta);
      await fs.writeFile(path.join(outDir, `equivalence-${turns}-${y}-windowed.png`), shotA);
      await fs.writeFile(path.join(outDir, `equivalence-${turns}-${y}-oracle.png`), shotB);
      failures.push(`${where}: ${diff.any} pixels differ, max channel delta ${diff.maxDelta} (screenshots in ${outDir})`);
    }
    checked += 1;
  }
  failures.push(...await findBand(windowed, oracle, turns));
  failures.push(...await pinning(windowed, turns));
  failures.push(...await feedNavigation(windowed, turns));
  failures.push(...await inAppFind(windowed, turns));
  process.stdout.write(`[typing-lag-browser] equivalence turns=${turns}: ${checked} scroll positions, windowed elements<=${maxElements} mounted turns=${(await transcriptState(windowed)).mountedTurns} vs unwindowed elements=${base.elements} turns=${base.mountedTurns}; pixels differing ${differing.any} (max channel delta ${differing.maxDelta})\n`);
  if (MAX_MOUNTED_ELEMENTS !== null && maxElements > MAX_MOUNTED_ELEMENTS) failures.push(`turns=${turns}: windowed transcript mounted ${maxElements} elements > ${MAX_MOUNTED_ELEMENTS}`);
  await windowed.context().close();
  await oracle.context().close();
  return failures;
}

/// Browser find-in-page cannot be driven headlessly (window.find and text
/// fragments do not reveal hidden="until-found" content in automation), so
/// this checks what the console owns: turns near the window are parked in
/// the DOM with their full text, and the path find takes on a match
/// (beforematch, the attribute removed, the turn scrolled into view) leaves
/// the turn mounted and visible with geometry unchanged.
async function findBand(windowed, oracle, turns) {
  const failures = [];
  for (const page of [windowed, oracle]) {
    await page.evaluate(() => { const body = document.querySelector(".conv__body"); body.scrollTop = (body.scrollHeight - body.clientHeight) * 0.5; });
    await settled(page);
  }
  const parked = await windowed.evaluate(() => [...document.querySelectorAll('.conv__body > [hidden="until-found"][data-chat-turn-index]')]
    .map((turn) => ({ index: turn.dataset.chatTurnIndex, text: turn.textContent })));
  if (parked.length < 10) failures.push(`turns=${turns}: only ${parked.length} turns parked for find-in-page around the window`);
  const texts = await oracle.evaluate((indexes) => Object.fromEntries(indexes.map((index) => [index, document.querySelector(`.conv__body > [data-chat-turn-index="${index}"]`)?.textContent ?? null])), parked.map((turn) => turn.index));
  const missing = parked.filter((turn) => !turn.text || turn.text !== texts[turn.index]);
  if (missing.length) failures.push(`turns=${turns}: ${missing.length} parked turns lack their full text (first: turn ${Number(missing[0].index) + 1})`);
  if (parked.length) {
    const target = parked[0].index;
    const before = await windowed.evaluate((index) => {
      const body = document.querySelector(".conv__body");
      const turn = document.querySelector(`.conv__body > [data-chat-turn-index="${index}"]`);
      const scrollHeight = body.scrollHeight;
      // What the browser does on a find match in a hidden="until-found" turn.
      turn.dispatchEvent(new Event("beforematch", { bubbles: true }));
      turn.removeAttribute("hidden");
      turn.scrollIntoView({ block: "center" });
      return { scrollHeight };
    }, target);
    await settled(windowed); await settled(windowed);
    const after = await windowed.evaluate((index) => {
      const body = document.querySelector(".conv__body");
      const turn = document.querySelector(`.conv__body > [data-chat-turn-index="${index}"]`);
      if (!turn) return null;
      const a = turn.getBoundingClientRect(), b = body.getBoundingClientRect();
      return { scrollHeight: body.scrollHeight, hidden: turn.hasAttribute("hidden"), visible: a.bottom > b.top && a.top < b.bottom };
    }, target);
    if (!after || after.hidden || !after.visible) failures.push(`turns=${turns}: a find match in parked turn ${Number(target) + 1} did not leave it shown in view`);
    else if (Math.abs(after.scrollHeight - before.scrollHeight) > 0.5) failures.push(`turns=${turns}: revealing a parked turn changed the scroll height (${before.scrollHeight} to ${after.scrollHeight})`);
  }
  return failures;
}

/// Find in transcript reaches a turn beyond the find band from the live edge:
/// it is mounted, brought into view and its match highlighted.
async function inAppFind(page, turns) {
  const failures = [];
  await page.evaluate(() => { const body = document.querySelector(".conv__body"); body.scrollTop = body.scrollHeight; });
  await settled(page);
  const needle = "(request 3)";
  const before = await page.evaluate(() => Boolean(document.querySelector('.conv__body > [data-chat-turn-index="3"]:not([hidden])')));
  if (before) failures.push(`turns=${turns}: turn 4 was already shown at the live edge, so the find check proves nothing`);
  await page.click('[data-testid^="chat-find-open:"]');
  await page.fill('[data-testid^="chat-find:"]', needle);
  await settled(page); await settled(page); await settled(page);
  const state = await page.evaluate(() => {
    const body = document.querySelector(".conv__body");
    const turn = document.querySelector('.conv__body > [data-chat-turn-index="3"]');
    const a = turn?.getBoundingClientRect(), b = body.getBoundingClientRect();
    return {
      count: document.querySelector('[data-testid^="chat-find-count:"]')?.textContent ?? "",
      shown: Boolean(turn && !turn.hasAttribute("hidden")),
      inView: Boolean(a && a.bottom > b.top && a.top < b.bottom),
      highlights: globalThis.CSS?.highlights?.get("transcript-find")?.size ?? 0,
    };
  });
  if (state.count !== "1 of 1") failures.push(`turns=${turns}: find in transcript counted "${state.count}" for a unique string`);
  if (!state.shown || !state.inView) failures.push(`turns=${turns}: find in transcript did not bring the match's turn into view`);
  if (state.highlights < 1) failures.push(`turns=${turns}: find in transcript did not highlight the match`);
  await page.keyboard.press("Escape");
  await settled(page);
  const closed = await page.evaluate(() => !document.querySelector('[data-testid^="chat-find:"]') && !(globalThis.CSS?.highlights?.has("transcript-find")));
  if (!closed) failures.push(`turns=${turns}: Escape did not close find and clear its highlight`);
  return failures;
}

/// The WAI-ARIA feed pattern: from the first turn, PageDown moves keyboard
/// focus through every loaded turn in order, mounting each as it goes.
async function feedNavigation(page, turns) {
  const failures = [];
  await page.evaluate(() => { document.querySelector(".conv__body").scrollTop = 0; });
  await settled(page); await settled(page);
  const setup = await page.evaluate(() => {
    const body = document.querySelector(".conv__body");
    const first = document.querySelector('.conv__body > [data-chat-turn-index="0"]');
    if (!first) return null;
    first.focus();
    return { role: body.getAttribute("role"), setsize: Number(first.getAttribute("aria-setsize")), article: first.getAttribute("role") };
  });
  if (!setup) return [`turns=${turns}: the first turn is not mounted at the top of the feed`];
  if (setup.role !== "feed" || setup.article !== "article") failures.push(`turns=${turns}: transcript is not a feed of articles`);
  const total = setup.setsize > 0 ? setup.setsize : turns;
  const visited = [];
  for (let step = 0; step < total; step += 1) {
    if (step > 0) { await page.keyboard.press("PageDown"); await settled(page); }
    visited.push(await page.evaluate(() => Number(document.activeElement?.getAttribute("aria-posinset") ?? 0)));
  }
  const expected = Array.from({ length: total }, (_, i) => i + 1);
  if (JSON.stringify(visited) !== JSON.stringify(expected)) {
    const at = visited.findIndex((value, i) => value !== expected[i]);
    failures.push(`turns=${turns}: feed focus did not reach every turn in order (step ${at + 1}: turn ${visited[at]} instead of ${expected[at]})`);
  }
  await page.evaluate(() => document.activeElement?.blur());
  // Outside the feed's articles the keys keep their normal behaviour: the
  // composer keeps focus, and the transcript body itself scrolls.
  const composer = await page.evaluate(() => {
    const textarea = document.querySelector('textarea[data-testid^="chat-composer"]');
    textarea?.focus();
    return Boolean(textarea) && document.activeElement === textarea;
  });
  if (composer) {
    await page.keyboard.press("PageDown"); await settled(page);
    const kept = await page.evaluate(() => document.activeElement?.matches('textarea[data-testid^="chat-composer"]') ?? false);
    if (!kept) failures.push(`turns=${turns}: PageDown in the composer moved focus into the feed`);
  }
  await page.evaluate(() => { const body = document.querySelector(".conv__body"); body.scrollTop = 0; body.focus(); });
  await settled(page);
  const before = await page.evaluate(() => document.querySelector(".conv__body").scrollTop);
  await page.keyboard.press("PageDown"); await settled(page);
  const after = await page.evaluate(() => ({ top: document.querySelector(".conv__body").scrollTop, onBody: document.activeElement === document.querySelector(".conv__body") }));
  if (!after.onBody || after.top <= before) failures.push(`turns=${turns}: PageDown on the transcript body did not scroll it (scrollTop ${before} to ${after.top}, focus on body ${after.onBody})`);
  await page.evaluate(() => document.activeElement?.blur());
  return failures;
}

/// A selection, keyboard focus and an opened disclosure survive scrolling
/// their turns far out of the window.
async function pinning(page, turns) {
  const failures = [];
  const scrollTo = async (fraction) => {
    await page.evaluate((f) => { const body = document.querySelector(".conv__body"); body.scrollTop = (body.scrollHeight - body.clientHeight) * f; }, fraction);
    await settled(page);
  };
  await scrollTo(0.5);
  const selected = await page.evaluate(() => {
    const turns = [...document.querySelectorAll(".conv__body > [data-chat-turn-index]:not([hidden])")];
    const texts = turns.map((turn) => turn.querySelector(".cc-rich-paragraph, .msg__text"));
    // Three adjacent shown turns, each with text.
    const index = (i) => Number(turns[i].dataset.chatTurnIndex);
    const first = texts.findIndex((text, i) => text && texts[i + 2] && index(i + 2) === index(i) + 2);
    const last = first + 2;
    if (first < 0) return null;
    const range = document.createRange();
    range.setStart(texts[first].firstChild ?? texts[first], 0);
    range.setEnd(texts[last].firstChild ?? texts[last], 1);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    window.__pinnedSelection = { text: getSelection().toString(), turns: last - first + 1 };
    return window.__pinnedSelection;
  });
  if (!selected) failures.push(`turns=${turns}: no selectable text for the pinning check`);
  else {
    await scrollTo(0.02);
    await scrollTo(0.98);
    const kept = await page.evaluate(() => ({ text: getSelection().toString(), connected: Boolean(getSelection().anchorNode?.isConnected) }));
    if (kept.text !== selected.text || !kept.connected) failures.push(`turns=${turns}: a selection across ${selected.turns} turns did not survive scrolling away`);
    await page.evaluate(() => getSelection().removeAllRanges());
  }
  await scrollTo(0.5);
  const focused = await page.evaluate(() => {
    const target = document.querySelector('.conv__body > [data-chat-turn-index]:not([hidden]) [role="button"][tabindex="0"], .conv__body > [data-chat-turn-index]:not([hidden]) button');
    if (!target) return false;
    target.focus();
    window.__pinnedFocus = target;
    return document.activeElement === target;
  });
  if (focused) {
    await scrollTo(0.02);
    await scrollTo(0.98);
    const kept = await page.evaluate(() => document.activeElement === window.__pinnedFocus && window.__pinnedFocus.isConnected);
    if (!kept) failures.push(`turns=${turns}: a focused control lost focus when its turn scrolled away`);
    await page.evaluate(() => document.activeElement?.blur());
  }
  await scrollTo(0.5);
  const opened = await page.evaluate(() => {
    const header = document.querySelector('.conv__body > [data-chat-turn-index]:not([hidden]) [role="button"][aria-expanded="false"]');
    if (!header) return null;
    const row = header.closest("[data-conversation-row-id]")?.dataset.conversationRowId;
    header.click();
    return row ?? null;
  });
  if (opened) {
    await scrollTo(0.02);
    await scrollTo(0.5);
    const still = await page.evaluate((row) => {
      const element = [...document.querySelectorAll("[data-conversation-row-id]")].find((candidate) => candidate.dataset.conversationRowId === row);
      return element ? element.querySelector('[role="button"][aria-expanded="true"]') !== null : null;
    }, opened);
    if (still === false) failures.push(`turns=${turns}: an opened tool call closed after its turn scrolled out and back`);
  }
  // A rail jump to a turn outside the window mounts it and brings it into view.
  await scrollTo(1);
  const jump = await page.evaluate(() => {
    const button = [...document.querySelectorAll('.conv-turn-rail__button[data-testid^="chat-turn-rail:"]')]
      .find((candidate) => /^\d+$/.test(candidate.dataset.testid.split(":").pop()));
    const turn = Number(button?.dataset.testid?.split(":").pop());
    if (!button || !Number.isFinite(turn)) return null;
    const mountedBefore = Boolean(document.querySelector(`.conv__body > [data-chat-turn-index="${turn}"]`));
    button.click();
    return { turn, mountedBefore };
  });
  if (jump && !jump.mountedBefore) {
    await settled(page); await settled(page);
    const inView = await page.evaluate((turn) => {
      const body = document.querySelector(".conv__body");
      const element = document.querySelector(`.conv__body > [data-chat-turn-index="${turn}"]`);
      if (!element) return false;
      const a = element.getBoundingClientRect(), b = body.getBoundingClientRect();
      return a.bottom > b.top && a.top < b.bottom;
    }, jump.turn);
    if (!inView) failures.push(`turns=${turns}: a rail jump to unmounted turn ${jump.turn + 1} did not bring it into view`);
  } else if (!jump) failures.push(`turns=${turns}: no rail tick for the jump check`);
  return failures;
}

async function main() {
  await buildHarness();
  const server = await serve();
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({ headless: !HEADED, args: EQUIVALENCE ? PIXEL_STABLE_TEXT : [] });
  if (EQUIVALENCE) {
    try {
      const failures = [];
      for (const turns of TURNS) failures.push(...await equivalence(browser, baseUrl, turns));
      for (const failure of failures) process.stdout.write(`[typing-lag-browser] FAIL ${failure}\n`);
      if (failures.length) process.exitCode = 1;
      else process.stdout.write("[typing-lag-browser] windowed transcript is equivalent at every position\n");
    } finally {
      await browser.close();
      server.close();
    }
    return;
  }
  const results = [];
  try {
    for (const turns of SESSION ? [0] : TURNS) {
      const result = await measureSize(browser, baseUrl, turns);
      results.push(result);
      for (const [scenario, m] of Object.entries(result.scenarios)) {
        const b = m.breakdown?.perKey;
        process.stdout.write(
          `[typing-lag-browser] turns=${turns} ${scenario.padEnd(9)} elements=${result.dom.elements} mounted=${result.dom.mountedTurns} ` +
            `p50=${fmt(m.latency.p50)} p95=${fmt(m.latency.p95)} max=${fmt(m.latency.max)} ms ` +
            `event-timing>=16ms ${m.eventTimingOver16}/${KEYS} longtasks=${m.longtasks.n} (max ${fmt(m.longtasks.max)} ms)` +
            (m.breakdown ? ` layout-objects<=${m.breakdown.maxLayoutObjects}` : "") +
            (m.work.tokens ? ` tokens=${m.work.tokens} full-derivations/token=${m.work.fullDerivationsPerToken === null ? "n/a" : fmt2(m.work.fullDerivationsPerToken)} presented-entries/token=${m.work.presentedEntriesPerToken === null ? "n/a" : fmt2(m.work.presentedEntriesPerToken)} turn-renders/token=${fmt2(m.work.turnRendersPerToken)} row-renders/token=${fmt2(m.work.rowRendersPerToken)} rect-reads/token=${fmt2(m.work.rectReadsPerToken)} markdown-chars/token=${fmt2(m.work.markdownCharsPerToken)} day-label-formats/token=${fmt2(m.work.dayLabelFormatsPerToken)} active-run-frames/token=${fmt2(m.work.activeRunFramesPerToken)} commits/s=${fmt2(m.work.commitsPerSecond)}${m.breakdown ? ` forced-layouts/token=${fmt2(m.work.forcedLayoutsPerToken)}` : ""}` : ` transcript-renders/key=${fmt2(m.work.transcriptRendersPerKeystroke)}`) +
            (b ? ` | per key: script ${fmt(b.scripting)} style ${fmt(b.style)} layout ${fmt(b.layout)} paint ${fmt(b.paint)} composite ${fmt(b.composite)} other ${fmt(b.other)} ms` : "") +
            "\n",
        );
        if (m.profile) {
          process.stdout.write("  profile self:\n");
          for (const t of m.profile.self) process.stdout.write(`    ${String(t.ms).padStart(6)} ms  ${t.name}\n`);
          process.stdout.write("  hot DOM read callers:\n");
          for (const t of m.profile.callers) process.stdout.write(`    ${String(t.ms).padStart(6)} ms  ${t.name}\n`);
          process.stdout.write("  profile inclusive:\n");
          for (const t of m.profile.inclusive) process.stdout.write(`    ${String(t.ms).padStart(6)} ms  ${t.name}\n`);
        }
        if (m.breakdown) {
          for (const t of m.breakdown.top) process.stdout.write(`    ${t.name.padEnd(52)} ${fmt(t.msPerKey)} ms/key\n`);
          for (const [source, count] of m.breakdown.forcedSources ?? []) process.stdout.write(`    forced layout x${count}: ${source}\n`);
        }
      }
      if (result.errors.length) process.stdout.write(`  page errors: ${result.errors.slice(0, 3).join(" | ")}\n`);
    }
  } finally {
    await browser.close();
    server.close();
  }
  if (JSON_OUT) await fs.writeFile(JSON_OUT, JSON.stringify(results, null, 2));
  const failures = [];
  const advisories = [];
  for (const r of results) {
    if (r.errors.length) failures.push(`turns=${r.turns}: page errors ${r.errors.join(" | ")}`);
    for (const [scenario, m] of Object.entries(r.scenarios)) {
      if (!m.typedOk) failures.push(`turns=${r.turns} ${scenario}: keystrokes were lost`);
      if (MAX_LAYOUT_OBJECTS !== null && scenario === "idle") {
        if (!m.breakdown) failures.push(`turns=${r.turns}: --max-layout-objects needs --trace`);
        else if (m.breakdown.maxLayoutObjects > MAX_LAYOUT_OBJECTS) {
          failures.push(`turns=${r.turns} idle: a keystroke laid out ${m.breakdown.maxLayoutObjects} layout objects > ${MAX_LAYOUT_OBJECTS}; typing is re-laying out the transcript`);
        }
      }
      const streamingScenario = scenario === "streaming" || scenario === "send-streaming";
      if (streamingScenario && MAX_MARKDOWN_CHARS_PER_TOKEN !== null && m.work.markdownCharsPerToken > MAX_MARKDOWN_CHARS_PER_TOKEN) {
        failures.push(`turns=${r.turns} ${scenario}: ${fmt2(m.work.markdownCharsPerToken)} Markdown source characters parsed per token > ${MAX_MARKDOWN_CHARS_PER_TOKEN}; the streaming reply re-parses closed blocks`);
      }
      if (streamingScenario && MAX_DAY_LABEL_FORMATS_PER_TOKEN !== null && m.work.dayLabelFormatsPerToken > MAX_DAY_LABEL_FORMATS_PER_TOKEN) {
        failures.push(`turns=${r.turns} ${scenario}: ${fmt2(m.work.dayLabelFormatsPerToken)} day-label formats per token > ${MAX_DAY_LABEL_FORMATS_PER_TOKEN}; streamed renders format their day separators again`);
      }
      if (streamingScenario && MAX_STREAM_COMMITS_PER_SECOND !== null && m.work.commitsPerSecond > MAX_STREAM_COMMITS_PER_SECOND) {
        failures.push(`turns=${r.turns} ${scenario}: ${fmt2(m.work.commitsPerSecond)} console renders per second while streaming > ${MAX_STREAM_COMMITS_PER_SECOND}; streamed text is rendering on every token`);
      }
      if (scenario === "idle" && MAX_MOUNTED_ELEMENTS !== null && r.dom.elements > MAX_MOUNTED_ELEMENTS) {
        failures.push(`turns=${r.turns}: ${r.dom.elements} elements mounted > ${MAX_MOUNTED_ELEMENTS}; the transcript window is not bounding the DOM`);
      }
      if (streamingScenario && MAX_FORCED_LAYOUTS_PER_TOKEN !== null) {
        if (!m.breakdown) failures.push(`turns=${r.turns} ${scenario}: --max-forced-layouts-per-token needs --trace`);
        else if (m.work.forcedLayoutsPerToken > MAX_FORCED_LAYOUTS_PER_TOKEN) {
          failures.push(`turns=${r.turns} ${scenario}: ${fmt2(m.work.forcedLayoutsPerToken)} script-forced layouts per token > ${MAX_FORCED_LAYOUTS_PER_TOKEN}; a streamed token reads geometry with layout dirty (rerun with --trace-invalidations for the source)`);
        }
      }
      if (streamingScenario && MAX_RECT_READS_PER_TOKEN !== null) {
        if (m.work.tokens < 20) failures.push(`turns=${r.turns} ${scenario}: only ${m.work.tokens} tokens streamed`);
        else if (m.work.rectReadsPerToken > MAX_RECT_READS_PER_TOKEN) {
          failures.push(`turns=${r.turns} ${scenario}: ${fmt2(m.work.rectReadsPerToken)} element rect reads per token > ${MAX_RECT_READS_PER_TOKEN}; each streamed token is measuring the transcript`);
        }
      }
      if (streamingScenario && (MAX_FULL_DERIVATIONS_PER_TOKEN !== null || MAX_TURN_RENDERS_PER_TOKEN !== null || MAX_PRESENTED_ENTRIES_PER_TOKEN !== null)) {
        if (m.work.tokens < 20) failures.push(`turns=${r.turns} ${scenario}: only ${m.work.tokens} tokens streamed`);
        if (MAX_FULL_DERIVATIONS_PER_TOKEN !== null) {
          if (m.work.fullDerivationsPerToken === null) failures.push(`turns=${r.turns} ${scenario}: derivation counters unavailable`);
          else if (m.work.fullDerivationsPerToken > MAX_FULL_DERIVATIONS_PER_TOKEN) {
            failures.push(`turns=${r.turns} ${scenario}: ${fmt2(m.work.fullDerivationsPerToken)} full transcript derivations per token > ${MAX_FULL_DERIVATIONS_PER_TOKEN}; streamed text is re-deriving the whole log`);
          }
        }
        if (MAX_PRESENTED_ENTRIES_PER_TOKEN !== null) {
          if (m.work.presentedEntriesPerToken === null) failures.push(`turns=${r.turns} ${scenario}: presented-entry counters unavailable`);
          else if (m.work.presentedEntriesPerToken > MAX_PRESENTED_ENTRIES_PER_TOKEN) {
            failures.push(`turns=${r.turns} ${scenario}: ${fmt2(m.work.presentedEntriesPerToken)} transcript entries presented per token > ${MAX_PRESENTED_ENTRIES_PER_TOKEN}; streamed text is presenting the whole transcript again`);
          }
        }
        if (MAX_TURN_RENDERS_PER_TOKEN !== null && m.work.turnRendersPerToken > MAX_TURN_RENDERS_PER_TOKEN) {
          failures.push(`turns=${r.turns} ${scenario}: ${fmt2(m.work.turnRendersPerToken)} turn renders per token > ${MAX_TURN_RENDERS_PER_TOKEN}; unchanged turns are re-rendering`);
        }
      }
      if (BUDGET_P95 !== null) {
        const budget = budgetFor(scenario);
        if (!(m.latency.p95 <= budget)) {
          (ENFORCE_TIMING ? failures : advisories).push(`turns=${r.turns} ${scenario}: p95 ${fmt(m.latency.p95)} ms > budget ${budget} ms`);
        }
      }
    }
  }
  for (const a of advisories) process.stdout.write(`[typing-lag-browser] ADVISORY (timing, not enforced) ${a}\n`);
  if (failures.length) {
    for (const f of failures) process.stderr.write(`[typing-lag-browser] FAIL ${f}\n`);
    process.exit(1);
  }
  process.stdout.write(`[typing-lag-browser] structural limits hold; timing ${ENFORCE_TIMING ? "enforced" : "advisory"} (p95 ${JSON.stringify(BUDGET_P95)})\n`);
}

if (require.main === module) {
  main().catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}

module.exports = { analyzeTrace, installProbe };
