"""Actual SDK/child-pipe controls, without native provider or credential I/O."""
import asyncio
from dataclasses import replace
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import time
from types import ModuleType
import zipfile

import pytest
from launchkit.connect_installer_caller import ExactFile
from launchkit.provider_setup_host import ForgeProviderSetupSession, ProviderSetupUnavailable
from launchkit.templates_v2.console_auth import ConsoleConnectLogin

WHEEL_SHA = '034e9044ef524a60f0dd4bbc0c37ce6094819bd08ca1a87d31ed2686e30cfa8c'
MODULE_SHA = 'b6f672c6cb127026d2116e05f271c3415f5f9db3787c730c80de58dd8d4abafe'

PEER = r'''
import json,sys,time
from pathlib import Path
init=json.loads(sys.stdin.readline());root=Path(init['binding']['context_root'])
(root/'started').write_text('started')
def emit(v):print(json.dumps(v),flush=True)
emit({'kind':'initialized','id':'initialize','protocol':'connect-provider-setup/1','interaction_id':'native-interaction'})
while (root/'hold_prompt').exists():time.sleep(.01)
emit({'kind':'prompt','interaction_id':'native-interaction','prompt_id':'native-prompt','prompt':{'kind':'choices','choices':[{'choice_id':'native-choice','model':'native-model','provider':'openai','auth':'chatgpt_oauth','selectable':True}]}})
request=json.loads(sys.stdin.readline())
(root/'commands').write_text(json.dumps(request))
if request['method']=='cancel':
 emit({'kind':'prompt_closed','prompt_id':'native-prompt'})
 emit({'kind':'cancelled','interaction_id':'native-interaction','joined':True,'join_scope':'provider_interaction'})
else:
 emit({'kind':'answered','prompt_id':'native-prompt','id':request['id']})
 emit({'kind':'selected','selection':{'interaction_id':'native-interaction','model':'native-model','provider':'openai','auth_binding':{'realm':'native-realm','binding':'native-binding','origin':'configured'},'credential':'ready','live_entitlement':'unobserved'}})
while (root/'hold_exit').exists():time.sleep(.01)
'''

@pytest.fixture
def sdk(monkeypatch, tmp_path):
    wheel = os.environ.get('FORGE_PROVIDER_SETUP_SDK_WHEEL')
    if not wheel:
        pytest.skip('Explicit pinned provider SDK wheel required.')
    content=Path(wheel).read_bytes()
    assert hashlib.sha256(content).hexdigest()==WHEEL_SHA
    with zipfile.ZipFile(wheel) as archive:
        source=archive.read('meerkat_toolkit/provider_setup.py')
    assert hashlib.sha256(source).hexdigest()==MODULE_SHA
    path=tmp_path/'sdk.py';path.write_bytes(source)
    package=ModuleType('meerkat_toolkit');package.__path__=[]
    monkeypatch.setitem(sys.modules,'meerkat_toolkit',package)
    name='meerkat_toolkit.provider_setup';spec=importlib.util.spec_from_file_location(name,path)
    module=importlib.util.module_from_spec(spec);monkeypatch.setitem(sys.modules,name,module);spec.loader.exec_module(module)
    return module

@pytest.fixture
def construction(sdk,tmp_path):
    exe=tmp_path/'peer';exe.write_text('#!'+sys.executable+'\n'+PEER);exe.chmod(0o700)
    deadline=int(time.time()*1000)+120000
    current=[True]
    login=ConsoleConnectLogin('fixture-human','fixture-login',deadline,'https://forge.example.invalid',lambda:current[0])
    binding=sdk.ProviderSetupBinding('forge','fixture-instance',login.principal,login.login_scope_id,deadline,
        'native-realm',str(tmp_path),str(tmp_path/'state'),str(tmp_path/'config'),{'kind':'file','root':str(tmp_path/'credentials')})
    pin=ExactFile(exe,hashlib.sha256(exe.read_bytes()).hexdigest())
    return pin,binding,login,current

async def until(check):
    async with asyncio.timeout(5):
        while not check():await asyncio.sleep(.01)

async def opened(construction):
    pin,binding,login,_=construction
    owner=ForgeProviderSetupSession(pin,binding,login)
    await owner.begin(login)
    await until(lambda:owner.snapshot(login)['status']=='prompt')
    return owner,login

async def test_exact_native_choices_and_selection_require_original_login_and_child_join(construction,tmp_path):
    (tmp_path/'hold_exit').touch()
    owner,login=await opened(construction)
    try:
        prompt=owner.snapshot(login)
        assert prompt['prompt']['choices'][0]['choice_id']=='native-choice'
        assert 'credential_scope' not in json.dumps(prompt)
        await owner.answer(login,prompt['prompt_id'],{'kind':'choice','choice_id':'native-choice'})
        await until(lambda:owner.snapshot(login)['status']=='joining')
        with pytest.raises(ProviderSetupUnavailable):owner.selection(login)
        assert not owner.snapshot(login)['joined']
        (tmp_path/'hold_exit').unlink()
        await until(lambda:owner.snapshot(login)['status']=='selected')
        selected=owner.selection(login)
        assert selected.auth_binding=={'realm':'native-realm','binding':'native-binding','origin':'configured'}
        assert owner.snapshot(login)['selection']=={'model':'native-model','provider':'openai','credential':'ready','live_entitlement':'unobserved'}
        assert 'auth_binding' not in owner.snapshot(login)['selection']
        with pytest.raises(ProviderSetupUnavailable):owner.selection(replace(login))
        request=json.loads((tmp_path/'commands').read_text())
        assert request['answer']=={'kind':'choice','choice_id':'native-choice'}
    finally:
        (tmp_path/'hold_exit').unlink(missing_ok=True);await owner.close()
    assert owner.retired

@pytest.mark.parametrize('field,value',[('principal_id','another'),('login_scope_id','another'),('expires_at_ms',0)])
def test_foreign_or_expired_initial_binding_refused_before_launch(construction,tmp_path,field,value):
    pin,binding,login,_=construction
    with pytest.raises(ProviderSetupUnavailable):ForgeProviderSetupSession(pin,replace(binding,**{field:value}),login)
    assert not (tmp_path/'started').exists()

async def test_observer_cancellation_keeps_same_launch_and_close_joins_late_child(construction,tmp_path,monkeypatch,sdk):
    import threading
    entered,release=threading.Event(),threading.Event()
    real=sdk.ProviderSetupClient
    creations=[]
    def delayed(*args,**kwargs):
        entered.set();assert release.wait(5)
        client=real(*args,**kwargs);creations.append(client);return client
    monkeypatch.setattr(sdk,'ProviderSetupClient',delayed)
    pin,binding,login,_=construction;owner=ForgeProviderSetupSession(pin,binding,login)
    caller=asyncio.create_task(owner.begin(login))
    try:
        await until(entered.is_set);caller.cancel()
        with pytest.raises(asyncio.CancelledError):await caller
        owner.revoke();close=asyncio.create_task(owner.close())
        await asyncio.sleep(.02);assert not owner.retired;assert not close.done()
        release.set();await asyncio.wait_for(close,5)
        assert len(creations)==1 and creations[0]._process.returncode==0
        assert owner.retired
        with pytest.raises(ProviderSetupUnavailable):await owner.begin(login)
    finally:
        release.set();await owner.close()

async def test_logout_fences_immediately_and_retains_cleanup_after_observer_timeout(construction,tmp_path):
    (tmp_path/'hold_exit').touch();owner,login=await opened(construction)
    try:
        child=owner._client._process
        owner.revoke()
        with pytest.raises(ProviderSetupUnavailable):owner.snapshot(login)
        with pytest.raises(ProviderSetupUnavailable):await owner.answer(login,'native-prompt',{'kind':'choice','choice_id':'native-choice'})
        with pytest.raises(TimeoutError):await asyncio.wait_for(owner.close(),.05)
        assert child.poll() is None and not owner.retired
        (tmp_path/'hold_exit').unlink();assert await asyncio.wait_for(owner.close(),5)
        assert owner._client._process is child and child.returncode==0
        assert json.loads((tmp_path/'commands').read_text())['method']=='cancel'
    finally:
        (tmp_path/'hold_exit').unlink(missing_ok=True);await owner.close()

async def test_login_retirement_triggers_cleanup_without_another_browser_request(construction,tmp_path):
    owner,login=await opened(construction)
    construction[3][0]=False
    await until(lambda:owner.retired)
    assert owner._client._process.returncode==0
    with pytest.raises(ProviderSetupUnavailable):owner.snapshot(login)
    await owner.close()

async def test_edited_binary_refuses_before_launch_and_can_retire(construction,tmp_path):
    pin,binding,login,_=construction;owner=ForgeProviderSetupSession(pin,binding,login)
    pin.path.write_text('#!'+sys.executable+'\nraise Exception("not selected")')
    with pytest.raises(ProviderSetupUnavailable):await owner.begin(login)
    assert not (tmp_path/'started').exists()
    assert await owner.close()

async def test_prompt_answer_is_reserved_once_and_foreign_context_never_enters_sdk(construction,tmp_path):
    owner,login=await opened(construction)
    try:
        with pytest.raises(ProviderSetupUnavailable):await owner.answer(replace(login),'native-prompt',{'kind':'choice','choice_id':'native-choice'})
        first=asyncio.create_task(owner.answer(login,'native-prompt',{'kind':'choice','choice_id':'native-choice'}))
        await asyncio.sleep(0)
        with pytest.raises(ProviderSetupUnavailable):await owner.answer(login,'native-prompt',{'kind':'choice','choice_id':'native-choice'})
        await first
        await until(lambda:owner.snapshot(login)['status']=='selected')
        assert json.loads((tmp_path/'commands').read_text())['id']=='answer-1'
    finally:await owner.close()

async def test_answer_failure_after_cancelled_observer_still_retires_original_owner(construction,tmp_path,monkeypatch):
    import threading
    owner,login=await opened(construction);entered,release=threading.Event(),threading.Event()
    def failed(*args):
        entered.set();assert release.wait(5);raise RuntimeError('private-answer-canary')
    monkeypatch.setattr(owner._client,'answer',failed)
    caller=asyncio.create_task(owner.answer(login,'native-prompt',{'kind':'choice','choice_id':'native-choice'}))
    try:
        await until(entered.is_set);caller.cancel()
        with pytest.raises(asyncio.CancelledError):await caller
        release.set();await until(lambda:owner.retired)
        assert owner._client._process.returncode==0
        with pytest.raises(ProviderSetupUnavailable) as failure:owner.snapshot(login)
        assert 'private-answer-canary' not in str(failure.value)
    finally:release.set();await owner.close()
