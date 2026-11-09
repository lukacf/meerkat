#!/usr/bin/env node

const crypto = require("node:crypto");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { build } = require("esbuild");

const root = __dirname;
const srcDir = path.join(root, "src");
const defaultOutDir = path.join(root, "dist");
const defaultRustOutDir = path.join(root, "../crates/meerkat-meta/flow-editor-dist");
const tmpDir = path.join(root, ".tmp");
const corePackageEntry = path.join(root, "../packages/flow-editor-core/src/index.ts");
const componentsPackageEntry = path.join(root, "../packages/flow-editor-components/src/index.ts");
const appEntry = path.join(srcDir, "app.tsx");
const consoleFontsDir = path.join(root, "../console/src/fonts");

// The designer handoff pins src/tokens.css byte for byte
// (test/handoff-visual-contract.test.cjs), and that file imports Inter Tight
// and IBM Plex Mono from Google Fonts. The shipped stylesheet carries the
// console's own copies of those fonts in that import's place, inlined as
// data: URLs, so the flow editor requests no font from another origin.
const remoteFontsImport =
  '@import url("https://fonts.googleapis.com/css2?family=Inter+Tight:wght@400;500;600;700&family=IBM+Plex+Mono:wght@400;500;600&display=swap");';

async function inlinedConsoleFontsCss() {
  const css = await fs.readFile(path.join(consoleFontsDir, "fonts.css"), "utf8");
  const fontUrl = /url\("\.\/([^"]+\.woff2)"\)/g;
  const files = [...new Set([...css.matchAll(fontUrl)].map((match) => match[1]))];
  if (files.length === 0) throw new Error("console/src/fonts/fonts.css names no woff2 font");
  const dataUrls = new Map(await Promise.all(files.map(async (file) => {
    const bytes = await fs.readFile(path.join(consoleFontsDir, file));
    return [file, `data:font/woff2;base64,${bytes.toString("base64")}`];
  })));
  return css.replace(fontUrl, (_, file) => `url("${dataUrls.get(file)}")`).trim();
}

function withLocalFonts(tokensCss, fontsCss) {
  if (!tokensCss.includes(remoteFontsImport)) {
    throw new Error(
      "flow-editor/src/tokens.css no longer has the Google Fonts import the build replaces with the console's fonts; update build.cjs",
    );
  }
  return tokensCss.replace(remoteFontsImport, () => fontsCss);
}

// S23 end-state: flow-editor.js is a single esbuild bundle of the app.tsx
// shell entry, which imports the views from @flow-editor-components and
// builds the controller facade from @flow-editor-core directly. React and
// ReactDOM are NOT bundled: nothing in the bundle graph imports "react" —
// the classic JSX transform emits free React/ReactDOM identifiers that
// resolve to the window globals react-globals.js provides, exactly like the
// legacy concatenated artifact. Artifact names, routes, and the 4-file
// embedded set never change.

async function buildCoreIife() {
  const result = await build({
    absWorkingDir: root,
    entryPoints: [corePackageEntry],
    bundle: true,
    format: "iife",
    globalName: "MetaFlowCore",
    platform: "neutral",
    target: ["es2020"],
    write: false,
    nodePaths: [path.join(root, "node_modules")],
  });
  const code = result.outputFiles[0].text;
  return `${code}\nwindow.MetaFlowCore = MetaFlowCore;\n`;
}

function hash(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex").slice(0, 12);
}

async function cleanDir(dir) {
  await fs.rm(dir, { force: true, recursive: true });
  await fs.mkdir(dir, { recursive: true });
}

function renderHtml({ vendorVersion, appVersion, cssVersion }) {
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="meta-base-url" content="" />
    <title>Meta · Flow Editor</title>
    <link rel="stylesheet" href="/flow-editor/assets/flow-editor.css?v=${cssVersion}" />
  </head>
  <body>
    <div id="root"></div>
    <script src="/flow-editor/assets/react-globals.js?v=${vendorVersion}" defer></script>
    <script src="/flow-editor/assets/flow-editor.js?v=${appVersion}" defer></script>
  </body>
</html>
`;
}

async function buildAssets({ outDir, rustOutDir }) {
  await cleanDir(outDir);
  await fs.mkdir(rustOutDir, { recursive: true });

  const vendorPath = path.join(outDir, "react-globals.js");
  await build({
    absWorkingDir: root,
    stdin: {
      contents: `
        import * as React from "react";
        import { createRoot } from "react-dom/client";
        window.React = React;
        window.ReactDOM = { createRoot };
      `,
      resolveDir: root,
      sourcefile: "react-globals-entry.js",
      loader: "js",
    },
    outfile: vendorPath,
    bundle: true,
    format: "iife",
    platform: "browser",
    target: ["es2020"],
    define: {
      "process.env.NODE_ENV": '"production"',
    },
    minify: true,
  });

  const appPath = path.join(outDir, "flow-editor.js");
  await build({
    absWorkingDir: root,
    entryPoints: [appEntry],
    outfile: appPath,
    bundle: true,
    format: "iife",
    platform: "browser",
    target: ["es2020"],
    jsxFactory: "React.createElement",
    jsxFragment: "React.Fragment",
    alias: {
      "@flow-editor-core": corePackageEntry,
      "@flow-editor-components": componentsPackageEntry,
    },
    nodePaths: [path.join(root, "node_modules")],
  });
  // React must stay external on the react-globals.js vendor: a value import
  // of react anywhere in the app graph would silently bundle a second copy
  // next to the window-global one. Every bundled React carries the
  // Symbol.for("react.*") registrations; the app bundle must not.
  const appBundleText = await fs.readFile(appPath, "utf8");
  if (/Symbol\.for\(["']react\./.test(appBundleText)) {
    throw new Error(
      "flow-editor.js bundled a React copy; React must resolve from the react-globals.js window vendor (remove the value import of react)",
    );
  }

  const [tokensCss, stylesCss, fontsCss] = await Promise.all([
    fs.readFile(path.join(srcDir, "tokens.css"), "utf8"),
    fs.readFile(path.join(srcDir, "styles.css"), "utf8"),
    inlinedConsoleFontsCss(),
  ]);
  const css = `${withLocalFonts(tokensCss, fontsCss)}\n${stylesCss}`;
  await fs.writeFile(path.join(outDir, "flow-editor.css"), css, "utf8");

  const [vendorBytes, appBytes, cssBytes] = await Promise.all([
    fs.readFile(vendorPath),
    fs.readFile(appPath),
    fs.readFile(path.join(outDir, "flow-editor.css")),
  ]);
  await fs.writeFile(
    path.join(outDir, "index.html"),
    renderHtml({
      vendorVersion: hash(vendorBytes),
      appVersion: hash(appBytes),
      cssVersion: hash(cssBytes),
    }),
    "utf8",
  );

  await Promise.all(
    ["index.html", "react-globals.js", "flow-editor.js", "flow-editor.css"].map((file) =>
      fs.copyFile(path.join(outDir, file), path.join(rustOutDir, file)),
    ),
  );
}

async function assertFresh(generatedDir, expectedDir) {
  const files = ["index.html", "react-globals.js", "flow-editor.js", "flow-editor.css"];
  const mismatches = [];
  for (const file of files) {
    const [generated, expected] = await Promise.all([
      fs.readFile(path.join(generatedDir, file)),
      fs.readFile(path.join(expectedDir, file)).catch(() => null),
    ]);
    if (!expected || !generated.equals(expected)) mismatches.push(file);
  }
  if (mismatches.length) {
    throw new Error(`flow-editor-dist is stale; rebuild with npm --prefix flow-editor run build (${mismatches.join(", ")})`);
  }
}

// The projection suite loads the controller through this Node-requirable
// bundle: the core package (window.MetaFlowCore) followed by a bootstrap
// shim equivalent to the app shell's module-scope facade construction, with
// the suite's window.__META_FLOW_CONTROLLER_TEST__ flag mapped to the
// includeTestExports option so the test-gated assembler exports stay
// reachable only through that explicit flag.
async function buildTestBundle() {
  await fs.mkdir(tmpDir, { recursive: true });
  const coreIife = await buildCoreIife();
  const bootstrap = [
    "window.MetaFlowController = window.MetaFlowCore.createMetaFlowController({",
    "  includeTestExports: !!window.__META_FLOW_CONTROLLER_TEST__,",
    "});",
    "",
  ].join("\n");
  const bundle = `/* generated by build.cjs --test-bundle; do not edit */\n${coreIife}\n${bootstrap}`;
  await fs.writeFile(path.join(tmpDir, "controller-under-test.cjs"), bundle, "utf8");
}

async function main() {
  if (process.argv.includes("--test-bundle")) {
    await buildTestBundle();
    return;
  }
  const checkOnly = process.argv.includes("--check");
  if (!checkOnly) {
    await buildAssets({ outDir: defaultOutDir, rustOutDir: defaultRustOutDir });
    return;
  }
  const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "meta-flow-editor-build-"));
  try {
    const generatedDist = path.join(tempRoot, "dist");
    const generatedRustDist = path.join(tempRoot, "flow-editor-dist");
    await buildAssets({ outDir: generatedDist, rustOutDir: generatedRustDist });
    await assertFresh(generatedRustDist, defaultRustOutDir);
  } finally {
    await fs.rm(tempRoot, { force: true, recursive: true });
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack || error.message}\n`);
  process.exit(1);
});
