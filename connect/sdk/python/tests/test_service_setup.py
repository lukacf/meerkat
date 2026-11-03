"""Actual subprocess transport controls; no native/provider acceptance implied."""
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

from meerkat_toolkit.service_setup import ServiceSetupBinding, ServiceSetupClient, ServiceSetupError, ServiceSetupLaunchError


PEER = r'''
import json, os, sys, time, threading
from pathlib import Path
# A broken client must not leave a test-owned subprocess alive indefinitely.
watchdog=threading.Timer(8,lambda:os._exit(74))
watchdog.daemon=True
watchdog.start()
p=Path(__file__)
mode=p.with_suffix('.mode').read_text()
record=p.with_suffix('.record')
def emit(v):
 print(json.dumps(v),flush=True)
def result(i,r):emit({'jsonrpc':'2.0','id':i,'result':r})
init=json.loads(sys.stdin.readline())
with record.open('w') as f:json.dump({'pid':os.getpid(),'env':dict(os.environ),'init':init,'args':sys.argv[1:]},f)
if mode=='close_race':time.sleep(.15)
scope='connect:fixture'
caps={'prompt_kinds':['approval','provider_target','provider_action','provider_api_key','provider_browser','provider_device','service_secret'],'service_preparation':True,'forge_configuration':False,'paired_host':False,'tools':[]}
emit({'jsonrpc':'2.0','method':'connect/event','params':{'target':{'initialize_request_id':init['id']},'kind':'state','state':{}}})
if mode=='startup_prompt':
 emit({'jsonrpc':'2.0','id':'p:startup','method':'connect/prompt','params':{'target':{'initialize_request_id':init['id']},'prompt':{'kind':'provider_api_key','label':'Provider key'}}})
 with record.with_suffix('.answer').open('w') as f:json.dump(json.loads(sys.stdin.readline()),f)
 emit({'jsonrpc':'2.0','method':'connect/event','params':{'target':{'initialize_request_id':init['id']},'kind':'prompt_closed','prompt_id':'p:startup','disposition':'answered'}})
result(init['id'],{'protocol_version':5,'scope':scope,'session_id':'native-fixture','capabilities':caps,'state':{}})
if mode=='backpressure':
 p.with_suffix('.blocked').write_text('ready')
 while not p.with_suffix('.resume').exists():time.sleep(.01)
if mode=='foreign':
 emit({'jsonrpc':'2.0','method':'connect/event','params':{'target':{'scope':'connect:other'},'kind':'state','state':{}}})
for line in sys.stdin:
 v=json.loads(line)
 if v.get('method')=='connect/close':
  if mode=='slow_close':time.sleep(.25)
  if mode!='unconfirmed':result(v['id'],{'target':v['params']['target'],'resources_closed':True})
  break
 if v.get('method')=='connect/send':
  send=v['id']
  if mode=='cancel_race':
   while not p.with_suffix('.cancel_queued').exists():time.sleep(.01)
  if mode in ('missing_scope','wrong_send','unknown_completion','outcome_unknown'):
   value={'scope':scope,'send_request_id':send,'completion':'outcome_unknown','state':{}}
   if mode=='missing_scope':del value['scope']
   if mode=='wrong_send':value['send_request_id']='h:other'
   if mode=='unknown_completion':value['completion']='ready'
   result(send,value)
   continue
  if mode=='refusal':
   emit({'jsonrpc':'2.0','id':send,'error':{'code':-32010,'message':'PRIVATE_PROVIDER_BODY','data':{'code':'private_token_canary'}}})
  else:
   emit({'jsonrpc':'2.0','id':'p:1','method':'connect/prompt','params':{'target':{'scope':scope},'prompt':{'kind':'service_secret','job_id':'job-a','service':'enterprise','slot':'PAT','label':'Enterprise access token'}}})
 elif v.get('id')=='p:1':
  with record.with_suffix('.answer').open('w') as f:json.dump(v,f)
  emit({'jsonrpc':'2.0','method':'connect/event','params':{'target':{'scope':scope},'kind':'prompt_closed','prompt_id':'p:1','disposition':'answered'}})
  result(send,{'scope':scope,'send_request_id':send,'completion':'cancelled' if v['result']['answer']['kind']=='cancel' else 'completed','state':{}})
 elif v.get('method')=='connect/cancel':
  emit({'jsonrpc':'2.0','method':'connect/event','params':{'target':{'scope':scope},'kind':'prompt_closed','prompt_id':'p:1','disposition':'cancelled'}})
  result(v['id'],{'scope':scope,'send_request_id':v['params']['send_request_id'],'status':'cancellation_requested'})
  result(send,{'scope':scope,'send_request_id':send,'completion':'cancelled','state':{}})
'''


class ServiceSetupTransportTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='connect-services-wire-test-'))
        self.executable = self.root / 'native-peer'
        self.executable.write_text('#!' + sys.executable + '\n' + PEER)
        self.executable.chmod(0o700)
        self.executable.with_suffix('.mode').write_text('secret')
        self.clients = []
        self.binding = ServiceSetupBinding('app','instance','http://127.0.0.1:1234','viewer','login',
            int(time.time()*1000)+60_000,'realm',str(self.root/'context'),str(self.root/'state'),
            str(self.root/'config'),'a'*64,{'kind':'file','root':str(self.root/'credentials')})

    def tearDown(self):
        failures=[]
        for client in self.clients:
            try:
                client.close(timeout=3)
            except Exception as error:
                failures.append(type(error).__name__)
                # Only fixture peers are used here. Their intrinsic watchdog
                # bounds the wait; retain their files until actual reaping.
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
                    # These are exclusively this test's artificial peers.
                    client._process.kill()
                    client._process.wait(timeout=3)
                    client.close(timeout=3)
                except Exception as error:
                    failures.append('fixture fallback: '+type(error).__name__)
            if client._process.poll()==74:
                failures.append('intrinsic fixture watchdog fired')
        if all(client._process.poll() is not None and not client._reader.is_alive() and not client._writer.is_alive() for client in self.clients):
            shutil.rmtree(self.root)
        else:
            failures.append('retained unsettled fixture files at '+str(self.root))
        self.assertFalse(failures,'fixture cleanup failed: '+','.join(failures))

    def client(self, mode='secret', initialized=True):
        self.executable.with_suffix('.mode').write_text(mode)
        client = ServiceSetupClient(str(self.executable), self.binding)
        self.clients.append(client)
        if initialized:
            event=client.next_event(3)
            self.assertEqual(event.kind,'state')
            event=client.next_event(3)
            self.assertEqual(event.kind,'result')
            self.assertEqual(event.payload['method'],'connect/initialize')
        return client

    def test_private_secret_answer_is_bound_to_actual_prompt_and_not_in_repr(self):
        client=self.client()
        request=client.submit('Prepare the selected service')
        event=client.next_event(3)
        self.assertEqual(event.kind,'prompt')
        answer={'kind':'service_secret','job_id':'job-a','service':'enterprise','slot':'PAT','value':'FAKE_PRIVATE_CANARY'}
        for field in ('job_id','service','slot'):
            with self.assertRaises(ServiceSetupError):client.answer(event.request_id,{**answer,field:'other'})
        with self.assertRaises(ServiceSetupError):client.answer('p:other',answer)
        with self.assertRaises(ServiceSetupError):client.answer(event.request_id,{**answer,'value':'PRIVATE_CANARY\ud800'})
        # Presentation mutation must not change the retained offered selectors.
        event.payload['service']='other'
        with self.assertRaises(ServiceSetupError):client.answer(event.request_id,{**answer,'service':'other'})
        client.answer(event.request_id,answer)
        closed=client.next_event(3)
        completed=client.next_event(3)
        self.assertEqual(closed.kind,'prompt_closed')
        self.assertEqual(completed.request_id,request)
        self.assertNotIn('FAKE_PRIVATE_CANARY',repr(event)+repr(completed)+repr(client))
        actual=json.loads(self.executable.with_suffix('.answer').read_text())
        self.assertEqual(actual,{'jsonrpc':'2.0','id':'p:1','result':{'answer':answer}})
        self.assertTrue(client.close(3).native_resources_closed)

    def test_timeout_preserves_same_process_until_real_close(self):
        client=self.client('slow_close')
        pid=client._process.pid
        with self.assertRaises(TimeoutError):client.close(.01)
        self.assertEqual(client._process.pid,pid)
        self.assertIsNone(client._process.poll())
        outcome=client.close(3)
        self.assertTrue(outcome.process_joined)
        self.assertTrue(outcome.native_resources_closed)
        self.assertEqual(client._process.pid,pid)

    def test_close_before_initialize_uses_original_echoed_target(self):
        client=self.client('close_race',initialized=False)
        self.assertTrue(client.close(3).native_resources_closed)
        self.assertEqual(client._close_target,{'initialize_request_id':'h:initialize'})

    def test_foreign_scope_fails_transport_and_cannot_claim_native_cleanup(self):
        client=self.client('foreign')
        with self.assertRaises(ServiceSetupError):client.next_event(3)
        self.assertFalse(client.close(3).native_resources_closed)

    def test_native_exit_without_close_receipt_is_not_cleanup_success(self):
        client=self.client('unconfirmed')
        outcome=client.close(3)
        self.assertTrue(outcome.process_joined)
        self.assertEqual(outcome.exit_code,0)
        self.assertFalse(outcome.native_resources_closed)

    def test_native_error_text_and_unrecognized_error_codes_are_not_forwarded(self):
        client=self.client('refusal')
        client.submit('inspect selected service')
        event=client.next_event(3)
        self.assertEqual(event.kind,'refused')
        self.assertEqual(event.payload['code'],'native_refused')
        serialized=json.dumps(dict(event.payload))
        self.assertNotIn('PRIVATE_PROVIDER_BODY',serialized)
        self.assertNotIn('private_token_canary',serialized)

    def test_minimal_launch_and_closed_initializer_do_not_inherit_auth_environment(self):
        client=self.client()
        record=json.loads(self.executable.with_suffix('.record').read_text())
        self.assertEqual(record['args'],['--stdio','--provider-store-root',str(self.root/'credentials')])
        self.assertEqual(set(record['init']['params']),{'protocol_version','adapter','application','viewer','realm','services'})
        self.assertNotIn('HOME',record['env'])
        for key in ('OPENAI_API_KEY','ANTHROPIC_API_KEY','MEERKAT_CONFIG','PYTHONPATH'):
            self.assertNotIn(key,record['env'])
            with self.assertRaises(ValueError):ServiceSetupClient(str(self.executable),self.binding,environment={key:'untrusted'})

    def test_deadline_and_nonfinite_waits_refuse_without_second_child(self):
        client=self.client()
        client._deadline=0
        with self.assertRaises(ServiceSetupError):client.submit('late')
        for timeout in (float('nan'),float('inf'),0,-1,True):
            with self.assertRaises(ValueError):client.next_event(timeout)
            with self.assertRaises(ValueError):client.close(timeout)
        with self.assertRaises(ValueError):ServiceSetupClient(str(self.executable),dataclasses.replace(self.binding,expires_at_ms=0))

    def test_stalled_stdin_does_not_defeat_close_timeout_or_replace_child(self):
        client=self.client('backpressure')
        client.submit('x'*(4*1024*1024))
        deadline=time.monotonic()+2
        while time.monotonic()<deadline:
            with client._condition:
                blocked=client._outbound_bytes>65536 and not client._outbound
            if blocked:break
            time.sleep(.005)
        self.assertTrue(blocked,'writer must actually own the in-flight frame')
        pid=client._process.pid
        started=time.monotonic()
        with self.assertRaises(TimeoutError):client.close(.03)
        self.assertLess(time.monotonic()-started,.5)
        self.assertEqual(client._process.pid,pid)
        self.assertIsNone(client._process.poll())
        self.executable.with_suffix('.resume').write_text('resume same peer')
        self.assertTrue(client.close(3).native_resources_closed)
        self.assertEqual(client.next_event(3).kind,'retired_prompt')
        self.assertFalse(client._writer.is_alive())
        self.assertFalse(client._reader.is_alive())
        self.assertEqual(client._outbound_bytes,0)

    def test_send_cancellation_permanently_retires_binding(self):
        client=self.client()
        request=client.submit('prepare')
        client.next_event(3)
        cancellation=client.cancel(request)
        with self.assertRaises(ServiceSetupError):client.submit('late')
        with self.assertRaises(ServiceSetupError):client.connector_cancel()
        self.assertEqual(client.next_event(3).kind,'prompt_closed')
        self.assertEqual(client.next_event(3).request_id,cancellation)
        self.assertEqual(client.next_event(3).payload['result']['completion'],'cancelled')
        with self.assertRaises(ServiceSetupError):client.submit('after cancellation completed')

    def test_cancel_answer_permanently_retires_binding(self):
        client=self.client()
        client.submit('prepare')
        prompt=client.next_event(3)
        client.answer(prompt.request_id,{'kind':'cancel'})
        with self.assertRaises(ServiceSetupError):client.submit('late')
        self.assertEqual(client.next_event(3).kind,'prompt_closed')
        self.assertEqual(client.next_event(3).payload['result']['completion'],'cancelled')
        with self.assertRaises(ServiceSetupError):client.submit('after cancellation completed')

    def test_reader_start_failure_exposes_same_child_owner_for_join(self):
        self.check_reader_start_failure(settle_writer_first=False)

    def test_reader_start_failure_preserves_already_settled_writer_flag(self):
        self.check_reader_start_failure(settle_writer_first=True)

    def check_reader_start_failure(self, *, settle_writer_first):
        import threading
        real_start=threading.Thread.start
        def start(thread):
            if thread.name=='connect-services-owner':
                if settle_writer_first:
                    owner=thread._target.__self__
                    owner._end_input()
                    owner._writer.join(2)
                    self.assertFalse(owner._writer.is_alive())
                raise RuntimeError('injected reader start failure')
            return real_start(thread)
        with mock.patch.object(threading.Thread,'start',start):
            with self.assertRaises(ServiceSetupLaunchError) as caught:
                ServiceSetupClient(str(self.executable),self.binding)
        owner=caught.exception.owner
        self.clients.append(owner)
        self.assertIsNotNone(owner._process)
        self.assertFalse(owner.close(3).native_resources_closed)
        self.assertFalse(owner._writer.is_alive())

    def test_interrupt_after_process_assignment_preserves_owner(self):
        # Trace the actual constructor assignment boundary, not an unreachable
        # mocked Popen-internal child that the client has never received.
        def trace(frame,event,arg):
            if (event=='line' and frame.f_code is ServiceSetupClient.__init__.__code__
                    and getattr(frame.f_locals.get('self'),'_process',None) is not None):
                sys.settrace(None)
                raise KeyboardInterrupt()
            return trace
        previous_trace=sys.gettrace()
        try:
            sys.settrace(trace)
            with self.assertRaises(ServiceSetupLaunchError) as caught:
                ServiceSetupClient(str(self.executable),self.binding)
        finally:
            sys.settrace(previous_trace)
        owner=caught.exception.owner
        self.clients.append(owner)
        self.assertTrue(owner.close(3).process_joined)
        self.assertFalse(owner.close(3).native_resources_closed)

    def test_closed_auth_and_connector_shapes_refuse_private_unknown_fields(self):
        with self.assertRaises(ValueError):ServiceSetupClient(str(self.executable),dataclasses.replace(self.binding,auth_binding={'realm':'r','binding':'b','secret':'PRIVATE'}))
        client=self.client()
        target={'slot_id':'credential:github','issuer':'https://issuer.example','client':'host','resource':'https://api.example',
                'scopes':['read'],'strategy_id':'oauth','account_selection':None}
        operations={'get':{'destination_origin':'https://api.example','required_scopes':['read'],'read_only':True}}
        before=client._counter
        for changed in ({**target,'secret':'PRIVATE'},{**target,'account_selection':{'access_token':'PRIVATE'}}):
            with self.assertRaises(ValueError):client.connector_start(job_id='job-a',connector=changed,redirect_uri='http://127.0.0.1:1/cb',operations=operations)
        with self.assertRaises(ValueError):client.connector_start(job_id='job-a',connector=target,redirect_uri='http://127.0.0.1:1/cb',operations={'get':{**operations['get'],'token':'PRIVATE'}})
        self.assertEqual(client._counter,before)

    def test_foreign_or_incomplete_send_results_refuse_and_unknown_is_preserved(self):
        for mode in ('missing_scope','wrong_send','unknown_completion'):
            client=self.client(mode)
            client.submit('inspect')
            with self.assertRaises(ServiceSetupError):client.next_event(3)
            self.assertFalse(client.close(3).native_resources_closed)
        client=self.client('outcome_unknown')
        request=client.submit('inspect')
        event=client.next_event(3)
        self.assertEqual(event.request_id,request)
        self.assertEqual(event.payload['result']['completion'],'outcome_unknown')
        self.assertNotIn('ready',event.payload)

    def test_provider_prompt_before_initialization_retains_initialize_target(self):
        client=self.client('startup_prompt',initialized=False)
        self.assertEqual(client.next_event(3).kind,'state')
        prompt=client.next_event(3)
        self.assertEqual(prompt.payload['kind'],'provider_api_key')
        with self.assertRaises(ServiceSetupError):client.answer(prompt.request_id,{'kind':'provider_api_key','key':'PRIVATE\ud800'})
        client.answer(prompt.request_id,{'kind':'provider_api_key','key':'FAKE_PROVIDER_KEY'})
        self.assertEqual(client.next_event(3).kind,'prompt_closed')
        self.assertEqual(client.next_event(3).payload['method'],'connect/initialize')
        self.assertTrue(client.close(3).native_resources_closed)

    def test_inflight_prompt_after_local_cancel_remains_nonactionable_until_closed(self):
        client=self.client('cancel_race')
        send=client.submit('prepare')
        cancellation=client.cancel(send)
        self.executable.with_suffix('.cancel_queued').write_text('cancel enqueued')
        prompt=client.next_event(3)
        self.assertEqual(prompt.kind,'retired_prompt')
        self.assertEqual(prompt.payload,{'prompt_kind':'service_secret'})
        with self.assertRaises(ServiceSetupError):client.answer(prompt.request_id,{'kind':'cancel'})
        self.assertEqual(client.next_event(3).kind,'prompt_closed')
        self.assertEqual(client.next_event(3).request_id,cancellation)
        self.assertEqual(client.next_event(3).request_id,send)
        self.assertTrue(client.close(3).native_resources_closed)


if __name__ == '__main__':
    unittest.main()
