"""Durable viewer demand, independent of native animation identities."""
from __future__ import annotations

from copy import deepcopy
from datetime import datetime, timezone
import math
from pathlib import Path
import time
import uuid

from .projects import fingerprint
from .templates.storage_setup import write_private
from .visualization_timeline import checked_hash, read_record


MAX_INTENT_REVISION = 2**53 - 1
INTENT_FIELDS = {'consumer_id', 'intent_revision', 'setup_id', 'stream_id', 'intent_token'}


class VisualizationIntentConflict(ValueError):
    """This viewer intent no longer owns admission of the submitted scene."""


def checked_uuid(value):
    try:
        if not isinstance(value, str) or str(uuid.UUID(value)) != value:
            raise ValueError
    except (ValueError, AttributeError):
        raise VisualizationIntentConflict('Invalid animation consumer identity.') from None
    return value


def checked_scope(consumer_id, intent_revision, setup_id, stream_id):
    checked_uuid(consumer_id)
    if type(intent_revision) is not int or not 0 < intent_revision <= MAX_INTENT_REVISION:
        raise VisualizationIntentConflict('Invalid animation consumer revision.')
    for value in ((setup_id, stream_id) if setup_id is not None else (stream_id,)):
        if not isinstance(value, str) or not value.strip() or len(value) > 256 or not all(c.isprintable() for c in value):
            raise VisualizationIntentConflict('Invalid animation consumer scope.')
    return {'consumer_id': consumer_id, 'intent_revision': intent_revision,
            'setup_id': setup_id, 'stream_id': stream_id}


def checked_intent(value):
    if not isinstance(value, dict) or set(value) != INTENT_FIELDS:
        raise VisualizationIntentConflict('The animation consumer binding is incomplete.')
    scope = checked_scope(**{name: value[name] for name in INTENT_FIELDS - {'intent_token'}})
    return {**scope, 'intent_token': checked_uuid(value['intent_token'])}


class VisualizationConsumers:
    """Single-coordinator storage for monotonic intents and their checked receipts.

    Callers serialize all changes with the coordinator admission lock. High-water
    tombstones survive release, lease expiry and host reconstruction.
    """

    def __init__(self, root=None, *, lease_seconds=90, clock=None):
        if not isinstance(lease_seconds, (int, float)) or isinstance(lease_seconds, bool) or not math.isfinite(lease_seconds) or lease_seconds <= 0:
            raise ValueError('Invalid animation consumer lease.')
        self.root = Path(root) / 'consumer-intents' if root is not None else None
        self.lease_seconds = lease_seconds
        self.clock = clock or time.time
        self.records = {}
        self.loaded = False

    def now(self):
        value = self.clock()
        if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(value):
            raise VisualizationIntentConflict('The animation lease clock is unavailable.')
        return value

    @staticmethod
    def _checked_record(value):
        fields = INTENT_FIELDS | {'version', 'status', 'expires_at', 'request_fingerprint',
                                  'receipt_id', 'previous_receipt_id', 'history'}
        if not isinstance(value, dict) or set(value) != fields or type(value['version']) is not int or value['version'] != 1:
            raise ValueError('Invalid animation consumer record.')
        checked_scope(**{name: value[name] for name in INTENT_FIELDS - {'intent_token'}})
        if value['intent_token'] is not None:
            checked_uuid(value['intent_token'])
        if value['status'] not in {'live', 'released', 'expired'} or value['status'] == 'live' and value['intent_token'] is None:
            raise ValueError('Invalid animation consumer state.')
        if type(value['expires_at']) not in (int, float) or not math.isfinite(value['expires_at']):
            raise ValueError('Invalid animation consumer expiry.')
        for name in ('request_fingerprint', 'receipt_id', 'previous_receipt_id'):
            if value[name] is not None:
                checked_hash(value[name])
        if (value['request_fingerprint'] is None) != (value['receipt_id'] is None):
            raise ValueError('Incomplete animation consumer request binding.')
        if not isinstance(value['history'], dict):
            raise ValueError('Invalid animation consumer history.')
        for key, previous in value['history'].items():
            if not isinstance(previous, dict) or set(previous) != {'setup_id', 'stream_id', 'receipt_id'}:
                raise ValueError('Invalid animation consumer prior receipt.')
            checked_scope(value['consumer_id'], value['intent_revision'], previous['setup_id'], previous['stream_id'])
            checked_hash(previous['receipt_id'])
            if key != fingerprint({name: previous[name] for name in ('setup_id', 'stream_id')}):
                raise ValueError('Invalid animation consumer history scope.')
        return value

    def load(self):
        if self.loaded:
            return
        records = {}
        if self.root is not None and self.root.exists():
            if self.root.is_symlink():
                raise VisualizationIntentConflict('Saved animation consumer ownership is unavailable.')
            try:
                for path in self.root.glob('*.json'):
                    checked_uuid(path.stem)
                    value = self._checked_record(read_record(path, max_bytes=1_000_000))
                    if value['consumer_id'] != path.stem:
                        raise ValueError('Animation consumer filename mismatch.')
                    records[path.stem] = value
            except (OSError, ValueError, KeyError, TypeError, RecursionError):
                raise VisualizationIntentConflict('Saved animation consumer ownership is unavailable.') from None
        self.records = records
        self.loaded = True

    def save(self, value):
        self._checked_record(value)
        if self.root is not None:
            self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
            write_private(self.root / (value['consumer_id'] + '.json'), value)
        self.records[value['consumer_id']] = value

    def expire(self):
        self.load()
        now = self.now()
        changed = []
        for value in list(self.records.values()):
            if value['status'] == 'live' and value['expires_at'] <= now:
                value = {**value, 'status': 'expired'}
                self.save(value)
                changed.append(value)
        return changed

    def projection(self, value):
        return {**{name: value[name] for name in INTENT_FIELDS}, 'status': value['status'],
                'lease_seconds': self.lease_seconds,
                'expires_at': datetime.fromtimestamp(value['expires_at'], timezone.utc).isoformat()}

    def _new(self, scope, previous, *, live):
        history = deepcopy(previous['history']) if previous else {}
        key = fingerprint({name: scope[name] for name in ('setup_id', 'stream_id')})
        return {**scope, 'version': 1, 'intent_token': str(uuid.uuid4()) if live else None,
                'status': 'live' if live else 'released',
                'expires_at': self.now() + self.lease_seconds if live else self.now(),
                'request_fingerprint': None, 'receipt_id': None,
                'previous_receipt_id': (history.get(key) or {}).get('receipt_id'), 'history': history}

    def declare(self, **scope):
        scope = checked_scope(**scope)
        self.load()
        previous = self.records.get(scope['consumer_id'])
        if previous and scope['intent_revision'] <= previous['intent_revision']:
            if (all(previous[name] == value for name, value in scope.items())
                    and previous['status'] == 'live' and previous['expires_at'] > self.now()):
                return previous
            raise VisualizationIntentConflict('This animation intent is stale or has been withdrawn.')
        value = self._new(scope, previous, live=True)
        self.save(value)
        return value

    def release(self, *, intent_token=None, **scope):
        scope = checked_scope(**scope)
        if intent_token is not None:
            checked_uuid(intent_token)
        self.load()
        previous = self.records.get(scope['consumer_id'])
        if previous and scope['intent_revision'] < previous['intent_revision']:
            # The old revision is retired. Never expose or alter the newer token.
            return self._new(scope, None, live=False)
        if previous and scope['intent_revision'] == previous['intent_revision']:
            if any(previous[name] != value for name, value in scope.items()) or intent_token is not None and previous['intent_token'] != intent_token:
                raise VisualizationIntentConflict('This release does not match its animation intent.')
            value = {**previous, 'status': 'released' if previous['status'] == 'live' else previous['status']}
        else:
            if intent_token is not None:
                raise VisualizationIntentConflict('An undeclared animation intent has no acknowledged token.')
            value = self._new(scope, previous, live=False)
        self.save(value)
        return value

    def current(self, binding):
        binding = checked_intent(binding)
        self.load()
        value = self.records.get(binding['consumer_id'])
        if (value is None or any(value[name] != item for name, item in binding.items())
                or value['status'] != 'live' or value['expires_at'] <= self.now()):
            raise VisualizationIntentConflict('This animation intent is stale or has been withdrawn.')
        return value

    def renew(self, binding):
        value = {**self.current(binding), 'expires_at': self.now() + self.lease_seconds}
        self.save(value)
        return value

    def attach(self, binding, receipt):
        value = self.current(binding)
        if value['request_fingerprint'] is not None and (value['request_fingerprint'] != receipt['request_fingerprint'] or value['receipt_id'] != receipt['id']):
            raise VisualizationIntentConflict('This animation intent already owns a different scene request.')
        if value['receipt_id'] is not None:
            return value
        key = fingerprint({name: value[name] for name in ('setup_id', 'stream_id')})
        history = {**value['history'], key: {name: receipt[name] for name in ('setup_id', 'stream_id', 'id')}}
        history[key]['receipt_id'] = history[key].pop('id')
        value = {**value, 'request_fingerprint': receipt['request_fingerprint'], 'receipt_id': receipt['id'], 'history': history}
        self.save(value)
        return value

    def interests(self, receipt_id):
        self.load()
        now = self.now()
        return [value for value in self.records.values()
                if value['status'] == 'live' and value['expires_at'] > now and value['receipt_id'] == receipt_id]

    def next_expiry(self):
        self.load()
        return min((value['expires_at'] for value in self.records.values() if value['status'] == 'live'), default=None)
