# Predecessor adoption: steward schedule fixtures

Sanitized carried schedule stores from a production predecessor
installation, as its host exported them (packet sha256
1530a2dd01f94556ff534569dd6790bcf61ed076c66db155a42fdb5e6b2d625b):

- `text_row_schedule.sqlite`: the production DDL and `meerkat_schema` row
  with three `schedule_schedules` rows: one non-steward schedule whose
  `schedule_json` is stored as TEXT, a deleted steward schedule, and a live
  steward schedule. Natural-language strings are redacted. The TEXT row's
  target mob id was then set to the neutral `example` by regenerating the
  store through SQLite (an UPDATE of that one cell, then VACUUM with
  secure_delete on), never by editing bytes. Every other cell, storage
  class, schema statement and header pragma was verified unchanged; the
  regenerated file's sha256 is
  f1543eb96490fb4eedc22129259ed8501cc43e56f036c2cfd228a132abff709f.
- `deleted_steward_schedule.sqlite`: the same DDL with only the deleted and
  the live steward schedules, unredacted.
- `text_row_synthetic_schedule.sqlite` (sha256
  a6933c2a67d2232ec14e37188d1f3deb2fe324e9b766ee43919c25ae0e3f7e44): the
  TEXT row with a self-consistent synthetic target, plus the live steward
  schedule as a BLOB. The redacted TEXT row above is a refusal control;
  this one is the TEXT positive control.

`storage_predecessor` tests copy them into a temporary state directory and
adopt them; they are never opened in place.
