# 036 - Live Audio (Python)

Talk to a Meerkat live OpenAI mob member from the command line. The app
streams microphone audio into a `gpt-realtime-2` channel, plays assistant
audio back through your speakers, and prints transcript, tool, and mob activity
as the conversation progresses.

## What This Shows

- OpenAI live audio transport through the Meerkat Python SDK
- `LiveChannel.session(...)` lifecycle wrapper
- Inline Meerkat mob skills
- Python callback tools invoked from a live turn
- Helper sub-agents spawned through a Meerkat mob

## Setup

Install the SDK and the audio dependency:

```bash
python3 -m pip install -e ../../sdks/python
python3 -m pip install -r requirements.txt
```

When running from a source checkout, build the local RPC binary from the
repository root and pass it with `--rkat-path` or `MEERKAT_BIN_PATH`:

```bash
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
```

Linux hosts may also need PortAudio:

```bash
sudo apt-get install portaudio19-dev
```

Then run:

```bash
OPENAI_API_KEY=sk-... python3 main.py
```

The example asks `rkat-rpc` to start its live WebSocket host, creates an
isolated realm by default, creates a mob, spawns a live `voice-host` member,
and opens an audio channel to that member.

## Try It

Say:

- "Remember that the release name is Northstar."
- "Delegate a second opinion on whether we should ship today."
- "What notes have you saved?"

The first prompt should trigger the `voice_session_note` callback tool. The
second should trigger `delegate_to_mob`, which spawns a helper member in the
same mob and prints the helper's final output when it is available.

## Options

```bash
python3 main.py --help
python3 main.py --text-probe
python3 main.py --input-device 1 --output-device 2
python3 main.py --helper-model gpt-5.5
python3 main.py --realm live-demo
```

`--text-probe` keeps the live WebSocket path but sends one text chunk
instead of opening local audio devices. It waits for a tool completion
or turn completion event, so it is useful for checking runtime plumbing on
machines without a microphone.

## Troubleshooting

If startup reports that live audio input is unavailable, `rkat-rpc` started
without an OpenAI live sideband factory. Check that `OPENAI_API_KEY` or your
OpenAI auth binding is available to the runtime.

If audio devices fail to open, run `python3 -m sounddevice` to list device names
and pass `--input-device` or `--output-device`.
