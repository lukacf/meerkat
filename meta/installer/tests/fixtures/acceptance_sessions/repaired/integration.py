import asyncio
import json
from .ledger import Ledger, BoundaryError


class Integration:
    def __init__(self, context):
        self.context = context
        self.ledger = None
        self.device = None
        self.frontend = None
        # Decision/proposal updates only. Never held over device transport I/O.
        self.lock = asyncio.Lock()

    def store(self):
        if self.ledger is None:
            directory = self.context.root / '.mobkit' / 'observing'
            directory.mkdir(parents=True, exist_ok=True, mode=0o700)
            self.ledger = Ledger(directory / 'decisions.sqlite3')
        return self.ledger

    async def proposal(self, args):
        async with self.lock:
            return self.store().propose(args)

    async def status(self, args):
        async with self.lock:
            return self.store().status(args['session'])

    async def action(self, args):
        # Ledger atomically persists admission/ownership before its first await.
        # Other callers fail closed rather than queue behind a hung transport.
        return await self.store().act(args, self.device)

    async def admitted_event(self, event):
        """Only a host-installed authenticated frontend may call this."""
        if self.frontend is None:
            raise BoundaryError('Authenticated frontend not configured')
        admitted = await self.frontend.admit(event)
        if admitted['channel'] not in ('telegram', 'console') or not admitted['actor']:
            raise BoundaryError('Untrusted channel admission')
        if admitted['kind'] == 'decision':
            async with self.lock:
                return self.store().decide(admitted['session'], admitted['digest'],
                                           admitted['decision'], admitted['actor'], admitted['channel'])
        return await self.context.runtime.agent('lead:main').dispatch_text_and_wait(
            json.dumps({'channel': admitted['channel'], 'actor': admitted['actor'],
                        'text': admitted['text'] }), origin='connector',
            correlation_id=admitted['event_id'])

    async def evaluate(self, packet):
        return await self.context.runtime.mob_handle().run_flow('opportunity_check', packet)

    def tools_for_agent(self, agent):
        def tool(name, handler, description, schema):
            return dict(name=name, handler=handler, description=description, schema=schema)
        status = tool('observing_session_status', self.status, 'Read authoritative scope, decisions and command receipts, not image quality.',
                      {'type': 'object', 'properties': {'session': {'type': 'string'}}, 'required': ['session'], 'additionalProperties': False})
        identity = agent['identity']
        if identity == 'lead:main':
            return [status, tool('observing_propose_scope', self.proposal,
                                'Save a versioned goal/window/constraints proposal. This never approves or starts capture. UTC epoch times.',
                                {'type': 'object', 'properties': {k: {'type': t} for k, t in
                                 [('session', 'string'), ('version', 'integer'), ('goal', 'string'), ('start', 'number'), ('end', 'number'), ('constraints', 'object')]},
                                 'required': ['session', 'version', 'goal', 'start', 'end', 'constraints'], 'additionalProperties': False})]
        if identity == 'operator:main':
            return [status, tool('observing_bounded_action', self.action,
                                'Exclusive bounded command boundary. Fails closed without verified adapter/approval. Unknown outcomes require reconciliation.',
                                {'type': 'object', 'properties': {k: {'type': t} for k, t in
                                 [('session', 'string'), ('digest', 'string'), ('key', 'string'), ('kind', 'string'), ('scope', 'object'), ('target', 'string'), ('until', 'number')]},
                                 'required': ['session', 'digest', 'key', 'kind', 'scope', 'target', 'until'], 'additionalProperties': False})]
        return []

    async def stop(self):
        if self.ledger is not None:
            self.ledger.close()
            self.ledger = None
