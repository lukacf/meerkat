// Preserve the actual SDK client and operation methods. Replace only its
// documented FetchFunction with the same owned, one-request transport.
import { IntercomClient as UpstreamClient } from '../node_modules/intercom-client/Client.js';
declare const __toolkitTransport: (request: unknown) => Promise<{ status: number; body: unknown }>;
export class IntercomClient extends UpstreamClient {
  constructor(options: ConstructorParameters<typeof UpstreamClient>[0]) {
    super({ ...options, fetcher: async args => {
      if (args.body !== undefined || args.requestType !== 'json' || args.contentType !== 'application/json' ||
        args.abortSignal !== undefined || args.maxRetries !== undefined || args.timeoutMs !== 20_000) {
        throw new Error('Unsupported SDK request');
      }
      const response = await __toolkitTransport({ method: args.method, url: args.url, headers: args.headers, queryParams: args.queryParameters });
      return response.status >= 200 && response.status < 300 ? { ok: true, body: response.body } :
        { ok: false, error: { reason: 'status-code', statusCode: response.status, body: undefined } };
    } });
  }
}
