// The camera sender: this browser's webcam, encoded to H.264 and fed to
// `/ws/camera`, whose far end is a virtual camera plugged into the remote.
//
// Opening the socket plugs the camera and closing it unplugs it — the same
// contract as audio's socket, with the media going the other way. A session
// started with the picker's Camera tick opens it as it connects, so the remote
// has a camera for the whole session, but this browser's camera is opened only
// while the remote consumes it: between its `cameraStart` and `cameraStop`,
// which an application over there opening and closing the camera sends. The
// camera light is on exactly when something on the remote is using it, as it
// is under a native client.
//
// The device is announced before any camera is opened, so it is announced in
// one fixed geometry and rate, and every frame is fitted to that geometry. The
// gateway never transcodes: what `VideoEncoder` emits here is what the remote
// decodes, which is why the encoder is configured for Annex B H.264 (the
// bitstream MS-RDPECAM carries) and why a browser that cannot encode it gets a
// named error instead of a fallback. A `cameraKeyframe` makes the next frame an
// IDR so a stream the gateway had to drop can resume.

import { type ClientMsg, encodeCameraFrame } from "./protocol";

export interface CameraSenderCallbacks {
  // The socket closed or the sender failed, and the sender has already stopped:
  // the camera is released and unplugged. `reason` is non-null for a failure
  // worth showing (no H.264 encoder, the target refusing the socket) and null
  // for an ordinary close.
  onStopped: (reason: string | null) => void;
  // The remote started or stopped consuming — an application over there opened
  // or closed the camera, which is also when this browser's camera opens and
  // closes.
  onStreaming: (streaming: boolean) => void;
  // Why this browser's camera could not be opened for the remote, or null once
  // it opens. The device stays plugged: the next time an application on the
  // remote opens it, opening is tried again, so a permission granted in the
  // meantime takes effect without a new session.
  onCaptureError: (reason: string | null) => void;
}

export interface CameraSender {
  stop: () => void;
}

// Frames allowed inside the encoder before new ones are skipped instead of
// queued. Skipping *input* is safe anywhere — an unencoded frame constrains no
// GOP — so this is free latency protection against an encoder slower than the
// camera.
const MAX_ENCODE_QUEUE = 2;

// The one format the remote is offered, announced before any camera is open:
// 16:9, which most webcams are, so their picture is scaled rather than cropped.
// Low on purpose — a redirected camera is a video call's picture, and a host
// without a GPU was measured asking for samples below 30 frames a second, so a
// faster camera only fills the gateway's queue until it drops a second's worth
// to the next keyframe.
export const CAMERA_WIDTH = 640;
export const CAMERA_HEIGHT = 360;
export const CAMERA_FPS = 15;

// How long this browser's camera stays open after the remote stops consuming.
// An application opening a camera commonly starts, stops and starts it again
// while it settles on a format, and reopening a webcam costs a moment each
// time; a camera nothing asks for again within this is closed.
const CAPTURE_LINGER_MS = 2_000;

// The part of a `sourceWidth`×`sourceHeight` frame shown in the announced
// geometry: all of it when the aspect ratios agree, and otherwise the centre,
// cropped on the long side, so the picture fills the frame undistorted. Split
// out pure so a unit test can pin it without a camera.
export function centreCrop(
  sourceWidth: number,
  sourceHeight: number,
  width: number,
  height: number,
): { x: number; y: number; width: number; height: number } {
  if (sourceWidth * height > width * sourceHeight) {
    const cropped = Math.round((sourceHeight * width) / height);
    return {
      x: Math.floor((sourceWidth - cropped) / 2),
      y: 0,
      width: cropped,
      height: sourceHeight,
    };
  }
  const cropped = Math.round((sourceWidth * height) / width);
  return {
    x: 0,
    y: Math.floor((sourceHeight - cropped) / 2),
    width: sourceWidth,
    height: cropped,
  };
}

// Bytes allowed to sit unsent in the socket before samples are dropped instead
// of queued: `send` itself queues without limit, so a slow uplink would
// otherwise become unbounded memory and a picture ever further behind the
// camera. Unlike the encoder queue above, what this drops is *output*, which
// is never free in H.264 — everything until the next keyframe goes with the
// first dropped delta, and that keyframe is asked for once the backlog clears,
// the same drop-whole-and-rekey the gateway's own credit queue applies. A
// quarter of a megabyte is a quarter second at the ceiling bitrate `h264Config`
// allows.
const MAX_BUFFERED_BYTES = 256 * 1024;

// The H.264 configuration for one capture geometry. Split out pure so a unit
// test can pin the level and bitrate choices without a camera.
//
// Constrained Baseline, because the far decoder is unknowable from here — it is
// whatever camera stack the remote's application brings — and Constrained
// Baseline is the profile everything decodes. The level is the smallest from
// 3.1 up whose frame size and macroblock rate both fit the geometry, so the
// level byte stays honest for a 4K camera. Bitrate at
// 0.1 bits per pixel per frame — the usual realtime-video rule of thumb —
// clamped to a floor a tiny capture still looks fine at and a ceiling a 4K one
// cannot flood the uplink with.
export function h264Config(
  width: number,
  height: number,
  fps: number,
): { codec: string; bitrate: number } {
  const macroblocks = Math.ceil(width / 16) * Math.ceil(height / 16);
  const mbRate = macroblocks * fps;
  // Level limits from the H.264 spec's table A-1: [level_idc, MaxFS in
  // macroblocks, MaxMBPS]. Past 5.2, the largest is named and the encoder's
  // support check refuses it.
  const levels: [string, number, number][] = [
    ["1f", 3_600, 108_000], // 3.1
    ["20", 5_120, 216_000], // 3.2
    ["28", 8_192, 245_760], // 4.0
    ["2a", 8_704, 522_240], // 4.2
    ["32", 22_080, 589_824], // 5.0
    ["33", 36_864, 983_040], // 5.1
    ["34", 36_864, 2_073_600], // 5.2
  ];
  const level = (levels.find(
    ([, maxFs, maxMbps]) => macroblocks <= maxFs && mbRate <= maxMbps,
  ) ?? levels[levels.length - 1])[0];
  const bitrate = Math.min(
    8_000_000,
    Math.max(300_000, width * height * fps * 0.1),
  );
  return { codec: `avc1.42e0${level}`, bitrate: Math.round(bitrate) };
}

// Chrome currently accepts an `avc1.42e0..` configuration but can emit a
// Baseline SPS whose profile-compatibility byte is zero. DirectShow decodes
// that stream, while Windows' MediaCapture camera source waits forever after
// its first sample. The encoder is already producing Baseline syntax; make the
// SPS advertise the Constrained Baseline contract we requested. Annex B start
// codes cannot occur unescaped inside a NAL, so finding both three- and
// four-byte forms this way cannot mistake encoded payload for an SPS.
export function normalizeConstrainedBaseline(unit: Uint8Array): void {
  for (let index = 0; index + 3 < unit.length; index += 1) {
    let header = -1;
    if (unit[index] === 0 && unit[index + 1] === 0 && unit[index + 2] === 1) {
      header = index + 3;
    } else if (
      unit[index] === 0 &&
      unit[index + 1] === 0 &&
      unit[index + 2] === 0 &&
      unit[index + 3] === 1
    ) {
      header = index + 4;
    }
    if (
      header >= 0 &&
      header + 2 < unit.length &&
      (unit[header] & 0x1f) === 7 &&
      unit[header + 1] === 0x42
    ) {
      // constraint_set0/1/2, matching the `42e0` codec string from
      // `h264Config`; keep any remaining compatibility bits intact.
      unit[header + 2] |= 0xe0;
    }
  }
}

// What a refused `getUserMedia` means, in the words the session card shows.
function captureRefusal(e: unknown): string {
  const name = e instanceof DOMException ? e.name : "";
  switch (name) {
    case "NotAllowedError":
      return "this browser was not allowed to use the camera; allow it in the site's settings";
    case "NotFoundError":
    case "OverconstrainedError":
      return "this browser has no camera to offer";
    case "NotReadableError":
      return "the camera could not be opened; another application may be using it";
    default:
      return e instanceof Error && e.message
        ? e.message
        : "the camera could not be opened";
  }
}

// This browser's camera, in the announced geometry and rate where it has them,
// or a refusal worded for the session card.
//
// A facing preference, not a bare `video: true`: bare leaves the browser its
// device-order default, which on a Mac with an auxiliary capture device — Desk
// View, a virtual camera — is a camera the user is not in front of. "user"
// names the one pointed at the person, which is what a redirected webcam
// means. Ideal rather than `exact`, so a webcam that reports no facing mode at
// all — most external ones — is still eligible rather than refused. The
// geometry and rate are asked for as `ideal`/`max` too, so a camera without
// that exact mode still opens at its nearest, and is fitted.
async function requestCamera(): Promise<MediaStreamTrack> {
  let stream: MediaStream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({
      video: {
        facingMode: "user",
        width: { ideal: CAMERA_WIDTH },
        height: { ideal: CAMERA_HEIGHT },
        frameRate: { ideal: CAMERA_FPS, max: CAMERA_FPS },
      },
    });
  } catch (e) {
    throw new Error(captureRefusal(e));
  }
  const track = stream.getVideoTracks()[0];
  if (!track) {
    for (const t of stream.getTracks()) {
      t.stop();
    }
    throw new Error("the camera produced no video track");
  }
  return track;
}

// Check, configure, connect. The returned sender is live until its socket
// closes (server side) or `stop` is called (this side); both end in exactly one
// `onStopped`. Nothing here opens a camera: the remote's `cameraStart` does.
export async function startCameraSender(
  url: string,
  callbacks: CameraSenderCallbacks,
): Promise<CameraSender> {
  // Named refusals before anything is opened. The encoder and the track
  // processor are checked here, where a browser without them costs one feature
  // instead of the page, as a missing decoder costs the pictures that need it
  // (videoCodecs.ts).
  if (typeof VideoEncoder === "undefined") {
    throw new Error("this browser has no VideoEncoder");
  }
  if (typeof MediaStreamTrackProcessor === "undefined") {
    throw new Error(
      "this browser cannot read camera frames (no MediaStreamTrackProcessor)",
    );
  }
  if (!navigator.mediaDevices?.getUserMedia) {
    throw new Error("this browser offers no camera capture");
  }

  const width = CAMERA_WIDTH;
  const height = CAMERA_HEIGHT;
  const fps = CAMERA_FPS;
  // Frames closer together than the announced rate are skipped before the
  // encoder, for a camera that ignored the `max` asked of it.
  const frameInterval = 1_000_000 / fps;
  const { codec, bitrate } = h264Config(width, height, fps);
  const config: VideoEncoderConfig = {
    codec,
    width,
    height,
    bitrate,
    framerate: fps,
    latencyMode: "realtime",
    // Annex B, because that is the H.264 MS-RDPECAM carries: parameter sets
    // travel inside each keyframe, so the wire needs no side channel for them.
    avc: { format: "annexb" },
  };
  const support = await VideoEncoder.isConfigSupported(config);
  if (!support.supported) {
    throw new Error(
      `this browser cannot encode ${codec} at ${width}x${height}`,
    );
  }

  let stopped = false;
  let streaming = false;
  let forceKeyframe = true;
  let lastEncoded = Number.NEGATIVE_INFINITY;
  // Whether the socket backed up past MAX_BUFFERED_BYTES and deltas are being
  // dropped. Set by the encoder's output, cleared by the keyframe that resumes
  // the stream; the pump requests that keyframe when the backlog has cleared.
  let droppingDeltas = false;
  // This browser's camera while it is open, and whether an open is waiting on
  // `getUserMedia` (its permission prompt, the first time).
  let capture: {
    track: MediaStreamTrack;
    reader: ReadableStreamDefaultReader<VideoFrame>;
  } | null = null;
  let opening = false;
  let lingerTimer: ReturnType<typeof setTimeout> | undefined;
  // Where a frame whose size is not the announced one is fitted to it.
  let fitCanvas: OffscreenCanvas | null = null;

  const socket = new WebSocket(url);
  socket.binaryType = "arraybuffer";

  const encoder = new VideoEncoder({
    output: (chunk) => {
      if (stopped || socket.readyState !== WebSocket.OPEN) {
        return;
      }
      const key = chunk.type === "key";
      // Backpressure. A delta after a dropped delta is undecodable, so the
      // first drop commits to dropping every delta until the next keyframe.
      // Keyframes always pass: one is what lets the stream resume.
      if (
        !key &&
        (droppingDeltas || socket.bufferedAmount > MAX_BUFFERED_BYTES)
      ) {
        droppingDeltas = true;
        return;
      }
      if (key) {
        droppingDeltas = false;
      }
      const unit = new Uint8Array(chunk.byteLength);
      chunk.copyTo(unit);
      normalizeConstrainedBaseline(unit);
      const frame = encodeCameraFrame(unit, key);
      if (frame) {
        socket.send(frame);
      }
    },
    error: (e) => stop(e.message || "the H.264 encoder failed"),
  });
  encoder.configure(config);

  const closeCapture = () => {
    clearTimeout(lingerTimer);
    if (!capture) {
      return;
    }
    void capture.reader.cancel().catch(() => {});
    capture.track.stop();
    capture = null;
  };

  // One stop for every path out, idempotent: the socket's close handler and an
  // explicit `stop` can both fire, and the second must find nothing to do.
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

  // The announced geometry, from whatever the camera gives: as it is when it
  // already matches, and otherwise its centre scaled into a canvas, which the
  // frame is rebuilt from. Null when the canvas cannot be drawn on.
  const fitted = (frame: VideoFrame): VideoFrame | null => {
    if (frame.displayWidth === width && frame.displayHeight === height) {
      return frame;
    }
    fitCanvas ??= new OffscreenCanvas(width, height);
    const context = fitCanvas.getContext("2d");
    if (!context) {
      return null;
    }
    const crop = centreCrop(
      frame.displayWidth,
      frame.displayHeight,
      width,
      height,
    );
    context.drawImage(
      frame,
      crop.x,
      crop.y,
      crop.width,
      crop.height,
      0,
      0,
      width,
      height,
    );
    return new VideoFrame(fitCanvas, { timestamp: frame.timestamp });
  };

  // One frame's fate: skipped while the remote is not consuming or the encoder
  // is behind — an unencoded frame constrains no GOP — and otherwise encoded,
  // at an IDR when one is owed. The caller closes the frame either way.
  const encodeFrame = (frame: VideoFrame) => {
    if (!streaming || encoder.encodeQueueSize > MAX_ENCODE_QUEUE) {
      return;
    }
    // Timestamps are microseconds. A frame early by less than a millisecond is
    // the camera's jitter, not a faster rate.
    if (frame.timestamp - lastEncoded < frameInterval - 1000) {
      return;
    }
    const sized = fitted(frame);
    if (!sized) {
      return;
    }
    lastEncoded = frame.timestamp;
    if (droppingDeltas && socket.bufferedAmount <= MAX_BUFFERED_BYTES) {
      // The backlog cleared: resume at the IDR the far decoder needs.
      forceKeyframe = true;
    }
    encoder.encode(sized, { keyFrame: forceKeyframe });
    forceKeyframe = false;
    if (sized !== frame) {
      sized.close();
    }
  };

  // The frame pump for one opening of the camera, ending with it.
  const pump = async (reader: ReadableStreamDefaultReader<VideoFrame>) => {
    for (;;) {
      let result: ReadableStreamReadResult<VideoFrame>;
      try {
        result = await reader.read();
      } catch {
        break; // cancelled by closeCapture()
      }
      if (result.done || stopped) {
        result.value?.close();
        break;
      }
      encodeFrame(result.value);
      result.value.close();
    }
  };

  // Open this browser's camera for a remote that has started consuming. The
  // first time on a page this is the permission prompt, which comes up when the
  // application on the remote opens the camera.
  const openCapture = async () => {
    if (capture || opening) {
      return;
    }
    opening = true;
    let track: MediaStreamTrack;
    try {
      track = await requestCamera();
    } catch (e) {
      opening = false;
      if (!stopped) {
        callbacks.onCaptureError(
          e instanceof Error ? e.message : "the camera could not be opened",
        );
      }
      return;
    }
    opening = false;
    // Stopped, or no longer wanted while the prompt was up: put it straight
    // down. The remote starting again opens it afresh.
    if (stopped || !streaming) {
      track.stop();
      return;
    }
    callbacks.onCaptureError(null);
    const reader = new MediaStreamTrackProcessor({
      track,
    }).readable.getReader();
    capture = { track, reader };
    lastEncoded = Number.NEGATIVE_INFINITY;
    void pump(reader);
  };

  socket.onopen = () => {
    const format: ClientMsg = {
      type: "cameraFormat",
      width,
      height,
      fpsNumerator: fps,
      fpsDenominator: 1,
    };
    socket.send(JSON.stringify(format));
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
      case "cameraStart":
        streaming = true;
        // Every (re)start opens on an IDR: the remote's decoder starts from
        // parameter sets, and Annex B keyframes carry them.
        forceKeyframe = true;
        clearTimeout(lingerTimer);
        callbacks.onStreaming(true);
        void openCapture();
        break;
      case "cameraStop":
        streaming = false;
        callbacks.onStreaming(false);
        clearTimeout(lingerTimer);
        lingerTimer = setTimeout(closeCapture, CAPTURE_LINGER_MS);
        break;
      case "cameraKeyframe":
        forceKeyframe = true;
        break;
    }
  };

  socket.onclose = (ev) => {
    // 4002 is the gateway saying the target carries no camera (or the engine
    // is gone); everything else is an ordinary end of the session's camera.
    stop(ev.code === 4002 ? "this target carries no camera" : null);
  };

  return { stop: () => stop(null) };
}
