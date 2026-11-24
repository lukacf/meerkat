import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace as Row

from main import verify


class WorkNoteTests(unittest.TestCase):
    def test_result_matches_real_file_and_claim(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / 'handoff.txt'
            output.write_text('nonce-123\n')
            item = Row(id='w1', status='completed', evidence_refs=[{'kind': 'file', 'id': str(output)}])
            events = [Row(seq=i, item_id='w1', kind=k, payload={'item': {}}) for i, k in
                      enumerate(['created', 'claimed', 'evidence_added', 'closed'])]
            owner = {'kind': 'agent', 'id': 'writer'}
            events[1].payload['item']['claim'] = {'owner': {'key': owner}}
            events[-1].payload['item']['status'] = 'completed'
            self.assertEqual(verify(item, events, output, 'nonce-123')[-1], 'closed')
            with self.assertRaisesRegex(ValueError, 'marker'):
                verify(item, events, output, 'different-nonce')
            owner['id'] = 'other'
            with self.assertRaisesRegex(ValueError, 'owned'):
                verify(item, events, output, 'nonce-123')
            with self.assertRaisesRegex(ValueError, 'lifecycle'):
                verify(item, events[:2] + events[3:], output, 'nonce-123')
