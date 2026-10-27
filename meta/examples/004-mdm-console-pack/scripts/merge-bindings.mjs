#!/usr/bin/env node
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const [, , outputArg, ...inputArgs] = process.argv;
if (!outputArg || inputArgs.length === 0) {
  console.error("Usage: merge-bindings.mjs <target-bindings.json> <binding.json>...");
  process.exit(2);
}

const output = resolve(outputArg);
const rawCurrent = existsSync(output) ? readFileSync(output, "utf8").trim() : "";
const current = existsSync(output)
  ? rawCurrent
    ? JSON.parse(rawCurrent)
    : []
  : [];
if (!Array.isArray(current)) {
  throw new Error(`${output} must contain a JSON array`);
}

const byId = new Map(current.map((entry) => [String(entry.id), entry]));
for (const inputArg of inputArgs) {
  const input = resolve(inputArg);
  const entry = JSON.parse(readFileSync(input, "utf8"));
  if (!entry || typeof entry !== "object" || !entry.id) {
    throw new Error(`${input} must contain one binding object with id`);
  }
  byId.set(String(entry.id), entry);
}

mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify([...byId.values()], null, 2)}\n`);
console.log(output);
