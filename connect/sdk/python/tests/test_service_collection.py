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
from meerkat_toolkit.service_collection import (
    ServiceCollectionClient, ServiceCollectionLauncher, CollectionStatus,
)


PEER = r'''
import json, os, sys, time, threading, copy
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
early={'available':False,'can_send':False,'can_stop':False,'outcome_uncertain':False,'active_send':None,'last_completion':None,'private_notice_pending':False,'can_collect':False,'commissioning':False,'active_collection':None,'collection_sources':['opaque-a','opaque-b'],'reusable_source_coverage':'unqualified_credential_continuity'}
if mode=='bad_state_extra':early['program']={}
if mode=='bad_state_coverage':early['reusable_source_coverage']='qualified'
if mode=='bad_state_active':early['active_collection']='foreign'
event('state',state=early)
if mode=='close_initial':
 v=json.loads(sys.stdin.readline());record(v);result(v['id'],{'target':v['params']['target'],'resources_closed':True});sys.exit(0)
if mode=='early':
 emit({'jsonrpc':'2.0','id':'p:early','method':'connect/prompt','params':{'target':target,'prompt':{'kind':'provider_api_key','label':'Provider'}}})
 record(json.loads(sys.stdin.readline()));event('prompt_closed',prompt_id='p:early',disposition='answered')
scope='connect:collection-fixture';session='native-session';job='native-job'
sources=['opaque-a','opaque-b']
def state(can_collect=False,commissioning=False,active=None):
 return {'available':False,'can_send':False,'can_stop':False,'outcome_uncertain':False,'active_send':None,'last_completion':None,'private_notice_pending':False,'can_collect':can_collect,'commissioning':commissioning,'active_collection':active,'collection_sources':sources,'reusable_source_coverage':'unqualified_credential_continuity'}
caps={'prompt_kinds':['approval','provider_target','provider_action','provider_api_key','provider_browser','provider_device','service_secret'],'service_preparation':True,'forge_configuration':False,'paired_host':False,'tools':[],
 'service_collections':{'selection':'trusted_process_sources','requires':'successful_finalization_and_governed_reopen','result':'historical_required_operation_observations','reusable_source_coverage':'unqualified_credential_continuity','slack_inbound':'unsupported'}}
result(init['id'],{'protocol_version':5 if mode=='downgrade' else 7,'scope':scope,'session_id':session,'capabilities':caps,'state':state()})
target={'scope':scope}
event('unavailable',reason='finalizing',first_fence={'origin':'other_native_transition','scope':scope,'send_request_id':None,'cancellation_requested_before':None})
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
 event('service_collection_state',state='available',job_id=job,session_id=session,capability_verified=False)
 event('state',state=state(can_collect=True))
if mode=='logout':event('unavailable',reason='viewer_retired')
if mode=='eof':sys.exit(0)
if mode=='backpressure':
 p.with_suffix('.blocked').write_text('ready')
 while not p.with_suffix('.resume').exists():time.sleep(.01)
last=None

def report(v,status='tool_completed'):
 b=init['params'];resource={'repository':'private-resource','nested':['selected']}
 native={'profile_id':'required-read','application_id':b['application']['application_id'],'instance_id':b['application']['instance_id'],'login_scope_id':b['viewer']['login_scope_id'],'viewer_id':b['viewer']['principal_id'],'origin':b['application']['origin'],'realm':b['realm']['realm_id'],'session_id':session,'job_id':v['params']['job_id'],'server':'native-server','agent':'native-agent','operation':'read','resource':resource,'status':status,'input_id':'native-input','run_id':'native-run','tool_call_id':'native-call','result_sha256':'a'*64,'installed_connection_sha256':'b'*64,'dispatch_outcome':'committed','native_terminal_committed':True,'native_close_joined':True}
 call={'source_id':v['params']['source_id'],'required_operation_id':'required-read','resource':resource,'ordinal':1,'raw_operation':'read','arguments_sha256':'c'*64,'native':native}
 row={'required_operation_id':'required-read','resource':resource,'raw_operation_ceiling':['read'],'assessment':'verified','calls':[call]}
 r={k:native[k] for k in ('application_id','instance_id','viewer_id','login_scope_id','origin','realm','session_id','job_id','agent')}
 r.update(source_id=v['params']['source_id'],attempt_id='00000000-0000-4000-8000-00000000000'+('1' if v['params']['source_id']=='opaque-a' else '2'),family='family',profile='default',status='historical_checks_completed',requirements=[row],observed_selection='rechecked_after_last_close',native_own_config_generation=1,reusable_source_coverage='unqualified_credential_continuity',limits={'calls':3,'argument_bytes':1024,'result_bytes':1024},total_calls=1,total_argument_bytes=2,total_result_bytes=64)
 if status=='cancelled':r['status']='cancelled';row['assessment']=None
 if mode in ('unknown','failed','cleanup'):
  r['status']={'unknown':'outcome_unknown','failed':'tool_failed','cleanup':'cleanup_unconfirmed'}[mode];row['assessment']=None
  native['status']=r['status'];native['dispatch_outcome']={'unknown':'unknown','failed':'failed','cleanup':'committed'}[mode]
  if mode=='cleanup':native['native_close_joined']=False
 if mode=='budget':r['status']='budget_exceeded';r['total_result_bytes']=2048;row['assessment']=None
 if mode=='reserved':r['status']='program_refused';r['total_calls']=2
 if mode=='not_observed':r['status']='not_observed';r['total_calls']=0;r['total_argument_bytes']=0;r['total_result_bytes']=0;row['assessment']=None;row['calls']=[]
 if mode.startswith('foreign_'):r[mode[len('foreign_'):]]='foreign'
 if mode=='extra':r['raw_result']='PRIVATE-RAW'
 if mode=='zero_success':row['calls']=[];r['total_calls']=0
 if mode=='unknown_success':native['dispatch_outcome']='unknown'
 if mode=='uncommitted':native['native_terminal_committed']=False
 if mode=='wrong_resource':call['resource']={'repository':'other'}
 if mode=='wrong_nested_source':call['source_id']='foreign'
 if mode=='wrong_native_profile':native['profile_id']='foreign'
 if mode=='wrong_ordinal':call['ordinal']=2
 if mode=='wrong_operation':call['raw_operation']='write'
 if mode=='missing_row':r['requirements']=[]
 if mode=='false_coverage':r['reusable_source_coverage']='qualified'
 if mode.startswith('reobserve'):
  second=copy.deepcopy(call);second['ordinal']=2
  second['native'].update(input_id='input-2',run_id='run-2',tool_call_id='call-2')
  if mode in ('reobserve_unentered','reobserve_partial','reobserve_partial_replay','reobserve_unentered_replay'):
   unentered=mode in ('reobserve_unentered','reobserve_unentered_replay')
   r['status']='selection_changed' if unentered else 'cancelled';row['assessment']=None
   second['native'].update(status=r['status'],input_id=None if unentered else 'input-2',run_id=None,tool_call_id=None,result_sha256=None,dispatch_outcome=None,native_terminal_committed=False,native_close_joined=not unentered)
   if unentered:second['native']['installed_connection_sha256']=None
  if mode=='reobserve_unknown':
   r['status']='outcome_unknown';row['assessment']=None
   second['native'].update(status='outcome_unknown',dispatch_outcome='unknown',result_sha256=None,native_terminal_committed=False,native_close_joined=True)
  if mode=='reobserve_cleanup':
   r['status']='cleanup_unconfirmed';row['assessment']=None
   second['native'].update(status='cleanup_unconfirmed',result_sha256=None,native_terminal_committed=False,native_close_joined=False)
  if mode=='reobserve_replay':second['native']=copy.deepcopy(native)
  if mode=='reobserve_other_args_replay':second['native']=copy.deepcopy(native);second['arguments_sha256']='d'*64
  if mode=='reobserve_foreign_resource':second['native']['resource']={'repository':'other'}
  if mode=='reobserve_foreign_context':second['native']['realm']='foreign'
  if mode=='reobserve_false_success':second['native']['native_close_joined']=False
  if mode=='reobserve_missing_success':second['native'].update(input_id=None,run_id=None,tool_call_id=None)
  if mode=='reobserve_other_requirement_replay':
   second['native']=copy.deepcopy(native);second['required_operation_id']='required-members';second['native']['profile_id']='required-members'
  row['calls'].append(second);r.update(total_calls=2,total_argument_bytes=4,total_result_bytes=128)
  if mode=='reobserve_other_requirement_replay':
   row['calls'].pop();other=copy.deepcopy(row);other.update(required_operation_id='required-members',calls=[second]);r['requirements'].append(other)
  if mode in ('reobserve_partial_replay','reobserve_unentered_replay'):
   third=copy.deepcopy(second);third['ordinal']=3;row['calls'].append(third)
   r.update(total_calls=3,total_argument_bytes=6)
 if mode in ('multi','duplicate_call'):
  second=copy.deepcopy(call);second['ordinal']=2
  if mode=='multi':
   second['arguments_sha256']='d'*64;second['native'].update(input_id='input-2',run_id='run-2',tool_call_id='call-2')
  row['calls'].append(second)
  lastrow=copy.deepcopy(row);lastrow['required_operation_id']='required-members';lastrow['calls']=lastrow['calls'][:1]
  lastrow['calls'][0].update(required_operation_id='required-members',ordinal=3)
  lastrow['calls'][0]['native'].update(profile_id='required-members',input_id='input-3',run_id='run-3',tool_call_id='call-3')
  r['requirements'].append(lastrow);r.update(total_calls=3,total_argument_bytes=6,total_result_bytes=192)
 return r

def collection_result(v,status='tool_completed'):
 result(v['id'],{'scope':'foreign' if mode=='wrong_scope' else scope,'collection_request_id':'h:other' if mode=='wrong_id' else v['id'],'report':report(v,status),'state':state(can_collect=True)})
for line in sys.stdin:
 v=json.loads(line);record(v);method=v.get('method')
 if method=='connect/close':
  if mode=='slow_close':time.sleep(.3)
  if mode!='unconfirmed':result(v['id'],{'target':v['params']['target'],'resources_closed':True})
  break
 if method=='connect/collect':
  last=v
  event('state',state=state(active=v['id']))
  if mode in ('refusal','known_refusal'):
   emit({'jsonrpc':'2.0','id':v['id'],'error':{'code':-32010,'message':'SECRET_ERROR','data':{'code':'collection_already_attempted' if mode=='known_refusal' else 'SECRET_TOKEN'}}})
  elif mode not in ('pending','late_prompt'):
   if mode=='late_result':time.sleep(.2)
   collection_result(v)
 elif method=='connect/cancel':
  if mode=='late_prompt':
   emit({'jsonrpc':'2.0','id':'p:late','method':'connect/prompt','params':{'target':target,'prompt':{'kind':'provider_api_key','label':'SECRET_LATE'}}})
   event('prompt_closed',prompt_id='p:late',disposition='cancelled')
  result(v['id'],{'scope':scope,'send_request_id':v['params']['send_request_id'],'status':'cancellation_requested' if mode in ('pending','late_prompt') else 'not_current'})
  if mode in ('pending','late_prompt'):collection_result(last,'cancelled')
'''


class ServiceCollectionTransportTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='connect-collection-wire-test-'))
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
        self.peer.with_suffix('.frames').write_text('')
        self.peer.with_suffix('.mode').write_text(mode)
        binding = binding or self.binding
        launcher = launcher or ServiceCollectionLauncher((str(self.peer),'--host-selected'),binding.credential_scope,{'PATH':os.defpath,'LANG':'C'})
        try:
            client = ServiceCollectionClient(launcher,binding)
        except ServiceSetupLaunchError as error:
            self.clients.append(error.owner)
            raise
        self.clients.append(client)
        if initialized:
            self.until(client,lambda e:e.kind=='state' and e.payload.get('state',{}).get('can_collect') is True)
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

    def test_explicit_v7_launcher_is_immutable_and_does_not_send_authority(self):
        env={'PATH':os.defpath,'LANG':'C'};scope=dict(self.binding.credential_scope);command=[str(self.peer),'--host-selected']
        launcher=ServiceCollectionLauncher(command,scope,env)
        command.append('--changed');env['API_KEY']='private';scope['root']='/wrong'
        client=self.start(launcher=launcher)
        launch=json.loads(self.peer.with_suffix('.record').read_text())
        self.assertEqual(launch['argv'][1:],['--host-selected'])
        self.assertNotIn('API_KEY',launch['env'])
        wire=self.frames()[0]['params'];self.assertEqual(wire['protocol_version'],7)
        for key in ('credential_scope','collections','programs','operations','caller','resources'):
            self.assertNotIn(key,wire)
        with mock.patch('meerkat_toolkit.service_setup.subprocess.Popen') as spawn:
            with self.assertRaises(ValueError):ServiceCollectionClient(str(self.peer),self.binding)
            with self.assertRaises(ValueError):ServiceCollectionLauncher((str(self.peer),),self.binding.credential_scope,{'API_KEY':'private'})
            self.assertFalse(spawn.called)
        self.assertTrue(client.close(3).native_resources_closed)

    def test_no_downgrade_or_replay(self):
        client=self.start('downgrade',initialized=False)
        with self.assertRaises(ServiceSetupError):self.until(client,lambda e:e.kind=='result')
        self.assertFalse(client.close(3).native_resources_closed)
        self.assertEqual(len([v for v in self.frames() if v.get('method')=='connect/initialize']),1)

    def test_early_prompt_and_commissioning_are_separate_from_collection(self):
        client=self.start('early',initialized=False)
        prompt=self.until(client,lambda e:e.kind=='prompt')
        client.answer(prompt.request_id,{'kind':'provider_api_key','key':'PRIVATE_TEST_KEY'})
        self.until(client,lambda e:e.kind=='state' and e.payload.get('state',{}).get('can_collect'))
        self.assertEqual(self.frames()[1]['id'],'p:early');self.assertNotIn('PRIVATE_TEST_KEY',repr(prompt))
        self.assertTrue(client.close(3).native_resources_closed)
        client=self.start('commissioning',initialized=False)
        prompt=self.until(client,lambda e:e.kind=='prompt')
        with self.assertRaises(ServiceSetupError):client.submit('do not reopen setup')
        with self.assertRaises(ServiceSetupError):client.connector_cancel()
        client.answer(prompt.request_id,{'kind':'approval','decision':'approve'})
        observed=self.until(client,lambda e:e.kind=='service_commissioning')
        self.assertIsNone(observed.report);self.assertFalse(observed.payload['capability_verified'])
        self.until(client,lambda e:e.kind=='state' and e.payload.get('state',{}).get('can_collect'))

    def test_no_ready_does_not_admit_collection(self):
        client=self.start('no_ready',initialized=False)
        self.until(client,lambda e:e.kind=='service_finalization')
        with self.assertRaises(ServiceSetupError):client.collect(job_id='native-job',source_id='opaque-a')
        self.assertFalse(any(f.get('method')=='connect/collect' for f in self.frames()))

    def test_exact_sources_and_nested_immutable_historical_observation(self):
        client=self.start()
        for job,source in [('foreign','opaque-a'),('native-job','foreign')]:
            with self.assertRaises(ServiceSetupError):client.collect(job_id=job,source_id=source)
        with self.assertRaises(TypeError):client.collect(job_id='native-job',source_id='opaque-a',operations=['read'])
        identity=client.collect(job_id='native-job',source_id='opaque-a')
        event=self.completed(client,identity);report=event.report
        self.assertEqual(report.status,CollectionStatus.HISTORICAL_CHECKS_COMPLETED)
        self.assertEqual(report.reusable_source_coverage,'unqualified_credential_continuity')
        self.assertEqual(client.state.collection_sources,('opaque-a','opaque-b'))
        with self.assertRaises(dataclasses.FrozenInstanceError):client.state.can_collect=True
        self.assertEqual(report.requirements[0].calls[0].native.profile_id,'required-read')
        with self.assertRaises(TypeError):report.requirements[0].resource['nested']='changed'
        with self.assertRaises(TypeError):event.payload['result']['report']['source_id']='foreign'
        with self.assertRaises(ServiceSetupError):client.collect(job_id='native-job',source_id='opaque-a')
        self.assertEqual([f['params'] for f in self.frames() if f.get('method')=='connect/collect'],[{'scope':'connect:collection-fixture','job_id':'native-job','source_id':'opaque-a'}])

    def test_partial_unknown_failure_and_budget_facts_are_not_upgraded(self):
        for mode,expected in [('unknown','outcome_unknown'),('failed','tool_failed'),('cleanup','cleanup_unconfirmed'),('budget','budget_exceeded'),('reserved','program_refused'),('not_observed','not_observed')]:
            with self.subTest(mode=mode):
                client=self.start(mode);identity=client.collect(job_id='native-job',source_id='opaque-a')
                report=self.completed(client,identity).report
                self.assertEqual(report.status.value,expected)
                self.assertEqual(report.reusable_source_coverage,'unqualified_credential_continuity')
                if mode=='budget':self.assertGreater(report.total_result_bytes,report.limits.result_bytes)
                if mode=='reserved':self.assertGreater(report.total_calls,len(report.requirements[0].calls))
                with self.assertRaises(ServiceSetupError):client.collect(job_id='native-job',source_id='opaque-a')
                self.assertTrue(client.close(3).native_resources_closed)

    def test_closed_report_census_and_cross_call_attribution(self):
        modes=['foreign_source_id','foreign_job_id','foreign_session_id','foreign_viewer_id','wrong_scope','wrong_id','extra',
               'zero_success','unknown_success','uncommitted','wrong_resource','wrong_nested_source','wrong_native_profile',
               'wrong_ordinal','wrong_operation','missing_row','false_coverage','duplicate_call']
        for mode in modes:
            with self.subTest(mode=mode):
                client=self.start(mode);identity=client.collect(job_id='native-job',source_id='opaque-a')
                with self.assertRaises(ServiceSetupError):self.completed(client,identity)
                self.assertFalse(client.close(3).native_resources_closed)

    def test_native_read_reobservation_keeps_distinct_and_partial_observations(self):
        for mode, status in [('reobserve','historical_checks_completed'),('reobserve_unentered','selection_changed'),
                ('reobserve_partial','cancelled'),('reobserve_unknown','outcome_unknown'),
                ('reobserve_cleanup','cleanup_unconfirmed')]:
            with self.subTest(mode=mode):
                client=self.start(mode);identity=client.collect(job_id='native-job',source_id='opaque-a')
                report=self.completed(client,identity).report
                self.assertEqual(report.status.value,status)
                first,second=report.requirements[0].calls
                self.assertEqual((first.ordinal,second.ordinal),(1,2))
                self.assertEqual(first.arguments_sha256,second.arguments_sha256)
                self.assertNotEqual(first.native.input_id,second.native.input_id)
                self.assertEqual(second.native.run_id,None if mode in ('reobserve_unentered','reobserve_partial') else 'run-2')
                self.assertEqual(second.native.tool_call_id,None if mode in ('reobserve_unentered','reobserve_partial') else 'call-2')
                self.assertEqual(report.requirements[0].assessment,'verified' if mode=='reobserve' else None)
                self.assertEqual(report.reusable_source_coverage,'unqualified_credential_continuity')
                self.assertEqual(len([f for f in self.frames() if f.get('method')=='connect/collect']),1)
                with self.assertRaises(ServiceSetupError):client.collect(job_id='native-job',source_id='opaque-a')
                self.assertTrue(client.close(3).native_resources_closed)

    def test_read_reobservation_does_not_allow_replayed_or_foreign_observations(self):
        for mode in ('reobserve_replay','reobserve_other_args_replay','reobserve_other_requirement_replay',
                'reobserve_partial_replay','reobserve_unentered_replay','reobserve_foreign_resource',
                'reobserve_foreign_context','reobserve_false_success','reobserve_missing_success'):
            with self.subTest(mode=mode):
                client=self.start(mode);identity=client.collect(job_id='native-job',source_id='opaque-a')
                with self.assertRaises(ServiceSetupError):self.completed(client,identity)
                self.assertFalse(client.close(3).native_resources_closed)

    def test_active_cancel_retires_and_preserves_partial_result(self):
        client=self.start('pending');identity=client.collect(job_id='native-job',source_id='opaque-a')
        cancel=client.cancel(identity)
        with self.assertRaises(ServiceSetupError):client.collect(job_id='native-job',source_id='opaque-b')
        self.completed(client,cancel)
        event=self.completed(client,identity)
        self.assertEqual(event.report.status,CollectionStatus.CANCELLED)
        self.assertFalse(event.viewer_current_at_observation)
        self.assertTrue(client.close(3).native_resources_closed)

    def test_only_latest_completed_collection_can_cancel(self):
        client=self.start();first=client.collect(job_id='native-job',source_id='opaque-a');self.completed(client,first)
        second=client.collect(job_id='native-job',source_id='opaque-b');self.completed(client,second)
        with self.assertRaises(ServiceSetupError):client.cancel(first)
        cancel=client.cancel(second);self.completed(client,cancel)
        with self.assertRaises(ServiceSetupError):client.submit('no restart')

    def test_late_prompt_is_nonactionable_after_cancel(self):
        client=self.start('late_prompt');identity=client.collect(job_id='native-job',source_id='opaque-a');client.cancel(identity)
        event=self.until(client,lambda e:e.kind=='retired_prompt')
        self.assertNotIn('SECRET_LATE',repr(event))
        with self.assertRaises(ServiceSetupError):client.answer(event.request_id,{'kind':'provider_api_key','key':'private'})
        self.completed(client,identity)

    def test_refusal_logout_and_expiry_do_not_replay(self):
        client=self.start('refusal');identity=client.collect(job_id='native-job',source_id='opaque-a')
        event=self.completed(client,identity);self.assertEqual(event.kind,'refused');self.assertIsNone(event.report)
        self.assertNotIn('SECRET',repr(event));self.assertNotIn('SECRET',str(event.payload))
        with self.assertRaises(ServiceSetupError):client.collect(job_id='native-job',source_id='opaque-a')
        self.assertTrue(client.close(3).native_resources_closed)
        client=self.start('logout',initialized=False)
        self.until(client,lambda e:e.kind=='unavailable' and e.payload.get('reason')=='viewer_retired')
        with self.assertRaises(ServiceSetupError):client.collect(job_id='native-job',source_id='opaque-a')
        self.assertTrue(client.close(3).native_resources_closed)
        client=self.start();client._deadline=int(time.time()*1000)-1
        with self.assertRaises(ServiceSetupError):client.collect(job_id='native-job',source_id='opaque-a')

    def test_close_timeout_retains_same_process_and_threads(self):
        client=self.start('slow_close');process=client._process;reader=client._reader;writer=client._writer
        with self.assertRaises(TimeoutError):client.close(.01)
        self.assertIs(client._process,process);self.assertIs(client._reader,reader);self.assertIs(client._writer,writer)
        result=client.close(3);self.assertTrue(result.process_joined)
        self.assertFalse(reader.is_alive());self.assertFalse(writer.is_alive())

    def test_only_closed_collection_refusal_codes_survive_redaction(self):
        client=self.start('known_refusal');identity=client.collect(job_id='native-job',source_id='opaque-a')
        event=self.completed(client,identity)
        self.assertEqual(event.payload['code'],'collection_already_attempted')
        self.assertNotIn('SECRET_ERROR',str(event.payload))

    def test_multiple_calls_and_required_rows_keep_exact_attribution(self):
        client=self.start('multi');identity=client.collect(job_id='native-job',source_id='opaque-a')
        report=self.completed(client,identity).report
        self.assertEqual(report.total_calls,3)
        self.assertEqual([len(r.calls) for r in report.requirements],[2,1])
        self.assertEqual([c.ordinal for r in report.requirements for c in r.calls],[1,2,3])
        self.assertEqual(report.requirements[1].calls[0].native.profile_id,'required-members')

    def test_closed_early_state_refuses_authority_or_active_owner(self):
        for mode in ('bad_state_extra','bad_state_coverage','bad_state_active'):
            with self.subTest(mode=mode):
                client=self.start(mode,initialized=False)
                with self.assertRaises(ServiceSetupError):self.until(client,lambda e:e.kind=='result')
                self.assertFalse(client.close(3).native_resources_closed)
