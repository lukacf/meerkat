import GoTrueAdminApi from '../node_modules/@supabase/auth-js/dist/module/GoTrueAdminApi.js';
import { sdkRequest } from './sdk-request';
declare const __toolkitTransport: (request: unknown) => Promise<{ status: number; body: unknown }>;
// Deliberate constructor port: this is only auth.admin, not a full Supabase
// client. The actual public admin SDK owns getUserById and response handling;
// no GoTrueClient session initialization, auth listener or realtime client runs.
export function createClient(url: string, apiKey: string) {
  return { auth: { admin: new GoTrueAdminApi({ url: url + '/auth/v1',
    headers: { Authorization: `Bearer ${apiKey}`, apikey: apiKey },
    fetch: async (url, init) => {
      try {
        if (typeof url !== 'string' || init?.signal !== undefined) throw new Error('Unsupported Supabase request');
        const response = await __toolkitTransport(sdkRequest(url, init?.method, init?.headers ?? {}, init?.body));
        return new Response(JSON.stringify(response.body ?? null), { status: response.status, headers: { 'Content-Type': 'application/json' } });
      } catch {
        // auth-js logs fetch exceptions unconditionally. This exception carries
        // no original cause, account, URL, token or provider response.
        throw new Error('Supabase owned transport failed');
      }
    },
  }) } };
}
