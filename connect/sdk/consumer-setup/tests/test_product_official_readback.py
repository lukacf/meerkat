"""Real official install/readback owners with an inert binary, no account calls.

Only the outer product archive verification is controlled, as in the existing
product local-session fixture. The launch, retained request and local lease are real.
"""
import copy
import shutil
import sys
import threading
import unittest
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import patch

import test_official_github as official
import test_official_github_enterprise as enterprise
from test_local_session import ROOT, backend, fixtures, load, local

product = load("official_product_readback", ROOT / "product/prepare.py")
host = load("official_product_host", ROOT / "prepare_host.py")


class OfficialProductTests(fixtures.FixtureMixin, unittest.TestCase):
    def tearDown(self):
        for owner in self.controllers:
            owner.close_admission(); owner.join()
        self.temp.cleanup()

    def test_retained_and_credential_readback_reuse_exact_official_owner(self):
        self.exercise_official_readback()

    def test_enterprise_readback_preserves_selected_host_profile_and_operation_subset(self):
        self.exercise_official_readback(enterprise_profile=True)

    def test_enterprise_identity_readback_preserves_protected_account_selection(self):
        self.exercise_official_readback(enterprise_profile=True, selected_operations=["get_me"])

    def exercise_official_readback(self, enterprise_profile=False, selected_operations=None):
        runner = enterprise.OwnerRunner if enterprise_profile else official.RecordingRunner
        profile = official.owner.GHE_PROFILE if enterprise_profile else "selected-reads"
        operations = list(official.owner.GHE_TOOLS[:2]) if enterprise_profile else official.owner.OFFICIAL_TOOLS
        if selected_operations is not None:
            operations = selected_operations
        values = {"token_env": official.owner.OFFICIAL_ENV}
        if enterprise_profile:
            values["github_host"] = "https://github.enterprise.example"
        state = self.root / "state"
        prepared = state / ".meerkat-connect/realm/setup-official/prepared"
        (prepared / "config").mkdir(parents=True)
        for directory in (state / ".meerkat-connect", prepared.parent.parent, prepared.parent):
            directory.chmod(0o700)
        release = prepared / "distribution/fixture"
        (release / "setup").mkdir(parents=True)
        shutil.copytree(self.bundle, release / "consumers")
        retained = {}
        for name in ("backend.py", "local_session.py", "mcp_adapter.py"):
            retained["setup/" + name] = (ROOT / name).read_bytes()
            (release / "setup" / name).write_bytes(retained["setup/" + name])
        installer = release / "setup/install-library-consumer.py"
        installer.write_bytes(official.INSTALLER.read_bytes())
        official_pin = official.bundle(release / "official", official.owner.OFFICIAL_MAPPED_REVISION)
        if enterprise_profile:
            official_pin = enterprise.enterprise_bundle(release / "official")
        options = {"official_bundle":release / "official", "official_contents_sha256":official_pin,
            "official_installer":installer, "official_installer_sha256":backend.digest(installer.read_bytes())}
        python = str(Path(sys.executable).resolve())
        configuration = {"mcpServers":{"toolkit-setup":{"command":"/usr/bin/env", "args":[
            "-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1", python, "-I", "-B",
            str(release / "setup/mcp_adapter.py"), "--workspace", str(prepared / "session")]}}}
        config_raw = backend.encoded(configuration)
        (prepared / "config/mcp.json").write_bytes(config_raw)
        config_sha = backend.digest(config_raw)
        helper, binding, descriptor = product.local_projection(host, prepared, release, retained, config_sha)
        descriptor_raw = backend.encoded(descriptor)
        (prepared / "product-mcp.json").write_bytes(descriptor_raw)
        product_root = self.root / "product"
        product_root.mkdir()
        (product_root / "starter.py").write_bytes(b"")
        archive = product_root / "fixture.tar.gz"
        archive.write_bytes(b"controlled outer product archive")
        manifest = {"target":product.HOST_TARGETS[(product.platform.system(),product.platform.machine())],
            "python":python, "node":python, "npm_cli":python, "archive":archive.name, "starter":"starter.py"}
        receipt_raw = backend.encoded({"target":manifest["target"], "workspace":str(prepared / "session"),
            "release":{"archive_sha256":product.sha256(archive)}, "files":{"mcp.json":config_sha}})
        (prepared / "config/prepared.json").write_bytes(receipt_raw)
        (prepared / "product-prepared.json").write_bytes(backend.encoded({"resources_sha256":"a"*64,
            "mcp_sha256":config_sha, "preparation":{"bootstrap":{"host":{"receipt_sha256":backend.digest(receipt_raw)}}},
            "local_setup":{"descriptor_sha256":backend.digest(descriptor_raw), "helper_sha256":binding["helper_sha256"]}}))
        snapshot = {"hashes":{"consumers/CONTENTS.json":self.sha,
            "consumers/install-library-consumer.py":self.manifest["files"]["install-library-consumer.py"]["sha256"],
            "official/CONTENTS.json":official_pin, "setup/install-library-consumer.py":options["official_installer_sha256"]}}
        with local.session_owner(prepared / "local-session", binding) as selected:
            owner = local.controller_factory(backend, *selected)(release / "consumers",self.sha,self.root / "unused",
                ("official-github",),python,python,python,runner_factory=runner,
                verifier=installer,verifier_sha256=options["official_installer_sha256"],**options)
            self.controllers.append(owner)
            owner.prepare({"family":"official-github"});owner.join()
            request = {"family":"official-github","profile":profile,"operations":operations,"values":values}
            checked = owner.check(request)
            job = owner.install({**request,"validation_id":checked["validation_id"],
                "configuration_sha256":checked["configuration_sha256"]})["job"]
            owner.join();self.assertEqual(owner.job["status"],"succeeded");owner.close_admission()
            historical = {p:p.read_bytes() for p in (prepared / "local-session").rglob('*') if p.is_file()}
            readbacks = []
            original_loader = host.owner_module
            def create_readback(**kwargs):
                self.assertEqual(kwargs['official_bundle'], options['official_bundle'])
                self.assertEqual(kwargs['official_contents_sha256'], official_pin)
                self.assertEqual(kwargs['official_installer'], installer)
                self.assertEqual(kwargs['verifier'], installer)
                value = backend.Controller(**kwargs,runner_factory=runner)
                self.controllers.append(value);readbacks.append(value);return value
            def load_owner(name,path,raw):
                if name == 'product_readback_backend':
                    return SimpleNamespace(Controller=create_readback,read_recovery_selection=backend.read_recovery_selection,
                        retained_request_summary=backend.retained_request_summary)
                return original_loader(name,path,raw)
            def starter(name):
                module=ModuleType(name);module.host=host;return module
            request_path=self.root/'readback.json'
            query={'realm_state_root':str(state),'realm':'realm','retained_installations':{'action':'list','cursor':None}}
            def observe(value,label):
                request_path.write_bytes(backend.encoded(value))
                return product.installation_readback(product_root,self.root/label,request_path,threading.Event())
            with patch.object(product,'verify_resources',return_value=(manifest,'a'*64)), \
                 patch.object(product,'ModuleType',side_effect=starter), \
                 patch.object(host,'release_snapshot',return_value=(snapshot,retained)), \
                 patch.object(host,'recheck_release'),patch.object(host,'owner_module',side_effect=load_owner):
                page=observe(query,'unused-list')
                self.assertEqual(len(page['candidates']),1);self.assertFalse(page['unconfirmed']);self.assertEqual(readbacks,[])
                locator=page['candidates'][0]['locator']
                result=observe({**query,'retained_installations':{'action':'select','locator':locator}},'select')
                self.assertEqual(result['metadata']['selection'],{**request,'values':{'token_env':official.owner.OFFICIAL_ENV}})
                self.assertEqual(result['metadata']['help']['operations'],[{'id':x,'tool':x} for x in
                    (official.owner.GHE_TOOLS if enterprise_profile else official.owner.OFFICIAL_TOOLS)])
                installation=result['installation']
                server=next(iter(installation['mcp_configuration']['mcpServers'].values()))
                self.assertEqual(server['cwd'],str(owner.install_intents[job['id']]['destination']))
                self.assertEqual(server['args'],["stdio","--read-only","--gh-host",values["github_host"],
                    "--toolsets=","--tools",",".join(operations)] if enterprise_profile else official.owner.OFFICIAL_ARGS)
                self.assertTrue(readbacks[-1].recovery_only);self.assertTrue(readbacks[-1].closing)
                software={k:installation[k] for k in ('family','profile','job_id','validation_id','recovery_request_sha256','mcp_path','receipt_path')}
                credential={'operation_basis':{'kind':'package_published_metadata'},'operations':[operations[0]],
                    'credential_slots':[{'source':'library_field','declaration':{'field_path':['config','token_env'],
                        'environment_variable':'GITHUB_PERSONAL_ACCESS_TOKEN'}}]}
                direct={'realm_state_root':str(state),'realm':'realm','software':software,'credential_service':credential}
                current=observe(direct,'credential')
                self.assertEqual(current['mcp_configuration'],installation['mcp_configuration'])
                for number,(pointer,value) in enumerate((('profile','other'),('operation','create_issue'),('slot','OTHER_TOKEN'),('path',['token_env']))):
                    bad=copy.deepcopy(direct)
                    if pointer=='profile':bad['software']['profile']=value
                    elif pointer=='operation':bad['credential_service']['operations']=[value]
                    else:bad['credential_service']['credential_slots'][0]['declaration'][
                        'field_path' if pointer=='path' else 'environment_variable']=value
                    before=len(readbacks)
                    with self.assertRaises(product.ResourceError):observe(bad,'refused-'+str(number))
                    if pointer != 'operation':self.assertEqual(len(readbacks),before)
                if enterprise_profile:
                    for number,(pointer,value) in enumerate((('profile','selected-reads'),
                            ('operation','get_me' if 'get_me' not in operations else 'issue_read'),('operation','get_commit'))):
                        bad=copy.deepcopy(direct)
                        if pointer=='profile':bad['software']['profile']=value
                        else:bad['credential_service']['operations']=[value]
                        with self.assertRaises((product.ResourceError,backend.SetupError)):
                            observe(bad,'enterprise-refused-'+str(number))
                    installed_config=owner.install_intents[job['id']]['configuration']['path']
                    original_config=installed_config.read_bytes()
                    changed=backend.decode(original_config)
                    changed['config']['github_host']='https://other.enterprise.example'
                    installed_config.write_bytes(backend.encoded(changed))
                    try:
                        with self.assertRaises((product.ResourceError,backend.SetupError)):
                            observe(direct,'enterprise-changed-host')
                    finally:installed_config.write_bytes(original_config)
                bad={k:v for k,v in direct.items() if k!='credential_service'}
                bad['public_service']={'operation_basis':{'kind':'package_published_metadata'},'operations':['get_me']}
                with self.assertRaises(product.ResourceError):observe(bad,'not-public')
            self.assertEqual({p:p.read_bytes() for p in historical},historical)
            self.assertTrue(all(all(any(flag in argv for flag in ('--verify-existing','--official-describe','--check-config-only'))
                for argv in reader.runner.calls) for reader in readbacks))


if __name__ == '__main__':
    unittest.main()
