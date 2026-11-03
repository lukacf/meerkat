"""Fixed official route with synthetic non-executable payloads and real owner code."""
import argparse
import asyncio
import copy
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

from test_mcp_adapter import adapter, connection

backend = adapter.backend
ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT.parents[1] / 'scripts/install-library-consumer.py'
spec = importlib.util.spec_from_file_location('official_installer', INSTALLER)
owner = importlib.util.module_from_spec(spec); spec.loader.exec_module(owner)


def bundle(root):
    root.mkdir()
    binary = bytes.fromhex('cffaedfe0c0000010000000002000000') + b'INERT_NOT_AN_EXECUTABLE'
    template = {'mcpServers': {'github-selected-reads': {'command': 'ABS_GITHUB_INSTALL/github-mcp-server',
        'args': owner.OFFICIAL_ARGS, 'cwd': 'ABS_GITHUB_INSTALL', 'env': {}}}}
    component = {'catalog_id':'github', 'source_revision':'a'*40, 'artifact':'github/github-mcp-server',
        'artifact_sha256':owner.sha(binary), 'configuration_template':'github/mcp.template.json',
        'notices':'github/notices', 'selected_operations':owner.OFFICIAL_TOOLS,
        'required_secret_environment':[owner.OFFICIAL_ENV], 'runtime_profile':'fixture macOS arm64'}
    files = {'delivery.json':owner.encoded({'format':'meerkat-toolkit.official-consumers.v1','role':'service_connector','components':[component]}),
        'github/github-mcp-server':binary,'github/mcp.template.json':owner.encoded(template),
        'github/UPSTREAM-PROFILE.md':b'Fixture retained upstream guide.\n',
        'github/notices/upstream-LICENSE':b'Fixture license'}
    files.update({'github/notices/dependency-'+str(i):b'Fixture dependency notice' for i in range(77)})
    for name,data in files.items():
        path=root/name; path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
    manifest={'format':1,'files':{name:{'sha256':owner.sha(data),'bytes':len(data)} for name,data in files.items()}}
    (root/'CONTENTS.json').write_bytes(owner.encoded(manifest))
    return owner.sha((root/'CONTENTS.json').read_bytes())


class RecordingRunner(backend.Runner):
    def __init__(self, workspace):
        super().__init__(workspace);self.calls=[];self.drop=False
    def run(self, argv, *, capture=False):
        self.calls.append(list(argv))
        if Path(argv[0]).name == 'github-mcp-server': raise AssertionError('GitHub was invoked')
        code,raw=super().run(argv,capture=capture)
        if self.drop and '--destination' in argv and '--verify-existing' not in argv: return 2,b''
        return code,raw


class Fixture:
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name).resolve();self.bundle=self.root/'official'
        self.sha=bundle(self.bundle);self.controllers=[];self.count=0
        self.config=self.root/'config.json';self.config.write_bytes(backend.configuration_bytes(self.request()))
    def tearDown(self):
        for c in self.controllers: c.join()
        self.temp.cleanup()
    def request(self):
        return {'family':'official-github','profile':'selected-reads','operations':list(owner.OFFICIAL_TOOLS),
                'values':{'token_env':owner.OFFICIAL_ENV}}
    def args(self,**updates):
        result=dict(official_bundle=str(self.bundle),contents_sha256=self.sha,component='github',
            official_describe=False,check_config_only=False,verify_existing=False,
            config=str(self.config),config_sha256=owner.sha(self.config.read_bytes()),server_name='selected-github',
            destination=str(self.root/'installation'))
        result.update(updates);return argparse.Namespace(**result)
    def controller(self,**updates):
        values=dict(bundle=None,contents_sha256=None,workspace=self.root/('work'+str(self.count)),
            families=('official-github',),python=sys.executable,node=None,npm_cli=None,runner_factory=RecordingRunner,
            official_bundle=self.bundle,official_contents_sha256=self.sha,
            official_installer=INSTALLER.resolve(),official_installer_sha256=owner.sha(INSTALLER.read_bytes()))
        values.update(updates);self.count+=1
        c=backend.Controller(**values);self.controllers.append(c);return c
    def checked(self,c):
        c.prepare({'family':'official-github'});c.join()
        self.assertEqual(c.job['status'],'succeeded')
        check=c.check(self.request())
        return dict(self.request(),validation_id=check['validation_id'],configuration_sha256=check['configuration_sha256'])


class OfficialTests(Fixture,unittest.TestCase):
    def test_fixed_operation_declaration_is_owned_strict_and_subset_refused(self):
        source=owner.official_consumer(self.args(official_describe=True))['help']
        self.assertIs(source['selectable_operations'],False)
        value=backend.normalize_help('official-github','selected-reads',source,source['guide_text'])
        self.assertIs(value['selectable_operations'],False)
        for flag in (None,0,1,'false',[],{}):
            with self.subTest(flag=flag), self.assertRaises(backend.SetupError):
                backend.normalize_help('official-github','selected-reads',{**source,'selectable_operations':flag},source['guide_text'])
        legacy=dict(source);legacy.pop('selectable_operations')
        self.assertIs(backend.normalize_help('official-github','selected-reads',legacy,source['guide_text'])['selectable_operations'],True)
        self.assertIs(backend.normalize_help('official-github','selected-reads',{**source,'selectable_operations':True},source['guide_text'])['selectable_operations'],True)
        c=self.controller();c.prepare({'family':'official-github'});c.join()
        self.assertIs(c.help({'family':'official-github','profile':'selected-reads'})['selectable_operations'],False)
        with self.assertRaises(backend.SetupError):c.check({**self.request(),'operations':[owner.OFFICIAL_TOOLS[0]]})
        self.assertFalse(any('--destination' in call for call in c.runner.calls))

    def test_owner_all_actions_never_execute_or_read_credentials(self):
        with patch.object(owner.subprocess,'Popen',side_effect=AssertionError('unexpected process')), patch.dict(os.environ,{owner.OFFICIAL_ENV:'SECRET_CANARY'},clear=False):
            description=owner.official_consumer(self.args(official_describe=True))
            checked=owner.official_consumer(self.args(check_config_only=True))
            installed=owner.official_consumer(self.args())
            verified=owner.official_consumer(self.args(verify_existing=True))
        self.assertEqual(description['target'],'macos-arm64')
        self.assertEqual(description['help']['prerequisites'][0]['kind'],'host_platform')
        self.assertEqual(checked['credential_environment_references'][0]['environment_variable'],owner.OFFICIAL_ENV)
        self.assertTrue(verified['verified']);self.assertEqual(installed['command'][1:],owner.OFFICIAL_ARGS)
        self.assertNotIn('SECRET_CANARY',json.dumps([description,checked,installed,verified]))
        self.assertEqual(installed['mcp_configuration']['mcpServers']['selected-github']['env'],{})

    def test_owner_refuses_config_expansion_and_wrong_platform(self):
        for change in [dict(operations=owner.OFFICIAL_TOOLS[:1]),dict(config={'token_env':'OTHER_TOKEN'}),dict(profile='github'),dict(env={'TOKEN':'secret'})]:
            value=backend.decode(self.config.read_bytes());value.update(change)
            with self.assertRaises(owner.InstallError):owner.official_configuration(backend.encoded(value))
        with patch('platform.machine',return_value='x86_64'):
            with self.assertRaises(owner.InstallError):owner.official_consumer(self.args())
        self.assertFalse((self.root/'installation').exists())

    def test_owner_rejects_receipt_and_mcp_self_consistent_tamper(self):
        owner.official_consumer(self.args());dest=self.root/'installation'
        receipt=json.loads((dest/'ready/installed.json').read_text());mcp=json.loads((dest/'ready/mcp.json').read_text())
        mcp['mcpServers']['selected-github']['args']=['stdio','--tools','everything']
        receipt['mcp_configuration']=mcp;receipt['command']=[str(dest/'github-mcp-server'),'stdio','--tools','everything']
        (dest/'ready/installed.json').write_bytes(owner.encoded(receipt));(dest/'ready/mcp.json').write_bytes(owner.encoded(mcp))
        with self.assertRaises(owner.InstallError):owner.official_consumer(self.args(verify_existing=True))

    def test_owner_missing_notice_extra_file_pending_mode_and_bool_schema(self):
        owner.official_consumer(self.args());dest=self.root/'installation'
        for name in ['notices/dependency-1','inputs/github/notices/dependency-1']:
            p=dest/name;data=p.read_bytes();p.unlink()
            with self.assertRaises(owner.InstallError):owner.official_consumer(self.args(verify_existing=True))
            p.write_bytes(data)
        for name in ['foreign.py','pending-ready']:
            p=dest/name;p.write_text('extra')
            with self.assertRaises(owner.InstallError):owner.official_consumer(self.args(verify_existing=True))
            p.unlink()
        p=dest/'github-mcp-server';p.chmod(0o700)
        with self.assertRaises(owner.InstallError):owner.official_consumer(self.args(verify_existing=True))
        p.chmod(0o755)
        receipt=dest/'ready/installed.json';value=json.loads(receipt.read_text());value['schema_version']=True;receipt.write_bytes(owner.encoded(value))
        with self.assertRaises(owner.InstallError):owner.official_consumer(self.args(verify_existing=True))
        manifest=json.loads((self.bundle/'CONTENTS.json').read_text());manifest['format']=True
        (self.bundle/'CONTENTS.json').write_bytes(owner.encoded(manifest))
        with self.assertRaises(owner.InstallError):owner.official_consumer(self.args(official_describe=True,contents_sha256=owner.sha((self.bundle/'CONTENTS.json').read_bytes())))

    def test_owner_digest_duplicate_component_symlink_refused(self):
        with self.assertRaises(owner.InstallError):owner.official_consumer(self.args(official_describe=True,contents_sha256='b'*64))
        path=self.bundle/'github/notices/dependency-1';data=path.read_bytes();path.unlink();path.symlink_to(self.bundle/'github/notices/dependency-2')
        with self.assertRaises(OSError):owner.official_consumer(self.args(official_describe=True))
        path.unlink();path.write_bytes(data)
        delivery=self.bundle/'delivery.json';value=json.loads(delivery.read_text());value['components']*=2;delivery.write_bytes(owner.encoded(value))
        manifest=json.loads((self.bundle/'CONTENTS.json').read_text());manifest['files']['delivery.json']={'sha256':owner.sha(delivery.read_bytes()),'bytes':delivery.stat().st_size}
        (self.bundle/'CONTENTS.json').write_bytes(owner.encoded(manifest))
        with self.assertRaises(owner.InstallError):owner.official_consumer(self.args(official_describe=True,contents_sha256=owner.sha((self.bundle/'CONTENTS.json').read_bytes())))

    def test_controller_distinct_requests_and_lost_return_reconcile(self):
        c=self.controller(verifier=INSTALLER.resolve(),verifier_sha256=owner.sha(INSTALLER.read_bytes()))
        request=self.checked(c);self.assertFalse((c.workspace/'inspection/official-github').exists())
        c.runner.drop=True;job=c.install(request)['job'];c.join()
        self.assertEqual(c.job['status'],'unconfirmed');self.assertEqual(c.install(request)['job']['id'],job['id'])
        observed=c.reconcile({'job_id':job['id']});self.assertEqual(observed['observation']['status'],'matched')
        check=c.check(self.request());second=dict(self.request(),validation_id=check['validation_id'],configuration_sha256=check['configuration_sha256'])
        other=c.install(second)['job'];c.join();self.assertNotEqual(job['id'],other['id'])
        self.assertNotEqual(c.install_intents[job['id']]['destination'],c.install_intents[other['id']]['destination'])
        self.assertEqual(len([a for a in c.runner.calls if '--destination' in a and '--verify-existing' not in a]),2)
        record=backend.decode((c.workspace/'requests'/ (job['id']+'.json')).read_bytes())
        self.assertEqual(record['schema_version'],2);self.assertEqual(record['official_component'],'github')
        self.assertEqual(record['contents_sha256'],self.sha)

    def test_recovery_remains_unknown_no_parser_and_v1_isolation(self):
        c=self.controller();request=self.checked(c);job=c.install(request)['job'];c.join();c.close_admission()
        args=dict(verifier=INSTALLER.resolve(),verifier_sha256=owner.sha(INSTALLER.read_bytes()),recover_workspace=c.workspace,
            recover_request=job['id'],recover_request_sha256=job['recovery_request']['sha256'])
        r=self.controller(**args);self.assertEqual(r.runner.calls,[]);self.assertEqual(r.job['status'],'unconfirmed')
        self.assertEqual(r.reconcile({'job_id':job['id']})['observation']['status'],'matched')
        self.assertEqual(r.job['status'],'unconfirmed');self.assertEqual(len(r.runner.calls),1)
        for route,value in [('prepare',{'family':'official-github'}),('check',self.request()),('install',request)]:
            with self.assertRaises(backend.SetupError):r.dispatch('/api/'+route,value)
        path=c.workspace/'requests'/(job['id']+'.json');record=backend.decode(path.read_bytes());record['schema_version']=1;record.pop('official_component');path.write_bytes(backend.encoded(record))
        args['recover_request_sha256']=backend.digest(path.read_bytes())
        with self.assertRaises(backend.SetupError):self.controller(**args)

    def test_public_cli_metadata_and_malformed_config_are_effect_free(self):
        base=[sys.executable,'-I','-B',str(INSTALLER),'--official-bundle',str(self.bundle),'--contents-sha256',self.sha,'--component','github']
        env={'PATH':'/usr/bin:/bin','PYTHONDONTWRITEBYTECODE':'1'}
        described=subprocess.run(base+['--official-describe'],capture_output=True,env=env,timeout=10)
        self.assertEqual(described.returncode,0,described.stderr)
        self.assertEqual(json.loads(described.stdout)['target'],'macos-arm64')
        bad=b'{"profile":"SECRET_CANARY"}'
        self.config.write_bytes(bad)
        result=subprocess.run(base+['--check-config-only','--config',str(self.config),'--config-sha256',owner.sha(bad),'--server-name','github'],capture_output=True,env=env,timeout=10)
        self.assertEqual(result.returncode,2)
        self.assertNotIn(b'SECRET_CANARY',result.stdout+result.stderr)
        self.assertIn(b'no installation was attempted',result.stderr)
        self.assertFalse((self.root/'installation').exists())

    def test_fresh_process_recovers_only_exact_original_intent(self):
        c=self.controller();request=self.checked(c);job=c.install(request)['job'];c.join();c.close_admission()
        code = """
import importlib.util,json,pathlib,sys
p=pathlib.Path(sys.argv[1]); spec=importlib.util.spec_from_file_location('recovery_backend',p); b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
class Observe(b.Runner):
    calls=[]
    def run(self,argv,**kw):
        assert '--verify-existing' in argv, 'no install/parser permitted'
        self.calls.append(argv)
        return super().run(argv,**kw)
a=json.loads(sys.argv[2]); root=pathlib.Path(a['root']); installer=pathlib.Path(a['installer'])
c=b.Controller(None,None,root/'fresh-process',('official-github',),sys.executable,None,None,runner_factory=Observe,
    official_bundle=root/'official',official_contents_sha256=a['sha'],official_installer=installer,official_installer_sha256=a['installer_sha'],
    verifier=installer,verifier_sha256=a['installer_sha'],recover_workspace=pathlib.Path(a['old']),recover_request=a['id'],recover_request_sha256=a['request_sha'])
assert c.state()['mode']=='recovery' and c.job['status']=='unconfirmed' and Observe.calls==[]
result=c.reconcile({'job_id':a['id']})
assert result['observation']['status']=='matched' and c.job['status']=='unconfirmed' and len(Observe.calls)==1
print(json.dumps({'status':result['observation']['status'],'historical_status':c.job['status'],'calls':len(Observe.calls)}))
"""
        data={'root':str(self.root),'installer':str(INSTALLER.resolve()),'installer_sha':owner.sha(INSTALLER.read_bytes()),'sha':self.sha,
              'old':str(c.workspace),'id':job['id'],'request_sha':job['recovery_request']['sha256']}
        result=subprocess.run([sys.executable,'-I','-B','-c',code,str(ROOT/'backend.py'),json.dumps(data)],capture_output=True,
            env={'PATH':'/usr/bin:/bin','PYTHONDONTWRITEBYTECODE':'1'},timeout=15)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout),{'status':'matched','historical_status':'unconfirmed','calls':1})
        altered=self.root/'different-installer.py';altered.write_bytes(INSTALLER.read_bytes()+b'\n# Different selected bytes\n')
        with self.assertRaises(backend.SetupError):
            self.controller(official_installer=altered,official_installer_sha256=owner.sha(altered.read_bytes()),
                verifier=INSTALLER.resolve(),verifier_sha256=owner.sha(INSTALLER.read_bytes()),recover_workspace=c.workspace,
                recover_request=job['id'],recover_request_sha256=job['recovery_request']['sha256'])

    def test_http_fixed_route_and_bad_reference(self):
        c=self.controller();self.checked(c)
        server=backend.Server(c);thread=threading.Thread(target=server.serve_forever);thread.start()
        try:
            connection=http.client.HTTPConnection(server.host,timeout=5)
            bad=self.request();bad['values']['token_env']='SECRET_MUST_NOT_REFLECT'
            connection.request('POST','/api/check',backend.encoded(bad),{'Origin':server.origin,'X-Toolkit-Session':server.capability,'Content-Type':'application/json'})
            response=connection.getresponse();raw=response.read();connection.close()
            self.assertEqual(response.status,400);self.assertNotIn(b'SECRET_MUST_NOT_REFLECT',raw)
            self.assertFalse(any('--destination' in a for a in c.runner.calls))
        finally:server.shutdown();server.server_close();thread.join()


class OfficialMCPTests(Fixture,unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_prepare_check_install_and_recovery(self):
        c=self.controller()
        async with connection(c) as (client, _):
            result=await client.call_tool('setup_prepare',{'family':'official-github'});self.assertFalse(result.is_error)
            await asyncio.to_thread(c.join)
            checked=await client.call_tool('setup_check',self.request());self.assertFalse(checked.is_error)
            value=checked.structured_content
            request=dict(self.request(),validation_id=value['validation_id'],configuration_sha256=value['configuration_sha256'])
            first=await client.call_tool('setup_install',request);self.assertFalse(first.is_error)
            await asyncio.to_thread(c.join)
            second=await client.call_tool('setup_install',request)
            self.assertEqual(first.structured_content['job']['id'],second.structured_content['job']['id'])
            self.assertEqual(c.job['status'],'succeeded')
            self.assertEqual(len([a for a in c.runner.calls if '--destination' in a]),1)
