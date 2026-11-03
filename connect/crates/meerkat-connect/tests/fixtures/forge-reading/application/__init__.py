from contextlib import closing
import json
from .collection import Integration as BaseIntegration
from .frontends import ReadingFrontend
from .telegram_runtime import ReadingTelegram


class Integration(BaseIntegration):
    def __init__(self, context, trial=False):
        super().__init__(context, trial=trial)
        self.frontends = ReadingFrontend(context, self.store)
        self.telegram = ReadingTelegram(context, self.store, self.frontends)
        self.frontends.activation_observer = self.telegram.observation

    async def start(self):
        self.frontends.start()
        await self.telegram.start()

    async def stop(self):
        try:
            await self.telegram.stop()
        finally:
            self.frontends.stop()

    async def frontend_management(self, request):
        # Called only through the protected host facade, never an agent tool.
        result = self.frontends.manage(request, explicit_submission=True)
        if request.operation != "submit":
            return result
        # The transaction owner declares disposition at its exact branch.
        # Observation strings and later native close status never choose it.
        observation = result.observation
        if observation.get("status") == "committed":
            await self.telegram.configuration_committed(observation["revision"])
            observation = {**observation, "management_observation": self.frontends.management_observation()}
        formatter = getattr(self.context, "frontend_submission_result", None)
        if formatter is not None:
            return formatter(request, observation, known=result.known)
        return observation

    def snapshot(self, args):
        result = super().snapshot(args)
        books = result['books']
        # Group the authoritative records, including their actual current reactions
        # and retained evidence. Never manufacture records from suggestions/memory.
        result['groups'] = {
            status: [book for book in books if book['status'] == status]
            for status in ('wishlist', 'finished')
        }
        wishlist = len(result['groups']['wishlist'])
        finished = len(result['groups']['finished'])
        result['summary'] = (
            f"Total books: {len(books)}; wishlist: {wishlist}; finished: {finished}."
        )
        result['status_explanation'] = (
            f"Stored records: {wishlist} saved for later (wishlist), {finished} marked finished. "
            "Wishlist does not establish ownership or completed reading. "
            "No finished record does not mean you have never read a book. "
            "Suggestions are not saved unless you explicitly request an add and confirm it."
        )
        return result

    def tools_for_agent(self, agent):
        tools = super().tools_for_agent(agent)
        for tool in tools:
            if tool['name'] == 'collection_snapshot':
                tool['description'] += (
                    ' For saved-collection questions, answer concisely from returned books, summary '
                    'and status_explanation. Use groups.wishlist (saved for later) and groups.finished '
                    '(marked finished) as separate scannable lists; empty lists explicitly mean no '
                    'stored records in that group. Groups contain the same actual records as books, '
                    'including revisions and only stored reactions, with corrections reflected. '
                    'Wishlist means saved for later, not ownership or '
                    'completed reading. Say no finished record is stored, not that the reader '
                    'has never read it. Distinguish unsaved suggestions from saved records. '
                    'Do not infer metadata, preferences or changes; this callback is read-only.'
                )
        return tools

    async def on_console_input(self, event):
        # Called only by the host, never registered as an agent callback.
        if (event.channel != 'host-authenticated-console'
                or event.identity not in ('lead:main', 'librarian:main')
                or not isinstance(event.content, str)):
            return
        text = event.content.strip()
        if not text.startswith('confirm '):
            return
        token = text[8:]
        with closing(self.store.connect()) as db, db:
            db.execute('CREATE TABLE IF NOT EXISTS approvals(token TEXT PRIMARY KEY,payload TEXT NOT NULL)')
            db.execute('BEGIN IMMEDIATE')
            pending = db.execute('SELECT * FROM pending WHERE token=?', (token,)).fetchone()
            if not pending:
                return
            if pending['result'] is not None:
                return
            approval = db.execute('SELECT payload FROM approvals WHERE token=?', (token,)).fetchone()
            if approval:
                payload = json.loads(approval['payload'])
            else:
                payload = json.loads(pending['payload'])
                evidence = {'proposed': payload.get('evidence'),
                            'confirmed_by': event.principal,
                            'input_frame_id': event.input_frame_id,
                            'confirmation_reference': token}
                for field in ('channel', 'identity', 'interaction_id', 'session_id',
                              'cursor', 'conversation_id', 'handling_mode'):
                    if hasattr(event, field):
                        evidence[field] = getattr(event, field)
                payload['evidence'] = evidence
                db.execute('INSERT INTO approvals VALUES(?,?)', (token, json.dumps(payload)))
        # Persist first admission before apply, preserving crash/retry provenance.
        result = self.store.apply(payload)
        with closing(self.store.connect()) as db, db:
            db.execute('UPDATE pending SET result=? WHERE token=?', (json.dumps(result), token))


def build(context):
    return Integration(context)


def rehearse(context, fixtures):
    from .rehearsal import RehearsalIntegration
    app = RehearsalIntegration(context, fixtures.get('synthetic_admissions', []))
    books = fixtures.get('books', [])
    if not isinstance(books, list) or len(books) > 100:
        raise ValueError('books must be a finite list of at most 100 fixtures')
    for index, item in enumerate(books):
        app.store.apply(dict(item, operation='add', request_id='fixture-' + str(index)))
    return app
