import type {
  DelegatedRequestProvenance,
  LiveObservationPage,
  LiveObservationRecord,
  LiveObservationSnapshot,
  LiveTranscriptObservation,
  MobMemberLiveObservationsResult,
  TranscriptUserRole,
  WireLiveAdapterObservation,
} from "../src/generated/types.js";
import type { SessionMessage } from "../src/types.js";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;
type Assert<T extends true> = T;

export type HistoryPageIsNamed = Assert<
  Equal<MobMemberLiveObservationsResult["page"], LiveObservationPage>
>;
export type CommittedObservationIsNamed = Assert<
  Equal<
    Extract<WireLiveAdapterObservation, { observation: "live_observation_committed" }>["record"],
    LiveObservationRecord
  >
>;
export type HistorySnapshotIsNamed = Assert<
  Equal<LiveObservationPage["snapshot"], LiveObservationSnapshot>
>;
export type HistoryRecordsAreNamed = Assert<
  Equal<LiveObservationPage["records"][number], LiveObservationRecord>
>;
export type HistoryObservationIsNamed = Assert<
  Equal<LiveObservationRecord["observation"], LiveTranscriptObservation>
>;
export type SessionRoleIsCanonical = Assert<
  Equal<NonNullable<SessionMessage["transcriptRole"]>, TranscriptUserRole>
>;
export type DelegatedProvenanceIsNamed = Assert<
  Equal<
    Extract<TranscriptUserRole, { delegated_request: unknown }>["delegated_request"]["provenance"],
    DelegatedRequestProvenance
  >
>;
