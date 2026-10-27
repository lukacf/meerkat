import test from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, cp, mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const sourceWeb = fileURLToPath(new URL(".", import.meta.url));
const sourceRepository = fileURLToPath(new URL("../../", import.meta.url));
const sourceAsset = fileURLToPath(new URL("../assets/meerkat-transparent.png", import.meta.url));
const sourceLimits = fileURLToPath(new URL("../launchkit/blueprint-limits.json", import.meta.url));
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

test("actual frontend build handles a relocated checkout with spaces and Unicode from another cwd", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "launch-build-path-"));
  try {
    const checkout = join(temporary, "Launch Projects - café 测试");
    const web = join(checkout, "installer", "web");
    const assets = join(checkout, "installer", "assets");
    const launchkit = join(checkout, "installer", "launchkit");
    const unrelated = join(temporary, "unrelated working directory");
    await mkdir(web, { recursive: true });
    await mkdir(assets, { recursive: true });
    await mkdir(launchkit, { recursive: true });
    await mkdir(unrelated);

    // Copy only the public flat web sources, not output, dependencies or state.
    for (const entry of await readdir(sourceWeb, { withFileTypes: true })) {
      if (!entry.isFile() || entry.name.startsWith(".") || entry.name.endsWith(".test.mjs")) continue;
      const publicSource = [".jsx", ".mjs", ".css", ".html"].includes(extname(entry.name));
      if (publicSource || ["package.json", "package-lock.json"].includes(entry.name)) {
        await copyFile(join(sourceWeb, entry.name), join(web, entry.name));
      }
    }
    // Forge's own fonts and their licenses are public inputs in one directory.
    await cp(join(sourceWeb, "fonts"), join(web, "fonts"), { recursive: true });
    await copyFile(sourceAsset, join(assets, "meerkat-transparent.png"));
    // state.mjs imports this known public planning input outside the flat web tree.
    await copyFile(sourceLimits, join(launchkit, "blueprint-limits.json"));
    // The existing dependencies and shared components are read-only build inputs.
    await symlink(join(sourceWeb, "node_modules"), join(web, "node_modules"), "dir");
    await symlink(join(sourceRepository, "console"), join(checkout, "console"), "dir");
    await symlink(join(sourceRepository, "packages"), join(checkout, "packages"), "dir");

    const script = join(web, "build.mjs");
    assert.deepEqual(await readFile(script), await readFile(join(sourceWeb, "build.mjs")));
    let result;
    try {
      result = await execute(process.execPath, [script], {
        cwd: unrelated,
        env: { PATH: process.env.PATH || "" },
        timeout: 60_000,
        maxBuffer: 1024 * 1024,
        encoding: "utf8",
      });
    } catch (error) {
      assert.fail(`Actual relocated build failed (${error.code || error.signal || "unknown"}).\n${String(error.stderr || error.message).slice(-6000)}`);
    }
    const scriptRoot = await realpath(web);
    assert.ok(result.stdout.includes(`Built Meta Forge at ${scriptRoot}/dist/`));

    const output = join(web, "dist");
    assert.deepEqual(await readFile(join(output, "index.html")), await readFile(join(sourceWeb, "index.html")));
    assert.deepEqual(await readFile(join(output, "assets", "meerkat-transparent.png")), await readFile(sourceAsset));
    const javascript = await readFile(join(output, "app.js"), "utf8");
    const css = await readFile(join(output, "app.css"), "utf8");
    assert.ok(javascript.length > 1000, "the real App bundle was emitted");
    assert.ok(css.length > 1000, "the real stylesheet bundle was emitted");
    // The page names only fonts the build emitted beside it, none elsewhere.
    const fonts = [...new Set([...css.matchAll(/url\("\.\/(fonts\/[^"]+\.woff2)"\)/g)].map((match) => match[1]))];
    assert.ok(fonts.length > 0, "the stylesheet names Forge's own fonts");
    for (const font of fonts) assert.equal((await stat(join(output, font))).isFile(), true, `${font} was emitted`);
    for (const page of [css, await readFile(join(output, "index.html"), "utf8")]) {
      assert.doesNotMatch(page, /fonts\.(googleapis|gstatic)\.com/, "the page loads no font from another origin");
    }
    for (const name of ["app.js.map", "app.css.map"]) {
      const map = JSON.parse(await readFile(join(output, name), "utf8"));
      assert.ok(map.sources.length > 0);
      assert.equal(map.sources.length, map.sourcesContent.length);
      assert.ok(map.sourcesContent.every((content) => typeof content === "string"));
    }

    const rendererHash = digest(await readFile(join(web, "freeform-animation.mjs")));
    const provenance = await readFile(join(web, "animation-review-renderer-hash.mjs"), "utf8");
    assert.equal(provenance, `// Generated by build.mjs from the renderer bytes in this build.\nexport const ANIMATION_RENDERER_HASH='${rendererHash}';\n`);
    assert.ok(javascript.includes(rendererHash), "the actual bundle carries the renderer provenance");
    assert.equal((await stat(output)).isDirectory(), true);
    assert.deepEqual(await readdir(unrelated), [], "the build is anchored to its script, not cwd");
    assert.deepEqual((await readdir(temporary)).sort(), ["Launch Projects - café 测试", "unrelated working directory"].sort(),
      "URL-encoded sibling output directories were not created");
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});
