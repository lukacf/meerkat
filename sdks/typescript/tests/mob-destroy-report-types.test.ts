import { MeerkatClient, MeerkatError, mobDestroyIncompleteReport } from "../src/index.js";
import type {
  WireMemberReloadRefusal,
  WireMobDestroyIncompleteData,
  WireMobDestroyReport,
} from "../src/index.js";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;

const error = new MeerkatError("mob_destroy_incomplete", "incomplete", {});
const report: WireMobDestroyReport | undefined = mobDestroyIncompleteReport(error);
const accessorIsTyped: Equal<ReturnType<typeof mobDestroyIncompleteReport>, WireMobDestroyReport | undefined> = true;

type DestroyReport = Awaited<ReturnType<MeerkatClient["mobDestroy"]>>["destroyReport"];
const destroyIsTyped: Equal<DestroyReport, WireMobDestroyReport> = true;
const dataIsTyped: Equal<WireMobDestroyIncompleteData["destroy_report"], WireMobDestroyReport> = true;

/** A host's exhaustive handling: a new refusal kind fails to type-check here. */
function retryLater(refusal: WireMemberReloadRefusal): boolean {
  switch (refusal.kind) {
    case "repair_blocked":
      return refusal.reason.length > 0;
    case "unknown":
      return true;
    default: {
      const unreachable: never = refusal;
      return unreachable;
    }
  }
}

// @ts-expect-error A report is not an arbitrary record.
const loose: WireMobDestroyReport = { reloaded_members: "worker-1" };
void [report, accessorIsTyped, destroyIsTyped, dataIsTyped, retryLater, loose];
