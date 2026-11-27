/** Compile-only public API contracts. The raw compatibility path is intentional. */
import {
  DecisionPresentation,
  type DecisionAudience,
  type DecisionApproverAudience,
  type DecisionReviewerAudience,
  type DecisionCanonicalPart,
  type DecisionPresentationLink,
  type DecisionPresentationOptions,
  type DecisionAttestation,
  type MobHandle,
} from "../src/index.js";

const approver: DecisionApproverAudience = { kind: "approver", approverId: "owners" };
const reviewer: DecisionReviewerAudience = { kind: "reviewer", reviewer: "principal:reviewer" };
const audiences: DecisionAudience[] = [approver, reviewer];
const part: DecisionCanonicalPart = { index: 0, count: 1, partDigest: "digest" };
const link: DecisionPresentationLink = { reference: "document", servedDigest: "digest" };
const options: DecisionPresentationOptions = {
  audience: audiences[0], subjectDigest: "digest", partsShown: [part],
  channelRef: "chat", messageRef: "message", atMs: 0,
  maxPartBytes: null, link,
};
const typed = new DecisionPresentation(options);

// @ts-expect-error An approver audience must identify its approver.
const missingAudience: DecisionAudience = { kind: "approver" };
// @ts-expect-error A reviewer cannot carry the approver variant's field.
const mixedAudience: DecisionAudience = { kind: "reviewer", reviewer: "r", approverId: "a" };
// @ts-expect-error A canonical part requires its digest.
const missingPart: DecisionCanonicalPart = { index: 0, count: 1 };
// @ts-expect-error A canonical part's index is numeric.
const wrongPart: DecisionCanonicalPart = { index: "0", count: 1, partDigest: "d" };
// @ts-expect-error A summary link requires the served digest.
const missingLink: DecisionPresentationLink = { reference: "document" };
// @ts-expect-error A typed request requires the audience and presentation fields.
new DecisionPresentation({ atMs: 0 });
// @ts-expect-error Wrong nested fields do not enter the typed constructor.
new DecisionPresentation({ ...options, audience: { kind: "approver", approverId: 7 } });

async function acceptsTypedAndRaw(handle: MobHandle) {
  const attestation: DecisionAttestation = {
    ingress: "chat", subject: "person", scope: "session",
    eventRef: "event", observedAtMs: 1, presentation: typed,
  };
  await handle.decisionsPresent("decision", typed);
  await handle.decisionsDecide("decision", { approve: true, presentedDigest: "d", attestation });
  const raw: Record<string, unknown> = { future: "gateway validates raw input" };
  await handle.decisionsPresent("decision", raw);
  await handle.decisionsDecide("decision", {
    approve: true, presentedDigest: "d", attestation: { ...attestation, presentation: raw },
  });
}
void acceptsTypedAndRaw;
void [missingAudience, mixedAudience, missingPart, wrongPart, missingLink];
