OLD (CHANGELOG.md, 0.9.15, Known limitations, the session limit item; exact bytes between the markers, as on main ac4e7319b4):
<<<OLD
gateway; a Core host's default is 100,000. Keep it at or above the number
  of agent sessions (members, helpers and scheduled runs included) that can
  be awake on the host at once, not only those with a turn running. Affects
>>>
NEW:
<<<NEW
gateway; a Core host's default is 100,000. (Corrected 2026-11-17: the
  limit counts work in flight, not members: a session being created, a
  first turn waiting to start and a turn that runs or is admitted each hold
  a slot, and an idle member holds none; see the item added under 0.9.16
  on that date. Raise it if more work than the limit can be in flight at
  one moment.) Affects
>>>
