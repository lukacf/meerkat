"""Application-owned durable fixture. Never shipped as a Forge default store."""
import json
import sqlite3


class Integration:
    def __init__(self, context):
        self.context = context
        self.store = sqlite3.connect(context.root / "fixture-admissions.sqlite")
        self.store.execute("PRAGMA synchronous=FULL")
        self.store.execute("""CREATE TABLE IF NOT EXISTS admissions (
            id TEXT PRIMARY KEY, identity TEXT UNIQUE NOT NULL, class TEXT NOT NULL,
            phase TEXT NOT NULL CHECK (phase IN ('admitting', 'listed', 'void')),
            row TEXT NOT NULL, bindings TEXT, spec TEXT)""")
        self.store.commit()
        self.publish_calls = 0

    def admission_roster(self):
        # One read captures both partitions; a row never falls between reads.
        rows = self.store.execute("SELECT phase, row FROM admissions WHERE phase != 'void' ORDER BY id").fetchall()
        seeds = [{"identity": row["identity"], "classId": row["classId"]}
                 for row in self.context.deployment["agents"]]
        return {"listed": seeds + [json.loads(row) for phase, row in rows if phase == "listed"],
                "admitting": [json.loads(row) for phase, row in rows if phase == "admitting"]}

    def desired_roster(self):
        return self.admission_roster()["listed"]

    def desired_edges(self, identities):
        return []

    def begin_identity(self, admission_id, row):
        # Durable correlation exists before any native request. UNIQUE keeps
        # a duplicate caller from replacing another operation's reservation.
        with self.store:
            self.store.execute("INSERT INTO admissions VALUES (?, ?, ?, 'admitting', ?, NULL, NULL)",
                (admission_id, row["identity"], row["classId"], json.dumps(row, sort_keys=True)))

    def _row(self, selection):
        row = self.store.execute("SELECT identity, class, phase, bindings, spec FROM admissions WHERE id=?",
                                 (selection["admission_id"],)).fetchone()
        if row is None or row[:2] != (selection["identity"], selection["class"]) or row[2] == "void":
            raise ValueError("Original application admission is unavailable")
        return row

    def bind_spec(self, selection, spec):
        encoded = json.dumps(spec, sort_keys=True)
        with self.store:
            row = self._row(selection)
            if row[2] != "admitting" or row[4] not in (None, encoded):
                raise ValueError("Original admission spec changed")
            self.store.execute("UPDATE admissions SET spec=? WHERE id=?", (encoded, selection["admission_id"]))

    def retain_member_mcp(self, value):
        selection, bindings = value["selection"], json.dumps(value["bindings"], separators=(",", ":"))
        with self.store:
            row = self._row(selection)
            if row[2] != "admitting" or row[3] not in (None, bindings):
                raise ValueError("Original admitting row or retained binding set changed")
            self.store.execute("UPDATE admissions SET bindings=? WHERE id=?", (bindings, selection["admission_id"]))

    def lookup_member_mcp(self, selection):
        row = self._row(selection)
        if row[3] is None:
            raise ValueError("No confirmed retained bindings")
        return {"selection": selection, "bindings": json.loads(row[3])}

    async def publish(self, selection, runtime):
        if runtime is not self.context.runtime:
            raise ValueError("Original application runtime changed")
        with self.store:
            row = self._row(selection)
            expected = next((value.to_dict() for value in self.context._application._snapshot.buildable_specs
                             if value.identity == selection["identity"]), None)
            if (row[2] != "admitting" or row[3] is None or row[4] is None
                    or expected != json.loads(row[4])):
                raise ValueError("Original retained admitting row is unavailable")
            self.store.execute("UPDATE admissions SET phase='listed' WHERE id=?", (selection["admission_id"],))
        self.publish_calls += 1
        # Held completion must publish its app view without requesting another
        # native reconcile or taking the reconcile lock held by the Add owner.
        await self.context._application._prepare_snapshot(runtime=runtime)

    async def void(self, selection):
        # The test caller invokes this only after native and listing join.
        with self.store:
            self._row(selection)
            self.store.execute("UPDATE admissions SET phase='void' WHERE id=?", (selection["admission_id"],))
        await self.context._application._prepare_snapshot(runtime=self.context.runtime)

    def phase(self, admission_id):
        return self.store.execute("SELECT phase FROM admissions WHERE id=?", (admission_id,)).fetchone()[0]

    def tools_for_agent(self, agent):
        if agent["classId"] != "resident":
            return []
        identity = agent["identity"]
        async def read(arguments):
            row = self.store.execute("SELECT phase, row FROM admissions WHERE identity=?", (identity,)).fetchone()
            if row is None or row[0] != "listed":
                raise ValueError("Resident is not currently listed")
            return {"identity": identity, "context": json.loads(row[1]).get("context")}
        return [{"name": "read_resident", "description": "Read the actual owner's record.",
                 "schema": {"type": "object", "properties": {}}, "handler": read}]

    def stop(self):
        self.store.close()
