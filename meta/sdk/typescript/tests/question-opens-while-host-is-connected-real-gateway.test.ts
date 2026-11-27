/**
 * A question an agent asks while the SDK host is connected reaches the host.
 *
 * One `rpc_gateway` process over a persistent state directory, for a launch
 * with a roster of one long-term agent, built through the self-hosted model
 * route with no credential, so the row needs no provider key and no seeded
 * state: the model is a local chat-completions fake that calls the stock
 * tool `ask_user` once, when the agent is sent the row's trigger, and
 * answers every other request with text.
 *
 * The roster matters: a gateway serves the questions of its agents only for
 * a launch with one. On a launch without a roster the model's call is
 * answered `no_human_surface` at once, nothing is held and nothing is
 * noticed.
 *
 * The row follows one question from the agent to the person's side and back:
 *
 * 1. The host registers its handler (`MetaRuntime.onDecisionNotice`) and
 *    makes no list call. Nothing is noticed while nothing is asked.
 * 2. The agent is sent the trigger (`MetaRuntime.send`) and its model calls
 *    `ask_user`. The
 *    handler receives one `held` notice of kind `ask_user.questions` for a
 *    decision that opened while the host was connected.
 * 3. With the id the notice names the host reads the set
 *    (`MobHandle.gatingQuestions`; a set's `pendingId` is the decision's
 *    id): it holds what the model asked, and its origin names the asking
 *    agent.
 * 4. The host answers (`MobHandle.gatingAnswer`). The set leaves the open
 *    sets, and the agent's model is handed the typed result on a later
 *    request: an `ask_user_result` input that says `answered` and carries
 *    the chosen choice.
 *
 * The gateway keeps the SDK's default Console policy: it requires sign-in
 * and trusts no signing key, so no person can answer in its Console and the
 * embedding host is the one that can. That is the case the notice exists
 * for.
 *
 * The row runs when META_RPC_GATEWAY_BIN names a built rpc_gateway.
 *
 * What this row does not show
 * ---------------------------
 *
 * It shows nothing of a restart of a gateway, of a second host connection,
 * or of what a person is shown. The host answers as the embedding host (the
 * stdio decide trusts its host and records the `approverId` it names). The
 * row records the calls of the SDK's notice handler only, so a failed wait
 * says that a notice was not observed by the handler in time, not that the
 * gateway wrote none. The rows of a question set that is open BEFORE the
 * gateway starts are in `held-question-at-opening-real-gateway.test.ts`.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Meta } from "../src/index.js";
import type { MetaRuntime } from "../src/index.js";

const gatewayBin = process.env.META_RPC_GATEWAY_BIN;
const skip = !gatewayBin && "META_RPC_GATEWAY_BIN is not set";

const MODEL = "question-row-fake";
const QUESTION_KIND = "ask_user.questions";
const AGENT = "agent-a";
// What the row sends the agent to make its model ask.
const ASK_NOW = "ASK-THE-PERSON-NOW-KILO-5";
// What the model asks: the arguments of its one `ask_user` call.
const ASKED = {
  title: "Release plan",
  questions: [
    {
      id: "day",
      prompt: "Which day?",
      choices: [
        { id: "mon", label: "Monday" },
        { id: "fri", label: "Friday" },
      ],
    },
  ],
};
// The choice the host answers with.
const CHOSEN = "fri";
// The approver the host names. The stdio decide trusts its host and records
// the name it gives; nobody is authenticated by it.
const ANSWERS_AS = "sdk-question-row";

// How long the row waits for an observation it expects.
const WAIT_MS = 30_000;
// How long the row watches before it says that no notice was observed.
const QUIET_MS = 500;

const MOB_TOML = `[mob]
id = "question-opens-while-connected-typescript"

[profiles.worker]
model = "${MODEL}"
provider = "self_hosted"
self_hosted_server_id = "local"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
`;

function configToml(baseUrl: string): string {
  return `[self_hosted]
default_model = "${MODEL}"

[self_hosted.servers.local]
transport = "openai_compatible"
base_url = "${baseUrl}"
api_style = "chat_completions"

[self_hosted.models.${MODEL}]
server = "local"
remote_model = "${MODEL}"

[realm.global]
default_binding = "local"

[realm.global.backend.local]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "local"

[realm.global.auth.local_auth]
provider = "self_hosted"
auth_method = "none"
source = { kind = "platform_default" }

[realm.global.binding.local]
backend_profile = "local"
auth_profile = "local_auth"
`;
}

/**
 * A chat-completions endpoint that calls `ask_user` once, at the first
 * request that carries the trigger, and answers every other request with
 * text. It keeps the messages of every request as text.
 */
class AskingModel {
  readonly requests: string[] = [];
  asked = false;
  private readonly server: Server;

  private constructor(server: Server) {
    this.server = server;
  }

  static async start(): Promise<AskingModel> {
    const server = createServer();
    const model = new AskingModel(server);
    server.on("request", (request, response) => {
      const parts: Buffer[] = [];
      request.on("data", (part: Buffer) => parts.push(part));
      request.on("end", () => {
        const body = JSON.parse(Buffer.concat(parts).toString("utf-8") || "{}") as {
          messages?: unknown;
          stream?: boolean;
        };
        const chunks = model.answer(body.messages ?? []);
        if (body.stream) {
          response.writeHead(200, { "Content-Type": "text/event-stream" });
          for (const chunk of chunks) response.write(`data: ${JSON.stringify(chunk)}\n\n`);
          response.end("data: [DONE]\n\n");
          return;
        }
        const first = chunks[0].choices as { delta: unknown }[];
        const second = chunks[1].choices as { finish_reason: unknown }[];
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(
          JSON.stringify({
            id: "fake",
            object: "chat.completion",
            created: 1,
            model: MODEL,
            choices: [
              { index: 0, message: first[0].delta, finish_reason: second[0].finish_reason },
            ],
            usage: chunks[2].usage,
          }),
        );
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    return model;
  }

  get baseUrl(): string {
    return `http://127.0.0.1:${(this.server.address() as AddressInfo).port}`;
  }

  private answer(messages: unknown): Record<string, unknown>[] {
    const text = JSON.stringify(messages);
    this.requests.push(text);
    let delta: Record<string, unknown>;
    let finish: string;
    if (text.includes(ASK_NOW) && !this.asked) {
      this.asked = true;
      delta = {
        role: "assistant",
        tool_calls: [
          {
            index: 0,
            id: "call-ask-1",
            type: "function",
            function: { name: "ask_user", arguments: JSON.stringify(ASKED) },
          },
        ],
      };
      finish = "tool_calls";
    } else {
      delta = { role: "assistant", content: "observed" };
      finish = "stop";
    }
    const base = { id: "fake", object: "chat.completion.chunk", created: 1, model: MODEL };
    return [
      { ...base, choices: [{ index: 0, delta, finish_reason: null }] },
      { ...base, choices: [{ index: 0, delta: {}, finish_reason: finish }] },
      { ...base, choices: [], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } },
    ];
  }

  /** Whether the model was sent a request in which every pattern is found. */
  requestMatching(...patterns: RegExp[]): boolean {
    return this.requests.some((text) => patterns.every((pattern) => pattern.test(text)));
  }

  async close(): Promise<void> {
    await new Promise<void>((resolve) => {
      this.server.close(() => resolve());
      this.server.closeAllConnections();
    });
  }
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** Wait until `condition()` holds, or fail naming what was awaited. */
async function until(condition: () => boolean | Promise<boolean>, what: string): Promise<void> {
  const deadline = Date.now() + WAIT_MS;
  while (!(await condition())) {
    assert.ok(Date.now() < deadline, `${what}: not observed within ${WAIT_MS / 1000} s`);
    await sleep(20);
  }
}

describe("a question an agent asks while the host is connected", { skip }, () => {
  it("is noticed, read and answered by the host, and the agent receives the answer", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-question-opens-"));
    const fake = await AskingModel.start();
    let runtime: MetaRuntime | undefined;
    const notices: Record<string, unknown>[] = [];
    try {
      const mobToml = join(dir, "mob.toml");
      const config = join(dir, "config.toml");
      writeFileSync(mobToml, MOB_TOML);
      writeFileSync(config, configToml(fake.baseUrl));
      runtime = await Meta.builder()
        .gateway(gatewayBin as string)
        .mob(mobToml)
        .meerkatConfig(config)
        .persistentState(join(dir, "state"))
        .rosterProvider({
          // The launch's roster: one long-term agent, of the profile worker.
          roster: async () => [
            {
              identity: AGENT,
              profile: "worker",
              addressability: "addressable",
              displayName: null,
              labels: {},
              context: null,
              additionalInstructions: [],
            },
          ],
        })
        .build();
      runtime.onDecisionNotice((notice) => {
        notices.push(notice);
      });
      const handle = runtime.mobHandle();

      // Nothing was asked, so nothing is noticed.
      await sleep(QUIET_MS);
      assert.equal(
        notices.length,
        0,
        `a notice was observed before anything was asked: ${JSON.stringify(notices)}`,
      );

      // The agent is sent the trigger; its model calls ask_user. The host
      // makes no list call: the gateway tells it of the decision that opened
      // while it was connected.
      await runtime.send(AGENT, `${ASK_NOW}: ask which day to release.`);
      await until(() => fake.asked, "the model's ask_user call");
      await until(() => notices.length >= 1, "the notice of the question the agent asked");
      await sleep(QUIET_MS);
      assert.equal(
        notices.length,
        1,
        "more than one notice was observed by the handler with no list call, " +
          `watched for ${QUIET_MS / 1000} s after the first: ${JSON.stringify(notices)}`,
      );
      const noticed = notices[0];
      assert.equal(noticed.event, "held", JSON.stringify(noticed));
      assert.equal(noticed.kind, QUESTION_KIND, JSON.stringify(noticed));
      assert.equal(noticed.phase, "open", JSON.stringify(noticed));
      assert.equal(noticed.presentation ?? null, null, JSON.stringify(noticed));

      // With that id the host reads what was asked, and who asked.
      const openSets = await handle.gatingQuestions();
      const named = openSets.filter((one) => one.pendingId === noticed.decision_id);
      assert.equal(
        named.length,
        1,
        `gatingQuestions returned ${named.length} sets with the noticed id ` +
          `${String(noticed.decision_id)}: ${JSON.stringify(openSets)}`,
      );
      const asked = named[0];
      assert.equal(asked.subjectDigest, noticed.rendering_ref, JSON.stringify([asked, noticed]));
      assert.equal(asked.origin?.identity, AGENT, JSON.stringify(asked));
      assert.equal(asked.title, ASKED.title, JSON.stringify(asked));
      assert.deepEqual(
        asked.questions.map((question) => question.id),
        ["day"],
      );
      assert.deepEqual(
        asked.questions[0].choices.map((choice) => choice.id),
        ["mon", "fri"],
      );

      // The host answers. The set leaves the open sets.
      const result = await handle.gatingAnswer(asked.pendingId, {
        approverId: ANSWERS_AS,
        presentedDigest: asked.subjectDigest,
        answers: [{ questionId: "day", answer: { kind: "choice", choiceId: CHOSEN } }],
      });
      assert.equal(result.pendingId, noticed.decision_id, JSON.stringify(result));
      assert.equal(result.kind, "question_set", JSON.stringify(result));
      assert.equal(result.outcome, "answered", JSON.stringify(result));
      const stillOpen = await handle.gatingQuestions();
      assert.deepEqual(
        stillOpen.map((one) => one.pendingId),
        [],
      );

      // The asking agent's model is handed the typed result: an
      // ask_user_result input that says answered and carries the choice.
      // The request is matched as JSON text inside JSON text, so a quote
      // may stand behind backslashes.
      await until(
        () =>
          fake.requestMatching(
            /ask_user_result/,
            /outcome\\*"\s*:\s*\\*"answered\b/,
            new RegExp(`choice_id\\\\*"\\s*:\\s*\\\\*"${CHOSEN}\\b`),
          ),
        "a model request that carries the answered result",
      );

      // The answer wrote no further notice.
      await sleep(QUIET_MS);
      assert.equal(
        notices.length,
        1,
        "a further notice was observed by the handler after the answer, " +
          `watched for ${QUIET_MS / 1000} s: ${JSON.stringify(notices)}`,
      );
    } finally {
      try {
        if (runtime) await runtime.shutdown();
      } finally {
        await fake.close();
        rmSync(dir, { recursive: true, force: true });
      }
    }
  });
});
