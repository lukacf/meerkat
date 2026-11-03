import { S3 } from '@aws-sdk/client-s3';
declare const __toolkitTransport: (request: unknown) => Promise<{
    status: number;
    headers: Record<string, string>;
    body: unknown;
}>;
declare const __toolkitContext: {
    invalidResponse: () => never;
    s3Page: (token: string | null) => void;
};
const invalid = (): never => __toolkitContext.invalidResponse();
const text = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && !/[\x00-\x1f\x7f]/.test(v) && !/[\ud800-\udfff]/u.test(v);
const record = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === 'object' && !Array.isArray(v);
export function createS3(auth: any) {
    class SelectedS3 extends S3 {
        override async listObjectsV2(params: any): Promise<any> {
            try {
                const response = await super.listObjectsV2(params);
                if (response.Name !== auth.bucket || typeof response.IsTruncated !== 'boolean' || !Number.isSafeInteger(response.KeyCount) || response.KeyCount! < 0 || response.KeyCount! > params.MaxKeys)
                    return invalid();
                if (response.Contents !== undefined && !Array.isArray(response.Contents))
                    return invalid();
                const contents = response.Contents ?? [];
                if (contents.length !== response.KeyCount)
                    return invalid();
                if (response.IsTruncated ? !text(response.NextContinuationToken) : response.NextContinuationToken !== undefined)
                    return invalid();
                const seen = new Set<string>();
                for (const object of contents) {
                    if (!record(object) || !text(object.Key) || Buffer.byteLength(object.Key) > 1024 || !object.Key.startsWith(params.Prefix ?? '') ||
                        !Number.isSafeInteger(object.Size) || (object.Size as number) < 0 || !(object.LastModified instanceof Date) || !Number.isFinite(object.LastModified.getTime()) || !text(object.ETag) ||
                        (object.StorageClass !== undefined && !text(object.StorageClass)) || seen.has(object.Key))
                        return invalid();
                    seen.add(object.Key);
                }
                __toolkitContext.s3Page(response.NextContinuationToken ?? null);
                return response;
            }
            catch {
                return invalid();
            }
            finally {
                this.destroy();
            }
        }
    }
    return new SelectedS3({ region: auth.region, endpoint: auth.endpoint, credentials: { accessKeyId: auth.accessKeyId, secretAccessKey: auth.secretAccessKey, ...(auth.sessionToken ? { sessionToken: auth.sessionToken } : {}) },
        forcePathStyle: true, useArnRegion: false, useFipsEndpoint: false, useDualstackEndpoint: false, followRegionRedirects: false, disableS3ExpressSessionAuth: true,
        // Pin optional SDK configuration too: no shared-file or ambient auth/UA lookup.
        authSchemePreference: ['aws.auth#sigv4'], sigv4aSigningRegionSet: [], disableClockSkewCorrection: true,
        userAgentAppId: 'meerkat-toolkit', defaultUserAgentProvider: async () => [['aws-sdk-js', '3.974.0']],
        defaultsMode: 'standard', retryMode: 'standard', maxAttempts: 1, requestChecksumCalculation: 'WHEN_REQUIRED', responseChecksumValidation: 'WHEN_REQUIRED',
        logger: { info() { }, warn() { }, error() { }, debug() { }, trace() { } },
        requestHandler: { async handle(request: any) {
                const response = await __toolkitTransport({ method: request.method, url: `${request.protocol}//${request.hostname}${request.port ? ':' + request.port : ''}${request.path}`, headers: request.headers, queryParams: request.query });
                if (response.status < 200 || response.status >= 300)
                    throw new Error('S3 owned request failed');
                return { response: { statusCode: response.status, headers: { 'content-type': 'application/xml' }, body: Buffer.from(response.body as string, 'utf8') } };
            }, destroy() { } },
    });
}
export const resolveS3Client = async ({ authProps }: {
    authProps: any;
}) => createS3(authProps);
export const getTemporaryCredentials = (): never => { throw new Error('S3 OIDC acquisition is not exposed'); };
