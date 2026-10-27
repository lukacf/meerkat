import test from 'node:test';
import assert from 'node:assert/strict';
import {JSDOM} from 'jsdom';
import {mountToolkitView} from '@rkat/toolkit/ui';
import {fixture} from '../examples/management-fixture.mjs';
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function mount(t){
  const dom=new JSDOM('<main></main>'),host=dom.window.document.querySelector('main'),calls=[];
  const view=mountToolkitView(host,{scope:'fixture-viewer',dispatch:async request=>{calls.push(request)}});
  view.update(fixture());t.after(()=>{view.dispose();dom.window.close()});
  return {host,calls,view,doc:dom.window.document};
}
const locations={
  root:s=>s, task:s=>s.tasks[0], phase:s=>s.tasks[0].phase,
  service:s=>s.tasks[0].roles.find(r=>r.kind==='service'),
  frontend:s=>s.frontends[0], connection:s=>s.connections[0],
  grant:s=>s.connections[0].grants[0], package:s=>s.packages[0],
  exported:s=>s.packages[0].exports[0], component:s=>s.packages[0].components[0],
  action:s=>s.tasks[0].userActions[0], control:s=>s.tasks[0].userActions[0].controls[0],
  target:s=>s.tasks[0].userActions[0].controls[0].target,
};
for(const [label,select] of Object.entries(locations)){
  test(`native attempt extras on ${label} are refused before DOM or dispatch and keep the last good revision`,async t=>{
    const ui=mount(t),html=ui.host.innerHTML;
    for(const name of ['authorizationUrl','authorize_url','displayedAuthorizationUrl','code','callback','pkce_verifier']){
      const s=fixture(2);select(s)[name]='SYNTHETIC_AUTH_MATERIAL';
      assert.throws(()=>ui.view.update(s),e=>{assert.ok(e instanceof TypeError);assert.ok(!e.message.includes('SYNTHETIC_AUTH_MATERIAL'));return true});
      assert.equal(ui.host.innerHTML,html);assert.ok(!ui.host.textContent.includes('SYNTHETIC_AUTH_MATERIAL'));
    }
    assert.equal(ui.view.update(fixture(2)),true);await tick();assert.deepEqual(ui.calls,[]);
  });
}
test('reference-only sign-in works without an origin, never advances auth state, and includes safe account context when present',async t=>{
  const ui=mount(t);const s=fixture(2);delete s.tasks[0].userActions[0].authorizationOrigin;
  ui.view.update(s);const button=[...ui.doc.querySelectorAll('button')].find(b=>b.textContent==='Review sign-in');
  const accessible=button.getAttribute('aria-labelledby').split(' ').map(id=>ui.doc.getElementById(id).textContent).join(' ');
  assert.ok(accessible.includes('Requested account: Work account'));assert.equal(button.disabled,false);
  button.click();button.click();await tick();assert.equal(ui.calls.length,1);
  assert.deepEqual(Object.keys(ui.calls[0]).sort(),['control','scope','viewRevision']);
  assert.deepEqual(ui.calls[0].control,s.tasks[0].userActions[0].controls[0]);
  assert.ok(ui.host.textContent.includes('Action needed'));assert.ok(!JSON.stringify(ui.calls[0]).includes('https://'));
});
test('a requested-account change cannot alias another displayed occurrence of the same sign-in offer',t=>{
  const ui=mount(t),before=ui.host.innerHTML,s=fixture(2),copy=structuredClone(s.tasks[0].userActions[0]);
  copy.id='second-view';copy.requestedAccountLabel='Other account';s.tasks[0].userActions.push(copy);
  assert.throws(()=>ui.view.update(s),TypeError);assert.equal(ui.host.innerHTML,before);assert.deepEqual(ui.calls,[]);
});
test('an omitted host control creates no sign-in action and disposal cannot cancel an owned ceremony',async t=>{
  const ui=mount(t),s=fixture(2);s.tasks[0].userActions[0].controls=[];
  s.tasks[0].userActions[0].instructions='Protected sign-in is not available in this host.';
  ui.view.update(s);assert.equal([...ui.doc.querySelectorAll('button')].filter(b=>b.textContent==='Review sign-in').length,0);
  ui.view.dispose();await tick();assert.deepEqual(ui.calls,[]);
});

test('the same sign-in offer without optional labels may appear in a task card and action row',async t=>{
  const ui=mount(t),s=fixture(2),action=s.tasks[0].userActions[0];
  delete action.authorizationOrigin;delete action.requestedAccountLabel;
  s.tasks[0].controls.push(structuredClone(action.controls[0]));
  assert.equal(ui.view.update(s),true);
  const buttons=[...ui.doc.querySelectorAll('button')].filter(b=>b.textContent==='Review sign-in');
  assert.equal(buttons.length,2);assert.ok(buttons.every(b=>!b.disabled));
  buttons[1].click();buttons[0].click();await tick();
  assert.equal(ui.calls.length,1);assert.deepEqual(ui.calls[0].control,action.controls[0]);
  assert.ok(buttons.every(b=>b.disabled));
});
