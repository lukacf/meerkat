"""SQLite admission authority for one physical adapter, shared by all sessions.
All users of that adapter must use this same durable local database. No transaction
or application lock spans transport I/O. Uncertain ownership never expires.
"""
import hashlib
import json
import sqlite3
import time


class BoundaryError(ValueError):
    pass


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False)


class Ledger:
    def __init__(self, path, clock=time.time):
        self.clock = clock
        self.db = sqlite3.connect(str(path), isolation_level=None)
        self.db.row_factory = sqlite3.Row
        self.db.executescript('''
        CREATE TABLE IF NOT EXISTS sessions(
          id TEXT PRIMARY KEY, scope TEXT NOT NULL, digest TEXT NOT NULL,
          decision TEXT NOT NULL, state TEXT NOT NULL, actor TEXT, channel TEXT);
        CREATE TABLE IF NOT EXISTS actions(
          key TEXT PRIMARY KEY, session TEXT NOT NULL, request TEXT NOT NULL,
          status TEXT NOT NULL, receipt TEXT);
        CREATE TABLE IF NOT EXISTS device_ownership(
          slot INTEGER PRIMARY KEY CHECK(slot=1), session TEXT, blocked INTEGER NOT NULL);
        ''')
        # Migrate existing ledgers conservatively, atomically across constructors.
        self.db.execute('BEGIN IMMEDIATE')
        try:
            if not self.db.execute('SELECT 1 FROM device_ownership').fetchone():
                active = list(self.db.execute("SELECT id FROM sessions WHERE state IN ('running','paused','uncertain')"))
                self.db.execute('INSERT INTO device_ownership VALUES(1,?,?)',
                                (active[0]['id'] if len(active) == 1 else None, int(len(active) > 1)))
            self.db.execute('COMMIT')
        except BaseException:
            self.db.execute('ROLLBACK')
            raise

    def close(self):
        self.db.close()

    def propose(self, scope):
        required = {'session', 'version', 'goal', 'start', 'end', 'constraints'}
        if set(scope) != required or not scope['goal'] or not scope['session']:
            raise BoundaryError('Complete exact scope required')
        if (type(scope['version']) is not int or scope['version'] < 1
                or type(scope['start']) not in (int, float)
                or type(scope['end']) not in (int, float)
                or not scope['start'] < scope['end'] or scope['end'] <= self.clock()
                or not isinstance(scope['constraints'], dict)):
            raise BoundaryError('Invalid version/window/constraints')
        text = canonical(scope)
        digest = hashlib.sha256(text.encode()).hexdigest()
        self.db.execute('BEGIN IMMEDIATE')
        try:
            old = self.db.execute('SELECT * FROM sessions WHERE id=?', (scope['session'],)).fetchone()
            if old:
                if old['digest'] == digest:
                    self.db.execute('COMMIT')
                    return self.status(scope['session'])
                if old['state'] != 'awaiting_approval':
                    raise BoundaryError('Cannot replace an active or uncertain session')
                if scope['version'] <= json.loads(old['scope'])['version']:
                    raise BoundaryError('Stale scope version')
            self.db.execute('INSERT OR REPLACE INTO sessions VALUES(?,?,?,?,?,?,?)',
                            (scope['session'], text, digest, 'pending', 'awaiting_approval', None, None))
            self.db.execute('COMMIT')
        except BaseException:
            self.db.execute('ROLLBACK')
            raise
        return self.status(scope['session'])

    def status(self, session):
        row = self.db.execute('SELECT * FROM sessions WHERE id=?', (session,)).fetchone()
        if not row:
            raise BoundaryError('Unknown session')
        result = dict(row)
        result['scope'] = json.loads(result['scope'])
        result['actions'] = [dict(r) for r in self.db.execute('SELECT * FROM actions WHERE session=?', (session,))]
        result['device_ownership'] = dict(self.db.execute('SELECT * FROM device_ownership WHERE slot=1').fetchone())
        return result

    def decide(self, session, digest, decision, actor, channel):
        # Actor/channel ONLY from trusted frontend admission, never tool arguments.
        if not actor or channel not in ('telegram', 'console') or decision not in ('approved', 'declined', 'revoked'):
            raise BoundaryError('Invalid authenticated decision')
        self.db.execute('BEGIN IMMEDIATE')
        try:
            current = self.status(session)
            if current['digest'] != digest or current['scope']['end'] <= self.clock():
                raise BoundaryError('Stale or expired approval')
            if current['decision'] == 'revoked' and decision != 'revoked':
                raise BoundaryError('Revoked scope requires a new proposal')
            self.db.execute('UPDATE sessions SET decision=?, actor=?, channel=? WHERE id=?',
                            (decision, actor, channel, session))
            self.db.execute('COMMIT')
        except BaseException:
            self.db.execute('ROLLBACK')
            raise
        return self.status(session)

    async def act(self, request, device):
        if device is None:
            raise BoundaryError('Verified Seestar adapter not configured; executed=false')
        if set(request) != {'session', 'digest', 'key', 'kind', 'scope', 'target', 'until'}:
            raise BoundaryError('Exact action packet required')
        if request['kind'] not in ('start', 'switch', 'continue', 'pause', 'finish') or not request['key']:
            raise BoundaryError('Invalid action')
        encoded = canonical(request)
        request = json.loads(encoded)  # caller cannot mutate a reserved packet during I/O
        self.db.execute('BEGIN IMMEDIATE')
        try:
            s = self.status(request['session'])
            prior = self.db.execute('SELECT * FROM actions WHERE key=?', (request['key'],)).fetchone()
            if prior:
                if prior['request'] != encoded:
                    raise BoundaryError('Idempotency key collision')
                self.db.execute('COMMIT')
                return dict(prior)  # never replay, even an unknown outcome
            now = self.clock()
            if (s['decision'] != 'approved' or s['digest'] != request['digest']
                    or s['scope'] != request['scope'] or not s['scope']['start'] <= now < s['scope']['end']
                    or type(request['until']) not in (int, float)
                    or not now <= request['until'] <= s['scope']['end']):
                raise BoundaryError('Missing approval or action exceeds scope/window')
            owner = s['device_ownership']
            if owner['blocked'] or owner['session'] not in (None, request['session']):
                raise BoundaryError('Device exclusively owned; reconciliation required if uncertain')
            if s['state'] == 'uncertain':
                raise BoundaryError('Reconciliation required; no blind retry')
            if request['kind'] == 'start' and s['state'] != 'awaiting_approval':
                raise BoundaryError('Duplicate start')
            if request['kind'] != 'start' and (s['state'] not in ('running', 'paused') or owner['session'] != request['session']):
                raise BoundaryError('No owned active session')
            self.db.execute('UPDATE device_ownership SET session=? WHERE slot=1', (request['session'],))
            self.db.execute('INSERT INTO actions VALUES(?,?,?,?,?)',
                            (request['key'], request['session'], encoded, 'uncertain', None))
            self.db.execute('UPDATE sessions SET state=? WHERE id=?', ('uncertain', request['session']))
            self.db.execute('COMMIT')
        except BaseException:
            self.db.execute('ROLLBACK')
            raise
        # Admission linearizes above; no await between authorization and invocation.
        # Revocation during I/O cannot unsend this call, but blocks every later call.
        receipt = await device.execute(request)
        if not isinstance(receipt, dict) or receipt.get('observed_state') not in ('running', 'paused', 'completed'):
            raise BoundaryError('No verified observed device state')
        text = canonical(receipt)
        self.db.execute('BEGIN IMMEDIATE')
        try:
            self.db.execute('UPDATE actions SET status=?,receipt=? WHERE key=?', ('observed', text, request['key']))
            self.db.execute('UPDATE sessions SET state=? WHERE id=?', (receipt['observed_state'], request['session']))
            # Only a verified completion releases ownership, never revocation,
            # approval expiry, timeout, cancellation or pause.
            if receipt['observed_state'] == 'completed':
                self.db.execute('UPDATE device_ownership SET session=NULL WHERE slot=1 AND session=?', (request['session'],))
            self.db.execute('COMMIT')
        except BaseException:
            self.db.execute('ROLLBACK')
            raise
        return {'executed': True, 'receipt': receipt, 'useful_integration': None}
