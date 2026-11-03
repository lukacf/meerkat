# MobKit MDM Console Implementation Plan

This plan tracks the independent MobKit-based MDM implementation in
`examples/004-mdm-console-pack`. The pack does not depend on
`(operator-retained path)/meerkat/examples/035-mdm-tux-rs`; it uses that work only as
background for the MDM scenario.

## Architecture

- MobKit is the console host and mob runtime.
- Hive is a normal local mob member.
- Targets are normal remote `backend = "external"` mob members with Meerkat
  comms addresses, Ed25519 public keys, bootstrap tokens, target-side sessions,
  and unrestricted target-side shell/tools.
- There is no kennel, target registry, HTTP turn proxy, generated contact
  directory, or metadata-based answer path.

The success signal is an operator prompt that causes hive to send real peer
traffic to the target members, target members to run on their own hosts, and
target answers to return through Meerkat/MobKit comms. Roster labels are only
presentation hints.

## Completed Work

- Added `examples/004-mdm-console-pack` as a self-contained MDM console pack.
- Added `mdm_mob_target`, a Rust remote target runtime that:
  - starts a signed TCP Meerkat comms listener;
  - creates or resumes a target-side session;
  - enables builtins, comms, mob tools, and unrestricted shell;
  - writes a canonical binding file for the console host.
- Added a MobKit TypeScript runner that:
  - loads target bindings from JSON or environment;
  - projects hive and targets through identity-first roster/topology providers;
  - configures Meerkat's external supervisor bridge endpoint for remote targets;
  - serves the stock MobKit console.
- Added local and GCP helpers for repeatable target provisioning:
  - `scripts/local-target.sh`
  - `scripts/gcp-target.sh`
  - `scripts/start-console.sh`
  - `scripts/merge-bindings.mjs`
- Added local smoke and browser smoke commands under `examples/package.json`.

## Runtime Configuration

The console runner writes `.state/mob.generated.toml` from `config/mob.toml`
and appends:

```toml
[backend.external.supervisor_bridge]
bind_address = "127.0.0.1:5790"
advertised_address = "tcp://127.0.0.1:5790"
```

Override the bridge for cross-host runs with:

```bash
MDM_SUPERVISOR_BIND_ADDRESS=0.0.0.0:5790
MDM_SUPERVISOR_ADVERTISED_ADDRESS=tcp://<console-reachable-host>:5790
```

The advertised address must be reachable from every target. The pack is pinned
to the Meerkat 0.6.30 family, which includes the bridge reply decode and
production reply-route fixes needed by real external members.

Version check and smoke:

```bash
cd examples
npm run mdm:upgrade-meerkat -- 0.6.30
npm run mdm:real-target-smoke
```

`mdm:real-target-smoke` starts a real `mdm_mob_target`, binds it as an external
member, and sends a queued mob turn to the target member. Passing means the old
fake path is gone for local remote-target delivery.

## Verification

Local verification:

```bash
npm --prefix examples run mdm:smoke
npm --prefix examples run mdm:browser-smoke
npm --prefix examples run mdm:real-target-smoke
npm --prefix examples run mdm:local-target
npm --prefix examples run mdm:console
```

Cross-host verification:

```bash
cd examples/004-mdm-console-pack
cp deploy/gcp.env.example .env.gcp
$EDITOR .env.gcp
./scripts/gcp-target.sh start --id target-b --name gcp-target-b
MDM_SUPERVISOR_BIND_ADDRESS=0.0.0.0:5790 \
MDM_SUPERVISOR_ADVERTISED_ADDRESS=tcp://<console-reachable-host>:5790 \
./scripts/start-console.sh
```

Ask hive to query target hardware or OS state. Passing means the target answers
from its own host via peer traffic, not from labels or static bindings.

Current published-crate status: `mdm:smoke` and `mdm:real-target-smoke` pass on
Meerkat 0.6.30. The real-target smoke now also checks that the target process
observed a peer turn, so a supervisor-side accepted-send is not enough.
