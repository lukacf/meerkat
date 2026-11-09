import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp, readFile, writeFile, rm} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';
import {configurationHelp} from '../dist/config-fields.js';
import {configurations} from './fixtures/config-assistance.mjs';

const CANARY='synthetic-credential-value-must-not-appear';
const guard=`
import {registerHooks} from 'node:module';
import {basename} from 'node:path';
registerHooks({resolve(specifier,context,nextResolve){
  if (['mcp.js','mcp-http.js','index.js','donor-factory.js','transport.js'].includes(basename(specifier)) ||
      (!specifier.startsWith('.') && !specifier.startsWith('/') && !specifier.startsWith('file:') && !specifier.startsWith('node:')))
    throw new Error('Runtime import forbidden');
  return nextResolve(specifier,context);
}});
process.env=new Proxy({SELECTED_TOKEN:${JSON.stringify(CANARY)}}, {
  get(_target,key){
    if (['FORCE_COLOR','NO_COLOR','NODE_DISABLE_COLORS','NODE_V8_COVERAGE','WATCH_REPORT_DEPENDENCIES'].includes(key)) return undefined;
    throw new Error('Environment read forbidden');
  },
  has(){throw new Error('Environment lookup forbidden');},
  ownKeys(){throw new Error('Environment iteration forbidden');}
});
globalThis.fetch=()=>{throw new Error('Network forbidden');};
`;

async function commands(callback){
  const directory=await mkdtemp(join(tmpdir(),'toolkit-credential-references-'));
  const hook=join(directory,'guard.mjs'),file=join(directory,'config.json');
  await writeFile(hook,guard);
  const run=async(value,expected=0)=>{
    const bytes=JSON.stringify(value);
    await writeFile(file,bytes);
    const result=spawnSync(process.execPath,['--import',pathToFileURL(hook).href,'dist/mcp-cli.js','check-config','--config',file],
      {cwd:new URL('..',import.meta.url),env:{LANG:'C.UTF-8'},encoding:'utf8',timeout:10000});
    assert.equal(result.status,expected,result.stderr);
    assert.equal(await readFile(file,'utf8'),bytes);
    assert.ok(!(result.stdout+result.stderr).includes(CANARY));
    if(expected){assert.equal(result.stdout,'');return;}
    assert.equal(result.stderr,'');
    const parsed=JSON.parse(result.stdout);
    assert.deepEqual(Object.keys(parsed).sort(),['credential_environment_references','profile','scope','valid']);
    assert.equal(parsed.profile,value.profile);assert.equal(parsed.valid,true);assert.equal(parsed.scope,'configuration_only');
    return parsed.credential_environment_references;
  };
  try {await callback(run);} finally {await rm(directory,{recursive:true});}
}
const row=(field,value)=>({field_path:[field],environment_variable:value});

test('all63 validated profiles project only declared present fields without credential or runtime reads',async()=>{
  assert.equal(configurations.length,63);
  await commands(async run=>{
    for(const value of configurations){
      const expected=configurationHelp(value.profile).fields.filter(field=>field.credential_reference&&Object.hasOwn(value,field.name))
        .map(field=>row(field.name,value[field.name]));
      assert.deepEqual(await run(value),expected,value.profile);
    }
  });
});

test('optional AWS references and aliases preserve individual exact paths',async()=>{
  const base={profile:'amazon-s3.list-files',token_env:'SELECTED_SECRET',access_key_id_env:'SELECTED_ACCESS',aws_region:'us-east-1',bucket:'selected-bucket'};
  await commands(async run=>{
    assert.deepEqual(await run(base),[row('token_env','SELECTED_SECRET'),row('access_key_id_env','SELECTED_ACCESS')]);
    assert.deepEqual(await run({...base,session_token_env:'SELECTED_SESSION'}),
      [row('token_env','SELECTED_SECRET'),row('access_key_id_env','SELECTED_ACCESS'),row('session_token_env','SELECTED_SESSION')]);
    // Describing PATH does not grant permission to inject or overwrite it.
    assert.deepEqual(await run({...base,token_env:'PATH',access_key_id_env:'PATH',session_token_env:'PATH'}),
      [row('token_env','PATH'),row('access_key_id_env','PATH'),row('session_token_env','PATH')]);
  });
});

test('separate accounts remain separate and ordinary fields are not credential references',async()=>{
  await commands(async run=>{
    for(const name of ['ACCOUNT_A','ACCOUNT_B','ACCOUNT_A']){
      const value={profile:'contentful.get-entry',token_env:name,space:'selected',environment:'ENV_NAME'};
      assert.deepEqual(await run(value),[row('token_env',name)]);
    }
  });
});

test('invalid configuration never produces a projection or echoes supplied values',async()=>{
  await commands(async run=>{
    for(const value of [
      {profile:'tally.get-form',token_env:CANARY},
      {profile:'tally.get-form',token_env:'SELECTED_TOKEN',timeout_ms:0},
      {profile:'tally.get-form',token_env:'SELECTED_TOKEN',extra:CANARY},
      {profile:'amazon-s3.list-files',token_env:'SELECTED_TOKEN',access_key_id_env:'SELECTED_ACCESS',aws_region:'us-east-1',bucket:'selected-bucket',session_token_env:CANARY},
    ]) await run(value,1);
  });
});
