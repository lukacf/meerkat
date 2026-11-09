import { WebClient as ActualWebClient } from '../node_modules/@slack/web-api/dist/index.js';
declare const __toolkitTransport: (request:unknown)=>Promise<{status:number;headers:Record<string,string>;body:unknown}>;
const logger={debug(){},info(){},warn(){},error(){},setLevel(){},getLevel(){return 'ERROR' as any;},setName(){}};
/** Keep the real WebClient and its serializer; replace only its per-client adapter. */
export class WebClient extends ActualWebClient {
 constructor(token?:string){super(token,{retryConfig:{retries:0},rejectRateLimitedCalls:true,allowAbsoluteUrls:false,attachOriginalToWebAPIRequestError:false,logger,
  adapter:async config=>{
   if(config.baseURL!=='https://slack.com/api/'||config.url!=='conversations.replies'||config.method!=='post'||config.proxy!==false||config.maxRedirects!==0)throw new Error('Unsupported selected Slack request');
   const response=await __toolkitTransport({method:'POST',url:config.baseURL+config.url,headers:Object.fromEntries(new Headers(config.headers.toJSON() as Record<string,string>)),body:config.data});
   return {config,request:{path:'/api/conversations.replies'},status:response.status,statusText:'',headers:response.headers,data:response.body};
  }});}
}
