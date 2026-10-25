// The microphone sender: this browser's microphone, encoded as speech-grade
// Opus and fed to `/ws/mic`, whose far end is a recording device on the remote.
//
// Opening the socket plugs the microphone and closing it unplugs it, the camera
// sender's contract: a session started with the picker's Microphone tick opens
// it as it connects, so the remote has a recording device for the whole
// session, and this browser's microphone is opened only while an application
// over there records — between its `micOpen` and `micClose` — as it is under a
// native client.
//
// What differs from the camera is what the audio is for. The remote's own
// sound and picture aim to be the desktop as it would be in front of you, and
// spend the bandwidth that takes; a redirected microphone is somebody talking,
// wanted now and then, so it is sent as cheaply as speech can be — mono Opus in
// voice mode at a low bitrate, on any link. The gateway decodes it to the PCM
// the remote records in.

import { encodeMicFrame } from "./protocol";

export interface MicSenderCallbacks {
  // The socket closed or the sender failed, and the sender has already
  // stopped: the microphone is released and unplugged. `reason` is non-null
  // for a failure worth showing (no Opus encoder, the target refusing the
  // socket) and null for an ordinary close.
  onStopped: (reason: string | null) => void;
  // The remote started or stopped recording, which is also when this
  // browser's microphone opens and closes.
  onStreaming: (streaming: boolean) => void;
  // Why this browser's microphone could not be opened or stopped giving audio
  // while the remote records, or null once it opens. The device stays
  // plugged: the next time an application on the remote records, opening is
  // tried again, so a permission granted in the meantime takes effect without
  // a new session.
  onCaptureError: (reason: string | null) => void;
}

export interface MicSender {
  stop: () => void;
}

// Speech, and no more than speech needs: wideband voice at 16 kbit/s.
export const MIC_BITRATE = 16_000;
// 60 ms packets. Every packet costs a WebSocket frame's header on top of the
// Opus, which at 20 ms would be a fifth of this bitrate again; a microphone
// can spare the extra 40 ms.
export const MIC_FRAME_MICROSECONDS = 60_000;

// Bytes allowed unsent before packets are dropped instead of queued: a
// microphone behind the speaker is worse than one missing a moment, and an
// Opus decoder takes a lost packet in its stride. About eight seconds of this
// bitrate — only a link that has stopped reaches it.
const MAX_BUFFERED_BYTES = 16 * 1024;

// Captured buffers allowed waiting in the encoder before new ones are dropped.
const MAX_ENCODE_QUEUE = 4;

// How long this browser's microphone stays open after the remote stops
// recording, the camera's linger: an application commonly closes and reopens
// its recording device as it settles, and reopening a microphone costs a
// moment and the browser's speech processing a fresh start each time.
const CAPTURE_LINGER_MS = 2_000;

// The encoder configuration for audio captured at `sampleRate`: always mono,
// in Opus's voice mode.
export function opusConfig(sampleRate: number): AudioEncoderConfig {
  return {
    codec: "opus",
    sampleRate,
    numberOfChannels: 1,
    bitrate: MIC_BITRATE,
    bitrateMode: "variable",
    opus: {
      application: "voip",
      signal: "voice",
      frameDuration: MIC_FRAME_MICROSECONDS,
    },
  };
}

// Average planar channels into one.
export function downmix(planes: Float32Array[]): Float32Array<ArrayBuffer> {
  const frames = planes[0]?.length ?? 0;
  const mono = new Float32Array(frames);
  for (const plane of planes) {
    for (let i = 0; i < frames; i += 1) {
      mono[i] += plane[i] / planes.length;
    }
  }
  return mono;
}

// The mono AudioData to encode: the captured one when it already is, and a
// downmix of it otherwise. The caller closes what it passed in.
function monoOf(data: AudioData): AudioData {
  if (data.numberOfChannels === 1) {
    return data;
  }
  const planes: Float32Array[] = [];
  for (let plane = 0; plane < data.numberOfChannels; plane += 1) {
    const samples = new Float32Array(data.numberOfFrames);
    data.copyTo(samples, { planeIndex: plane, format: "f32-planar" });
    planes.push(samples);
  }
  return new AudioData({
    format: "f32-planar",
    sampleRate: data.sampleRate,
    numberOfFrames: data.numberOfFrames,
    numberOfChannels: 1,
    timestamp: data.timestamp,
    data: downmix(planes),
  });
}

// What a refused `getUserMedia` means, in the words the session card shows.
function captureRefusal(e: unknown): string {
  const name = e instanceof DOMException ? e.name : "";
  switch (name) {
    case "NotAllowedError":
      return "this browser was not allowed to use the microphone; allow it in the site's settings";
    case "NotFoundError":
    case "OverconstrainedError":
      return "this browser has no microphone to offer";
    case "NotReadableError":
      return "the microphone could not be opened; another application may be using it";
    default:
      return e instanceof Error && e.message
        ? e.message
        : "the microphone could not be opened";
  }
}

// This browser's microphone, or a refusal worded for the session card.
//
// The browser's own speech processing, all of it: a remote call hears this
// microphone through the remote's speakers and this browser's, so echo
// cancellation matters as much as on any call.
async function requestMicrophone(): Promise<MediaStreamTrack> {
  let stream: MediaStream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        channelCount: { ideal: 1 },
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      },
    });
  } catch (e) {
    throw new Error(captureRefusal(e));
  }
  const track = stream.getAudioTracks()[0];
  if (!track) {
    for (const t of stream.getTracks()) {
      t.stop();
    }
    throw new Error("the microphone produced no audio track");
  }
  return track;
}

// Check and connect. The returned sender is live until its socket closes
// (server side) or `stop` is called (this side); both end in exactly one
// `onStopped`. Nothing here opens a microphone: the remote's `micOpen` does.
export async function startMicSender(
  url: string,
  callbacks: MicSenderCallbacks,
): Promise<MicSender> {
  if (typeof AudioEncoder === "undefined") {
    throw new Error("this browser has no AudioEncoder");
  }
  if (typeof MediaStreamTrackProcessor === "undefined") {
    throw new Error(
      "this browser cannot read microphone audio (no MediaStreamTrackProcessor)",
    );
  }
  if (!navigator.mediaDevices?.getUserMedia) {
    throw new Error("this browser offers no microphone capture");
  }
  // Checked at 48 kHz, the rate a browser's microphone almost always runs at,
  // since the real one is known only once a microphone is open. The encoder
  // is configured for whatever rate the audio then arrives at.
  const support = await AudioEncoder.isConfigSupported(
    opusConfig(48_000),
  ).catch(() => ({ supported: false }));
  if (!support.supported) {
    throw new Error("this browser cannot encode Opus from the microphone");
  }

  let stopped = false;
  let streaming = false;
  // The rate the encoder is configured for, which follows the audio.
  let configuredRate = 0;
  // This browser's microphone while it is open, and whether an open is
  // waiting on `getUserMedia` (its permission prompt, the first time).
  let capture: {
    track: MediaStreamTrack;
    reader: ReadableStreamDefaultReader<AudioData>;
  } | null = null;
  let opening = false;
  let lingerTimer: ReturnType<typeof setTimeout> | undefined;

  const socket = new WebSocket(url);
  socket.binaryType = "arraybuffer";

  const encoder = new AudioEncoder({
    output: (chunk) => {
      if (
        stopped ||
        !streaming ||
        socket.readyState !== WebSocket.OPEN ||
        socket.bufferedAmount > MAX_BUFFERED_BYTES
      ) {
        return;
      }
      const packet = new Uint8Array(chunk.byteLength);
      chunk.copyTo(packet);
      const frame = encodeMicFrame(packet);
      if (frame) {
        socket.send(frame);
      }
    },
    error: (e) => stop(e.message || "the Opus encoder failed"),
  });

  const closeCapture = () => {
    clearTimeout(lingerTimer);
    if (!capture) {
      return;
    }
    void capture.reader.cancel().catch(() => {});
    capture.track.stop();
    capture = null;
  };

  const stop = (reason: string | null = null) => {
    if (stopped) {
      return;
    }
    stopped = true;
    closeCapture();
    if (encoder.state !== "closed") {
      encoder.close();
    }
    if (
      socket.readyState === WebSocket.OPEN ||
      socket.readyState === WebSocket.CONNECTING
    ) {
      socket.close();
    }
    callbacks.onStopped(reason);
  };

  const encode = (data: AudioData) => {
    if (!streaming || encoder.state === "closed") {
      return;
    }
    if (data.sampleRate !== configuredRate) {
      if (encoder.state === "configured") {
        encoder.reset();
      }
      encoder.configure(opusConfig(data.sampleRate));
      configuredRate = data.sampleRate;
    }
    // An encoder falling behind the microphone drops audio rather than lagging it.
    if (encoder.encodeQueueSize > MAX_ENCODE_QUEUE) {
      return;
    }
    const mono = monoOf(data);
    encoder.encode(mono);
    if (mono !== data) {
      mono.close();
    }
  };

  // One captured buffer's fate, closed either way.
  const take = (data: AudioData) => {
    try {
      encode(data);
    } catch (e) {
      stop(
        e instanceof Error ? e.message : "the microphone could not be encoded",
      );
    } finally {
      data.close();
    }
  };

  // The capture behind `reader` ended by itself — the device removed, the
  // permission revoked — rather than by closeCapture(), which is said, and
  // leaves the microphone plugged for the remote's next recording.
  const captureEnded = (
    reader: ReadableStreamDefaultReader<AudioData>,
    reason: string,
  ) => {
    if (stopped || capture?.reader !== reader) {
      return;
    }
    closeCapture();
    callbacks.onCaptureError(reason);
  };

  // The capture pump for one opening of the microphone, ending with it.
  const pump = async (reader: ReadableStreamDefaultReader<AudioData>) => {
    for (;;) {
      let result: ReadableStreamReadResult<AudioData>;
      try {
        result = await reader.read();
      } catch (e) {
        captureEnded(
          reader,
          e instanceof Error ? e.message : "the microphone capture failed",
        );
        break;
      }
      if (result.done || stopped) {
        result.value?.close();
        captureEnded(reader, "the microphone stopped");
        break;
      }
      take(result.value);
    }
  };

  // Open this browser's microphone for a remote that has started recording.
  // The first time on a page this is the permission prompt, which comes up
  // when the application on the remote starts recording.
  const openCapture = async () => {
    if (capture || opening) {
      return;
    }
    opening = true;
    let track: MediaStreamTrack;
    try {
      track = await requestMicrophone();
    } catch (e) {
      opening = false;
      if (!stopped) {
        callbacks.onCaptureError(
          e instanceof Error ? e.message : "the microphone could not be opened",
        );
      }
      return;
    }
    opening = false;
    // Stopped, or no longer wanted while the prompt was up: put it straight
    // down. The remote recording again opens it afresh.
    if (stopped || !streaming) {
      track.stop();
      return;
    }
    callbacks.onCaptureError(null);
    const reader = new MediaStreamTrackProcessor<AudioData>({
      track,
    }).readable.getReader();
    capture = { track, reader };
    void pump(reader);
  };

  socket.onmessage = (ev) => {
    if (typeof ev.data !== "string" || stopped) {
      return;
    }
    let msg: { type?: string };
    try {
      msg = JSON.parse(ev.data) as { type?: string };
    } catch {
      return;
    }
    switch (msg.type) {
      case "micOpen":
        streaming = true;
        clearTimeout(lingerTimer);
        callbacks.onStreaming(true);
        void openCapture();
        break;
      case "micClose":
        streaming = false;
        // Packets still in the encoder belong to the recording that ended.
        if (encoder.state === "configured") {
          encoder.reset();
          configuredRate = 0;
        }
        callbacks.onStreaming(false);
        clearTimeout(lingerTimer);
        lingerTimer = setTimeout(closeCapture, CAPTURE_LINGER_MS);
        break;
    }
  };

  socket.onclose = (ev) => {
    // 4002 is the gateway saying the target carries no microphone.
    stop(ev.code === 4002 ? "this target carries no microphone" : null);
  };

  return { stop: () => stop(null) };
}
