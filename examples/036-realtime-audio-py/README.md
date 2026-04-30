# 036 - Realtime Audio (Python)

Talk to a Meerkat realtime OpenAI mob member from the command line. The app
streams microphone audio into a `gpt-realtime-1.5` channel, plays assistant
audio back through your speakers, and prints transcript, tool, and mob activity
as the conversation progresses.

## What This Shows

- OpenAI realtime transport through the Meerkat Python SDK
- `RealtimeChannel.mob_member(...)` identity-first routing
- Inline Meerkat mob skills
- Python callback tools invoked from a realtime turn
- Helper sub-agents spawned through a Meerkat mob

## Setup

Install the SDK and the audio dependency:

```bash
python3 -m pip install -e ../../sdks/python
python3 -m pip install -r requirements.txt
```

Linux hosts may also need PortAudio:

```bash
sudo apt-get install portaudio19-dev
```

Then run:

```bash
OPENAI_API_KEY=sk-... python3 main.py
```

The example asks `rkat-rpc` to start its realtime WebSocket host, creates an
isolated realm by default, creates a mob, spawns a realtime `voice-host` member,
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
python3 main.py --helper-model gpt-5.2
python3 main.py --realm realtime-demo
```

`--text-probe` keeps the realtime WebSocket path but sends one text chunk
instead of opening local audio devices. It waits for a realtime tool completion
or turn completion event, so it is useful for checking runtime plumbing on
machines without a microphone.

## Troubleshooting

If startup reports that realtime audio input is unavailable, `rkat-rpc` started
without an OpenAI realtime sideband factory. Check that `OPENAI_API_KEY` or your
OpenAI connection binding is available to the runtime.

If audio devices fail to open, run `python3 -m sounddevice` to list device names
and pass `--input-device` or `--output-device`.
