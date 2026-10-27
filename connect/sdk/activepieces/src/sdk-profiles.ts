import { createHash, createHmac, timingSafeEqual } from 'node:crypto';
import { ActivepiecesError, type FailureKind } from './errors.js';
import { canonicalOrigin, configurationKeys as keys, selectedSecret as secret, record, type Selection } from './profiles.js';
import { MONDAY_QUERY } from './sdk-contracts.js';
export const SDK_PROFILE_IDS = ['ghost.get-post', 'monday.get-items', 'mailchimp.get-campaign-report', 'amazon-s3.list-files'] as const;
export type SdkProfile = typeof SDK_PROFILE_IDS[number];
export type SdkConfiguration = {
    readonly profile: 'ghost.get-post';
    readonly auth: {
        readonly origin: string;
        readonly adminApiKey: string;
    };
} | {
    readonly profile: 'monday.get-items';
    readonly auth: {
        readonly apiToken: string;
    };
} | {
    readonly profile: 'mailchimp.get-campaign-report';
    readonly auth: {
        readonly accessToken: string;
        readonly dataCenter: string;
    };
} | {
    readonly profile: 'amazon-s3.list-files';
    readonly auth: {
        readonly accessKeyId: string;
        readonly secretAccessKey: string;
        readonly sessionToken?: string;
        readonly region: string;
        readonly bucket: string;
    };
};
export interface SdkPlan {
    kind: 'sdk';
    profile: SdkProfile;
    expected: Record<string, unknown>;
}
const includes = (profile: string): profile is SdkProfile => (SDK_PROFILE_IDS as readonly string[]).includes(profile);
const end = '(?![\\s\\S])';
const full = (value: unknown, pattern: string): value is string => typeof value === 'string' && new RegExp(pattern + end).test(value);
const scalarPattern = `^(?:[^\\u0000-\\u001f\\u007f\\ud800-\\udfff]|[\\ud800-\\udbff][\\udc00-\\udfff])*${end}`;
const scalar = (value: unknown, max: number): value is string => typeof value === 'string' && [...value].length <= max && new RegExp(scalarPattern).test(value);
const ghostKey = (value: unknown): value is string => full(value, '^[0-9a-fA-F]{24}:[0-9a-fA-F]{64}');
export { ghostKey };
export function selectSdk(profile: string, auth: Record<string, unknown>): Pick<Selection, 'origin' | 'auth' | 'secret'> | undefined {
    if (!includes(profile))
        return undefined;
    if (profile === 'ghost.get-post' && keys(auth, ['origin', 'adminApiKey']) && ghostKey(auth.adminApiKey)) {
        const origin = canonicalOrigin(auth.origin);
        return { origin, secret: auth.adminApiKey, auth: { type: 'CUSTOM_AUTH', props: { baseUrl: origin, apiKey: auth.adminApiKey } } };
    }
    if (profile === 'monday.get-items' && keys(auth, ['apiToken']) && secret(auth.apiToken))
        return { origin: 'https://api.monday.com', secret: auth.apiToken, auth: { type: 'SECRET_TEXT', secret_text: auth.apiToken } };
    if (profile === 'mailchimp.get-campaign-report' && keys(auth, ['accessToken', 'dataCenter']) && secret(auth.accessToken) && full(auth.dataCenter, '^us[1-9][0-9]{0,2}'))
        return {
            origin: `https://${auth.dataCenter}.api.mailchimp.com`, secret: auth.accessToken, auth: { type: 'OAUTH2', access_token: auth.accessToken, props: { dataCenter: auth.dataCenter } },
        };
    if (profile === 'amazon-s3.list-files' && keys(auth, ['accessKeyId', 'secretAccessKey', 'sessionToken', 'region', 'bucket'], ['accessKeyId', 'secretAccessKey', 'region', 'bucket']) &&
        full(auth.accessKeyId, '^[A-Z0-9]{16,128}') && secret(auth.secretAccessKey) && (auth.sessionToken === undefined || secret(auth.sessionToken)) &&
        full(auth.region, '^[a-z]{2}(?:-[a-z]+)+-[0-9]') && full(auth.bucket, '^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]') && !auth.bucket.includes('..') && !/^\d+\.\d+\.\d+\.\d+$/.test(auth.bucket)) {
        const origin = `https://s3.${auth.region}.amazonaws.com${auth.region.startsWith('cn-') ? '.cn' : ''}`;
        return { origin, secret: auth.secretAccessKey, auth: { type: 'CUSTOM_AUTH', props: { ...auth, endpoint: origin } } };
    }
    throw 0;
}
export function describeSdk(profile: string): {
    action: string;
    inputSchema: Record<string, unknown>;
} | undefined {
    if (!includes(profile))
        return undefined;
    const descriptions = {
        'ghost.get-post': { action: 'ghost_get_post', properties: { post_id: { type: 'string', pattern: `^[0-9a-fA-F]{24}${end}` } }, required: ['post_id'] },
        'monday.get-items': { action: 'monday_get_items', properties: { item_ids: { type: 'array', minItems: 1, maxItems: 100, uniqueItems: true, items: { type: 'string', pattern: `^[1-9][0-9]{0,63}${end}` } } }, required: ['item_ids'] },
        'mailchimp.get-campaign-report': { action: 'get_campaign_report', properties: { campaign_id: { type: 'string', pattern: `^[A-Za-z0-9_-]{1,128}${end}` } }, required: ['campaign_id'] },
        'amazon-s3.list-files': { action: 'list-files', properties: { prefix: { anyOf: [{ type: 'string', pattern: scalarPattern, maxLength: 1024, description: 'Literal prefix, at most 1,024 UTF-8 bytes; empty/absent/null selects the bucket root.' }, { type: 'null' }] }, maxKeys: { anyOf: [{ type: 'integer', minimum: 1, maximum: 1000 }, { type: 'null' }] } }, required: [] },
    };
    const d = descriptions[profile];
    return { action: d.action, inputSchema: { type: 'object', properties: d.properties, required: d.required, additionalProperties: false } };
}
export function sdkInputs(selection: Selection, value: Record<string, unknown>): {
    props: Record<string, unknown>;
    pathname: string;
    query?: Record<string, string>;
    read: SdkPlan;
} | undefined {
    const profile = selection.profile;
    if (!includes(profile))
        return undefined;
    const done = (props: Record<string, unknown>, pathname: string, expected: Record<string, unknown>, query?: Record<string, string>) => ({ props, pathname, query, read: { kind: 'sdk' as const, profile, expected } });
    if (profile === 'ghost.get-post' && keys(value, ['post_id']) && full(value.post_id, '^[0-9a-fA-F]{24}'))
        return done({ ...value }, '/ghost/api/admin/posts/' + value.post_id, { id: value.post_id }, { formats: 'html', include: 'tags,authors' });
    if (profile === 'monday.get-items' && keys(value, ['item_ids']) && Array.isArray(value.item_ids) && value.item_ids.length >= 1 && value.item_ids.length <= 100 && new Set(value.item_ids).size === value.item_ids.length && value.item_ids.every(id => full(id, '^[1-9][0-9]{0,63}')))
        return done({ item_ids: [...value.item_ids] }, '/v2', { ids: [...value.item_ids] });
    if (profile === 'mailchimp.get-campaign-report' && keys(value, ['campaign_id']) && full(value.campaign_id, '^[A-Za-z0-9_-]{1,128}'))
        return done({ ...value }, '/3.0/reports/' + value.campaign_id, { id: value.campaign_id });
    if (profile === 'amazon-s3.list-files' && keys(value, ['prefix', 'maxKeys'], [])) {
        const prefix = value.prefix ?? '', maxKeys = value.maxKeys ?? 100;
        if (scalar(prefix, 1024) && Buffer.byteLength(prefix) <= 1024 && Number.isSafeInteger(maxKeys) && (maxKeys as number) >= 1 && (maxKeys as number) <= 1000) {
            const props = (selection.auth as {
                props: Record<string, unknown>;
            }).props;
            return done({ prefix, maxKeys }, '/' + props.bucket + '/', { prefix, maxKeys }, { 'list-type': '2', 'max-keys': String(maxKeys), ...(prefix ? { prefix } : {}) });
        }
    }
    throw new ActivepiecesError('invalid_arguments');
}
const stringify = (value: unknown) => JSON.stringify(value);
const encode = (value: string) => encodeURIComponent(value).replace(/[!'()*]/g, c => '%' + c.charCodeAt(0).toString(16).toUpperCase());
const hash = (value: string) => createHash('sha256').update(value).digest('hex');
const hmac = (key: string | Buffer, value: string) => createHmac('sha256', key).update(value).digest();
const equal = (a: Buffer, b: Buffer) => a.length === b.length && timingSafeEqual(a, b);
const get = (value: any, path: string): any => path.split('.').reduce((v, key) => v?.[key], value);
const set = (value: any, path: string, item: unknown) => { const parts = path.split('.'), last = parts.pop()!; const target = parts.reduce((v, key) => v?.[key], value); if (target && typeof target === 'object')
    target[last] = item; };
const METRICS: Record<string, string> = {
    'summary.unique_opens': 'opens.unique_opens', 'summary.unique_clicks': 'clicks.unique_clicks', 'summary.open_rate': 'opens.open_rate', 'summary.click_rate': 'clicks.click_rate',
    'performance_metrics.opens.total': 'opens.opens_total', 'performance_metrics.opens.unique': 'opens.unique_opens', 'performance_metrics.opens.rate': 'opens.open_rate',
    'performance_metrics.opens.proxy_excluded.total': 'opens.proxy_excluded_opens', 'performance_metrics.opens.proxy_excluded.unique': 'opens.proxy_excluded_unique_opens', 'performance_metrics.opens.proxy_excluded.rate': 'opens.proxy_excluded_open_rate',
    'performance_metrics.clicks.total': 'clicks.clicks_total', 'performance_metrics.clicks.unique': 'clicks.unique_clicks', 'performance_metrics.clicks.unique_subscribers': 'clicks.unique_subscriber_clicks', 'performance_metrics.clicks.rate': 'clicks.click_rate',
    'performance_metrics.bounces.hard': 'bounces.hard_bounces', 'performance_metrics.bounces.soft': 'bounces.soft_bounces', 'performance_metrics.bounces.syntax_errors': 'bounces.syntax_errors',
    'performance_metrics.engagement.forwards.count': 'forwards.forwards_count', 'performance_metrics.engagement.forwards.opens': 'forwards.forwards_opens',
    'performance_metrics.engagement.facebook_likes.recipient_likes': 'facebook_likes.recipient_likes', 'performance_metrics.engagement.facebook_likes.unique_likes': 'facebook_likes.unique_likes', 'performance_metrics.engagement.facebook_likes.facebook_likes': 'facebook_likes.facebook_likes',
};
for (const group of ['a', 'b'])
    for (const metric of ['bounces', 'abuse_reports', 'unsubs', 'recipient_clicks', 'forwards', 'forwards_opens', 'opens', 'unique_opens'])
        METRICS[`ab_split_data.group_${group}.${metric}`] = `ab_split.${group}.${metric}`;
// Preserve SDK Date values in JSON and omit original undefined optional fields.
function clean(value: any): any {
    if (value instanceof Date) {
        if (!Number.isFinite(value.getTime()))
            throw 0;
        return value.toISOString();
    }
    if (Array.isArray(value))
        return value.map(clean);
    if (record(value))
        return Object.fromEntries(Object.entries(value).filter(([, v]) => v !== undefined).map(([k, v]) => [k, clean(v)]));
    return value;
}
// This state validates one selected read. Invocation still owns all deadlines,
// HTTP promises and response retirement. No SDK-specific lifecycle is added.
export class SdkRequestState {
    private requested = false;
    private pageToken: string | null | undefined;
    private report: Record<string, unknown> | undefined;
    readonly responseType: 'xml' | 'json';
    constructor(private readonly s: Selection, private readonly path: string, private readonly query: Readonly<Record<string, string>> | undefined, private readonly plan: SdkPlan) {
        if (plan.profile !== s.profile)
            throw new ActivepiecesError('invalid_arguments');
        this.responseType = plan.profile === 'amazon-s3.list-files' ? 'xml' : 'json';
    }
    prepare(value: unknown): {
        url: string;
        headers: Headers;
        method: 'GET' | 'POST';
        body?: string;
    } {
        const invalid = (): never => { throw new ActivepiecesError('upstream_error'); };
        const p = this.plan.profile, method = p === 'monday.get-items' ? 'POST' : 'GET', props = (this.s.auth as {
            props?: Record<string, unknown>;
        }).props ?? {};
        if (this.requested || !record(value) || !keys(value, ['method', 'url', 'headers', 'body', 'queryParams'], ['method', 'url', 'headers']) || value.method !== method || value.url !== this.s.origin + this.path || !record(value.headers))
            return invalid();
        const headers = new Headers(value.headers as Record<string, string>), url = new URL(this.s.origin + this.path);
        if (url.origin !== this.s.origin || url.pathname !== this.path || url.search || url.hash || url.username || url.password)
            return invalid();
        if (p === 'monday.get-items') {
            if (value.queryParams !== undefined || stringify(value.body) !== stringify({ query: MONDAY_QUERY, variables: { ids: this.plan.expected.ids } }) ||
                stringify(value.headers) !== stringify({ Authorization: this.s.secret, 'Content-Type': 'application/json', 'API-Version': '2026-07' }))
                return invalid();
        }
        else {
            if (value.body !== undefined)
                return invalid();
            if (p === 'ghost.get-post') {
                if (Object.keys(value.headers).length !== 1 || typeof value.headers.Authorization !== 'string' || !value.headers.Authorization.startsWith('Ghost ') || stringify(value.queryParams) !== stringify(this.query))
                    return invalid();
                const jwt = value.headers.Authorization.slice(6).split('.');
                if (jwt.length !== 3)
                    return invalid();
                try {
                    const head = JSON.parse(Buffer.from(jwt[0]!, 'base64url').toString()), claims = JSON.parse(Buffer.from(jwt[1]!, 'base64url').toString()), [kid, key] = this.s.secret.split(':');
                    if (head.alg !== 'HS256' || head.typ !== 'JWT' || head.kid !== kid || claims.aud !== '/admin/' || !Number.isSafeInteger(claims.iat) || claims.exp - claims.iat !== 300 || Math.abs(Date.now() / 1000 - claims.iat) > 65 ||
                        !equal(hmac(Buffer.from(key!, 'hex'), jwt[0] + '.' + jwt[1]), Buffer.from(jwt[2]!, 'base64url')))
                        return invalid();
                }
                catch {
                    return invalid();
                }
            }
            else if (p === 'mailchimp.get-campaign-report') {
                if (stringify(value.queryParams) !== '{}' || stringify(value.headers) !== stringify({ Authorization: `Bearer ${this.s.secret}`, 'Content-Type': 'application/json', Accept: 'application/json' }))
                    return invalid();
            }
            else {
                if (!record(value.queryParams) || stringify(Object.fromEntries(Object.entries(value.queryParams).sort())) !== stringify(Object.fromEntries(Object.entries(this.query ?? {}).sort())))
                    return invalid();
                const allowed = ['authorization', 'host', 'x-amz-date', 'x-amz-content-sha256', 'x-amz-security-token', 'amz-sdk-invocation-id', 'amz-sdk-request', 'user-agent', 'x-amz-user-agent'];
                if (Object.keys(value.headers).some(k => !allowed.includes(k)) || headers.get('host') !== url.host || headers.get('x-amz-content-sha256') !== hash('') || headers.get('x-amz-security-token') !== (props.sessionToken ?? null))
                    return invalid();
                const authorization = headers.get('authorization') ?? '', match = /^AWS4-HMAC-SHA256 Credential=([^/]+)\/([0-9]{8})\/([^/]+)\/s3\/aws4_request, SignedHeaders=([a-z0-9;-]+), Signature=([a-f0-9]{64})$/.exec(authorization);
                if (!match || match[1] !== props.accessKeyId || match[3] !== props.region)
                    return invalid();
                const names = match[4]!.split(';'), date = headers.get('x-amz-date') ?? '';
                if (names.join(';') !== [...new Set(names)].sort().join(';') || ['host', 'x-amz-date', 'x-amz-content-sha256', ...(props.sessionToken ? ['x-amz-security-token'] : [])].some(k => !names.includes(k)) || names.some(k => !allowed.includes(k) || !headers.has(k)) || !new RegExp('^' + match[2] + 'T[0-9]{6}Z$').test(date))
                    return invalid();
                const canonicalQuery = Object.entries(this.query!).map(([k, v]) => [encode(k), encode(v)]).sort(([a], [b]) => a! < b! ? -1 : a! > b! ? 1 : 0).map(([k, v]) => k + '=' + v).join('&');
                const canonicalHeaders = names.map(k => k + ':' + headers.get(k)!.trim().replace(/\s+/g, ' ') + '\n').join('');
                const request = [method, this.path, canonicalQuery, canonicalHeaders, match[4], hash('')].join('\n'), scope = match[2] + '/' + props.region + '/s3/aws4_request';
                const signing = hmac(hmac(hmac(hmac('AWS4' + this.s.secret, match[2]!), props.region as string), 's3'), 'aws4_request');
                if (!equal(hmac(signing, 'AWS4-HMAC-SHA256\n' + date + '\n' + scope + '\n' + hash(request)), Buffer.from(match[5]!, 'hex')))
                    return invalid();
                this.requested = true;
                return { url: this.s.origin + this.path + '?' + canonicalQuery, headers, method };
            }
        }
        for (const [key, item] of Object.entries(this.query ?? {}))
            url.searchParams.set(key, item);
        headers.set('Accept', 'application/json');
        this.requested = true;
        return { url: url.toString(), headers, method, ...(method === 'POST' ? { body: stringify(value.body) } : {}) };
    }
    bodyFailure(body: unknown): FailureKind | undefined {
        const p = this.plan.profile, bad = 'invalid_response';
        if (p === 'amazon-s3.list-files')
            return typeof body === 'string' && !/<!\s*(?:DOCTYPE|ENTITY)\b/i.test(body) ? undefined : bad;
        if (!record(body))
            return bad;
        if (p === 'ghost.get-post')
            return Array.isArray(body.posts) && body.posts.length === 1 && record(body.posts[0]) && body.posts[0].id === this.plan.expected.id ? undefined : bad;
        if (p === 'monday.get-items') {
            if ((Array.isArray(body.errors) && body.errors.length > 0) || body.error_message != null)
                return undefined; // Real donor query helper extracts these errors.
            if (body.errors !== undefined && !Array.isArray(body.errors) || !record(body.data) || !Array.isArray(body.data.items) || body.data.items.length > (this.plan.expected.ids as string[]).length)
                return bad;
            const seen = new Set();
            for (const item of body.data.items) {
                if (!record(item) || typeof item.id !== 'string' || !(this.plan.expected.ids as string[]).includes(item.id) || seen.has(item.id) || !['name', 'state', 'url', 'created_at', 'updated_at'].every(k => typeof item[k] === 'string') || !Array.isArray(item.column_values) || item.column_values.length > 1000)
                    return bad;
                seen.add(item.id);
                for (const cv of item.column_values)
                    if (!record(cv) || typeof cv.id !== 'string' || typeof cv.type !== 'string' || (cv.text != null && typeof cv.text !== 'string') || (cv.value != null && typeof cv.value !== 'string'))
                        return bad;
                for (const [key, names] of [['board', ['id', 'name']], ['group', ['id', 'title']], ['parent_item', ['id']]] as const)
                    if (item[key] != null && (!record(item[key]) || names.some(k => typeof (item[key] as Record<string, unknown>)[k] !== 'string')))
                        return bad;
            }
            return undefined;
        }
        if (body.id !== this.plan.expected.id || !Number.isSafeInteger(body.emails_sent) || (body.emails_sent as number) < 0)
            return bad;
        for (const path of [...Object.values(METRICS), 'abuse_reports', 'unsubscribed']) {
            const value = get(body, path);
            if (value != null && (typeof value !== 'number' || !Number.isFinite(value) || value < 0))
                return bad;
        }
        for (const group of ['opens', 'clicks', 'bounces', 'forwards', 'facebook_likes', 'ab_split', 'industry_stats', 'list_stats', 'ecommerce', 'delivery_status', 'share_report'])
            if (body[group] != null && !record(body[group]))
                return bad;
        if (record(body.ab_split))
            for (const group of ['a', 'b'])
                if (body.ab_split[group] != null && !record(body.ab_split[group]))
                    return bad;
        this.report = body;
        return undefined;
    }
    s3Page(token: string | null): void {
        if (this.plan.profile !== 'amazon-s3.list-files' || this.pageToken !== undefined || (token !== null && (!scalar(token, 8192) || token.length === 0)))
            throw new ActivepiecesError('invalid_response');
        this.pageToken = token;
    }
    projectResult(value: unknown): unknown {
        if (this.plan.profile === 'amazon-s3.list-files') {
            if (this.pageToken === undefined || !record(value) || value.isTruncated !== (this.pageToken !== null))
                throw 0;
            return { ...clean(value), nextContinuationToken: this.pageToken };
        }
        if (this.plan.profile !== 'mailchimp.get-campaign-report')
            return value;
        const body = this.report;
        if (!body || !record(value) || value.success !== true)
            throw 0;
        const result = clean(value);
        // The action defaults missing metrics to zero; the port represents unknown
        // values explicitly while preserving every supplied numeric zero.
        for (const [target, source] of Object.entries(METRICS))
            if (get(body, source) == null)
                set(result, target, null);
        const sent = body.emails_sent as number;
        for (const [target, sources] of [['bounce_rate', ['bounces.hard_bounces', 'bounces.soft_bounces']], ['unsub_rate', ['unsubscribed']], ['abuse_rate', ['abuse_reports']]] as const)
            if (sent === 0 || sources.some(path => get(body, path) == null))
                set(result, 'summary.' + target, null);
        if (['hard_bounces', 'soft_bounces', 'syntax_errors'].some(k => get(body, 'bounces.' + k) == null))
            set(result, 'performance_metrics.bounces.total', null);
        if (body.timewarp == null)
            result.timewarp_data = null;
        if (body.timeseries == null)
            result.timeseries_data = null;
        return result;
    }
}
