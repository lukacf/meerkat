import { createClient as upstreamCreateClient } from '../node_modules/contentful-management/dist/es-modules/contentful-management.js';
import { sdkRequest } from './sdk-request';
declare const __toolkitTransport: (request: unknown) => Promise<{ status: number; body: unknown }>;
// Actual plain entry endpoint and Axios instance, with its supported adapter.
export function createClient(options: any, plainOptions: any) {
  return upstreamCreateClient({ ...options, retryOnError: false, retryLimit: 0, throttle: 0, proxy: false,
    logHandler: () => {}, requestLogger: () => {}, responseLogger: () => {},
    adapter: async (config: any) => {
      if (config.params !== undefined || config.signal !== undefined) throw new Error('Unsupported Contentful request');
      const url = new URL(config.url.replace(/^\//, ''), config.baseURL.endsWith('/') ? config.baseURL : config.baseURL + '/');
      const response = await __toolkitTransport(sdkRequest(url, config.method?.toUpperCase(), config.headers.toJSON(), config.data));
      const result = { status: response.status, statusText: '', data: response.body, headers: { 'content-type': 'application/json' }, config };
      if (response.status < 200 || response.status >= 300) throw Object.assign(new Error('Contentful HTTP error'), { response: result, config });
      return result;
    },
  }, plainOptions);
}
