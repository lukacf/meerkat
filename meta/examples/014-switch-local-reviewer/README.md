# Switch reviewers in one local mob

At a shift change, the editor should have the night reviewer as a peer instead
of the day reviewer. This Node.js program creates three turn-driven agents
and starts no model turns. The example host may create agents and change
peers; peer targets must have the demo worker profile.

These instructions were tested on Linux x86-64 GNU only. On that platform,
use Node 22 and a local `rpc_gateway` matching the SDK version in `package.json`.
Run from this directory:

```sh
npm install --ignore-scripts
META_GATEWAY_BIN=/absolute/path/to/rpc_gateway npm start
```

The host reads the current spec digest of the editor, then submits one identity
update naming the peer to remove and the peer to add. It checks the saved peer
declaration separately. It prints the three `getMember().wiredTo` lists as JSON
under `before` and `after`. The table summarizes the expected values of those
roster reads:

| Member | Peers before | Peers after |
| --- | --- | --- |
| editor | day-review | night-review |
| day-review | editor | none |
| night-review | none | editor |

These are recorded peer relationships, not proof of message delivery or of
work transferring to the new reviewer. On Linux x86-64 GNU, the demo puts
Meta state and session identity files under its own temporary directory and
removes that directory on exit. Do not use these steps as an isolated demo
on macOS or Windows: setting `XDG_STATE_HOME` there does not isolate session
identity files. No provider key, remote host, or existing mob is needed.
