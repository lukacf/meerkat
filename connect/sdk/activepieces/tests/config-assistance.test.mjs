import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp, readFile, writeFile, rm} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';
import {listProfiles, configurationHelp} from '../dist/config-fields.js';
import {parseConfiguration} from '../dist/mcp-config.js';
import {configurations} from './fixtures/config-assistance.mjs';
import {cases} from './effects-fixtures.mjs';
const CANARY='unselected-account-value-must-not-appear';

const guard = `
import {registerHooks} from 'node:module';
import {basename} from 'node:path';
registerHooks({resolve(specifier,context,nextResolve){
  if (['mcp.js','mcp-http.js','index.js','donor-factory.js','transport.js'].includes(basename(specifier)) ||
      (!specifier.startsWith('.') && !specifier.startsWith('/') && !specifier.startsWith('file:') && !specifier.startsWith('node:')))
    throw new Error('Runtime import forbidden');
  return nextResolve(specifier,context);
}});
process.env=new Proxy({}, {get(_target,key){
  if (['FORCE_COLOR','NO_COLOR','NODE_DISABLE_COLORS','NODE_V8_COVERAGE','WATCH_REPORT_DEPENDENCIES'].includes(key)) return undefined;
  throw new Error('Environment read forbidden');
}});
globalThis.fetch=()=>{throw new Error('Network forbidden');};
`;

test('every existing profile fixture agrees with help field requirements',()=>{
  assert.equal(configurations.length,63);
  assert.deepEqual(configurations.map(x=>x.profile).sort(),listProfiles().profiles.map(x=>x.profile).sort());
  for(const original of configurations){
    assert.deepEqual(parseConfiguration(original),original);
    const help=configurationHelp(original.profile);
    assert.ok(help.scope.includes('not a complete JSON Schema'));
    assert.equal(help.effect, cases.some(item=>item.profile===original.profile&&item.body)?'write':'read');
    const fields=new Map(help.fields.map(field=>[field.name,field]));
    for(const name of Object.keys(original))assert.ok(fields.has(name),`${original.profile}: ${name}`);
    for(const field of fields.values()){
      const changed={...original};delete changed[field.name];
      if(field.required)assert.throws(()=>parseConfiguration(changed),`${original.profile}: missing ${field.name}`);
      else assert.doesNotThrow(()=>parseConfiguration(changed));
      if(field.credential_reference)assert.equal(field.kind,'string');
    }
  }
});

test('help and list results cannot mutate subsequent calls',()=>{
  const help=configurationHelp('amazon-s3.list-files');help.fields.length=0;help.operations.length=0;
  const listing=listProfiles();listing.profiles.length=0;
  assert.equal(listProfiles().profiles.length,63);
  assert.ok(configurationHelp('amazon-s3.list-files').fields.some(field=>field.name==='access_key_id_env'&&field.required&&field.credential_reference));
});

test('public inert commands refuse runtime imports and credential reads for all profiles',async()=>{
  const directory=await mkdtemp(join(tmpdir(),'toolkit-config-help-'));
  const hook=join(directory,'guard.mjs'),file=join(directory,'config.json');
  await writeFile(hook,guard);
  const run=args=>spawnSync(process.execPath,['--import',pathToFileURL(hook).href,'dist/mcp-cli.js',...args],
    {cwd:new URL('..',import.meta.url),env:{LANG:'C.UTF-8'},encoding:'utf8',timeout:10000});
  try{
    const listing=run(['profiles']);assert.equal(listing.status,0,listing.stderr);assert.deepEqual(JSON.parse(listing.stdout),listProfiles());
    for(const value of configurations){
      await writeFile(file,JSON.stringify(value));const before=await readFile(file);
      for(const args of [['config-help','--profile',value.profile],['check-config','--config',file]]){
        const result=run(args);assert.equal(result.status,0,result.stderr);assert.equal(result.stderr,'');
        assert.equal(JSON.parse(result.stdout).profile,value.profile);
      }
      assert.deepEqual(await readFile(file),before);
    }
  }finally{await rm(directory,{recursive:true});}
});

test('invalid public inputs have no stdout or submitted-value diagnostics',async()=>{
  const directory=await mkdtemp(join(tmpdir(),'toolkit-config-invalid-')),file=join(directory,'config.json');
  const run=args=>spawnSync(process.execPath,['dist/mcp-cli.js',...args],
    {cwd:new URL('..',import.meta.url),env:{LANG:'C.UTF-8'},encoding:'utf8',timeout:10000});
  try{
    const args=[['config-help','--profile',CANARY],['config-help','--profile','Tally'],['profiles',CANARY],
      ['check-config','--profile',CANARY],['check-config','--config',join(directory,CANARY)]];
    for(const value of [CANARY, '[]', '{}', JSON.stringify({profile:'tally.get-form',token_env:CANARY}),
                       JSON.stringify({profile:'tally.get-form',token_env:'SELECTED_TOKEN',secret:CANARY})]){
      await writeFile(file,value);
      const result=run(['check-config','--config',file]);assert.equal(result.status,1);assert.equal(result.stdout,'');assert.ok(!result.stderr.includes(CANARY));
    }
    for(const value of args){const result=run(value);assert.equal(result.status,1);assert.equal(result.stdout,'');assert.ok(!result.stderr.includes(CANARY));}
  }finally{await rm(directory,{recursive:true});}
});
