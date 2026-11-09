"""Real subprocess wire controls, not native commissioning or provider proof."""
from __future__ import annotations

import dataclasses
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import time
import unittest
from unittest import mock

from meerkat_toolkit.service_setup import ServiceSetupBinding, ServiceSetupError, ServiceSetupLaunchError
from meerkat_toolkit.service_probe import (
    ServiceProbeClient, ServiceProbeLauncher, ProbeStatus, ProbeDispatchOutcome,
)


PEER = r'''
import json, os, sys, time, threading
from pathlib import Path
p=Path(__file__)
mode=p.with_suffix('.mode').read_text()
watchdog=threading.Timer(8,lambda:os._exit(74));watchdog.daemon=True;watchdog.start()
def emit(v):print(json.dumps(v),flush=True)
def result(i,r):emit({'jsonrpc':'2.0','id':i,'result':r})
def event(kind,**fields):emit({'jsonrpc':'2.0','method':'connect/event','params':{'target':target,'kind':kind,**fields}})
def record(v):
 with p.with_suffix('.frames').open('a') as f:f.write(json.dumps(v)+'\n')
init=json.loads(sys.stdin.readline());record(init)
p.with_suffix('.record').write_text(json.dumps({'pid':os.getpid(),'env':dict(os.environ),'argv':sys.argv,'init':init}))
target={'initialize_request_id':init['id']}
event('state',state={'available':False,'can_send':False,'can_stop':False,'outcome_uncertain':False,'active_send':None,'last_completion':None,'private_notice_pending':False,'can_probe':False,'commissioning':False,'active_probe':None,'probe_profiles':['opaque-a','opaque-b']})
if mode=='close_initial':
 v=json.loads(sys.stdin.readline());record(v);result(v['id'],{'target':v['params']['target'],'resources_closed':True});sys.exit(0)
if mode=='early':
 emit({'jsonrpc':'2.0','id':'p:early','method':'connect/prompt','params':{'target':target,'prompt':{'kind':'provider_api_key','label':'Provider'}}})
 record(json.loads(sys.stdin.readline()));event('prompt_closed',prompt_id='p:early',disposition='answered')
scope='connect:probe-fixture';session='native-session';job='native-job'
profiles=['opaque-a','opaque-b']
def state(can_probe=False,commissioning=False,active=None):
 return {'available':False,'can_send':False,'can_stop':False,'outcome_uncertain':False,'active_send':None,'last_completion':None,'private_notice_pending':False,'can_probe':can_probe,'commissioning':commissioning,'active_probe':active,'probe_profiles':profiles}
caps={'prompt_kinds':['approval','provider_target','provider_action','provider_api_key','provider_browser','provider_device','service_secret'],'service_preparation':True,'forge_configuration':False,'paired_host':False,'tools':[],
 'service_probes':{'selection':'trusted_process_profiles','requires':'successful_finalization_and_governed_reopen','result':'native_tool_completion_or_profile_verified_resource','slack_inbound':'unsupported'}}
result(init['id'],{'protocol_version':5 if mode=='downgrade' else 6,'scope':scope,'session_id':session,'capabilities':caps,'state':state()})
target={'scope':scope}
event('unavailable',reason='finalizing')
event('state',state=state())
event('service_finalization',state='native_closed',grant_issued=False)
if mode=='commissioning':
 event('state',state=state(commissioning=True))
 emit({'jsonrpc':'2.0','id':'p:commission','method':'connect/prompt','params':{'target':target,'prompt':{'kind':'approval','label':'Review selected realm'}}})
 answer=json.loads(sys.stdin.readline());record(answer)
 event('prompt_closed',prompt_id='p:commission',disposition='answered')
 event('service_commissioning',state='observed',capability_verified=False,observation={'realm':init['params']['realm']['realm_id'],'session_id':session,'job_id':job,'agent':'native-agent','state':'completed','governance':'committed','grants':[],'controller_disposal':'drained','issuance':'not_entered','custody_released':True,'capability_verified':False})
event('service_finalization',state='observed',observation={'scope':'invocation_finalization_observation_only'})
if mode!='no_ready':
 event('service_probe_state',state='available',job_id=job,session_id=session,capability_verified=False)
 event('state',state=state(can_probe=True))
if mode=='logout':event('unavailable',reason='viewer_retired')
if mode=='eof':sys.exit(0)
if mode=='backpressure':
 p.with_suffix('.blocked').write_text('ready')
 while not p.with_suffix('.resume').exists():time.sleep(.01)
last=None

def report(v,status='tool_completed'):
 b=init['params'];r={'profile_id':v['params']['profile_id'],'application_id':b['application']['application_id'],'instance_id':b['application']['instance_id'],'login_scope_id':b['viewer']['login_scope_id'],'viewer_id':b['viewer']['principal_id'],'origin':b['application']['origin'],'realm':b['realm']['realm_id'],'session_id':session,'job_id':v['params']['job_id'],'server':'native-server','agent':'native-agent','operation':'native-operation','resource':{'kind':'host_selected','nested':['resource']},'status':status,'input_id':'native-input','run_id':'native-run','tool_call_id':'native-call','result_sha256':'a'*64,'installed_connection_sha256':'b'*64,'dispatch_outcome':'committed','native_terminal_committed':True,'native_close_joined':True}
 if mode=='unknown':r.update(status='outcome_unknown',dispatch_outcome='unknown')
 if mode=='failed':r.update(status='tool_failed',dispatch_outcome='failed')
 if mode=='not_verified':r['status']='capability_not_verified'
 if mode=='verified':r['status']='capability_verified'
 if mode=='cleanup':r.update(status='cleanup_unconfirmed',native_close_joined=False)
 if mode=='unknown_verified':r.update(status='capability_verified',dispatch_outcome='unknown')
 if mode=='uncommitted':r.update(status='capability_verified',native_terminal_committed=False)
 if mode.startswith('foreign_'):r[mode[len('foreign_'):]]='foreign'
 if mode=='extra':r['grant']='must_not_be_accepted'
 return r

def probe_result(v,status='tool_completed'):
 result(v['id'],{'scope':'foreign' if mode=='wrong_scope' else scope,'probe_request_id':'h:other' if mode=='wrong_id' else v['id'],'report':report(v,status),'state':state(can_probe=True)})
for line in sys.stdin:
 v=json.loads(line);record(v);method=v.get('method')
 if method=='connect/close':
  if mode=='slow_close':time.sleep(.3)
  if mode!='unconfirmed':result(v['id'],{'target':v['params']['target'],'resources_closed':True})
  break
 if method=='connect/probe':
  last=v
  event('state',state=state(active=v['id']))
  if mode=='refusal':
   emit({'jsonrpc':'2.0','id':v['id'],'error':{'code':-32010,'message':'SECRET_ERROR','data':{'code':'SECRET_TOKEN'}}})
  elif mode not in ('pending','late_prompt'):
   if mode=='late_result':time.sleep(.2)
   probe_result(v)
 elif method=='connect/cancel':
  if mode=='late_prompt':
   emit({'jsonrpc':'2.0','id':'p:late','method':'connect/prompt','params':{'target':target,'prompt':{'kind':'provider_api_key','label':'SECRET_LATE'}}})
   event('prompt_closed',prompt_id='p:late',disposition='cancelled')
  result(v['id'],{'scope':scope,'send_request_id':v['params']['send_request_id'],'status':'cancellation_requested' if mode in ('pending','late_prompt') else 'not_current'})
  if mode in ('pending','late_prompt'):probe_result(last,'cancelled')
'''


class ServiceProbeTransportTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='connect-probe-wire-test-'))
        self.peer = self.root / 'trusted-host'
        self.peer.write_text('#!' + sys.executable + '\n' + PEER)
        self.peer.chmod(0o700)
        self.clients = []
        self.binding = ServiceSetupBinding('app','instance','http://127.0.0.1:1234','viewer','login',
            int(time.time()*1000)+60_000,'realm',str(self.root/'context'),str(self.root/'state'),
            str(self.root/'config'),'a'*64,{'kind':'file','root':str(self.root/'credentials')})

    def tearDown(self):
        failures = []
        self.peer.with_suffix('.resume').touch()
        for client in self.clients:
            try:
                client.close(timeout=3)
            except Exception as error:
                failures.append(type(error).__name__)
                try:
                    client._process.wait(timeout=10)
                except Exception as error:
                    failures.append('fixture wait: '+type(error).__name__)
                try:
                    client.close(timeout=3)
                except Exception as error:
                    failures.append('owner join: '+type(error).__name__)
            if client._process.poll() is None:
                failures.append('test peer exceeded intrinsic bound')
                try:
                    # Exclusively this test's artificial peer, never production.
                    client._process.kill(); client._process.wait(timeout=3); client.close(timeout=3)
                except Exception as error:
                    failures.append('fixture fallback: '+type(error).__name__)
            if client._process.poll() == 74:
                failures.append('intrinsic fixture watchdog fired')
        if all(c._process.poll() is not None and not c._reader.is_alive() and not c._writer.is_alive() for c in self.clients):
            shutil.rmtree(self.root)
        else:
            failures.append('retained unsettled fixture files at '+str(self.root))
        self.assertFalse(failures, 'fixture cleanup failed: '+','.join(failures))

    def start(self, mode='normal', *, initialized=True, binding=None, launcher=None):
        self.peer.with_suffix('.mode').write_text(mode)
        binding = binding or self.binding
        launcher = launcher or ServiceProbeLauncher((str(self.peer),'--host-selected'),binding.credential_scope,{'PATH':os.defpath,'LANG':'C'})
        try:
            client = ServiceProbeClient(launcher,binding)
        except ServiceSetupLaunchError as error:
            self.clients.append(error.owner)
            raise
        self.clients.append(client)
        if initialized:
            self.until(client,lambda e:e.kind=='state' and e.payload.get('state',{}).get('can_probe') is True)
        return client

    def until(self, client, predicate):
        deadline=time.monotonic()+3
        while True:
            event=client.next_event(max(.001,deadline-time.monotonic()))
            if predicate(event):return event
            if time.monotonic()>=deadline:self.fail('expected native observation was not delivered')

    def completed(self, client, identity):
        return self.until(client,lambda e:e.request_id==identity and e.kind in ('result','refused'))

    def frames(self):
        return [json.loads(line) for line in self.peer.with_suffix('.frames').read_text().splitlines()]

    def test_explicit_launcher_and_immutable_binding(self):
        env={'PATH':os.defpath,'LANG':'C'};scope=dict(self.binding.credential_scope);command=[str(self.peer),'--host-selected']
        launcher=ServiceProbeLauncher(command,scope,env)
        command.append('--not-sent');env['AUTH_TOKEN']='secret';scope['root']='/wrong'
        real=__import__('subprocess').Popen
        calls=[]
        def spawn(*args,**kwargs):
            calls.append((args[0],dict(kwargs['env'])))
            return real(*args,**kwargs)
        with mock.patch('meerkat_toolkit.service_setup.subprocess.Popen',side_effect=spawn):
            client=self.start(launcher=launcher)
        self.assertEqual(calls,[([str(self.peer),'--host-selected'],{'PATH':os.defpath,'LANG':'C'})])
        wire=self.frames()[0]['params'];self.assertEqual(wire['protocol_version'],6)
        self.assertNotIn('credential_scope',wire);self.assertNotIn('profiles',wire)
        self.assertEqual(client.close(3).native_resources_closed,True)

    def test_no_implicit_launcher_or_downgrade(self):
        with mock.patch('meerkat_toolkit.service_setup.subprocess.Popen') as spawn:
            with self.assertRaises(ValueError):ServiceProbeClient(str(self.peer),self.binding)
            with self.assertRaises(ValueError):ServiceProbeLauncher((str(self.peer),),self.binding.credential_scope,{'API_KEY':'private'})
            self.assertFalse(spawn.called)
        client=self.start('downgrade',initialized=False)
        with self.assertRaises(ServiceSetupError):self.until(client,lambda e:e.kind=='result')
        self.assertFalse(client.close(3).native_resources_closed)
        self.assertEqual(len([f for f in self.frames() if f.get('method')=='connect/initialize']),1)

    def test_early_provider_prompt_keeps_actual_target(self):
        client=self.start('early',initialized=False)
        prompt=self.until(client,lambda e:e.kind=='prompt')
        client.answer(prompt.request_id,{'kind':'provider_api_key','key':'PRIVATE_TEST_KEY'})
        self.until(client,lambda e:e.kind=='state' and e.payload.get('state',{}).get('can_probe'))
        self.assertEqual(self.frames()[1]['id'],'p:early')
        self.assertNotIn('PRIVATE_TEST_KEY',repr(prompt))

    def test_commissioning_approval_after_setup_owner_closed(self):
        client=self.start('commissioning',initialized=False)
        prompt=self.until(client,lambda e:e.kind=='prompt')
        with self.assertRaises(ServiceSetupError):client.submit('must not restart setup')
        with self.assertRaises(ServiceSetupError):client.connector_cancel()
        client.answer(prompt.request_id,{'kind':'approval','decision':'approve'})
        observed=self.until(client,lambda e:e.kind=='service_commissioning')
        self.assertIsNone(observed.report);self.assertFalse(observed.payload['capability_verified'])
        self.until(client,lambda e:e.kind=='state' and e.payload.get('state',{}).get('can_probe'))
        identity=client.probe(job_id='native-job',profile_id='opaque-a')
        self.assertEqual(self.completed(client,identity).report.status,ProbeStatus.TOOL_COMPLETED)

    def test_finalization_does_not_enable_probe(self):
        client=self.start('no_ready',initialized=False)
        self.until(client,lambda e:e.kind=='service_finalization')
        with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-a')
        self.assertFalse(any(f.get('method')=='connect/probe' for f in self.frames()))

    def test_exact_selector_and_frozen_typed_report(self):
        client=self.start()
        for job,profile in [('foreign','opaque-a'),('native-job','foreign')]:
            with self.assertRaises(ServiceSetupError):client.probe(job_id=job,profile_id=profile)
        identity=client.probe(job_id='native-job',profile_id='opaque-a')
        event=self.completed(client,identity);report=event.report
        self.assertEqual(report.status,ProbeStatus.TOOL_COMPLETED)
        self.assertEqual(report.dispatch_outcome,ProbeDispatchOutcome.COMMITTED)
        self.assertTrue(event.viewer_current_at_observation)
        with self.assertRaises(TypeError):report.resource['nested']='changed'
        with self.assertRaises(TypeError):event.payload['result']['report']['job_id']='changed'
        with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-a')
        self.assertEqual([f['params'] for f in self.frames() if f.get('method')=='connect/probe'],[{'scope':'connect:probe-fixture','job_id':'native-job','profile_id':'opaque-a'}])

    def test_honest_outcomes_and_no_replay(self):
        for mode,status,dispatch in [('unknown',ProbeStatus.OUTCOME_UNKNOWN,ProbeDispatchOutcome.UNKNOWN),('failed',ProbeStatus.TOOL_FAILED,ProbeDispatchOutcome.FAILED),('not_verified',ProbeStatus.CAPABILITY_NOT_VERIFIED,ProbeDispatchOutcome.COMMITTED),('verified',ProbeStatus.CAPABILITY_VERIFIED,ProbeDispatchOutcome.COMMITTED),('cleanup',ProbeStatus.CLEANUP_UNCONFIRMED,ProbeDispatchOutcome.COMMITTED)]:
            with self.subTest(mode=mode):
                client=self.start(mode);identity=client.probe(job_id='native-job',profile_id='opaque-a')
                event=self.completed(client,identity)
                self.assertEqual((event.report.status,event.report.dispatch_outcome),(status,dispatch))
                with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-a')
                client.close(3)

    def test_foreign_and_false_positive_reports_refuse(self):
        modes=['foreign_'+key for key in ('profile_id','application_id','instance_id','login_scope_id','viewer_id','origin','realm','session_id','job_id')]+['wrong_scope','wrong_id','extra','unknown_verified','uncommitted']
        for mode in modes:
            with self.subTest(mode=mode):
                client=self.start(mode);identity=client.probe(job_id='native-job',profile_id='opaque-a')
                with self.assertRaises(ServiceSetupError):self.completed(client,identity)
                self.assertFalse(client.close(3).native_resources_closed)

    def test_active_probe_cancel_permanently_retires(self):
        client=self.start('pending');identity=client.probe(job_id='native-job',profile_id='opaque-a')
        cancel=client.cancel(identity)
        with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-b')
        event=self.completed(client,cancel);self.assertEqual(event.payload['result']['status'],'cancellation_requested')
        observed=self.completed(client,identity);self.assertEqual(observed.report.status,ProbeStatus.CANCELLED)
        self.assertFalse(observed.viewer_current_at_observation)

    def test_only_latest_completed_probe_can_cancel(self):
        client=self.start();first=client.probe(job_id='native-job',profile_id='opaque-a');self.completed(client,first)
        second=client.probe(job_id='native-job',profile_id='opaque-b');self.completed(client,second)
        with self.assertRaises(ServiceSetupError):client.cancel(first)
        cancel=client.cancel(second)
        self.assertEqual(self.completed(client,cancel).payload['result']['status'],'not_current')
        with self.assertRaises(ServiceSetupError):client.submit('retired')

    def test_late_report_keeps_facts_but_not_current_viewer(self):
        client=self.start('late_result');identity=client.probe(job_id='native-job',profile_id='opaque-a')
        client.cancel(identity)
        event=self.completed(client,identity)
        self.assertEqual(event.report.status,ProbeStatus.TOOL_COMPLETED)
        self.assertFalse(event.viewer_current_at_observation)

    def test_late_prompt_after_cancel_is_not_actionable(self):
        client=self.start('late_prompt');identity=client.probe(job_id='native-job',profile_id='opaque-a');client.cancel(identity)
        event=self.until(client,lambda e:e.kind=='retired_prompt')
        self.assertNotIn('SECRET_LATE',repr(dict(event.payload)))
        with self.assertRaises(ServiceSetupError):client.answer(event.request_id,{'kind':'provider_api_key','key':'not-sent'})

    def test_cancel_queue_refusal_still_retires_same_owner(self):
        client=self.start('backpressure');identity=client.probe(job_id='native-job',profile_id='opaque-a')
        process=client._process
        with mock.patch.object(client,'_enqueue',side_effect=ServiceSetupError('fixture queue refusal')):
            with self.assertRaises(ServiceSetupError):client.cancel(identity)
        self.peer.with_suffix('.resume').touch()
        with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-b')
        self.assertIs(client._process,process)
        self.assertFalse(client.close(3).native_resources_closed)

    def test_expiry_and_invalid_waits_do_not_enter(self):
        client=self.start()
        with mock.patch('meerkat_toolkit.service_setup.time.time',return_value=self.binding.expires_at_ms/1000+1):
            with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-a')
        for timeout in (0,-1,float('nan'),float('inf'),True):
            with self.assertRaises(ValueError):client.close(timeout)
            with self.assertRaises(ValueError):client.next_event(timeout)
        identity=client.probe(job_id='native-job',profile_id='opaque-a');self.completed(client,identity)

    def test_refusal_redaction_and_eof_do_not_verify(self):
        client=self.start('refusal');identity=client.probe(job_id='native-job',profile_id='opaque-a')
        event=self.completed(client,identity)
        self.assertEqual(event.kind,'refused');self.assertIsNone(event.report)
        self.assertNotIn('SECRET',repr(dict(event.payload)))
        with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-a')
        client.close(3)
        client=self.start('eof')
        with self.assertRaises(EOFError):
            while True:client.next_event(3)
        self.assertFalse(client.close(3).native_resources_closed)

    def test_close_timeout_retains_process_and_workers(self):
        client=self.start('slow_close');process=client._process
        with self.assertRaises(TimeoutError):client.close(.01)
        self.assertIs(client._process,process)
        with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-a')
        closed=client.close(3)
        self.assertTrue(closed.process_joined);self.assertTrue(closed.native_resources_closed)
        self.assertFalse(client._reader.is_alive());self.assertFalse(client._writer.is_alive())

    def test_host_logout_event_permanently_retires_admission(self):
        client=self.start('logout')
        self.until(client,lambda e:e.kind=='unavailable')
        with self.assertRaises(ServiceSetupError):client.probe(job_id='native-job',profile_id='opaque-a')
        with self.assertRaises(ServiceSetupError):client.connector_cancel()
        self.assertTrue(client.close(3).native_resources_closed)

    def test_value_free_protected_conversion_refusal(self):
        client=self.start('early',initialized=False)
        prompt=self.until(client,lambda e:e.kind=='prompt')
        # A malformed protected value is refused without forwarding it or using
        # arbitrary object text in the exception shown to the host UI.
        with self.assertRaises(ServiceSetupError) as caught:
            client.answer(prompt.request_id,{'kind':'provider_api_key','key':'\ud800'})
        self.assertNotIn('PRIVATE_CANARY',str(caught.exception))
        self.assertNotIn('surrogate',str(caught.exception))
        client.answer(prompt.request_id,{'kind':'provider_api_key','key':'test-key'})
        self.until(client,lambda e:e.kind=='state' and e.payload.get('state',{}).get('can_probe'))
        with self.assertRaises(ServiceSetupError) as caught:
            client.connector_complete(state='\ud800',code='PRIVATE_CANARY')
        self.assertNotIn('PRIVATE_CANARY',str(caught.exception))

    def test_close_before_initialization_retains_original_target(self):
        client=self.start('close_initial',initialized=False)
        process=client._process
        closed=client.close(3)
        self.assertTrue(closed.process_joined);self.assertTrue(closed.native_resources_closed)
        self.assertIs(client._process,process)
        close=[f for f in self.frames() if f.get('method')=='connect/close']
        self.assertEqual(len(close),1)
        self.assertEqual(close[0]['params'],{'target':{'initialize_request_id':'h:initialize'}})

    def test_v6_startup_failure_exposes_same_actual_owner(self):
        import threading
        start=threading.Thread.start
        def failing_start(thread):
            if thread.name=='connect-services-owner':raise RuntimeError('fixture reader start failure')
            return start(thread)
        with mock.patch.object(threading.Thread,'start',failing_start):
            with self.assertRaises(ServiceSetupLaunchError) as caught:self.start(initialized=False)
        client=caught.exception.owner
        self.assertIn(client,self.clients)
        self.assertIsNotNone(client._process)
        self.assertFalse(client.close(3).native_resources_closed)
        self.assertFalse(client._reader.is_alive());self.assertFalse(client._writer.is_alive())


if __name__ == '__main__':
    unittest.main()
