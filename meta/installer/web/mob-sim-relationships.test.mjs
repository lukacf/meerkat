import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
import { createSimulation } from './simulation-state.mjs';
import { createSimulationScene } from './simulation-scene.mjs';
import { createRelationshipProjection, OVERVIEW_RELATIONSHIP_THRESHOLD } from './mob-sim-relationships.mjs';
import { MobSim as DrawMobSim } from './mob-sim-renderer.mjs';

function populationPlan() {
  const owner = {id: 'lead', name: 'Coordinator', description: 'Own the shared outcome', instructions: 'Coordinate existing context owners.', instances: 1, tools: {memory: true}};
  const contexts = ['alpha', 'beta'].map(id => ({id, name: `${id} contexts`, description: 'Independent continuing context',
    instructions: 'Maintain the named context.', instances: 305,
    members: Array.from({length: 305}, (_, index) => ({key: `case_${index + 1}`, name: `${id} case ${index + 1}`, instructions: 'Own this case.'})),
    tools: {memory: true, workspace: true, notes: id === 'alpha' ? 'read' : 'write', builtins: true, schedule: true, workgraph: true,
      mcp: [{connection_id: 'records', tool_names: [id === 'alpha' ? 'read_summary' : 'read_record']}]}}));
  return {project: {name: 'Large configured population', slug: 'population-links'}, agent_classes: [owner, ...contexts],
    topology: {mode: 'hub', entrypoint: 'lead:main', edges: []}, flows: [], knowledge: {mode: 'none'},
    connections: [{id: 'records', name: 'Context records', description: 'Read context data', transport: 'stdio'}]};
}

const exactEdges = (model, selected) => model.edges.filter(edge => edge.source === selected || edge.target === selected);
function assertExactIncident(model, wiring, selected) {
  const original = new Map(model.edges.map(edge => [edge.id, edge]));
  for (const edge of wiring) {
    const exact = original.get(edge.edgeId);
    assert.ok(exact, edge.edgeId);
    assert.equal(edge.source, exact.source);
    assert.equal(edge.target, exact.target);
    assert.equal(edge.kind, exact.kind);
    assert.deepEqual(edge.pairs, [[exact.source, exact.target]]);
  }
  const incident = wiring.filter(edge => edge.source === selected || edge.target === selected);
  assert.deepEqual(incident.map(edge => edge.edgeId).sort(), exactEdges(model, selected).map(edge => edge.id).sort());
  assert.ok(incident.every(edge => edge.count === 1));
  assert.equal(new Set(wiring.map(edge => edge.edgeId)).size, wiring.length);
}

test('a late and final member reveal every actual peer and scoped tool relationship above the grouping threshold', () => {
  const value = populationPlan(), model = createSimulation(value), source = createSimulationScene(model, value), before = structuredClone(source);
  assert.equal(model.nodes.filter(node => node.kind === 'agent').length, 611);
  assert.ok(model.edges.length > OVERVIEW_RELATIONSHIP_THRESHOLD);
  const projection = createRelationshipProjection(source), overview = projection.select(null);
  assert.equal(overview.length, source.edgeGroups.length);
  assert.ok(overview.length < 20, 'Neutral work remains aggregated instead of expanding thousands of relationships');
  assert.equal(overview.reduce((sum, edge) => sum + edge.count, 0), model.edges.length);
  for (const selected of ['alpha:case_304', 'beta:case_305']) {
    assert.equal(overview.filter(edge => edge.source === selected || edge.target === selected).length, 0, 'Fixture member is absent from the old representatives');
    const shown = projection.select(selected);
    assertExactIncident(model, shown, selected);
    assert.equal(exactEdges(model, selected).length, 7);
    assert.ok(shown.length <= overview.length + 7);
    assert.equal(projection.select(selected), shown, 'Repeated frames reuse the same source-bound selected projection');
  }
  const selected = 'beta:case_305', connector = exactEdges(model, selected).find(edge => edge.capability === 'mcp');
  assert.deepEqual(connector.toolNames, ['read_record']);
  assert.ok(!exactEdges(model, selected).some(edge => edge.toolNames?.includes('read_summary')));
  assert.equal(projection.select(null), overview);
  assert.equal(projection.select('foreign:identity'), overview);
  assert.deepEqual(source, before, 'Relationship projection cannot change graph or grant authority');
});

test('selecting a resource reveals all and only its configured grants, and switching selection releases unrelated exact wiring', () => {
  const value = populationPlan(), model = createSimulation(value), source = createSimulationScene(model, value), projection = createRelationshipProjection(source);
  const resource = 'resource:connector:records';
  const shown = projection.select(resource);
  assertExactIncident(model, shown, resource);
  assert.equal(exactEdges(model, resource).length, 610);
  assert.equal(projection.select(null).length, source.edgeGroups.length);
  const member = projection.select('beta:case_305');
  assert.ok(member.length < shown.length);
  assertExactIncident(model, member, 'beta:case_305');
});

test('small exact previews and all aggregated groups preserve the pre-existing default relationship coverage', () => {
  const rows = Array.from({length: 2001}, (_, index) => ({id: `edge:${index}`, kind: 'peer', source: `a:${index}`, target: `b:${index}`}));
  const grouped = {relationships: rows, edgeGroups: rows.map(edge => ({kind: edge.kind, count: 1, relationships: [edge]}))};
  const projection = createRelationshipProjection(grouped);
  assert.equal(projection.select(null).length, 2001, 'Grouping must not silently truncate whole configured groups');
  assert.deepEqual(projection.select(null).map(edge => edge.edgeId), rows.map(edge => edge.id));
  const atThreshold = createRelationshipProjection({relationships: rows.slice(0, 2000), edgeGroups: [{kind: 'peer', count: 2000, relationships: rows.slice(0, 2000)}]});
  assert.equal(atThreshold.select(null).length, 2000);
  const replacement = createRelationshipProjection({relationships: rows.slice(0, 1), edgeGroups: []});
  assert.equal(replacement.select('a:2000').length, 1, 'An old selected ID cannot carry wiring into a replacement source');
});

const inertNetwork = `export const COL={ink:[0,0,0],mid:[0,0,0],screen:[255,255,255],accent:[255,0,0]};
export const rgba=()=>'';export const SANS='sans-serif';
export class Net {constructor(canvas){this.canvas=canvas;this.selectedId=null;this.showPeers=false;this.t=0;}
setScene(factory){this.factory=factory;this.scene=factory(this.canvas.width,this.canvas.height);}
seek(time){this.t=time;}destroy(){}}`;
const bundle = await build({entryPoints: [fileURLToPath(new URL('mob-sim-renderer.mjs', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', logLevel: 'silent', plugins: [{name: 'inert-trusted-network', setup(builder) {
    builder.onResolve({filter: /network-renderer\.mjs$/}, () => ({path: 'inert-network', namespace: 'trusted-host'}));
    builder.onLoad({filter: /.*/, namespace: 'trusted-host'}, () => ({contents: inertNetwork, loader: 'js'}));
  }}]});
const {MobSim} = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

test('the real automatic renderer reads exact selected links at normal and narrow widths without rebuilding the scene', () => {
  const value = populationPlan(), model = createSimulation(value), source = createSimulationScene(model, value);
  const selected = 'beta:case_305';
  for (const [width, height] of [[640, 380], [320, 300]]) {
    const sim = new MobSim({width, height});
    sim.setScene(source);
    const scene = sim.scene, overview = scene.edges;
    assert.equal(overview.length, source.edgeGroups.length);
    sim.selectedId = selected;
    assert.equal(sim.scene, scene);
    assertExactIncident(model, scene.edges, selected);
    for (const edge of scene.edges.filter(edge => edge.source === selected || edge.target === selected)) {
      assert.equal(scene.edgeVisible(edge, sim), true);
      assert.equal(scene.edgeAlpha(edge, sim), .5);
    }
    assert.ok(scene.nodes.every(node => Number.isFinite(node.x) && Number.isFinite(node.y)));
    sim.selectedId = null;
    assert.equal(scene.edges, overview);
    sim.destroy();
  }
});

test('the trusted Net draw seam consumes late-member peer and tool paths with exact endpoints at both viewport sizes', () => {
  const value = populationPlan(), model = createSimulation(value), source = createSimulationScene(model, value), selected = 'beta:case_305';
  const original = new Map(model.edges.map(edge => [edge.id, edge]));
  const replacements = {window: {devicePixelRatio: 1, matchMedia: () => ({matches: false})},
    document: {hidden: false, addEventListener() {}, removeEventListener() {}},
    ResizeObserver: class {observe() {} disconnect() {}}, IntersectionObserver: class {observe() {} disconnect() {}},
    requestAnimationFrame: () => 1};
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, replacement] of Object.entries(replacements)) Object.defineProperty(globalThis, key, {configurable: true, value: replacement});
  try {
    for (const [width, height] of [[640, 380], [320, 300]]) {
      // Canvas operations are recorded in trusted host code. RAF never executes
      // and no authored program, browser or generated application is loaded.
      const paths = [], operations = [];
      const context = new Proxy({measureText: text => ({width: text.length * 6})}, {get(target, key) {
        return Object.hasOwn(target, key) ? target[key] : (...args) => operations.push({key, args});
      }});
      const sim = new DrawMobSim({getContext: () => context, getBoundingClientRect: () => ({width, height})});
      try {
        sim.setScene(source);
        const scene = sim.scene, route = scene.routePoints;
        scene.routePoints = (id, a, b, net) => {paths.push({id, source: a.spec.id, target: b.spec.id}); return route(id, a, b, net);};
        sim.msgs = []; sim.showPeers = false; sim.selectedId = selected;
        operations.length = 0; sim.draw();
        const incident = paths.filter(edge => edge.source === selected || edge.target === selected);
        assert.deepEqual(incident.map(edge => edge.id).sort(), exactEdges(model, selected).map(edge => edge.id).sort());
        assert.equal(incident.filter(edge => original.get(edge.id).kind === 'peer').length, 1);
        assert.equal(incident.filter(edge => original.get(edge.id).kind === 'tool').length, 6);
        for (const edge of incident) {
          assert.equal(edge.source, original.get(edge.id).source);
          assert.equal(edge.target, original.get(edge.id).target);
          const a = sim.node(edge.source), b = sim.node(edge.target);
          assert.ok(operations.some(op => op.key === 'moveTo' && op.args[0] === a.x && op.args[1] === a.y));
          assert.ok(operations.some(op => op.key === 'lineTo' && op.args[0] === b.x && op.args[1] === b.y));
        }
        paths.length = 0; sim.selectedId = null; sim.draw();
        assert.equal(paths.some(edge => edge.source === selected || edge.target === selected), false, 'Neutral grouped drawing does not retain selected expansion');
        const tool = scene.edges.find(edge => edge.kind === 'tool');
        assert.equal(scene.edgeVisible(tool, sim), false, 'Neutral tool grants stay hidden without active access or All tool access');
        sim.showPeers = true;
        assert.equal(scene.edgeVisible(tool, sim), true);
        assert.equal(scene.edges.length, source.edgeGroups.length);
      } finally {sim.destroy();}
    }
  } finally {
    for (const [key, descriptor] of previous) {if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];}
  }
});
