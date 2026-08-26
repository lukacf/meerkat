import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';

import { chromium } from 'playwright';

const here = path.dirname(fileURLToPath(import.meta.url));
const fixtureRoot = path.resolve(here, '..', 'fixtures', 'gpt_live_client');
let browser;
let page;

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
  };
  const offerSdp = await page.evaluate(async ({ fixtures }) => {
    const audioContext = new AudioContext({ sampleRate: 24_000 });
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
                if (rms >= 0.002) remoteAudio.decodedNonSilentFrames += audioData.numberOfFrames;
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
      fixtures,
      peer,
      remoteAudio,
    };
    channel.onmessage = async (event) => {
      const state = globalThis.__gptLivePeer;
      state.eventTransport.rawMessages += 1;
      try {
        const text = typeof event.data === 'string'
          ? event.data
          : event.data instanceof Blob
            ? await event.data.text()
            : new TextDecoder().decode(event.data);
        state.events.push(JSON.parse(text));
      } catch {
        state.eventTransport.parseFailures += 1;
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
  }, { fixtures });
  return { offer_sdp: offerSdp };
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
  await page.evaluate(async (fixtureName) => {
    const state = globalThis.__gptLivePeer;
    const response = await fetch(state.fixtures[fixtureName]);
    const buffer = await state.audioContext.decodeAudioData(await response.arrayBuffer());
    const source = state.audioContext.createBufferSource();
    source.buffer = buffer;
    source.connect(state.destination);
    const ended = new Promise((resolve) => { source.onended = resolve; });
    source.start();
    await ended;
  }, name);
  return { played: name };
}

async function snapshot() {
  return page.evaluate(async () => {
    const state = globalThis.__gptLivePeer;
    const inboundAudio = {
      bytes_received: 0,
      packets_received: 0,
      total_audio_energy: 0,
      total_samples_received: 0,
    };
    for (const report of (await state.peer.getStats()).values()) {
      if (report.type !== 'inbound-rtp' || (report.kind !== 'audio' && report.mediaType !== 'audio')) {
        continue;
      }
      inboundAudio.bytes_received += Number(report.bytesReceived || 0);
      inboundAudio.packets_received += Number(report.packetsReceived || 0);
      inboundAudio.total_audio_energy += Number(report.totalAudioEnergy || 0);
      inboundAudio.total_samples_received += Number(report.totalSamplesReceived || 0);
    }
    return {
      audio: {
        decoded_frames: state.remoteAudio.decodedFrames,
        decoded_non_silent_frames: state.remoteAudio.decodedNonSilentFrames,
        max_decoded_rms: state.remoteAudio.maxDecodedRms,
        processor_errors: state.remoteAudio.processorErrors,
        processor_supported: state.remoteAudio.processorSupported,
        max_rms: state.remoteAudio.maxRms,
        non_silent_frames: state.remoteAudio.nonSilentFrames,
        sampled_frames: state.remoteAudio.sampledFrames,
        ...inboundAudio,
      },
      event_transport: state.eventTransport,
      events: state.events,
    };
  });
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
