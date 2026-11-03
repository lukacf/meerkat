# Meta 0.9.3 remote-continuity host captures

Inputs for `meta_gateway storage-migrate-native --continuity-export`: the
state of a host that kept its sessions in a continuity provider, exported with
the standalone export scripts (`tools/continuity-export`).

Each capture comes from the published `rpc_gateway` 0.9.3 binary
(`meta-rpc-gateway-0.9.3-x86_64-unknown-linux-gnu.tar.gz`, sha256
`b416df1ba5648ca3ed849dec2692f664c37b0e7277232a6d2421ac53961a18b0`, the
checksum the v0.9.3 release lists; binary sha256 `757519ed`). It ran in raw RPC
mode with `persistent_state`, a file-backed continuity store, a lease provider
and a roster provider, against a loopback model fake; no provider keys were
involved. Boot 1 creates and turns the identities; boot 2 resets `hr-reset`
(twice), deletes `hr-deleted` (0.9.3 retires the member, then deletes the
snapshot and the record) and drops `hr-unrostered` from the roster. The export
lists six identities: four ready and two uninitialized (`hr-deleted`, and
`hr-never`, which never existed). `hr-big` carries a 3.9 MB session document.
Both exports were written by the pinned Python script and verified by both
scripts.

| File | What it is |
| --- | --- |
| `memory-export.jsonl.gz` | the export of a host with in-memory mob storage |
| `memory-state.tar.gz` | that host's `runtime.sqlite` from boot 2's new volume: it lacks `hr-big` and `hr-unrostered` |
| `durable-export.jsonl.gz` | the export of a host with durable mob storage |
| `durable-state.tar.gz` | that host's `runtime.sqlite` and its mob store (`event_log.sqlite3`, its `-wal` and composition record), one directory across both boots |

The archives are deterministic (`tar --sort=name --owner=0 --group=0
--numeric-owner --mtime='2026-10-19 00:00:00Z'`, `gzip -n -9`) and carry the
binary's files byte for byte.
