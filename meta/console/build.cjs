#!/usr/bin/env node

const fs = require("node:fs/promises");
const path = require("node:path");
const crypto = require("node:crypto");
const { build } = require("esbuild");
const { EMBEDDED_SHARED_FILES, EMBEDDED_SOURCE_FILES } = require("./generated-assets.cjs");

const outDir = path.join(__dirname, "dist");
const embeddedOutDir = path.join(__dirname, "../crates/meerkat-meta/console-dist");
const indexSourcePath = path.join(__dirname, "src/index.tsx");
const browserSourcePath = path.join(__dirname, "src/browser.tsx");
const libraryBundlePath = path.join(outDir, "index.cjs");
const appBundlePath = path.join(outDir, "console-app.js");
const htmlPath = path.join(outDir, "index.html");

// Resolve shared packages from local workspace
const alias = {
  "@console-core": path.resolve(__dirname, "../packages/console-core/src/index.ts"),
  "@console-components": path.resolve(__dirname, "../packages/console-components/src/index.ts"),
  "@console-components/styles": path.resolve(__dirname, "../packages/console-components/src/styles/index.ts"),
};

// Shared packages import clsx etc. — resolve from console/node_modules
const nodePaths = [path.resolve(__dirname, "node_modules")];

const bundleSyntaxOptions = {
  jsx: "automatic",
  // Emit escaped strings so file-hygiene hooks cannot trim whitespace inside
  // vendor template literals. Compiler lowering preserves their string values.
  supported: { "template-literal": false },
};

// console-host.css pulls in the console's own fonts (src/fonts). Inlined as
// data: URLs they travel inside the stylesheet, so every host that serves the
// stylesheet serves the fonts too and no page requests a font elsewhere.
const fontLoaders = { ".woff2": "dataurl" };

function assetVersion(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex").slice(0, 12);
}

function renderHtml({ jsVersion, cssVersion }) {
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="meta-base-url" content="" />
    <title>Meta Console</title>
    <link rel="stylesheet" href="/console/assets/console-app.css?v=${cssVersion}" />
  </head>
  <body>
    <div id="root"></div>
    <script src="/console/assets/console-app.js?v=${jsVersion}" defer></script>
  </body>
</html>
`;
}

async function main() {
  await fs.mkdir(outDir, { recursive: true });
  await fs.mkdir(embeddedOutDir, { recursive: true });

  // Library bundle (CJS) for JSDOM / smoke tests
  await build({
    absWorkingDir: __dirname,
    entryPoints: [indexSourcePath],
    outfile: libraryBundlePath,
    bundle: true,
    format: "cjs",
    platform: "neutral",
    conditions: ["browser"],
    mainFields: ["module", "main"],
    target: ["es2020"],
    external: ["react", "react-dom", "react-dom/client", "react/jsx-runtime", "react/jsx-dev-runtime"],
    alias,
    nodePaths,
    loader: fontLoaders,
    ...bundleSyntaxOptions,
    minify: false,
  });

  // Browser app bundle (IIFE) served by the gateway
  await build({
    absWorkingDir: __dirname,
    entryPoints: [browserSourcePath],
    outfile: appBundlePath,
    bundle: true,
    format: "iife",
    platform: "browser",
    target: ["es2020"],
    define: {
      "process.env.NODE_ENV": '"production"',
      NODE_ENV: '"production"',
    },
    alias,
    nodePaths,
    loader: fontLoaders,
    ...bundleSyntaxOptions,
    keepNames: true,
    minify: true,
  });

  const [appBundle, cssBundle] = await Promise.all([
    fs.readFile(appBundlePath),
    fs.readFile(path.join(outDir, "console-app.css")),
  ]);
  await fs.writeFile(
    htmlPath,
    renderHtml({
      jsVersion: assetVersion(appBundle),
      cssVersion: assetVersion(cssBundle),
    }),
    "utf8",
  );

  await Promise.all(
    EMBEDDED_SHARED_FILES.map((file) => (
      fs.copyFile(path.join(outDir, file), path.join(embeddedOutDir, file))
    )),
  );

  // The MCP Apps sandbox proxy ships inside the gateway binaries, which write
  // it back out with --write-mcp-apps-sandbox <dir>.
  await Promise.all(
    EMBEDDED_SOURCE_FILES.map(({ source, embedded }) => (
      fs.copyFile(path.join(__dirname, source), path.join(embeddedOutDir, embedded))
    )),
  );
}

module.exports = { bundleSyntaxOptions, fontLoaders };

if (require.main === module) {
  main().catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}
