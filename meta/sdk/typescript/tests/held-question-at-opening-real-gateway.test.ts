/**
 * A question set pending before the gateway starts reaches an SDK host.
 *
 * A member's question set is a durable decision of kind
 * `ask_user.questions`. A gateway that starts over a state directory which
 * already holds one open tells its host of it with no list call
 * (`MetaRuntime.onDecisionNotice`); the host's first `decisionsList` returns
 * that one open set and writes its notice once more, and a later list
 * writes none.
 *
 * A second row follows the notice to its end as a host would: with the id
 * the notice names it reads the set (`MobHandle.gatingQuestions`; a set's
 * `pendingId` is the decision's id) and answers it
 * (`MobHandle.gatingAnswer`). The set then leaves the open sets and its
 * decision is no longer open.
 *
 * The pairing and its order
 * -------------------------
 *
 * No SDK call opens a question set, so these rows need a state directory
 * that was seeded before the gateway starts. Three steps, in this order:
 *
 * 1. The seed step, the ignored Rust test
 *    `seed_a_pending_question_set_for_an_sdk_row` of the Meta crate's
 *    `decision_notice_at_opening` test target, run by name. It holds one
 *    question set through another owner on the store, as the stock tool's
 *    host does, and writes a carrier file (the set's decision id, when it
 *    was seeded and after how many seconds it expires) BESIDE the directory,
 *    not in it: in the directory's parent, named as the directory with the
 *    suffix `.sdk-held-question-seed.json`. With the repository's cargo
 *    wrapper:
 *
 *        META_SDK_HELD_QUESTION_STATE=<a fresh directory> \
 *            cargo test -p meerkat-meta --test decision_notice_at_opening \
 *            -- --ignored --exact seed_a_pending_question_set_for_an_sdk_row
 *
 * 2. A gateway built from the same revision (`rpc_gateway`).
 *
 * 3. These rows, from `meta/sdk/typescript`, before the seeded set expires
 *    (the carrier file says when it was seeded and after how long it
 *    expires; the rows hold no number of their own for that):
 *
 *        META_RPC_GATEWAY_BIN=<path to that rpc_gateway> \
 *            META_SDK_HELD_QUESTION_STATE=<that directory> \
 *            npx tsx --test tests/held-question-at-opening-real-gateway.test.ts
 *
 * The two rows run in the order of this file, and the second one ends the
 * set: a seeded directory serves ONE run of this file. The second row leaves
 * a marker file beside the directory (the directory's name with the suffix
 * `.sdk-held-question-answered`), and both rows refuse a directory that has
 * one. The Python SDK has the same two rows
 * (`sdk/python/tests/test_held_question_at_opening_real_gateway.py`); they
 * need a seeded directory of their own.
 *
 * The directory is the fixture's own and disposable. These rows refuse a
 * path that is not absolute, a symbolic link or a path through one, any path
 * in the tree of `$HOME`, `$XDG_STATE_HOME` or `$XDG_CONFIG_HOME` (compared
 * in resolved form), a directory without its carrier file beside it, and an
 * expired seed. The gateway takes the directory as its persistent state,
 * which is intended: the decision store it must read lives there. It also
 * writes the rest of its own state there (its mob, sessions and logs).
 *
 * The gateway process gets an environment built from an allow list, not
 * this process's environment with names removed: `HOME` and the four XDG
 * homes are directories of the row, and `PATH`, the locale variables,
 * `TMPDIR` and the SDK transport's two stderr switches are passed on when
 * set (see `PASSED_ON`). Nothing else is inherited, so no provider key
 * reaches the gateway whatever its name. The names a row removes come back
 * when it ends.
 *
 * What these rows do not show
 * ---------------------------
 *
 * They run on the revision they are run on, not on a joined line. They show
 * nothing of a restart of a gateway, of how often a notice is delivered
 * across restarts, or of what a person is shown. The set was seeded before
 * the gateway started: a question that opens while the host is connected is
 * not shown here. The handler is registered once the runtime is built, so
 * the first notice reaches it from the notices the SDK keeps for a handler
 * that registers late. The second row answers as the embedding host (the
 * stdio decide trusts its host and records the `approverId` it names); it
 * does not show that the asking agent receives the answers, since the seeded
 * asker is no member of the row's mob. The rows record the calls of the
 * SDK's notice handler only, so a failed wait says that a notice was not
 * observed by the handler in time, not that the gateway wrote none.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, sep } from "node:path";

import { Meta } from "../src/index.js";
import type { MetaRuntime } from "../src/index.js";

const gatewayBin = (process.env.META_RPC_GATEWAY_BIN ?? "").trim();
const seededState = (process.env.META_SDK_HELD_QUESTION_STATE ?? "").trim();

const QUESTION_KIND = "ask_user.questions";
const SEED_STEP = "seed_a_pending_question_set_for_an_sdk_row";
const SEED_CARRIER_SUFFIX = ".sdk-held-question-seed.json";
// The marker the answering row leaves beside a seeded directory whose set it
// ended: such a directory holds no open set for another run.
const ANSWERED_MARKER_SUFFIX = ".sdk-held-question-answered";
// The approver the answering row names. The stdio decide trusts its host and
// records the name it gives; nobody is authenticated by it.
const ANSWERS_AS = "sdk-held-question-row";

// How long a row waits for an observation it expects.
const WAIT_MS = 30_000;
// How long a row watches before it says that no further notice was observed.
const QUIET_MS = 500;

const skip =
  !(gatewayBin && existsSync(gatewayBin) && seededState) &&
  "these rows need both META_RPC_GATEWAY_BIN (a built rpc_gateway) and " +
    "META_SDK_HELD_QUESTION_STATE (a directory seeded by the ignored Rust test " +
    `${SEED_STEP} of the Meta crate's decision_notice_at_opening target, ` +
    "run by name with --ignored)";

// The whole environment of the gateway process is an allow list: the names
// below, passed on from this process when set, and the homes a row sets.
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
// Set by a row to directories of its own, so the gateway writes nothing to
// the user's home or XDG homes.
const OWN_HOMES = ["HOME", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"];

const MOB_TOML = `[mob]
id = "held-question-at-opening-real-gateway-typescript"

[profiles.default]
model = "gpt-5.5"
`;

type Notice = Record<string, unknown>;

/** The carrier file of a seeded directory: beside it, never inside it. */
function carrierOf(state: string): string {
  return join(dirname(state), basename(state) + SEED_CARRIER_SUFFIX);
}

/** The marker of a seeded directory whose set was answered: beside it. */
function answeredMarkerOf(state: string): string {
  return join(dirname(state), basename(state) + ANSWERED_MARKER_SUFFIX);
}

function within(path: string, tree: string): boolean {
  return path === tree || path.startsWith(tree.endsWith(sep) ? tree : tree + sep);
}

/** The seeded directory, or a failure that says why it is not used. */
function seededDirectory(): string {
  const state = seededState;
  assert.ok(isAbsolute(state), `META_SDK_HELD_QUESTION_STATE is not an absolute path: ${state}`);
  assert.ok(existsSync(state), `the seeded directory does not exist: ${state}; run ${SEED_STEP} first`);
  const entry = lstatSync(state);
  assert.ok(!entry.isSymbolicLink(), `the seeded directory is a symbolic link: ${state}`);
  assert.ok(entry.isDirectory(), `the seeded path is not a directory: ${state}`);
  assert.equal(
    realpathSync(dirname(state)),
    dirname(state),
    `the path of the seeded directory holds a symbolic link or a relative part: ${state}`,
  );
  const resolved = realpathSync(state);
  for (const name of ["HOME", "XDG_STATE_HOME", "XDG_CONFIG_HOME"]) {
    const location = process.env[name];
    if (!location || !existsSync(location)) continue;
    const tree = realpathSync(location);
    assert.ok(
      !within(resolved, tree),
      `the seeded directory lies in the tree of ${name} (${tree}): ${state}`,
    );
  }
  const carrier = carrierOf(state);
  assert.ok(
    existsSync(carrier) && lstatSync(carrier).isFile(),
    `there is no carrier file ${carrier} beside ${state}: it was not seeded by ` +
      `${SEED_STEP} (run that step by name with --ignored on a fresh directory)`,
  );
  const marker = answeredMarkerOf(state);
  assert.ok(
    !existsSync(marker),
    `the question set of ${state} was answered by an earlier run of this file ` +
      `(${marker} says so): run ${SEED_STEP} again on a fresh directory`,
  );
  return state;
}

/**
 * The seeded decision id and a note of the seed's age. Fails, before
 * anything is started, when the seeded set has expired. The expiry is the
 * carrier's own; the rows hold no number for it.
 */
function seedOf(state: string): { seededId: string; note: string } {
  const carrier = JSON.parse(readFileSync(carrierOf(state), "utf-8")) as {
    decision_id: string;
    kind: string;
    seeded_at_ms: number;
    expires_after_seconds: number;
  };
  assert.equal(carrier.kind, QUESTION_KIND, JSON.stringify(carrier));
  const ageSeconds = (Date.now() - carrier.seeded_at_ms) / 1000;
  const note =
    `seeded ${Math.floor(ageSeconds / 60)} minutes ago; the question set expires ` +
    `${Math.floor(carrier.expires_after_seconds / 60)} minutes after its seed`;
  assert.ok(
    ageSeconds < carrier.expires_after_seconds,
    `the seed is too old to use: ${note}. Run ${SEED_STEP} again on a fresh directory`,
  );
  return { seededId: carrier.decision_id, note };
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** Wait until the handler received `count` notices. */
async function observed(notices: Notice[], count: number, what: string, note: string): Promise<void> {
  const deadline = Date.now() + WAIT_MS;
  while (notices.length < count) {
    assert.ok(
      Date.now() < deadline,
      `${what}: not observed by the handler within ${WAIT_MS / 1000} s ` +
        `(the handler received ${notices.length} of ${count}; ${note})`,
    );
    await sleep(10);
  }
}

/** The answer of `call`, or a failure that names what was awaited. */
async function answered<T>(call: Promise<T>, what: string, note: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`${what}: not received within ${WAIT_MS / 1000} s (${note})`)),
      WAIT_MS,
    );
  });
  try {
    return await Promise.race([call, late]);
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Run `row` against a gateway that takes the seeded directory as its state
 * and inherits the allow list only. The notices its handler received are
 * handed to the row as they arrive.
 */
async function withGatewayOver(
  state: string,
  note: string,
  row: (runtime: MetaRuntime, notices: Notice[]) => Promise<void>,
): Promise<void> {
  const own = mkdtempSync(join(tmpdir(), "meta-held-question-"));
  const before = { ...process.env };
  // The gateway process takes this process's environment when its transport
  // is built, which happens after this point. Make that environment the
  // allow list first; what is removed here comes back in the finally block.
  for (const name of Object.keys(process.env)) {
    if (!PASSED_ON.includes(name)) delete process.env[name];
  }
  for (const name of OWN_HOMES) {
    const home = join(own, name.toLowerCase());
    mkdirSync(home);
    process.env[name] = home;
  }
  let runtime: MetaRuntime | undefined;
  try {
    const notAllowed = Object.keys(process.env)
      .filter((name) => !PASSED_ON.includes(name) && !OWN_HOMES.includes(name))
      .sort();
    assert.deepEqual(notAllowed, [], `names outside the allow list would be inherited: ${notAllowed}`);

    const mobToml = join(own, "mob.toml");
    writeFileSync(mobToml, MOB_TOML);
    runtime = await answered(
      Meta.builder().gateway(gatewayBin).mob(mobToml).persistentState(state).build(),
      "the gateway's start (build)",
      note,
    );
    const notices: Notice[] = [];
    runtime.onDecisionNotice((notice) => {
      notices.push(notice);
    });
    await row(runtime, notices);
  } finally {
    try {
      if (runtime) await runtime.shutdown();
    } finally {
      for (const name of Object.keys(process.env)) delete process.env[name];
      Object.assign(process.env, before);
      rmSync(own, { recursive: true, force: true });
    }
  }
}

describe("a question set pending before the gateway starts", { skip }, () => {
  it("is noticed to the host before any list", async () => {
    const state = seededDirectory();
    const { seededId, note } = seedOf(state);
    await withGatewayOver(state, note, async (runtime, notices) => {
      // The host has made no list call: the gateway tells of the open
      // question set on its own, once.
      await observed(notices, 1, "the notice of the seeded question set before any list", note);
      await sleep(QUIET_MS);
      assert.equal(
        notices.length,
        1,
        "more than one notice was observed by the handler before any list, " +
          `watched for ${QUIET_MS / 1000} s after the first: ${JSON.stringify(notices)}`,
      );
      const opening = notices[0];
      assert.equal(opening.decision_id, seededId, JSON.stringify(opening));
      assert.equal(opening.kind, QUESTION_KIND, JSON.stringify(opening));
      assert.equal(opening.event, "held", JSON.stringify(opening));
      assert.equal(opening.phase, "open", JSON.stringify(opening));
      assert.equal(opening.presentation ?? null, null, JSON.stringify(opening));

      // The host's first list returns exactly that one open question set
      // and writes its notice once more.
      const first = await answered(
        runtime.mobHandle().decisionsList(),
        "the first list's answer",
        note,
      );
      const listed = first.decisions as Record<string, unknown>[];
      assert.equal(listed.length, 1, `the first list returned ${listed.length} decisions`);
      assert.equal(listed[0].decision_id, seededId, JSON.stringify(listed[0]));
      assert.equal(listed[0].kind, QUESTION_KIND, JSON.stringify(listed[0]));
      assert.equal(listed[0].phase, "open", JSON.stringify(listed[0]));
      await observed(notices, 2, "the notice of the seeded question set at the first list", note);
      await sleep(QUIET_MS);
      assert.equal(
        notices.length,
        2,
        "more than one notice was observed by the handler at the first list, " +
          `watched for ${QUIET_MS / 1000} s after it: ${JSON.stringify(notices)}`,
      );
      const atList = notices[1];
      assert.equal(atList.decision_id, seededId, JSON.stringify(atList));
      assert.equal(atList.kind, QUESTION_KIND, JSON.stringify(atList));
      assert.equal(atList.event, "held", JSON.stringify(atList));

      // A later list writes none.
      await answered(runtime.mobHandle().decisionsList(), "the second list's answer", note);
      await sleep(QUIET_MS);
      assert.equal(
        notices.length,
        2,
        "a further notice was observed by the handler at the second list, " +
          `watched for ${QUIET_MS / 1000} s after its answer: ${JSON.stringify(notices)}`,
      );
    });
  });

  it("is read and answered by the host with the id its notice names", async () => {
    const state = seededDirectory();
    const { seededId, note } = seedOf(state);
    await withGatewayOver(state, note, async (runtime, notices) => {
      const handle = runtime.mobHandle();

      // The notice is all the host has: the id of an open decision and its
      // kind, which says that an agent asked a person something.
      await observed(notices, 1, "the notice of the seeded question set before any list", note);
      const noticed = notices[0];
      assert.equal(noticed.event, "held", JSON.stringify(noticed));
      assert.equal(noticed.kind, QUESTION_KIND, JSON.stringify(noticed));
      assert.equal(noticed.decision_id, seededId, JSON.stringify(noticed));

      // With that id the host reads what was asked: the one open set whose
      // pending id is the decision's id.
      const openSets = await answered(
        handle.gatingQuestions(),
        "the open question sets (gatingQuestions)",
        note,
      );
      const named = openSets.filter((one) => one.pendingId === noticed.decision_id);
      assert.equal(
        named.length,
        1,
        `gatingQuestions returned ${named.length} sets with the noticed id ` +
          `${String(noticed.decision_id)}: ${JSON.stringify(openSets)}`,
      );
      const asked = named[0];
      assert.ok(asked.questions.length > 0, JSON.stringify(asked));
      assert.ok(
        asked.questions.every((question) => question.choices.length > 0),
        JSON.stringify(asked),
      );
      assert.equal(asked.subjectDigest, noticed.rendering_ref, JSON.stringify([asked, noticed]));

      // The host answers every question with its first choice, as the set it
      // read offers them.
      const result = await answered(
        handle.gatingAnswer(asked.pendingId, {
          approverId: ANSWERS_AS,
          presentedDigest: asked.subjectDigest,
          answers: asked.questions.map((question) => ({
            questionId: question.id,
            answer: { kind: "choice" as const, choiceId: question.choices[0].id },
          })),
        }),
        "the answer's result (gatingAnswer)",
        note,
      );
      // The set is ended from here on: say so beside the directory before
      // anything else can fail.
      writeFileSync(
        answeredMarkerOf(state),
        JSON.stringify({ decision_id: seededId, outcome: result.outcome }),
      );
      assert.equal(result.pendingId, seededId, JSON.stringify(result));
      assert.equal(result.kind, "question_set", JSON.stringify(result));
      assert.equal(result.outcome, "answered", JSON.stringify(result));

      // The answered set is no longer among the open sets, and its decision
      // is no longer open.
      const stillOpen = await answered(
        handle.gatingQuestions(),
        "the open question sets after the answer",
        note,
      );
      assert.deepEqual(
        stillOpen.map((one) => one.pendingId),
        [],
        JSON.stringify(stillOpen),
      );
      const listed = await answered(
        handle.decisionsList(),
        "the list after the answer (decisionsList)",
        note,
      );
      const rows = [
        ...((listed.decisions as Record<string, unknown>[] | undefined) ?? []),
        ...((listed.statuses as Record<string, unknown>[] | undefined) ?? []),
      ];
      const row = rows.find((one) => one.decision_id === seededId);
      assert.ok(row, `the list after the answer does not name ${seededId}: ${JSON.stringify(listed)}`);
      assert.notEqual(row.phase, "open", JSON.stringify(row));

      // A list notices open decisions only: the ended set gets no further
      // notice.
      await sleep(QUIET_MS);
      assert.equal(
        notices.length,
        1,
        "a further notice was observed by the handler after the answer, " +
          `watched for ${QUIET_MS / 1000} s after the list: ${JSON.stringify(notices)}`,
      );
    });
  });
});
