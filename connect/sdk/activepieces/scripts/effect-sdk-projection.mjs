import fs from 'node:fs/promises';
import path from 'node:path';
import {createHash} from 'node:crypto';
/** The selected release embeds its fetch binding; keep the exact SDK except that binding. */
export async function effectSdkProjection(root){
 const file=path.join(root,'node_modules/@linear/sdk/dist/index-es.js');
 const source=await fs.readFile(file,'utf8');
 const original="var isomorphicUnfetch = commonjsGlobal.fetch = commonjsGlobal.fetch || (\n\ttypeof process=='undefined' ? r(require$$0) : (function(url, opts) {\n\t\treturn r(require$$1)(String(url).replace(/^\\/\\//g,'https://'), opts);\n\t})\n);";
 const replacement='var isomorphicUnfetch = __toolkitLinearFetch;';
 if(source.split(original).length!==2)throw new Error('Linear binding changed');
 const contents=source.replace(original,replacement),sha=v=>createHash('sha256').update(v).digest('hex');
 const retained='dist/EFFECT-SDK-PORTS';
 await fs.rm(path.join(root,retained),{recursive:true,force:true});
 const keep=async(file,bytes)=>{await fs.mkdir(path.dirname(path.join(root,file)),{recursive:true});await fs.writeFile(path.join(root,file),bytes);return {path:file,sha256:sha(bytes)};};
 const sourceMap=await fs.readFile(file+'.map');
 const originalFile=await keep(retained+'/original/npm/@linear/sdk/dist/index-es.js',source);
 const projectedFile=await keep(retained+'/replacement/npm/@linear/sdk/dist/index-es.js',contents);
 const mapFile=await keep(retained+'/original/npm/@linear/sdk/dist/index-es.js.map',sourceMap);
 await fs.writeFile(path.join(root,'dist/LINEAR-SDK-PROJECTION.json'),JSON.stringify({source:'node_modules/@linear/sdk/dist/index-es.js',source_sha256:sha(source),original,replacement,projected_sha256:sha(contents),retained_original:originalFile,retained_projected:projectedFile,retained_source_map:mapFile},null,2)+'\n');
 const records=[];
 for(const [purpose,sourceFile,portFile]of [
  ['Slack real WebClient per-instance adapter and retry configuration','node_modules/@slack/web-api/dist/WebClient.js','bundle/slack-seam.ts'],
  ['Datadog explicit Configuration and owned HTTP API','vendor/activepieces/packages/pieces/community/datadog/src/lib/common/helpers.ts','bundle/datadog-config-port.ts'],
 ]){
  const sourceBytes=await fs.readFile(path.join(root,sourceFile)),portBytes=await fs.readFile(path.join(root,portFile));
  records.push({purpose,original:sourceFile,replacement:portFile,retained_original:await keep(retained+'/original/'+sourceFile.replace(/^node_modules\//,'npm/'),sourceBytes),retained_replacement:await keep(retained+'/replacement/'+portFile,portBytes)});
 }
 await fs.writeFile(path.join(root,'dist/EFFECT-SDK-PORTS.json'),JSON.stringify({schema_version:1,scope:'Exact source and documented import/composition ports only. Retained source is inert evidence, never an executable fallback. Linear projection has its own exact manifest.',records},null,2)+'\n');
 const embedded=JSON.parse(await fs.readFile(path.join(root,'licenses/linear-embedded/MANIFEST.json'),'utf8'));
 const mapped=JSON.parse(sourceMap.toString());
 for(const record of embedded.packages){
  if(!record.integrity_in_tag_lock||!record.source_bindings.length)throw new Error('Embedded SDK dependency proof missing');
  for(const binding of record.source_bindings){
   const index=mapped.sources.indexOf(binding.source_map_path);
   if(index<0||!binding.exact_tarball_match||binding.source_sha256!==binding.tarball_sha256||sha(mapped.sourcesContent[index])!==binding.source_sha256)throw new Error('Embedded SDK source binding changed');
  }
  for(const item of record.notices)if(sha(await fs.readFile(path.join(root,item.path)))!==item.sha256)throw new Error('Embedded SDK notice changed');
 }
 for(const item of embedded.tag_source)if(sha(await fs.readFile(path.join(root,item.path)))!==item.sha256)throw new Error('Embedded SDK tag source changed');
 const references=mapped.sources.filter(item=>item.includes('/node_modules/'));
 if(references.length!==embedded.packages.reduce((n,item)=>n+item.source_bindings.length,0))throw new Error('Embedded SDK source inventory changed');
 return {name:'owned-linear-fetch',setup(build){build.onLoad({filter:/\/node_modules\/@linear\/sdk\/dist\/index-es\.js$/},()=>({contents,loader:'js',resolveDir:path.dirname(file)}));}};
}
export const effectFactoryPrelude=`
const __toolkitLinearFetch = async (url, init) => {
 if (typeof url !== 'string' || !init || init.method !== 'POST') throw new Error('Unsupported Linear request');
 const response = await __toolkitTransport({ method: init.method, url, headers: Object.fromEntries(new Headers(init.headers)), body: init.body });
 return new Response(JSON.stringify(response.body), { status: response.status, headers: { 'content-type': 'application/json', ...response.headers } });
};
`;
