# Round 5 Traceability

Maps dogma rows to owning lane, milestone, and gate status.

| Row | Milestone | Branch | Contract / Surface Delta | Static Gate | Build Gate | Final Gate | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `DOGMA-08` | `A1a` | `codex/dogma-A-runtime-authority` | protocol-owned peer projection seam | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-09` | `A1a` | `codex/dogma-A-runtime-authority` | ops wait/terminate semantics no longer shell-owned | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-10` | `B1` | `codex/dogma-B-public-surfaces` | late-cancel parity | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-11` | `B1` | `codex/dogma-B-public-surfaces` | session-control rename across public surfaces | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-12` | `C1` | `codex/dogma-C-comms-trust` | single-owner ingress trust | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-13` | `A1b` | `codex/dogma-A-runtime-authority` | runtime-backed turn-state authority unification | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-14` | `EG1b-a` | `codex/dogma-EG-session-schedule` | runtime-backed recovery fallback correctness | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-15` | `D1-1` | `codex/dogma-D1-auth-identity` | binding-scoped auth identity | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-16` | `D2-2` | `codex/dogma-D2-auth-policy` | external-authorizer materialization | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-17` | `D1-1` | `codex/dogma-D1-auth-identity` | WASM external-auth key correctness | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-18` | `EG2` | `codex/dogma-EG-session-schedule` | default skill-source identity through registry | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-19` | `C1` | `codex/dogma-C-comms-trust` | trust publication rollback semantics | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-20` | `EG1a` | `codex/dogma-EG-session-schedule` | websocket rotation rebuilds product session | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-21` | `B1` | `codex/dogma-B-public-surfaces` | public mob MCP uses `member_ref` only | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-23` | `EG1b-b` | `codex/dogma-EG-session-schedule` | schedule keeps typed runtime terminal meaning | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-24` | `D2-2` | `codex/dogma-D2-auth-policy` | policy fields affect resolved runtime behavior | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-25` | `D2-1` | `codex/dogma-D2-auth-policy` | effective capability truth drives skill filtering | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-26` | `B2` | `codex/dogma-B-public-surfaces` | realtime status semantics match RPC and WS | `approved` | `approved` | `approved` | final gate green |
| `DOGMA-27` | `B1` | `codex/dogma-B-public-surfaces` | `connection_ref` plumbed through session creation | `approved` | `approved` | `approved` | final gate green |
