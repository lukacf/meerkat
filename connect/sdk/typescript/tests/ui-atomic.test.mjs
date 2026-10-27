import test from "node:test";
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";
import { mountToolkitView } from "@rkat/toolkit/ui";
import { fixture } from "../examples/management-fixture.mjs";

// Public component contract only. Host projections and dispatch are synthetic.
function mount(t, dispatch = async () => {}) {
  const dom = new JSDOM("<!doctype html><main></main>");
  const doc = dom.window.document, container = doc.querySelector("main");
  const view = mountToolkitView(container, { scope: "fixture-viewer", dispatch });
  assert.equal(view.update(fixture(1)), true);
  t.after(() => { view.dispose(); dom.window.close(); });
  return { doc, container, view };
}
const buttons = (ui, text) => [...ui.container.querySelectorAll("button")].filter(node => node.textContent === text);
const description = (ui, button) => ui.doc.getElementById(button.getAttribute("aria-describedby")).textContent;
const settle = () => new Promise(resolve => setImmediate(resolve));
function lateMalformed() {
  const next = fixture(2);
  next.tasks[0].title = "Unaccepted replacement";
  next.packages.at(-1).components = null;
  return next;
}
function lateRenderFailure(ui, action) {
  const create = ui.doc.createElement;
  ui.doc.createElement = function(tag, ...args) {
    if (tag === "pre") throw new Error("injected late render failure");
    return create.call(this, tag, ...args);
  };
  try { action(); } finally { ui.doc.createElement = create; }
}
function withLateDiagnostics() {
  const next = fixture(2);
  for (const row of [...next.tasks, ...next.connections]) delete row.diagnostics;
  next.packages.at(-1).diagnostics = "Final rendering step";
  return next;
}

for (const mode of ["malformed DTO", "late DOM failure"]) {
  function reject(ui) {
    if (mode === "malformed DTO") assert.throws(() => ui.view.update(lateMalformed()));
    else lateRenderFailure(ui, () => assert.throws(() => ui.view.update(withLateDiagnostics()), /injected late render failure/));
  }
  test(`${mode} preserves accepted DOM, focus, navigation and control revision`, async t => {
    const calls = [], ui = mount(t, async request => { calls.push(request); });
    const oldButton = buttons(ui, "Cancel whole task")[0]; oldButton.focus();
    const html = ui.container.innerHTML;
    reject(ui);
    assert.equal(ui.container.innerHTML, html);
    assert.equal(ui.doc.activeElement, oldButton);
    buttons(ui, "View package")[0].click();
    assert.equal(ui.doc.activeElement.textContent, "Documentation integration fixture");
    oldButton.click(); await settle();
    assert.equal(calls.length, 1);
    assert.equal(calls[0].viewRevision, 1);
    assert.equal(calls[0].control.kind, "cancel_task");
  });
  test(`${mode} does not consume revision and corrected same revision is accepted`, t => {
    const ui = mount(t);
    reject(ui);
    assert.equal(ui.view.update(fixture(2)), true);
    assert.equal(ui.view.update(fixture(2)), false);
  });
  test(`${mode} retains all visible pending offer occurrences through rejection and replay`, async t => {
    let rejectDispatch;
    const pending = new Promise((resolve, reject) => { rejectDispatch = reject; });
    const calls = [], ui = mount(t, request => { calls.push(request); return pending; });
    const copies = buttons(ui, "Revoke this access"); assert.equal(copies.length, 2);
    copies[0].click();
    assert.ok(copies.every(button => button.disabled));
    reject(ui);
    assert.ok(copies.every(button => /Waiting for owner status/.test(description(ui, button))));
    rejectDispatch(new Error("private dispatch detail")); await settle();
    assert.ok(copies.every(button => /Could not confirm/.test(description(ui, button))));
    assert.ok(!ui.container.textContent.includes("private dispatch detail"));
    assert.equal(calls.length, 1);
    assert.equal(ui.view.update(fixture(2)), true);
    for (const button of buttons(ui, "Revoke this access")) {
      assert.equal(button.disabled, true);
      assert.match(description(ui, button), /Could not confirm/);
      button.click();
    }
    await settle(); assert.equal(calls.length, 1);
  });
}

const malformed = [
  ["unknown role kind", next => { next.tasks[0].roles[0].kind = "mystery"; }],
  ["unknown control kind", next => { next.tasks[0].controls[0].kind = "launch_everything"; }],
  ["unknown status tone", next => { next.packages[0].status.tone = "green"; }],
  ["nonstring displayed fact", next => { next.connections[0].account = { secret: "do not stringify" }; }],
  ["malformed target", next => { next.tasks[0].controls[0].target.revision = null; }],
  ["duplicate task identity", next => { next.tasks.push(structuredClone(next.tasks[0])); }],
  ["duplicate same-kind role identity", next => { next.tasks[0].roles.push(structuredClone(next.tasks[0].roles[0])); }],
  ["conflicting same-offer target", next => {
    next.connections[0].grants[0] = structuredClone(next.connections[0].grants[0]);
    next.connections[0].grants[0].controls[0].target.reference = "different target";
  }],
  ["conflicting same-offer destination origin", next => {
    const other = structuredClone(next.tasks[0].userActions[0]);
    other.id = "other-action"; other.authorizationOrigin = "https://other.example.test";
    next.tasks[0].userActions.push(other);
  }],
];
for (const [name, change] of malformed) {
  test(`${name} is refused without changing the accepted view`, t => {
    const ui = mount(t), html = ui.container.innerHTML;
    const next = fixture(2); change(next);
    assert.throws(() => ui.view.update(next), TypeError);
    assert.equal(ui.container.innerHTML, html);
    assert.equal(ui.view.update(fixture(2)), true);
  });
}

test("duplicate exact offers across rows retain independent focus and shared submission custody", async t => {
  const calls = [], ui = mount(t, async request => { calls.push(request); });
  buttons(ui, "Revoke this access")[1].focus();
  assert.equal(ui.view.update(fixture(2)), true);
  assert.equal(ui.doc.activeElement, buttons(ui, "Revoke this access")[1]);
  buttons(ui, "Revoke this access")[1].click(); await settle();
  assert.equal(calls.length, 1);
  assert.ok(buttons(ui, "Revoke this access").every(button => button.disabled));
});
