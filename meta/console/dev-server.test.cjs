// The console's dev server (`npm run dev`, vite.config.ts): it pre-bundles
// its dependencies with the installed packages, and it forwards to the
// gateway every backend route the console calls.

const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const { buildSync } = require("esbuild");

const configFile = path.join(__dirname, "vite.config.ts");

// The backend routes the console calls, from its contract constants. A path
// template's parameters get a sample value.
function contractRoutes() {
  const routes = new Set();
  for (const file of ["../packages/console-core/src/contract.ts", "src/lib/contract.ts"]) {
    const { outputFiles } = buildSync({
      entryPoints: [path.join(__dirname, file)],
      bundle: true,
      format: "cjs",
      platform: "node",
      write: false,
    });
    const module = { exports: {} };
    vm.runInNewContext(outputFiles[0].text, { module, exports: module.exports });
    const { CONSOLE_REST_PATHS, CONSOLE_RPC_PATHS, CONSOLE_BLOB_PATH_PREFIX } = module.exports;
    for (const route of [
      ...Object.values(CONSOLE_REST_PATHS),
      ...Object.values(CONSOLE_RPC_PATHS),
      `${CONSOLE_BLOB_PATH_PREFIX}example`,
    ]) {
      routes.add(route.replace(/\{[^}]+\}/g, "example"));
    }
  }
  return [...routes];
}

// Where the dev server sends a request for `url`: the target of the first
// proxy key the URL starts with (a key starting with "^" is a regular
// expression), as Vite matches them, or null when the dev server serves it.
function proxyTarget(proxy, url) {
  for (const [context, options] of Object.entries(proxy)) {
    const matches = context.startsWith("^") ? new RegExp(context).test(url) : url.startsWith(context);
    if (matches) return typeof options === "string" ? options : options.target;
  }
  return null;
}

test("npm run dev pre-bundles the MCP client and core with their sub-path imports", async () => {
  const { optimizeDeps, resolveConfig } = await import("vite");
  const config = await resolveConfig({
    root: __dirname,
    configFile,
    cacheDir: path.join(__dirname, ".tmp", "dev-server-deps"),
    logLevel: "silent",
  }, "serve");
  // The same pre-bundle the dev server runs at startup.
  const { optimized } = await optimizeDeps(config, true);
  for (const dependency of ["@modelcontextprotocol/client", "@modelcontextprotocol/core"]) {
    assert.ok(dependency in optimized, `${dependency} is not pre-bundled: ${Object.keys(optimized).join(", ")}`);
  }
});

test("npm run dev forwards every backend route the console calls to the gateway", async () => {
  const { loadConfigFromFile } = await import("vite");
  const loaded = await loadConfigFromFile({ command: "serve", mode: "development" }, configFile, __dirname, "silent");
  const proxy = loaded?.config.server?.proxy ?? {};
  const gateway = proxyTarget(proxy, "/console/experience");
  assert.ok(gateway, "npm run dev forwards no console route");
  assert.deepEqual(
    contractRoutes().filter((route) => proxyTarget(proxy, route) !== gateway),
    [],
    "backend routes npm run dev does not forward to the gateway",
  );
  // The console's own page and modules stay with the dev server.
  for (const own of ["/", "/index.html", "/src/browser.tsx", "/@vite/client"]) {
    assert.equal(proxyTarget(proxy, own), null, `npm run dev must serve ${own} itself`);
  }
});
