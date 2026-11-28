# Switch reviewers in one local mob

At a shift change, the editor should have the night reviewer as a peer instead
of the day reviewer. This Node.js program creates three turn-driven agents
and starts no model turns. The example host may create agents and change
peers; peer targets must have the demo worker profile.

With Node 22 and a local rpc_gateway matching the SDK version in package.json,
run from this directory:

```sh
npm install --ignore-scripts
META_GATEWAY_BIN=/absolute/path/to/rpc_gateway npm start
```

The host reads the current spec digest of the editor, then submits one identity
update naming the peer to remove and the peer to add. It checks the saved peer
declaration and the roster field `wiredTo` for all three agents. Expected output:

| Member | Peers before | Peers after |
| --- | --- | --- |
| editor | day-review | night-review |
| day-review | editor | none |
| night-review | none | editor |

These are recorded peer relationships, not proof of message delivery or of
work transferring to the new reviewer. The program creates and removes its
own temporary state. No provider key, remote host, or existing mob is needed.
Tested on Linux x86-64 GNU.
