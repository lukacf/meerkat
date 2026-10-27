import { docs as upstreamDocs } from '../node_modules/@googleapis/docs/build/index.js';
import { ownedGoogleOptions } from './google-client-seam';
export const docs = (options: Record<string, any>) => upstreamDocs(ownedGoogleOptions(options) as never);
