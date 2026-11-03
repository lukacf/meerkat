"""Fixed Playwright route: synthetic npm realization; no browser execution."""
import argparse
import asyncio
import copy
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from test_official_github import owner, backend, INSTALLER, adapter, connection


def archive(package,version,dependencies):
    out=io.BytesIO()
    data={'package.json':owner.encoded({'name':package,'version':version,'dependencies':dependencies}), 'cli.js':b'throw Error("MUST_NOT_EXECUTE");', 'LICENSE':b'Synthetic license'}
    with tarfile.open(fileobj=out,mode='w:gz') as t:
        for name,raw in data.items():
            item=tarfile.TarInfo('package/'+name);item.size=len(raw);t.addfile(item,io.BytesIO(raw))
    return out.getvalue()


def bundle(root):
    root.mkdir(); versions={'@playwright/mcp':'0.0.83','playwright':'1.64.0-alpha-1790635538000','playwright-core':'1.64.0-alpha-1790635538000'}
    project={'name':'fixture','version':'0.1.0','private':True,'dependencies':{'@playwright/mcp':'0.0.83'}}
    lock={'name':'fixture','version':'0.1.0','lockfileVersion':3,'packages':{'':project}}
    files={};rows=[]
    for i,(name,version) in enumerate(versions.items()):
        deps= {k:v for k,v in versions.items() if (name=='@playwright/mcp' and k!=name) or (name=='playwright' and k=='playwright-core')}
        raw=archive(name,version,deps); parsed=owner.npm_archive(raw);path='playwright/archives/'+str(i)+'.tgz';loc='node_modules/'+name;files[path]=raw
        url='https://registry.example/'+str(i)+'.tgz'
        rows.append({'id':name,'version':version,'path':path,'url':url,'integrity':parsed['integrity'],'sha256':owner.sha(raw),'bytes':len(raw),'install_path':loc,'payload_files':len(parsed['files'])})
        lock['packages'][loc]={'version':version,'resolved':url,'integrity':parsed['integrity'],'dependencies':deps}
    files['playwright/package.json']=owner.encoded(project);files['playwright/package-lock.json']=owner.encoded(lock)
    files['playwright/closure.json']=owner.encoded({'packages':rows,'package_json_sha256':owner.sha(files['playwright/package.json']),'package_lock_sha256':owner.sha(files['playwright/package-lock.json'])})
    files['playwright/mcp.template.json']=owner.encoded({'mcpServers':{'playwright-isolated':{'command':'ABS_NODE_EXECUTABLE','args':owner.PLAYWRIGHT_ARGS,'cwd':'ABS_PROCESS_WORKSPACE','env':{'PATH':'/usr/bin:/bin','HOME':'ABS_PROCESS_HOME','TMPDIR':'ABS_PROCESS_TMP','PWTEST_SOCKETS_DIR':'ABS_SHORT_PROCESS_SOCKET_DIRECTORY'}}}})
    files['playwright/UPSTREAM-PROFILE.md']=b'Synthetic retained guide.'
    files.update({'playwright/notices/'+str(i):b'Synthetic notice' for i in range(7)})
    files['delivery.json']=owner.encoded({'format':'meerkat-toolkit.official-consumers.v1','role':'service_connector','components':[{'catalog_id':'playwright','source_revision':'a'*40,'selected_operations':owner.PLAYWRIGHT_OPERATIONS,'whole_endpoint_exposed':True,'browser_distributed':False,'package_json':'playwright/package.json','package_lock':'playwright/package-lock.json','closure':'playwright/closure.json','configuration_template':'playwright/mcp.template.json'}]})
    for p,v in files.items():
        target=root/p;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(v)
    (root/'CONTENTS.json').write_bytes(owner.encoded({'format':1,'files':{p:{'sha256':owner.sha(v),'bytes':len(v)} for p,v in files.items()}}))
    return owner.sha((root/'CONTENTS.json').read_bytes())


class FixedRunner(backend.Runner):
    """Run the real Python owner in-process; only npm realization is synthetic."""
    def __init__(self,workspace):super().__init__(workspace);self.calls=[];self.npm_calls=[];self.drop=False
    def run(self,argv,*,capture=False):
        self.calls.append(argv)
        def val(flag):return argv[argv.index(flag)+1] if flag in argv else None
        args=argparse.Namespace(official_bundle=val('--official-bundle'),contents_sha256=val('--contents-sha256'),component=val('--component'),
            playwright_host_profile=val('--playwright-host-profile'),playwright_host_profile_sha256=val('--playwright-host-profile-sha256'),
            node=val('--node'),npm_cli=val('--npm-cli'),official_describe='--official-describe' in argv,check_config_only='--check-config-only' in argv,
            verify_existing='--verify-existing' in argv,config=val('--config'),config_sha256=val('--config-sha256'),server_name=val('--server-name'),destination=val('--destination'))
        try:
            with patch.object(owner,'run',side_effect=lambda *a,**k:realize(self.npm_calls,*a,**k)):
                result=owner.official_consumer(args)
        except (ValueError,OSError,KeyError,TypeError):return 2,b''
        if self.drop and args.destination and not args.verify_existing:return 2,b''
        return 0,owner.encoded(result) if capture else b''


def realize(calls,argv,cwd,env,label,**kwargs):
    calls.append((argv,dict(env),label))
    if label=='node-runtime':return owner.encoded({'version':'v24.19.0','platform':'darwin','arch':'arm64'})
    if label=='npm-runtime':return b'11.12.1\n'
    if label=='npm-install':
        for a in (cwd/'inputs/playwright/archives').iterdir():
            content=owner.npm_archive(a.read_bytes());root=cwd/'node_modules'/content['name']
            for path,raw in content['files'].items():
                p=root/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(raw)
    return b''


class Fixture:
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='pw-',dir='/private/tmp');self.root=Path(self.temp.name);self.controllers=[]
        self.bundle=self.root/'bundle';self.digest=bundle(self.bundle)
        self.chrome=self.root/'chrome';self.chrome.write_bytes(b'NOT_EXECUTABLE_CHROME');self.chrome.chmod(0o700)
        self.parent=self.root/'p';self.parent.mkdir()
        self.profile=self.root/'host.json';self.profile.write_bytes(owner.encoded({'schema_version':1,'chrome_executable':str(self.chrome),'chrome_sha256':owner.sha(self.chrome.read_bytes()),'process_parent':str(self.parent)}))
        self.node=self.root/'node';self.node.write_bytes(b'NOT_EXECUTABLE_NODE');self.node.chmod(0o700)
        self.npm=self.root/'npm.js';self.npm.write_bytes(b'NOT_EXECUTABLE_NPM')
        self.config=self.root/'config.json';self.config.write_bytes(backend.configuration_bytes(self.request()))
        self.calls=[]
    def tearDown(self):
        for c in self.controllers:c.join()
        self.temp.cleanup()
    def request(self):return {'family':owner.PLAYWRIGHT_FAMILY,'profile':owner.PLAYWRIGHT_PROFILE,'operations':owner.PLAYWRIGHT_OPERATIONS,'values':{}}
    def args(self,**changes):
        d=dict(official_bundle=str(self.bundle),contents_sha256=self.digest,component='playwright',playwright_host_profile=str(self.profile),playwright_host_profile_sha256=owner.sha(self.profile.read_bytes()),node=str(self.node),npm_cli=str(self.npm),official_describe=False,check_config_only=False,verify_existing=False,config=str(self.config),config_sha256=owner.sha(self.config.read_bytes()),server_name='playwright',destination=str(self.root/'install'))
        d.update(changes);return argparse.Namespace(**d)
    def install(self,**changes):
        with patch.object(owner,'run',side_effect=lambda *a,**k:realize(self.calls,*a,**k)):
            return owner.official_consumer(self.args(**changes))
    def controller(self,**changes):
        d=dict(bundle=None,contents_sha256=None,workspace=self.root/('w'+str(len(self.controllers))),families=(owner.PLAYWRIGHT_FAMILY,),python=os.sys.executable,node=str(self.node),npm_cli=str(self.npm),runner_factory=FixedRunner,
            official_bundle=self.bundle,official_contents_sha256=self.digest,official_installer=INSTALLER.resolve(),official_installer_sha256=owner.sha(INSTALLER.read_bytes()),playwright_host_profile=self.profile,playwright_host_profile_sha256=owner.sha(self.profile.read_bytes()),verifier=INSTALLER.resolve(),verifier_sha256=owner.sha(INSTALLER.read_bytes()))
        d.update(changes);c=backend.Controller(**d);self.controllers.append(c);return c
    def checked(self,c):
        c.prepare({'family':owner.PLAYWRIGHT_FAMILY});c.join();self.assertEqual(c.job['status'],'succeeded')
        result=c.check(self.request());self.assertEqual(result['credential_environment_references'],[])
        return {**self.request(),'validation_id':result['validation_id'],'configuration_sha256':result['configuration_sha256']}


class PlaywrightTests(Fixture,unittest.TestCase):
    def test_fixed_operation_declaration_and_controller_subset_refusal(self):
        source=owner.official_consumer(self.args(official_describe=True))['help']
        self.assertIs(source['selectable_operations'],False)
        c=self.controller();c.prepare({'family':owner.PLAYWRIGHT_FAMILY});c.join()
        self.assertIs(c.help({'family':owner.PLAYWRIGHT_FAMILY,'profile':owner.PLAYWRIGHT_PROFILE})['selectable_operations'],False)
        with self.assertRaises(backend.SetupError):c.check({**self.request(),'operations':owner.PLAYWRIGHT_OPERATIONS[:1]})
        self.assertEqual(c.runner.npm_calls,[]);self.assertEqual(list(self.parent.iterdir()),[])

    def test_metadata_and_check_have_no_child_or_allocation(self):
        with patch.object(owner.subprocess,'Popen',side_effect=AssertionError('child')):
            result=owner.official_consumer(self.args(official_describe=True)); check=owner.official_consumer(self.args(check_config_only=True))
        self.assertTrue(result['whole_endpoint_exposed']);self.assertEqual(result['profiles'][0]['effect'],'unspecified')
        self.assertEqual(check['credential_environment_references'],[]);self.assertEqual(list(self.parent.iterdir()),[])
    def test_install_npm_only_and_readback_no_execution(self):
        result=self.install(); self.assertEqual(len(self.calls),6)
        for argv,env,label in self.calls:
            self.assertEqual(argv[0],str(self.node));self.assertNotIn('cli.js',argv)
            self.assertEqual(env['PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD'],'1')
            if label=='npm-install':self.assertTrue({'--offline','--ignore-scripts','--bin-links=false'}<=set(argv))
        with patch.object(owner.subprocess,'Popen',side_effect=AssertionError('child')):
            check=owner.official_consumer(self.args(verify_existing=True))
        self.assertTrue(check['verified']);self.assertEqual(result['process_destination_sha256'],owner.sha(str(self.root/'install').encode()))
        self.assertFalse(Path(result['process_root']).is_relative_to(self.root/'install'))
    def test_independent_install_roots_and_collision_refusal(self):
        a=self.install(); b=self.install(destination=str(self.root/'other'));self.assertNotEqual(a['process_root'],b['process_root'])
        dest=self.root/'third';root=owner.playwright_process(json.loads(self.profile.read_text()),dest);root.mkdir()
        with self.assertRaises(OSError):self.install(destination=str(dest))
        self.assertFalse(dest.exists())
    def test_missing_notice_and_closure_tamper_refuse_before_allocation(self):
        p=self.bundle/'playwright/notices/0';p.unlink()
        with self.assertRaises(OSError):self.install()
        self.assertFalse((self.root/'install').exists());self.assertEqual(list(self.parent.iterdir()),[])
    def test_extra_config_and_operation_subset_refused(self):
        for value in ({'profile':owner.PLAYWRIGHT_PROFILE,'operations':owner.PLAYWRIGHT_OPERATIONS[:1],'config':{}}, {'profile':owner.PLAYWRIGHT_PROFILE,'operations':owner.PLAYWRIGHT_OPERATIONS,'config':{'chrome_executable':'SECRET'}}):
            self.config.write_bytes(owner.encoded(value))
            with self.assertRaises(ValueError):owner.official_consumer(self.args(check_config_only=True))
        self.assertEqual(list(self.parent.iterdir()),[])
    def test_host_hash_schema_and_browser_changes_refused(self):
        with self.assertRaises(ValueError):owner.official_consumer(self.args(check_config_only=True,playwright_host_profile_sha256='f'*64))
        saved=self.profile.read_bytes(); value=json.loads(saved);value['schema_version']=True;self.profile.write_bytes(owner.encoded(value))
        with self.assertRaises(ValueError):owner.official_consumer(self.args(check_config_only=True))
        self.profile.write_bytes(saved);self.chrome.write_bytes(b'CHANGED')
        with self.assertRaises(ValueError):owner.official_consumer(self.args(check_config_only=True))
        self.assertEqual(list(self.parent.iterdir()),[])
    def test_process_parent_alias_overlong_and_inside_install_refused(self):
        original=json.loads(self.profile.read_text())
        alias=self.root/'alias';alias.symlink_to(self.parent,target_is_directory=True)
        long=self.root/('x'*80);long.mkdir()
        for parent in (alias,long,self.root/'install'):
            if parent.name=='install':parent.mkdir()
            value={**original,'process_parent':str(parent)};self.profile.write_bytes(owner.encoded(value))
            with self.assertRaises((ValueError,OSError)):self.install()
        self.assertEqual(self.calls,[])
    def test_self_consistent_launch_tamper_extra_file_pending_and_payload(self):
        self.install(); dest=self.root/'install'
        for relative in ('foreign.js','pending-ready'):
            p=dest/relative;p.write_text('extra')
            with self.assertRaises(ValueError):owner.official_consumer(self.args(verify_existing=True))
            p.unlink()
        receipt=dest/'ready/installed.json';mcp=dest/'ready/mcp.json';r=json.loads(receipt.read_text());v=json.loads(mcp.read_text())
        v['mcpServers']['playwright']['args']+=['--no-sandbox'];r['mcp_configuration']=v;r['command']+=['--no-sandbox']
        receipt.write_bytes(owner.encoded(r));mcp.write_bytes(owner.encoded(v))
        with self.assertRaises(ValueError):owner.official_consumer(self.args(verify_existing=True))
    def test_readback_excludes_mutable_serving_files_and_never_recreates(self):
        result=self.install();root=Path(result['process_root']);(root/'output/snapshot.yml').write_text('MUTABLE_BROWSER_DATA')
        with patch.object(owner.subprocess,'Popen',side_effect=AssertionError('child')):owner.official_consumer(self.args(verify_existing=True))
        self.assertEqual((root/'output/snapshot.yml').read_text(),'MUTABLE_BROWSER_DATA')
        import shutil;shutil.rmtree(root)
        owner.official_consumer(self.args(verify_existing=True));self.assertFalse(root.exists())
    def test_controller_replay_recovery_v3_and_old_format_refusal(self):
        c=self.controller();request=self.checked(c);c.runner.drop=True;job=c.install(request)['job'];c.join();self.assertEqual(c.job['status'],'unconfirmed')
        self.assertEqual(c.install(request)['job']['id'],job['id']);self.assertEqual(c.reconcile({'job_id':job['id']})['observation']['status'],'matched')
        c.close_admission();r=self.controller(recover_workspace=c.workspace,recover_request=job['id'],recover_request_sha256=job['recovery_request']['sha256'])
        self.assertEqual(r.runner.calls,[]);self.assertEqual(r.job['status'],'unconfirmed');r.reconcile({'job_id':job['id']});self.assertEqual(r.runner.npm_calls,[])
        p=c.workspace/'requests'/(job['id']+'.json');v=json.loads(p.read_text());self.assertEqual(v['schema_version'],3)
        v['schema_version']=2;v.pop('playwright_host_profile_sha256');p.write_bytes(owner.encoded(v))
        with self.assertRaises(backend.SetupError):self.controller(recover_workspace=c.workspace,recover_request=job['id'],recover_request_sha256=owner.sha(p.read_bytes()))

    def repin(self, changes):
        for name,value in changes.items(): (self.bundle/name).write_bytes(owner.encoded(value))
        c=json.loads((self.bundle/'CONTENTS.json').read_bytes())
        for name in c['files']:
            raw=(self.bundle/name).read_bytes();c['files'][name]={'sha256':owner.sha(raw),'bytes':len(raw)}
        (self.bundle/'CONTENTS.json').write_bytes(owner.encoded(c));self.digest=owner.sha((self.bundle/'CONTENTS.json').read_bytes())
    def test_strict_structures_and_numeric_fields_refuse_before_effect(self):
        originals={p.relative_to(self.bundle).as_posix():p.read_bytes() for p in self.bundle.rglob('*') if p.is_file()}
        variants=[]
        for name in ('delivery.json','playwright/package.json','playwright/package-lock.json','playwright/closure.json'):
            for malformed in (None,[],True):variants.append({name:malformed})
        delivery=json.loads(originals['delivery.json']);delivery['components'][0]['source_revision']=True;variants.append({'delivery.json':delivery})
        for key in ('bytes','payload_files'):
            closure=json.loads(originals['playwright/closure.json']);closure['packages'][0][key]=True;variants.append({'playwright/closure.json':closure})
        lock=json.loads(originals['playwright/package-lock.json']);lock['packages']['']=[];variants.append({'playwright/package-lock.json':lock})
        for values in variants:
            with self.subTest(values=values):
                for name,raw in originals.items():(self.bundle/name).write_bytes(raw)
                self.repin(values)
                with patch.object(owner.subprocess,'Popen',side_effect=AssertionError('child')):
                    with self.assertRaises((ValueError,KeyError,TypeError)):owner.official_consumer(self.args(official_describe=True))
                self.assertEqual(list(self.parent.iterdir()),[])
    def test_unknown_root_dependencies_and_bad_lock_refuse(self):
        project=json.loads((self.bundle/'playwright/package.json').read_bytes());project['devDependencies']={'unselected':'1.0.0'}
        closure=json.loads((self.bundle/'playwright/closure.json').read_bytes());closure['package_json_sha256']=owner.sha(owner.encoded(project))
        self.repin({'playwright/package.json':project,'playwright/closure.json':closure})
        with self.assertRaises(ValueError):self.install()
        self.assertEqual(self.calls,[]);self.assertEqual(list(self.parent.iterdir()),[])
    def test_receipt_nonobjects_and_bool_schema_refuse_cleanly(self):
        self.install();p=self.root/'install/ready/installed.json';original=json.loads(p.read_bytes())
        for value in (None,[],True,{**original,'schema_version':True}):
            p.write_bytes(owner.encoded(value))
            with self.assertRaises(ValueError):owner.official_consumer(self.args(verify_existing=True))
    def test_wrong_node_runtime_preserves_unpublished_attempt(self):
        with patch.object(owner,'run',return_value=owner.encoded({'version':'v22.0.0','platform':'darwin','arch':'arm64'})):
            with self.assertRaises(ValueError):owner.official_consumer(self.args())
        self.assertFalse((self.root/'install/ready').exists());self.assertEqual(len(list(self.parent.iterdir())),1)
    def test_recovery_changed_profile_or_runtime_refuses_before_worker(self):
        c=self.controller();request=self.checked(c);job=c.install(request)['job'];c.join();c.close_admission()
        kw={'recover_workspace':c.workspace,'recover_request':job['id'],'recover_request_sha256':job['recovery_request']['sha256']}
        changed=self.root/'other-node';changed.write_bytes(b'OTHER');changed.chmod(0o700)
        with self.assertRaises(backend.SetupError):self.controller(**kw,node=str(changed))
        value=json.loads(self.profile.read_bytes());value['chrome_sha256']='f'*64;self.profile.write_bytes(owner.encoded(value))
        with self.assertRaises(backend.SetupError):self.controller(**kw)

    def test_public_metadata_failure_is_redacted_and_effect_free(self):
        import contextlib, signal, sys
        self.repin({'delivery.json': ['PRIVATE_DIAGNOSTIC_CANARY']})
        argv=['installer','--official-bundle',str(self.bundle),'--contents-sha256',self.digest,'--component','playwright',
              '--node',str(self.node),'--npm-cli',str(self.npm),'--playwright-host-profile',str(self.profile),
              '--playwright-host-profile-sha256',owner.sha(self.profile.read_bytes()),'--official-describe']
        output,error=io.StringIO(),io.StringIO();old=signal.getsignal(signal.SIGTERM)
        try:
            with patch.object(sys,'argv',argv), patch.object(owner.subprocess,'Popen',side_effect=AssertionError('child')), contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
                self.assertEqual(owner.main(),2)
        finally:signal.signal(signal.SIGTERM,old)
        self.assertEqual(output.getvalue(),'');self.assertNotIn('PRIVATE_DIAGNOSTIC_CANARY',error.getvalue())
        self.assertIn('no installation was attempted',error.getvalue());self.assertEqual(list(self.parent.iterdir()),[])

    def test_inert_browser_check_uses_mode_not_current_execution_permission(self):
        self.install()
        with patch.object(owner.os,'access',side_effect=AssertionError('execution permission must not be queried')):
            for action in ('official_describe','check_config_only','verify_existing'):
                result=owner.official_consumer(self.args(**{action:True}))
                self.assertIsInstance(result,dict)
        self.chrome.chmod(0o600)
        with self.assertRaises(ValueError):owner.official_consumer(self.args(official_describe=True))
        self.chrome.chmod(0o700)
        self.chrome.write_bytes(b'CHANGED')
        with self.assertRaises(ValueError):owner.official_consumer(self.args(official_describe=True))


class PlaywrightMCPTests(Fixture,unittest.IsolatedAsyncioTestCase):
    async def test_existing_mcp_dispatch_installs_without_browser(self):
        c=self.controller()
        async with connection(c) as (client,_):
            await client.call_tool('setup_prepare',{'family':owner.PLAYWRIGHT_FAMILY});await asyncio.to_thread(c.join)
            check=await client.call_tool('setup_check',self.request());self.assertFalse(check.is_error)
            self.assertEqual(check.structured_content['credential_environment_references'],[])
            req={**self.request(),'validation_id':check.structured_content['validation_id'],'configuration_sha256':check.structured_content['configuration_sha256']}
            first=await client.call_tool('setup_install',req);await asyncio.to_thread(c.join);second=await client.call_tool('setup_install',req)
            self.assertEqual(first.structured_content['job']['id'],second.structured_content['job']['id']);self.assertEqual(c.job['status'],'succeeded')
