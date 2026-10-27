import { build } from "esbuild";
import { createRequire } from "node:module";
import { mkdtemp, writeFile } from "node:fs/promises";
import { rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const require = createRequire(new URL("../package.json", import.meta.url));

// Share React with the test renderer, including components from sibling sources.
// A file URL keeps failure stacks readable instead of printing an entire bundle.
export async function loadTestModule(relativePath, options = {}) {
  const result = await build({
    entryPoints: [fileURLToPath(new URL("../" + relativePath, import.meta.url))],
    bundle: true, write: false, format: "esm", platform: "node", jsx: "automatic",
    logLevel: "silent", ...options,
    plugins: [...(options.plugins || []), { name: "test-renderer-react", setup(builder) {
      builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, ({ path }) => ({
        path: pathToFileURL(require.resolve(path)).href, external: true,
      }));
    } }],
  });
  const directory = await mkdtemp(join(tmpdir(), "forge-test-module-"));
  process.once("exit", () => rmSync(directory, { recursive: true, force: true }));
  const filename = join(directory, "module.mjs");
  await writeFile(filename, result.outputFiles[0].contents);
  return import(pathToFileURL(filename).href);
}
