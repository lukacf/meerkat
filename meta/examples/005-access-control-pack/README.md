# Access Control Pack

Demonstrates the optional ABAC (attribute-based access control) layer against
the **stock** Meta console — no custom UI. With access control enabled, every
console caller's experience is filtered by their authenticated identity and
group membership: agents they can't view vanish from the sidebar/roster/
topology, per-agent affordances are gated, event streams are filtered, and
lifecycle/admin RPCs are denied.

The server runs the deterministic `TestClient`, so the pack needs **no API
key** and replies are stable.

## Source layout

- `server.rs` — runnable console server, exposed as Cargo example `access_control_console`
- `tokens.mjs` — shared HS256 dev-token minter + persona list
- `persona-proxy.mjs` — per-persona auth proxies (one local port per identity)
- `smoke.mjs` — deterministic HTTP smoke asserting the seeded scenario
- `examples.sh` — build + run (smoke by default, `--serve` for browser exploration)

## The seeded scenario (`access.toml`)

Three agents are spawned: `ops-lead` (`org=platform`), `scout-1`
(`org=payments`), `scout-2` (`org=people`). The seeded rules:

| Persona | Sees | Can send to | Admin |
|---------|------|-------------|-------|
| `root@example.test` | every agent | every agent | yes (gets the **Access** panel) |
| `alice@example.test` (group `ops`) | every agent | `ops-lead` only | no |
| `bob@example.test` | `org=payments` agents (`scout-1`) | `scout-1` | no |
| anonymous | nothing | nothing | no |

## What it proves

- per-principal experience filtering on the stock `/console/experience`
- per-agent send affordances (`alice` views `scout-1` but its composer is blocked)
- label-selector rules (`bob` is scoped to `org=payments`)
- deny-by-default (anonymous lands in an empty console)
- RPC enforcement: denied sends return JSON-RPC `-32030` (`access_denied`)
- SSE gating: `agent.view` per `/agents/{id}/events`, `mob.observe` for `/mob/events`,
  and `mob.observe` streams still filtered per agent
- the admin-only **Access** panel and live reconfiguration (edits apply on the
  next request and persist back to `access.toml`)

## Run

Complete the [shared prerequisites](../README.md#prerequisites), including
the console build dependencies. This pack's HTTP-only smoke needs neither
Playwright Chromium nor provider credentials.

Deterministic smoke (offline, picks a free port):

```bash
cd examples
./005-access-control-pack/examples.sh
```

Browse it as each persona:

```bash
./005-access-control-pack/examples.sh --serve
```

This starts the console on `http://127.0.0.1:7300` and a proxy per persona:

| Persona | URL |
|---------|-----|
| anonymous | http://127.0.0.1:7301/console |
| alice (ops) | http://127.0.0.1:7302/console |
| bob (payments) | http://127.0.0.1:7303/console |
| root (admin) | http://127.0.0.1:7304/console |

Open each in its own tab and compare sidebars, composers, and (on root) the
Access panel. Editing rules as root takes effect on the other tabs on their
next poll.

> The pack runs the open console (`require_app_auth = false`) and mints HS256
> tokens against a `.localhost` dev issuer purely so each persona can present
> an identity locally. This is a demo convenience, **not** a production auth
> mechanism — real deployments enable `require_app_auth` with a real OIDC
> provider and the email allowlist.
