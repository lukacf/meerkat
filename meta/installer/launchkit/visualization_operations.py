"""Background animation authoring with fingerprinted, reconnectable jobs."""
from __future__ import annotations

import asyncio
from copy import deepcopy
from datetime import datetime, timezone
import json
from pathlib import Path
import uuid

from .projects import fingerprint
from .templates.storage_setup import write_private
from .visualizer import VisualizerPending
from .visualization_consumers import VisualizationConsumers, VisualizationIntentConflict, checked_intent


class VisualizationConflict(ValueError):
    """A scene correction no longer owns the desired proposal."""


MAX_CORRECTION_MIGRATION_RECEIPTS = 4096
MAX_CORRECTION_MIGRATION_BYTES = 64_000_000


class VisualizationOperations:
    def __init__(self, *, root=None, reconnect_interval=1, reconnect_max_interval=15,
                 consumer_lease_seconds=90, consumer_clock=None):
        self.tasks = {}
        self.receipts = {}
        self.lock = asyncio.Lock()
        self.generation_lock = asyncio.Lock()
        self.jobs = {}
        self.streams = {}
        self.request_receipts = {}
        self.completed = {}
        self.last_published = {}
        self.sequence = 0
        self.root = Path(root) if root is not None else None
        self.owner_id = str(uuid.uuid4())
        self.reconnect_interval = max(0, reconnect_interval)
        self.reconnect_max_interval = max(self.reconnect_interval, reconnect_max_interval)
        self.consumers = VisualizationConsumers(self.root, lease_seconds=consumer_lease_seconds, clock=consumer_clock)
        self.consumer_lease_task = None
        self.corrections = {}

    def _scheduling(self, receipt):
        count = len(self.consumers.interests(receipt['id']))
        scope = (receipt.get('setup_id'), receipt.get('stream_id'))
        legacy = scope[1] is not None and self._desired_head(scope) == receipt['id']
        return {'desired': bool(count or legacy), 'managed_consumers': count,
                'legacy_interest': legacy, 'status': 'wanted' if count or legacy else 'withdrawn'}

    def _has_demand(self, receipt):
        if receipt.get('stream_id') is None:
            return True
        return self._scheduling(receipt)['desired']

    def _retire_undemanded(self):
        """Retire desire only; missing tasks never prove native work has stopped."""
        for receipt in list(self.receipts.values()):
            if not receipt.get('consumer_managed'):
                continue
            projection = self._scheduling(receipt)
            if receipt.get('scheduling') != projection:
                receipt['scheduling'] = projection
                self._persist(receipt)
            job = self.jobs.get(receipt['id'])
            if projection['desired'] or job is None or job['admitted']:
                continue
            job['withdrawn'] = True
            if job.get('draining'):
                # Keep the unrelated prior native observer tracked until its
                # bounded call returns. This candidate's admission then fails.
                continue
            task = self.tasks.pop(receipt['id'], None)
            self.jobs.pop(receipt['id'], None)
            if task is not None:
                task.cancel()

    def _expire_intents_locked(self):
        self.consumers.expire()
        self._retire_undemanded()

    def _watch_consumer_leases(self):
        if self.consumer_lease_task is not None and not self.consumer_lease_task.done():
            self.consumer_lease_task.cancel()
        if self.consumers.next_expiry() is None:
            self.consumer_lease_task = None
            return
        async def observe():
            try:
                while True:
                    async with self.lock:
                        self._expire_intents_locked()
                        expiry = self.consumers.next_expiry()
                        if expiry is None:
                            return
                        delay = max(0, expiry - self.consumers.now())
                    await asyncio.sleep(delay)
            finally:
                if self.consumer_lease_task is asyncio.current_task():
                    self.consumer_lease_task = None
        task = asyncio.create_task(observe())
        task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
        self.consumer_lease_task = task

    async def declare_intent(self, consumer_id, intent_revision, setup_id, stream_id):
        async with self.lock:
            self._expire_intents_locked()
            value = self.consumers.declare(consumer_id=consumer_id, intent_revision=intent_revision,
                                           setup_id=setup_id, stream_id=stream_id)
            self._retire_undemanded()
            self._watch_consumer_leases()
            return self.consumers.projection(value)

    async def release_intent(self, consumer_id, intent_revision, setup_id, stream_id, intent_token=None):
        async with self.lock:
            self._expire_intents_locked()
            value = self.consumers.release(consumer_id=consumer_id, intent_revision=intent_revision,
                                           setup_id=setup_id, stream_id=stream_id, intent_token=intent_token)
            self._retire_undemanded()
            self._watch_consumer_leases()
            return self.consumers.projection(value)

    async def renew_intent(self, consumer_id, intent_revision, setup_id, stream_id, intent_token):
        async with self.lock:
            self._expire_intents_locked()
            value = self.consumers.renew(dict(consumer_id=consumer_id, intent_revision=intent_revision,
                                             setup_id=setup_id, stream_id=stream_id, intent_token=intent_token))
            self._watch_consumer_leases()
            return self.consumers.projection(value)

    async def expire_intents(self):
        """Host lease maintenance, also available to deterministic offline clocks."""
        async with self.lock:
            self._expire_intents_locked()
            self._watch_consumer_leases()

    def _attach_consumer(self, binding, receipt):
        # Save the receipt before the durable pointer acknowledges it. A crash
        # cannot leave an accepted pointer authorizing an unknown new request.
        receipt['consumer_managed'] = True
        self._persist(receipt)
        self.consumers.attach(binding, receipt)
        self._retire_undemanded()
        self._watch_consumer_leases()

    def _persist(self, receipt):
        if self.root is None:
            return
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        write_private(self.root / (receipt['id'] + '.json'), receipt)
        index = fingerprint({name: receipt.get(name) for name in ('setup_id', 'stream_id', 'request_fingerprint')})
        write_private(self.root / ('request-' + index + '.json'), {'id': receipt['id']})

    @staticmethod
    def _checked_revision(receipt):
        """Authenticate copied continuation context without selecting new context."""
        if 'revision_context' not in receipt and 'revision_binding' not in receipt:
            artifact = receipt.get('artifact')
            if receipt.get('contract') is not None or isinstance(artifact, dict) and artifact.get('revision_binding') is not None:
                raise ValueError('The saved animation revision context is missing.')
            return None
        from .visualizer import CONTRACT_VERSION, checked_revision_context, revision_context_binding
        from .visualization_timeline import checked_hash
        context = checked_revision_context(receipt['revision_context'])
        binding = revision_context_binding(context)
        if (context['setup_id'] != receipt.get('setup_id') or context['stream_id'] != receipt.get('stream_id')
                or binding != receipt['revision_binding'] or receipt.get('contract') != CONTRACT_VERSION):
            raise ValueError('The saved animation revision context does not match its scope.')
        identity = {name: receipt[name] for name in ('contract', 'blueprint_hash', 'model_hash', 'reference_hash')}
        for name in ('blueprint_hash', 'model_hash', 'reference_hash'):
            checked_hash(identity[name])
        if 'feedback_hash' in receipt:
            identity['feedback_hash'] = checked_hash(receipt['feedback_hash'])
        identity['revision_binding'] = binding
        if fingerprint(identity) != receipt.get('native_request_key'):
            raise ValueError('The saved animation revision binding does not match its request.')
        return context

    def _previous_program(self, receipt, service, reference_hash):
        from .visualizer import CONTRACT_VERSION, checked_revision_context
        from .visualization_timeline import activity_request, publication
        if (receipt is None or receipt.get('stream_id') is None
                or receipt.get('status') not in {'published', 'completed'}):
            return None
        try:
            if not publication(receipt)['available']:
                return None
            artifact = receipt['artifact']
            if artifact.get('contract') != CONTRACT_VERSION or artifact.get('reference_hash') != reference_hash:
                return None
            self._checked_revision(receipt)
            record = activity_request(receipt, service)
            if record is None or not record.get('admission'):
                return None
            program = {name: artifact[name] for name in (
                'contract', 'blueprint_hash', 'model_hash', 'reference_hash', 'code_hash', 'code', 'summary')}
            program.update(operation_id=receipt['id'], request_key=artifact['key'], matches_current=False)
            context = checked_revision_context({'version': 'prior-program-v1',
                'setup_id': receipt.get('setup_id'), 'stream_id': receipt['stream_id'], 'previous_program': program})
            return context['previous_program']
        except (OSError, ValueError, KeyError, TypeError):
            return None

    def _published_pointer(self, setup_id, stream_id):
        return self.root / ('published-' + fingerprint({'setup_id': setup_id, 'stream_id': stream_id}) + '.json')

    def _head_pointer(self, setup_id, stream_id):
        return self.root / ('head-' + fingerprint({'setup_id': setup_id, 'stream_id': stream_id}) + '.json')

    def _desired_head(self, scope):
        if scope in self.streams:
            return self.streams[scope]
        if self.root is None:
            return None
        from .visualization_timeline import read_record
        try:
            value = read_record(self._head_pointer(*scope), max_bytes=2000)
            receipt = self.get(value['id'], project_scheduling=False)
            fields = ('id', 'setup_id', 'stream_id', 'source_generation', 'blueprint_hash', 'model_hash')
            if (receipt is not None and (receipt.get('setup_id'), receipt.get('stream_id')) == scope
                    and value == {name: receipt.get(name) for name in fields}):
                self.streams[scope] = receipt['id']
                return receipt['id']
        except (OSError, ValueError, KeyError, TypeError):
            pass
        # A historical publication cannot establish the user's current desire.
        return None

    def _remember_head(self, receipt):
        scope = (receipt.get('setup_id'), receipt['stream_id'])
        previous_id = self._desired_head(scope)
        if self.root is not None:
            self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
            write_private(self._head_pointer(*scope), {name: receipt.get(name) for name in (
                'id', 'setup_id', 'stream_id', 'source_generation', 'blueprint_hash', 'model_hash')})
        self.streams[scope] = receipt['id']
        self._replace_queued(previous_id, receipt['id'])
        self._retire_undemanded()

    def _correction_reservation(self, parent_id):
        if parent_id in self.corrections:
            return self.corrections[parent_id]
        if self.root is None:
            return None
        from .visualization_timeline import read_record, checked_hash
        path = self.root / ('correction-' + parent_id + '.json')
        try:
            value = read_record(path, max_bytes=8000)
            if (not isinstance(value, dict) or set(value) != {'parent', 'request_fingerprint', 'receipt_id'}
                    or not isinstance(value['parent'], dict) or value['parent'].get('operation_id') != parent_id):
                raise ValueError('Invalid saved correction reservation.')
            checked_hash(value['request_fingerprint'])
            checked_hash(value['receipt_id'])
        except FileNotFoundError:
            # Older hosts recorded the checked parent on the receipt before
            # there was an independent parent reservation. Recover that spent
            # budget, even if a different viewer has since moved the head.
            candidates = {receipt['id']: receipt for receipt in self.receipts.values()}
            count, size = 0, 0
            for receipt_path in self.root.glob('*.json'):
                try:
                    checked_hash(receipt_path.stem)
                except ValueError:
                    continue
                count += 1
                try:
                    size += receipt_path.stat().st_size
                    if count > MAX_CORRECTION_MIGRATION_RECEIPTS or size > MAX_CORRECTION_MIGRATION_BYTES:
                        raise VisualizationConflict('The retained correction inventory exceeds its bounded migration budget.')
                    raw = read_record(receipt_path, max_bytes=1_000_000)
                    if not isinstance(raw, dict) or raw.get('id') != receipt_path.stem:
                        raise ValueError('Uncertain retained correction authority.')
                    if (raw.get('correction_parent') is not None
                            and (not isinstance(raw['correction_parent'], dict)
                                 or raw['correction_parent'].get('operation_id') is None)):
                        raise ValueError('Uncertain retained correction parent.')
                    receipt = self.get(receipt_path.stem, project_scheduling=False)
                    if receipt is None:
                        raise ValueError('Unverifiable retained correction authority.')
                    if isinstance(raw.get('correction_parent'), dict) and raw['correction_parent'].get('operation_id') == parent_id:
                        candidates[receipt['id']] = receipt
                except VisualizationConflict:
                    raise
                except (OSError, ValueError, KeyError, TypeError, RecursionError):
                    raise VisualizationConflict('The retained correction inventory is unavailable.') from None
            matches = [receipt for receipt in candidates.values()
                       if isinstance(receipt.get('correction_parent'), dict)
                       and receipt['correction_parent'].get('operation_id') == parent_id]
            if not matches:
                self.corrections[parent_id] = None
                return None
            if len(matches) != 1:
                raise VisualizationConflict('The retained correction budget is ambiguous.')
            receipt = matches[0]
            previous = ((receipt.get('revision_context') or {}).get('previous_program') or {})
            parent = self.get(parent_id, project_scheduling=False)
            expected = receipt['correction_parent']
            fields = {'operation_id', 'code_hash', 'blueprint_hash', 'model_hash', 'setup_id', 'stream_id'}
            if (set(expected) != fields or parent is None or previous.get('operation_id') != parent_id
                    or any(expected.get(name) != parent.get(name) or receipt.get(name) != parent.get(name)
                           for name in ('setup_id', 'stream_id', 'blueprint_hash', 'model_hash'))
                    or expected.get('code_hash') != (parent.get('artifact') or {}).get('code_hash')
                    or previous.get('request_key') != (parent.get('artifact') or {}).get('key')
                    or any(previous.get(name) != (parent.get('artifact') or {}).get(name)
                           for name in ('blueprint_hash', 'model_hash', 'reference_hash', 'code_hash'))):
                raise VisualizationConflict('The retained correction provenance is unavailable.')
            value = {'parent': deepcopy(receipt['correction_parent']),
                     'request_fingerprint': receipt['request_fingerprint'], 'receipt_id': receipt['id']}
            write_private(path, value)
        except (OSError, ValueError, KeyError, TypeError):
            raise VisualizationConflict('The retained correction reservation is unavailable.') from None
        self.corrections[parent_id] = value
        return value

    def _reserve_correction(self, expected, identity, receipt):
        value = {'parent': deepcopy(expected), 'request_fingerprint': identity, 'receipt_id': receipt['id']}
        previous = self._correction_reservation(expected['operation_id'])
        if previous is not None and previous != value:
            raise VisualizationConflict('This proposal already used its automatic visual correction.')
        if self.root is not None:
            write_private(self.root / ('correction-' + expected['operation_id'] + '.json'), value)
        self.corrections[expected['operation_id']] = value

    def _correction_parent(self, expected, service, source, scope, identity, consumer=None):
        """Check the parent's source and desired head while holding self.lock."""
        from .visualization_timeline import checked_hash
        fields = {'operation_id', 'code_hash', 'blueprint_hash', 'model_hash', 'setup_id', 'stream_id'}
        if not isinstance(expected, dict) or set(expected) != fields:
            raise VisualizationConflict('The reviewed animation binding is incomplete.')
        try:
            for name in ('operation_id', 'code_hash', 'blueprint_hash', 'model_hash'):
                checked_hash(expected[name])
        except ValueError:
            raise VisualizationConflict('The reviewed animation binding is invalid.') from None
        parent = self.get(expected['operation_id'])
        if (scope[1] is None or (expected['setup_id'], expected['stream_id']) != scope or parent is None
                or any(parent.get(name) != expected[name] for name in ('setup_id', 'stream_id', 'blueprint_hash', 'model_hash'))
                or any(source.get(name) != expected[name] for name in ('blueprint_hash', 'model_hash'))
                or (parent.get('artifact') or {}).get('code_hash') != expected['code_hash']):
            raise VisualizationConflict('The correction does not match its reviewed proposal and scene.')
        if self._previous_program(parent, service, source.get('reference_hash')) is None:
            raise VisualizationConflict('The reviewed animation publication is no longer verifiable.')
        if consumer is not None:
            owned = {consumer.get('receipt_id'), consumer.get('previous_receipt_id')}
            reservation = self._correction_reservation(parent['id'])
            replay = self.get(reservation['receipt_id']) if reservation is not None else None
            if parent['id'] not in owned and (replay is None or replay['id'] not in owned):
                raise VisualizationConflict('This viewer did not own the reviewed animation.')
            if reservation is not None:
                if (reservation['parent'] != expected or reservation['request_fingerprint'] != identity
                        or replay is None or replay.get('correction_parent') != expected
                        or replay.get('request_fingerprint') != identity):
                    raise VisualizationConflict('This proposal already used its automatic visual correction.')
                return parent, replay
            if parent.get('correction_parent') is not None:
                raise VisualizationConflict('This proposal already used its automatic visual correction.')
            return parent, None
        head_id = self._desired_head(scope)
        if head_id != parent['id']:
            head = self.get(head_id) if head_id is not None else None
            if (head is not None and head.get('correction_parent') == expected
                    and head.get('request_fingerprint') == identity
                    and head.get('status') in {'queued', 'running', 'observing', 'published', 'completed'}):
                return parent, head
            raise VisualizationConflict('A newer scene request superseded this review.')
        if parent.get('correction_parent') is not None:
            raise VisualizationConflict('This proposal already used its automatic visual correction.')
        return parent, None

    def _revision_context(self, service, setup_id, stream_id, reference_hash):
        from .visualizer import checked_revision_context
        from .visualization_timeline import read_record
        scope = (setup_id, stream_id)
        previous_id = self.last_published.get(scope)
        if previous_id is None and self.root is not None:
            try:
                pointer = read_record(self._published_pointer(*scope), max_bytes=1000)
                if isinstance(pointer, dict) and pointer.get('setup_id') == setup_id and pointer.get('stream_id') == stream_id:
                    previous_id = pointer['id']
            except (OSError, ValueError, KeyError, TypeError):
                pass
        if previous_id is not None:
            self.get(previous_id)
        candidates = [value for value in self.receipts.values()
                      if value.get('setup_id') == setup_id and value.get('stream_id') == stream_id]
        candidates.sort(key=lambda value: value.get('published_at') if isinstance(value.get('published_at'), str) else '', reverse=True)
        previous_program = None
        for candidate in candidates:
            previous_program = self._previous_program(candidate, service, reference_hash)
            if previous_program is not None:
                break
        return checked_revision_context({'version': 'prior-program-v1', 'setup_id': setup_id,
            'stream_id': stream_id, 'previous_program': previous_program})

    def _remember_publication(self, receipt, service):
        if (service is None or receipt.get('stream_id') is None
                or self._previous_program(receipt, service, (receipt.get('artifact') or {}).get('reference_hash')) is None):
            return
        scope = (receipt.get('setup_id'), receipt['stream_id'])
        previous = self._latest_publication(service, *scope, reference_hash=receipt['artifact']['reference_hash'])
        # A later completion of an old publication must not move the saved
        # selection backwards. published_at is set only on first publication.
        if previous is not None and self._publication_order(previous) > self._publication_order(receipt):
            return
        self.last_published[scope] = receipt['id']
        if self.root is not None:
            write_private(self._published_pointer(*scope), {
                'setup_id': scope[0], 'stream_id': scope[1], 'id': receipt['id']})

    @staticmethod
    def _publication_order(receipt):
        return (receipt.get('published_at') or '', receipt.get('queued_at') or '', receipt['id'])

    def _latest_publication(self, service, setup_id, stream_id, *, blueprint_hash=None, model_hash=None,
                            reference_hash=None):
        from .visualizer import visualization_reference_hash
        from .visualization_timeline import read_record
        scope = (setup_id, stream_id)
        previous_id = self.last_published.get(scope)
        if self.root is not None:
            try:
                pointer = read_record(self._published_pointer(*scope), max_bytes=1000)
                if isinstance(pointer, dict) and set(pointer) == {'setup_id', 'stream_id', 'id'} and (
                        pointer['setup_id'], pointer['stream_id']) == scope:
                    self.get(pointer['id'], project_scheduling=False)
            except (OSError, ValueError, KeyError, TypeError):
                pass
        if previous_id is not None:
            self.get(previous_id, project_scheduling=False)
        reference_hash = reference_hash or visualization_reference_hash()
        candidates = [value for value in self.receipts.values()
            if (value.get('setup_id'), value.get('stream_id')) == scope
            and (blueprint_hash is None or value.get('blueprint_hash') == blueprint_hash)
            and (model_hash is None or value.get('model_hash') == model_hash)
            and self._previous_program(value, service, reference_hash) is not None]
        return max(candidates, key=self._publication_order) if candidates else None

    def latest_saved(self, service, *, setup_id, stream_id, blueprint_hash, model_hash):
        """Read an existing checked publication; no request or native work is admitted."""
        from .visualization_timeline import checked_hash
        checked_hash(blueprint_hash)
        checked_hash(model_hash)
        if any(not isinstance(value, str) or not value.strip() or len(value) > 256
               or not all(character.isprintable() for character in value) for value in (setup_id, stream_id)):
            raise ValueError('Invalid saved animation scope.')
        value = self._latest_publication(service, setup_id, stream_id,
            blueprint_hash=blueprint_hash, model_hash=model_hash)
        return deepcopy(value) if value is not None else None

    async def adopt_saved(self, service, *, publication, consumer_intent, require_latest=True):
        """Attach a declared viewer to exact saved source without rewriting its receipt."""
        from .visualizer import visualization_reference_hash
        publication, consumer_intent = deepcopy(publication), checked_intent(deepcopy(consumer_intent))
        if type(require_latest) is not bool:
            raise ValueError('Invalid saved animation selection.')
        fields = {'operation_id', 'code_hash', 'blueprint_hash', 'model_hash', 'setup_id', 'stream_id'}
        if not isinstance(publication, dict) or set(publication) != fields or (
                publication['setup_id'], publication['stream_id']) != (
                consumer_intent['setup_id'], consumer_intent['stream_id']):
            raise VisualizationConflict('The saved animation does not match this view.')
        async with self.lock:
            consumer = self.consumers.current(consumer_intent)
            receipt = self.get(publication['operation_id'], project_scheduling=False)
            latest = self.latest_saved(service, **{name: publication[name] for name in (
                'setup_id', 'stream_id', 'blueprint_hash', 'model_hash')})
            if (receipt is None or any(receipt.get(name) != publication[name] for name in (
                    'setup_id', 'stream_id', 'blueprint_hash', 'model_hash'))
                    or (receipt.get('artifact') or {}).get('code_hash') != publication['code_hash']
                    or self._previous_program(receipt, service, visualization_reference_hash()) is None
                    or (require_latest and consumer.get('receipt_id') is None and (latest or {}).get('id') != receipt['id'])):
                raise VisualizationConflict('The saved animation changed or no longer matches this proposal. Check it again.')
            self.consumers.attach(consumer_intent, receipt)
            self._watch_consumer_leases()
            return deepcopy(receipt)

    @staticmethod
    def _cache_key(receipt, identity):
        return ('native', receipt['native_request_key']) if 'revision_context' in receipt else identity

    async def submit(self, service, blueprint, model, *, feedback="", stream_id=None, setup_id=None,
                     expected_parent=None, consumer_intent=None, replay_parent=None):
        # Snapshot caller-owned data before waiting for the coordinator lock.
        # Hashing and generation must describe the same submitted proposal.
        blueprint, model = deepcopy(blueprint), deepcopy(model)
        expected_parent = deepcopy(expected_parent)
        replay_parent = deepcopy(replay_parent)
        consumer_intent = deepcopy(consumer_intent)
        if stream_id is not None and (not isinstance(stream_id, str) or not stream_id.strip() or len(stream_id) > 256):
            raise ValueError('The visualization stream must be a non-empty string of at most 256 characters.')
        if setup_id is not None and (not isinstance(setup_id, str) or not setup_id.strip() or len(setup_id) > 256):
            raise ValueError('The visualization setup must be a non-empty string of at most 256 characters.')
        identity = fingerprint({'blueprint': blueprint, 'model': model, **({'feedback': feedback} if feedback else {})})
        source = {'blueprint_hash': fingerprint(blueprint), 'model_hash': fingerprint(model)}
        binding = getattr(service, 'request_binding', None)
        if binding:
            source.update(binding(blueprint, model, feedback))
        ordinary_source = source
        source = {key: value for key, value in source.items() if key in {'blueprint_hash', 'model_hash', 'native_request_key'}}
        continuous = stream_id is not None and callable(getattr(service, 'generate_continuous_with_native_admission', None))
        async with self.lock:
            consumer = None
            if consumer_intent is not None:
                consumer_intent = checked_intent(consumer_intent)
                self._expire_intents_locked()
                if (consumer_intent['setup_id'], consumer_intent['stream_id']) != (setup_id, stream_id):
                    raise VisualizationIntentConflict('The animation intent does not match its submitted view.')
                consumer = self.consumers.current(consumer_intent)
                if consumer['request_fingerprint'] is not None and consumer['request_fingerprint'] != identity:
                    raise VisualizationIntentConflict('This animation intent already owns a different scene request.')
            if replay_parent is not None:
                # Restoring ownership must never turn a missing request index
                # or changed reference into permission for new native work.
                from .visualization_timeline import checked_hash
                fields = {'operation_id', 'code_hash', 'blueprint_hash', 'model_hash', 'setup_id', 'stream_id'}
                try:
                    if (not continuous or consumer is None or expected_parent is not None
                            or not isinstance(replay_parent, dict) or set(replay_parent) != fields):
                        raise ValueError('Invalid parent restoration.')
                    for name in ('operation_id', 'code_hash', 'blueprint_hash', 'model_hash'):
                        checked_hash(replay_parent[name])
                    restored = self.get(replay_parent['operation_id'])
                    if (restored is None or restored.get('correction_parent') is not None
                            or (replay_parent['setup_id'], replay_parent['stream_id']) != (setup_id, stream_id)
                            or restored.get('request_fingerprint') != identity
                            or any(restored.get(name) != replay_parent[name] or source[name] != replay_parent[name]
                                   for name in ('blueprint_hash', 'model_hash'))
                            or any(restored.get(name) != replay_parent[name] for name in ('setup_id', 'stream_id'))
                            or (restored.get('artifact') or {}).get('code_hash') != replay_parent['code_hash']
                            or self._previous_program(restored, service, ordinary_source.get('reference_hash')) is None):
                        raise ValueError('The exact saved parent is unavailable.')
                    replay_source = binding(blueprint, model, feedback, revision_context=self._checked_revision(restored))
                    if any(restored.get(name) != replay_source.get(name) for name in (
                            'contract', 'reference_hash', 'native_request_key', 'revision_binding')):
                        raise ValueError('The saved parent source changed.')
                except (OSError, ValueError, KeyError, TypeError):
                    raise VisualizationConflict('The saved animation cannot be attached exactly. No new work was sent.') from None
                self._attach_consumer(consumer_intent, restored)
                return restored
            parent = None
            reserved_replay = None
            if expected_parent is not None:
                if not continuous or not feedback:
                    raise VisualizationConflict('A visual correction needs a supported scene service and findings.')
                parent, replay = self._correction_parent(expected_parent, service, ordinary_source,
                    (setup_id, stream_id), identity, consumer)
                if replay is not None:
                    try:
                        replay_source = binding(blueprint, model, feedback,
                            revision_context=self._checked_revision(replay))
                        if (any(replay.get(name) != replay_source.get(name) for name in (
                                'contract', 'reference_hash', 'native_request_key', 'revision_binding'))
                                or ((replay.get('revision_context') or {}).get('previous_program') or {}).get('operation_id') != parent['id']):
                            raise ValueError('The correction source binding changed.')
                    except (OSError, ValueError, KeyError, TypeError):
                        raise VisualizationConflict('This retained correction does not match its current source binding.') from None
                    if replay['status'] not in ('queued', 'running', 'observing', 'published', 'completed'):
                        if consumer is not None:
                            self._attach_consumer(consumer_intent, replay)
                        return replay
                    # Exact correction recovery must use the same task and
                    # retained-native checks as any other accepted receipt.
                    # A reservation alone does not establish a live observer.
                    reserved_replay = replay
            request_key = (setup_id, stream_id, identity)
            previous_id = reserved_replay['id'] if reserved_replay is not None else consumer.get('receipt_id') if consumer is not None else None
            previous_id = previous_id or self.request_receipts.get(request_key)
            if previous_id is None and self.root is not None:
                index = fingerprint({'setup_id': setup_id, 'stream_id': stream_id, 'request_fingerprint': identity})
                path = self.root / ('request-' + index + '.json')
                try:
                    if not path.is_symlink() and path.stat().st_size <= 256:
                        previous_id = json.loads(path.read_text())['id']
                except (OSError, ValueError, KeyError, TypeError):
                    pass
            previous = self.get(previous_id) if previous_id is not None else None
            if consumer is not None and consumer.get('receipt_id') is not None and previous is None:
                raise VisualizationIntentConflict('The accepted animation request is unavailable. No new work was sent.')
            if previous and any(previous.get(name) != value for name, value in {
                    'setup_id': setup_id, 'stream_id': stream_id, 'request_fingerprint': identity,
                    'blueprint_hash': source['blueprint_hash'], 'model_hash': source['model_hash']}.items()):
                previous = None
            if previous and parent is not None and (previous.get('correction_parent') != expected_parent
                    or ((previous.get('revision_context') or {}).get('previous_program') or {}).get('operation_id') != parent['id']):
                previous = None
            if previous:
                if 'revision_context' in previous:
                    replay_source = binding(blueprint, model, feedback, revision_context=self._checked_revision(previous)) if continuous and binding else None
                    if replay_source is not None and any(previous.get(name) != replay_source.get(name) for name in (
                            'contract', 'reference_hash', 'native_request_key', 'revision_binding')):
                        previous = None
                elif previous.get('native_request_key') != source.get('native_request_key'):
                    previous = None
            if reserved_replay is not None and previous is None:
                raise VisualizationConflict('The reserved correction no longer matches its checked source. No new work was sent.')
            reusable = previous and previous['status'] in ('queued', 'running', 'observing', 'published', 'completed')
            if consumer is not None and consumer.get('receipt_id') is not None and not reusable:
                # A lost response replays the actual prior terminal receipt,
                # never a fresh native retry under the same accepted intent.
                if previous is not None and previous.get('request_fingerprint') == identity:
                    self._attach_consumer(consumer_intent, previous)
                    return previous
                raise VisualizationIntentConflict('The accepted animation binding no longer matches its source.')
            if reusable:
                receipt = previous
            else:
                revision_context = None
                if continuous:
                    if parent is None:
                        revision_context = self._revision_context(service, setup_id, stream_id, ordinary_source['reference_hash'])
                    else:
                        from .visualizer import checked_revision_context
                        revision_context = checked_revision_context({'version': 'prior-program-v1',
                            'setup_id': setup_id, 'stream_id': stream_id,
                            'previous_program': self._previous_program(parent, service, ordinary_source['reference_hash'])})
                    source = binding(blueprint, model, feedback, revision_context=revision_context)
                    source = {key: value for key, value in source.items() if key in {
                        'contract', 'blueprint_hash', 'model_hash', 'reference_hash', 'feedback_hash', 'native_request_key', 'revision_binding'}}
                self.sequence += 1
                # Stream receipts describe individual requests. A superseded
                # receipt must remain stable even if the user returns to its content.
                receipt_id = identity if stream_id is None and setup_id is None else fingerprint({
                    'setup': setup_id, 'stream': stream_id, 'request': identity, 'sequence': self.sequence,
                    **({'owner': self.owner_id} if self.root is not None else {}),
                })
                now = datetime.now(timezone.utc).isoformat()
                receipt = {'id': receipt_id, 'status': 'queued' if stream_id is not None else 'running', 'queued_at': now,
                           'setup_id': setup_id, 'stream_id': stream_id, 'request_fingerprint': identity,
                           'source_generation': self.sequence, **source}
                if revision_context is not None:
                    receipt['revision_context'] = revision_context
                    self._checked_revision(receipt)
                if parent is not None:
                    receipt['correction_parent'] = deepcopy(expected_parent)
                self.receipts[receipt_id] = receipt
                self.request_receipts[request_key] = receipt_id

            if parent is not None:
                # Persist the exact reservation before the task can admit work.
                self._persist(receipt)
                self._reserve_correction(expected_parent, identity, receipt)
            if consumer is not None:
                self._attach_consumer(consumer_intent, receipt)
            elif stream_id is not None:
                self._remember_head(receipt)
            if reusable and (receipt['id'] in self.tasks or receipt['status'] == 'completed'):
                return receipt
            if 'revision_context' in receipt and not continuous:
                receipt.update(observation_status='unavailable',
                    detail='This saved revision needs its supported animation service. No new native work was sent.')
                self._persist(receipt)
                return receipt
            retained_native = any(receipt.get(name) is not None for name in ('native_admission', 'native_interaction_id', 'started_at'))
            if reusable and self.root is not None and (receipt['status'] != 'queued' or retained_native):
                # A lost HTTP owner is not permission to repeat native work.
                # Resume only its exact saved native owner or checked artifact.
                reattach = getattr(service, 'can_reattach', None)
                try:
                    available = reattach is not None and reattach(receipt.get('native_request_key'))
                except (OSError, ValueError, KeyError, TypeError):
                    available = False
                if not available:
                    saved = None
                    reader = getattr(service, 'saved_request', None)
                    if reader:
                        try:
                            saved = reader(receipt.get('native_request_key'))
                        except (OSError, ValueError, KeyError, TypeError):
                            pass
                    if saved and saved.get('native_run') and not receipt.get('artifact'):
                        receipt.update(status='failed', detail='The saved native request ended without a matching published animation. No new native work was sent.')
                        self._persist(receipt)
                        return receipt
                    receipt.update(observation_status='unavailable',
                        detail='This saved scene request has no reattachable generation owner. Its retained native activity can be observed; no new native work was sent.')
                    self._persist(receipt)
                    return receipt
            cache_key = self._cache_key(receipt, identity)
            if cache_key in self.completed:
                self._publish_receipt(receipt, {**self.completed[cache_key], 'cached': True}, service=service)
                return receipt
            self._persist(receipt)
            self.jobs[receipt['id']] = {'admitted': bool(reusable and retained_native), 'fingerprint': identity}
            task = asyncio.create_task(self._run(service, blueprint, model, receipt, feedback, cache_key))
            self.tasks[receipt['id']] = task
            task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
            return receipt

    def _replace_queued(self, previous_id, replacement_id):
        if previous_id is None or previous_id == replacement_id:
            return
        job = self.jobs.get(previous_id)
        if job is None or job['admitted']:
            return
        if self._has_demand(self.receipts[previous_id]):
            return
        if self.receipts[previous_id].get('consumer_managed'):
            self._retire_undemanded()
            return
        self.receipts[previous_id].update(status='superseded', replacement_id=replacement_id,
                                         superseded_at=datetime.now(timezone.utc).isoformat())
        self._persist(self.receipts[previous_id])
        if job.get('draining'):
            # This coroutine is observing a different saved native owner. Let
            # that bounded window finish; its admission fence will reject the
            # superseded desired request without cancelling the prior observer.
            return
        task = self.tasks.pop(previous_id, None)
        self.jobs.pop(previous_id, None)
        if task is not None:
            task.cancel()

    async def _run(self, service, blueprint, model, receipt, feedback, identity):
        # Capture the exact coroutine owner even while it is waiting for the
        # generation lock. A later same-receipt attachment may own a new job.
        job = self.jobs.get(receipt['id'])
        try:
            # Once admitted, a frozen proposal owns the native identity until it
            # finishes. A desired revision draining a prior owner is still queued.
            async with self.generation_lock:
                async with self.lock:
                    self._expire_intents_locked()
                    job = self.jobs.get(receipt['id'])
                    if job is None or receipt['status'] == 'superseded' or not job['admitted'] and not self._has_demand(receipt):
                        return
                    artifact = self.completed.get(identity)
                    if artifact is not None:
                        artifact = {**artifact, 'cached': True}
                if artifact is None:
                    native_admission_aware = getattr(service, 'generate_with_native_admission', None)
                    continuous = getattr(service, 'generate_continuous_with_native_admission', None) if 'revision_context' in receipt else None
                    if continuous is not None:
                        native_admission_aware = continuous
                    admission_aware = native_admission_aware or getattr(service, 'generate_with_admission', None)
                    async def admit():
                        async with self.lock:
                            self._expire_intents_locked()
                            job = self.jobs.get(receipt['id'])
                            if job is None or receipt['status'] == 'superseded' or not job['admitted'] and not self._has_demand(receipt):
                                raise asyncio.CancelledError
                            if not job['admitted']:
                                job['admitted'] = True
                                receipt.update(status='running')
                            # Retained custody protects reconnects from demand
                            # cancellation. This actually observed admission
                            # boundary may still fill a missing legacy start.
                            receipt.setdefault('started_at', datetime.now(timezone.utc).isoformat())
                            self._persist(receipt)
                    async def native_admit(request_key, admission):
                        from .visualization_timeline import checked_admission
                        value = checked_admission(admission)
                        async with self.lock:
                            owned = self.jobs.get(receipt['id'])
                            if (owned is None or not owned['admitted'] or receipt['status'] == 'superseded'
                                    or request_key != receipt.get('native_request_key')):
                                raise ValueError('The native admission does not belong to this scene request.')
                            previous = receipt.get('native_admission')
                            if previous is not None and previous != value:
                                raise ValueError('The native admission changed for this scene request.')
                            receipt['native_admission'] = value
                            self._persist(receipt)
                    # Legacy services begin their own work when generate starts.
                    # The native service reports its exact request boundary so
                    # observing a restored prior owner remains replaceable.
                    if admission_aware is None:
                        await admit()
                    reconnects = 0
                    while artifact is None:
                        try:
                            options = {"feedback": feedback} if feedback else {}
                            if continuous is not None:
                                # This context was frozen at submit. Queue waits
                                # and reconnects never select a later publication.
                                options['revision_context'] = deepcopy(self._checked_revision(receipt))
                            if native_admission_aware is not None:
                                options['on_native_admission'] = native_admit
                            if admission_aware is not None:
                                job['draining'] = True
                                try:
                                    artifact = await admission_aware(blueprint, model, on_admission=admit, **options)
                                finally:
                                    job['draining'] = False
                            else:
                                artifact = await service.generate(blueprint, model, **options)
                        except VisualizerPending as pending:
                            if receipt['status'] == 'superseded' or not job['admitted'] and not self._has_demand(receipt):
                                return
                            reconnects += 1
                            # A replaceable revision can be draining an older
                            # saved owner. Never attach that owner's native ID
                            # or source key to the newer HTTP operation receipt.
                            matches = receipt.get('native_request_key') in (None, pending.request_key)
                            receipt.update(status='observing' if matches else 'queued', observation_status='reconnecting',
                                native_run_status='unknown', reconnect_attempts=reconnects,
                                detail='Still composing. Reconnecting to the saved native request; no terminal result has been observed.')
                            if matches:
                                receipt['native_request_key'] = pending.request_key
                                receipt['native_interaction_id'] = pending.interaction_id
                            self._persist(receipt)
                            # Each service call has a bounded observation window.
                            # Cap retry pacing while this owned task lives; its
                            # shutdown cancellation stops observation cleanly.
                            delay = min(self.reconnect_max_interval,
                                self.reconnect_interval * 2 ** min(reconnects - 1, 16))
                            await asyncio.sleep(delay)
                if receipt['status'] == 'superseded':
                    return
                self._publish_receipt(receipt, artifact, service=service)
                self.completed[identity] = artifact
                observer = getattr(service, 'observe_generation', None)
                if receipt['status'] == 'published' and observer:
                    try:
                        observed = await observer(artifact['key'])
                    except TimeoutError:
                        observed = None
                        receipt.update(observation_status='timed_out',
                                       detail='The animation is published. Its optional native tail has not completed observation.')
                    if observed:
                        self._publish_receipt(receipt, observed, service=service)
                        self.completed[identity] = observed
        except asyncio.CancelledError:
            withdrawn = ((job or {}).get('withdrawn') or (job or {}).get('stopping_unadmitted')
                         or receipt.get('consumer_managed') and not (job or {}).get('admitted') and not self._has_demand(receipt))
            if receipt['status'] != 'superseded' and not withdrawn:
                if receipt.get('artifact'):
                    receipt.update(status='published', observation_status='interrupted',
                                   detail='The animation was published before the setup server stopped.')
                else:
                    receipt.update(status='interrupted', detail='The setup server stopped while composing this scene.')
            raise
        except Exception:
            if receipt['status'] == 'superseded' or receipt.get('consumer_managed') and not (job or {}).get('admitted') and not self._has_demand(receipt):
                return
            if receipt.get('artifact'):
                receipt.update(status='published', detail='The animation is available; its optional native tail could not be observed.')
            else:
                receipt.update(status='failed', detail='The visualization agent could not finish this scene. The automatic network preview remains available.')
        finally:
            try:
                self._persist(receipt)
            finally:
                if self.tasks.get(receipt['id']) is asyncio.current_task():
                    self.tasks.pop(receipt['id'], None)
                    self.jobs.pop(receipt['id'], None)

    def get(self, identity, *, project_scheduling=True):
        from .visualization_timeline import checked_hash, checked_admission, read_record
        try:
            checked_hash(identity)
        except ValueError:
            return None
        receipt = self.receipts.get(identity)
        if receipt is None and self.root is not None:
            try:
                value = read_record(self.root / (identity + '.json'), max_bytes=1_000_000)
                if (value.get('id') == identity and value.get('status') in {'queued', 'running', 'observing', 'published', 'completed', 'failed', 'interrupted', 'superseded'}
                        and type(value.get('source_generation')) is int and value['source_generation'] > 0
                        and all(value.get(name) is None or isinstance(value[name], str) and 0 < len(value[name]) <= 256 and all(c.isprintable() for c in value[name]) for name in ('setup_id', 'stream_id'))
                        and all(checked_hash(value.get(name)) for name in ('request_fingerprint', 'blueprint_hash', 'model_hash', 'native_request_key'))):
                    if value.get('native_admission') is not None:
                        value['native_admission'] = checked_admission(value['native_admission'])
                    self._checked_revision(value)
                    receipt = self.receipts[identity] = value
            except (OSError, ValueError, KeyError, TypeError):
                pass
        if receipt is not None:
            try:
                self._checked_revision(receipt)
            except (ValueError, KeyError, TypeError):
                return None
            if project_scheduling and receipt.get('consumer_managed'):
                try:
                    # Desire is a current host projection, not native terminal
                    # evidence. A restored receipt may not have been loaded
                    # when its viewer was released or its lease expired.
                    receipt['scheduling'] = self._scheduling(receipt)
                except VisualizationIntentConflict:
                    receipt['scheduling'] = {'status': 'unavailable', 'desired': None,
                                             'managed_consumers': None, 'legacy_interest': None}
        return receipt

    def _publish_receipt(self, receipt, artifact, *, service=None):
        from .visualization_timeline import checked_admission, publication
        if 'revision_context' in receipt:
            self._checked_revision(receipt)
            if (artifact.get('revision_binding') != receipt['revision_binding']
                    or not publication({**receipt, 'artifact': artifact})['available']):
                raise ValueError('The published animation does not match this revision request.')
        if artifact.get('native_admission') is not None:
            admission = checked_admission(artifact['native_admission'])
            if receipt.get('native_admission') is not None and receipt['native_admission'] != admission:
                raise ValueError('The published native admission changed for this scene request.')
            receipt['native_admission'] = admission
        native_status = artifact.get('native_run', {}).get('status')
        # Legacy artifacts were returned only after a native terminal. New
        # acknowledged outputs can be shown independently of optional narration.
        complete = native_status in (None, 'completed')
        if receipt.get('observation_status') == 'reconnecting':
            receipt.pop('observation_status', None)
            receipt.pop('detail', None)
        receipt.update(status='completed' if complete else 'published', artifact=artifact,
                       native_run_status=native_status or 'completed')
        # First publication and later native completion are separate observations.
        receipt.setdefault('published_at', datetime.now(timezone.utc).isoformat())
        if complete:
            receipt['completed_at'] = datetime.now(timezone.utc).isoformat()
        self._persist(receipt)
        self._remember_publication(receipt, service)

    async def shutdown(self):
        lease_task = self.consumer_lease_task
        self.consumer_lease_task = None
        if lease_task is not None:
            lease_task.cancel()
            await asyncio.gather(lease_task, return_exceptions=True)
        pending = list(self.tasks.values())
        # A task cancelled before its coroutine starts cannot update its receipt.
        for identity in self.tasks:
            receipt = self.receipts[identity]
            if receipt['status'] == 'superseded':
                continue
            if receipt.get('consumer_managed') and not self.jobs.get(identity, {}).get('admitted'):
                self.jobs[identity]['stopping_unadmitted'] = True
                # This host positively owns an unadmitted queue. Closing its
                # observation task does not make the wanted request terminal.
                self._persist(receipt)
                continue
            if receipt.get('artifact'):
                receipt.update(status='published', observation_status='interrupted',
                               detail='The animation was published before the setup server stopped.')
            else:
                receipt.update(status='interrupted', detail='The setup server stopped while composing this scene.')
            self._persist(receipt)
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)
        self.tasks.clear()
        self.jobs.clear()
