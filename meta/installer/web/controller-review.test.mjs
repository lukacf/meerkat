import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mountControllerReview } from '../launchkit/templates_v2/controller_review.mjs';

const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');

function mounted(status, body) {
  const dom = new JSDOM('<!doctype html><div id="review"></div>');
  const container = dom.window.document.getElementById('review');
  const calls = [];
  const fetch = async (path, init) => {
    calls.push({ path, method: init.method });
    return { status, ok: status >= 200 && status < 300, text: async () => JSON.stringify(body) };
  };
  const view = mountControllerReview(container, { fetch, schedule: () => 0, unschedule: () => {} });
  return { dom, container, calls, view };
}

for (const [code, notice] of [
  ['approver_unauthenticated', /Sign in with your own named account to answer this review/],
  ['not_administrator', /Only Console administrators answer this review/],
]) {
  test(`a ${code} refusal shows its notice and keeps the Console session`, async () => {
    const { dom, container, calls, view } = mounted(403, { error: { code } });
    try {
      await view.ready;
      assert.match(container.textContent, notice);
      assert.doesNotMatch(container.textContent, /Session ended/);
      assert.deepEqual(calls, [{ path: '/console/forge/controller-review/state', method: 'GET' }]);
    } finally { view.dispose(); dom.window.close(); }
  });
}

test('a 401 ends the review session', async () => {
  const { dom, container, view } = mounted(401, { error: { code: 'invalid_binding' } });
  try {
    await view.ready;
    assert.match(container.textContent, /Session ended/);
  } finally { view.dispose(); dom.window.close(); }
});
