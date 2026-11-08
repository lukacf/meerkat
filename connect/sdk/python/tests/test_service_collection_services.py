"""Multiple setup phases on one real test pipe; no native receipt or grant proof."""
import json
import sys
import unittest
from unittest import mock
from meerkat_toolkit.service_setup import ServiceSetupError
import test_service_collection as fixtures
import test_service_collection_adoption as adopted_fixtures

# Reuse the bounded test peer's framing/report fixture, adding a second phase.
MULTI = fixtures.PEER.replace("mode=p.with_suffix('.mode').read_text()", "mode=p.with_suffix('.mode').read_text()\nservice_index=0\nobserved_sources=set()")
MULTI = MULTI.replace("event('state',state=early)", "early.update(service_ids=['official','python'],service_id='official',can_open_next_service=False)\nevent('state',state=early)", 1)
a = MULTI.index('def state('); b = MULTI.index('caps=', a)
MULTI = MULTI[:a] + '''def state(can_collect=False,commissioning=False,active=None):
 return {'available':False,'can_send':False,'can_stop':False,'outcome_uncertain':False,'active_send':None,'last_completion':None,'private_notice_pending':False,'can_collect':can_collect,'commissioning':commissioning,'active_collection':active,'collection_sources':sources,'reusable_source_coverage':'unqualified_credential_continuity',
 'service_ids':['official','python'],'service_id':['official','python'][service_index],
 'can_open_next_service':service_index==0 and 'opaque-a' in observed_sources and can_collect}
''' + MULTI[b:]
MULTI = MULTI.replace(" if method=='connect/collect':\n  last=v", " if method=='connect/collect':\n  observed_sources.add(v['params']['source_id'])\n  last=v")
MULTI = MULTI.replace(" if method=='connect/collect':", ''' if method=='connect/collection/open_next':
  # Deliberately deliver a prior phase state after the client queued transition.
  event('state',state=state(can_collect=True))
  if mode=='refuse_next':
   emit({'jsonrpc':'2.0','id':v['id'],'error':{'code':-32010,'message':'refused','data':{'code':'collection_unavailable'}}});continue
  service_index=1
  session='native-session-2';job='native-job-2'
  event('state',state=state())
  emit({'jsonrpc':'2.0','id':'p:second','method':'connect/prompt','params':{'target':target,'prompt':{'kind':'provider_api_key','label':'Second setup'}}})
  answer=json.loads(sys.stdin.readline());record(answer)
  event('prompt_closed',prompt_id='p:second',disposition='answered')
  result(v['id'],{'scope':'foreign' if mode=='wrong_next_scope' else scope,'service_id':'foreign' if mode=='wrong_next_service' else 'python','session_id':'native-session' if mode=='reused_session' else session,'state':dict(state(),service_id='official') if mode=='wrong_next_state' else state()})
  event('unavailable',reason='finalizing',first_fence={'origin':'other_native_transition','scope':scope,'send_request_id':None,'cancellation_requested_before':None})
  event('service_collection_state',state='available',job_id='native-job' if mode=='reused_job' else job,session_id=session,capability_verified=False)
  event('state',state=state(can_collect=True))
  continue
 if method=='connect/collect':''')

# Existing approval frames may precede the governed next-service response.
STARTUP_REVIEW_PEER = MULTI.replace(
    "{'kind':'provider_api_key','label':'Second setup'}",
    "{'kind':'approval','summary':'Review next service setup','body':{'change':'review_next_service_setup'}}")
STARTUP_REVIEW_PEER = STARTUP_REVIEW_PEER.replace(
    "  event('prompt_closed',prompt_id='p:second',disposition='answered')",
    """  event('prompt_closed',prompt_id='p:second',disposition='answered')
  if answer['result']['answer']['decision']=='deny':
   emit({'jsonrpc':'2.0','id':v['id'],'error':{'code':-32010,'message':'refused','data':{'code':'connector_unavailable'}}});continue""")

# Exercise diagnostic compatibility and malformed observations on the real
# test pipe. These are transport controls, not native receipts or installed
# acceptance evidence.
MULTI = MULTI.replace(
    "def event(kind,**fields):emit({'jsonrpc':'2.0','method':'connect/event','params':{'target':target,'kind':kind,**fields}})",
    '''def event(kind,**fields):
 if kind=='unavailable' and fields.get('reason')=='finalizing':
  if mode.startswith('future_fence'):fields['first_fence']['origin']='future_native_transition_v8'
  if mode=='boundary_ascii':fields['first_fence']['origin']='x'*256
  if mode=='boundary_multibyte':fields['first_fence']['origin']='é'*128
  if mode=='legacy_fence':fields.pop('first_fence')
  if service_index==0:
   invalid={'first_bad_fence_origin':[], 'first_null_fence_origin':None,
            'first_empty_fence_origin':'', 'first_long_fence_origin':'x'*257,
            'first_control_fence_origin':chr(10), 'first_multibyte_long_fence_origin':'é'*128+'x'}
   if mode in invalid:fields['first_fence']['origin']=invalid[mode]
   if mode=='first_bad_fence_scope':fields['first_fence']['scope']='foreign'
   if mode=='first_bad_fence_send':fields['first_fence']['send_request_id']=3
   if mode=='first_bad_fence_cancel':fields['first_fence']['cancellation_requested_before']='false'
   if mode=='first_missing_fence_origin':fields['first_fence'].pop('origin')
   if mode=='first_extra_fence_field':fields['first_fence']['authority']=True
 if service_index==1:
  if kind=='unavailable' and fields.get('reason')=='finalizing':
   if mode=='bad_fence_origin':fields['first_fence']['origin']=[]
   if mode=='bad_fence_scope':fields['first_fence']['scope']='foreign'
   if mode=='changed_fence':fields['first_fence']['origin']='other_native_failure'
  if kind=='state' and fields['state']['can_collect']:
   if mode=='changed_services':fields['state']['service_ids']=['other','python']
   if mode=='missing_services':
    for key in ('service_ids','service_id','can_open_next_service'):fields['state'].pop(key)
   if mode=='next_after_last':fields['state']['can_open_next_service']=True
 emit({'jsonrpc':'2.0','method':'connect/event','params':{'target':target,'kind':kind,**fields}})
''')

MULTI = MULTI.replace("if mode!='no_ready':", "if mode not in ('no_ready','future_fence_no_ready'):")
MULTI = MULTI.replace("if mode!='unconfirmed':", "if mode not in ('unconfirmed','future_fence_no_ready'):")

# Native begin_collection refuses a foreign service source with only an error,
# before any active observation. The synthetic admitted control also resets state.
REJECTED_SOURCE_PEER = MULTI.replace(
    " if method=='connect/collect':\n  observed_sources.add",
    """ if method=='connect/collect' and service_index==0 and v['params']['source_id']=='opaque-b':
  if mode=='rejected_after_admission':event('state',state=state(active=v['id']))
  code={'rejected_unavailable':'collection_unavailable','rejected_already':'collection_already_attempted','rejected_unknown':'PRIVATE_ERROR'}.get(mode,'invalid_binding')
  emit({'jsonrpc':'2.0','id':v['id'],'error':{'code':-32010,'message':'PRIVATE_ERROR','data':{'code':code}}})
  if mode=='rejected_after_admission':event('state',state=state(can_collect=True))
  continue
 if method=='connect/collect':
  observed_sources.add""")

# One future source is refused in two earlier phases. All three source IDs stay
# visible throughout; the peer alone decides which service can admit a source.
THREE_SERVICE_PEER = MULTI.replace("['opaque-a','opaque-b']", "['opaque-a','opaque-b','opaque-c']")
THREE_SERVICE_PEER = THREE_SERVICE_PEER.replace("['official','python']", "['official','python','third']")
THREE_SERVICE_PEER = THREE_SERVICE_PEER.replace(
    "service_index==0 and 'opaque-a' in observed_sources",
    "service_index<2 and sources[service_index] in observed_sources")
THREE_SERVICE_PEER = THREE_SERVICE_PEER.replace(
    "service_index=1\n  session='native-session-2';job='native-job-2'",
    "service_index+=1\n  session='native-session-'+str(service_index+1);job='native-job-'+str(service_index+1)")
THREE_SERVICE_PEER = THREE_SERVICE_PEER.replace("else 'python','session_id'", "else ['official','python','third'][service_index],'session_id'")
THREE_SERVICE_PEER = THREE_SERVICE_PEER.replace(
    "('1' if v['params']['source_id']=='opaque-a' else '2')", "str(sources.index(v['params']['source_id'])+1)")
THREE_SERVICE_PEER = THREE_SERVICE_PEER.replace(
    " if method=='connect/collect':\n  observed_sources.add",
    """ if method=='connect/collect' and v['params']['source_id']!=sources[service_index]:
  emit({'jsonrpc':'2.0','id':v['id'],'error':{'code':-32010,'message':'refused','data':{'code':'invalid_binding'}}})
  continue
 if method=='connect/collect':
  observed_sources.add""")

# Later retirement carries the same optional historical first-fence field.
RETIREMENT_PEER = MULTI.replace("   collection_result(v)\n", """   collection_result(v)
   fence={'origin':'other_native_transition','scope':scope,'send_request_id':None,'cancellation_requested_before':None}
   if mode=='retirement_changed':fence['origin']='different_origin'
   if mode=='retirement_foreign':fence['scope']='foreign'
   if mode=='retirement_bad_type':fence['origin']=[]
   if mode=='retirement_extra':fence['authority']=True
   if mode=='retirement_missing':fence.pop('origin')
   fields={'reason':'expired',**({} if mode=='retirement_legacy' else {'first_fence':fence})}
   if mode=='retirement_bad_reason':fields['reason']=[]
   if mode=='retirement_no_reason':fields.pop('reason')
   if mode=='retirement_extra_event':fields['authority']=True
   event('unavailable',**fields)
""")

class MultipleServicePipeTests(unittest.TestCase):
    setUp = fixtures.ServiceCollectionTransportTests.setUp
    tearDown = fixtures.ServiceCollectionTransportTests.tearDown
    start = fixtures.ServiceCollectionTransportTests.start
    until = fixtures.ServiceCollectionTransportTests.until
    completed = fixtures.ServiceCollectionTransportTests.completed
    frames = fixtures.ServiceCollectionTransportTests.frames

    def multi(self, mode='normal'):
        self.peer.write_text('#!' + sys.executable + '\n' + MULTI)
        client = self.start(mode)
        first = self.completed(client, client.collect(job_id='native-job', source_id='opaque-a'))
        return client, first

    def transition(self, client, service_id='python'):
        request = client.open_next_service(service_id=service_id)
        prompt = self.until(client, lambda e: e.kind == 'prompt')
        client.answer(prompt.request_id, {'kind':'provider_api_key','key':'TEST_ONLY'})
        return request

    def test_governed_startup_review_precedes_result_without_replacing_prior_report(self):
        for decision in ('approve', 'deny'):
            with self.subTest(decision=decision):
                self.peer.write_text('#!' + sys.executable + '\n' + STARTUP_REVIEW_PEER)
                client = self.start()
                first = self.completed(client, client.collect(job_id='native-job', source_id='opaque-a'))
                original = first.report
                pid = client._process.pid
                request = client.open_next_service(service_id='python')
                prompt = self.until(client, lambda e: e.kind == 'prompt')
                self.assertEqual(prompt.payload['kind'], 'approval')
                self.assertEqual(client.state.service_id, 'python')
                self.assertFalse(client.state.can_collect)
                with self.assertRaises(ServiceSetupError): client.submit('before setup approval')
                client.answer(prompt.request_id, {'kind':'approval','decision':decision})
                result = self.completed(client, request)
                self.assertEqual(result.kind, 'refused' if decision == 'deny' else 'result')
                self.assertIs(first.report, original)
                self.assertEqual((original.session_id, original.job_id), ('native-session','native-job'))
                self.assertEqual(client._process.pid, pid)
                if decision == 'deny':
                    self.assertFalse(client.viewer_current)
                    with self.assertRaises(ServiceSetupError): client.open_next_service(service_id='python')
                self.assertTrue(client.close(3).native_resources_closed)

    def rejected_source(self, mode):
        self.peer.write_text('#!' + sys.executable + '\n' + REJECTED_SOURCE_PEER)
        client = self.start(mode)
        request = client.collect(job_id='native-job', source_id='opaque-b')
        event = self.completed(client, request)
        self.assertEqual(event.kind, 'refused')
        self.assertIsNone(event.report)
        if mode == 'rejected_after_admission':
            self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
        self.assertTrue(client.state.can_collect)
        self.assertTrue(client.viewer_current)
        with self.assertRaises(ServiceSetupError):
            client.collect(job_id='native-job', source_id='opaque-b')
        with self.assertRaises(ServiceSetupError):
            client.open_next_service(service_id='python')
        first = self.completed(client, client.collect(job_id='native-job', source_id='opaque-a'))
        self.completed(client, self.transition(client))
        self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
        return client, event, first

    def test_pre_admission_refusal_does_not_burn_later_service_source(self):
        client, refused, first = self.rejected_source('rejected_before_admission')
        self.assertEqual(refused.payload['code'], 'invalid_binding')
        pid = client._process.pid
        second = self.completed(client, client.collect(job_id='native-job-2', source_id='opaque-b'))
        self.assertEqual((first.report.source_id, first.report.job_id), ('opaque-a', 'native-job'))
        self.assertEqual((second.report.source_id, second.report.job_id), ('opaque-b', 'native-job-2'))
        for source in ('opaque-a', 'opaque-b'):
            with self.assertRaises(ServiceSetupError):
                client.collect(job_id='native-job-2', source_id=source)
        self.assertEqual(client._process.pid, pid)
        self.assertTrue(client.close(3).native_resources_closed)
        self.assertEqual([(v['params']['job_id'], v['params']['source_id']) for v in self.frames() if v.get('method') == 'connect/collect'],
                         [('native-job','opaque-b'), ('native-job','opaque-a'), ('native-job-2','opaque-b')])

    def test_admitted_or_other_refusals_remain_burned_across_services(self):
        for mode in ('rejected_after_admission', 'rejected_unavailable', 'rejected_already', 'rejected_unknown'):
            with self.subTest(mode=mode):
                client, refused, first = self.rejected_source(mode)
                with self.assertRaises(ServiceSetupError):
                    client.collect(job_id='native-job-2', source_id='opaque-b')
                self.assertEqual(first.report.source_id, 'opaque-a')
                self.assertIsNone(refused.report)
                self.assertTrue(client.close(3).native_resources_closed)
                self.assertEqual(sum(v.get('method') == 'connect/collect' for v in self.frames()), 2)

    def test_future_source_refused_in_two_jobs_is_collectable_in_its_own_third_service(self):
        self.peer.write_text('#!' + sys.executable + '\n' + THREE_SERVICE_PEER)
        client = self.start()
        pid = client._process.pid
        reports = []
        for index, (service, job, source) in enumerate([
                ('official', 'native-job', 'opaque-a'),
                ('python', 'native-job-2', 'opaque-b'),
                ('third', 'native-job-3', 'opaque-c')]):
            self.assertEqual(client.state.service_id, service)
            self.assertEqual(client.state.collection_sources, ('opaque-a', 'opaque-b', 'opaque-c'))
            if index < 2:
                refused = self.completed(client, client.collect(job_id=job, source_id='opaque-c'))
                self.assertEqual((refused.kind, refused.payload['code'], refused.report), ('refused', 'invalid_binding', None))
                # No follow-up state frame is sent. Recovery uses the refusal.
                self.assertTrue(client.viewer_current)
                self.assertTrue(client.state.can_collect)
                with self.assertRaises(ServiceSetupError):
                    client.collect(job_id=job, source_id='opaque-c')
            completed = self.completed(client, client.collect(job_id=job, source_id=source))
            reports.append(completed.report)
            self.assertEqual((completed.report.job_id, completed.report.source_id), (job, source))
            for earlier in reports:
                with self.assertRaises(ServiceSetupError):
                    client.collect(job_id=job, source_id=earlier.source_id)
            if index < 2:
                self.completed(client, self.transition(client, ('python', 'third')[index]))
                self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
        self.assertEqual([r.session_id for r in reports], ['native-session', 'native-session-2', 'native-session-3'])
        self.assertEqual(len({r.attempt_id for r in reports}), 3)
        self.assertEqual(len({(r.instance_id, r.login_scope_id) for r in reports}), 1)
        self.assertEqual(client._process.pid, pid)
        self.assertFalse(client.state.can_open_next_service)
        self.assertTrue(client.close(3).native_resources_closed)
        self.assertEqual(len(self.clients), 1)
        self.assertEqual(sum(v.get('method') == 'connect/initialize' for v in self.frames()), 1)
        self.assertEqual([(v['params']['job_id'], v['params']['source_id']) for v in self.frames() if v.get('method') == 'connect/collect'],
                         [('native-job', 'opaque-c'), ('native-job', 'opaque-a'),
                          ('native-job-2', 'opaque-c'), ('native-job-2', 'opaque-b'), ('native-job-3', 'opaque-c')])
        self.assertEqual([v['params']['service_id'] for v in self.frames() if v.get('method') == 'connect/collection/open_next'], ['python', 'third'])

    def test_two_services_keep_one_process_scope_and_actual_event_objects(self):
        client, first = self.multi()
        original = first.report
        pid = client._process.pid
        with mock.patch('meerkat_toolkit.service_setup.subprocess.Popen') as spawn:
            request = self.transition(client)
            self.completed(client, request)
            self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
            second = self.completed(client, client.collect(job_id='native-job-2', source_id='opaque-b'))
            spawn.assert_not_called()
        self.assertIs(first.report, original)
        self.assertEqual((original.session_id, original.job_id), ('native-session','native-job'))
        self.assertEqual((second.report.session_id, second.report.job_id), ('native-session-2','native-job-2'))
        self.assertEqual(original.instance_id, second.report.instance_id)
        self.assertEqual(original.login_scope_id, second.report.login_scope_id)
        self.assertEqual(client._process.pid, pid)
        self.assertFalse(client.state.can_open_next_service)
        with self.assertRaises(ServiceSetupError): client.open_next_service(service_id='official')
        with self.assertRaises(ServiceSetupError): client.collect(job_id='native-job-2', source_id='opaque-a')
        self.assertTrue(client.close(3).native_resources_closed)
        frames = self.frames()
        self.assertEqual(sum(f.get('method') == 'connect/initialize' for f in frames), 1)
        frame = next(f for f in frames if f.get('method') == 'connect/collection/open_next')
        self.assertEqual(frame['params'], {'scope':'connect:collection-fixture','service_id':'python'})

    def test_next_service_is_not_queued_before_collection_or_for_another_id(self):
        self.peer.write_text('#!' + sys.executable + '\n' + MULTI)
        client = self.start()
        for service in ('python','foreign','official'):
            with self.assertRaises(ServiceSetupError): client.open_next_service(service_id=service)
        self.completed(client, client.collect(job_id='native-job', source_id='opaque-a'))
        with self.assertRaises(ServiceSetupError): client.open_next_service(service_id='foreign')
        self.assertTrue(client.close(3).native_resources_closed)
        self.assertFalse(any(f.get('method') == 'connect/collection/open_next' for f in self.frames()))

    def test_native_transition_refusal_is_terminal_for_this_client_and_not_replayed(self):
        client, first = self.multi('refuse_next')
        request = client.open_next_service(service_id='python')
        event = self.completed(client, request)
        self.assertEqual(event.kind, 'refused')
        self.assertFalse(client.viewer_current)
        with self.assertRaises(ServiceSetupError): client.open_next_service(service_id='python')
        with self.assertRaises(ServiceSetupError): client.cancel(first.request_id)
        client.close(3)
        self.assertFalse(any(f.get('method') == 'connect/cancel' for f in self.frames()))
        self.assertEqual(sum(f.get('method') == 'connect/collection/open_next' for f in self.frames()), 1)

    def test_foreign_transition_or_reused_session_and_job_fail_closed(self):
        for mode in ('wrong_next_scope','wrong_next_service','wrong_next_state','reused_session','reused_job'):
            with self.subTest(mode=mode):
                client, _ = self.multi(mode)
                self.transition(client)
                with self.assertRaises(ServiceSetupError):
                    self.until(client, lambda e: e.report is not None)
                self.assertFalse(client.close(3).native_resources_closed)

    def test_collection_cannot_queue_against_buffered_prior_phase_during_transition(self):
        client, _ = self.multi()
        with client._condition:
            request = client.open_next_service(service_id='python')
            with self.assertRaises(ServiceSetupError):
                client.collect(job_id='native-job', source_id='opaque-b')
        prompt = self.until(client, lambda e: e.kind == 'prompt')
        client.answer(prompt.request_id, {'kind':'provider_api_key','key':'TEST_ONLY'})
        self.completed(client, request)
        client.close(3)
        self.assertEqual(sum(f.get('method') == 'connect/collect' for f in self.frames()), 1)

    def test_stale_cancel_during_queued_transition_cannot_retire_next_service(self):
        client, first = self.multi()
        original_pid = client._process.pid
        # Hold the same condition used by the reader, so no next-phase frame
        # can clear the preceding collection before this cancellation attempt.
        with client._condition:
            request = client.open_next_service(service_id='python')
            with self.assertRaisesRegex(ServiceSetupError, '^setup send is not current$'):
                client.cancel(first.request_id)
            self.assertTrue(client.viewer_current)
        prompt = self.until(client, lambda e: e.kind == 'prompt')
        client.answer(prompt.request_id, {'kind':'provider_api_key','key':'TEST_ONLY'})
        self.completed(client, request)
        self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
        second = self.completed(client, client.collect(job_id='native-job-2', source_id='opaque-b'))
        self.assertEqual(second.report.job_id, 'native-job-2')
        self.assertEqual(first.report.job_id, 'native-job')
        self.assertEqual(client._process.pid, original_pid)
        self.assertTrue(client.close(3).native_resources_closed)
        frames = self.frames()
        self.assertFalse(any(f.get('method') == 'connect/cancel' for f in frames))
        self.assertEqual(sum(f.get('method') == 'connect/collection/open_next' for f in frames), 1)

    def test_second_phase_cannot_change_first_fence_or_selected_service_order(self):
        for mode in ('bad_fence_origin', 'bad_fence_scope', 'changed_fence',
                     'changed_services', 'missing_services', 'next_after_last'):
            with self.subTest(mode=mode):
                client, first = self.multi(mode)
                original_report = first.report
                with self.assertRaises(ServiceSetupError):
                    request = self.transition(client)
                    self.completed(client, request)
                    self.until(client, lambda e: e.kind == 'state'
                               and e.payload['state']['can_collect'])
                self.assertFalse(client.viewer_current)
                with self.assertRaises(ServiceSetupError):
                    client.collect(job_id='native-job-2', source_id='opaque-b')
                with self.assertRaises(ServiceSetupError):
                    client.open_next_service(service_id='python')
                self.assertFalse(client.close(3).native_resources_closed)
                self.assertEqual((original_report.session_id, original_report.job_id),
                                 ('native-session', 'native-job'))
                frames = self.frames()
                self.assertEqual(sum(f.get('method') == 'connect/initialize' for f in frames), 1)
                self.assertEqual(sum(f.get('method') == 'connect/collection/open_next' for f in frames), 1)
                self.assertEqual(sum(f.get('method') == 'connect/collect' for f in frames), 1)

    def test_initial_fence_rejects_malformed_diagnostics_before_collection(self):
        # No earlier fence exists, so equality with a retained observation
        # cannot accidentally mask a missing origin or scope check.
        for mode in ('first_bad_fence_origin', 'first_null_fence_origin',
                     'first_empty_fence_origin', 'first_long_fence_origin',
                     'first_control_fence_origin', 'first_bad_fence_scope',
                     'first_bad_fence_send', 'first_bad_fence_cancel',
                     'first_missing_fence_origin', 'first_extra_fence_field', 'first_multibyte_long_fence_origin'):
            with self.subTest(mode=mode):
                self.peer.write_text('#!' + sys.executable + '\n' + MULTI)
                client = self.start(mode, initialized=False)
                with self.assertRaises(ServiceSetupError):
                    self.until(client, lambda e: e.kind == 'state'
                               and e.payload['state']['can_collect'])
                self.assertFalse(client.viewer_current)
                self.assertFalse(client.close(3).native_resources_closed)
                self.assertFalse(any(f.get('method') in ('connect/collect', 'connect/collection/open_next')
                                     for f in self.frames()))


    def test_unknown_diagnostic_origin_survives_both_service_phases(self):
        self.peer.write_text('#!' + sys.executable + '\n' + MULTI)
        client = self.start('future_fence', initialized=False)
        first_fence = self.until(client, lambda e: e.kind == 'unavailable').payload['first_fence']
        self.assertEqual(first_fence['origin'], 'future_native_transition_v8')
        self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
        first = self.completed(client, client.collect(job_id='native-job', source_id='opaque-a'))
        original_pid = client._process.pid
        self.completed(client, self.transition(client))
        second_fence = self.until(client, lambda e: e.kind == 'unavailable').payload['first_fence']
        self.assertEqual(second_fence, first_fence)
        self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
        second = self.completed(client, client.collect(job_id='native-job-2', source_id='opaque-b'))
        self.assertEqual(second.report.job_id, 'native-job-2')
        self.assertEqual(first.report.job_id, 'native-job')
        self.assertEqual(client._process.pid, original_pid)
        self.assertTrue(client.close(3).native_resources_closed)

    def test_first_fence_accepts_exact_utf8_byte_boundary_in_both_phases(self):
        for mode, origin in [('boundary_ascii', 'x'*256), ('boundary_multibyte', 'é'*128)]:
            with self.subTest(mode=mode):
                self.peer.write_text('#!' + sys.executable + '\n' + MULTI)
                client = self.start(mode, initialized=False)
                first = self.until(client, lambda e: e.kind == 'unavailable').payload['first_fence']
                self.assertEqual(first['origin'], origin)
                self.assertEqual(len(first['origin'].encode('utf-8')), 256)
                self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
                self.completed(client, client.collect(job_id='native-job', source_id='opaque-a'))
                self.completed(client, self.transition(client))
                second = self.until(client, lambda e: e.kind == 'unavailable').payload['first_fence']
                self.assertEqual(second, first)
                self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
                self.completed(client, client.collect(job_id='native-job-2', source_id='opaque-b'))
                self.assertTrue(client.close(3).native_resources_closed)

    def test_later_retirement_keeps_valid_or_omitted_diagnostic(self):
        for mode in ('retirement_same', 'retirement_legacy'):
            with self.subTest(mode=mode):
                self.peer.write_text('#!' + sys.executable + '\n' + RETIREMENT_PEER)
                client = self.start(mode)
                completed = self.completed(client, client.collect(job_id='native-job', source_id='opaque-a'))
                retired = self.until(client, lambda e: e.kind == 'unavailable' and e.payload['reason'] == 'expired')
                if mode == 'retirement_same':
                    self.assertEqual(retired.payload['first_fence']['origin'], 'other_native_transition')
                else:
                    self.assertNotIn('first_fence', retired.payload)
                self.assertEqual(completed.report.job_id, 'native-job')
                self.assertFalse(client.viewer_current)
                with self.assertRaises(ServiceSetupError):client.open_next_service(service_id='python')
                self.assertTrue(client.close(3).native_resources_closed)

    def test_later_retirement_rejects_malformed_or_changed_diagnostic(self):
        for mode in ('retirement_changed', 'retirement_foreign', 'retirement_bad_type', 'retirement_extra', 'retirement_missing', 'retirement_bad_reason', 'retirement_no_reason', 'retirement_extra_event'):
            with self.subTest(mode=mode):
                self.peer.write_text('#!' + sys.executable + '\n' + RETIREMENT_PEER)
                client = self.start(mode)
                completed = self.completed(client, client.collect(job_id='native-job', source_id='opaque-a'))
                with self.assertRaisesRegex(ServiceSetupError, 'transport failed'):
                    self.until(client, lambda e: e.kind == 'unavailable')
                self.assertEqual(completed.report.job_id, 'native-job')
                self.assertFalse(client.close(3).native_resources_closed)

    def test_unknown_diagnostic_does_not_authorize_collection_or_confirm_close(self):
        self.peer.write_text('#!' + sys.executable + '\n' + MULTI)
        client = self.start('future_fence_no_ready', initialized=False)
        fence = self.until(client, lambda e: e.kind == 'unavailable').payload['first_fence']
        self.assertEqual(fence['origin'], 'future_native_transition_v8')
        self.until(client, lambda e: e.kind == 'service_finalization'
                   and e.payload['state'] == 'observed')
        self.assertTrue(client.viewer_current)
        with self.assertRaises(ServiceSetupError):
            client.collect(job_id='native-job', source_id='opaque-a')
        with self.assertRaises(ServiceSetupError):
            client.open_next_service(service_id='python')
        self.assertFalse(client.close(3).native_resources_closed)
        self.assertFalse(any(f.get('method') == 'connect/collect' for f in self.frames()))

    def test_legacy_peer_without_first_fence_remains_compatible(self):
        client, first = self.multi('legacy_fence')
        self.completed(client, self.transition(client))
        self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
        second = self.completed(client, client.collect(job_id='native-job-2', source_id='opaque-b'))
        self.assertEqual(first.report.job_id, 'native-job')
        self.assertEqual(second.report.job_id, 'native-job-2')
        self.assertTrue(client.close(3).native_resources_closed)


class AdoptedMultipleServicePipeTests(unittest.TestCase):
    setUp = adopted_fixtures.OriginalCollectionPipeTests.setUp
    tearDown = adopted_fixtures.OriginalCollectionPipeTests.tearDown
    prepare = adopted_fixtures.OriginalCollectionPipeTests.prepare
    adopt = adopted_fixtures.OriginalCollectionPipeTests.adopt
    until = adopted_fixtures.OriginalCollectionPipeTests.until

    def test_two_services_retain_original_adopted_pipe_and_single_retirement(self):
        with mock.patch.object(adopted_fixtures, 'PEER', MULTI):
            transport, process, peer, retired = self.prepare()
        original_pid = process.pid
        with mock.patch('meerkat_toolkit.service_setup.subprocess.Popen', side_effect=AssertionError('second process')):
            client = self.adopt(transport)
            request = client.collect(job_id='native-job', source_id='opaque-a')
            first = self.until(client, lambda e: e.request_id == request and e.report is not None)
            request = client.open_next_service(service_id='python')
            prompt = self.until(client, lambda e: e.kind == 'prompt')
            client.answer(prompt.request_id, {'kind':'provider_api_key','key':'TEST_ONLY'})
            self.until(client, lambda e: e.request_id == request)
            self.until(client, lambda e: e.kind == 'state' and e.payload['state']['can_collect'])
            request = client.collect(job_id='native-job-2', source_id='opaque-b')
            second = self.until(client, lambda e: e.request_id == request and e.report is not None)
            self.assertIs(client._process.stdin, process.stdin)
            self.assertIs(client._process.stdout, process.stdout)
            self.assertEqual(process.pid, original_pid)
            self.assertNotEqual(first.report.session_id, second.report.session_id)
            self.assertEqual(first.report.instance_id, second.report.instance_id)
            self.assertEqual(retired, [])
            self.assertTrue(client.close(3).native_resources_closed)
        frames = [json.loads(line) for line in peer.with_suffix('.frames').read_text().splitlines()]
        self.assertEqual(sum(f.get('method') == 'connect/initialize' for f in frames), 1)
        self.assertEqual(retired, [{'output_closed':True}])
