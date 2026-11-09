import test from 'node:test';
import assert from 'node:assert/strict';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { Invocation } from '../dist/transport.js';
import { select, inputs } from '../dist/profiles.js';
import { parseConfiguration, describeMcpConfiguration } from '../dist/mcp.js';
import { oauthProfiles } from './fixtures/oauth-profiles.mjs';
const CANARY = 'provider-private-fixture';
const response = value => new Response(JSON.stringify(value));
const safe = (kind, fields={}) => e => {
  assert.ok(e instanceof ActivepiecesError); assert.equal(e.kind, kind);
  for (const [k,v] of Object.entries(fields)) assert.equal(e[k],v);
  assert.equal(e.writeOutcome, 'not_applicable');
  assert.ok(!JSON.stringify(e).includes(CANARY)); assert.ok(!String(e).includes(CANARY));
  assert.equal(e.cause, undefined); return true;
};
for (const d of oauthProfiles) {
  test(`actual OAuth donor ${d.profile} preserves data and exact selected GET`, async () => {
    let calls=0, borrowed;
    const r=createReader({profile:d.profile,auth:d.auth('selected'),transport:async(url,init)=>{
      calls++; assert.equal(url,d.url); assert.equal(init.method,'GET'); assert.equal(init.redirect,'error');
      assert.equal(init.body,undefined); assert.deepEqual(Object.fromEntries(new Headers(init.headers)), {accept:'application/json',authorization:'Bearer selected'});
      return borrowed=response(d.body);
    }});
    assert.deepEqual(await r.invoke(d.args),d.result); assert.equal(calls,1);
    assert.equal(borrowed.body.locked,false); assert.equal(borrowed.bodyUsed,true);
    assert.equal(r.describe().effect,'read');
    const config={profile:d.profile,token_env:'SELECTED'};
    assert.deepEqual(parseConfiguration(config),config);
    assert.equal(describeMcpConfiguration(config).tools.length,1);
    assert.throws(()=>parseConfiguration({...config,origin:'https://other.invalid'}));
  });
  test(`OAuth donor ${d.profile} isolates overlapping selected accounts`,async()=>{
    let release, arrivals=0;
    const gate=new Promise(resolve=>{release=resolve;});
    const operations=['account-a','account-b'].map(account=>createReader({profile:d.profile,auth:d.auth(account),transport:async(url,init)=>{
      assert.equal(url,d.url); assert.equal(new Headers(init.headers).get('Authorization'),`Bearer ${account}`);
      arrivals++; if(arrivals===2)release(); await gate;
      return response(d.profile==='acuity.find-appointments'?[{account}]:{account});
    }}).invoke(d.args));
    const results=await Promise.all(operations);
    assert.deepEqual(results,['account-a','account-b'].map(account=>d.wrap(d.profile==='acuity.find-appointments'?[{account}]:{account})));
  });
  test(`OAuth donor ${d.profile} preserves failure metadata without retries`,async()=>{
    for(const status of [302,401,403,404,429,503]){
      let calls=0,closed=false;
      const body=new ReadableStream({cancel(){closed=true;}});
      const r=createReader({profile:d.profile,auth:d.auth('selected'),transport:async()=>{calls++;return new Response(body,{status});}});
      await assert.rejects(r.invoke(d.args),safe(status===302?'redirect_refused':'http_error',{entered:true,dispatched:true,...(status===302?{}:{status})}));
      assert.equal(calls,1);assert.equal(closed,true);assert.equal(body.locked,false);
    }
    const r=createReader({profile:d.profile,auth:d.auth('selected'),transport:async()=>{throw new Error(CANARY);}});
    await assert.rejects(r.invoke(d.args),safe('transport_error',{dispatched:true}));
  });
}

test('OAuth argument bounds and injected credentials refuse before transport',async()=>{
  const invalid={
    'google-contacts.search-contact':[
      {query:'',readMask:['names']},{query:'x\n',readMask:['names']},{query:'\ud800',readMask:['names']},
      {query:'x'.repeat(257),readMask:['names']},{query:'x',readMask:[]},{query:'x',readMask:['names','names']},
      {query:'x',readMask:['unsupported']},{query:'x',readMask:['names'],pageSize:true},{query:'x',readMask:['names'],pageSize:31},
    ],
    'figma.get-comments':[{file_key:'../file'},{file_key:'x?more=1'},{file_key:'x\n'}],
    'acuity.find-appointments':[
      {minDate:'2026-02-30'},{minDate:'2025-02-29'},{minDate:'2026-13-01'},{minDate:'2026-10-03T00:00:00Z'},
      {minDate:'2026-10-10',maxDate:'2026-10-03'},{minDate:'2026-10-03\n'},{maxResults:0},{maxResults:101},
      {maxResults:true},{calendarID:'12'},{status:'deleted'},{direction:'asc'},{phone:'x\u0000'},
    ],
  };
  for(const d of oauthProfiles){
    let calls=0;
    const r=createReader({profile:d.profile,auth:d.auth('selected'),transport:async()=>{calls++;return response(d.body);}});
    for(const args of [...invalid[d.profile],{...d.args,access_token:CANARY}])await assert.rejects(r.invoke(args),safe('invalid_arguments',{entered:false,dispatched:false}));
    assert.equal(calls,0);
  }
});

test('selected query descriptor rejects donor retargeting and altered filters',async()=>{
  for(const d of oauthProfiles.filter(d=>d.profile!=='figma.get-comments')){
    let calls=0;
    const s=select({profile:d.profile,auth:d.auth('selected'),transport:async()=>{calls++;return response(d.body);}});
    const prepared=inputs(s,d.args);
    const base={method:'GET',url:s.origin+prepared.pathname,authentication:{type:'BEARER_TOKEN',token:'selected'},queryParams:prepared.query};
    for(const changed of [undefined,{}, {...prepared.query,unexpected:CANARY}, {...prepared.query,[Object.keys(prepared.query)[0]]:CANARY}]){
      const call=new Invocation(s,prepared.pathname,undefined,prepared.query);
      try{await assert.rejects(call.request({...base,queryParams:changed}),safe('upstream_error',{dispatched:false}));}finally{call.finish();}
    }
    assert.equal(calls,0);
  }
});

test('Acuity defaults are bounded, date-only leap days preserved and malformed result refused',async()=>{
  let seen=[];
  const r=createReader({profile:'acuity.find-appointments',auth:{accessToken:'selected'},transport:async url=>{seen.push(url);return response([]);}});
  assert.deepEqual(await r.invoke({calendarID:12}),{found:false,data:[]});
  assert.equal(seen[0],'https://acuityscheduling.com/api/v1/appointments?calendarID=12&max=100&direction=DESC');
  await r.invoke({calendarID:12,minDate:'2024-02-29',status:'canceled'});
  assert.equal(new URL(seen[1]).searchParams.get('minDate'),'2024-02-29');
  assert.equal(new URL(seen[1]).searchParams.get('canceled'),'true');
  for(const body of ['wrong',[false],[{},{}]]){
    const malformed=createReader({profile:'acuity.find-appointments',auth:{accessToken:'selected'},transport:async()=>response(body)});
    await assert.rejects(malformed.invoke({calendarID:12,maxResults:1}),safe('invalid_response'));
  }
});

test('Google Contacts default page size and Unicode query preserve exact values',async()=>{
  const r=createReader({profile:'google-contacts.search-contact',auth:{accessToken:'selected'},transport:async url=>{
    const qs=new URL(url).searchParams;assert.equal(qs.get('query'),'\u{1f600}'.repeat(256));assert.equal(qs.get('pageSize'),'30');
    return response({results:[]});
  }});
  assert.deepEqual(await r.invoke({query:'\u{1f600}'.repeat(256),readMask:['names']}),{results:[]});
});
