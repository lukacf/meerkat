#!/usr/bin/env python3
"""Actual pinned-planner/local-Git stager tests; no remote or cluster access."""
import argparse
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('gitops_staging', ROOT/'scripts/stage-gitops-proposal.py')
STAGER = importlib.util.module_from_spec(SPEC); SPEC.loader.exec_module(STAGER)
PLANNER_INPUT = os.environ.get('TOOLKIT_GITOPS_PLANNER')
PLANNER_SHA = os.environ.get('TOOLKIT_GITOPS_PLANNER_SHA256')
if not PLANNER_INPUT or not PLANNER_SHA:
    raise RuntimeError('Set both TOOLKIT_GITOPS_PLANNER (absolute executable path) and '
                       'TOOLKIT_GITOPS_PLANNER_SHA256 (expected SHA-256) to run these tests')
PLANNER = Path(PLANNER_INPUT)
if not PLANNER.is_absolute() or not STAGER.PIN.fullmatch(PLANNER_SHA):
    raise RuntimeError('Invalid TOOLKIT_GITOPS_PLANNER or TOOLKIT_GITOPS_PLANNER_SHA256')
GIT = Path('/usr/bin/git')
sha = lambda raw: hashlib.sha256(raw).hexdigest()
raw = lambda value: json.dumps(value, sort_keys=True, separators=(',',':')).encode()


class StagingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='toolkit-gitops-stager-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.source = self.root/'source'; self.source.mkdir()
        self.env = {'PATH':'/usr/bin:/bin', 'HOME':str(self.root), 'GIT_CONFIG_NOSYSTEM':'1',
                    'GIT_CONFIG_GLOBAL':'/dev/null', 'GIT_AUTHOR_NAME':'Fixture', 'GIT_AUTHOR_EMAIL':'fixture@example.invalid',
                    'GIT_COMMITTER_NAME':'Fixture', 'GIT_COMMITTER_EMAIL':'fixture@example.invalid'}
        self.git('init','-q')
        self.request = json.loads((ROOT/'docs/verification/20261004-gitops-preparation/root/request.json').read_bytes())
        for name, value in self.request['base_documents'].items():
            p=self.source/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(value)
        (self.source/'README').write_bytes(b'Unrelated committed bytes\r\n')
        self.git('add','.');self.git('commit','-qm','Fixture base')
        self.commit = self.git('rev-parse','HEAD').decode().strip()
        profile=json.loads(self.request['profile_json']);profile['source_commit']=self.commit
        self.request['profile_json']=raw(profile).decode();self.request['profile_sha256']=sha(raw(profile))
        self.git('remote','add','origin',profile['source_repository'])
        self.assertEqual(sha(PLANNER.read_bytes()),PLANNER_SHA)
        self.args=self.arguments()

    def git(self,*args):
        return subprocess.run([str(GIT),'-c','core.hooksPath=/dev/null',*args],cwd=self.source,env=self.env,
                              check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE).stdout

    def arguments(self, name='result'):
        path=self.root/(name+'-request.json');path.write_bytes(raw(self.request))
        result=subprocess.run([str(PLANNER)],input=path.read_bytes(),env=self.env,check=True,
                              stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        proposal=json.loads(result.stdout)
        return argparse.Namespace(source_repository=self.source,request=path,request_sha256=sha(path.read_bytes()),
                                  proposal_sha256=proposal['proposal_sha256'],planner=PLANNER,planner_sha256=PLANNER_SHA,
                                  git=GIT,git_sha256=sha(GIT.read_bytes()),output=self.root/name,timeout_seconds=30)

    def inventory(self):
        return {str(p.relative_to(self.source)):(sha(p.read_bytes()),p.stat().st_mode) for p in self.source.rglob('*') if p.is_file()}

    def refusal(self,args=None):
        with self.assertRaises((STAGER.Refusal,OSError,ValueError)):
            STAGER.stage(args or self.args)
        self.assertFalse(((args or self.args).output/'receipt.json').exists())

    def test_actual_stage_preserves_dirty_source_and_uses_independent_store(self):
        (self.source/'README').write_bytes(b'Dirty user bytes\n')
        (self.source/'staged').write_bytes(b'Staged user work\n');self.git('add','staged')
        (self.source/'untracked').write_bytes(b'Untracked user work\n')
        before=self.inventory();result=STAGER.stage(self.args)
        self.assertEqual(result['status'],'staged_proposal_only');self.assertEqual(before,self.inventory())
        checkout=self.args.output/'checkout'
        self.assertEqual((checkout/'README').read_bytes(),b'Unrelated committed bytes\r\n')
        self.assertFalse((checkout/'staged').exists());self.assertFalse((checkout/'untracked').exists())
        self.assertFalse((checkout/'.git/objects/info/alternates').exists())
        self.source.rename(self.root/'source-offline')
        observed=subprocess.run([str(GIT),'fsck','--full','--strict'],cwd=checkout,env=self.env,capture_output=True)
        self.assertEqual(observed.returncode,0,observed.stderr)
        self.assertEqual(json.loads((self.args.output/'receipt.json').read_bytes()),result)
        self.assertEqual(sha((self.args.output/'staged.patch').read_bytes()),result['patch_sha256'])

    def test_cli_outputs_only_staging_status(self):
        argv=[sys.executable,str(ROOT/'scripts/stage-gitops-proposal.py')]
        for key,value in vars(self.args).items(): argv.extend(['--'+key.replace('_','-'),str(value)])
        result=subprocess.run(argv,env=self.env,capture_output=True,timeout=35)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['status'],'staged_proposal_only')
        self.assertEqual(result.stderr,b'')

    def test_existing_output_is_never_replaced(self):
        self.args.output.mkdir();(self.args.output/'keep').write_bytes(b'unchanged')
        self.refusal();self.assertEqual((self.args.output/'keep').read_bytes(),b'unchanged')

    def test_wrong_pins_refuse_before_output(self):
        for field in ['request_sha256','planner_sha256','git_sha256']:
            args=copy.copy(self.args);setattr(args,field,'0'*64);self.refusal(args)
            self.assertFalse(args.output.exists())

    def test_wrong_proposal_or_origin_refuses(self):
        self.args.proposal_sha256='0'*64;self.refusal()
        other=self.arguments('second');self.git('remote','set-url','origin','https://other.invalid/repo')
        self.refusal(other)

    def test_mismatched_base_is_refused_without_writing_source(self):
        self.request['base_documents']['deploy/service.json']+=' '
        profile=json.loads(self.request['profile_json']);profile['deployment']['sha256']=sha(self.request['base_documents']['deploy/service.json'].encode())
        self.request['profile_json']=raw(profile).decode();self.request['profile_sha256']=sha(raw(profile))
        args=self.arguments('mismatch');before=self.inventory();self.refusal(args);self.assertEqual(before,self.inventory())

    def test_source_unsupported_alternates_shallow_and_promisor(self):
        for index,name in enumerate(['objects/info/alternates','shallow','objects/pack/empty.promisor']):
            path=self.source/'.git'/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(b'')
            args=copy.copy(self.args);args.output=self.root/('bad'+str(index));self.refusal(args);path.unlink()
            self.assertFalse(args.output.exists())

    def test_symlink_commit_refuses_before_materialization(self):
        (self.source/'link').symlink_to('/private/tmp/outside');self.git('add','link');self.git('commit','-qm','Fixture symlink')
        profile=json.loads(self.request['profile_json']);profile['source_commit']=self.git('rev-parse','HEAD').decode().strip()
        self.request['profile_json']=raw(profile).decode();self.request['profile_sha256']=sha(raw(profile))
        self.refusal(self.arguments('symlink'))
        self.assertFalse((self.root/'symlink/checkout/link').exists())

    def test_output_cannot_overlap_source(self):
        self.args.output=self.source/'new';self.refusal();self.assertFalse(self.args.output.exists())

    def test_fifo_request_refuses_without_blocking(self):
        self.args.request.unlink();os.mkfifo(self.args.request)
        start=time.monotonic();self.refusal()
        self.assertLess(time.monotonic()-start,1)
        self.assertFalse(self.args.output.exists())

    def test_request_changed_during_planner_call_withholds_receipt(self):
        run=STAGER.Runner.run
        def mutate(runner,argv,*args,**kwargs):
            result=run(runner,argv,*args,**kwargs)
            if str(argv[0])==str(PLANNER):self.args.request.write_bytes(b'{}')
            return result
        before=self.inventory()
        with mock.patch.object(STAGER.Runner,'run',mutate):self.refusal()
        self.assertEqual(before,self.inventory())
        self.assertTrue((self.args.output/'failure.json').is_file())

    def test_mutation_before_receipt_withholds_success(self):
        before=self.inventory();write=STAGER.write_new
        def interrupt(path,*args,**kwargs):
            if path.name=='receipt.pending': raise STAGER.Refusal('injected_interrupt')
            return write(path,*args,**kwargs)
        with mock.patch.object(STAGER,'write_new',interrupt):self.refusal()
        self.assertEqual(before,self.inventory());self.assertTrue((self.args.output/'failure.json').is_file())
        self.assertTrue((self.args.output/'checkout').is_dir())

    def test_post_publication_interruption_retains_complete_receipt(self):
        link=os.link
        def interrupt(*args,**kwargs):
            link(*args,**kwargs)
            raise STAGER.Refusal('injected_lost_acknowledgement')
        with mock.patch.object(STAGER.os,'link',interrupt):
            with self.assertRaises(STAGER.Refusal):STAGER.stage(self.args)
        receipt=json.loads((self.args.output/'receipt.json').read_bytes())
        self.assertEqual(receipt['status'],'staged_proposal_only')
        self.assertEqual(sha((self.args.output/'staged.patch').read_bytes()),receipt['patch_sha256'])
        self.assertTrue((self.args.output/'failure.json').is_file())

    def test_one_deadline_and_child_output_bound(self):
        runner=STAGER.Runner(GIT,time.monotonic()+0.1,self.root)
        with self.assertRaises(STAGER.Refusal):runner.run([sys.executable,'-c','import time; time.sleep(30)'])
        runner=STAGER.Runner(GIT,time.monotonic()+5,self.root)
        with self.assertRaises(STAGER.Refusal):runner.run([sys.executable,'-c','print("x"*10000)'],limit=100)
        for call in runner.calls:
            with self.assertRaises(ProcessLookupError):os.kill(call['pid'],0)
            self.assertTrue(call['process_reaped'])


if __name__=='__main__':unittest.main(verbosity=2)
