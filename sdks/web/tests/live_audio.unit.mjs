import assert from 'node:assert/strict';
import test from 'node:test';

import {
  liveAudioMediaConstraints,
  openBrowserLiveAudioCapture,
} from '../dist/index.js';

test('liveAudioMediaConstraints requests browser AEC by default', () => {
  const constraints = liveAudioMediaConstraints();

  assert.equal(constraints.video, false);
  assert.equal(constraints.audio.echoCancellation, true);
  assert.equal(constraints.audio.noiseSuppression, true);
  assert.equal(constraints.audio.autoGainControl, true);
  assert.equal(constraints.audio.channelCount, 1);
  assert.equal(constraints.audio.sampleRate, 24000);
});

test('openBrowserLiveAudioCapture stops tracks, closes audio context, and closes socket', async () => {
  const stopped = [];
  const stream = {
    getTracks() {
      return [
        {
          stop() {
            stopped.push('track');
          },
        },
      ];
    },
  };
  const mediaDevices = {
    async getUserMedia(constraints) {
      assert.equal(constraints.audio.echoCancellation, true);
      return stream;
    },
  };
  const closed = [];
  const audioContext = {
    state: 'running',
    async close() {
      closed.push('context');
      this.state = 'closed';
    },
  };
  const socket = {
    readyState: 1,
    close() {
      closed.push('socket');
      this.readyState = 3;
    },
  };

  const capture = await openBrowserLiveAudioCapture({
    mediaDevices,
    audioContextFactory: () => audioContext,
    socket,
    closeSocketOnStop: true,
  });
  await capture.stop();

  assert.deepEqual(stopped, ['track']);
  assert.deepEqual(closed, ['context', 'socket']);
});
