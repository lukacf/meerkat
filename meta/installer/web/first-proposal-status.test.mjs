import test from "node:test";
import assert from "node:assert/strict";
import { firstProposalStatus } from "./first-proposal-status.mjs";

const running = () => ({
  id: "first-idea-guide",
  version: 1,
  status: "question",
  operation: {
    status: "running",
    request_id: "a".repeat(64),
    started_at: "2026-10-10T09:10:00Z",
    request: { text: "Help our neighbours share tools." },
  },
});
const live = () => ({ guideId: "first-idea-guide", available: true, error: "", startup: null });
const recoverySnapshot = changes => ({ ...live(), ...changes });
const isRecovery = value => {
  assert.ok(value, "The empty proposal pane retains a visible status.");
  assert.equal(value.showConversation, true, "Recovery remains reachable from the hidden Conversation pane.");
  assert.match(value.detail, /Open Conversation/);
  assert.doesNotMatch(`${value.title} ${value.detail}`, /guide is working|proposal will appear/,
    "Observation uncertainty must not promise healthy guide progress or a forthcoming proposal.");
};

test("normal admitted first turn preserves the existing working banner", () => {
  assert.deepEqual(firstProposalStatus(running(), live()), {
    title: "Your guide is working on your idea.",
    detail: "Your first proposal will appear here. You can keep writing.",
    tone: "working",
    showConversation: false,
  });
});

test("mobile empty pane does not claim healthy work when current timeline observation is interrupted", () => {
  const state = running();
  const timeline = recoverySnapshot({ error: "Network observation interrupted" });
  const before = structuredClone({ state, timeline });
  const value = firstProposalStatus(state, timeline);
  isRecovery(value);
  assert.match(value.title, /Live updates interrupted.*Reconnecting/);
  assert.equal(value.tone, "notice");
  assert.deepEqual({ state, timeline }, before, "The notice never changes the saved request or observation owner.");
});

test("typed startup failure takes precedence over the timeline error without starting or resending anything", () => {
  let effects = 0;
  const value = firstProposalStatus(running(), recoverySnapshot({
    error: "Timeline unavailable",
    startup: { status: "failed", retryRequired: true, available: true },
    retrySetupTeam() { effects += 1; },
    checkSetupTeamStatus() { effects += 1; },
  }));
  isRecovery(value);
  assert.match(value.title, /could not start/);
  assert.equal(value.tone, "attention");
  assert.doesNotMatch(value.title, /Reconnecting/);
  assert.equal(effects, 0, "The helper offers navigation, never operation authority.");
});

test("an explicit retry remains distinct from failed startup and ordinary working", () => {
  const value = firstProposalStatus(running(), recoverySnapshot({
    retrying: true, error: "Earlier read failed",
    startup: { status: "failed", retryRequired: true },
  }));
  isRecovery(value);
  assert.match(value.title, /^Retrying setup team/);
  assert.equal(value.tone, "notice");
  assert.doesNotMatch(value.title, /could not start/);
});

test("starting the setup team is reported as waiting for live updates", () => {
  const value = firstProposalStatus(running(), recoverySnapshot({
    error: "Earlier timeline read failed", startup: { status: "starting", available: true },
  }));
  isRecovery(value);
  assert.match(value.title, /Setup team is starting/);
  assert.match(value.detail, /Waiting for live updates/);
});

test("typed unavailable setup team does not imply a completed or healthy first proposal", () => {
  const value = firstProposalStatus(running(), recoverySnapshot({
    startup: { status: "ready", available: false },
  }));
  isRecovery(value);
  assert.match(value.title, /unavailable/);
  assert.equal(value.tone, "attention");
});

test("another guide cannot lend its recovery state to the current first request", () => {
  for (const changes of [
    { error: "Other guide lost observation" },
    { startup: { status: "failed", retryRequired: true } },
    { startup: { status: "starting" } },
    { startup: { available: false } },
    { retrying: true },
  ]) {
    const stale = { ...recoverySnapshot(changes), guideId: "other-guide" };
    assert.deepEqual(firstProposalStatus(running(), stale), firstProposalStatus(running(), live()));
  }
});

test("saved failure and cancellation interruption retain recovery rather than an endless working promise", () => {
  for (const status of ["failed", "interrupted"]) {
    const state = running();
    state.operation.status = status;
    const before = structuredClone(state);
    const value = firstProposalStatus(state, recoverySnapshot({ retrying: true }));
    isRecovery(value);
    assert.equal(value.tone, "attention");
    assert.match(value.title, status === "failed" ? /could not finish/ : /was interrupted/);
    assert.deepEqual(state, before, "Terminal operations remain terminal; presentation admits no retry.");
  }
});

test("the ordinary opening, completed turns and requests without a saved binding remain silent", () => {
  const failedTimeline = recoverySnapshot({ startup: { status: "failed", retryRequired: true } });
  const state = running();
  for (const value of [null, { id: state.id }, { ...state, id: null },
    { ...state, operation: null }, { ...state, operation: { status: "running", request_id: "" } },
    ...["completed", "cancelled", "future-status", undefined].map(status => ({
      ...state, operation: { ...state.operation, status },
    }))]) {
    assert.equal(firstProposalStatus(value, failedTimeline), null);
  }
});

test("an actual saved proposal closes the initial banner even if old recovery data remains", () => {
  for (const key of ["proposal", "blueprint"]) {
    assert.equal(firstProposalStatus({ ...running(), [key]: { project: { name: "Tool sharing" } } },
      recoverySnapshot({ error: "An earlier observation failed" })), null);
  }
});

test("successful current observation restores the original working copy without changing the request", () => {
  const state = running();
  const before = structuredClone(state);
  isRecovery(firstProposalStatus(state, recoverySnapshot({ error: "Transient observation failure" })));
  const recovered = firstProposalStatus(state, live());
  assert.equal(recovered.showConversation, false);
  assert.equal(recovered.tone, "working");
  assert.match(recovered.title, /guide is working/);
  assert.deepEqual(state, before);
});

test("no native history yet is not classified as a setup failure", () => {
  const value = firstProposalStatus(running(), { ...live(), available: false, frames: [], bindings: [] });
  assert.equal(value.tone, "working");
  assert.equal(value.showConversation, false);
});
