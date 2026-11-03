import { SlackOperations } from "@rkat/toolkit";
import type { SlackIntervalResult, SlackIntervalOptions, SlackIntervalGapKind, SlackIntervalStop,
  SlackIntervalContinuation, SlackIntervalPending, SlackIntervalSelection } from "@rkat/toolkit";
import type { SlackIntervalResult as SubpathResult, SlackIntervalChain } from "@rkat/toolkit/connectors/slack";

declare const operations: SlackOperations;
const options: SlackIntervalOptions = { start: "10", end: "20", rootOldest: "0", maxRequests: 10, maxMessages: 100 };
const result: Promise<SlackIntervalResult> = operations.readInterval("C1", options);
const subpath: Promise<SubpathResult> = result;
void subpath;
const gap: SlackIntervalGapKind = "pagination_contradiction";
const stop: SlackIntervalStop = "message_budget";
const chain: SlackIntervalChain = { state: "stopped", pages: 1, stop_reason: stop, next_cursor: null };
void gap; void chain;
// @ts-expect-error Creation bounds are exact strings, never numbers.
operations.readInterval("C1", { ...options, start: 10 });
// @ts-expect-error Unknown diagnostics are not part of the portable result contract.
const unknown: SlackIntervalGapKind = "probably_complete";
void unknown;

const selection: SlackIntervalSelection = { channel: "C1", start: "10", end: "20", root_oldest: "0",
  page_limit: 15, include_all_metadata: false };
const pending: SlackIntervalPending = { root_ts: null, cursor: null, seen_cursors: [] };
const continuation: SlackIntervalContinuation = { schema_version: 1, selection, pending: [pending], discovered_roots: [] };
operations.readInterval("C1", { ...options, resumable: true, continuation });
const resumedChain: SlackIntervalChain = { state: "not_requested", pages: 0, stop_reason: null };
const continuationStop: SlackIntervalStop = "continuation_budget";
void resumedChain; void continuationStop;
// @ts-expect-error Continuations use a versioned object, not opaque strings.
operations.readInterval("C1", { ...options, resumable: true, continuation: "opaque" });
// @ts-expect-error The opt-in flag is explicitly boolean.
operations.readInterval("C1", { ...options, resumable: 1 });

const resultBudgetStop: SlackIntervalStop = "result_budget";
const resultBudgetGap: SlackIntervalGapKind = "result_budget";
operations.readInterval("C1", { ...options, maxResultBytes: 4096 });
void resultBudgetStop; void resultBudgetGap;
// @ts-expect-error Result bytes are a numeric bound.
operations.readInterval("C1", { ...options, maxResultBytes: "4096" });
