export const LIVE_PCM_WORKLET_PROCESSOR_NAME = 'meerkat-live-pcm';

export interface BrowserLiveAudioSocket {
  readyState: number;
  send?(data: ArrayBuffer | Uint8Array | Blob | string): void;
  close(): void;
}

export interface BrowserLiveAudioContext {
  state?: string;
  destination?: AudioNode;
  audioWorklet?: {
    addModule(moduleURL: string | URL): Promise<void>;
  };
  createMediaStreamSource?(stream: MediaStream): MediaStreamAudioSourceNode;
  close(): Promise<void>;
}

export interface BrowserLiveAudioOptions {
  sampleRate?: number;
  channelCount?: number;
  echoCancellation?: boolean | ConstrainBooleanParameters;
  noiseSuppression?: boolean | ConstrainBooleanParameters;
  autoGainControl?: boolean | ConstrainBooleanParameters;
  mediaDevices?: Pick<MediaDevices, 'getUserMedia'>;
  audioContextFactory?: () => BrowserLiveAudioContext;
  audioWorkletModuleUrl?: string | URL;
  audioWorkletProcessorName?: string;
  onPcmChunk?: (chunk: ArrayBuffer | Uint8Array) => void;
  socket?: BrowserLiveAudioSocket;
  closeSocketOnStop?: boolean;
  monitorInput?: boolean;
}

export interface BrowserLiveAudioConstraints extends MediaStreamConstraints {
  audio: MediaTrackConstraints;
  video: false;
}

export interface BrowserLiveAudioCapture {
  stream: MediaStream;
  audioContext?: BrowserLiveAudioContext;
  socket?: BrowserLiveAudioSocket;
  stop(): Promise<void>;
}

export function liveAudioMediaConstraints(
  options: BrowserLiveAudioOptions = {},
): BrowserLiveAudioConstraints {
  return {
    audio: {
      echoCancellation: options.echoCancellation ?? true,
      noiseSuppression: options.noiseSuppression ?? true,
      autoGainControl: options.autoGainControl ?? true,
      channelCount: options.channelCount ?? 1,
      sampleRate: options.sampleRate ?? 24_000,
    },
    video: false,
  };
}

export async function openBrowserLiveAudioCapture(
  options: BrowserLiveAudioOptions = {},
): Promise<BrowserLiveAudioCapture> {
  const mediaDevices = options.mediaDevices ?? globalThis.navigator?.mediaDevices;
  if (!mediaDevices?.getUserMedia) {
    throw new Error('browser live audio requires navigator.mediaDevices.getUserMedia');
  }

  const stream = await mediaDevices.getUserMedia(liveAudioMediaConstraints(options));
  const audioContext =
    options.audioContextFactory?.() ?? createDefaultAudioContext(options.sampleRate);
  let source: MediaStreamAudioSourceNode | undefined;
  let workletNode: AudioWorkletNode | undefined;

  if (options.audioWorkletModuleUrl) {
    if (!audioContext?.audioWorklet?.addModule || !audioContext.createMediaStreamSource) {
      stopTracks(stream);
      throw new Error('browser live audio PCM conversion requires AudioWorklet support');
    }
    if (typeof globalThis.AudioWorkletNode !== 'function') {
      stopTracks(stream);
      throw new Error('browser live audio PCM conversion requires AudioWorkletNode');
    }

    await audioContext.audioWorklet.addModule(options.audioWorkletModuleUrl);
    source = audioContext.createMediaStreamSource(stream);
    workletNode = new AudioWorkletNode(
      audioContext as unknown as BaseAudioContext,
      options.audioWorkletProcessorName ?? LIVE_PCM_WORKLET_PROCESSOR_NAME,
      {
        processorOptions: {
          sampleRate: options.sampleRate ?? 24_000,
          channelCount: options.channelCount ?? 1,
        },
      },
    );
    workletNode.port.onmessage = (event: MessageEvent<ArrayBuffer | Uint8Array>) => {
      const chunk = event.data;
      options.onPcmChunk?.(chunk);
      if (options.socket?.readyState === 1) {
        options.socket.send?.(chunk);
      }
    };
    source.connect(workletNode);
    if (options.monitorInput && audioContext.destination) {
      workletNode.connect(audioContext.destination);
    }
  }

  return {
    stream,
    audioContext,
    socket: options.socket,
    async stop() {
      disconnectNode(workletNode);
      disconnectNode(source);
      stopTracks(stream);
      if (audioContext && audioContext.state !== 'closed') {
        await audioContext.close();
      }
      if (
        options.closeSocketOnStop &&
        options.socket &&
        (options.socket.readyState === 0 || options.socket.readyState === 1)
      ) {
        options.socket.close();
      }
    },
  };
}

function createDefaultAudioContext(sampleRate = 24_000): BrowserLiveAudioContext | undefined {
  const AudioContextCtor =
    globalThis.AudioContext ??
    (globalThis as typeof globalThis & { webkitAudioContext?: typeof AudioContext })
      .webkitAudioContext;
  return AudioContextCtor ? new AudioContextCtor({ sampleRate }) : undefined;
}

function stopTracks(stream: MediaStream): void {
  for (const track of stream.getTracks()) {
    track.stop();
  }
}

function disconnectNode(node: { disconnect(): void } | undefined): void {
  if (!node) {
    return;
  }
  try {
    node.disconnect();
  } catch {
    // Some browser nodes throw if already disconnected; cleanup remains best-effort.
  }
}
