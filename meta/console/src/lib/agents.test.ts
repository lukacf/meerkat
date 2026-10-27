import assert from "node:assert/strict";
import test from "node:test";

import { canonicalConsoleIdentity, normalizeAgents } from "./agents";

test("normalizeAgents preserves identity-status summary fields without invention", () => {
  const agents = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: {
          agents: [
            {
              identity: " identity:luka ",
              display_name: " Luka ",
              role: " operator ",
              state: " running ",
              addressability: "addressable",
              generation: 4,
            // A 0.9.3 gateway still sends these; 0.9.4 has no such fields.
            checkpoint_version: 8,
            lease_healthy: true,
            progress: {
              run_state: "run_open",
              in_flight_work: 1,
              last_progress_at_ms: 1752300000000,
              last_progress_event: "execution_advanced",
              health: "wedged",
            },
            labels: { team: " console " },
            model_capabilities: { image_input: true },
          },
        ],
      },
      },
    },
    [],
  );

  assert.equal(agents.length, 1);
  const agent = agents[0];
  assert.equal(agent?.identity, "identity:luka");
  assert.equal(agent?.member_id, "identity:luka");
  assert.equal(agent?.label, "Luka");
  assert.equal(agent?.role, "operator");
  assert.equal(agent?.state, "running");
  assert.equal(agent?.addressability, "addressable");
  assert.equal(agent?.generation, 4);
  assert.ok(agent && !("checkpoint_version" in agent) && !("lease_healthy" in agent));
  assert.equal(agent?.progress?.health, "wedged");
  assert.equal(agent?.progress?.run_state, "run_open");
  assert.deepEqual(agent?.labels, { team: "console" });
});

test("normalizeAgents falls back to identity_status rows when sidebar snapshot is absent", () => {
  const agents = normalizeAgents(
    {
      identity_status: {
        schema_version: "1",
        refresh: { mode: "poll", interval_ms: 5000 },
        rows: [
          {
            identity: " identity:luka ",
            display_name: " Luka ",
            role: " operator ",
            state: " running ",
            addressability: "addressable",
            labels: { team: " console " },
            generation: 4,
            // 0.9.4 first sends the removed fields as null.
            checkpoint_version: null,
            lease_healthy: null,
            model_capabilities: { image_input: true },
          },
        ],
      },
    },
    [],
  );

  assert.equal(agents.length, 1);
  const agent = agents[0];
  assert.equal(agent?.identity, "identity:luka");
  assert.equal(agent?.member_id, "identity-only:identity:luka");
  assert.equal(agent?.label, "Luka");
  assert.equal(agent?.role, "operator");
  assert.equal(agent?.state, "running");
  assert.equal(agent?.addressability, "addressable");
  assert.equal(agent?.addressable, false);
  assert.equal(agent?.generation, 4);
  assert.ok(agent && !("checkpoint_version" in agent) && !("lease_healthy" in agent));
  assert.deepEqual(agent?.labels, { team: "console" });
  assert.deepEqual(agent?.model_capabilities, { image_input: true });
  assert.deepEqual(agent?.affordances, { can_send_message: false });
});

test("normalizeAgents enriches sidebar snapshot rows with identity_status when both surfaces exist", () => {
  const agents = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: {
          agents: [
            {
              agent_id: "domain:billing",
              member_id: "domain:billing",
              label: "Billing",
              kind: "module_agent",
              state: "running",
              addressable: true,
            },
          ],
        },
      },
      identity_status: {
        schema_version: "1",
        refresh: { mode: "poll", interval_ms: 5000 },
        rows: [
          {
            identity: "domain:billing",
            display_name: "Billing",
            role: "operator",
            state: "running",
            addressability: "addressable",
            labels: { team: "finance" },
            generation: 2,
            model_capabilities: { image_input: true },
          },
        ],
      },
    },
    [],
  );

  assert.equal(agents.length, 1);
  const agent = agents[0];
  assert.equal(agent?.identity, "domain:billing");
  assert.equal(agent?.member_id, "domain:billing");
  assert.equal(agent?.addressability, "addressable");
  assert.equal(agent?.addressable, true);
  assert.equal(agent?.generation, 2);
  assert.ok(agent && !("checkpoint_version" in agent) && !("lease_healthy" in agent));
  assert.deepEqual(agent?.labels, { team: "finance" });
  assert.deepEqual(agent?.model_capabilities, { image_input: true });
});

test("normalizeAgents maps runtime member sidebar rows back to durable agent identity", () => {
  const agents = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: {
          agents: [
            {
              identity: "rt:channel:C0SMOKEOPS:0",
              member_id: "rt:channel:C0SMOKEOPS:0",
              label: "C0SMOKEOPS",
              kind: "channel",
              state: "active",
              addressable: true,
              labels: {
                agent_identity: "channel:C0SMOKEOPS",
                agent_type: "channel",
              },
            },
          ],
        },
      },
      identity_status: {
        schema_version: "1",
        rows: [
          {
            identity: "channel:C0SMOKEOPS",
            display_name: "C0SMOKEOPS",
            role: "domain",
            state: "active",
            addressability: "addressable",
            labels: {
              agent_identity: "channel:C0SMOKEOPS",
              agent_type: "channel",
            },
          },
        ],
      },
    },
    [],
  );

  assert.equal(agents.length, 1);
  const agent = agents[0];
  assert.equal(agent?.identity, "channel:C0SMOKEOPS");
  assert.equal(agent?.member_id, "rt:channel:C0SMOKEOPS:0");
  assert.equal(agent?.agent_id, "channel:C0SMOKEOPS");
});

test("canonicalConsoleIdentity maps runtime/member aliases back to durable chat identity", () => {
  const agents = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: {
          agents: [
            {
              identity: "rt:deep-investigator:0",
              agent_id: "deep-investigator",
              member_id: "rt:deep-investigator:0",
              label: "Deep Investigator",
              role: "deep-investigator",
              state: "active",
              addressable: true,
              labels: {
                agent_identity: "deep-investigator:singleton",
              },
            },
          ],
        },
      },
      identity_status: {
        schema_version: "1",
        rows: [
          {
            identity: "deep-investigator:singleton",
            display_name: "Deep Investigator",
            role: "deep-investigator",
            state: "active",
            addressability: "addressable",
          },
        ],
      },
    },
    [],
  );

  assert.equal(
    canonicalConsoleIdentity("deep-investigator", agents),
    "deep-investigator:singleton",
  );
  assert.equal(
    canonicalConsoleIdentity("rt:deep-investigator:0", agents),
    "deep-investigator:singleton",
  );
  assert.equal(
    canonicalConsoleIdentity("deep-investigator:singleton", agents),
    "deep-investigator:singleton",
  );
  assert.equal(canonicalConsoleIdentity("unknown-agent", agents), "unknown-agent");
});

test("normalizeAgents appends live identities missing from the sidebar snapshot", () => {
  const agents = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: {
          agents: [
            {
              identity: "full-tools-worker-1",
              member_id: "full-tools-worker-1",
              label: "full-tools-worker-1",
              role: "full-tools-worker",
              state: "active",
              addressable: true,
            },
          ],
        },
      },
      identity_status: {
        schema_version: "1",
        rows: [
          {
            identity: "full-tools-worker-1",
            role: "full-tools-worker",
            state: "active",
            addressability: "addressable",
            labels: {},
          },
          {
            identity: "sub-worker-1",
            role: "identity",
            state: "active",
            addressability: "addressable",
            labels: {},
          },
        ],
      },
    },
    [],
  );

  assert.equal(agents.length, 2);
  const subWorker = agents.find((agent) => agent.identity === "sub-worker-1");
  assert.equal(subWorker?.member_id, "sub-worker-1");
  assert.equal(subWorker?.addressable, true);
  assert.deepEqual(subWorker?.affordances, { can_send_message: true });
});

test("normalizeAgents distinguishes explicit quiet phase from missing or invalid activity", () => {
  const phases = [null, undefined, "waiting", "tool-executing", "generating", "thinking"];
  const agents = normalizeAgents({ agent_sidebar: { live_snapshot: { agents: phases.map((phase, index) => ({
    identity: `agent-${index}`, ...(phase === undefined ? {} : { response_phase: phase }),
  })) } } } as never, []);
  assert.equal(agents[0].response_phase, null);
  assert.equal(Object.hasOwn(agents[1], "response_phase"), false);
  assert.equal(agents[2].response_phase, "waiting");
  assert.equal(agents[3].response_phase, "tool-executing");
  assert.equal(agents[4].response_phase, "generating");
  assert.equal(Object.hasOwn(agents[5], "response_phase"), false);
});

test("normalizeAgents carries the unobserved marker of a pending sidebar row", () => {
  const unobserved = { reason: "refresh_pending", fields: ["state", "role", "labels"] };
  const agents = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: {
          agents: [
            {
              agent_id: "voice-keeper",
              member_id: "voice-keeper",
              identity: "voice-keeper",
              label: "voice-keeper",
              kind: "mob_agent",
              state: "unknown",
              session_id: "successor-session",
              labels: { agent_identity: "voice-keeper" },
              addressable: false,
              affordances: { addressable: false, can_send_message: false, can_retire: false, can_respawn: true },
              unobserved,
            },
            {
              agent_id: "other-keeper",
              member_id: "other-keeper",
              identity: "other-keeper",
              label: "other-keeper",
              kind: "mob_agent",
              state: "active",
            },
          ],
        },
      },
      identity_status: {
        schema_version: "1",
        refresh: { mode: "poll", interval_ms: 5000 },
        rows: [
          {
            identity: "voice-keeper",
            state: "unknown",
            addressability: "internal_only",
            labels: { agent_identity: "voice-keeper" },
            unobserved,
          },
        ],
      },
    },
    [],
  );

  assert.equal(agents.length, 2);
  assert.deepEqual(agents[0]?.unobserved, unobserved);
  assert.equal(agents[0]?.session_id, "successor-session");
  assert.equal(agents[0]?.state, "unknown");
  assert.equal(agents[0]?.affordances?.can_send_message, false);
  assert.equal(agents[1]?.unobserved, undefined);
});
