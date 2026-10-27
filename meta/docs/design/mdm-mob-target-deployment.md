# MDM Mob Target Deployment

This note describes the deployment model for the Meta MDM console example in
`examples/004-mdm-console-pack`.

## Roles

- Console host: runs Meta, the hive member, the browser console, persistent
  Meta state, and the externally reachable supervisor bridge.
- Target runtime: runs `mdm_mob_target` on each managed machine. It owns local
  credentials, sessions, shell/tool execution, comms identity, and state.
- Meerkat/Meta mob: owns peer wiring between hive and targets. Remote target
  members are ordinary external mob members, not proxy records.

The console host can ask targets to do work, but targets perform work locally.
The console host must never synthesize target-machine answers from labels,
bindings, or deployment metadata.

## Network Shape

Each target exposes a signed Meerkat comms TCP endpoint:

```text
target bind:       0.0.0.0:5791 or 10.x.y.z:5791
target advertised: tcp://<console-reachable-target>:5791
```

The console host exposes the Meerkat mob supervisor bridge:

```text
console bind:       0.0.0.0:5790 or 127.0.0.1:5790
console advertised: tcp://<target-reachable-console>:5790
```

For single-machine demos, both sides can use loopback. For GCP/VPC demos, the
advertised addresses must be routable between the console host and the target
VMs, and firewall rules must allow the chosen ports.

## Local Development

From `examples/`:

```bash
npm install
./004-mdm-console-pack/scripts/local-target.sh start --id target-a --name this-mac
./004-mdm-console-pack/scripts/start-console.sh
```

The local helper builds and starts `mdm_mob_target`, waits for a binding file,
and merges it into `.state/target-bindings.json`.

## GCP Target

From `examples/004-mdm-console-pack`:

```bash
cp deploy/gcp.env.example .env.gcp
$EDITOR .env.gcp
./scripts/gcp-target.sh start --id target-b --name gcp-target-b
```

The helper creates or reuses the VM, syncs this checkout, forwards selected
provider credential environment variables, builds `mdm_mob_target` on the VM,
starts it, fetches the binding, and merges the binding into the console target
file.

Then start the console with a supervisor bridge address reachable from the VM:

```bash
MDM_SUPERVISOR_BIND_ADDRESS=0.0.0.0:5790 \
MDM_SUPERVISOR_ADVERTISED_ADDRESS=tcp://<console-reachable-host>:5790 \
./scripts/start-console.sh
```

## Current Integration Gate

The deployment helpers now create real target runtimes and real Meta external
member bindings. The pack uses the workspace's checked-in exact Meerkat pins
(0.8.40 at the audited baseline), not an independent pack dependency selection.
Historically, Meerkat 0.6.30 supplied the typed bridge reply and production
reply-route fixes needed for peer-only external targets. Validating this
checkout does not require changing dependency versions. From `examples/`, run:

```bash
npm run mdm:real-target-smoke
```

Run this from the repository root to reproduce the current gate:

```bash
npm --prefix examples run mdm:real-target-smoke
```

The script is the acceptance check for local target bind plus delivery of a
queued mob turn to the target. It fails if the target process never logs an
observed peer turn. The GCP helper uses the same target binary and binding path,
so it exercises the same mechanics across hosts once the console supervisor
advertised address is routable.

## Production Hardening

This example is suitable for validating the real remote mob path. Before using
it as production MDM, add:

- authenticated/TLS transport for target and supervisor ports;
- restricted firewall rules between console and target subnets;
- systemd/launchd installers for `mdm_mob_target`;
- durable state backup for target identities and session state;
- audit-log retention outside the demo `.state/` directory.
