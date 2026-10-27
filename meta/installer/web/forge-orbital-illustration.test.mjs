import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import React, { act } from 'react';
import { makeOrbitalIllustration } from './forge-orbital-illustration.mjs';

const { JSDOM } = createRequire(new URL('../../console/package.json', import.meta.url))('jsdom');
const OrbitalIllustration = makeOrbitalIllustration(React);
const preference = '(prefers-reduced-motion: reduce)';
const motionNodes = svg => svg.querySelectorAll('animate, animateTransform, animateMotion, set');

async function mountedSvg({ reduced = false, still = false, mini = false,
  unavailable = false, initialRead } = {}, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://offline.test/' });
  const listeners = new Set(), added = [], removed = [], reads = [];
  const media = {
    matches: reduced,
    addEventListener(type, callback) {
      assert.equal(type, 'change');
      listeners.add(callback);
      added.push(callback);
    },
    removeEventListener(type, callback) {
      assert.equal(type, 'change');
      assert.equal(listeners.delete(callback), true, 'Cleanup removes the exact installed listener.');
      removed.push(callback);
    },
  };
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    matchMedia: unavailable ? undefined : query => {
      assert.equal(query, preference);
      reads.push(query);
      // The operating-system preference may change after render but before the effect subscribes.
      return reads.length === 1 && initialRead !== undefined ? { matches: initialRead } : media;
    },
    fetch: () => { throw new Error('The decorative SVG must not make network requests.'); },
  };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  let root, parentRenders = 0;
  function Parent() {
    parentRenders += 1;
    return React.createElement(OrbitalIllustration, { mini, still });
  }
  try {
    const { createRoot } = await import('react-dom/client');
    root = createRoot(dom.window.document.getElementById('root'));
    await act(async () => root.render(React.createElement(Parent)));
    const svg = dom.window.document.querySelector('svg');
    assert.ok(svg, 'The production component mounts an actual SVG.');
    assert.equal(svg.getAttribute('role'), 'img');
    assert.equal(svg.getAttribute('aria-label'), 'Illustration of a system taking shape. Not a proposal.');
    await verify({ svg, listeners, reads, parentRenders: () => parentRenders,
      change: async matches => act(async () => {
        media.matches = matches;
        for (const listener of [...listeners]) listener({ matches, media: preference });
      }) });
  } finally {
    try {
      if (root) await act(async () => root.unmount());
      assert.equal(listeners.size, 0, 'Unmount leaves no preference listener active.');
      assert.deepEqual(removed, added, 'Every preference subscription is cleaned up exactly once.');
    } finally {
      dom.window.close();
      for (const [key, descriptor] of previous) {
        if (descriptor) Object.defineProperty(globalThis, key, descriptor);
        else delete globalThis[key];
      }
    }
  }
}

test('full orbital SVG reacts to reduced motion and reversal without a parent rerender', async () => {
  await mountedSvg({}, async ({ svg, change, listeners, parentRenders }) => {
    const movingCount = motionNodes(svg).length;
    assert.ok(movingCount > 0);
    assert.ok(svg.querySelector('animate'));
    assert.ok(svg.querySelector('animateTransform'));
    assert.equal(listeners.size, 1);
    const labels = [...svg.querySelectorAll('text')];
    assert.equal(labels.length, 4);
    assert.ok(labels.every(label => label.getAttribute('fill') === '#6f6a5f'));
    await change(true);
    assert.equal(motionNodes(svg).length, 0, 'Live preference changes remove every indefinite SMIL node.');
    assert.equal(parentRenders(), 1, 'The illustration responds without needing another parent render.');
    await change(false);
    assert.equal(motionNodes(svg).length, movingCount, 'Reversing the preference restores the normal motion.');
    assert.equal(parentRenders(), 1);
  });
});

test('mini orbital SVG reacts to reduced motion and reversal without a parent rerender', async () => {
  await mountedSvg({ mini: true }, async ({ svg, change, parentRenders }) => {
    const movingCount = motionNodes(svg).length;
    assert.ok(movingCount > 0);
    await change(true);
    assert.equal(motionNodes(svg).length, 0);
    await change(false);
    assert.equal(motionNodes(svg).length, movingCount);
    assert.equal(parentRenders(), 1);
  });
});

test('initial reduced-motion and explicit still render no SMIL in either SVG size', async t => {
  for (const mini of [false, true]) {
    await t.test(`initial reduced motion, ${mini ? 'mini' : 'full'}`, async () => {
      await mountedSvg({ reduced: true, mini }, async ({ svg }) => {
        assert.equal(motionNodes(svg).length, 0);
      });
    });
    await t.test(`explicit still, ${mini ? 'mini' : 'full'}`, async () => {
      await mountedSvg({ still: true, mini }, async ({ svg, change }) => {
        assert.equal(motionNodes(svg).length, 0);
        await change(true);
        await change(false);
        assert.equal(motionNodes(svg).length, 0, 'Explicit still remains calm regardless of preference reversal.');
      });
    });
  }
});

test('subscription rereads a preference that changed after the initial render sample', async () => {
  await mountedSvg({ reduced: true, initialRead: false }, async ({ svg, reads, listeners }) => {
    assert.ok(reads.length >= 2, 'The effect observes the current query rather than keeping its render sample.');
    assert.equal(listeners.size, 1);
    assert.equal(motionNodes(svg).length, 0);
  });
});

test('absence of matchMedia keeps ordinary SVG rendering and explicit still usable', async t => {
  await t.test('ordinary motion', async () => {
    await mountedSvg({ unavailable: true }, async ({ svg, listeners, reads }) => {
      assert.ok(motionNodes(svg).length > 0);
      assert.equal(listeners.size, 0);
      assert.deepEqual(reads, []);
    });
  });
  await t.test('explicit still', async () => {
    await mountedSvg({ unavailable: true, still: true }, async ({ svg, listeners }) => {
      assert.equal(motionNodes(svg).length, 0);
      assert.equal(listeners.size, 0);
    });
  });
});
