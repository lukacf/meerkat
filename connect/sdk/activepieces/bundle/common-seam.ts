// The generated factory supplies this lexical binding separately for each call.
declare const __toolkitTransport: (request: unknown) => Promise<{ status: number; headers: Record<string, string>; body: unknown }>;
export { AuthenticationType } from '../vendor/activepieces/packages/pieces/common/src/lib/authentication/index';
export { HttpMethod } from '../vendor/activepieces/packages/pieces/common/src/lib/http/core/http-method';
import { HttpError } from '../vendor/activepieces/packages/pieces/common/src/lib/http/core/http-error';
export { HttpError };
export { getAccessTokenOrThrow } from '../vendor/activepieces/packages/pieces/common/src/lib/helpers/index';
export const httpClient = Object.freeze({
  async sendRequest(request: unknown) {
    const response = await __toolkitTransport(request);
    if (response.status < 200 || response.status >= 300) {
      throw new HttpError(undefined, { status: response.status, responseBody: undefined });
    }
    return response;
  },
});

export { propsValidation } from '../vendor/activepieces/packages/pieces/common/src/lib/validation/index';
