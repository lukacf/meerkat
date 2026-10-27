"""Prevent onboarding claims from changing profile authority or hiding limits."""
import hashlib
from importlib.resources import files
import json
from pathlib import Path
import unittest

from meerkat_toolkit.adapters.agno_profiles import configuration_help, list_profiles


class ProviderGuidanceTests(unittest.TestCase):
    def test_all_remote_profiles_have_dated_registration_and_operation_guidance(self):
        unchanged = {'hackernews', 'pubmed', 'searxng', 'arxiv', 'youtube', 'wikipedia', 'postgres'}
        names = {r['profile'] for r in list_profiles()['profiles']}
        self.assertEqual(len(names - unchanged), 29)
        for name in names - unchanged:
            with self.subTest(profile=name):
                help_ = configuration_help(name)
                text = ' '.join(r['instruction'] for r in help_['prerequisites'])
                # The existing Cal.com v2 guidance was reviewed one day later.
                self.assertIn('2026-10-04' if name == 'calcom' else '2026-10-03', text)
                self.assertIn('https://', text)
                self.assertTrue(any(r['kind'] == 'developer_registration' for r in help_['prerequisites']))
                for op in help_['operations']:
                    self.assertIn(op, text)
                self.assertIn('does not acquire, refresh', text)
                self.assertIn('not authentication, authorization or readiness', help_['scope'])

    def test_existing_rows_and_seven_other_profiles_are_unchanged(self):
        baseline = json.loads((Path(__file__).parent / 'fixtures/agno-guidance-baseline.json').read_text())
        for name, expected in baseline['unchanged'].items():
            actual = configuration_help(name)
            actual.pop('selected_guide')
            actual['guide'].pop('package_resource')
            self.assertEqual(actual, expected, name)
        for name, expected in baseline['retained_prerequisites'].items():
            actual = configuration_help(name)['prerequisites'][:len(expected)]
            if name != 'zendesk':
                self.assertEqual(actual, expected, name)
                continue
            # The OAuth addition changes only the credential and account rows.
            # Keep historical checks on the shared MCP and credential owners.
            self.assertEqual(len(expected), 4)
            self.assertEqual([row['kind'] for row in actual], [row['kind'] for row in expected])
            self.assertEqual(actual[0], expected[0])
            self.assertEqual(actual[2], expected[2])
            credential = actual[1]['instruction']
            for meaning in ('api_key_env names the selected Zendesk credential',
                            'auth_mode=oauth_access_token', 'OAuth access token sent as Bearer',
                            'omit account_email in that mode',
                            'Omitted auth_mode or auth_mode=api_token retains legacy Basic authentication',
                            'requires account_email', 'combined with /token',
                            'never guesses the credential type or falls back between modes'):
                self.assertIn(meaning, credential)
            account = actual[3]['instruction']
            for meaning in ('Select the Help Center subdomain explicitly',
                            'Legacy api_token mode also requires account_email',
                            'oauth_access_token mode rejects it because the token determines the acting identity',
                            'does not supply ticket access or a frontend'):
                self.assertIn(meaning, account)

    def test_permissions_follow_operations_and_indirect_calls(self):
        def text(name):
            return ' '.join(r['instruction'] for r in configuration_help(name)['prerequisites'])
        self.assertIn('data:read', text('todoist'))
        self.assertIn('two geocoding requests', text('openrouteservice'))
        self.assertIn('even when geocode_location is not separately exposed', text('openrouteservice'))
        self.assertIn('User permission', text('bright-data'))
        self.assertIn('only selected operations', text('bright-data'))
        self.assertIn('user-top-read', text('spotify'))
        self.assertIn('lambda:ListFunctions', text('aws-lambda'))

    def test_migrated_fixed_host_still_requires_host_permission_and_account_access(self):
        help_ = configuration_help('openrouteservice')
        text = ' '.join(r['instruction'] for r in help_['prerequisites'])
        self.assertIn('api.openrouteservice.org', text)
        self.assertIn('api.heigit.org', text)
        self.assertIn('GET /pelias/v1/search', text)
        self.assertIn('POST /openrouteservice/v2/directions/{profile}', text)
        self.assertIn('host must permit', text)
        self.assertIn('No legacy-host fallback', text)
        self.assertIn('changing a credential reference does not establish that permission', ' '.join(help_['unresolved_details']))

    def test_every_response_is_detached(self):
        for row in list_profiles()['profiles']:
            name = row['profile']
            expected = configuration_help(name)
            changed = configuration_help(name)
            for prerequisite in changed['prerequisites']:
                prerequisite['instruction'] = 'mutated'
            changed['unresolved_details'].append('mutated')
            self.assertEqual(configuration_help(name), expected, name)

    def test_added_guidance_is_bound_to_actual_package_resources(self):
        package = 'meerkat_toolkit.adapters'
        resources = files(package)
        manual = resources.joinpath('PROFILE-GUIDE.md').read_bytes()
        generated = json.loads(resources.joinpath('CONFIGURATION-GUIDES.json').read_bytes())
        manual_digest = hashlib.sha256(manual).hexdigest()
        self.assertEqual(generated['full_guide_sha256'], manual_digest)
        profiles = {row['profile'] for row in list_profiles()['profiles']}
        self.assertEqual(set(generated['profiles']), profiles)
        for name in profiles:
            with self.subTest(profile=name):
                actual = configuration_help(name)
                self.assertEqual(actual['guide']['package_resource'],
                                 {'package': package, 'name': 'PROFILE-GUIDE.md'})
                self.assertEqual(actual['selected_guide'], {
                    'profile': name,
                    'full_guide_sha256': manual_digest,
                    'text': '\n\n'.join(generated['sections'][key]
                                       for key in generated['profiles'][name]),
                })


if __name__ == '__main__':
    unittest.main()
