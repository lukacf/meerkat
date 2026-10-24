/**
 * Type-level tests for the Web SDK's typed mob destroy report, checked with
 * `tsc --noEmit`.
 */

import { mobDestroyIncompleteReport } from '../src/index.js';
import type {
  Mob,
  MobLifecycleResult,
  WireMemberReloadRefusal,
  WireMobDestroyIncompleteData,
  WireMobDestroyReport,
} from '../src/index.js';

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;

const accessorIsTyped: Equal<
  ReturnType<typeof mobDestroyIncompleteReport>,
  WireMobDestroyReport | undefined
> = true;
const lifecycleIsTyped: Equal<Awaited<ReturnType<Mob['lifecycle']>>, MobLifecycleResult> = true;
const lifecycleReportIsTyped: Equal<
  MobLifecycleResult['destroy_report'],
  WireMobDestroyReport | null | undefined
> = true;
const dataIsTyped: Equal<WireMobDestroyIncompleteData['destroy_report'], WireMobDestroyReport> = true;

/** A host's exhaustive handling: a new refusal kind fails to type-check here. */
function retryLater(refusal: WireMemberReloadRefusal): boolean {
  switch (refusal.kind) {
    case 'repair_blocked':
      return refusal.reason.length > 0;
    case 'unknown':
      return true;
    default: {
      const unreachable: never = refusal;
      return unreachable;
    }
  }
}

// @ts-expect-error A report is not an arbitrary record.
const loose: WireMobDestroyReport = { reloaded_members: 'worker-1' };
void [accessorIsTyped, lifecycleIsTyped, lifecycleReportIsTyped, dataIsTyped, retryLater, loose];
