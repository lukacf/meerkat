/**
 * Type-level tests for the Web SDK tool callback's optional dedupe key,
 * checked with `tsc --noEmit`. Unrun until the candidate's validation.
 */

import { parseToolCallDedupeKey } from '../src/index.js';
import type { ToolCallback, ToolCallDedupeKey } from '../src/index.js';

const plain: ToolCallback = async (args) => ({ content: args, is_error: false });
const keyed: ToolCallback = async (_args, dedupeKey) => ({
  content: dedupeKey?.call_id ?? 'no key',
  is_error: false,
});
const parsed: ToolCallDedupeKey | undefined = parseToolCallDedupeKey(undefined);
// @ts-expect-error The second argument is a typed key, not its JSON text.
const raw: ToolCallback = async (_args, dedupeKey: string) => ({ content: dedupeKey, is_error: false });

void [plain, keyed, parsed, raw];
