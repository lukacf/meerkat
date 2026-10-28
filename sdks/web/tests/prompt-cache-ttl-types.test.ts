import type {
  OpenAiPromptCacheOptions,
  OpenAiPromptCacheTtl,
} from '../src/generated/events.js';

// The OpenAI prompt-cache TTL vocabulary carries the 12-hour lifetime.
// Checked with `tsc --noEmit`; unrun until the candidate's validation.
type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;

const ttls: Equal<OpenAiPromptCacheTtl, '30m' | '12h'> = true;
const options: OpenAiPromptCacheOptions = { mode: 'implicit', ttl: '12h' };
// @ts-expect-error `24h` is a prompt_cache_retention value, not a TTL.
const retentionValue: OpenAiPromptCacheTtl = '24h';

void [ttls, options, retentionValue];
