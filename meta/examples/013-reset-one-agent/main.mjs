// Only a new, private gateway state directory is used. Never reset a real agent here.
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Meta } from '@rkat/meta-sdk';

const gateway = process.env.META_GATEWAY_BIN;
if (!gateway) throw Error('Set META_GATEWAY_BIN to the matching local rpc_gateway');
const state = await mkdtemp(join(tmpdir(), 'reset-one-'));
const names = ['draft', 'index'];
try {
  const runtime = await Meta.builder().gateway(gateway)
    .mob(fileURLToPath(new URL('./mob.toml', import.meta.url))).persistentState(state)
    .rosterProvider({async roster() { return names.map(identity => ({identity, profile:'worker', displayName:identity, addressability:'addressable', labels:{}, context:null, additionalInstructions:[]})); }})
    .build();
  try {
    async function snapshot(name) {
      const { sessionId, generation, lifecycleState } = await runtime.status(name);
      assert.ok(sessionId && Number.isInteger(generation));
      assert.equal(lifecycleState, 'active');
      return { sessionId, generation, lifecycleState };
    }
    const before = await Promise.all(names.map(snapshot));
    assert.notEqual(before[0].sessionId, before[1].sessionId);
    // Retire first rather than using a status read as an idle check. No other
    // sender exists in this private demo. Do not retry an ambiguous reset.
    await runtime.retire('draft');
    await runtime.reset('draft');
    const after = await Promise.all(names.map(snapshot));
    assert.notEqual(after[0].sessionId, before[0].sessionId);
    assert.ok(after[0].generation > before[0].generation);
    assert.deepEqual(after[1], before[1]);
    console.log(JSON.stringify({names,before,after}, null, 2));
  } finally { await runtime.shutdown(); }
} finally { await rm(state, {recursive:true,force:true}); }
