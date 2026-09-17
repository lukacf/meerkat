import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';

import { chromium } from 'playwright';

const here = path.dirname(fileURLToPath(import.meta.url));
const fixtureRoot = path.resolve(here, '..', 'fixtures', 'gpt_live_client');
// Provider protocol observed on the `oai-events` data channel:
// - `experimental`: deprecated private ChatGPT-brokered protocol (`turn.*`).
// - `public`: public OpenAI Live API (`session.*`). The browser only applies
//   the answer SDP; it never sends `session.start` on the data channel.
const PROTOCOLS = new Set(['experimental', 'public']);
const protocol = parseProtocol(process.argv.slice(2));
let browser;
let page;

function parseProtocol(args) {
  const index = args.indexOf('--protocol');
  const value = index === -1 ? 'experimental' : args[index + 1];
  if (!PROTOCOLS.has(value)) {
    throw new Error(`unsupported --protocol ${JSON.stringify(value)}; expected experimental or public`);
  }
  return value;
}

function audioDataUrl(name) {
  return `data:audio/wav;base64,${fs.readFileSync(path.join(fixtureRoot, name)).toString('base64')}`;
}

async function prepare() {
  browser = await chromium.launch({
    headless: true,
    args: ['--autoplay-policy=no-user-gesture-required', '--use-fake-ui-for-media-stream'],
  });
  page = await browser.newPage();
  await page.goto('data:text/html,<title>Meerkat GPT Live E2E peer</title>');
  const fixtures = {
    greeting: audioDataUrl('no-delegation-greeting.wav'),
    delegation: audioDataUrl('delegate-working-directory.wav'),
    remember: audioDataUrl('remember-code-word.wav'),
    recall: audioDataUrl('recall-code-word.wav'),
    // Synthetic speech (macOS say, Samantha, 155 wpm; PCM16 mono 24 kHz).
    // history: "What was my historical vault phrase from the earlier text
    // conversation? If that history is not available yet, say I don't know
    // yet. Don't guess and don't ask a delegate."
    history: audioDataUrl('historical-vault-query.wav'),
    // correction: "Correction: the current code word is Cobalt, replacing
    // every older code word. Please acknowledge Cobalt briefly. Do not delegate."
    correction: audioDataUrl('correct-code-word.wav'),
    // current: "What are the current code word and my current favorite flower,
    // according to the newest updates? Say both briefly, without delegating."
    current: audioDataUrl('current-context-query.wav'),
  };
  const offerSdp = await page.evaluate(async ({ fixtures, protocol }) => {
    const audioContext = new AudioContext({ sampleRate: 24_000 });
    const fixtureBuffers = {};
    for (const [name, fixture] of Object.entries(fixtures)) {
      const response = await fetch(fixture);
      const buffer = await audioContext.decodeAudioData(await response.arrayBuffer());
      const samples = buffer.getChannelData(0);
      const nonSilentSamples = samples.reduce((count, sample) => count + (Math.abs(sample) >= 0.002 ? 1 : 0), 0);
      if (buffer.duration < 0.1 || nonSilentSamples < buffer.sampleRate * 0.1) {
        throw new Error(`speech fixture ${name} is empty, silent, or too short`);
      }
      fixtureBuffers[name] = buffer;
    }
    const destination = audioContext.createMediaStreamDestination();
    const oscillator = audioContext.createOscillator();
    const gain = audioContext.createGain();
    gain.gain.value = 0;
    oscillator.connect(gain).connect(destination);
    oscillator.start();
    await audioContext.resume();
    const peer = new RTCPeerConnection();
    peer.addTrack(destination.stream.getAudioTracks()[0], destination.stream);
    const remoteAudio = {
      decodedFrames: 0,
      decodedNonSilentFrames: 0,
      decodedNonSilentSeconds: 0,
      maxDecodedRms: 0,
      processorErrors: 0,
      processorSupported: typeof MediaStreamTrackProcessor === 'function',
      sampledFrames: 0,
      nonSilentFrames: 0,
      maxRms: 0,
      sources: [],
    };
    peer.ontrack = (event) => {
      if (event.track.kind !== 'audio') return;
      const stream = new MediaStream([event.track]);
      const playback = document.createElement('audio');
      playback.autoplay = true;
      playback.srcObject = event.streams[0] || stream;
      document.body.append(playback);
      playback.play().catch(() => {});
      const source = audioContext.createMediaStreamSource(stream);
      const analyser = audioContext.createAnalyser();
      analyser.fftSize = 2048;
      source.connect(analyser);
      analyser.connect(audioContext.destination);
      const samples = new Float32Array(analyser.fftSize);
      const timer = setInterval(() => {
        analyser.getFloatTimeDomainData(samples);
        let squareSum = 0;
        for (const sample of samples) squareSum += sample * sample;
        const rms = Math.sqrt(squareSum / samples.length);
        remoteAudio.sampledFrames += 1;
        remoteAudio.maxRms = Math.max(remoteAudio.maxRms, rms);
        if (rms >= 0.002) remoteAudio.nonSilentFrames += 1;
      }, 50);
      event.track.addEventListener('ended', () => clearInterval(timer), { once: true });
      remoteAudio.sources.push({ analyser, playback, source, stream, timer });
      if (remoteAudio.processorSupported) {
        const processor = new MediaStreamTrackProcessor({ track: event.track });
        const reader = processor.readable.getReader();
        const task = (async () => {
          try {
            while (true) {
              const { value: audioData, done } = await reader.read();
              if (done) break;
              try {
                const samples = new Float32Array(audioData.numberOfFrames);
                audioData.copyTo(samples, { planeIndex: 0 });
                let squareSum = 0;
                for (const sample of samples) squareSum += sample * sample;
                const rms = Math.sqrt(squareSum / Math.max(samples.length, 1));
                remoteAudio.decodedFrames += audioData.numberOfFrames;
                remoteAudio.maxDecodedRms = Math.max(remoteAudio.maxDecodedRms, rms);
                if (rms >= 0.002) {
                  remoteAudio.decodedNonSilentFrames += audioData.numberOfFrames;
                  remoteAudio.decodedNonSilentSeconds += audioData.numberOfFrames / audioData.sampleRate;
                }
              } finally {
                audioData.close();
              }
            }
          } catch {
            remoteAudio.processorErrors += 1;
          }
        })();
        remoteAudio.sources.push({ processor, reader, task });
      }
    };
    const channel = peer.createDataChannel('oai-events', { ordered: true });
    globalThis.__gptLivePeer = {
      audioContext,
      channel,
      destination,
      events: [],
      eventTransport: { rawMessages: 0, parseFailures: 0 },
      fixtureBuffers,
      // Keep an active zero-PCM source between WAVs. Ending the last source
      // can stall outbound RTP and prevent the provider's context ACK.
      continuousInput: { oscillator, gain, track: destination.stream.getAudioTracks()[0] },
      bargeIn: { armedFixture: null, failures: 0, starts: [] },
      peer,
      remoteAudio,
    };
    globalThis.__gptLivePeer.startFixture = (fixtureName, waitForEnd) => {
      const state = globalThis.__gptLivePeer;
      const buffer = state.fixtureBuffers[fixtureName];
      if (!buffer) throw new Error(`unknown audio fixture: ${fixtureName}`);
      const source = state.audioContext.createBufferSource();
      source.buffer = buffer;
      source.connect(state.destination);
      const ended = new Promise((resolve) => { source.onended = resolve; });
      source.start();
      return waitForEnd ? ended : Promise.resolve();
    };
    channel.onmessage = async (event) => {
      const state = globalThis.__gptLivePeer;
      state.eventTransport.rawMessages += 1;
      let parsed;
      try {
        const text = typeof event.data === 'string'
          ? event.data
          : event.data instanceof Blob
            ? await event.data.text()
            : new TextDecoder().decode(event.data);
        parsed = JSON.parse(text);
      } catch {
        state.eventTransport.parseFailures += 1;
        return;
      }
      state.events.push(parsed);
      // Assistant-start boundary used to fire an armed barge-in. The private
      // protocol announces assistant turns; the public Live API has no turn
      // identifiers, so the first assistant output delta is the boundary.
      const assistantStarted = protocol === 'public'
        ? (parsed?.type === 'session.output_transcript.delta'
          || parsed?.type === 'session.output_audio.delta')
        : (parsed?.type === 'turn.created' && parsed?.turn?.role === 'assistant');
      if (assistantStarted && state.bargeIn.armedFixture) {
        const fixtureName = state.bargeIn.armedFixture;
        state.bargeIn.armedFixture = null;
        try {
          await state.startFixture(fixtureName, false);
          state.bargeIn.starts.push({
            assistant_turn_id: protocol === 'public' ? null : parsed.turn.id,
            assistant_event_type: parsed.type,
            event_count_at_start: state.events.length,
          });
        } catch {
          state.bargeIn.failures += 1;
        }
      }
    };
    const offer = await peer.createOffer();
    await peer.setLocalDescription(offer);
    if (peer.iceGatheringState !== 'complete') {
      await Promise.race([
        new Promise((resolve) => peer.addEventListener('icegatheringstatechange', () => {
          if (peer.iceGatheringState === 'complete') resolve();
        })),
        new Promise((resolve) => setTimeout(resolve, 3000)),
      ]);
    }
    return peer.localDescription?.sdp;
  }, { fixtures, protocol });
  return { offer_sdp: offerSdp, protocol };
}

async function answer(sdp) {
  await page.evaluate(async (answerSdp) => {
    await globalThis.__gptLivePeer.peer.setRemoteDescription({ type: 'answer', sdp: answerSdp });
  }, sdp);
  await page.waitForFunction(() => globalThis.__gptLivePeer.channel.readyState === 'open', null, {
    timeout: 60_000,
  });
  return { ready: true };
}

async function play(name) {
  const input = await page.evaluate(
    async (fixtureName) => {
      const state = globalThis.__gptLivePeer;
      if (state.peer.connectionState !== 'connected') throw new Error('speech requires connected WebRTC');
      const sentBytes = async () => {
        let bytes = 0;
        for (const report of (await state.peer.getStats()).values()) {
          if (report.type === 'outbound-rtp' && (report.kind === 'audio' || report.mediaType === 'audio')) {
            bytes += Number(report.bytesSent || 0);
          }
        }
        return bytes;
      };
      const before = await sentBytes();
      await state.startFixture(fixtureName, true);
      const bytesSent = (await sentBytes()) - before;
      if (bytesSent <= 0) throw new Error('speech fixture produced no outbound WebRTC audio');
      const buffer = state.fixtureBuffers[fixtureName];
      return { duration_seconds: buffer.duration, samples: buffer.length, bytes_sent: bytesSent };
    },
    name,
  );
  return { played: name, input };
}

async function armBargeIn(name) {
  await page.evaluate((fixtureName) => {
    const state = globalThis.__gptLivePeer;
    if (!state.fixtureBuffers[fixtureName]) throw new Error(`unknown audio fixture: ${fixtureName}`);
    if (state.bargeIn.armedFixture) throw new Error('barge-in fixture is already armed');
    state.bargeIn.armedFixture = fixtureName;
  }, name);
  return { armed: name };
}

async function snapshot() {
  return page.evaluate(async () => {
    const state = globalThis.__gptLivePeer;
    const inboundAudio = {
      bytes_received: 0,
      packets_received: 0,
      total_audio_energy: null,
      total_samples_received: null,
      total_samples_duration: null,
    };
    const outboundAudio = { bytes_sent: 0, packets_sent: 0 };
    for (const report of (await state.peer.getStats()).values()) {
      if (report.type === 'outbound-rtp' && (report.kind === 'audio' || report.mediaType === 'audio')) {
        outboundAudio.bytes_sent += Number(report.bytesSent || 0);
        outboundAudio.packets_sent += Number(report.packetsSent || 0);
      }
      if (report.type !== 'inbound-rtp' || (report.kind !== 'audio' && report.mediaType !== 'audio')) {
        continue;
      }
      inboundAudio.bytes_received += Number(report.bytesReceived || 0);
      inboundAudio.packets_received += Number(report.packetsReceived || 0);
      for (const [key, value] of [
        ['total_audio_energy', report.totalAudioEnergy],
        ['total_samples_received', report.totalSamplesReceived],
        ['total_samples_duration', report.totalSamplesDuration],
      ]) {
        if (typeof value === 'number' && Number.isFinite(value)) {
          inboundAudio[key] = (inboundAudio[key] ?? 0) + value;
        }
      }
    }
    return {
      audio: {
        decoded_frames: state.remoteAudio.decodedFrames,
        decoded_non_silent_frames: state.remoteAudio.decodedNonSilentFrames,
        decoded_non_silent_seconds: state.remoteAudio.decodedNonSilentSeconds,
        max_decoded_rms: state.remoteAudio.maxDecodedRms,
        processor_errors: state.remoteAudio.processorErrors,
        processor_supported: state.remoteAudio.processorSupported,
        max_rms: state.remoteAudio.maxRms,
        non_silent_frames: state.remoteAudio.nonSilentFrames,
        sampled_frames: state.remoteAudio.sampledFrames,
        ...inboundAudio,
      },
      event_transport: state.eventTransport,
      connection: {
        state: state.peer.connectionState,
        data_channel: state.channel.readyState,
        audio_context: state.audioContext.state,
        input_track: state.continuousInput.track.readyState,
        ...outboundAudio,
      },
      events: state.events,
      barge_in: state.bargeIn,
    };
  }).then((snapshot) => ({ ...snapshot, protocol }));
}

async function close() {
  await browser?.close();
  browser = undefined;
  page = undefined;
  return { closed: true };
}

async function handle(command) {
  switch (command.type) {
    case 'prepare': return prepare();
    case 'answer': return answer(command.answer_sdp);
    case 'arm_barge_in': return armBargeIn(command.name);
    case 'play': return play(command.name);
    case 'snapshot': return snapshot();
    case 'close': return close();
    default: throw new Error(`unsupported peer command: ${command.type}`);
  }
}

const lines = readline.createInterface({ input: process.stdin });
for await (const line of lines) {
  let command;
  try {
    command = JSON.parse(line);
    const result = await handle(command);
    process.stdout.write(`${JSON.stringify({ id: command.id, result })}\n`);
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      id: command?.id,
      error: String(error?.message || error).slice(0, 1000),
    })}\n`);
  }
}
await close();
