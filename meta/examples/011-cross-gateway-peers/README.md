# Python: a conversation between agents on two Meta gateways

An editor asks an archivist where a ticket belongs. The two members live in different gateway processes, one member per gateway. The application wires them once with `wire_cross_mob`, gives the ticket only to the editor, and observes the result. The archivist knows a separate random shelf code; both members must report both values. A send receipt alone does not pass the check. The host does not forward messages.

Use Python 3.11+, `meerkat-meta==0.9.16`, the matching `rpc_gateway` from the `meta-v0.9.16` release, and an `ANTHROPIC_API_KEY` in your environment. Set `META_GATEWAY_BIN` to the absolute path of the extracted binary, then run `python main.py` from this directory. This calls a live provider; a missing reply fails the check and both runtimes are shut down.

The two `member_comms_tcp('127.0.0.1:0')` settings let the runtime allocate separate local ports for signed peer traffic. The example uses disposable state and does not promise that these temporary endpoints survive a restart. `send_cross_mob` would inject a host message into the destination; it is not a substitute for this agent-to-agent exchange.
