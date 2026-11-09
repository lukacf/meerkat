import { Client as UpstreamClient } from '../node_modules/@hubspot/api-client/lib/index.js';
import { PromiseBasicApi as BasicApi } from '../node_modules/@hubspot/api-client/lib/codegen/crm/contacts/types/PromiseAPI.js';
import { createConfiguration } from '../node_modules/@hubspot/api-client/lib/codegen/crm/contacts/configuration.js';
import { ResponseContext, wrapHttpLibrary } from '../node_modules/@hubspot/api-client/lib/codegen/crm/contacts/http/http.js';
import { sdkRequest } from './sdk-request';
declare const __toolkitTransport: (request: unknown) => Promise<{ status: number; body: unknown }>;
export class Client extends UpstreamClient {
  constructor(options: { accessToken: string }) {
    // The real Client and its module-local decorator singleton live inside the
    // invocation factory. No limiter, retry decorator or process global changes.
    super({ ...options, numberOfApiCallRetries: 0 });
    this.crm.contacts.basicApi = new BasicApi(createConfiguration({
      authMethods: { oauth2: { accessToken: options.accessToken } }, middleware: [],
      httpApi: wrapHttpLibrary({ async send(request) {
        if (request.getAgent() !== undefined) throw new Error('Unsupported HubSpot request');
        const response = await __toolkitTransport(sdkRequest(request.getUrl(), request.getHttpMethod(), request.getHeaders(), request.getBody()));
        const text = JSON.stringify(response.body ?? null);
        return new ResponseContext(response.status, { 'content-type': 'application/json' }, {
          text: async () => text, binary: async () => Buffer.from(text),
        });
      } }),
    }));
  }
}
