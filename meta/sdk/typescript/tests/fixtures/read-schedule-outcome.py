#!/usr/bin/env python3
"""Read a native schedule's first occurrence and receipts without writing."""
import json
from pathlib import Path
import sqlite3
import sys

database, runnable = sys.argv[1:3]
minimum_ordinal = int(sys.argv[3]) if len(sys.argv) > 3 else 0
with sqlite3.connect(Path(database).resolve().as_uri() + "?mode=ro", uri=True) as connection:
    # One read transaction keeps the schedule, occurrence and receipts from
    # different driver commits from being mixed during the live observation.
    connection.execute("BEGIN")
    schedules = [json.loads(row[0]) for row in connection.execute(
        "SELECT schedule_json FROM schedule_schedules"
    )]
    selected = [schedule for schedule in schedules if schedule["target"].get("runnable") == runnable]
    if not selected:
        print("null")
    else:
        if len(selected) != 1:
            raise ValueError("expected exactly one schedule for the test runnable")
        schedule = selected[0]
        rows = connection.execute(
            "SELECT phase, action_at_ms, occurrence_json FROM schedule_occurrences "
            "WHERE schedule_id = ? AND occurrence_ordinal >= ? ORDER BY occurrence_ordinal",
            (schedule["schedule_id"], minimum_ordinal),
        ).fetchall()
        if not rows:
            print("null")
        else:
            phase, action_at_ms, encoded = rows[0]
            occurrence = json.loads(encoded)
            receipts = [json.loads(row[0]) for row in connection.execute(
                "SELECT receipt_json FROM schedule_receipts WHERE occurrence_id = ? "
                "ORDER BY recorded_at_ms, receipt_id",
                (occurrence["occurrence_id"],),
            )]
            print(json.dumps({
                "schedule": schedule,
                "phase": phase,
                "action_at_ms": action_at_ms,
                "occurrence": occurrence,
                "receipts": receipts,
                "future_pending": sum(row[0] == "pending" for row in rows[1:]),
            }))
