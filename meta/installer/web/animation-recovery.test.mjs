import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fingerprint } from './state.mjs';
import { readAnimationRecovery, readAnimationSelection, saveAnimationRecovery, forgetAnimationRecovery, recoveredAnimation,
  recoveredAnimationSelection } from './animation-recovery.mjs';

const scope={setup_id:'owned-guide',stream_id:'owned-guide:__overview__'};
const storage=()=>{
  const entries=new Map();
  return {entries,getItem:key=>entries.get(key) ?? null,setItem:(key,value)=>entries.set(key,value),removeItem:key=>entries.delete(key)};
};

async function fixture() {
  const plan={project:{name:'Saved proposal'},agent_classes:[{id:'owner',instances:610}],customDesign:{shape:'Free composition'}};
  const model={nodes:[{id:'actual-owner'}],design_context:{facts:{ownership:{summary:'Historical ownership'}}},customPresentation:{anything:'Preserved'}};
  const [blueprint_hash,model_hash]=await Promise.all([fingerprint(plan),fingerprint(model)]);
  const code='return customProgram; // opaque bytes, never evaluated';
  const code_hash=[...new Uint8Array(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(code)))].map(value=>value.toString(16).padStart(2,'0')).join('');
  const native_admission={interaction_id:'owned-native-interaction',cursor:'console:4'};
  const revision_binding={version:'prior-program-v1',...scope,context_hash:'f'.repeat(64)};
  const publication_receipt={authority:'publish_visualization',request_key:'b'.repeat(64),blueprint_hash,model_hash,
    reference_hash:'c'.repeat(64),code_hash,revision_binding};
  const artifact={contract:'freeform-animation-v3',key:'b'.repeat(64),blueprint_hash,model_hash,reference_hash:'c'.repeat(64),code_hash,code,
    native_admission:{...native_admission},revision_binding,publication_receipt};
  const receipt={id:'a'.repeat(64),...scope,status:'completed',source_generation:1,request_fingerprint:'d'.repeat(64),
    native_request_key:'b'.repeat(64),blueprint_hash,model_hash,native_admission,revision_binding,artifact};
  const candidate={plan,model,streamId:scope.stream_id,receipt,artifact};
  const saved=storage();assert.equal(saveAnimationRecovery(saved,scope,candidate),true);
  return {candidate,receipt,saved,snapshot:readAnimationRecovery(saved,scope)};
}

test('session record contains only exact inputs and owned scope/reference, never animation code or artifact evidence',async()=>{
  const {candidate,saved,snapshot}=await fixture();
  const raw=[...saved.entries.values()][0];
  assert.deepEqual(Object.keys(snapshot).sort(),['blueprint','model','operation_id','setup_id','stream_id','version']);
  assert.equal(raw.includes(candidate.artifact.code),false);
  assert.equal(raw.includes('publication_receipt'),false);
  assert.deepEqual(snapshot.blueprint,candidate.plan);
  candidate.plan.project.name='Changed after saving';
  assert.equal(snapshot.blueprint.project.name,'Saved proposal');
  assert.equal(readAnimationRecovery(saved,{...scope,setup_id:'foreign-guide'}),null);
  assert.equal(readAnimationRecovery(saved,{...scope,stream_id:'owned-guide:other-view'}),null);
});

test('an explicitly adopted scene stores only its exact source reference and can clear only that selection',async()=>{
  const {candidate,saved}=await fixture();candidate.adopted=true;candidate.feedback='Never persist imagined author feedback';
  assert.equal(saveAnimationRecovery(saved,scope,candidate),true);
  const value=readAnimationSelection(saved,scope),raw=[...saved.entries.values()][0];
  assert.deepEqual(value,{version:1,selection:'saved',...scope,operation_id:candidate.receipt.id,
    blueprint_hash:candidate.receipt.blueprint_hash,model_hash:candidate.receipt.model_hash,code_hash:candidate.artifact.code_hash});
  assert.ok(raw.length<1000);assert.equal(raw.includes(candidate.feedback),false);assert.equal(raw.includes(candidate.artifact.code),false);
  assert.equal(readAnimationRecovery(saved,scope),null);assert.equal(readAnimationSelection(saved,{...scope,stream_id:'other'}),null);
  assert.equal(forgetAnimationRecovery(saved,scope,'e'.repeat(64)),false);
  assert.equal(forgetAnimationRecovery(saved,scope,candidate.receipt.id),true);assert.equal(readAnimationSelection(saved,scope),null);
});

test('an automatically resumed publication is historical recovery, not an explicit saved selection',async()=>{
  const {candidate,saved}=await fixture();candidate.adopted=true;candidate.resumed=true;
  assert.equal(saveAnimationRecovery(saved,scope,candidate),true);
  assert.equal(readAnimationSelection(saved,scope),null);
  const snapshot=readAnimationRecovery(saved,scope);
  assert.equal(snapshot.operation_id,candidate.receipt.id);
  assert.deepEqual(snapshot.blueprint,candidate.plan);assert.deepEqual(snapshot.model,candidate.model);
  assert.equal([...saved.entries.values()][0].includes(candidate.artifact.code),false);
});

test('an exact selected reference recovers checked current inputs without changing its source or request',async()=>{
  const {candidate,receipt,saved}=await fixture();candidate.adopted=true;
  assert.equal(saveAnimationRecovery(saved,scope,candidate),true);
  const selection=readAnimationSelection(saved,scope);
  const before=structuredClone({selection,receipt,plan:candidate.plan,model:candidate.model});
  const recovered=await recoveredAnimationSelection(selection,receipt,scope,candidate.plan,candidate.model);
  assert.ok(recovered);
  assert.equal(recovered.receipt.id,selection.operation_id);
  assert.equal(recovered.artifact.code_hash,selection.code_hash);
  assert.deepEqual(recovered.plan,candidate.plan);assert.deepEqual(recovered.model,candidate.model);
  recovered.plan.project.name='Detached selection';recovered.artifact.code='Detached source';
  assert.deepEqual({selection,receipt,plan:candidate.plan,model:candidate.model},before);
  assert.deepEqual(readAnimationSelection(saved,scope),selection);
});

test('selected recovery refuses changed inputs, model, scope and exact source references',async()=>{
  const {candidate,receipt,saved}=await fixture();candidate.adopted=true;
  assert.equal(saveAnimationRecovery(saved,scope,candidate),true);
  const selection=readAnimationSelection(saved,scope);
  const alteredPlan=structuredClone(candidate.plan);alteredPlan.project.name='Changed proposal';
  const alteredModel=structuredClone(candidate.model);alteredModel.nodes[0].id='Changed identity';
  assert.equal(await recoveredAnimationSelection(selection,receipt,scope,alteredPlan,candidate.model),null);
  assert.equal(await recoveredAnimationSelection(selection,receipt,scope,candidate.plan,alteredModel),null);
  for(const damage of [value=>{value.operation_id='e'.repeat(64);},value=>{value.code_hash='e'.repeat(64);},
    value=>{value.blueprint_hash='e'.repeat(64);},value=>{value.model_hash='e'.repeat(64);},
    value=>{value.setup_id='another-guide';},value=>{value.stream_id='another-view';},
    value=>{value.selection='generated';},value=>{value.code='Untrusted source';}]){
    const changed=structuredClone(selection);damage(changed);
    assert.equal(await recoveredAnimationSelection(changed,receipt,scope,candidate.plan,candidate.model),null);
  }
  assert.equal(await recoveredAnimationSelection(selection,receipt,{...scope,setup_id:'another-guide'},candidate.plan,candidate.model),null);
  assert.equal(await recoveredAnimationSelection(selection,receipt,{...scope,stream_id:'another-view'},candidate.plan,candidate.model),null);
});

test('selected recovery refuses unavailable or damaged host admission and publication evidence',async()=>{
  const {candidate,receipt,saved}=await fixture();candidate.adopted=true;
  assert.equal(saveAnimationRecovery(saved,scope,candidate),true);
  const selection=readAnimationSelection(saved,scope);
  assert.equal(await recoveredAnimationSelection(selection,null,scope,candidate.plan,candidate.model),null);
  for(const damage of [value=>{value.status='failed';},value=>{value.status='queued';},
    value=>{value.native_admission=null;},value=>{value.artifact.native_admission.interaction_id='other-native';},
    value=>{value.artifact.code+='Changed raw bytes';},value=>{value.artifact.publication_receipt.authority='browser';},
    value=>{value.artifact.publication_receipt.code_hash='e'.repeat(64);}]){
    const changed=structuredClone(receipt);damage(changed);
    assert.equal(await recoveredAnimationSelection(selection,changed,scope,candidate.plan,candidate.model),null);
  }
});

test('forgetting a known execution-failed selected source removes its automatic recovery reference',async()=>{
  const {candidate,receipt,saved}=await fixture();candidate.adopted=true;
  assert.equal(saveAnimationRecovery(saved,scope,candidate),true);
  assert.equal(forgetAnimationRecovery(saved,scope,receipt.id),true);
  assert.equal(readAnimationSelection(saved,scope),null);
  assert.equal(await recoveredAnimationSelection(readAnimationSelection(saved,scope),receipt,scope,candidate.plan,candidate.model),null);
});

test('only a matching host publication recovers exact historical inputs without imposing a composition or roster schema',async()=>{
  const {receipt,snapshot}=await fixture();
  const before=structuredClone({receipt,snapshot});
  const recovered=await recoveredAnimation(snapshot,receipt,scope);
  assert.ok(recovered);
  assert.deepEqual(recovered.plan,snapshot.blueprint);
  assert.equal(recovered.plan.agent_classes[0].instances,610);
  assert.deepEqual(recovered.model.customPresentation,{anything:'Preserved'});
  recovered.plan.project.name='Detached result';recovered.receipt.artifact.code='Changed detached code';
  assert.deepEqual({receipt,snapshot},before);
});

test('source, scope, publication, code and native admission damage cannot recover a scene',async()=>{
  const {receipt,snapshot}=await fixture();
  const damages=[
    value=>{value.setup_id='foreign';},value=>{value.stream_id='foreign-view';},
    value=>{value.id='e'.repeat(64);},value=>{value.status='queued';},value=>{value.status='failed';},
    value=>{value.source_generation=0;},value=>{value.native_request_key='e'.repeat(64);},
    value=>{value.blueprint_hash='e'.repeat(64);},value=>{value.model_hash='e'.repeat(64);},
    value=>{value.native_admission=null;},value=>{value.native_admission.cursor='unknown';},
    value=>{value.artifact.contract='old-contract';},value=>{value.artifact.code+=' changed bytes';},
    value=>{value.artifact.publication_receipt.authority='browser';},
    value=>{value.artifact.publication_receipt.request_key='e'.repeat(64);},
    value=>{value.artifact.publication_receipt.code_hash='e'.repeat(64);},
    value=>{value.artifact.native_admission.interaction_id='other-native-interaction';},
    value=>{value.revision_binding.setup_id='foreign';},
  ];
  for(const damage of damages){const changed=structuredClone(receipt);damage(changed);assert.equal(await recoveredAnimation(snapshot,changed,scope),null);}
  const altered=structuredClone(snapshot);altered.model.nodes[0].id='invented';
  assert.equal(await recoveredAnimation(altered,receipt,scope),null);
  altered.model=structuredClone(snapshot.model);altered.blueprint.project.name='Invented current desire';
  assert.equal(await recoveredAnimation(altered,receipt,scope),null);
});

test('equivalent native binding field order is accepted while different optional bindings are rejected',async()=>{
  const {receipt,snapshot}=await fixture();
  receipt.artifact.native_admission={cursor:'console:4',interaction_id:'owned-native-interaction'};
  assert.ok(await recoveredAnimation(snapshot,receipt,scope));
  receipt.artifact.native_admission.conversation_id='unrelated';
  assert.equal(await recoveredAnimation(snapshot,receipt,scope),null);
});

test('legacy publications without native admission are intentionally not recovered',async()=>{
  const {receipt,snapshot}=await fixture();
  delete receipt.native_admission;delete receipt.artifact.native_admission;
  assert.equal(await recoveredAnimation(snapshot,receipt,scope),null);
});

test('recovery respects the current contract code length and checks raw code bytes at its boundaries',async()=>{
  const {receipt,snapshot}=await fixture();
  for(const length of [19,20,48000,48001]){
    const changed=structuredClone(receipt);changed.artifact.code='x'.repeat(length);
    const codeHash=[...new Uint8Array(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(changed.artifact.code)))].map(value=>value.toString(16).padStart(2,'0')).join('');
    changed.artifact.code_hash=codeHash;changed.artifact.publication_receipt.code_hash=codeHash;
    assert.equal(!!await recoveredAnimation(snapshot,changed,scope),length>=20 && length<=48000);
  }
});

test('denied, oversized, malformed or expanded storage stays optional and cannot carry executable artifacts',async()=>{
  const {candidate,saved}=await fixture();
  const name=[...saved.entries.keys()][0];
  for(const raw of ['{broken',JSON.stringify({artifact:{code:'untrusted'}}),' '.repeat(8_000_001)]){
    saved.entries.set(name,raw);assert.equal(readAnimationRecovery(saved,scope),null);
  }
  const denied={getItem(){throw new Error('denied');},setItem(){throw new Error('quota');}};
  assert.equal(saveAnimationRecovery(denied,scope,candidate),false);
  assert.equal(readAnimationRecovery(denied,scope),null);
  assert.equal(saveAnimationRecovery(null,scope,candidate),false);
  assert.equal(saveAnimationRecovery(saved,{setup_id:null,stream_id:scope.stream_id},candidate),false);
  candidate.model={large:'x'.repeat(8_000_001)};
  assert.equal(saveAnimationRecovery(saved,scope,candidate),false);
});

test('a failed recovered scene can clear only its own pointer, preserving a newer displayed scene',async()=>{
  const {candidate,saved}=await fixture();
  candidate.receipt.id='e'.repeat(64);
  assert.equal(saveAnimationRecovery(saved,scope,candidate),true);
  assert.equal(forgetAnimationRecovery(saved,scope,'a'.repeat(64)),false);
  assert.equal(readAnimationRecovery(saved,scope).operation_id,'e'.repeat(64));
  assert.equal(forgetAnimationRecovery(saved,scope,'e'.repeat(64)),true);
  assert.equal(readAnimationRecovery(saved,scope),null);
});
