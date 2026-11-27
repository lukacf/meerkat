# Reset one idle agent without resetting its sibling

Use this when a single agent should start a new conversation, not when it
should recover its old one. This local example creates its own temporary
stock Meta gateway state, declares two turn-driven roster members, and sends
neither any work. It retires `draft` before resetting it. A busy retirement
is a reason to stop, not to force a reset. In a real application another
sender can wake the member between those calls; this demo has no such sender.
A reset is destructive: it does not move queued or running work to the new
session. If the result is ambiguous, inspect status instead of repeating it.

With Node 22 and the matching released Meta `rpc_gateway` on Linux x86-64:

```sh
npm ci --ignore-scripts
META_GATEWAY_BIN=/absolute/path/to/rpc_gateway npm start
```

No provider key is needed. Compare the two snapshots in roster order:
`draft` stays active but gets a different session and a later generation;
`index` stays active on the same session and generation. The program checks
all three fields, shuts down the gateway and removes its private state. It
does not verify remote devices, delivery, queued-work handling or recovery
after a gateway restart.
