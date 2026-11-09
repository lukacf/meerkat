import { Client as UpstreamClient } from '../node_modules/@microsoft/microsoft-graph-client/lib/src/Client.js';
import { AuthenticationHandler } from '../node_modules/@microsoft/microsoft-graph-client/lib/src/middleware/AuthenticationHandler.js';
import { sdkRequest } from './sdk-request';
declare const __toolkitTransport: (request: unknown) => Promise<{ status: number; body: unknown }>;
export class Client extends UpstreamClient {
  static initWithMiddleware(options: any) {
    return UpstreamClient.initWithMiddleware({ baseUrl: options.baseUrl, debugLogging: false,
      middleware: [new AuthenticationHandler(options.authProvider), {
        async execute(context: any) {
          if (typeof context.request !== 'string' || context.options.signal !== undefined) throw new Error('Unsupported Graph request');
          const response = await __toolkitTransport(sdkRequest(context.request, context.options.method, context.options.headers, context.options.body));
          context.response = new Response(JSON.stringify(response.body ?? null), { status: response.status, headers: { 'Content-Type': 'application/json' } });
        },
      }],
    });
  }
}
