#!/usr/bin/env node
"use strict";

const path = require("node:path");
const ts = require("typescript");
const baseline = require("./typecheck-contracts-baseline.json");
const repoRoot = path.resolve(__dirname, "..");
const configPath = path.join(__dirname, "tsconfig.contracts.json");
const loaded = ts.readConfigFile(configPath, ts.sys.readFile);
const parsed = ts.parseJsonConfigFileContent(loaded.config, ts.sys, __dirname, undefined, configPath);
const program = ts.createProgram(parsed.fileNames, parsed.options);
const diagnostics = [...(loaded.error ? [loaded.error] : []), ...parsed.errors, ...ts.getPreEmitDiagnostics(program)];
const signature = diagnostic => JSON.stringify({ file: diagnostic.file ? path.relative(repoRoot, diagnostic.file.fileName).split(path.sep).join("/") : null, code: diagnostic.code, message: ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n") });
// Explicit pre-existing diagnostic debt only. No diagnostic is ignored by code,
// directory or line. Counted exact signatures prevent an extra instance hiding.
const allowed = new Map(baseline.diagnostics.map(item => [JSON.stringify({ file: item.file, code: item.code, message: item.message }), item.count]));
const unexpected = [];
let known = 0;
for (const diagnostic of diagnostics) {
  const key = signature(diagnostic);
  const count = allowed.get(key) || 0;
  if (count > 0) { allowed.set(key, count - 1); known += 1; }
  else unexpected.push(diagnostic);
}
const host = { getCanonicalFileName: file => file, getCurrentDirectory: () => repoRoot, getNewLine: () => "\n" };
if (unexpected.length) process.stderr.write(ts.formatDiagnosticsWithColorAndContext(unexpected, host));
const removed = [...allowed.values()].reduce((sum, count) => sum + count, 0);
process.stdout.write(`TypeScript ${ts.version}: public core/components, stock entrypoint and shared host contracts; ${unexpected.length} new errors, ${known} documented pre-existing diagnostics${removed ? `, ${removed} baseline diagnostics no longer present` : ""}.\n`);
process.exitCode = unexpected.length ? 1 : 0;
