"""Real child/pipe controls only; no provider, native auth or model I/O."""
import dataclasses
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest import mock
import re
import subprocess
import meerkat_toolkit.provider_setup as provider_module
from meerkat_toolkit.provider_setup import ProviderSetupBinding, ProviderSetupClient, ProviderSelection, ProviderSetupError

PEER = r'''
import json,os,sys,time,threading
from pathlib import Path
limit=threading.Timer(10,lambda:os._exit(88));limit.daemon=True;limit.start()
request=json.loads(sys.stdin.readline()); b=request['binding']
root=Path(b['context_root']); (root/'observed.json').write_text(json.dumps({'env':dict(os.environ),'argv':sys.argv,'binding':b}))
def out(v): print(json.dumps(v),flush=True)
out({'kind':'initialized','id':'initialize','protocol':'connect-provider-setup/1','interaction_id':'actual'})
out({'kind':'prompt','interaction_id':'actual','prompt_id':'p','prompt':{'kind':'choices','choices':[]}})
request=json.loads(sys.stdin.readline())
if request['method']=='cancel':
 out({'kind':'cancelling','id':request['id'],'interaction_id':'actual'})
 out({'kind':'prompt_closed','prompt_id':'p'})
 while (root/'hold').exists(): time.sleep(.01)
 out({'kind':'cancelled','interaction_id':'actual','joined':True,'join_scope':'provider_interaction','native_effects':'reobserve'})
else:
 out({'kind':'answered','id':request['id'],'prompt_id':'p'})
 out({'kind':'selected','selection':{'interaction_id':'actual','model':'native-model','provider':'openai','auth_binding':{'realm':'owned','binding':'selected'},'credential':'ready','live_entitlement':'unobserved'}})
'''

class ProviderSetupTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name)
        self.exe=self.root/'peer';self.exe.write_text('#!'+sys.executable+'\n'+PEER);self.exe.chmod(0o700)
        self.binding=ProviderSetupBinding('application','instance','user','login',int(time.time()*1000)+120000,
            'owned',str(self.root),str(self.root/'state'),str(self.root/'home'),{'kind':'file','root':str(self.root/'credentials')})
        self.client=None
    def tearDown(self):
        (self.root/'hold').unlink(missing_ok=True)
        failure=None
        if self.client is not None:
            while True:
                try:self.client.close(1);break
                except TimeoutError as error:
                    failure=failure or error
                    continue # Same reader/Popen owner, never a replacement.
                except ProviderSetupError as error:
                    failure=failure or error
                    break # close reports transport failure only after join.
        self.temp.cleanup()
        if failure is not None:raise failure
    def open(self, **kwargs):
        self.client=ProviderSetupClient(str(self.exe),self.binding,**kwargs)
        self.assertEqual(self.client.next_event(10)['kind'],'initialized')
        self.assertEqual(self.client.next_event(10)['kind'],'prompt')
        return self.client
    def test_real_pipe_keeps_explicit_native_selection_without_session(self):
        client=self.open();client.answer('p',{'kind':'choice','choice_id':'offered'})
        self.assertEqual(client.next_event(10)['kind'],'answered')
        value=client.next_event(10);selected=ProviderSelection.decode(value['selection'])
        self.assertEqual(selected.auth_binding,{'realm':'owned','binding':'selected'})
        self.assertNotIn('session_id',value);self.assertEqual(client.close(10)['kind'],'selected')
    def test_isolated_launch_does_not_inherit_credentials_or_home(self):
        launched=[];real_popen=subprocess.Popen
        def observed_popen(*args,**kwargs):
            launched.append(dict(kwargs['env']))
            return real_popen(*args,**kwargs) # Retain the actual real-child oracle.
        with mock.patch.object(provider_module.subprocess,'Popen',side_effect=observed_popen):
            client=self.open()
        client.close(10)
        expected={'PATH':os.defpath,'LANG':'C'}
        self.assertEqual(launched,[expected],"exact host launch environment, before runtime initialization")
        observed=json.loads((self.root/'observed.json').read_text())
        runtime=observed['env']
        self.assertEqual({key:runtime[key] for key in expected},expected)
        additions=set(runtime)-set(expected)
        allowed={'LC_CTYPE'} # Python's C-locale coercion, never supplied at spawn.
        if sys.platform=='darwin':allowed.add('__CF_USER_TEXT_ENCODING')
        self.assertLessEqual(additions,allowed)
        if 'LC_CTYPE' in additions:self.assertIn(runtime['LC_CTYPE'],('C.UTF-8','C.utf8','UTF-8'))
        if '__CF_USER_TEXT_ENCODING' in additions:
            fields=runtime['__CF_USER_TEXT_ENCODING'].split(':')
            self.assertEqual(len(fields),3)
            self.assertTrue(all(re.fullmatch(r'0x[0-9a-fA-F]{1,8}',field) for field in fields))
            self.assertEqual(int(fields[0],16),os.geteuid())
        self.assertTrue(runtime.keys().isdisjoint({'HOME','OPENAI_API_KEY','ANTHROPIC_API_KEY','RKAT_CONFIG','MEERKAT_CONFIG'}))
        self.assertEqual(observed['argv'][1:],['--provider-stdio','--provider-store-root',str(self.root/'credentials')])
        self.assertEqual(observed['binding']['credential_scope'],self.binding.credential_scope)
    def test_native_default_requires_explicit_fixed_host_environment(self):
        self.binding=dataclasses.replace(self.binding,credential_scope={'kind':'native_default'})
        with self.assertRaises(ValueError):ProviderSetupClient(str(self.exe),self.binding)
        client=self.open(environment={'PATH':os.defpath,'HOME':str(self.root/'fixed-home'),'LANG':'C'})
        client.close(10);observed=json.loads((self.root/'observed.json').read_text())
        self.assertEqual(observed['env']['HOME'],str(self.root/'fixed-home'))
        self.assertEqual(observed['argv'][1:],['--provider-stdio','--provider-native-default'])
    def test_timeout_retains_same_live_child_and_later_joins(self):
        (self.root/'hold').touch();client=self.open();pid=client._process.pid
        with self.assertRaises(TimeoutError):client.close(.03)
        self.assertIsNone(client._process.poll());self.assertTrue(client._reader.is_alive())
        (self.root/'hold').unlink();self.assertEqual(client.close(10)['kind'],'cancelled')
        self.assertEqual(client._process.pid,pid);self.assertEqual(client._process.returncode,0)
        self.assertFalse(client._reader.is_alive())
    def test_stale_prompt_and_rebinding_are_refused(self):
        client=self.open()
        with self.assertRaises(ProviderSetupError):client.answer('old',{'kind':'callback','url':'private'})
        client.close(10)
        with self.assertRaises(ProviderSetupError):client.answer('p',{'kind':'confirm'})
    def test_ambient_override_and_relative_store_refuse_before_spawn(self):
        for key in ('OPENAI_API_KEY','__CF_USER_TEXT_ENCODING','LC_CTYPE'):
            with self.subTest(key=key):
                with self.assertRaises(ValueError):self.open(environment={key:'not-host-admitted'})
        self.binding=dataclasses.replace(self.binding,credential_scope={'kind':'file','root':'relative'})
        with self.assertRaises(ValueError):self.open()
        self.assertFalse((self.root/'observed.json').exists())
    def test_selection_decoder_rejects_env_only_or_runtime_claims(self):
        value={'interaction_id':'actual','model':'model','provider':'openai','auth_binding':{'realm':'owned','binding':'named'},'credential':'ready','live_entitlement':'unobserved'}
        for changes in ({'auth_binding':None},{'auth_binding':{'realm':'owned'}},{'session_id':'forged'},{'live_entitlement':'proven'}):
            with self.subTest(changes=tuple(changes)):
                with self.assertRaises(ProviderSetupError):ProviderSelection.decode({**value,**changes})

    def test_invalid_timeouts_do_not_cancel_live_interaction(self):
        client=self.open()
        for timeout in (0,-1,float('nan'),float('inf'),2_147_484):
            with self.assertRaises(ValueError):client.close(timeout)
            with self.assertRaises(ValueError):client.next_event(timeout)
            self.assertFalse(client._cancelled)
            self.assertIsNone(client._process.poll())
        client.answer('p',{'kind':'choice','choice_id':'offered'})
        self.assertEqual(client.close(10)['kind'],'selected')
    def test_oversized_initialization_refuses_before_child_exists(self):
        self.binding=dataclasses.replace(self.binding,context_root='/'+'x'*(1024*1024))
        with self.assertRaises(ValueError):self.open()
        self.assertFalse((self.root/'observed.json').exists())

    def test_configured_origin_and_optional_native_profile_are_closed(self):
        value={'interaction_id':'actual','model':'model','provider':'openai','auth_binding':{'realm':'owned','binding':'named'},'credential':'ready','live_entitlement':'unobserved'}
        for fields in ({},{'origin':'configured'},{'origin':'configured','profile':'named'},{'profile':None}):
            ProviderSelection.decode({**value,'auth_binding':{**value['auth_binding'],**fields}})
        for fields in ({'origin':'synthetic_env_default'},{'origin':'unknown'},{'origin':None},{'origin':4},{'profile':''},{'profile':7}):
            with self.subTest(fields=fields):
                with self.assertRaises(ProviderSetupError):ProviderSelection.decode({**value,'auth_binding':{**value['auth_binding'],**fields}})
