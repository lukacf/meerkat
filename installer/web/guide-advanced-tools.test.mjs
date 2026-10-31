import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import React, { act } from 'react';
import { loadTestModule } from './test-support/bundle.mjs';
const require=createRequire(import.meta.url);
const {JSDOM}=createRequire(new URL('../../console/package.json',import.meta.url))('jsdom');
const {GuideAdvancedTools}=await loadTestModule('guide-advanced-tools.jsx');

test('saved tool entry needs this guide capability; late reads cannot enable another guide',async()=>{
  const dom=new JSDOM('<div id="root"></div>'),calls=[],pending=[];
  const values={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,
    IS_REACT_ACT_ENVIRONMENT:true,fetch:async(url,options={})=>{
      calls.push({url,method:options.method||'GET'});
      return new Promise(resolve=>pending.push(value=>resolve({ok:true,json:async()=>value})));
    }};
  const previous=new Map(Object.keys(values).map(k=>[k,Object.getOwnPropertyDescriptor(globalThis,k)]));
  for(const [k,v] of Object.entries(values))Object.defineProperty(globalThis,k,{configurable:true,value:v});
  const root=require('react-dom/client').createRoot(document.getElementById('root'));
  const render=id=>act(async()=>root.render(React.createElement(GuideAdvancedTools,{guideId:id,active:true,onSavedTools(){assert.fail('read must not import');}})));
  try{
    await render('a');assert.doesNotMatch(document.body.textContent,/Import saved tool/);
    await render('b');await act(async()=>pending[0]({available:true}));
    assert.doesNotMatch(document.body.textContent,/Import saved tool/);
    await act(async()=>pending[1]({available:false}));assert.doesNotMatch(document.body.textContent,/Import saved tool/);
    await render('c');await act(async()=>pending[2]({available:true}));assert.match(document.body.textContent,/Import saved tool/);
    const disclosure=document.querySelector('details');disclosure.open=true;
    await act(async()=>disclosure.dispatchEvent(new dom.window.KeyboardEvent('keydown',{key:'Escape',bubbles:true})));
    assert.equal(disclosure.open,false);assert.equal(document.activeElement,document.querySelector('summary'));
    assert.ok(calls.every(call=>call.method==='GET'));
  }finally{
    await act(async()=>root.unmount());dom.window.close();
    for(const [k,d] of previous){if(d)Object.defineProperty(globalThis,k,d);else delete globalThis[k];}
  }
});
