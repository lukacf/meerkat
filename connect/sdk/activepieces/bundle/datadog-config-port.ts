import { client } from '@datadog/datadog-api-client';
declare const __toolkitTransport: (request:unknown)=>Promise<{status:number;headers:Record<string,string>;body:unknown}>;
/** Declared helper port: explicit Configuration avoids createConfiguration's ambient keys and shared site mutation. */
export function getDatadogConfiguration(auth:{props:{apiKey:string;site:string}}){
 const httpApi={send:async(request:any)=>{
  const response=await __toolkitTransport({method:request.getHttpMethod(),url:request.getUrl(),headers:Object.fromEntries(new Headers(request.getHeaders())),body:request.getBody()});
  return new client.ResponseContext(response.status,{'content-type':'application/json',...response.headers},{text:async()=>JSON.stringify(response.body),binary:async()=>Buffer.from(JSON.stringify(response.body)??'')});
 }};
 const config=new client.Configuration(undefined,0,{},httpApi,{apiKeyAuth:new client.ApiKeyAuthAuthentication(auth.props.apiKey)},{},false,false,0,2,2,{});
 config.setServerVariables({site:auth.props.site});return config;
}
