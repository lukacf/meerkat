# 040 — Let an agent schedule one follow-up (Python)

Start one session. Its first task is to call `meerkat_schedule_create` for itself. The Python host waits for the matching tool result and stored schedule, then reads the occurrence and the target session history. It never sends the second prompt. Keep the process running: the local `rkat-rpc` owns delivery.

Use Python 3.10+ and matching releases of `meerkat-sdk` and `rkat-rpc`. For the tested Linux x86_64 setup, install `meerkat-sdk==0.9.16` in a virtual environment and extract `rkat-rpc` from its checksum-verified v0.9.16 archive. Set `RKAT_RPC` to its absolute path and provide `ANTHROPIC_API_KEY` in the environment. From this directory run:

```sh
python main.py
```

A keep-alive session needs a comms name. Session creation returns when the session is built; the agent may not have called the tool yet. The example gives the agent a future UTC time. On the scheduled turn it must join two random fragments and reply with the result. The completed occurrence attests delivery only. The separate output check requires a committed assistant message after the matching delivered prompt in the same session; an initial answer, prompt echo, or receipt alone is insufficient.

The temporary realm is removed when the client closes. No production schedule, recurrence, offline catch-up or restart is involved. A timeout means the example could not establish the expected evidence within its bounds, not that the scheduler necessarily failed.
