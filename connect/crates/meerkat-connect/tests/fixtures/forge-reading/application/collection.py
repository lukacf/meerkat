"""Local guarded event ledger; native agents own conversation and recommendations."""
from contextlib import closing
import json
import os
import secrets
import sqlite3
import uuid
from pathlib import Path

REVISION_POLICY = {
    'scope': 'Per-book revisions only; no global collection revision exists.',
    'new_add': 'For add with title and author absent from books, expected_revision and book_id are irrelevant: use null (or omit expected_revision/book_id). Empty IDs and a positive integer revision placeholder are also accepted for provider compatibility, not treated as existing identity or revision evidence. Creation starts at revision 1 only after human confirmation.',
    'duplicate_add': 'Normalized title+author add returns unchanged canonical ID/revision/history. Revision is irrelevant for this no-op; use null or omit it. A supplied nonempty book_id must agree with supplied title/author.',
    'existing_update': 'finished/reaction/correct require the exact positive integer expected_revision from the target book snapshot; null/missing/stale revisions fail. No placeholder revision is accepted for existing mutations.',
    'presentation': 'A successful awaiting-human-confirmation proposal is valid. Librarian must return its entire proposal and exact confirmation to lead; lead must include the exact confirmation in FINAL VISIBLE PROSE. Nothing is saved until authenticated human confirmation and an actual receipt.'
}

def norm(value):
    return ' '.join(value.split()).casefold()

class Store:
    def __init__(self, path):
        self.path = Path(path)

    def connect(self):
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        db = sqlite3.connect(self.path, timeout=10)
        try:
            os.chmod(self.path, 0o600)
            db.row_factory = sqlite3.Row
            db.executescript('''
        CREATE TABLE IF NOT EXISTS books(id TEXT PRIMARY KEY,title TEXT,author TEXT,nt TEXT,na TEXT,status TEXT,revision INTEGER, UNIQUE(nt,na));
        CREATE TABLE IF NOT EXISTS events(id TEXT PRIMARY KEY,book_id TEXT,request_id TEXT,payload TEXT);
        CREATE TABLE IF NOT EXISTS requests(id TEXT PRIMARY KEY,payload TEXT,result TEXT);
        CREATE TABLE IF NOT EXISTS pending(token TEXT PRIMARY KEY,payload TEXT,result TEXT);
        CREATE TRIGGER IF NOT EXISTS no_event_update BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT,'append only'); END;
        CREATE TRIGGER IF NOT EXISTS no_event_delete BEFORE DELETE ON events BEGIN SELECT RAISE(ABORT,'append only'); END;
        ''')
        except BaseException:
            # Initialization failed before the caller could own the connection.
            try:
                db.close()
            except BaseException:
                pass  # Preserve the original initialization failure.
            raise
        return db

    def snapshot(self):
        with closing(self.connect()) as db, db:
            books = []
            for row in db.execute('SELECT * FROM books ORDER BY rowid'):
                b = dict(row)
                b.pop('nt'); b.pop('na')
                b['events'] = [dict(id=e['id'], **json.loads(e['payload'])) for e in db.execute('SELECT * FROM events WHERE book_id=? ORDER BY rowid', (b['id'],))]
                superseded = {e.get('supersedes') for e in b['events']}
                b['reactions'] = [e['new'] for e in b['events'] if e['field']=='reaction' and e['id'] not in superseded]
                books.append(b)
            return {'books': books, 'source': 'local SQLite event ledger', 'revision_policy': dict(REVISION_POLICY)}

    def apply(self, p):
        encoded = json.dumps(p, sort_keys=True)
        with closing(self.connect()) as db, db:
            db.execute('BEGIN IMMEDIATE')
            prior = db.execute('SELECT * FROM requests WHERE id=?', (p['request_id'],)).fetchone()
            if prior:
                if prior['payload'] != encoded: return {'status':'request-conflict'}
                return json.loads(prior['result'])
            op = p['operation']
            if op not in ('add','finished','reaction','correct'):
                return {'status':'refused', 'reason':'No delete or implicit update operations'}
            row = None
            title, author = p.get('title') or '', p.get('author') or ''
            if not isinstance(title,str) or not isinstance(author,str): return {'status':'refused'}
            if p.get('book_id'):
                row = db.execute('SELECT * FROM books WHERE id=?', (p['book_id'],)).fetchone()
                if not row: return {'status':'not-found'}
                if (title.strip() and norm(title)!=row['nt']) or (author.strip() and norm(author)!=row['na']):
                    return {'status':'identity-conflict'}
            else:
                if not title.strip() or not author.strip():
                    return {'status':'needs-clarification', 'question':'Supply title and author; title alone is not an identity.'}
                row = db.execute('SELECT * FROM books WHERE nt=? AND na=?', (norm(title),norm(author))).fetchone()
            if op == 'add' and row:
                result = {'status':'unchanged','book_id':row['id'],'revision':row['revision']}
            else:
                if op != 'add' and not row: return {'status':'not-found'}
                revision_arg = p.get('expected_revision')
                if row and (type(revision_arg) is not int or revision_arg != row['revision']):
                    return {'status':'revision-conflict','revision':row['revision']}
                book_id = row['id'] if row else str(uuid.uuid4())
                revision = row['revision']+1 if row else 1
                field, old, new, supersedes = 'status', row['status'] if row else None, 'wishlist', None
                if op == 'finished': new = 'finished'
                if op == 'reaction':
                    field, old, new = 'reaction', None, p.get('value')
                    if not isinstance(new,str) or not new.strip(): return {'status':'invalid-reaction'}
                if op == 'correct':
                    target = db.execute('SELECT * FROM events WHERE id=? AND book_id=?',(p.get('event_id'),book_id)).fetchone()
                    if not target: return {'status':'not-found'}
                    if any(json.loads(e[0]).get('supersedes') == target['id'] for e in db.execute('SELECT payload FROM events WHERE book_id=?',(book_id,))):
                        return {'status':'already-superseded'}
                    data = json.loads(target['payload'])
                    field, old, new, supersedes = data['field'], data['new'], p.get('value'), target['id']
                    if field == 'status' and new not in ('wishlist','finished'): return {'status':'invalid-status'}
                    if field == 'reaction' and (not isinstance(new,str) or not new.strip()): return {'status':'invalid-reaction'}
                if not row:
                    db.execute('INSERT INTO books VALUES(?,?,?,?,?,?,?)',(book_id,title,author,norm(title),norm(author),new,revision))
                else:
                    db.execute('UPDATE books SET revision=?,status=? WHERE id=?',(revision,new if field=='status' else row['status'],book_id))
                event_id = str(uuid.uuid4())
                event = dict(field=field,old=old,new=new,supersedes=supersedes,operation=op,evidence=p.get('evidence'),request_id=p['request_id'])
                db.execute('INSERT INTO events VALUES(?,?,?,?)',(event_id,book_id,p['request_id'],json.dumps(event)))
                result = dict(status='saved',book_id=book_id,event_id=event_id,revision=revision)
            db.execute('INSERT INTO requests VALUES(?,?,?)',(p['request_id'],encoded,json.dumps(result)))
            return result

class Integration:
    def __init__(self, context, trial=False):
        self.context, self.trial = context, trial
        self.store = Store(Path(context.root)/'data'/('trial-collection.sqlite' if trial else 'collection.sqlite'))

    def snapshot(self, args): return self.store.snapshot()

    def propose(self, args):
        allowed = {'operation','request_id','title','author','book_id','expected_revision','event_id','value','evidence'}
        if set(args)-allowed or args.get('operation') not in ('add','finished','reaction','correct') or not isinstance(args.get('request_id'),str) or not args['request_id']:
            return {'status':'refused'}
        for key in ('title','author','book_id','event_id','value'):
            if args.get(key) is not None and not isinstance(args[key],str): return {'status':'refused'}
        revision = args.get('expected_revision')
        if revision is not None and (type(revision) is not int or revision < 1): return {'status':'refused'}
        if args['operation']=='add':
            if args.get('event_id') or args.get('value') not in (None,'','wishlist'):
                return {'status':'refused','reason':'add is wishlist only; no event target or other value'}
        # Identity conflicts are checked again transactionally at apply.
        if args.get('book_id'):
            book = next((b for b in self.store.snapshot()['books'] if b['id']==args['book_id']), None)
            if not book: return {'status':'not-found'}
            if any(args.get(k) and norm(args[k])!=norm(book[k]) for k in ('title','author')):
                return {'status':'identity-conflict'}
        token = secrets.token_hex(12)
        with closing(self.store.connect()) as db, db:
            db.execute('INSERT INTO pending VALUES(?,?,NULL)',(token,json.dumps(args)))
        confirmation = 'confirm '+token
        return {'status':'awaiting-human-confirmation','proposal':args,'confirmation':confirmation,
                'instructions':'This is a valid staged proposal, not a write receipt. Librarian: return proposal and exact confirmation to lead. Lead: show all values/evidence and include '+confirmation+' in FINAL VISIBLE PROSE so the reader can copy it into authenticated Console. Do not hide it in tools or peer messages. Nothing saved yet.'}

    def receipt(self, args):
        with closing(self.store.connect()) as db, db:
            row = db.execute('SELECT result FROM pending WHERE token=?',(args.get('token'),)).fetchone()
        return {'status':'not-confirmed'} if not row or not row[0] else json.loads(row[0])

    def tools_for_agent(self, agent):
        identity = agent['identity']
        if identity not in ('lead:main','librarian:main'): return []
        tools = [dict(name='collection_snapshot',handler=self.snapshot,description='Authoritative local books, per-book revisions, retained evidence and revision_policy. No global revision exists. Empty books permits new add with nullable irrelevant fields; no revision exists yet. Read before recommendations. For valid staged proposals, lead must show exact confirmation in FINAL VISIBLE PROSE.',schema={'type':'object','properties':{},'additionalProperties':False}),
                 dict(name='collection_receipt',handler=self.receipt,description='Read actual persistence receipt after human confirmation; token is the part after confirm. Before confirmation nothing saved; lead must display exact staged confirmation in final visible prose.',schema={'type':'object','properties':{'token':{'type':'string'}},'required':['token'],'additionalProperties':False})]
        if identity == 'librarian:main':
            properties = {k:{'type':['string','null']} for k in ('title','author','book_id','event_id','value')}
            properties.update({k:{'type':'string'} for k in ('request_id','evidence')})
            properties['operation']={'type':'string','enum':['add','finished','reaction','correct'], 'description':'add creates wishlist or returns canonical duplicate unchanged; finished/reaction/correct mutate existing target with real revision.'}
            properties['title']['description']='Supplied title required with author for add without ID. Null allowed when existing book_id identifies target.'
            properties['author']['description']='Supplied author with title for add without ID; never invent. Null allowed for existing ID.'
            properties['book_id']['description']='Use null, empty string or omission for new add; never invent an ID. Nonempty ID must exist and agree with supplied title/author.'
            properties['event_id']['description']='correct requires retained event ID of target book. For add use null or empty string; no event exists yet.'
            properties['value']['description']='add accepts null, empty string or wishlist only. finished sets finished regardless of irrelevant value. reaction requires exact nonempty reaction; correct requires replacement status or reaction.'
            properties['expected_revision']={'type':['integer','null'],'minimum':1, 'description':'For add no revision exists on new creation: use null or omit expected_revision; positive integer placeholder also accepted and ignored for add only. Duplicate add also ignores revision. For finished/reaction/correct REQUIRED exact positive integer target revision from snapshot; null/missing/stale fails. No global revision.'}
            tools.append(dict(name='collection_propose',handler=self.propose,description='Propose explicitly requested changes only. NEW add supplies title/author; book_id/event_id may be null or empty, value null/empty/wishlist, expected_revision null/omitted or positive integer irrelevant placeholder. No revision exists before creation; no field omission is required. Duplicate add is unchanged canonical identity/history. Nonempty IDs must agree with identity. finished/reaction/correct require real target expected_revision; missing/null/stale fails at apply. correct requires retained event_id. Read snapshot. awaiting-human-confirmation is a VALID staged proposal: return entire proposal and EXACT confirmation to lead, who must include it in FINAL VISIBLE PROSE for reader to copy. Authenticated human confirmation required; proposing never saves.',schema={'type':'object','properties':properties,'required':['operation','request_id','evidence'],'additionalProperties':False}))
        return tools

    def stop(self): pass
