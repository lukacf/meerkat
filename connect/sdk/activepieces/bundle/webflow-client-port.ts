// Narrow documented v2 port. The actual donor action and inherited HTTP helper
// remain in use; only the selected get-item resource prefix changes.
import { WebflowApiClient as UpstreamClient } from '../vendor/activepieces/packages/pieces/community/webflow/src/lib/common/client';
import { HttpMethod } from '../vendor/activepieces/packages/pieces/common/src/lib/http/core/http-method';
export class WebflowApiClient extends UpstreamClient {
  override async getCollectionItem(collectionId: string, itemId: string) {
    return await this.makeRequest(HttpMethod.GET, `/v2/collections/${collectionId}/items/${itemId}`);
  }
}
