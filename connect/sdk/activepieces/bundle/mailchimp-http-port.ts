declare const __toolkitTransport: (request: unknown) => Promise<{
    status: number;
    headers: Record<string, string>;
    body: unknown;
}>;
const refused = (): never => { throw new Error('Unsupported Mailchimp transport operation'); };
// Selected GET chain only. Real ApiClient/ReportsApi still marshal and deserialize.
function transport(method: string, url: string) {
    const headers: Record<string, string> = {};
    let query: Record<string, string> = {}, ended = false;
    const request = { header: headers, auth: refused, send: refused, attach: refused, field: refused, withCredentials: refused, responseType: refused,
        set(values: Record<string, string>) { Object.assign(headers, values); return this; },
        query(values: Record<string, string>) { query = values; return this; },
        timeout(_value: number) { return this; }, type(value: string) { headers['Content-Type'] = value; return this; }, accept(value: string) { headers.Accept = value; return this; },
        end(callback: (error: Error | null, response?: unknown) => void) {
            if (ended)
                return refused();
            ended = true;
            void Promise.resolve().then(() => __toolkitTransport({ method, url, headers, queryParams: query })).then(response => {
                if (response.status < 200 || response.status >= 300)
                    throw new Error('Mailchimp HTTP request failed');
                callback(null, { status: response.status, body: response.body, text: JSON.stringify(response.body) });
            }).catch(() => callback(new Error('Mailchimp owned request failed')));
        },
    };
    return request;
}
transport.agent = class {
    attachCookies = refused;
    saveCookies = refused;
};
export = transport;
