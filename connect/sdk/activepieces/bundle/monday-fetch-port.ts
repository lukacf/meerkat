declare const __toolkitTransport: (request: unknown) => Promise<{
    status: number;
    headers: Record<string, string>;
    body: unknown;
}>;
// Import-scoped replacement used only by the real monday SDK server transport.
async function ownedFetch(url: string, init: {
    method: string;
    headers: Record<string, string>;
    body: string;
}) {
    const response = await __toolkitTransport({ url, method: init.method, headers: init.headers, body: JSON.parse(init.body) });
    if (response.status < 200 || response.status >= 300)
        throw new Error('monday HTTP request failed');
    return new Response(JSON.stringify(response.body), { status: response.status, headers: { 'content-type': 'application/json' } });
}
export = ownedFetch;
