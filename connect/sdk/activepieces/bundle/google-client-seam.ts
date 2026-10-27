// The unchanged donor creates a real OAuth2Client. Configure only that instance's
// supported Gaxios adapter; real Google resource methods and auth headers remain.
declare const __toolkitTransport: (request: unknown) => Promise<{ status: number; body: unknown }>;
export function ownedGoogleOptions(options: Record<string, any>) {
  const auth = options.auth;
  if (!auth || !auth.transporter || !auth.credentials || typeof auth.credentials.access_token !== 'string' ||
    Object.keys(auth.credentials).some(key => !['type', 'access_token'].includes(key))) throw new Error('Unsupported Google auth');
  auth.transporter.defaults = {
    ...auth.transporter.defaults,
    retry: false,
    // A supplied agent prevents Gaxios from constructing an ambient proxy agent.
    // The owned adapter never invokes this function or a default HTTP adapter.
    agent: () => undefined,
    adapter: async (request: any) => {
      if (request.data !== undefined || request.body !== undefined || request.timeout !== undefined || request.signal !== undefined) throw new Error('Unsupported Google request');
      const url = new URL(request.url);
      const queryParams = Object.fromEntries(url.searchParams);
      if (Object.keys(queryParams).length !== [...url.searchParams].length) throw new Error('Duplicate Google query');
      const response = await __toolkitTransport({ method: request.method, url: url.origin + url.pathname,
        headers: Object.fromEntries(new Headers(request.headers)), ...(url.search ? { queryParams } : {}) });
      return Object.assign(new Response(JSON.stringify(response.body ?? null), { status: response.status,
        headers: { 'Content-Type': 'application/json' } }), { data: response.body, config: request });
    },
  };
  return { ...options, universeDomain: 'googleapis.com', retry: false };
}
