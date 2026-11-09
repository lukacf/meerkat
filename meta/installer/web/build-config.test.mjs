import assert from "node:assert/strict";
import { realpath } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { forgeBuildOptions } from "./build-config.mjs";

test("the complete Forge and native Console graph shares one React installation", async () => {
  const result = await build({
    ...forgeBuildOptions(),
    write: false,
    metafile: true,
    sourcemap: false,
    logLevel: "silent",
  });
  const inputs = Object.keys(result.metafile.inputs).map((input) => path.resolve(input));
  assert.ok(inputs.some((input) => input.endsWith("packages/console-components/src/conversation/conversation-pane.tsx")),
    "check the actual shared Console chat, not a substitute entry point");
  const packageRoot = await realpath(fileURLToPath(new URL("node_modules/", import.meta.url)));
  for (const input of inputs.filter((input) => input.replaceAll("\\", "/").includes("/node_modules/"))) {
    assert.ok((await realpath(input)).startsWith(packageRoot + path.sep),
      `browser dependency resolved outside the declared Forge install: ${input}`);
  }
  for (const name of ["react", "react-dom"]) {
    const files = inputs.filter((input) => input.replaceAll("\\", "/").includes(`/node_modules/${name}/`));
    assert.ok(files.length, `${name} is present in the browser graph`);
    for (const file of files) {
      const resolved = await realpath(path.resolve(file));
      assert.ok(resolved.startsWith(path.join(packageRoot, name) + path.sep),
        `${name} resolved outside the Forge installation: ${resolved}`);
    }
  }
});
