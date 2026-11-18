# Python: a conversation between agents on two Meta gateways

An editor asks an archivist where a ticket belongs. The two members live in different gateway processes, one member per gateway. The application wires them once with `wire_cross_mob`, gives the ticket only to the editor, and observes the result. The archivist knows a separate random shelf code; both members must report both values. A send receipt alone does not pass the check. The host does not forward messages.

Use Python 3.10 or later and the `meerkat-meta` package and `rpc_gateway` executable from the same Meta release. This example was written for and run on 0.9.16. Install the package and extract the matching gateway for your platform.

**Offline check:** From this directory, run `python -m unittest test_example` with the SDK installed. This checks the missing-gateway diagnostic and endpoint validation without a gateway or provider key. It does not test peer delivery.

**Live run:** Set `META_GATEWAY_BIN` to the absolute path of the extracted executable and supply `ANTHROPIC_API_KEY` in your environment. Run `python main.py`. A missing reply fails the check; both runtimes are shut down.

The two `member_comms_tcp('127.0.0.1:0')` settings let the runtime allocate separate local ports for signed peer traffic. The example uses disposable state and does not promise that these temporary endpoints survive a restart. `send_cross_mob` would inject a host message into the destination; it is not a substitute for this agent-to-agent exchange.
