import { MeerkatClient } from "../src/index.js";
import type { ToolCallDedupeKey } from "../src/index.js";

// A callback handler may take the call's dedupe key as an optional second
// argument; a one-argument handler stays valid. Checked with `tsc --noEmit`;
// unrun until the candidate's validation.
const client = new MeerkatClient();
client.registerTool("plain", "Plain", { type: "object" }, async (args) => String(args.q));
client.registerTool(
  "keyed",
  "Keyed",
  { type: "object" },
  async (_args, dedupeKey?: ToolCallDedupeKey) => dedupeKey?.call_id ?? "no key",
);

const key: ToolCallDedupeKey = {
  run_id: "0b9f3c55-6b8e-4f5c-9a51-2a7d1c4e8f10",
  round: 0,
  call_id: "call-1",
  call_digest: "a".repeat(64),
};
// @ts-expect-error The key is read-only.
key.round = 1;
const parsed: ToolCallDedupeKey | undefined = MeerkatClient.parseToolCallDedupeKey(key);

void [key, parsed];
