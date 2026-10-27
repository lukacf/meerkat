import { sheets as upstreamSheets } from '../node_modules/@googleapis/sheets/build/index.js';
import { ownedGoogleOptions } from './google-client-seam';
export const sheets = (options: Record<string, any>) => upstreamSheets(ownedGoogleOptions(options) as never);
