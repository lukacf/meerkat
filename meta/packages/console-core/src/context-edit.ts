import { validateConsoleContexts, type ConsoleContextRecord } from "./context-record";

/** Edit a local snapshot without claiming an old range still names its text. */
export function editConsoleContextQuote(
  records: readonly ConsoleContextRecord[], id: string, quote: string,
): ConsoleContextRecord[] {
  if (!records.some(record => record.id === id)) throw new Error("This quote is no longer in the draft.");
  const next = records.map(record => {
    if (record.id !== id || record.quote === quote) return record;
    const { sourceRange: _sourceRange, ...snapshot } = record;
    return { ...snapshot, quote };
  });
  validateConsoleContexts(next);
  return next;
}
