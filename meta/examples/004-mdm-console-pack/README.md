# 004 - MDM Console Pack

This pack re-imagines the old TUX MDM flow as a Meta console over one real
mob roster. There is no kennel service and no target HTTP registry in this
design. Meta owns the roster, the hive is a local member, and every target is
declared as a remote `backend: external` mob member with a real Meerkat comms
address and Ed25519 public key.

The important test is whether hive-to-target traffic is actual peer/comms
traffic. A roster label saying `platform=linux-gcp-vm` is not a target answer.

## Target Bindings

Run or provision target agents separately, then pass their bindings to the
console:

```json
[
  {
    "id": "target-a",
    "name": "this-mac",
    "site": "local",
    "platform": "darwin-local",
    "address": "tcp://127.0.0.1:5791",
    "public_key": "ed25519:...",
    "bootstrap_token": "..."
  }
]
```

The same value can be supplied as `binding` in the canonical Meta/Meerkat
wire shape:

```json
{
  "kind": "external",
  "address": "tcp://127.0.0.1:5791",
  "identity": {
    "kind": "ed25519_public_key",
    "public_key": "ed25519:..."
  },
  "bootstrap_token": "..."
}
```

See `target-bindings.example.json` for the file shape. The example keys are not
live credentials.

## Run

For a local target on this machine:

```bash
cd examples
npm install
./004-mdm-console-pack/scripts/local-target.sh start --id target-a --name this-mac
./004-mdm-console-pack/scripts/start-console.sh
```

The local target helper starts the real `mdm_mob_target` Rust example, waits
until it writes a binding, and upserts that binding into
`004-mdm-console-pack/.state/target-bindings.json`.

For a GCP target:

```bash
cd examples/004-mdm-console-pack
cp deploy/gcp.env.example .env.gcp
$EDITOR .env.gcp
./scripts/gcp-target.sh start --id target-b --name gcp-target-b
./scripts/start-console.sh
```

`gcp-target.sh` creates the VM if needed, syncs the repo, forwards provider
credential env vars such as `OPENAI_API_KEY`, starts the same real target
runtime on the VM, fetches its binding, and merges it into the console target
file.

Important: target `--listen HOST:PORT` is the bind address. Target
`--advertise tcp://HOST:PORT` is the address the console host dials. The GCP
helper defaults to binding `0.0.0.0:5791` and advertising the VM internal IP,
which is right for a VPN/VPC-reachable console host. Pass `--advertise` if your
network path is different.

For cross-host targets, the console's Meerkat supervisor bridge must also be
reachable from the targets:

```bash
MDM_SUPERVISOR_BIND_ADDRESS=0.0.0.0:5790 \
MDM_SUPERVISOR_ADVERTISED_ADDRESS=tcp://<console-reachable-host>:5790 \
MDM_MEMBER_COMMS_ADDRESS=<console-reachable-ip>:0 \
./004-mdm-console-pack/scripts/start-console.sh
```

`MDM_MEMBER_COMMS_ADDRESS` must name a concrete local interface, not
`0.0.0.0`; each Hive/member runtime binds an ephemeral port on that interface
and advertises the resolved TCP endpoint back to external targets.

For a local-only demo, the default supervisor bridge is
`tcp://127.0.0.1:5790`.

To query hardware with a pre-existing binding file, configure provider
credentials for Hive and the target runtimes, and keep the required target-side
tools enabled. The default Hive model uses `OPENAI_API_KEY`:

```bash
cd examples
npm install
export OPENAI_API_KEY=...
./004-mdm-console-pack/scripts/start-console.sh --targets ./004-mdm-console-pack/target-bindings.json --real-llm
```

Open the printed `/console` URL. The roster should show `Hive` plus the remote
targets from the binding file. Asking the hive to query hardware should produce
peer messages to the target members. Local metadata alone is not proof of a
remote query; check provider selection, target readiness, tool access, and peer
connectivity before diagnosing a bridge regression.

The separate `examples.sh --run` launcher always forces `--demo-llm`, even
when credentials or `--real-llm` are supplied. It is a demo/shape-only console,
not a provider-backed Hive-to-target query proof.

The success signal is not "target is listed." The success signal is a target
turn that runs on the target host, uses target-side tools or shell where
appropriate, and returns over the Meta/Meerkat peer path.

## Smoke

```bash
cd examples
npm run mdm:smoke
npm run mdm:browser-smoke
npm run mdm:real-target-smoke
npm run mdm:real-target-e2e
npm run mdm:real-target-multi-e2e
npm run mdm:local-target
npm run mdm:console
```

The empty-target smokes verify that the console boots without the old kennel
path. `mdm:real-target-smoke` starts a disposable real `mdm_mob_target`, binds it
as an external mob member, sends through the same console RPC used by the UI,
and checks that the target process observed the peer turn. This deterministic
lane does not require a provider response. `mdm:real-target-e2e` additionally
requires configured provider credentials, sends the operator turn to the local
hive, waits for the hive to query the wired target through peer comms, and
fails unless target-side model/shell execution returns through the hive's
tracked console terminal. This lane uses the hive interaction as the
terminal-owning MDM path, rather than treating a direct external-member
ingress acknowledgement as proof of target execution.
`mdm:real-target-multi-e2e` raises the same gate with two independent target
processes and requires Hive's verified terminal summary to name both targets.

The pack uses the repository's pinned Meerkat dependency family in
`crates/meerkat-meta/Cargo.toml` (currently `=0.8.40`), not an independent pack pin.
Validate the checked-in dependencies without changing them:

```bash
cd examples
npm run mdm:real-target-smoke
```

`mdm:upgrade-meerkat` is an intentional shared dependency-update helper: it
rewrites the Rust crate's direct Meerkat normal/dev dependencies and updates
their lockfile entries after a registry precheck. It is not a normal example
setup step.

Full operator validation requires at least one local or remote target runtime
with unrestricted shell tools enabled on the target side. The useful prompt is
something like: "Ask every target what machine it is running on." The answer
should come from target-side peer turns, not from roster labels.

Historical acceptance: `mdm:real-target-smoke` and the credential-gated
`mdm:real-target-e2e` passed on the published Meerkat 0.8.2 line. That result
does not establish acceptance of the current dependency pin; rerun the lanes
above when validating a new version. Investigate failures rather than replacing
the proof with labels, demo model text, or static binding metadata.
