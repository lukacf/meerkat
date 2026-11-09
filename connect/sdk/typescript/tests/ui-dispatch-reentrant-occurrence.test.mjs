import assert from 'node:assert/strict';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { mountToolkitView } from '@rkat/toolkit/ui';

const copy = (v) => structuredClone(v);
const status = { tone: 'ready', text: 'Owner says available' };
const control = (offer = 'display-offer') => ({ kind: 'disconnect', offer,
  target: { owner: 'owner-ref', reference: 'connection-ref', revision: 'owner-revision' } });
function snapshot(revision = 1, duplicate = false) {
  const connection = { id: 'north', packageId: 'package', service: 'North archive',
    account: 'north@example.test', credentialScope: 'Provider read', status,
    grants: [], pendingTaskIds: [], controls: [control(), { ...control('change-offer'), kind: 'change_account' }] };
  return copy({ scope: 'human-panel', revision, observedAt: '2026-10-02 UTC', announcement: 'Observed revision ' + revision,
    tasks: [], frontends: [], packages: [], connections: duplicate
      ? [connection, { ...connection, id: 'south', service: 'South archive', account: 'south@example.test' }]
      : [connection] });
}
function setup(dispatch) {
  const dom = new JSDOM('<!doctype html><button id="outside">Other host work</button><main id="panel"></main>',
    { url: 'https://host.example.test/' });
  const { document, HTMLButtonElement } = dom.window;
  const root = document.getElementById('panel');
  const native = Object.getOwnPropertyDescriptor(HTMLButtonElement.prototype, 'disabled');
  const disableBlur = [];
  // jsdom retains disabled focus. Model the separately observed native behavior,
  // without claiming this shim constitutes actual-browser acceptance.
  Object.defineProperty(HTMLButtonElement.prototype, 'disabled', { ...native,
    set(value) {
      if (value && document.activeElement === this) {
        disableBlur.push(this);
        this.blur();
      }
      native.set.call(this, value);
    } });
  const calls = [];
  let view;
  view = mountToolkitView(root, { scope: 'human-panel', dispatch(request) {
    calls.push(copy(request));
    return dispatch ? dispatch(request, { view, document, root, calls }) : Promise.resolve();
  } });
  return { dom, document, root, view, calls, disableBlur,
    close() { view.dispose(); dom.window.close(); } };
}
function heading(c, title = 'North archive') {
  const h = [...c.root.querySelectorAll('h2,h3,h4')].find((n) => n.textContent === title);
  assert.ok(h, 'Visible contextual heading exists: ' + title);
  return h;
}
function button(c, title = 'North archive', label = 'Disconnect account') {
  const h = heading(c, title);
  const b = [...h.parentElement.querySelectorAll('button')].find((n) => n.textContent === label);
  assert.ok(b, 'Visible control exists: ' + title + '/' + label);
  return b;
}
function oneOldRequest(c, revision = 1) {
  assert.equal(c.calls.length, 1, 'Exactly one admitted click dispatch');
  assert.equal(c.calls[0].scope, 'human-panel');
  assert.equal(c.calls[0].viewRevision, revision, 'Capture revision before focus/blur or owner callbacks');
  assert.deepEqual(c.calls[0].control, control());
}
const live = (c) => c.root.querySelector('[aria-live="polite"]').textContent;
const tick = async () => { await Promise.resolve(); await Promise.resolve(); };

for (const transition of ['owner', 'blur', 'focusin']) {
  test('host ' + transition + ' moving focus to another pending occurrence does not strand BODY', () => {
    let c;
    c = setup(() => {
      if (transition === 'owner') button(c, 'South archive').focus();
      return Promise.resolve();
    });
    try {
      c.view.update(snapshot(1, true)); const north = button(c); const south = button(c, 'South archive');
      north.focus(); let moved = false;
      if (transition !== 'owner') {
        const listener = (event) => {
          if (moved || (transition === 'focusin' && event.target !== heading(c))) return;
          moved = true; south.focus();
        };
        (transition === 'blur' ? north : c.root).addEventListener(transition, listener);
      }
      north.click(); oneOldRequest(c);
      if (transition !== 'owner') assert.equal(moved, true);
      assert.equal(north.disabled, true); assert.equal(south.disabled, true);
      assert.equal(c.document.activeElement, heading(c, 'South archive'), 'Preserve the host-selected affected occurrence, not BODY');
      c.view.update(snapshot(2, true)); assert.equal(c.document.activeElement, heading(c, 'South archive')); oneOldRequest(c);
    } finally { c.close(); }
  });
}
