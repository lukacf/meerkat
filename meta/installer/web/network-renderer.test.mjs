import assert from 'node:assert/strict';
import test from 'node:test';
import { MobSim } from './mob-sim-renderer.mjs';

test('a simulation mounted in a hidden pane draws its identities when revealed', () => {
  const previous = Object.fromEntries(['window', 'document', 'ResizeObserver', 'IntersectionObserver', 'requestAnimationFrame']
    .map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  let rectangle = {width: 0, height: 0};
  const arcs = [];
  const context = new Proxy({measureText: text => ({width: text.length * 6})}, {
    get: (target, key) => key in target ? target[key] : (...args) => {
      if (key === 'arc') arcs.push(args);
    },
  });
  Object.assign(globalThis, {
    window: {matchMedia: () => ({matches: false}), devicePixelRatio: 1},
    document: {hidden: false, addEventListener() {}, removeEventListener() {}},
    ResizeObserver: class {observe() {} disconnect() {}},
    IntersectionObserver: class {observe() {} disconnect() {}},
    requestAnimationFrame: () => 0,
  });
  let simulation;
  try {
    simulation = new MobSim({getContext: () => context, getBoundingClientRect: () => rectangle});
    simulation.setScene({
      W: 1000, H: 560, duration: 8.4, script: [], routes: {},
      phaseOffsets: [{activeEdgeIds: []}],
      nodes: [{id: 'owner:main', kind: 'coord', x: 500, y: 280, r: 8, label: 'Owner'}],
      edges: [],
    });
    rectangle = {width: 520, height: 400};
    simulation.resize();
    simulation.step(0.05);
    simulation.draw();
    const owner = simulation.node('owner:main');
    assert.ok([owner.x, owner.y, owner.r, owner.a].every(Number.isFinite));
    assert.ok(owner.r > 0 && owner.a > 0, 'the revealed identity must be visible');
    assert.ok(arcs.some(([x, y, radius]) => [x, y, radius].every(Number.isFinite) && radius > 0),
      'the actual renderer must draw a finite identity circle');
  } finally {
    simulation?.destroy();
    for (const [key, descriptor] of Object.entries(previous)) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
});
