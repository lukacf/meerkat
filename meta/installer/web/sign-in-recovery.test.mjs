import test from "node:test";
import assert from "node:assert/strict";
import {hostSignInPath, isSignInRequired} from "./sign-in-recovery.mjs";
const documentFor = value => ({location:{href:"https://forge.example/app"},querySelector:()=>({getAttribute:()=>value})});
test("only typed authentication failures request sign-in",()=>{
  assert.equal(isSignInRequired({status:401}),true);
  assert.equal(isSignInRequired({status:403,code:"sign_in_required"}),true);
  for(const value of [{status:403},{status:500},{message:"Current sign-in required"},null]) assert.equal(isSignInRequired(value),false);
});
test("host sign-in hint stays strictly same-origin and never guesses a host route",()=>{
  assert.equal(hostSignInPath(documentFor("/host/login?return=%2Fapp")),"/host/login?return=%2Fapp");
  for(const path of [null,"", "//outside.example/login","https://outside.example/login","javascript:alert(1)","/\\outside.example/login","/host/login#secret"," /host/login","/host/login\n"])
    assert.equal(hostSignInPath(documentFor(path)),null,path);
  assert.equal(hostSignInPath(undefined),null);
});
