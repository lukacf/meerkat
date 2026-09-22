import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';

import { chromium } from 'playwright';

const here = path.dirname(fileURLToPath(import.meta.url));
const fixtureRoot = path.resolve(here, '..', 'fixtures', 'gpt_live_client');
// Fixture names, files, and spoken scripts are declared in the manifest
// (verified offline by `voice_fixtures verify` and the integration crate's
// unit test); the harness never hardcodes a WAV name.
const manifest = JSON.parse(fs.readFileSync(path.join(fixtureRoot, 'manifest.json'), 'utf8'));
// Provider protocol observed on the `oai-events` data channel:
// - `experimental`: deprecated private ChatGPT-brokered protocol (`turn.*`).
// - `public`: public OpenAI Live API (`session.*`). The browser only applies
//   the answer SDP; it never sends `session.start` on the data channel.
const PROTOCOLS = new Set(['experimental', 'public']);
const protocol = parseProtocol(process.argv.slice(2));
const captureEvidence = process.argv.includes('--capture-evidence');
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

async function prepare(command) {
  browser = await chromium.launch({
    headless: true,
    args: ['--autoplay-policy=no-user-gesture-required', '--use-fake-ui-for-media-stream'],
  });
  page = await browser.newPage();
  if (captureEvidence) {
    await page.exposeFunction('__gptLiveEvidence', (record) => {
      if (process.stdout.writableLength > 262144) throw new Error('evidence stdout queue bound');
      process.stdout.write(`${JSON.stringify({ evidence: record })}\n`);
    });
  }
  await page.goto('data:text/html,<title>Meerkat GPT Live E2E peer</title>');
  const fixtures = Object.fromEntries(manifest.fixtures
    .filter((fixture) => fs.existsSync(path.join(fixtureRoot, fixture.file)))
    .map((fixture) => [fixture.name, audioDataUrl(fixture.file)]));
  // Assistant speech detection over the remote track: RMS of the analyser's
  // latest time-domain block, sampled every `window_ms`. Opus-decoded silence
  // sits near 0; speech sits well above 0.01.
  const energyConfig = {
    threshold: typeof command.energy_threshold === 'number' ? command.energy_threshold : 0.005,
    window_ms: 100,
    // The assistant is "quiet" for timeline purposes after this much
    // continuous sub-threshold audio (intra-sentence pauses are shorter).
    end_hysteresis_ms: 600,
    // Input transcript deltas are final once nothing more arrives for this
    // long (the public protocol has no explicit input-final event).
    input_final_quiet_ms: 700,
    // Amplitude below which fixture samples count as silence when locating
    // the end of the spoken part of a fixture (100/32768).
    fixture_silence: 0.0031,
  };
  const offerSdp = await page.evaluate(async ({ fixtures, protocol, captureEvidence, energyConfig }) => {
    const t0 = performance.now();
    const nowMs = () => Math.round(performance.now() - t0);
    const audioContext = new AudioContext({ sampleRate: 24_000 });
    const fixtureBuffers = {};
    const fixtureSpeechMs = {};
    for (const [name, fixture] of Object.entries(fixtures)) {
      const response = await fetch(fixture);
      const buffer = await audioContext.decodeAudioData(await response.arrayBuffer());
      const samples = buffer.getChannelData(0);
      const nonSilentSamples = samples.reduce((count, sample) => count + (Math.abs(sample) >= 0.002 ? 1 : 0), 0);
      if (buffer.duration < 0.1 || nonSilentSamples < buffer.sampleRate * 0.1) {
        throw new Error(`speech fixture ${name} is empty, silent, or too short`);
      }
      let lastSpeech = samples.length - 1;
      while (lastSpeech > 0 && Math.abs(samples[lastSpeech]) < energyConfig.fixture_silence) lastSpeech -= 1;
      fixtureBuffers[name] = buffer;
      fixtureSpeechMs[name] = Math.round((lastSpeech + 1) * 1000 / buffer.sampleRate);
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
    // One shared analyser for the 100 ms energy windows; every remote track
    // feeds it so the timeline sees the assistant regardless of renegotiation.
    const energyAnalyser = audioContext.createAnalyser();
    energyAnalyser.fftSize = 2048;
    const energySamples = new Float32Array(energyAnalyser.fftSize);
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
      source.connect(energyAnalyser);
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
      fixtureSpeechMs,
      // Keep an active zero-PCM source between WAVs. Ending the last source
      // can stall outbound RTP and prevent the provider's context ACK.
      continuousInput: { oscillator, gain, track: destination.stream.getAudioTracks()[0] },
      bargeIn: { armedFixture: null, failures: 0, starts: [] },
      peer,
      remoteAudio,
      evidence: { enabled: captureEvidence, pending: 0, count: 0, failed: false, chain: Promise.resolve(), timer: null },
      // Ordered {t_ms, kind, detail} observations, bounded.
      timeline: [],
      // Soft faults the scenario asserts on ({overlap:{ms}} / {duplicate_readout:{text}}).
      faults: [],
      energy: {
        threshold: energyConfig.threshold,
        window_ms: energyConfig.window_ms,
        windows: [],
        assistant_active: false,
        active_since_ms: null,
        last_active_ms: null,
        overlap_ms: 0,
        first_assistant_audio_ms: [],
        timer: null,
      },
      inputTranscript: { pending: false, last_delta_ms: null, text: '', finals: [] },
      response: { text: '', started_ms: null, index: 0 },
      playing: new Map(),
      scheduled: [],
      nextScheduleId: 1,
      nowMs,
      energyConfig,
      disconnected: null,
    };
    const state = globalThis.__gptLivePeer;
    state.pushTimeline = (kind, detail) => {
      if (state.timeline.length >= 4000) return;
      state.timeline.push({ t_ms: nowMs(), kind, detail: detail ?? {} });
    };
    state.pushFault = (fault) => {
      if (state.faults.length < 256) state.faults.push(fault);
      state.recordEvidence({ kind: 'fault', fault });
    };
    state.captureAudio = async () => {
      const audio = {
        decoded_non_silent_frames: remoteAudio.decodedNonSilentFrames,
        decoded_non_silent_seconds: remoteAudio.decodedNonSilentSeconds,
        non_silent_frames: remoteAudio.nonSilentFrames,
        total_audio_energy: null, total_samples_received: null, total_samples_duration: null,
        bytes_received: 0, packets_received: 0,
      };
      let stats;
      try {
        stats = await peer.getStats();
      } catch {
        return audio;
      }
      for (const report of stats.values()) {
        if (report.type !== 'inbound-rtp' || (report.kind !== 'audio' && report.mediaType !== 'audio')) continue;
        audio.bytes_received += Number(report.bytesReceived || 0);
        audio.packets_received += Number(report.packetsReceived || 0);
        for (const [key, value] of [
          ['total_audio_energy', report.totalAudioEnergy],
          ['total_samples_received', report.totalSamplesReceived],
          ['total_samples_duration', report.totalSamplesDuration],
        ]) {
          if (typeof value === 'number' && Number.isFinite(value)) audio[key] = (audio[key] ?? 0) + value;
        }
      }
      return audio;
    };
    state.recordEvidence = (record) => {
      const evidence = state.evidence;
      if (!evidence.enabled || evidence.failed) return;
      const fault = evidence.pending >= 128 || evidence.count >= 20000
        ? 'queue_limit'
        : typeof record.delta === 'string' && new TextEncoder().encode(record.delta).length > 16384
          ? 'string_limit' : null;
      if (fault) {
        evidence.failed = true;
        evidence.chain = evidence.chain.then(() => globalThis.__gptLiveEvidence({ kind: 'fault', fault }));
        return;
      }
      evidence.pending += 1;
      evidence.count += 1;
      // Sample media immediately at this transcript/timer observation. The
      // ordered evidence chain is never awaited by provider event handling.
      const audio = state.captureAudio();
      evidence.chain = evidence.chain.then(async () => {
        await globalThis.__gptLiveEvidence({ ...record, audio: await audio });
        evidence.pending -= 1;
      }).catch(() => {
        evidence.failed = true;
        return globalThis.__gptLiveEvidence({ kind: 'fault', fault: 'capture_failure' });
      });
    };
    if (captureEvidence) {
      state.evidence.timer = setInterval(() => {
        state.recordEvidence({ kind: 'audio', browser_ms: performance.now() });
      }, 250);
    }
    // ---- fixtures -------------------------------------------------------
    // Every fixture start (direct `play`, armed barge-in, scheduled play)
    // goes through here so the timeline and overlap accounting are uniform.
    state.startFixture = (fixtureName, waitForEnd, meta = {}) => {
      const buffer = state.fixtureBuffers[fixtureName];
      if (!buffer) throw new Error(`unknown audio fixture: ${fixtureName}`);
      const source = state.audioContext.createBufferSource();
      source.buffer = buffer;
      source.connect(state.destination);
      const id = meta.id ?? `play-${state.nextScheduleId++}`;
      const play = {
        id,
        name: fixtureName,
        started_ms: nowMs(),
        speech_ms: state.fixtureSpeechMs[fixtureName] ?? Math.round(buffer.duration * 1000),
        overlap_ms: 0,
        overlap_bound_ms: typeof meta.overlap_bound_ms === 'number' ? meta.overlap_bound_ms : 300,
      };
      state.playing.set(id, play);
      state.pushTimeline('fixture_start', { id, name: fixtureName, duration_ms: Math.round(buffer.duration * 1000), speech_ms: play.speech_ms });
      const ended = new Promise((resolve) => {
        source.onended = () => {
          state.playing.delete(id);
          state.pushTimeline('fixture_end', { id, name: fixtureName, overlap_ms: play.overlap_ms, overlap_bound_ms: play.overlap_bound_ms });
          if (play.overlap_ms > play.overlap_bound_ms) {
            state.pushFault({ overlap: { ms: play.overlap_ms, fixture: fixtureName, bound_ms: play.overlap_bound_ms } });
          }
          if (typeof meta.onEnded === 'function') meta.onEnded(play);
          resolve(play);
        };
      });
      source.start();
      return waitForEnd ? ended : Promise.resolve(play);
    };
    // ---- responses and duplicate readouts ------------------------------
    const normalizeSentence = (text) => text.toLowerCase().replace(/[^a-z0-9' ]+/g, ' ').replace(/\s+/g, ' ').trim();
    state.finishResponse = () => {
      const text = state.response.text;
      if (!text.trim()) return;
      const seen = new Map();
      for (const sentence of text.split(/(?<=[.!?])\s+/)) {
        const normalized = normalizeSentence(sentence);
        if (normalized.split(' ').length < 5) continue;
        seen.set(normalized, (seen.get(normalized) ?? 0) + 1);
      }
      for (const [sentence, count] of seen) {
        if (count >= 2) {
          state.pushFault({ duplicate_readout: { text: sentence.slice(0, 200), response: state.response.index } });
        }
      }
      state.pushTimeline('response_end', { index: state.response.index, chars: text.length });
      state.response = { text: '', started_ms: null, index: state.response.index + 1 };
    };
    state.finalizeInput = () => {
      const input = state.inputTranscript;
      if (!input.pending) return;
      input.pending = false;
      input.finals.push({ t_ms: input.last_delta_ms, text: input.text });
      state.pushTimeline('input_final', { t_ms: input.last_delta_ms, text: input.text.slice(0, 400), index: input.finals.length - 1 });
      input.text = '';
    };
    // ---- scheduling ----------------------------------------------------
    state.armSchedule = (item) => {
      item.state = 'armed';
      item.armed_ms = nowMs();
      item.armed_active = state.energy.assistant_active;
      item.armed_event_count = state.events.length;
      item.armed_input_finals = state.inputTranscript.finals.length;
      state.pushTimeline('scheduled', { id: item.id, name: item.name, anchor: item.anchor, offset_ms: item.offset_ms });
    };
    state.anchorReady = (item, t) => {
      const energy = state.energy;
      switch (item.anchor) {
        case 'now':
          return true;
        case 'first_assistant_audio':
          if (item.allow_active && item.armed_active) return true;
          return energy.first_assistant_audio_ms.some((start) => start >= item.armed_ms);
        case 'assistant_quiet': {
          const spoke = item.armed_active || (energy.last_active_ms !== null && energy.last_active_ms >= item.armed_ms);
          if (item.require_speech && !spoke) return false;
          if (energy.assistant_active) return false;
          const quietSince = energy.last_active_ms ?? item.armed_ms;
          return t - Math.max(quietSince, item.require_speech ? quietSince : item.armed_ms) >= item.quiet_ms;
        }
        case 'input_final':
          return state.inputTranscript.finals.length > item.armed_input_finals;
        case 'event':
          return state.events.slice(item.armed_event_count).some((event) => event?.type === item.event_type);
        default:
          return false;
      }
    };
    state.fireSchedule = (item) => {
      item.state = 'delay';
      item.fired_ms = nowMs();
      state.pushTimeline('anchor_fired', { id: item.id, name: item.name, anchor: item.anchor, offset_ms: item.offset_ms });
      setTimeout(() => {
        if (state.disconnected || state.peer.connectionState !== 'connected') {
          item.state = 'failed';
          state.pushTimeline('fixture_failed', { id: item.id, name: item.name, reason: `connection ${state.peer.connectionState}` });
          return;
        }
        item.state = 'playing';
        try {
          state.startFixture(item.name, false, {
            id: item.id,
            overlap_bound_ms: item.overlap_bound_ms,
            onEnded: () => {
              item.state = 'done';
              if (item.next) state.armSchedule(item.next);
            },
          });
        } catch (error) {
          item.state = 'failed';
          state.pushTimeline('fixture_failed', { id: item.id, name: item.name, reason: String(error?.message || error) });
        }
      }, Math.max(0, item.offset_ms));
    };
    state.newScheduleItem = (spec) => {
      if (!state.fixtureBuffers[spec.name]) throw new Error(`unknown audio fixture: ${spec.name}`);
      const anchor = spec.anchor ?? 'now';
      if (!['now', 'first_assistant_audio', 'assistant_quiet', 'input_final', 'event'].includes(anchor)) {
        throw new Error(`unknown anchor: ${anchor}`);
      }
      if (anchor === 'event' && typeof spec.event_type !== 'string') throw new Error('event anchor requires event_type');
      return {
        id: state.nextScheduleId++,
        name: spec.name,
        anchor,
        event_type: spec.event_type ?? null,
        offset_ms: Number(spec.offset_ms ?? 0),
        quiet_ms: Number(spec.quiet_ms ?? 1200),
        require_speech: spec.require_speech !== false,
        allow_active: spec.allow_active === true,
        overlap_bound_ms: typeof spec.overlap_bound_ms === 'number' ? spec.overlap_bound_ms : 300,
        state: 'waiting',
        armed_ms: null,
        fired_ms: null,
        next: null,
      };
    };
    state.checkScheduled = (t) => {
      for (const item of state.scheduled) {
        if (item.state === 'armed' && state.anchorReady(item, t)) state.fireSchedule(item);
      }
    };
    // ---- energy windows -------------------------------------------------
    state.energy.timer = setInterval(() => {
      const energy = state.energy;
      const t = nowMs();
      energyAnalyser.getFloatTimeDomainData(energySamples);
      let squareSum = 0;
      for (const sample of energySamples) squareSum += sample * sample;
      const rms = Math.sqrt(squareSum / energySamples.length);
      if (energy.windows.length < 60000) energy.windows.push({ t_ms: t, rms: Number(rms.toFixed(5)) });
      if (rms >= energy.threshold) {
        energy.last_active_ms = t;
        if (!energy.assistant_active) {
          energy.assistant_active = true;
          energy.active_since_ms = t;
          energy.first_assistant_audio_ms.push(t);
          if (state.response.started_ms === null) state.response.started_ms = t;
          state.pushTimeline('assistant_audio_start', { response: state.response.index });
        }
        for (const play of state.playing.values()) {
          if (t - play.started_ms <= play.speech_ms) {
            play.overlap_ms += energy.window_ms;
            energy.overlap_ms += energy.window_ms;
          }
        }
      } else if (energy.assistant_active && t - energy.last_active_ms >= energyConfig.end_hysteresis_ms) {
        energy.assistant_active = false;
        state.pushTimeline('assistant_audio_end', { last_active_ms: energy.last_active_ms, started_ms: energy.active_since_ms, response: state.response.index });
      }
      const input = state.inputTranscript;
      if (input.pending && t - input.last_delta_ms >= energyConfig.input_final_quiet_ms) state.finalizeInput();
      state.checkScheduled(t);
    }, energyConfig.window_ms);
    // ---- provider events ------------------------------------------------
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
      const t = nowMs();
      const isInputDelta = protocol === 'public'
        ? parsed?.type === 'session.input_transcript.delta'
        : parsed?.type === 'input_transcript.added';
      const isOutputDelta = protocol === 'public'
        ? parsed?.type === 'session.output_transcript.delta'
        : parsed?.type === 'output_transcript.added';
      if (captureEvidence && (parsed?.type === 'session.input_transcript.delta'
        || parsed?.type === 'session.output_transcript.delta')) {
        const delta = typeof parsed.delta === 'string' ? parsed.delta
          : typeof parsed.text === 'string' ? parsed.text : null;
        if (delta === null && state.evidence.enabled && !state.evidence.failed) {
          state.evidence.failed = true;
          state.evidence.chain = state.evidence.chain.then(() =>
            globalThis.__gptLiveEvidence({ kind: 'fault', fault: 'capture_failure' }));
        } else if (delta !== null) {
          state.recordEvidence({
            kind: 'transcript',
            direction: parsed.type === 'session.input_transcript.delta' ? 'input' : 'output',
            delta,
            event_index: state.events.length - 1,
            browser_ms: performance.now(),
            provider_start_ms: typeof parsed.start_ms === 'number' ? parsed.start_ms : null,
          });
        }
      }
      if (isInputDelta) {
        const delta = typeof parsed.delta === 'string' ? parsed.delta : typeof parsed.text === 'string' ? parsed.text : '';
        const finals = state.inputTranscript.finals;
        const lastFinal = finals[finals.length - 1];
        if (!state.inputTranscript.pending && lastFinal && t - lastFinal.t_ms < 1500 && /^[\s.,!?;:]*$/.test(delta)) {
          // Trailing punctuation the provider finalizes after the answer
          // began belongs to the previous utterance, not a new one.
          lastFinal.text += delta;
        } else {
          // A new user utterance closes the previous assistant response.
          if (state.response.text && !state.inputTranscript.pending) state.finishResponse();
          state.inputTranscript.pending = true;
          state.inputTranscript.last_delta_ms = t;
          if (state.inputTranscript.text.length < 4000) state.inputTranscript.text += delta;
          if (protocol !== 'public') state.finalizeInput();
        }
      }
      // Assistant-start boundary used to fire an armed barge-in. The private
      // protocol announces assistant turns; the public Live API has no turn
      // identifiers, so the first assistant output delta is the boundary.
      const assistantStarted = protocol === 'public'
        ? (parsed?.type === 'session.output_transcript.delta'
          || parsed?.type === 'session.output_audio.delta')
        : (parsed?.type === 'turn.created' && parsed?.turn?.role === 'assistant');
      if (isOutputDelta) {
        const delta = typeof parsed.delta === 'string' ? parsed.delta : typeof parsed.text === 'string' ? parsed.text : '';
        if (state.response.text.length < 20000) state.response.text += delta;
      }
      if (parsed?.type === 'session.delegation.created') {
        state.pushTimeline('delegation_created', { target: parsed?.delegation?.target ?? null, event_index: state.events.length - 1 });
      }
      if (parsed?.type === 'session.commentary.appended') {
        state.pushTimeline('commentary_appended', { event_index: state.events.length - 1 });
      }
      if (assistantStarted && state.inputTranscript.pending) state.finalizeInput();
      if (assistantStarted && state.bargeIn.armedFixture) {
        const fixtureName = state.bargeIn.armedFixture;
        state.bargeIn.armedFixture = null;
        try {
          await state.startFixture(fixtureName, false, { overlap_bound_ms: Number.MAX_SAFE_INTEGER });
          state.bargeIn.starts.push({
            assistant_turn_id: protocol === 'public' ? null : parsed.turn.id,
            assistant_event_type: parsed.type,
            event_count_at_start: state.events.length,
          });
        } catch {
          state.bargeIn.failures += 1;
        }
      }
      state.checkScheduled(t);
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
  }, { fixtures, protocol, captureEvidence, energyConfig });
  return { offer_sdp: offerSdp, protocol, fixtures: Object.keys(fixtures) };
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
      // Direct plays are the caller's own timing decision; never fault them
      // for overlap (S97-S99 play into whatever the assistant is doing).
      await state.startFixture(fixtureName, true, { overlap_bound_ms: Number.MAX_SAFE_INTEGER });
      const bytesSent = (await sentBytes()) - before;
      if (bytesSent <= 0) throw new Error('speech fixture produced no outbound WebRTC audio');
      const buffer = state.fixtureBuffers[fixtureName];
      return { duration_seconds: buffer.duration, samples: buffer.length, bytes_sent: bytesSent };
    },
    name,
  );
  return { played: name, input };
}

async function playAt(command) {
  return page.evaluate((spec) => {
    const state = globalThis.__gptLivePeer;
    const item = state.newScheduleItem(spec);
    state.scheduled.push(item);
    state.armSchedule(item);
    state.checkScheduled(state.nowMs());
    return { scheduled: item.id, anchor: item.anchor, offset_ms: item.offset_ms };
  }, command);
}

async function queue(command) {
  return page.evaluate((specs) => {
    const state = globalThis.__gptLivePeer;
    if (!Array.isArray(specs) || specs.length === 0) throw new Error('queue requires a non-empty items array');
    const items = specs.map((spec) => state.newScheduleItem(spec));
    for (let index = 0; index + 1 < items.length; index += 1) items[index].next = items[index + 1];
    state.scheduled.push(...items);
    state.armSchedule(items[0]);
    state.checkScheduled(state.nowMs());
    return { scheduled: items.map((item) => item.id) };
  }, command.items);
}

async function silence(command) {
  return page.evaluate(async (ms) => {
    const state = globalThis.__gptLivePeer;
    if (!(ms > 0)) throw new Error('silence requires ms > 0');
    if (state.peer.connectionState !== 'connected') throw new Error('silence requires connected WebRTC');
    const frames = Math.round(state.audioContext.sampleRate * ms / 1000);
    const buffer = state.audioContext.createBuffer(1, frames, state.audioContext.sampleRate);
    const source = state.audioContext.createBufferSource();
    source.buffer = buffer;
    source.connect(state.destination);
    const id = `silence-${state.nextScheduleId++}`;
    state.pushTimeline('fixture_start', { id, name: `silence:${ms}`, duration_ms: ms, speech_ms: 0 });
    const ended = new Promise((resolve) => { source.onended = resolve; });
    source.start();
    await ended;
    state.pushTimeline('fixture_end', { id, name: `silence:${ms}`, overlap_ms: 0, overlap_bound_ms: 0 });
    return { played_silence_ms: ms };
  }, Number(command.ms));
}

async function disconnect(command) {
  const mode = command.mode ?? 'graceful';
  if (mode !== 'hard' && mode !== 'graceful') throw new Error(`unknown disconnect mode: ${mode}`);
  return page.evaluate(async (mode) => {
    const state = globalThis.__gptLivePeer;
    if (state.disconnected) throw new Error(`already disconnected (${state.disconnected})`);
    state.disconnected = mode;
    state.finishResponse();
    const before = state.peer.connectionState;
    state.pushTimeline('disconnect', { mode, connection_state_before: before });
    if (mode === 'hard') {
      // Destroy the transport without any goodbye on the data channel or
      // the media track: the host must notice via ICE/DTLS, not a message.
      state.peer.close();
    } else {
      state.continuousInput.track.stop();
      if (state.channel.readyState === 'open' || state.channel.readyState === 'connecting') {
        const closed = new Promise((resolve) => {
          state.channel.addEventListener('close', resolve, { once: true });
          setTimeout(resolve, 2000);
        });
        state.channel.close();
        await closed;
      }
      state.peer.close();
    }
    return { disconnected: mode, connection_state: state.peer.connectionState, data_channel: state.channel.readyState };
  }, mode);
}

async function timeline() {
  return page.evaluate(() => ({ timeline: globalThis.__gptLivePeer.timeline }));
}

async function energy() {
  return page.evaluate(() => {
    const state = globalThis.__gptLivePeer;
    return {
      energy: {
        threshold: state.energy.threshold,
        window_ms: state.energy.window_ms,
        windows: state.energy.windows,
        assistant_active: state.energy.assistant_active,
        overlap_ms: state.energy.overlap_ms,
        first_assistant_audio_ms: state.energy.first_assistant_audio_ms,
      },
      input_finals: state.inputTranscript.finals,
    };
  });
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
    let stats = new Map();
    try {
      stats = await state.peer.getStats();
    } catch {
      // A closed RTCPeerConnection has no stats; report zeros.
    }
    for (const report of stats.values()) {
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
      timeline: state.timeline,
      faults: state.faults,
      energy: {
        threshold: state.energy.threshold,
        window_ms: state.energy.window_ms,
        assistant_active: state.energy.assistant_active,
        overlap_ms: state.energy.overlap_ms,
        first_assistant_audio_ms: state.energy.first_assistant_audio_ms,
        window_count: state.energy.windows.length,
      },
      input_finals: state.inputTranscript.finals,
      scheduled: state.scheduled.map((item) => ({
        id: item.id, name: item.name, anchor: item.anchor, offset_ms: item.offset_ms,
        state: item.state, armed_ms: item.armed_ms, fired_ms: item.fired_ms,
      })),
      disconnected: state.disconnected,
    };
  }).then((snapshot) => ({ ...snapshot, protocol }));
}

async function close() {
  await browser?.close();
  browser = undefined;
  page = undefined;
  return { closed: true };
}

async function stopEvidence() {
  if (!page || !captureEvidence) return { evidence_stopped: true };
  await page.evaluate(async () => {
    const state = globalThis.__gptLivePeer;
    state.finishResponse();
    const evidence = state.evidence;
    evidence.enabled = false;
    clearInterval(evidence.timer);
    await evidence.chain;
  });
  return { evidence_stopped: true };
}

async function handle(command) {
  switch (command.type) {
    case 'prepare': return prepare(command);
    case 'answer': return answer(command.answer_sdp);
    case 'arm_barge_in': return armBargeIn(command.name);
    case 'play': return play(command.name);
    case 'play_at': return playAt(command);
    case 'queue': return queue(command);
    case 'silence': return silence(command);
    case 'disconnect': return disconnect(command);
    case 'timeline': return timeline();
    case 'energy': return energy();
    case 'snapshot': return snapshot();
    case 'stop_evidence': return stopEvidence();
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
