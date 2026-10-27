import fs from 'node:fs/promises';
import path from 'node:path';
import {createHash} from 'node:crypto';
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');

// These are import/composition ports, not modified donor action bodies. Retain
// the exact replacement source and original binding input with the artifact.
export async function sdkPortManifest(root){
 const plans=[
  ['Monday Node entry','node_modules/monday-sdk-js/src/index.js','node_modules/monday-sdk-js/server-sdk.js'],
  ['Monday fetch','node_modules/monday-sdk-js/src/monday-api-client/fetch.js','bundle/monday-fetch-port.ts'],
  ['Monday warning projection','node_modules/monday-sdk-js/src/helpers/monday-api-helpers.js','bundle/monday-warnings-port.ts'],
  ['Mailchimp instance composition','node_modules/@mailchimp/mailchimp_marketing/src/index.js','bundle/mailchimp-sdk-port.ts'],
  ['Mailchimp HTTP callback transport','node_modules/@mailchimp/mailchimp_marketing/src/ApiClient.js','bundle/mailchimp-http-port.ts'],
  ['Mailchimp selected data center','vendor/activepieces/packages/pieces/community/mailchimp/src/lib/common/index.ts','bundle/mailchimp-common-port.ts'],
  ['S3 selected client composition','vendor/activepieces/packages/pieces/community/amazon-s3/src/lib/common.ts','bundle/s3-client-port.ts'],
 ];
 // Neutral retained paths avoid dependency-directory filtering ambiguity.
 // Keep source references exact and clear stale derived output.
 const retainedPath=value=>value.replace(/^node_modules\//,'npm/');
 await fs.rm(path.join(root,'dist/SDK-PORTS'),{recursive:true,force:true});
 const records=[];
 for(const [purpose,original,replacement]of plans){
  const source=await fs.readFile(path.join(root,original)),port=await fs.readFile(path.join(root,replacement));
  const retainedOriginal='dist/SDK-PORTS/original/'+retainedPath(original),retainedReplacement='dist/SDK-PORTS/replacement/'+retainedPath(replacement);
  for(const [file,bytes]of [[retainedOriginal,source],[retainedReplacement,port]]){await fs.mkdir(path.dirname(path.join(root,file)),{recursive:true});await fs.writeFile(path.join(root,file),bytes)}
  records.push({purpose,original,original_sha256:sha(source),replacement,replacement_sha256:sha(port),retained_original:retainedOriginal,retained_replacement:retainedReplacement});
 }
 await fs.writeFile(path.join(root,'dist/SDK-PORTS.json'),JSON.stringify({schema_version:1,scope:'Four-service SDK expansion only. Binding sites are in scripts/build.mjs and source-projections.mjs. ApiClient uses an explicit Superagent-compatible I/O port; actual ReportsApi and ApiClient remain executed. Retained source is inert documentation.',records},null,2)+'\n');
}
