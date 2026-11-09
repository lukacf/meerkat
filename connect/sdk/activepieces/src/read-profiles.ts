import { ActivepiecesError, type FailureKind } from './errors.js';
import { canonicalOrigin, configurationKeys as keys, selectedSecret as token, record, type Selection } from './profiles.js';

export const READ_PROFILE_IDS = ['jira-cloud.get-issue', 'dropbox.get-file-metadata', 'google-sheets.get-values',
  'microsoft-excel-365.get-table-columns', 'microsoft-teams.get-channel-message', 'quickbooks.find-invoice', 'xero.find-invoice', 'algolia.browse-records'] as const;
export type ReadProfile = typeof READ_PROFILE_IDS[number];
export type ReadConfiguration =
  { readonly profile:'jira-cloud.get-issue'; readonly auth:{ readonly origin:string; readonly username:string; readonly apiToken:string } } |
  { readonly profile:'dropbox.get-file-metadata' | 'google-sheets.get-values'; readonly auth:{ readonly accessToken:string } } |
  { readonly profile:'microsoft-excel-365.get-table-columns' | 'microsoft-teams.get-channel-message'; readonly auth:{ readonly accessToken:string; readonly cloud:'commercial'|'government' } } |
  { readonly profile:'quickbooks.find-invoice'; readonly auth:{ readonly accessToken:string; readonly companyId:string } } |
  { readonly profile:'xero.find-invoice'; readonly auth:{ readonly accessToken:string; readonly tenantId:string } } |
  { readonly profile:'algolia.browse-records'; readonly auth:{ readonly apiKey:string; readonly applicationId:string; readonly indexName:string } };
const hasProfile = (value:string):value is ReadProfile => (READ_PROFILE_IDS as readonly string[]).includes(value);
const end = '(?![\\s\\S])';
const scalarPattern = `^(?:[^\\u0000-\\u001f\\u007f\\ud800-\\udfff]|[\\ud800-\\udbff][\\udc00-\\udfff])+${end}`;
const scalar = (max=256) => ({type:'string', minLength:1, maxLength:max, pattern:scalarPattern});
const text = (value:unknown,max=256):value is string => typeof value==='string' && [...value].length>0 && [...value].length<=max && !/[\x00-\x1f\x7f]/.test(value) && !/[\ud800-\udfff]/u.test(value);
const nonempty = (value:unknown,max=256):value is string => text(value,max) && !!value.trim();
const idPattern = `^[A-Za-z0-9_-]{1,128}${end}`;
const issuePattern = `^(?:[1-9][0-9]{0,63}|[A-Z][A-Z0-9_]{0,63}-[1-9][0-9]{0,63})${end}`;
const uuidPattern = `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}${end}`;
const a1Pattern = `^[A-Z]{1,3}[1-9][0-9]{0,6}(?::[A-Z]{1,3}[1-9][0-9]{0,6})?${end}`;
const trimWhitespace = '[\\u0009-\\u000d\\u0020\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000\\ufeff]';
const boolNull = {anyOf:[{type:'boolean'},{type:'null'}]};
const integer = (max:number) => ({type:'integer',minimum:1,maximum:max});
const encodedId = () => ({...scalar(),not:{pattern:`^(?:${trimWhitespace}*|\\.{1,2})${end}`}});
const literalId = (value:unknown):value is string => nonempty(value) && value!=='.' && value!=='..';
const matches = (value:unknown,pattern:string):value is string => typeof value==='string' && new RegExp(pattern).test(value);
const positive = (value:unknown,max:number):value is number => Number.isSafeInteger(value) && (value as number)>=1 && (value as number)<=max;
const encode = encodeURIComponent;
const googleEncode = (value:string) => encode(value).replace(/[!'()*]/g,char=>'%'+char.charCodeAt(0).toString(16).toUpperCase());
const expansions = ['renderedFields','names','schema','transitions','editmeta','changelog'];

// Effects are declared per selected operation. POST itself grants no mutation
// capability: only these two exact read RPC routes/body contracts are admitted.
export const READ_OPERATION_METADATA: Record<ReadProfile,{effect:'read';method:'GET'|'POST';maxRequests:number}> = {
  'jira-cloud.get-issue':{effect:'read',method:'GET',maxRequests:1},
  'dropbox.get-file-metadata':{effect:'read',method:'POST',maxRequests:1},
  'google-sheets.get-values':{effect:'read',method:'GET',maxRequests:1},
  'microsoft-excel-365.get-table-columns':{effect:'read',method:'GET',maxRequests:1},
  'microsoft-teams.get-channel-message':{effect:'read',method:'GET',maxRequests:1},
  'quickbooks.find-invoice':{effect:'read',method:'GET',maxRequests:1},
  'xero.find-invoice':{effect:'read',method:'GET',maxRequests:1},
  'algolia.browse-records':{effect:'read',method:'POST',maxRequests:10},
};
for (const metadata of Object.values(READ_OPERATION_METADATA)) Object.freeze(metadata);
Object.freeze(READ_OPERATION_METADATA);
export interface ReadPlan { profile:ReadProfile; body?:Record<string,unknown>; expected:Record<string,unknown> }
export function selectRead(profile:string,auth:Record<string,unknown>):Pick<Selection,'origin'|'secret'|'auth'>|undefined {
  if(!hasProfile(profile))return undefined;
  const oauth = (origin:string,props?:Record<string,unknown>)=>({origin,secret:auth.accessToken as string,auth:{type:'OAUTH2',access_token:auth.accessToken,...(props?{props}:{})}});
  if(profile==='jira-cloud.get-issue' && keys(auth,['origin','username','apiToken']) && token(auth.apiToken) && nonempty(auth.username) && !auth.username.includes(':')) {
    const origin=canonicalOrigin(auth.origin);return {origin,secret:auth.apiToken,auth:{type:'CUSTOM_AUTH',props:{instanceUrl:origin,email:auth.username,apiToken:auth.apiToken}}};
  }
  if((profile==='dropbox.get-file-metadata'||profile==='google-sheets.get-values') && keys(auth,['accessToken']) && token(auth.accessToken))return oauth(profile==='dropbox.get-file-metadata'?'https://api.dropboxapi.com':'https://sheets.googleapis.com');
  if((profile==='microsoft-excel-365.get-table-columns'||profile==='microsoft-teams.get-channel-message') && keys(auth,['accessToken','cloud']) && token(auth.accessToken) && ['commercial','government'].includes(auth.cloud as string))return oauth(auth.cloud==='government'?'https://graph.microsoft.us':'https://graph.microsoft.com',{cloud:auth.cloud==='government'?'login.microsoftonline.us':'login.microsoftonline.com'});
  if(profile==='quickbooks.find-invoice' && keys(auth,['accessToken','companyId']) && token(auth.accessToken) && matches(auth.companyId,`^[1-9][0-9]{0,19}${end}`))return oauth('https://quickbooks.api.intuit.com',{companyId:auth.companyId});
  if(profile==='xero.find-invoice' && keys(auth,['accessToken','tenantId']) && token(auth.accessToken) && matches(auth.tenantId,uuidPattern))return oauth('https://api.xero.com',{tenantId:auth.tenantId});
  if(profile==='algolia.browse-records' && keys(auth,['apiKey','applicationId','indexName']) && token(auth.apiKey) && matches(auth.applicationId,`^[A-Z0-9]{1,64}${end}`) && literalId(auth.indexName) && auth.indexName===auth.indexName.trim())return {
    origin:`https://${auth.applicationId.toLowerCase()}.algolia.net`,secret:auth.apiKey,auth:{type:'CUSTOM_AUTH',props:{apiKey:auth.apiKey,applicationId:auth.applicationId,indexName:auth.indexName}},
  };
  throw 0;
}
const actionNames:Record<ReadProfile,string>={'jira-cloud.get-issue':'get_issue','dropbox.get-file-metadata':'get_dropbox_file_metadata','google-sheets.get-values':'sheets_get_values',
  'microsoft-excel-365.get-table-columns':'get_table_columns','microsoft-teams.get-channel-message':'microsoft_teams_get_channel_message','quickbooks.find-invoice':'find_invoice','xero.find-invoice':'xero_find_invoice','algolia.browse-records':'browse-records'};
export function describeRead(profile:string):{action:string;inputSchema:Record<string,unknown>}|undefined {
  if(!hasProfile(profile))return undefined;
  let properties:Record<string,unknown>,required:string[],extra:Record<string,unknown>={};
  if(profile==='jira-cloud.get-issue'){properties={issueId:{type:'string',pattern:issuePattern},expand:{anyOf:[{type:'array',maxItems:6,uniqueItems:true,items:{enum:expansions}},{type:'null'}]}};required=['issueId'];}
  else if(profile==='dropbox.get-file-metadata'){properties={path:{...scalar(2048),pattern:`^(?:/(?:[^\\u0000-\\u001f\\u007f\\ud800-\\udfff]|[\\ud800-\\udbff][\\udc00-\\udfff])*|id:[A-Za-z0-9_-]+|rev:[a-f0-9]+)${end}`},include_deleted:boolNull,include_media_info:boolNull,include_has_explicit_shared_members:boolNull};required=['path'];}
  else if(profile==='google-sheets.get-values'){properties={spreadsheet_id:{type:'string',pattern:idPattern},worksheet_title:{...scalar(100)},range:{type:'string',pattern:a1Pattern,description:'Explicit finite A1 cell or rectangle. Up to 1,000 rows, 100 columns and 10,000 cells; ordered endpoints; row <= 1,000,000 and column <= ZZZ.'},major_dimension:{enum:['ROWS','COLUMNS']},value_render_option:{enum:['FORMATTED_VALUE','UNFORMATTED_VALUE','FORMULA']}};required=['spreadsheet_id','worksheet_title','range'];}
  else if(profile==='microsoft-excel-365.get-table-columns'){properties={storageSource:{enum:['onedrive','sharepoint']},siteId:encodedId(),documentId:encodedId(),workbookId:encodedId(),worksheetId:encodedId(),tableId:encodedId(),limit:integer(100)};required=['storageSource','workbookId','worksheetId','tableId'];extra={oneOf:[{properties:{storageSource:{const:'onedrive'}},not:{anyOf:[{required:['siteId']},{required:['documentId']}]}},{properties:{storageSource:{const:'sharepoint'}},required:['siteId','documentId']}]};}
  else if(profile==='microsoft-teams.get-channel-message'){properties={teamId:encodedId(),channelId:encodedId(),messageId:encodedId(),replyId:{anyOf:[encodedId(),{const:''},{type:'null'}]}};required=['teamId','channelId','messageId'];}
  else if(profile==='quickbooks.find-invoice'){properties={invoice_number:{...scalar(),not:{pattern:'\\\\'}}};required=['invoice_number'];}
  else if(profile==='xero.find-invoice'){properties={value:{...scalar(),not:{pattern:`^${trimWhitespace}*${end}`}},type_filter:{enum:['ACCREC','ACCPAY','',null]},summary_only:boolNull,page:{anyOf:[integer(10000),{type:'null'}]}};required=['value'];}
  else{properties={limit:{anyOf:[integer(1000),{type:'null'}]}};required=[];}
  return {action:actionNames[profile],inputSchema:{type:'object',properties,required,additionalProperties:false,...extra}};
}
function a1Bounds(range:string){
  const cell=(value:string)=>{const match=/^([A-Z]+)([0-9]+)$/.exec(value)!;return {col:[...match[1]!].reduce((n,c)=>n*26+c.charCodeAt(0)-64,0),row:Number(match[2])};};
  const [start,endCell]=range.split(':');const a=cell(start!),b=cell(endCell??start!);
  const rows=b.row-a.row+1,cols=b.col-a.col+1;
  if(rows<1||cols<1||b.row>1000000||b.col>18278||rows>1000||cols>100||rows*cols>10000)throw 0;
  return {rows,cols,startRow:a.row,startCol:a.col};
}
export function readInputs(selection:Selection,value:Record<string,unknown>):{props:Record<string,unknown>;pathname:string;query?:Record<string,string>;inlineQuery?:string;read:ReadPlan}|undefined {
  const profile=selection.profile;if(!hasProfile(profile))return undefined;
  const fail=():never=>{throw new ActivepiecesError('invalid_arguments');};
  const auth=(selection.auth as {props?:Record<string,unknown>}).props??{};
  const finish=(props:Record<string,unknown>,pathname:string,expected:Record<string,unknown>,query?:Record<string,string>,body?:Record<string,unknown>,inlineQuery?:string)=>({props,pathname,query,inlineQuery,read:{profile,body,expected}});
  if(profile==='jira-cloud.get-issue' && keys(value,['issueId','expand'],['issueId']) && matches(value.issueId,issuePattern) && (value.expand==null||(Array.isArray(value.expand)&&value.expand.length<=6&&new Set(value.expand).size===value.expand.length&&value.expand.every(v=>expansions.includes(v)))))return finish({...structuredClone(value),mapNames:false,mapTransitions:false},'/rest/api/3/issue/'+value.issueId,{issueId:value.issueId},Array.isArray(value.expand)?{expand:value.expand.join(',')}:{});
  if(profile==='dropbox.get-file-metadata' && keys(value,['path','include_deleted','include_media_info','include_has_explicit_shared_members'],['path']) && text(value.path,2048) && new RegExp((describeRead(profile)!.inputSchema.properties as any).path.pattern).test(value.path)){
    const body:Record<string,unknown>={path:value.path};for(const name of ['include_deleted','include_media_info','include_has_explicit_shared_members']){if(value[name]!=null&&typeof value[name]!=='boolean')return fail();body[name]=value[name]??false;}
    return finish(structuredClone(value),'/2/files/get_metadata',body,undefined,body);
  }
  if(profile==='google-sheets.get-values' && keys(value,['spreadsheet_id','worksheet_title','range','major_dimension','value_render_option'],['spreadsheet_id','worksheet_title','range']) && matches(value.spreadsheet_id,idPattern) && text(value.worksheet_title,100) && matches(value.range,a1Pattern)){
    const dimension=value.major_dimension??'ROWS',render=value.value_render_option??'FORMATTED_VALUE';
    if((Object.hasOwn(value,'major_dimension')&&value.major_dimension==null)||(Object.hasOwn(value,'value_render_option')&&value.value_render_option==null)||!['ROWS','COLUMNS'].includes(dimension as string)||!['FORMATTED_VALUE','UNFORMATTED_VALUE','FORMULA'].includes(render as string))return fail();
    let bounds;try{bounds=a1Bounds(value.range);}catch{return fail();}
    const title="'"+value.worksheet_title.replace(/'/g,"''")+"'",range=title+'!'+value.range;
    return finish({...value,worksheet_title:title,major_dimension:dimension,value_render_option:render},`/v4/spreadsheets/${value.spreadsheet_id}/values/${googleEncode(range)}`,{...bounds,dimension,title:value.worksheet_title},{majorDimension:dimension as string,valueRenderOption:render as string});
  }
  if(profile==='microsoft-excel-365.get-table-columns' && keys(value,['storageSource','siteId','documentId','workbookId','worksheetId','tableId','limit'],['storageSource','workbookId','worksheetId','tableId']) && ['onedrive','sharepoint'].includes(value.storageSource as string)){
    const names=['workbookId','worksheetId','tableId',...(value.storageSource==='sharepoint'?['siteId','documentId']:[])];
    if(names.some(name=>!literalId(value[name]))||(value.storageSource==='onedrive'&&(Object.hasOwn(value,'siteId')||Object.hasOwn(value,'documentId'))))return fail();
    const limit=value.limit===undefined?100:value.limit;if(!positive(limit,100))return fail();
    const props:Record<string,unknown>={...value,limit};for(const name of names)props[name]=encode(value[name] as string);
    const route=value.storageSource==='onedrive'?'/me/drive':`/sites/${props.siteId}/drives/${props.documentId}`;
    return finish(props,`/v1.0${route}/items/${props.workbookId}/workbook/worksheets/${props.worksheetId}/tables/${props.tableId}/columns`,{limit},{'$top':String(limit)});
  }
  if(profile==='microsoft-teams.get-channel-message' && keys(value,['teamId','channelId','messageId','replyId'],['teamId','channelId','messageId']) && ['teamId','channelId','messageId'].every(name=>literalId(value[name])) && (value.replyId==null||value.replyId===''||literalId(value.replyId))){
    const props:Record<string,unknown>={};for(const name of ['teamId','channelId','messageId'])props[name]=encode(value[name] as string);if(value.replyId)props.replyId=encode(value.replyId as string);
    return finish(props,`/v1.0/teams/${props.teamId}/channels/${props.channelId}/messages/${props.messageId}${props.replyId?'/replies/'+props.replyId:''}`,{id:value.replyId||value.messageId});
  }
  if(profile==='quickbooks.find-invoice' && keys(value,['invoice_number']) && text(value.invoice_number) && !value.invoice_number.includes('\\'))return finish({...value},`/v3/company/${auth.companyId}/query`,{invoiceNumber:value.invoice_number},{minorversion:'75',query:`SELECT * FROM Invoice WHERE DocNumber = '${value.invoice_number.replace(/'/g,"\\'")}' MAXRESULTS 1`});
  if(profile==='xero.find-invoice' && keys(value,['value','type_filter','summary_only','page'],['value']) && nonempty(value.value) && [undefined,null,'','ACCREC','ACCPAY'].includes(value.type_filter as any) && (value.summary_only==null||typeof value.summary_only==='boolean')){
    const page=value.page??1;if(!positive(page,10000))return fail();
    const summary=Object.hasOwn(value,'summary_only')?value.summary_only:true;
    const query:Record<string,string>={};if(summary)query.summaryOnly='true';query.page=String(page);query.SearchTerm=value.value;if(value.type_filter)query.where=`Type=="${value.type_filter}"`;
    const inline='?'+Object.entries(query).map(([k,v])=>k+'='+encode(v)).join('&');
    return finish({...value,tenant_id:auth.tenantId,search_by:'SEARCH_TERM',page,summary_only:summary},'/api.xro/2.0/Invoices',{type:value.type_filter},query,undefined,inline);
  }
  if(profile==='algolia.browse-records' && keys(value,['limit'],[])){const limit=value.limit??100;if(!positive(limit,1000))return fail();return finish({indexName:auth.indexName,limit},`/1/indexes/${encode(auth.indexName as string)}/browse`,{limit},undefined,{});}
  return fail();
}

/** Per-invocation exact request/response contract for the eight added readers. */
export class ReadRequestState {
  private requests=0;private cursor:string|undefined;private readonly cursors=new Set<string>();private hits=0;
  readonly method:'GET'|'POST';readonly maxRequests:number;
  constructor(private readonly selection:Selection,private readonly pathname:string,private readonly query:Readonly<Record<string,string>>|undefined,private readonly inline:string|undefined,private readonly plan:ReadPlan){
    if(plan.profile!==selection.profile)throw new ActivepiecesError('invalid_arguments');
    const metadata=READ_OPERATION_METADATA[plan.profile];this.method=metadata.method;this.maxRequests=metadata.maxRequests;
  }
  prepare(value:unknown):{url:string;headers:Headers;body?:string;method:'GET'|'POST'}{
    const invalid=():never=>{throw new ActivepiecesError('upstream_error');};
    const s=this.selection,p=this.plan.profile,auth=s.auth as {props?:Record<string,unknown>},props=auth.props??{};
    if(!record(value)||Object.keys(value).some(k=>!['method','url','headers','authentication','queryParams','body',...(p==='jira-cloud.get-issue'?['auth']:[])].includes(k))||value.method!==this.method||this.requests>=this.maxRequests)return invalid();
    const rawOrigin=p==='algolia.browse-records'?`https://${props.applicationId}.algolia.net`:s.origin;
    if(value.url!==rawOrigin+this.pathname+(this.inline??''))return invalid();
    const url=new URL(s.origin+this.pathname);if(url.origin!==s.origin||url.pathname!==this.pathname||url.search||url.hash||url.username||url.password)return invalid();
    if(this.query){
      if(this.inline!==undefined?value.queryParams!==undefined:!record(value.queryParams)||JSON.stringify(value.queryParams)!==JSON.stringify(this.query))return invalid();
      for(const [key,item]of Object.entries(this.query))url.searchParams.set(key,item);
    }else if(value.queryParams!==undefined)return invalid();
    const headers=new Headers({Accept:'application/json'});
    const expectedHeaders=p==='dropbox.get-file-metadata'?{'Content-Type':'application/json'}:p==='quickbooks.find-invoice'?{Accept:'application/json'}:p==='xero.find-invoice'?{'Xero-Tenant-Id':props.tenantId as string}:p==='algolia.browse-records'?{'x-algolia-application-id':props.applicationId as string,'x-algolia-api-key':s.secret,Accept:'application/json','Content-Type':'application/json'}:p==='google-sheets.get-values'?{'accept-encoding':'gzip',authorization:`Bearer ${s.secret}`,'user-agent':'google-api-nodejs-client/8.0.3 (gzip)','x-goog-api-client':`gdcl/8.0.3 gl-node/${process.versions.node}`}:p.startsWith('microsoft-')?{authorization:`Bearer ${s.secret}`}:undefined;
    if(expectedHeaders){if(!record(value.headers)||Object.keys(value.headers).length!==Object.keys(expectedHeaders).length)return invalid();for(const [k,v]of Object.entries(expectedHeaders)){if(value.headers[k]!==v)return invalid();headers.set(k,v);}}
    else if(value.headers!==undefined)return invalid();
    if(p==='jira-cloud.get-issue'){
      if(JSON.stringify(value.auth)!==JSON.stringify(s.auth)||!record(value.authentication)||!keys(value.authentication,['type','username','password'])||value.authentication.type!=='BASIC'||value.authentication.username!==props.email||value.authentication.password!==s.secret)return invalid();
      headers.set('Authorization',`Basic ${Buffer.from(`${props.email}:${s.secret}`).toString('base64')}`);
    }else if(['dropbox.get-file-metadata','quickbooks.find-invoice','xero.find-invoice'].includes(p)){
      if(!record(value.authentication)||!keys(value.authentication,['type','token'])||value.authentication.type!=='BEARER_TOKEN'||value.authentication.token!==s.secret)return invalid();headers.set('Authorization',`Bearer ${s.secret}`);
    }else if(value.authentication!==undefined)return invalid();
    const expectedBody=p==='algolia.browse-records'?(this.cursor?{cursor:this.cursor}:{}):this.plan.body;
    if(JSON.stringify(value.body)!==JSON.stringify(expectedBody))return invalid();
    this.requests++;
    return {url:url.toString(),headers,method:this.method,...(expectedBody?{body:JSON.stringify(expectedBody)}:{})};
  }
  bodyFailure(body:unknown):FailureKind|undefined{
    const p=this.plan.profile,e=this.plan.expected,bad='invalid_response';if(!record(body))return bad;
    if(p==='jira-cloud.get-issue')return typeof body.id==='string'&&typeof body.key==='string'&&record(body.fields)&&(/^[0-9]+$/.test(e.issueId as string)?body.id===e.issueId:body.key===e.issueId)?undefined:bad;
    if(p==='dropbox.get-file-metadata'){
      if(!['file','folder','deleted'].includes(body['.tag'] as string)||!nonempty(body.name,2048))return bad;
      if(body['.tag']==='deleted')return e.include_deleted===true?undefined:bad;
      if(!nonempty(body.id))return bad;
      if((e.path as string).startsWith('id:')&&body.id!==e.path)return bad;
      if((e.path as string).startsWith('rev:')&&(body['.tag']!=='file'||body.rev!==(e.path as string).slice(4)))return bad;
      return undefined;
    }
    if(p==='google-sheets.get-values'){
      if(typeof body.range!=='string'||body.majorDimension!==e.dimension||(body.values!==undefined&&!Array.isArray(body.values)))return bad;
      const split=body.range.lastIndexOf('!');if(split<0)return bad;
      let title=body.range.slice(0,split);if(title.startsWith("'")&&title.endsWith("'"))title=title.slice(1,-1).replace(/''/g,"'");
      if(title!==e.title||!matches(body.range.slice(split+1),a1Pattern))return bad;
      try {const bounds=a1Bounds(body.range.slice(split+1));for(const key of ['startRow','startCol','rows','cols'] as const)if(bounds[key]!==e[key])return bad;} catch {return bad;}
      const values=(body.values??[]) as unknown[],outer=e.dimension==='ROWS'?e.rows:e.cols,inner=e.dimension==='ROWS'?e.cols:e.rows;
      if(values.length>(outer as number))return 'response_too_large';
      for(const row of values){if(!Array.isArray(row))return bad;if(row.length>(inner as number))return 'response_too_large';if(!row.every(v=>typeof v==='string'||typeof v==='boolean'||typeof v==='number'&&Number.isFinite(v)))return bad;}
      return undefined;
    }
    if(p==='microsoft-excel-365.get-table-columns'){
      if(!Array.isArray(body.value)||!body.value.every(v=>record(v)&&typeof v.name==='string'))return bad;
      if(body.value.length>(e.limit as number))return 'response_too_large';
      if(body['@odata.nextLink']!=null&&body['@odata.nextLink']!=='')return bad;
      return undefined;
    }
    if(p==='microsoft-teams.get-channel-message')return body.id===e.id?undefined:bad;
    if(p==='quickbooks.find-invoice'){
      if(Object.hasOwn(body,'Fault')||!record(body.QueryResponse))return bad;
      const values=body.QueryResponse.Invoice;
      if(Object.keys(body.QueryResponse).some(k=>!['Invoice','startPosition','maxResults','totalCount'].includes(k)))return bad;
      for(const key of ['startPosition','maxResults','totalCount'])if(Object.hasOwn(body.QueryResponse,key)&&(!Number.isSafeInteger(body.QueryResponse[key])||(body.QueryResponse[key] as number)<0))return bad;
      if(values!==undefined&&(!Array.isArray(values)||values.length>1||!values.every(v=>record(v)&&nonempty(v.Id)&&v.DocNumber===e.invoiceNumber)))return bad;
      const count=values===undefined?0:(values as unknown[]).length;
      // MAXRESULTS 1 returns zero or one row. A larger aggregate total can
      // describe more matching invoices, but neither count can contradict
      // an empty result or a returned invoice.
      if(Object.hasOwn(body.QueryResponse,'maxResults')&&body.QueryResponse.maxResults!==count)return bad;
      if(Object.hasOwn(body.QueryResponse,'totalCount')&&((body.QueryResponse.totalCount===0)!==(count===0)))return bad;
      return undefined;
    }
    if(p==='xero.find-invoice'){
      if(Object.hasOwn(body,'ErrorNumber')||(body.Status!=null&&body.Status!=='OK')||!Array.isArray(body.Invoices)||!body.Invoices.every(v=>record(v)&&matches(v.InvoiceID,uuidPattern)&&(!e.type||v.Type===e.type)))return bad;
      return body.Invoices.length<=100?undefined:'response_too_large';
    }
    if(!Array.isArray(body.hits)||!body.hits.every(v=>record(v)&&nonempty(v.objectID))||(body.cursor!=null&&(typeof body.cursor!=='string'||(body.cursor!==''&&!text(body.cursor,4096)))))return bad;
    if(body.hits.length>1000)return 'response_too_large';
    this.hits+=body.hits.length;const next=body.cursor as string|undefined;
    if(next&&this.hits<(e.limit as number)){
      if(this.cursors.has(next))return bad;
      if(this.requests>=this.maxRequests)return 'response_too_large';
      this.cursors.add(next);this.cursor=next;
    }
    return undefined;
  }
}
