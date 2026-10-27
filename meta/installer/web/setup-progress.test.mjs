import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { inputKey, captureGuideResumeInput, preservedGuideResume, recoverGuideResumeInput, readInput, writeInput } from './wizard-state.mjs';

const bundled = await build({ entryPoints: [fileURLToPath(new URL('./setup-progress.mjs', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', logLevel: 'silent' });
const { setupProgress, setupNeedsApplicationBuild, setupProgressHasLifecycle, setupGuideResume, admitSetupGuideResume, GUIDE_RESUME_PROMPT, SETUP_DISCOVERY_PROMPT } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString('base64')}`);
const rendered = await build({ entryPoints: [fileURLToPath(new URL('./guided.jsx', import.meta.url))],
  bundle: true, write: false, format: 'esm', platform: 'node', logLevel: 'silent', loader: { '.css': 'empty' },
  nodePaths: [fileURLToPath(new URL('node_modules', import.meta.url))],
  alias: { '@console-core': fileURLToPath(new URL('../../packages/console-core/src', import.meta.url)) } });
const { SetupProgressCard, SetupCurrentStatusAnchor, SetupGuideQuestionHeading, SetupGuideStatusCaption, guideStatusText, navigateSetupProgress } = await import(`data:text/javascript;base64,${Buffer.from(rendered.outputFiles[0].text).toString('base64')}`);

const guide = { id: 'reading', version: 4, proposal: { target: 'mac' } };
const accepted = { guide_id: 'reading', guide_version: 4, current: true, status: 'completed', build_id: 'build',
  source_verified: true, source_digest: 'source', ready_to_package: true, acceptance_current: true, acceptance_trusted: true,
  acceptance: { issuer: 'mobkit-launch-acceptance-host', source_digest: 'source', status: 'pass', tests_run: 7 } };

const remoteInstalled = { protocol: 1, guide_id: guide.id, guide_version: 4, deployment_id: 'saved-remote',
  build_id: accepted.build_id, accepted_source_digest: 'a'.repeat(64), package_sha256: 'b'.repeat(64),
  transferred_source_digest: 'c'.repeat(64), endpoint: { host: 'workshop.local', user: 'reader', ssh_port: 22 },
  destination: '/home/reader/workshop', status: 'running', admitted: true, remote_admission_requested: true,
  source_current: true, ready: true, observation: { available: true } };

test('remote lifecycle progress routes running, stopped and failed installed source to existing remote controls', () => {
  for (const [status, title, label] of [['running', 'Your installed remote application is running', 'Use your remote application'],
    ['stopped', 'Your installed remote application is stopped', 'Manage remote application'],
    ['failed', 'Your remote installation needs attention', 'Review remote recovery']]) {
    const remoteDeployment = { ...remoteInstalled, status }, before = structuredClone(remoteDeployment);
    const progress = setupProgress(guide, { build: accepted, remoteDeployment },
      { remoteObservation: 'known', deploymentObservation: 'known' });
    assert.equal(progress.title, title);
    assert.deepEqual(progress.actions, [{ target: 'remote', label }]);
    assert.ok(!progress.actions.some(item => item.url || item.sourceKind), 'Status cannot invent browser access or admit another installation');
    assert.equal(setupProgressHasLifecycle(guide, { remoteDeployment }), true);
    assert.equal(setupGuideResume(guide, { remoteDeployment }).text, GUIDE_RESUME_PROMPT);
    const target = { scrollIntoView() { assert.fail('The existing reveal owner must navigate'); } }, revealed = [];
    assert.equal(navigateSetupProgress(progress.actions[0], progress, { targets: { remote: { current: target } },
      revealTarget: value => revealed.push(value), onInstall: () => assert.fail('Remote narration cannot call installation') }), true);
    assert.deepEqual(revealed, [target]);
    assert.deepEqual(remoteDeployment, before);
  }
});

test('remote lifecycle progress qualifies cloud readiness and preserves exact source and removal guards', () => {
  const operation = '11111111-1111-4111-8111-111111111111';
  const cloud = { ...remoteInstalled, target: 'gcp', cloud_operation_id: operation,
    endpoint: { transport: 'gcp-iap', operation_id: operation, account: 'fixture@example.test', project: 'fixture-project',
      zone: 'europe-west1-b', instance: 'fixture-app', instance_id: '1234', ownership_token: 'e'.repeat(32),
      host: 'compute.1234', user: 'mobkit', ssh_port: 22 } };
  assert.equal(setupProgress(guide, { remoteDeployment: cloud }).title, 'Your installed cloud application is running');
  for (const changed of [{ ready: false }, { source_current: false }]) {
    const progress = setupProgress(guide, { remoteDeployment: { ...cloud, ...changed } });
    assert.equal(progress.title, 'Your remote runtime needs a readiness check');
    assert.deepEqual(progress.actions, [{ target: 'remote', label: 'Check remote readiness' }]);
  }
  for (const cloud_status of ['removing', 'removal-required', 'removed']) {
    const progress = setupProgress(guide, { remoteDeployment: { ...cloud, cloud_status } });
    assert.deepEqual(progress.actions, [{ target: 'remote', label: 'Review cloud removal' }]);
    assert.doesNotMatch(progress.detail, /is running|is stopped|Set up and run/);
  }
});

test('remote lifecycle progress masks unconfirmed observations and rejects foreign or unadmitted source', () => {
  for (const remoteObservation of ['loading', 'unknown']) {
    const progress = setupProgress(guide, { build: accepted, remoteDeployment: remoteInstalled }, { remoteObservation });
    assert.equal(progress.title, remoteObservation === 'loading' ? 'Checking your remote installation status' : 'Remote installation status is unavailable');
    assert.deepEqual(progress.actions, [{ target: 'remote', label: 'Review remote status' }]);
    assert.doesNotMatch(progress.detail, /is running|is stopped|passed independent|browser.*ready/);
  }
  const unavailable = { ...remoteInstalled, observation: { available: false } };
  assert.equal(setupProgress(guide, { remoteDeployment: unavailable }, { remoteObservation: 'known' }).title,
    'Remote installation status is unavailable');
  for (const remoteDeployment of [{ ...remoteInstalled, guide_id: 'other' },
    { ...remoteInstalled, accepted_source_digest: 'invalid' }, { ...remoteInstalled, status: 'failed', admitted: false, remote_admission_requested: false },
    { ...remoteInstalled, status: 'configuration-required', admitted: false, remote_admission_requested: false }]) {
    assert.equal(setupProgressHasLifecycle(guide, { remoteDeployment }), false);
    assert.equal(setupProgress(guide, { build: accepted, remoteDeployment }, { remoteObservation: 'known' }).title,
      'Your application passed independent offline checks');
  }
  const pending = setupProgress({ ...guide, proposal: { target: 'gcp' } }, { build: accepted }, { remoteObservation: 'loading' });
  assert.equal(pending.title, 'Checking your remote installation status');
  assert.equal(navigateSetupProgress(pending.actions[0], pending, { targets: {} }), false);
  const local = { guide_id: guide.id, deployment_id: 'saved-local', status: 'stopped' };
  const cloudGuide = { ...guide, proposal: { target: 'gcp' } };
  for (const remoteDeployment of [null, { ...remoteInstalled, guide_id: 'other' },
    { ...remoteInstalled, accepted_source_digest: 'invalid' }]) for (const remoteObservation of ['loading', 'unknown', 'known']) {
    const progress = setupProgress(cloudGuide, { deployment: local, remoteDeployment }, { remoteObservation });
    assert.equal(progress.title, 'Your installed application is stopped', 'An unowned remote read cannot hide a saved local installation');
    assert.equal(progress.actions[0].target, 'deployment');
  }
});

test('installation observation uncertainty preserves saved receipts without current runtime or new-install claims', () => {
  const stopped = { guide_id: guide.id, deployment_id: 'owned', build_id: accepted.build_id, status: 'stopped' };
  const running = { ...stopped, status: 'running', process_live: true, health: 'ready',
    console_url: 'http://127.0.0.1:18789/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  for (const deploymentObservation of ['loading', 'unknown']) for (const deployment of [null, stopped, running,
    { ...running, source_kind: 'native-team', build_id: null }, { ...running, guide_id: 'other' }]) {
    const receipts = { build: accepted, deployment }, before = structuredClone(receipts);
    const progress = setupProgress(guide, receipts, { deploymentObservation });
    assert.equal(progress.title, deploymentObservation === 'loading' ? 'Checking your installation status' : 'Installation status is unavailable');
    assert.deepEqual(progress.actions, [{ target: 'deployment', label: 'Review installation status' }]);
    assert.doesNotMatch(progress.detail, /is running|is stopped|Console.*available|passed independent/);
    assert.equal(progress.detail.includes('Saved installation findings remain available.'), deployment?.guide_id === guide.id);
    assert.deepEqual(receipts, before);
  }
});

test('installation observation certainty remains separate from builder uncertainty and legacy pure projections', () => {
  const deployment = { guide_id: guide.id, deployment_id: 'owned', status: 'stopped' };
  for (const receipts of [{ build: accepted }, { deployment }, { build: accepted, deployment }])
    assert.deepEqual(setupProgress(guide, receipts, { deploymentObservation: 'known' }), setupProgress(guide, receipts));
  const progress = setupProgress(guide, { build: accepted, deployment }, { deploymentObservation: 'unknown', buildObservation: 'unknown' });
  assert.equal(progress.title, 'Installation status is unavailable');
  assert.deepEqual(progress.actions.map(item => item.target), ['deployment', 'build']);
  assert.match(progress.detail, /saved source are kept/);
});

test('installation observation uncertainty preserves existing active trial and cleanup navigation', () => {
  const build = { ...accepted, cleanup_pending: true }, deployment = { guide_id: guide.id, deployment_id: 'owned', status: 'running' };
  const trial = { guide_id: guide.id, guide_version: guide.version, rehearsal_id: 'owned-trial', mode: 'design',
    status: 'waiting_input', current: true, can_continue: true, keep_open: true };
  const progress = setupProgress(guide, { build, deployment, trial }, { deploymentObservation: 'unknown' });
  assert.equal(progress.title, 'Installation status is unavailable');
  assert.ok(progress.actions.some(item => item.target === 'deployment'));
  assert.ok(progress.actions.some(item => item.target === 'build'));
  assert.ok(progress.actions.some(item => item.target === 'trial'));
  assert.match(progress.detail, /runtime has not finished closing/);
});

test('unconfirmed build observation never presents a new build before the saved receipt is checked', () => {
  const state = { ...guide, readiness: { context_ready: true, decision_blockers: [],
    implementation_blockers: ['Build custom screens.'] } };
  for (const buildObservation of ['loading', 'unknown']) {
    const progress = setupProgress(state, { build: null }, { buildObservation });
    assert.equal(progress.title, buildObservation === 'loading' ? 'Checking your saved build' : 'Build status is unavailable');
    assert.deepEqual(progress.actions, [{ target: 'build', label: 'Review build status' }]);
    assert.equal(setupNeedsApplicationBuild(state, null, { buildObservation }), false);
  }
  assert.equal(setupNeedsApplicationBuild(state, null, { buildObservation: 'known' }), true);
  assert.equal(setupProgress(state, { build: null }, { buildObservation: 'known' }).title, 'Your application needs to be built');
  assert.deepEqual(setupProgress(state), setupProgress(state, {}, { buildObservation: 'known' }), 'Omitted observation retains legacy pure-call behavior');
});

test('unavailable retained builds stay saved evidence without new candidate recommendations', () => {
  for (const build of [accepted, { ...accepted, status: 'failed' }, { ...accepted, status: 'running' },
    { ...accepted, cleanup_pending: true }, { ...accepted, guide_version: 3, current: false }]) {
    const before = structuredClone(build);
    const progress = setupProgress(guide, { build }, { buildObservation: 'unknown' });
    assert.equal(progress.title, 'Build status is unavailable');
    assert.match(progress.detail, /Saved build findings remain available/);
    assert.deepEqual(progress.actions, [{ target: 'build', label: 'Review build status' }]);
    assert.deepEqual(build, before);
  }
  const foreign = setupProgress(guide, { build: { ...accepted, guide_id: 'other' } }, { buildObservation: 'unknown' });
  assert.doesNotMatch(foreign.detail, /Saved build findings/);
});

test('unconfirmed builder status preserves installed operation and Console without a candidate trial', () => {
  const state = { ...guide, readiness: { context_ready: true } };
  const stopped = { guide_id: guide.id, deployment_id: 'owned', build_id: 'installed', status: 'stopped' };
  const running = { ...stopped, status: 'running', process_live: true, health: 'ready',
    console_url: 'http://127.0.0.1:18789/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  for (const buildObservation of ['loading', 'unknown']) for (const deployment of [stopped, running]) {
    const before = structuredClone({ deployment, build: accepted });
    const progress = setupProgress(state, { build: accepted, deployment }, { buildObservation });
    assert.equal(progress.title, deployment.status === 'stopped' ? 'Your installed application is stopped' : 'Your installed custom application is running');
    assert.equal(progress.actions[0].label, deployment.status === 'stopped' ? 'Manage application' : 'Use your application');
    assert.equal(progress.actions.some(item => item.target === 'console'), deployment.status === 'running');
    assert.equal(progress.actions.some(item => item.target === 'trial'), false);
    assert.deepEqual(progress.actions.at(-1), { target: 'build', label: 'Review build status' });
    assert.match(progress.detail, /Forge (?:is checking|could not check)/);
    assert.doesNotMatch(progress.detail, /newer application|newer candidate|passed independent offline/);
    assert.deepEqual({ deployment, build: accepted }, before);
  }
  const native = { ...stopped, source_kind: 'native-team', build_id: null };
  const team = setupProgress(state, { deployment: native }, { buildObservation: 'unknown' });
  assert.equal(team.title, 'Your installed agent team is stopped');
  assert.equal(team.actions[0].label, 'Manage agent team');
});

test('restored and newer candidate narration requires a known builder receipt', () => {
  const state = { ...guide, readiness: { context_ready: true } };
  const build = { ...accepted, source_digest: 'a'.repeat(64),
    acceptance: { ...accepted.acceptance, source_digest: 'a'.repeat(64) } };
  const update = { update_id: 'e7972227-024d-4741-b0b6-618dbdf3b53f', status: 'rolled-back',
    from_build_id: 'installed', to_build_id: build.build_id, guide_version: build.guide_version,
    source_digest: build.source_digest, previous_source_digest: 'b'.repeat(64),
    package_sha256: 'c'.repeat(64), previous_package_sha256: 'd'.repeat(64) };
  const stopped = { guide_id: guide.id, deployment_id: 'owned', build_id: 'installed',
    package_sha256: update.previous_package_sha256, status: 'stopped', update };
  const known = setupProgress(state, { build, deployment: stopped }, { buildObservation: 'known' });
  assert.equal(known.actions[0].label, 'Review recovery findings');
  assert.equal(known.actions[1].label, 'Try the newer application');
  const unknown = setupProgress(state, { build, deployment: stopped }, { buildObservation: 'unknown' });
  assert.equal(unknown.title, 'Your installed application is stopped');
  assert.equal(unknown.actions[0].label, 'Manage application');
  assert.doesNotMatch(unknown.detail, /source was restored|newer candidate|newer application/);
  assert.deepEqual(unknown.actions.map(item => item.target), ['deployment', 'build']);
});

test('owned active trials retain their existing controls when builder observation is unknown', () => {
  const trial = { rehearsal_id: 'trial', guide_id: guide.id, guide_version: guide.version, current: true,
    mode: 'application', status: 'waiting_input', keep_open: true, can_continue: true, turn_count: 1,
    application_source: { build_id: accepted.build_id, source_digest: accepted.source_digest } };
  const stopped = { guide_id: guide.id, deployment_id: 'owned', build_id: accepted.build_id, status: 'stopped' };
  for (const deployment of [undefined, stopped]) {
    const progress = setupProgress(guide, { build: accepted, trial, deployment }, { buildObservation: 'unknown' });
    assert.deepEqual(progress.actions.find(item => item.target === 'trial'), { target: 'trial', label: 'Continue trial' });
    assert.match(progress.detail, /Forge could not check/);
    assert.doesNotMatch(progress.detail, /earlier trial|cannot continue with this proposal/);
    if (deployment) assert.equal(progress.title, 'Your installed application is stopped');
    else assert.equal(progress.title, 'Your saved trial is waiting for input');
  }
  const failed = { ...trial, application_observations: [{ kind: 'callback_failed' }] };
  const recovery = setupProgress(guide, { build: accepted, trial: failed }, { buildObservation: 'unknown' });
  assert.equal(recovery.title, 'Your trial needs an application fix');
  assert.equal(recovery.actions[0].label, 'Review trial problem');
  assert.match(recovery.detail, /End this trial; once cleanup is confirmed/);
  for (const status of ['running', 'ending', 'canceling']) {
    const progress = setupProgress(guide, { build: null, trial: { ...trial, status } }, { buildObservation: 'loading' });
    assert.equal(progress.actions[0].label, 'Follow trial');
    assert.doesNotMatch(progress.detail, /earlier proposal|earlier trial|cannot continue with the current design/);
    assert.match(progress.detail, /Forge is checking for saved application work/);
  }
});

test('foreign and finished trials cannot replace an unconfirmed build status check', () => {
  for (const trial of [{ guide_id: 'other', rehearsal_id: 'foreign', status: 'running' },
    { guide_id: guide.id, rehearsal_id: 'saved', mode: 'application', status: 'completed', current: true,
      application_source: { build_id: accepted.build_id, source_digest: accepted.source_digest } }]) {
    const progress = setupProgress(guide, { build: accepted, trial }, { buildObservation: 'unknown' });
    assert.equal(progress.title, 'Build status is unavailable');
    assert.deepEqual(progress.actions.map(item => item.target), ['build']);
  }
});

test('known builder observations preserve the existing saved receipt projection', () => {
  for (const build of [null, accepted, { ...accepted, status: 'running' }, { ...accepted, status: 'failed' },
    { ...accepted, cleanup_pending: true }])
    assert.deepEqual(setupProgress(guide, { build }, { buildObservation: 'known' }), setupProgress(guide, { build }));
  const state = { ...guide, readiness: { context_ready: true } };
  const stopped = { guide_id: guide.id, deployment_id: 'owned', build_id: 'installed', status: 'stopped' };
  assert.deepEqual(setupProgress(state, { build: accepted, deployment: stopped }, { buildObservation: 'known' }),
    setupProgress(state, { build: accepted, deployment: stopped }));
});

test('waiting presentation keeps an earlier active build reachable while the guide has an unanswered question', () => {
  const question = { title: 'Who will use the new staff page?', options: [{ id: 'staff', label: 'Staff only' }] };
  const revised = { ...guide, version: 5, status: 'refining', question,
    readiness: { context_ready: true, decision_blockers: ['Choose who can use it'], implementation_blockers: ['Build the staff page'] } };
  for (const status of ['queued', 'running', 'checking']) {
    const build = { ...accepted, status, current: false, cleanup_pending: true };
    const before = structuredClone(build);
    assert.equal(setupProgressHasLifecycle(revised, { build }), true, 'The ongoing owned build must keep its work section reachable');
    const progress = setupProgress(revised, { build });
    assert.equal(progress.title, 'An earlier proposal is still building');
    assert.deepEqual(progress.actions, [{ target: 'build', label: 'Follow earlier build' }]);
    const html = renderToStaticMarkup(React.createElement(SetupProgressCard,
      { progress, onNavigate: () => {}, Icon: () => null }));
    assert.match(html, /Follow earlier build/);
    assert.doesNotMatch(html, /Install|Download|Resume saved build|Build application/);
    const heading = renderToStaticMarkup(React.createElement(SetupGuideQuestionHeading,
      { question, lifecycle: setupProgressHasLifecycle(revised, { build }) }));
    assert.match(heading, /<h2>Who will use/);
    const target = { scrollIntoView() { assert.fail('The ordinary reveal callback should own navigation'); } };
    const revealed = [];
    assert.equal(navigateSetupProgress(progress.actions[0], progress,
      { targets: { build: { current: target } }, revealTarget: node => revealed.push(node) }), true);
    assert.deepEqual(revealed, [target]);
    assert.deepEqual(build, before, 'Following work must not change admission or source evidence');
    const unrelated = { ...build, guide_id: 'another-guide' };
    assert.equal(setupProgressHasLifecycle(revised, { build: unrelated }), false);
    assert.doesNotMatch(setupProgress(revised, { build: unrelated }).actions.map(item => item.label).join(' '), /Follow earlier build/);
  }
  for (const status of ['completed', 'failed', 'interrupted']) {
    const stale = { ...accepted, current: false, status, cleanup_pending: false, stop_requested: true };
    assert.equal(setupProgressHasLifecycle(revised, { build: stale }), false, 'A terminal stale receipt without pending cleanup is not active work');
    const cleanup = { ...stale, cleanup_pending: true };
    assert.equal(setupProgressHasLifecycle(revised, { build: cleanup }), true);
    assert.deepEqual(setupProgress(revised, { build: cleanup }).actions, [{ target: 'build', label: 'Review cleanup' }]);
  }
});

test('a saved proposal with core context can open the builder before application agreement', () => {
  const contextual = { ...guide, readiness: { context_ready: true, ready: false,
    blockers: ['Agree how to handle: Authenticated application UI.'] } };
  const progress = setupProgress(contextual);
  assert.equal(progress.title, 'You can start building');
  assert.match(progress.detail, /turns your proposal into an application/);
  assert.match(progress.detail, /continue refining/);
  assert.deepEqual(progress.actions, [{ target: 'build', label: 'Open builder' }]);
  for (const readiness of [undefined, { context_ready: false }, { context_ready: 'true' }]) {
    assert.equal(setupProgress({ ...contextual, readiness }), null);
  }
  assert.equal(setupProgress({ ...contextual, id: null }), null);
  assert.equal(setupProgress({ ...contextual, proposal: null }), null);
});

test('stale or unrelated builds offer only a context-ready builder navigation', () => {
  const contextual = { ...guide, readiness: { context_ready: true, ready: false } };
  for (const build of [{ ...accepted, guide_id: 'other' }, { ...accepted, guide_version: 3 },
    { ...accepted, current: false }]) {
    assert.deepEqual(setupProgress(contextual, { build }).actions,
      [{ target: 'build', label: 'Open builder' }]);
    assert.equal(setupProgress(guide, { build }), null);
  }
  assert.equal(setupProgress(contextual, {}, { pendingChanges: true }), null);
  assert.equal(setupProgress(contextual, {}, { designBusy: true }), null);
  const stopped = { guide_id: 'reading', deployment_id: 'owned', status: 'stopped' };
  assert.deepEqual(setupProgress(contextual, { deployment: stopped }).actions.map(x => x.target), ['deployment']);
});

test('authored success and stale builds cannot offer installation', () => {
  assert.equal(setupProgress(guide, { build: { ...accepted, guide_id: 'other' } }), null);
  assert.equal(setupProgress(guide, { build: { ...accepted, guide_version: 3 } }), null);
  for (const override of [{ acceptance_trusted: false }, { source_verified: false },
    { acceptance: { ...accepted.acceptance, source_digest: 'other' } }, { ready_to_package: false }]) {
    assert.deepEqual(setupProgress(guide, { build: { ...accepted, ...override } }).actions.map(x => x.target), ['build']);
  }
  assert.deepEqual(setupProgress(guide, { build: accepted }).actions.map(x => x.target), ['trial', 'deployment']);
});

test('stopping and terminal cleanup offer the actual recovery step without discarding checked source evidence', () => {
  for (const status of ['queued', 'running', 'checking']) {
    const ordinary = setupProgress(guide, { build: { ...accepted, status, cleanup_pending: true, cleanup_in_progress: false } });
    assert.equal(ordinary.title, 'Your application is being built and checked');
    assert.deepEqual(ordinary.actions, [{ target: 'build', label: 'Follow the build' }]);
    assert.doesNotMatch(ordinary.detail, /closing|retry cleanup|stopping/);
    const build = { ...accepted, status, stop_requested: true };
    const progress = setupProgress(guide, { build });
    assert.equal(progress.title, 'Your builder is stopping');
    assert.match(progress.detail, /closing its runtime/);
    assert.deepEqual(progress.actions, [{ target: 'build', label: 'Follow cleanup' }]);
    assert.doesNotMatch(progress.detail, /being built and checked|Resume.*now/);
  }
  for (const status of ['completed', 'failed', 'interrupted']) {
    const build = { ...accepted, status, cleanup_pending: true, stop_requested: true };
    const before = structuredClone(build), progress = setupProgress(guide, { build });
    assert.equal(progress.title, 'Your build needs cleanup');
    assert.equal(progress.attention, true);
    assert.match(progress.detail, /retry cleanup/);
    assert.match(progress.detail, /Saved source and verification reports remain available/);
    assert.deepEqual(progress.actions, [{ target: 'build', label: 'Review cleanup' }]);
    assert.deepEqual(build, before);
  }
  const closing = setupProgress(guide, { build: { ...accepted, status: 'checking', cleanup_pending: true, cleanup_in_progress: true } });
  assert.equal(closing.title, 'Your build runtime is closing');
  assert.deepEqual(closing.actions, [{ target: 'build', label: 'Follow cleanup' }]);
  const completedClosing = setupProgress(guide, { build: { ...accepted, cleanup_pending: true, cleanup_in_progress: true } });
  assert.equal(completedClosing.title, 'Your build runtime is closing');
  assert.deepEqual(completedClosing.actions, [{ target: 'build', label: 'Follow cleanup' }]);
  assert.doesNotMatch(completedClosing.detail, /retry cleanup/);
  assert.notEqual(completedClosing.attention, true);
  const completed = { ...accepted, stop_requested: true, cleanup_pending: false };
  assert.deepEqual(setupProgress(guide, { build: completed }).actions.map(item => item.target), ['trial', 'deployment']);
  assert.equal(completed.source_digest, accepted.source_digest);
  assert.deepEqual(completed.acceptance, accepted.acceptance);
  const changedGuide = { ...guide, version: guide.version + 1, readiness: { context_ready: true } };
  const staleCleanup = { ...accepted, current: false, cleanup_pending: true };
  assert.deepEqual(setupProgress(changedGuide, { build: staleCleanup }).actions,
    [{ target: 'build', label: 'Review cleanup' }]);
  assert.equal(setupProgress(changedGuide, { build: staleCleanup }).title, 'Your build needs cleanup');
  assert.deepEqual(setupProgress(changedGuide, { build: { ...staleCleanup, guide_id: 'other' } }).actions,
    [{ target: 'build', label: 'Open builder' }]);
});

test('pending builder cleanup leaves installed operations available and never offers a new trial or install', () => {
  const contextual = { ...guide, readiness: { context_ready: true } };
  const build = { ...accepted, build_id: 'candidate', cleanup_pending: true, stop_requested: true };
  const stopped = { guide_id: guide.id, deployment_id: 'owned', build_id: 'installed', status: 'stopped' };
  const running = { ...stopped, status: 'running', process_live: true, health: 'ready',
    console_url: 'http://127.0.0.1:18789/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  const before = structuredClone({ build, stopped, running });
  const stoppedProgress = setupProgress(contextual, { build, deployment: stopped });
  assert.equal(stoppedProgress.title, 'Your build needs cleanup');
  assert.deepEqual(stoppedProgress.actions, [{ target: 'build', label: 'Review cleanup' },
    { target: 'deployment', label: 'Manage application' }]);
  const runningProgress = setupProgress(contextual, { build, deployment: running });
  assert.equal(runningProgress.title, 'Your installed custom application is running');
  assert.deepEqual(runningProgress.actions, [{ target: 'deployment', label: 'Use your application' },
    { target: 'console', label: 'Administration Console', url: running.console_url },
    { target: 'build', label: 'Review cleanup' }]);
  assert.match(runningProgress.detail, /installed application stays unchanged/);
  assert.match(runningProgress.detail, /retry cleanup/);
  assert.doesNotMatch(runningProgress.detail, /preparing and checking an improvement/);
  const ordinary = setupProgress(contextual, { build: { ...build, status: 'running', stop_requested: false,
    cleanup_in_progress: false }, deployment: running });
  assert.match(ordinary.detail, /preparing and checking an improvement/);
  assert.doesNotMatch(ordinary.detail, /closing|retry cleanup|stopping/);
  assert.deepEqual(ordinary.actions, runningProgress.actions.slice(0, 2));
  assert.deepEqual({ build, stopped, running }, before);
  const settled = { ...build, cleanup_pending: false };
  assert.deepEqual(setupProgress(contextual, { build: settled, deployment: stopped }).actions,
    [{ target: 'deployment', label: 'Manage application' }, { target: 'trial', label: 'Try the newer application', mode: 'application' }]);
});

test('an already admitted application trial remains current while cleanup pauses new admissions', () => {
  const build = { ...accepted, cleanup_pending: true };
  const trial = { rehearsal_id: 'trial', guide_id: guide.id, guide_version: guide.version, current: true,
    mode: 'application', status: 'waiting_input', keep_open: true, can_continue: true, turn_count: 1,
    application_source: { build_id: build.build_id, source_digest: build.source_digest } };
  const progress = setupProgress(guide, { build, trial });
  assert.equal(progress.title, 'Your trial is waiting for a follow-up');
  assert.match(progress.detail, /Continue the same isolated conversation/);
  assert.match(progress.detail, /retry cleanup/);
  assert.doesNotMatch(progress.detail, /earlier trial|cannot continue/);
  assert.deepEqual(progress.actions, [{ target: 'trial', label: 'Continue trial' },
    { target: 'build', label: 'Review cleanup' }]);
});

test('only a completed application trial on the exact source changes the next step', () => {
  const trial = { guide_id: 'reading', guide_version: 4, current: true, mode: 'application', status: 'completed',
    application_source: { build_id: 'build', source_digest: 'source' } };
  const progress = setupProgress(guide, { build: accepted, trial });
  assert.equal(progress.actions[0].label, 'Review trial');
  assert.match(progress.detail, /does not establish/);
  for (const override of [{ mode: 'design' }, { guide_id: 'other' }, { guide_version: 3 },
    { application_source: { build_id: 'build', source_digest: 'old' } }]) {
    assert.equal(setupProgress(guide, { build: accepted, trial: { ...trial, ...override } }).actions[0].label, 'Try the application');
  }
});

test('application trial actions select callbacks while active trials lead to existing activity', () => {
  assert.equal(setupProgress(guide, { build: accepted }).actions[0].mode, 'application');
  const trial = { rehearsal_id: 'trial', guide_id: guide.id, guide_version: guide.version, current: true,
    mode: 'application', status: 'running', application_source: { build_id: accepted.build_id, source_digest: accepted.source_digest } };
  for (const status of ['queued', 'starting', 'running', 'canceling']) {
    const progress = setupProgress(guide, { build: accepted, trial: { ...trial, status } });
    assert.deepEqual(progress.actions, [{ target: 'trial', label: 'Follow trial' }]);
    assert.match(progress.detail, status === 'canceling' ? /workers stop/ : /does not test your custom UI or live connections/);
    assert.equal(progress.title, status === 'canceling' ? 'Your trial is stopping' : 'Your application trial is running');
  }
  const design = setupProgress(guide, { build: accepted, trial: { ...trial, mode: 'design' } });
  assert.equal(design.title, 'Your design trial is running');
  assert.match(design.detail, /not part of this design trial/);
  const stale = setupProgress(guide, { build: accepted,
    trial: { ...trial, application_source: { ...trial.application_source, source_digest: 'old' } } });
  assert.equal(stale.title, 'An earlier trial is still running');
  assert.deepEqual(stale.actions, [{ target: 'trial', label: 'Follow trial' }]);
  assert.equal(setupProgress(guide, { build: accepted, trial: { ...trial, guide_id: 'other' } }).actions[0].label, 'Try the application');
  assert.equal(setupProgress(guide, { build: accepted, trial }, { pendingChanges: true }), null);
  assert.equal(setupProgress(guide, { build: accepted, trial }, { designBusy: true }), null);
});

const openTrial = { rehearsal_id: 'open-trial', guide_id: guide.id, guide_version: guide.version, current: true,
  mode: 'application', keep_open: true, status: 'waiting_input', can_continue: true, turn_count: 1,
  worker_stop_confirmed: false, application_source: { build_id: accepted.build_id, source_digest: accepted.source_digest } };

test('waiting application callback failures lead to End then repair, keeping cleanup gates intact', () => {
  const failed = { ...openTrial, application_observations: [{ kind: 'callback_failed' }] };
  const result = setupProgress(guide, { build: accepted, trial: failed });
  assert.equal(result.title, 'Your trial needs an application fix');
  assert.match(result.detail, /End this trial; once cleanup is confirmed, choose Fix this trial/);
  assert.deepEqual(result.actions, [{ target: 'trial', label: 'Review trial problem' }]);
  const earlier = setupProgress(guide, { build: accepted, trial: { ...failed, current: false } });
  assert.equal(earlier.title, 'An earlier trial is still open');
  assert.doesNotMatch(earlier.detail, /Fix this trial/);
});

test('only a current host-eligible waiting trial offers continuation navigation without claiming active work', () => {
  const result = setupProgress(guide, { build: accepted, trial: openTrial });
  assert.equal(result.title, 'Your trial is waiting for a follow-up');
  assert.match(result.detail, /same isolated conversation and sample state/);
  assert.deepEqual(result.actions, [{ target: 'trial', label: 'Continue trial' }]);
  assert.doesNotMatch(result.title + result.detail, /trial is running|also running|your job is complete/);
  for (const change of [{ can_continue: false }, { can_continue: undefined }, { keep_open: false },
    { worker_stop_confirmed: true }, { turn_count: 0 }]) {
    const blocked = setupProgress(guide, { build: accepted, trial: { ...openTrial, ...change } });
    assert.deepEqual(blocked.actions, [{ target: 'trial', label: 'Follow trial' }]);
    assert.match(blocked.detail, /cannot accept a follow-up now/);
  }
  const stale = setupProgress(guide, { build: accepted, trial: { ...openTrial, current: false } });
  assert.equal(stale.title, 'An earlier trial is still open');
  assert.match(stale.detail, /cannot continue with the current design/);
  assert.deepEqual(stale.actions, [{ target: 'trial', label: 'Follow trial' }]);
  const changed = setupProgress(guide, { build: { ...accepted, source_digest: 'new-source' }, trial: openTrial });
  assert.equal(changed.title, stale.title);
  assert.deepEqual(changed.actions, stale.actions);
  assert.equal(setupProgress(guide, { build: accepted, trial: { ...openTrial, guide_id: 'other' } }).actions[0].label, 'Try the application');
});

test('settling and ending describe actual native lanes and retain follow-only navigation', () => {
  const settling = setupProgress(guide, { build: accepted, trial: { ...openTrial, status: 'running', session_status: 'settling' } });
  assert.equal(settling.title, 'Your trial is settling');
  assert.match(settling.detail, /Additional native work is still settling/);
  assert.deepEqual(settling.actions, [{ target: 'trial', label: 'Follow trial' }]);
  for (const status of ['ending', 'canceling']) {
    const progress = setupProgress(guide, { build: accepted, trial: { ...openTrial, status } });
    assert.equal(progress.title, status === 'ending' ? 'Your trial is ending' : 'Your trial is stopping');
    assert.match(progress.detail, /workers stop/);
    assert.deepEqual(progress.actions, [{ target: 'trial', label: 'Follow trial' }]);
    assert.doesNotMatch(progress.detail, /workers have stopped|Continue the same/);
  }
});

test('installed runtime and candidate narration remain primary alongside waiting, settling or ending trials', () => {
  const deployment = { guide_id: guide.id, deployment_id: 'owned', build_id: 'installed', status: 'running',
    process_live: true, health: 'ready', console_url: 'http://127.0.0.1:18789/console',
    bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  for (const build of [accepted, { ...accepted, status: 'running' }, { ...accepted, status: 'failed' }]) {
    const baseline = setupProgress(guide, { build, deployment });
    for (const trial of [openTrial, { ...openTrial, current: false },
      { ...openTrial, status: 'running', session_status: 'settling' }, { ...openTrial, status: 'ending' }]) {
      const result = setupProgress(guide, { build, deployment, trial });
      assert.equal(result.title, baseline.title);
      assert.ok(result.detail.startsWith(baseline.detail));
      assert.deepEqual(result.actions.slice(0, -1), baseline.actions);
      const expectedContinue = build.status !== 'running' && trial === openTrial;
      assert.equal(result.actions.at(-1).label, expectedContinue ? 'Continue trial' : 'Follow trial');
      if (trial.status === 'waiting_input') assert.match(result.detail, /waiting/);
      if (trial.session_status === 'settling') assert.match(result.detail, /settling its remaining native work/);
      if (trial.status === 'ending') assert.match(result.detail, /saved trial is ending/);
      assert.doesNotMatch(result.detail, /also running/);
    }
  }
});

test('completed turns with unavailable or failed callbacks lead to trial recovery without implying installation readiness', () => {
  const trial = { rehearsal_id: 'trial', guide_id: guide.id, guide_version: guide.version, current: true,
    mode: 'application', status: 'completed', application_source: { build_id: accepted.build_id, source_digest: accepted.source_digest } };
  for (const issue of [{ application_trial: { available: false } }, { application_observations: [{ kind: 'callback_failed' }] },
    { status: 'failed' }, { status: 'interrupted' }, { status: 'unavailable' }]) {
    const progress = setupProgress(guide, { build: accepted, trial: { ...trial, ...issue } });
    assert.equal(progress.title, 'Your application trial needs attention');
    assert.deepEqual(progress.actions, [{ target: 'trial', label: 'Review trial', mode: 'application' }]);
  }
  const progress = setupProgress(guide, { build: accepted, trial });
  assert.equal(progress.title, 'Your application trial has finished');
  assert.match(progress.detail, /does not establish/);
  assert.equal(progress.actions[0].mode, 'application');
  assert.equal(progress.actions[1].target, 'deployment');
  for (const changed of [{ ...trial, current: false }, { ...trial, guide_version: 3 },
    { ...trial, application_source: { build_id: 'other', source_digest: accepted.source_digest } }])
    assert.equal(setupProgress(guide, { build: accepted, trial: { ...changed, application_trial: { available: false } } }).actions[0].label,
      'Try the application');
});

test('existing installations retain their primary operation while exposing owned active trials', () => {
  const trial = { rehearsal_id: 'trial', guide_id: guide.id, guide_version: guide.version, current: true,
    mode: 'application', status: 'running', application_source: { build_id: accepted.build_id, source_digest: accepted.source_digest } };
  const stopped = { guide_id: guide.id, deployment_id: 'owned', build_id: accepted.build_id, status: 'stopped' };
  const running = { ...stopped, status: 'running', process_live: true, health: 'ready',
    console_url: 'http://127.0.0.1:18789/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  for (const deployment of [stopped, running, { ...stopped, status: 'failed' }, { ...stopped, status: 'installing' }]) {
    const without = setupProgress(guide, { build: accepted, deployment });
    const withTrial = setupProgress(guide, { build: accepted, deployment, trial });
    assert.equal(withTrial.title, without.title);
    assert.deepEqual(withTrial.actions.slice(0, -1), without.actions.filter(item => item.target !== 'trial'));
    assert.deepEqual(withTrial.actions.at(-1), { target: 'trial', label: 'Follow trial' });
    assert.match(withTrial.detail, /trial of this proposal is also running/);
    assert.deepEqual(setupProgress(guide, { build: accepted, deployment, trial: { ...trial, guide_id: 'other' } }), without);
    assert.deepEqual(setupProgress(guide, { build: accepted, deployment, trial: { ...trial, status: 'completed' } }), without);
  }
  assert.match(setupProgress(guide, { build: accepted, deployment: running,
    trial: { ...trial, current: false } }).detail, /earlier trial is also running/);
  assert.match(setupProgress(guide, { build: accepted, deployment: stopped,
    trial: { ...trial, status: 'canceling' } }).detail, /saved trial is stopping/);
});

test('a stopped installation keeps management first and offers only a current checked application trial', () => {
  const contextual = { ...guide, readiness: { context_ready: true } };
  const deployment = { guide_id: guide.id, deployment_id: 'owned', build_id: 'old', status: 'stopped' };
  const progress = setupProgress(contextual, { build: accepted, deployment });
  assert.equal(progress.title, 'Your installed application is stopped');
  assert.match(progress.detail, /stopped installed source/);
  assert.match(progress.detail, /trial uses that newer candidate and does not update this installation/);
  assert.deepEqual(progress.actions, [{ target: 'deployment', label: 'Manage application' },
    { target: 'trial', label: 'Try the newer application', mode: 'application' }]);
  for (const build of [{ ...accepted, current: false }, { ...accepted, guide_id: 'other' },
    { ...accepted, source_verified: false }, { ...accepted, acceptance_trusted: false },
    { ...accepted, ready_to_package: false }])
    assert.deepEqual(setupProgress(contextual, { build, deployment }).actions,
      [{ target: 'deployment', label: 'Manage application' }]);
  assert.equal(setupProgress(guide, { build: accepted, deployment }).actions.length, 1);
  assert.equal(setupProgress(contextual, { build: accepted, deployment }, { pendingChanges: true }), null);
  assert.equal(setupProgress(contextual, { build: accepted, deployment }, { designBusy: true }), null);
  const trial = { rehearsal_id: 'trial', guide_id: guide.id, guide_version: 3, current: false,
    mode: 'application', status: 'running', application_source: { build_id: 'old', source_digest: 'old' } };
  assert.deepEqual(setupProgress(contextual, { build: accepted, deployment, trial }).actions,
    [{ target: 'deployment', label: 'Manage application' }, { target: 'trial', label: 'Follow trial' }]);
});

test('stopped installed source remains distinct from failed, unchecked, same-source and unrelated candidates', () => {
  const current = { ...guide, readiness: { context_ready: true } };
  const deployment = { guide_id: guide.id, deployment_id: 'owned', build_id: 'installed', status: 'stopped' };
  for (const change of [{ status: 'failed' }, { status: 'interrupted' }, { source_verified: false },
    { ready_to_package: false }, { acceptance_trusted: false }]) {
    const progress = setupProgress(current, { deployment, build: { ...accepted, ...change } });
    assert.match(progress.detail, /newer candidate still needs work/);
    assert.match(progress.detail, /separate from this installation/);
    assert.deepEqual(progress.actions, [{ target: 'deployment', label: 'Manage application' }]);
    assert.doesNotMatch(progress.detail, /passed independent offline/);
  }
  const same = setupProgress(current, { deployment, build: { ...accepted, build_id: deployment.build_id } });
  assert.deepEqual(same.actions, [{ target: 'deployment', label: 'Manage application' },
    { target: 'trial', label: 'Try the application', mode: 'application' }]);
  assert.doesNotMatch(same.detail, /newer|candidate/);
  for (const change of [{ guide_id: 'other' }, { guide_version: 3 }, { current: false }]) {
    const progress = setupProgress(current, { deployment, build: { ...accepted, ...change } });
    assert.doesNotMatch(progress.detail, /newer|candidate/);
    assert.deepEqual(progress.actions, [{ target: 'deployment', label: 'Manage application' }]);
  }
});

test('only the exact restored transaction and current checked candidate direct attention to recovery findings', () => {
  const current = { ...guide, readiness: { context_ready: true } };
  const build = { ...accepted, source_digest: 'a'.repeat(64),
    acceptance: { ...accepted.acceptance, source_digest: 'a'.repeat(64) } };
  const update = { update_id: 'e7972227-024d-4741-b0b6-618dbdf3b53f', status: 'rolled-back',
    from_build_id: 'installed', to_build_id: build.build_id, guide_version: build.guide_version,
    source_digest: build.source_digest, previous_source_digest: 'b'.repeat(64),
    package_sha256: 'c'.repeat(64), previous_package_sha256: 'd'.repeat(64) };
  const stopped = { guide_id: guide.id, guide_version: 3, deployment_id: 'owned', build_id: 'installed',
    package_sha256: update.previous_package_sha256, status: 'stopped', update };
  const before = structuredClone({ current, build, stopped });
  const result = setupProgress(current, { build, deployment: stopped });
  assert.match(result.detail, /previous installed source was restored and is now stopped/);
  assert.deepEqual(result.actions, [{ target: 'deployment', label: 'Review recovery findings' },
    { target: 'trial', label: 'Try the newer application', mode: 'application' }]);
  const running = { ...stopped, status: 'running', process_live: true, health: 'ready',
    console_url: 'http://127.0.0.1:18789/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  const live = setupProgress(current, { build, deployment: running });
  assert.match(live.title, /previous installed custom application is running/);
  assert.match(live.detail, /separate from this running source/);
  assert.equal(live.actions[0].label, 'Review recovery findings');
  assert.equal(live.actions[1].target, 'console');
  for (const change of [{ update_id: '' }, { status: 'completed' }, { from_build_id: 'other' },
    { to_build_id: 'old-candidate' }, { source_digest: 'e'.repeat(64) }, { guide_version: 3 },
    { previous_package_sha256: 'e'.repeat(64) }, { previous_source_digest: '' }, { package_sha256: '' }]) {
    assert.equal(setupProgress(current, { build, deployment: { ...stopped, update: { ...update, ...change } } })
      .actions[0].label, 'Manage application');
  }
  for (const change of [{ current: false }, { source_verified: false }, { guide_version: 3 },
    { build_id: 'next-candidate' }, { source_digest: 'e'.repeat(64) }]) {
    assert.equal(setupProgress(current, { build: { ...build, ...change }, deployment: stopped })
      .actions[0].label, 'Manage application');
  }
  assert.equal(setupProgress(current, { build, deployment: { ...stopped, source_digest: 'e'.repeat(64) } })
    .actions[0].label, 'Manage application');
  const open = setupProgress(current, { build, deployment: stopped, trial: {
    ...openTrial, application_source: { build_id: build.build_id, source_digest: build.source_digest } } });
  assert.deepEqual(open.actions, [{ target: 'deployment', label: 'Review recovery findings' },
    { target: 'trial', label: 'Continue trial' }]);
  assert.deepEqual({ current, build, stopped }, before);
  assert.doesNotMatch(JSON.stringify(result.actions), /retry|apply|rollback/);
});

test('current lifecycle narration supersedes the retained design heading without changing its content or choices', () => {
  const state = { ...guide, question: { title: 'Ready to build this application?', description: 'Keep this exact saved description.',
    options: [{ id: 'build', label: 'Build this application' }], multiple: false } };
  const before = structuredClone(state), progress = setupProgress(state, { build: accepted });
  assert.equal(setupProgressHasLifecycle(state, { build: accepted }), true);
  assert.equal(setupProgressHasLifecycle(state, { trial: openTrial }), true);
  assert.equal(setupProgressHasLifecycle(state, { deployment: { guide_id: state.id, deployment_id: 'owned' } }), true);
  for (const receipts of [{}, { build: { ...accepted, guide_id: 'other' } }, { build: { ...accepted, current: false } },
    { trial: { ...openTrial, guide_id: 'other' } }, { deployment: { guide_id: 'other', deployment_id: 'owned' } }])
    assert.equal(setupProgressHasLifecycle(state, receipts), false);
  assert.equal(guideStatusText({ state, question: state.question, ready: true, progress }), progress.title);
  const card = renderToStaticMarkup(React.createElement(SetupProgressCard, { progress, primary: true, Icon: () => null }));
  const heading = renderToStaticMarkup(React.createElement(SetupGuideQuestionHeading, { question: state.question, lifecycle: true }));
  const caption = renderToStaticMarkup(React.createElement(SetupGuideStatusCaption,
    { lifecycle: true, statusText: progress.title, ready: true }));
  assert.ok(card.includes(`<h1>${progress.title}</h1>`));
  assert.match(heading, /DESIGN CONVERSATION/);
  assert.match(heading, /<h2>Ready to build this application\?<\/h2>/);
  assert.doesNotMatch(heading, /This saved design question|Current application status above/);
  assert.ok(heading.includes(state.question.description));
  assert.ok(caption.includes(progress.title));
  assert.doesNotMatch(heading, /<h1>|disabled|type="submit"/);
  assert.deepEqual(state, before);
  const first = renderToStaticMarkup(React.createElement(SetupGuideQuestionHeading, { question: state.question }));
  assert.match(first, /<h1>Ready to build this application\?<\/h1>/);
  assert.doesNotMatch(first, /DESIGN CONVERSATION/);
  const plain = renderToStaticMarkup(React.createElement(SetupGuideStatusCaption, { ready: true }));
  assert.match(plain, /Plan accepted/);
});

test('busy, transient error and exact saved guide failure remain primary over application progress', () => {
  const progress = setupProgress(guide, { build: accepted });
  const state = { ...guide, operation: { status: 'running', detail: 'Checking your new guide answer.' } };
  assert.equal(guideStatusText({ state, progress, busy: true }), state.operation.detail);
  assert.equal(setupProgress(state, { build: accepted }, { designBusy: true }), null);
  assert.equal(setupProgressHasLifecycle(state, { build: accepted }), true);
  assert.match(renderToStaticMarkup(React.createElement(SetupGuideStatusCaption,
    { lifecycle: true, statusText: guideStatusText({ state, busy: true }) })), /Checking your new guide answer/);
  const waitingHeading = renderToStaticMarkup(React.createElement(SetupGuideQuestionHeading,
    { question: { title: 'Your saved question', description: 'Keep this exact description.' }, lifecycle: true, waiting: true }));
  assert.match(waitingHeading, /SAVED QUESTION/);
  assert.match(waitingHeading, /Your guide is responding\. Choices are paused; you can keep writing below\./);
  assert.match(waitingHeading, /Keep this exact description\./);
  for (const status of ['failed', 'interrupted']) {
    const failure = { ...state, operation: { status, detail: 'Your guide answer could not be published.' } };
    const text = guideStatusText({ state: failure, progress });
    assert.equal(text, failure.operation.detail);
    assert.match(renderToStaticMarkup(React.createElement(SetupGuideStatusCaption,
      { lifecycle: true, statusText: text })), /Your guide answer could not be published/);
  }
  for (const errors of [{ error: 'Offline' }, { appError: 'Application request failed' }]) {
    assert.equal(guideStatusText({ state, progress, ...errors }),
      'The guide needs your attention. Your saved setup is preserved.');
  }
});

test('returning-user status anchor keeps the exact first admitted action and labels retained history', () => {
  const progress = setupProgress(guide, { deployment: { guide_id: guide.id, deployment_id: 'owned', status: 'stopped' } });
  const props = { progress, statusText: progress.title, disabled: true, Icon: () => null };
  const html = renderToStaticMarkup(React.createElement(SetupCurrentStatusAnchor, props));
  assert.match(html, /CURRENT APPLICATION STATUS/);
  assert.ok(html.includes(`<h2>${progress.title}</h2>`));
  assert.ok(html.includes(progress.actions[0].label));
  assert.match(html, /<button type="button" disabled=""/);
  assert.equal((html.match(/<button/g) || []).length, 1);
  assert.match(html, /Conversation history/);
  assert.match(html, /Earlier messages are kept as written\. Current status is above\./);
  assert.doesNotMatch(html, /aria-live|status-dot/);
  const consoleProgress = { ...progress, actions: [{ target: 'console', url: 'http://127.0.0.1:4110/console', label: 'Existing Console' },
    { target: 'deployment', label: 'A separate action' }] };
  const linked = renderToStaticMarkup(React.createElement(SetupCurrentStatusAnchor,
    { ...props, progress: consoleProgress, disabled: false }));
  assert.match(linked, /href="http:\/\/127\.0\.0\.1:4110\/console" target="_blank" rel="noreferrer"/);
  assert.doesNotMatch(linked, /A separate action/);
});

test('returning-user status anchor preserves unknown status and yields to guide busy, error and saved failure', () => {
  const progress = setupProgress(guide, {}, { deploymentObservation: 'unknown' });
  const props = { progress, statusText: progress.title, Icon: () => null };
  const html = renderToStaticMarkup(React.createElement(SetupCurrentStatusAnchor, props));
  assert.match(html, /Installation status is unavailable/);
  assert.match(html, /Review installation status/);
  assert.doesNotMatch(html, /is running|is stopped/);
  for (const status of [guideStatusText({ state: guide, progress, busy: true }),
    guideStatusText({ state: guide, progress, error: 'Offline' }),
    guideStatusText({ state: { ...guide, operation: { status: 'failed', detail: 'Saved response failed.' } }, progress })]) {
    assert.equal(renderToStaticMarkup(React.createElement(SetupCurrentStatusAnchor,
      { ...props, statusText: status })), '');
  }
  assert.equal(renderToStaticMarkup(React.createElement(SetupCurrentStatusAnchor,
    { ...props, progress: null })), '');
});

test('a saved refinement has no artificial question or accepted-plan claim', () => {
  const state = { ...guide, status: 'refining', question: null,
    readiness: { context_ready: true, ready: false, decision_blockers: [],
      implementation_blockers: ['Implementation still required: Staff UI.'] } };
  const before = structuredClone(state);
  assert.equal(guideStatusText({ state }), 'Your updated draft is saved. Keep refining it, or explore the next step below.');
  assert.deepEqual(setupProgress(state).actions, [{ target: 'build', label: 'Build application' }]);
  assert.equal(guideStatusText({ state, progress: setupProgress(state, { build: accepted }) }),
    setupProgress(state, { build: accepted }).title);
  assert.deepEqual(state, before);
});

test('a process alone cannot claim the installed native runtime is ready', () => {
  const deployment = { guide_id: 'reading', deployment_id: 'install', build_id: 'build', status: 'running',
    process_live: true, health: 'ready', console_url: 'http://127.0.0.1:18789/console',
    bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  const progress = setupProgress(guide, { build: accepted, deployment });
  assert.deepEqual(progress.actions, [{ target: 'deployment', label: 'Use your application' },
    { target: 'console', label: 'Administration Console', url: deployment.console_url }]);
  const navigations = [], props = { progress, onNavigate: (item) => navigations.push(item), Icon: () => null };
  const html = renderToStaticMarkup(React.createElement(SetupProgressCard, props));
  assert.ok(html.indexOf('Use your application') < html.indexOf('Administration Console'));
  assert.doesNotMatch(html, /href="undefined"|[?&](task|token|identity)=/);
  assert.deepEqual(navigations, []);
  const controls = React.Children.toArray(SetupProgressCard(props).props.children.at(-1).props.children);
  controls.find((item) => item.type === 'button').props.onClick();
  assert.deepEqual(navigations, [{ target: 'deployment', label: 'Use your application' }]);
  for (const override of [{ health: 'unknown' }, { process_live: false },
    { bootstrap: { complete: true, ready: true, counts: { broken: 1 } } },
    { console_url: 'https://external.example/console' }]) {
    assert.deepEqual(setupProgress(guide, { deployment: { ...deployment, ...override } }).actions.map(x => x.target), ['deployment']);
  }
  assert.match(setupProgress(guide, { build: { ...accepted, build_id: 'new' }, deployment }).detail,
    /newer application passed independent offline checks/);
});

test('running installed source distinguishes preparing and unaccepted candidates from a checked update', () => {
  const deployment = { guide_id: guide.id, deployment_id: 'owned', build_id: 'installed', status: 'running',
    process_live: true, health: 'ready', console_url: 'http://127.0.0.1:18789/console',
    bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  const base = setupProgress(guide, { build: { ...accepted, build_id: deployment.build_id }, deployment });
  for (const status of ['queued', 'running', 'checking']) {
    const result = setupProgress(guide, { build: { ...accepted, build_id: 'candidate', status }, deployment });
    assert.match(result.detail, /builder is preparing and checking/);
    assert.match(result.detail, /installed application stays unchanged/);
    assert.doesNotMatch(result.detail, /passed independent|build is available/);
    assert.deepEqual(result.actions, base.actions);
  }
  for (const change of [{ status: 'failed' }, { status: 'interrupted' }, { ready_to_package: false },
    { acceptance_trusted: false }, { acceptance_current: false }, { source_verified: false },
    { acceptance: { ...accepted.acceptance, source_digest: 'another-source' } },
    { acceptance: { ...accepted.acceptance, status: 'needs_repair' } }]) {
    const result = setupProgress(guide, { build: { ...accepted, build_id: 'candidate', ...change }, deployment });
    assert.match(result.detail, /candidate still needs work/);
    assert.doesNotMatch(result.detail, /passed independent|build is available/);
    assert.deepEqual(result.actions, base.actions);
  }
  const checked = setupProgress(guide, { build: { ...accepted, build_id: 'candidate' }, deployment });
  assert.match(checked.detail, /passed independent offline checks/);
  assert.match(checked.detail, /Review it before updating/);
  assert.deepEqual(checked.actions, base.actions);
  for (const change of [{ guide_id: 'another' }, { guide_version: 3 }, { current: false }]) {
    const result = setupProgress(guide, { build: { ...accepted, build_id: 'candidate', ...change }, deployment });
    assert.equal(result.detail, base.detail);
  }
});

test('an older installed version remains manageable after a proposal changes', () => {
  const deployment = { guide_id: 'reading', guide_version: 3, current: false, deployment_id: 'owned', status: 'stopped' };
  assert.equal(setupProgress(guide, { deployment }).actions[0].target, 'deployment');
  assert.equal(setupProgress(guide, { deployment: { ...deployment, guide_id: 'other' } }), null);
  assert.equal(setupProgress(guide, { build: accepted }, { pendingChanges: true }), null);
  assert.equal(setupProgress(guide, { build: accepted }, { designBusy: true }), null);
});

test('an earlier failed installation does not hide a current repair build', () => {
  const deployment = {guide_id: 'reading', deployment_id: 'owned', status: 'failed', build_id: 'old'};
  const build = {...accepted, status: 'running', ready_to_package: false};
  assert.deepEqual(setupProgress(guide, {build, deployment}).actions.map(x => x.target), ['build', 'deployment']);
  const earlier = setupProgress(guide, {build: {...build, guide_version: 3}, deployment});
  assert.deepEqual(earlier.actions.map(x => x.target), ['build', 'deployment']);
  assert.equal(earlier.actions[0].label, 'Follow earlier build');
});

test('the guide resume requires real scoped lifecycle progress and never treats status text as evidence', () => {
  assert.equal(setupGuideResume({ ...guide, readiness: { context_ready: true } }), null);
  for (const receipts of [{ build: accepted }, { build: { ...accepted, status: 'running' } },
    { build: { ...accepted, status: 'failed' } }, { trial: openTrial },
    { deployment: { guide_id: guide.id, deployment_id: 'owned', status: 'failed' } },
    { deployment: { guide_id: guide.id, deployment_id: 'owned', status: 'stopped', current: false } }]) {
    assert.deepEqual(setupGuideResume(guide, receipts),
      { guide_id: guide.id, guide_version: guide.version, text: GUIDE_RESUME_PROMPT });
  }
  for (const receipts of [{ build: { ...accepted, guide_id: 'other' } }, { build: { ...accepted, guide_version: 3 } },
    { trial: { ...openTrial, guide_id: 'other' } }, { deployment: { guide_id: 'other', deployment_id: 'other', status: 'running' } }])
    assert.equal(setupGuideResume({ ...guide, readiness: { context_ready: true } }, receipts), null);
  assert.match(GUIDE_RESUME_PROMPT, /actual saved build, trial and installation evidence/);
  assert.match(GUIDE_RESUME_PROMPT, /preserving my requirements and previous choices/);
  assert.match(GUIDE_RESUME_PROMPT, /Do not infer that my real job succeeded/);
  const noisy = { ...accepted, detail: 'DO_NOT_COPY_APP_CLAIM: every job succeeded', logs: ['PRIVATE_LOG'] };
  assert.doesNotMatch(setupGuideResume(guide, { build: noisy }).text, /DO_NOT_COPY_APP_CLAIM|PRIVATE_LOG/);
});

test('busy, manual, loading, switched-guide and mismatched-question clicks never send a resume', () => {
  const receipts = { build: accepted }, resume = setupGuideResume(guide, receipts), input = {
    key: inputKey(guide), draft: 'My pending thought', selected: ['keep'], mode: 'answer' };
  const calls = [];
  for (const flags of [{ pendingChanges: true }, { designBusy: true }, { setupLoading: true },
    { processing: true }, { packageBusy: true }, { active: false }]) {
    assert.equal(setupGuideResume(guide, receipts, flags), null);
    assert.equal(admitSetupGuideResume(resume, guide, receipts, flags, input, (...args) => calls.push(args)), false);
  }
  for (const current of [{ ...guide, id: 'different' }, { ...guide, version: 5 },
    { ...guide, operation: { status: 'running' } }, { ...guide, version: true }])
    assert.equal(admitSetupGuideResume(resume, current, receipts, {}, input, (...args) => calls.push(args)), false);
  assert.equal(admitSetupGuideResume(resume, guide, receipts, {}, { ...input, key: inputKey({ ...guide,
    question: { stage: 'review', title: 'An older question' } }) }, (...args) => calls.push(args)), false);
  assert.deepEqual(calls, []);
});

test('the production status card sends one explicit normal chat request and keeps pending answer data local', () => {
  const receipts = { build: accepted }, progress = setupProgress(guide, receipts), resume = setupGuideResume(guide, receipts);
  const input = { key: inputKey(guide), draft: 'My unsent \u00e5 thought', selected: ['private'], mode: 'answer' };
  const before = structuredClone(input), sends = [], navigations = [];
  const props = { progress, resume, disabled: false, Icon: () => null,
    onNavigate: item => navigations.push(item),
    onResume: item => admitSetupGuideResume(item, guide, receipts, {}, input, (...args) => sends.push(args)) };
  const html = renderToStaticMarkup(React.createElement(SetupProgressCard, props));
  assert.match(html, /Current application status/);
  assert.match(html, /Ask the guide what is next/);
  assert.deepEqual(sends, []);
  const controls = React.Children.toArray(SetupProgressCard(props).props.children.at(-1).props.children);
  const button = controls.find(item => item.props?.children?.[0] === 'Ask the guide what is next ');
  assert.equal(button.props.type, 'button');
  button.props.onClick();
  assert.deepEqual(sends, [['chat', { text: GUIDE_RESUME_PROMPT }, {
    preserveInput: true, expectedGuide: { id: guide.id, version: guide.version } }]]);
  assert.doesNotMatch(JSON.stringify(sends[0][1]), /My unsent|private|preserveInput|expectedGuide/);
  assert.deepEqual(input, before);
  assert.deepEqual(navigations, []);
  const disabled = SetupProgressCard({ ...props, disabled: true });
  const disabledControls = React.Children.toArray(disabled.props.children.at(-1).props.children);
  assert.equal(disabledControls.find(item => item.props?.children?.[0] === 'Ask the guide what is next ').props.disabled, true);
});

const pausedDiscovery = { ...guide, status: 'refining', question: null,
  operation: { status: 'completed' }, readiness: { context_ready: false, ready: false,
    decision_blockers: ['Resolve destination or show an explicit proposed default.'] } };

test('a saved refinement with unfinished context explicitly resumes discovery without changing readiness', () => {
  const before = structuredClone(pausedDiscovery), progress = setupProgress(pausedDiscovery);
  assert.deepEqual(progress, { kind: 'discovery', title: 'Continue shaping your setup',
    detail: "Resume the guide's questions from your saved proposal, earlier answers and manual edits.", actions: [] });
  assert.equal(setupProgressHasLifecycle(pausedDiscovery), false);
  assert.deepEqual(setupGuideResume(pausedDiscovery), { guide_id: guide.id, guide_version: guide.version,
    text: SETUP_DISCOVERY_PROMPT });
  assert.match(SETUP_DISCOVERY_PROMPT, /saved proposal, brief, earlier answers and manual edits/);
  assert.match(SETUP_DISCOVERY_PROMPT, /context-derived multiple-choice question/);
  assert.match(SETUP_DISCOVERY_PROMPT, /Preserve my confirmed choices and manual changes/);
  assert.match(SETUP_DISCOVERY_PROMPT, /provisional defaults/);
  assert.doesNotMatch(SETUP_DISCOVERY_PROMPT, /GitHub|Telegram|Slack|GCP|Mac|install|running|succeeded/);
  assert.deepEqual(pausedDiscovery, before);
});

test('discovery continuation is hidden for answered, ready, failed or unowned guide states', () => {
  for (const change of [{ id: null }, { proposal: null }, { status: 'ready' }, { status: 'review' },
    { status: 'collecting' }, { question: { title: 'The active decision', options: [] } },
    { readiness: undefined }, { readiness: { context_ready: 'false' } },
    { operation: { status: 'running' } }, { operation: { status: 'failed' } },
    { operation: { status: 'interrupted' } }, { operation: { status: 'unknown' } }]) {
    const changed = { ...pausedDiscovery, ...change };
    assert.equal(setupProgress(changed), null, JSON.stringify(change));
    assert.equal(setupGuideResume(changed), null, JSON.stringify(change));
  }
  for (const version of [undefined, true, '4', NaN, Number.MAX_SAFE_INTEGER + 1])
    assert.equal(setupGuideResume({ ...pausedDiscovery, version }), null);
  const contextual = { ...pausedDiscovery, readiness: { context_ready: true, ready: false } };
  assert.deepEqual(setupProgress(contextual).actions, [{ target: 'build', label: 'Open builder' }]);
  assert.equal(setupGuideResume(contextual), null);
  assert.equal(setupGuideResume({ ...pausedDiscovery, operation: undefined }).text, SETUP_DISCOVERY_PROMPT);
});

test('actual lifecycle progress keeps its existing actions and resume prompt ahead of discovery', () => {
  for (const receipts of [{ build: accepted }, { build: { ...accepted, status: 'running' } },
    { trial: openTrial }, { deployment: { guide_id: guide.id, deployment_id: 'owned', status: 'stopped' } }]) {
    assert.equal(setupProgress(pausedDiscovery, receipts).kind, undefined);
    assert.equal(setupGuideResume(pausedDiscovery, receipts).text, GUIDE_RESUME_PROMPT);
  }
  const staleCleanup = { ...accepted, current: false, cleanup_pending: true };
  const cleanup = setupProgress(pausedDiscovery, { build: staleCleanup });
  assert.equal(cleanup.title, 'Your build needs cleanup');
  assert.deepEqual(cleanup.actions, [{ target: 'build', label: 'Review cleanup' }]);
  assert.equal(cleanup.kind, undefined);
  assert.equal(setupGuideResume(pausedDiscovery, { build: staleCleanup }).text, GUIDE_RESUME_PROMPT);
});

test('terminal same-guide trials do not suppress paused discovery or substitute the lifecycle prompt', () => {
  for (const status of ['completed', 'failed', 'interrupted', 'canceled']) {
    for (const binding of [{ guide_version: guide.version, current: true },
      { guide_version: guide.version - 1, current: false }]) {
      const trial = { guide_id: guide.id, rehearsal_id: 'past-trial', mode: 'design', status, ...binding };
      const receipts = { trial }, before = structuredClone({ guide: pausedDiscovery, receipts });
      assert.equal(setupProgressHasLifecycle(pausedDiscovery, receipts), true);
      assert.equal(setupProgress(pausedDiscovery, receipts).kind, 'discovery');
      assert.equal(setupGuideResume(pausedDiscovery, receipts).text, SETUP_DISCOVERY_PROMPT);
      const html = renderToStaticMarkup(React.createElement(SetupProgressCard, {
        progress: setupProgress(pausedDiscovery, receipts), resume: setupGuideResume(pausedDiscovery, receipts), Icon: () => null }));
      assert.match(html, /Continue setup/);
      assert.doesNotMatch(html, /APPLICATION STATUS|Ask the guide what is next/);
      assert.deepEqual({ guide: pausedDiscovery, receipts }, before);
    }
  }
});

test('active earlier trials and retained build cleanup still take priority over discovery after historical trials', () => {
  for (const status of ['queued', 'starting', 'running', 'waiting_input', 'ending', 'canceling']) {
    const trial = { guide_id: guide.id, guide_version: guide.version - 1, current: false,
      rehearsal_id: 'earlier-active', mode: 'design', status };
    const receipts = { trial }, progress = setupProgress(pausedDiscovery, receipts);
    assert.equal(progress.kind, undefined);
    assert.deepEqual(progress.actions, [{ target: 'trial', label: 'Follow trial' }]);
    assert.equal(setupGuideResume(pausedDiscovery, receipts).text, GUIDE_RESUME_PROMPT);
  }
  const pastTrial = { guide_id: guide.id, guide_version: guide.version - 1, current: false,
    rehearsal_id: 'earlier-finished', mode: 'design', status: 'completed' };
  const receipts = { trial: pastTrial, build: { ...accepted, guide_version: guide.version - 1,
    current: false, cleanup_pending: true } };
  const progress = setupProgress(pausedDiscovery, receipts);
  assert.equal(progress.kind, undefined);
  assert.equal(progress.title, 'Your build needs cleanup');
  assert.deepEqual(progress.actions, [{ target: 'build', label: 'Review cleanup' }]);
  assert.equal(setupGuideResume(pausedDiscovery, receipts).text, GUIDE_RESUME_PROMPT);
  const discoveryResume = setupGuideResume(pausedDiscovery, { trial: pastTrial }), sends = [];
  assert.equal(admitSetupGuideResume(discoveryResume, pausedDiscovery, receipts, {},
    { key: inputKey(pausedDiscovery), draft: 'Unsent', selected: [], mode: 'chat' }, (...args) => sends.push(args)), false);
  assert.deepEqual(sends, []);
});

test('foreign historical receipts neither block discovery nor enter its continuation request', () => {
  const receipts = { trial: { guide_id: 'other', rehearsal_id: 'foreign-trial', status: 'completed', mode: 'design' },
    build: { ...accepted, guide_id: 'other', current: false, cleanup_pending: true },
    deployment: { guide_id: 'other', deployment_id: 'foreign-install', status: 'stopped' } };
  assert.equal(setupProgressHasLifecycle(pausedDiscovery, receipts), false);
  assert.deepEqual(setupProgress(pausedDiscovery, receipts), setupProgress(pausedDiscovery));
  assert.deepEqual(setupGuideResume(pausedDiscovery, receipts), setupGuideResume(pausedDiscovery));
  const sends = [];
  assert.equal(admitSetupGuideResume(setupGuideResume(pausedDiscovery, receipts), pausedDiscovery, receipts, {},
    { key: inputKey(pausedDiscovery), draft: 'Unsent', selected: [], mode: 'chat' }, (...args) => sends.push(args)), true);
  assert.deepEqual(sends[0][1], { text: SETUP_DISCOVERY_PROMPT });
  assert.doesNotMatch(JSON.stringify(sends), /foreign-trial|foreign-install|"other"|Unsent/);
});

test('a discovery click rechecks guide, version, input and pending-work ownership before sending', () => {
  const resume = setupGuideResume(pausedDiscovery), input = { key: inputKey(pausedDiscovery),
    draft: 'My unsent request', selected: [], mode: 'chat' }, sends = [];
  for (const flags of [{ pendingChanges: true }, { designBusy: true }, { setupLoading: true },
    { processing: true }, { packageBusy: true }, { active: false }]) {
    assert.equal(setupGuideResume(pausedDiscovery, {}, flags), null);
    assert.equal(admitSetupGuideResume(resume, pausedDiscovery, {}, flags, input, (...args) => sends.push(args)), false);
  }
  for (const current of [{ ...pausedDiscovery, id: 'other' }, { ...pausedDiscovery, version: 5 },
    { ...pausedDiscovery, operation: { status: 'running' } },
    { ...pausedDiscovery, question: { title: 'A newer question', options: [] } },
    { ...pausedDiscovery, readiness: { context_ready: true } }])
    assert.equal(admitSetupGuideResume(resume, current, {}, {}, input, (...args) => sends.push(args)), false);
  assert.equal(admitSetupGuideResume({ ...resume, text: 'Another intent' }, pausedDiscovery, {}, {}, input,
    (...args) => sends.push(args)), false);
  assert.equal(admitSetupGuideResume(resume, pausedDiscovery, {}, {}, { ...input, key: inputKey({ ...pausedDiscovery,
    question: { title: 'Earlier question', options: [] } }) }, (...args) => sends.push(args)), false);
  assert.deepEqual(sends, []);
});

test('the discovery card offers one explicit Continue setup action without application-status claims', () => {
  const progress = setupProgress(pausedDiscovery), resume = setupGuideResume(pausedDiscovery), sends = [], navigations = [];
  const input = { key: inputKey(pausedDiscovery), draft: 'Private unsent thought', selected: [], mode: 'chat' };
  const before = structuredClone(input), props = { progress, resume, disabled: false, Icon: () => null,
    onNavigate: item => navigations.push(item),
    onResume: item => admitSetupGuideResume(item, pausedDiscovery, {}, {}, input, (...args) => sends.push(args)) };
  const html = renderToStaticMarkup(React.createElement(SetupProgressCard, props));
  assert.match(html, /aria-label="Continue your setup"/);
  assert.match(html, /SETUP CONVERSATION/);
  assert.match(html, /Continue setup/);
  assert.doesNotMatch(html, /APPLICATION STATUS|Current application status|installed|running|Ask the guide what is next/);
  assert.deepEqual(sends, []);
  const controls = React.Children.toArray(SetupProgressCard(props).props.children.at(-1).props.children);
  assert.equal(controls.length, 1);
  const button = controls[0];
  assert.equal(button.props.type, 'button');
  assert.equal(button.props.children[0], 'Continue setup ');
  button.props.onClick();
  assert.deepEqual(sends, [['chat', { text: SETUP_DISCOVERY_PROMPT }, { preserveInput: true,
    expectedGuide: { id: guide.id, version: guide.version } }]]);
  assert.doesNotMatch(JSON.stringify(sends[0][1]), /Private unsent|preserveInput|expectedGuide/);
  assert.deepEqual(input, before);
  assert.deepEqual(navigations, []);
  const disabled = SetupProgressCard({ ...props, disabled: true });
  assert.equal(React.Children.toArray(disabled.props.children.at(-1).props.children)[0].props.disabled, true);
});

test('discovery continuation preserves exact unsent drafts and never revives earlier answer choices', () => {
  const values = new Map(), storage = { getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value) };
  const earlier = { ...pausedDiscovery, version: 3,
    question: { title: 'Earlier scope', stage: 'audience', options: [{ id: 'shared' }] } };
  writeInput(storage, { key: inputKey(earlier), draft: '', selected: ['shared'], mode: 'answer' });
  const input = { key: inputKey(pausedDiscovery), draft: SETUP_DISCOVERY_PROMPT, selected: [], mode: 'chat' };
  const resume = setupGuideResume(pausedDiscovery), sends = [];
  assert.equal(admitSetupGuideResume(resume, pausedDiscovery, {}, {}, input, (action, body, options) => {
    sends.push([action, body, options]);
    assert.equal(captureGuideResumeInput(storage, pausedDiscovery, input, body.text), true);
  }), true);
  const operation = { status: 'running', request_id: 'a'.repeat(64), version: guide.version,
    request: { action: 'chat', text: SETUP_DISCOVERY_PROMPT, selected: [], version: guide.version } };
  assert.equal(preservedGuideResume(storage, { ...pausedDiscovery, operation }).request_id, operation.request_id);
  const next = { ...pausedDiscovery, version: 5, operation: { ...operation, status: 'completed' },
    question: { title: 'The next relevant context decision', stage: 'destination', options: [{ id: 'shared' }, { id: 'new' }] } };
  const recovered = recoverGuideResumeInput(storage, next, input);
  assert.equal(recovered.draft, SETUP_DISCOVERY_PROMPT, 'even a same-text unsent draft is preserved by the explicit button');
  assert.equal(recovered.key, inputKey(next));
  assert.deepEqual(recovered.selected, []);
  assert.deepEqual(readInput(storage, earlier).selected, ['shared']);
  assert.equal(readInput(storage, next).draft, SETUP_DISCOVERY_PROMPT);
  const editing = { ...recovered, draft: 'A newer unsent detail\n\u00e5' };
  assert.equal(recoverGuideResumeInput(storage, next, editing).draft, editing.draft);
  assert.equal(sends.length, 1);
  assert.deepEqual(Object.keys(sends[0][1]), ['text']);
});


test('unresolved application work leads to the builder while an agent-only plan keeps its package journey', () => {
  const state = { ...guide, status: 'question', proposal: { project: { name: 'Study team' }, target: 'mac' },
    readiness: { context_ready: true, ready: false, decision_blockers: [], implementation_blockers: ['Build custom screens.'] } };
  const before = structuredClone(state);
  const progress = setupProgress(state);
  assert.equal(progress.title, 'Your application needs to be built');
  assert.deepEqual(progress.actions, [{ target: 'build', label: 'Build application' }]);
  assert.match(progress.detail, /agent team package does not implement/);
  assert.equal(setupNeedsApplicationBuild(state), true);
  assert.equal(setupProgressHasLifecycle(state), false);
  assert.equal(setupGuideResume(state), null);
  const agentOnly = { ...state, readiness: { ...state.readiness, implementation_blockers: [] } };
  assert.equal(setupNeedsApplicationBuild(agentOnly), false);
  assert.equal(setupProgress(agentOnly).title, 'Your agent team package is available');
  assert.deepEqual(setupProgress(agentOnly).actions, [{ target: 'package', label: 'Get your agent team' },
    { target: 'build', label: 'Optional custom application' }]);
  for (const flags of [{ pendingChanges: true }, { designBusy: true }]) assert.equal(setupProgress(state, {}, flags), null);
  const decisions = { ...state, readiness: { ...state.readiness, decision_blockers: ['Choose authority.'] } };
  assert.equal(setupNeedsApplicationBuild(decisions), false);
  const unresolved = setupProgress(decisions);
  assert.deepEqual(unresolved.actions, []);
  assert.match(unresolved.title, /open setup decisions/);
  assert.deepEqual(setupProgress(state, { build: accepted }).actions.map(item => item.target), ['trial', 'deployment']);
  assert.equal(setupNeedsApplicationBuild(state, accepted), false);
  assert.deepEqual(state, before);
});

test('foreign, stale and untrusted acceptance cannot turn required application work into an optional build', () => {
  const state = { ...guide, status: 'refining', question: null, proposal: { project: { name: 'Study team' }, target: 'mac' },
    readiness: { context_ready: true, ready: false, decision_blockers: [], implementation_blockers: ['Build custom screens.'] } };
  for (const change of [{ guide_id: 'other' }, { guide_version: guide.version - 1 },
    { current_guide_version: guide.version - 1 }, { current: false }]) {
    const build = { ...accepted, ...change }, before = structuredClone(build);
    assert.equal(setupNeedsApplicationBuild(state, build), true);
    assert.deepEqual(setupProgress(state, { build }).actions, [{ target: 'build', label: 'Build application' }]);
    assert.deepEqual(build, before);
  }
  for (const change of [{ source_verified: false }, { acceptance_trusted: false }, { acceptance_current: false },
    { ready_to_package: false }, { acceptance: { ...accepted.acceptance, tests_run: 0 } },
    { acceptance: { ...accepted.acceptance, source_digest: 'foreign-source' } }]) {
    const build = { ...accepted, ...change };
    assert.equal(setupNeedsApplicationBuild(state, build), true);
    assert.deepEqual(setupProgress(state, { build }).actions, [{ target: 'build', label: 'Review and repair' }]);
  }
  for (const readiness of [undefined, { context_ready: false, implementation_blockers: ['Build custom screens.'] },
    { context_ready: true, implementation_blockers: [] }, { context_ready: true, implementation_blockers: 'unfinished' }]) {
    assert.equal(setupNeedsApplicationBuild({ ...state, readiness }), false);
  }
});

test('an accepted ready team offers direct private installation while proposals and actual installed lifecycle stay distinct', () => {
  const ready = { ...guide, status: 'ready', proposal: null, question: null,
    blueprint: { project: { name: 'Study team' }, target: 'mac' }, readiness: { ready: true, context_ready: true } };
  assert.deepEqual(setupProgress(ready).actions, [{ target: 'deployment', label: 'Install agent team', sourceKind: 'native-team' },
    { target: 'package', label: 'Download package' }, { target: 'build', label: 'Optional custom application' }]);
  assert.match(setupProgress(ready).detail, /No custom application build/);
  const native = { guide_id: ready.id, guide_version: ready.version, deployment_id: 'native-team-install', source_kind: 'native-team',
    build_id: null, status: 'stopped' };
  const progress = setupProgress(ready, { deployment: native });
  assert.equal(progress.title, 'Your installed agent team is stopped');
  assert.equal(progress.actions[0].label, 'Manage agent team');
  assert.equal(progress.actions.some(item => item.target === 'trial'), false);
  assert.equal(setupProgress(ready, { deployment: { ...native, status: 'starting' } }).actions[0].label, 'View installation');
  const running = { ...native, status: 'running', process_live: true, health: 'ready',
    console_url: 'http://127.0.0.1:18789/console', bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  const live = setupProgress({ ...ready, version: 5 }, { deployment: running });
  assert.equal(live.title, 'Your installed agent team is running');
  assert.match(live.detail, /Installed proposal 4; current proposal 5/);
  assert.deepEqual(live.actions, [{ target: 'deployment', label: 'Use your agent team' },
    { target: 'console', label: 'Open agent team Console', url: running.console_url }]);
  assert.deepEqual(setupProgress(ready, { build: { ...accepted, guide_id: ready.id, guide_version: ready.version } }).actions.map(item => item.target), ['trial', 'deployment']);
});
