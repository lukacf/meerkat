# Meta 0.9.3 gateway state directories

Inputs for the native-layout startup check
(`storage_migrate_native::native_layout_status`) and the
`meta_gateway storage-migrate-native` tests (#306). Each tarball holds the
databases one state directory needs for the migration:
`runtime.sqlite`, the session projection (`sessions.sqlite3`, or
`continuity.sqlite3` with its `-wal`), and the mob store
(`event_log.sqlite3` with its `-wal` and composition record) when the
layout has one.

Produced by the published `rpc_gateway` 0.9.3 binary
(`meta-rpc-gateway-0.9.3-x86_64-unknown-linux-gnu.tar.gz`, sha256
`b416df1ba5648ca3ed849dec2692f664c37b0e7277232a6d2421ac53961a18b0`, the
checksum the v0.9.3 release lists) against a loopback model fake; no
provider keys were involved. Every boot shut down cleanly unless the name
says otherwise. No row was edited: the mixed shapes combine files written at
different boots.

Each run had three boots: boot 1 creates `alpha` and `beta` with three turns
each (`alpha` auto-compacts once), boot 2 runs three turns for `alpha` only,
and boot 3 creates `gamma`.

| Tarball | Layout | What it is |
| --- | --- | --- |
| `s1b-plain` | no roster provider | boot 3 as the binary left it |
| `s1b-projection-behind` | no roster provider | boot 2 with `sessions.sqlite3` from boot 1 |
| `s1b-runtime-missing` | no roster provider | boot 3 with `runtime.sqlite` from boot 2 |
| `s1-roster` | roster provider | boot 3 as the binary left it |
| `s1-projection-behind` | roster provider | boot 2 with `continuity.sqlite3` from boot 1 |
| `s1-runtime-missing` | roster provider | boot 3 with `runtime.sqlite` from boot 2 |
| `s1e-memory-mob` | roster provider, in-memory mob storage | boot 3 as the binary left it |
| `s1-compact-member-broken-killed` | roster provider | `meta/compact_member` failed with a stale checkpoint version and marked the identity broken; the gateway was then killed |

The tarballs are deterministic (`tar --sort=name --owner=0 --group=0
--numeric-owner --mtime='2026-10-19 00:00:00Z'`, `gzip -n -9`) and carry the
binary's files byte for byte.
