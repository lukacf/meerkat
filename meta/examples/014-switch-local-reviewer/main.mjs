import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Meta } from '@rkat/meta-sdk';

const gateway = process.env.META_GATEWAY_BIN;
if (!gateway) throw Error('Set META_GATEWAY_BIN to the matching released rpc_gateway');
const names = ['editor', 'day-review', 'night-review'];
const policy = {
  enabled: true,
  approval: { rules: [{ change: ['create', 'peers'], requester: {kind:'host', surface:'rpc'}, outcome: {kind:'allow'} }] },
  peers: { default:'deny', rules:[{id:'review-team', target:{profile:['worker']}, outcome:'allow'}] },
};
const root = await mkdtemp(join(tmpdir(), 'switch-reviewer-'));
try {
  process.env.XDG_STATE_HOME = join(root, 'xdg-state');
  const runtime = await Meta.builder().gateway(gateway)
    .mob(fileURLToPath(new URL('./mob.toml', import.meta.url)))
    .persistentState(join(root, 'meta-state')).rosterProvider({async roster(){ return []; }})
    .identityCreation(policy).build();
  try {
    const ids = runtime.identities;
    const created = await ids.create(names.map(name => ({name, profile:'worker', peers:name === 'editor' ? ['day-review'] : []})), {requestId:'create-team'});
    assert.equal(created.outcome, 'done');
    const record = await ids.get('editor');
    assert.deepEqual(record.peers, ['day-review']);
    const mob = runtime.mobHandle();
    async function edges() {
      const rows = await Promise.all(names.map(n => mob.getMember(n)));
      return Object.fromEntries(names.map((n,i) => [n, [...rows[i].wiredTo].sort()]));
    }
    const before = await edges();
    assert.deepEqual(before, {editor:['day-review'], 'day-review':['editor'], 'night-review':[]});
    const change = await ids.update('editor', {expectedSpecDigest:record.specDigest,
      peersRemove:['day-review'], peersAdd:['night-review'], requestId:'switch-reviewer'});
    assert.equal(change.outcome, 'done');
    assert.deepEqual((await ids.get('editor')).peers, ['night-review']);
    const after = await edges();
    assert.deepEqual(after, {editor:['night-review'], 'day-review':[], 'night-review':['editor']});
    console.log(JSON.stringify({before, after}, null, 2));
  } finally { await runtime.shutdown(); }
} finally { await rm(root, {recursive:true, force:true}); }
