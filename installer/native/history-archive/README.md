# Forge historical archive

This leaf crate retains bounded private JSON files. It has no Core, Meta,
Connect, native caller, receipt, process, provider or collection types. Its
filesystem controls can run independently of the native integration lane.

`save(state_root, instance_id, data, max_bytes)` requires an existing canonical
absolute `0700` directory. It creates a `0700` `forge-ghe-history-v1` subdirectory
owned by the same UID and refuses symlink or permission mismatches. The final
filename is the lowercase SHA-256 of the UTF-8 instance ID plus `.json`.
That digest is a locator only; the consumer must compare the original full ID.

Writing is bounded to at most 256 MiB including the final newline. The writer
uses an exclusive `.json.pending` file, syncs its data and `0400` permissions,
then creates the final name with a hard link that cannot overwrite an existing
name. It syncs the directory before and after removing the pending name.
Existing final/pending files refuse a replacement write. Failed partial bytes
remain pending; no automatic deletion, retry, rotation or credential write occurs.

The caller must keep the original selected root protected against hostile
same-UID access and path replacement for the complete operation. Permission and
symlink checks here do not create that isolation or authenticate the root.
Call this outside native synchronous callbacks. A saved file establishes only
file retention, never current source coverage, native close or ingestion success.

The five focused filesystem controls run on the current Mac using temporary
directories. They cover exact bytes/digest/permissions, duplicate preservation,
pending-file behavior at a byte limit, invalid selection limits, and symlink/
insecure-directory refusal. They are not protected Linux runtime acceptance.
