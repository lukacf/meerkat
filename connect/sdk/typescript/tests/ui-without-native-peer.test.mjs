import test from "node:test";
import assert from "node:assert/strict";
import { cp, mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

test("the actual UI package declaration closure typechecks without the optional native peer", async t => {
  const root = await mkdtemp(join(tmpdir(), "toolkit-ui-peer-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const packageRoot = join(root, "node_modules/@rkat/toolkit");
  await mkdir(packageRoot, { recursive: true });
  // This is the current package's complete compiled tree, not hand-authored
  // replacement declarations. Its optional peer is intentionally absent.
  await cp(resolve("dist"), join(packageRoot, "dist"), { recursive: true });
  await cp(resolve("package.json"), join(packageRoot, "package.json"));
  await writeFile(join(root, "package.json"), '{"type":"module"}\n');
  await writeFile(join(root, "consumer.ts"), `
import { mountToolkitView, mountSetupConversation } from "@rkat/toolkit/ui";
import type { SetupConversationPort } from "@rkat/toolkit/ui";
declare const node: HTMLElement;
declare const port: SetupConversationPort;
mountToolkitView(node, { scope: "host", dispatch: async () => {} });
mountSetupConversation(node, { scope: "host", conversation: port });
`);
  const result = spawnSync(process.execPath, [resolve("node_modules/typescript/bin/tsc"),
    "--noEmit", "--strict", "--target", "ES2022", "--module", "Node16", "--moduleResolution", "Node16", "consumer.ts"],
  { cwd: root, encoding: "utf8", timeout: 20000 });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  await assert.rejects(readFile(join(root, "node_modules/@rkat/sdk/package.json")), { code: "ENOENT" });
});
