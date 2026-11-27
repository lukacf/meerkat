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
 *    `ask_user`. The handler receives one `held` notice of kind
 *    `ask_user.questions` for a decision that opened while the host was
 *    connected. The row counts the requests the host had written to the
 *    gateway before that notice arrived: none under `meta/decisions/` or
 *    `meta/gating/`.
 * 3. With the id the notice names the host reads the set
 *    (`MobHandle.gatingQuestions`; a set's `pendingId` is the decision's
 *    id): it holds what the model asked, and its origin names the asking
 *    agent.
 * 4. The host answers (`MobHandle.gatingAnswer`). The set leaves the open
 *    sets, and the agent's model is sent the end of its request on a later
 *    model request: an `ask_user_result` input, a label on a line of its own
 *    and then the result as one JSON document. The row parses that
 *    document: it carries the request id the call's own result gave, the
 *    outcome `answered` and the chosen choice.
 *
 * The gateway keeps the SDK's default Console policy: it requires sign-in
 * and trusts no signing key, so no person can answer in its Console and the
 * embedding host is the one that can. That is the case the notice exists
 * for.
 *
 * The row runs when META_RPC_GATEWAY_BIN names a built rpc_gateway, and is
 * skipped otherwise.
 *
 * How the row knows what the host asked
 * -------------------------------------
 *
 * Every line the SDK writes to the gateway's stdin and every line it reads
 * from the gateway's stdout passes one method each of the SDK's transport.
 * The row wraps both before the gateway starts and keeps the direction and
 * the method of each line, so "no list call before the notice" is a count
 * over everything this host wrote, the SDK's own requests included.
 *
 * The gateway process gets an environment built from an allow list, not this
 * process's environment with names removed: `HOME` and the four XDG homes
 * are directories of this test, so the key pair a gateway writes for an
 * agent it runs lands under the test's own directory, and no provider key
 * reaches the gateway whatever its name.
 *
 * What this row does not show
 * ---------------------------
 *
 * It shows nothing of a restart of a gateway, of a second host connection,
 * or of what a person is shown. The host answers as the embedding host (the
 * stdio decide trusts its host and records the `approverId` it names). The
 * model is the row's own fake, so the row shows what the agent's model is
 * sent, not what a model does with it. The rows of a question set that is
 * open BEFORE the gateway starts are in
 * `held-question-at-opening-real-gateway.test.ts`.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Meta } from "../src/index.js";
import type { MetaRuntime } from "../src/index.js";
import { PersistentTransport } from "../src/transport.js";

// The gateway the row starts: the one this variable names, and no other.
const gatewayBin = (process.env.META_RPC_GATEWAY_BIN ?? "").trim();
const skip =
  !(gatewayBin && existsSync(gatewayBin)) &&
  "META_RPC_GATEWAY_BIN does not name a built rpc_gateway";

const MODEL = "question-row-fake";
const QUESTION_KIND = "ask_user.questions";
const AGENT = "agent-a";
// What the row sends the agent to make its model ask.
const ASK_NOW = "ASK-THE-PERSON-NOW-KILO-5";
// The id the model gives its one `ask_user` call.
const CALL_ID = "call-ask-1";
// What the model asks: the arguments of that call.
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
// The label of the input that carries the end of a request, on a line of its
// own before the result.
const RESULT_LABEL = "ask_user_result";
// The requests of a host that read or decide decisions and question sets.
const DECISION_METHODS = ["meta/decisions/", "meta/gating/"];
const NOTICE_METHOD = "meta/decisions/notice";

// How long the row waits for an observation it expects.
const WAIT_MS = 30_000;
// How long the row watches before it says that no notice was observed.
const QUIET_MS = 500;

// The whole environment of the gateway process is an allow list: the names
// below, passed on from this process when set, and the homes the row sets.
const PASSED_ON = [
  // Programs the gateway starts by name are found as on the host.
  "PATH",
  // The locale: the text encoding of the gateway's own output.
  "LANG",
  "LC_ALL",
  "LC_CTYPE",
  // Where the gateway's temporary files go.
  "TMPDIR",
  // The SDK transport's own switches for where the gateway's stderr goes.
  "META_GATEWAY_STDERR",
  "META_GATEWAY_STDERR_FILE",
];
// Set by the row to directories of its own, so the gateway writes nothing to
// the user's home or XDG homes.
const OWN_HOMES = ["HOME", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"];

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

/** The transport's two line methods, which the class keeps private. */
type LineMethods = {
  _writeLine: (this: unknown, obj: Record<string, unknown>, ...rest: unknown[]) => void;
  _handleLine: (this: unknown, line: string, ...rest: unknown[]) => void;
};

/**
 * Every line between this host and its gateway, in order: which way it went
 * and its method (`null` for an answer to a request).
 */
class Wire {
  private readonly lines: { way: "sent" | "received"; method: string | null }[] = [];
  private unwrap: (() => void) | undefined;

  /** Record every line of every gateway started after this call. */
  wrapTheTransport(): void {
    const methods = PersistentTransport.prototype as unknown as LineMethods;
    const writeLine = methods._writeLine;
    const handleLine = methods._handleLine;
    assert.equal(typeof writeLine, "function", "the transport has no _writeLine to wrap");
    assert.equal(typeof handleLine, "function", "the transport has no _handleLine to wrap");
    const add = (way: "sent" | "received", message: unknown): void => {
      const method =
        message !== null && typeof message === "object"
          ? (message as Record<string, unknown>).method
          : undefined;
      this.lines.push({ way, method: typeof method === "string" ? method : null });
    };
    methods._writeLine = function (obj, ...rest) {
      add("sent", obj);
      return writeLine.call(this, obj, ...rest);
    };
    methods._handleLine = function (line, ...rest) {
      let message: unknown = null;
      try {
        message = JSON.parse(line);
      } catch {
        // A line that is no JSON is recorded with no method.
      }
      add("received", message);
      return handleLine.call(this, line, ...rest);
    };
    this.unwrap = () => {
      methods._writeLine = writeLine;
      methods._handleLine = handleLine;
    };
  }

  /** Put the transport's two methods back. */
  restoreTheTransport(): void {
    this.unwrap?.();
    this.unwrap = undefined;
  }

  /** The method of every request the host wrote, in order. */
  requestsSent(): string[] {
    return this.lines.flatMap((line) => (line.way === "sent" && line.method ? [line.method] : []));
  }

  /**
   * The method of every request the host wrote before the first line of
   * `method` it received; `null` when it received none.
   */
  requestsSentBeforeTheFirst(method: string): string[] | null {
    const index = this.lines.findIndex((line) => line.way === "received" && line.method === method);
    if (index < 0) return null;
    return this.lines
      .slice(0, index)
      .flatMap((line) => (line.way === "sent" && line.method ? [line.method] : []));
  }
}

function decisionRequests(methods: string[]): string[] {
  return methods.filter((method) => DECISION_METHODS.some((prefix) => method.startsWith(prefix)));
}

type ChatMessage = Record<string, unknown>;

/** The text of a chat-completions message: its content, or its text parts joined. */
function textOf(message: ChatMessage): string {
  const content = message.content;
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((part) =>
        part !== null && typeof part === "object" && typeof (part as ChatMessage).text === "string"
          ? ((part as ChatMessage).text as string)
          : "",
      )
      .join("");
  }
  return "";
}

/** `text` as one JSON object, or `null` when it is not one. */
function documentOf(text: string): Record<string, unknown> | null {
  try {
    const document: unknown = JSON.parse(text);
    return document !== null && typeof document === "object" && !Array.isArray(document)
      ? (document as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

/**
 * A chat-completions endpoint that calls `ask_user` once, at the first
 * request that carries the trigger, and answers every other request with
 * text. It keeps the messages of every request as it was sent them.
 */
class AskingModel {
  readonly requests: ChatMessage[][] = [];
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
    this.requests.push(
      Array.isArray(messages)
        ? messages.filter((one): one is ChatMessage => one !== null && typeof one === "object")
        : [],
    );
    let delta: Record<string, unknown>;
    let finish: string;
    if (text.includes(ASK_NOW) && !this.asked) {
      this.asked = true;
      delta = {
        role: "assistant",
        tool_calls: [
          {
            index: 0,
            id: CALL_ID,
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

  /**
   * The result of the model's `ask_user` call, parsed, once for each request
   * that carried it: the content of the tool message that names the call's
   * id.
   */
  resultsOfTheCall(): Record<string, unknown>[] {
    const found: Record<string, unknown>[] = [];
    for (const messages of this.requests) {
      for (const message of messages) {
        if (message.role !== "tool" || message.tool_call_id !== CALL_ID) continue;
        const result = documentOf(textOf(message));
        assert.ok(result, `the call's result is no JSON object: ${JSON.stringify(message)}`);
        found.push(result);
      }
    }
    return found;
  }

  /**
   * The end of the request `requestId` as the model was sent it, parsed,
   * once for each model request that carried it: an input whose first line
   * is the label and whose rest is the result as one JSON document.
   */
  endedRequests(requestId: unknown): Record<string, unknown>[] {
    const found: Record<string, unknown>[] = [];
    for (const messages of this.requests) {
      for (const message of messages) {
        const text = textOf(message);
        const cut = text.indexOf("\n");
        if (message.role !== "user" || cut < 0 || text.slice(0, cut) !== RESULT_LABEL) continue;
        const result = documentOf(text.slice(cut + 1));
        if (result && result.request_id === requestId) found.push(result);
      }
    }
    return found;
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
    const before = { ...process.env };
    // The gateway process takes this process's environment when its
    // transport is built, which happens after this point. Make that
    // environment the allow list first; what is removed here comes back in
    // the finally block.
    for (const name of Object.keys(process.env)) {
      if (!PASSED_ON.includes(name)) delete process.env[name];
    }
    for (const name of OWN_HOMES) {
      const home = join(dir, name.toLowerCase());
      mkdirSync(home);
      process.env[name] = home;
    }
    // Every line between this host and its gateway is recorded from its
    // start.
    const wire = new Wire();
    const fake = await AskingModel.start();
    let runtime: MetaRuntime | undefined;
    const notices: Record<string, unknown>[] = [];
    try {
      const notAllowed = Object.keys(process.env)
        .filter((name) => !PASSED_ON.includes(name) && !OWN_HOMES.includes(name))
        .sort();
      assert.deepEqual(notAllowed, [], `names outside the allow list would be inherited: ${notAllowed}`);
      wire.wrapTheTransport();

      const mobToml = join(dir, "mob.toml");
      const config = join(dir, "config.toml");
      writeFileSync(mobToml, MOB_TOML);
      writeFileSync(config, configToml(fake.baseUrl));
      runtime = await Meta.builder()
        .gateway(gatewayBin)
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
      assert.equal(
        wire.requestsSentBeforeTheFirst(NOTICE_METHOD),
        null,
        "the gateway wrote a notice before anything was asked",
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

      // Counted on the wire: before the notice line the host had written no
      // request that lists, reads or decides a decision or a question set.
      const beforeTheNotice = wire.requestsSentBeforeTheFirst(NOTICE_METHOD);
      assert.ok(beforeTheNotice, "the handler holds a notice the wire log does not");
      assert.equal(
        decisionRequests(beforeTheNotice).length,
        0,
        "the host wrote decision or question requests before the notice: " +
          `${JSON.stringify(decisionRequests(beforeTheNotice))} ` +
          `(all it wrote before: ${JSON.stringify(beforeTheNotice)})`,
      );

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

      // The asking agent's model is sent the end of its request. The call
      // itself answered at once that the questions were asked, under a
      // request id; a later model request carries an ask_user_result input
      // for that request id. Both are parsed as the model was sent them.
      await until(
        () => fake.endedRequests(noticed.decision_id).length >= 1,
        "a model request that carries the end of the asked request",
      );
      const ofTheCall = fake.resultsOfTheCall();
      assert.ok(ofTheCall.length >= 1, "no model request carried the result of the ask_user call");
      assert.equal(ofTheCall[0].outcome, "asked", JSON.stringify(ofTheCall[0]));
      assert.equal(ofTheCall[0].request_id, noticed.decision_id, JSON.stringify(ofTheCall[0]));
      const ended = fake.endedRequests(ofTheCall[0].request_id)[0];
      assert.equal(ended.outcome, "answered", JSON.stringify(ended));
      assert.deepEqual(
        ended.answers,
        [{ question_id: "day", answer: { kind: "choice", choice_id: CHOSEN } }],
        JSON.stringify(ended),
      );

      // The answer wrote no further notice.
      await sleep(QUIET_MS);
      assert.equal(
        notices.length,
        1,
        "a further notice was observed by the handler after the answer, " +
          `watched for ${QUIET_MS / 1000} s: ${JSON.stringify(notices)}`,
      );

      // Over the whole row the host wrote the three decision and question
      // requests the row itself made, and no other.
      assert.deepEqual(
        decisionRequests(wire.requestsSent()),
        ["meta/gating/questions", "meta/gating/decide", "meta/gating/questions"],
        JSON.stringify(wire.requestsSent()),
      );
    } finally {
      try {
        if (runtime) await runtime.shutdown();
      } finally {
        await fake.close();
        wire.restoreTheTransport();
        for (const name of Object.keys(process.env)) delete process.env[name];
        Object.assign(process.env, before);
        rmSync(dir, { recursive: true, force: true });
      }
    }
  });
});
