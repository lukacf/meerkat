import { drive as upstreamDrive } from '../node_modules/@googleapis/drive/build/index.js';
import { ownedGoogleOptions } from './google-client-seam';
export const drive = (options: Record<string, any>) => upstreamDrive(ownedGoogleOptions(options) as never);
