import { execFileSync } from "node:child_process";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { appGallerySections, flattenGallerySections } from "./app-gallery";

// The console app's kit gallery sections (console v2 spec B23, K1): found
// under console/src in dev, and nowhere in the production bundle.

const consoleDir = path.resolve(__dirname, "../..");

// Words only the gallery code holds: the URL switch, the page's test id and
// title, and the app sections' loader.
const GALLERY_MARKERS = ["kit_gallery", "kit-gallery", "Console kit", "appGallerySections"];

// Bundles src/browser.tsx as build.cjs does (its esbuild options, the given
// NODE_ENV) in a plain Node process (esbuild refuses jsdom's globals) and
// prints the markers the bundle holds.
const BUNDLE_SCRIPT = `
const path = require("node:path");
const { build } = require("esbuild");
const { bundleSyntaxOptions, fontLoaders } = require("./build.cjs");
const [nodeEnv, markers] = [process.argv[1], JSON.parse(process.argv[2])];
build({
  absWorkingDir: process.cwd(),
  entryPoints: [path.join(process.cwd(), "src/browser.tsx")],
  outfile: path.join(process.cwd(), ".tmp/app-gallery-probe/console-app.js"),
  write: false,
  bundle: true,
  format: "iife",
  platform: "browser",
  target: ["es2020"],
  define: { "process.env.NODE_ENV": JSON.stringify(nodeEnv), NODE_ENV: JSON.stringify(nodeEnv) },
  alias: {
    "@console-core": path.resolve("../packages/console-core/src/index.ts"),
    "@console-components": path.resolve("../packages/console-components/src/index.ts"),
    "@console-components/styles": path.resolve("../packages/console-components/src/styles/index.ts"),
  },
  nodePaths: [path.resolve("node_modules")],
  loader: fontLoaders,
  ...bundleSyntaxOptions,
  keepNames: true,
  minify: true,
  logLevel: "silent",
}).then((result) => {
  const js = result.outputFiles.filter((file) => file.path.endsWith(".js")).map((file) => file.text).join("\\n");
  process.stdout.write(JSON.stringify({ bytes: js.length, markers: markers.filter((marker) => js.includes(marker)) }));
}, (error) => {
  process.stderr.write(String(error && error.stack || error));
  process.exit(1);
});
`;

function bundleMarkers(nodeEnv: "production" | "development"): { bytes: number; markers: string[] } {
  const out = execFileSync(process.execPath, ["-e", BUNDLE_SCRIPT, nodeEnv, JSON.stringify(GALLERY_MARKERS)], {
    cwd: consoleDir,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  return JSON.parse(out) as { bytes: number; markers: string[] };
}

describe("app gallery sections", () => {
  it("flattens every gallerySections export in path order", () => {
    const render = () => null;
    const sections = flattenGallerySections({
      "../panels/roster.gallery.tsx": { gallerySections: [{ id: "roster-near-limit", title: "Roster", render }] },
      "../panels/memory.gallery.tsx": { gallerySections: [{ id: "memory-near-limit", title: "Memory", render }] },
      "../panels/empty.gallery.tsx": {},
    });
    expect(sections.map((section) => section.id)).toEqual(["memory-near-limit", "roster-near-limit"]);
    expect(Array.isArray(appGallerySections())).toBe(true);
  });

  it("leaves no gallery code in the production bundle", () => {
    const production = bundleMarkers("production");
    expect(production.bytes > 100_000).toBe(true);
    expect(production.markers).toEqual([]);
    // The same bundle in development carries the gallery, so the markers do
    // find it when it is there.
    expect(bundleMarkers("development").markers).toEqual(GALLERY_MARKERS);
  }, 120_000);
});
