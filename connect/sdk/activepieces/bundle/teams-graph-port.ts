// Retain the real helper and error formatting, with retry explicitly disabled.
export * from '../vendor/activepieces/packages/pieces/community/microsoft-teams/src/lib/common/graph';
import { withGraphRetry as upstreamRetry } from '../vendor/activepieces/packages/pieces/community/microsoft-teams/src/lib/common/graph';
export const withGraphRetry = <T>(request: () => Promise<T>) => upstreamRetry(request, { maxRetries: 0 });
