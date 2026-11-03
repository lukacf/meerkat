// Real pinned SDK operations, with its documented fetch injection point.
import { Client as UpstreamClient } from '../node_modules/@notionhq/client/build/src/index.js';
declare const __toolkitTransport: (request: unknown) => Promise<{ status: number; body: unknown }>;
export class Client extends UpstreamClient {
  constructor(options: ConstructorParameters<typeof UpstreamClient>[0]) {
    super({ ...options, logger: () => {}, fetch: async (url, init) => {
      if (init?.agent !== undefined) throw new Error('Unsupported SDK request');
      const response = await __toolkitTransport({ method: init?.method, url, headers: init?.headers, body: init?.body });
      // The host body is already bounded, decoded and retired. This detached
      // response lets the real SDK perform its own JSON/error handling.
      return new Response(JSON.stringify(response.body ?? null), { status: response.status,
        headers: { 'Content-Type': 'application/json' } }) as never;
    } });
  }
}
