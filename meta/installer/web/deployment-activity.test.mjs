import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const bundled = await build({ entryPoints: [fileURLToPath(new URL("deployment-activity.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
  plugins: [{ name: "shared-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
const { DeploymentActivity } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);
const progress = length => Array.from({ length }, (_, index) => ({ at: "2026-10-17T08:00:00Z", message: `Update ${index + 1}` }));
const receipt = entries => ({ guide_id: "synthetic-guide", deployment_id: "synthetic-installation", status: "starting", progress: entries });
function inspect(value, check, active = true) {
  const dom = new JSDOM(renderToStaticMarkup(React.createElement(DeploymentActivity, { receipt: value, active })));
  try { check(dom.window.document); } finally { dom.window.close(); }
}

test("long activity exposes the newest five first and keeps complete earlier history collapsed", () => {
  const value = receipt(progress(100)), before = structuredClone(value);
  inspect(value, document => {
    const section = document.querySelector("details"), latest = section.querySelector(":scope > ol");
    assert.equal(section.open, true);
    assert.equal(latest.reversed, true);
    assert.equal(latest.start, 100);
    assert.deepEqual([...latest.children].map(row => row.textContent), ["Update 100", "Update 99", "Update 98", "Update 97", "Update 96"]);
    const earlier = section.querySelector(":scope > details");
    assert.equal(earlier.open, false);
    assert.equal(earlier.querySelector("summary").textContent, "Earlier activity (95 updates)");
    assert.equal(earlier.querySelectorAll("li").length, 95);
    assert.equal(earlier.querySelector("li").textContent, "Update 1");
    assert.equal(earlier.querySelector("li:last-child").textContent, "Update 95");
    assert.equal(new Set([...section.querySelectorAll("li")].map(row => row.textContent)).size, 100);
    assert.match(section.textContent, /Latest 5 of 100 saved updates/);
  });
  assert.deepEqual(value, before, "Presentation cannot reorder or truncate the retained receipt.");
});

test("small histories stay intact and inactive or running installations retain the collapsed default", () => {
  for (const length of [1, 5, 6]) inspect(receipt(progress(length)), document => {
    const section = document.querySelector("details");
    assert.equal(section.querySelectorAll("li").length, length);
    assert.equal(section.querySelectorAll(":scope > ol > li").length, Math.min(length, 5));
    const earlier = section.querySelector(":scope > details");
    assert.equal(!!earlier, length > 5);
    if (earlier) assert.equal(earlier.querySelector("summary").textContent, "Earlier activity (1 update)");
  });
  inspect(receipt(progress(2)), document => assert.equal(document.querySelector("details").open, false), false);
  inspect({ ...receipt(progress(2)), status: "running" }, document => assert.equal(document.querySelector("details").open, false));
});

test("absent activity stays absent and progress text is rendered as text", () => {
  for (const value of [null, {}, receipt([]), receipt("unavailable")])
    assert.equal(renderToStaticMarkup(React.createElement(DeploymentActivity, { receipt: value })), "");
  inspect(receipt([{ message: '<script>alert("not executable")</script>' }]), document => {
    assert.equal(document.querySelector("script") === null, true);
    assert.equal(document.querySelector("li").textContent, '<script>alert("not executable")</script>');
  });
});
