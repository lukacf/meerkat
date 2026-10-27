import { isDeepStrictEqual } from 'node:util';
import { ActivepiecesError, type FailureKind } from './errors.js';
import { canonicalOrigin, configurationKeys as keys, selectedSecret as token, record, type Selection } from './profiles.js';
import { GET_ISSUE_QUERY, ISSUE_LABELS_PAGE_QUERY } from './effect-queries.js';

export const WRITE_PROFILE_IDS = ['activecampaign.create-contact', 'freshservice.create-ticket', 'gitea.create-issue', 'datadog.submit-log'] as const;
export const EFFECT_READ_PROFILE_IDS = ['gmail.get-thread', 'slack.get-thread-replies', 'linear.get-issue', 'google-cloud-storage.search-objects'] as const;
export const EFFECT_PROFILE_IDS = [...WRITE_PROFILE_IDS, ...EFFECT_READ_PROFILE_IDS] as const;
export type WriteProfile = typeof WRITE_PROFILE_IDS[number];
export type EffectProfile = typeof EFFECT_PROFILE_IDS[number];
export const isWriteProfile = (value: string): value is WriteProfile => (WRITE_PROFILE_IDS as readonly string[]).includes(value);
const isEffectProfile = (value: string): value is EffectProfile => (EFFECT_PROFILE_IDS as readonly string[]).includes(value);
export type WriteConfiguration =
  { readonly profile:'activecampaign.create-contact'; readonly auth:{readonly origin:string; readonly apiKey:string} } |
  { readonly profile:'freshservice.create-ticket'; readonly auth:{readonly domain:string; readonly apiKey:string} } |
  { readonly profile:'gitea.create-issue'; readonly auth:{readonly origin:string; readonly accessToken:string; readonly owner:string; readonly repository:string} } |
  { readonly profile:'datadog.submit-log'; readonly auth:{readonly site:string; readonly apiKey:string} };
export type EffectReadConfiguration =
  { readonly profile:'gmail.get-thread'|'slack.get-thread-replies'; readonly auth:{readonly accessToken:string} } |
  { readonly profile:'linear.get-issue'; readonly auth:{readonly apiKey:string} } |
  { readonly profile:'google-cloud-storage.search-objects'; readonly auth:{readonly accessToken:string; readonly bucket:string} };
const match = (v:unknown,p:RegExp):v is string => typeof v==='string'&&p.exec(v)?.[0]===v;
const text = (v:unknown,max=4096,min=1):v is string => typeof v==='string'&&!/[\uD800-\uDFFF]/u.test(v)&&[...v].length>=min&&[...v].length<=max&&!/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(v);
const segment = (v:unknown):v is string => match(v,/^[A-Za-z0-9][A-Za-z0-9_.-]{0,99}$/)&&v!=='.'&&v!=='..';
const posint = (v:unknown):v is number => Number.isSafeInteger(v)&&(v as number)>0;
const scalar = (v:unknown):boolean => v===null||typeof v==='boolean'||typeof v==='string'||(typeof v==='number'&&Number.isFinite(v));
const safeMap = (v:unknown,max=100):v is Record<string,unknown> => record(v)&&Object.keys(v).length<=max&&Object.entries(v).every(([k,x])=>text(k,100)&&!['__proto__','prototype','constructor'].includes(k)&&scalar(x)&&(typeof x!=='string'||text(x,8192,0)));
const timestamp = (v:unknown):v is string => match(v,/^[0-9]{1,16}\.[0-9]{6}$/);
const cursor = (v:unknown):v is string => text(v,2048)&&!/[\x00-\x20\x7f]/.test(v);
const bucketName = (v:unknown):v is string => match(v,/^[a-z0-9][a-z0-9._-]{1,220}[a-z0-9]$/)&&v.split('.').every(x=>x.length<=63)&&!/^\d+\.\d+\.\d+\.\d+$/.test(v);
export function selectEffect(profile:string,auth:Record<string,unknown>):Pick<Selection,'origin'|'secret'|'auth'>|undefined {
  if(!isEffectProfile(profile))return undefined;
  if(profile==='activecampaign.create-contact'&&keys(auth,['origin','apiKey'])&&token(auth.apiKey))return {origin:canonicalOrigin(auth.origin),secret:auth.apiKey,auth:{type:'CUSTOM_AUTH',props:{apiUrl:canonicalOrigin(auth.origin),apiKey:auth.apiKey}}};
  if(profile==='freshservice.create-ticket'&&keys(auth,['domain','apiKey'])&&token(auth.apiKey)&&match(auth.domain,/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/))return {origin:`https://${auth.domain}.freshservice.com`,secret:auth.apiKey,auth:{type:'CUSTOM_AUTH',props:{domain:auth.domain,api_key:auth.apiKey}}};
  if(profile==='gitea.create-issue'&&keys(auth,['origin','accessToken','owner','repository'])&&token(auth.accessToken)&&segment(auth.owner)&&segment(auth.repository))return {origin:canonicalOrigin(auth.origin),secret:auth.accessToken,auth:{access_token:auth.accessToken,props:{baseUrl:canonicalOrigin(auth.origin),owner:auth.owner,repository:auth.repository}}};
  if(profile==='datadog.submit-log'&&keys(auth,['site','apiKey'])&&token(auth.apiKey)&&['datadoghq.com','us3.datadoghq.com','us5.datadoghq.com','datadoghq.eu','ap1.datadoghq.com','ap2.datadoghq.com','ddog-gov.com'].includes(auth.site as string))return {origin:`https://http-intake.logs.${auth.site}`,secret:auth.apiKey,auth:{type:'CUSTOM_AUTH',props:{site:auth.site,apiKey:auth.apiKey}}};
  if((profile==='gmail.get-thread'||profile==='slack.get-thread-replies')&&keys(auth,['accessToken'])&&token(auth.accessToken))return {origin:profile==='gmail.get-thread'?'https://gmail.googleapis.com':'https://slack.com',secret:auth.accessToken,auth:{type:'OAUTH2',access_token:auth.accessToken}};
  if(profile==='linear.get-issue'&&keys(auth,['apiKey'])&&token(auth.apiKey))return {origin:'https://api.linear.app',secret:auth.apiKey,auth:{type:'SECRET_TEXT',secret_text:auth.apiKey}};
  if(profile==='google-cloud-storage.search-objects'&&keys(auth,['accessToken','bucket'])&&token(auth.accessToken)&&bucketName(auth.bucket))return {origin:'https://www.googleapis.com',secret:auth.accessToken,auth:{access_token:auth.accessToken,bucket:auth.bucket}};
  throw new ActivepiecesError('invalid_configuration',false,false,undefined,isWriteProfile(profile)?'write':'read');
}
const stringSchema = (maxLength=4096,minLength=1) => ({type:'string',minLength,maxLength,pattern:'^(?:[^\\u0000-\\u0008\\u000b\\u000c\\u000e-\\u001f\\u007f\\uD800-\\uDFFF]|[\\uD800-\\uDBFF][\\uDC00-\\uDFFF])*$'});
const exactPattern = (pattern:string) => pattern+'(?![\\s\\S])';
const cursorSchema = {...stringSchema(2048),pattern:exactPattern('^(?:[^\\u0000-\\u0020\\u007f\\uD800-\\uDFFF]|[\\uD800-\\uDBFF][\\uDC00-\\uDFFF])+$')};
const fields = (properties:Record<string,unknown>,required:string[]) => ({type:'object',additionalProperties:false,properties,required});
const mapSchema = {type:'object',maxProperties:100,propertyNames:{...stringSchema(100),not:{enum:['__proto__','prototype','constructor']}},additionalProperties:{anyOf:[stringSchema(8192,0),{type:['number','boolean','null']}]}};
const positiveSchema = {type:'integer',minimum:1,maximum:Number.MAX_SAFE_INTEGER};
const schemas:Record<EffectProfile,Record<string,unknown>> = {
 'activecampaign.create-contact':fields({email:{...stringSchema(320),allOf:[{pattern:exactPattern('^[^\\s@]+@[^\\s@]+\\.[^\\s@]+$')}]},firstName:stringSchema(255,0),lastName:stringSchema(255,0),phone:stringSchema(100,0),contactCustomFields:{...mapSchema,propertyNames:{pattern:exactPattern('^[1-9][0-9]{0,18}$')}}},['email','contactCustomFields']),
 'freshservice.create-ticket':fields({subject:stringSchema(255),description:stringSchema(65536),requester_id:positiveSchema,status:{enum:[2,3,4,5]},priority:{enum:[1,2,3,4]},source:{enum:[1,2,3,7,9,10]},urgency:{enum:[1,2,3]},impact:{enum:[1,2,3]},type:stringSchema(255,0),responder_id:positiveSchema,department_id:positiveSchema,group_id:positiveSchema,category:stringSchema(255,0),sub_category:stringSchema(255,0),tags:{type:'array',maxItems:100,items:stringSchema(255)},custom_fields:mapSchema},['subject','description','requester_id']),
 'gitea.create-issue':fields({title:stringSchema(255),body:stringSchema(65536,0)},['title']),
 'datadog.submit-log':fields({message:stringSchema(65536),ddsource:stringSchema(1024,0),ddtags:stringSchema(4096,0),hostname:stringSchema(1024,0),service:stringSchema(1024,0),additionalProperties:{...mapSchema,propertyNames:{...mapSchema.propertyNames,not:{enum:['__proto__','prototype','constructor','message','ddsource','ddtags','hostname','service','additionalProperties']}}}},['message']),
 'gmail.get-thread':fields({thread_id:{...stringSchema(128),pattern:exactPattern('^[A-Za-z0-9_-]+$')},format:{enum:['full','metadata','minimal'],default:'full'}},['thread_id']),
 'slack.get-thread-replies':fields({channel:{...stringSchema(64),pattern:exactPattern('^[CDG][A-Z0-9]+$')},threadTs:{...stringSchema(23),pattern:exactPattern('^[0-9]{1,16}\\.[0-9]{6}$')},cursor:cursorSchema},['channel','threadTs']),
 'linear.get-issue':fields({issue_id:{...stringSchema(100),pattern:exactPattern('^(?:[A-Za-z][A-Za-z0-9_]*-[0-9]+|[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})$')}},['issue_id']),
 'google-cloud-storage.search-objects':{...fields({prefix:stringSchema(1024,0),matchGlob:stringSchema(1024),delimiter:{enum:['/']},includeFoldersAsPrefixes:{type:'boolean'},versions:{type:'boolean'},pageToken:cursorSchema,maxResults:{type:'integer',minimum:1,maximum:1000,default:100}},[]),allOf:[{if:{properties:{includeFoldersAsPrefixes:{const:true}},required:['includeFoldersAsPrefixes']},then:{properties:{delimiter:{const:'/'}},required:['delimiter']}}]},
};
const actions:Record<EffectProfile,string>={'activecampaign.create-contact':'activecampaign_create_contact','freshservice.create-ticket':'create_ticket','gitea.create-issue':'create_issue','datadog.submit-log':'sendOneLog','gmail.get-thread':'gmail_get_thread','slack.get-thread-replies':'slack_get_thread_replies','linear.get-issue':'linear_get_issue','google-cloud-storage.search-objects':'search_objects'};
export function describeEffect(profile:string) {
 if(!isEffectProfile(profile))return undefined;
 return {profile,action:actions[profile],effect:isWriteProfile(profile)?'write' as const:'read' as const,inputSchema:structuredClone(schemas[profile]),omittedSetupCallbacks:['authentication validation','dynamic dropdowns','credential acquisition and refresh']};
}
export interface EffectPlan {kind:'effect';effect:'read'|'write';profile:EffectProfile;method:'GET'|'POST';body?:unknown;query?:Record<string,string>;expected:Record<string,unknown>}
export function effectInputs(selection:Selection,value:Record<string,unknown>):{props:Record<string,unknown>;pathname:string;effect:EffectPlan}|undefined {
 const p=selection.profile;if(!isEffectProfile(p))return undefined;
 const schema=schemas[p],required=schema.required as string[],allowed=Object.keys(schema.properties as object);
 if(!record(value)||!keys(value,allowed,required))throw 0;
 const props=structuredClone(value),plan:EffectPlan={kind:'effect',effect:isWriteProfile(p)?'write':'read',profile:p,method:isWriteProfile(p)||p==='slack.get-thread-replies'||p==='linear.get-issue'?'POST':'GET',expected:{}};
 let pathname='';
 if(p==='activecampaign.create-contact'){
  if(!text(props.email,320)||!match(props.email,/^[^\s@]+@[^\s@]+\.[^\s@]+$/)||!safeMap(props.contactCustomFields)||!Object.keys(props.contactCustomFields).every(k=>match(k,/^[1-9][0-9]{0,18}$/)))throw 0;
  for(const k of ['firstName','lastName','phone'])if(props[k]!==undefined&&!text(props[k],k==='phone'?100:255,0))throw 0;
  pathname='/api/3/contacts';plan.body={contact:{email:props.email,firstName:props.firstName,lastName:props.lastName,phone:props.phone,fieldValues:Object.entries(props.contactCustomFields).map(([field,value])=>({field,value}))}};plan.expected={email:props.email};
 }else if(p==='freshservice.create-ticket'){
  if(!text(props.subject,255)||!text(props.description,65536)||!posint(props.requester_id))throw 0;
  const enums:Record<string,number[]>={status:[2,3,4,5],priority:[1,2,3,4],source:[1,2,3,7,9,10],urgency:[1,2,3],impact:[1,2,3]};
  for(const [k,values]of Object.entries(enums))if(props[k]!==undefined&&!values.includes(props[k] as number))throw 0;
  for(const k of ['responder_id','department_id','group_id'])if(props[k]!==undefined&&!posint(props[k]))throw 0;
  for(const k of ['type','category','sub_category'])if(props[k]!==undefined&&!text(props[k],255,0))throw 0;
  if(props.tags!==undefined){
   if(!Array.isArray(props.tags)||props.tags.length>100)throw 0;
   for(let index=0;index<props.tags.length;index++)if(!Object.hasOwn(props.tags,index)||!text(props.tags[index],255))throw 0;
  }
  if(props.custom_fields!==undefined&&!safeMap(props.custom_fields))throw 0;
  const body:Record<string,unknown>={subject:props.subject,description:props.description,requester_id:props.requester_id};
  for(const [k,v]of Object.entries(props))if(v&&(k!=='tags'||(v as unknown[]).length>0))body[k]=v;
  pathname='/api/v2/tickets';plan.body=body;plan.expected={requester_id:props.requester_id,subject:props.subject};
 }else if(p==='gitea.create-issue'){
  if(!text(props.title,255)||(props.body!==undefined&&!text(props.body,65536,0)))throw 0;
  const auth=selection.auth as {props:{owner:string;repository:string}};props.repository={owner:auth.props.owner,repo:auth.props.repository};
  pathname=`/api/v1/repos/${auth.props.owner}/${auth.props.repository}/issues`;plan.body={title:props.title,body:props.body};plan.expected={title:props.title};
 }else if(p==='datadog.submit-log'){
  if(!text(props.message,65536))throw 0;
  for(const k of ['ddsource','ddtags','hostname','service'])if(props[k]!==undefined&&!text(props[k],k==='ddtags'?4096:1024,0))throw 0;
  if(props.additionalProperties!==undefined&&(!safeMap(props.additionalProperties)||Object.keys(props.additionalProperties).some(k=>['message','ddsource','ddtags','hostname','service','additionalProperties'].includes(k))))throw 0;
  const {additionalProperties,...standard}=props;plan.body=[{...standard,...(additionalProperties as object??{})}];pathname='/api/v2/logs';
 }else if(p==='gmail.get-thread'){
  if(!match(props.thread_id,/^[A-Za-z0-9_-]{1,128}$/)||props.format!==undefined&&!['full','metadata','minimal'].includes(props.format as string))throw 0;
  props.format??='full';pathname=`/gmail/v1/users/me/threads/${props.thread_id}`;plan.query={format:props.format as string};plan.expected={id:props.thread_id,format:props.format};
 }else if(p==='slack.get-thread-replies'){
  if(!match(props.channel,/^[CDG][A-Z0-9]{1,63}$/)||!timestamp(props.threadTs)||props.cursor!==undefined&&!cursor(props.cursor))throw 0;
  pathname='/api/conversations.replies';plan.expected={channel:props.channel,ts:props.threadTs,cursor:props.cursor};
 }else if(p==='linear.get-issue'){
  if(!text(props.issue_id,100)||!match(props.issue_id,/^(?:[A-Za-z][A-Za-z0-9_]*-[0-9]+|[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})$/))throw 0;
  pathname='/graphql';plan.expected={id:props.issue_id.includes('-')&&props.issue_id.split('-').length===2?props.issue_id.toUpperCase():props.issue_id};
 }else{
  for(const k of ['prefix','matchGlob'])if(props[k]!==undefined&&!text(props[k],1024,k==='prefix'?0:1))throw 0;
  if(props.delimiter!==undefined&&props.delimiter!=='/'||props.includeFoldersAsPrefixes===true&&props.delimiter!=='/'||props.matchGlob!==undefined&&props.delimiter!==undefined&&props.delimiter!=='/')throw 0;
  for(const k of ['includeFoldersAsPrefixes','versions'])if(props[k]!==undefined&&typeof props[k]!=='boolean')throw 0;
  if(props.pageToken!==undefined&&!cursor(props.pageToken)||props.maxResults!==undefined&&(!posint(props.maxResults)||(props.maxResults as number)>1000))throw 0;
  props.maxResults??=100;props.bucket=(selection.auth as Record<string,unknown>).bucket;
  const query:Record<string,string>={};for(const k of ['prefix','matchGlob','delimiter','includeFoldersAsPrefixes','versions','pageToken','maxResults'])if(props[k])query[k]=String(props[k]);
  plan.query=query;pathname=`/storage/v1/b/${props.bucket}/o`;plan.expected={bucket:props.bucket,limit:props.maxResults,pageToken:props.pageToken};
 }
 if(Buffer.byteLength(JSON.stringify(props))>131072)throw 0;
 return {props,pathname,effect:plan};
}
function sameHeaders(actual:unknown,expected:Record<string,string>):boolean {
 if(!record(actual)||Object.keys(actual).length!==Object.keys(expected).length)return false;
 const a=new Headers(actual as Record<string,string>),b=new Headers(expected);return isDeepStrictEqual([...a],[...b]);
}
function labels(value:unknown,max:number):value is {nodes:Record<string,unknown>[];pageInfo:{hasNextPage:boolean;endCursor?:string|null}} {
 return record(value)&&Array.isArray(value.nodes)&&value.nodes.length<=max&&value.nodes.every(v=>record(v)&&text(v.id,128)&&text(v.name,1024,0))&&record(value.pageInfo)&&typeof value.pageInfo.hasNextPage==='boolean'&&(value.pageInfo.endCursor===undefined||value.pageInfo.endCursor===null||cursor(value.pageInfo.endCursor));
}
function mime(value:unknown,depth=0):boolean {
 if(depth>32||!record(value))return false;
 for(const k of ['partId','mimeType','filename'])if(value[k]!==undefined&&typeof value[k]!=='string')return false;
 if(value.headers!==undefined&&(!Array.isArray(value.headers)||!value.headers.every(x=>record(x)&&typeof x.name==='string'&&typeof x.value==='string')))return false;
 if(value.body!==undefined&&(!record(value.body)||value.body.size!==undefined&&(!Number.isSafeInteger(value.body.size)||(value.body.size as number)<0)||value.body.data!==undefined&&typeof value.body.data!=='string'||value.body.attachmentId!==undefined&&typeof value.body.attachmentId!=='string'))return false;
 return value.parts===undefined||Array.isArray(value.parts)&&value.parts.every(x=>mime(x,depth+1));
}
/** Fixed selected-operation contract. HTTP POST alone never grants write semantics. */
export class EffectRequestState {
 private requests=0;
 private expectedCursor:unknown;
 private readonly cursors=new Set<string>();
 private linearId:string|undefined;
 private linearLabels:string[]=[];
 private linearHasMore=false;
 constructor(private readonly selection:Selection,private readonly pathname:string,readonly plan:EffectPlan){this.expectedCursor=plan.expected.cursor;if(typeof this.expectedCursor==='string')this.cursors.add(this.expectedCursor);}
 prepare(value:unknown):{url:string;headers:Headers;method:'GET'|'POST';body?:string}{
  const p=this.plan.profile,s=this.selection.secret;
  if(!record(value)||!keys(value,['method','url','headers','authentication','queryParams','body'],['method','url'])||value.method!==this.plan.method)throw 0;
  if(++this.requests>(p==='slack.get-thread-replies'?50:p==='linear.get-issue'?21:1))throw 0;
  let expectedUrl=this.selection.origin+this.pathname;
  if(p==='google-cloud-storage.search-objects')expectedUrl+='?'+new URLSearchParams(this.plan.query).toString();
  if(value.url!==expectedUrl)throw 0;
  const headers=new Headers({Accept:'application/json'});
  if(p==='activecampaign.create-contact'){
   if(value.authentication!==undefined||!sameHeaders(value.headers,{'Api-Token':s}))throw 0;headers.set('Api-Token',s);
  }else if(p==='freshservice.create-ticket'){
   if(!isDeepStrictEqual(value.authentication,{type:'BASIC',username:s,password:'X'})||!sameHeaders(value.headers,{Accept:'application/json','Content-Type':'application/json'}))throw 0;headers.set('Authorization',`Basic ${Buffer.from(`${s}:X`).toString('base64')}`);
  }else if(['gitea.create-issue','gmail.get-thread','google-cloud-storage.search-objects'].includes(p)){
   if(value.headers!==undefined||!isDeepStrictEqual(value.authentication,{type:'BEARER_TOKEN',token:s}))throw 0;headers.set('Authorization',`Bearer ${s}`);
  }else if(p==='slack.get-thread-replies'){
   if(value.authentication!==undefined||!record(value.headers)||value.headers.authorization!==`Bearer ${s}`||value.headers['content-type']!=='application/x-www-form-urlencoded'||Object.keys(value.headers).some(k=>!['authorization','content-type','user-agent','accept'].includes(k)))throw 0;
   headers.set('Authorization',`Bearer ${s}`);headers.set('Content-Type','application/x-www-form-urlencoded');
  }else if(p==='linear.get-issue'){
   if(value.authentication!==undefined||!record(value.headers)||value.headers.authorization!==s||value.headers['content-type']!=='application/json'||Object.keys(value.headers).some(k=>!['authorization','content-type','user-agent'].includes(k)))throw 0;
   headers.set('Authorization',s);
  }else{
   if(value.authentication!==undefined||!record(value.headers)||value.headers['dd-api-key']!==s||value.headers['content-type']!=='application/json'||Object.keys(value.headers).some(k=>!['dd-api-key','content-type','accept','user-agent'].includes(k)))throw 0;
   headers.set('DD-API-KEY',s);
  }
  let body:string|undefined;
  if(p==='slack.get-thread-replies'){
   if(typeof value.body!=='string')throw 0;
   const form=new URLSearchParams(value.body);if([...form].length!==new Set([...form.keys()]).size)throw 0;
   const expected:Record<string,string>={channel:this.plan.expected.channel as string,ts:this.plan.expected.ts as string,limit:'200'};if(this.expectedCursor)expected.cursor=this.expectedCursor as string;
   if(!isDeepStrictEqual(Object.fromEntries(form),expected))throw 0;body=value.body;
  }else if(p==='linear.get-issue'){
   if(typeof value.body!=='string')throw 0;
   const actual=JSON.parse(value.body);
   const expected=this.requests===1?{query:GET_ISSUE_QUERY,variables:{id:this.plan.expected.id}}:{query:ISSUE_LABELS_PAGE_QUERY,variables:{id:this.linearId,first:250,...(this.linearLabels.length?{filter:{id:{nin:this.linearLabels}}}:{})}};
   if(this.requests>1&&!this.linearHasMore||!isDeepStrictEqual(actual,expected))throw 0;body=value.body;
  }else if(this.plan.method==='POST'){
   const actual=p==='datadog.submit-log'&&typeof value.body==='string'?JSON.parse(value.body):value.body;
   if(!isDeepStrictEqual(JSON.parse(JSON.stringify(actual)),JSON.parse(JSON.stringify(this.plan.body))))throw 0;body=JSON.stringify(actual);
  }else if(value.body!==undefined)throw 0;
  if(body!==undefined&&Buffer.byteLength(body)>131072)throw 0;
  if(body!==undefined&&p!=='slack.get-thread-replies')headers.set('Content-Type','application/json');
  const url=new URL(this.selection.origin+this.pathname);
  if(this.plan.query){if(p==='google-cloud-storage.search-objects'?value.queryParams!==undefined:!isDeepStrictEqual(value.queryParams,this.plan.query))throw 0;for(const [k,v]of Object.entries(this.plan.query))url.searchParams.set(k,v);}else if(value.queryParams!==undefined)throw 0;
  return {url:url.toString(),headers,method:this.plan.method,...(body===undefined?{}:{body})};
 }
 bodyFailure(body:unknown):FailureKind|undefined {
  const p=this.plan.profile,e=this.plan.expected,bad='invalid_response';
  if(!record(body))return bad;
  if(p==='activecampaign.create-contact')return record(body.contact)&&text(body.contact.id,128)&&body.contact.email===e.email?undefined:bad;
  if(p==='freshservice.create-ticket')return record(body.ticket)&&posint(body.ticket.id)&&body.ticket.requester_id===e.requester_id&&body.ticket.subject===e.subject?undefined:bad;
  if(p==='gitea.create-issue')return posint(body.id)&&posint(body.number)&&body.title===e.title?undefined:bad;
  if(p==='datadog.submit-log')return Object.keys(body).length===0?undefined:bad;
  if(p==='gmail.get-thread'){
   if(body.id!==e.id||!Array.isArray(body.messages)||body.messages.length>10000)return bad;
   const ids=new Set<string>();
   for(const message of body.messages){if(!record(message)||!text(message.id,128)||ids.has(message.id)||message.threadId!==e.id||message.payload!==undefined&&!mime(message.payload)||message.labelIds!==undefined&&(!Array.isArray(message.labelIds)||!message.labelIds.every(x=>text(x,128)))||Object.hasOwn(message,'raw'))return bad;ids.add(message.id);}
   return undefined;
  }
  if(p==='slack.get-thread-replies'){
   if(body.ok!==true||!Array.isArray(body.messages)||body.messages.length>200||!body.messages.every(m=>record(m)&&timestamp(m.ts)&&(m.thread_ts===undefined||m.thread_ts===e.ts)&&(!Object.hasOwn(m,'channel')||m.channel===e.channel))||body.has_more!==undefined&&typeof body.has_more!=='boolean'||body.response_metadata!==undefined&&!record(body.response_metadata))return bad;
   if(this.requests===1&&!e.cursor&&body.messages.length>0&&(body.messages[0] as Record<string,unknown>).ts!==e.ts)return bad;
   const next=record(body.response_metadata)?body.response_metadata.next_cursor:undefined;
   if(next!==undefined&&next!==''&&!cursor(next)||body.has_more===true&&!next)return bad;
   if(typeof next==='string'&&next){if(this.cursors.has(next))return bad;this.cursors.add(next);}
   this.expectedCursor=next;return undefined;
  }
  if(p==='linear.get-issue'){
   if(Object.hasOwn(body,'errors'))return 'upstream_error';
   if(!record(body.data)||!record(body.data.issue))return bad;
   const issue=body.data.issue;
   if(!labels(issue.labels,this.requests===1?20:250))return bad;
   if(this.requests===1){
    if(!text(issue.id,128)||!(issue.id.toLowerCase()===(e.id as string).toLowerCase()||issue.identifier===e.id)||!posint(issue.number)||!Number.isFinite(issue.priority)||!['identifier','title','url','priorityLabel','branchName','createdAt','updatedAt'].every(k=>typeof issue[k]==='string'))return bad;
    this.linearId=issue.id;this.linearHasMore=issue.labels.pageInfo.hasNextPage;
   }else{
    const ids=issue.labels.nodes.map(n=>n.id as string);if(ids.some(id=>this.linearLabels.includes(id)))return bad;
    this.linearLabels.push(...ids);this.linearHasMore=issue.labels.pageInfo.hasNextPage;
   }
   return undefined;
  }
  if(Object.hasOwn(body,'error'))return 'upstream_error';
  if(body.items!==undefined&&(!Array.isArray(body.items)||!body.items.every(x=>record(x)&&x.bucket===e.bucket&&text(x.name,1024))))return bad;
  if(body.prefixes!==undefined&&(!Array.isArray(body.prefixes)||!body.prefixes.every(x=>text(x,1024))))return bad;
  if((Array.isArray(body.items)?body.items.length:0)+(Array.isArray(body.prefixes)?body.prefixes.length:0)>(e.limit as number))return 'response_too_large';
  if(body.nextPageToken!==undefined&&(!cursor(body.nextPageToken)||body.nextPageToken===e.pageToken))return bad;
  return undefined;
 }
 projectResult(value:unknown):unknown {
  const p=this.plan.profile;
  if(this.plan.effect==='write'){
   if(p==='datadog.submit-log'&&(!record(value)||value.success!==true||value.message!=='Logs sent successfully'))throw 0;
   return {writeOutcome:'acknowledged',acknowledgement:p==='datadog.submit-log'?'accepted_for_processing':'created',data:value};
  }
  if(p==='google-cloud-storage.search-objects'){
   if(!record(value)||value.success!==true||value.bucket!==this.plan.expected.bucket||!Array.isArray(value.items)||!Array.isArray(value.prefixes)||value.totalItems!==value.items.length)throw 0;
   const {nextPageToken,...rest}=value;return {...rest,...(nextPageToken===undefined?{}:{nextPageToken}),pageOnly:true};
  }
  return value;
 }
}
