"""Generate routes from exact retained maintainer and component inputs, without executing them."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name('export-library-consumers.py')
spec = importlib.util.spec_from_file_location('route_export', SCRIPT)
exporter = importlib.util.module_from_spec(spec); spec.loader.exec_module(exporter)
ROOT = Path(os.environ['TOOLKIT_ROUTE_SOURCE_ROOT'])


class RouteExportTests(unittest.TestCase):
    def files(self, target):
        selected = exporter.service_selection(target)
        return {'catalog.json': (ROOT / selected['catalog']).read_bytes(),
            'agno/' + exporter.AGNO_WHEEL: (ROOT / selected['agno_artifact']).read_bytes(),
            'activepieces/' + exporter.AP_ARCHIVE: (ROOT / selected['ap_artifact']).read_bytes(),
            'services/' + selected['wheel']: (ROOT / selected['artifact']).read_bytes()}

    def test_exact_both_target_generation_and_official_manual_scope(self):
        for target in exporter.TARGETS:
            with self.subTest(target=target):
                files = self.files(target)
                value = json.loads(exporter.build_routes(ROOT, files, target))
                self.assertEqual(value['catalog_sha256'], exporter.sha(files['catalog.json']))
                self.assertEqual(len(value['routes']), 103)
                self.assertEqual(len({r['identity']['name'] for r in value['routes']}), 99)
                self.assertEqual({r['family'] for r in value['routes']}, {'agno', 'activepieces', 'services'})
                self.assertFalse({'github', 'playwright'} & {r['identity']['name'] for r in value['routes']})
                self.assertEqual(value['runtime_target'], None if target == exporter.TARGETS[0] else exporter.LINUX_RUNTIME_TARGET)
                for family, component in value['components'].items():
                    self.assertEqual(component['sha256'], exporter.sha(files[component['path']]))
                    self.assertEqual(component['path'].split('/')[0], family)
                self.assertEqual([len(value['components'][f]['profiles']) for f in ('agno', 'activepieces', 'services')], [36, 63, 4])
                calendar_routes = [row for row in value['routes'] if row['identity']['name'] == 'google-calendar']
                self.assertEqual({(row['family'], row['profile']) for row in calendar_routes},
                                 {('activepieces', 'google-calendar.get-event'), ('services', 'google-calendar')})
                drive_routes = [row for row in value['routes'] if row['identity']['name'] == 'google-drive']
                self.assertEqual({(row['family'], row['profile']) for row in drive_routes},
                                 {('activepieces', 'google-drive.get-file'), ('services', 'google-drive')})
                catalog = json.loads(files['catalog.json'])
                gmail = next(row for row in catalog if row['identity']['name'] == 'gmail')
                send = next(row for row in gmail['known_exports'] if row['id'] == 'gmail_send_message')
                self.assertIn('effect write', send['description'])

    def test_artifact_change_catalog_strategy_loss_and_identity_duplication_refused(self):
        target = exporter.TARGETS[0]
        for mutation in ('artifact', 'strategy', 'identity'):
            files = self.files(target)
            if mutation == 'artifact': files['agno/' + exporter.AGNO_WHEEL] += b'changed'
            else:
                catalog = json.loads(files['catalog.json'])
                if mutation == 'strategy':
                    next(row for row in catalog if row['identity']['name'] == 'slack')['strategies'] = []
                else: catalog.append(copy.deepcopy(catalog[-1]))
                files['catalog.json'] = exporter.encoded(catalog)
            with self.subTest(mutation=mutation), self.assertRaises(exporter.ExportError):
                exporter.build_routes(ROOT, files, target)

    def test_current_profile_and_inventory_bindings_are_required(self):
        target = exporter.TARGETS[0]; files = self.files(target)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            for source in exporter.ROUTE_SOURCES.values():
                path = root / source['path']; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes((ROOT / source['path']).read_bytes())
            path = root / exporter.ROUTE_SOURCES['agno']['path']; path.write_bytes(path.read_bytes() + b' ')
            with self.assertRaisesRegex(exporter.ExportError, 'source changed'): exporter.build_routes(root, files, target)
            pins = copy.deepcopy(exporter.ROUTE_SOURCES)
            value = json.loads(path.read_bytes()); value['profiles'] = [row for row in value['profiles'] if row['profile'] != 'searxng']
            path.write_bytes(exporter.encoded(value)); pins['agno']['sha256'] = exporter.sha(path.read_bytes())
            with patch.object(exporter, 'ROUTE_SOURCES', pins), self.assertRaisesRegex(exporter.ExportError, 'profile mismatch'):
                exporter.build_routes(root, files, target)

    def test_composition_preserves_adopted_routes_and_adds_unmapped_discovery_both_targets(self):
        discovery = (ROOT / exporter.DISCOVERY_CATALOG).read_bytes()
        for target in exporter.TARGETS:
            with self.subTest(target=target):
                files = self.files(target)
                old_routes = exporter.build_routes(ROOT, files, target)
                catalog, routes = exporter.compose_catalog(files['catalog.json'], discovery, old_routes)
                rows, index = json.loads(catalog), json.loads(routes)
                self.assertEqual(rows[:101], json.loads(files['catalog.json']))
                self.assertEqual(rows[101:], json.loads(discovery))
                self.assertEqual(len(rows), 111)
                previous = json.loads(old_routes)
                previous['catalog_sha256'] = exporter.sha(catalog)
                self.assertEqual(index, previous)
                self.assertEqual(len(index['routes']), 103)
                telegram = next(row for row in rows if row['identity']['name'] == 'telegram')
                self.assertEqual({item['role'] for item in telegram['known_exports']}, {'agent_frontend'})
                self.assertFalse(any(route['identity'] == telegram['identity'] for route in index['routes']))
                # Same-name editions remain separate immutable records.
                self.assertEqual(len([row for row in rows if row['identity']['name'] == 'slack']), 2)

    def test_composition_refuses_full_identity_duplicates_and_changed_route_meaning(self):
        files = self.files(exporter.TARGETS[0])
        routes = exporter.build_routes(ROOT, files, exporter.TARGETS[0])
        discovery = json.loads((ROOT / exporter.DISCOVERY_CATALOG).read_bytes())
        for extra in (discovery[0], json.loads(files['catalog.json'])[0]):
            with self.subTest(identity=extra['identity']), self.assertRaisesRegex(exporter.ExportError, 'duplicate'):
                exporter.compose_catalog(files['catalog.json'], exporter.encoded(discovery + [extra]), routes)
        for mutation in ('binding', 'strategy', 'discovery-route'):
            index = json.loads(routes)
            if mutation == 'binding': index['catalog_sha256'] = 'f' * 64
            elif mutation == 'strategy': index['routes'][0]['strategy_id'] = 'not-the-original-strategy'
            else:
                index['routes'][0]['identity'] = discovery[0]['identity']
                index['routes'][0]['strategy_id'] = discovery[0]['strategies'][0]['id']
            with self.subTest(mutation=mutation), self.assertRaises(exporter.ExportError):
                exporter.compose_catalog(files['catalog.json'], exporter.encoded(discovery), exporter.encoded(index))
