// Inert protocol child. It never authenticates, installs, grants or admits work.
import { readSync, writeSync, closeSync } from "node:fs";
function line(fd) {
  const bytes = [], byte = Buffer.alloc(1);
  while (readSync(fd, byte, 0, 1, null)) { if (byte[0] === 10) return Buffer.from(bytes).toString(); bytes.push(byte[0]); }
  throw new Error("expected complete bootstrap command");
}
const bootstrap = JSON.parse(line(3));
if (bootstrap.command.kind !== "bootstrap") throw new Error("expected bootstrap");
writeSync(4, JSON.stringify({ version: 1, id: bootstrap.id, result: { kind: "bootstrap_accepted" } }) + "\n");
process.stdout.write("paused\n");
// Wait on a separate test-only release pipe while the real command pipe fills.
if (line(0) !== "drain") throw new Error("expected drain release");
const chunks = [], chunk = Buffer.alloc(65_536);
let count;
while ((count = readSync(3, chunk, 0, chunk.length, null)) > 0) chunks.push(Buffer.from(chunk.subarray(0, count)));
const frames = Buffer.concat(chunks).toString().split("\n");
for (const frame of frames.slice(0, -1)) {
  const { id, command } = JSON.parse(frame);
  process.stdout.write(JSON.stringify({ id, kind: command.kind, original: command.submission?.original.original }) + "\n");
}
process.stdout.write(JSON.stringify({ trailingBytes: Buffer.byteLength(frames.at(-1)) }) + "\n");
closeSync(3); closeSync(4);
