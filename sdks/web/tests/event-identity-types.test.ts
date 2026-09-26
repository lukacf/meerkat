import type {
  TranscriptMessageIdentity,
  RealtimeMessageOrigin,
  LiveContextObservationId,
  ObjectiveId,
  LiveChannelId,
  RunId,
  RunStartedEvent,
  RunCompletedEvent,
  RunFailedEvent,
} from "../src/index.js";
const objective: ObjectiveId = "objective";
const channel: LiveChannelId = "channel";
const run: RunId = "run";
const observation: LiveContextObservationId = {
  channel_id: channel,
  namespace: "test",
  nonce: "nonce",
};
const origin: RealtimeMessageOrigin = {
  channel_id: channel,
  session_id: "session",
  canonical_row_sequence: 0,
  context_observation_id: observation,
  provider_item_ids: [],
};
const identity: TranscriptMessageIdentity = {
  interaction_id: null,
  run_id: run,
  objective_id: objective,
  realtime_origin: origin,
};
const empty: TranscriptMessageIdentity = {};
const nullable: TranscriptMessageIdentity = {
  interaction_id: null,
  run_id: null,
  objective_id: null,
  realtime_origin: null,
};
function read(
  event: RunStartedEvent | RunCompletedEvent | RunFailedEvent,
): TranscriptMessageIdentity | undefined {
  return event.identity;
}
// @ts-expect-error Present identity must be a record.
const invalid: TranscriptMessageIdentity = "guessed";
// @ts-expect-error Realtime provenance requires canonical sequence, channel and session.
const missing: TranscriptMessageIdentity = { realtime_origin: {} };
void [identity, empty, nullable, read, invalid, missing];
