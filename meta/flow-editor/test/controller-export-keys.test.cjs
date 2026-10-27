#!/usr/bin/env node
// Migration anchor: window.MetaFlowController must expose EXACTLY the key
// set recorded in controller-export-manifest.json, no matter how much of the
// controller has moved into @flow-editor-core. The JSX views consume the
// facade stringly, so a missing key is a silent runtime failure.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const manifest = JSON.parse(
  fs.readFileSync(path.join(__dirname, "controller-export-manifest.json"), "utf8"),
);
const bundlePath = path.join(__dirname, "..", ".tmp", "controller-under-test.cjs");
assert.ok(
  fs.existsSync(bundlePath),
  "controller-under-test.cjs missing — run `node build.cjs --test-bundle` first",
);

function diff(label, actual, expected) {
  const missing = expected.filter((key) => !actual.includes(key));
  const unexpected = actual.filter((key) => !expected.includes(key));
  assert.deepEqual(
    { missing, unexpected },
    { missing: [], unexpected: [] },
    `${label}: facade key drift`,
  );
}

// With the projection-test flag set, the facade carries the manifest plus the
// test-gated assembler exports.
global.window = { __META_FLOW_CONTROLLER_TEST__: true };
require(bundlePath);
const testKeys = Object.keys(global.window.MetaFlowController).sort();
diff(
  "test-flagged facade",
  testKeys,
  [...manifest.exports, ...manifest.testOnlyExports].sort(),
);

// Without the flag (fresh process), the test-gated exports must stay hidden.
const baseKeys = JSON.parse(
  execFileSync(
    process.execPath,
    [
      "-e",
      `global.window = {}; require(${JSON.stringify(bundlePath)});` +
        "process.stdout.write(JSON.stringify(Object.keys(global.window.MetaFlowController).sort()));",
    ],
    { encoding: "utf8" },
  ),
);
diff("base facade", baseKeys, manifest.exports);

process.stdout.write(
  `controller facade parity ok (${manifest.exports.length} exports, ${manifest.testOnlyExports.length} test-gated)\n`,
);
