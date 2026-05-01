import assert from 'node:assert/strict';
import test from 'node:test';

import { Mob } from '../dist/mob.js';

test('Mob.spawn strips legacy generation from wasm payloads', async () => {
  let captured;
  const mob = new Mob('mob-web-unit', {
    async mob_spawn(mobId, specsJson) {
      captured = { mobId, specs: JSON.parse(specsJson) };
      return JSON.stringify([
        {
          mob_id: mobId,
          agent_identity: 'worker-1',
          member_ref: 'ref-worker-1',
        },
      ]);
    },
  });

  const result = await mob.spawn([
    {
      profile: 'worker',
      agent_identity: 'worker-1',
      generation: 7,
      turnMetadata: {
        additionalInstructions: [{ kind: 'user', body: 'use nested metadata' }],
        flowToolOverlay: {
          allowedTools: ['read'],
          blockedTools: ['write'],
        },
      },
    },
  ]);

  assert.equal(result[0].agent_identity, 'worker-1');
  assert.deepEqual(captured, {
    mobId: 'mob-web-unit',
    specs: [
        {
          profile: 'worker',
          agent_identity: 'worker-1',
          turn_metadata: {
            additional_instructions: [{ kind: 'user', body: 'use nested metadata' }],
            flow_tool_overlay: {
              allowed_tools: ['read'],
              blocked_tools: ['write'],
            },
          },
        },
      ],
    });
});
