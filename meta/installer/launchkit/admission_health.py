"""Validate pending-admission observations without changing native state."""
from __future__ import annotations

import re
from datetime import datetime, timezone


# K1/K2 producers must emit pending frames more often than this window.
PENDING_HEALTH_WINDOW = 30.0


def health_is_fresh(value, observed_at=None):
    """A manager poll cannot renew a native observation's timestamp."""
    try:
        checked = datetime.fromisoformat(value)
        age = ((observed_at or datetime.now(timezone.utc)) - checked).total_seconds()
        return checked.tzinfo is not None and 0 <= age <= PENDING_HEALTH_WINDOW
    except (TypeError, ValueError):
        return False


def fresh_pending_admission(receipt, observed_at=None):
    if not health_is_fresh(receipt.get("health_checked_at"), observed_at):
        return None
    return retained_pending_admission(receipt)


def retained_pending_admission(receipt):
    """Read a valid observation bound to the current startup attempt.

    Callers still establish owned source, current process state and freshness.
    A service manager observation cannot refresh this native observation.
    """
    attempt, observed = receipt.get("startup_attempt_id"), receipt.get("admission_health")
    if (not isinstance(attempt, str)
            or not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", attempt)
            or not isinstance(observed, dict) or observed.get("attempt_id") != attempt):
        return None
    try:
        return pending_admission(receipt.get("bootstrap"),
            {key: value for key, value in observed.items() if key != "attempt_id"})
    except ValueError:
        return None


def pending_admission(bootstrap, admission, *, seeds=()):
    """Validate a private host observation; return pending rows or None.

    These rows describe the app's admission projection. They grant no access
    and cannot turn a native failure into success. Only Meta's typed restore
    refusal identifies held completion owed; diagnostic text is never parsed.
    """
    def refuse():
        raise ValueError("The application returned inconsistent admission health.")

    if not isinstance(admission, dict) or set(admission) != {"admitting", "owed"}:
        refuse()
    rows = {}
    for key in ("admitting", "owed"):
        values = admission[key]
        if (not isinstance(values, list)
                or any(not isinstance(item, str) or not item or item.strip() != item
                       or any(ord(char) < 32 for char in item) for item in values)
                or len(set(values)) != len(values)):
            refuse()
        rows[key] = list(values)
    if (set(rows["admitting"]) & (set(rows["owed"]) | set(seeds))
            or not isinstance(bootstrap, dict)
            or type(bootstrap.get("complete")) is not bool
            or type(bootstrap.get("ready")) is not bool):
        refuse()
    counts, identities = bootstrap.get("counts"), bootstrap.get("identities")
    if (not isinstance(counts, dict) or type(counts.get("broken")) is not int
            or counts["broken"] < 0 or not isinstance(identities, dict)):
        refuse()
    broken, dormant = set(), set()
    for identity, entry in identities.items():
        if (not isinstance(entry, dict) or not isinstance(entry.get("state"), str)
                or entry["state"] not in {"active", "dormant", "warming", "broken"}):
            refuse()
        if entry["state"] == "dormant":
            dormant.add(identity)
        if entry["state"] != "broken":
            continue
        restore = entry.get("restore")
        if (not isinstance(restore, dict) or restore.get("stage") != "refused"
                or type(restore.get("code")) is not int or restore["code"] != -32603
                or restore.get("kind") != "held_completion_owed"):
            refuse()
        broken.add(identity)
    owed = set(rows["owed"])
    # Lazy bootstrap can leave a listed owed identity dormant. Keep it owed
    # and pending even when that unchanged native snapshot reports ready.
    if (not broken <= owed or not owed - broken <= dormant or len(broken) != counts["broken"]
            or broken and bootstrap["ready"]):
        refuse()
    return rows if rows["admitting"] or rows["owed"] else None


class NativeAdmissionPending(RuntimeError):
    """Nonterminal private-pipe observation, distinct from a startup failure."""

    def __init__(self, bootstrap, admission):
        super().__init__("Some agents are waiting for admission to finish.")
        self.bootstrap = bootstrap
        self.admission = admission
