import assert from 'node:assert/strict';
import test from 'node:test';
import { Session } from '../dist/session.js';

test('direct turn transports canonical structured skill intent', async () => {
  const requests = [];
  const session = new Session(
    7,
    async (handle, prompt, options) => {
      requests.push({ handle, prompt: JSON.parse(prompt), options: JSON.parse(options) });
      return JSON.stringify({ text: 'workflow applied' });
    },
    async () => '{}',
    async () => {},
    () => '[]',
    async () => '{}',
    async () => {},
    async () => {},
  );
  const skill = {
    source_uuid: '00000000-0000-4b11-8111-000000000001',
    skill_name: 'task-workflow',
  };
  const result = await session.turn('Plan this task.', {
    skillReferences: [skill],
    transientTurnContext: 'Current host facts',
  });
  assert.equal(result.text, 'workflow applied');
  assert.deepEqual(requests, [{
    handle: 7,
    prompt: { text: 'Plan this task.' },
    options: {
      skill_references: [skill],
      transient_turn_context: 'Current host facts',
    },
  }]);
});
